//! A phone camera's stream decoded straight into libobs (Windows), as on macOS
//! (`phone_decode.rs`): each access unit through Media Foundation's own H.264 / HEVC decoder in
//! low-latency mode, and each AAC frame through its AAC decoder, the moment they arrive, into
//! the private async source libobs shows unbuffered. This replaces libobs's media source
//! reading the feed's UDP port (its probing and buffering held pictures about 1.5 s behind).
//!
//! The decoders take Annex B as it comes (parameter sets in band, a change of them followed by
//! the decoder); pictures come out as NV12.

use std::mem::ManuallyDrop;
use std::sync::Arc;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::core::{GUID, Interface};

use crate::phone_camera::{Feed, Unit, UnitKind};
use crate::phone_units::{RATES, adts, bt709};
use crate::sys;

/// The first synchronous decoder Media Foundation has for `subtype` (hardware-backed ones
/// first, as it sorts them).
unsafe fn decoder(category: GUID, major: GUID, subtype: GUID) -> Option<IMFTransform> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: major, guidSubtype: subtype };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    let flags = MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0);
    unsafe { MFTEnumEx(category, flags, Some(&input), None, &mut list, &mut count) }.ok()?;
    let mut out = None;
    for i in 0..count as usize {
        // (each entry is ours to release)
        let activate = unsafe { std::ptr::read(list.add(i)) };
        if out.is_none()
            && let Some(a) = &activate
        {
            out = unsafe { a.ActivateObject::<IMFTransform>() }.ok();
        }
    }
    if !list.is_null() {
        unsafe { CoTaskMemFree(Some(list as *const _)) };
    }
    out
}

/// `data` as a Media Foundation sample at `time` (100 ns units).
unsafe fn sample_of(data: &[u8], time: i64) -> Option<IMFSample> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(data.len() as u32).ok()?;
        let mut p = std::ptr::null_mut();
        buffer.Lock(&mut p, None, None).ok()?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        buffer.Unlock().ok()?;
        buffer.SetCurrentLength(data.len() as u32).ok()?;
        let sample = MFCreateSample().ok()?;
        sample.AddBuffer(&buffer).ok()?;
        sample.SetSampleTime(time).ok()?;
        Some(sample)
    }
}

/// A sample with a buffer of `size` bytes for a decoder to fill.
unsafe fn empty_sample(size: u32) -> Option<IMFSample> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(size.max(1)).ok()?;
        let sample = MFCreateSample().ok()?;
        sample.AddBuffer(&buffer).ok()?;
        Some(sample)
    }
}

/// One output of a decoder: a sample, or why there's none.
enum Output {
    Sample(IMFSample),
    /// The format changed: choose an output again.
    Changed,
    /// It needs more input (or failed).
    Done,
}

unsafe fn output(mft: &IMFTransform, provides: bool, size: u32) -> Output {
    let given = if provides { None } else { unsafe { empty_sample(size) } };
    let mut out = MFT_OUTPUT_DATA_BUFFER {
        dwStreamID: 0,
        pSample: ManuallyDrop::new(given),
        dwStatus: 0,
        pEvents: ManuallyDrop::new(None),
    };
    let mut status = 0u32;
    let r = unsafe { mft.ProcessOutput(0, std::slice::from_mut(&mut out), &mut status) };
    let sample = ManuallyDrop::into_inner(out.pSample);
    drop(ManuallyDrop::into_inner(out.pEvents));
    match r {
        Ok(()) => sample.map_or(Output::Done, Output::Sample),
        Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => Output::Changed,
        Err(_) => Output::Done,
    }
}

/// How the decoder lays a picture out.
#[derive(Clone, Copy)]
struct Layout {
    /// What's shown (the stream's own size).
    width: u32,
    height: u32,
    /// Rows in the luma plane (the coded height, a multiple of 16) and bytes in a row.
    rows: u32,
    stride: u32,
    full: bool,
}

/// A Media Foundation video decoder for one kind of stream.
struct Video {
    hevc: bool,
    mft: IMFTransform,
    provides: bool,
    size: u32,
    layout: Option<Layout>,
}

impl Video {
    fn new(hevc: bool) -> Option<Video> {
        let subtype = if hevc { MFVideoFormat_HEVC } else { MFVideoFormat_H264 };
        unsafe {
            let Some(mft) = decoder(MFT_CATEGORY_VIDEO_DECODER, MFMediaType_Video, subtype) else {
                tracing::warn!(hevc, "phone camera: Windows has no decoder for the stream");
                return None;
            };
            // a frame out for every frame in, nothing held for reordering
            if let Ok(attributes) = mft.GetAttributes() {
                let _ = attributes.SetUINT32(&CODECAPI_AVLowLatencyMode, 1);
            }
            let input = MFCreateMediaType().ok()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok()?;
            input.SetGUID(&MF_MT_SUBTYPE, &subtype).ok()?;
            if let Err(e) = mft.SetInputType(0, &input, 0) {
                tracing::warn!(%e, hevc, "phone camera: the decoder didn't take the stream");
                return None;
            }
            let mut v = Video { hevc, mft, provides: false, size: 0, layout: None };
            v.choose_output()?;
            let _ = v.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            let _ = v.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            Some(v)
        }
    }

