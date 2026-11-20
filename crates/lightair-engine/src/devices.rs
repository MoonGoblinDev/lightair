//! Audio interfaces, captured directly (CoreAudio on macOS, WASAPI on Windows, through cpal): every channel of a device, so
//! an 8-input interface is eight mics (docs/plans/audio-engine.md, "Inputs and outputs").
//! libobs's own device capture mixes a device down to stereo; this doesn't.
//!
//! cpal's streams live on one thread of their own; the engine opens and closes them there.

use std::collections::HashMap;
use std::sync::mpsc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// The input kind of a device input (its settings: `{"device": "<id>"}`, "" for the system's).
pub const DEVICE_KIND: &str = lightair_model::kinds::AUDIO_DEVICE;

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    /// What settings store: the device's own id (CoreAudio's UID, WASAPI's endpoint id), the
    /// same one libobs uses. Two identical interfaces have different ids; renaming one in the
    /// system's settings doesn't change it.
    pub id: String,
    /// What it's called (shown, never looked up by).
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub default: bool,
}

/// A device's id, as settings store it (its host's own id: CoreAudio's UID, WASAPI's endpoint id).
pub fn id_of(d: &cpal::Device) -> Option<String> {
    d.id().ok().map(|i| i.id().to_string())
}

/// What a device is called.
pub fn name_of(d: &cpal::Device) -> String {
    d.description().map(|x| x.name().to_string()).unwrap_or_default()
}

/// Whether a device setting means the system's default: "" (and "default", as some settings
/// saved it).
pub fn is_system_default(device: &str) -> bool {
    device.is_empty() || device == "default"
}

/// The device a setting names: the system's default, the device with that id, or (settings
/// saved before devices were stored by id) the one with that name.
fn find(device: &str, default: Option<cpal::Device>, all: impl Iterator<Item = cpal::Device>) -> Option<cpal::Device> {
    if is_system_default(device) {
        return default;
    }
    let all: Vec<cpal::Device> = all.collect();
    all.iter().find(|d| id_of(d).as_deref() == Some(device)).or_else(|| all.iter().find(|d| name_of(d) == device)).cloned()
}

/// The input a setting names (see [`find`]).
pub fn find_input(device: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    find(device, host.default_input_device(), host.input_devices().ok()?)
}

/// The output a setting names (see [`find`]).
pub fn find_output(device: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    find(device, host.default_output_device(), host.output_devices().ok()?)
}

/// The audio inputs this computer has, with their channel counts.
pub fn input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| id_of(&d));
    let Ok(devices) = host.input_devices() else { return vec![] };
    devices
        .filter_map(|d| {
            let id = id_of(&d)?;
            let channels = d.supported_input_configs().ok()?.map(|c| c.channels()).max()?;
            let rate = d.default_input_config().ok().map_or(48_000, |c| c.sample_rate());
            Some(DeviceInfo { default: default.as_deref() == Some(id.as_str()), name: name_of(&d), id, channels, sample_rate: rate })
        })
        .collect()
}

/// What a device setting is called: "" for the system's default, else the device's name (the
/// setting itself when that device isn't here).
pub fn device_name(device: &str) -> Option<String> {
    if is_system_default(device) {
        return None;
    }
    Some(find_input(device).or_else(|| find_output(device)).map_or_else(|| device.to_string(), |d| name_of(&d)))
}

/// Turns a device's rate into the engine's by linear interpolation (devices that can't run at
/// the engine's rate). Keeps its buffers between callbacks: no allocation once warm.
struct Resampler {
    channels: usize,
    step: f64,
    pos: f64,
    last: Vec<f32>,
    out: Vec<f32>,
}

impl Resampler {
    fn new(channels: usize, from: u32, to: u32) -> Self {
        Self {
            channels,
            step: from as f64 / to as f64,
            pos: 0.0,
            last: vec![0.0; channels],
            out: Vec::with_capacity(16_384 * channels),
        }
    }

