//! A sequence's sound (docs/plans/video-editor.md, phase 4): every clip's sound cut, sped,
//! faded, shaped by its volume keys and mixed (as an FFmpeg filter, for the export and the
//! player); voices cleaned up offline by the audio engine's Voice Clarity chain; waveforms.

use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;

use crate::media::{self, MediaError};
use crate::sequence::{Clip, MediaKind, Sequence, TrackKind, VoiceClean};

/// The rate sound is decoded, cleaned and played at.
pub const RATE: u32 = 48_000;
/// Waveform columns per second.
pub const PEAKS_PER_SECOND: u32 = 100;

static CACHE: OnceLock<PathBuf> = OnceLock::new();

/// Where cleaned voices are kept (the app sets its data folder's cache; a temporary folder
/// otherwise). Set once, before any sound is made.
pub fn set_cache(dir: PathBuf) {
    let _ = CACHE.set(dir);
}

/// The cache folder (cleaned voices, proxies).
pub fn cache() -> PathBuf {
    CACHE.get().cloned().unwrap_or_else(|| std::env::temp_dir().join("lightair-sound"))
}

/// A file's sound, decoded: interleaved f32 with `channels` at `rate`.
pub fn decode(path: &Path, rate: u32, channels: u32) -> Result<Vec<f32>, MediaError> {
    decode_track(path, None, rate, channels)
}

