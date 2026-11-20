//! A phone camera's USB link, the desktop's end (lightair-live's docs/05 §6, USB; the phone's end
//! is the kit's `lightair_link::usb`). One byte stream between phone and desktop, in frames:
//!
//! ```text
//! frame = channel: u8 | length: u32 big-endian | payload (≤ 1 MiB)
//! 0 · link     both ways        one JSON message: hello, ping
//! 1 · control  both ways        the bytes of a connection to this desktop's phone port (4456),
//!                               piped as they are: the phone runs the same pinned TLS and signed
//!                               /camera WebSocket over it as on Wi-Fi. A zero-length frame
//!                               closes it; the next bytes open a new one
//! 2 · media    phone → desktop  MPEG-TS, the stream it sends over SRT on Wi-Fi
//! ```
//!
//! The phone speaks first (`{"type": "hello", "version": 1, "source", "device"}`); the desktop
//! answers `{"type": "hello", "version": 1, "desktop"}` for a layer it has with that phone
//! paired to it, and closes the link otherwise. Both sides ping every 2 s; 10 s of silence ends
//! the link. Media is taken only while the layer's camera is signed in on the control channel,
//! so a cable alone can't put pictures on a layer.
//!
//! The stream comes from usbmuxd (an iPhone, [`crate::usbmux`]), an Android accessory
//! ([`crate::aoa`]), or a TCP address (`adb forward`, the iOS simulator: [`crate::tcp`]).

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::Feeds;

pub const LINK: u8 = 0;
pub const CONTROL: u8 = 1;
pub const MEDIA: u8 = 2;
/// The port Lightair Live listens on in Camera mode (usbmuxd connects to it; `adb forward` too).
pub const PHONE_PORT: u16 = 4457;
/// Larger frames are an error.
pub const MAX_FRAME: usize = 1 << 20;
const VERSION: u64 = 1;
const PING_EVERY: Duration = Duration::from_secs(2);
const SILENT_FOR: Duration = Duration::from_secs(10);
/// How long the phone has to say hello.
const HELLO_WITHIN: Duration = Duration::from_secs(10);

/// What the app knows, for USB links (media goes through [`Feeds`], as from SRT).
pub trait LinkHost: Feeds {
    /// This desktop's id (the phone checks it's the one it's paired to).
    fn desktop_id(&self) -> String;
    /// The phones' TLS port on this machine, when it's open.
    fn phones_port(&self) -> Option<u16>;
    /// Whether `device` is paired as the camera of a layer `source` that exists.
    fn knows(&self, source: &str, device: &str) -> bool;
    /// Whether the layer's camera is signed in on the control channel now.
    fn signed_in(&self, source: &str) -> bool;
    /// A USB link for `source` came up (`true`) or went (`false`): its media goes this way.
    fn link(&self, source: &str, up: bool);
}

/// One frame's bytes.
pub fn frame(channel: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(5 + payload.len());
    f.push(channel);
    f.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// Reads frames out of a byte stream, however it's chunked.
#[derive(Default)]
pub struct Frames {
    buf: Vec<u8>,
}

impl Frames {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole frame, if there is one; an error for one larger than [`MAX_FRAME`].
    pub fn next_frame(&mut self) -> io::Result<Option<(u8, Vec<u8>)>> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a frame too large"));
        }
        if self.buf.len() < 5 + len {
            return Ok(None);
        }
        let channel = self.buf[0];
        let payload = self.buf[5..5 + len].to_vec();
        self.buf.drain(..5 + len);
        Ok(Some((channel, payload)))
    }
}

fn would_block(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted)
}

/// The control channel's pipe to the phones' port.
struct Pipe {
    stream: TcpStream,
    serial: u64,
}

struct Link {
    out: Sender<Vec<u8>>,
    open: AtomicBool,
    pipe: Mutex<Option<Pipe>>,
    pipe_serial: AtomicU64,
}

impl Link {
    fn send(&self, bytes: Vec<u8>) -> bool {
        self.out.send(bytes).is_ok()
    }

