//! A sequence rendered (docs/plans/video-editor.md, phase 1): the frame at any moment (every
//! visible track's clip, back to front, placed by its transform), playback, and export with every
//! clip's sound cut, sped, set to its volume, placed in time and mixed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lightair_core::Id;

use crate::compose::{Compositor, Layer, fit};
use crate::edit::{Edit, Look};
use crate::export::{Renderer, Source};
use crate::media::{self, Codec, Encoder, MediaError, Stream};
use crate::sequence::{Clip, Media, MediaKind, Sequence, TrackKind, TransitionKind};

/// A layer, before it's handed to the compositor.
struct Pic {
    rgba: Vec<u8>,
    size: (u32, u32),
    center: (f32, f32),
    draw: (f32, f32),
    rotation: f32,
    opacity: f32,
    premultiplied: bool,
    filter: crate::sequence::Filter,
    masks: Vec<(f32, f32, f32, f32)>,
    effects: Vec<crate::sequence::ClipEffect>,
    time: f32,
    /// The part of the picture drawn (x, y, w, h; negative: mirrored).
    uv: (f32, f32, f32, f32),
}

/// How a transition draws one of its two clips: faded, moved (frame widths and heights),
/// grown, turned, blurred, and shown only from its left edge to `reveal` (0..1).
#[derive(Clone, Copy)]
struct Shift {
    opacity: f32,
    dx: f32,
    dy: f32,
    grow: f32,
    spin: f32,
    blur: f32,
    reveal: f32,
}

impl Default for Shift {
    fn default() -> Self {
        Shift { opacity: 1.0, dx: 0.0, dy: 0.0, grow: 1.0, spin: 0.0, blur: 0.0, reveal: 1.0 }
    }
}

fn faded(opacity: f32) -> Shift {
    Shift { opacity, ..Shift::default() }
}

/// Where a clip's picture landed in the last frame (result pixels; back to front).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub clip: Id,
    pub center: (f32, f32),
    pub size: (f32, f32),
    /// Degrees.
    pub rotation: f32,
}

pub struct SequenceRenderer {
    seq: Sequence,
    compositor: Compositor,
    fps: f64,
    /// Videos read in time order, by clip (one clip, one reader: two clips of a file at once).
    streams: HashMap<Id, Stream>,
    /// Stills, by medium: RGBA and size.
    stills: HashMap<Id, (Vec<u8>, (u32, u32))>,
    /// Takes, by medium: drawn with their screen recording look at the sequence's size.
    takes: HashMap<Id, (Renderer, Option<PathBuf>)>,
    /// Text and shapes, made once per content.
    made: crate::generate::Cache,
    /// The most a preview's frames are (`None`: the sequence's size), and the size they are.
    preview: Option<(u32, u32)>,
    out: (u32, u32),
    /// Proxies found, by medium (looked for again when the sequence is set).
    proxies: HashMap<Id, Option<PathBuf>>,
    placed: Vec<Placed>,
    /// Previews drawn into IOSurfaces for the window (macOS), not read back.
    zero_copy: bool,
    /// Windows: the composition visual previews are presented into (`IDCompositionVisual*`).
    #[cfg(windows)]
    visual: usize,
}

/// A frame for the player: its pixels (BGRA in a preview), or (macOS) an IOSurface picture.
pub enum Shown {
    Pixels(Vec<u8>),
    #[cfg(target_os = "macos")]
    Surface(crate::surface::Frame),
    /// Windows: presented into the window's player visual (nothing for the UI to draw).
    #[cfg(windows)]
    Presented,
}

/// The size frames are made at: the sequence's, or fitted inside `preview` (never larger).
fn out_size(size: (u32, u32), preview: Option<(u32, u32)>) -> (u32, u32) {
    match preview {
        Some((pw, ph)) if pw < size.0 || ph < size.1 => {
            let k = (pw as f32 / size.0 as f32).min(ph as f32 / size.1 as f32);
            (even(size.0 as f32 * k), even(size.1 as f32 * k))
        }
        _ => size,
    }
}

