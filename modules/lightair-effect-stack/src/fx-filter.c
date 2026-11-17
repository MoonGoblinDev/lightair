/*
 * Lightair FX (filter id lightair_fx): data-driven GPU effects (docs/plans/effects-revamp.md).
 *
 * One filter type, one shader per effect in data/fx/<name>.effect, chosen by the "fx" setting.
 * Every uniform an effect declares is also a setting of the same name, so adding an effect is
 * adding a file: floats and ints read the number (a switch reads 0 or 1), float3/float4 read an
 * OBS colour (0xAABBGGRR). Missing settings take the uniform's default from the file.
 *
 * The filter fills in these uniforms itself: image (the input, or the previous pass), source
 * (the input, in multi-pass effects), size (px), texel (1 / size), time (seconds).
 *
 * An effect is one technique "Draw", or passes "Pass0".."Pass3" run in order, each reading the
 * last one's output as image.
 */

#include <obs-module.h>
#include <graphics/vec2.h>
#include <graphics/vec3.h>
#include <graphics/vec4.h>
#include <util/platform.h>
#include <util/dstr.h>
#include <string.h>

#define MAX_PASSES 4
#define MAX_EFFECTS 64

/* Shaders are shared by every filter and live until libobs's graphics shut down (gs_effect_create
 * caches them by path). Touched only inside the graphics context. */
static struct {
	char name[48];
	gs_effect_t *effect; /* NULL: failed to load */
} loaded[MAX_EFFECTS];
static int loaded_n;

static gs_effect_t *fx_effect(const char *name)
{
	for (int i = 0; i < loaded_n; i++)
		if (strcmp(loaded[i].name, name) == 0)
			return loaded[i].effect;
	if (loaded_n == MAX_EFFECTS)
		return NULL;
	struct dstr rel = {0};
	dstr_printf(&rel, "fx/%s.effect", name);
	char *common_path = obs_module_file("fx/common.inc");
	char *path = obs_module_file(rel.array);
	char *common = common_path ? os_quick_read_utf8_file(common_path) : NULL;
	char *body = path ? os_quick_read_utf8_file(path) : NULL;
	gs_effect_t *e = NULL;
	if (common && body) {
		struct dstr src = {0};
		dstr_copy(&src, common);
		dstr_cat(&src, "\n");
		dstr_cat(&src, body);
		char *err = NULL;
		e = gs_effect_create(src.array, path, &err);
		if (!e)
			blog(LOG_ERROR, "[lightair-fx] %s: %s", name, err ? err : "(unknown error)");
		bfree(err);
		dstr_free(&src);
	} else {
		blog(LOG_WARNING, "[lightair-fx] no effect named %s", name);
	}
	snprintf(loaded[loaded_n].name, sizeof(loaded[loaded_n].name), "%s", name);
	loaded[loaded_n++].effect = e;
	bfree(common);
	bfree(body);
	bfree(common_path);
	bfree(path);
	dstr_free(&rel);
	return e;
}

struct fx_filter {
	obs_source_t *context;
	obs_data_t *settings; /* our reference to the latest settings */
	char name[48];
	gs_effect_t *effect;
	int passes; /* 0: single "Draw" technique */
	gs_texrender_t *tr[3];
	float time;
};

static const char *fx_get_name(void *unused)
{
	UNUSED_PARAMETER(unused);
	return obs_module_text("Lightair FX");
}

static void fx_update(void *data, obs_data_t *settings)
{
	struct fx_filter *f = data;
	obs_data_addref(settings);
	obs_data_release(f->settings);
	f->settings = settings;
	const char *name = obs_data_get_string(settings, "fx");
	if (strcmp(name, f->name) != 0) {
		snprintf(f->name, sizeof(f->name), "%s", name);
		obs_enter_graphics();
		f->effect = name[0] ? fx_effect(name) : NULL;
		f->passes = 0;
		if (f->effect && !gs_effect_get_technique(f->effect, "Draw")) {
			char tech[8];
			for (int i = 0; i < MAX_PASSES; i++) {
				snprintf(tech, sizeof(tech), "Pass%d", i);
				if (!gs_effect_get_technique(f->effect, tech))
					break;
				f->passes = i + 1;
			}
			if (!f->passes)
				f->effect = NULL;
		}
		obs_leave_graphics();
	}
}

