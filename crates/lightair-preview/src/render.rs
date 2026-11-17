//! Drawing the canvas (or a source) into a preview-sized libobs render target: shared by every
//! preview backend. Runs on libobs's graphics thread, inside its graphics context.

use std::ffi::CString;
use std::ptr;

use lightair_engine::sys;

/// Renders `render` (a canvas's view or a source, in a `cw`×`ch` coordinate space) directly
/// into `dst`, scaled to the preview.
pub(crate) unsafe fn draw_with(cw: u32, ch: u32, dst: *mut sys::gs_texture_t, dw: u32, dh: u32, render: impl FnOnce()) {
    unsafe {
        let prev_rt = sys::gs_get_render_target();
        let prev_zs = sys::gs_get_zstencil_target();
        sys::gs_viewport_push();
        sys::gs_projection_push();
        sys::gs_matrix_push();
        sys::gs_matrix_identity();
        sys::gs_set_render_target(dst, ptr::null_mut());
        sys::gs_set_viewport(0, 0, dw as i32, dh as i32);
        sys::gs_ortho(0.0, cw as f32, 0.0, ch as f32, -100.0, 100.0);
        let black = sys::vec4::default();
        sys::gs_clear(sys::GS_CLEAR_COLOR, &black, 1.0, 0);
        render();
        sys::gs_set_render_target(prev_rt, prev_zs);
        sys::gs_matrix_pop();
        sys::gs_projection_pop();
        sys::gs_viewport_pop();
    }
}

pub(crate) unsafe fn draw_scaled(src: *mut sys::gs_texture_t, sw: u32, sh: u32, dst: *mut sys::gs_texture_t, dw: u32, dh: u32) {
    thread_local! {
        static IMAGE: CString = CString::new("image").unwrap();
        static DRAW: CString = CString::new("Draw").unwrap();
    }
    unsafe {
        let prev_rt = sys::gs_get_render_target();
        let prev_zs = sys::gs_get_zstencil_target();
        sys::gs_viewport_push();
        sys::gs_projection_push();
        sys::gs_matrix_push();
        sys::gs_matrix_identity();
        sys::gs_blend_state_push();
        sys::gs_enable_blending(false);

        sys::gs_set_render_target(dst, ptr::null_mut());
        sys::gs_set_viewport(0, 0, dw as i32, dh as i32);
        sys::gs_ortho(0.0, sw as f32, 0.0, sh as f32, -100.0, 100.0);

        // much smaller than the source: libobs's area filter (each pixel the average of the ones
        // it covers; bilinear would skip most of them and shimmer), as OBS scales its output
        let area = dw * 3 / 2 < sw && dh * 3 / 2 < sh;
        let effect = sys::obs_get_base_effect(if area { sys::OBS_EFFECT_AREA } else { sys::OBS_EFFECT_DEFAULT });
        let image = IMAGE.with(|n| sys::gs_effect_get_param_by_name(effect, n.as_ptr()));
        sys::gs_effect_set_texture(image, src);
        if area {
            let size = sys::vec2 { __bindgen_anon_1: sys::vec2__bindgen_ty_1 { __bindgen_anon_1: sys::vec2__bindgen_ty_1__bindgen_ty_1 { x: sw as f32, y: sh as f32 } } };
            let inverse = sys::vec2 { __bindgen_anon_1: sys::vec2__bindgen_ty_1 { __bindgen_anon_1: sys::vec2__bindgen_ty_1__bindgen_ty_1 { x: 1.0 / sw as f32, y: 1.0 / sh as f32 } } };
            sys::gs_effect_set_vec2(sys::gs_effect_get_param_by_name(effect, c"base_dimension".as_ptr()), &size);
            sys::gs_effect_set_vec2(sys::gs_effect_get_param_by_name(effect, c"base_dimension_i".as_ptr()), &inverse);
        }
        DRAW.with(|t| {
            while sys::gs_effect_loop(effect, t.as_ptr()) {
                sys::gs_draw_sprite(src, 0, sw, sh);
            }
        });

        sys::gs_set_render_target(prev_rt, prev_zs);
        sys::gs_blend_state_pop();
        sys::gs_matrix_pop();
        sys::gs_projection_pop();
        sys::gs_viewport_pop();
    }
}