    /// Opens a pipe to the phones' port; its answers go back on the control channel.
    fn open_pipe(self: &Arc<Self>, port: u16) -> io::Result<()> {
        let stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_nodelay(true)?;
        let serial = self.pipe_serial.fetch_add(1, Ordering::AcqRel) + 1;
        let mut read = stream.try_clone()?;
        *self.pipe.lock() = Some(Pipe { stream, serial });
        let me = self.clone();
        std::thread::Builder::new().name("lightair-usb-control".into()).spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match read.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if !me.send(frame(CONTROL, &buf[..n])) {
                            return;
                        }
                    }
                }
            }
            // the desktop's end closed: the phone hears it (unless a newer pipe took over)
            let mut pipe = me.pipe.lock();
            if pipe.as_ref().is_some_and(|p| p.serial == serial) {
                pipe.take();
                drop(pipe);
                me.send(frame(CONTROL, &[]));
            }
        })?;
        Ok(())
    }

    fn close_pipe(&self) {
        if let Some(p) = self.pipe.lock().take() {
            let _ = p.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Runs a link on a connected stream until it ends: `read` and `write` are two handles on it.
/// `what` names it in the log ("iPhone 15 over USB").
pub fn serve<R, W>(mut read: R, write: W, host: Arc<dyn LinkHost>, what: &str) -> io::Result<()>
where
    R: Read,
    W: Write + Send + 'static,
{
    let (tx, rx) = channel::<Vec<u8>>();
    let link =
        Arc::new(Link { out: tx, open: AtomicBool::new(true), pipe: Mutex::new(None), pipe_serial: AtomicU64::new(0) });
    let writer = {
        let link = link.clone();
        std::thread::Builder::new().name("lightair-usb-out".into()).spawn(move || write_loop(write, rx, &link))?
    };

    let mut frames = Frames::default();
    let mut chunk = vec![0u8; 64 * 1024];
    let result = (|| {
        // the phone's hello
        let started = Instant::now();
        let hello = loop {
            if let Some((channel, payload)) = frames.next_frame()? {
                if channel == LINK
                    && let Ok(v) = serde_json::from_slice::<Value>(&payload)
                    && v["type"] == "hello"
                {
                    break v;
                }
                continue;
            }
            if started.elapsed() > HELLO_WITHIN {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the phone didn't say hello"));
            }
            match read.read(&mut chunk) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the phone closed the link")),
                Ok(n) => frames.push(&chunk[..n]),
                Err(e) if would_block(&e) => {}
                Err(e) => return Err(e),
            }
        };
        let source = hello["source"].as_str().unwrap_or_default().to_owned();
        let device = hello["device"].as_str().unwrap_or_default().to_owned();
        if !host.knows(&source, &device) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("no layer {source:?} with this phone paired"),
            ));
        }
        link.send(frame(
            LINK,
            json!({"type": "hello", "version": VERSION, "desktop": host.desktop_id()}).to_string().as_bytes(),
        ));
        tracing::info!(what, source, "phone camera link up");
        host.link(&source, true);

        // pings, and silence
        let heard = Arc::new(Mutex::new(Instant::now()));
        {
            let (link, heard) = (link.clone(), heard.clone());
            std::thread::Builder::new().name("lightair-usb-ping".into()).spawn(move || {
                while link.open.load(Ordering::Acquire) {
                    if heard.lock().elapsed() > SILENT_FOR {
                        link.open.store(false, Ordering::Release);
                        break;
                    }
                    link.send(frame(LINK, br#"{"type":"ping"}"#));
                    std::thread::sleep(PING_EVERY);
                }
            })?;
        }

        let out = UdpSocket::bind(("127.0.0.1", 0))?;
        let mut media_to: Option<Option<SocketAddr>> = None;
        let mut dump: Option<std::fs::File> = None;
        let ended = loop {
            while let Some((channel, payload)) = frames.next_frame()? {
                *heard.lock() = Instant::now();
                match channel {
                    CONTROL if payload.is_empty() => link.close_pipe(),
                    CONTROL => {
                        if link.pipe.lock().is_none() {
                            let port =
                                host.phones_port().ok_or_else(|| io::Error::other("the phones' port isn't open"))?;
                            // (refused: the phone hears its connection close, and tries again)
                            if link.open_pipe(port).is_err() {
                                link.send(frame(CONTROL, &[]));
                                continue;
                            }
                        }
                        let mut pipe = link.pipe.lock();
                        if let Some(p) = pipe.as_mut()
                            && p.stream.write_all(&payload).is_err()
                        {
                            pipe.take();
                            drop(pipe);
                            link.send(frame(CONTROL, &[]));
                        }
                    }
                    MEDIA => {
                        if !host.signed_in(&source) {
                            continue;
                        }
                        if media_to.is_none() {
                            dump = crate::dump_file(&source);
                        }
                        let to = *media_to
                            .get_or_insert_with(|| host.started(&source, "127.0.0.1:0".parse().expect("an address")));
                        if let Some(f) = dump.as_mut() {
                            let _ = f.write_all(&payload);
                        }
                        if let Some(to) = to {
                            // in datagrams of seven packets, as SRT carries them (macOS takes no
                            // datagram over 9,216 bytes: a frame of 56 packets would be dropped)
                            for d in payload.chunks(188 * 7) {
                                let _ = out.send_to(d, to);
                            }
                        }
                        host.data(&source, payload.len());
                    }
                    // pings, and anything newer
                    _ => {}
                }
            }
            if !link.open.load(Ordering::Acquire) {
                break Ok(());
            }
            match read.read(&mut chunk) {
                Ok(0) => break Ok(()),
                Ok(n) => frames.push(&chunk[..n]),
                Err(e) if would_block(&e) => {}
                Err(e) => break Err(e),
            }
        };
        tracing::info!(what, source, "phone camera link down");
        host.link(&source, false);
        if media_to.is_some() {
            host.ended(&source);
        }
        ended
    })();
    link.open.store(false, Ordering::Release);
    link.close_pipe();
    drop(link);
    let _ = writer.join();
    result
}

fn write_loop<W: Write>(mut w: W, rx: Receiver<Vec<u8>>, link: &Link) {
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(bytes) => {
                if w.write_all(&bytes).is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if !link.open.load(Ordering::Acquire) {
                    break;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // what's waiting, then one flush
        loop {
            match rx.try_recv() {
                Ok(bytes) => {
                    if w.write_all(&bytes).is_err() {
                        link.open.store(false, Ordering::Release);
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if w.flush().is_err() {
            break;
        }
    }
    link.open.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_come_out_however_the_bytes_are_chunked() {
        let mut bytes = frame(LINK, br#"{"type":"hello"}"#);
        bytes.extend(frame(MEDIA, &[0x47; 188 * 7]));
        bytes.extend(frame(CONTROL, &[]));
        let mut got = vec![];
        let mut f = Frames::default();
        for piece in bytes.chunks(3) {
            f.push(piece);
            while let Some(x) = f.next_frame().unwrap() {
                got.push(x);
            }
        }
        assert_eq!(got.len(), 3);
        assert_eq!((got[0].0, got[1].0, got[1].1.len(), got[2].0, got[2].1.len()), (LINK, MEDIA, 188 * 7, CONTROL, 0));
    }

    #[test]
    fn a_frame_too_large_is_an_error() {
        let mut f = Frames::default();
        f.push(&[MEDIA, 0x7f, 0, 0, 0]);
        assert!(f.next_frame().is_err());
    }
}
