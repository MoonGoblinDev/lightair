//! A phone as a camera layer (docs/07-scenes-layers-canvas.md, LY-16; lightair-live's docs/05 §6),
//! the parts that don't need libobs or the UI:
//!
//! - [`srt`]: the listener phones send their MPEG-TS to over Wi-Fi (SRT, the phone calling, AES-128
//!   with a passphrase per session), each stream handed on as UDP datagrams to this machine's
//!   decoder (libobs's FFmpeg has no SRT; `lightair_engine::phone_camera` decodes from UDP).
//!
//! - [`link`]: the USB link (Lightair Live's `lightair_link::usb`): control tunnelled to the
//!   phones' TLS port, media on its own channel; reached through [`usbmux`] (iPhones), [`aoa`]
//!   (Android accessories) or [`tcp`] (the iOS simulator, `adb forward`).
//!
//! Which phone may send for which source, and with which passphrase, is the app's: it answers
//! through [`Feeds`].

pub mod aoa;
pub mod keep;
pub mod link;
pub mod srt;
pub mod tcp;
pub mod usbmux;

use std::net::SocketAddr;

/// The port phones send their pictures to (lightair-live's docs/05 §6: 4460 + n; 4460 itself is
/// the widget server's, over TCP), one for every phone camera on this computer: the SRT stream id
/// says which source a stream is for. Another is used when it's taken; phones are told which.
pub const SRT_PORT: u16 = 4461;

/// What the app knows about phone camera feeds, asked from the receiving thread.
pub trait Feeds: Send + Sync {
    /// The passphrase a source's stream must use right now, or `None` to refuse it (no such
    /// source, or no phone paired to it).
    fn passphrase(&self, source: &str) -> Option<String>;
    /// A stream for `source` started: where its datagrams go (the decoder), or `None` to drop it.
    fn started(&self, source: &str, peer: SocketAddr) -> Option<SocketAddr>;
    /// `n` bytes arrived for `source` and went on.
    fn data(&self, source: &str, n: usize);
    /// The stream for `source` ended (the phone left, or another stream took its place).
    fn ended(&self, source: &str);
}

/// For looking into a phone's stream: with `LIGHTAIR_PHONE_FEED_DUMP=<folder>`, each stream
/// that starts is also written to `<folder>/<source>-<ms>.ts` as it came.
pub fn dump_file(source: &str) -> Option<std::fs::File> {
    let dir = std::path::PathBuf::from(std::env::var_os("LIGHTAIR_PHONE_FEED_DUMP")?);
    std::fs::create_dir_all(&dir).ok()?;
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let name: String = source.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    std::fs::File::create(dir.join(format!("{name}-{ms}.ts"))).ok()
}

/// The source a stream is for, from its SRT stream id: `#!::r=<source>,m=publish` (the access
/// control syntax), or a bare id.
pub fn source_of_stream_id(id: &str) -> Option<String> {
    let Some(rest) = id.strip_prefix("#!::") else {
        return (!id.is_empty() && !id.contains([',', '=', '#'])).then(|| id.to_owned());
    };
    let mut source = None;
    for kv in rest.split(',') {
        match kv.split_once('=') {
            Some(("r", v)) if !v.is_empty() => source = Some(v.to_owned()),
            // only publishing (a phone sending); a request to play is refused
            Some(("m", m)) if m != "publish" => return None,
            _ => {}
        }
    }
    source
}

#[cfg(test)]
mod tests {
    use super::source_of_stream_id;

    #[test]
    fn stream_ids_name_their_source() {
        assert_eq!(source_of_stream_id("#!::r=0193abcd,m=publish").as_deref(), Some("0193abcd"));
        assert_eq!(source_of_stream_id("#!::m=publish,r=cam-1").as_deref(), Some("cam-1"));
        assert_eq!(source_of_stream_id("#!::r=cam-1").as_deref(), Some("cam-1"));
        assert_eq!(source_of_stream_id("#!::r=cam-1,m=request"), None);
        assert_eq!(source_of_stream_id("#!::m=publish"), None);
        assert_eq!(source_of_stream_id("cam-1").as_deref(), Some("cam-1"));
        assert_eq!(source_of_stream_id(""), None);
    }
}