    /// Interleaved input in, interleaved output (in `self.out`).
    fn run(&mut self, input: &[f32]) {
        self.out.clear();
        let n = input.len() / self.channels;
        let at = |i: isize, c: usize, last: &[f32]| if i < 0 { last[c] } else { input[i as usize * self.channels + c] };
        while self.pos < n as f64 {
            let i = self.pos.floor() as isize - 1; // `last` is frame -1
            let f = (self.pos - self.pos.floor()) as f32;
            for c in 0..self.channels {
                let a = at(i, c, &self.last);
                let b = at(i + 1, c, &self.last);
                self.out.push(a + (b - a) * f);
            }
            self.pos += self.step;
        }
        self.pos -= n as f64;
        if n > 0 {
            self.last.copy_from_slice(&input[(n - 1) * self.channels..n * self.channels]);
        }
    }
}

enum Cmd {
    Open {
        input: lightair_model::Id,
        device: String,
        sample_rate: u32,
        ring: rtrb::Producer<f32>,
        reply: mpsc::Sender<Result<u16, String>>,
    },
    Close(lightair_model::Id),
    /// The monitor: plays `ring` (stereo, interleaved) on an output device.
    OpenOutput {
        device: String,
        sample_rate: u32,
        ring: rtrb::Consumer<f32>,
        reply: mpsc::Sender<Result<(), String>>,
    },
    CloseOutput,
}

