//! The Editor (docs/plans/video-editor.md): a multi-track video editor on the workspace's
//! projects. Media on the left (the workspace's Library items and files imported into the
//! project), the player in the middle, the inspector on the right, the timeline below.
//!
//! The work is `lightair_render`'s: frames for the player and playback, and the export, on threads
//! of their own. The project is saved as it changes; undo and redo step through its versions.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input as TextInput, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::Id;
use lightair_render::edit::Edit;
use lightair_render::seqrender::SequenceRenderer;
use lightair_render::sequence::{Project, Media, MediaKind, Sequence, Part, TrackKind, Transform};
use lightair_render::zoom;
use lightair_store::recordings::{File as ItemFile, Index, Item, Kind};

use crate::studio::Studio;
use crate::ui::{self, tok};

mod audio;
mod effects;
mod export;
pub(crate) mod kit;
mod ghost;
mod grids;
mod looks;
mod menus;
mod speaker;
mod speed;
mod select;
mod stage;
mod tools;
mod voiceover;
mod words;
mod zooms;
use lightair_render::sequence::Content;
use looks::{heading, hint, label};

/// What the background work sends back.
enum Done {
    /// A frame for the player (made into an image off the window's thread).
    Frame(u64, Option<Picture>, Vec<lightair_render::seqrender::Placed>),
    Progress(f32),
    Exported(Result<PathBuf, String>),
    /// Media made from Library items or files (probing takes a moment), and where they go (on
    /// a track, if given).
    Media(Vec<Part>, Put, Option<usize>),
    /// Caption lines heard in the project's sound.
    Captions(Result<Vec<(u64, u64, String)>, String>),
    Downloaded(Result<(), String>),
    /// Playback reached the end.
    Ended,
    /// A file's waveform.
    Peaks((PathBuf, Option<u32>), Option<Peaks>),
    /// Voices cleaned up.
    Cleaned(Result<(), String>),
    /// What's inside a take (its Library item's id).
    StudioTakeParts(Id, Vec<Part>),
    /// A frame frozen (the clip, where, the still), a clip reversed (the file played backwards).
    Frozen(Id, u64, Result<Media, String>),
    Reversed(Id, Result<Media, String>),
    /// A medium's words.
    Transcript(Id, Result<lightair_render::speech::Transcript, String>),
    /// A proxy being made, and made (its original file).
    ProxyProgress(f32),
    /// A video's filmstrip.
    Strip(PathBuf, Option<Arc<speed::Strip>>),
    Proxied(PathBuf, Result<(), String>),
    /// The looks' thumbnails made.
    Thumbs(Result<(), String>),
}

/// Where media being added go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Put {
    /// At a time on the timeline.
    At(u64),
    /// At the main track's end.
    End,
    /// Only kept in the project (in the media panel), not on the timeline.
    Keep,
}

impl Put {
    /// At `t`, or the main track's end.
    fn at(t: Option<u64>) -> Put {
        t.map_or(Put::End, Put::At)
    }
}

/// A medium being dragged from the media panel.
#[derive(Clone)]
pub struct DraggedMedia {
    label: SharedString,
    source: MediaSource,
    /// Its picture (a thumbnail), or an icon (a symbol name, or an emoji to show as it is).
    thumb: Option<PathBuf>,
    icon: SharedString,
    /// Over the timeline, where its ghost shows instead.
    hidden: std::rc::Rc<std::cell::Cell<bool>>,
}

impl DraggedMedia {
    fn new(label: impl Into<SharedString>, source: MediaSource, thumb: Option<PathBuf>, icon: impl Into<SharedString>, hidden: &std::rc::Rc<std::cell::Cell<bool>>) -> Self {
        DraggedMedia { label: label.into(), source, thumb, icon: icon.into(), hidden: hidden.clone() }
    }
}

impl Render for DraggedMedia {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.hidden.get() {
            return div().into_any_element();
        }
        let icon = self.icon.clone();
        let symbol = icon.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        div()
            .flex()
            .items_center()
            .gap_2()
            .p_1p5()
            .pr_3()
            .max_w(px(280.))
            .rounded(px(8.))
            .bg(tok::bg_raised())
            .border_2()
            .border_color(tok::accent_primary())
            .shadow_lg()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(tok::text())
            .child(
                div()
                    .flex_none()
                    .w(px(64.))
                    .h(px(38.))
                    .rounded(px(5.))
                    .overflow_hidden()
                    .bg(tok::bg_canvas_well())
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| match &self.thumb {
                        Some(p) => d.child(img(p.clone()).size_full().object_fit(ObjectFit::Cover)),
                        None if symbol => d.child(ui::icon(&icon).size(px(18.)).text_color(tok::text_secondary())),
                        None => d.text_size(px(24.)).child(icon),
                    }),
            )
            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(self.label.clone()))
            .into_any_element()
    }
}

/// Where a medium comes from.
#[derive(Clone)]
enum MediaSource {
    /// A Library item of the workspace.
    Item(Item),
    /// Already in the project (imported).
    Media(Id),
    /// Text or a shape (where it goes in the frame).
    Content(Content, Transform),
    /// One part of a take (where it was in the studio_take's scene).
    Part(Box<Part>),
}

/// A drag on the timeline.
#[derive(Clone, Copy)]
enum Drag {
    Move { clip: Id, grab_ms: i64 },
    TrimStart(Id),
    TrimEnd(Id),
    Scrub,
    /// A fade's handle, dragged along the clip.
    FadeIn(Id),
    FadeOut(Id),
    /// The volume line, dragged up and down (from a window y, at a volume).
    Volume { clip: Id, y: f32, from: f32 },
    /// A track's bottom edge: its height (from a window y, at a height).
    TrackHeight { track: usize, y: f32, from: f32 },
    /// A zoom block on the zoom track (its studio_take, its place in the studio_take's zooms, the clip it's
    /// over).
    Zoom { media: Id, index: usize, clip: Id, part: zooms::ZoomPart },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InspectorTab {
    /// A made picture's words or shape.
    Text,
    Video,
    Audio,
    Speed,
    Filter,
    Effects,
    Transition,
    Words,
    Screen,
    Animation,
}

/// What the left panel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftTab {
    Media,
    Audio,
    Text,
    Shapes,
    Captions,
    Effects,
    Transitions,
    Filters,
}

/// Pixels per second at zoom 1.
const PPS: f32 = 40.0;
const HEADER_W: f32 = 112.0;
const RULER_H: f32 = 26.0;
const VIDEO_H: f32 = 58.0;
const AUDIO_H: f32 = 44.0;

/// A sound's waveform (shared with the threads that read it).
type Peaks = Arc<Vec<f32>>;

/// A track's height on the timeline.
fn track_h(t: &lightair_render::sequence::Track) -> f32 {
    t.height.unwrap_or(if t.kind == TrackKind::Video { VIDEO_H } else { AUDIO_H })
}
const EDGE: f32 = 8.0;
/// Where a fade's knob sits when there's no fade: clear of the clip's edge, which trims.
const FADE_KNOB: f32 = 16.0;

pub struct Editor {
    studio: Entity<Studio>,
    /// The window's layer under the UI (Windows: the player's frames are presented there).
    pub native: crate::native_preview::Shared,
    workspace: Option<PathBuf>,
    project: Option<Project>,
    /// The workspace's Library, as the media panel lists it (read when it changes).
    library: Vec<Item>,
    library_seen: Option<u64>,
    undo: Vec<Sequence>,
    redo: Vec<Sequence>,
    /// The sequence when a drag began (its undo step).
    drag_before: Option<Sequence>,
    renderer: Arc<Mutex<Option<SequenceRenderer>>>,
    /// The sequence as last changed, for the renderer to studio_take before its next frame (it may be
    /// busy, or still being made, when the change happens).
    pending: Arc<Mutex<Pending>>,
    /// The frame being shown, and the one before it (freed from the GPU when the next comes).
    retired: Option<Picture>,
    /// The player's size in pixels (measured as it's drawn) and the preview size asked for.
    player_px: std::rc::Rc<std::cell::Cell<(f32, f32)>>,
    preview_box: Option<(u32, u32)>,
    /// The player's place in the window; the pictures in the frame shown; a drag on it; the
    /// centre lines pulling (x, y); drawing a box to hide.
    player_bounds: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
    placed: Vec<lightair_render::seqrender::Placed>,
    stage_drag: Option<stage::StageDrag>,
    guides: (bool, bool),
    hiding: bool,
    /// A medium being transcribed; words marked to cut (clip, source start, end).
    transcribing: Option<Id>,
    marked: Vec<(Id, u64, u64)>,
    /// A voice-over being recorded.
    recorder: Option<voiceover::Recorder>,
    /// The clips picked (with `selected`, the inspector's), and a box being dragged to pick.
    picked: Vec<Id>,
    marquee: Option<select::Marquee>,
    /// The timeline's tool; its switches (snapping, linkage, the
    /// preview axis) and the moment under the pointer; clips copied; the zoom slider.
    tool: tools::Tool,
    snapping: bool,
    linkage: bool,
    /// The clip being dragged was taken with ⌥: its linked clips stay put.
    drag_alone: bool,
    /// The zoom picked on the zoom track (its studio_take, its place in the studio_take's zooms), and takes'
    /// zooms as their looks have them (read once).
    zoom_sel: Option<(Id, usize)>,
    zoom_files: std::cell::RefCell<std::collections::HashMap<Id, Vec<lightair_render::edit::Zoom>>>,
    axis: bool,
    hover: Option<u64>,
    clipboard: Vec<tools::Copied>,
    zoom_bar: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
    zoom_dragging: bool,
    /// A number field being dragged, and the sequence before (its one undo step).
    field_drag: Option<kit::FieldDrag>,
    field_before: Option<Sequence>,
    /// The studio_take opened in the media panel (its parts shown), and takes' parts read so far.
    opened_studio_take: Option<Item>,
    studio_take_parts: std::collections::HashMap<Id, Option<Vec<Part>>>,
    /// Sound while scrubbing: on, and the snippet playing (when it began).
    scrub_audio: bool,
    scrub: Option<(speaker::Speaker, std::time::Instant)>,
    /// Where the last scrub sound was taken from.
    scrub_at: Option<u64>,
    /// Where something dragged over the timeline would land; and the dragged card hidden
    /// meanwhile (it's on the timeline as the ghost).
    ghost: Option<ghost::Ghost>,
    drag_hidden: std::rc::Rc<std::cell::Cell<bool>>,
    preview: Option<Picture>,
    wanted: Option<u64>,
    busy: bool,
    playing: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// The player's sound, while it plays.
    speaker: Option<speaker::Speaker>,
    exporting: Option<f32>,
    /// The export panel, when open; and the text fields of it and of the project's settings.
    export_panel: Option<export::ExportPanel>,
    fields: export::Fields,
    note: Option<SharedString>,
    playhead: u64,
    selected: Option<Id>,
    /// Timeline zoom (× PPS) and scroll (ms at the left edge).
    zoom: f32,
    scroll_ms: f32,
    drag: Option<Drag>,
    timeline_bounds: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
    /// Where the timeline was right-pressed (for its menu's Paste here).
    menu_at: Option<u64>,
    tab: InspectorTab,
    left: LeftTab,
    /// The looks' thumbnails; the Animation tab's slot (in, out, combo).
    thumbs: grids::Thumbs,
    anim_slot: lightair_render::thumbs::Slot,
    /// The selected text clip's words, being edited, and the sequence before typing began.
    text_edit: Option<(Id, Entity<InputState>)>,
    text_before: Option<Sequence>,
    captioning: bool,
    downloading: bool,
    /// Waveforms by file (`None`: being read).
    peaks: std::collections::HashMap<(PathBuf, Option<u32>), Option<Peaks>>,
    cleaning: bool,
    /// The proxy being made (its medium's name, how far), and files whose proxy failed.
    proxying: Option<(SharedString, f32)>,
    proxy_failed: Vec<PathBuf>,
    /// Filmstrips by file (`None`: being read).
    strips: std::collections::HashMap<PathBuf, Option<Arc<speed::Strip>>>,
    /// Frames shown while playing, and when the first was (for measuring).
    shown: u64,
    shown_since: Option<std::time::Instant>,
    naming: Option<(Option<Id>, Entity<InputState>)>,
    watching: bool,
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
    _subscriptions: Vec<Subscription>,
}

/// What the renderer is to studio_take before its next frame.
#[derive(Default)]
struct Pending {
    /// The sequence's last change, if it hasn't been taken.
    seq: Option<Sequence>,
    /// The most the player shows (always set: the player's frames are BGRA previews).
    preview: Option<(u32, u32)>,
}

