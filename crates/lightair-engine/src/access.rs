//! Camera and microphone access (macOS) without freezing: the first time a source opens either,
//! macOS asks the user, and the opening call (CoreAudio, AVFoundation) waits on that prompt.
//! On the engine thread, that wait froze the window (it waits on the engine) until the user
//! answered. So while macOS hasn't asked yet, the engine asks without waiting ([`ask`]) and
//! holds those sources back; once the user answers ([`changed`]) it makes them
//! (`Reconciler::access_answered`).

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Microphone,
    Camera,
}

/// The access a libobs source type needs, if any.
pub fn needed_by(kind: &str) -> Option<Media> {
    match kind {
        // (Lightair's own device inputs are a placeholder in libobs: the mixer opens the device,
        // and holds back there)
        "coreaudio_input_capture" => Some(Media::Microphone),
        "macos-avcapture" | "macos-avcapture-fast" | "av_capture_input" | "av_capture_input_v2" => Some(Media::Camera),
        _ => None,
    }
}

static GENERATION: AtomicU64 = AtomicU64::new(0);
static SEEN: AtomicU64 = AtomicU64::new(0);

/// Whether the user answered an access prompt since the last call: the held-back sources can
/// be made now.
pub fn changed() -> bool {
    let g = GENERATION.load(Ordering::Acquire);
    SEEN.swap(g, Ordering::AcqRel) != g
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::atomic::Ordering;

    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2::{msg_send, rc::autoreleasepool};

    use std::sync::atomic::AtomicBool;

    use super::{GENERATION, Media};

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVMediaTypeAudio: *const AnyObject;
        static AVMediaTypeVideo: *const AnyObject;
    }

    /// `AVAuthorizationStatusNotDetermined`.
    const NOT_DETERMINED: isize = 0;

    fn media_type(m: Media) -> *const AnyObject {
        unsafe {
            match m {
                Media::Microphone => AVMediaTypeAudio,
                Media::Camera => AVMediaTypeVideo,
            }
        }
    }

    pub fn needs_asking(m: Media) -> bool {
        status(m) == NOT_DETERMINED
    }

    /// AVAuthorizationStatus: 0 not asked, 1 restricted, 2 denied, 3 allowed.
    pub fn status(m: Media) -> isize {
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else { return 3 };
        unsafe { msg_send![class, authorizationStatusForMediaType: media_type(m)] }
    }

    pub fn ask(m: Media) {
        static ASKED: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];
        if ASKED[m as usize].swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else { return };
        tracing::info!(?m, "asking macOS for access");
        let done = block2::RcBlock::new(move |granted: Bool| {
            tracing::info!(?m, granted = granted.as_bool(), "access answered");
            GENERATION.fetch_add(1, Ordering::AcqRel);
        });
        autoreleasepool(|_| unsafe {
            let _: () = msg_send![class, requestAccessForMediaType: media_type(m), completionHandler: &*done];
        });
    }
}

/// Windows: camera and microphone have privacy switches (Settings › Privacy & security) for the
/// whole PC, for the user, and for desktop apps; any of them off blocks Lightair, which then
/// gets black frames or silence without an error. They're registry values Windows itself reads.
/// Windows never prompts a desktop app: there's nothing to ask, only switches to point to.
#[cfg(windows)]
mod imp {
    use super::Media;
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// The switch's value ("Allow" / "Deny"), if it's set.
    fn switch(root: HKEY, key: &str) -> Option<String> {
        let (key, value) = (wide(key), wide("Value"));
        let mut buf = [0u16; 32];
        let mut len = (buf.len() * 2) as u32;
        let r = unsafe {
            RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
        };
        (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
    }

    pub fn needs_asking(_: Media) -> bool {
        false
    }

    pub fn status(m: Media) -> isize {
        let store = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\";
        let cap = match m {
            Media::Camera => "webcam",
            Media::Microphone => "microphone",
        };
        let base = format!("{store}{cap}");
        let denied = [
            (HKEY_LOCAL_MACHINE, base.clone()),
            (HKEY_CURRENT_USER, base.clone()),
            (HKEY_CURRENT_USER, format!(r"{base}\NonPackaged")),
        ]
        .into_iter()
        .any(|(root, key)| switch(root, &key).is_some_and(|v| v.eq_ignore_ascii_case("Deny")));
        if denied { 2 } else { 3 }
    }

    pub fn ask(_: Media) {}
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::Media;
    pub fn needs_asking(_: Media) -> bool {
        false
    }
    pub fn status(_: Media) -> isize {
        3
    }
    pub fn ask(_: Media) {}
}

/// Whether the user allowed `m`: `None` not asked yet, `Some(false)` refused (or restricted).
pub fn allowed(m: Media) -> Option<bool> {
    match imp::status(m) {
        0 => None,
        3 => Some(true),
        _ => Some(false),
    }
}

/// macOS hasn't asked the user for `m` yet (opening it now would wait on the prompt).
pub fn needs_asking(m: Media) -> bool {
    imp::needs_asking(m)
}

/// Asks the user for `m` (once), without waiting; [`changed`] says when they've answered.
pub fn ask(m: Media) {
    imp::ask(m)
}

/// Whether opening `m` now would wait on the user: if so, asks (without waiting) and says so.
pub fn hold_back(m: Media) -> bool {
    let wait = needs_asking(m);
    if wait {
        ask(m);
    }
    wait
}
