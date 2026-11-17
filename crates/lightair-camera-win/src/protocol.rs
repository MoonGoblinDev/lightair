//! What Lightair and its Windows camera share (this file is also the engine's, by path): the
//! camera's identity and one NV12 frame in shared memory.
//!
//! The camera (in Windows' Frame Server service) makes the memory, as `Global\` (a service may;
//! an app may not) and open to the signed-in user; Lightair opens it while its virtual camera
//! runs and writes each frame under a sequence lock: `seq` is odd while a frame is being
//! written, and the camera copies a frame only if `seq` was even and unchanged across the copy.

use std::sync::atomic::AtomicU64;

/// The camera's media source (a COM class, registered by `regsvr32 lightair_camera.dll`).
pub const CLSID: u128 = 0x2A3528B8_0834_47CF_9DA6_910CCF68CBF1;
pub const CLSID_STRING: &str = "{2A3528B8-0834-47CF-9DA6-910CCF68CBF1}";
/// What other apps list it as.
pub const NAME: &str = "Lightair Virtual Camera";

/// The shared memory's name: `Global\` from the camera's service, `Local\` if the camera
/// runs in a process that can't make global memory.
pub const MEMORY_GLOBAL: &str = r"Global\LightairVirtualCamera";
pub const MEMORY_LOCAL: &str = r"Local\LightairVirtualCamera";

/// The camera's picture: 1080p NV12 at 30 frames a second, BT.601 limited range (what apps
/// assume of a webcam's NV12 when they don't read the media type's colour flags; it says so too).
pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
pub const FPS: u32 = 30;

pub const MAGIC: u32 = u32::from_le_bytes(*b"LACM");
pub const VERSION: u32 = 1;

#[repr(C)]
pub struct Header {
    pub magic: u32,
    pub version: u32,
    pub width: u32,
    pub height: u32,
    /// Odd while a frame is being written.
    pub seq: AtomicU64,
    /// When Lightair last wrote (`GetTickCount64`, ms): older than `STALE_MS`, the camera
    /// shows black (Lightair stopped sending).
    pub written_ms: AtomicU64,
    /// When the camera last read (ms): Lightair can tell an app has it open.
    pub read_ms: AtomicU64,
}

pub const STALE_MS: u64 = 1000;
/// The header's room (the frame starts 64 bytes in, aligned).
pub const HEADER_SIZE: usize = 64;
pub const FRAME_SIZE: usize = (WIDTH * HEIGHT * 3 / 2) as usize;
pub const SIZE: usize = HEADER_SIZE + FRAME_SIZE;

const _: () = assert!(std::mem::size_of::<Header>() <= HEADER_SIZE);