/// The renderer brought up to the sequence's last change and the player's size.
fn catch_up(r: &mut SequenceRenderer, pending: &Mutex<Pending>) {
    let mut p = pending.lock().unwrap();
    if let Some(seq) = p.seq.take() {
        let _ = r.set(&seq);
    }
    r.set_preview(p.preview);
}

/// Preview sizes, smallest first: the player gets the first that covers it.
const PREVIEW_BOXES: [(u32, u32); 5] = [(640, 360), (960, 540), (1280, 720), (1920, 1080), (2560, 1440)];

/// A frame the player shows: an image, or (macOS) the IOSurface the renderer drew into, or
/// (Windows) a frame of this size presented under the UI (the window's player visual).
#[derive(Clone)]
enum Picture {
    Image(Arc<RenderImage>),
    #[cfg(target_os = "macos")]
    Surface(lightair_render::surface::Frame),
    #[cfg_attr(not(windows), allow(dead_code))]
    Presented((u32, u32)),
}

impl Picture {
    fn of(shown: lightair_render::seqrender::Shown, size: (u32, u32)) -> Option<Picture> {
        match shown {
            lightair_render::seqrender::Shown::Pixels(p) => image_of(p, size).map(Picture::Image),
            #[cfg(target_os = "macos")]
            lightair_render::seqrender::Shown::Surface(f) => Some(Picture::Surface(f)),
            #[cfg(windows)]
            lightair_render::seqrender::Shown::Presented => Some(Picture::Presented(size)),
        }
    }

    fn element(self) -> AnyElement {
        match self {
            Picture::Image(i) => img(ImageSource::Render(i)).absolute().size_full().object_fit(ObjectFit::Contain).into_any_element(),
            #[cfg(target_os = "macos")]
            Picture::Surface(f) => surface(f.0).absolute().size_full().object_fit(ObjectFit::Contain).into_any_element(),
            // drawn under the UI: the player is left clear over it
            Picture::Presented(_) => div().into_any_element(),
        }
    }
}

/// A preview frame (BGRA, as the renderer makes them for the player) as an image.
fn image_of(bgra: Vec<u8>, size: (u32, u32)) -> Option<Arc<RenderImage>> {
    if bgra.is_empty() {
        return None;
    }
    let buffer = image::RgbaImage::from_raw(size.0, size.1, bgra)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}.{}", s / 3600, (s / 60) % 60, s % 60, (ms % 1000) / 100)
}

fn short(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

/// The media a Library item gives (probing its files): one per video or sound.
/// A studio_take item's folder.
fn studio_take_folder(item: &Item) -> Option<PathBuf> {
    item.files.iter().find_map(|f| f.path.parent().filter(|p| p.extension().is_some_and(|e| e == "lightair-take")).map(Path::to_path_buf))
}

/// A Library item as what goes on the timeline: a studio_take as its layers (the screen with its
/// look, the scene's other layers where they were, each recorded mixer track's sound).
fn media_of(item: &Item) -> Vec<Part> {
    if item.kind == Kind::StudioTake {
        let mut parts = studio_take_folder(item).map(|f| Media::studio_take_layout(&f)).unwrap_or_default();
        for p in parts.iter_mut() {
            if let Some(m) = &mut p.media {
                m.name = if m.studio_take.is_some() { item.title.clone() } else { format!("{} · {}", item.title, p.name) };
                m.item = Some(item.id);
            }
        }
        return parts;
    }
    let playable = |p: &Path| {
        p.exists()
            && p.extension().is_some_and(|e| {
                ["mp4", "mov", "mkv", "m4v", "wav", "m4a", "mp3", "flac", "aac", "png", "jpg", "jpeg", "gif"].iter().any(|x| e.eq_ignore_ascii_case(x))
            })
    };
    let files: Vec<&ItemFile> = item.files.iter().filter(|f| playable(&f.path)).collect();
    let sound_only = |p: &Path| p.extension().is_some_and(|e| ["wav", "m4a", "mp3", "flac", "aac"].iter().any(|x| e.eq_ignore_ascii_case(x)));
    // a recording of several canvases: the first picture; a multitrack set: every file
    let picture = files.iter().find(|f| !sound_only(&f.path)).copied();
    let pick: Vec<&ItemFile> = match (item.kind, picture) {
        (Kind::Multitrack, _) | (_, None) => files.clone(),
        (_, Some(p)) => vec![p],
    };
    // a recording's mixer tracks recorded beside it (multitrack): with it, each on its own
    let track_files: Vec<&ItemFile> = if item.kind != Kind::Multitrack && picture.is_some() { files.iter().filter(|f| sound_only(&f.path)).copied().collect() } else { Vec::new() };
    let mut out: Vec<Part> = pick
        .into_iter()
        .filter_map(|f| {
            let mut m = Media::from_file(&f.path).ok()?;
            m.item = Some(item.id);
            if item.kind != Kind::Multitrack {
                m.name = item.title.clone();
            }
            Some(Part::of(m))
        })
        .collect();
    // whose sound each is: the recording's multitrack sidecar says
    let sidecar: serde_json::Value = item
        .files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(".multitrack.json"))
        .and_then(|f| std::fs::read(&f.path).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let named = |path: &Path| -> Option<String> {
        let file = path.file_name()?.to_string_lossy().into_owned();
        sidecar["tracks"].as_array()?.iter().find(|t| t["file"].as_str() == Some(file.as_str()))?["name"].as_str().map(String::from)
    };
    for f in track_files {
        if let Ok(mut m) = Media::from_file(&f.path) {
            // (recordings from before the sidecar said: only the file's name, "… - Mic.wav")
            let track_file = named(&f.path).unwrap_or_else(|| m.name.rsplit(" - ").next().unwrap_or(&m.name).to_string());
            m.name = format!("{} · {track_file}", item.title);
            m.item = Some(item.id);
            out.push(Part::of(m));
        }
    }
    if out.len() > 1 && out.iter().skip(1).all(|p| p.media.as_ref().is_some_and(|m| m.kind == MediaKind::Audio)) {
        // the mixer tracks are apart: the picture's own sound (their mix) kept, but quiet
        out[0].mute_own_sound = true;
    }
    out
}

fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Recording => "video",
        Kind::StudioTake => "mouse-pointer-2",
        Kind::Clip => "scissors",
        Kind::Multitrack => "audio-lines",
        Kind::Export => "film",
    }
}

