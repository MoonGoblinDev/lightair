//! A sequence exported with the export panel's settings: each container and codec, a size and
//! rate of its own, a stretch of time, sound only, frames. Needs FFmpeg (skipped without it)
//! and a GPU.

use std::path::Path;
use std::process::Command;

use lightair_render::exporter::{AudioCodec, Container, ExportSettings, Rate, VideoCodec, PRESETS};
use lightair_render::media;
use lightair_render::seqrender::SequenceRenderer;
use lightair_render::sequence::{Media, Sequence, Transform};

/// ffprobe's word on a file: (video codec, width, height, fps, audio codec, sample rate,
/// channels, duration ms).
#[derive(Debug, Default)]
struct Probed {
    video: String,
    width: u32,
    height: u32,
    fps: f64,
    audio: String,
    sample_rate: u32,
    channels: u32,
    duration_ms: u64,
}

fn probed(path: &Path) -> Probed {
    let ffprobe = media::ffmpeg().unwrap().with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    let ffprobe = if ffprobe.exists() { ffprobe } else { "ffprobe".into() };
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height,r_frame_rate,sample_rate,channels:format=duration", "-of", "json"])
        .arg(path)
        .output()
        .unwrap();
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut p = Probed { duration_ms: (j["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0) * 1000.0) as u64, ..Default::default() };
    for s in j["streams"].as_array().cloned().unwrap_or_default() {
        match s["codec_type"].as_str() {
            Some("video") => {
                p.video = s["codec_name"].as_str().unwrap_or("").into();
                p.width = s["width"].as_u64().unwrap_or(0) as u32;
                p.height = s["height"].as_u64().unwrap_or(0) as u32;
                let r = s["r_frame_rate"].as_str().unwrap_or("0/1");
                let (n, d) = r.split_once('/').unwrap_or((r, "1"));
                p.fps = n.parse::<f64>().unwrap_or(0.0) / d.parse::<f64>().unwrap_or(1.0).max(1.0);
            }
            Some("audio") => {
                p.audio = s["codec_name"].as_str().unwrap_or("").into();
                p.sample_rate = s["sample_rate"].as_str().and_then(|r| r.parse().ok()).unwrap_or(0);
                p.channels = s["channels"].as_u64().unwrap_or(0) as u32;
            }
            _ => {}
        }
    }
    p
}

#[test]
fn every_kind_of_export() {
    let dir = tempfile::tempdir().unwrap();
    let Ok(ff) = media::ffmpeg() else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let red = dir.path().join("red.mp4");
    let tone = dir.path().join("tone.wav");
    assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=640x360:r=30:d=3", "-pix_fmt", "yuv420p"]).arg(&red).status().unwrap().success());
    assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"]).arg(&tone).status().unwrap().success());
    let mut seq = Sequence::new((640, 360), 30);
    let r = seq.add_media(Media::from_file(&red).unwrap());
    let t = seq.add_media(Media::from_file(&tone).unwrap());
    seq.append(r).unwrap();
    seq.place(t, 0, None).unwrap();
    let mut renderer = match SequenceRenderer::new(&seq) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no renderer here ({e}); skipping");
            return;
        }
    };
    let all = (0, seq.duration_ms());
    let mut export = |name: &str, s: ExportSettings, range: (u64, u64)| {
        let written = renderer.export_with(&dir.path().join(name), &s, range, |_| true).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(written.exists(), "{name}: {}", written.display());
        written
    };

    // the standard: H.264 and AAC, the project's size and rate
    let p = probed(&export("standard.mp4", ExportSettings::default(), all));
    assert_eq!((p.video.as_str(), p.width, p.height, p.audio.as_str()), ("h264", 640, 360, "aac"), "{p:?}");
    assert!((p.fps - 30.0).abs() < 0.01 && (p.duration_ms as i64 - 3_000).abs() < 200, "{p:?}");

    // HEVC, by bitrate, a size and rate of its own (fitted), mono 44.1 kHz, software
    let s = ExportSettings {
        video: VideoCodec::Hevc,
        hardware: false,
        rate: Rate::Bitrate(2_000),
        size: Some((1280, 720)),
        fps: Some(25.0),
        sample_rate: 44_100,
        channels: 1,
        ..ExportSettings::default()
    };
    let p = probed(&export("hevc.mp4", s, all));
    assert_eq!((p.video.as_str(), p.width, p.height, p.sample_rate, p.channels), ("hevc", 1280, 720, 44_100, 1), "{p:?}");
    assert!((p.fps - 25.0).abs() < 0.01, "{p:?}");

    // ProRes 422 HQ with 24-bit PCM, at 29.97
    let p = probed(&export("prores", ExportSettings { fps: Some(29.97), ..PRESETS[4].2 }, all));
    assert_eq!((p.video.as_str(), p.audio.as_str()), ("prores", "pcm_s24le"), "{p:?}");
    assert!((p.fps - 29.97).abs() < 0.01, "{p:?}");

    // VP9 and Opus in WebM; only the second second
    let p = probed(&export("web", PRESETS[6].2, (1_000, 2_000)));
    assert_eq!((p.video.as_str(), p.audio.as_str()), ("vp9", "opus"), "{p:?}");
    assert!((p.duration_ms as i64 - 1_000).abs() < 150, "{p:?}");

    // DNxHR in MOV, and FFV1 with FLAC in MKV
    assert_eq!(probed(&export("dnx", PRESETS[5].2, all)).video, "dnxhd");
    let p = probed(&export("archive", PRESETS[7].2, (0, 1_000)));
    assert_eq!((p.video.as_str(), p.audio.as_str()), ("ffv1", "flac"), "{p:?}");

    // a GIF: no sound, at most 960 wide
    let p = probed(&export("anim", ExportSettings { size: Some((1920, 1080)), ..PRESETS[11].2 }, (0, 1_000)));
    assert_eq!((p.video.as_str(), p.width, p.audio.as_str()), ("gif", 960, ""), "{p:?}");

    // the sound alone: WAV and MP3, no picture
    let p = probed(&export("sound", PRESETS[13].2, all));
    assert_eq!((p.video.as_str(), p.audio.as_str()), ("", "pcm_s24le"), "{p:?}");
    assert!((p.duration_ms as i64 - 3_000).abs() < 150, "{p:?}");
    let mp3 = export("sound", ExportSettings { container: Container::Mp3, audio: Some(AudioCodec::Mp3), ..PRESETS[14].2 }, (0, 1_500));
    let p = probed(&mp3);
    assert_eq!(p.audio, "mp3", "{p:?}");
    assert!((p.duration_ms as i64 - 1_500).abs() < 150, "{p:?}");

    // PNG frames: a folder of them, one a frame
    let frames = export("frames", PRESETS[12].2, (0, 500));
    assert!(frames.is_dir());
    let n = std::fs::read_dir(&frames).unwrap().filter(|e| e.as_ref().is_ok_and(|e| e.path().extension().is_some_and(|x| x == "png"))).count();
    assert!((14..=16).contains(&n), "{n} frames for half a second at 30");
}

/// The pixel format and alpha mode (WebM's alpha is a side stream) of a file's video.
fn alpha_of(path: &Path) -> (String, bool) {
    let ffprobe = media::ffmpeg().unwrap().with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    let ffprobe = if ffprobe.exists() { ffprobe } else { "ffprobe".into() };
    let out = Command::new(ffprobe).args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=pix_fmt:stream_tags", "-of", "json"]).arg(path).output().unwrap();
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let s = &j["streams"][0];
    let pix = s["pix_fmt"].as_str().unwrap_or("").to_string();
    let tagged = s["tags"].as_object().is_some_and(|t| t.iter().any(|(k, v)| k.eq_ignore_ascii_case("alpha_mode") && v == "1"));
    (pix.clone(), pix.contains('a') && pix != "gray" || tagged)
}

#[test]
fn transparent_exports_keep_the_background_see_through() {
    let dir = tempfile::tempdir().unwrap();
    let Ok(ff) = media::ffmpeg() else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let red = dir.path().join("red.mp4");
    assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=640x360:r=30:d=1", "-pix_fmt", "yuv420p"]).arg(&red).status().unwrap().success());
    // a small red picture in the middle: around it, nothing
    let mut seq = Sequence::new((640, 360), 30);
    let r = seq.add_media(Media::from_file(&red).unwrap());
    let c = seq.append(r).unwrap();
    seq.clip_mut(c).unwrap().transform = Transform { scale: 0.4, ..Transform::default() };
    let mut renderer = match SequenceRenderer::new(&seq) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no renderer here ({e}); skipping");
            return;
        }
    };
    let range = (0, 500);
    // PNG frames: the corner clear, the middle solid red
    let frames = renderer.export_with(&dir.path().join("frames"), &ExportSettings { alpha: true, ..PRESETS[12].2 }, range, |_| true).unwrap();
    let first = frames.join("frames_00000.png");
    let raw = Command::new(&ff).args(["-v", "error", "-i"]).arg(&first).args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]).output().unwrap().stdout;
    assert_eq!(raw.len(), 640 * 360 * 4);
    let px = |x: usize, y: usize| { let i = (y * 640 + x) * 4; [raw[i], raw[i + 1], raw[i + 2], raw[i + 3]] };
    assert_eq!(px(5, 5)[3], 0, "the corner is see-through");
    let mid = px(320, 180);
    assert!(mid[3] > 250 && mid[0] > 200 && mid[2] < 60, "the middle is solid red: {mid:?}");
    // the same project, not transparent: a black corner, opaque
    let flat = renderer.export_with(&dir.path().join("flat"), &PRESETS[12].2, range, |_| true).unwrap();
    let raw2 = Command::new(&ff).args(["-v", "error", "-i"]).arg(flat.join("flat_00000.png")).args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]).output().unwrap().stdout;
    assert_eq!(&raw2[..4], &[0, 0, 0, 255], "without transparency, black");

    // the video formats that carry it
    let prores = renderer.export_with(&dir.path().join("prores"), &PRESETS[8].2, range, |_| true).unwrap();
    let (pix, alpha) = alpha_of(&prores);
    assert!(alpha && pix.starts_with("yuva444"), "ProRes 4444: {pix}");
    let webm = renderer.export_with(&dir.path().join("web"), &PRESETS[10].2, range, |_| true).unwrap();
    assert!(alpha_of(&webm).1, "WebM keeps its alpha: {:?}", alpha_of(&webm));
    if cfg!(target_os = "macos") && media::can_encode("hevc_videotoolbox") {
        let hevc = renderer.export_with(&dir.path().join("hevc"), &PRESETS[9].2, range, |_| true).unwrap();
        let p = probed(&hevc);
        assert_eq!(p.video, "hevc", "{p:?}");
    }
}
