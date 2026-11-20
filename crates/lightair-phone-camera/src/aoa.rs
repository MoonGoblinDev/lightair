//! Android phones over USB as accessories (AOA 2; lightair-live's docs/05 §6): no developer
//! mode. The desktop, as USB host, asks a phone for the accessory protocol (request 51), sends its
//! strings (52) and starts accessory mode (53); the phone comes back as Google's accessory device
//! (18d1:2d00–2d05) and offers Lightair Live, which opens it. Its first interface's two bulk
//! endpoints are then the link.
//!
//! Through nusb (pure Rust, MIT/Apache), not libusb. A phone is switched only when the app says a
//! layer has an Android phone paired, and once per plug: switching makes the phone ask to open
//! Lightair Live, which shouldn't happen to every phone that charges here. On Windows a phone's
//! default driver doesn't let other apps send it requests, so this is macOS and Linux for now.

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Duration;

use crate::keep::Stop;
use crate::link::LinkHost;

/// The accessory strings (Lightair Live's accessory filter matches the first two).
pub const MANUFACTURER: &str = "Lightair";
pub const MODEL: &str = "Lightair Camera";
pub const DESCRIPTION: &str = "Lightair Live camera";
pub const VERSION: &str = "1";
pub const URI: &str = "https://lightair.app/camera";

/// Google's accessory device ids (accessory, accessory + ADB, and the audio variants).
pub const GOOGLE: u16 = 0x18d1;
pub const ACCESSORY_PIDS: [u16; 6] = [0x2d00, 0x2d01, 0x2d02, 0x2d03, 0x2d04, 0x2d05];

/// Vendors that make Android phones: only their devices, or devices showing an Android
/// interface (ADB, MTP), are asked for the accessory protocol.
const ANDROID_VENDORS: &[u16] = &[
    0x18d1, // Google
    0x04e8, // Samsung
    0x2717, // Xiaomi
    0x2a70, // OnePlus
    0x22b8, // Motorola
    0x1004, // LG
    0x12d1, // Huawei
    0x0fce, // Sony
    0x22d9, // OPPO
    0x2d95, // vivo
    0x0b05, // ASUS
    0x2b4c, // Nothing
    0x0bb4, // HTC
    0x17ef, // Lenovo
    0x19d2, // ZTE
    0x05c6, // Qualcomm (many phones)
    0x0e8d, // MediaTek (many phones)
    0x1949, // Amazon
    0x2ae5, // Fairphone
];

/// Whether a device is in accessory mode.
pub fn is_accessory(vendor: u16, product: u16) -> bool {
    vendor == GOOGLE && ACCESSORY_PIDS.contains(&product)
}

/// Whether a device may be an Android phone worth asking: an Android vendor, or an ADB
/// (ff/42/01) or MTP/PTP (06/01) interface.
pub fn maybe_android(vendor: u16, interfaces: &[(u8, u8, u8)]) -> bool {
    ANDROID_VENDORS.contains(&vendor)
        || interfaces.iter().any(|&(c, s, p)| (c, s, p) == (0xff, 0x42, 0x01) || (c, s) == (0x06, 0x01))
}

/// Keeps links to Android phones in accessory mode, and switches phones into it while
/// `switch()` says so.
pub struct Watcher {
    _stop: Stop,
}

