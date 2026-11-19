//! The apps that are open, for picking one app's audio (or window) as a source.

/// An open app: its name as people know it, and the id its platform's capture takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub name: String,
    /// macOS: the bundle id (ScreenCaptureKit's `application`). Windows: libobs's window string
    /// (`title:class:executable`, with `#` and `:` escaped), as per-app audio capture expects.
    pub id: String,
}

/// Whether a window in a capture's window list is one to leave out: Lightair's own (a mirror),
/// or the system's (the Dock, the menu bar, Control Centre, the desktop…). `window` is the value
/// the platform's window capture lists: macOS, the window's number; Windows, libobs's
/// `title:class:exe` string. Decided by the window's owner (its process, its kind of app, its
/// program file), never by the names people see, which change with the language.
pub fn hide_window(window: &str) -> bool {
    imp::hide_window(window)
}

/// Whether a window in a capture's window list is this Lightair's own (macOS: by its owner
/// process). For recording Lightair itself in a demo, never anyone else's screen.
pub fn is_own_window(window: &str) -> bool {
    imp::is_own_window(window)
}

/// The open apps with windows (the ones in the Dock or taskbar), by name; not Lightair itself.
pub fn running() -> Vec<App> {
    let mut out = imp::running();
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.id == b.id);
    out
}

#[cfg(target_os = "macos")]
mod imp {
    use super::App;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const std::ffi::c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    /// The process that owns window `number` (CoreGraphics' window list).
    fn window_owner(number: u32) -> Option<i32> {
        // kCGWindowListOptionIncludingWindow: just that window
        let list = unsafe { CGWindowListCopyWindowInfo(1 << 3, number) } as *mut AnyObject;
        if list.is_null() {
            return None;
        }
        unsafe {
            let n: usize = msg_send![&*list, count];
            let pid = if n > 0 {
                let info: *mut AnyObject = msg_send![&*list, objectAtIndex: 0usize];
                let key = NSString::from_str("kCGWindowOwnerPID");
                let value: *mut AnyObject = msg_send![&*info, objectForKey: &*key];
                (!value.is_null()).then(|| msg_send![&*value, intValue])
            } else {
                None
            };
            CFRelease(list as *const std::ffi::c_void);
            pid
        }
    }

    pub fn is_own_window(window: &str) -> bool {
        window.parse::<u32>().ok().and_then(window_owner).is_some_and(|pid| pid as u32 == std::process::id())
    }

    pub fn hide_window(window: &str) -> bool {
        let Ok(number) = window.parse::<u32>() else { return false };
        let Some(pid) = window_owner(number) else { return false };
        if pid as u32 == std::process::id() {
            return true;
        }
        // a regular app (in the Dock) is one to capture; the rest are the system's
        let Some(class) = AnyClass::get(c"NSRunningApplication") else { return false };
        unsafe {
            let app: Option<Retained<AnyObject>> = msg_send![class, runningApplicationWithProcessIdentifier: pid];
            app.is_none_or(|app| {
                let policy: isize = msg_send![&*app, activationPolicy];
                policy != 0
            })
        }
    }

    pub fn running() -> Vec<App> {
        let mut out = Vec::new();
        let Some(class) = AnyClass::get(c"NSWorkspace") else { return out };
        unsafe {
            let ws: Retained<AnyObject> = msg_send![class, sharedWorkspace];
            let apps: Retained<AnyObject> = msg_send![&*ws, runningApplications];
            let n: usize = msg_send![&*apps, count];
            for k in 0..n {
                let app: Retained<AnyObject> = msg_send![&*apps, objectAtIndex: k];
                let policy: isize = msg_send![&*app, activationPolicy];
                let pid: i32 = msg_send![&*app, processIdentifier];
                // regular apps only (in the Dock)
                if policy != 0 || pid as u32 == std::process::id() {
                    continue;
                }
                let name: Option<Retained<NSString>> = msg_send![&*app, localizedName];
                let bundle: Option<Retained<NSString>> = msg_send![&*app, bundleIdentifier];
                if let (Some(name), Some(bundle)) = (name, bundle) {
                    out.push(App { name: name.to_string(), id: bundle.to_string() });
                }
            }
        }
        out
    }
}

#[cfg(windows)]
mod imp {
    use super::App;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId,
        GW_OWNER, IsWindowVisible, WS_EX_TOOLWINDOW,
    };

    /// libobs's escaping for its `title:class:exe` window strings.
    fn encode(s: &str) -> String {
        s.replace('#', "#22").replace(':', "#3A")
    }

    fn wide(buf: &[u16], len: usize) -> String {
        String::from_utf16_lossy(&buf[..len.min(buf.len())])
    }

    unsafe extern "system" fn each(hwnd: HWND, list: LPARAM) -> i32 {
        let out = unsafe { &mut *(list as *mut Vec<App>) };
        unsafe {
            // visible, top-level, not a tool window: what the taskbar shows
            if IsWindowVisible(hwnd) == 0
                || !GetWindow(hwnd, GW_OWNER).is_null()
                || (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW) != 0
            {
                return 1;
            }
            let mut title = [0u16; 256];
            let tn = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
            if tn <= 0 {
                return 1;
            }
            let mut class = [0u16; 256];
            let cn = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == std::process::id() {
                return 1;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return 1;
            }
            let mut path = [0u16; 1024];
            let mut len = path.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut len) != 0;
            CloseHandle(process);
            if !ok {
                return 1;
            }
            let path = wide(&path, len as usize);
            let exe = path.rsplit('\\').next().unwrap_or(&path).to_string();
            let title = wide(&title, tn as usize);
            let class = wide(&class, cn.max(0) as usize);
            let name = exe.strip_suffix(".exe").unwrap_or(&exe).to_string();
            out.push(App { name, id: format!("{}:{}:{}", encode(&title), encode(&class), encode(&exe)) });
        }
        1
    }

    /// The Windows shell's own programs (the desktop, the taskbar, input and search).
    const SHELL: [&str; 6] =
        ["explorer.exe", "textinputhost.exe", "applicationframehost.exe", "shellexperiencehost.exe", "searchhost.exe", "startmenuexperiencehost.exe"];

    pub fn is_own_window(window: &str) -> bool {
        let exe = window.rsplit(':').next().unwrap_or("").replace("#3A", ":").replace("#22", "#").to_lowercase();
        std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())).as_deref() == Some(exe.as_str())
    }

    pub fn hide_window(window: &str) -> bool {
        // libobs's `title:class:exe`, `#3A` for ":" and `#22` for "#" inside each part
        let exe = window.rsplit(':').next().unwrap_or("").replace("#3A", ":").replace("#22", "#").to_lowercase();
        let own = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        SHELL.contains(&exe.as_str()) || own.as_deref() == Some(exe.as_str())
    }

    pub fn running() -> Vec<App> {
        let mut out: Vec<App> = Vec::new();
        unsafe { EnumWindows(Some(each), &mut out as *mut Vec<App> as LPARAM) };
        // one entry per program
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.dedup_by(|a, b| a.name == b.name);
        out
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    pub fn running() -> Vec<super::App> {
        Vec::new()
    }

    pub fn hide_window(_: &str) -> bool {
        false
    }

    pub fn is_own_window(_: &str) -> bool {
        false
    }
}