fn even(v: f32) -> u32 {
    ((v.round() as u32).max(2)) & !1
}

impl SequenceRenderer {
    pub fn new(seq: &Sequence) -> Result<SequenceRenderer, MediaError> {
        Self::new_with(seq, None)
    }

    /// A renderer making previews of at most `preview` (see [`SequenceRenderer::set_preview`]).
    pub fn new_with(seq: &Sequence, preview: Option<(u32, u32)>) -> Result<SequenceRenderer, MediaError> {
        let out = out_size(seq.size, preview);
        let compositor = Compositor::new_with(out, Look::default(), preview.is_some()).map_err(MediaError::Failed)?;
        Ok(SequenceRenderer {
            seq: seq.clone(),
            compositor,
            fps: seq.fps.max(1.0),
            streams: HashMap::new(),
            stills: HashMap::new(),
            takes: HashMap::new(),
            made: Default::default(),
            preview,
            out,
            proxies: HashMap::new(),
            placed: Vec::new(),
            zero_copy: false,
            #[cfg(windows)]
            visual: 0,
        })
    }

    /// Frames for a preview: at most `max`, and BGRA (as a window draws them); `None`: the
    /// sequence's own size, RGBA (for the export). Smaller frames are quicker to decode, draw
    /// and show.
    pub fn set_preview(&mut self, max: Option<(u32, u32)>) {
        let was = self.preview.is_some();
        self.preview = max;
        let out = out_size(self.seq.size, max);
        if (out != self.out || was != max.is_some())
            && let Ok(c) = Compositor::new_with(out, Look::default(), max.is_some())
        {
            self.compositor = c;
            self.out = out;
            self.takes.clear();
            self.streams.clear();
            self.attach();
        }
    }

    /// macOS: previews drawn into IOSurfaces the window shows as they are (see
    /// [`SequenceRenderer::shown`]). Elsewhere it does nothing.
    pub fn set_zero_copy(&mut self, on: bool) {
        self.zero_copy = on;
        self.attach();
    }

    /// Windows: previews presented into `visual` (an `IDCompositionVisual*` the caller keeps
    /// alive), with zero copy on; `None`: read back.
    #[cfg(windows)]
    pub fn set_visual(&mut self, visual: Option<*mut std::ffi::c_void>) {
        self.visual = visual.map_or(0, |v| v as usize);
        // (a new compositor: the old one's surface goes with it)
        if let Ok(c) = crate::compose::Compositor::new_with(self.out, Look::default(), self.preview.is_some()) {
            self.compositor = c;
            self.takes.clear();
            self.streams.clear();
        }
        self.attach();
    }

    fn attach(&mut self) {
        #[cfg(target_os = "macos")]
        if self.zero_copy && self.preview.is_some() {
            self.compositor.attach_surface();
        }
        #[cfg(windows)]
        if self.zero_copy && self.preview.is_some() && self.visual != 0 {
            self.compositor.attach_visual(self.visual as *mut std::ffi::c_void);
        }
    }

    /// The frame at `t` for the player.
    pub fn shown(&mut self, t: u64) -> Shown {
        let pixels = self.frame(t);
        #[cfg(target_os = "macos")]
        if pixels.is_empty()
            && let Some(f) = self.compositor.surface_frame()
        {
            return Shown::Surface(f);
        }
        #[cfg(windows)]
        if pixels.is_empty() && self.compositor.presents() {
            return Shown::Presented;
        }
        Shown::Pixels(pixels)
    }

    /// Frames for the player from `from` to the end (see [`SequenceRenderer::play`]).
    pub fn play_shown(&mut self, from: u64, mut each: impl FnMut(u64, Shown) -> bool) {
        let end = self.seq.duration_ms();
        let step = 1000.0 / self.fps;
        let mut i = (from as f64 / step).ceil() as u64;
        loop {
            let t = (i as f64 * step) as u64;
            if t >= end {
                break;
            }
            let f = self.shown(t);
            if !each(t, f) {
                break;
            }
            i += 1;
        }
    }