/// One of a file's audio tracks (`None`: its first), decoded.
pub fn decode_track(path: &Path, track: Option<u32>, rate: u32, channels: u32) -> Result<Vec<f32>, MediaError> {
    let map = format!("0:a:{}", track.unwrap_or(0));
    let mut child = media::command(media::ffmpeg()?)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", &map, "-f", "f32le", "-ac", &channels.to_string(), "-ar", &rate.to_string(), "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut bytes = Vec::new();
    child.stdout.take().ok_or_else(|| MediaError::Failed("no output".into()))?.read_to_end(&mut bytes)?;
    if !child.wait()?.success() && bytes.is_empty() {
        return Err(MediaError::Unreadable(path.to_path_buf()));
    }
    Ok(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// A file's waveform: the loudest sample in each 1/[`PEAKS_PER_SECOND`] of a second (0..1).
pub fn peaks(path: &Path) -> Result<Vec<f32>, MediaError> {
    peaks_of(path, None)
}

/// One of a file's audio tracks' waveform.
pub fn peaks_of(path: &Path, track: Option<u32>) -> Result<Vec<f32>, MediaError> {
    const R: u32 = 8_000;
    let samples = decode_track(path, track, R, 1)?;
    Ok(samples.chunks((R / PEAKS_PER_SECOND) as usize).map(|c| c.iter().fold(0f32, |m, s| m.max(s.abs())).min(1.0)).collect())
}

/// The waveform of sounds played together: theirs added up, as they're mixed.
pub fn combine(peaks: &[&[f32]]) -> Vec<f32> {
    let n = peaks.iter().map(|p| p.len()).max().unwrap_or(0);
    (0..n).map(|i| peaks.iter().map(|p| p.get(i).copied().unwrap_or(0.0)).sum::<f32>().min(1.0)).collect()
}

/// Writes interleaved stereo f32 as a WAV.
pub fn write_wav(out: &Path, stereo: &[f32], rate: u32) -> std::io::Result<()> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let data = (stereo.len() * 4) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(out)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    // IEEE float, 2 channels
    f.write_all(&3u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 8).to_le_bytes())?;
    f.write_all(&8u16.to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for s in stereo {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

/// Where `file` cleaned with `v` is kept (named by the file, its size and time, and `v`).
pub fn cleaned_path(file: &Path, v: VoiceClean) -> PathBuf {
    cleaned_path_of(file, None, v)
}

/// Where one of a file's audio tracks cleaned with `v` is kept.
pub fn cleaned_path_of(file: &Path, track: Option<u32>, v: VoiceClean) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    track.hash(&mut h);
    if let Ok(m) = std::fs::metadata(file) {
        m.len().hash(&mut h);
        m.modified().ok().hash(&mut h);
    }
    ((v.amount * 100.0).round() as i32).hash(&mut h);
    v.podcast.hash(&mut h);
    cache().join(format!("voice-{:016x}.wav", h.finish()))
}

/// Cleans up the voice in `file` into `out`: noise, a gate, de-essing, EQ, compression and a
/// limiter (the audio engine's Voice Clarity), in time with the original.
pub fn clean(file: &Path, v: VoiceClean, out: &Path) -> Result<(), MediaError> {
    clean_track(file, None, v, out)
}

/// Cleans one of a file's audio tracks.
pub fn clean_track(file: &Path, track: Option<u32>, v: VoiceClean, out: &Path) -> Result<(), MediaError> {
    use lightair_audio::dsp::{Ctx, Params, VoiceParams};
    let mut audio = decode_track(file, track, RATE, 2)?;
    let params = Params::Voice(VoiceParams { amount: v.amount.clamp(0.0, 1.0), podcast: v.podcast, ..VoiceParams::default() });
    let mut p = params.make(RATE as f32);
    let ctx = Ctx { sample_rate: RATE as f32, key: None };
    let lat = p.latency();
    // its delay made up: silence after, the same length cut from the front
    audio.extend(std::iter::repeat_n(0.0, lat * 2));
    let frames = audio.len() / 2;
    let mut l = vec![0f32; lightair_audio::MAX_BLOCK];
    let mut r = vec![0f32; lightair_audio::MAX_BLOCK];
    let mut done = Vec::with_capacity(audio.len());
    let mut at = 0;
    while at < frames {
        let n = (frames - at).min(lightair_audio::MAX_BLOCK);
        for i in 0..n {
            l[i] = audio[(at + i) * 2];
            r[i] = audio[(at + i) * 2 + 1];
        }
        p.process(&mut l[..n], &mut r[..n], &ctx);
        for i in 0..n {
            done.push(l[i]);
            done.push(r[i]);
        }
        at += n;
    }
    let part = out.with_extension("part");
    write_wav(&part, &done[(lat * 2).min(done.len())..], RATE)?;
    std::fs::rename(&part, out)?;
    Ok(())
}

/// Where a sound comes from: a file, and which of its audio tracks (`None`: its first).
pub type Source = (PathBuf, Option<u32>);

/// What a clip's sound comes from, before any clean-up: a take's stems, or the file's audio
/// track the clip plays (every track, for a video clip whose sound isn't apart).
pub fn sources(seq: &Sequence, c: &Clip, video_track: bool) -> Vec<Source> {
    let Some(m) = seq.media(c.media) else { return Vec::new() };
    // a picture track's clips play their own sound; stills have none
    if m.kind == MediaKind::Image || (video_track && m.audio.is_empty()) {
        return Vec::new();
    }
    // a take's stems: files of their own
    if !m.audio.is_empty() && m.audio.iter().any(|f| *f != m.path) {
        return m.audio.iter().map(|f| (f.clone(), None)).collect();
    }
    match c.stream {
        Some(n) => vec![(m.path.clone(), Some(n))],
        None if m.tracks.len() > 1 => (0..m.tracks.len() as u32).map(|n| (m.path.clone(), Some(n))).collect(),
        None => vec![(m.path.clone(), None)],
    }
}

/// What a clip's sound is played from: its sources, a cleaned voice in place of each when
/// it's been made.
fn sound_files(seq: &Sequence, c: &Clip, video_track: bool) -> Vec<Source> {
    let files = sources(seq, c, video_track);
    match c.voice {
        Some(v) => files
            .into_iter()
            .map(|(f, s)| {
                let cleaned = cleaned_path_of(&f, s, v);
                if cleaned.is_file() { (cleaned, None) } else { (f, s) }
            })
            .collect(),
        None => files,
    }
}

fn sounding(seq: &Sequence) -> impl Iterator<Item = (&Clip, bool)> {
    // (a video track isn't muted: its sound is on the audio tracks)
    seq.tracks.iter().filter(|t| !t.muted || t.kind == TrackKind::Video).flat_map(|t| {
        let video = t.kind == TrackKind::Video;
        t.clips.iter().filter(|c| !c.muted && c.content.is_none() && (c.volume > 0.0 || !c.volume_keys.is_empty())).map(move |c| (c, video))
    })
}

/// The voices the sequence asks to be cleaned that haven't been yet (file, its track, setting).
pub fn voices_to_clean(seq: &Sequence) -> Vec<(PathBuf, Option<u32>, VoiceClean)> {
    let mut out: Vec<(PathBuf, Option<u32>, VoiceClean)> = Vec::new();
    for (c, video) in sounding(seq) {
        let Some(v) = c.voice else { continue };
        for (f, s) in sources(seq, c, video) {
            if !cleaned_path_of(&f, s, v).is_file() && !out.iter().any(|(g, t, w)| *g == f && *t == s && *w == v) {
                out.push((f, s, v));
            }
        }
    }
    out
}

/// Cleans every voice the sequence asks for that isn't yet.
pub fn prepare(seq: &Sequence) -> Result<(), MediaError> {
    for (f, s, v) in voices_to_clean(seq) {
        clean_track(&f, s, v, &cleaned_path_of(&f, s, v))?;
    }
    Ok(())
}

/// A clip's volume over its own time `t` (seconds from its start) as an FFmpeg expression.
fn volume_expr(c: &Clip) -> String {
    let k = &c.volume_keys;
    if k.is_empty() {
        return format!("{:.4}", c.volume);
    }
    let mut e = format!("{:.4}", k[k.len() - 1].volume);
    for i in (0..k.len() - 1).rev() {
        let (a, b) = (&k[i], &k[i + 1]);
        let (ta, tb) = (a.at_ms as f64 / 1000.0, b.at_ms as f64 / 1000.0);
        e = format!(
            "if(lt(t,{tb:.3}),{:.4}+({:.4})*(max(t,{ta:.3})-{ta:.3})/{:.3},{e})",
            a.volume,
            b.volume - a.volume,
            (tb - ta).max(0.001)
        );
    }
    e
}

/// The sound of the whole sequence as an FFmpeg filter over `inputs` (in order, from input
/// 1), or `None` when nothing makes a sound.
pub fn mix_of(seq: &Sequence) -> (Vec<PathBuf>, Option<String>) {
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut parts = Vec::new();
    // which parts are turned down under the others
    let mut ducked: Vec<bool> = Vec::new();
    for (c, video) in sounding(seq) {
        for (f, track) in sound_files(seq, c, video) {
            inputs.push(f);
            let i = inputs.len();
            let pick = track.map_or(String::new(), |n| format!(":{n}"));
            let s = c.speed.clamp(0.25, 16.0);
            let tempo = if s < 0.5 { format!("atempo={},atempo=0.5", s / 0.5) } else { format!("atempo={s}") };
            let len = c.len_ms() as f64 / 1000.0;
            let mut shape = format!("volume='{}':eval=frame", volume_expr(c));
            if c.fade_in_ms > 0 {
                shape.push_str(&format!(",afade=t=in:st=0:d={:.3}", c.fade_in_ms as f64 / 1000.0));
            }
            if c.fade_out_ms > 0 {
                let d = (c.fade_out_ms as f64 / 1000.0).min(len);
                shape.push_str(&format!(",afade=t=out:st={:.3}:d={d:.3}", len - d));
            }
            parts.push(format!(
                "[{i}:a{pick}]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS,{tempo},{shape},adelay={}|{}[s{}]",
                c.in_ms as f64 / 1000.0,
                c.out_ms as f64 / 1000.0,
                c.start_ms,
                c.start_ms,
                parts.len()
            ));
            ducked.push(c.duck);
        }
    }
    if parts.is_empty() {
        return (inputs, None);
    }
    let labels = |pick: &dyn Fn(bool) -> bool| -> (String, usize) {
        let l: Vec<String> = (0..parts.len()).filter(|k| pick(ducked[*k])).map(|k| format!("[s{k}]")).collect();
        (l.concat(), l.len())
    };
    let (voices, nv) = labels(&|d| !d);
    let (music, nm) = labels(&|d| d);
    let f = if nv == 0 || nm == 0 {
        let (all, n) = labels(&|_| true);
        format!("{};{all}amix=inputs={n}:normalize=0:dropout_transition=0[aout]", parts.join(";"))
    } else {
        // the music pressed down by the voices (their sound as the key, padded so the music
        // carries on after them)
        format!(
            "{};{voices}amix=inputs={nv}:normalize=0:dropout_transition=0,asplit=2[vmix][vkey];[vkey]apad[vk];{music}amix=inputs={nm}:normalize=0:dropout_transition=0[music];[music][vk]sidechaincompress=threshold=0.02:ratio=8:attack=20:release=400[ducked];[vmix][ducked]amix=inputs=2:normalize=0:dropout_transition=0[aout]",
            parts.join(";")
        )
    };
    (inputs, Some(f))
}

/// FFmpeg reading the sequence's sound, its mix at `[aout]`; `None` if it has none.
fn mixing(seq: &Sequence) -> Result<Option<Command>, MediaError> {
    let (inputs, mix) = mix_of(seq);
    let Some(mix) = mix else { return Ok(None) };
    let mut cmd = media::command(media::ffmpeg()?);
    cmd.args(["-v", "error", "-y"]);
    for i in &inputs {
        cmd.arg("-i").arg(i);
    }
    // the inputs are numbered from 1 in the mix (0 is the pictures, when exporting)
    let f = (1..=inputs.len()).fold(mix, |f, n| f.replace(&format!("[{n}:a"), &format!("[{}:a", n - 1)));
    cmd.args(["-filter_complex", &f, "-map", "[aout]"]);
    Ok(Some(cmd))
}

/// The sequence's sound alone, mixed into `out` (a WAV, 16 kHz mono, for speech). False if it
/// has none.
pub fn sound_to(seq: &Sequence, out: &Path) -> Result<bool, MediaError> {
    prepare(seq)?;
    let Some(mut cmd) = mixing(seq)? else { return Ok(false) };
    let ok = cmd.args(["-ac", "1", "-ar", "16000"]).arg(out).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if ok { Ok(true) } else { Err(MediaError::Failed("couldn't mix the sound".into())) }
}

/// The sequence's sound from `from_ms`, as it's mixed: FFmpeg writing interleaved stereo f32 at
/// `rate` to its stdout (for the player). Voices not yet cleaned play as they are.
pub fn stream(seq: &Sequence, from_ms: u64, rate: u32) -> Result<Option<Child>, MediaError> {
    let Some(mut cmd) = mixing(seq)? else { return Ok(None) };
    cmd.args(["-ss", &format!("{:.3}", from_ms as f64 / 1000.0), "-f", "f32le", "-ac", "2", "-ar", &rate.to_string(), "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    Ok(Some(cmd.spawn()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::{Clip, VolumeKey};
    use lightair_core::Id;

    #[test]
    fn volume_keys_make_a_curve() {
        let mut c = Clip::new(Id::nil(), 1_000, 4_000);
        assert_eq!(volume_expr(&c), "1.0000");
        c.volume_keys = vec![VolumeKey { at_ms: 1_000, volume: 1.0 }, VolumeKey { at_ms: 3_000, volume: 0.2 }];
        assert_eq!(c.volume_at(1_500), 1.0, "before the first key");
        assert!((c.volume_at(3_000) - 0.6).abs() < 1e-4, "halfway, in a straight line");
        assert_eq!(c.volume_at(4_500), 0.2);
        let e = volume_expr(&c);
        assert!(e.starts_with("if(lt(t,3.000)") && e.ends_with(",0.2000)"), "{e}");
        c.fade_in_ms = 1_000;
        assert!((c.gain_at(1_500) - 0.5).abs() < 1e-4, "half faded in");
        c.muted = true;
        assert_eq!(c.gain_at(2_500), 0.0);
    }
}
