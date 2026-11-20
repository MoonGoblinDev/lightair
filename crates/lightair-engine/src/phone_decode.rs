//! A phone camera's stream decoded straight into libobs (macOS): each video frame through
//! VideoToolbox and each AAC frame through AudioToolbox the moment it arrives, handed to a
//! private async source that libobs shows unbuffered. No demuxer probing, no pacing by
//! timestamps, no queues: a frame is on screen as soon as it's decoded (the media player this
//! replaces held phone pictures about 1.5 s behind).
//!
//! Units come from the app (`Feed::push`), split out of the phone's MPEG-TS as they arrived.
//! Parameter sets (H.264 SPS/PPS, HEVC VPS/SPS/PPS) come before every keyframe; a change of them
//! makes a new decompression session. Annex B is turned into the length-prefixed form
//! VideoToolbox takes.

use std::ffi::c_void;
use std::sync::Arc;

use crate::phone_camera::{Feed, Unit, UnitKind};
use crate::phone_units::{adts, bt709, nals, parameter_set};
use crate::sys;

type OSStatus = i32;
type CFTypeRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTime {
    fn ticks_90k(t: u64) -> CMTime {
        CMTime { value: t as i64, timescale: 90_000, flags: 1, epoch: 0 }
    }
    fn invalid() -> CMTime {
        CMTime::default()
    }
}

#[repr(C)]
struct CMSampleTimingInfo {
    duration: CMTime,
    pts: CMTime,
    dts: CMTime,
}

type OutputCallback = unsafe extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CFTypeRef, CMTime, CMTime);

#[repr(C)]
struct OutputCallbackRecord {
    callback: OutputCallback,
    refcon: *mut c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    static kCFBooleanTrue: CFTypeRef;
    static kCFAllocatorNull: CFTypeRef;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        alloc: CFTypeRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_header_length: i32,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        alloc: CFTypeRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_header_length: i32,
        extensions: CFTypeRef,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        alloc: CFTypeRef,
        memory: *mut c_void,
        length: usize,
        block_allocator: CFTypeRef,
        custom_source: *const c_void,
        offset: usize,
        data_length: usize,
        flags: u32,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMSampleBufferCreateReady(
        alloc: CFTypeRef,
        data: CFTypeRef,
        format: CFTypeRef,
        samples: isize,
        timings: isize,
        timing: *const CMSampleTimingInfo,
        sizes: isize,
        size: *const usize,
        out: *mut CFTypeRef,
    ) -> OSStatus;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTDecompressionSessionCreate(
        alloc: CFTypeRef,
        format: CFTypeRef,
        decoder_spec: CFTypeRef,
        image_attributes: CFTypeRef,
        callback: *const OutputCallbackRecord,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: CFTypeRef,
        sample: CFTypeRef,
        flags: u32,
        refcon: *mut c_void,
        info: *mut u32,
    ) -> OSStatus;
    fn VTDecompressionSessionInvalidate(session: CFTypeRef);
    fn VTSessionSetProperty(session: CFTypeRef, key: CFTypeRef, value: CFTypeRef) -> OSStatus;
    static kVTDecompressionPropertyKey_RealTime: CFTypeRef;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetPixelFormatType(buffer: *mut c_void) -> u32;
    fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(buffer: *mut c_void, plane: usize) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRowOfPlane(buffer: *mut c_void, plane: usize) -> usize;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    channels: u32,
    size: u32,
    data: *mut c_void,
}

