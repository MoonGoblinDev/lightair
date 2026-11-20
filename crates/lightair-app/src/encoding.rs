//! From settings to encoders (docs/11-outputs-streaming-recording.md, "Encoding"): which libobs
//! encoder a choice means on this machine, the size it encodes at, and what Automatic picks for
//! a destination.

use lightair_model::{Destination, Encoding, Canvas, Workspace, RecordingSettings, VideoEncoder};
use lightair_engine::outputs::{
    EncoderConfig, H264_ENCODERS, HEVC_ENCODERS, PRORES_ENCODERS, aac_encoder, first_available,
};

use crate::platforms;

/// The libobs encoder for a choice, among those this machine has; hardware H.264 (then x264)
/// when the choice isn't available.
pub fn encoder_id(available: &[String], choice: VideoEncoder) -> String {
    let h264 = || first_available(available, H264_ENCODERS).unwrap_or_else(|| "obs_x264".into());
    match choice {
        VideoEncoder::H264 => h264(),
        VideoEncoder::Hevc => first_available(available, HEVC_ENCODERS).unwrap_or_else(h264),
        VideoEncoder::ProRes => first_available(available, PRORES_ENCODERS).unwrap_or_else(h264),
        VideoEncoder::X264 => "obs_x264".into(),
    }
}

/// Whether this machine has an encoder for the choice.
pub fn available(available: &[String], choice: VideoEncoder) -> bool {
    let id = encoder_id(available, choice);
    match choice {
        VideoEncoder::H264 => true,
        VideoEncoder::Hevc => HEVC_ENCODERS.contains(&id.as_str()),
        VideoEncoder::ProRes => id.contains("prores"),
        VideoEncoder::X264 => available.iter().any(|e| e == "obs_x264"),
    }
}

/// The size to encode at for a height (the width follows the canvas, both even); `None` when
/// that is the canvas's own size or larger.
pub fn scaled(canvas: &Canvas, height: Option<u32>) -> Option<(u32, u32)> {
    let h = height?;
    // "1080p" means the short side: a vertical canvas scales its width to it.
    let (w, ch) = (canvas.width as f32, canvas.height as f32);
    let (sw, sh) =
        if canvas.height > canvas.width { (h as f32, h as f32 * ch / w) } else { (h as f32 * w / ch, h as f32) };
    let (sw, sh) = ((sw.round() as u32) & !1, (sh.round() as u32) & !1);
    (sw < canvas.width || sh < canvas.height).then_some((sw, sh))
}

/// What Automatic means for a destination: the shared bitrate, capped at what its platform
/// takes; H.264, 2-second keyframes, the canvas's size.
pub fn automatic(workspace: &Workspace, d: &Destination) -> Encoding {
    let platform = d.platform.as_deref().and_then(platforms::by_id).or_else(|| platforms::guess(&d.server));
    let cap = platform.and_then(|p| p.max_kbps).unwrap_or(u32::MAX);
    Encoding { bitrate_kbps: workspace.outputs.stream_bitrate_kbps.min(cap), ..Encoding::default() }
}

/// A destination's encoders.
pub fn for_stream(caps: &[String], workspace: &Workspace, d: &Destination) -> EncoderConfig {
    let e = d.encoding.unwrap_or_else(|| automatic(workspace, d));
    let canvas = d.canvas.and_then(|f| workspace.canvas(f)).unwrap_or(workspace.primary_canvas());
    EncoderConfig {
        video_encoder: encoder_id(caps, e.encoder),
        video_bitrate_kbps: e.bitrate_kbps,
        audio_encoder: audio_id(caps),
        audio_bitrate_kbps: e.audio_kbps,
        constant_bitrate: true,
        bframes: true,
        keyint_sec: e.keyframe_s.max(1),
        scaled: scaled(canvas, e.height),
        // its sound: a mix, or the master
        audio_mix: d.audio_mix.and_then(|m| libobs_mix(workspace, m)).unwrap_or(0),
    }
}

/// The libobs mix a mixer mix plays into: the audio engine plays the first five mixes, in mixer
/// order, into libobs mixes 1–5 (0 is Program, the master); `None` past those or when gone.
pub fn libobs_mix(workspace: &Workspace, mix: lightair_model::Id) -> Option<usize> {
    workspace.audio.mixes.iter().position(|m| m.id == mix).filter(|&k| k < 5).map(|k| k + 1)
}

