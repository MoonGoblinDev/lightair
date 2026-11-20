//! Decoding and encoding through FFmpeg, run as a helper program (`ffmpeg`, `ffprobe`), frames
//! over pipes as raw RGBA. The same on macOS and Windows, with each platform's hardware codecs
//! (VideoToolbox; NVENC, AMF, Quick Sync) when FFmpeg has them.
//!
//! The helpers are found in `LIGHTAIR_FFMPEG_DIR`, next to Lightair, or on the PATH.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::edit::Clip;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("FFmpeg isn't installed (Lightair looks next to itself, in LIGHTAIR_FFMPEG_DIR and on the PATH)")]
    NoFfmpeg,
    #[error("couldn't read {0}")]
    Unreadable(PathBuf),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("FFmpeg stopped: {0}")]
    Failed(String),
}

fn find(tool: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{tool}.exe") } else { tool.to_string() };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("LIGHTAIR_FFMPEG_DIR") {
        dirs.push(d.into());
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        dirs.push(d);
    }
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    // where Homebrew puts it, for apps that don't inherit a shell's PATH
    if cfg!(target_os = "macos") {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

pub fn ffmpeg() -> Result<PathBuf, MediaError> {
    find("ffmpeg").ok_or(MediaError::NoFfmpeg)
}

/// A video's size, frame rate and length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: u64,
}

pub fn probe(path: &Path) -> Result<Probe, MediaError> {
    let ffprobe = find("ffprobe").ok_or(MediaError::NoFfmpeg)?;
    let out = command(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,avg_frame_rate:format=duration"])
        .args(["-of", "json"])
        .arg(path)
        .output()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| MediaError::Unreadable(path.into()))?;
    let s = &v["streams"][0];
    let fps = s["avg_frame_rate"].as_str().and_then(|r| {
        let (n, d) = r.split_once('/')?;
        let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
        (d > 0.0).then_some(n / d)
    });
    let duration = v["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok());
    match (s["width"].as_u64(), s["height"].as_u64(), duration) {
        (Some(w), Some(h), Some(d)) => Ok(Probe { width: w as u32, height: h as u32, fps: fps.unwrap_or(30.0), duration_ms: (d * 1000.0) as u64 }),
        _ => Err(MediaError::Unreadable(path.into())),
    }
}

/// A video read in time order, at one size: the frame for a time, reading ahead from where it
/// is and starting again only for a jump (back, or far ahead).
pub struct Stream {
    path: PathBuf,
    size: (u32, u32),
    fps: f64,
    dec: Option<Decoder>,
    /// The frames read lately, in time order (the last is the current one): scrubbing back
    /// finds them here.
    recent: std::collections::VecDeque<(u64, Vec<u8>)>,
    keep: usize,
    /// The time of the current frame.
    at: Option<u64>,
}

/// What a stream keeps of the frames it read lately.
const RECENT_BYTES: usize = 160 << 20;

impl Stream {
    pub fn new(path: &Path, size: (u32, u32), fps: f64) -> Stream {
        let keep = (RECENT_BYTES / (size.0 as usize * size.1 as usize * 4).max(1)).clamp(4, 150);
        Stream { path: path.to_path_buf(), size, fps, dec: None, recent: Default::default(), keep, at: None }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The frame at `ms` of the video.
    pub fn frame(&mut self, ms: u64) -> Option<&[u8]> {
        let frame = (1000.0 / self.fps) as u64;
        // read lately (scrubbing back): no decoding
        let behind = self.at.is_some_and(|a| ms + frame / 2 < a);
        if behind && let Some(i) = self.recent.iter().position(|(t, _)| *t <= ms && ms < t + frame.max(1)) {
            return Some(self.recent[i].1.as_slice());
        }
        let far = self.dec.as_ref().is_some_and(|d| ms > d.next_ms() + 2_000);
        if self.dec.is_none() || behind || far {
            // going back: from a second before, so the frames around land in `recent`
            let from = if behind { ms.saturating_sub(1_000) } else { ms };
            self.dec = Decoder::open(&self.path, from, self.fps, self.size).ok();
            self.at = None;
            self.recent.clear();
        }
        let dec = self.dec.as_mut()?;
        while self.at.is_none() || dec.next_ms() <= ms {
            let t = dec.next_ms();
            let mut f = Vec::new();
            if !dec.next(&mut f) {
                break;
            }
            self.recent.push_back((t, f));
            if self.recent.len() > self.keep {
                self.recent.pop_front();
            }
            self.at = Some(t);
        }
        self.recent.back().map(|(_, f)| f.as_slice())
    }
}

/// What a file holds: video (with or without sound), sound only, or a still; its size and length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contents {
    pub video: bool,
    pub audio: bool,
    pub still: bool,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
}

/// A file's audio tracks, by name (their title, else "Track 1", "Track 2"…).
pub fn audio_tracks(path: &Path) -> Vec<String> {
    let Some(ffprobe) = find("ffprobe") else { return Vec::new() };
    let Ok(out) = command(ffprobe).args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index:stream_tags=title,handler_name", "-of", "json"]).arg(path).output() else {
        return Vec::new();
    };
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    v["streams"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(n, st)| {
                    let tags = &st["tags"];
                    // (the names muxers give every track aren't names: "SoundHandler", "Core Media Audio"…)
                    let named = |k: &str| tags[k].as_str().map(str::trim).filter(|t| !t.is_empty() && !DEFAULT_TRACK_NAMES.contains(t)).map(String::from);
                    named("title").or_else(|| named("handler_name")).unwrap_or_else(|| format!("Track {}", n + 1))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The track names muxers write when the track has none of its own.
const DEFAULT_TRACK_NAMES: [&str; 8] = ["SoundHandler", "VideoHandler", "Core Media Audio", "Core Media Video", "Apple Sound Media Handler", "Apple Video Media Handler", "ISO Media file produced by Google Inc.", "Mainconcept MP4 Sound Media Handler"];

pub fn contents(path: &Path) -> Result<Contents, MediaError> {
    let ffprobe = find("ffprobe").ok_or(MediaError::NoFfmpeg)?;
    let out = command(ffprobe)
        .args(["-v", "error", "-show_entries", "stream=codec_type,width,height,nb_frames:format=duration,format_name", "-of", "json"])
        .arg(path)
        .output()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| MediaError::Unreadable(path.into()))?;
    let streams = v["streams"].as_array().ok_or_else(|| MediaError::Unreadable(path.into()))?;
    let video = streams.iter().find(|s| s["codec_type"] == "video");
    let audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let format = v["format"]["format_name"].as_str().unwrap_or("");
    // a picture: an image format, or one frame and no length
    let still = video.is_some()
        && (format.contains("image2") || format.contains("_pipe") || ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "heic"].iter().any(|e| path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e))));
    let duration_ms = v["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).map_or(0, |d| (d * 1000.0) as u64);
    if video.is_none() && !audio {
        return Err(MediaError::Unreadable(path.into()));
    }
    Ok(Contents {
        video: video.is_some(),
        audio,
        still,
        width: video.and_then(|s| s["width"].as_u64()).unwrap_or(0) as u32,
        height: video.and_then(|s| s["height"].as_u64()).unwrap_or(0) as u32,
        duration_ms,
    })
}