/// Times the monitor ran short (a gap in what's heard), for diagnostics.
pub static MONITOR_SHORTFALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The device thread: owns the open streams.
pub(crate) struct Devices {
    tx: mpsc::Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Devices {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let thread = std::thread::Builder::new()
            .name("lightair-devices".into())
            .spawn(move || {
                let mut streams: HashMap<lightair_model::Id, cpal::Stream> = HashMap::new();
                // the monitor's stream, held open (at most one)
                let mut output: Vec<cpal::Stream> = Vec::new();
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Cmd::Open { input, device, sample_rate, ring, reply } => {
                            streams.remove(&input);
                            match open(&device, sample_rate, ring) {
                                Ok((stream, channels)) => {
                                    streams.insert(input, stream);
                                    let _ = reply.send(Ok(channels));
                                }
                                Err(e) => {
                                    let _ = reply.send(Err(e));
                                }
                            }
                        }
                        Cmd::Close(input) => {
                            streams.remove(&input);
                        }
                        Cmd::OpenOutput { device, sample_rate, ring, reply } => {
                            output.clear();
                                match open_output(&device, sample_rate, ring) {
                                Ok(stream) => {
                                    output.push(stream);
                                    let _ = reply.send(Ok(()));
                                }
                                Err(e) => {
                                    let _ = reply.send(Err(e));
                                }
                            }
                        }
                        Cmd::CloseOutput => output.clear(),
                    }
                }
            })
            .ok();
        Self { tx, thread }
    }

    /// Opens `device` ("" for the default input) for `input`, feeding `ring` (interleaved, at
    /// `sample_rate`). Returns its channel count.
    pub fn open(
        &self,
        input: lightair_model::Id,
        device: &str,
        sample_rate: u32,
        ring: rtrb::Producer<f32>,
    ) -> Result<u16, String> {
        let (reply, answer) = mpsc::channel();
        self.tx
            .send(Cmd::Open { input, device: device.into(), sample_rate, ring, reply })
            .map_err(|_| "the device thread stopped".to_string())?;
        answer.recv_timeout(std::time::Duration::from_secs(5)).map_err(|_| "the device didn't open".to_string())?
    }

    pub fn close(&self, input: lightair_model::Id) {
        let _ = self.tx.send(Cmd::Close(input));
    }

    /// Plays `ring` (stereo, interleaved, at `sample_rate`) on output `device` ("" for the
    /// default output).
    pub fn open_output(&self, device: &str, sample_rate: u32, ring: rtrb::Consumer<f32>) -> Result<(), String> {
        let (reply, answer) = mpsc::channel();
        self.tx
            .send(Cmd::OpenOutput { device: device.into(), sample_rate, ring, reply })
            .map_err(|_| "the device thread stopped".to_string())?;
        answer.recv_timeout(std::time::Duration::from_secs(5)).map_err(|_| "the output didn't open".to_string())?
    }

    pub fn close_output(&self) {
        let _ = self.tx.send(Cmd::CloseOutput);
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        // closing the channel ends the thread, which drops the streams
        let (tx, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.tx, tx));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn open(name: &str, sample_rate: u32, mut ring: rtrb::Producer<f32>) -> Result<(cpal::Stream, u16), String> {
    let device = find_input(name).ok_or_else(|| format!("no audio input \"{name}\""))?;
    // all its channels, at the engine's rate if it can, in 32-bit float
    let configs: Vec<_> = device.supported_input_configs().map_err(|e| e.to_string())?.collect();
    let channels = configs.iter().map(|c| c.channels()).max().ok_or("the device has no inputs")?;
    let at_rate = configs.iter().find(|c| {
        c.channels() == channels && c.min_sample_rate() <= sample_rate && c.max_sample_rate() >= sample_rate
    });
    let rate = if at_rate.is_some() {
        sample_rate
    } else {
        device.default_input_config().map_err(|e| e.to_string())?.sample_rate()
    };
    let config = cpal::StreamConfig {
        channels,
        sample_rate: rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let mut resampler = (rate != sample_rate).then(|| Resampler::new(channels as usize, rate, sample_rate));
    let stream = device
        .build_input_stream(
            config,
            move |data: &[f32], _| {
                let samples: &[f32] = match &mut resampler {
                    Some(r) => {
                        r.run(data);
                        &r.out
                    }
                    None => data,
                };
                // a full ring means the engine stopped reading: drop rather than block
                if ring.slots() >= samples.len()
                    && let Ok(mut chunk) = ring.write_chunk_uninit(samples.len())
                {
                    let (a, b) = chunk.as_mut_slices();
                    for (dst, src) in a.iter_mut().chain(b.iter_mut()).zip(samples) {
                        dst.write(*src);
                    }
                    unsafe { chunk.commit_all() };
                }
            },
            |e| tracing::warn!(%e, "audio input"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    tracing::info!(device = name, channels, rate, "audio input open");
    Ok((stream, channels))
}

fn open_output(name: &str, sample_rate: u32, mut ring: rtrb::Consumer<f32>) -> Result<cpal::Stream, String> {
    let device = find_output(name).ok_or_else(|| format!("no audio output \"{name}\""))?;
    // the engine's rate, as few channels as it takes (at least two), small buffers
    let config = device
        .supported_output_configs()
        .map_err(|e| e.to_string())?
        .filter(|c| c.min_sample_rate() <= sample_rate && c.max_sample_rate() >= sample_rate && c.channels() >= 2)
        .min_by_key(|c| c.channels())
        .ok_or("the output can't play at the engine's rate")?;
    let channels = config.channels() as usize;
    let buffer = match config.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => cpal::BufferSize::Fixed(128.clamp(*min, *max)),
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    };
    let stream_config = cpal::StreamConfig { channels: channels as u16, sample_rate, buffer_size: buffer };
    let stream = device
        .build_output_stream(
            stream_config,
            move |data: &mut [f32], _| {
                let frames = data.len() / channels;
                let have = (ring.slots() / 2).min(frames);
                if have < frames {
                    MONITOR_SHORTFALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                data.fill(0.0);
                if let Ok(chunk) = ring.read_chunk(have * 2) {
                    let (a, b) = chunk.as_slices();
                    for k in 0..have {
                        for c in 0..2 {
                            let i = k * 2 + c;
                            data[k * channels + c] = if i < a.len() { a[i] } else { b[i - a.len()] };
                        }
                    }
                    chunk.commit_all();
                }
            },
            |e| tracing::warn!(%e, "monitor output"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    tracing::info!(device = name, "monitor output open");
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_this_macs_inputs() {
        // listing doesn't open anything (no permission prompt)
        for d in input_devices() {
            eprintln!("{} · {} ch · {} Hz{}", d.name, d.channels, d.sample_rate, if d.default { " (default)" } else { "" });
            assert!(d.channels > 0);
        }
    }

    #[test]
    fn resampling_keeps_a_ramp_straight() {
        // 44.1 → 48 kHz on a ramp: the output is the same ramp, denser
        let mut r = Resampler::new(1, 44_100, 48_000);
        let input: Vec<f32> = (0..441).map(|i| i as f32).collect();
        r.run(&input);
        assert!((r.out.len() as i32 - 480).abs() <= 1, "{} frames", r.out.len());
        for w in r.out[2..].windows(2) {
            assert!((w[1] - w[0] - 44_100.0 / 48_000.0).abs() < 1e-3);
        }
    }
}
