//! lightair-preview's tests against real libobs, one test program (one build and link, not one per file).
//! libobs runs once per process: a test that starts an engine runs in a process of its
//! own (`lightair_engine::testing`).

mod crop_view;
mod effects;
mod face_tracking;
mod fx;
mod green_key;
mod hierarchy;
mod key_colour;
#[cfg(target_os = "macos")]
mod media_native;
mod media_on_air;
#[cfg(target_os = "macos")]
mod nv12;
mod resize_canvas;
mod screen_zoom;
mod shape_mask;
mod stinger;
mod studio_preview;
mod track_picture;
mod transform3d;
mod transitions;
#[cfg(windows)]
mod virtual_camera;
