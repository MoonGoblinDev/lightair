//! Links to phones at TCP addresses: the iOS simulator's Camera mode (it listens on this Mac's
//! `127.0.0.1:4457`) and an Android phone behind `adb forward tcp:4457 tcp:4457` (developers).
//! The same link as over USB.

use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use crate::keep::{Halves, Stop, keep};
use crate::link::LinkHost;

/// Keeps links to these addresses while it lives.
pub struct Watcher {
    _stop: Stop,
}

impl Watcher {
    pub fn start(addresses: &[SocketAddr], host: Arc<dyn LinkHost>) -> std::io::Result<Watcher> {
        let stop = Stop::new();
        for &addr in addresses {
            keep(format!("the phone at {addr}"), stop.0.clone(), host.clone(), move || {
                let s = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
                s.set_nodelay(true)?;
                s.set_read_timeout(Some(Duration::from_millis(200)))?;
                let read = s.try_clone()?;
                let halves: Halves = (Box::new(read), Box::new(s));
                Ok(halves)
            })?;
        }
        Ok(Watcher { _stop: stop })
    }
}
