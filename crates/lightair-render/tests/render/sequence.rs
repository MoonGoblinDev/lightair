//! A sequence of several tracks rendered and exported. Needs FFmpeg (skipped without it) and a
//! GPU.

use std::path::Path;
use std::process::Command;

use lightair_render::media::{self, Codec};
use lightair_render::seqrender::SequenceRenderer;
use lightair_render::sequence::{Content, Filter, Keyframe, Media, Sequence, TextStyle, TrackKind, Transform, Transition, TransitionKind};

fn lavfi(dir: &Path, name: &str, input: &str) -> Option<std::path::PathBuf> {
    let ff = media::ffmpeg().ok()?;
    let out = dir.join(name);
    Command::new(ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", input]).args(["-pix_fmt", "yuv420p"]).arg(&out).status().ok()?.success().then_some(out)
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

#[test]
fn tracks_stack_clips_follow_each_other_and_the_export_sounds() {
    let dir = tempfile::tempdir().unwrap();
    let (Some(red), Some(blue)) = (
        lavfi(dir.path(), "red.mp4", "color=c=red:s=320x180:r=30:d=3"),
        lavfi(dir.path(), "blue.mp4", "color=c=blue:s=320x180:r=30:d=3"),
    ) else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let ff = media::ffmpeg().unwrap();
    let tone = dir.path().join("tone.wav");
    assert!(Command::new(ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=5"]).arg(&tone).status().unwrap().success());

    let mut seq = Sequence::new((640, 360), 30);
    let r = seq.add_media(Media::from_file(&red).unwrap());
    let b = seq.add_media(Media::from_file(&blue).unwrap());
    let t = seq.add_media(Media::from_file(&tone).unwrap());
    let first = seq.append(r).unwrap();
    let second = seq.append(b).unwrap();
    assert_eq!(seq.clip(second).unwrap().start_ms, seq.clip(first).unwrap().end_ms());
    // a small blue picture over the red, from 1 s
    let over = seq.place(b, 1_000, None).unwrap();
    seq.trim_end(over, 2_000);
    seq.clip_mut(over).unwrap().transform = Transform { scale: 0.3, ..Transform::default() };
    let tone_clip = seq.place(t, 0, None).unwrap();
    let (tt, _) = seq.find(tone_clip).unwrap();
    assert_eq!(seq.tracks[tt].kind, TrackKind::Audio);

    let mut r = match SequenceRenderer::new(&seq) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no renderer here ({e}); skipping");
            return;
        }
    };
    let at = |img: &[u8], x, y| px(img, 640, x, y);
    let red_frame = r.frame(500);
    assert!(at(&red_frame, 320, 180)[0] > 200 && at(&red_frame, 320, 180)[2] < 60, "red first");
    let both = r.frame(1_500);
    assert!(at(&both, 320, 180)[2] > 200, "the small blue clip in front, in the middle");
    assert!(at(&both, 20, 20)[0] > 200, "the red around it");
    let later = r.frame(3_500);
    assert!(at(&later, 20, 20)[2] > 200, "then the blue clip on the main track");

    let out = dir.path().join("Sequence.mp4");
    r.export(&out, Codec::H264, |_| true).unwrap();
    let p = media::probe(&out).unwrap();
    assert_eq!((p.width, p.height), (640, 360));
    assert!((p.duration_ms as i64 - 6_000).abs() < 200, "{} ms", p.duration_ms);
    let c = media::contents(&out).unwrap();
    assert!(c.audio, "the tone is in it");
}

#[test]
fn text_filters_keyframes_and_a_crossfade() {
    let dir = tempfile::tempdir().unwrap();
    let (Some(red), Some(blue)) = (
        lavfi(dir.path(), "red.mp4", "color=c=red:s=320x180:r=30:d=2"),
        lavfi(dir.path(), "blue.mp4", "color=c=blue:s=320x180:r=30:d=2"),
    ) else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let mut seq = Sequence::new((640, 360), 30);
    let r = seq.add_media(Media::from_file(&red).unwrap());
    let b = seq.add_media(Media::from_file(&blue).unwrap());
    let first = seq.append(r).unwrap();
    let second = seq.append(b).unwrap();
    seq.clip_mut(second).unwrap().transition = Some(Transition { kind: TransitionKind::Crossfade, duration_ms: 1_000 });
    // the red one in black and white
    seq.clip_mut(first).unwrap().filter = Filter::PRESETS.iter().find(|p| p.0 == "Mono").unwrap().1;
    // a title over both, moving from the left to the right
    let mut style = TextStyle::title("HELLO");
    style.background = Some((0x00FF00, 1.0));
    let title = seq.place_content(Content::Text(style), 0, 4_000);
    let (tt, _) = seq.find(title).unwrap();
    assert_eq!(tt, 0, "a title goes on the top track");
    let c = seq.clip_mut(title).unwrap();
    c.transform.scale = 0.5;
    c.keyframes = vec![
        Keyframe { at_ms: 0, transform: Transform { x: -200.0, scale: 0.5, ..Transform::default() } },
        Keyframe { at_ms: 4_000, transform: Transform { x: 200.0, scale: 0.5, ..Transform::default() } },
    ];

    let mut r = match SequenceRenderer::new(&seq) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no renderer here ({e}); skipping");
            return;
        }
    };
    let at = |img: &[u8], x, y| px(img, 640, x, y);
    let early = r.frame(100);
    let grey = at(&early, 600, 340);
    assert!(grey[0].abs_diff(grey[1]) < 12 && grey[0].abs_diff(grey[2]) < 12, "the red clip is grey: {grey:?}");
    // (in the box's padding above the words: where a glyph falls depends on the system's font)
    let g = at(&early, 120, 142);
    assert!(g[1] > 180 && g[0] < 80, "the title's green box on the left at first: {g:?}");
    assert!(at(&early, 520, 180)[1] < 120, "and not on the right");
    let late = r.frame(3_900);
    assert!(at(&late, 520, 180)[1] > 180, "the title moved to the right");
    // halfway through the crossfade: red's grey under half the blue
    let mid = r.frame(2_000);
    let m = at(&mid, 600, 340);
    assert!(m[2] > 90 && m[2] < 220 && m[0] > 20, "a mix of both: {m:?}");
    let after = r.frame(2_600);
    let a = at(&after, 600, 340);
    assert!(a[2] > 220 && a[0] < 40, "then the blue alone: {a:?}");
}

