//! The USB link's desktop end against a stand-in phone (as Lightair Live's `lightair_link::usb`
//! speaks): hello both ways, the control channel piped to the phones' port and back, media taken
//! only while the layer's camera is signed in, and a phone the desktop doesn't know turned away.
//! Then the same through a stand-in usbmuxd, as an attached iPhone.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lightair_phone_camera::link::{CONTROL, Frames, LINK, LinkHost, MEDIA, frame};
use lightair_phone_camera::{Feeds, tcp};
use parking_lot::Mutex;
use serde_json::{Value, json};

struct Desk {
    phones_port: u16,
    decoder: SocketAddr,
    signed_in: AtomicBool,
    log: Mutex<Vec<String>>,
}

impl Feeds for Desk {
    fn passphrase(&self, _: &str) -> Option<String> {
        None
    }
    fn started(&self, source: &str, _: SocketAddr) -> Option<SocketAddr> {
        self.log.lock().push(format!("started {source}"));
        Some(self.decoder)
    }
    fn data(&self, _: &str, _: usize) {}
    fn ended(&self, source: &str) {
        self.log.lock().push(format!("ended {source}"));
    }
}

impl LinkHost for Desk {
    fn desktop_id(&self) -> String {
        "desk-1".into()
    }
    fn phones_port(&self) -> Option<u16> {
        Some(self.phones_port)
    }
    fn knows(&self, source: &str, device: &str) -> bool {
        source == "cam-1" && device == "phone-1"
    }
    fn signed_in(&self, _: &str) -> bool {
        self.signed_in.load(Ordering::Acquire)
    }
    fn link(&self, source: &str, up: bool) {
        self.log.lock().push(format!("link {source} {up}"));
    }
}

/// The phones' port: echoes what comes, uppercased.
fn phones_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut buf = [0u8; 1024];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let up: Vec<u8> = buf[..n].to_ascii_uppercase();
                    if s.write_all(&up).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

/// A stand-in phone on a stream.
struct Phone {
    s: TcpStream,
    frames: Frames,
}

impl Phone {
    fn send(&mut self, channel: u8, payload: &[u8]) {
        self.s.write_all(&frame(channel, payload)).unwrap();
    }

    /// The next frame on a channel (pings skipped), within a few seconds.
    fn next(&mut self, channel: u8) -> Option<Vec<u8>> {
        let until = Instant::now() + Duration::from_secs(4);
        let mut buf = [0u8; 4096];
        while Instant::now() < until {
            while let Some((c, p)) = self.frames.next_frame().unwrap() {
                if c == channel && !(c == LINK && p == br#"{"type":"ping"}"#) {
                    return Some(p);
                }
            }
            match self.s.read(&mut buf) {
                Ok(0) => return None,
                Ok(n) => self.frames.push(&buf[..n]),
                Err(_) => {}
            }
        }
        None
    }
}

fn desk() -> (Arc<Desk>, UdpSocket) {
    let decoder = UdpSocket::bind("127.0.0.1:0").unwrap();
    decoder.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let desk = Arc::new(Desk {
        phones_port: phones_port(),
        decoder: decoder.local_addr().unwrap(),
        signed_in: AtomicBool::new(false),
        log: Mutex::new(vec![]),
    });
    (desk, decoder)
}

fn wait_for(what: impl Fn() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if what() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// The phone listens (as in Camera mode); the desktop's watcher connects to it.
fn phone_listening() -> (TcpListener, SocketAddr) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap();
    (l, a)
}

fn accept(l: &TcpListener) -> Phone {
    let (s, _) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    Phone { s, frames: Frames::default() }
}

#[test]
fn a_phone_over_a_link() {
    let (desk, decoder) = desk();
    let (listener, addr) = phone_listening();
    let _watcher = tcp::Watcher::start(&[addr], desk.clone()).unwrap();
    let mut phone = accept(&listener);

    phone.send(
        LINK,
        json!({"type": "hello", "version": 1, "source": "cam-1", "device": "phone-1"}).to_string().as_bytes(),
    );
    let hello: Value = serde_json::from_slice(&phone.next(LINK).expect("the desktop's hello")).unwrap();
    assert_eq!(hello, json!({"type": "hello", "version": 1, "desktop": "desk-1"}));
    assert!(wait_for(|| desk.log.lock().contains(&"link cam-1 true".to_string())));

    // the control channel reaches the phones' port and comes back
    phone.send(CONTROL, b"client hello");
    assert_eq!(phone.next(CONTROL).as_deref(), Some(&b"CLIENT HELLO"[..]));
    // closed, then a new connection
    phone.send(CONTROL, &[]);
    phone.send(CONTROL, b"again");
    assert_eq!(phone.next(CONTROL).as_deref(), Some(&b"AGAIN"[..]));

    // media before the camera signed in: dropped
    let packets = vec![0x47u8; 188 * 7];
    phone.send(MEDIA, &packets);
    let mut buf = [0u8; 4096];
    decoder.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    assert!(decoder.recv(&mut buf).is_err(), "media taken before the camera signed in");
    // signed in: to the decoder
    desk.signed_in.store(true, Ordering::Release);
    phone.send(MEDIA, &packets);
    decoder.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    assert_eq!(decoder.recv(&mut buf).unwrap(), 188 * 7);

    // a full frame (56 packets, as the phone sends at a high bitrate) arrives whole, in
    // datagrams small enough for any system's loopback
    let big: Vec<u8> = (0..188 * 56).map(|i| if i % 188 == 0 { 0x47 } else { (i % 251) as u8 }).collect();
    phone.send(MEDIA, &big);
    let mut got = Vec::new();
    while got.len() < big.len() {
        let n = decoder.recv(&mut buf).expect("the whole frame at the decoder");
        assert!(n <= 1316, "a datagram of {n} bytes");
        got.extend_from_slice(&buf[..n]);
    }
    assert_eq!(got, big);

    // the phone leaves
    drop(phone);
    assert!(wait_for(|| {
        let l = desk.log.lock();
        l.contains(&"link cam-1 false".to_string()) && l.contains(&"ended cam-1".to_string())
    }));
}