    /// NV12 out (offered by every decoder), and how it's laid out.
    fn choose_output(&mut self) -> Option<()> {
        unsafe {
            for i in 0.. {
                let t = self.mft.GetOutputAvailableType(0, i).ok()?;
                if t.GetGUID(&MF_MT_SUBTYPE).ok()? != MFVideoFormat_NV12 {
                    continue;
                }
                self.mft.SetOutputType(0, &t, 0).ok()?;
                let info = self.mft.GetOutputStreamInfo(0).ok()?;
                self.provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
                self.size = info.cbSize;
                self.layout = layout_of(&t);
                return Some(());
            }
        }
        None
    }

    /// Decodes one access unit (Annex B, parameter sets and all) and shows what comes out.
    fn decode(&mut self, au: &[u8], pts_90k: u64, frames: *mut sys::obs_source_t, feed: &Feed) {
        let Some(sample) = (unsafe { sample_of(au, (pts_90k as i64) * 1000 / 9) }) else { return };
        if unsafe { self.mft.ProcessInput(0, &sample, 0) }.is_err() {
            // (full: what's waiting first, then this once more)
            self.drain(frames, feed);
            if unsafe { self.mft.ProcessInput(0, &sample, 0) }.is_err() {
                return;
            }
        }
        self.drain(frames, feed);
    }

    fn drain(&mut self, frames: *mut sys::obs_source_t, feed: &Feed) {
        loop {
            match unsafe { output(&self.mft, self.provides, self.size) } {
                Output::Sample(s) => self.show(&s, frames, feed),
                Output::Changed => {
                    if self.choose_output().is_none() {
                        return;
                    }
                }
                Output::Done => return,
            }
        }
    }

    fn show(&self, sample: &IMFSample, frames: *mut sys::obs_source_t, feed: &Feed) {
        let Some(l) = self.layout else { return };
        unsafe {
            let Ok(buffer) = sample.ConvertToContiguousBuffer() else { return };
            // a 2D buffer says its own pitch
            let two_d = buffer.cast::<IMF2DBuffer>().ok();
            let (mut p, mut pitch) = (std::ptr::null_mut(), l.stride as i32);
            let locked = match &two_d {
                Some(b) => b.Lock2D(&mut p, &mut pitch).is_ok(),
                None => buffer.Lock(&mut p, None, None).is_ok(),
            };
            if !locked || p.is_null() || pitch <= 0 {
                return;
            }
            let (m, lo, hi) = bt709(l.full);
            let mut f = sys::obs_source_frame {
                width: l.width,
                height: l.height,
                timestamp: sys::os_gettime_ns(),
                format: sys::VIDEO_FORMAT_NV12,
                full_range: l.full,
                color_matrix: m,
                color_range_min: lo,
                color_range_max: hi,
                ..Default::default()
            };
            f.data[0] = p;
            f.data[1] = p.add(pitch as usize * l.rows as usize);
            f.linesize[0] = pitch as u32;
            f.linesize[1] = pitch as u32;
            // (libobs copies the picture)
            sys::obs_source_output_video(frames, &f);
            match &two_d {
                Some(b) => {
                    let _ = b.Unlock2D();
                }
                None => {
                    let _ = buffer.Unlock();
                }
            }
            if let Ok(t) = sample.GetSampleTime() {
                let pts = (t.max(0) as u64) * 9 / 1000;
                feed.decoded(pts);
                feed.shown_next(pts);
            }
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

/// An NV12 output type's picture layout.
unsafe fn layout_of(t: &IMFMediaType) -> Option<Layout> {
    unsafe {
        let size = t.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
        let (coded_w, coded_h) = ((size >> 32) as u32, (size & 0xFFFF_FFFF) as u32);
        let stride = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| (s as i32).unsigned_abs()).unwrap_or(coded_w);
        // what's shown, where the decoder pads the picture (1080 in 1088 rows)
        let mut area = MFVideoArea::default();
        let bytes = std::slice::from_raw_parts_mut(
            &mut area as *mut MFVideoArea as *mut u8,
            std::mem::size_of::<MFVideoArea>(),
        );
        let (w, h) = match t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, bytes, None) {
            Ok(()) if area.Area.cx > 0 && area.Area.cy > 0 => (area.Area.cx as u32, area.Area.cy as u32),
            _ => (coded_w, coded_h),
        };
        let full = t.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE).is_ok_and(|r| r == MFNominalRange_0_255.0 as u32);
        Some(Layout { width: w, height: h, rows: coded_h, stride, full })
    }
}

/// A Media Foundation AAC decoder for one format, to float.
struct Audio {
    mft: IMFTransform,
    rate: u32,
    channels: u32,
    provides: bool,
    size: u32,
}

