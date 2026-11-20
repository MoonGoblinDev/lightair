//! A sequence (docs/plans/video-editor.md): the Editor's multi-track document. Media, video and
//! audio tracks, and clips placed on them. Times are milliseconds on the timeline unless a name
//! says `in`/`out` (the medium's own time).

use std::path::{Path, PathBuf};

use lightair_core::Id;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

/// A file the sequence uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub id: Id,
    pub name: String,
    pub kind: MediaKind,
    /// The picture or sound (a take: its screen video).
    pub path: PathBuf,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    /// Its own sound (videos), or the files that play with it (a studio_take's stems).
    #[serde(default)]
    pub audio: Vec<PathBuf>,
    /// A Studio studio_take's folder: drawn with its screen recording look.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(alias = "take")]
    pub studio_take: Option<PathBuf>,
    /// The Library item it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<Id>,
    /// A studio_take's look (its `Edit`: zooms, pointer, backdrop), kept in the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<PathBuf>,
    /// Its words, when it's been transcribed (source time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<crate::speech::Transcript>,
    /// The names of its audio tracks, when the file has several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<String>,
    /// A studio_take's zooms (source time), as edited on the zoom track; `None`: its look's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zooms: Option<Vec<crate::edit::Zoom>>,
}

impl Media {
    /// A file as a medium: what it holds, its size and length (from FFmpeg).
    pub fn from_file(path: &Path) -> Result<Media, crate::media::MediaError> {
        let c = crate::media::contents(path)?;
        let kind = if c.still {
            MediaKind::Image
        } else if c.video {
            MediaKind::Video
        } else {
            MediaKind::Audio
        };
        Ok(Media {
            id: Id::new(),
            name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            kind,
            path: path.to_path_buf(),
            duration_ms: c.duration_ms,
            width: c.width,
            height: c.height,
            audio: if c.audio && kind == MediaKind::Video { vec![path.to_path_buf()] } else { Vec::new() },
            studio_take: None,
            item: None,
            look: None,
            transcript: None,
            tracks: if c.audio { Some(crate::media::audio_tracks(path)).filter(|t| t.len() > 1).unwrap_or_default() } else { Vec::new() },
            zooms: None,
        })
    }

    /// A Studio studio_take as layers, as its scene was (its `scene.json`): each layer in drawing
    /// order where it was (cameras, pictures, video files, text, colours), with the screen drawn
    /// with its look across the frame (when it has one) where its layer was, so what was behind
    /// it stays behind it; then each recorded mixer track's sound. A studio_take without a scene:
    /// its parts ([`Media::studio_take_parts`]), unplaced.
    pub fn studio_take_layout(folder: &Path) -> Vec<Part> {
        let parts = Media::studio_take_parts(folder);
        let Some(scene) = lightair_store::studio_take_scene::StudioTakeScene::read(folder) else {
            return parts.into_iter().map(Part::of).collect();
        };
        let studio_take: serde_json::Value = std::fs::read(folder.join("take.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let videos: Vec<&serde_json::Value> = studio_take["video"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
        let screen = crate::export::studio_take_screen(&videos);
        let (canvas_w, canvas_h) = (scene.canvas.0.max(1) as f32, scene.canvas.1.max(1) as f32);
        let mut out = Vec::new();
        // the screen's layer in the scene: what was drawn before it stays behind it
        let screen_layer = screen.and_then(|screen| {
            scene.objects.iter().position(|l| {
                matches!(&l.what, lightair_store::studio_take_scene::What::Recorded { source: input } if screen["input"].as_str() == Some(&input.to_string()))
            })
        });
        let mut screen_part = None;
        if screen.is_some()
            && let Some(main) = parts.first()
        {
            // named as its layer was in the scene
            let named = screen_layer.map(|i| scene.objects[i].name.clone());
            screen_part = Some(Part { name: named.unwrap_or_else(|| "Screen".into()), ..Part::of(main.clone()) });
        }
        // a screen with no layer of its own goes at the back
        if screen_layer.is_none() {
            out.extend(screen_part.take());
        }
        for (i, layer) in scene.objects.iter().enumerate() {
            // the screen, with its look, across the frame, where its layer was
            if Some(i) == screen_layer {
                out.extend(screen_part.take());
            }
            use lightair_store::studio_take_scene::What;
            let place = Some(Placement { x: layer.x, y: layer.y, w: layer.w, h: layer.h, rotation: layer.rotation, opacity: layer.opacity, canvas: scene.canvas });
            let part = match &layer.what {
                What::Recorded { source: input } => {
                    let Some(v) = videos.iter().find(|v| v["input"].as_str() == Some(&input.to_string())) else { continue };
                    if screen.is_some_and(|s| std::ptr::eq(s, *v)) {
                        continue;
                    }
                    let Some(file) = v["file"].as_str() else { continue };
                    let Ok(mut m) = Media::from_file(&folder.join(file)) else { continue };
                    m.name = layer.name.clone();
                    Part { name: layer.name.clone(), media: Some(m), content: None, place, mute_own_sound: false }
                }
                What::Image { file } => {
                    let Ok(mut m) = Media::from_file(&folder.join(file)) else { continue };
                    m.name = layer.name.clone();
                    Part { name: layer.name.clone(), media: Some(m), content: None, place, mute_own_sound: false }
                }
                What::Video { path } => {
                    let Ok(mut m) = Media::from_file(path) else { continue };
                    m.name = layer.name.clone();
                    Part { name: layer.name.clone(), media: Some(m), content: None, place, mute_own_sound: false }
                }
                What::Text { text, size, color, bold } => Part {
                    name: layer.name.clone(),
                    media: None,
                    mute_own_sound: false,
                    content: Some(Content::Text(TextStyle {
                        text: text.clone(),
                        size: *size,
                        color: *color,
                        bold: *bold,
                        align: 0,
                        background: None,
                        outline: None,
                        width: (layer.w * canvas_w).max(*size),
                        font: None,
                    })),
                    place,
                },
                What::Colour { color, alpha } => Part {
                    name: layer.name.clone(),
                    media: None,
                    mute_own_sound: false,
                    content: Some(Content::Shape(ShapeStyle {
                        shape: ShapeKind::Rectangle,
                        width: layer.w * canvas_w,
                        height: layer.h * canvas_h,
                        fill: Some((*color, *alpha)),
                        stroke: None,
                        radius: 0.0,
                    })),
                    place,
                },
                What::Other { .. } => continue,
            };
            out.push(part);
        }
        // the sounds
        out.extend(parts.into_iter().filter(|m| m.kind == MediaKind::Audio).map(Part::of));
        out
    }

    /// A Studio studio_take as the media it's made of: its main picture (the screen, else the first
    /// video) drawn with its look and silent, its other pictures (the camera beside a screen)
    /// as plain videos, and each recorded mixer track's sound as a medium of its own. The main
    /// picture first.
    pub fn studio_take_parts(folder: &Path) -> Vec<Media> {
        let Some(mut main) = Media::from_studio_take(folder) else { return Vec::new() };
        let stems = std::mem::take(&mut main.audio);
        let mut out = vec![main.clone()];
        let studio_take: serde_json::Value = std::fs::read(folder.join("take.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        for v in studio_take["video"].as_array().into_iter().flatten() {
            let Some(file) = v["file"].as_str().map(|f| folder.join(f)) else { continue };
            if file == main.path {
                continue;
            }
            if let Ok(mut m) = Media::from_file(&file) {
                m.name = v["name"].as_str().unwrap_or("Camera").to_string();
                out.push(m);
            }
        }
        for stem in stems {
            if let Ok(mut m) = Media::from_file(&stem) {
                m.name = crate::export::studio_take_track_name(&studio_take, &stem);
                out.push(m);
            }
        }
        out
    }

    /// A Studio studio_take folder as a medium: its screen, drawn with its look, and its stems.
    pub fn from_studio_take(folder: &Path) -> Option<Media> {
        let source = crate::export::Source::of_studio_take(folder)?;
        let c = crate::media::contents(&source.video).ok()?;
        Some(Media {
            id: Id::new(),
            name: folder.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            kind: MediaKind::Video,
            path: source.video,
            duration_ms: c.duration_ms,
            width: c.width,
            height: c.height,
            audio: source.audio,
            studio_take: Some(folder.to_path_buf()),
            item: None,
            look: None,
            transcript: None,
            tracks: Vec::new(),
            zooms: None,
        })
    }
}

/// Where a picture sits: its centre's offset from the frame's centre (result pixels), its scale
/// (1: as large as fits the frame), its turn (degrees) and opacity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation: f32,
    pub opacity: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { x: 0.0, y: 0.0, scale: 1.0, rotation: 0.0, opacity: 1.0 }
    }
}

/// What a generated clip draws (no file behind it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Text(TextStyle),
    Shape(ShapeStyle),
}

