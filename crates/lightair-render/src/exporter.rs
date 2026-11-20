//! What an export makes: its container, its video and sound and how they're encoded, its size,
//! rate and stretch of time; ready-made settings for where it's going (`PRESETS`); and the
//! FFmpeg arguments that make it (`Encoder::start_export`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file an export is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Container {
    Mp4,
    Mov,
    Mkv,
    WebM,
    Gif,
    /// A folder of numbered PNG frames.
    Png,
    Wav,
    M4a,
    Mp3,
    Flac,
}

impl Container {
    pub const ALL: [Container; 10] = [
        Container::Mp4,
        Container::Mov,
        Container::Mkv,
        Container::WebM,
        Container::Gif,
        Container::Png,
        Container::Wav,
        Container::M4a,
        Container::Mp3,
        Container::Flac,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mov => "QuickTime (MOV)",
            Container::Mkv => "Matroska (MKV)",
            Container::WebM => "WebM",
            Container::Gif => "GIF",
            Container::Png => "PNG frames",
            Container::Wav => "WAV",
            Container::M4a => "M4A",
            Container::Mp3 => "MP3",
            Container::Flac => "FLAC",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Mkv => "mkv",
            Container::WebM => "webm",
            Container::Gif => "gif",
            Container::Png => "png",
            Container::Wav => "wav",
            Container::M4a => "m4a",
            Container::Mp3 => "mp3",
            Container::Flac => "flac",
        }
    }

    pub fn has_video(self) -> bool {
        !matches!(self, Container::Wav | Container::M4a | Container::Mp3 | Container::Flac)
    }

    pub fn has_audio(self) -> bool {
        !matches!(self, Container::Gif | Container::Png)
    }

    /// The video codecs it holds, the usual one first.
    pub fn video_codecs(self) -> &'static [VideoCodec] {
        use VideoCodec::*;
        match self {
            Container::Mp4 => &[H264, Hevc, Av1],
            Container::Mov => &[ProRes, H264, Hevc, DnxHr],
            Container::Mkv => &[H264, Hevc, Av1, Vp9, Ffv1],
            Container::WebM => &[Vp9, Av1],
            Container::Gif => &[Gif],
            Container::Png => &[Png],
            _ => &[],
        }
    }

    /// The audio codecs it holds, the usual one first.
    pub fn audio_codecs(self) -> &'static [AudioCodec] {
        use AudioCodec::*;
        match self {
            Container::Mp4 => &[Aac, Alac, Mp3],
            Container::Mov => &[Aac, Pcm16, Pcm24, Alac],
            Container::Mkv => &[Aac, Opus, Flac, Pcm24, Mp3],
            Container::WebM => &[Opus],
            Container::Wav => &[Pcm16, Pcm24],
            Container::M4a => &[Aac, Alac],
            Container::Mp3 => &[Mp3],
            Container::Flac => &[Flac],
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    H264,
    Hevc,
    /// ProRes: `profile` 0 Proxy, 1 LT, 2 422, 3 HQ, 4 4444.
    ProRes,
    /// DNxHR: `profile` 0 LB, 1 SQ, 2 HQ, 3 HQX (10-bit).
    DnxHr,
    Av1,
    Vp9,
    /// Lossless.
    Ffv1,
    Gif,
    Png,
}

