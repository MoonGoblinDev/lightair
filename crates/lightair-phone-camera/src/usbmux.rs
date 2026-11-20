//! iPhones over USB through usbmuxd (Apple's USB multiplexer: built into macOS at
//! `/var/run/usbmuxd`; on Windows Apple's Mobile Device Service at `127.0.0.1:27015`). The same
//! mechanism Xcode uses: no developer setting on the phone, which only has to trust this computer
//! once.
//!
//! The protocol: 16-byte little-endian headers (length, version 1, message 8 = property list,
//! tag) and XML property lists. `Listen` reports `Attached` and `Detached` devices; `Connect`
//! (a device id, a port in network byte order) turns the socket into a stream to that port on
//! the phone. For each attached iPhone the watcher keeps a link to Lightair Live's port 4457.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use plist::{Dictionary, Value};

use crate::keep::{Halves, Stop, keep};
use crate::link::{LinkHost, PHONE_PORT};

/// A connection to usbmuxd.
pub trait Stream: Read + Write + Send + 'static {
    fn try_clone_box(&self) -> io::Result<Box<dyn Stream>>;
    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()>;
}

#[cfg(unix)]
impl Stream for std::os::unix::net::UnixStream {
    fn try_clone_box(&self) -> io::Result<Box<dyn Stream>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, t)
    }
}

impl Stream for std::net::TcpStream {
    fn try_clone_box(&self) -> io::Result<Box<dyn Stream>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, t)
    }
}

/// Where usbmuxd is: `USBMUXD_SOCKET_ADDRESS` (`host:port` or `UNIX:/path`, as libimobiledevice
/// reads it), else the platform's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    #[cfg(unix)]
    Unix(std::path::PathBuf),
    Tcp(String),
}

impl Address {
    pub fn system() -> Address {
        if let Ok(a) = std::env::var("USBMUXD_SOCKET_ADDRESS") {
            #[cfg(unix)]
            if let Some(p) = a.strip_prefix("UNIX:") {
                return Address::Unix(p.into());
            }
            return Address::Tcp(a);
        }
        #[cfg(unix)]
        return Address::Unix("/var/run/usbmuxd".into());
        #[cfg(not(unix))]
        return Address::Tcp("127.0.0.1:27015".into());
    }

    pub fn connect(&self) -> io::Result<Box<dyn Stream>> {
        match self {
            #[cfg(unix)]
            Address::Unix(p) => Ok(Box::new(std::os::unix::net::UnixStream::connect(p)?)),
            Address::Tcp(a) => Ok(Box::new(std::net::TcpStream::connect(a)?)),
        }
    }
}

const PLIST: u32 = 8;

/// Sends one message.
pub fn send(w: &mut impl Write, tag: u32, message: Dictionary) -> io::Result<()> {
    let mut body = Vec::new();
    Value::Dictionary(message).to_writer_xml(&mut body).map_err(io::Error::other)?;
    let mut head = Vec::with_capacity(16);
    for v in [16 + body.len() as u32, 1, PLIST, tag] {
        head.extend_from_slice(&v.to_le_bytes());
    }
    w.write_all(&head)?;
    w.write_all(&body)?;
    w.flush()
}

/// Reads one message: its tag and property list.
pub fn receive(r: &mut impl Read) -> io::Result<(u32, Dictionary)> {
    let mut head = [0u8; 16];
    r.read_exact(&mut head)?;
    let word = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
    let len = word(0) as usize;
    if !(16..=1 << 20).contains(&len) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "usbmuxd sent a bad length"));
    }
    let mut body = vec![0u8; len - 16];
    r.read_exact(&mut body)?;
    let v = Value::from_reader(io::Cursor::new(body)).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let d = v.into_dictionary().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a dictionary"))?;
    Ok((word(12), d))
}

fn request(kind: &str) -> Dictionary {
    let mut d = Dictionary::new();
    d.insert("MessageType".into(), kind.into());
    d.insert("ClientVersionString".into(), "lightair".into());
    d.insert("ProgName".into(), "Lightair".into());
    d.insert("kLibUSBMuxVersion".into(), 3u64.into());
    d
}

fn result_number(d: &Dictionary) -> Option<u64> {
    (d.get("MessageType").and_then(Value::as_string) == Some("Result"))
        .then(|| d.get("Number").and_then(Value::as_unsigned_integer))
        .flatten()
}