    /// Frames for the player from `from` to the end, each made only once `due(t)` says so:
    /// `Some(true)` make it now, `Some(false)` skip it (late), `None` stop. For frames presented
    /// as they're made (Windows's player visual): made ahead, they'd show early.
    pub fn play_paced(&mut self, from: u64, mut due: impl FnMut(u64) -> Option<bool>, mut each: impl FnMut(u64, Shown) -> bool) {
        let end = self.seq.duration_ms();
        let step = 1000.0 / self.fps;
        let mut i = (from as f64 / step).ceil() as u64;
        loop {
            let t = (i as f64 * step) as u64;
            if t >= end {
                break;
            }
            i += 1;
            match due(t) {
                None => break,
                Some(false) => continue,
                Some(true) => {}
            }
            let f = self.shown(t);
            if !each(t, f) {
                break;
            }
        }
    }

    /// Windows: whether previews are presented into the player's visual.
    pub fn presents(&self) -> bool {
        #[cfg(windows)]
        return self.compositor.presents();
        #[cfg(not(windows))]
        return false;
    }

    /// The size frames come at.
    pub fn out_size(&self) -> (u32, u32) {
        self.out
    }

    /// Result pixels to frame pixels.
    fn k(&self) -> f32 {
        self.out.0 as f32 / self.seq.size.0.max(1) as f32
    }

    /// Takes a changed sequence (readers of clips that changed size start again).
    pub fn set(&mut self, seq: &Sequence) -> Result<(), MediaError> {
        let out = out_size(seq.size, self.preview);
        if seq.size != self.seq.size || out != self.out {
            self.compositor = Compositor::new_with(out, Look::default(), self.preview.is_some()).map_err(MediaError::Failed)?;
            self.out = out;
            self.takes.clear();
            self.attach();
        }
        self.fps = seq.fps.max(1.0);
        // a studio_take whose zooms changed: its camera made again
        for m in &seq.media {
            if self.seq.media(m.id).is_some_and(|old| old.zooms != m.zooms) {
                match (&m.zooms, self.takes.get_mut(&m.id)) {
                    (Some(z), Some((r, _))) => r.set_zooms(z),
                    _ => {
                        self.takes.remove(&m.id);
                    }
                }
            }
        }
        self.seq = seq.clone();
        self.proxies.clear();
        let alive: Vec<Id> = seq.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
        self.streams.retain(|id, _| alive.contains(id));
        Ok(())
    }

    /// Draws a studio_take with its look again (the look changed).
    pub fn forget_studio_take(&mut self, media: Id) {
        self.takes.remove(&media);
    }

    pub fn sequence(&self) -> &Sequence {
        &self.seq
    }

    /// Where a clip's picture sits (its centre and drawn size, result pixels) at `t`: by its
    /// transform there, the medium fitted to the frame (made pictures at their own size).
    pub fn placement(&self, clip: &Clip, media: Option<&Media>, made: Option<(u32, u32)>, t: u64) -> ((f32, f32), (f32, f32)) {
        let (w, h) = (self.seq.size.0 as f32, self.seq.size.1 as f32);
        let tr = clip.transform_at(t);
        let (bw, bh) = match (made, media) {
            (Some(s), _) => (s.0 as f32, s.1 as f32),
            (None, Some(m)) if m.studio_take.is_none() && m.width > 0 => {
                let (_, _, bw, bh) = fit(m.width as f32 / m.height.max(1) as f32, 0.0, 0.0, w, h);
                (bw, bh)
            }
            _ => (w, h),
        };
        ((w / 2.0 + tr.x, h / 2.0 + tr.y), (bw * tr.scale, bh * tr.scale))
    }