impl VideoCodec {
    pub fn name(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC (H.265)",
            VideoCodec::ProRes => "Apple ProRes",
            VideoCodec::DnxHr => "Avid DNxHR",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Ffv1 => "FFV1 (lossless)",
            VideoCodec::Gif => "GIF",
            VideoCodec::Png => "PNG",
        }
    }

    /// Its profiles, if it has a choice of them.
    pub fn profiles(self) -> &'static [&'static str] {
        match self {
            VideoCodec::ProRes => &["Proxy", "LT", "422", "HQ", "4444"],
            VideoCodec::DnxHr => &["LB", "SQ", "HQ", "HQX"],
            _ => &[],
        }
    }

    /// Whether its size is set by quality or bitrate (the others have a fixed rate, or none).
    pub fn rate_controlled(self) -> bool {
        matches!(self, VideoCodec::H264 | VideoCodec::Hevc | VideoCodec::Av1 | VideoCodec::Vp9)
    }

    /// Whether a hardware encoder can make it (this computer's: Apple's, NVIDIA's, AMD's, Intel's).
    pub fn hardware_possible(self) -> bool {
        matches!(self, VideoCodec::H264 | VideoCodec::Hevc | VideoCodec::ProRes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioCodec {
    Aac,
    /// 16-bit PCM (uncompressed).
    Pcm16,
    /// 24-bit PCM (uncompressed).
    Pcm24,
    Alac,
    Flac,
    Mp3,
    Opus,
}

impl AudioCodec {
    pub fn name(self) -> &'static str {
        match self {
            AudioCodec::Aac => "AAC",
            AudioCodec::Pcm16 => "PCM 16-bit",
            AudioCodec::Pcm24 => "PCM 24-bit",
            AudioCodec::Alac => "Apple Lossless",
            AudioCodec::Flac => "FLAC",
            AudioCodec::Mp3 => "MP3",
            AudioCodec::Opus => "Opus",
        }
    }

    /// Whether a bitrate is chosen (the lossless and uncompressed ones take what they need).
    pub fn has_bitrate(self) -> bool {
        matches!(self, AudioCodec::Aac | AudioCodec::Mp3 | AudioCodec::Opus)
    }
}

/// How big the video comes out: by quality (0 smallest to 100 best), or a bitrate (kbps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rate {
    Quality(u8),
    Bitrate(u32),
}

/// Everything an export is made with.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    pub container: Container,
    pub video: VideoCodec,
    /// The codec's profile (`VideoCodec::profiles`).
    pub profile: u8,
    /// A hardware encoder when there is one.
    pub hardware: bool,
    pub rate: Rate,
    /// 10 bits a colour (HEVC, AV1, ProRes and DNxHR HQX are 10-bit where they can be).
    pub ten_bit: bool,
    /// The frame size; `None`: the project's.
    pub size: Option<(u32, u32)>,
    /// Frames a second; `None`: the project's.
    pub fps: Option<f64>,
    /// The sound; `None`: none.
    pub audio: Option<AudioCodec>,
    pub audio_kbps: u32,
    pub sample_rate: u32,
    pub channels: u8,
    /// Keeps transparency: where nothing covers the frame, it's see-through (formats that can
    /// carry it: `ExportSettings::alpha_possible`).
    #[serde(default)]
    pub alpha: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        PRESETS[0].2
    }
}

const fn video(container: Container, video: VideoCodec, rate: Rate, audio: Option<AudioCodec>, audio_kbps: u32) -> ExportSettings {
    ExportSettings {
        container,
        video,
        profile: 2,
        hardware: true,
        rate,
        ten_bit: false,
        size: None,
        fps: None,
        audio,
        audio_kbps,
        sample_rate: 48_000,
        channels: 2,
        alpha: false,
    }
}

