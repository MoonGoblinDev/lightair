//! MPEG-TS from a phone, split into access units as it arrives (the stream Lightair Live's
//! `lightair-feed` makes: PAT and PMT, H.264 or HEVC video, AAC in ADTS, one access unit per
//! PES). Whole packets in, whole units out, each with its PTS (and DTS) on the 90 kHz clock.
//!
//! A video PES of open length (0, as video may be) is known to be complete only when the next
//! one starts, one frame later; a PES whose length is given is handed on the moment it's whole.

use std::collections::HashMap;

pub const PACKET: usize = 188;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Video(Codec),
    /// AAC in ADTS.
    Audio,
}

/// One access unit: a video frame (Annex B) or AAC frames (ADTS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub kind: Kind,
    pub pts: u64,
    pub dts: Option<u64>,
    pub data: Vec<u8>,
}

struct Pes {
    /// The PES bytes so far (header included).
    bytes: Vec<u8>,
    /// Its whole length when the header says (6 + the length field), else open.
    whole: Option<usize>,
}

#[derive(Default)]
pub struct Demuxer {
    pmt: Option<u16>,
    streams: HashMap<u16, Kind>,
    pes: HashMap<u16, Pes>,
    /// Bytes of a packet cut off at the end of the last push.
    partial: Vec<u8>,
}

/// A PTS or DTS: 33 bits in five bytes.
fn timestamp(b: &[u8]) -> u64 {
    (((b[0] as u64 >> 1) & 0x07) << 30)
        | ((b[1] as u64) << 22)
        | (((b[2] as u64) >> 1) << 15)
        | ((b[3] as u64) << 7)
        | ((b[4] as u64) >> 1)
}

impl Demuxer {
    pub fn new() -> Demuxer {
        Demuxer::default()
    }

    /// The units completed by these bytes (whole packets, or packets cut anywhere).
    pub fn push(&mut self, data: &[u8]) -> Vec<Unit> {
        let mut out = vec![];
        let mut buf = std::mem::take(&mut self.partial);
        buf.extend_from_slice(data);
        let mut i = 0;
        while i + PACKET <= buf.len() {
            if buf[i] != 0x47 {
                // lost sync: find the next packet start
                i += 1;
                continue;
            }
            self.packet(&buf[i..i + PACKET], &mut out);
            i += PACKET;
        }
        self.partial = buf[i..].to_vec();
        out
    }

    fn packet(&mut self, p: &[u8], out: &mut Vec<Unit>) {
        let start = p[1] & 0x40 != 0;
        let pid = (((p[1] & 0x1F) as u16) << 8) | p[2] as u16;
        let afc = (p[3] >> 4) & 0x03;
        if afc & 0x01 == 0 {
            return;
        }
        let at = if afc & 0x02 != 0 { 5 + p[4] as usize } else { 4 };
        if at >= PACKET {
            return;
        }
        let payload = &p[at..];
        if pid == 0 {
            if start {
                self.pat(payload);
            }
        } else if Some(pid) == self.pmt {
            if start {
                self.pmt(payload);
            }
        } else if let Some(kind) = self.streams.get(&pid).cloned() {
            if start {
                // the one before it is whole now
                if let Some(prev) = self.pes.remove(&pid) {
                    out.extend(unit(&kind, &prev.bytes));
                }
                let whole = (payload.len() >= 6)
                    .then(|| ((payload[4] as usize) << 8) | payload[5] as usize)
                    .filter(|l| *l > 0)
                    .map(|l| l + 6);
                self.pes.insert(pid, Pes { bytes: payload.to_vec(), whole });
            } else if let Some(pes) = self.pes.get_mut(&pid) {
                pes.bytes.extend_from_slice(payload);
            }
            // a PES of known length: handed on the moment it's whole
            if let Some(pes) = self.pes.get(&pid)
                && pes.whole.is_some_and(|w| pes.bytes.len() >= w)
            {
                let pes = self.pes.remove(&pid).expect("just seen");
                let w = pes.whole.unwrap_or(pes.bytes.len());
                out.extend(unit(&kind, &pes.bytes[..w.min(pes.bytes.len())]));
            }
        }
    }