/// An AudioBufferList with room for two buffers (mono or stereo).
#[repr(C)]
struct AudioBufferList2 {
    count: u32,
    buffers: [AudioBuffer; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioStreamPacketDescription {
    start_offset: i64,
    variable_frames: u32,
    data_size: u32,
}

type InputProc = unsafe extern "C" fn(
    *mut c_void,
    *mut u32,
    *mut AudioBufferList2,
    *mut *mut AudioStreamPacketDescription,
    *mut c_void,
) -> OSStatus;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioConverterNew(
        from: *const AudioStreamBasicDescription,
        to: *const AudioStreamBasicDescription,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn AudioConverterFillComplexBuffer(
        converter: *mut c_void,
        input: InputProc,
        user: *mut c_void,
        packets: *mut u32,
        output: *mut AudioBufferList2,
        descriptions: *mut AudioStreamPacketDescription,
    ) -> OSStatus;
    fn AudioConverterDispose(converter: *mut c_void) -> OSStatus;
}

const FOURCC_420V: u32 = u32::from_be_bytes(*b"420v");
const FOURCC_420F: u32 = u32::from_be_bytes(*b"420f");
const FOURCC_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const FOURCC_AAC: u32 = u32::from_be_bytes(*b"aac ");
const FOURCC_LPCM: u32 = u32::from_be_bytes(*b"lpcm");

/// What the output callback needs: where frames go, and what to tell.
struct Out {
    frames: *mut sys::obs_source_t,
    feed: Arc<Feed>,
}

unsafe extern "C" fn decoded(
    refcon: *mut c_void,
    _frame: *mut c_void,
    status: OSStatus,
    _info: u32,
    image: CFTypeRef,
    pts: CMTime,
    _duration: CMTime,
) {
    if status != 0 || image.is_null() {
        return;
    }
    let out = unsafe { &*(refcon as *const Out) };
    let image = image as *mut c_void;
    unsafe {
        CVPixelBufferLockBaseAddress(image, 1);
        let format = CVPixelBufferGetPixelFormatType(image);
        let (w, h) = (CVPixelBufferGetWidth(image) as u32, CVPixelBufferGetHeight(image) as u32);
        let mut f =
            sys::obs_source_frame { width: w, height: h, timestamp: sys::os_gettime_ns(), ..Default::default() };
        let ok = match format {
            FOURCC_420V | FOURCC_420F => {
                let full = format == FOURCC_420F;
                let (m, lo, hi) = bt709(full);
                f.format = sys::VIDEO_FORMAT_NV12;
                f.full_range = full;
                f.color_matrix = m;
                f.color_range_min = lo;
                f.color_range_max = hi;
                for p in 0..2 {
                    f.data[p] = CVPixelBufferGetBaseAddressOfPlane(image, p) as *mut u8;
                    f.linesize[p] = CVPixelBufferGetBytesPerRowOfPlane(image, p) as u32;
                }
                true
            }
            FOURCC_BGRA => {
                f.format = sys::VIDEO_FORMAT_BGRA;
                f.data[0] = CVPixelBufferGetBaseAddress(image) as *mut u8;
                f.linesize[0] = CVPixelBufferGetBytesPerRow(image) as u32;
                true
            }
            _ => false,
        };
        if ok {
            // (libobs copies the picture)
            sys::obs_source_output_video(out.frames, &f);
        }
        CVPixelBufferUnlockBaseAddress(image, 1);
    }
    if pts.timescale == 90_000 {
        out.feed.decoded(pts.value as u64);
        out.feed.shown_next(pts.value as u64);
    }
}

/// A VideoToolbox session for one set of parameter sets.
struct Video {
    hevc: bool,
    params: Vec<Vec<u8>>,
    format: CFTypeRef,
    session: CFTypeRef,
    /// Kept while the session may call back with it.
    _out: Box<Out>,
}

impl Video {
    fn new(hevc: bool, params: Vec<Vec<u8>>, out: Box<Out>) -> Option<Video> {
        let pointers: Vec<*const u8> = params.iter().map(|p| p.as_ptr()).collect();
        let sizes: Vec<usize> = params.iter().map(Vec::len).collect();
        let mut format: CFTypeRef = std::ptr::null();
        let status = unsafe {
            if hevc {
                CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    std::ptr::null(),
                    params.len(),
                    pointers.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    std::ptr::null(),
                    &mut format,
                )
            } else {
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    std::ptr::null(),
                    params.len(),
                    pointers.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    &mut format,
                )
            }
        };
        if status != 0 || format.is_null() {
            tracing::warn!(status, hevc, "phone camera: the stream's parameter sets didn't read");
            return None;
        }
        let record = OutputCallbackRecord { callback: decoded, refcon: &*out as *const Out as *mut c_void };
        let mut session: CFTypeRef = std::ptr::null();
        let status = unsafe {
            VTDecompressionSessionCreate(
                std::ptr::null(),
                format,
                std::ptr::null(),
                std::ptr::null(),
                &record,
                &mut session,
            )
        };
        if status != 0 || session.is_null() {
            tracing::warn!(status, hevc, "phone camera: no decoder for the stream");
            unsafe { CFRelease(format) };
            return None;
        }
        unsafe { VTSessionSetProperty(session, kVTDecompressionPropertyKey_RealTime, kCFBooleanTrue) };
        Some(Video { hevc, params, format, session, _out: out })
    }

