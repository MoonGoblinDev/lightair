//! Devices as video sources.

/// Lets an iPhone or iPad connected by USB appear as a video device (its screen), as QuickTime
/// does (macOS: CoreMediaIO's screen capture devices; no permission involved). Call once, early.
/// Elsewhere, nothing to do.
pub fn allow_ios_screens() {
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct Address {
            selector: u32,
            scope: u32,
            element: u32,
        }
        #[link(name = "CoreMediaIO", kind = "framework")]
        unsafe extern "C" {
            fn CMIOObjectSetPropertyData(
                object: u32,
                address: *const Address,
                qualifier_size: u32,
                qualifier: *const std::ffi::c_void,
                size: u32,
                data: *const std::ffi::c_void,
            ) -> i32;
        }
        // kCMIOHardwarePropertyAllowScreenCaptureDevices ('yes '), global scope ('glob'),
        // the main element, on the system object (1)
        let address = Address { selector: 0x7965_7320, scope: 0x676C_6F62, element: 0 };
        let allow: u32 = 1;
        let status = unsafe {
            CMIOObjectSetPropertyData(1, &address, 0, std::ptr::null(), 4, &allow as *const u32 as *const std::ffi::c_void)
        };
        if status != 0 {
            tracing::warn!(status, "iPhone and iPad screens won't show as devices");
        }
    }
}

/// Watches cameras for the app's whole life (macOS): keeps a discovery session of every kind of
/// camera open, so an iPhone (Continuity Camera) that went away is offered again when it comes
/// back, and calls `connected(unique id)` each time a camera or a phone's screen arrives.
/// Elsewhere, nothing to do. Call once, on the main thread, after `allow_ios_screens`.
pub fn watch_cameras(connected: impl Fn(String) + 'static) {
    #[cfg(target_os = "macos")]
    mac::watch(connected);
    #[cfg(not(target_os = "macos"))]
    let _ = connected;
}

/// The iPhones and iPads connected by cable whose screens can be shown: (name, unique id).
/// macOS only (`allow_ios_screens` first); a phone shows here once it's unlocked and trusts
/// this Mac. Elsewhere, none.
pub fn ios_screens() -> Vec<(String, String)> {
    #[cfg(target_os = "macos")]
    return mac::ios_screens();
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSArray, NSString};

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVCaptureDeviceWasConnectedNotification: &'static NSString;
        static AVCaptureDeviceTypeBuiltInWideAngleCamera: &'static NSString;
        static AVCaptureDeviceTypeExternal: &'static NSString;
        static AVCaptureDeviceTypeContinuityCamera: &'static NSString;
        static AVCaptureDeviceTypeDeskViewCamera: &'static NSString;
        static AVMediaTypeVideo: &'static NSString;
        static AVMediaTypeMuxed: &'static NSString;
    }

    /// The cameras of every kind (built in, plugged in, iPhones) for `media`, as a session.
    fn session(media: &NSString) -> Option<Retained<AnyObject>> {
        let class = AnyClass::get(c"AVCaptureDeviceDiscoverySession")?;
        unsafe {
            let types = NSArray::from_slice(&[
                AVCaptureDeviceTypeBuiltInWideAngleCamera,
                AVCaptureDeviceTypeExternal,
                AVCaptureDeviceTypeContinuityCamera,
                AVCaptureDeviceTypeDeskViewCamera,
            ]);
            // position: unspecified (0)
            msg_send![class, discoverySessionWithDeviceTypes: &*types, mediaType: media, position: 0isize]
        }
    }

    fn string(s: &NSString) -> String {
        s.to_string()
    }

    pub fn watch(connected: impl Fn(String) + 'static) {
        // the sessions stay open: macOS offers an iPhone that comes back to apps looking
        unsafe {
            for media in [AVMediaTypeVideo, AVMediaTypeMuxed] {
                if let Some(s) = session(media) {
                    std::mem::forget(s);
                }
            }
        }
        let block = RcBlock::new(move |note: NonNull<AnyObject>| {
            let device: Option<Retained<AnyObject>> = unsafe { msg_send![note.as_ref(), object] };
            let Some(device) = device else { return };
            let uid: Option<Retained<NSString>> = unsafe { msg_send![&*device, uniqueID] };
            if let Some(uid) = uid {
                connected(string(&uid));
            }
        });
        unsafe {
            let Some(centre) = AnyClass::get(c"NSNotificationCenter") else { return };
            let centre: Retained<AnyObject> = msg_send![centre, defaultCenter];
            let main: Option<Retained<AnyObject>> = AnyClass::get(c"NSOperationQueue").and_then(|q| msg_send![q, mainQueue]);
            let observer: Option<Retained<AnyObject>> = msg_send![
                &*centre,
                addObserverForName: AVCaptureDeviceWasConnectedNotification,
                object: std::ptr::null::<AnyObject>(),
                queue: main.as_deref(),
                usingBlock: &*block
            ];
            // (observing for the app's life)
            std::mem::forget(observer);
        }
    }

    pub fn ios_screens() -> Vec<(String, String)> {
        let Some(s) = (unsafe { session(AVMediaTypeMuxed) }) else { return Vec::new() };
        let devices: Retained<NSArray<AnyObject>> = unsafe { msg_send![&*s, devices] };
        let n: usize = unsafe { msg_send![&*devices, count] };
        (0..n)
            .filter_map(|i| unsafe {
                let d: Retained<AnyObject> = msg_send![&*devices, objectAtIndex: i];
                // a phone's or tablet's screen: muxed, its model "iOS Device"
                let model: Option<Retained<NSString>> = msg_send![&*d, modelID];
                let name: Option<Retained<NSString>> = msg_send![&*d, localizedName];
                let uid: Option<Retained<NSString>> = msg_send![&*d, uniqueID];
                let ios = model.as_deref().map(string).is_none_or(|m| m.contains("iOS"));
                match (ios, name, uid) {
                    (true, Some(name), Some(uid)) => Some((string(&name), string(&uid))),
                    _ => None,
                }
            })
            .collect()
    }
}

/// Whether a camera is a virtual one (a camera made by an app: OBS's, a meeting app's…) rather
/// than hardware. macOS: its transport type (`kIOAudioDeviceTransportTypeVirtual`, as
/// AVFoundation reports it); elsewhere, not known (false). `uid` is the device's unique id.
pub fn camera_is_virtual(uid: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::rc::Retained;
        use objc2::runtime::{AnyClass, AnyObject};
        use objc2_foundation::NSString;
        #[link(name = "AVFoundation", kind = "framework")]
        unsafe extern "C" {}
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else { return false };
        let id = NSString::from_str(uid);
        unsafe {
            let device: Option<Retained<AnyObject>> = msg_send![class, deviceWithUniqueID: &*id];
            // 'virt'
            device.is_some_and(|d| {
                let transport: i32 = msg_send![&*d, transportType];
                transport == 0x7669_7274
            })
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = uid;
        false
    }
}
