//! The SRT listener against srt-tokio's caller, as Lightair Live sends: the right passphrase
//! gets through to the decoder's port, a wrong one or an unknown source doesn't, a silent
//! connection doesn't take over, and a second stream that brings data does.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::SinkExt;
use lightair_phone_camera::Feeds;
use lightair_phone_camera::srt::Receiver;
use parking_lot::Mutex;
use srt_tokio::SrtSocket;

struct Desk {
    to: SocketAddr,
    log: Mutex<Vec<String>>,
}

impl Feeds for Desk {
    fn passphrase(&self, source: &str) -> Option<String> {
        (source == "cam-1").then(|| "correct horse battery".to_string())
    }
    fn started(&self, source: &str, _: SocketAddr) -> Option<SocketAddr> {
        self.log.lock().push(format!("started {source}"));
        Some(self.to)
    }
    fn data(&self, _: &str, _: usize) {}
    fn ended(&self, source: &str) {
        self.log.lock().push(format!("ended {source}"));
    }
}

fn wait_for(what: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if what() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn a_phone_stream_reaches_the_decoder() {
    let decoder = UdpSocket::bind("127.0.0.1:0").unwrap();
    decoder.set_nonblocking(true).unwrap();
    let desk = Arc::new(Desk { to: decoder.local_addr().unwrap(), log: Mutex::new(vec![]) });
    let receiver = Receiver::start(0, Duration::from_millis(40), desk.clone()).unwrap();
    let addr = format!("127.0.0.1:{}", receiver.port());
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    rt.block_on(async {
        // the phone: AES-128, its stream id naming the source
        let mut phone = SrtSocket::builder()
            .latency(Duration::from_millis(40))
            .encryption(16, "correct horse battery")
            .call(addr.as_str(), Some("#!::r=cam-1,m=publish"))
            .await
            .expect("the phone's call");
        let packet = Bytes::from(vec![0x47u8; 188 * 7]);
        for _ in 0..5 {
            phone.send((Instant::now(), packet.clone())).await.unwrap();
        }
        // (the phone's socket runs on this thread: waited for without blocking it)
        let mut buf = [0u8; 2048];
        let mut n = 0;
        for _ in 0..250 {
            if let Ok(got) = decoder.recv(&mut buf) {
                n = got;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(n, 188 * 7);
        assert_eq!(buf[0], 0x47);

        // a wrong passphrase doesn't get in
        let wrong = SrtSocket::builder()
            .encryption(16, "not the passphrase")
            .call(addr.as_str(), Some("#!::r=cam-1,m=publish"));
        let wrong = tokio::time::timeout(Duration::from_secs(3), wrong).await;
        assert!(!matches!(wrong, Ok(Ok(_))), "a wrong passphrase connected");

        // nor a source the desk doesn't have
        let unknown = SrtSocket::builder()
            .encryption(16, "correct horse battery")
            .call(addr.as_str(), Some("#!::r=cam-9,m=publish"));
        let unknown = tokio::time::timeout(Duration::from_secs(3), unknown).await;
        assert!(!matches!(unknown, Ok(Ok(_))), "an unknown source connected");

        // a handshake the phone gave up on, accepted here but silent: it doesn't take over
        let zombie = SrtSocket::builder()
            .encryption(16, "correct horse battery")
            .call(addr.as_str(), Some("#!::r=cam-1,m=publish"))
            .await
            .expect("a silent call");
        // (what came before, read)
        tokio::time::sleep(Duration::from_millis(200)).await;
        while decoder.recv(&mut buf).is_ok() {}
        for _ in 0..5 {
            phone.send((Instant::now(), packet.clone())).await.unwrap();
        }
        let mut more = false;
        for _ in 0..100 {
            if decoder.recv(&mut buf).is_ok() {
                more = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(more, "the first stream still reaches the decoder");

        // the phone reconnects: the new stream takes over once it brings data
        let mut again = SrtSocket::builder()
            .encryption(16, "correct horse battery")
            .call(addr.as_str(), Some("#!::r=cam-1,m=publish"))
            .await
            .expect("the phone's second call");
        for _ in 0..5 {
            again.send((Instant::now(), packet.clone())).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(zombie);
        let _ = phone.close().await;
        again.close().await.unwrap();
    });
    assert!(wait_for(|| desk.log.lock().iter().filter(|l| *l == "started cam-1").count() == 2));
    drop(receiver);
}