    /// The program's PMT PID, from the PAT.
    fn pat(&mut self, payload: &[u8]) {
        let Some(s) = section(payload) else { return };
        let len = (((s[1] & 0x0F) as usize) << 8) | s[2] as usize;
        let end = (3 + len).saturating_sub(4).min(s.len());
        let mut i = 8;
        while i + 4 <= end {
            let program = ((s[i] as u16) << 8) | s[i + 1] as u16;
            let pid = (((s[i + 2] & 0x1F) as u16) << 8) | s[i + 3] as u16;
            if program != 0 {
                self.pmt = Some(pid);
                return;
            }
            i += 4;
        }
    }

    /// The streams, from the PMT.
    fn pmt(&mut self, payload: &[u8]) {
        let Some(s) = section(payload) else { return };
        if s.len() < 12 {
            return;
        }
        let len = (((s[1] & 0x0F) as usize) << 8) | s[2] as usize;
        let end = (3 + len).saturating_sub(4).min(s.len());
        let info = (((s[10] & 0x0F) as usize) << 8) | s[11] as usize;
        let mut i = 12 + info;
        let mut streams = HashMap::new();
        while i + 5 <= end {
            let kind = s[i];
            let pid = (((s[i + 1] & 0x1F) as u16) << 8) | s[i + 2] as u16;
            let es = (((s[i + 3] & 0x0F) as usize) << 8) | s[i + 4] as usize;
            match kind {
                0x1B => streams.insert(pid, Kind::Video(Codec::H264)),
                0x24 => streams.insert(pid, Kind::Video(Codec::Hevc)),
                0x0F => streams.insert(pid, Kind::Audio),
                _ => None,
            };
            i += 5 + es;
        }
        if streams != self.streams {
            self.pes.retain(|pid, _| streams.contains_key(pid));
            self.streams = streams;
        }
    }

    /// The video codec, once the PMT has said.
    pub fn video_codec(&self) -> Option<Codec> {
        self.streams.values().find_map(|k| match k {
            Kind::Video(c) => Some(*c),
            Kind::Audio => None,
        })
    }
}

/// A section from a payload that starts one (after its pointer field).
fn section(payload: &[u8]) -> Option<&[u8]> {
    let skip = *payload.first()? as usize;
    payload.get(1 + skip..).filter(|s| s.len() >= 3)
}

fn unit(kind: &Kind, pes: &[u8]) -> Option<Unit> {
    if pes.len() < 9 || pes[0..3] != [0, 0, 1] {
        return None;
    }
    let flags = pes[7] >> 6;
    let header = 9 + pes[8] as usize;
    if pes.len() < header || flags & 0x02 == 0 {
        return None;
    }
    let pts = timestamp(&pes[9..14]);
    let dts = (flags == 0x03 && pes.len() >= 19).then(|| timestamp(&pes[14..19]));
    Some(Unit { kind: kind.clone(), pts, dts, data: pes[header..].to_vec() })
}

/// Whether an Annex B H.264 or HEVC access unit holds a keyframe (IDR, or HEVC IRAP).
pub fn is_keyframe(codec: Codec, au: &[u8]) -> bool {
    nals(au).any(|n| match codec {
        Codec::H264 => n.first().is_some_and(|b| b & 0x1F == 5),
        Codec::Hevc => n.first().is_some_and(|b| (16..=23).contains(&((b >> 1) & 0x3F))),
    })
}