/// A text clip's words and look. Sizes are result pixels (at the sequence's size).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub text: String,
    pub size: f32,
    /// 0xRRGGBB.
    pub color: u32,
    pub bold: bool,
    /// 0 left, 1 centre, 2 right.
    pub align: u8,
    /// A box behind the words (0xRRGGBB, and its opacity).
    #[serde(default)]
    pub background: Option<(u32, f32)>,
    /// An outline around the letters.
    #[serde(default)]
    pub outline: Option<u32>,
    /// The widest a line goes before wrapping (result pixels).
    pub width: f32,
    /// The font, by name (docs/plans/fonts.md): none is the renderer's sans-serif (Inter where
    /// the caller gave it the bundled fonts). `bold` makes it at least semibold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<lightair_fonts::FontRef>,
}

impl TextStyle {
    pub fn title(text: &str) -> TextStyle {
        TextStyle { text: text.into(), size: 96.0, color: 0xFFFFFF, bold: true, align: 1, background: None, outline: None, width: 1600.0, font: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Arrow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShapeStyle {
    pub shape: ShapeKind,
    pub width: f32,
    pub height: f32,
    /// 0xRRGGBB, and an opacity; `None`: not filled.
    pub fill: Option<(u32, f32)>,
    /// Line colour and width.
    pub stroke: Option<(u32, f32)>,
    /// Corner radius (rectangles).
    pub radius: f32,
}

/// How a clip arrives from the clip before it on its track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Crossfade,
    DipToBlack,
    Slide,
    Zoom,
    DipToWhite,
    SlideRight,
    SlideUp,
    SlideDown,
    ZoomOut,
    Spin,
    Blur,
    Wipe,
}

impl TransitionKind {
    /// Every transition, by name.
    pub const ALL: [(&'static str, TransitionKind); 12] = [
        ("Crossfade", TransitionKind::Crossfade),
        ("Dip to black", TransitionKind::DipToBlack),
        ("Dip to white", TransitionKind::DipToWhite),
        ("Slide left", TransitionKind::Slide),
        ("Slide right", TransitionKind::SlideRight),
        ("Slide up", TransitionKind::SlideUp),
        ("Slide down", TransitionKind::SlideDown),
        ("Zoom in", TransitionKind::Zoom),
        ("Zoom out", TransitionKind::ZoomOut),
        ("Spin", TransitionKind::Spin),
        ("Blur", TransitionKind::Blur),
        ("Wipe", TransitionKind::Wipe),
    ];

    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(_, k)| *k == self).map_or("Transition", |(n, _)| n)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    pub duration_ms: u64,
}

/// Colour and look adjustments; zero (and 1 for contrast and saturation) is none.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    /// -1..1
    pub brightness: f32,
    /// 0..2
    pub contrast: f32,
    /// 0..2 (0: grey)
    pub saturation: f32,
    /// -1 cool .. 1 warm
    pub temperature: f32,
    /// 0..1
    pub vignette: f32,
    /// 0..1
    pub blur: f32,
}

impl Default for Filter {
    fn default() -> Self {
        Filter { brightness: 0.0, contrast: 1.0, saturation: 1.0, temperature: 0.0, vignette: 0.0, blur: 0.0 }
    }
}

impl Filter {
    /// Lightair's looks, by name.
    pub const PRESETS: &[(&str, Filter)] = &[
        ("None", Filter { brightness: 0.0, contrast: 1.0, saturation: 1.0, temperature: 0.0, vignette: 0.0, blur: 0.0 }),
        ("Vivid", Filter { brightness: 0.02, contrast: 1.12, saturation: 1.35, temperature: 0.0, vignette: 0.0, blur: 0.0 }),
        ("Warm", Filter { brightness: 0.02, contrast: 1.05, saturation: 1.08, temperature: 0.45, vignette: 0.15, blur: 0.0 }),
        ("Cool", Filter { brightness: 0.0, contrast: 1.05, saturation: 0.95, temperature: -0.45, vignette: 0.0, blur: 0.0 }),
        ("Mono", Filter { brightness: 0.0, contrast: 1.15, saturation: 0.0, temperature: 0.0, vignette: 0.2, blur: 0.0 }),
        ("Faded", Filter { brightness: 0.06, contrast: 0.82, saturation: 0.75, temperature: 0.1, vignette: 0.0, blur: 0.0 }),
        ("Cinematic", Filter { brightness: -0.03, contrast: 1.2, saturation: 0.85, temperature: -0.15, vignette: 0.35, blur: 0.0 }),
    ];

    pub fn is_none(&self) -> bool {
        *self == Filter::default()
    }
}

/// The transform at a moment of a clip (from its start, timeline time).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub at_ms: u64,
    pub transform: Transform,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Id,
    pub media: Id,
    /// Where it starts on the timeline.
    pub start_ms: u64,
    /// The piece of the medium it plays.
    pub in_ms: u64,
    pub out_ms: u64,
    pub speed: f32,
    /// 1: as recorded.
    pub volume: f32,
    pub muted: bool,
    #[serde(default)]
    pub transform: Transform,
    /// Generated content (text, a shape); `media` is unused then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
    /// How it arrives from the clip just before it on its track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
    #[serde(default, skip_serializing_if = "Filter::is_none")]
    pub filter: Filter,
    /// The transform over time, when it moves (in time order). The older kind: keys per
    /// property are in `anim`, which these become when one is set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<Keyframe>,
    /// Keys per property (transform, filter, crop, effect settings).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub anim: crate::keys::Anim,
    /// Its sound faded in from silence and out to it (timeline ms).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade_in_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade_out_ms: u64,
    /// The volume over time, when it changes (in time order; `volume` otherwise).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_keys: Vec<VolumeKey>,
    /// A voice cleaned up (noise, harshness, level) before it's mixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceClean>,
    /// Its sound turned down while the other (unducked) clips sound: music under voices.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub duck: bool,
    /// Areas of its picture hidden (pixelated): fractions of the picture (x, y, w, h).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<(f32, f32, f32, f32)>,
    /// Effects from the effect catalog ("fx.vhs", …), run in order on its picture.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<ClipEffect>,
    /// Its picture cut in from each side (fractions: left, top, right, bottom).
    #[serde(default, skip_serializing_if = "is_uncropped")]
    pub crop: (f32, f32, f32, f32),
    /// Its picture mirrored left↔right, top↔bottom.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip_h: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip_v: bool,
    /// Which of its file's audio tracks it plays (`None`: the first, or every one for a video
    /// clip whose sound isn't apart).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<u32>,
    /// Clips that go together (a studio_take's parts, a video and its detached sound): picked,
    /// moved and deleted as one while linkage is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Id>,
    /// Its animations: coming in, going out, or a combo across it.
    #[serde(default, skip_serializing_if = "crate::motion::Motions::is_none")]
    pub motion: crate::motion::Motions,
}

fn is_uncropped(c: &(f32, f32, f32, f32)) -> bool {
    *c == (0.0, 0.0, 0.0, 0.0)
}

/// An effect on a clip's picture: the catalog's kind and its settings (by uniform name).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipEffect {
    /// Its keys are named for it (`fx.<id>.<setting>`), so they stay with it wherever it moves.
    #[serde(default = "Id::new")]
    pub id: Id,
    pub kind: String,
    #[serde(default)]
    pub settings: serde_json::Map<String, serde_json::Value>,
    #[serde(default = "yes")]
    pub on: bool,
}