impl Editor {
    pub fn new(studio: Entity<Studio>) -> Self {
        let (tx, rx) = mpsc::channel();
        // cleaned voices are kept with the app's data
        lightair_render::sound::set_cache(lightair_platform::paths::data_dir().join("cache").join("sound"));
        Editor {
            studio,
            native: Default::default(),
            workspace: None,
            project: None,
            library: Vec::new(),
            library_seen: None,
            undo: Vec::new(),
            redo: Vec::new(),
            drag_before: None,
            renderer: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(Pending { seq: None, preview: Some(PREVIEW_BOXES[2]) })),
            retired: None,
            player_px: Default::default(),
            preview_box: Some(PREVIEW_BOXES[2]),
            player_bounds: Default::default(),
            placed: Vec::new(),
            stage_drag: None,
            guides: (false, false),
            hiding: false,
            transcribing: None,
            marked: Vec::new(),
            recorder: None,
            picked: Vec::new(),
            marquee: None,
            tool: tools::Tool::Select,
            snapping: true,
            linkage: true,
            drag_alone: false,
            zoom_sel: None,
            zoom_files: Default::default(),
            axis: false,
            hover: None,
            clipboard: Vec::new(),
            zoom_bar: Default::default(),
            zoom_dragging: false,
            field_drag: None,
            field_before: None,
            opened_studio_take: None,
            studio_take_parts: Default::default(),
            scrub_audio: true,
            scrub: None,
            scrub_at: None,
            ghost: None,
            drag_hidden: Default::default(),
            preview: None,
            wanted: None,
            busy: false,
            playing: None,
            speaker: None,
            exporting: None,
            export_panel: None,
            fields: Default::default(),
            note: None,
            playhead: 0,
            selected: None,
            zoom: 1.0,
            scroll_ms: 0.0,
            drag: None,
            timeline_bounds: Default::default(),
            menu_at: None,
            tab: InspectorTab::Video,
            left: LeftTab::Media,
            thumbs: grids::Thumbs::new(),
            anim_slot: lightair_render::thumbs::Slot::In,
            text_edit: None,
            text_before: None,
            captioning: false,
            downloading: false,
            peaks: Default::default(),
            cleaning: false,
            proxying: None,
            proxy_failed: Vec::new(),
            strips: Default::default(),
            shown: 0,
            shown_since: None,
            naming: None,
            watching: false,
            tx,
            rx,
            _subscriptions: Vec::new(),
        }
    }

    /// Shown: the workspace's last project opens (the workspace may have changed since).
    pub fn show(&mut self, cx: &mut Context<Self>) {
        let ws = self.studio.read(cx).workspace_dir();
        if ws != self.workspace {
            self.stop(cx);
            self.workspace = ws.clone();
            self.project = None;
            self.preview = None;
            self.library_seen = None;
            *self.renderer.lock().unwrap() = None;
            if let Some(ws) = &ws
                && let Some(last) = Project::list(ws).first()
            {
                self.open_project(last.id, cx);
            }
        }
        self.watch(cx);
        self.want_thumbs();
        cx.notify();
    }

    /// A Library item, added at the end of the open project (a new one if none is open).
    pub fn add_item(&mut self, id: Id, cx: &mut Context<Self>) {
        self.show(cx);
        let Some(ws) = &self.workspace else { return };
        if let Some(item) = Index::load(ws).get(id).cloned() {
            self.add(MediaSource::Item(item), None, None, cx);
        }
    }

    /// Opens a studio_take in the media panel: its parts (read on a thread the first time).
    fn open_studio_take(&mut self, item: Item, cx: &mut Context<Self>) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.studio_take_parts.entry(item.id) {
            e.insert(None);
            let (tx, it) = (self.tx.clone(), item.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Done::StudioTakeParts(it.id, media_of(&it)));
            });
        }
        self.opened_studio_take = Some(item);
        cx.notify();
    }

    /// Parts of a take (from the Library), at the end of the open project (a new one if none
    /// is open), each where it was in the studio_take's scene.
    pub fn add_parts(&mut self, parts: Vec<Part>, cx: &mut Context<Self>) {
        self.show(cx);
        if self.project.is_none() {
            self.new_project("Untitled", cx);
        }
        self.take_media(parts, Put::End, None, cx);
    }

    /// Takes what the threads send.
    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.watching {
            return;
        }
        self.watching = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(30)).await;
                let alive = this.update(cx, |this, cx| {
                    let mut changed = false;
                    while let Ok(done) = this.rx.try_recv() {
                        changed = true;
                        match done {
                            Done::Frame(t, image, placed) => {
                                this.busy = false;
                                if this.playing.is_none() {
                                    this.placed = placed;
                                }
                                if this.playing.is_some() {
                                    this.playhead = t;
                                }
                                if this.playing.is_some() {
                                    this.shown += 1;
                                    this.shown_since.get_or_insert_with(std::time::Instant::now);
                                }
                                if let Some(image) = image {
                                    // the frame before last leaves the GPU's atlas (the last one
                                    // may still be on screen until the next draw)
                                    let last = this.preview.replace(image);
                                    if let Some(Picture::Image(gone)) = std::mem::replace(&mut this.retired, last) {
                                        cx.drop_image(gone, None);
                                    }
                                }
                            }
                            Done::Progress(p) => this.exporting = Some(p),
                            Done::Exported(r) => {
                                this.exporting = None;
                                this.export_ended(r);
                                this.studio.update(cx, |s, _| s.library_version += 1);
                            }
                            Done::Media(media, put, track) => this.take_media(media, put, track, cx),
                            Done::Captions(r) => this.take_captions(r, cx),
                            Done::Peaks(f, p) => {
                                this.peaks.insert(f, p);
                            }
                            Done::Cleaned(r) => this.take_cleaned(r, cx),
                            Done::Transcript(m, r) => this.take_transcript(m, r, cx),
                            Done::Frozen(c, t, r) => this.take_frozen(c, t, r, cx),
                            Done::Reversed(c, r) => this.take_reversed(c, r, cx),
                            Done::StudioTakeParts(id, parts) => {
                                this.studio_take_parts.insert(id, Some(parts));
                            }
                            Done::ProxyProgress(f) => {
                                if let Some(p) = &mut this.proxying {
                                    p.1 = f;
                                }
                            }
                            Done::Proxied(file, r) => this.take_proxy(file, r, cx),
                            Done::Thumbs(r) => this.take_thumbs(r, cx),
                            Done::Strip(f, s) => {
                                this.strips.insert(f, s);
                            }
                            Done::Ended => {
                                this.playing = None;
                                this.speaker = None;
                            }
                            Done::Downloaded(r) => {
                                this.downloading = false;
                                this.note = Some(match r {
                                    Ok(()) => "The speech model is ready: make captions now.".into(),
                                    Err(e) => format!("The speech model couldn't be downloaded: {e}").into(),
                                });
                            }
                        }
                    }
                    this.pump();
                    if changed {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // ------------------------------------------------------------ projects

    fn open_project(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(ws) = self.workspace.clone() else { return };
        let Ok(p) = Project::load(&ws, id) else {
            self.note = Some("That project couldn't be read.".into());
            return;
        };
        self.stop(cx);
        self.undo.clear();
        self.redo.clear();
        self.pick_only(None);
        self.playhead = 0;
        self.scroll_ms = 0.0;
        self.preview = None;
        let seq = p.sequence.clone();
        self.project = Some(p);
        self.pending.lock().unwrap().seq = None;
        let (r, tx, pending) = (self.renderer.clone(), self.tx.clone(), self.pending.clone());
        self.busy = true;
        #[cfg(windows)]
        let visual = self.native.borrow_mut().as_mut().map_or(0usize, |n| n.player_visual() as usize);
        std::thread::spawn(move || {
            let mut guard = r.lock().unwrap();
            let preview = pending.lock().unwrap().preview;
            *guard = SequenceRenderer::new_with(&seq, preview).ok();
            let frame = guard.as_mut().map(|r| {
                // macOS: the player shows what the GPU drew, with no read back
                r.set_zero_copy(true);
                // Windows: presented into the player's visual under the UI
                #[cfg(windows)]
                if visual != 0 {
                    r.set_visual(Some(visual as *mut std::ffi::c_void));
                }
                catch_up(r, &pending);
                let shown = r.shown(0);
                (Picture::of(shown, r.out_size()), r.placed().to_vec())
            });
            drop(guard);
            let (image, placed) = frame.unwrap_or_default();
            let _ = tx.send(Done::Frame(0, image, placed));
        });
        self.make_proxies(cx);
        cx.notify();
    }

    /// A new project (1080p, 30 fps) called `name`, opened.
    pub fn new_project(&mut self, name: &str, cx: &mut Context<Self>) -> Option<Id> {
        let ws = self.workspace.clone().or_else(|| self.studio.read(cx).workspace_dir())?;
        self.workspace = Some(ws.clone());
        let name = if name.trim().is_empty() { "Untitled".to_string() } else { name.trim().to_string() };
        let mut p = Project::new(&name, (1920, 1080), 30);
        p.save(&ws).ok()?;
        let id = p.id;
        self.open_project(id, cx);
        Some(id)
    }

    fn start_naming(&mut self, rename: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        let current = rename.and(self.project.as_ref().map(|p| p.name.clone())).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Project name"));
        input.update(cx, |s, cx| {
            s.set_value(current, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, _, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let name = input.read(cx).value().trim().to_string();
                this.naming = None;
                match rename {
                    Some(_) => {
                        if let (Some(ws), Some(p)) = (this.workspace.clone(), this.project.as_mut())
                            && !name.is_empty()
                        {
                            p.name = name;
                            let _ = p.save(&ws);
                        }
                    }
                    None => {
                        this.new_project(&name, cx);
                    }
                }
                cx.notify();
            }
            InputEvent::Blur => {
                this.naming = None;
                cx.notify();
            }
            _ => {}
        });
        self._subscriptions.push(sub);
        self.naming = Some((rename, input));
        cx.notify();
    }

    fn delete_project(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(p)) = (self.workspace.clone(), self.project.take()) else { return };
        self.stop(cx);
        let _ = Project::delete(&ws, p.id);
        *self.renderer.lock().unwrap() = None;
        self.preview = None;
        self.note = Some(format!("Deleted “{}” (its media stay in the Library)", p.name).into());
        if let Some(next) = Project::list(&ws).first() {
            self.open_project(next.id, cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------ changes

    fn seq(&self) -> Option<&Sequence> {
        self.project.as_ref().map(|p| &p.sequence)
    }

    /// A change to the sequence: saved, undoable, drawn again.
    fn change(&mut self, f: impl FnOnce(&mut Sequence), cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        let before = p.sequence.clone();
        f(&mut p.sequence);
        if p.sequence == before {
            return;
        }
        // a field being dragged is one undo step, when it ends
        if self.field_before.is_some() {
            self.settle(cx);
            return;
        }
        self.undo.push(before);
        self.redo.clear();
        self.settle(cx);
    }

    fn settle(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(p)) = (self.workspace.clone(), self.project.as_mut()) else { return };
        let _ = p.save(&ws);
        self.pending.lock().unwrap().seq = Some(p.sequence.clone());
        self.request(self.playhead);
        self.clean_voices(cx);
        self.make_proxies(cx);
        cx.notify();
    }

    pub fn undo(&mut self, back: bool, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        let (from, to) = if back { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(s) = from.pop() else { return };
        to.push(std::mem::replace(&mut p.sequence, s));
        self.settle(cx);
    }

    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if self.delete_zoom(cx) {
            return;
        }
        // with linkage, what's linked to it goes too
        let picked = self.picked_with_links();
        if picked.is_empty() {
            return;
        }
        self.pick_only(None);
        self.change(
            |s| {
                for c in picked {
                    s.remove(c);
                }
                s.tidy();
            },
            cx,
        );
    }

    fn split(&mut self, cx: &mut Context<Self>) {
        let t = self.playhead;
        let Some(seq) = self.seq() else { return };
        let under = |c: &Id| seq.clip(*c).is_some_and(|c| c.start_ms < t && t < c.end_ms());
        // what's picked under the playhead (else everything under it), linked clips along
        let mut targets: Vec<Id> = self.picked_clips().into_iter().filter(under).collect();
        if targets.is_empty() {
            targets = seq.tracks.iter().filter(|tr| !tr.locked).flat_map(|tr| tr.clips.iter()).filter(|c| c.start_ms < t && t < c.end_ms()).map(|c| c.id).collect();
        } else if self.linkage {
            let linked: Vec<Id> = targets.iter().flat_map(|c| seq.linked(*c)).filter(under).collect();
            for c in linked {
                if !targets.contains(&c) {
                    targets.push(c);
                }
            }
        }
        // each linked group split together: its right halves linked anew
        let mut groups: Vec<(Option<Id>, Vec<Id>)> = Vec::new();
        for c in targets {
            let link = seq.clip(c).and_then(|c| c.link);
            match groups.iter_mut().find(|(l, _)| l.is_some() && *l == link) {
                Some((_, g)) => g.push(c),
                None => groups.push((link, vec![c])),
            }
        }
        self.change(
            |s| {
                for (_, g) in &groups {
                    tools::split_together(s, g, t);
                }
            },
            cx,
        );
    }

    /// Adds a Library item or imported medium at `at` (on `track` if given), or the main track's end.
    fn add(&mut self, source: MediaSource, at: Option<u64>, track: Option<usize>, cx: &mut Context<Self>) {
        if self.project.is_none() {
            self.new_project("Untitled", cx);
        }
        match source {
            MediaSource::Media(id) => self.change(
                |s| {
                    let c = match at {
                        Some(t) => s.place(id, t, track),
                        None => s.append(id),
                    };
                    // a video's sound on an audio track of its own, linked (as Premiere has it)
                    if let Some(c) = c {
                        s.detach_sounds(c);
                    }
                },
                cx,
            ),
            MediaSource::Part(part) => self.take_media(vec![*part], Put::at(at), track, cx),
            MediaSource::Content(content, place) => {
                let at = at.unwrap_or(self.playhead);
                let mut made = None;
                self.change(
                    |s| {
                        let id = s.place_content_on(content, at, 3_000, track);
                        if let Some(c) = s.clip_mut(id) {
                            c.transform = place;
                        }
                        made = Some(id);
                    },
                    cx,
                );
                self.pick_only(made);
                self.tab = InspectorTab::Text;
                cx.notify();
            }
            MediaSource::Item(item) => {
                // reading the files takes a moment: off the window's thread
                let tx = self.tx.clone();
                self.note = Some(format!("Adding “{}”…", item.title).into());
                std::thread::spawn(move || {
                    let _ = tx.send(Done::Media(media_of(&item), Put::at(at), track));
                });
                cx.notify();
            }
        }
    }

    fn take_media(&mut self, parts: Vec<Part>, put: Put, track: Option<usize>, cx: &mut Context<Self>) {
        if parts.is_empty() {
            self.note = Some("Its files couldn't be read (moved? Locate them in the Library).".into());
            return;
        }
        self.note = None;
        let ws = self.workspace.clone();
        let at = match put {
            Put::At(t) => Some(t),
            Put::End | Put::Keep => None,
        };
        self.change(
            |s| {
                // made layers (text, colours) last as long as the longest part
                let len = parts.iter().filter_map(|p| p.media.as_ref()).map(|m| m.duration_ms).max().unwrap_or(5_000);
                // the parts of one thing (a studio_take's picture, layers and sounds) start together
                let mut start: Option<u64> = None;
                let mut pictures = 0;
                let mut placed_all: Vec<Id> = Vec::new();
                for mut part in parts {
                    // a studio_take's look lives in the workspace (the screen recording look)
                    if let Some(m) = &mut part.media
                        && m.studio_take.is_some()
                        && let (Some(ws), Some(item)) = (&ws, m.item)
                    {
                        m.look = Some(ws.join("recordings").join("edits").join(format!("{item}.json")));
                    }
                    let video = part.media.as_ref().is_some_and(|m| m.kind == MediaKind::Video);
                    let unplaced = part.place.is_none();
                    let quiet = part.mute_own_sound;
                    let placed = match (put, start) {
                        (Put::Keep, _) => {
                            if let Some(m) = part.media {
                                s.add_media(m);
                            }
                            None
                        }
                        (_, Some(t)) => s.place_part(part, t, len, None),
                        // the first: where it was dropped (or the main track's end)
                        (_, None) => match (part.media.take(), at) {
                            (Some(m), at) => {
                                let id = s.add_media(m);
                                let c = match at {
                                    Some(t) => s.place(id, t, track),
                                    None => s.append(id),
                                };
                                if let (Some(c), Some(p)) = (c, part.place) {
                                    let tr = s.transform_for(s.media(id), None, p);
                                    if let Some(cl) = s.clip_mut(c) {
                                        cl.transform = tr;
                                    }
                                }
                                c
                            }
                            (None, at) => s.place_part(part, at.unwrap_or(0), len, track),
                        },
                    };
                    let Some(c) = placed else { continue };
                    if start.is_none() {
                        start = s.clip(c).map(|c| c.start_ms);
                    }
                    placed_all.push(c);
                    // a video's sound on an audio track of its own, linked (as Premiere has it)
                    // (one per audio track of its file)
                    let sounds = s.detach_sounds(c);
                    if quiet {
                        for a in &sounds {
                            if let Some(a) = s.clip_mut(*a) {
                                a.muted = true;
                            }
                        }
                    }
                    placed_all.extend(sounds);
                    // a studio_take without a scene: its second picture (the camera beside the
                    // screen) small, bottom right
                    if video && unplaced {
                        pictures += 1;
                        if pictures > 1 {
                            let (w, h) = (s.size.0 as f32, s.size.1 as f32);
                            if let Some(cl) = s.clip_mut(c) {
                                cl.transform = Transform { x: w * 0.34, y: h * 0.31, scale: 0.25, ..Transform::default() };
                            }
                        }
                    }
                }
                // the parts of one thing go together (linkage)
                if placed_all.len() > 1 {
                    let link = Id::new();
                    for c in &placed_all {
                        if let Some(c) = s.clip_mut(*c) {
                            c.link = Some(link);
                        }
                    }
                }
            },
            cx,
        );
    }

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            self.new_project("Untitled", cx);
        }
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Import".into()) });
        let tx = self.tx.clone();
        cx.spawn_in(window, async move |_, _| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            std::thread::spawn(move || {
                let media: Vec<Part> = paths.iter().filter_map(|p| Media::from_file(p).ok()).map(Part::of).collect();
                let _ = tx.send(Done::Media(media, Put::Keep, None));
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ the player

    /// Frames at the size the player shows them (the smallest preview size that covers it).
    fn fit_preview(&mut self) {
        let (w, h) = self.player_px.get();
        if w < 2.0 {
            return;
        }
        // larger than them all: the sequence's own size (a box it fits in)
        let fits = PREVIEW_BOXES.iter().copied().find(|b| b.0 as f32 >= w && b.1 as f32 >= h).or(Some((7680, 4320)));
        if fits != self.preview_box {
            self.preview_box = fits;
            self.pending.lock().unwrap().preview = fits;
            if self.playing.is_none() {
                self.request(self.playhead);
            }
        }
    }

    fn request(&mut self, t: u64) {
        self.wanted = Some(t);
        self.pump();
    }

    fn pump(&mut self) {
        if self.busy || self.playing.is_some() {
            return;
        }
        let Some(t) = self.wanted.take() else { return };
        let (r, tx, pending) = (self.renderer.clone(), self.tx.clone(), self.pending.clone());
        self.busy = true;
        std::thread::spawn(move || {
            let mut guard = r.lock().unwrap();
            let frame = guard.as_mut().map(|r| {
                catch_up(r, &pending);
                let shown = r.shown(t);
                (Picture::of(shown, r.out_size()), r.placed().to_vec())
            });
            let (image, placed) = frame.unwrap_or_default();
            let _ = tx.send(Done::Frame(t, image, placed));
        });
    }

    pub fn play(&mut self, cx: &mut Context<Self>) {
        if self.recorder.is_some() {
            // playing while a voice-over records: it stops
            self.voice_over(cx);
            return;
        }
        if self.playing.is_some() {
            self.stop(cx);
            return;
        }
        self.play_with(true, cx);
    }

    /// Plays from the playhead, with its sound or without.
    fn play_with(&mut self, sound: bool, cx: &mut Context<Self>) {
        let end = self.seq().map_or(0, |s| s.duration_ms());
        if self.playhead + 100 >= end {
            self.playhead = 0;
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (r, tx, from, flag, pending) = (self.renderer.clone(), self.tx.clone(), self.playhead, stop.clone(), self.pending.clone());
        self.playing = Some(stop);
        self.speaker = if sound { self.seq().and_then(|s| speaker::Speaker::start(s, from)) } else { None };
        let clock = self.speaker.as_ref().map(|s| s.clock());
        let step = (1000.0 / self.seq().map_or(30.0, |s| s.fps.max(1.0))).round() as u64;
        // frames are made ahead on one thread and shown in time on another
        let (made, ready) = std::sync::mpsc::sync_channel::<(u64, Option<Picture>)>(4);
        let render_flag = flag.clone();
        let paced_clock = clock.clone();
        let paced_tx = tx.clone();
        std::thread::spawn(move || {
            let mut guard = r.lock().unwrap();
            let Some(r) = guard.as_mut() else { return };
            catch_up(r, &pending);
            let size = r.out_size();
            // Windows: frames presented as they're made go to the screen then, so each is made
            // when it's due (made ahead, they'd show early, ahead of the sound)
            if r.presents() {
                drop(made);
                let stopped = || render_flag.load(std::sync::atomic::Ordering::Relaxed);
                if let Some(c) = &paced_clock {
                    c.start();
                }
                let start = std::time::Instant::now();
                let now = || match paced_clock.as_ref().and_then(|c| c.heard_ms()) {
                    Some(ms) => ms,
                    None if paced_clock.is_some() && start.elapsed() < std::time::Duration::from_secs(1) => 0,
                    None => start.elapsed().as_millis() as u64,
                };
                r.play_paced(
                    from,
                    |t| {
                        let due = t.saturating_sub(from);
                        loop {
                            let n = now();
                            if stopped() {
                                return None;
                            }
                            if n >= due {
                                // more than two frames late: skipped
                                return Some(n <= due + 2 * step);
                            }
                            std::thread::sleep(std::time::Duration::from_millis((due - n).min(10)));
                        }
                    },
                    |t, shown| !stopped() && paced_tx.send(Done::Frame(t, Picture::of(shown, size), Vec::new())).is_ok(),
                );
                if !stopped() {
                    let _ = paced_tx.send(Done::Ended);
                }
                return;
            }
            r.play_shown(from, |t, shown| made.send((t, Picture::of(shown, size))).is_ok() && !render_flag.load(std::sync::atomic::Ordering::Relaxed));
        });
        std::thread::spawn(move || {
            let stopped = || flag.load(std::sync::atomic::Ordering::Relaxed);
            // the clock (and the sound) start with the first frame
            let Ok(first) = ready.recv() else { return };
            if let Some(c) = &clock {
                c.start();
            }
            let start = std::time::Instant::now();
            // in time with the sound when there is some, else the clock
            let now = || match clock.as_ref().and_then(|c| c.heard_ms()) {
                Some(ms) => ms,
                // the sound starting: a moment's wait for it
                None if clock.is_some() && start.elapsed() < std::time::Duration::from_secs(1) => 0,
                None => start.elapsed().as_millis() as u64,
            };
            let mut shown = std::time::Instant::now();
            for (t, image) in std::iter::once(first).chain(ready.iter()) {
                let due = t.saturating_sub(from);
                loop {
                    let n = now();
                    if n >= due || stopped() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis((due - n).min(10)));
                }
                if stopped() {
                    return;
                }
                // more than two frames late: skipped (unless nothing has been shown lately)
                if now() > due + 2 * step && shown.elapsed() < std::time::Duration::from_millis(100) {
                    continue;
                }
                shown = std::time::Instant::now();
                let _ = tx.send(Done::Frame(t, image, Vec::new()));
            }
            if !stopped() {
                let _ = tx.send(Done::Ended);
            }
        });
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.playing.take() {
            s.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.speaker = None;
        self.busy = false;
        cx.notify();
    }

    fn seek(&mut self, t: u64, cx: &mut Context<Self>) {
        self.stop(cx);
        self.playhead = t;
        self.request(t);
        cx.notify();
    }

    // ------------------------------------------------------------ the timeline's maths

    fn pps(&self) -> f32 {
        PPS * self.zoom
    }

    /// A window x as a time on the timeline.
    fn time_at(&self, x: Pixels) -> Option<u64> {
        let b = (*self.timeline_bounds.borrow())?;
        let px = f32::from(x - b.origin.x) - HEADER_W;
        Some(((px / self.pps() * 1000.0) + self.scroll_ms).max(0.0) as u64)
    }

    fn x_of(&self, ms: u64) -> f32 {
        (ms as f32 - self.scroll_ms) / 1000.0 * self.pps()
    }

    /// The track under a window y.
    fn track_at(&self, y: Pixels) -> Option<usize> {
        let b = (*self.timeline_bounds.borrow())?;
        let seq = self.seq()?;
        let mut top = f32::from(b.origin.y) + self.tracks_top();
        let y = f32::from(y);
        for (i, t) in seq.tracks.iter().enumerate() {
            let h = track_h(t);
            if y >= top && y < top + h {
                return Some(i);
            }
            top += h;
        }
        None
    }

    /// `t`, pulled to a snap point (clip edges, the playhead) within 8 pixels.
    fn snap(&self, t: i64, except: Option<Id>) -> u64 {
        let Some(seq) = self.seq().filter(|_| self.snapping) else { return t.max(0) as u64 };
        let mut points = seq.snap_points(except);
        points.push(self.playhead);
        let reach = 8.0 / self.pps() * 1000.0;
        points
            .into_iter()
            .map(|p| (p, (p as i64 - t).abs()))
            .filter(|(_, d)| (*d as f32) < reach)
            .min_by_key(|(_, d)| *d)
            .map_or(t.max(0) as u64, |(p, _)| p)
    }

    fn begin_drag(&mut self, d: Drag) {
        self.drag_before = self.project.as_ref().map(|p| p.sequence.clone());
        self.drag = Some(d);
    }

    fn drag_to(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else { return };
        let Some(t) = self.time_at(pos.x) else { return };
        match drag {
            Drag::Scrub => {
                self.playhead = t.min(self.seq().map_or(0, |s| s.duration_ms()));
                self.request(self.playhead);
                self.scrub_sound(self.playhead, cx);
            }
            Drag::Move { clip, grab_ms } => {
                let len = self.seq().and_then(|s| s.clip(clip)).map_or(0, |c| c.len_ms());
                let start = self.snap(t as i64 - grab_ms, Some(clip));
                // or its end snaps
                let by_end = self.snap(start as i64 + len as i64, Some(clip));
                let start = if by_end != start + len { by_end.saturating_sub(len) } else { start };
                let track = self.track_at(pos.y);
                // several picked: they all move by as much (in time)
                if self.move_picked(clip, start) {
                    cx.notify();
                    return;
                }
                if let Some(p) = &mut self.project {
                    let (cur, _) = p.sequence.find(clip).unwrap_or((0, 0));
                    let kind = p.sequence.tracks[cur].kind;
                    let to = track.filter(|t| p.sequence.tracks.get(*t).is_some_and(|tr| tr.kind == kind)).unwrap_or(cur);
                    if !p.sequence.move_clip(clip, to, start) {
                        p.sequence.move_clip(clip, cur, start);
                    }
                }
            }
            Drag::FadeIn(clip) | Drag::FadeOut(clip) => {
                let out = matches!(drag, Drag::FadeOut(_));
                if let Some(c) = self.project.as_mut().and_then(|p| p.sequence.clip_mut(clip)) {
                    let len = c.len_ms();
                    let v = if out { c.end_ms().saturating_sub(t) } else { t.saturating_sub(c.start_ms) };
                    let v = (v / 50 * 50).min(len / 2);
                    if out {
                        c.fade_out_ms = v;
                    } else {
                        c.fade_in_ms = v;
                    }
                }
                self.after_drag_change();
            }
            Drag::Volume { clip, y, from } => {
                // half the clip's height is a doubling
                let v = (from + (y - f32::from(pos.y)) / ((VIDEO_H.min(AUDIO_H) - 10.0) / 2.0)).clamp(0.0, 2.0);
                let v = (v * 20.0).round() / 20.0;
                if let Some(c) = self.project.as_mut().and_then(|p| p.sequence.clip_mut(clip)) {
                    // with volume keys, all of them move together
                    let d = v - c.volume_at(self.playhead.clamp(c.start_ms, c.end_ms().saturating_sub(1)));
                    if c.volume_keys.is_empty() {
                        c.volume = v;
                    } else {
                        c.volume_keys.iter_mut().for_each(|k| k.volume = (k.volume + d).clamp(0.0, 4.0));
                    }
                    self.note = Some(format!("Volume {:.0}%", v * 100.0).into());
                }
                self.after_drag_change();
            }
            Drag::Zoom { media, index, clip, part } => self.drag_zoom(media, index, clip, part, t),
            Drag::TrackHeight { track, y, from } => {
                let h = (from + f32::from(pos.y) - y).clamp(26.0, 220.0).round();
                if let Some(t) = self.project.as_mut().and_then(|p| p.sequence.tracks.get_mut(track)) {
                    t.height = Some(h);
                }
            }
            Drag::TrimStart(clip) | Drag::TrimEnd(clip) => {
                let start = matches!(drag, Drag::TrimStart(_));
                let t = self.snap(t as i64, Some(clip));
                let (linkage, alone) = (self.linkage, self.drag_alone);
                if let Some(p) = &mut self.project {
                    // linked clips with the same edge (a video and its sound) trim with it
                    let edge = |c: &lightair_render::sequence::Clip| if start { c.start_ms } else { c.end_ms() };
                    let mine = p.sequence.clip(clip).map(edge);
                    let mut all = vec![clip];
                    if linkage && !alone {
                        all.extend(p.sequence.linked(clip).into_iter().filter(|c| *c != clip && p.sequence.clip(*c).map(edge).zip(mine).is_some_and(|(a, b)| a.abs_diff(b) <= 2)));
                    }
                    for c in all {
                        if start {
                            p.sequence.trim_start(c, t);
                        } else {
                            p.sequence.trim_end(c, t);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// A moment of sound at `t` while scrubbing (a new one at most every 90 ms, each heard for
    /// 140 ms).
    fn scrub_sound(&mut self, t: u64, cx: &mut Context<Self>) {
        if !self.scrub_audio || self.playing.is_some() {
            return;
        }
        // only when the playhead reached another frame (holding still is silent), and not
        // more often than a snippet lasts
        let frame = (1000.0 / self.seq().map_or(30.0, |s| s.fps.max(1.0))).round() as u64;
        if self.scrub_at.is_some_and(|a| a.abs_diff(t) < frame) || self.scrub.as_ref().is_some_and(|(_, at)| at.elapsed() < std::time::Duration::from_millis(80)) {
            return;
        }
        self.scrub_at = Some(t);
        let Some(sp) = self.seq().and_then(|s| speaker::Speaker::snippet(s, t, 110)) else { return };
        sp.clock().start();
        let began = std::time::Instant::now();
        self.scrub = Some((sp, began));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
            let _ = this.update(cx, |this, _| {
                if this.scrub.as_ref().is_some_and(|(_, at)| *at == began) {
                    this.scrub = None;
                }
            });
        })
        .detach();
    }

    /// The top of track `ti` (a window y).
    fn track_top(&self, ti: usize) -> Option<f32> {
        let b = (*self.timeline_bounds.borrow())?;
        let seq = self.seq()?;
        let above: f32 = seq.tracks.iter().take(ti).map(track_h).sum();
        Some(f32::from(b.origin.y) + self.tracks_top() + above)
    }

    /// The renderer takes a change made by dragging (one undo step when the drag ends).
    fn after_drag_change(&mut self) {
        if let Some(p) = &self.project {
            self.pending.lock().unwrap().seq = Some(p.sequence.clone());
        }
        self.request(self.playhead);
    }

    /// A drag ended: what it changed is one undo step.
    fn drag_end(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        if matches!(drag, Drag::Scrub) {
            self.scrub_at = None;
            return;
        }
        if let (Some(before), Some(p)) = (self.drag_before.take(), &mut self.project) {
            p.sequence.tidy();
            if before != p.sequence {
                self.undo.push(before);
                self.redo.clear();
            } else if let Drag::Move { clip, .. } = drag {
                self.picked = vec![clip];
                self.selected = Some(clip);
            }
        }
        self.settle(cx);
    }

    /// A studio_take's look, changed (saved where the media says, drawn again).
    fn change_look(&mut self, media: Id, f: impl FnOnce(&mut Edit), cx: &mut Context<Self>) {
        let Some(m) = self.seq().and_then(|s| s.media(media)).cloned() else { return };
        let Some(path) = m.look.clone().or_else(|| m.studio_take.as_ref().map(|t| t.join("edit.json"))) else { return };
        let mut e = Edit::load(&path).unwrap_or_else(|_| {
            let mut e = Edit::new(m.duration_ms.max(1));
            if let Some(log) = m.studio_take.as_ref().and_then(|t| lightair_store::input_log::Log::read(&t.join("input.bin")).ok()) {
                e.zooms = zoom::auto(&log, m.duration_ms);
            }
            e
        });
        f(&mut e);
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = e.save(&path);
        // playback holds the renderer: stopped first
        self.stop(cx);
        if let Some(r) = self.renderer.lock().unwrap().as_mut() {
            r.forget_studio_take(media);
        }
        self.request(self.playhead);
        cx.notify();
    }

    fn look_of(&self, media: &Media) -> Edit {
        media
            .look
            .clone()
            .or_else(|| media.studio_take.as_ref().map(|t| t.join("edit.json")))
            .and_then(|p| Edit::load(&p).ok())
            .unwrap_or_else(|| Edit::new(media.duration_ms.max(1)))
    }

    // ------------------------------------------------------------ view

    fn chips<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        options: &[(&'static str, T)],
        current: T,
        on: impl Fn(&mut Self, T, &mut Context<Self>) + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        self.segmented(SharedString::from(id), options, current, on, cx)
    }

    /// A labelled value with − and + beside it.
    fn stepper(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        step: impl Fn(&mut Self, f32, &mut Context<Self>) + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        self.field(SharedString::from(id), label, value, std::rc::Rc::new(step), cx)
    }

    /// For the window's title bar, beside the workspace: the project picker (and its name
    /// being typed).
    pub fn title_left(&self, cx: &mut Context<Self>) -> AnyElement {
        let (picker, _) = self.title_parts(cx);
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(ui::icon("chevron-right").size(px(12.)).text_color(tok::text_tertiary()))
            .child(div().p_0p5().rounded(px(8.)).bg(tok::bg_panel()).border_1().border_color(tok::border()).child(picker))
            .when_some(self.naming.as_ref(), |d, (_, input)| d.child(div().w(px(200.)).child(TextInput::new(input).small())))
            .into_any_element()
    }

    /// For the window's title bar, on the right: undo, redo, export.
    pub fn title_right(&self, cx: &mut Context<Self>) -> AnyElement {
        let (_, export) = self.title_parts(cx);
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(ui::icon_button("ed-undo", "undo-2", tok::text_secondary()).on_click(cx.listener(|this, _, _, cx| this.undo(true, cx))))
            .child(ui::icon_button("ed-redo", "redo-2", tok::text_secondary()).on_click(cx.listener(|this, _, _, cx| this.undo(false, cx))))
            .child(div().w(px(6.)))
            .child(export)
            .into_any_element()
    }

    fn title_parts(&self, cx: &mut Context<Self>) -> (AnyElement, AnyElement) {
        let me = cx.entity().downgrade();
        let (ws, current) = (self.workspace.clone(), self.project.as_ref().map(|p| p.id));
        let name = self.project.as_ref().map_or("No project".to_string(), |p| p.name.clone());
        let picker = Button::new("ed-project").label(name).small().ghost().dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
            for p in ws.as_ref().map(|w| Project::list(w)).unwrap_or_default() {
                let (me, id) = (me.clone(), p.id);
                menu = menu.item(PopupMenuItem::new(p.name).checked(current == Some(p.id)).on_click(move |_, _, cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |this, cx| this.open_project(id, cx));
                    }
                }));
            }
            let (a, b, c) = (me.clone(), me.clone(), me.clone());
            menu.separator()
                .item(PopupMenuItem::new("New Project…").icon(ui::icon("plus")).on_click(move |_, w, cx| {
                    if let Some(me) = a.upgrade() {
                        me.update(cx, |this, cx| this.start_naming(None, w, cx));
                    }
                }))
                .item(PopupMenuItem::new("Rename…").icon(ui::icon("pencil")).disabled(current.is_none()).on_click(move |_, w, cx| {
                    if let Some(me) = b.upgrade() {
                        me.update(cx, |this, cx| {
                            let id = this.project.as_ref().map(|p| p.id);
                            this.start_naming(id, w, cx)
                        });
                    }
                }))
                .separator()
                .item(PopupMenuItem::new("Delete Project").icon(ui::icon("trash")).disabled(current.is_none()).on_click(move |_, _, cx| {
                    if let Some(me) = c.upgrade() {
                        me.update(cx, |this, cx| this.delete_project(cx));
                    }
                }))
        });
        let export = match self.exporting {
            Some(p) => Button::new("ed-export")
                .label(format!("Exporting {:.0}%", p * 100.0))
                .small()
                .outline()
                .loading(true)
                .on_click(cx.listener(|this, _, _, cx| this.open_export(cx)))
                .into_any_element(),
            None => Button::new("ed-export")
                .label("Export…")
                .icon(ui::icon("upload"))
                .small()
                .primary()
                .on_click(cx.listener(|this, _, _, cx| this.open_export(cx)))
                .into_any_element(),
        };
        (picker.into_any_element(), export)
    }

    fn media_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hidden = self.drag_hidden.clone();
        let ed = cx.entity().downgrade();
        let card = |id: SharedString, title: String, detail: String, icon: &'static str, thumb: Option<PathBuf>, source: MediaSource, cx: &mut Context<Self>| {
            let drag = DraggedMedia::new(title.clone(), source.clone(), thumb.clone(), icon, &hidden);
            let (ed, menu_source) = (ed.clone(), source.clone());
            div()
                .id(id.clone())
                .flex()
                .flex_col()
                .gap_1()
                .w(px(122.))
                .cursor_grab()
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(68.))
                        .rounded(px(6.))
                        .overflow_hidden()
                        .bg(tok::bg_canvas_well())
                        .flex()
                        .items_center()
                        .justify_center()
                        .map(|d| match thumb {
                            Some(p) => d.child(img(p).size_full().object_fit(ObjectFit::Cover)),
                            None => d.child(ui::icon(icon).size(px(20.)).text_color(tok::text_tertiary())),
                        })
                        .child(
                            div()
                                .id((id.clone(), 1usize))
                                .absolute()
                                .right(px(4.))
                                .bottom(px(4.))
                                .size(px(20.))
                                .rounded_full()
                                .bg(rgba(0x000000c0))
                                .border_1()
                                .border_color(rgba(0xffffff60))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|d| d.bg(rgba(0x000000ff)))
                                .child(ui::icon("plus").size(px(12.)).text_color(rgb(0xFFFFFF)))
                                .on_click(cx.listener(move |this, _, _, cx| this.add(source.clone(), None, None, cx))),
                        ),
                )
                .child(div().text_size(px(11.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(title))
                .child(div().text_size(px(10.)).text_color(tok::text_tertiary()).child(detail))
                .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                .context_menu(move |m, _, cx| menus::media_menu(m, &ed, menu_source.clone(), cx))
        };
        let mut grid = div().flex().flex_wrap().gap_2();
        if let Some(studio_take) = self.opened_studio_take.clone() {
            // a studio_take opened: its parts, each on its own
            let parts = self.studio_take_parts.get(&studio_take.id).cloned().flatten();
            match parts {
                None => grid = grid.child(hint("Reading the take…")),
                Some(parts) => {
                    for (n, p) in parts.into_iter().enumerate() {
                        let detail = match &p.media {
                            Some(m) if m.kind != MediaKind::Image => format!("{} · {}", p.what(), short(m.duration_ms)),
                            _ => p.what().to_string(),
                        };
                        use lightair_render::sequence::PartKind;
                        let thumb = match (&p.media, p.kind()) {
                            (Some(m), Some(PartKind::Picture)) => Some(m.path.clone()),
                            (_, Some(PartKind::Screen)) => studio_take.thumbnail.clone().filter(|t| t.exists()),
                            _ => None,
                        };
                        let icon = crate::library::part_icon(&p);
                        grid = grid.child(card(SharedString::from(format!("ed-part-{n}")), p.name.clone(), detail, icon, thumb, MediaSource::Part(Box::new(p)), cx));
                    }
                }
            }
        } else {
            for (n, item) in self.library.iter().enumerate() {
                let detail = format!("{} · {}", item.kind.label(), item.duration_ms.map_or("—".into(), short));
                let thumb = item.thumbnail.clone().filter(|t| t.exists());
                let studio_take = (item.kind == Kind::StudioTake).then(|| item.clone());
                grid = grid.child(
                    div()
                        .relative()
                        .child(card(SharedString::from(format!("ed-lib-{n}")), item.title.clone(), detail, kind_icon(item.kind), thumb, MediaSource::Item(item.clone()), cx))
                        .when_some(studio_take, |d, studio_take| {
                            // a studio_take opens like a folder: its parts
                            d.child(
                                div()
                                    .id(("ed-open-take", n))
                                    .absolute()
                                    .left(px(4.))
                                    .top(px(44.))
                                    .h(px(20.))
                                    .px_1p5()
                                    .rounded(px(10.))
                                    .bg(rgba(0x000000c0))
                                    .border_1()
                                    .border_color(rgba(0xffffff60))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .cursor_pointer()
                                    .hover(|d| d.bg(rgba(0x000000ff)))
                                    .text_size(px(10.))
                                    .text_color(rgb(0xFFFFFF))
                                    .child(ui::icon("layers").size(px(11.)))
                                    .child("Parts")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.open_studio_take(studio_take.clone(), cx)
                                    })),
                            )
                        }),
                );
            }
        }
        let imported: Vec<Media> = self.seq().map(|s| s.media.iter().filter(|m| m.item.is_none()).cloned().collect()).unwrap_or_default();
        let mut own = div().flex().flex_wrap().gap_2();
        for (n, m) in imported.iter().enumerate() {
            let icon = match m.kind {
                MediaKind::Video => "film",
                MediaKind::Audio => "music",
                MediaKind::Image => "image",
            };
            let thumb = (m.kind == MediaKind::Image).then(|| m.path.clone());
            own = own.child(card(SharedString::from(format!("ed-imp-{n}")), m.name.clone(), short(m.duration_ms), icon, thumb, MediaSource::Media(m.id), cx));
        }
        let tabs: [(&'static str, &'static str, LeftTab); 8] = [
            ("Media", "film", LeftTab::Media),
            ("Audio", "music", LeftTab::Audio),
            ("Text", "type", LeftTab::Text),
            ("Stickers", "sticker", LeftTab::Shapes),
            ("Effects", "sparkles", LeftTab::Effects),
            ("Transitions", "blend", LeftTab::Transitions),
            ("Captions", "captions", LeftTab::Captions),
            ("Filters", "aperture", LeftTab::Filters),
        ];
        let mut bar = div().id("ed-left-tabs").flex().flex_none().items_center().gap_0p5().px_2().h(px(48.)).border_b_1().border_color(tok::border()).overflow_x_scroll();
        for (n, (name, icon, tab)) in tabs.into_iter().enumerate() {
            let on = self.left == tab;
            bar = bar.child(
                div()
                    .id(("ed-left", n))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_0p5()
                    .flex_none()
                    .min_w(px(40.))
                    .px_1()
                    .py_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(if on { tok::text() } else { tok::text_secondary() })
                    .when(on, |d| d.bg(tok::bg_selected()))
                    .hover(|d| d.bg(tok::bg_selected()))
                    .child(ui::icon(icon).size(px(15.)))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.left = tab;
                        cx.notify();
                    })),
            );
        }
        let body = match self.left {
            LeftTab::Media => match self.opened_studio_take.clone() {
                Some(studio_take) => div()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .pt_2()
                            .pb_1()
                            .child(ui::icon_button("ed-take-back", "arrow-left", tok::text_secondary()).on_click(cx.listener(|this, _, _, cx| {
                                this.opened_studio_take = None;
                                cx.notify();
                            })))
                            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(studio_take.title.clone()))
                            .child(Button::new("ed-take-all").label("Add all").icon(ui::icon("plus")).xsmall().outline().on_click(cx.listener(move |this, _, _, cx| {
                                this.add(MediaSource::Item(studio_take.clone()), None, None, cx)
                            }))),
                    )
                    .child(hint("Its parts: drag one onto a track, or add it with +. Each keeps its place in the take's scene."))
                    .child(div().pt_2().child(grid)),
                None => div()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().child(heading("THIS WORKSPACE")))
                            .child(Button::new("ed-import").label("Import").icon(ui::icon("plus")).xsmall().outline().on_click(cx.listener(|this, _, w, cx| this.import(w, cx)))),
                    )
                    .child(if self.library.is_empty() { hint("Recordings, takes and clips appear here.") } else { grid })
                    .when(!imported.is_empty(), |d| d.child(heading("IMPORTED")).child(own)),
            },
            LeftTab::Audio => self.audio_tab(cx),
            LeftTab::Text => self.text_tab(cx),
            LeftTab::Shapes => self.shapes_tab(cx),
            LeftTab::Captions => self.captions_tab(cx),
            LeftTab::Effects => self.effects_tab(cx),
            LeftTab::Transitions => self.transitions_tab(cx),
            LeftTab::Filters => self.filters_tab(cx),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .border_r_1()
            .border_color(tok::border())
            .bg(tok::bg_panel())
            .child(bar)
            .child(div().id("ed-media").flex_1().min_h_0().overflow_y_scroll().px_3().pb_3().pt_1().child(body))
    }

    fn player(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let presented_size = match &self.preview {
            Some(Picture::Presented(size)) => Some(*size),
            _ => None,
        };
        let presented = presented_size.is_some();
        let duration = self.seq().map_or(0, |s| s.duration_ms());
        let size = self.seq().map_or((1920, 1080), |s| s.size);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .m_2()
                    .rounded(px(6.))
                    .overflow_hidden()
                    // Windows: the frames are under the UI (with their own black behind them)
                    .when(!presented, |d| d.bg(rgb(0x000000)))
                    .child({
                        let (cell, bounds) = (self.player_px.clone(), self.player_bounds.clone());
                        let native = self.native.clone();
                        canvas(
                            move |b, window, _| {
                                cell.set((f32::from(b.size.width) * window.scale_factor(), f32::from(b.size.height) * window.scale_factor()));
                                *bounds.borrow_mut() = Some(b);
                                if let Some(n) = native.borrow_mut().as_mut() {
                                    n.place_player(presented_size.map(|frame| (b, frame)));
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    })
                    .when_some(self.preview.clone(), |d, p| d.child(p.element()))
                    .children(self.stage_overlay())
                    .when(self.hiding, |d| d.cursor_crosshair())
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| this.stage_down(ev.position, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, _, cx| this.menu_pick_stage(ev.position, cx)))
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        if this.stage_drag.is_some() {
                            this.stage_move(ev.position, cx);
                        }
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.stage_up(cx)))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.stage_up(cx)))
                    .context_menu({
                        let ed = cx.entity().downgrade();
                        move |m, _, cx| menus::clip_menu(m, &ed, cx)
                    })
                    .when(self.project.is_none(), |d| {
                        d.flex().items_center().justify_center().child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_2()
                                .child(div().text_color(tok::text_secondary()).child("No project open"))
                                .child(Button::new("ed-first").label("New project").small().primary().on_click(cx.listener(|this, _, w, cx| this.start_naming(None, w, cx)))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .pb_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .font_family(crate::ui::MONO)
                            .child(div().flex().gap_1().child(div().text_color(tok::accent_primary()).child(clock(self.playhead))).child(div().text_color(tok::text_tertiary()).child(format!("/ {}", clock(duration))))),
                    )
                    .child(ui::icon_button("ed-play", if self.playing.is_some() { "pause" } else { "play" }, tok::text()).on_click(cx.listener(|this, _, _, cx| this.play(cx))))
                    .child(div().flex_1().flex().justify_end().text_size(px(11.)).text_color(tok::text_tertiary()).child(format!("{}×{} · {} fps", size.0, size.1, fps_label(self.seq().map_or(30.0, |s| s.fps))))),
            )
    }

    fn inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let clip = self.selected.and_then(|c| self.seq().and_then(|s| s.clip(c)).cloned());
        let media = clip.as_ref().and_then(|c| self.seq().and_then(|s| s.media(c.media)).cloned());
        let mut body = div().flex().flex_col().gap_2().px_3().pb_4();
        // the head and the tabs stay; the sections scroll under them
        let mut head: Option<AnyElement> = None;
        let mut tab_row: Option<AnyElement> = None;
        if let Some((h, b)) = self.zoom_inspector(cx) {
            head = Some(h);
            body = b;
        } else {
        match (clip, media) {
            (Some(c), None) if c.content.is_some() => {
                let name = match &c.content {
                    Some(Content::Shape(_)) => "Shape",
                    _ => "Text",
                };
                let mut tabs = vec![(name, InspectorTab::Text), ("Transform", InspectorTab::Video), ("Animation", InspectorTab::Animation)];
                if !c.effects.is_empty() {
                    tabs.push(("Effects", InspectorTab::Effects));
                }
                if c.transition.is_some() {
                    tabs.push(("Transition", InspectorTab::Transition));
                }
                let tab = if tabs.iter().any(|t| t.1 == self.tab) { self.tab } else { InspectorTab::Text };
                let icon = if matches!(c.content, Some(Content::Shape(_))) { "shapes" } else { "type" };
                head = Some(self.inspector_head(icon, c.content.as_ref().map(looks::content_name).unwrap_or_default(), format!("{name} · {}", short(c.len_ms()))));
                tab_row = Some(self.tab_bar(&tabs, tab, cx));
                body = match tab {
                    InspectorTab::Video => self.transform_section(&c, body, cx),
                    InspectorTab::Effects => self.effects_section(&c, body, cx),
                    InspectorTab::Transition => self.transition_section(&c, body, cx),
                    InspectorTab::Animation => self.animation_section(&c, body, cx),
                    _ => self.content_section(&c, body, cx),
                };
            }
            (Some(c), Some(m)) => {
                let studio_take = m.studio_take.is_some();
                let audio = m.kind == MediaKind::Audio;
                let mut tabs: Vec<(&'static str, InspectorTab)> = Vec::new();
                if !audio {
                    tabs.push(("Video", InspectorTab::Video));
                }
                // a video whose sound is on its own track: that clip has the sound's controls
                let sound_apart = !audio && c.muted && c.link.is_some();
                if !sound_apart {
                    tabs.push(("Audio", InspectorTab::Audio));
                }
                tabs.push(("Speed", InspectorTab::Speed));
                if !audio {
                    tabs.extend([("Animation", InspectorTab::Animation), ("Adjust", InspectorTab::Filter)]);
                    // what's been put on it from the media panel
                    if !c.effects.is_empty() {
                        tabs.push(("Effects", InspectorTab::Effects));
                    }
                    if c.transition.is_some() {
                        tabs.push(("Transition", InspectorTab::Transition));
                    }
                }
                if m.kind == MediaKind::Audio || !m.audio.is_empty() {
                    tabs.push(("Words", InspectorTab::Words));
                }
                if studio_take {
                    tabs.push(("Screen", InspectorTab::Screen));
                }
                let tab = if tabs.iter().any(|t| t.1 == self.tab) { self.tab } else { tabs[0].1 };
                let (icon, what) = match (studio_take, m.kind) {
                    (true, _) => ("mouse-pointer-2", "Studio take"),
                    (_, MediaKind::Audio) => ("audio-lines", "Sound"),
                    (_, MediaKind::Image) => ("image", "Picture"),
                    _ => ("video", "Video"),
                };
                head = Some(self.inspector_head(icon, m.name.clone(), format!("{what} · {}", short(c.len_ms()))));
                tab_row = Some(self.tab_bar(&tabs, tab, cx));
                let cid = c.id;
                match tab {
                    InspectorTab::Text | InspectorTab::Video => body = self.crop_section(&c, self.hidden_section(&c, self.transform_section(&c, body, cx), cx), cx),
                    InspectorTab::Filter => body = self.filter_section(&c, body, cx),
                    InspectorTab::Effects => body = self.effects_section(&c, body, cx),
                    InspectorTab::Words => body = self.words_section(&c, &m, body, cx),
                    InspectorTab::Transition => body = self.transition_section(&c, body, cx),
                    InspectorTab::Animation => body = self.animation_section(&c, body, cx),
                    InspectorTab::Audio => body = self.audio_section(&c, &m, body, cx),
                    InspectorTab::Speed => {
                        body = body.child(label("SPEED")).child(self.chips(
                            "ed-speed",
                            &[("0.25×", 25), ("0.5×", 50), ("1×", 100), ("1.5×", 150), ("2×", 200), ("4×", 400), ("8×", 800)],
                            (c.speed * 100.0).round() as i32,
                            move |this, v, cx| {
                                let mut ok = true;
                                this.change(|s| ok = s.set_speed(cid, v as f32 / 100.0), cx);
                                if !ok {
                                    this.note = Some("Slower would run into the next clip: make room first.".into());
                                    cx.notify();
                                }
                            },
                            cx,
                        ));
                    }
                    InspectorTab::Screen => {
                        let look = self.look_of(&m);
                        let mid = m.id;
                        let mut sw = div().flex().flex_wrap().gap_1p5();
                        for (n, (colours, _)) in lightair_render::compose::BACKDROPS.iter().enumerate() {
                            let on = look.look.backdrop == n as i64;
                            sw = sw.child(
                                div()
                                    .id(("ed-bd", n))
                                    .size(px(26.))
                                    .rounded(px(6.))
                                    .border_2()
                                    .border_color(if on { tok::accent_primary() } else { tok::bg_panel() })
                                    .bg(rgb(colours[1]))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| this.change_look(mid, |e| e.look.backdrop = n as i64, cx))),
                            );
                        }
                        body = body
                            .child(label("BACKDROP"))
                            .child(sw)
                            .child(label("PADDING"))
                            .child(self.chips("ed-pad", &[("None", 0), ("Small", 1), ("Medium", 2), ("Large", 3)], ((look.look.padding / 0.035).round() as i32).clamp(0, 3), move |this, v, cx| this.change_look(mid, |e| e.look.padding = v as f32 * 0.035, cx), cx))
                            .child(label("POINTER"))
                            .child(self.chips("ed-ps", &[("Pointer", 0i64), ("Dot", 1), ("Ring", 2), ("Hidden", -1)], if look.pointer.hidden { -1 } else { look.pointer.style }, move |this, v, cx| {
                                this.change_look(mid, |e| {
                                    e.pointer.hidden = v < 0;
                                    if v >= 0 {
                                        e.pointer.style = v;
                                    }
                                }, cx)
                            }, cx))
                            .child(self.chips("ed-psize", &[("Small", 10), ("Medium", 15), ("Large", 25)], (look.pointer.size * 10.0).round() as i32, move |this, v, cx| this.change_look(mid, |e| e.pointer.size = v as f32 / 10.0, cx), cx))
                            .child(label("CLICKS"))
                            .child(self.chips("ed-clicks", &[("None", 0i64), ("Circle", 1), ("Ripple", 2)], look.pointer.clicks, move |this, v, cx| this.change_look(mid, |e| e.pointer.clicks = v, cx), cx))
                            .child(label("ZOOMS"))
                            .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child(format!("{} zooms, on the zoom track under the timeline", self.zooms_of(&m).len())))
                            .child(Button::new("ed-autozoom").label("Automatic zooms").icon(ui::icon("sparkles")).xsmall().outline().on_click(cx.listener(move |this, _, _, cx| {
                                let Some(m) = this.seq().and_then(|s| s.media(mid)).cloned() else { return };
                                let Some(log) = m.studio_take.as_ref().and_then(|t| lightair_store::input_log::Log::read(&t.join("input.bin")).ok()) else { return };
                                let auto = zoom::auto(&log, m.duration_ms);
                                let mut zs = this.zooms_of(&m);
                                zs.retain(|z| !z.auto);
                                zs.extend(auto);
                                zs.sort_by_key(|z| z.start_ms);
                                this.zoom_sel = None;
                                this.change(|s| if let Some(m) = s.media.iter_mut().find(|x| x.id == mid) { m.zooms = Some(zs) }, cx)
                            })));
                    }
                }
            }
            _ => {
                body = self.project_section(body, cx);
                body = self.proxies_section(body, cx).child(div().pt_2().text_size(px(12.)).text_color(tok::text_tertiary()).child("Select a clip to change it."));
            }
        }
        }
        let head = head.unwrap_or_else(|| {
            let p = self.project.as_ref();
            self.inspector_head("clapperboard", p.map_or("No project".into(), |p| p.name.clone()), self.seq().map_or(String::new(), |s| format!("Project · {}×{} · {}", s.size.0, s.size.1, short(s.duration_ms()))))
        });
        div()
            .flex()
            .flex_col()
            .size_full()
            .border_l_1()
            .border_color(tok::border())
            .bg(tok::bg_panel())
            .child(head)
            .children(tab_row)
            .child(div().id("ed-inspector").flex_1().min_h_0().overflow_y_scroll().child(body))
            // a number field dragged: it follows the pointer anywhere over the inspector
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.field_drag.is_some() {
                    this.field_move(ev.position.x, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.field_end()))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.field_end()))
    }

    fn timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ed = cx.entity().downgrade();
        let pps = self.pps();
        let bounds = self.timeline_bounds.clone();
        let Some(seq) = self.seq().cloned() else {
            return div().h(px(200.)).flex().items_center().justify_center().text_color(tok::text_tertiary()).child("Make a project to start editing.").into_any_element();
        };
        // the ruler: a label every ~90 px
        let step_s = [0.5f32, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0].into_iter().find(|s| s * pps >= 90.0).unwrap_or(600.0);
        let mut ruler = div().relative().h(px(RULER_H)).flex_1().overflow_hidden();
        let first = (self.scroll_ms / 1000.0 / step_s).floor() as i64;
        for k in first..first + 60 {
            let s = k as f32 * step_s;
            let x = self.x_of((s * 1000.0) as u64);
            if x > 4000.0 {
                break;
            }
            ruler = ruler.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top_0()
                    .h_full()
                    .border_l_1()
                    .border_color(tok::border())
                    .pl_1()
                    .text_size(px(10.))
                    .text_color(tok::text_tertiary())
                    .child(short((s * 1000.0) as u64)),
            );
        }
        let mut rows = div().flex().flex_col();
        let mut video_n = seq.tracks.iter().filter(|t| t.kind == TrackKind::Video).count();
        let mut audio_n = 0;
        for (ti, track) in seq.tracks.iter().enumerate() {
            let h = track_h(track);
            let name = if track.kind == TrackKind::Video {
                let n = video_n;
                video_n -= 1;
                format!("V{n}")
            } else {
                audio_n += 1;
                format!("A{audio_n}")
            };
            let toggle = |icon: &'static str, on: bool, id: usize, f: fn(&mut lightair_render::sequence::Track), cx: &mut Context<Self>| {
                ui::icon_button(("ed-tr", ti * 4 + id), icon, if on { tok::text_tertiary() } else { tok::text_secondary() })
                    .on_click(cx.listener(move |this, _, _, cx| this.change(|s| if let Some(t) = s.tracks.get_mut(ti) { f(t) }, cx)))
            };
            let header = div()
                .id(("ed-track-head", ti))
                .flex()
                .items_center()
                .gap_0p5()
                .w(px(HEADER_W))
                .flex_none()
                .h_full()
                .px_1()
                .border_r_1()
                .border_color(tok::border())
                .child(div().w(px(24.)).text_size(px(11.)).text_color(tok::text_tertiary()).child(if track.captions { "CC".to_string() } else { name }))
                .when(track.kind == TrackKind::Video, |d| d.child(toggle(if track.hidden { "eye-off" } else { "eye" }, track.hidden, 0, |t| t.hidden = !t.hidden, cx)))
                // sound is on the audio tracks: only they're muted
                .when(track.kind == TrackKind::Audio, |d| d.child(toggle(if track.muted { "volume-x" } else { "volume-2" }, track.muted, 1, |t| t.muted = !t.muted, cx)))
                .child(toggle(if track.locked { "lock" } else { "lock-open" }, track.locked, 2, |t| t.locked = !t.locked, cx))
                .context_menu({
                    let ed = ed.clone();
                    move |m, _, cx| menus::track_menu(m, &ed, ti, cx)
                });
            let mut lane = div().relative().flex_1().h_full().overflow_hidden().when(self.tool == tools::Tool::Split, |d| d.cursor(CursorStyle::ResizeColumn));
            let mut badges = Vec::new();
            for c in &track.clips {
                let m = seq.media(c.media);
                let left = self.x_of(c.start_ms);
                let width = (c.len_ms() as f32 / 1000.0 * pps).max(4.0);
                if left > 4000.0 || left + width < 0.0 {
                    continue;
                }
                let selected = self.is_picked(c.id);
                let colour = match (track.kind, &c.content, m.and_then(|m| m.studio_take.as_ref())) {
                    (TrackKind::Audio, _, _) => rgb(0x1E4E8C),
                    (_, Some(Content::Text(_)), _) => rgb(0xB45309),
                    (_, Some(Content::Shape(_)), _) => rgb(0xBE185D),
                    (_, _, Some(_)) => rgb(0x6D28D9),
                    _ => rgb(0x0E7C86),
                };
                // linked: a link before its name (underlined), and how far out of step it is with
                // its partner of the same file, if it is
                let partners: Vec<lightair_render::sequence::Clip> = seq.linked(c.id).into_iter().filter(|p| *p != c.id).filter_map(|p| seq.clip(p).cloned()).collect();
                let linked = !partners.is_empty();
                let slip: Option<i64> = partners.iter().find(|p| p.media == c.media).map(|p| (c.start_ms as i64 - (c.in_ms as f64 / c.speed.max(0.05) as f64) as i64) - (p.start_ms as i64 - (p.in_ms as f64 / p.speed.max(0.05) as f64) as i64)).filter(|d| d.abs() > 20);
                let title = match (&c.content, m) {
                    (Some(content), _) => looks::content_name(content),
                    (None, Some(m)) => match c.stream.and_then(|n| m.tracks.get(n as usize)) {
                        Some(track) => format!("{} · {track}", m.name),
                        None => m.name.clone(),
                    },
                    (None, None) => "Missing".into(),
                };
                let pps_ = pps;
                // every property's keys (not the volume's, drawn apart)
                let diamonds: Vec<f32> = c.key_times().into_iter().map(|k| k as f32 / 1000.0 * pps_).collect();
                let volume_marks: Vec<f32> = c.volume_keys.iter().map(|k| k.at_ms as f32 / 1000.0 * pps_).collect();
                let wave = if track.kind == TrackKind::Audio { self.wave(c, m, left, width, h) } else { None };
                let strip = if track.kind == TrackKind::Video && c.content.is_none() { self.filmstrip(c, m, left, width, h - 6.0) } else { None };
                let has_strip = strip.is_some();
                // a transition straddles the cut before the clip
                if let Some(tn) = c.transition {
                    let half = tn.duration_ms as f32 / 2000.0 * pps;
                    badges.push(
                        div()
                            .absolute()
                            .top(px(h / 2.0 - 9.0))
                            .left(px(left - half.max(7.0)))
                            .w(px((half * 2.0).max(14.0)))
                            .h(px(18.))
                            .rounded(px(4.))
                            .bg(rgba(0xFFFFFFD0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(ui::icon("blend").size(px(11.)).text_color(rgb(0x111111))),
                    );
                }
                let (cid, start, locked) = (c.id, c.start_ms, track.locked);
                // fades: shaded ends; the selected clip's handles and, for sound, its volume line
                let (fin, fout) = (c.fade_in_ms as f32 / 1000.0 * pps, c.fade_out_ms as f32 / 1000.0 * pps);
                let inner = h - 10.0;
                let is_audio = track.kind == TrackKind::Audio;
                let volume_y = (1.0 - c.volume_at(self.playhead.clamp(c.start_ms, c.end_ms().saturating_sub(1))).min(2.0) / 2.0) * inner;
                let mut extras: Vec<AnyElement> = Vec::new();
                if fin > 0.5 {
                    extras.push(div().absolute().top_0().bottom_0().left_0().w(px(fin)).bg(linear_gradient(90., linear_color_stop(rgba(0x000000b0), 0.), linear_color_stop(rgba(0x00000000), 1.))).into_any_element());
                }
                if fout > 0.5 {
                    extras.push(div().absolute().top_0().bottom_0().right_0().w(px(fout)).bg(linear_gradient(270., linear_color_stop(rgba(0x000000b0), 0.), linear_color_stop(rgba(0x00000000), 1.))).into_any_element());
                }
                if selected && !locked {
                    // (kept clear of the edges, which trim)
                    let knob = |x: f32| {
                        div()
                            .absolute()
                            .top(px(1.))
                            .left(px(x - 5.0))
                            .size(px(10.))
                            .rounded_full()
                            .bg(rgb(0xFFFFFF))
                            .border_1()
                            .border_color(rgb(0x111111))
                            .cursor(CursorStyle::ResizeLeftRight)
                            .into_any_element()
                    };
                    extras.push(knob(fin.max(FADE_KNOB)));
                    extras.push(knob((width - fout.max(FADE_KNOB)).max(fin.max(FADE_KNOB) + 10.0)));
                    if is_audio {
                        // the volume line, in a band that's easy to catch
                        extras.push(
                            div()
                                .absolute()
                                .left(px(EDGE))
                                .right(px(EDGE))
                                .top(px(volume_y - 4.0))
                                .h(px(9.))
                                .flex()
                                .items_center()
                                .cursor(CursorStyle::ResizeUpDown)
                                .child(div().w_full().h(px(1.5)).bg(rgba(0xFFFFFFE0)))
                                .into_any_element(),
                        );
                    }
                }
                // the edges trim (with the select tool, on tracks that aren't locked)
                if !locked && self.tool == tools::Tool::Select {
                    for left_edge in [true, false] {
                        extras.push(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .when(left_edge, |d| d.left_0())
                                .when(!left_edge, |d| d.right_0())
                                .w(px(EDGE))
                                .cursor(CursorStyle::ResizeLeftRight)
                                .into_any_element(),
                        );
                    }
                }
                lane = lane.child(
                    div()
                        .id(SharedString::from(format!("ed-clip-{}", c.id)))
                        .absolute()
                        .top(px(3.))
                        .bottom(px(3.))
                        .left(px(left))
                        .w(px(width))
                        .rounded(px(5.))
                        .bg(colour)
                        .border_2()
                        .border_color(if selected { rgb(0xFFFFFF).into() } else { tok::bg_app() })
                        .overflow_hidden()
                        .px_1p5()
                        .py_0p5()
                        .text_size(px(11.))
                        .text_color(rgb(0xFFFFFF))
                        // the tool's cursor over clips too
                        .map(|d| match self.tool {
                            tools::Tool::Split => d.cursor(CursorStyle::ResizeColumn),
                            tools::Tool::Leftward => d.cursor(CursorStyle::ResizeLeft),
                            tools::Tool::Rightward => d.cursor(CursorStyle::ResizeRight),
                            tools::Tool::Select => d,
                        })
                        .children(wave)
                        .children(strip)
                        .child(
                            div()
                                .relative()
                                .flex()
                                .gap_2()
                                .whitespace_nowrap()
                                .when(has_strip, |d| d.self_start().px_1().rounded(px(3.)).bg(rgba(0x000000a0)))
                                .when(linked, |d| d.child(ui::icon("link").size(px(10.)).text_color(rgba(0xFFFFFFD0))))
                                .child(div().when(linked, |d| d.underline()).child(title))
                                .child(div().opacity(0.7).child(short(c.len_ms())))
                                .when_some(slip, |d, ms| {
                                    d.child(
                                        div()
                                            .px_1()
                                            .rounded(px(3.))
                                            .bg(rgb(0xDC2626))
                                            .text_color(rgb(0xFFFFFF))
                                            .child(format!("{}{:.2}s", if ms > 0 { "+" } else { "−" }, ms.unsigned_abs() as f32 / 1000.0)),
                                    )
                                })
                                .when((c.speed - 1.0).abs() > 0.01, |d| d.child(div().opacity(0.8).child(format!("{}×", c.speed))))
                                // (a video whose sound is apart is muted as a matter of course)
                                .when(c.muted && !(linked && track.kind == TrackKind::Video), |d| d.child(ui::icon("volume-x").size(px(11.)))),
                        )
                        .children(diamonds.into_iter().map(|x| {
                            div().absolute().bottom(px(2.)).left(px(x - 5.0)).child(ui::icon("diamond").size(px(10.)).text_color(rgb(0xFDE68A)))
                        }))
                        .children(volume_marks.into_iter().map(|x| {
                            div().absolute().top(px(2.)).left(px(x - 5.0)).child(ui::icon("diamond").size(px(10.)).text_color(rgb(0x93C5FD)))
                        }))
                        .when(c.fade_in_ms > 0 || c.fade_out_ms > 0, |d| d.child(div().absolute().right(px(4.)).top(px(2.)).child(ui::icon("audio-waveform").size(px(10.)).text_color(rgba(0xFFFFFFA0)))))
                        .when(c.voice.is_some(), |d| d.child(div().absolute().right(px(16.)).top(px(2.)).child(ui::icon("mic-vocal").size(px(10.)).text_color(rgba(0xFFFFFFA0)))))
                        .children(extras)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                let was = this.selected == Some(cid);
                                // the split tool, or picking a side
                                if let Some(t) = this.time_at(ev.position.x)
                                    && this.tool_click(Some(cid), t, cx)
                                {
                                    return;
                                }
                                // ⌘: add or studio_take away; ⇧: the range from the last picked
                                if ev.modifiers.platform || ev.modifiers.control || ev.modifiers.shift {
                                    this.pick_click(cid, ev.modifiers.shift, cx);
                                    return;
                                }
                                // a clip of several picked keeps them (they move together); with
                                // linkage, the clips linked with it come along. ⌥ (or linkage
                                // off): only it, its linked clips left where they are
                                this.drag_alone = ev.modifiers.alt;
                                this.pick_for_drag(cid, ev.modifiers.alt);
                                this.selected = Some(cid);
                                if locked {
                                    cx.notify();
                                    return;
                                }
                                let Some(t) = this.time_at(ev.position.x) else { return };
                                let Some(b) = *this.timeline_bounds.borrow() else { return };
                                let x = f32::from(ev.position.x - b.origin.x) - HEADER_W;
                                let left = this.x_of(start);
                                let width = this.seq().and_then(|s| s.clip(cid)).map_or(0.0, |c| c.len_ms() as f32 / 1000.0 * this.pps());
                                // the selected clip's fade handles (top corners) and volume line
                                let top = this.track_top(ti).map_or(0.0, |y| f32::from(ev.position.y) - y - 5.0);
                                let (fin, fout, vol) = this.seq().and_then(|s| s.clip(cid)).map_or((0.0, 0.0, 1.0), |c| {
                                    (c.fade_in_ms as f32 / 1000.0 * this.pps(), c.fade_out_ms as f32 / 1000.0 * this.pps(), c.volume_at(this.playhead.clamp(c.start_ms, c.end_ms().saturating_sub(1))))
                                });
                                let local = x - left;
                                let near_line = is_audio && (top - (1.0 - vol.min(2.0) / 2.0) * inner).abs() < 5.0;
                                let d = if local < EDGE {
                                    Drag::TrimStart(cid)
                                } else if width - local < EDGE {
                                    Drag::TrimEnd(cid)
                                } else if was && top < 13.0 && (local - fin.max(FADE_KNOB)).abs() < 7.0 {
                                    Drag::FadeIn(cid)
                                } else if was && top < 13.0 && (local - (width - fout.max(FADE_KNOB))).abs() < 7.0 {
                                    Drag::FadeOut(cid)
                                } else if was && near_line {
                                    Drag::Volume { clip: cid, y: f32::from(ev.position.y), from: vol }
                                } else {
                                    Drag::Move { clip: cid, grab_ms: t as i64 - start as i64 }
                                };
                                this.begin_drag(d);
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.menu_pick(cid, cx)))
                        // a look dragged from the media panel goes on it
                        .when(track.kind == TrackKind::Video, |d| {
                            d.drag_over::<grids::DraggedLook>(|s, _, _, _| s.border_color(tok::accent_primary()))
                                .on_drop(cx.listener(move |this, l: &grids::DraggedLook, _, cx| {
                                    this.pick_only(Some(cid));
                                    this.apply_look(l.look, vec![cid], cx)
                                }))
                        })
                        .context_menu({
                            let ed = ed.clone();
                            move |m, _, cx| menus::clip_menu(m, &ed, cx)
                        }),
                );
            }
            let lane = lane.children(badges);
            rows = rows.child(
                div()
                    .relative()
                    .flex()
                    .h(px(h))
                    .border_b_1()
                    .border_color(tok::border())
                    .when(track.kind == TrackKind::Audio, |d| d.bg(tok::bg_panel()))
                    .child(header)
                    .child(lane)
                    // its bottom edge, under the header: dragged to make it taller or shorter
                    .child(
                        div()
                            .id(("ed-track-h", ti))
                            .absolute()
                            .left_0()
                            .bottom(px(-3.))
                            .w(px(HEADER_W))
                            .h(px(6.))
                            .cursor(CursorStyle::ResizeRow)
                            .hover(|d| d.bg(rgba(0x60A5FA80)))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.begin_drag(Drag::TrackHeight { track: ti, y: f32::from(ev.position.y), from: h });
                                cx.notify();
                            })),
                    ),
            );
        }
        let zoom_lane = self.zoom_lane(cx);
        let head = HEADER_W + self.x_of(self.playhead);
        // markers on the ruler; the moment under the pointer (preview axis)
        let marks: Vec<f32> = seq.markers.iter().map(|m| HEADER_W + self.x_of(*m)).filter(|x| *x >= HEADER_W).collect();
        let hover = self.hover.filter(|_| self.axis).map(|t| HEADER_W + self.x_of(t));
        let dragging_cursor = match self.drag {
            Some(Drag::TrimStart(_) | Drag::TrimEnd(_) | Drag::FadeIn(_) | Drag::FadeOut(_)) => Some(CursorStyle::ResizeLeftRight),
            Some(Drag::Volume { .. }) => Some(CursorStyle::ResizeUpDown),
            Some(Drag::TrackHeight { .. }) => Some(CursorStyle::ResizeRow),
            Some(Drag::Move { .. } | Drag::Zoom { part: zooms::ZoomPart::Body { .. }, .. }) => Some(CursorStyle::ClosedHand),
            Some(Drag::Zoom { .. }) => Some(CursorStyle::ResizeLeftRight),
            _ => None,
        };
        div()
            .id("ed-timeline")
            .when_some(dragging_cursor, |d, c| d.cursor(c))
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(180.))
            .overflow_hidden()
            .bg(tok::bg_app())
            .child(canvas(move |b, _, _| *bounds.borrow_mut() = Some(b), |_, _, _, _| {}).absolute().size_full())
            .child(div().flex().h(px(RULER_H)).border_b_1().border_color(tok::border()).child(div().w(px(HEADER_W)).flex_none()).child(ruler))
            // the zoom track, under the ruler (when there's a screen studio_take)
            .children(zoom_lane)
            .child(rows)
            .children(marks.into_iter().map(|x| {
                div().absolute().top(px(2.)).left(px(x - 5.0)).child(ui::icon("flag").size(px(11.)).text_color(rgb(0xFBBF24)))
            }))
            .when_some(hover.filter(|x| *x >= HEADER_W), |d, x| d.child(div().absolute().top_0().bottom_0().left(px(x)).w(px(1.)).bg(rgba(0xFFFFFF60))))
            .when(head >= HEADER_W, |d| d.child(div().absolute().top_0().bottom_0().left(px(head)).w(px(2.)).bg(rgb(0xFFFFFF))))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.hover_at(None, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, _, _| this.menu_at = this.time_at(ev.position.x)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    let Some(t) = this.time_at(ev.position.x) else { return };
                    let on_ruler = this.timeline_bounds.borrow().is_some_and(|b| f32::from(ev.position.y - b.origin.y) < RULER_H);
                    this.seek(t.min(this.seq().map_or(0, |s| s.duration_ms())), cx);
                    if on_ruler {
                        this.begin_drag(Drag::Scrub);
                    } else if !this.tool_click(None, t, cx) {
                        // the tracks' empty space: a box to pick clips with (⌘ or ⇧ adds)
                        let m = ev.modifiers;
                        this.begin_marquee(ev.position, m.platform || m.control || m.shift);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    this.drag_to(ev.position, cx);
                } else if this.marquee.is_some() {
                    this.marquee_to(ev.position, cx);
                } else {
                    let inside = this.timeline_bounds.borrow().is_some_and(|b| b.contains(&ev.position) && f32::from(ev.position.x - b.origin.x) > HEADER_W);
                    this.hover_at(inside.then_some(ev.position.x), cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.end_marquee(cx);
                this.drag_end(cx)
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.end_marquee(cx);
                this.drag_end(cx)
            }))
            .children(self.marquee_overlay())
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, w, cx| {
                let d = ev.delta.pixel_delta(px(20.));
                if ev.modifiers.platform || ev.modifiers.control {
                    // zoom around the pointer
                    let k = (1.0 - f32::from(d.y) * 0.01).clamp(0.5, 2.0);
                    this.zoom = (this.zoom * k).clamp(0.05, 40.0);
                } else {
                    let dx = if f32::from(d.x).abs() > f32::from(d.y).abs() { f32::from(d.x) } else { f32::from(d.y) };
                    this.scroll_ms = (this.scroll_ms - dx / this.pps() * 1000.0).max(0.0);
                }
                let _ = w;
                cx.notify();
            }))
            .children(self.ghost_overlay())
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<DraggedMedia>, _, cx| {
                let drag = ev.drag(cx).clone();
                this.drag_over(&drag, ev.event.position, ev.bounds.contains(&ev.event.position), cx);
            }))
            .on_drop(cx.listener(|this, m: &DraggedMedia, w, cx| {
                this.ghost = None;
                this.drag_hidden.set(false);
                let pos = w.mouse_position();
                let t = this.time_at(pos.x).map(|t| this.snap(t as i64, None));
                let track = this.track_at(pos.y);
                this.add(m.source.clone(), t, track, cx);
            }))
            .context_menu(move |m, _, cx| menus::blank_menu(m, &ed, cx))
            .into_any_element()
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !cx.has_active_drag() && std::env::var("LIGHTAIR_OPEN").as_deref() != Ok("editor-ghost") {
            self.ghost = None;
            self.drag_hidden.set(false);
        }
        self.sync_text_input(window, cx);
        self.sync_fields(window, cx);
        self.want_peaks();
        self.want_strips();
        self.fit_preview();
        // the media panel lists the workspace's Library as it is now
        let v = self.studio.read(cx).library_version;
        if self.library_seen != Some(v) {
            self.library_seen = Some(v);
            self.library = self.workspace.as_ref().map(|ws| Index::load(ws).newest_first().into_iter().cloned().collect()).unwrap_or_default();
        }
        use gpui_kit::component::{h_resizable, resizable_panel, v_resizable};
        // the media, the player and the inspector over the timeline: each edge dragged
        let top = h_resizable("ed-panels")
            .child(resizable_panel().size(px(300.)).size_range(px(220.)..px(560.)).child(self.media_panel(cx)))
            .child(resizable_panel().size_range(px(320.)..Pixels::MAX).child(self.player(cx)))
            .child(resizable_panel().size(px(320.)).size_range(px(240.)..px(560.)).child(self.inspector(cx)));
        let bottom = div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.tools(cx))
            .child(div().id("ed-timeline-wrap").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().child(self.timeline(cx)));
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .child(
                v_resizable("ed-rows")
                    .child(resizable_panel().size(px(470.)).size_range(px(220.)..Pixels::MAX).child(top))
                    .child(resizable_panel().size_range(px(160.)..Pixels::MAX).child(bottom)),
            )
            .children(self.export_overlay(cx))
    }
}

/// A frame rate as people write it: "30", "29.97", "23.976".
pub(crate) fn fps_label(fps: f64) -> String {
    let s = format!("{fps:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
