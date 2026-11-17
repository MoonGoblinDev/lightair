/*
 * Lightair transform (docs/07-scenes-layers-canvas.md, "Transform"): flips and a 3D tilt with
 * perspective, drawn on the GPU. The picture is rendered flat, then drawn through one 4x4
 * matrix: centre, flip, tilt around X, tilt around Y, perspective, back. The perspective
 * divide happens on the GPU, so texturing stays correct.
 *
 * Settings: flip_h, flip_v (bool), tilt_x, tilt_y (degrees).
 */

#include <obs-module.h>
#include <graphics/matrix4.h>

struct transform3d {
	obs_source_t *context;
	bool flip_h, flip_v;
	float tilt_x, tilt_y;
};

static const char *transform3d_name(void *unused)
{
	UNUSED_PARAMETER(unused);
	return "Transform";
}

static void transform3d_update(void *data, obs_data_t *settings)
{
	struct transform3d *f = data;
	f->flip_h = obs_data_get_bool(settings, "flip_h");
	f->flip_v = obs_data_get_bool(settings, "flip_v");
	f->tilt_x = (float)obs_data_get_double(settings, "tilt_x");
	f->tilt_y = (float)obs_data_get_double(settings, "tilt_y");
}

static void *transform3d_create(obs_data_t *settings, obs_source_t *context)
{
	struct transform3d *f = bzalloc(sizeof(*f));
	f->context = context;
	transform3d_update(f, settings);
	return f;
}

static void transform3d_destroy(void *data)
{
	bfree(data);
}

static void transform3d_render(void *data, gs_effect_t *unused)
{
	UNUSED_PARAMETER(unused);
	struct transform3d *f = data;
	obs_source_t *target = obs_filter_get_target(f->context);
	float w = target ? (float)obs_source_get_base_width(target) : 0.f;
	float h = target ? (float)obs_source_get_base_height(target) : 0.f;
	bool active = f->flip_h || f->flip_v || fabsf(f->tilt_x) > 0.01f || fabsf(f->tilt_y) > 0.01f;
	if (w <= 0.f || h <= 0.f || !active) {
		obs_source_skip_video_filter(f->context);
		return;
	}

	const enum gs_color_space preferred[] = {GS_CS_SRGB, GS_CS_SRGB_16F, GS_CS_709_EXTENDED};
	const enum gs_color_space space = obs_source_get_color_space(target, OBS_COUNTOF(preferred), preferred);
	const enum gs_color_format format = gs_get_format_from_space(space);
	if (!obs_source_process_filter_begin_with_color_space(f->context, format, space, OBS_NO_DIRECT_RENDERING))
		return;

	/* Row vectors: each step applies after the previous one. */
	struct matrix4 m;
	matrix4_identity(&m);
	matrix4_translate3f(&m, &m, -w * 0.5f, -h * 0.5f, 0.f);
	matrix4_scale3f(&m, &m, f->flip_h ? -1.f : 1.f, f->flip_v ? -1.f : 1.f, 1.f);
	matrix4_rotate_aa4f(&m, &m, 1.f, 0.f, 0.f, RAD(f->tilt_x));
	matrix4_rotate_aa4f(&m, &m, 0.f, 1.f, 0.f, RAD(f->tilt_y));
	/* Perspective: depth goes into w (so the GPU divides by it) and z flattens to 0, keeping
	 * the quad inside the scene's depth range. The eye sits two picture-widths away. */
	float d = fmaxf(w, h) * 2.f;
	struct matrix4 p;
	matrix4_identity(&p);
	vec4_set(&p.z, 0.f, 0.f, 0.f, 1.f / d);
	matrix4_mul(&m, &m, &p);
	matrix4_translate3f(&m, &m, w * 0.5f, h * 0.5f, 0.f);

	gs_matrix_push();
	gs_matrix_mul(&m);
	gs_blend_state_push();
	gs_blend_function(GS_BLEND_ONE, GS_BLEND_INVSRCALPHA);
	obs_source_process_filter_end(f->context, obs_get_base_effect(OBS_EFFECT_DEFAULT), 0, 0);
	gs_blend_state_pop();
	gs_matrix_pop();
}

static enum gs_color_space transform3d_color_space(void *data, size_t count, const enum gs_color_space *preferred)
{
	UNUSED_PARAMETER(count);
	UNUSED_PARAMETER(preferred);
	struct transform3d *f = data;
	const enum gs_color_space potential[] = {GS_CS_SRGB, GS_CS_SRGB_16F, GS_CS_709_EXTENDED};
	return obs_source_get_color_space(obs_filter_get_target(f->context), OBS_COUNTOF(potential), potential);
}

struct obs_source_info transform3d_info = {
	.id = "lightair_transform",
	.type = OBS_SOURCE_TYPE_FILTER,
	.output_flags = OBS_SOURCE_VIDEO | OBS_SOURCE_SRGB,
	.get_name = transform3d_name,
	.create = transform3d_create,
	.destroy = transform3d_destroy,
	.update = transform3d_update,
	.video_render = transform3d_render,
	.video_get_color_space = transform3d_color_space,
};