impl ClipEffect {
    pub fn new(kind: impl Into<String>, settings: serde_json::Map<String, serde_json::Value>) -> ClipEffect {
        ClipEffect { id: Id::new(), kind: kind.into(), settings, on: true }
    }
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// Where a layer goes: its box as fractions of the frame (top-left, size), turned and faded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rotation: f32,
    pub opacity: f32,
    /// The canvas the box was on (made content's sizes are its pixels).
    pub canvas: (u32, u32),
}

/// A part of something added to the Editor: a medium or made content, and where it goes (its
/// place in a studio_take's scene; `None`: as media go).
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub media: Option<Media>,
    pub content: Option<Content>,
    pub place: Option<Placement>,
    /// Its own sound, when it's taken apart, starts muted (other parts have it in parts).
    pub mute_own_sound: bool,
}

impl Part {
    pub fn of(m: Media) -> Part {
        Part { name: m.name.clone(), media: Some(m), content: None, place: None, mute_own_sound: false }
    }

    /// The file it is (a studio_take's screen: the screen recording itself); `None` for made layers.
    pub fn file(&self) -> Option<&Path> {
        self.media.as_ref().map(|m| m.path.as_path())
    }

    /// What it is.
    pub fn kind(&self) -> Option<PartKind> {
        Some(match (&self.media, &self.content) {
            (Some(m), _) if m.studio_take.is_some() => PartKind::Screen,
            (Some(m), _) => match m.kind {
                MediaKind::Video => PartKind::Video,
                MediaKind::Audio => PartKind::Sound,
                MediaKind::Image => PartKind::Picture,
            },
            (None, Some(Content::Text(_))) => PartKind::Text,
            (None, Some(Content::Shape(_))) => PartKind::Shape,
            _ => return None,
        })
    }

    /// What it is, in a word.
    pub fn what(&self) -> &'static str {
        self.kind().map_or("", PartKind::word)
    }
}

/// What a studio_take's part is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    /// The screen, drawn with its look.
    Screen,
    Video,
    Sound,
    Picture,
    Text,
    Shape,
}

impl PartKind {
    pub fn word(self) -> &'static str {
        match self {
            PartKind::Screen => "Screen",
            PartKind::Video => "Video",
            PartKind::Sound => "Sound",
            PartKind::Picture => "Picture",
            PartKind::Text => "Text",
            PartKind::Shape => "Shape",
        }
    }
}

impl Sequence {
    /// Places a part at `start` (made content lasting `len`): a medium as [`Sequence::place`]
    /// does, made content over the pictures; in its box when it has one. The clip's id.
    pub fn place_part(&mut self, part: Part, start: u64, len: u64, track: Option<usize>) -> Option<Id> {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        match (part.media, part.content) {
            (Some(m), _) => {
                let id = self.add_media(m);
                let c = self.place(id, start, track)?;
                if let Some(p) = part.place {
                    let tr = self.transform_for(self.media(id), None, p);
                    self.clip_mut(c)?.transform = tr;
                }
                Some(c)
            }
            (None, Some(mut content)) => {
                // its sizes from the canvas's pixels to the sequence's
                let k = part.place.map_or(1.0, |p| h / p.canvas.1.max(1) as f32);
                match &mut content {
                    Content::Text(t) => {
                        t.size *= k;
                        t.width *= k;
                    }
                    Content::Shape(sh) => {
                        sh.width *= k;
                        sh.height *= k;
                        sh.radius *= k;
                    }
                }
                let c = self.place_content_on(content, start, len.max(500), track);
                if let Some(p) = part.place {
                    let tr = Transform { x: (p.x + p.w / 2.0) * w - w / 2.0, y: (p.y + p.h / 2.0) * h - h / 2.0, scale: 1.0, rotation: p.rotation, opacity: p.opacity };
                    self.clip_mut(c)?.transform = tr;
                }
                Some(c)
            }
            _ => None,
        }
    }

    /// The transform that puts `clip`'s picture (a medium fitted to the frame, or made content
    /// `made` pixels big) in the box `p`.
    pub fn transform_for(&self, media: Option<&Media>, made: Option<(f32, f32)>, p: Placement) -> Transform {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let (bx, by, bw, bh) = (p.x * w, p.y * h, p.w * w, p.h * h);
        let base = match (made, media) {
            (Some(s), _) => s,
            (None, Some(m)) if m.width > 0 && m.height > 0 => {
                let (_, _, fw, fh) = crate::compose::fit(m.width as f32 / m.height as f32, 0.0, 0.0, w, h);
                (fw, fh)
            }
            _ => (w, h),
        };
        // fitted in its box, as the scene fits a picture in its layer
        let scale = (bw / base.0.max(1.0)).min(bh / base.1.max(1.0));
        Transform { x: bx + bw / 2.0 - w / 2.0, y: by + bh / 2.0 - h / 2.0, scale, rotation: p.rotation, opacity: p.opacity }
    }
}

/// The volume at a moment of a clip (from its start, timeline time).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VolumeKey {
    pub at_ms: u64,
    pub volume: f32,
}

/// Voice clean-up: the audio engine's Voice Clarity chain, run on the file once.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VoiceClean {
    /// 0 light .. 1 strong.
    pub amount: f32,
    /// Warmer and denser, for narration.
    #[serde(default)]
    pub podcast: bool,
}

impl Default for VoiceClean {
    fn default() -> Self {
        VoiceClean { amount: 0.5, podcast: false }
    }
}

/// Smooth in and out.
fn ease(k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    k * k * (3.0 - 2.0 * k)
}

fn mix_transform(a: &Transform, b: &Transform, k: f32) -> Transform {
    let l = |x: f32, y: f32| x + (y - x) * k;
    Transform { x: l(a.x, b.x), y: l(a.y, b.y), scale: l(a.scale, b.scale), rotation: l(a.rotation, b.rotation), opacity: l(a.opacity, b.opacity) }
}

impl Clip {
    pub fn new(media: Id, start_ms: u64, len_ms: u64) -> Clip {
        Clip {
            id: Id::new(),
            media,
            start_ms,
            in_ms: 0,
            out_ms: len_ms,
            speed: 1.0,
            volume: 1.0,
            muted: false,
            transform: Transform::default(),
            content: None,
            transition: None,
            filter: Filter::default(),
            keyframes: Vec::new(),
            anim: Default::default(),
            fade_in_ms: 0,
            fade_out_ms: 0,
            volume_keys: Vec::new(),
            voice: None,
            duck: false,
            hidden: Vec::new(),
            effects: Vec::new(),
            crop: (0.0, 0.0, 0.0, 0.0),
            flip_h: false,
            flip_v: false,
            stream: None,
            link: None,
            motion: Default::default(),
        }
    }

    /// Its volume at timeline time `t` (its volume keys, straight between, or its volume).
    pub fn volume_at(&self, t: u64) -> f32 {
        let at = t.saturating_sub(self.start_ms);
        let k = &self.volume_keys;
        match (k.first(), k.last()) {
            (None, _) | (_, None) => self.volume,
            (Some(f), _) if at <= f.at_ms => f.volume,
            (_, Some(l)) if at >= l.at_ms => l.volume,
            _ => {
                let i = k.partition_point(|f| f.at_ms <= at);
                let (a, b) = (&k[i - 1], &k[i]);
                let x = (at - a.at_ms) as f32 / (b.at_ms - a.at_ms).max(1) as f32;
                a.volume + (b.volume - a.volume) * x
            }
        }
    }

    /// Sets its volume at `t`: a key there when it has keys, else the volume.
    pub fn set_volume_at(&mut self, t: u64, v: f32) {
        if self.volume_keys.is_empty() {
            self.volume = v;
            return;
        }
        let at = t.saturating_sub(self.start_ms).min(self.len_ms());
        match self.volume_keys.iter_mut().find(|k| k.at_ms.abs_diff(at) < 40) {
            Some(k) => k.volume = v,
            None => {
                self.volume_keys.push(VolumeKey { at_ms: at, volume: v });
                self.volume_keys.sort_by_key(|k| k.at_ms);
            }
        }
    }