/// Ready-made settings: (name, what it's for, the settings).
pub const PRESETS: &[(&str, &str, ExportSettings)] = &[
    ("Standard", "H.264 MP4: plays anywhere, uploads anywhere", video(Container::Mp4, VideoCodec::H264, Rate::Quality(70), Some(AudioCodec::Aac), 192)),
    (
        "YouTube",
        "H.264, a high bitrate for YouTube's own encode",
        video(Container::Mp4, VideoCodec::H264, Rate::Quality(85), Some(AudioCodec::Aac), 320),
    ),
    (
        "Social (vertical)",
        "1080×1920 H.264 for Reels, TikTok and Shorts",
        ExportSettings { size: Some((1080, 1920)), ..video(Container::Mp4, VideoCodec::H264, Rate::Bitrate(12_000), Some(AudioCodec::Aac), 192) },
    ),
    ("Small file", "HEVC: half the size of H.264 for the same look", video(Container::Mp4, VideoCodec::Hevc, Rate::Quality(45), Some(AudioCodec::Aac), 128)),
    (
        "ProRes 422 HQ",
        "For editing elsewhere (Final Cut, Premiere, Resolve)",
        ExportSettings { profile: 3, ten_bit: true, ..video(Container::Mov, VideoCodec::ProRes, Rate::Quality(100), Some(AudioCodec::Pcm24), 192) },
    ),
    (
        "DNxHR HQ",
        "For Avid and Resolve",
        ExportSettings { hardware: false, ..video(Container::Mov, VideoCodec::DnxHr, Rate::Quality(100), Some(AudioCodec::Pcm24), 192) },
    ),
    (
        "Web (WebM)",
        "VP9 and Opus, for the web",
        ExportSettings { hardware: false, ..video(Container::WebM, VideoCodec::Vp9, Rate::Quality(70), Some(AudioCodec::Opus), 160) },
    ),
    (
        "Archive (lossless)",
        "FFV1 and FLAC in MKV: every pixel, big",
        ExportSettings { hardware: false, ..video(Container::Mkv, VideoCodec::Ffv1, Rate::Quality(100), Some(AudioCodec::Flac), 192) },
    ),
    (
        "ProRes 4444 (transparent)",
        "Keeps transparency, for compositing in any editor",
        ExportSettings { profile: 4, ten_bit: true, alpha: true, hardware: false, ..video(Container::Mov, VideoCodec::ProRes, Rate::Quality(100), Some(AudioCodec::Pcm24), 192) },
    ),
    (
        "HEVC (transparent)",
        "Keeps transparency, small: Final Cut, Motion, Safari (Mac)",
        ExportSettings { alpha: true, ..video(Container::Mov, VideoCodec::Hevc, Rate::Quality(85), Some(AudioCodec::Aac), 192) },
    ),
    (
        "WebM (transparent)",
        "Keeps transparency, VP9: browsers and overlays",
        ExportSettings { alpha: true, hardware: false, ..video(Container::WebM, VideoCodec::Vp9, Rate::Quality(85), Some(AudioCodec::Opus), 160) },
    ),
    (
        "GIF",
        "Animated, 15 fps, at most 960 wide, no sound",
        ExportSettings { size: None, fps: Some(15.0), hardware: false, ..video(Container::Gif, VideoCodec::Gif, Rate::Quality(70), None, 192) },
    ),
    (
        "PNG frames",
        "Every frame as a PNG image, in a folder",
        ExportSettings { hardware: false, ..video(Container::Png, VideoCodec::Png, Rate::Quality(100), None, 192) },
    ),
    (
        "Audio (WAV)",
        "The sound alone, uncompressed",
        ExportSettings { hardware: false, ..video(Container::Wav, VideoCodec::H264, Rate::Quality(70), Some(AudioCodec::Pcm24), 192) },
    ),
    (
        "Audio (MP3)",
        "The sound alone, small",
        ExportSettings { hardware: false, ..video(Container::Mp3, VideoCodec::H264, Rate::Quality(70), Some(AudioCodec::Mp3), 320) },
    ),
];

impl ExportSettings {
    /// Whether this container and codec can keep transparency (HEVC's: Apple's encoder only).
    pub fn alpha_possible(&self) -> bool {
        matches!(
            (self.container, self.video),
            (Container::Mov, VideoCodec::ProRes) | (Container::WebM | Container::Mkv, VideoCodec::Vp9) | (Container::Mkv, VideoCodec::Ffv1) | (Container::Png, VideoCodec::Png)
        ) || (cfg!(target_os = "macos") && (self.container, self.video) == (Container::Mov, VideoCodec::Hevc))
    }

