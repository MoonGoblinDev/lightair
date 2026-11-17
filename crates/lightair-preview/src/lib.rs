//! Canvas preview (docs/04-engine-libobs.md#preview-rendering).
//!
//! libobs renders the canvas, scaled to the preview's size, on its graphics thread; the UI takes
//! the newest finished frame. On macOS frames are IOSurface-backed `CVPixelBuffer`s for GPUI's
//! `surface()` element, with no pixel copied on the CPU (`mac`). Elsewhere (Windows today) frames
//! are read back to memory and shown as images (`readback`), until a zero-copy path replaces it.
//!
//! Threading: the render callback runs on libobs's graphics thread. It never waits on the UI;
//! the only shared lock is held for a pointer swap.

mod render;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(not(target_os = "macos"))]
mod readback;
#[cfg(windows)]
mod layer;
#[cfg(windows)]
pub use layer::Layer;

#[cfg(target_os = "macos")]
pub use mac::{Frame, Preview, frame_is_nv12, frame_rgba, frame_size, nv12, sample_ycbcr};
#[cfg(not(target_os = "macos"))]
pub use readback::{CpuFrame, Frame, Preview, frame_rgba, frame_size, sample_ycbcr};