/// A stream to `port` on the device usbmuxd knows as `device`.
pub fn connect(address: &Address, device: u64, port: u16) -> io::Result<Box<dyn Stream>> {
    let mut s = address.connect()?;
    let mut d = request("Connect");
    d.insert("DeviceID".into(), device.into());
    // (the port in network byte order, as a number)
    d.insert("PortNumber".into(), (port.swap_bytes() as u64).into());
    send(&mut s, 1, d)?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let (_, answer) = receive(&mut s)?;
    match result_number(&answer) {
        Some(0) => Ok(s),
        Some(3) => Err(io::Error::new(io::ErrorKind::ConnectionRefused, "nothing listening on the phone")),
        n => Err(io::Error::other(format!("usbmuxd refused the connection ({n:?})"))),
    }
}

/// An attached device: (usbmuxd's id, its serial number), USB only.
fn attached(d: &Dictionary) -> Option<(u64, String)> {
    if d.get("MessageType").and_then(Value::as_string) != Some("Attached") {
        return None;
    }
    let props = d.get("Properties").and_then(Value::as_dictionary)?;
    if props.get("ConnectionType").and_then(Value::as_string) != Some("USB") {
        return None;
    }
    let id = props.get("DeviceID").or(d.get("DeviceID")).and_then(Value::as_unsigned_integer)?;
    let serial = props.get("SerialNumber").and_then(Value::as_string).unwrap_or("an iPhone").to_owned();
    Some((id, serial))
}

fn detached(d: &Dictionary) -> Option<u64> {
    (d.get("MessageType").and_then(Value::as_string) == Some("Detached"))
        .then(|| d.get("DeviceID").and_then(Value::as_unsigned_integer))
        .flatten()
}

/// Keeps links to attached iPhones while it lives.
pub struct Watcher {
    _stop: Stop,
}

impl Watcher {
    pub fn start(address: Address, host: Arc<dyn LinkHost>) -> io::Result<Watcher> {
        let stop = Stop::new();
        let flag = stop.0.clone();
        std::thread::Builder::new().name("lightair-usbmux".into()).spawn(move || {
            let mut quiet = false;
            while !flag.load(Ordering::Acquire) {
                if let Err(e) = listen(&address, &flag, &host)
                    && !quiet
                {
                    tracing::info!(%e, "usbmuxd isn't answering (no iPhones over USB)");
                    quiet = true;
                }
                for _ in 0..50 {
                    if flag.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })?;
        Ok(Watcher { _stop: stop })
    }
}

/// Listens for devices until usbmuxd goes or `stop`; a link kept for each attached iPhone.
fn listen(address: &Address, stop: &Arc<AtomicBool>, host: &Arc<dyn LinkHost>) -> io::Result<()> {
    let mut s = address.connect()?;
    send(&mut s, 0, request("Listen"))?;
    s.set_read_timeout(Some(Duration::from_millis(500)))?;
    let mut devices: HashMap<u64, Stop> = HashMap::new();
    let mut partial: Vec<u8> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        // (read with a timeout, so stopping is noticed: whole messages from what's come)
        let mut buf = [0u8; 4096];
        match s.read(&mut buf) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "usbmuxd closed")),
            Ok(n) => partial.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(e),
        }
        while partial.len() >= 16 {
            let len = u32::from_le_bytes([partial[0], partial[1], partial[2], partial[3]]) as usize;
            if partial.len() < len {
                break;
            }
            let (_, d) = receive(&mut io::Cursor::new(partial[..len].to_vec()))?;
            partial.drain(..len);
            if let Some((id, serial)) = attached(&d) {
                tracing::info!(serial, "an iPhone is attached by USB");
                let device = Stop::new();
                let addr = address.clone();
                keep(format!("the iPhone {serial} over USB"), device.0.clone(), host.clone(), move || {
                    let s = connect(&addr, id, PHONE_PORT)?;
                    s.set_read_timeout(Some(Duration::from_millis(200)))?;
                    let read = s.try_clone_box()?;
                    Ok((Box::new(read) as Box<dyn Read + Send>, Box::new(s) as Box<dyn Write + Send>) as Halves)
                })?;
                devices.insert(id, device);
            } else if let Some(id) = detached(&d) {
                devices.remove(&id);
            }
        }
    }
    Ok(())
}