#[test]
fn captions_hear_the_sequences_sound() {
    let dir = tempfile::tempdir().unwrap();
    let Ok(ff) = media::ffmpeg() else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let tone = dir.path().join("tone.wav");
    assert!(Command::new(ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=2"]).arg(&tone).status().unwrap().success());
    let mut seq = Sequence::new((640, 360), 30);
    let silent = seq.clone();
    let t = seq.add_media(Media::from_file(&tone).unwrap());
    seq.place(t, 1_000, None).unwrap();
    let wav = dir.path().join("speech.wav");
    assert!(!lightair_render::seqrender::sound_to(&silent, &wav).unwrap(), "no sound, nothing to hear");
    assert!(lightair_render::seqrender::sound_to(&seq, &wav).unwrap());
    let c = media::contents(&wav).unwrap();
    assert!(c.audio && (c.duration_ms as i64 - 3_000).abs() < 150, "{} ms", c.duration_ms);

    // the lines become text clips on a captions track, made again in place
    let (style, place) = seq.caption_style();
    let lines = vec![(1_000, 1_800, "Hello there".to_string()), (1_900, 2_600, "again".to_string())];
    let ct = seq.set_captions(&lines, &style, place);
    assert!(seq.tracks[ct].captions && seq.tracks[ct].clips.len() == 2);
    assert_eq!(seq.tracks[ct].clips[0].end_ms(), 1_900, "the short gap closed");
    let again = seq.set_captions(&lines[..1], &style, place);
    assert_eq!((again, seq.tracks[again].clips.len()), (ct, 1));
}

/// The loudest sample (of the left channel) between `a` and `b` ms of 48 kHz stereo.
fn loudest(stereo: &[f32], a: u64, b: u64) -> f32 {
    let f = |ms: u64| ((ms * 48) as usize * 2).min(stereo.len());
    stereo[f(a)..f(b)].iter().step_by(2).fold(0f32, |m, s| m.max(s.abs()))
}

#[test]
fn fades_volume_keys_detached_sound_and_clean_voices() {
    use lightair_render::sequence::{VoiceClean, VolumeKey};
    use lightair_render::sound;
    let dir = tempfile::tempdir().unwrap();
    sound::set_cache(dir.path().join("cache"));
    let Some(video) = lavfi(dir.path(), "v.mp4", "color=c=red:s=320x180:r=30:d=4") else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    // the same picture with a tone in it
    let ff = media::ffmpeg().unwrap();
    let toned = dir.path().join("toned.mp4");
    assert!(Command::new(&ff)
        .args(["-v", "error", "-y", "-i"])
        .arg(&video)
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4:sample_rate=48000", "-shortest", "-c:v", "copy", "-c:a", "aac"])
        .arg(&toned)
        .status()
        .unwrap()
        .success());
    let mut seq = Sequence::new((640, 360), 30);
    let m = seq.add_media(Media::from_file(&toned).unwrap());
    let clip = seq.append(m).unwrap();
    let full = sound::decode(&toned, 48_000, 2).unwrap();
    let level = loudest(&full, 1_000, 3_000);
    assert!(level > 0.05, "a tone to hear: {level}");

    // its sound, detached: the video goes quiet, the new clip sounds
    let sound_clip = seq.detach_sound(clip).unwrap();
    assert!(seq.clip(clip).unwrap().muted);
    let (t, _) = seq.find(sound_clip).unwrap();
    assert_eq!(seq.tracks[t].kind, TrackKind::Audio);
    assert!(seq.detach_sound(clip).is_none(), "only once");
    // faded in over the first second, then down to a fifth from 2 s to 3 s
    let c = seq.clip_mut(sound_clip).unwrap();
    c.fade_in_ms = 1_000;
    c.volume_keys = vec![VolumeKey { at_ms: 2_000, volume: 1.0 }, VolumeKey { at_ms: 3_000, volume: 0.2 }];

    let mut child = sound::stream(&seq, 0, 48_000).unwrap().expect("it has sound");
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(child.stdout.as_mut().unwrap(), &mut bytes).unwrap();
    child.wait().unwrap();
    let mixed: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    assert!(loudest(&mixed, 0, 100) < level * 0.2, "silent at first");
    assert!(loudest(&mixed, 400, 500) < level * 0.6, "fading in");
    assert!(loudest(&mixed, 1_200, 1_900) > level * 0.9, "full");
    assert!(loudest(&mixed, 3_200, 3_800) < level * 0.3, "turned down");

    // from a later point
    let mut child = sound::stream(&seq, 1_500, 48_000).unwrap().unwrap();
    let mut later = Vec::new();
    std::io::Read::read_to_end(child.stdout.as_mut().unwrap(), &mut later).unwrap();
    child.wait().unwrap();
    assert!((later.len() as i64 / 8 - 48 * 2_500).abs() < 48 * 150, "{} frames", later.len() / 8);

    // a cleaned voice is made once, as long as the original, and then mixed instead
    seq.clip_mut(sound_clip).unwrap().voice = Some(VoiceClean::default());
    assert_eq!(sound::voices_to_clean(&seq).len(), 1);
    sound::prepare(&seq).unwrap();
    assert!(sound::voices_to_clean(&seq).is_empty());
    let (inputs, _) = sound::mix_of(&seq);
    // (the cache is set once a run: whichever test set it first)
    assert!(inputs[0].starts_with(sound::cache()), "{inputs:?}");
    let cleaned = sound::decode(&inputs[0], 48_000, 2).unwrap();
    assert!((cleaned.len() as i64 - full.len() as i64).abs() < 48 * 2 * 30, "{} vs {}", cleaned.len(), full.len());
    // waveforms
    let p = sound::peaks(&toned).unwrap();
    assert!((p.len() as i64 - 400).abs() < 5 && p[200] > 0.05, "{}", p.len());
}

#[test]
fn proxies_and_filmstrips() {
    let dir = tempfile::tempdir().unwrap();
    lightair_render::sound::set_cache(dir.path().join("cache"));
    let Some(big) = lavfi(dir.path(), "big.mp4", "testsrc2=s=2560x1440:r=30:d=3") else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let mut seq = Sequence::new((1920, 1080), 30);
    let m = seq.add_media(Media::from_file(&big).unwrap());
    assert!(seq.proxies_missing().is_empty(), "only media on the timeline");
    seq.append(m).unwrap();
    let missing = seq.proxies_missing();
    assert_eq!(missing.len(), 1);
    let medium = &missing[0];
    let mut steps = 0;
    media::make_proxy(&medium.path, &medium.proxy_path(), medium.duration_ms, |_| {
        steps += 1;
        true
    })
    .unwrap();
    assert!(steps > 0, "it tells how far it is");
    let proxy = medium.proxy().expect("made");
    let p = media::probe(&proxy).unwrap();
    assert_eq!((p.width, p.height), (1920, 1080), "fitted in 1080p");
    assert!(seq.proxies_missing().is_empty());
    seq.proxies = false;
    assert!(seq.proxies_missing().is_empty(), "none wanted when they're off");
    seq.proxies = true;

    // a preview reads the proxy, at the player's size; the export the original
    let mut r = match SequenceRenderer::new(&seq) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no renderer here ({e}); skipping");
            return;
        }
    };
    r.set_preview(Some((960, 540)));
    assert_eq!(r.out_size(), (960, 540));
    let f = r.frame(1_000);
    assert_eq!(f.len(), 960 * 540 * 4);
    r.set_preview(None);
    assert_eq!(r.frame(1_000).len(), 1920 * 1080 * 4);

    let (frames, size) = media::filmstrip(&big, 3_000, 500, 80, 16.0 / 9.0).unwrap();
    assert_eq!(size, (142, 80));
    assert!((5..=7).contains(&frames.len()), "{}", frames.len());
    assert_eq!(frames[0].len(), 142 * 80 * 4);
}

