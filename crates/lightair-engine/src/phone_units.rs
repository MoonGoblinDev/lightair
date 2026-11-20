//! What the phone camera's direct decoders share (macOS: VideoToolbox, `phone_decode.rs`;
//! Windows: Media Foundation, `phone_decode_win.rs`): splitting the phone's units (Annex B
//! access units, ADTS frames), BT.709 for libobs, and the private async source decoded frames
//! go into.

use std::ffi::{CStr, c_void};

use crate::sys;

/// BT.709 YCbCr to RGB, as libobs takes it (rows of [y, cb, cr, 1]), and the ranges.
pub(crate) fn bt709(full: bool) -> ([f32; 16], [f32; 3], [f32; 3]) {
    if full {
        (
            [1.0, 0.0, 1.5748, -0.7874, 1.0, -0.1873, -0.4681, 0.3277, 1.0, 1.8556, 0.0, -0.9278, 0.0, 0.0, 0.0, 1.0],
            [0.0; 3],
            [1.0; 3],
        )
    } else {
        (
            [
                1.1644, 0.0, 1.7927, -0.9729, 1.1644, -0.2132, -0.5329, 0.3015, 1.1644, 2.1124, 0.0, -1.1334, 0.0, 0.0,
                0.0, 1.0,
            ],
            [16.0 / 255.0; 3],
            [235.0 / 255.0, 240.0 / 255.0, 240.0 / 255.0],
        )
    }
}

/// The NAL units of an Annex B access unit, without their start codes.
// (Windows hands its decoder Annex B as it is)
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn nals(au: &[u8]) -> Vec<&[u8]> {
    let mut starts = vec![];
    let mut i = 0;
    while i + 3 <= au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = vec![];
    for (k, &s) in starts.iter().enumerate() {
        let mut e = starts.get(k + 1).map_or(au.len(), |n| n - 3);
        while e > s && au[e - 1] == 0 && k + 1 < starts.len() {
            e -= 1;
        }
        out.push(&au[s..e]);
    }
    out
}

/// Whether a NAL is a parameter set, and its order among them (VPS, SPS, PPS).
// (Windows hands its decoder Annex B as it is)
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parameter_set(hevc: bool, nal: &[u8]) -> Option<u8> {
    let first = *nal.first()?;
    if hevc {
        match (first >> 1) & 0x3F {
            32 => Some(0),
            33 => Some(1),
            34 => Some(2),
            _ => None,
        }
    } else {
        match first & 0x1F {
            7 => Some(1),
            8 => Some(2),
            _ => None,
        }
    }
}

/// ADTS sampling frequency index to Hz.
pub(crate) const RATES: [u32; 13] =
    [96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350];

/// The ADTS frames of a unit: (sample rate, channels, the raw AAC).
pub(crate) fn adts(mut data: &[u8]) -> Vec<(u32, u32, &[u8])> {
    let mut out = vec![];
    while data.len() >= 7 && data[0] == 0xFF && data[1] & 0xF0 == 0xF0 {
        let header = if data[1] & 0x01 == 0 { 9 } else { 7 };
        let rate = RATES.get(((data[2] >> 2) & 0x0F) as usize).copied().unwrap_or(48_000);
        let channels = (((data[2] & 0x01) << 2) | (data[3] >> 6)) as u32;
        let len = (((data[3] & 0x03) as usize) << 11) | ((data[4] as usize) << 3) | ((data[5] as usize) >> 5);
        if len < header || len > data.len() {
            break;
        }
        out.push((rate, channels.max(1), &data[header..len]));
        data = &data[len..];
    }
    out
}

// ---------------------------------------------------------------- the frames source

pub(crate) const FRAMES_ID: &CStr = c"lightair_phone_frames";

unsafe extern "C" fn frames_name(_: *mut c_void) -> *const std::ffi::c_char {
    c"Phone camera frames".as_ptr()
}

unsafe extern "C" fn frames_create(_: *mut sys::obs_data_t, _: *mut sys::obs_source_t) -> *mut c_void {
    Box::into_raw(Box::new(0u8)) as *mut c_void
}

unsafe extern "C" fn frames_destroy(data: *mut c_void) {
    drop(unsafe { Box::from_raw(data as *mut u8) });
}

/// Registers the private async source decoded frames go into (once).
pub(crate) fn register() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| unsafe {
        let mut info: sys::obs_source_info = std::mem::zeroed();
        info.id = FRAMES_ID.as_ptr();
        info.type_ = sys::OBS_SOURCE_TYPE_INPUT;
        info.output_flags = sys::OBS_SOURCE_ASYNC_VIDEO | sys::OBS_SOURCE_AUDIO | sys::OBS_SOURCE_DO_NOT_DUPLICATE;
        info.get_name = Some(frames_name);
        info.create = Some(frames_create);
        info.destroy = Some(frames_destroy);
        sys::obs_register_source_s(&info, std::mem::size_of::<sys::obs_source_info>());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_and_adts_split() {
        let au = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        let n = nals(&au);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]);
        assert_eq!(n.iter().map(|x| parameter_set(false, x)).collect::<Vec<_>>(), [Some(1), Some(2), None]);
        // two ADTS frames: 48 kHz (index 3), mono, 7-byte headers
        let frame = |payload: &[u8]| {
            let len = 7 + payload.len();
            let mut h = vec![
                0xFF,
                0xF1,
                (1 << 6) | (3 << 2),
                (1 << 6) | ((len >> 11) & 0x03) as u8,
                ((len >> 3) & 0xFF) as u8,
                (((len & 7) << 5) | 0x1F) as u8,
                0xFC,
            ];
            h.extend_from_slice(payload);
            h
        };
        let mut data = frame(&[1, 2, 3]);
        data.extend(frame(&[4, 5]));
        let got = adts(&data);
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].0, got[0].1, got[0].2), (48_000, 1, &[1, 2, 3][..]));
        assert_eq!(got[1].2, &[4, 5][..]);
    }
}