fn scale_filter(fps: f64, size: (u32, u32)) -> String {
    format!("fps={fps},scale={}:{}:flags=bicubic", size.0, size.1)
}

/// One frame at `t_ms`, as RGBA at `size` (scrubbing).
pub fn frame_at(path: &Path, t_ms: u64, size: (u32, u32)) -> Result<Vec<u8>, MediaError> {
    let out = command(ffmpeg()?)
        .args(["-v", "error", "-ss", &format!("{:.3}", t_ms as f64 / 1000.0), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", &format!("scale={}:{}", size.0, size.1), "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .output()?;
    let want = (size.0 * size.1 * 4) as usize;
    if out.stdout.len() < want {
        return Err(MediaError::Failed(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Ok(out.stdout[..want].to_vec())
}

/// A video read frame by frame from `start_ms`, at `fps`, scaled to `size`.
pub struct Decoder {
    child: Child,
    /// Frames read ahead on a thread of their own (a few, so decoding runs while the last one
    /// is drawn).
    frames: std::sync::mpsc::Receiver<Vec<u8>>,
    size: (u32, u32),
    fps: f64,
    start_ms: u64,
    /// Frames read so far.
    pub read: u64,
}

impl Decoder {
    pub fn open(path: &Path, start_ms: u64, fps: f64, size: (u32, u32)) -> Result<Decoder, MediaError> {
        let mut child = command(ffmpeg()?)
            .args(["-v", "error", "-hwaccel", "auto", "-ss", &format!("{:.3}", start_ms as f64 / 1000.0), "-i"])
            .arg(path)
            .args(["-an", "-vf", &scale_filter(fps, size), "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let out = child.stdout.take().ok_or(MediaError::NoFfmpeg)?;
        let (tx, frames) = std::sync::mpsc::sync_channel(AHEAD);
        let bytes = (size.0 * size.1 * 4) as usize;
        std::thread::spawn(move || read_ahead(out, bytes, tx));
        Ok(Decoder { child, frames, size, fps, start_ms, read: 0 })
    }

    /// The source time of the next frame.
    pub fn next_ms(&self) -> u64 {
        self.start_ms + (self.read as f64 * 1000.0 / self.fps) as u64
    }

    /// The next frame into `buf` (RGBA, `size`); false at the end.
    pub fn next(&mut self, buf: &mut Vec<u8>) -> bool {
        match self.frames.recv() {
            Ok(f) => {
                debug_assert_eq!(f.len(), (self.size.0 * self.size.1 * 4) as usize);
                *buf = f;
                self.read += 1;
                true
            }
            Err(_) => false,
        }
    }
}

/// Frames a decoder reads ahead.
const AHEAD: usize = 3;

/// Reads whole frames from FFmpeg until it ends or the decoder is gone.
fn read_ahead(mut out: ChildStdout, bytes: usize, tx: std::sync::mpsc::SyncSender<Vec<u8>>) {
    loop {
        let mut f = vec![0u8; bytes];
        if out.read_exact(&mut f).is_err() || tx.send(f).is_err() {
            break;
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the result is encoded as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// H.264 in MP4: hardware when there is one.
    H264,
    /// ProRes 422 in MOV (VideoToolbox's on a Mac, FFmpeg's own elsewhere).
    ProRes,
    /// An animated GIF: 15 frames a second, at most 960 wide, no sound.
    Gif,
}

/// FFmpeg's encoders, once.
fn encoders() -> &'static Vec<String> {
    static LIST: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        let Ok(ff) = ffmpeg() else { return Vec::new() };
        let Ok(out) = command(ff).args(["-hide_banner", "-encoders"]).output() else { return Vec::new() };
        String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.split_whitespace().nth(1).map(String::from)).collect()
    })
}

/// Whether a hardware encoder FFmpeg lists really encodes here, tried once with a tenth of a
/// second of black: a listed encoder may still lack its driver (NVENC on an older NVIDIA
/// driver, AMF without an AMD GPU), and would fail the whole export.
fn encodes(encoder: &str) -> bool {
    static TRIED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = std::sync::OnceLock::new();
    let tried = TRIED.get_or_init(Default::default);
    if let Some(ok) = tried.lock().unwrap().get(encoder) {
        return *ok;
    }
    let ok = ffmpeg().ok().is_some_and(|ff| {
        command(ff)
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=black:s=256x256:r=30:d=0.1", "-c:v", encoder, "-f", "null", "-"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    });
    if !ok {
        tracing::info!(encoder, "FFmpeg lists this encoder but it doesn't encode here");
    }
    tried.lock().unwrap().insert(encoder.to_string(), ok);
    ok
}

/// A command for FFmpeg and its kin. Windows: without a console window of its own (from a
/// window app, each run would flash one).
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The FFmpeg encoder and its options for a codec, best first.
fn video_args(codec: Codec, size: (u32, u32), fps: f64) -> Vec<String> {
    let has = |e: &str| encoders().iter().any(|x| x == e);
    let hardware = |e: &str| has(e) && (e == "libx264" || encodes(e));
    let kbps = ((size.0 as f64 * size.1 as f64 * fps) / 30_000.0).clamp(4_000.0, 60_000.0) as u32;
    let v = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match codec {
        Codec::ProRes if has("prores_videotoolbox") => v(&["-c:v", "prores_videotoolbox", "-profile:v", "2", "-pix_fmt", "p210le"]),
        Codec::ProRes => v(&["-c:v", "prores_ks", "-profile:v", "2", "-pix_fmt", "yuv422p10le"]),
        Codec::Gif => v(&["-loop", "0"]),
        Codec::H264 => {
            let enc = ["h264_videotoolbox", "h264_nvenc", "h264_amf", "h264_qsv"].into_iter().find(|e| hardware(e)).unwrap_or("libx264");
            let mut a = v(&["-c:v", enc, "-b:v", &format!("{kbps}k"), "-pix_fmt", "yuv420p"]);
            if enc == "libx264" {
                a.extend(v(&["-preset", "medium"]));
            }
            a
        }
    }
}

/// The audio filter that plays `inputs` audio streams (inputs 1..) through the clips: each cut
/// and sped like the video, then mixed.
fn audio_filter(inputs: usize, clips: &[Clip]) -> Option<String> {
    if inputs == 0 || clips.is_empty() {
        return None;
    }
    let mut f = String::new();
    for i in 0..inputs {
        for (k, c) in clips.iter().enumerate() {
            // atempo takes 0.5..100; Lightair's speeds stay within 0.25..16 (two stages)
            let s = c.speed.clamp(0.25, 16.0);
            let tempo = if s < 0.5 { format!("atempo={},atempo=0.5", s / 0.5) } else { format!("atempo={s}") };
            f.push_str(&format!(
                "[{}:a]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS,{tempo}[a{i}c{k}];",
                i + 1,
                c.start_ms as f64 / 1000.0,
                c.end_ms as f64 / 1000.0
            ));
        }
        for k in 0..clips.len() {
            f.push_str(&format!("[a{i}c{k}]"));
        }
        f.push_str(&format!("concat=n={}:v=0:a=1[a{i}];", clips.len()));
    }
    for i in 0..inputs {
        f.push_str(&format!("[a{i}]"));
    }
    f.push_str(&format!("amix=inputs={inputs}:normalize=0:dropout_transition=0[aout]"));
    Some(f)
}

/// Writes frames (RGBA, `size`, at `fps`) and the audio of `audio` through `clips` into `out`.
pub struct Encoder {
    child: Child,
    input: Option<ChildStdin>,
}

impl Encoder {
    /// `subtitles`: a SubRip file, added as a subtitle track (MP4 and MOV).
    pub fn start(
        out: &Path,
        size: (u32, u32),
        fps: f64,
        codec: Codec,
        audio: &[PathBuf],
        clips: &[Clip],
        subtitles: Option<&Path>,
    ) -> Result<Encoder, MediaError> {
        Self::start_with(out, size, fps, codec, audio, clips, subtitles, None)
    }

    /// As `start`, with the sound made by `mix` (an FFmpeg filter over inputs 1.., ending in
    /// `[aout]`) instead of the edit's cuts.
    #[allow(clippy::too_many_arguments)]
    pub fn start_with(
        out: &Path,
        size: (u32, u32),
        fps: f64,
        codec: Codec,
        audio: &[PathBuf],
        clips: &[Clip],
        subtitles: Option<&Path>,
        mix: Option<&str>,
    ) -> Result<Encoder, MediaError> {
        let mut cmd = command(ffmpeg()?);
        cmd.args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{}x{}", size.0, size.1)])
            .args(["-r", &format!("{fps}"), "-i", "-"]);
        if codec == Codec::Gif {
            // one palette for the whole clip, made from its own colours
            cmd.args([
                "-filter_complex",
                "[0:v]fps=15,scale='min(960,iw)':-2:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4",
            ]);
        } else {
            for a in audio {
                cmd.arg("-i").arg(a);
            }
            if let Some(subs) = subtitles {
                cmd.arg("-i").arg(subs);
            }
            match mix.map(String::from).or_else(|| audio_filter(audio.len(), clips)) {
                Some(f) => cmd.args(["-filter_complex", &f, "-map", "0:v", "-map", "[aout]", "-c:a", "aac", "-b:a", "192k"]),
                None => cmd.args(["-map", "0:v"]),
            };
            if subtitles.is_some() {
                cmd.args(["-map", &format!("{}:s", audio.len() + 1), "-c:s", "mov_text"]);
            }
        }
        cmd.args(video_args(codec, size, fps));
        if codec == Codec::H264 {
            cmd.args(["-movflags", "+faststart"]);
        }
        let mut child = cmd.arg(out).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
        let input = child.stdin.take();
        Ok(Encoder { child, input })
    }

    pub fn write(&mut self, rgba: &[u8]) -> Result<(), MediaError> {
        let Some(i) = &mut self.input else { return Ok(()) };
        i.write_all(rgba).map_err(|e| MediaError::Failed(format!("the encoder stopped taking frames ({e})")))
    }

    /// Ends the file; FFmpeg's complaint if it failed.
    pub fn finish(mut self) -> Result<(), MediaError> {
        drop(self.input.take());
        let out = self.child.wait_with_output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(MediaError::Failed(String::from_utf8_lossy(&out.stderr).trim().lines().last().unwrap_or("").to_string()))
        }
    }
}

/// Whether this FFmpeg encodes with `encoder` here (a hardware one: tried once).
pub fn can_encode(encoder: &str) -> bool {
    let listed = encoders().iter().any(|x| x == encoder);
    let hardware = ["videotoolbox", "nvenc", "amf", "qsv"].iter().any(|h| encoder.contains(h));
    listed && (!hardware || encodes(encoder))
}

impl Encoder {
    /// Starts an export made with `settings` into `out` (PNG frames: a folder beside it): frames
    /// come `source` big at `fps` (none for a sound-only export); the sound is `mix` (an FFmpeg
    /// filter over inputs 1.., ending in `[aout]`) over `audio`.
    pub fn start_export(
        out: &Path,
        source: (u32, u32),
        fps: f64,
        settings: &crate::exporter::ExportSettings,
        audio: &[PathBuf],
        mix: Option<&str>,
    ) -> Result<Encoder, MediaError> {
        let target = settings.target(out);
        if let Some(d) = target.parent() {
            std::fs::create_dir_all(d)?;
        }
        let video = settings.container.has_video();
        let sound = settings.container.has_audio() && settings.audio.is_some() && mix.is_some();
        let mut cmd = command(ffmpeg()?);
        cmd.args(["-v", "error", "-y"]);
        if video {
            cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{}x{}", source.0, source.1)])
                .args(["-r", &format!("{fps}"), "-i", "-"]);
        }
        if sound {
            for a in audio {
                cmd.arg("-i").arg(a);
            }
        }
        let has = |e: &str| can_encode(e);
        let (vfilter, vargs) = if video { settings.video_args(source, fps, &has) } else { (None, Vec::new()) };
        // the inputs are numbered from 1 when frames come first, from 0 when they don't
        let mix = mix.filter(|_| sound).map(|m| if video { m.to_string() } else { shift_inputs(m) });
        let mut graph = Vec::new();
        if let Some(f) = &vfilter {
            graph.push(format!("[0:v]{f}[vout]"));
        }
        if let Some(m) = &mix {
            graph.push(m.clone());
        }
        if !graph.is_empty() {
            cmd.args(["-filter_complex", &graph.join(";")]);
        }
        if video {
            cmd.args(["-map", if vfilter.is_some() { "[vout]" } else { "0:v" }]);
        }
        if mix.is_some() {
            cmd.args(["-map", "[aout]"]);
        }
        cmd.args(&vargs);
        cmd.args(if sound { settings.audio_args(&has) } else { vec!["-an".to_string()] });
        if matches!(settings.container, crate::exporter::Container::Mp4 | crate::exporter::Container::Mov | crate::exporter::Container::M4a) {
            cmd.args(["-movflags", "+faststart"]);
        }
        let mut child = cmd
            .arg(&target)
            .stdin(if video { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take();
        Ok(Encoder { child, input })
    }
}

/// A filter over inputs 1.. made to read inputs 0.. (`[3:a]` → `[2:a]`).
fn shift_inputs(filter: &str) -> String {
    let mut out = String::with_capacity(filter.len());
    let mut rest = filter;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find(']') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let label = &after[..j];
        match label.split_once(':').and_then(|(n, t)| n.parse::<usize>().ok().map(|n| (n, t))) {
            Some((n, t)) if n > 0 => out.push_str(&format!("[{}:{t}]", n - 1)),
            _ => out.push_str(&format!("[{label}]")),
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

/// A smaller copy of a video for editing (no sound): fitted in 1920×1080, a keyframe every half
/// second (quick to jump around in), written to `out` once it's whole. `progress(fraction)`
/// returns false to stop.
pub fn make_proxy(file: &Path, out: &Path, duration_ms: u64, mut progress: impl FnMut(f32) -> bool) -> Result<(), MediaError> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let part = out.with_extension("part.mp4");
    let mut args = video_args(Codec::H264, (1920, 1080), 30.0);
    args.extend(["-g", "15"].map(String::from));
    let mut child = command(ffmpeg()?)
        .args(["-v", "error", "-y", "-hwaccel", "auto", "-i"])
        .arg(file)
        .args(["-an", "-vf", "scale=w=1920:h=1080:force_original_aspect_ratio=decrease:force_divisible_by=2"])
        .args(&args)
        .args(["-progress", "pipe:1", "-nostats"])
        .arg(&part)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let out_pipe = child.stdout.take().ok_or(MediaError::NoFfmpeg)?;
    let mut stopped = false;
    for line in std::io::BufRead::lines(std::io::BufReader::new(out_pipe)).map_while(Result::ok) {
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<u64>().ok())
            && !progress((us as f64 / 1000.0 / duration_ms.max(1) as f64).min(1.0) as f32)
        {
            stopped = true;
            let _ = child.kill();
            break;
        }
    }
    let ok = child.wait()?.success() && !stopped;
    if !ok {
        let _ = std::fs::remove_file(&part);
        return Err(MediaError::Failed(if stopped { "stopped".into() } else { "the proxy couldn't be made".into() }));
    }
    std::fs::rename(&part, out)?;
    Ok(())
}

/// Thumbnails (BGRA) and their size.
pub type Thumbnails = (Vec<Vec<u8>>, (u32, u32));

/// Thumbnails along a video of `duration_ms`, one every `every_ms`, `height` pixels tall and as
/// wide as the video's shape: BGRA, in order, and their size. Read from its keyframes (quick),
/// or every frame when it has too few keyframes for that.
pub fn filmstrip(path: &Path, duration_ms: u64, every_ms: u64, height: u32, aspect: f32) -> Result<Thumbnails, MediaError> {
    let size = ((((height as f32 * aspect).round() as u32).max(2)) & !1, height & !1);
    let wanted = (duration_ms / every_ms.max(1)) as usize;
    let quick = thumbnails(path, every_ms, size, true)?;
    if quick.len() * 2 >= wanted && !quick.is_empty() {
        return Ok((quick, size));
    }
    let all = thumbnails(path, every_ms, size, false)?;
    if all.is_empty() {
        return Err(MediaError::Unreadable(path.to_path_buf()));
    }
    Ok((all, size))
}

fn thumbnails(path: &Path, every_ms: u64, size: (u32, u32), keyframes: bool) -> Result<Vec<Vec<u8>>, MediaError> {
    let mut cmd = command(ffmpeg()?);
    cmd.args(["-v", "error"]);
    if keyframes {
        cmd.args(["-skip_frame", "nokey"]);
    } else {
        cmd.args(["-hwaccel", "auto"]);
    }
    let mut child = cmd
        .arg("-i")
        .arg(path)
        .args(["-an", "-vf", &format!("fps={}:eof_action=pass,scale={}:{}", 1000.0 / every_ms.max(1) as f64, size.0, size.1), "-f", "rawvideo", "-pix_fmt", "bgra", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut out = child.stdout.take().ok_or(MediaError::NoFfmpeg)?;
    let bytes = (size.0 * size.1 * 4) as usize;
    let mut frames = Vec::new();
    loop {
        let mut f = vec![0u8; bytes];
        if out.read_exact(&mut f).is_err() {
            break;
        }
        frames.push(f);
    }
    let _ = child.wait();
    Ok(frames)
}

#[cfg(test)]
mod tests {
    #[test]
    fn mixes_read_from_input_zero_without_frames() {
        assert_eq!(super::shift_inputs("[1:a]atrim=start=0[s0];[2:a:0]volume=1[s1];[s0][s1]amix[aout]"), "[0:a]atrim=start=0[s0];[1:a:0]volume=1[s1];[s0][s1]amix[aout]");
    }

    use super::*;

    #[test]
    fn the_audio_follows_the_cuts_and_speed() {
        let clips = [Clip { start_ms: 0, end_ms: 4_000, speed: 1.0 }, Clip { start_ms: 6_000, end_ms: 10_000, speed: 2.0 }];
        let f = audio_filter(2, &clips).unwrap();
        assert!(f.contains("[1:a]atrim=start=0.000:end=4.000"));
        assert!(f.contains("[2:a]atrim=start=6.000:end=10.000,asetpts=PTS-STARTPTS,atempo=2"));
        assert!(f.contains("concat=n=2:v=0:a=1[a1]"));
        assert!(f.ends_with("amix=inputs=2:normalize=0:dropout_transition=0[aout]"));
        assert!(audio_filter(0, &clips).is_none());
    }
}