#[test]
fn music_ducks_under_voices() {
    use lightair_render::sound;
    let dir = tempfile::tempdir().unwrap();
    let Ok(ff) = media::ffmpeg() else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let tone = |name: &str, f: u32, d: u32| {
        let p = dir.path().join(name);
        assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={f}:duration={d}:sample_rate=48000")]).arg(&p).status().unwrap().success());
        p
    };
    let (voice, music) = (tone("voice.wav", 300, 2), tone("music.wav", 800, 6));
    let mut seq = Sequence::new((640, 360), 30);
    let v = seq.add_media(Media::from_file(&voice).unwrap());
    let m = seq.add_media(Media::from_file(&music).unwrap());
    seq.place(v, 2_000, None).unwrap();
    let bed = seq.place(m, 0, None).unwrap();
    let level = |seq: &Sequence, a: u64, b: u64| {
        let mut child = sound::stream(seq, 0, 48_000).unwrap().unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(child.stdout.as_mut().unwrap(), &mut bytes).unwrap();
        child.wait().unwrap();
        let s: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        assert!((s.len() as i64 / 2 - 48 * 6_000).abs() < 48 * 100, "the music carries on to its end: {} frames", s.len() / 2);
        // the music's own frequency, by its energy against the voice: a band-pass by ear is too
        // much here, so its loudness while it's alone and while the voice sounds is compared
        let f = |ms: u64| (ms * 48) as usize * 2;
        (s[f(a)..f(b)].iter().step_by(2).map(|x| x * x).sum::<f32>() / ((b - a) * 48) as f32).sqrt()
    };
    let alone = level(&seq, 500, 1_500);
    seq.clip_mut(bed).unwrap().duck = true;
    let ducked_alone = level(&seq, 500, 1_500);
    assert!((ducked_alone - alone).abs() < alone * 0.1, "not ducked while nobody speaks: {alone} vs {ducked_alone}");
    // after the voice: the music alone again, back to full
    let after = level(&seq, 5_000, 5_800);
    assert!(after > alone * 0.8, "back up after the voice: {after} vs {alone}");
    // while the voice sounds: the sum is quieter than voice + full music
    let mut plain = seq.clone();
    plain.clip_mut(bed).unwrap().duck = false;
    let (with, without) = (level(&seq, 2_500, 3_500), level(&plain, 2_500, 3_500));
    assert!(with < without * 0.85, "the music turned down under the voice: {with} vs {without}");
}

