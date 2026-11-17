//! Speech, on the computer (docs/plans/screen-studio.md, 4.4): a transcript with each word's
//! time, from whisper.cpp run as a helper program (`whisper-cli`), the same on macOS and Windows.
//! From the transcript: captions as subtitles (`.srt`, and a subtitle track in exports), and cuts
//! that take out filler words and long pauses.
//!
//! The model is a file the user downloads once, in Settings (Lightair never sends audio
//! anywhere). It lives in Lightair's data folder, `models/`.

use std::path::{Path, PathBuf};
use crate::media;

use serde::{Deserialize, Serialize};

use crate::edit::{Clip, Edit};
use crate::timeline;

/// The model Lightair uses, and where it's downloaded from.
pub const MODEL: &str = "ggml-base.bin";
pub const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub language: String,
    pub words: Vec<Word>,
}

#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    #[error("whisper.cpp isn't installed (whisper-cli, next to Lightair or on the PATH)")]
    NoWhisper,
    #[error("the speech model isn't downloaded yet (Settings › Speech)")]
    NoModel,
    #[error("{0}")]
    Failed(String),
}

fn find(tool: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{tool}.exe") } else { tool.to_string() };
    let mut dirs: Vec<PathBuf> = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)).into_iter().collect();
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    if cfg!(target_os = "macos") {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// Where the model is (or goes), in `data_dir`.
pub fn model_path(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join(MODEL)
}

/// Transcribes `audio` (any number of files, mixed) with the model at `model`.
pub fn transcribe(audio: &[PathBuf], model: &Path) -> Result<Transcript, SpeechError> {
    let whisper = find("whisper-cli").ok_or(SpeechError::NoWhisper)?;
    if !model.is_file() {
        return Err(SpeechError::NoModel);
    }
    let ffmpeg = crate::media::ffmpeg().map_err(|e| SpeechError::Failed(e.to_string()))?;
    let dir = std::env::temp_dir().join(format!("lightair-speech-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| SpeechError::Failed(e.to_string()))?;
    // whisper takes 16 kHz mono
    let wav = dir.join("speech.wav");
    let mut mix = media::command(ffmpeg);
    mix.args(["-v", "error", "-y"]);
    for a in audio {
        mix.arg("-i").arg(a);
    }
    if audio.len() > 1 {
        mix.args(["-filter_complex", &format!("amix=inputs={}:normalize=0", audio.len())]);
    }
    let ok = mix.args(["-ac", "1", "-ar", "16000"]).arg(&wav).status().is_ok_and(|s| s.success());
    if !ok {
        return Err(SpeechError::Failed("couldn't read the audio".into()));
    }
    let base = dir.join("speech");
    let out = media::command(whisper)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&wav)
        // a word per segment, with its time
        .args(["-ml", "1", "-sow", "-oj", "-l", "auto", "-np", "-of"])
        .arg(&base)
        .output()
        .map_err(|e| SpeechError::Failed(e.to_string()))?;
    if !out.status.success() {
        return Err(SpeechError::Failed(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("").into()));
    }
    let json = std::fs::read(base.with_extension("json")).map_err(|e| SpeechError::Failed(e.to_string()))?;
    let _ = std::fs::remove_dir_all(&dir);
    parse(&json).ok_or_else(|| SpeechError::Failed("whisper's answer couldn't be read".into()))
}

/// whisper.cpp's JSON (`-oj` with a word per segment) as a transcript.
pub fn parse(json: &[u8]) -> Option<Transcript> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let language = v["result"]["language"].as_str().unwrap_or("").to_string();
    let words = v["transcription"]
        .as_array()?
        .iter()
        .filter_map(|s| {
            let text = s["text"].as_str()?.trim().to_string();
            (!text.is_empty()).then(|| Word {
                start_ms: s["offsets"]["from"].as_u64().unwrap_or(0),
                end_ms: s["offsets"]["to"].as_u64().unwrap_or(0),
                text,
            })
        })
        .collect();
    Some(Transcript { language, words })
}

/// Filler sounds that are words in no language Lightair's users speak.
const FILLERS_ANY: &[&str] = &["um", "umm", "uh", "uhh", "erm", "hmm", "mm", "ehm", "äh", "ähm", "euh"];

/// Filler words of a language (whisper's code: "en", "de"…), beyond [`FILLERS_ANY`]: short
/// words that are fillers there but real words elsewhere (German "er" is "he").
fn fillers_of(language: &str) -> &'static [&'static str] {
    match language {
        "en" => &["er", "ah", "eh"],
        "de" => &["öh", "hm"],
        "fr" => &["ben", "bah"],
        "ja" => &["eto", "ano", "anō", "ēto"],
        "id" | "ms" => &["eh", "em", "anu"],
        _ => &[],
    }
}

fn is_filler(w: &str, language: &str) -> bool {
    let w: String = w.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
    FILLERS_ANY.contains(&w.as_str()) || fillers_of(language).contains(&w.as_str())
}

