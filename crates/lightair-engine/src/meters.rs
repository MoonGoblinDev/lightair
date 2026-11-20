//! Audio level telemetry (docs/03-architecture.md, "Runtime and telemetry flow").
//!
//! libobs volume meters call back on the audio thread; each callback writes into a lock-free
//! [`MeterCell`]. The UI reads the cells once per frame while a meter is visible. Nothing here
//! allocates or locks on the audio thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use lightair_model::Id;
use parking_lot::RwLock;

pub const CHANNELS: usize = 2;

/// Latest levels for one track, in dBFS (−∞ when silent).
#[derive(Default)]
pub struct MeterCell {
    peak: [AtomicU32; CHANNELS],
    magnitude: [AtomicU32; CHANNELS],
    /// Microseconds since the meter bank started, at the last update.
    updated_us: AtomicU64,
    /// Loudest peak since `take_window_peak`, as linear amplitude bits (non-negative floats
    /// order the same as their bits, so `fetch_max` works lock-free).
    window_max: AtomicU32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    pub peak: [f32; CHANNELS],
    pub magnitude: [f32; CHANNELS],
    /// Seconds since the last update (a stalled source shows as silence).
    pub age: f32,
}

impl MeterCell {
    pub fn store(&self, peak: &[f32], magnitude: &[f32]) {
        for c in 0..CHANNELS {
            let p = peak.get(c).or(peak.first()).copied().unwrap_or(f32::NEG_INFINITY);
            let m = magnitude.get(c).or(magnitude.first()).copied().unwrap_or(f32::NEG_INFINITY);
            self.peak[c].store(p.to_bits(), Ordering::Relaxed);
            self.magnitude[c].store(m.to_bits(), Ordering::Relaxed);
        }
        self.updated_us.store(epoch().elapsed().as_micros() as u64, Ordering::Release);
        let loudest = peak.iter().copied().map(db_to_mul).fold(0.0f32, f32::max);
        self.window_max.fetch_max(loudest.to_bits(), Ordering::Relaxed);
    }

    /// The loudest peak (dBFS) since the last call, and starts a new window.
    pub fn take_window_peak(&self) -> f32 {
        let lin = f32::from_bits(self.window_max.swap(0, Ordering::Relaxed));
        if lin > 0.0 { 20.0 * lin.log10() } else { f32::NEG_INFINITY }
    }

    pub fn levels(&self) -> Levels {
        let updated = self.updated_us.load(Ordering::Acquire);
        let age = if updated == 0 {
            f32::INFINITY
        } else {
            (epoch().elapsed().as_micros() as u64).saturating_sub(updated) as f32 / 1e6
        };
        let read = |a: &[AtomicU32; CHANNELS]| {
            [f32::from_bits(a[0].load(Ordering::Relaxed)), f32::from_bits(a[1].load(Ordering::Relaxed))]
        };
        let (mut peak, mut magnitude) = (read(&self.peak), read(&self.magnitude));
        if age > 0.5 || updated == 0 {
            peak = [f32::NEG_INFINITY; CHANNELS];
            magnitude = [f32::NEG_INFINITY; CHANNELS];
        }
        Levels { peak, magnitude, age }
    }
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Meter cells by track id. One bank per process, like libobs itself.
#[derive(Default)]
pub struct MeterBank {
    cells: RwLock<HashMap<Id, Arc<MeterCell>>>,
}

impl MeterBank {
    pub fn get(&self, track: Id) -> Option<Arc<MeterCell>> {
        self.cells.read().get(&track).cloned()
    }

    pub(crate) fn insert(&self, track: Id) -> Arc<MeterCell> {
        let cell = Arc::new(MeterCell::default());
        epoch();
        self.cells.write().insert(track, cell.clone());
        cell
    }

    pub(crate) fn remove(&self, track: Id) {
        self.cells.write().remove(&track);
    }
}

pub fn meters() -> &'static MeterBank {
    static BANK: OnceLock<MeterBank> = OnceLock::new();
    BANK.get_or_init(MeterBank::default)
}

/// dB → linear amplitude multiplier for libobs volume.
pub fn db_to_mul(db: f32) -> f32 {
    if db.is_finite() { 10f32.powf(db / 20.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_peak_keeps_the_loudest() {
        let cell = MeterCell::default();
        cell.store(&[-30.0, -20.0], &[-40.0, -40.0]);
        cell.store(&[-6.0, -50.0], &[-40.0, -40.0]);
        cell.store(&[-12.0, -12.0], &[-40.0, -40.0]);
        assert!((cell.take_window_peak() + 6.0).abs() < 0.01);
        assert_eq!(cell.take_window_peak(), f32::NEG_INFINITY, "a new window starts silent");
    }
}