#[test]
fn scrubbing_back_reads_frames_already_decoded() {
    let dir = tempfile::tempdir().unwrap();
    let Some(src) = lavfi(dir.path(), "count.mp4", "testsrc2=s=320x180:r=30:d=4") else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let fresh = |ms: u64| media::Stream::new(&src, (320, 180), 30.0).frame(ms).unwrap().to_vec();
    let mut s = media::Stream::new(&src, (320, 180), 30.0);
    s.frame(3_000).unwrap();
    // back a little at a time, as a scrub goes: the same pictures as read afresh
    for ms in [2_900, 2_500, 2_200, 2_100] {
        let f = s.frame(ms).unwrap().to_vec();
        assert!(f == fresh(ms), "the frame at {ms} ms");
    }
    let start = std::time::Instant::now();
    for k in 0..20 {
        s.frame(2_100 - k * 10).unwrap();
    }
    assert!(start.elapsed() < std::time::Duration::from_millis(400), "from what was read: {:?}", start.elapsed());
}

#[test]
fn a_take_comes_as_its_parts() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Take.lightair-take");
    std::fs::create_dir_all(&folder).unwrap();
    let (Some(_), Some(_)) = (lavfi(&folder, "screen.mp4", "color=c=red:s=640x360:r=30:d=2"), lavfi(&folder, "camera.mp4", "color=c=blue:s=320x180:r=30:d=2")) else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let ff = media::ffmpeg().unwrap();
    for name in ["Audio - Mic.wav", "Audio - Desktop audio.wav"] {
        assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=2"]).arg(folder.join(name)).status().unwrap().success());
    }
    std::fs::write(
        folder.join("take.json"),
        r#"{"duration_ms":2000,"video":[{"name":"Camera","file":"camera.mp4"},{"name":"Screen","file":"screen.mp4"}],"audio":["Audio - Mic.wav","Audio - Desktop audio.wav"]}"#,
    )
    .unwrap();
    let parts = Media::studio_take_parts(&folder);
    let names: Vec<&str> = parts.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names[1..], ["Camera", "Mic", "Desktop audio"]);
    assert!(parts[0].studio_take.is_some() && parts[0].path.ends_with("screen.mp4"), "the screen, with its look");
    assert!(parts[0].audio.is_empty(), "its sound is in the parts, not doubled");
    assert_eq!(parts[1].kind, lightair_render::sequence::MediaKind::Video);
    assert!(parts[2..].iter().all(|m| m.kind == lightair_render::sequence::MediaKind::Audio));
    // placed together, each sound on a track of its own
    let mut seq = Sequence::new((1920, 1080), 30);
    let ids: Vec<_> = parts.into_iter().map(|m| seq.add_media(m)).collect();
    let clips: Vec<_> = ids.iter().map(|id| seq.place(*id, 1_000, None).unwrap()).collect();
    let tracks: Vec<usize> = clips.iter().map(|c| seq.find(*c).unwrap().0).collect();
    assert!(clips.iter().all(|c| seq.clip(*c).unwrap().start_ms == 1_000));
    let mut unique = tracks.clone();
    unique.dedup();
    assert_eq!(unique.len(), 4, "four tracks: {tracks:?}");
}