#[test]
fn a_phone_the_desktop_doesnt_know_is_turned_away() {
    let (desk, _decoder) = desk();
    let (listener, addr) = phone_listening();
    let _watcher = tcp::Watcher::start(&[addr], desk.clone()).unwrap();
    let mut phone = accept(&listener);
    phone.send(
        LINK,
        json!({"type": "hello", "version": 1, "source": "cam-1", "device": "stranger"}).to_string().as_bytes(),
    );
    assert_eq!(phone.next(LINK), None, "a hello for a phone that isn't paired");
    assert!(!desk.log.lock().iter().any(|l| l.starts_with("link")));
}

#[cfg(unix)]
#[test]
fn an_iphone_through_usbmuxd() {
    use lightair_phone_camera::usbmux::{self, Address};
    use plist::{Dictionary, Value as P};
    use std::os::unix::net::UnixListener;

    let (desk, _decoder) = desk();
    let path = std::env::temp_dir().join(format!("lightair-usbmuxd-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let muxd = UnixListener::bind(&path).unwrap();
    // the phone, behind the stand-in usbmuxd
    let (phone_l, phone_addr) = phone_listening();
    std::thread::spawn(move || {
        for s in muxd.incoming().flatten() {
            let mut s = s;
            let Ok((tag, d)) = usbmux::receive(&mut s) else { continue };
            let kind = d.get("MessageType").and_then(P::as_string).unwrap_or("").to_owned();
            let result = |n: u64| {
                let mut r = Dictionary::new();
                r.insert("MessageType".into(), "Result".into());
                r.insert("Number".into(), n.into());
                r
            };
            if kind == "Listen" {
                usbmux::send(&mut s, tag, result(0)).unwrap();
                let mut props = Dictionary::new();
                props.insert("ConnectionType".into(), "USB".into());
                props.insert("DeviceID".into(), 7u64.into());
                props.insert("SerialNumber".into(), "00008110-TEST".into());
                let mut a = Dictionary::new();
                a.insert("MessageType".into(), "Attached".into());
                a.insert("DeviceID".into(), 7u64.into());
                a.insert("Properties".into(), P::Dictionary(props));
                usbmux::send(&mut s, 0, a).unwrap();
                // (kept open: devices come and go on it)
                std::thread::spawn(move || {
                    let _s = s;
                    std::thread::sleep(Duration::from_secs(30));
                });
            } else if kind == "Connect" {
                let device = d.get("DeviceID").and_then(P::as_unsigned_integer);
                let port = d.get("PortNumber").and_then(P::as_unsigned_integer).map(|p| (p as u16).swap_bytes());
                assert_eq!((device, port), (Some(7), Some(4457)));
                usbmux::send(&mut s, tag, result(0)).unwrap();
                // from here the socket is the phone's port: piped to the stand-in phone
                let phone = TcpStream::connect(phone_addr).unwrap();
                let (mut a, mut b) = (s.try_clone().unwrap(), phone.try_clone().unwrap());
                let (mut c, mut d) = (phone, s);
                std::thread::spawn(move || std::io::copy(&mut a, &mut b));
                std::thread::spawn(move || std::io::copy(&mut c, &mut d));
            }
        }
    });
    let _watcher = usbmux::Watcher::start(Address::Unix(path.clone()), desk.clone()).unwrap();
    let mut phone = accept(&phone_l);
    phone.send(
        LINK,
        json!({"type": "hello", "version": 1, "source": "cam-1", "device": "phone-1"}).to_string().as_bytes(),
    );
    let hello: Value = serde_json::from_slice(&phone.next(LINK).expect("the desktop's hello through usbmuxd")).unwrap();
    assert_eq!(hello["desktop"], "desk-1");
    assert!(wait_for(|| desk.log.lock().contains(&"link cam-1 true".to_string())));
    let _ = std::fs::remove_file(&path);
}