    /// Decodes one access unit (Annex B, its parameter sets already taken out), synchronously.
    fn decode(&mut self, nals: &[&[u8]], pts: u64) {
        let mut avcc = Vec::with_capacity(nals.iter().map(|n| n.len() + 4).sum());
        for n in nals {
            avcc.extend_from_slice(&(n.len() as u32).to_be_bytes());
            avcc.extend_from_slice(n);
        }
        if avcc.is_empty() {
            return;
        }
        unsafe {
            let mut block: CFTypeRef = std::ptr::null();
            let status = CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null(),
                avcc.as_mut_ptr() as *mut c_void,
                avcc.len(),
                kCFAllocatorNull,
                std::ptr::null(),
                0,
                avcc.len(),
                0,
                &mut block,
            );
            if status != 0 || block.is_null() {
                return;
            }
            let timing =
                CMSampleTimingInfo { duration: CMTime::invalid(), pts: CMTime::ticks_90k(pts), dts: CMTime::invalid() };
            let size = avcc.len();
            let mut sample: CFTypeRef = std::ptr::null();
            let status =
                CMSampleBufferCreateReady(std::ptr::null(), block, self.format, 1, 1, &timing, 1, &size, &mut sample);
            if status == 0 && !sample.is_null() {
                // (no asynchronous flag: the frame is out before this returns, `avcc` with it)
                let mut info = 0u32;
                VTDecompressionSessionDecodeFrame(self.session, sample, 0, std::ptr::null_mut(), &mut info);
                CFRelease(sample);
            }
            CFRelease(block);
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        unsafe {
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session);
            CFRelease(self.format);
        }
    }
}

/// An AudioToolbox converter from AAC to float, for one format.
struct Audio {
    converter: *mut c_void,
    rate: u32,
    channels: u32,
}

/// What the converter's input callback hands out: one AAC packet.
struct Packet<'a> {
    data: &'a [u8],
    given: bool,
    description: AudioStreamPacketDescription,
}

unsafe extern "C" fn feed_packet(
    _converter: *mut c_void,
    packets: *mut u32,
    list: *mut AudioBufferList2,
    descriptions: *mut *mut AudioStreamPacketDescription,
    user: *mut c_void,
) -> OSStatus {
    let p = unsafe { &mut *(user as *mut Packet) };
    if p.given {
        unsafe { *packets = 0 };
        // (no more for now: any status but 0 stops the converter here)
        return 1;
    }
    p.given = true;
    p.description =
        AudioStreamPacketDescription { start_offset: 0, variable_frames: 0, data_size: p.data.len() as u32 };
    unsafe {
        *packets = 1;
        (*list).count = 1;
        (*list).buffers[0] =
            AudioBuffer { channels: 0, size: p.data.len() as u32, data: p.data.as_ptr() as *mut c_void };
        if !descriptions.is_null() {
            *descriptions = &mut p.description;
        }
    }
    0
}