#[test]
fn a_takes_scene_comes_back_as_layers_where_they_were() {
    use lightair_render::sequence::{Content, MediaKind};
    use lightair_store::studio_take_scene::{SceneObject, StudioTakeScene, What};
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Take.lightair-take");
    std::fs::create_dir_all(folder.join("layers")).unwrap();
    let Some(_) = lavfi(&folder, "camera.mp4", "color=c=blue:s=640x360:r=30:d=2") else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let ff = media::ffmpeg().unwrap();
    assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=yellow:s=200x200", "-frames:v", "1"]).arg(folder.join("layers/0-logo.png")).status().unwrap().success());
    assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=2"]).arg(folder.join("Audio - Mic.wav")).status().unwrap().success());
    let camera = lightair_core::Id::new();
    std::fs::write(
        folder.join("take.json"),
        format!(r#"{{"duration_ms":2000,"video":[{{"name":"Camera","file":"camera.mp4","input":"{camera}"}}],"audio":["Audio - Mic.wav"]}}"#),
    )
    .unwrap();
    let layer = |name: &str, x, y, w, h, what| SceneObject { name: name.into(), x, y, w, h, rotation: 0.0, opacity: 1.0, what };
    StudioTakeScene {
        canvas: (1920, 1080),
        objects: vec![
            layer("Background", 0.0, 0.0, 1.0, 1.0, What::Colour { color: 0x00FF00, alpha: 1.0 }),
            layer("Camera", 0.05, 0.1, 0.5, 0.5, What::Recorded { source: camera }),
            layer("Logo", 0.8, 0.05, 0.1, 0.1 * 16.0 / 9.0, What::Image { file: "layers/0-logo.png".into() }),
            layer("Title", 0.6, 0.7, 0.35, 0.2, What::Text { text: "Hello".into(), size: 96.0, color: 0xFFFFFF, bold: true }),
            layer("Web", 0.0, 0.0, 0.1, 0.1, What::Other { kind: "browser_source".into() }),
        ],
    }
    .write(&folder)
    .unwrap();

    let parts = Media::studio_take_layout(&folder);
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Background", "Camera", "Logo", "Title", "Mic"], "the browser left out; no screen, so no look");
    assert!(parts[1].media.as_ref().is_some_and(|m| m.studio_take.is_none() && m.kind == MediaKind::Video));
    assert!(matches!(parts[0].content, Some(Content::Shape(_))) && matches!(parts[3].content, Some(Content::Text(_))));

    let mut seq = Sequence::new((1920, 1080), 30);
    let clips: Vec<_> = parts.into_iter().map(|p| seq.place_part(p, 0, 2_000, None).unwrap()).collect();
    // bottom to top as in the scene: each later layer on a higher track (track 0 is the top)
    let tracks: Vec<usize> = clips[..4].iter().map(|c| seq.find(*c).unwrap().0).collect();
    assert!(tracks.windows(2).all(|w| w[0] > w[1]), "{tracks:?}");
    let mut r = match SequenceRenderer::new(&seq) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no renderer here ({e}); skipping");
            return;
        }
    };
    let f = r.frame(1_000);
    let at = |x: f32, y: f32| px(&f, 1920, (x * 1920.0) as u32, (y * 1080.0) as u32);
    assert!(at(0.02, 0.95)[1] > 200 && at(0.02, 0.95)[0] < 60, "the green background: {:?}", at(0.02, 0.95));
    let cam = at(0.3, 0.35);
    assert!(cam[2] > 200 && cam[0] < 60, "the camera in its box: {cam:?}");
    assert!(at(0.6, 0.35)[1] > 200 && at(0.6, 0.35)[2] < 60, "and not beyond it");
    let logo = at(0.85, 0.14);
    assert!(logo[0] > 200 && logo[1] > 200 && logo[2] < 80, "the logo: {logo:?}");
}