/// The edit without the filler words and the pauses longer than `pause_ms` (a pause keeps
/// `keep_ms` of its silence, so speech doesn't run together). Source ranges already cut stay cut.
pub fn tighten(edit: &Edit, t: &Transcript, pause_ms: u64, keep_ms: u64) -> Edit {
    // the ranges to take out, in source time
    let mut out: Vec<(u64, u64)> = Vec::new();
    for w in t.words.iter().filter(|w| is_filler(&w.text, &t.language)) {
        out.push((w.start_ms, w.end_ms));
    }
    for pair in t.words.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if b.start_ms > a.end_ms + pause_ms {
            out.push((a.end_ms + keep_ms / 2, b.start_ms.saturating_sub(keep_ms / 2)));
        }
    }
    out.sort();
    let mut clips: Vec<Clip> = Vec::new();
    for c in &edit.clips {
        let mut start = c.start_ms;
        for &(a, b) in out.iter().filter(|(a, b)| *b > c.start_ms && *a < c.end_ms) {
            if a > start + 40 {
                clips.push(Clip { start_ms: start, end_ms: a.min(c.end_ms), speed: c.speed });
            }
            start = start.max(b);
        }
        if c.end_ms > start + 40 {
            clips.push(Clip { start_ms: start, end_ms: c.end_ms, speed: c.speed });
        }
    }
    Edit { clips, ..edit.clone() }
}

/// What to take out of `from..to` of a recording (source time) to tighten its speech: the
/// filler words and the pauses longer than `pause_ms` (a pause keeps `keep_ms`), in order.
pub fn tightening(t: &Transcript, from: u64, to: u64, pause_ms: u64, keep_ms: u64) -> Vec<(u64, u64)> {
    let words: Vec<&Word> = t.words.iter().filter(|w| w.end_ms > from && w.start_ms < to).collect();
    let mut out: Vec<(u64, u64)> = words.iter().filter(|w| is_filler(&w.text, &t.language)).map(|w| (w.start_ms.max(from), w.end_ms.min(to))).collect();
    for pair in words.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b.start_ms > a.end_ms + pause_ms {
            out.push((a.end_ms + keep_ms / 2, b.start_ms.saturating_sub(keep_ms / 2)));
        }
    }
    merge(out)
}

/// Ranges sorted, the overlapping ones joined.
pub fn merge(mut r: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    r.retain(|(a, b)| b > a);
    r.sort();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (a, b) in r {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Whether a word is a filler ("um", "uh", …) in the transcript's language.
pub fn filler(t: &Transcript, w: &str) -> bool {
    is_filler(w, &t.language)
}

/// Words (start, end, text) as lines of up to `max_chars`, a line ending at a pause or after
/// 3 seconds.
pub fn lines<'a>(words: impl IntoIterator<Item = (u64, u64, &'a str)>, max_chars: usize) -> Vec<(u64, u64, String)> {
    let mut lines: Vec<(u64, u64, String)> = Vec::new();
    for (a, b, text) in words {
        match lines.last_mut() {
            Some(l) if l.2.len() + 1 + text.len() <= max_chars && a < l.1 + 600 && b < l.0 + 3_000 => {
                l.1 = b;
                l.2.push(' ');
                l.2.push_str(text);
            }
            _ => lines.push((a, b, text.to_string())),
        }
    }
    lines
}

/// Captions for a sequence: its transcript's words (fillers left out) as lines.
pub fn caption_lines(t: &Transcript, max_chars: usize) -> Vec<(u64, u64, String)> {
    lines(t.words.iter().filter(|w| !is_filler(&w.text, &t.language)).map(|w| (w.start_ms, w.end_ms, w.text.as_str())), max_chars)
}

/// Subtitles (SubRip) for the result: the transcript's words through the edit's cuts and speed,
/// in lines of up to `max_chars`, a line ending at a pause or after 3 seconds.
pub fn srt(edit: &Edit, t: &Transcript, max_chars: usize) -> String {
    // words in result time (those cut, dropped)
    let words: Vec<(u64, u64, &str)> = t
        .words
        .iter()
        .filter(|w| !is_filler(&w.text, &t.language))
        .filter_map(|w| Some((timeline::result_at(edit, w.start_ms)?, timeline::result_at(edit, w.end_ms.saturating_sub(1))?, w.text.as_str())))
        .collect();
    let lines = lines(words, max_chars);
    let stamp = |ms: u64| format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000);
    lines
        .iter()
        .enumerate()
        .map(|(n, (a, b, text))| format!("{}\n{} --> {}\n{}\n", n + 1, stamp(*a), stamp((*b).max(a + 400)), text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(a: u64, b: u64, t: &str) -> Word {
        Word { start_ms: a, end_ms: b, text: t.into() }
    }

    #[test]
    fn whispers_json_reads_as_words() {
        let json = br#"{"result":{"language":"en"},"transcription":[
            {"offsets":{"from":0,"to":320},"text":" Hello"},
            {"offsets":{"from":320,"to":700},"text":" there"},
            {"offsets":{"from":700,"to":700},"text":""}]}"#;
        let t = parse(json).unwrap();
        assert_eq!(t.language, "en");
        assert_eq!(t.words, vec![w(0, 320, "Hello"), w(320, 700, "there")]);
    }

    #[test]
    fn fillers_and_long_pauses_come_out() {
        let t = Transcript {
            language: "en".into(),
            words: vec![w(0, 400, "So"), w(400, 700, "um,"), w(700, 1_000, "this"), w(4_000, 4_400, "works")],
        };
        let e = tighten(&Edit::new(5_000), &t, 1_000, 300);
        // "um" (400–700) out; the 3 s pause (1000–4000) kept to 300 ms
        assert_eq!(
            e.clips.iter().map(|c| (c.start_ms, c.end_ms)).collect::<Vec<_>>(),
            vec![(0, 400), (700, 1_150), (3_850, 5_000)]
        );
        // subtitles follow the cuts, without the filler
        let s = srt(&e, &t, 42);
        assert!(s.starts_with("1\n00:00:00,000 --> "), "{s}");
        assert!(s.contains("So this") && !s.contains("um"), "{s}");
        assert!(s.contains("works"), "{s}");
    }
}
