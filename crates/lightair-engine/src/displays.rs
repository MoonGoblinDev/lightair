//! Display identifiers as OBS's capture sources expect them, and the displays there are (each
//! with a capture of its own, for a picker's live thumbnails).

/// UUID of the main display, as used by `screen_capture`'s `display_uuid` setting.
#[cfg(target_os = "macos")]
pub fn main_display_uuid() -> Option<String> {
    use std::ffi::{c_char, c_void};

    type CFTypeRef = *const c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGMainDisplayID() -> u32;
    }
    #[link(name = "ColorSync", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFTypeRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFUUIDCreateString(alloc: CFTypeRef, uuid: CFTypeRef) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFRelease(cf: CFTypeRef);
    }
    const UTF8: u32 = 0x0800_0100;

    unsafe {
        let uuid = CGDisplayCreateUUIDFromDisplayID(CGMainDisplayID());
        if uuid.is_null() {
            return None;
        }
        let text = CFUUIDCreateString(std::ptr::null(), uuid);
        CFRelease(uuid);
        if text.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 64];
        let ok = CFStringGetCString(text, buf.as_mut_ptr(), buf.len() as isize, UTF8);
        CFRelease(text);
        ok.then(|| std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

/// Windows: the primary monitor as OBS's display capture names it (its `monitor_id`): the
/// display device's interface path, or the GDI device name where it has none.
#[cfg(windows)]
pub fn main_display_uuid() -> Option<String> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        DISPLAY_DEVICEW, EnumDisplayDevicesW, GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFOEXW, MonitorFromPoint,
    };
    const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;
    let text = |w: &[u16]| String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())]);
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut _) == 0 {
            return None;
        }
        let mut device: DISPLAY_DEVICEW = std::mem::zeroed();
        device.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        if EnumDisplayDevicesW(info.szDevice.as_ptr(), 0, &mut device, EDD_GET_DEVICE_INTERFACE_NAME) != 0 {
            let id = text(&device.DeviceID);
            if !id.is_empty() {
                return Some(id);
            }
        }
        Some(text(&info.szDevice)).filter(|s| !s.is_empty())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn main_display_uuid() -> Option<String> {
    None
}

/// A display there is, as the display capture source lists it.
pub struct Display {
    /// "Built-in Retina Display".
    pub name: String,
    /// Its size as the capture source reports it ("1512x982"), parsed.
    pub size: Option<(u32, u32)>,
    /// The portable `display` setting that captures it.
    pub value: serde_json::Value,
    /// The main display (the one with the menu bar; Windows's primary monitor).
    pub main: bool,
    /// A private capture of it, for a thumbnail (dropped on the engine thread).
    pub capture: Option<crate::obs::ObsSource>,
}

/// "Built-in Retina Display: 1512x982 @ 0,0" → ("Built-in Retina Display", 1512×982, main?).
fn parse_label(label: &str) -> (String, Option<(u32, u32)>, bool) {
    let (name, rest) = label.split_once(": ").unwrap_or((label, ""));
    let size = rest.split_whitespace().next().and_then(|wh| wh.split_once('x')).and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
    (name.trim().to_string(), size, rest.contains("Primary"))
}

/// Settings for the screen capture source in `mode`, with `key` (the portable "display",
/// "window" or "app") set to `value`: (libobs kind, its settings).
/// The capture itself (OBS's screen_capture) and its settings: the lists and thumbnails need
/// a capture that runs now, not the wrapper that runs one only while shown.
fn screen_settings(mode: i64, key: &str, value: Option<&serde_json::Value>) -> Option<(&'static str, serde_json::Value)> {
    let mut s = serde_json::json!({"mode": mode, "cursor": true});
    if let Some(v) = value {
        s[key] = v.clone();
    }
    let (kind, settings) = crate::kinds::resolve(lightair_model::kinds::SCREEN, &s)?;
    #[cfg(target_os = "macos")]
    let kind = crate::on_screen::inner_id(kind).unwrap_or(kind);
    Some((kind, settings))
}

/// What the screen capture source offers for `key` ("display", "window" or "app") in `mode`:
/// its labels and values, as it lists them. Engine thread.
pub fn screen_options(mode: i64, key: &str) -> Vec<(String, serde_json::Value)> {
    // (listed in its own mode: that's what keeps the lists to windows and apps on screen; with
    // nothing chosen yet libobs logs that it has no window to capture, harmlessly)
    let (Some((kind, settings)), Some(obs_key)) =
        (screen_settings(mode, key, None), crate::kinds::libobs_key(lightair_model::kinds::SCREEN, key))
    else {
        return vec![];
    };
    match crate::obs::ObsSource::create_private(kind, "Lightair screen choices", Some(&crate::obs::Data::from_json(&settings))) {
        Some(lister) => crate::obs::source_list_items(lister.as_ptr(), obs_key),
        None => vec![],
    }
}

/// A private capture of one of those choices (for a thumbnail; release it on the engine
/// thread). Engine thread.
pub fn screen_capture(mode: i64, key: &str, value: &serde_json::Value, name: &str) -> Option<crate::obs::ObsSource> {
    let (kind, settings) = screen_settings(mode, key, Some(value))?;
    crate::obs::ObsSource::create_private(kind, name, Some(&crate::obs::Data::from_json(&settings)))
}

/// The displays, each with a capture of it (`captures`). Engine thread.
pub fn list(captures: bool) -> Vec<Display> {
    use lightair_model::kinds::DISPLAY;
    let main = main_display_uuid();
    screen_options(DISPLAY, "display")
        .into_iter()
        .enumerate()
        .map(|(i, (label, value))| {
            let (name, size, primary) = parse_label(&label);
            // (a monitor that reports no name of its own, as a laptop's panel may)
            let name = if name.is_empty() { format!("Display {}", i + 1) } else { name };
            let capture = if captures { screen_capture(DISPLAY, "display", &value, &name) } else { None };
            let is_main = primary || (main.is_some() && value.as_str() == main.as_deref());
            Display { name, size, value, main: is_main, capture }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn display_labels_read_as_name_and_size() {
        assert_eq!(
            super::parse_label("Built-in Retina Display: 1512x982 @ 0,0"),
            ("Built-in Retina Display".into(), Some((1512, 982)), false)
        );
        assert_eq!(super::parse_label("DELL U2720Q: 3840x2160 @ 0,0 (Primary Monitor)"), ("DELL U2720Q".into(), Some((3840, 2160)), true));
        assert_eq!(super::parse_label("Screen"), ("Screen".into(), None, false));
    }
}
