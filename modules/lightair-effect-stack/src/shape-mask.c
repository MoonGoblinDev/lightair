/*
 * Lightair shape mask (docs/08-video-effects.md, "Masks"): rounded rectangle, circle or pill
 * cut-outs with a soft edge and an optional border, computed on the GPU at the source's size.
 *
 * Settings: shape (0 rounded box, 1 circle, 2 pill), radius (0..1 of half the short side),
 * feather (px), border (px), border_color (0xAABBGGRR).
 *
 * Crop view: window_left/top/right/bottom (px) place the shape in a part of the picture (the
 * crop, while the whole picture is shown), and `outside` (0..1) is how much of the rest shows,
 * as a faded ghost. The filter eases towards `outside` on its own, so the cut-away part fades
 * in and out instead of popping.
 */

#include <obs-module.h>
#include <graphics/vec2.h>
#include <graphics/vec4.h>
#include <math.h>

struct shape_mask {
	obs_source_t *context;
	gs_effect_t *effect;
	int shape;
	float radius;
	float feather;
	float border;
	struct vec4 border_color;
	float window[4];
	float outside_target;
	float outside_now;
};

static const char *shape_mask_name(void *unused)
{
	UNUSED_PARAMETER(unused);
	return "Shape";
}

static void shape_mask_update(void *data, obs_data_t *settings)
{
	struct shape_mask *f = data;
	f->shape = (int)obs_data_get_int(settings, "shape");
	f->radius = (float)obs_data_get_double(settings, "radius");
	f->feather = (float)obs_data_get_double(settings, "feather");
	f->border = (float)obs_data_get_double(settings, "border");
	/* The picture is drawn in linear light: the colour as chosen (sRGB) made linear too, or
	 * the border shows lighter than picked. */
	vec4_from_rgba_srgb(&f->border_color, (uint32_t)obs_data_get_int(settings, "border_color"));
	f->window[0] = (float)obs_data_get_double(settings, "window_left");
	f->window[1] = (float)obs_data_get_double(settings, "window_top");
	f->window[2] = (float)obs_data_get_double(settings, "window_right");
	f->window[3] = (float)obs_data_get_double(settings, "window_bottom");
	f->outside_target = fminf(fmaxf((float)obs_data_get_double(settings, "outside"), 0.f), 1.f);
}

static void shape_mask_tick(void *data, float seconds)
{
	struct shape_mask *f = data;
	float diff = f->outside_target - f->outside_now;
	if (fabsf(diff) < 0.002f) {
		f->outside_now = f->outside_target;
		return;
	}
	/* About 90% of the way in 0.2 s, whatever the frame rate. */
	f->outside_now += diff * (1.f - expf(-seconds / 0.085f));
}

static void shape_mask_defaults(obs_data_t *settings)
{
	obs_data_set_default_int(settings, "shape", 0);
	obs_data_set_default_double(settings, "radius", 0.2);
	obs_data_set_default_double(settings, "feather", 1.0);
	obs_data_set_default_double(settings, "border", 0.0);
	obs_data_set_default_int(settings, "border_color", 0xFFFFFFFF);
}

static void *shape_mask_create(obs_data_t *settings, obs_source_t *context)
{
	struct shape_mask *f = bzalloc(sizeof(*f));
	f->context = context;
	char *path = obs_module_file("shape-mask.effect");
	obs_enter_graphics();
	f->effect = gs_effect_create_from_file(path, NULL);
	obs_leave_graphics();
	bfree(path);
	if (!f->effect)
		blog(LOG_WARNING, "[shape mask] couldn't load shape-mask.effect");
	shape_mask_update(f, settings);
	return f;
}

static void shape_mask_destroy(void *data)
{
	struct shape_mask *f = data;
	obs_enter_graphics();
	gs_effect_destroy(f->effect);
	obs_leave_graphics();
	bfree(f);
}