impl Audio {
    fn new(rate: u32, channels: u32) -> Option<Audio> {
        let from = AudioStreamBasicDescription {
            sample_rate: rate as f64,
            format_id: FOURCC_AAC,
            frames_per_packet: 1024,
            channels_per_frame: channels,
            ..Default::default()
        };
        // float, planar
        let to = AudioStreamBasicDescription {
            sample_rate: rate as f64,
            format_id: FOURCC_LPCM,
            format_flags: 1 | 8 | 32,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: channels,
            bits_per_channel: 32,
            reserved: 0,
        };
        let mut converter = std::ptr::null_mut();
        let status = unsafe { AudioConverterNew(&from, &to, &mut converter) };
        if status != 0 || converter.is_null() {
            tracing::warn!(status, rate, channels, "phone camera: no AAC decoder");
            return None;
        }
        Some(Audio { converter, rate, channels })
    }

    /// Decodes one raw AAC frame (no ADTS header) into the frames source.
    fn decode(&mut self, raw: &[u8], frames: *mut sys::obs_source_t) {
        let mut planes = vec![vec![0f32; 1024]; self.channels as usize];
        let mut list = AudioBufferList2 {
            count: self.channels,
            buffers: [AudioBuffer { channels: 1, size: 0, data: std::ptr::null_mut() }; 2],
        };
        for (i, p) in planes.iter_mut().enumerate().take(2) {
            list.buffers[i] =
                AudioBuffer { channels: 1, size: (p.len() * 4) as u32, data: p.as_mut_ptr() as *mut c_void };
        }
        let mut packet = Packet { data: raw, given: false, description: AudioStreamPacketDescription::default() };
        let mut count = 1024u32;
        unsafe {
            AudioConverterFillComplexBuffer(
                self.converter,
                feed_packet,
                &mut packet as *mut Packet as *mut c_void,
                &mut count,
                &mut list,
                std::ptr::null_mut(),
            );
        }
        if count == 0 {
            return;
        }
        let mut a = sys::obs_source_audio {
            frames: count,
            speakers: if self.channels >= 2 { sys::SPEAKERS_STEREO } else { sys::SPEAKERS_MONO },
            format: sys::AUDIO_FORMAT_FLOAT_PLANAR,
            samples_per_sec: self.rate,
            timestamp: unsafe { sys::os_gettime_ns() },
            ..Default::default()
        };
        for (o, p) in a.data.iter_mut().zip(&planes) {
            *o = p.as_ptr() as *const u8;
        }
        unsafe { sys::obs_source_output_audio(frames, &a) };
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe { AudioConverterDispose(self.converter) };
    }
}

/// Decodes a feed's units into `frames` (a private async source) until the feed lets go.
pub(crate) fn run(frames: crate::obs::ObsSource, feed: Arc<Feed>, units: crossbeam_channel::Receiver<Unit>) {
    let mut video: Option<Video> = None;
    let mut audio: Option<Audio> = None;
    for unit in units {
        match unit.kind {
            UnitKind::H264 | UnitKind::Hevc => {
                let hevc = unit.kind == UnitKind::Hevc;
                let all = nals(&unit.data);
                let mut params: Vec<(u8, Vec<u8>)> =
                    all.iter().filter_map(|n| parameter_set(hevc, n).map(|o| (o, n.to_vec()))).collect();
                params.sort_by_key(|(o, _)| *o);
                let params: Vec<Vec<u8>> = params.into_iter().map(|(_, p)| p).collect();
                let fresh = !params.is_empty() && video.as_ref().is_none_or(|v| v.hevc != hevc || v.params != params);
                if fresh {
                    // (the old session goes before the new one is made)
                    drop(video.take());
                    let out = Box::new(Out { frames: frames.as_ptr(), feed: feed.clone() });
                    video = Video::new(hevc, params, out);
                }
                let Some(v) = video.as_mut() else { continue };
                let picture: Vec<&[u8]> = all.into_iter().filter(|n| parameter_set(hevc, n).is_none()).collect();
                v.decode(&picture, unit.pts);
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
