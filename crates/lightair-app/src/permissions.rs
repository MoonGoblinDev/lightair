//! What Lightair needs macOS to allow, checked as it is now (Settings › Permissions): each with
//! what it's for, whether it's allowed, and the way to allow it (ask, or the right pane of
//! System Settings).

/// Where a permission stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Allowed,
    /// macOS hasn't asked yet (it asks when Lightair first needs it, or with Allow).
    NotAsked,
    /// Refused, or not allowed yet.
    Denied,
    /// macOS doesn't say until it's used (it asks then).
    AskedOnUse,
    /// Nothing to check now (the app it's about isn't open).
    NotNow,
}

pub struct Permission {
    pub key: &'static str,
    pub name: &'static str,
    pub why: &'static str,
    pub state: State,
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[repr(C)]
    struct AEDesc {
        descriptor_type: u32,
        data_handle: *mut c_void,
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn AECreateDesc(type_code: u32, data: *const c_void, size: isize, result: *mut AEDesc) -> i16;
        fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
        fn AEDeterminePermissionToAutomateTarget(target: *const AEDesc, class: u32, id: u32, ask: u8) -> i32;
    }

    #[link(name = "CoreBluetooth", kind = "framework")]
    unsafe extern "C" {}

    /// CBManagerAuthorization: 0 not asked, 1 restricted, 2 denied, 3 allowed.
    pub fn bluetooth() -> isize {
        use objc2::msg_send;
        use objc2::runtime::AnyClass;
        let Some(class) = AnyClass::get(c"CBManager") else { return 0 };
        unsafe { msg_send![class, authorization] }
    }

    pub fn screen() -> bool {
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    pub fn ask_screen() {
        unsafe { CGRequestScreenCaptureAccess() };
    }

    /// May Lightair send Apple Events to the app `bundle`: Some(true) yes, Some(false) no,
    /// None not asked yet; Err(()) the app isn't open. `ask` asks (waits for the answer).
    pub fn automation(bundle: &str, ask: bool) -> Result<Option<bool>, ()> {
        const BUNDLE: u32 = u32::from_be_bytes(*b"bund");
        const ANY: u32 = u32::from_be_bytes(*b"****");
        let mut desc = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
        if unsafe { AECreateDesc(BUNDLE, bundle.as_ptr().cast(), bundle.len() as isize, &mut desc) } != 0 {
            return Err(());
        }
        let status = unsafe { AEDeterminePermissionToAutomateTarget(&desc, ANY, ANY, ask as u8) };
        unsafe { AEDisposeDesc(&mut desc) };
        match status {
            0 => Ok(Some(true)),
            -1743 => Ok(Some(false)),
            -1744 => Ok(None),
            // procNotFound: not open
            _ => Err(()),
        }
    }
}

/// Every permission, as it stands now.
pub fn all() -> Vec<Permission> {
    use lightair_engine::access::{Media, allowed};
    let av = |m| match allowed(m) {
        Some(true) => State::Allowed,
        Some(false) => State::Denied,
        None => State::NotAsked,
    };
    #[cfg(target_os = "macos")]
    let (screen, music) = {
        let screen = if mac::screen() { State::Allowed } else { State::Denied };
        // the first music app that's open
        let music = ["com.spotify.client", "com.apple.Music"].iter().map(|b| mac::automation(b, false)).find(|r| r.is_ok()).map_or(State::NotNow, |r| match r {
            Ok(Some(true)) => State::Allowed,
            Ok(Some(false)) => State::Denied,
            _ => State::NotAsked,
        });
        (screen, music)
    };
    #[cfg(not(target_os = "macos"))]
    let (screen, music) = (State::Allowed, State::NotNow);
    #[cfg(target_os = "macos")]
    let bluetooth = match mac::bluetooth() {
        3 => State::Allowed,
        0 => State::AskedOnUse,
        _ => State::Denied,
    };
    #[cfg(not(target_os = "macos"))]
    let bluetooth = State::AskedOnUse;
    // Windows: only camera and microphone have privacy switches for desktop apps (screen and
    // desktop audio capture, input and the network need no permission there)
    #[cfg(windows)]
    {
        let _ = (screen, music, bluetooth);
        vec![
            Permission { key: "camera", name: "Camera", why: "Camera objects and Effects (Settings › Privacy & security › Camera)", state: av(Media::Camera) },
            Permission { key: "microphone", name: "Microphone", why: "Microphones in the mixer, recording and streaming (Settings › Privacy & security › Microphone)", state: av(Media::Microphone) },
        ]
    }
    #[cfg(not(windows))]
    vec![
        Permission { key: "screen", name: "Screen recording", why: "Display, window and app capture, and their thumbnails", state: screen },
        Permission { key: "camera", name: "Camera", why: "Camera objects and Effects", state: av(Media::Camera) },
        Permission { key: "microphone", name: "Microphone", why: "Microphones in the mixer, recording and streaming", state: av(Media::Microphone) },
        Permission { key: "audio", name: "System audio recording", why: "Desktop and app audio. macOS asks the first time it's captured", state: State::AskedOnUse },
        Permission {
            key: "input",
            name: "Input monitoring",
            why: "Screen zoom and pointer: following clicks and typing in screen captures",
            state: if lightair_platform::input::can_listen() { State::Allowed } else { State::Denied },
        },
        Permission { key: "music", name: "Control Spotify and Music", why: "The Now playing widget (checked while one of them is open)", state: music },
        Permission { key: "bluetooth", name: "Bluetooth", why: "Signing in with a passkey on your phone. macOS asks the first time", state: bluetooth },
        Permission { key: "network", name: "Local network", why: "Elgato Key Lights, devices on your network and remote control from them. macOS asks on first use", state: State::AskedOnUse },
    ]
}

/// Asks macOS for it where it can be asked (the prompt), else opens its System Settings pane.
pub fn ask(key: &str) {
    use lightair_engine::access::{Media, allowed, ask as ask_av};
    match key {
        "camera" | "microphone" => {
            let m = if key == "camera" { Media::Camera } else { Media::Microphone };
            if allowed(m).is_none() {
                ask_av(m);
                return;
            }
        }
        #[cfg(target_os = "macos")]
        "screen" => mac::ask_screen(),
        "input" => {
            if !lightair_platform::input::request_listen() {
                lightair_platform::input::open_input_monitoring_settings();
            }
            return;
        }
        #[cfg(target_os = "macos")]
        "music" => {
            // waits for the answer: not on the main thread
            std::thread::spawn(|| {
                for b in ["com.spotify.client", "com.apple.Music"] {
                    let _ = mac::automation(b, true);
                }
            });
            return;
        }
        _ => {}
    }
    open_settings(key);
}

/// The permission's pane in System Settings.
pub fn open_settings(key: &str) {
    let pane = match key {
        "screen" => "Privacy_ScreenCapture",
        "camera" => "Privacy_Camera",
        "microphone" => "Privacy_Microphone",
        "audio" => "Privacy_AudioCapture",
        "input" => "Privacy_ListenEvent",
        "music" => "Privacy_Automation",
        "network" => "Privacy_LocalNetwork",
        "bluetooth" => "Privacy_Bluetooth",
        _ => "",
    };
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("/usr/bin/open").arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}")).spawn();
    } else if cfg!(windows) {
        let page = match key {
            "camera" => "ms-settings:privacy-webcam",
            "microphone" => "ms-settings:privacy-microphone",
            _ => "ms-settings:privacy",
        };
        // (Explorer opens Settings at a page given by its address)
        let _ = std::process::Command::new("explorer.exe").arg(page).spawn();
    }
}