/// The NAL units of an Annex B access unit, without their start codes.
pub fn nals(au: &[u8]) -> impl Iterator<Item = &[u8]> {
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
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|s| {
            let mut e = s - 3;
            // a four-byte start code's leading zero belongs to it
            while e > 0 && au[e - 1] == 0 {
                e -= 1;
            }
            e
        })
        .chain(std::iter::once(au.len()))
        .collect();
    starts.into_iter().zip(ends).map(move |(s, e)| &au[s..e.max(s)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PES split into packets with a continuity counter (the phone's way).
    fn packets(pid: u16, pes: &[u8]) -> Vec<u8> {
        let mut out = vec![];
        let mut first = true;
        let mut cc = 0u8;
        for chunk in pes.chunks(184) {
            let mut p = vec![0x47, ((pid >> 8) as u8 & 0x1F) | if first { 0x40 } else { 0 }, pid as u8];
            if chunk.len() < 184 {
                // adaptation field stuffing to a whole packet
                let stuff = 184 - chunk.len();
                p.push(0x30 | cc);
                p.push((stuff - 1) as u8);
                if stuff > 1 {
                    p.push(0);
                    p.extend(std::iter::repeat_n(0xFF, stuff - 2));
                }
            } else {
                p.push(0x10 | cc);
            }
            p.extend_from_slice(chunk);
            out.extend(p);
            first = false;
            cc = (cc + 1) & 0x0F;
        }
        out
    }

    fn ts(t: u64, marker: u8) -> [u8; 5] {
        [
            (marker << 4) | (((t >> 30) as u8 & 0x07) << 1) | 1,
            (t >> 22) as u8,
            (((t >> 15) as u8) << 1) | 1,
            (t >> 7) as u8,
            ((t as u8) << 1) | 1,
        ]
    }

    fn pes(sid: u8, es: &[u8], pts: u64, length: bool) -> Vec<u8> {
        let mut p = vec![0, 0, 1, sid, 0, 0, 0x80, 0x80, 5];
        p.extend_from_slice(&ts(pts, 2));
        p.extend_from_slice(es);
        if length {
            let l = p.len() - 6;
            p[4] = (l >> 8) as u8;
            p[5] = l as u8;
        }
        p
    }

    fn tables() -> Vec<u8> {
        let mut pat = vec![0, 0x00, 0xB0, 13, 0, 1, 0xC1, 0, 0, 0, 1, 0xF0, 0x00, 0, 0, 0, 0];
        pat[11] = 0xE0 | 0x10;
        let mut pmt = vec![
            0, 0x02, 0xB0, 23, 0, 1, 0xC1, 0, 0, 0xE1, 0x00, 0xF0, 0x00, 0x1B, 0xE1, 0x00, 0xF0, 0x00, 0x0F, 0xE1,
            0x01, 0xF0, 0x00, 0, 0, 0, 0,
        ];
        pmt[9] = 0xE1;
        let mut out = vec![];
        for (pid, s) in [(0u16, pat), (0x1000u16, pmt)] {
            let mut p = vec![0x47, 0x40 | (pid >> 8) as u8, pid as u8, 0x10];
            p.extend_from_slice(&s);
            p.resize(PACKET, 0xFF);
            out.extend(p);
        }
        out
    }

    #[test]
    fn units_come_out_with_their_timestamps() {
        let idr = [0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x65, 9, 9, 9];
        let frame: Vec<u8> = idr.iter().copied().chain(std::iter::repeat_n(7, 1000)).collect();
        let mut stream = tables();
        stream.extend(packets(0x100, &pes(0xE0, &frame, 900_000, false)));
        stream.extend(packets(0x101, &pes(0xC0, &[0xFF, 0xF1, 1, 2, 3], 900_100, true)));
        stream.extend(packets(0x100, &pes(0xE0, &[0, 0, 1, 0x41, 5, 5], 903_000, false)));
        let mut d = Demuxer::new();
        // however it's cut
        let mut units = vec![];
        for piece in stream.chunks(100) {
            units.extend(d.push(piece));
        }
        assert_eq!(d.video_codec(), Some(Codec::H264));
        // the audio as soon as it's whole (its length is given); the first frame once the
        // second starts; the second still open
        assert_eq!(units.len(), 2, "{units:?}");
        assert_eq!(
            (units[0].kind.clone(), units[0].pts, units[0].data.as_slice()),
            (Kind::Audio, 900_100, &[0xFF, 0xF1, 1, 2, 3][..])
        );
        assert_eq!((units[1].kind.clone(), units[1].pts), (Kind::Video(Codec::H264), 900_000));
        assert_eq!(units[1].data, frame);
        assert!(is_keyframe(Codec::H264, &units[1].data));
        assert!(!is_keyframe(Codec::H264, &[0, 0, 1, 0x41, 5, 5]));
        assert_eq!(nals(&idr).map(|n| n[0]).collect::<Vec<_>>(), [0x67, 0x65]);
    }
}