    /// The picture of one clip at timeline time `t` (held at its edge frame outside it):
    /// RGBA, its size, and whether it's premultiplied (made pictures).
    fn picture(&mut self, clip: &Clip, t: u64) -> Option<(Vec<u8>, (u32, u32), bool)> {
        if let Some(content) = &clip.content {
            let r = self.made.get(content);
            return Some((r.rgba, r.size, true));
        }
        let media = self.seq.media(clip.media)?.clone();
        let s = clip.source_held(t);
        if let Some(folder) = &media.studio_take {
            // a preview reads the screen's proxy when there is one
            let proxy = if self.preview.is_some() && self.seq.proxies { self.proxies.entry(media.id).or_insert_with(|| media.proxy()).clone() } else { None };
            if self.takes.get(&media.id).is_none_or(|(_, used)| *used != proxy) {
                let mut source = Source::of_studio_take(folder)?;
                if let Some(p) = &proxy {
                    source.video = p.clone();
                }
                let look = media.look.clone().unwrap_or_else(|| folder.join("edit.json"));
                let mut edit = Edit::load(&look).unwrap_or_else(|_| Edit::new(media.duration_ms.max(1)));
                edit.size = self.out;
                edit.clips = Edit::new(media.duration_ms.max(1)).clips;
                // the zooms as edited on the zoom track
                if let Some(z) = &media.zooms {
                    edit.zooms = z.clone();
                }
                let r = Renderer::new(source, &edit).ok()?;
                self.takes.insert(media.id, (r, proxy));
            }
            let fps = self.fps;
            let frame = self.takes.get_mut(&media.id)?.0.source_frame(s, fps)?;
            return Some((frame, self.out, false));
        }
        if media.kind == MediaKind::Image {
            if let std::collections::hash_map::Entry::Vacant(e) = self.stills.entry(media.id) {
                let k = (2048.0 / media.width.max(media.height).max(1) as f32).min(1.0);
                let size = (even(media.width as f32 * k), even(media.height as f32 * k));
                e.insert((media::frame_at(&media.path, 0, size).ok()?, size));
            }
            return self.stills.get(&media.id).cloned().map(|(r, s)| (r, s, false));
        }
        // the size it's drawn at (enough for its largest scale), never more than the file's own
        let (_, draw) = self.placement(clip, Some(&media), None, t);
        let biggest = clip.keyframes.iter().map(|k| k.transform.scale).chain(clip.keys_of("scale").iter().map(|k| k.v)).fold(clip.transform.scale, f32::max);
        let k = (draw.0 * self.k() / clip.transform_at(t).scale.max(0.01) * biggest / media.width.max(1) as f32).min(1.0);
        let size = (even(media.width as f32 * k), even(media.height as f32 * k));
        // a preview reads the proxy when there is one
        let proxy = if self.preview.is_some() && self.seq.proxies { self.proxies.entry(media.id).or_insert_with(|| media.proxy()).clone() } else { None };
        let file = proxy.unwrap_or_else(|| media.path.clone());
        if self.streams.get(&clip.id).is_none_or(|s| s.size() != size || s.path() != file) {
            self.streams.insert(clip.id, Stream::new(&file, size, self.fps));
        }
        let rgba = self.streams.get_mut(&clip.id)?.frame(s)?.to_vec();
        Some((rgba, size, false))
    }

    /// White across the frame (a dip to white).
    fn white(&self, opacity: f32) -> Pic {
        let (w, h, k) = (self.seq.size.0 as f32, self.seq.size.1 as f32, self.k());
        Pic {
            rgba: vec![255; 16],
            size: (2, 2),
            center: (w / 2.0 * k, h / 2.0 * k),
            draw: (w * k, h * k),
            rotation: 0.0,
            opacity,
            premultiplied: false,
            filter: Default::default(),
            masks: Vec::new(),
            effects: Vec::new(),
            time: 0.0,
            uv: (0.0, 0.0, 1.0, 1.0),
        }
    }