    /// Whether this export keeps transparency.
    pub fn keeps_alpha(&self) -> bool {
        self.alpha && self.alpha_possible()
    }

    /// The settings made to fit together after one changed: a video codec and audio codec the
    /// container holds, a profile the codec has.
    pub fn fixed(mut self) -> ExportSettings {
        let vs = self.container.video_codecs();
        if !vs.is_empty() && !vs.contains(&self.video) {
            self.video = vs[0];
        }
        let n = self.video.profiles().len();
        if n > 0 && self.profile as usize >= n {
            self.profile = (n as u8).saturating_sub(1).min(2);
        }
        let aus = self.container.audio_codecs();
        match (self.container.has_audio(), self.audio) {
            (false, _) => self.audio = None,
            (true, Some(a)) if !aus.contains(&a) => self.audio = aus.first().copied(),
            (true, None) if !self.container.has_video() => self.audio = aus.first().copied(),
            _ => {}
        }
        if self.audio_kbps == 0 {
            self.audio_kbps = 192;
        }
        if self.alpha && !self.alpha_possible() {
            self.alpha = false;
        }
        // ProRes keeps transparency in 4444 only
        if self.alpha && self.video == VideoCodec::ProRes {
            self.profile = 4;
        }
        self
    }

    /// The frame size made from a project of `project` size.
    pub fn size_for(&self, project: (u32, u32)) -> (u32, u32) {
        let (w, h) = self.size.unwrap_or(project);
        match self.video {
            // a GIF at most 960 wide
            VideoCodec::Gif if w > 960 => (960, even(h as f64 * 960.0 / w as f64)),
            _ => (even(w as f64), even(h as f64)),
        }
    }

    /// Frames a second, from a project at `project`.
    pub fn fps_for(&self, project: f64) -> f64 {
        self.fps.unwrap_or(project).clamp(1.0, 240.0)
    }

    /// A rough size of the file (bytes) for `duration_ms`, when it can be known beforehand.
    pub fn estimate(&self, project: (u32, u32), project_fps: f64, duration_ms: u64) -> Option<u64> {
        let secs = duration_ms as f64 / 1000.0;
        let (w, h) = self.size_for(project);
        let fps = self.fps_for(project_fps);
        let pixels = w as f64 * h as f64 * fps;
        let video_kbps = if !self.container.has_video() {
            0.0
        } else {
            match (self.video, self.rate) {
                (v, Rate::Bitrate(k)) if v.rate_controlled() => k as f64,
                (VideoCodec::H264, Rate::Quality(q)) => pixels / 30_000.0 * quality_factor(q),
                (VideoCodec::Hevc | VideoCodec::Av1 | VideoCodec::Vp9, Rate::Quality(q)) => pixels / 55_000.0 * quality_factor(q),
                // ProRes (Apple's rates at 1080p30, scaled by pixels)
                (VideoCodec::ProRes, _) => [45_000.0, 102_000.0, 147_000.0, 220_000.0, 330_000.0][self.profile.min(4) as usize] * pixels / (1920.0 * 1080.0 * 30.0),
                (VideoCodec::DnxHr, _) => [45_000.0, 145_000.0, 220_000.0, 220_000.0][self.profile.min(3) as usize] * pixels / (1920.0 * 1080.0 * 30.0),
                _ => return None,
            }
        };
        let audio_kbps = match self.audio {
            Some(a) if a.has_bitrate() => self.audio_kbps as f64,
            Some(AudioCodec::Pcm16) => self.sample_rate as f64 * 16.0 * self.channels as f64 / 1000.0,
            Some(AudioCodec::Pcm24) => self.sample_rate as f64 * 24.0 * self.channels as f64 / 1000.0,
            Some(_) => self.sample_rate as f64 * 12.0 * self.channels as f64 / 1000.0,
            None => 0.0,
        };
        Some(((video_kbps + audio_kbps) * 1000.0 / 8.0 * secs) as u64)
    }

