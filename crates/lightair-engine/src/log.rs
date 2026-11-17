//! Routes libobs's `blog` output into `tracing` (target `libobs`).

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Once;

use libobs_sys as sys;

// The UCRT defines the printf family inline in its headers; this library has them as functions.
#[cfg_attr(windows, link(name = "legacy_stdio_definitions"))]
unsafe extern "C" {
    fn vsnprintf(buf: *mut c_char, size: usize, fmt: *const c_char, args: sys::va_list) -> c_int;
}

unsafe extern "C" fn handler(level: c_int, fmt: *const c_char, args: sys::va_list, _param: *mut c_void) {
    let mut buf = [0 as c_char; 4096];
    // SAFETY: libobs passes a valid printf-style canvas and matching va_list.
    let msg = unsafe {
        vsnprintf(buf.as_mut_ptr(), buf.len(), fmt, args);
        CStr::from_ptr(buf.as_ptr()).to_string_lossy()
    };
    let level = level as u32;
    if level <= sys::LOG_ERROR {
        tracing::error!(target: "libobs", "{msg}");
    } else if level <= sys::LOG_WARNING {
        tracing::warn!(target: "libobs", "{msg}");
    } else if level <= sys::LOG_INFO {
        tracing::info!(target: "libobs", "{msg}");
    } else {
        tracing::debug!(target: "libobs", "{msg}");
    }
}

pub(crate) fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe { sys::base_set_log_handler(Some(handler), std::ptr::null_mut()) });
}