    /// How far faded in and out it is at `t` (0..1): its picture's opacity and its sound's
    /// level both follow it.
    pub fn fade_at(&self, t: u64) -> f32 {
        let at = t.saturating_sub(self.start_ms) as f32;
        let left = self.end_ms().saturating_sub(t) as f32;
        let fin = if self.fade_in_ms > 0 { (at / self.fade_in_ms as f32).min(1.0) } else { 1.0 };
        let fout = if self.fade_out_ms > 0 { (left / self.fade_out_ms as f32).min(1.0) } else { 1.0 };
        fin * fout
    }

    /// How loud it is at `t`: its volume, faded in and out (0 when muted).
    pub fn gain_at(&self, t: u64) -> f32 {
        if self.muted {
            return 0.0;
        }
        self.volume_at(t) * self.fade_at(t)
    }

    /// Whether the volume ever changes along the clip (keys, fades).
    pub fn sound_shaped(&self) -> bool {
        !self.volume_keys.is_empty() || self.fade_in_ms > 0 || self.fade_out_ms > 0
    }

    /// Its transform at timeline time `t` (its keyframes eased between, or its one transform).
    pub fn transform_at(&self, t: u64) -> Transform {
        let mut tr = self.transform_of_keyframes(t);
        // keys per property over that
        for (i, p) in crate::keys::TRANSFORM.iter().enumerate() {
            if let Some(v) = crate::keys::value(self.keys_of(p), t.saturating_sub(self.start_ms)) {
                crate::keys::set_transform_field(&mut tr, i, v);
            }
        }
        tr
    }

    fn transform_of_keyframes(&self, t: u64) -> Transform {
        let at = t.saturating_sub(self.start_ms);
        let k = &self.keyframes;
        match (k.first(), k.last()) {
            (None, _) | (_, None) => self.transform,
            (Some(f), _) if at <= f.at_ms => f.transform,
            (_, Some(l)) if at >= l.at_ms => l.transform,
            _ => {
                let i = k.partition_point(|f| f.at_ms <= at);
                let (a, b) = (&k[i - 1], &k[i]);
                let span = (b.at_ms - a.at_ms).max(1) as f32;
                mix_transform(&a.transform, &b.transform, ease((at - a.at_ms) as f32 / span))
            }
        }
    }

    /// Sets its transform at `t`: a keyframe there when it has keyframes, else the transform.
    pub fn set_transform_at(&mut self, t: u64, tr: Transform) {
        if self.keyframes.is_empty() {
            self.transform = tr;
            return;
        }
        let at = t.saturating_sub(self.start_ms).min(self.len_ms());
        match self.keyframes.iter_mut().find(|k| k.at_ms.abs_diff(at) < 40) {
            Some(k) => k.transform = tr,
            None => {
                self.keyframes.push(Keyframe { at_ms: at, transform: tr });
                self.keyframes.sort_by_key(|k| k.at_ms);
            }
        }
    }

    /// The medium's time at `t`, held at the clip's first or last frame outside it (for
    /// transitions, which show a clip a little beyond its edge).
    pub fn source_held(&self, t: u64) -> u64 {
        let t = t.clamp(self.start_ms, self.end_ms().saturating_sub(1).max(self.start_ms));
        self.source_at(t).unwrap_or(self.in_ms)
    }

    /// Its length on the timeline.
    pub fn len_ms(&self) -> u64 {
        ((self.out_ms.saturating_sub(self.in_ms)) as f64 / self.speed.max(0.05) as f64).round() as u64
    }

    pub fn end_ms(&self) -> u64 {
        self.start_ms + self.len_ms()
    }