    /// One clip as a layer at `t`, with its animations, shifted by a transition.
    fn layer_of(&mut self, clip: &Clip, t: u64, sh: Shift, out: &mut Vec<Pic>) {
        let Some((rgba, size, premultiplied)) = self.picture(clip, t) else { return };
        let media = self.seq.media(clip.media).cloned();
        let made = clip.content.as_ref().map(|_| size);
        let (center, draw) = self.placement(clip, media.as_ref(), made, t);
        // its animation and the transition's move, on top of where it is
        let (fw, fh) = (self.seq.size.0 as f32, self.seq.size.1 as f32);
        let m = clip.motion_at(t);
        let center = (center.0 + (m.dx + sh.dx) * fw, center.1 + (m.dy + sh.dy) * fh);
        let grow = m.scale * sh.grow;
        // cropped: what's left stays where it was, its box smaller
        let (l, top, r, b) = clip.crop_at(t);
        let (l, top) = (l.clamp(0.0, 0.95), top.clamp(0.0, 0.95));
        let (r, b) = (r.clamp(0.0, 0.95 - l), b.clamp(0.0, 0.95 - top));
        let center = (center.0 + (l - r) / 2.0 * draw.0, center.1 + (top - b) / 2.0 * draw.1);
        let draw = (draw.0 * (1.0 - l - r), draw.1 * (1.0 - top - b));
        let mut uv = (l, top, 1.0 - l - r, 1.0 - top - b);
        if clip.flip_h {
            uv = (uv.0 + uv.2, uv.1, -uv.2, uv.3);
        }
        if clip.flip_v {
            uv = (uv.0, uv.1 + uv.3, uv.2, -uv.3);
        }
        // a wipe: only its left part, up to where the edge has got to
        let (center, draw) = if sh.reveal < 1.0 {
            let r = sh.reveal.clamp(0.0, 1.0);
            let left = center.0 - draw.0 * grow / 2.0;
            uv = (uv.0, uv.1, uv.2 * r, uv.3);
            ((left + draw.0 * grow * r / 2.0, center.1), (draw.0 * r, draw.1))
        } else {
            (center, draw)
        };
        let tr = clip.transform_at(t);
        let rotation = tr.rotation + m.rotation + sh.spin;
        let k = self.k();
        self.placed.push(Placed { clip: clip.id, center, size: (draw.0 * grow, draw.1 * grow), rotation });
        let mut filter = clip.filter_at(t);
        filter.blur = (filter.blur + m.blur + sh.blur).min(1.0);
        out.push(Pic {
            rgba,
            size,
            center: (center.0 * k, center.1 * k),
            draw: (draw.0 * grow * k, draw.1 * grow * k),
            rotation,
            // faded in and out, with its sound
            opacity: tr.opacity * sh.opacity * m.opacity * clip.fade_at(t),
            premultiplied,
            filter,
            masks: clip.hidden.clone(),
            effects: clip.effects_at(t),
            time: t.saturating_sub(clip.start_ms) as f32 / 1000.0,
            uv,
        });
    }

    /// The frame at timeline time `t`: RGBA at the sequence's size.
    /// Where each clip's picture landed in the last frame.
    pub fn placed(&self) -> &[Placed] {
        &self.placed
    }