/// A recording's encoders for one canvas.
pub fn for_recording(caps: &[String], settings: &RecordingSettings, canvas: &Canvas) -> EncoderConfig {
    EncoderConfig {
        video_encoder: encoder_id(caps, settings.encoder),
        video_bitrate_kbps: settings.bitrate_kbps,
        audio_encoder: audio_id(caps),
        audio_bitrate_kbps: settings.audio_kbps,
        constant_bitrate: false,
        bframes: true,
        keyint_sec: 2,
        scaled: scaled(canvas, settings.height),
        audio_mix: 0,
    }
}

fn audio_id(caps: &[String]) -> String {
    aac_encoder(caps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac() -> Vec<String> {
        [
            "obs_x264",
            "com.apple.videotoolbox.videoencoder.ave.avc",
            "com.apple.videotoolbox.videoencoder.ave.hevc",
            "com.apple.videotoolbox.videoencoder.appleproreshw.422",
            "CoreAudio_AAC",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn choices_map_to_this_macs_encoders() {
        let caps = mac();
        assert!(encoder_id(&caps, VideoEncoder::H264).ends_with("ave.avc"));
        assert!(encoder_id(&caps, VideoEncoder::Hevc).ends_with("ave.hevc"));
        assert!(encoder_id(&caps, VideoEncoder::ProRes).contains("prores"));
        // No HEVC here: H.264 instead, and the choice reads as unavailable.
        let old = vec!["obs_x264".to_string(), "com.apple.videotoolbox.videoencoder.h264".to_string()];
        assert!(encoder_id(&old, VideoEncoder::Hevc).ends_with("h264"));
        assert!(!available(&old, VideoEncoder::Hevc));
    }

    #[test]
    fn choices_map_to_a_pcs_encoders() {
        let nvidia: Vec<String> =
            ["obs_x264", "obs_nvenc_h264_tex", "obs_nvenc_hevc_tex", "ffmpeg_aac"].map(String::from).to_vec();
        assert_eq!(encoder_id(&nvidia, VideoEncoder::H264), "obs_nvenc_h264_tex");
        assert_eq!(encoder_id(&nvidia, VideoEncoder::Hevc), "obs_nvenc_hevc_tex");
        assert!(available(&nvidia, VideoEncoder::Hevc));
        // no ProRes in libobs on Windows: H.264, and the choice reads as unavailable
        assert_eq!(encoder_id(&nvidia, VideoEncoder::ProRes), "obs_nvenc_h264_tex");
        assert!(!available(&nvidia, VideoEncoder::ProRes));
        assert_eq!(audio_id(&nvidia), "ffmpeg_aac");
        let intel: Vec<String> = ["obs_x264", "obs_qsv11_v2"].map(String::from).to_vec();
        assert_eq!(encoder_id(&intel, VideoEncoder::H264), "obs_qsv11_v2");
        assert_eq!(encoder_id(&intel, VideoEncoder::Hevc), "obs_qsv11_v2", "no HEVC: H.264");
    }

    #[test]
    fn heights_scale_by_the_short_side() {
        let landscape = Canvas::landscape();
        assert_eq!(scaled(&landscape, Some(720)), Some((1280, 720)));
        assert_eq!(scaled(&landscape, Some(1080)), None, "its own size");
        assert_eq!(scaled(&landscape, None), None);
        let vertical = Canvas::vertical();
        assert_eq!(scaled(&vertical, Some(720)), Some((720, 1280)));
    }

    #[test]
    fn automatic_caps_at_the_platform() {
        let mut workspace = Workspace::new("P");
        workspace.outputs.stream_bitrate_kbps = 9000;
        let d = |platform: &str| Destination {
            id: lightair_model::Id::new(),
            name: platform.into(),
            server: "rtmp://example.com/live".into(),
            enabled: true,
            canvas: None,
            platform: Some(platform.into()),
            encoding: None,
            audio_mix: None,
            vod_mix: None,
            chat: None,
        };
        assert_eq!(automatic(&workspace, &d("twitch")).bitrate_kbps, 6000, "Twitch takes 6 Mbps");
        assert_eq!(automatic(&workspace, &d("youtube")).bitrate_kbps, 9000);
        let cfg = for_stream(&mac(), &workspace, &d("youtube"));
        assert!(cfg.constant_bitrate && cfg.keyint_sec == 2 && cfg.scaled.is_none());
    }
}
