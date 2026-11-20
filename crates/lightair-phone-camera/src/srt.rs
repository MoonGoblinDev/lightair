//! The SRT listener phones send to (lightair-live's docs/05 §6): one port for every phone camera,
//! the stream id naming the source, each source's passphrase asked of the app when a phone
//! calls. A source has one stream at a time: a new one (the phone reconnected) ends the old.
//! What arrives goes on, datagram by datagram, to the address the app names (the decoder).

use std::collections::HashMap;
use std::io;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use parking_lot::Mutex;
use srt_protocol::options::KeySize;
use srt_protocol::settings::{KeySettings, Passphrase};
use srt_tokio::SrtListener;
use srt_tokio::access::RejectReason;
use tokio::sync::oneshot;

use crate::Feeds;

/// The listener, running on a thread of its own until dropped.
pub struct Receiver {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Receiver {
    /// Listens on `port` (any free port when it's taken, or for 0) with SRT latency `latency`.
    pub fn start(port: u16, latency: Duration, feeds: Arc<dyn Feeds>) -> io::Result<Receiver> {
        let socket = UdpSocket::bind(("0.0.0.0", port)).or_else(|e| {
            if port == 0 {
                Err(e)
            } else {
                tracing::warn!(port, %e, "phone cameras: the SRT port is taken, using another");
                UdpSocket::bind(("0.0.0.0", 0))
            }
        })?;
        socket.set_nonblocking(true)?;
        let port = socket.local_addr()?.port();
        // and the same port over IPv6, for a phone whose address for this computer is an IPv6
        // one (a Bonjour host name, Android's NSD); none where the computer has no IPv6
        let socket6 = ipv6_only(port).inspect_err(|e| tracing::info!(port, %e, "phone cameras: no SRT over IPv6")).ok();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let (stop, stopped) = oneshot::channel();
        let (ready_tx, ready) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new().name("lightair-phone-srt".into()).spawn(move || {
            rt.block_on(async move {
                let socket = match tokio::net::UdpSocket::from_std(socket) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let bound = SrtListener::builder().latency(latency).socket(socket).bind(port).await;
                let (mut listener, mut incoming) = match bound {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let mut v6 = match socket6.map(tokio::net::UdpSocket::from_std) {
                    Some(Ok(s)) => {
                        let addr = std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port));
                        SrtListener::builder().latency(latency).socket(s).bind(addr).await.ok()
                    }
                    _ => None,
                };
                let _ = ready_tx.send(Ok(()));
                let mut none = futures::stream::pending();
                let requests = match v6.as_mut() {
                    Some((_, incoming6)) => {
                        futures::stream::select(incoming.incoming(), incoming6.incoming()).left_stream()
                    }
                    None => futures::stream::select(incoming.incoming(), &mut none).right_stream(),
                };
                let mut requests = std::pin::pin!(requests);
                // each source's current stream (its serial), and every stream task by serial
                let current: Arc<Mutex<HashMap<String, u64>>> = Default::default();
                let tasks: Arc<Mutex<HashMap<u64, tokio::task::AbortHandle>>> = Default::default();
                let mut serial = 0u64;
                let accept = async {
                    while let Some(request) = requests.next().await {
                        let id = request.stream_id().map(|s| s.to_string()).unwrap_or_default();
                        let peer = request.remote();
                        let Some(source) = crate::source_of_stream_id(&id) else {
                            tracing::info!(%peer, id, "phone cameras: refusing a stream for no source");
                            let _ = request.reject(RejectReason::User(403)).await;
                            continue;
                        };
                        let Some(passphrase) = feeds.passphrase(&source) else {
                            tracing::info!(%peer, source, "phone cameras: refusing a stream for an unknown source");
                            let _ = request.reject(RejectReason::User(404)).await;
                            continue;
                        };
                        serial += 1;
                        let me = serial;
                        let (feeds, current, tasks2) = (feeds.clone(), current.clone(), tasks.clone());
                        let task = tokio::spawn(async move {
                            let key = (!passphrase.is_empty()).then(|| KeySettings {
                                key_size: KeySize::AES128,
                                passphrase: Passphrase::from(passphrase.as_str()),
                            });
                            let accepted = request.accept(key).await;
                            let Ok(mut socket) = accepted else {
                                // (a wrong passphrase, or a repeated handshake for a stream that's
                                // already up: the stream there is left alone)
                                tracing::info!(%peer, source, "phone cameras: a stream didn't start");
                                tasks2.lock().remove(&me);
                                return;
                            };
                            tracing::info!(%peer, source, "phone cameras: stream accepted");
                            // a source has one stream: the newest that brings data takes over (a
                            // handshake the phone gave up on can still be accepted here, and
                            // never sends anything)
                            let mut to = None;
                            let mut dump: Option<crate::Dump> = None;
                            let mut demux = crate::ts::Demuxer::new();
                            let out = UdpSocket::bind(("127.0.0.1", 0)).ok();
                            while let Some(Ok((_, data))) = socket.next().await {
                                if current.lock().get(&source) != Some(&me) {
                                    let old = current.lock().insert(source.clone(), me);
                                    if let Some(old) = old.and_then(|o| tasks2.lock().remove(&o)) {
                                        old.abort();
                                    }
                                    tracing::info!(%peer, source, "phone cameras: stream started");
                                    to = feeds.started(&source, peer);
                                    dump = crate::dump_file(&source);
                                }
                                if let (Some(to), Some(out)) = (to, &out) {
                                    // (the decoder not listening yet: dropped, a keyframe comes each second)
                                    let _ = out.send_to(&data, to);
                                }
                                if let Some(f) = dump.as_mut() {
                                    f.write(&data);
                                }
                                for u in demux.push(&data) {
                                    feeds.unit(&source, u);
                                }
                                feeds.data(&source, data.len());
                            }
                            tracing::info!(%peer, source, "phone cameras: stream ended");
                            tasks2.lock().remove(&me);
                            let mut c = current.lock();
                            if c.get(&source) == Some(&me) {
                                c.remove(&source);
                                drop(c);
                                feeds.ended(&source);
                            }
                        });
                        tasks.lock().insert(me, task.abort_handle());
                    }
                };
                tokio::select! {
                    _ = accept => {}
                    _ = stopped => {}
                }
                for (_, t) in tasks.lock().drain() {
                    t.abort();
                }
                listener.close().await;
                if let Some((mut l, _)) = v6 {
                    l.close().await;
                }
            });
        })?;
        match ready.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Receiver { port, stop: Some(stop), thread: Some(thread) }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "the SRT listener didn't start")),
        }
    }

    /// The port phones send to.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// A UDP socket on `port` for IPv6 alone (the IPv4 one has the port for IPv4).
fn ipv6_only(port: u16) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_only_v6(true)?;
    s.set_nonblocking(true)?;
    s.bind(&std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())?;
    Ok(s.into())
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