static void *fx_create(obs_data_t *settings, obs_source_t *context)
{
	struct fx_filter *f = bzalloc(sizeof(*f));
	f->context = context;
	obs_enter_graphics();
	for (int i = 0; i < 3; i++)
		f->tr[i] = gs_texrender_create(GS_RGBA, GS_ZS_NONE);
	obs_leave_graphics();
	fx_update(f, settings);
	return f;
}

static void fx_destroy(void *data)
{
	struct fx_filter *f = data;
	obs_enter_graphics();
	for (int i = 0; i < 3; i++)
		gs_texrender_destroy(f->tr[i]);
	obs_leave_graphics();
	obs_data_release(f->settings);
	bfree(f);
}

static void fx_tick(void *data, float seconds)
{
	struct fx_filter *f = data;
	f->time += seconds;
	if (f->time > 3600.0f)
		f->time -= 3600.0f;
}

static bool reserved(const char *name)
{
	static const char *names[] = {"ViewProj", "image", "source", "size", "texel", "time"};
	for (size_t i = 0; i < sizeof(names) / sizeof(names[0]); i++)
		if (strcmp(name, names[i]) == 0)
			return true;
	return false;
}

static void colour_of(obs_data_item_t *item, struct vec4 *out)
{
	uint32_t c = (uint32_t)obs_data_item_get_int(item);
	vec4_set(out, (float)(c & 0xFF) / 255.0f, (float)((c >> 8) & 0xFF) / 255.0f, (float)((c >> 16) & 0xFF) / 255.0f,
		 1.0f);
}

/* Settings into the effect's own uniforms, and the frame's into ours. */
static void set_params(struct fx_filter *f, float w, float h)
{
	gs_effect_t *e = f->effect;
	size_t n = gs_effect_get_num_params(e);
	for (size_t i = 0; i < n; i++) {
		gs_eparam_t *p = gs_effect_get_param_by_idx(e, i);
		struct gs_effect_param_info info;
		gs_effect_get_param_info(p, &info);
		if (reserved(info.name))
			continue;
		obs_data_item_t *item = f->settings ? obs_data_item_byname(f->settings, info.name) : NULL;
		bool have = item && obs_data_item_has_user_value(item);
		if (!have) {
			gs_effect_set_default(p);
			obs_data_item_release(&item);
			continue;
		}
		bool is_bool = obs_data_item_gettype(item) == OBS_DATA_BOOLEAN;
		switch (info.type) {
		case GS_SHADER_PARAM_FLOAT:
			gs_effect_set_float(p, is_bool ? (obs_data_item_get_bool(item) ? 1.0f : 0.0f)
						       : (float)obs_data_item_get_double(item));
			break;
		case GS_SHADER_PARAM_INT:
			gs_effect_set_int(p, is_bool ? obs_data_item_get_bool(item) : (int)obs_data_item_get_int(item));
			break;
		case GS_SHADER_PARAM_BOOL:
			gs_effect_set_bool(p, is_bool ? obs_data_item_get_bool(item) : obs_data_item_get_int(item) != 0);
			break;
		case GS_SHADER_PARAM_VEC3: {
			struct vec4 c;
			colour_of(item, &c);
			struct vec3 v;
			vec3_set(&v, c.x, c.y, c.z);
			gs_effect_set_vec3(p, &v);
			break;
		}
		case GS_SHADER_PARAM_VEC4: {
			struct vec4 c;
			colour_of(item, &c);
			gs_effect_set_vec4(p, &c);
			break;
		}
		default:
			gs_effect_set_default(p);
			break;
		}
		obs_data_item_release(&item);
	}
	struct vec2 size, texel;
	vec2_set(&size, w, h);
	vec2_set(&texel, 1.0f / w, 1.0f / h);
	gs_effect_set_vec2(gs_effect_get_param_by_name(e, "size"), &size);
	gs_effect_set_vec2(gs_effect_get_param_by_name(e, "texel"), &texel);
	gs_effect_set_float(gs_effect_get_param_by_name(e, "time"), f->time);
}