static void shape_mask_render(void *data, gs_effect_t *unused)
{
	UNUSED_PARAMETER(unused);
	struct shape_mask *f = data;
	obs_source_t *target = obs_filter_get_target(f->context);
	if (!target || !f->effect) {
		obs_source_skip_video_filter(f->context);
		return;
	}
	float w = (float)obs_source_get_base_width(target);
	float h = (float)obs_source_get_base_height(target);
	if (w <= 0.f || h <= 0.f) {
		obs_source_skip_video_filter(f->context);
		return;
	}
	/* The shape sits in the crop window (the whole picture when there's none). */
	float x0 = fmaxf(f->window[0], 0.f), y0 = fmaxf(f->window[1], 0.f);
	float x1 = fmaxf(w - f->window[2], x0 + 1.f), y1 = fmaxf(h - f->window[3], y0 + 1.f);
	struct vec2 size, half_box, centre;
	vec2_set(&size, w, h);
	vec2_set(&centre, (x0 + x1) * 0.5f, (y0 + y1) * 0.5f);
	w = x1 - x0;
	h = y1 - y0;
	float short_half = fminf(w, h) * 0.5f;
	float radius;
	switch (f->shape) {
	case 1: /* circle: the largest one that fits, centred */
		vec2_set(&half_box, short_half, short_half);
		radius = short_half;
		break;
	case 2: /* pill: fully rounded short ends */
		vec2_set(&half_box, w * 0.5f, h * 0.5f);
		radius = short_half;
		break;
	default: /* rounded box */
		vec2_set(&half_box, w * 0.5f, h * 0.5f);
		radius = fminf(fmaxf(f->radius, 0.f), 1.f) * short_half;
		break;
	}

	const enum gs_color_space preferred[] = {GS_CS_SRGB, GS_CS_SRGB_16F, GS_CS_709_EXTENDED};
	const enum gs_color_space space = obs_source_get_color_space(target, OBS_COUNTOF(preferred), preferred);
	const enum gs_color_format format = gs_get_format_from_space(space);
	if (!obs_source_process_filter_begin_with_color_space(f->context, format, space, OBS_ALLOW_DIRECT_RENDERING))
		return;
	gs_effect_set_vec2(gs_effect_get_param_by_name(f->effect, "size"), &size);
	gs_effect_set_vec2(gs_effect_get_param_by_name(f->effect, "centre"), &centre);
	gs_effect_set_float(gs_effect_get_param_by_name(f->effect, "outside"), f->outside_now);
	gs_effect_set_vec2(gs_effect_get_param_by_name(f->effect, "half_box"), &half_box);
	gs_effect_set_float(gs_effect_get_param_by_name(f->effect, "radius"), radius);
	gs_effect_set_float(gs_effect_get_param_by_name(f->effect, "feather"), f->feather);
	gs_effect_set_float(gs_effect_get_param_by_name(f->effect, "border"), f->border);
	gs_effect_set_vec4(gs_effect_get_param_by_name(f->effect, "border_color"), &f->border_color);
	gs_blend_state_push();
	gs_blend_function(GS_BLEND_ONE, GS_BLEND_INVSRCALPHA);
	obs_source_process_filter_end(f->context, f->effect, 0, 0);
	gs_blend_state_pop();
}

static enum gs_color_space shape_mask_color_space(void *data, size_t count, const enum gs_color_space *preferred)
{
	struct shape_mask *f = data;
	const enum gs_color_space potential[] = {GS_CS_SRGB, GS_CS_SRGB_16F, GS_CS_709_EXTENDED};
	return obs_source_get_color_space(obs_filter_get_target(f->context), OBS_COUNTOF(potential), potential);
	UNUSED_PARAMETER(count);
	UNUSED_PARAMETER(preferred);
}

struct obs_source_info shape_mask_info = {
	.id = "lightair_shape_mask",
	.type = OBS_SOURCE_TYPE_FILTER,
	.output_flags = OBS_SOURCE_VIDEO | OBS_SOURCE_SRGB,
	.get_name = shape_mask_name,
	.create = shape_mask_create,
	.destroy = shape_mask_destroy,
	.update = shape_mask_update,
	.get_defaults = shape_mask_defaults,
	.video_render = shape_mask_render,
	.video_tick = shape_mask_tick,
	.video_get_color_space = shape_mask_color_space,
};