    pub fn frame(&mut self, t: u64) -> Vec<u8> {
        self.placed.clear();
        let tracks: Vec<Vec<Clip>> = self
            .seq
            .tracks
            .iter()
            .filter(|tr| tr.kind == TrackKind::Video && !tr.hidden)
            .rev()
            .map(|tr| tr.clips.clone())
            .collect();
        let mut pics: Vec<Pic> = Vec::new();
        for clips in tracks {
            let Some(i) = clips.iter().position(|c| c.start_ms <= t && t < c.end_ms()) else { continue };
            let c = &clips[i];
            // a transition across a cut: half in the clip before it, half in the clip after
            let across = match (c.transition, i.checked_sub(1).map(|p| &clips[p])) {
                (Some(tn), Some(prev)) if prev.end_ms() == c.start_ms && t < c.start_ms + tn.duration_ms / 2 => {
                    Some((prev.clone(), c.clone(), 0.5 + (t - c.start_ms) as f32 / tn.duration_ms.max(1) as f32, tn.kind))
                }
                _ => clips.get(i + 1).and_then(|n| {
                    let tn = n.transition?;
                    (n.start_ms == c.end_ms() && t + tn.duration_ms / 2 >= c.end_ms())
                        .then(|| (c.clone(), n.clone(), 0.5 - (c.end_ms() - t) as f32 / tn.duration_ms.max(1) as f32, tn.kind))
                }),
            };
            match across {
                None => self.layer_of(c, t, Shift::default(), &mut pics),
                Some((a, b, k, kind)) => {
                    let k = k.clamp(0.0, 1.0);
                    let e = k * k * (3.0 - 2.0 * k);
                    let d = Shift::default();
                    let (sa, sb) = match kind {
                        TransitionKind::Crossfade => (d, faded(k)),
                        TransitionKind::DipToBlack | TransitionKind::DipToWhite => {
                            // black: the frame shows through; white: drawn over the clips
                            let dip = if kind == TransitionKind::DipToBlack { 1.0 } else { 0.0 };
                            if k < 0.5 { (faded(1.0 - 2.0 * k * dip), faded(0.0)) } else { (faded(0.0), faded(1.0 - (2.0 - 2.0 * k) * dip)) }
                        }
                        TransitionKind::Slide => (Shift { dx: -e, ..d }, Shift { dx: 1.0 - e, ..d }),
                        TransitionKind::SlideRight => (Shift { dx: e, ..d }, Shift { dx: e - 1.0, ..d }),
                        TransitionKind::SlideUp => (Shift { dy: -e, ..d }, Shift { dy: 1.0 - e, ..d }),
                        TransitionKind::SlideDown => (Shift { dy: e, ..d }, Shift { dy: e - 1.0, ..d }),
                        TransitionKind::Zoom => (Shift { grow: 1.0 + 0.15 * k, ..d }, Shift { opacity: k, grow: 1.25 - 0.25 * k, ..d }),
                        TransitionKind::ZoomOut => (Shift { opacity: 1.0 - e, grow: 1.0 - 0.4 * e, ..d }, Shift { grow: 1.0 + 0.3 * (1.0 - e), ..d }),
                        TransitionKind::Spin => (Shift { opacity: 1.0 - e, spin: e * 180.0, grow: 1.0 - 0.6 * e, ..d }, Shift { opacity: e, spin: (e - 1.0) * 180.0, grow: 0.4 + 0.6 * e, ..d }),
                        TransitionKind::Blur => {
                            let haze = 1.0 - (2.0 * k - 1.0).abs();
                            (Shift { blur: haze * 0.9, ..d }, Shift { opacity: e, blur: haze * 0.9, ..d })
                        }
                        TransitionKind::Wipe => (d, Shift { reveal: e, ..d }),
                    };
                    if sa.opacity > 0.0 {
                        self.layer_of(&a, t, sa, &mut pics);
                    }
                    if sb.opacity > 0.0 {
                        self.layer_of(&b, t, sb, &mut pics);
                    }
                    if kind == TransitionKind::DipToWhite {
                        pics.push(self.white(1.0 - (2.0 * k - 1.0).abs()));
                    }
                }
            }
        }
        let layers: Vec<Layer> = pics
            .iter()
            .map(|p| Layer {
                rgba: &p.rgba,
                size: p.size,
                center: p.center,
                draw: p.draw,
                rotation: p.rotation,
                opacity: p.opacity,
                premultiplied: p.premultiplied,
                filter: p.filter,
                masks: &p.masks,
                effects: &p.effects,
                time: p.time,
                uv: p.uv,
            })
            .collect();
        self.compositor.layers(&layers)
    }

    /// Frames from `from` to the end at the sequence's rate: `each(t, rgba)` until it says stop.
    pub fn play(&mut self, from: u64, mut each: impl FnMut(u64, Vec<u8>) -> bool) {
        let end = self.seq.duration_ms();
        let step = 1000.0 / self.fps;
        let mut i = (from as f64 / step).ceil() as u64;
        loop {
            let t = (i as f64 * step) as u64;
            if t >= end {
                break;
            }
            let f = self.frame(t);
            if !each(t, f) {
                break;
            }
            i += 1;
        }
    }

    /// The sound of the whole sequence as an FFmpeg filter over `inputs` (in order, from input
    /// 1), or `None` when nothing makes a sound.
    pub fn mix(&self) -> (Vec<PathBuf>, Option<String>) {
        mix_of(&self.seq)
    }