impl Audio {
    fn new(rate: u32, channels: u32) -> Option<Audio> {
        unsafe {
            let Some(mft) = decoder(MFT_CATEGORY_AUDIO_DECODER, MFMediaType_Audio, MFAudioFormat_AAC) else {
                tracing::warn!(rate, channels, "phone camera: Windows has no AAC decoder");
                return None;
            };
            let input = MFCreateMediaType().ok()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ok()?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).ok()?;
            input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).ok()?;
            input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels).ok()?;
            // raw AAC frames (the ADTS headers taken off), described by their AudioSpecificConfig
            input.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0).ok()?;
            input.SetBlob(&MF_MT_USER_DATA, &user_data(rate, channels)).ok()?;
            if let Err(e) = mft.SetInputType(0, &input, 0) {
                tracing::warn!(%e, rate, channels, "phone camera: the AAC decoder didn't take the sound");
                return None;
            }
            let mut chosen = false;
            for i in 0.. {
                let Ok(t) = mft.GetOutputAvailableType(0, i) else { break };
                if t.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFAudioFormat_Float) {
                    chosen = mft.SetOutputType(0, &t, 0).is_ok();
                    break;
                }
            }
            if !chosen {
                tracing::warn!("phone camera: the AAC decoder gives no float sound");
                return None;
            }
            let info = mft.GetOutputStreamInfo(0).ok()?;
            let _ = mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            Some(Audio {
                mft,
                rate,
                channels,
                provides: info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0,
                // (1024 frames of float, at the least)
                size: info.cbSize.max(1024 * 4 * channels),
            })
        }
    }

    /// Decodes one raw AAC frame into the frames source.
    fn decode(&mut self, raw: &[u8], frames: *mut sys::obs_source_t) {
        let Some(sample) = (unsafe { sample_of(raw, 0) }) else { return };
        if unsafe { self.mft.ProcessInput(0, &sample, 0) }.is_err() {
            return;
        }
        while let Output::Sample(s) = unsafe { output(&self.mft, self.provides, self.size) } {
            unsafe {
                let Ok(buffer) = s.ConvertToContiguousBuffer() else { return };
                let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
                if buffer.Lock(&mut p, None, Some(&mut len)).is_err() || p.is_null() {
                    return;
                }
                let count = len / 4 / self.channels.max(1);
                if count > 0 {
                    let mut a = sys::obs_source_audio {
                        frames: count,
                        speakers: if self.channels >= 2 { sys::SPEAKERS_STEREO } else { sys::SPEAKERS_MONO },
                        format: sys::AUDIO_FORMAT_FLOAT,
                        samples_per_sec: self.rate,
                        timestamp: sys::os_gettime_ns(),
                        ..Default::default()
                    };
                    a.data[0] = p as *const u8;
                    sys::obs_source_output_audio(frames, &a);
                }
                let _ = buffer.Unlock();
            }
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

/// The AAC decoder's `MF_MT_USER_DATA`: what follows the `WAVEFORMATEX` in a `HEAACWAVEINFO`
/// (raw payload, AAC LC), then the AudioSpecificConfig.
fn user_data(rate: u32, channels: u32) -> Vec<u8> {
    let index = RATES.iter().position(|r| *r == rate).unwrap_or(3) as u16;
    // object type 2 (AAC LC), the rate's index, the channel configuration
    let asc: u16 = (2 << 11) | (index << 7) | ((channels.min(7) as u16) << 3);
    let mut out = vec![0u8; 12];
    // wPayloadType 0 (raw), wAudioProfileLevelIndication 0xFE (none given)
    out[2..4].copy_from_slice(&0xFEu16.to_le_bytes());
    out.extend_from_slice(&asc.to_be_bytes());
    out
}

/// Decodes a feed's units into `frames` (a private async source) until the feed lets go.
pub(crate) fn run(frames: crate::obs::ObsSource, feed: Arc<Feed>, units: crossbeam_channel::Receiver<Unit>) {
    // (this thread's own COM and Media Foundation)
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_LITE) {
            tracing::warn!(%e, "phone camera: Media Foundation didn't start");
            return;
        }
    }
    {
        let mut video: Option<Video> = None;
        let mut audio: Option<Audio> = None;
        for unit in units {
            match unit.kind {
                UnitKind::H264 | UnitKind::Hevc => {
                    let hevc = unit.kind == UnitKind::Hevc;
                    if video.as_ref().is_none_or(|v| v.hevc != hevc) {
                        drop(video.take());
                        video = Video::new(hevc);
                    }
                    if let Some(v) = video.as_mut() {
                        v.decode(&unit.data, unit.pts, frames.as_ptr(), &feed);
                    }
                }
                UnitKind::Aac => {
                    for (rate, channels, raw) in adts(&unit.data) {
                        if audio.as_ref().is_none_or(|a| a.rate != rate || a.channels != channels) {
                            audio = Audio::new(rate, channels);
                        }
                        if let Some(a) = audio.as_mut() {
                            a.decode(raw, frames.as_ptr());
                        }
                    }
                }
            }
        }
    }
    unsafe {
        let _ = MFShutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aac_user_data() {
        // 48 kHz (index 3), stereo: AAC LC's AudioSpecificConfig is 0x11 0x90
        let d = user_data(48_000, 2);
        assert_eq!(d.len(), 14);
        assert_eq!(&d[..4], &[0, 0, 0xFE, 0]);
        assert_eq!(&d[12..], &[0x11, 0x90]);
    }
}
