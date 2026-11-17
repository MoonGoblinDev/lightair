//! Windows: the preview presented straight into a DirectComposition swap chain (the window's
//! canvas picture, under its UI), with no copy through memory and no redraw of the UI.
//!
//! The swap chain belongs to libobs's own D3D11 device: each frame the graphics thread copies
//! the preview's render target into the back buffer (a GPU copy) and presents without waiting.
//! The app shows the swap chain in a composition visual under GPUI's (`native_preview`).
//!
//! Never waiting matters: libobs renders every output on this thread, so a present that blocked
//! (the compositor busy, say while a display capture starts up and Windows reconfigures it)
//! would make the program skip frames. The swap chain has a frame-latency waitable object; a
//! frame is presented only when it's signalled (the compositor has taken the last one), else
//! the preview skips that frame and the program goes on.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

use lightair_engine::sys;
use parking_lot::Mutex;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Resource, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_WAS_STILL_DRAWING, DXGI_PRESENT_DO_NOT_WAIT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain1, IDXGISwapChain2,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::Interface;

/// A swap chain the preview presents its frames to, shown by a composition visual.
pub struct Layer {
    swap: IDXGISwapChain1,
    /// Signalled when the swap chain can take a frame without blocking (`Present` would wait
    /// otherwise); null where the system doesn't give one.
    ready: HANDLE,
    /// The back buffers' size, packed (width << 32 | height).
    size: AtomicU64,
    /// The back buffers' format (graphics thread).
    format: Mutex<DXGI_FORMAT>,
    /// The size the picture is shown at (pixels, packed; 0: not known): frames are rendered at
    /// it (up to the canvas's own size), so the compositor shows them as they are. On a laptop
    /// whose screen hangs off the other GPU, each frame crosses between the GPUs: a third of the
    /// width is a ninth of the copying.
    wanted: AtomicU64,
    /// Serialises presenting (graphics thread) with dropping.
    lock: Mutex<()>,
}

// SAFETY: the swap chain is used on libobs's graphics thread only (resize, copy, present); other
// threads read `size` or hand the swap chain's pointer to DirectComposition, which takes its own
// reference. The waitable handle is a kernel handle.
unsafe impl Send for Layer {}
unsafe impl Sync for Layer {}

const START: u32 = 16;
const FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

impl Layer {
    /// A swap chain on libobs's device (libobs must be running on Direct3D 11).
    pub fn new() -> Option<Self> {
        unsafe {
            sys::obs_enter_graphics();
            let device = sys::gs_get_device_obj();
            let swap = if device.is_null() { None } else { Self::make(device) };
            sys::obs_leave_graphics();
            if swap.is_none() {
                tracing::warn!("couldn't make a composition swap chain for the preview");
            }
            let swap = swap?;
            // at most one frame queued: the newest is what the window shows
            let ready = match swap.cast::<IDXGISwapChain2>() {
                Ok(s2) => {
                    let _ = s2.SetMaximumFrameLatency(1);
                    s2.GetFrameLatencyWaitableObject()
                }
                Err(_) => HANDLE::default(),
            };
            Some(Self {
                swap,
                ready,
                size: AtomicU64::new(pack(START, START)),
                format: Mutex::new(DXGI_FORMAT_B8G8R8A8_UNORM),
                wanted: AtomicU64::new(0),
                lock: Mutex::new(()),
            })
        }
    }

    unsafe fn make(device: *mut c_void) -> Option<IDXGISwapChain1> {
        unsafe {
            let device = ID3D11Device::from_raw_borrowed(&device)?;
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: START,
                Height: START,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: FLAGS.0 as u32,
            };
            factory.CreateSwapChainForComposition(device, &desc, None).ok()
        }
    }

    /// `IDXGISwapChain1*` (borrowed), for `IDCompositionVisual::SetContent`.
    pub fn swap_chain(&self) -> *mut c_void {
        self.swap.as_raw()
    }

    /// The size the picture is shown at, in pixels (the UI, when it places it).
    pub fn show_at(&self, w: u32, h: u32) {
        let packed = if w >= 2 && h >= 2 { pack(w & !1, h & !1) } else { 0 };
        self.wanted.store(packed, Ordering::Relaxed);
    }

    /// What `show_at` asked for, if anything.
    pub(crate) fn shown_at(&self) -> Option<(u32, u32)> {
        let p = self.wanted.load(Ordering::Relaxed);
        (p != 0).then_some(((p >> 32) as u32, p as u32))
    }

    /// The back buffers' size in pixels (the picture's visual scales it to its place).
    pub fn size(&self) -> (u32, u32) {
        let p = self.size.load(Ordering::Relaxed);
        ((p >> 32) as u32, p as u32)
    }

    /// Whether `tex` can be presented as it is (8-bit BGRA or RGBA: the formats a composition
    /// swap chain takes, and a copy keeps).
    pub(crate) unsafe fn can_present(&self, tex: *mut sys::gs_texture_t) -> bool {
        format_of(unsafe { sys::gs_texture_get_color_format(tex) }).is_some()
    }

    /// Whether the swap chain can take a frame now, without `Present` waiting.
    fn can_take(&self) -> bool {
        self.ready.is_invalid() || unsafe { WaitForSingleObject(self.ready, 0) } == WAIT_OBJECT_0
    }

    /// Copies `tex` (a `w`×`h` render target of libobs's) into the back buffer and presents it,
    /// never waiting: when the compositor hasn't taken the last frame yet, this one is skipped.
    /// Graphics thread, inside libobs's graphics context. Whether it was presented.
    pub(crate) unsafe fn present(&self, tex: *mut sys::gs_texture_t, w: u32, h: u32) -> bool {
        let _held = self.lock.lock();
        if !self.can_take() {
            return false;
        }
        unsafe {
            let Some(format) = format_of(sys::gs_texture_get_color_format(tex)) else { return false };
            let mut current = self.format.lock();
            if self.size() != (w, h) || *current != format {
                if self.swap.ResizeBuffers(0, w, h, format, FLAGS).is_err() {
                    return false;
                }
                self.size.store(pack(w, h), Ordering::Relaxed);
                *current = format;
            }
            let src = sys::gs_texture_get_obj(tex);
            let Some(src) = ID3D11Resource::from_raw_borrowed(&src) else { return false };
            let Ok(back) = self.swap.GetBuffer::<ID3D11Texture2D>(0) else { return false };
            let Ok(device) = back.GetDevice() else { return false };
            let Ok(context) = device.GetImmediateContext() else { return false };
            context.CopyResource(&back, src);
            drop(back);
            let started = std::time::Instant::now();
            let hr = self.swap.Present(0, DXGI_PRESENT_DO_NOT_WAIT);
            let took = started.elapsed();
            if took > std::time::Duration::from_millis(4) {
                tracing::debug!(?took, "the preview's present waited");
            }
            if hr.is_err() && hr != DXGI_ERROR_WAS_STILL_DRAWING {
                tracing::debug!("preview present: {hr:?}");
            }
            hr.is_ok()
        }
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        if !self.ready.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.ready);
            }
        }
    }
}

fn format_of(format: sys::gs_color_format) -> Option<DXGI_FORMAT> {
    match format {
        sys::GS_BGRA => Some(DXGI_FORMAT_B8G8R8A8_UNORM),
        sys::GS_RGBA => Some(DXGI_FORMAT_R8G8B8A8_UNORM),
        _ => None,
    }
}

fn pack(w: u32, h: u32) -> u64 {
    ((w as u64) << 32) | h as u64
}