    /// Renders the sequence into `out`. `progress(fraction)` returns false to stop.
    pub fn export(&mut self, out: &Path, codec: Codec, mut progress: impl FnMut(f32) -> bool) -> Result<(), MediaError> {
        let total = self.seq.duration_ms().max(1);
        // voices to clean up, first
        crate::sound::prepare(&self.seq)?;
        self.zero_copy = false;
        self.set_preview(None);
        let (inputs, mix) = self.mix();
        let mut enc = Encoder::start_with(out, self.seq.size, self.fps, codec, &inputs, &[], None, mix.as_deref())?;
        let mut failed = None;
        let mut stopped = false;
        let step = 1000.0 / self.fps;
        self.play(0, |t, rgba| {
            if let Err(e) = enc.write(&rgba) {
                failed = Some(e);
                return false;
            }
            stopped = !progress(((t as f64 + step) / total as f64).min(1.0) as f32);
            !stopped
        });
        if let Some(e) = failed {
            return Err(e);
        }
        if stopped {
            drop(enc);
            let _ = std::fs::remove_file(out);
            return Err(MediaError::Failed("stopped".into()));
        }
        progress(1.0);
        enc.finish()
    }
}

impl SequenceRenderer {
    /// Renders `from_ms..to_ms` of the sequence into `out` as `settings` say. `progress(fraction)`
    /// returns false to stop. Where it went (PNG frames: their folder).
    pub fn export_with(
        &mut self,
        out: &Path,
        settings: &crate::exporter::ExportSettings,
        (from_ms, to_ms): (u64, u64),
        mut progress: impl FnMut(f32) -> bool,
    ) -> Result<std::path::PathBuf, MediaError> {
        let end = to_ms.min(self.seq.duration_ms());
        let start = from_ms.min(end);
        let total = (end - start).max(1);
        crate::sound::prepare(&self.seq)?;
        self.zero_copy = false;
        self.set_preview(None);
        // where nothing covers the frame: see-through, or black
        self.compositor.set_transparent(settings.keeps_alpha());
        let project_fps = self.fps;
        let fps = settings.fps_for(project_fps);
        self.fps = fps;
        let (inputs, mix) = self.mix();
        // the sound of the same stretch of time
        // (the mix ends in `[aout]`: that becomes `[amix]`, trimmed into `[aout]`)
        let mix = mix.map(|m| {
            let body = m.strip_suffix("[aout]").unwrap_or(&m);
            format!("{body}[amix];[amix]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS[aout]", start as f64 / 1000.0, end as f64 / 1000.0)
        });
        let result = (|| {
            let mut enc = Encoder::start_export(out, self.seq.size, fps, settings, &inputs, mix.as_deref())?;
            let mut failed = None;
            let mut stopped = false;
            if settings.container.has_video() {
                let step = 1000.0 / fps;
                self.play(start, |t, rgba| {
                    if t >= end {
                        return false;
                    }
                    if let Err(e) = enc.write(&rgba) {
                        failed = Some(e);
                        return false;
                    }
                    stopped = !progress(((t - start) as f64 + step).min(total as f64) as f32 / total as f32);
                    !stopped
                });
            }
            if let Some(e) = failed {
                return Err(e);
            }
            if stopped {
                drop(enc);
                return Err(MediaError::Failed("stopped".into()));
            }
            enc.finish()?;
            progress(1.0);
            Ok(())
        })();
        self.fps = project_fps;
        self.compositor.set_transparent(false);
        let target = settings.target(out);
        let written = if settings.container == crate::exporter::Container::Png { target.parent().map(Path::to_path_buf).unwrap_or(target) } else { target };
        match result {
            Ok(()) => Ok(written),
            Err(e) => {
                // nothing half made stays
                if written.is_dir() {
                    let _ = std::fs::remove_dir_all(&written);
                } else {
                    let _ = std::fs::remove_file(&written);
                }
                Err(e)
            }
        }
    }
}

pub use crate::sound::{mix_of, sound_to};
