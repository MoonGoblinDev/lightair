//! Keeping a link to one phone: connect, serve until it ends, wait, again; until stopped. The
//! phone listens only while Camera mode is open, so a refused connection is the usual case.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::link::{LinkHost, serve};

/// A stream's two halves.
pub type Halves = (Box<dyn Read + Send>, Box<dyn Write + Send>);

/// How often a phone that isn't listening is tried again.
pub const RETRY: Duration = Duration::from_secs(2);

/// Runs on its own thread until `stop` (or the host goes): `connect` makes a stream to the phone.
pub fn keep(
    what: String,
    stop: Arc<AtomicBool>,
    host: Arc<dyn LinkHost>,
    connect: impl Fn() -> io::Result<Halves> + Send + 'static,
) -> io::Result<()> {
    std::thread::Builder::new().name("lightair-phone-link".into()).spawn(move || {
        let mut said = false;
        while !stop.load(Ordering::Acquire) {
            match connect() {
                Ok((read, write)) => {
                    said = false;
                    if let Err(e) = serve(read, write, host.clone(), &what) {
                        tracing::info!(what, %e, "phone camera link ended");
                    }
                }
                Err(e) => {
                    if !said {
                        tracing::debug!(what, %e, "no phone camera listening");
                        said = true;
                    }
                }
            }
            let mut waited = Duration::ZERO;
            while waited < RETRY && !stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(100));
                waited += Duration::from_millis(100);
            }
        }
    })?;
    Ok(())
}

/// A stop flag that's set when dropped.
pub struct Stop(pub Arc<AtomicBool>);

impl Stop {
    pub fn new() -> Stop {
        Stop(Arc::new(AtomicBool::new(false)))
    }
}

impl Default for Stop {
    fn default() -> Self {
        Stop::new()
    }
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