    /// The medium's time at timeline time `t` (`None` outside the clip).
    pub fn source_at(&self, t: u64) -> Option<u64> {
        (t >= self.start_ms && t < self.end_ms())
            .then(|| (self.in_ms + ((t - self.start_ms) as f64 * self.speed.max(0.05) as f64) as u64).min(self.out_ms.saturating_sub(1)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: Id,
    pub kind: TrackKind,
    /// Clips in time order, never overlapping.
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub locked: bool,
    /// The track captions were made on (made again in place).
    #[serde(default)]
    pub captions: bool,
    /// Its height on the timeline (points), when it's been changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
}

impl Track {
    /// An empty track of `kind`.
    pub fn empty(kind: TrackKind) -> Track {
        Track::new(kind)
    }

    fn new(kind: TrackKind) -> Track {
        Track { id: Id::new(), kind, clips: Vec::new(), muted: false, hidden: false, locked: false, captions: false, height: None }
    }

    /// Whether `start..end` is free on this track (ignoring `except`).
    pub fn free(&self, start: u64, end: u64, except: Option<Id>) -> bool {
        self.clips.iter().filter(|c| Some(c.id) != except).all(|c| end <= c.start_ms || start >= c.end_ms())
    }

    pub fn sort(&mut self) {
        self.clips.sort_by_key(|c| c.start_ms);
    }
}

/// The sequence. Tracks are in display order: video tracks first, the top one in front; then
/// audio tracks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    pub version: u32,
    pub size: (u32, u32),
    /// Frames a second (23.976, 29.97… too; projects from before wrote a whole number).
    pub fps: f64,
    pub media: Vec<Media>,
    pub tracks: Vec<Track>,
    /// Large videos edited through smaller copies (the export reads the originals).
    #[serde(default = "yes")]
    pub proxies: bool,
    /// Markers on the timeline (ms, in order).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<u64>,
    /// How it was last exported (the export panel opens with it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<crate::exporter::ExportSettings>,
}

fn yes() -> bool {
    true
}

impl Sequence {
    /// The fonts its text clips name, each face once (what a device must have, or download,
    /// before it draws the project; docs/plans/fonts.md §6).
    pub fn fonts_used(&self) -> Vec<lightair_fonts::FontRef> {
        let mut out: Vec<lightair_fonts::FontRef> = Vec::new();
        for clip in self.tracks.iter().flat_map(|t| &t.clips) {
            if let Some(Content::Text(style)) = &clip.content
                && let Some(font) = &style.font
                && !out.iter().any(|f| f.same_face(font) && f.weight == font.weight)
            {
                out.push(font.clone());
            }
        }
        out
    }
}

impl Media {
    /// Whether it's worth editing through a proxy: a video larger than 1080p (a studio_take's: its
    /// screen recording).
    pub fn wants_proxy(&self) -> bool {
        self.kind == MediaKind::Video && (self.width > 1920 || self.height > 1080)
    }

    /// Where its proxy is (or goes): named by the file, its size and time.
    pub fn proxy_path(&self) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.path.hash(&mut h);
        if let Ok(m) = std::fs::metadata(&self.path) {
            m.len().hash(&mut h);
            m.modified().ok().hash(&mut h);
        }
        crate::sound::cache().join(format!("proxy-{:016x}.mp4", h.finish()))
    }

    /// Its proxy, when it has one made.
    pub fn proxy(&self) -> Option<PathBuf> {
        self.wants_proxy().then(|| self.proxy_path()).filter(|p| p.is_file())
    }
}

impl Sequence {
    /// The media used on the timeline that want a proxy and have none yet.
    pub fn proxies_missing(&self) -> Vec<Media> {
        if !self.proxies {
            return Vec::new();
        }
        let used: Vec<Id> = self.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.media)).collect();
        self.media.iter().filter(|m| used.contains(&m.id) && m.wants_proxy() && m.proxy().is_none()).cloned().collect()
    }

    /// Empty: one video track and one audio track.
    pub fn new(size: (u32, u32), fps: u32) -> Sequence {
        Sequence { version: 1, size, fps: fps as f64, media: Vec::new(), tracks: vec![Track::new(TrackKind::Video), Track::new(TrackKind::Audio)], proxies: true, markers: Vec::new(), export: None }
    }

    pub fn load(path: &Path) -> std::io::Result<Sequence> {
        let mut s: Sequence = serde_json::from_slice(&std::fs::read(path)?).map_err(std::io::Error::other)?;
        s.name_effect_keys();
        Ok(s)
    }

    /// Effect keys from before effects had ids ("fx.<place>.<setting>"), named for their
    /// effects ("fx.<id>.<setting>").
    pub fn name_effect_keys(&mut self) {
        for c in self.tracks.iter_mut().flat_map(|t| t.clips.iter_mut()) {
            let old: Vec<String> = c.anim.keys().filter(|p| p.starts_with("fx.")).cloned().collect();
            for p in old {
                let Some((n, setting)) = p["fx.".len()..].split_once('.') else { continue };
                let Some(e) = n.parse::<usize>().ok().and_then(|n| c.effects.get(n)) else { continue };
                let named = format!("fx.{}.{setting}", e.id);
                if let Some(keys) = c.anim.remove(&p) {
                    c.anim.insert(named, keys);
                }
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn duration_ms(&self) -> u64 {
        self.tracks.iter().flat_map(|t| t.clips.iter().map(Clip::end_ms)).max().unwrap_or(0)
    }

    pub fn media(&self, id: Id) -> Option<&Media> {
        self.media.iter().find(|m| m.id == id)
    }

    /// Adds a medium (once per file); its id.
    pub fn add_media(&mut self, m: Media) -> Id {
        if let Some(e) = self.media.iter().find(|e| e.path == m.path) {
            return e.id;
        }
        let id = m.id;
        self.media.push(m);
        id
    }

    /// (track, clip) indices of a clip.
    pub fn find(&self, clip: Id) -> Option<(usize, usize)> {
        self.tracks.iter().enumerate().find_map(|(t, tr)| tr.clips.iter().position(|c| c.id == clip).map(|c| (t, c)))
    }

    pub fn clip(&self, id: Id) -> Option<&Clip> {
        self.find(id).map(|(t, c)| &self.tracks[t].clips[c])
    }

    pub fn clip_mut(&mut self, id: Id) -> Option<&mut Clip> {
        self.find(id).map(|(t, c)| &mut self.tracks[t].clips[c])
    }

    fn kind_for(m: &Media) -> TrackKind {
        if m.kind == MediaKind::Audio { TrackKind::Audio } else { TrackKind::Video }
    }

    /// A track of `kind` free over `start..end`, the preferred one first; a new one if none is
    /// (video tracks go above the others, audio tracks below).
    fn free_track(&mut self, kind: TrackKind, preferred: Option<usize>, start: u64, end: u64) -> usize {
        if let Some(p) = preferred.filter(|p| self.tracks.get(*p).is_some_and(|t| t.kind == kind && t.free(start, end, None))) {
            return p;
        }
        // the bottom-most free video track (the main one), the top-most free audio track
        let found = match kind {
            TrackKind::Video => self.tracks.iter().rposition(|t| t.kind == kind && t.free(start, end, None)),
            TrackKind::Audio => self.tracks.iter().position(|t| t.kind == kind && t.free(start, end, None)),
        };
        if let Some(i) = found {
            return i;
        }
        match kind {
            TrackKind::Video => {
                self.tracks.insert(0, Track::new(TrackKind::Video));
                0
            }
            TrackKind::Audio => {
                self.tracks.push(Track::new(TrackKind::Audio));
                self.tracks.len() - 1
            }
        }
    }

    /// Places all of a medium at `start` (on `track` if it's free there); the new clip's id.
    pub fn place(&mut self, media: Id, start: u64, track: Option<usize>) -> Option<Id> {
        let m = self.media(media)?.clone();
        let len = if m.kind == MediaKind::Image { 5_000 } else { m.duration_ms.max(1) };
        let clip = Clip::new(media, start, len);
        let t = self.free_track(Self::kind_for(&m), track, start, start + len);
        let id = clip.id;
        self.tracks[t].clips.push(clip);
        self.tracks[t].sort();
        Some(id)
    }

    /// Places generated content (text, a shape) at `start` for `len` on the top free video track
    /// (a new one above if none is free); its id.
    pub fn place_content(&mut self, content: Content, start: u64, len: u64) -> Id {
        self.place_content_on(content, start, len, None)
    }

    /// A made picture placed on `track` when that's a video track free then, else as
    /// [`Sequence::place_content`] does.
    pub fn place_content_on(&mut self, content: Content, start: u64, len: u64, track: Option<usize>) -> Id {
        let mut clip = Clip::new(Id::nil(), start, len);
        clip.content = Some(content);
        let wanted = track.filter(|t| self.tracks.get(*t).is_some_and(|tr| tr.kind == TrackKind::Video && !tr.captions && tr.free(start, start + len, None)));
        // over the pictures: the top free video track
        let t = match wanted.or_else(|| self.tracks.iter().position(|t| t.kind == TrackKind::Video && t.free(start, start + len, None))) {
            Some(i) if wanted == Some(i) => i,
            Some(i) if !self.tracks[i].captions && self.tracks[i].clips.iter().all(|c| c.content.is_some()) => i,
            _ => {
                self.tracks.insert(0, Track::new(TrackKind::Video));
                0
            }
        };
        let id = clip.id;
        self.tracks[t].clips.push(clip);
        self.tracks[t].sort();
        id
    }

    /// The main track: the bottom video track (where the magnet keeps clips end to end).
    pub fn main_track(&self) -> Option<usize> {
        self.tracks.iter().rposition(|t| t.kind == TrackKind::Video)
    }

    /// Track `t`'s clips end to end from the start, in their order (the magnet).
    pub fn pack(&mut self, t: usize) {
        let Some(track) = self.tracks.get_mut(t) else { return };
        track.sort();
        let mut at = 0;
        for c in &mut track.clips {
            c.start_ms = at;
            at = c.end_ms();
        }
    }

    /// Cuts away what's before `at` of the clips under it (`only`: those clips; else every
    /// unlocked one). Their ids.
    pub fn delete_left(&mut self, at: u64, only: &[Id]) -> Vec<Id> {
        let under = self.under(at, only);
        for c in &under {
            self.trim_start(*c, at);
        }
        under
    }

    /// Cuts away what's after `at` of the clips under it.
    pub fn delete_right(&mut self, at: u64, only: &[Id]) -> Vec<Id> {
        let under = self.under(at, only);
        for c in &under {
            self.trim_end(*c, at);
        }
        under
    }

    fn under(&self, at: u64, only: &[Id]) -> Vec<Id> {
        self.tracks
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.start_ms < at && at < c.end_ms() && (only.is_empty() || only.contains(&c.id)))
            .map(|c| c.id)
            .collect()
    }

    /// The clips linked with `clip` (itself included).
    pub fn linked(&self, clip: Id) -> Vec<Id> {
        let Some(link) = self.clip(clip).and_then(|c| c.link) else { return vec![clip] };
        self.tracks.iter().flat_map(|t| t.clips.iter()).filter(|c| c.link == Some(link)).map(|c| c.id).collect()
    }

    /// A marker at `at`, or the one there taken away.
    pub fn toggle_marker(&mut self, at: u64, near: u64) {
        match self.markers.iter().position(|m| m.abs_diff(at) <= near) {
            Some(i) => {
                self.markers.remove(i);
            }
            None => {
                self.markers.push(at);
                self.markers.sort();
            }
        }
    }

    /// Takes `ranges` (the clip's source time) out of a clip: what's left of it stays in order
    /// from its start, and the clips after it on its track move up to close the gaps. The
    /// timeline time taken out.
    pub fn cut_out(&mut self, clip: Id, ranges: &[(u64, u64)]) -> u64 {
        let mut removed = 0;
        // from the last range back: what's still to cut stays in the first piece, `clip`
        for &(a, b) in ranges.iter().rev() {
            let Some((t, _)) = self.find(clip) else { break };
            let piece = self.tracks[t].clips.iter().find(|c| c.id == clip).cloned().expect("found");
            let (a, b) = (a.max(piece.in_ms), b.min(piece.out_ms));
            if b <= a {
                continue;
            }
            let speed = piece.speed.max(0.05) as f64;
            let at = |s: u64| piece.start_ms + ((s - piece.in_ms) as f64 / speed).round() as u64;
            let (ta, tb) = (at(a), at(b).min(piece.end_ms()));
            // split out the middle and drop it
            let middle = if ta > piece.start_ms { self.split(clip, ta) } else { Some(clip) };
            let Some(middle) = middle else { continue };
            if tb < piece.end_ms() {
                self.split(middle, tb);
            }
            self.remove(middle);
            // what follows moves up
            for c in self.tracks[t].clips.iter_mut().filter(|c| c.start_ms >= tb) {
                c.start_ms -= tb - ta;
            }
            removed += tb - ta;
            if middle == clip {
                break;
            }
        }
        removed
    }

    /// A video clip's sound as clips of their own, one per audio track of its file, each on an
    /// audio track of its own, linked (the video clip muted). Their ids.
    pub fn detach_sounds(&mut self, clip: Id) -> Vec<Id> {
        let tracks = self.clip(clip).and_then(|c| self.media(c.media)).map_or(0, |m| if m.audio.iter().any(|f| *f != m.path) { 0 } else { m.tracks.len() });
        if tracks < 2 {
            return self.detach_sound(clip).into_iter().collect();
        }
        let mut made = Vec::new();
        for n in 0..tracks as u32 {
            // the video stays as it was until the last one is apart
            if let Some(c) = self.clip_mut(clip) {
                c.muted = false;
            }
            if let Some(id) = self.detach_sound(clip) {
                if let Some(c) = self.clip_mut(id) {
                    c.stream = Some(n);
                }
                made.push(id);
            }
        }
        made
    }

    /// A video clip's sound as a clip of its own on an audio track (the video clip muted). The
    /// new clip's id; `None` for clips without sound.
    pub fn detach_sound(&mut self, clip: Id) -> Option<Id> {
        let (t, c) = self.find(clip)?;
        let cl = self.tracks[t].clips[c].clone();
        let m = self.media(cl.media)?;
        if self.tracks[t].kind != TrackKind::Video || cl.content.is_some() || m.audio.is_empty() || cl.muted {
            return None;
        }
        let at = self.free_track(TrackKind::Audio, None, cl.start_ms, cl.end_ms());
        let sound = Clip {
            id: Id::new(),
            transform: Transform::default(),
            transition: None,
            filter: Filter::default(),
            keyframes: Vec::new(),
            anim: Default::default(),
            hidden: Vec::new(),
            effects: Vec::new(),
            crop: (0.0, 0.0, 0.0, 0.0),
            flip_h: false,
            flip_v: false,
            link: Some(cl.link.unwrap_or(cl.id)),
            motion: Default::default(),
            ..cl
        };
        let id = sound.id;
        self.tracks[at].clips.push(sound);
        self.tracks[at].sort();
        if let Some(v) = self.clip_mut(clip) {
            // its fades stay: they're its picture's too; it and its sound go together
            v.link = Some(v.link.unwrap_or(clip));
            v.muted = true;
            v.volume_keys.clear();
            v.voice = None;
        }
        Some(id)
    }

    /// Captions: a text clip for each line (start, end, text) in `style` at `place`, on the captions track
    /// (made at the top, or emptied first). The track's index.
    pub fn set_captions(&mut self, lines: &[(u64, u64, String)], style: &TextStyle, place: Transform) -> usize {
        let t = match self.tracks.iter().position(|t| t.captions) {
            Some(t) => t,
            None => {
                let mut track = Track::new(TrackKind::Video);
                track.captions = true;
                self.tracks.insert(0, track);
                0
            }
        };
        let mut clips: Vec<Clip> = Vec::new();
        for (a, b, text) in lines {
            // never over the line before; at least 0.4 s on screen
            let a = (*a).max(clips.last().map_or(0, Clip::end_ms));
            let b = (*b).max(a + 400);
            let mut c = Clip::new(Id::nil(), a, b - a);
            c.content = Some(Content::Text(TextStyle { text: text.clone(), ..style.clone() }));
            c.transform = place;
            clips.push(c);
        }
        // the gap before the next line closed, so they don't flicker
        for i in 1..clips.len() {
            let (start, end) = (clips[i].start_ms, clips[i - 1].end_ms());
            if start > end && start - end < 300 {
                clips[i - 1].out_ms += start - end;
            }
        }
        self.tracks[t].clips = clips;
        t
    }

    /// The caption style: white words in a dark box, low in the frame.
    pub fn caption_style(&self) -> (TextStyle, Transform) {
        let h = self.size.1 as f32;
        let style = TextStyle {
            text: String::new(),
            size: (h / 18.0).max(18.0),
            color: 0xFFFFFF,
            bold: true,
            align: 1,
            background: Some((0x000000, 0.6)),
            outline: None,
            width: self.size.0 as f32 * 0.8,
            font: None,
        };
        (style, Transform { y: h * 0.38, ..Transform::default() })
    }

    /// The clip just before `clip` on its track, if it ends where `clip` starts.
    pub fn before(&self, clip: Id) -> Option<&Clip> {
        let (t, c) = self.find(clip)?;
        let me = &self.tracks[t].clips[c];
        c.checked_sub(1).map(|p| &self.tracks[t].clips[p]).filter(|p| p.end_ms() == me.start_ms)
    }

    /// Places a medium at the end of the main track (the bottom video track, or the first audio
    /// track for sound).
    pub fn append(&mut self, media: Id) -> Option<Id> {
        let kind = Self::kind_for(self.media(media)?);
        let t = match kind {
            TrackKind::Video => self.tracks.iter().rposition(|t| t.kind == kind),
            TrackKind::Audio => self.tracks.iter().position(|t| t.kind == kind),
        };
        let end = t.and_then(|t| self.tracks[t].clips.last().map(Clip::end_ms)).unwrap_or(0);
        self.place(media, end, t)
    }

    /// Moves a clip to `start` on `track` (same kind). False (and nothing moves) if it would
    /// overlap another clip there.
    pub fn move_clip(&mut self, clip: Id, track: usize, start: u64) -> bool {
        let Some((t, c)) = self.find(clip) else { return false };
        let Some(dest) = self.tracks.get(track) else { return false };
        let len = self.tracks[t].clips[c].len_ms();
        if dest.kind != self.tracks[t].kind || dest.locked || !dest.free(start, start + len, Some(clip)) {
            return false;
        }
        let mut moved = self.tracks[t].clips.remove(c);
        moved.start_ms = start;
        self.tracks[track].clips.push(moved);
        self.tracks[track].sort();
        true
    }

    /// Moves a clip's left edge to timeline time `at` (its in point follows), within its medium
    /// and its neighbours.
    pub fn trim_start(&mut self, clip: Id, at: u64) {
        let Some((t, c)) = self.find(clip) else { return };
        let prev_end = if c > 0 { self.tracks[t].clips[c - 1].end_ms() } else { 0 };
        let cl = &mut self.tracks[t].clips[c];
        let speed = cl.speed.max(0.05) as f64;
        let end = cl.end_ms();
        let was = cl.start_ms;
        if cl.content.is_some() {
            // made pictures have no medium to run out of
            let at = at.clamp(prev_end, end.saturating_sub(100));
            cl.start_ms = at;
            cl.in_ms = 0;
            cl.out_ms = ((end - at) as f64 * speed).round() as u64;
        } else {
            // the earliest the edge can go: the medium's start, the clip before
            let earliest = cl.start_ms.saturating_sub((cl.in_ms as f64 / speed) as u64).max(prev_end);
            let at = at.clamp(earliest, end.saturating_sub(100));
            let in_ms = cl.in_ms as i64 + ((at as i64 - cl.start_ms as i64) as f64 * speed) as i64;
            cl.in_ms = in_ms.max(0) as u64;
            cl.start_ms = at;
        }
        // keyframes stay where they were on the timeline
        let moved = cl.start_ms as i64 - was as i64;
        for k in &mut cl.keyframes {
            k.at_ms = (k.at_ms as i64 - moved).max(0) as u64;
        }
        for k in &mut cl.volume_keys {
            k.at_ms = (k.at_ms as i64 - moved).max(0) as u64;
        }
        cl.shift_keys(-moved);
    }

    /// Moves a clip's right edge to timeline time `at` (its out point follows), within its medium
    /// (images stretch freely) and the clip after.
    pub fn trim_end(&mut self, clip: Id, at: u64) {
        let Some((t, c)) = self.find(clip) else { return };
        let next_start = self.tracks[t].clips.get(c + 1).map_or(u64::MAX, |n| n.start_ms);
        let media = self.tracks[t].clips[c].media;
        let (limit, image) = self.media(media).map_or((u64::MAX, false), |m| (m.duration_ms, m.kind == MediaKind::Image));
        let cl = &mut self.tracks[t].clips[c];
        let speed = cl.speed.max(0.05) as f64;
        let at = at.clamp(cl.start_ms + 100, next_start);
        let out = cl.in_ms + ((at - cl.start_ms) as f64 * speed) as u64;
        cl.out_ms = if image { out } else { out.min(limit) };
    }

    /// Cuts a clip at timeline time `at`: the second half becomes a clip of its own (its id).
    pub fn split(&mut self, clip: Id, at: u64) -> Option<Id> {
        let (t, c) = self.find(clip)?;
        let cl = self.tracks[t].clips[c].clone();
        let s = cl.source_at(at).filter(|_| at > cl.start_ms)?;
        // keyframes on either side of the cut, with the transform at the cut on both
        let cut = at - cl.start_ms;
        let here = cl.transform_at(at);
        let (mut before, mut after) = (Vec::new(), Vec::new());
        if !cl.keyframes.is_empty() {
            before = cl.keyframes.iter().filter(|k| k.at_ms < cut).copied().collect();
            before.push(Keyframe { at_ms: cut, transform: here });
            after.push(Keyframe { at_ms: 0, transform: here });
            after.extend(cl.keyframes.iter().filter(|k| k.at_ms > cut).map(|k| Keyframe { at_ms: k.at_ms - cut, transform: k.transform }));
        }
        let loud = cl.volume_at(at);
        let (mut vbefore, mut vafter) = (Vec::new(), Vec::new());
        if !cl.volume_keys.is_empty() {
            vbefore = cl.volume_keys.iter().filter(|k| k.at_ms < cut).copied().collect();
            vbefore.push(VolumeKey { at_ms: cut, volume: loud });
            vafter.push(VolumeKey { at_ms: 0, volume: loud });
            vafter.extend(cl.volume_keys.iter().filter(|k| k.at_ms > cut).map(|k| VolumeKey { at_ms: k.at_ms - cut, volume: k.volume }));
        }
        let (abefore, aafter) = cl.split_keys(cut);
        let first = &mut self.tracks[t].clips[c];
        first.out_ms = s;
        first.keyframes = before;
        first.volume_keys = vbefore;
        first.anim = abefore;
        // a fade (and an animation in or out) belongs to the end it was on
        first.fade_out_ms = 0;
        first.motion.exit = None;
        // a transition belongs to the cut it was on: the first half's start
        let second = Clip { id: Id::new(), start_ms: at, in_ms: s, keyframes: after, volume_keys: vafter, anim: aafter, transition: None, fade_in_ms: 0, motion: crate::motion::Motions { enter: None, ..cl.motion }, ..cl };
        let id = second.id;
        self.tracks[t].clips.insert(c + 1, second);
        Some(id)
    }

    /// Cuts every unlocked clip under the playhead (CapCut's split with nothing selected).
    pub fn split_all(&mut self, at: u64) -> Vec<Id> {
        let under: Vec<Id> = self
            .tracks
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter().filter(|c| c.start_ms < at && at < c.end_ms()).map(|c| c.id))
            .collect();
        under.into_iter().filter_map(|c| self.split(c, at)).collect()
    }

    /// Sets a clip's speed (its length follows). False if it would then run into the next clip.
    pub fn set_speed(&mut self, clip: Id, speed: f32) -> bool {
        let Some((t, c)) = self.find(clip) else { return false };
        let mut changed = self.tracks[t].clips[c].clone();
        changed.speed = speed.clamp(0.1, 16.0);
        let next = self.tracks[t].clips.get(c + 1).map_or(u64::MAX, |n| n.start_ms);
        if changed.end_ms() > next {
            return false;
        }
        self.tracks[t].clips[c] = changed;
        true
    }

    pub fn remove(&mut self, clip: Id) {
        if let Some((t, c)) = self.find(clip) {
            self.tracks[t].clips.remove(c);
        }
    }

    /// Removes empty tracks beyond one of each kind.
    pub fn tidy(&mut self) {
        for kind in [TrackKind::Video, TrackKind::Audio] {
            while self.tracks.iter().filter(|t| t.kind == kind).count() > 1 {
                let Some(i) = self.tracks.iter().position(|t| t.kind == kind && t.clips.is_empty()) else { break };
                self.tracks.remove(i);
            }
        }
    }

    /// Times things snap to: every clip's edges, and the start.
    pub fn snap_points(&self, except: Option<Id>) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter(|c| Some(c.id) != except).flat_map(|c| [c.start_ms, c.end_ms()]))
            .collect();
        v.push(0);
        v.sort();
        v.dedup();
        v
    }

    /// The video clips showing at `t`, back to front, with their track.
    pub fn video_at(&self, t: u64) -> Vec<(usize, &Clip)> {
        let mut out: Vec<(usize, &Clip)> = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, tr)| tr.kind == TrackKind::Video && !tr.hidden)
            .filter_map(|(i, tr)| tr.clips.iter().find(|c| c.start_ms <= t && t < c.end_ms()).map(|c| (i, c)))
            .collect();
        // the top track (index 0) is in front: drawn last
        out.reverse();
        out
    }
}