    /// Where it's written for `out` (PNG frames: a folder named like it).
    pub fn target(&self, out: &Path) -> PathBuf {
        if self.container == Container::Png {
            let dir = out.with_extension("");
            let stem = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "frame".into());
            dir.join(format!("{stem}_%05d.png"))
        } else {
            out.with_extension(self.container.ext())
        }
    }

    /// FFmpeg's video arguments: the filter that sizes the picture (from frames `source` big)
    /// and the encoder's options. `has(encoder)` says whether this FFmpeg encodes with it.
    pub fn video_args(&self, source: (u32, u32), fps: f64, has: &dyn Fn(&str) -> bool) -> (Option<String>, Vec<String>) {
        let (w, h) = self.size_for(source);
        let alpha = self.keeps_alpha() && (self.video != VideoCodec::Hevc || has("hevc_videotoolbox"));
        // frames come premultiplied (the GPU blends that way): straight alpha for the encoder
        let pad = if alpha { "black@0" } else { "black" };
        let fit = (source != (w, h)).then(|| format!("scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color={pad},setsar=1"));
        let fit = match (alpha, fit) {
            (true, Some(f)) => Some(format!("unpremultiply=inplace=1,{f}")),
            (true, None) => Some("unpremultiply=inplace=1".to_string()),
            (false, f) => f,
        };
        let v = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let q = match self.rate {
            Rate::Quality(q) => q.min(100) as f64,
            Rate::Bitrate(_) => 70.0,
        };
        // quality 0..100 to an encoder's CRF (lower is better), `best`..`worst`
        let crf = |best: f64, worst: f64| format!("{:.0}", worst - (worst - best) * q / 100.0);
        let bitrate = |a: &mut Vec<String>| {
            if let Rate::Bitrate(k) = self.rate {
                a.extend(v(&["-b:v", &format!("{k}k"), "-maxrate", &format!("{}k", k * 3 / 2), "-bufsize", &format!("{}k", k * 2)]));
                true
            } else {
                false
            }
        };
        let hw = |e: &str| self.hardware && has(e);
        let mut a = Vec::new();
        let mut filter = fit;
        match self.video {
            VideoCodec::H264 => {
                let enc = ["h264_videotoolbox", "h264_nvenc", "h264_amf", "h264_qsv"].into_iter().find(|e| hw(e)).unwrap_or("libx264");
                a.extend(v(&["-c:v", enc, "-pix_fmt", "yuv420p", "-profile:v", "high"]));
                if !bitrate(&mut a) {
                    match enc {
                        "libx264" => a.extend(v(&["-crf", &crf(14.0, 34.0), "-preset", "medium"])),
                        "h264_videotoolbox" => a.extend(v(&["-q:v", &format!("{:.0}", 30.0 + q * 0.65)])),
                        "h264_nvenc" => a.extend(v(&["-rc", "vbr", "-cq", &crf(16.0, 36.0)])),
                        _ => a.extend(v(&["-b:v", &format!("{:.0}k", w as f64 * h as f64 * fps / 30_000.0 * quality_factor(q as u8))])),
                    }
                }
                a.extend(v(&["-g", &format!("{:.0}", (fps * 2.0).round())]));
            }
            // HEVC with alpha: Apple's encoder, from BGRA
            VideoCodec::Hevc if alpha => {
                a.extend(v(&["-c:v", "hevc_videotoolbox", "-pix_fmt", "bgra", "-alpha_quality", "0.75", "-tag:v", "hvc1"]));
                if !bitrate(&mut a) {
                    a.extend(v(&["-q:v", &format!("{:.0}", 30.0 + q * 0.65)]));
                }
            }
            VideoCodec::Hevc => {
                let enc = ["hevc_videotoolbox", "hevc_nvenc", "hevc_amf", "hevc_qsv"].into_iter().find(|e| hw(e)).unwrap_or("libx265");
                let pix = match (self.ten_bit, enc) {
                    (true, "hevc_videotoolbox") => "p010le",
                    (true, _) => "yuv420p10le",
                    _ => "yuv420p",
                };
                a.extend(v(&["-c:v", enc, "-pix_fmt", pix, "-tag:v", "hvc1"]));
                if !bitrate(&mut a) {
                    match enc {
                        "libx265" => a.extend(v(&["-crf", &crf(16.0, 36.0), "-preset", "medium"])),
                        "hevc_videotoolbox" => a.extend(v(&["-q:v", &format!("{:.0}", 30.0 + q * 0.65)])),
                        "hevc_nvenc" => a.extend(v(&["-rc", "vbr", "-cq", &crf(18.0, 38.0)])),
                        _ => a.extend(v(&["-b:v", &format!("{:.0}k", w as f64 * h as f64 * fps / 55_000.0 * quality_factor(q as u8))])),
                    }
                }
                a.extend(v(&["-g", &format!("{:.0}", (fps * 2.0).round())]));
            }
            VideoCodec::ProRes => {
                let p = self.profile.min(4);
                if alpha {
                    a.extend(v(&["-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le", "-alpha_bits", "16"]));
                } else if hw("prores_videotoolbox") && p < 4 {
                    a.extend(v(&["-c:v", "prores_videotoolbox", "-profile:v", &p.to_string(), "-pix_fmt", "p210le"]));
                } else if p == 4 {
                    a.extend(v(&["-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuv444p10le"]));
                } else {
                    a.extend(v(&["-c:v", "prores_ks", "-profile:v", &p.to_string(), "-pix_fmt", "yuv422p10le"]));
                }
                a.extend(v(&["-vendor", "apl0"]));
            }
            VideoCodec::DnxHr => {
                let (profile, pix) = match self.profile.min(3) {
                    0 => ("dnxhr_lb", "yuv422p"),
                    1 => ("dnxhr_sq", "yuv422p"),
                    2 => ("dnxhr_hq", "yuv422p"),
                    _ => ("dnxhr_hqx", "yuv422p10le"),
                };
                a.extend(v(&["-c:v", "dnxhd", "-profile:v", profile, "-pix_fmt", pix]));
            }
            VideoCodec::Av1 => {
                a.extend(v(&["-c:v", "libsvtav1", "-pix_fmt", if self.ten_bit { "yuv420p10le" } else { "yuv420p" }, "-preset", "8"]));
                if !bitrate(&mut a) {
                    a.extend(v(&["-crf", &crf(20.0, 50.0)]));
                }
                a.extend(v(&["-g", &format!("{:.0}", (fps * 5.0).round())]));
            }
            VideoCodec::Vp9 => {
                a.extend(v(&["-c:v", "libvpx-vp9", "-pix_fmt", if alpha { "yuva420p" } else { "yuv420p" }, "-row-mt", "1", "-deadline", "good", "-cpu-used", "4"]));
                if alpha {
                    a.extend(v(&["-auto-alt-ref", "0"]));
                }
                if !bitrate(&mut a) {
                    a.extend(v(&["-crf", &crf(18.0, 48.0), "-b:v", "0"]));
                }
            }
            VideoCodec::Ffv1 => a.extend(v(&["-c:v", "ffv1", "-level", "3", "-g", "1", "-slices", "12", "-pix_fmt", if alpha { "yuva444p10le" } else { "yuv444p10le" }])),
            VideoCodec::Gif => {
                // one palette for the whole clip, made from its own colours
                let fit = format!("fps={fps},scale={w}:{h}:flags=lanczos");
                filter = Some(format!("{fit},split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4"));
                a.extend(v(&["-loop", "0"]));
            }
            VideoCodec::Png => a.extend(v(&["-c:v", "png", "-pix_fmt", if alpha { "rgba" } else { "rgb24" }, "-start_number", "0"])),
        }
        (filter, a)
    }

    /// FFmpeg's audio arguments. `has(encoder)` says whether this FFmpeg encodes with it.
    pub fn audio_args(&self, has: &dyn Fn(&str) -> bool) -> Vec<String> {
        let Some(codec) = self.audio else { return vec!["-an".into()] };
        let k = format!("{}k", self.audio_kbps.clamp(32, 512));
        let mut a: Vec<String> = match codec {
            AudioCodec::Aac => vec!["-c:a".into(), if has("aac_at") { "aac_at" } else { "aac" }.into(), "-b:a".into(), k],
            AudioCodec::Pcm16 => vec!["-c:a".into(), "pcm_s16le".into()],
            AudioCodec::Pcm24 => vec!["-c:a".into(), "pcm_s24le".into()],
            AudioCodec::Alac => vec!["-c:a".into(), "alac".into()],
            AudioCodec::Flac => vec!["-c:a".into(), "flac".into()],
            AudioCodec::Mp3 => vec!["-c:a".into(), "libmp3lame".into(), "-b:a".into(), k],
            AudioCodec::Opus => vec!["-c:a".into(), "libopus".into(), "-b:a".into(), k],
        };
        // Opus is 48 kHz only
        let rate = if codec == AudioCodec::Opus { 48_000 } else { self.sample_rate };
        a.extend(["-ar".into(), rate.to_string(), "-ac".into(), self.channels.clamp(1, 2).to_string()]);
        a
    }
}