static void fx_render(void *data, gs_effect_t *unused)
{
	UNUSED_PARAMETER(unused);
	struct fx_filter *f = data;
	obs_source_t *target = obs_filter_get_target(f->context);
	uint32_t w = target ? obs_source_get_base_width(target) : 0;
	uint32_t h = target ? obs_source_get_base_height(target) : 0;
	if (!f->effect || !w || !h) {
		obs_source_skip_video_filter(f->context);
		return;
	}

	if (f->passes == 0) {
		if (!obs_source_process_filter_begin(f->context, GS_RGBA, OBS_ALLOW_DIRECT_RENDERING))
			return;
		set_params(f, (float)w, (float)h);
		gs_blend_state_push();
		gs_blend_function(GS_BLEND_ONE, GS_BLEND_INVSRCALPHA);
		obs_source_process_filter_end(f->context, f->effect, 0, 0);
		gs_blend_state_pop();
		return;
	}

	/* the input, then each pass but the last into a texture */
	gs_blend_state_push();
	gs_reset_blend_state();
	gs_enable_blending(false);
	gs_texrender_reset(f->tr[0]);
	bool ok = false;
	if (gs_texrender_begin(f->tr[0], w, h)) {
		struct vec4 clear;
		vec4_zero(&clear);
		gs_clear(GS_CLEAR_COLOR, &clear, 0.0f, 0);
		gs_ortho(0.0f, (float)w, 0.0f, (float)h, -100.0f, 100.0f);
		if (obs_source_process_filter_begin(f->context, GS_RGBA, OBS_NO_DIRECT_RENDERING)) {
			obs_source_process_filter_end(f->context, obs_get_base_effect(OBS_EFFECT_DEFAULT), w, h);
			ok = true;
		}
		gs_texrender_end(f->tr[0]);
	}
	if (!ok) {
		gs_blend_state_pop();
		obs_source_skip_video_filter(f->context);
		return;
	}
	gs_texture_t *source = gs_texrender_get_texture(f->tr[0]);
	gs_texture_t *in = source;
	char tech[8];
	for (int i = 0; i + 1 < f->passes; i++) {
		gs_texrender_t *tr = f->tr[1 + (i & 1)];
		gs_texrender_reset(tr);
		if (!gs_texrender_begin(tr, w, h))
			break;
		gs_ortho(0.0f, (float)w, 0.0f, (float)h, -100.0f, 100.0f);
		set_params(f, (float)w, (float)h);
		gs_effect_set_texture(gs_effect_get_param_by_name(f->effect, "image"), in);
		gs_effect_set_texture(gs_effect_get_param_by_name(f->effect, "source"), source);
		snprintf(tech, sizeof(tech), "Pass%d", i);
		while (gs_effect_loop(f->effect, tech))
			gs_draw_sprite(in, 0, w, h);
		gs_texrender_end(tr);
		in = gs_texrender_get_texture(tr);
	}
	gs_blend_state_pop();

	/* the last pass straight to the output */
	set_params(f, (float)w, (float)h);
	gs_effect_set_texture(gs_effect_get_param_by_name(f->effect, "image"), in);
	gs_effect_set_texture(gs_effect_get_param_by_name(f->effect, "source"), source);
	snprintf(tech, sizeof(tech), "Pass%d", f->passes - 1);
	gs_blend_state_push();
	gs_blend_function(GS_BLEND_ONE, GS_BLEND_INVSRCALPHA);
	while (gs_effect_loop(f->effect, tech))
		gs_draw_sprite(in, 0, w, h);
	gs_blend_state_pop();
}

static void fx_defaults(obs_data_t *settings)
{
	obs_data_set_default_string(settings, "fx", "");
}

static obs_properties_t *fx_properties(void *data)
{
	UNUSED_PARAMETER(data);
	obs_properties_t *props = obs_properties_create();
	obs_properties_add_text(props, "fx", obs_module_text("Effect"), OBS_TEXT_DEFAULT);
	return props;
}

struct obs_source_info fx_filter_info = {
	.id = "lightair_fx",
	.type = OBS_SOURCE_TYPE_FILTER,
	.output_flags = OBS_SOURCE_VIDEO,
	.get_name = fx_get_name,
	.create = fx_create,
	.destroy = fx_destroy,
	.update = fx_update,
	.get_defaults = fx_defaults,
	.get_properties = fx_properties,
	.video_tick = fx_tick,
	.video_render = fx_render,
};