/// An Editor project (docs/plans/video-editor.md): a named sequence, one file in the workspace's
/// `edits` folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub created_ms: i64,
    pub modified_ms: i64,
    pub sequence: Sequence,
}

/// A project as the list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectInfo {
    pub id: Id,
    pub name: String,
    pub modified_ms: i64,
    pub duration_ms: u64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl Project {
    pub fn new(name: &str, size: (u32, u32), fps: u32) -> Project {
        let now = now_ms();
        Project { id: Id::new(), name: name.into(), created_ms: now, modified_ms: now, sequence: Sequence::new(size, fps) }
    }

    pub fn dir(workspace: &Path) -> PathBuf {
        workspace.join("edits")
    }

    fn path(workspace: &Path, id: Id) -> PathBuf {
        Self::dir(workspace).join(format!("{id}.json"))
    }

    /// The workspace's projects, the latest changed first.
    pub fn list(workspace: &Path) -> Vec<ProjectInfo> {
        let mut out: Vec<ProjectInfo> = std::fs::read_dir(Self::dir(workspace))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice::<Project>(&std::fs::read(e.path()).ok()?).ok())
            .map(|p| ProjectInfo { id: p.id, name: p.name.clone(), modified_ms: p.modified_ms, duration_ms: p.sequence.duration_ms() })
            .collect();
        out.sort_by_key(|p| std::cmp::Reverse(p.modified_ms));
        out
    }

    pub fn load(workspace: &Path, id: Id) -> std::io::Result<Project> {
        let mut p: Project = serde_json::from_slice(&std::fs::read(Self::path(workspace, id))?).map_err(std::io::Error::other)?;
        p.sequence.name_effect_keys();
        Ok(p)
    }

    /// Saves it (whole, through a temporary file), marked changed now.
    pub fn save(&mut self, workspace: &Path) -> std::io::Result<()> {
        self.modified_ms = now_ms();
        let path = Self::path(workspace, self.id);
        std::fs::create_dir_all(Self::dir(workspace))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }

    /// Removes the project (its file; the media it uses stay).
    pub fn delete(workspace: &Path, id: Id) -> std::io::Result<()> {
        std::fs::remove_file(Self::path(workspace, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_are_files_in_the_workspace() {
        let ws = tempfile::tempdir().unwrap();
        let mut a = Project::new("Intro", (1920, 1080), 30);
        a.save(ws.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut b = Project::new("Tutorial", (1080, 1920), 60);
        b.save(ws.path()).unwrap();
        let list = Project::list(ws.path());
        assert_eq!(list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Tutorial", "Intro"], "latest first");
        assert_eq!(Project::load(ws.path(), a.id).unwrap().name, "Intro");
        Project::delete(ws.path(), a.id).unwrap();
        assert_eq!(Project::list(ws.path()).len(), 1);
    }

    fn video(seq: &mut Sequence, name: &str, ms: u64) -> Id {
        seq.add_media(Media {
            id: Id::new(),
            name: name.into(),
            kind: MediaKind::Video,
            path: PathBuf::from(format!("/m/{name}.mp4")),
            duration_ms: ms,
            width: 1920,
            height: 1080,
            audio: vec![PathBuf::from(format!("/m/{name}.mp4"))],
            studio_take: None,
            item: None,
            look: None,
            transcript: None,
            tracks: Vec::new(),
            zooms: None,
        })
    }

    #[test]
    fn clips_go_end_to_end_and_overlaps_go_up_a_track() {
        let mut s = Sequence::new((1920, 1080), 30);
        let a = video(&mut s, "a", 10_000);
        let b = video(&mut s, "b", 4_000);
        let ca = s.append(a).unwrap();
        let cb = s.append(b).unwrap();
        assert_eq!(s.clip(cb).unwrap().start_ms, 10_000, "after the first");
        assert_eq!(s.duration_ms(), 14_000);
        // placed over the first clip: a new track above it
        let over = s.place(b, 2_000, None).unwrap();
        let (t_over, _) = s.find(over).unwrap();
        let (t_main, _) = s.find(ca).unwrap();
        assert!(t_over < t_main, "above (in front)");
        let shown: Vec<Id> = s.video_at(3_000).iter().map(|(_, c)| c.id).collect();
        assert_eq!(shown, vec![ca, over], "back to front");
        // a move onto an occupied place is refused
        assert!(!s.move_clip(over, t_main, 1_000));
        assert!(s.move_clip(over, t_main, 14_000));
        s.tidy();
        assert_eq!(s.tracks.iter().filter(|t| t.kind == TrackKind::Video).count(), 1);
    }

    #[test]
    fn trims_and_splits_keep_the_medium_in_step() {
        let mut s = Sequence::new((1920, 1080), 30);
        let a = video(&mut s, "a", 10_000);
        let c = s.append(a).unwrap();
        s.trim_start(c, 2_000);
        let cl = s.clip(c).unwrap().clone();
        assert_eq!((cl.start_ms, cl.in_ms, cl.out_ms), (2_000, 2_000, 10_000));
        s.trim_end(c, 50_000);
        assert_eq!(s.clip(c).unwrap().out_ms, 10_000, "not past the medium's end");
        s.clip_mut(c).unwrap().speed = 2.0;
        assert_eq!(s.clip(c).unwrap().len_ms(), 4_000);
        let second = s.split(c, 4_000).unwrap();
        let (first, second) = (s.clip(c).unwrap().clone(), s.clip(second).unwrap().clone());
        assert_eq!((first.in_ms, first.out_ms), (2_000, 6_000));
        assert_eq!((second.start_ms, second.in_ms, second.out_ms), (4_000, 6_000, 10_000));
        assert_eq!(second.source_at(5_000), Some(8_000));
        assert!(s.snap_points(None).contains(&4_000));
    }

    #[test]
    fn titles_keyframes_split_and_trim() {
        let mut s = Sequence::new((1920, 1080), 30);
        let t = s.place_content(Content::Text(TextStyle::title("Hi")), 1_000, 4_000);
        assert_eq!(s.tracks[0].kind, TrackKind::Video);
        let c = s.clip_mut(t).unwrap();
        c.keyframes = vec![
            Keyframe { at_ms: 0, transform: Transform { x: 0.0, ..Transform::default() } },
            Keyframe { at_ms: 4_000, transform: Transform { x: 400.0, ..Transform::default() } },
        ];
        assert_eq!(c.transform_at(3_000).x, 200.0, "halfway, eased");
        assert!(c.transform_at(1_500).x < 50.0, "slow at first");
        let second = s.split(t, 3_000).unwrap();
        assert_eq!(s.clip(t).unwrap().transform_at(3_000).x, 200.0);
        assert_eq!(s.clip(second).unwrap().transform_at(3_000).x, 200.0);
        assert_eq!(s.clip(second).unwrap().transform_at(5_000).x, 400.0);
        // a made picture stretches either way; its keyframes stay put on the timeline
        s.trim_start(t, 0);
        let first = s.clip(t).unwrap();
        assert_eq!((first.start_ms, first.end_ms()), (0, 3_000));
        assert_eq!(first.transform_at(1_000).x, 0.0);
        assert_eq!(first.transform_at(3_000).x, 200.0);
        s.trim_end(second, 9_000);
        assert_eq!(s.clip(second).unwrap().end_ms(), 9_000);
    }

    #[test]
    fn cutting_words_out_closes_up_the_track() {
        let mut s = Sequence::new((1920, 1080), 30);
        let a = video(&mut s, "a", 10_000);
        let b = video(&mut s, "b", 4_000);
        let first = s.append(a).unwrap();
        let next = s.append(b).unwrap();
        // the clip starts 1 s into its medium: cut 2–3 s and 6–8 s of the medium
        s.trim_start(first, 1_000);
        s.move_clip(first, 0, 0);
        let removed = s.cut_out(first, &[(2_000, 3_000), (6_000, 8_000)]);
        assert_eq!(removed, 3_000);
        let pieces: Vec<(u64, u64, u64)> = s.tracks[0].clips.iter().map(|c| (c.start_ms, c.in_ms, c.out_ms)).collect();
        assert_eq!(pieces[..3], [(0, 1_000, 2_000), (1_000, 3_000, 6_000), (4_000, 8_000, 10_000)]);
        assert_eq!(s.clip(next).unwrap().start_ms, 7_000, "the next clip moved up by what was cut");
        // fillers and long pauses
        let w = |a: u64, b: u64, t: &str| crate::speech::Word { start_ms: a, end_ms: b, text: t.into() };
        let t = crate::speech::Transcript { language: "en".into(), words: vec![w(0, 400, "So"), w(400, 700, "um"), w(700, 1_000, "this"), w(4_000, 4_400, "works")] };
        assert_eq!(crate::speech::tightening(&t, 0, 5_000, 1_000, 300), vec![(400, 700), (1_150, 3_850)]);
    }

    #[test]
    fn delete_left_right_magnet_markers_and_links() {
        let mut s = Sequence::new((1920, 1080), 30);
        let a = video(&mut s, "a", 4_000);
        let b = video(&mut s, "b", 4_000);
        let first = s.append(a).unwrap();
        let second = s.append(b).unwrap();
        // the first's start cut at 1 s: the magnet closes the gap
        s.delete_left(1_000, &[first]);
        assert_eq!(s.clip(first).unwrap().start_ms, 1_000);
        let main = s.main_track().unwrap();
        s.pack(main);
        assert_eq!((s.clip(first).unwrap().start_ms, s.clip(second).unwrap().start_ms), (0, 3_000));
        // the second's end cut at 5 s (2 s into it)
        s.delete_right(5_000, &[]);
        assert_eq!(s.clip(second).unwrap().end_ms(), 5_000);
        s.toggle_marker(2_000, 50);
        s.toggle_marker(500, 50);
        assert_eq!(s.markers, vec![500, 2_000]);
        s.toggle_marker(2_020, 50);
        assert_eq!(s.markers, vec![500], "one near is taken away");
        // a detached sound goes with its video
        let sound = s.detach_sound(second).unwrap();
        let mut both = s.linked(second);
        both.sort();
        let mut want = vec![second, sound];
        want.sort();
        assert_eq!(both, want);
        assert_eq!(s.linked(first), vec![first]);
    }
}