/// How much more a quality asks for than "good enough" (quality 70).
fn quality_factor(q: u8) -> f64 {
    0.35 + (q.min(100) as f64 / 100.0).powf(2.2) * 2.0
}

fn even(v: f64) -> u32 {
    ((v.round() as u32).max(2)) & !1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_fit_their_containers() {
        for (name, _, p) in PRESETS {
            assert_eq!(p.fixed(), *p, "{name}");
        }
    }

    #[test]
    fn a_container_change_fixes_the_codecs() {
        let s = ExportSettings { container: Container::WebM, ..ExportSettings::default() }.fixed();
        assert_eq!((s.video, s.audio), (VideoCodec::Vp9, Some(AudioCodec::Opus)));
        let s = ExportSettings { container: Container::Wav, ..ExportSettings::default() }.fixed();
        assert_eq!(s.audio, Some(AudioCodec::Pcm16));
        let s = ExportSettings { container: Container::Gif, ..ExportSettings::default() }.fixed();
        assert_eq!((s.video, s.audio), (VideoCodec::Gif, None));
    }

    #[test]
    fn sizes_fit_and_scale() {
        let s = ExportSettings { size: Some((1280, 720)), ..ExportSettings::default() };
        let (f, _) = s.video_args((1920, 1080), 30.0, &|_| false);
        assert!(f.unwrap().starts_with("scale=1280:720"));
        let (f, a) = ExportSettings::default().video_args((1920, 1080), 30.0, &|_| false);
        assert!(f.is_none());
        assert!(a.contains(&"libx264".to_string()));
        let gif = ExportSettings { container: Container::Gif, video: VideoCodec::Gif, ..ExportSettings::default() };
        assert_eq!(gif.size_for((1920, 1080)), (960, 540));
    }

    #[test]
    fn estimates_are_sensible() {
        let mb = |s: ExportSettings| s.estimate((1920, 1080), 30.0, 60_000).unwrap() as f64 / 1e6;
        let h264 = mb(ExportSettings::default());
        assert!((10.0..200.0).contains(&h264), "{h264}");
        let prores = mb(PRESETS[4].2);
        assert!(prores > h264 * 5.0, "{prores} vs {h264}");
        let fixed = mb(ExportSettings { rate: Rate::Bitrate(8_000), audio: None, ..ExportSettings::default() });
        assert!((fixed - 60.0).abs() < 1.0, "{fixed}");
    }
}