#[test]
fn what_was_behind_a_takes_screen_stays_behind_it() {
    use lightair_store::studio_take_scene::{SceneObject, StudioTakeScene, What};
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Studio take.lightair-take");
    std::fs::create_dir_all(folder.join("layers")).unwrap();
    let Some(_) = lavfi(&folder, "screen.mp4", "color=c=blue:s=640x360:r=30:d=2") else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    let ff = media::ffmpeg().unwrap();
    assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=yellow:s=200x200", "-frames:v", "1"]).arg(folder.join("layers/0-logo.png")).status().unwrap().success());
    let screen = lightair_core::Id::new();
    std::fs::write(
        folder.join("take.json"),
        format!(r#"{{"duration_ms":2000,"video":[{{"name":"Screen","kind":"screen","file":"screen.mp4","input":"{screen}"}}],"audio":[]}}"#),
    )
    .unwrap();
    let layer = |name: &str, what| SceneObject { name: name.into(), x: 0.0, y: 0.0, w: 1.0, h: 1.0, rotation: 0.0, opacity: 1.0, what };
    StudioTakeScene {
        canvas: (1920, 1080),
        objects: vec![
            layer("Backdrop", What::Colour { color: 0x00FF00, alpha: 1.0 }),
            layer("My app", What::Recorded { source: screen }),
            layer("Logo", What::Image { file: "layers/0-logo.png".into() }),
        ],
    }
    .write(&folder)
    .unwrap();
    let names: Vec<String> = Media::studio_take_layout(&folder).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Backdrop", "My app", "Logo"], "the screen where its layer was, named after it");
}

#[test]
fn every_audio_track_of_a_file_is_a_clip_of_its_own() {
    use lightair_render::sound;
    let dir = tempfile::tempdir().unwrap();
    let Ok(ff) = media::ffmpeg() else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    // a picture and three sounds: quiet, middling, loud; the first two named
    let file = dir.path().join("three.mp4");
    assert!(Command::new(&ff)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=gray:s=320x180:r=30:d=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=300:duration=2,volume=0.1"])
        .args(["-f", "lavfi", "-i", "sine=frequency=500:duration=2,volume=0.4"])
        .args(["-f", "lavfi", "-i", "sine=frequency=700:duration=2,volume=0.9"])
        .args(["-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:a", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .args(["-metadata:s:a:0", "handler_name=Mic", "-metadata:s:a:1", "handler_name=Desktop audio"])
        .arg(&file)
        .status()
        .unwrap()
        .success());
    let m = Media::from_file(&file).unwrap();
    assert_eq!(m.tracks, ["Mic", "Desktop audio", "Track 3"]);
    let mut seq = Sequence::new((320, 180), 30);
    let id = seq.add_media(m);
    let video = seq.append(id).unwrap();
    let sounds = seq.detach_sounds(video);
    assert_eq!(sounds.len(), 3, "a clip per track");
    assert!(seq.clip(video).unwrap().muted);
    let tracks: Vec<usize> = sounds.iter().map(|c| seq.find(*c).unwrap().0).collect();
    assert!(tracks[0] != tracks[1] && tracks[1] != tracks[2] && tracks[0] != tracks[2], "each on a track of its own: {tracks:?}");
    assert_eq!(sounds.iter().map(|c| seq.clip(*c).unwrap().stream).collect::<Vec<_>>(), [Some(0), Some(1), Some(2)]);
    let mut all = seq.linked(video);
    all.sort();
    let mut want = sounds.clone();
    want.push(video);
    want.sort();
    assert_eq!(all, want, "all linked");

    // each plays its own track: alone, each is as loud as its track
    let level = |seq: &Sequence| {
        let mut child = sound::stream(seq, 0, 48_000).unwrap().unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(child.stdout.as_mut().unwrap(), &mut bytes).unwrap();
        child.wait().unwrap();
        let s: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        s[48 * 2 * 500..48 * 2 * 1500].iter().step_by(2).fold(0f32, |m, v| m.max(v.abs()))
    };
    let mut levels = Vec::new();
    for keep in &sounds {
        let mut one = seq.clone();
        for c in &sounds {
            one.clip_mut(*c).unwrap().muted = c != keep;
        }
        levels.push(level(&one));
    }
    assert!(levels[0] < levels[1] && levels[1] < levels[2], "{levels:?}");
    // (the tone is an eighth of full scale): 0.1, 0.4 and 0.9 of it
    let r = levels[2] / levels[0];
    assert!((6.0..12.0).contains(&r), "{levels:?}");
    // their waveforms too: the middle track's
    let p1 = sound::peaks_of(&file, Some(1)).unwrap();
    assert!((p1[100] - 0.4 * 0.125).abs() < 0.01, "{}", p1[100]);
}

#[test]
fn a_clips_waveform_is_what_it_plays() {
    use lightair_render::sound;
    use lightair_render::sequence::Clip;
    let dir = tempfile::tempdir().unwrap();
    let Ok(ff) = media::ffmpeg() else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    // a studio_take's two stems: a quiet voice for the first second, loud desktop sound the next
    let make = |name: &str, filter: &str| {
        let p = dir.path().join(name);
        assert!(Command::new(&ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", filter]).arg(&p).status().unwrap().success());
        p
    };
    let mic = make("Audio - Mic.wav", "sine=frequency=300:duration=2,volume='if(lt(t,1),0.8,0.05)':eval=frame");
    let desktop = make("Audio - Desktop audio.wav", "sine=frequency=700:duration=2,volume='if(lt(t,1),0,1)':eval=frame");
    // an old-style studio_take clip: one picture playing both stems
    let video = lavfi(dir.path(), "camera.mp4", "color=c=gray:s=320x180:r=30:d=2").unwrap();
    let mut m = Media::from_file(&video).unwrap();
    m.audio = vec![mic.clone(), desktop.clone()];
    let mut seq = Sequence::new((320, 180), 30);
    let id = seq.add_media(m);
    let clip = Clip::new(id, 0, 2_000);
    // what it plays: both
    let src = sound::sources(&seq, &clip, false);
    assert_eq!(src, vec![(mic.clone(), None), (desktop.clone(), None)]);
    let (pm, pd) = (sound::peaks(&mic).unwrap(), sound::peaks(&desktop).unwrap());
    let both = sound::combine(&[&pm, &pd]);
    // its waveform is the mix's, not the first stem's: loud in the second second too
    let at = |p: &[f32], s: f32| p[(s * 100.0) as usize];
    assert!(at(&pm, 1.5) < 0.02, "the mic alone is quiet then");
    assert!(at(&both, 1.5) > 0.1, "the two together aren't: {}", at(&both, 1.5));
    assert!(at(&both, 0.5) > 0.09, "and loud in the first second: {}", at(&both, 0.5));
    // as the mix itself is
    let mixed = dir.path().join("mix.wav");
    assert!(Command::new(&ff).args(["-v", "error", "-y", "-i"]).arg(&mic).arg("-i").arg(&desktop).args(["-filter_complex", "amix=inputs=2:normalize=0"]).arg(&mixed).status().unwrap().success());
    let pmix = sound::peaks(&mixed).unwrap();
    for s in [0.5, 1.5] {
        assert!((at(&pmix, s) - at(&both, s)).abs() < 0.03, "at {s} s: {} vs {}", at(&pmix, s), at(&both, s));
    }
}

#[test]
fn a_takes_zooms_from_the_zoom_track_show() {
    use lightair_render::edit::{Focus, Zoom};
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Take.lightair-take");
    std::fs::create_dir_all(&folder).unwrap();
    let Some(_) = lavfi(&folder, "screen.mp4", "testsrc2=size=640x360:rate=30:duration=3") else {
        eprintln!("FFmpeg isn't installed; skipping");
        return;
    };
    std::fs::write(folder.join("take.json"), r#"{"duration_ms":3000,"video":[{"name":"Screen","file":"screen.mp4"}],"audio":[]}"#).unwrap();
    let mut m = Media::studio_take_parts(&folder).into_iter().next().expect("the screen");
    m.zooms = Some(Vec::new());
    let mut seq = Sequence::new((640, 360), 30);
    let id = seq.add_media(m);
    seq.place(id, 0, None).unwrap();
    let mut r = SequenceRenderer::new(&seq).unwrap();
    let flat = r.frame(2_500);
    // a zoom added on the zoom track: the same moment, magnified
    seq.media[0].zooms = Some(vec![Zoom { start_ms: 0, end_ms: 3_000, scale: 3.0, focus: Focus::Point { x: 0.5, y: 0.5 }, auto: false }]);
    r.set(&seq).unwrap();
    let zoomed = r.frame(2_500);
    let differ = flat.chunks(4).zip(zoomed.chunks(4)).filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 30)).count();
    assert!(differ > flat.len() / 4 / 3, "the picture changed: {differ} pixels");
    // and taken away again
    seq.media[0].zooms = Some(Vec::new());
    r.set(&seq).unwrap();
    let back = r.frame(2_500);
    let differ = flat.chunks(4).zip(back.chunks(4)).filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 30)).count();
    assert!(differ < flat.len() / 4 / 50, "as it was: {differ} pixels");
}