impl Watcher {
    /// `serial`: the accessory serial phones are given (this desktop's id).
    pub fn start(
        host: Arc<dyn LinkHost>,
        serial: String,
        switch: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::io::Result<Watcher> {
        let stop = Stop::new();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        imp::run(stop.0.clone(), host, serial, switch)?;
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = (host, serial, switch);
        Ok(Watcher { _stop: stop })
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod imp {
    use std::io;
    use std::sync::atomic::Ordering;

    use nusb::MaybeFuture;
    use nusb::descriptors::TransferType;
    use nusb::transfer::{Bulk, ControlIn, ControlOut, ControlType, Direction, In, Out, Recipient};

    use super::*;
    use crate::keep::{Halves, keep};

    const TIMEOUT: Duration = Duration::from_secs(1);

    pub fn run(
        stop: Arc<std::sync::atomic::AtomicBool>,
        host: Arc<dyn LinkHost>,
        serial: String,
        switch: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> io::Result<()> {
        std::thread::Builder::new().name("lightair-aoa".into()).spawn(move || {
            // accessories already linked, and phones already asked (by location: once per plug)
            let mut linked: HashSet<String> = HashSet::new();
            let mut asked: HashSet<String> = HashSet::new();
            let mut links: Vec<(String, Stop)> = vec![];
            while !stop.load(Ordering::Acquire) {
                let devices: Vec<nusb::DeviceInfo> =
                    nusb::list_devices().wait().map(|d| d.collect()).unwrap_or_default();
                let present: HashSet<String> = devices.iter().map(key).collect();
                linked.retain(|k| present.contains(k));
                asked.retain(|k| present.contains(k));
                links.retain(|(k, _)| present.contains(k));
                for info in &devices {
                    let k = key(info);
                    if is_accessory(info.vendor_id(), info.product_id()) {
                        if linked.insert(k.clone()) {
                            let what = format!("the Android phone {} over USB", info.serial_number().unwrap_or(""));
                            let device_stop = Stop::new();
                            let info = info.clone();
                            if keep(what, device_stop.0.clone(), host.clone(), move || open_accessory(&info)).is_ok() {
                                links.push((k, device_stop));
                            }
                        }
                        continue;
                    }
                    let interfaces: Vec<(u8, u8, u8)> =
                        info.interfaces().map(|i| (i.class(), i.subclass(), i.protocol())).collect();
                    if !maybe_android(info.vendor_id(), &interfaces) || asked.contains(&k) || !switch() {
                        continue;
                    }
                    asked.insert(k);
                    match start_accessory(info, &serial) {
                        Ok(true) => tracing::info!(
                            product = info.product_string().unwrap_or(""),
                            "asked an Android phone to be Lightair's accessory"
                        ),
                        Ok(false) => {}
                        Err(e) => tracing::debug!(%e, "a USB device didn't take the accessory request"),
                    }
                }
                for _ in 0..20 {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })?;
        Ok(())
    }

    fn key(info: &nusb::DeviceInfo) -> String {
        format!("{:?}", info.id())
    }

    /// Switches a phone into accessory mode: false when it doesn't speak AOA 2.
    fn start_accessory(info: &nusb::DeviceInfo, serial: &str) -> io::Result<bool> {
        let device = info.open().wait().map_err(io::Error::other)?;
        let version = device
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 51,
                    value: 0,
                    index: 0,
                    length: 2,
                },
                TIMEOUT,
            )
            .wait()
            .map_err(io::Error::other)?;
        let version = version.get(..2).map_or(0, |v| u16::from_le_bytes([v[0], v[1]]));
        if version < 1 {
            return Ok(false);
        }
        for (index, s) in [MANUFACTURER, MODEL, DESCRIPTION, VERSION, URI, serial].into_iter().enumerate() {
            let mut data = s.as_bytes().to_vec();
            data.push(0);
            device
                .control_out(
                    ControlOut {
                        control_type: ControlType::Vendor,
                        recipient: Recipient::Device,
                        request: 52,
                        value: 0,
                        index: index as u16,
                        data: &data,
                    },
                    TIMEOUT,
                )
                .wait()
                .map_err(io::Error::other)?;
        }
        device
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 53,
                    value: 0,
                    index: 0,
                    data: &[],
                },
                TIMEOUT,
            )
            .wait()
            .map_err(io::Error::other)?;
        Ok(true)
    }

    /// The accessory's bulk endpoints as a stream (reads in 16 KiB transfers or more: Android's
    /// accessory driver needs them that large).
    fn open_accessory(info: &nusb::DeviceInfo) -> io::Result<Halves> {
        let device = info.open().wait().map_err(io::Error::other)?;
        let interface = device.claim_interface(0).wait().map_err(io::Error::other)?;
        let desc = interface.descriptor().ok_or_else(|| io::Error::other("the accessory has no interface"))?;
        let bulk = |dir: Direction| {
            desc.endpoints()
                .find(|e| e.transfer_type() == TransferType::Bulk && e.direction() == dir)
                .map(|e| e.address())
        };
        let (Some(inp), Some(outp)) = (bulk(Direction::In), bulk(Direction::Out)) else {
            return Err(io::Error::other("the accessory has no bulk endpoints"));
        };
        let mut reader =
            interface.endpoint::<Bulk, In>(inp).map_err(io::Error::other)?.reader(64 * 1024).with_num_transfers(4);
        reader.set_read_timeout(Duration::from_millis(200));
        let writer =
            interface.endpoint::<Bulk, Out>(outp).map_err(io::Error::other)?.writer(64 * 1024).with_num_transfers(4);
        Ok((Box::new(reader), Box::new(writer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_devices_are_asked() {
        assert!(is_accessory(0x18d1, 0x2d01));
        assert!(!is_accessory(0x18d1, 0x4ee7));
        assert!(maybe_android(0x04e8, &[]));
        assert!(maybe_android(0x1234, &[(0xff, 0x42, 0x01)]));
        assert!(maybe_android(0x1234, &[(0x06, 0x01, 0x01)]));
        // a keyboard, a hub
        assert!(!maybe_android(0x05ac, &[(0x03, 0x01, 0x01)]));
        assert!(!maybe_android(0x2109, &[(0x09, 0x00, 0x00)]));
    }
}
