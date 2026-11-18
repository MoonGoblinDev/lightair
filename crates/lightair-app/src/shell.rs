//! The studio window (docs/05-interface-shell.md): title bar, Objects + Scenes, Canvas, Mixer,
//! Outputs + Inspector, status bar. Every edit goes through `Studio::execute`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::{
    Command, Direction, Canvas, Id, Source, Object, Layout, Workspace, Scene, TransitionKind, TransitionSettings,
};
use lightair_preview::Preview;
use serde_json::json;

pub(crate) mod app_menu;
mod add_object;
mod remote;
mod screen_pick;
mod assets;
mod docking;
mod effects;
mod golive;
mod hierarchy;
mod inspector;
mod object_props;
mod canvas_text;
mod colour_field;
mod connections;
mod console_panel;
mod panel_window;
mod live_api;
mod widget_props;
mod menus;
pub(crate) mod mixer_props;
mod palette;
mod screen_zoom_pointer;
mod settings;
mod source;

use crate::geometry::{self as geo, CORNERS, Corner};
use crate::mixer::{MixerDock, MixerPanel};
use crate::outputs_panel::{OutputsPanel, elapsed};
use crate::rows::{DraggedRow, Renaming, RowTarget};
use crate::safe_areas::{self, Profile};
use crate::studio::Studio;
use crate::ui::{self, tok};
use lightair_engine::outputs::{OutputState, runtime};
use std::time::Instant;

actions!(
    lightair,
    [
        Undo,
        Redo,
        DeleteSelection,
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
        Deselect,
        RenameSelection,
        OpenPalette,
        Take,
        AddMarker,
        SelectAll,
        OpenSettings,
        OpenAudioSettings,
        ToggleRotateTool,
        ToggleMics,
        GroupSelection,
        UngroupSelection,
        CreateEmpty,
        CreateEmptyChild,
        CopySelection,
        CutSelection,
        PasteObjects,
        DuplicateSelection,
        BringToFront,
        SendToBack,
        BringForward,
        SendBackward,
        ToggleObjectVisible,
        ToggleObjectLocked,
        MoveOutOfParent,
        FillCanvas,
        SaveAsPrefab,
        ZoomIn,
        ZoomOut,
        ZoomToFit,
        ZoomToActual,
        Scene1,
        Scene2,
        Scene3,
        Scene4,
        Scene5,
        Scene6,
        Scene7,
        Scene8,
        Scene9,
        ShowLibrary,
        ShowLive,
        ShowEditor
    ]
);

/// The three places (docs/plans/screen-studio.md, "Library, Live, Editor").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Library,
    Live,
    Editor,
}

/// The switcher's shortcuts: ⌃1–⌃3 on macOS (⌘1–⌘9 are scenes); Alt+1–3 on Windows, where
/// Ctrl+1–9 are the scenes.
const PLACE_KEYS: [&str; 3] = if cfg!(windows) { ["alt-1", "alt-2", "alt-3"] } else { ["ctrl-1", "ctrl-2", "ctrl-3"] };

/// Whether a text field has the keyboard. Fields pass on keys they don't use (Backspace with
/// nothing before the cursor, Enter, Esc), which must not then act on the show.
fn typing(window: &Window) -> bool {
    window.context_stack().iter().any(|c| c.contains("Input"))
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(PLACE_KEYS[0], ShowLibrary, Some("Studio")),
        KeyBinding::new(PLACE_KEYS[1], ShowLive, Some("Studio")),
        KeyBinding::new(PLACE_KEYS[2], ShowEditor, Some("Studio")),
    ]);
    cx.bind_keys([
        KeyBinding::new(crate::ui::key("cmd-z"), Undo, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-shift-z"), Redo, Some("Studio")),
        KeyBinding::new("backspace", DeleteSelection, Some("Studio")),
        KeyBinding::new("delete", DeleteSelection, Some("Studio")),
        KeyBinding::new("left", NudgeLeft, Some("Studio")),
        KeyBinding::new("right", NudgeRight, Some("Studio")),
        KeyBinding::new("up", NudgeUp, Some("Studio")),
        KeyBinding::new("down", NudgeDown, Some("Studio")),
        KeyBinding::new("escape", Deselect, Some("Studio")),
        KeyBinding::new("enter", RenameSelection, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-k"), OpenPalette, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-enter"), Take, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-m"), AddMarker, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-a"), SelectAll, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-,"), OpenSettings, Some("Studio")),
        // (never a plain letter: it would take that letter from anything being typed)
        KeyBinding::new(crate::ui::key("cmd-shift-e"), ToggleRotateTool, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-shift-m"), ToggleMics, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-g"), GroupSelection, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-="), ZoomIn, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-+"), ZoomIn, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd--"), ZoomOut, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-0"), ZoomToFit, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-alt-0"), ZoomToActual, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-c"), CopySelection, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-x"), CutSelection, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-v"), PasteObjects, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-d"), DuplicateSelection, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-shift-]"), BringToFront, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-shift-["), SendToBack, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-]"), BringForward, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-["), SendBackward, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-shift-h"), ToggleObjectVisible, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-shift-l"), ToggleObjectLocked, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-shift-g"), UngroupSelection, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-shift-n"), CreateEmpty, Some("Studio")),
        KeyBinding::new("alt-shift-n", CreateEmptyChild, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-1"), Scene1, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-2"), Scene2, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-3"), Scene3, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-4"), Scene4, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-5"), Scene5, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-6"), Scene6, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-7"), Scene7, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-8"), Scene8, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-9"), Scene9, Some("Studio")),
    ]);
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    Move,
    Resize(Corner),
    /// Turn around the box centre.
    Rotate,
    /// ⌥ + corner: crop the source; `size` is the source's pixel size.
    Crop(Corner, (f32, f32)),
    /// Crop mode: an edge handle crops one side.
    CropEdge(geo::Edge, (f32, f32)),
    /// ⌥-drag inside: slides the picture under its crop.
    CropPan((f32, f32)),
    /// The bottom knob: tilt in 3D, both axes.
    Tilt,
    /// A rotate-gizmo ring: tilt around one axis.
    TiltAxis(geo::Axis),
}

/// A handle of the selected object, for hover and drag feedback.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    Gizmo(geo::Axis),
    Rotate,
    Tilt,
    Corner(Corner),
    Edge(geo::Edge),
    /// ⌥ inside a cropped picture: slide it under the crop.
    Pan,
}

impl DragMode {
    /// The handle this drag holds (lit up while dragging).
    fn handle(self, gizmo: bool) -> Option<Handle> {
        Some(match self {
            DragMode::Move => return None,
            DragMode::Resize(c) | DragMode::Crop(c, _) => Handle::Corner(c),
            DragMode::Rotate if gizmo => Handle::Gizmo(geo::Axis::Z),
            DragMode::Rotate => Handle::Rotate,
            DragMode::CropEdge(e, _) => Handle::Edge(e),
            DragMode::CropPan(_) => Handle::Pan,
            DragMode::Tilt => Handle::Tilt,
            DragMode::TiltAxis(a) => Handle::Gizmo(a),
        })
    }

    fn is_crop(self) -> bool {
        matches!(self, DragMode::Crop(..) | DragMode::CropEdge(..) | DragMode::CropPan(..))
    }
}

/// The resize cursor along a direction (degrees, screen space).
fn resize_cursor(degrees: f32) -> CursorStyle {
    match (degrees / 45.0).round().rem_euclid(4.0) as i32 {
        0 => CursorStyle::ResizeLeftRight,
        1 => CursorStyle::ResizeUpLeftDownRight,
        2 => CursorStyle::ResizeUpDown,
        _ => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// How long crop view stays after a crop drag (another crop handle can be grabbed meanwhile),
/// and how long its ghost takes to fade out.
const CROP_LINGER: std::time::Duration = std::time::Duration::from_millis(700);
const CROP_FADE: std::time::Duration = std::time::Duration::from_millis(240);

/// Distance of the rotation handle above the selection, in screen pixels.
const ROTATE_OFFSET: f32 = 22.0;

struct Drag {
    scene: Id,
    object: Id,
    canvas: Id,
    mode: DragMode,
    start: (f32, f32),
    layout: Layout,
    /// Other selected objects moving with it (Move only), with their starting layouts.
    others: Vec<(Id, Layout)>,
    /// Whether the pointer moved (a still click on the 3D knob toggles the rotate gizmo).
    moved: bool,
}

/// Sources' pixel sizes and when they were last read.
type SizeCache = (Option<Instant>, HashMap<Id, (u32, u32)>);

/// A source's list choices: (label, setting value).
type SourceOptions = Vec<(String, serde_json::Value)>;

/// A selection box being dragged: canvas, start, current point (canvas units), adding (⇧).
type Marquee = (Id, (f32, f32), (f32, f32), bool);

/// A crop bar on a side: its two ends on the stage, and whether it is lit.
type CropBar = ((f32, f32), (f32, f32), bool);

/// A rotate-gizmo ring to paint: colour, stroke width, points on the stage.
type Ring = (Hsla, f32, Vec<(f32, f32)>);

/// Snap guides shown while dragging: (vertical?, canvas coordinate).
type Guides = Vec<(bool, f32)>;

pub struct StudioWindow {
    studio: Entity<Studio>,
    mixer: Entity<MixerPanel>,
    /// The mixer's dock panel: its strips and the meter overlay.
    mixer_dock: Entity<MixerDock>,
    outputs: Entity<OutputsPanel>,
    /// Every destination's chat (a tab beside Outputs).
    chat: Entity<crate::chat_panel::ChatPanel>,
    /// Follows, subs, donations and raids (a tab beside Chat).
    activity: Entity<crate::activity_panel::ActivityPanel>,
    /// How each platform shows the stream (a tab beside Activity).
    stream: Entity<crate::stream_panel::StreamPanel>,
    /// Automations (a tab beside the mixer).
    automations_panel: Entity<crate::automations_panel::AutomationsPanel>,
    /// First click on "End stream" arms it; a second click within 3 s ends the stream.
    confirm_end: Option<Instant>,
    /// One preview per canvas (the primary one renders the main canvas).
    previews: HashMap<Id, Preview>,
    /// Library, Live or Editor.
    place: Place,
    library: Entity<crate::library::Library>,
    pub(crate) editor: Entity<crate::editor::Editor>,
    /// A Studio studio_take's countdown: when it began.
    studio_take_countdown: Option<Instant>,
    /// A program preview made for the Library's stills, and the stills it's for.
    thumbnail_shot: Option<(Preview, Vec<crate::studio::ThumbnailRequest>)>,
    /// Screen bounds of each visible canvas's stage, from the last frame.
    stage_bounds: Rc<RefCell<HashMap<Id, Bounds<Pixels>>>>,
    /// The canvas being edited (tabs), or all canvases side by side.
    active_canvas: Option<Id>,
    side_by_side: bool,
    focus: FocusHandle,
    /// Settings, focused while a shortcut is being set (so the keys pressed reach it).
    settings_focus: FocusHandle,
    /// Takes the keys pressed while a global shortcut is being set, before the window's own
    /// shortcuts would.
    _shortcut_keys: Subscription,
    drag: Option<Drag>,
    guides: Guides,
    add_menu: bool,
    /// Inspector sections the user collapsed, slider states, and the source/effect whose looks
    /// dialog is open.
    collapsed: RefCell<std::collections::HashSet<&'static str>>,
    sliders: RefCell<HashMap<String, inspector::BoundSlider>>,
    /// The Effect catalog sheet, when open.
    effect_sheet: Option<effects::EffectSheet>,
    /// What each source's face effects see (polled while it's selected).
    presence: std::collections::HashMap<Id, effects::Presence>,
    /// Effect control groups toggled from their default (first open, the rest closed).
    fx_groups: std::collections::HashSet<String>,
    /// Colour pickers of the selected object's effects, by "effect:key".
    colour_pickers: std::collections::HashMap<String, effects::ColourPicker>,
    /// Colour controls of text, colour objects and widgets.
    colours: colour_field::ColourFields,
    /// A text object being typed into on the canvas.
    canvas_text: Option<canvas_text::CanvasText>,
    /// Open plugin editor windows, by effect.
    plugin_editors: std::collections::HashMap<Id, crate::plugin_editor::Editor>,
    /// The installed plugins (scanned when the audio catalog first opens).
    plugin_list: Option<std::sync::Arc<Vec<lightair_engine::plugins::PluginInfo>>>,
    /// Audio interfaces (for track channel pickers), and when they were read.
    interfaces: RefCell<(std::time::Instant, Vec<lightair_engine::devices::DeviceInfo>)>,
    /// The transition tile playing its preview (under the pointer), and since when.
    transition_hover: Option<(usize, Instant)>,
    /// The object showing crop handles instead of resize handles.
    crop_mode: Option<Id>,
    crop_preview_sent: Option<(Id, Id, Id)>,
    /// The rotate gizmo (Unity-style rings) is showing, and which ring is under the pointer.
    rotate_tool: bool,
    gizmo_hover: Option<geo::Axis>,
    /// The handle under the pointer (nothing dragged).
    hover_handle: Option<Handle>,
    /// The pointer over the canvas: canvas and canvas point (drag readouts sit by it).
    pointer: Option<(Id, (f32, f32))>,
    /// Bumped by every crop drag and exit, so a pending exit can tell it's stale.
    crop_epoch: u64,
    /// Crop view is fading out (its handles are already gone).
    crop_leaving: bool,
    /// Bumped by every grab, restarting the grabbed handle's pop.
    grabs: usize,
    /// Hierarchy rows folded shut.
    collapsed_objects: HashSet<Id>,
    /// Where a row being dragged in the Hierarchy would land.
    tree_drop: Option<(Id, hierarchy::TreeDrop)>,
    /// The assets folder's files, and when they were read.
    asset_cache: RefCell<(Option<Instant>, Vec<assets::Asset>)>,
    selected_asset: Option<PathBuf>,
    /// Canvas zoom (1 = the canvas fits the stage) and pan (screen pixels from centred).
    zoom: f32,
    pan: (f32, f32),
    /// The performance panel under the FPS readout is open.
    perf_open: bool,
    /// The devices connected now (looked at every few seconds) and whether the console is open.
    connected: Option<crate::console::Connected>,
    console_open: bool,
    /// Panels in windows of their own (shell/panel_window), and their windows.
    panel_windows: Vec<(gpui_kit::component::dock::PanelId, AnyWindowHandle, Entity<panel_window::PanelWindow>)>,
    /// macOS's camera effects as last set (`effects::sync_macos_effects`); `None` before the
    /// first look.
    macos_fx: Option<effects::MacosEffects>,
    /// A pan drag: where the pointer started and the pan then.
    panning: Option<(Point<Pixels>, (f32, f32))>,
    /// Space is held: dragging the canvas pans it.
    space_down: bool,
    /// Sources' pixel sizes, refreshed a few times a second: frames and handles hug the picture.
    sizes: RefCell<SizeCache>,
    /// The source picker, and fetched source lists per (source, setting).
    source_picker: Option<source::SourcePicker>,
    source_cache: RefCell<HashMap<(Id, &'static str), SourceOptions>>,
    /// The Add Object picker, while open.
    add_object: Option<add_object::AddObject>,
    /// Camera devices (label, id), refreshed when the Add Object menu opens.
    camera_devices: Vec<(String, String)>,
    /// The open inline rename field in the Objects or Scenes list.
    renaming: Option<Renaming>,
    /// Studio mode (docs/07-scenes-objects-canvas.md): the stage shows the selected scene, a
    /// monitor shows Program, and Take puts the selected scene on air.
    cue_scenes: bool,
    program_monitor: Option<Preview>,
    /// Inspector content sections (text field, media status).
    props: object_props::ObjectProps,
    /// Safe-area overlay shown on landscape and on vertical stages.
    safe_landscape: Option<Profile>,
    safe_vertical: Option<Profile>,
    /// The open command palette (⌘K).
    palette: Option<palette::Palette>,
    /// The Go Live review sheet, while open.
    go_live_sheet: Option<golive::GoLiveSheet>,
    /// A drag-to-select box on the canvas: canvas, start and current point (canvas units), and
    /// whether it adds to the selection (⇧).
    marquee: Option<Marquee>,
    /// The Add canvas row, with its custom width and height fields.
    settings_open: bool,
    settings_page: settings::Page,
    /// A canvas being edited in Settings › Canvases.
    canvas_edit: Option<settings::CanvasEdit>,
    /// Settings › Shortcuts is waiting for the keys of this global shortcut.
    shortcut_capture: Option<crate::shortcuts::GlobalAction>,
    shortcut_note: Option<String>,
    /// Monitoring devices, read when the Audio page opens.
    headphones_devices: Vec<(String, String)>,
    prefs: crate::prefs::Preferences,
    /// The remote API (obs-websocket, JSON-RPC, MCP).
    remote: remote::Remote,
    /// Automations (docs/plans/live-integrations.md, 2.6).
    pub(crate) automations: lightair_automation::Automations,
    /// The Permissions page is being refreshed.
    permissions_watch: std::cell::Cell<bool>,
    /// Heart rate being read (Pulsoid): set to stop it.
    heart_rate: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// What pairing with a Hue bridge is doing (None: idle).
    hue_note: Option<String>,
    /// What an official sign-in is doing or said, per platform.
    oauth_notes: std::collections::HashMap<String, String>,
    /// The canvas preview drawn under the UI (macOS: native_preview), the canvases whose
    /// previews show in it, and where each canvas's picture is (window points), as laid out.
    native: std::rc::Rc<RefCell<Option<crate::native_preview::NativePreview>>>,
    native_layers: HashSet<Id>,
    native_rects: std::rc::Rc<RefCell<HashMap<Id, Bounds<Pixels>>>>,
    /// Resizable, collapsible, rearrangeable panels (built on first render).
    dock: Option<Entity<gpui_kit::component::dock::DockArea>>,
    canvas_menu:
        Option<(Entity<gpui_kit::component::input::InputState>, Entity<gpui_kit::component::input::InputState>)>,
}

const HANDLE: f32 = 10.0;
/// How close (screen px) the pointer must be to a gizmo ring.
const GIZMO_TOL: f32 = 6.0;

/// The rotate gizmo's radius in canvas units: most of the picture, but never tiny on screen.
fn gizmo_radius(l: &Layout, scale: f32) -> f32 {
    (l.width.min(l.height) * 0.42).max(56.0 / scale)
}
const SNAP_PX: f32 = 8.0;


/// A new display capture: the main display (macOS names it; Windows takes the primary one), with
/// the pointer, without Lightair's own windows.
pub(crate) fn display_settings() -> serde_json::Value {
    let mut settings = json!({"mode": lightair_core::kinds::DISPLAY, "cursor": true, "hide_obs": true});
    if let Some(uuid) = lightair_engine::displays::main_display_uuid() {
        settings["display"] = uuid.into();
    }
    settings
}

impl StudioWindow {
    pub fn new(studio: Entity<Studio>, preview: Preview, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_in(&studio, window, |this, studio, window, cx| {
            this.sync_studio_preview(cx);
            this.prune_plugin_editors(cx);
            this.tell_remote(cx);
            if let Some(req) = studio.update(cx, |s, _| s.ui_request.take()) {
                this.take_ui_request(req, window, cx);
            }
            cx.notify()
        })
        .detach();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Shortcuts that work in any app: registered now, handled as they come.
        let saved = crate::prefs::load_from(&crate::prefs::path());
        crate::shortcuts::register_all(&saved.global_shortcuts.iter().map(|h| (h.action, h.keys.clone())).collect::<Vec<_>>());
        // a newer Lightair: looked for at launch and every few hours (Settings › About)
        crate::updater::start(|| crate::prefs::load_from(&crate::prefs::path()).check_updates);
        // Face effects' "no face in view" hints.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                if this
                    .update(cx, |this, cx| {
                        this.refresh_presence(cx);
                        // (also when the window isn't drawing)
                        this.take_thumbnails(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        // What changes on its own (clocks, thumbnails coming in, a countdown) is redrawn at a
        // pace, not at every display refresh: live pictures redraw their own panels when they
        // have a frame (docking), meters theirs (mixer).
        cx.spawn(async move |this, cx| {
            let mut n = 0u32;
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(33)).await;
                n = n.wrapping_add(1);
                let alive = this.update(cx, |this, cx| {
                    if this.moving() || n.is_multiple_of(15) {
                        cx.notify();
                    }
                    // the preview under the UI follows its pictures' new sizes (Windows)
                    if let Some(n) = this.native.borrow_mut().as_mut() {
                        n.refit();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        // Chromium for web page sources: driven from this (the main) thread, often while pages
        // are open, and now and then to pick up the first one asked for.
        // It runs when Chromium asks (CEF's external pump): a page that animates asks often, a
        // still page now and then, none never.
        let (wake, mut woken) = futures_channel::mpsc::unbounded::<()>();
        let wake = parking_lot::Mutex::new(wake);
        lightair_web::set_waker(move || {
            let _ = wake.lock().unbounded_send(());
        });
        cx.spawn(async move |this, cx| {
            use futures_util::StreamExt;
            loop {
                lightair_web::pump();
                let wait = lightair_web::next_pump();
                if !wait.is_zero() {
                    let timer = cx.background_executor().timer(wait);
                    let _ = futures_util::future::select(timer, woken.next()).await;
                }
                while woken.try_recv().is_ok() {}
                if this.upgrade().is_none() {
                    break;
                }
            }
        })
        .detach();
        // The badge by the real pointer while viewers see the screen zoomed in (on the user's
        // screen only): following the pointer every frame while shown.
        cx.spawn(async move |this, cx| {
            let mut badge: Option<lightair_platform::pointer_badge::Badge> = None;
            loop {
                let zoom = lightair_engine::pointer::live_zoom();
                if zoom.is_some() && badge.is_none() {
                    badge = lightair_platform::pointer_badge::Badge::new();
                }
                if let Some(b) = &mut badge {
                    b.show(zoom);
                }
                cx.background_executor().timer(std::time::Duration::from_millis(if zoom.is_some() { 16 } else { 100 })).await;
                if this.upgrade().is_none() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(40)).await;
                // macOS's camera effects follow the scene on air (at once, not on a slow timer)
                if this.update(cx, |this, cx| this.sync_macos_effects(cx)).is_err() {
                    break;
                }
                let pressed = crate::shortcuts::take_pressed();
                if pressed.is_empty() {
                    continue;
                }
                let alive = this.update_in(cx, |this, window, cx| {
                    for action in pressed {
                        this.global_action(action, window, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let native: crate::native_preview::Shared = Default::default();
        let mixer = cx.new(|cx| MixerPanel::new(studio.clone(), native.clone(), cx));
        let mixer_dock = cx.new(|cx| MixerDock::new(mixer.clone(), cx));
        let outputs = cx.new(|cx| OutputsPanel::new(studio.clone(), cx));
        let chat = cx.new(|cx| crate::chat_panel::ChatPanel::new(studio.clone(), window, cx));
        let activity = cx.new(|cx| crate::activity_panel::ActivityPanel::new(studio.clone(), cx));
        let stream = cx.new(|cx| crate::stream_panel::StreamPanel::new(studio.clone(), cx));
        // Its Add destination dialog is drawn over the whole window.
        cx.observe(&outputs, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&outputs, |this, _, _: &crate::outputs_panel::AddVerticalCanvas, cx| {
            this.add_canvas(Canvas::vertical(), cx)
        })
        .detach();
        cx.subscribe(&outputs, |this, _, _: &crate::outputs_panel::OpenRecordingSettings, cx| {
            this.open_settings(settings::Page::Recording, cx)
        })
        .detach();
        let primary = studio.read(cx).workspace().primary_canvas().id;
        let library = cx.new(|cx| crate::library::Library::new(studio.clone(), window, cx));
        cx.observe(&library, |this, library, cx| {
            if let Some(dir) = library.update(cx, |l, _| l.open_workspace.take()) {
                this.open_workspace(dir, cx);
            }
            if let Some(name) = library.update(cx, |l, _| l.new_workspace.take()) {
                this.new_workspace(&name, cx);
            }
            if let Some((_, id)) = library.update(cx, |l, _| l.open_in_editor.take()) {
                this.show_place(Place::Editor, cx);
                this.editor.update(cx, |e, cx| e.add_item(id, cx));
            }
            if let Some(ids) = library.update(cx, |l, _| l.open_many.take()) {
                this.show_place(Place::Editor, cx);
                this.editor.update(cx, |e, cx| {
                    for id in ids {
                        e.add_item(id, cx);
                    }
                });
            }
            if let Some(parts) = library.update(cx, |l, _| l.open_parts.take()) {
                this.show_place(Place::Editor, cx);
                this.editor.update(cx, |e, cx| e.add_parts(parts, cx));
            }
            cx.notify();
        })
        .detach();
        let remote = Self::new_remote(window, cx);
        let automations = live_api::start_automations(&studio.read(cx).chat, remote.jobs());
        live_api::feed_widgets(automations.clone(), studio.read(cx).widgets.clone());
        let automations_panel = {
            let a = automations.clone();
            let w = studio.read(cx).widgets.clone();
            cx.new(|cx| crate::automations_panel::AutomationsPanel::new(a, w, cx))
        };
        cx.defer_in(window, |this, _, cx| {
            this.sync_remote(cx);
            this.load_connections(cx);
            // what's connected (for the console): looked at every few seconds
            this.watch_devices(cx);
        });
        Self {
            previews: HashMap::from([(primary, preview)]),
            place: Place::Live,
            library,
            editor: {
                let (s, n) = (studio.clone(), native.clone());
                cx.new(|_| {
                    let mut e = crate::editor::Editor::new(s);
                    e.native = n;
                    e
                })
            },
            thumbnail_shot: None,
            studio_take_countdown: None,
            stage_bounds: Rc::default(),
            active_canvas: Some(primary),
            side_by_side: false,
            mixer,
            mixer_dock,
            outputs,
            chat,
            activity,
            stream,
            confirm_end: None,
            studio,
            focus,
            settings_focus: cx.focus_handle(),
            _shortcut_keys: {
                let me = cx.entity().downgrade();
                cx.intercept_keystrokes(move |ev, window, cx| {
                    let Some(me) = me.upgrade() else { return };
                    let taken = me.update(cx, |this, cx| {
                        this.shortcut_capture?;
                        this.capture_shortcut(&ev.keystroke, cx);
                        Some((this.shortcut_capture.is_none(), this.focus.clone()))
                    });
                    if let Some((done, focus)) = taken {
                        cx.stop_propagation();
                        // set (or cancelled): the window's keys again
                        if done {
                            window.focus(&focus, cx);
                        }
                    }
                })
            },
            drag: None,
            guides: vec![],
            add_menu: false,
            add_object: None,
            collapsed: RefCell::new(["Canvases"].into_iter().collect()),
            sliders: RefCell::default(),
            effect_sheet: None,
            presence: Default::default(),
            fx_groups: Default::default(),
            colour_pickers: Default::default(),
            colours: Default::default(),
            canvas_text: None,
            plugin_editors: Default::default(),
            plugin_list: None,
            interfaces: RefCell::new((std::time::Instant::now(), Vec::new())),
            crop_mode: None,
            transition_hover: None,
            crop_preview_sent: None,
            rotate_tool: false,
            gizmo_hover: None,
            hover_handle: None,
            pointer: None,
            crop_epoch: 0,
            crop_leaving: false,
            grabs: 0,
            collapsed_objects: HashSet::new(),
            tree_drop: None,
            asset_cache: RefCell::new((None, Vec::new())),
            selected_asset: None,
            zoom: 1.0,
            pan: (0.0, 0.0),
            macos_fx: None,
            perf_open: false,
            connected: None,
            console_open: false,
            panel_windows: Vec::new(),
            panning: None,
            space_down: false,
            sizes: RefCell::default(),
            source_picker: None,
            source_cache: RefCell::default(),
            camera_devices: vec![],
            renaming: None,
            cue_scenes: false,
            program_monitor: None,
            props: Default::default(),
            safe_landscape: None,
            safe_vertical: None,
            palette: None,
            go_live_sheet: None,
            marquee: None,
            canvas_menu: None,
            settings_open: false,
            settings_page: settings::Page::General,
            canvas_edit: None,
            shortcut_capture: None,
            shortcut_note: None,
            headphones_devices: Vec::new(),
            dock: None,
            prefs: crate::prefs::load_from(&crate::prefs::path()),
            remote,
            automations,
            heart_rate: None,
            hue_note: None,
            oauth_notes: Default::default(),
            permissions_watch: Default::default(),
            automations_panel,
            native,
            native_layers: HashSet::new(),
            native_rects: Default::default(),
        }
    }

    pub(crate) fn exec(&self, cmd: Command, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            let _ = s.execute(cmd, cx);
        });
    }

    fn start_rename(&mut self, target: RowTarget, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = Some(Renaming::open(target, name, window, cx, |this, commit, window, cx| {
            this.finish_rename(commit, window, cx)
        }));
        cx.notify();
    }

    fn finish_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else { return };
        if commit && let Some(cmd) = renaming.command(cx) {
            self.exec(cmd, cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Enter renames the selected object (or the selected scene when no object is selected).
    fn rename_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        match studio.selected_object.and_then(|id| scene.object(id)) {
            Some(object) => {
                let target = RowTarget::Object { scene: scene.id, object: object.id };
                let name = object.name.clone();
                self.start_rename(target, &name, window, cx)
            }
            None => {
                let name = scene.name.clone();
                self.start_rename(RowTarget::Scene(scene.id), &name, window, cx)
            }
        }
    }

    /// Drops `row` onto the row at model index `to` of the same list.
    fn drop_row(&mut self, row: &DraggedRow, onto: RowTarget, to: usize, cx: &mut Context<Self>) {
        let cmd = match (row.target, onto) {
            (RowTarget::Scene(scene), RowTarget::Scene(other)) if scene != other => Command::MoveScene { scene, to },
            (RowTarget::Object { scene, object }, RowTarget::Object { scene: s2, object: other })
                if scene == s2 && object != other =>
            {
                Command::MoveObject { scene, object, to }
            }
            _ => return,
        };
        self.exec(cmd, cx);
    }

    /// A row's name, or the rename field when it's being renamed.
    fn row_label(&self, target: RowTarget, name: &str, color: Hsla) -> AnyElement {
        match &self.renaming {
            Some(r) if r.target == target => div()
                .flex_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(gpui_kit::component::input::Input::new(&r.input).small())
                .into_any_element(),
            _ => div()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(color)
                .child(name.to_string())
                .into_any_element(),
        }
    }

    /// Selects just this object (or nothing).
    fn select_object(&mut self, object: Option<Id>, cx: &mut Context<Self>) {
        if self.crop_mode.is_some() && self.crop_mode != object {
            self.leave_crop_view(std::time::Duration::ZERO, cx);
        }
        self.studio.update(cx, |s, cx| {
            s.selected_object = object;
            s.also_selected.clear();
            if object.is_some() {
                s.selected_track = None;
            }
            cx.notify();
        });
    }

    fn toggle_selected(&self, object: Id, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            s.toggle_selected(object);
            cx.notify();
        });
    }

    /// One command for several objects' layouts (a plain SetLayout for one).
    /// `layouts` are frames in canvas terms (as `frame_of` gives them); each is stored in its
    /// object's parent space. An empty moves its pivot by as much as its frame moved.
    fn set_layouts(&self, scene: Id, canvas: Id, layouts: Vec<(Id, Layout)>, label: &str, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(sc) = workspace.scene(scene) else { return };
        let mut commands: Vec<Command> = layouts
            .into_iter()
            .filter_map(|(object, frame)| {
                let l = sc.object(object)?;
                let world = if l.is_empty() {
                    let now = self.frame_of(sc, l, canvas)?;
                    let pivot = sc.world(object, canvas)?;
                    let (dx, dy) = (frame.x - now.x, frame.y - now.y);
                    Layout { x: pivot.x + dx, y: pivot.y + dy, ..pivot }
                } else {
                    frame
                };
                let layout = sc.local(object, canvas, &world)?;
                Some(Command::SetLayout { scene, object, canvas, layout })
            })
            .collect();
        match commands.len() {
            0 => {}
            1 => self.exec(commands.remove(0), cx),
            _ => self.exec(Command::Batch { label: label.into(), commands }, cx),
        }
    }

    fn canvas_id(&self, workspace: &Workspace) -> Id {
        self.active_canvas.filter(|f| workspace.canvas(*f).is_some()).unwrap_or(workspace.primary_canvas().id)
    }

    fn current_canvas<'a>(&self, workspace: &'a Workspace) -> &'a Canvas {
        workspace.canvas(self.canvas_id(workspace)).unwrap_or(workspace.primary_canvas())
    }

    /// Canvases shown on screen: the active one, or all of them side by side.
    fn visible_canvases(&self, workspace: &Workspace) -> Vec<Canvas> {
        if self.side_by_side && workspace.canvases.len() > 1 {
            workspace.canvases.clone()
        } else {
            vec![self.current_canvas(workspace).clone()]
        }
    }

    /// Canvas-space mapping for a canvas's stage: (origin x, origin y, scale) in window pixels.
    fn mapping(&self, workspace: &Workspace, canvas: Id) -> Option<(f32, f32, f32)> {
        let b = *self.stage_bounds.borrow().get(&canvas)?;
        let f = workspace.canvas(canvas)?;
        let (cw, ch) = (f.width as f32, f.height as f32);
        let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
        let s = (bw / cw).min(bh / ch) * self.zoom;
        Some((
            f32::from(b.origin.x) + (bw - cw * s) / 2.0 + self.pan.0,
            f32::from(b.origin.y) + (bh - ch * s) / 2.0 + self.pan.1,
            s,
        ))
    }

    /// Zooms by `factor`, keeping the canvas point under `anchor` (a window point; `None`: the
    /// middle of the stage) where it is.
    fn zoom_by(&mut self, factor: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let canvas = anchor.and_then(|a| self.stage_at(&workspace, a)).unwrap_or_else(|| self.canvas_id(&workspace));
        let Some(b) = self.stage_bounds.borrow().get(&canvas).copied() else { return };
        let Some(f) = workspace.canvas(canvas) else { return };
        let (cw, ch) = (f.width as f32, f.height as f32);
        let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
        let fit = (bw / cw).min(bh / ch);
        let anchor = anchor.unwrap_or(b.center());
        let Some((ox, oy, s)) = self.mapping(&workspace, canvas) else { return };
        let (ax, ay) = (f32::from(anchor.x), f32::from(anchor.y));
        let (px_, py_) = ((ax - ox) / s, (ay - oy) / s);
        let zoom = (self.zoom * factor).clamp(0.1, 16.0);
        let s2 = fit * zoom;
        // Where the canvas origin lands centred at the new zoom, then shift so the point stays.
        let (bx, by) = (f32::from(b.origin.x) + (bw - cw * s2) / 2.0, f32::from(b.origin.y) + (bh - ch * s2) / 2.0);
        self.zoom = zoom;
        self.pan = (ax - bx - px_ * s2, ay - by - py_ * s2);
        self.clamp_pan(&workspace);
        cx.notify();
    }

    /// Keeps the pan inside its bounds (geometry::clamp_pan): the canvas never leaves the view.
    fn clamp_pan(&mut self, workspace: &Workspace) {
        let canvas = self.canvas_id(workspace);
        let Some(b) = self.stage_bounds.borrow().get(&canvas).copied() else { return };
        let Some(f) = workspace.canvas(canvas) else { return };
        let (cw, ch) = (f.width as f32, f.height as f32);
        let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
        let s = (bw / cw).min(bh / ch) * self.zoom;
        self.pan = (geo::clamp_pan(self.pan.0, bw, cw * s), geo::clamp_pan(self.pan.1, bh, ch * s));
    }

    fn zoom_to_fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
        cx.notify();
    }

    /// One canvas pixel to one screen point.
    fn zoom_to_actual(&mut self, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let canvas = self.canvas_id(&workspace);
        let Some((_, _, s)) = self.mapping(&workspace, canvas) else { return };
        self.zoom_by(1.0 / s, None, cx);
    }

    /// The current scale as a percentage (100: one canvas pixel per screen point).
    fn zoom_percent(&self, workspace: &Workspace) -> f32 {
        self.mapping(workspace, self.canvas_id(workspace)).map_or(100.0, |(_, _, s)| s * 100.0)
    }

    fn canvas_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        // a sheet over the window: its scrolling is its own (the sheets block the mouse too)
        if self.sheet_open(cx) {
            return;
        }
        let delta = match ev.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
            ScrollDelta::Lines(p) => (p.x * 20.0, p.y * 20.0),
        };
        if ev.modifiers.platform || ev.modifiers.control {
            // ⌘/Ctrl + scroll zooms at the pointer.
            self.zoom_by((delta.1 * 0.01).exp(), Some(ev.position), cx);
        } else {
            self.pan = (self.pan.0 + delta.0, self.pan.1 + delta.1);
            let workspace = self.studio.read(cx).workspace().clone();
            self.clamp_pan(&workspace);
            cx.notify();
        }
    }

    fn canvas_pinch(&mut self, ev: &PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.sheet_open(cx) {
            return;
        }
        self.zoom_by(1.0 + ev.delta, Some(ev.position), cx);
    }

    /// Whether a sheet is over the window (settings, a dialog, a picker, the palette).
    fn sheet_open(&self, cx: &App) -> bool {
        self.settings_open
            || self.palette.is_some()
            || self.add_object.is_some()
            || self.effect_sheet.is_some()
            || self.go_live_sheet.is_some()
            || self.source_picker.is_some()
            || self.outputs.read(cx).dialog_open()
    }

    fn to_canvas(&self, workspace: &Workspace, canvas: Id, p: Point<Pixels>) -> Option<(f32, f32)> {
        let (ox, oy, s) = self.mapping(workspace, canvas)?;
        Some(((f32::from(p.x) - ox) / s, (f32::from(p.y) - oy) / s))
    }

    /// The visible canvas whose stage contains the pointer.
    fn stage_at(&self, workspace: &Workspace, p: Point<Pixels>) -> Option<Id> {
        let bounds = self.stage_bounds.borrow();
        self.visible_canvases(workspace)
            .into_iter()
            .map(|f| f.id)
            .find(|id| bounds.get(id).is_some_and(|b| b.contains(&p)))
    }

    fn set_cue_scenes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.cue_scenes = on;
        self.program_monitor = on.then(Preview::attach);
        if let Some(m) = &self.program_monitor {
            let f = self.studio.read(cx).workspace().primary_canvas().clone();
            m.set_size(Some((512, 512 * f.height / f.width.max(1))));
        }
        self.sync_studio_preview(cx);
        cx.notify();
    }

    /// In Studio mode the primary stage shows the selected scene rather than Program.
    fn sync_studio_preview(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let canvas = studio.workspace().primary_canvas().id;
        let Some(preview) = self.previews.get(&canvas) else { return };
        let want = studio.selected_scene.filter(|_| self.cue_scenes);
        let current = preview.source().map(|p| p as usize);
        let engine = studio.engine().clone();
        let next = match want {
            None => None,
            Some(scene) => {
                // Only studio_take a new reference when the libobs scene changed (rebuilds replace it).
                let ptr = engine.call(move |s| s.reconciler.scene_source(scene, canvas).map(|p| p as usize));
                if ptr == current {
                    return;
                }
                engine.call(move |s| s.reconciler.scene_source_ref(scene, canvas).map(|p| p as usize))
            }
        };
        if next.is_some() || current.is_some() {
            preview.set_source(next.map(|p| p as *mut _));
        }
    }

    /// Studio mode's Take: the selected scene goes on air with the current transition.
    /// A global shortcut was pressed (in any app).
    fn global_action(&mut self, action: crate::shortcuts::GlobalAction, window: &mut Window, cx: &mut Context<Self>) {
        use crate::shortcuts::GlobalAction as A;
        match action {
            A::Automation(i) => {
                if let Some(id) = crate::shortcuts::automation_for(i)
                    && let Err(e) = self.automations.run(&id, None, &[])
                {
                    tracing::warn!(%e, "hotkey automation");
                }
            }
            A::ToggleRecording => self.studio.update(cx, |s, cx| s.toggle_recording(cx)),
            A::ToggleLive => {
                if runtime().any_stream_running() {
                    self.studio.update(cx, |s, cx| s.end_stream(cx))
                } else {
                    self.studio.update(cx, |s, cx| s.go_live(cx))
                }
            }
            A::Take => self.take(cx),
            A::StudioTake => self.studio_take(cx),
            A::AddMarker => self.studio.update(cx, |s, cx| s.add_marker(cx)),
            A::SaveClip => self.studio.update(cx, |s, cx| s.save_clip(cx)),
            A::MuteMics => self.studio.update(cx, |s, cx| s.toggle_mics(cx)),
            A::FollowZoom => {
                lightair_engine::pointer::toggle_follow();
                cx.notify();
            }
            A::Scene(n) => self.scene_shortcut(n.saturating_sub(1) as usize, window, cx),
        }
    }

    fn take(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let Some(scene) = studio.selected_scene else { return };
        if studio.workspace().program == Some(scene) {
            return;
        }
        self.exec(Command::SetProgram { scene: Some(scene) }, cx);
    }

    /// Creates previews for new canvases and drops those of removed ones.
    fn sync_previews(&mut self, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        self.previews.retain(|id, _| workspace.canvas(*id).is_some());
        for f in &workspace.canvases {
            if self.previews.contains_key(&f.id) {
                continue;
            }
            let id = f.id;
            let canvas = self.studio.read(cx).engine().call(move |s| s.reconciler.canvas_ref(id).map(|p| p as usize));
            if let Some(ptr) = canvas {
                self.previews.insert(id, Preview::attach_canvas(ptr as *mut _));
            }
        }
    }

    /// Adds a vertical canvas with every object auto-reframed (one undo step).
    pub(super) fn add_vertical(&mut self, cx: &mut Context<Self>) {
        self.add_canvas(Canvas::vertical(), cx)
    }

    /// Adds a canvas with every object reframed into it (one undo step), and shows both.
    fn add_canvas(&mut self, to: Canvas, cx: &mut Context<Self>) {
        self.canvas_menu = None;
        let workspace = self.studio.read(cx).workspace().clone();
        let from = workspace.primary_canvas().clone();
        let to_id = to.id;
        let label = format!("Add {} format", to.name.to_lowercase());
        let mut commands = vec![Command::AddCanvas { canvas: to.clone() }];
        for scene in &workspace.scenes {
            for object in &scene.objects {
                if let Some(l) = object.layouts.get(&from.id) {
                    let layout = crate::reframe::reframe(l, &from, &to);
                    commands.push(Command::SetLayout { scene: scene.id, object: object.id, canvas: to_id, layout });
                }
            }
        }
        self.exec(Command::Batch { label, commands }, cx);
        self.active_canvas = Some(to_id);
        self.side_by_side = true;
    }

    // ------------------------------------------------------------ canvas interaction
    fn canvas_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // a click outside the text being typed: done typing
        if self.canvas_text.is_some() {
            self.finish_canvas_text(true, window, cx);
        }
        window.focus(&self.focus, cx);
        if self.space_down {
            self.panning = Some((ev.position, self.pan));
            return;
        }
        self.add_menu = false;
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(canvas) = self.stage_at(&workspace, ev.position) else { return };
        self.active_canvas = Some(canvas);
        let studio = self.studio.read(cx);
        let (Some(scene_id), Some((x, y)), Some((_, _, s))) =
            (studio.selected_scene, self.to_canvas(&workspace, canvas, ev.position), self.mapping(&workspace, canvas))
        else {
            return;
        };
        let Some(scene) = workspace.scene(scene_id) else { return };

        // A handle of the selected object? (See `handle_at`.)
        if let Some(sel) = studio.selected_object.and_then(|id| scene.object(id))
            && let Some(frame) = self.frame_of(scene, sel, canvas).filter(|_| !scene.locked(sel.id) && !sel.is_empty())
        {
            // The frame hugs the picture: handles sit on it, and a drag snaps the box to it.
            let l = &frame;
            let size =
                sel.source.and_then(|i| self.sizes.borrow().1.get(&i).copied()).map(|(w, h)| (w as f32, h as f32));
            let handle = self.handle_at(l, (x, y), s, ev.modifiers.alt, size.is_some());
            if handle == Some(Handle::Tilt) && ev.click_count == 2 {
                // Double-click the 3D knob: flat again.
                let stored = sel.layouts[&canvas];
                let layout = Layout { tilt_x: 0.0, tilt_y: 0.0, ..stored };
                self.exec(Command::SetLayout { scene: scene_id, object: sel.id, canvas, layout }, cx);
                return;
            }
            let mode = handle.and_then(|h| {
                Some(match h {
                    Handle::Gizmo(geo::Axis::Z) | Handle::Rotate => DragMode::Rotate,
                    Handle::Gizmo(a) => DragMode::TiltAxis(a),
                    Handle::Tilt => DragMode::Tilt,
                    Handle::Corner(c) => match (ev.modifiers.alt, size) {
                        (true, Some(sz)) => DragMode::Crop(c, sz),
                        _ => DragMode::Resize(c),
                    },
                    Handle::Edge(e) => DragMode::CropEdge(e, size?),
                    Handle::Pan => DragMode::CropPan(size?),
                })
            });
            if let Some(mode) = mode {
                // Cropping shows the whole picture, the cut-away part fading in as a ghost.
                if mode.is_crop() {
                    self.crop_epoch += 1;
                    if self.crop_leaving {
                        // Grabbed again while fading out: fade back in.
                        self.crop_leaving = false;
                        self.crop_preview_sent = None;
                    }
                    self.crop_mode = Some(sel.id);
                }
                self.grabs += 1;
                self.hover_handle = None;
                self.drag = Some(Drag {
                    scene: scene_id,
                    object: sel.id,
                    canvas,
                    mode,
                    start: (x, y),
                    layout: *l,
                    others: vec![],
                    moved: false,
                });
                self.studio.update(cx, |s, _| s.begin_gesture());
                return;
            }
        }
        // Topmost visible picture under the pointer (the deepest object: a child, not the empty
        // holding it). A selected empty is grabbed anywhere in the box round its objects.
        let selected_empty = studio
            .selected_object
            .and_then(|id| scene.object(id))
            .filter(|l| l.is_empty() && !scene.locked(l.id))
            .filter(|l| self.frame_of(scene, l, canvas).is_some_and(|lo| geo::contains(&lo, (x, y))));
        let hit =
            selected_empty.or_else(|| {
                scene.objects.iter().rev().filter(|l| !l.is_empty()).find(|l| {
                    self.frame_of(scene, l, canvas).is_some_and(|lo| lo.visible && geo::contains(&lo, (x, y)))
                })
            });
        // double-click a text object: type into it right there
        if ev.click_count == 2
            && let Some(object) = hit.filter(|o| !scene.locked(o.id))
            && let Some(src) = object.source.and_then(|i| workspace.sources.get(&i)).filter(|i| object_props::is_text(&i.kind))
        {
            let (object, source) = (object.id, src.id);
            self.select_object(Some(object), cx);
            self.start_canvas_text(object, source, canvas, window, cx);
            return;
        }
        let selection = studio.selection();
        match hit {
            // ⇧ or ⌘: add or studio_take away
            Some(object) if ev.modifiers.shift || ev.modifiers.platform => self.toggle_selected(object.id, cx),
            Some(object) => {
                let id = object.id;
                // Dragging one of several selected objects moves them all; otherwise select it.
                let in_selection = selection.contains(&id);
                if !scene.locked(object.id) {
                    let Some(layout) = self.frame_of(scene, object, canvas) else { return };
                    let others: Vec<(Id, Layout)> = if in_selection {
                        selection
                            .iter()
                            .filter(|o| **o != id)
                            .filter_map(|o| scene.object(*o).filter(|l| !scene.locked(l.id)))
                            // Not inside another moving object: it moves with that one.
                            .filter(|l| !selection.iter().any(|a| scene.is_under(l.id, *a)))
                            .filter_map(|l| self.frame_of(scene, l, canvas).map(|lo| (l.id, lo)))
                            .collect()
                    } else {
                        vec![]
                    };
                    let mode = DragMode::Move;
                    self.drag = Some(Drag {
                        scene: scene_id,
                        object: id,
                        canvas,
                        mode,
                        start: (x, y),
                        layout,
                        others,
                        moved: false,
                    });
                    self.studio.update(cx, |s, _| s.begin_gesture());
                }
                if in_selection {
                    // Keep the group; the grabbed object becomes the Inspector's.
                    self.studio.update(cx, |s, cx| {
                        if s.selected_object != Some(id) {
                            s.also_selected.retain(|l| *l != id);
                            if let Some(prev) = s.selected_object.replace(id) {
                                s.also_selected.insert(0, prev);
                            }
                        }
                        cx.notify();
                    });
                } else {
                    self.select_object(Some(id), cx);
                }
            }
            // Empty space: start a selection box (⇧ adds to the selection).
            None => {
                let adding = ev.modifiers.shift || ev.modifiers.platform;
                if !adding {
                    self.select_object(None, cx);
                }
                self.marquee = Some((canvas, (x, y), (x, y), adding));
            }
        }
    }

    fn canvas_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some((from, pan)) = self.panning {
            let d = ev.position - from;
            self.pan = (pan.0 + f32::from(d.x), pan.1 + f32::from(d.y));
            let workspace = self.studio.read(cx).workspace().clone();
            self.clamp_pan(&workspace);
            cx.notify();
            return;
        }
        if let Some((canvas, ..)) = self.marquee {
            let workspace = self.studio.read(cx).workspace().clone();
            if let Some(p) = self.to_canvas(&workspace, canvas, ev.position)
                && let Some((_, _, to, _)) = &mut self.marquee
            {
                *to = p;
                cx.notify();
            }
            return;
        }
        {
            let workspace = self.studio.read(cx).workspace().clone();
            let canvas = self.drag.as_ref().map(|d| d.canvas).unwrap_or_else(|| self.canvas_id(&workspace));
            self.pointer = self.to_canvas(&workspace, canvas, ev.position).map(|p| (canvas, p));
        }
        // The handle under the pointer (lit up, with a matching cursor) when nothing is dragged.
        if self.drag.is_none() {
            let workspace = self.studio.read(cx).workspace().clone();
            let canvas = self.canvas_id(&workspace);
            let studio = self.studio.read(cx);
            let hover = studio.selected_scene.and_then(|sid| workspace.scene(sid)).and_then(|scene| {
                let object = studio.selected_object.and_then(|id| scene.object(id)).filter(|l| !scene.locked(l.id))?;
                if object.is_empty() {
                    return None;
                }
                let frame = self.frame_of(scene, object, canvas)?;
                let (_, _, s) = self.mapping(&workspace, canvas)?;
                let p = self.pointer.map(|(_, p)| p)?;
                let has_size = object.source.is_some_and(|i| self.sizes.borrow().1.contains_key(&i));
                self.handle_at(&frame, p, s, ev.modifiers.alt, has_size)
            });
            let gizmo = match hover {
                Some(Handle::Gizmo(a)) => Some(a),
                _ => None,
            };
            if hover != self.hover_handle || gizmo != self.gizmo_hover {
                self.hover_handle = hover;
                self.gizmo_hover = gizmo;
                cx.notify();
            }
            return;
        }
        if let Some(d) = &mut self.drag {
            d.moved = true;
        }
        let Some(drag) = &self.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let workspace = self.studio.read(cx).workspace().clone();
        let (Some((x, y)), Some((_, _, s))) =
            (self.to_canvas(&workspace, drag.canvas, ev.position), self.mapping(&workspace, drag.canvas))
        else {
            return;
        };
        let (dx, dy) = (x - drag.start.0, y - drag.start.1);
        let Some(f) = workspace.canvas(drag.canvas) else { return };
        let (cw, ch) = (f.width as f32, f.height as f32);
        let mut l = drag.layout;
        let mut guides = Guides::new();
        // A web page's box is its window: resizing it resizes the page (any shape), so the
        // picture stays one page pixel a canvas pixel instead of scaling.
        let web = self.web_source_of(&workspace, drag.scene, drag.object);
        match drag.mode {
            DragMode::Rotate => {
                let (cx_, cy_) = (l.x + l.width / 2.0, l.y + l.height / 2.0);
                let mut angle = (y - cy_).atan2(x - cx_).to_degrees() + 90.0;
                if ev.modifiers.shift {
                    angle = (angle / 15.0).round() * 15.0;
                }
                l.rotation = geo::normalise_degrees(angle);
            }
            DragMode::Crop(corner, size) => l = geo::crop(&l, corner, size, (dx, dy)),
            DragMode::CropEdge(edge, size) => l = geo::crop_edge(&l, edge, size, (dx, dy)),
            DragMode::CropPan(size) => l = geo::crop_pan(&l, size, (dx, dy)),
            DragMode::TiltAxis(axis) => {
                let k = 0.35 * s;
                let snap = |a: f32| if ev.modifiers.shift { (a / 15.0).round() * 15.0 } else { a };
                match axis {
                    geo::Axis::X => l.tilt_x = snap((drag.layout.tilt_x + dy * k).clamp(-75.0, 75.0)),
                    _ => l.tilt_y = snap((drag.layout.tilt_y - dx * k).clamp(-75.0, 75.0)),
                }
            }
            DragMode::Tilt => {
                // Drag right: the right side turns away; drag down: the bottom does. Screen
                // degrees per pixel; ⇧ snaps to 15°.
                let k = 0.35 * s;
                let snap = |a: f32| if ev.modifiers.shift { (a / 15.0).round() * 15.0 } else { a };
                l.tilt_y = snap((drag.layout.tilt_y - dx * k).clamp(-75.0, 75.0));
                l.tilt_x = snap((drag.layout.tilt_x + dy * k).clamp(-75.0, 75.0));
            }
            DragMode::Move => {
                l.x += dx;
                l.y += dy;
                if !ev.modifiers.platform {
                    // Snap edges and centre to the canvas edges and centre.
                    let thr = SNAP_PX / s;
                    let snap = |pos: f32, size: f32, extent: f32, guides: &mut Guides, vertical: bool| -> f32 {
                        for (offset, target) in [(0.0, 0.0), (size / 2.0, extent / 2.0), (size, extent)] {
                            for t in [0.0, extent / 2.0, extent] {
                                if (pos + offset - t).abs() < thr {
                                    guides.push((vertical, t));
                                    return t - offset;
                                }
                            }
                            let _ = target;
                        }
                        pos
                    };
                    l.x = snap(l.x, l.width, cw, &mut guides, true);
                    l.y = snap(l.y, l.height, ch, &mut guides, false);
                }
            }
            // In Fit the picture keeps its shape, so the frame does too (Stretch to distort).
            DragMode::Resize(corner) => {
                let keep = ev.modifiers.shift || (l.fit == lightair_core::Fit::Fit && web.is_none());
                l = geo::resize(&l, corner, (dx, dy), keep);
                if !ev.modifiers.platform {
                    // The dragged corner snaps to the canvas edges and centre lines (⌘: free).
                    let (snapped, lines) = geo::snap_resize(&l, corner, (cw, ch), SNAP_PX / s, keep);
                    l = snapped;
                    guides.extend(lines);
                }
            }
        }
        self.guides = guides;
        // The group follows the grabbed object (after snapping).
        let (mx, my) = (l.x - drag.layout.x, l.y - drag.layout.y);
        let mut layouts = vec![(drag.object, l)];
        layouts.extend(drag.others.iter().map(|(id, o)| (*id, Layout { x: o.x + mx, y: o.y + my, ..*o })));
        let (scene, canvas, resizing) = (drag.scene, drag.canvas, matches!(drag.mode, DragMode::Resize(_)));
        self.set_layouts(scene, canvas, layouts, "Move objects", cx);
        if resizing && let Some(source) = web {
            self.size_web_page(source, &l, cx);
        }
    }

    /// The web page source `object` shows, if it shows one.
    fn web_source_of(&self, workspace: &Workspace, scene: Id, object: Id) -> Option<Id> {
        let source = workspace.scene(scene)?.object(object)?.source?;
        workspace.sources.get(&source).filter(|i| object_props::is_web(&i.kind)).map(|i| i.id)
    }

    /// Sizes a web page to show at one page pixel a canvas pixel in `frame` (its box on the
    /// canvas, after cropping): the page is the box plus what's cropped away.
    fn size_web_page(&mut self, source: Id, frame: &Layout, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(i) = workspace.sources.get(&source) else { return };
        let c = frame.crop;
        let w = (frame.width.round() as i64 + (c.left + c.right) as i64).clamp(16, 7680);
        let h = (frame.height.round() as i64 + (c.top + c.bottom) as i64).clamp(16, 7680);
        if i.settings["width"].as_i64() == Some(w) && i.settings["height"].as_i64() == Some(h) {
            return;
        }
        let mut settings = if i.settings.is_object() { i.settings.clone() } else { serde_json::json!({}) };
        settings["width"] = serde_json::json!(w);
        settings["height"] = serde_json::json!(h);
        self.exec(Command::SetSourceSettings { source, settings }, cx);
    }

    fn canvas_up(&mut self, _: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.panning.take().is_some() {
            cx.notify();
            return;
        }
        if let Some((canvas, from, to, additive)) = self.marquee.take() {
            let moved = (from.0 - to.0).abs() > 2.0 || (from.1 - to.1).abs() > 2.0;
            self.studio.update(cx, |s, cx| {
                let p = s.workspace().clone();
                if let Some(scene) = s.selected_scene.and_then(|id| p.scene(id))
                    && moved
                {
                    let hit: Vec<Id> = scene
                        .objects
                        .iter()
                        .rev()
                        .filter(|l| !l.is_empty() && !scene.locked(l.id))
                        .filter(|l| {
                            scene
                                .world(l.id, canvas)
                                .is_some_and(|lo| scene.shown(l.id, canvas) && geo::touches(&lo, from, to))
                        })
                        .map(|l| l.id)
                        .collect();
                    let mut all = if additive { s.selection() } else { vec![] };
                    for id in hit {
                        if !all.contains(&id) {
                            all.push(id);
                        }
                    }
                    s.selected_object = all.first().copied();
                    s.also_selected = all.into_iter().skip(1).collect();
                }
                cx.notify();
            });
            return;
        }
        if let Some(drag) = self.drag.take() {
            self.guides.clear();
            if matches!(drag.mode, DragMode::Tilt) && !drag.moved {
                self.rotate_tool = !self.rotate_tool;
                self.gizmo_hover = None;
            }
            // A crop drag showed the whole picture: stay a moment (for another crop handle),
            // then fade back to the result.
            if drag.mode.is_crop() {
                self.leave_crop_view(CROP_LINGER, cx);
            }
            self.studio.update(cx, |s, cx| {
                s.end_gesture();
                cx.notify();
            });
        }
    }

    /// The handle of `l` (the selected object's frame) at canvas point `p`. One set does
    /// everything: corners resize (⌥ crops), edges crop, the top knob rotates, the bottom knob
    /// tilts in 3D (and opens the rotate gizmo), ⌥-drag inside pans a cropped picture.
    fn handle_at(&self, l: &Layout, (x, y): (f32, f32), s: f32, alt: bool, has_size: bool) -> Option<Handle> {
        let tol = HANDLE / s;
        let near = |(px_, py_): (f32, f32)| (x - px_).abs() <= tol && (y - py_).abs() <= tol;
        if self.rotate_tool {
            if let Some(axis) = geo::gizmo_hit(l, gizmo_radius(l, s), (x, y), GIZMO_TOL / s) {
                return Some(Handle::Gizmo(axis));
            }
            // With the gizmo on, the other handles are hidden; inside still moves.
            return near(geo::tilt_handle(l, ROTATE_OFFSET / s)).then_some(Handle::Tilt);
        }
        if near(geo::rotate_handle(l, ROTATE_OFFSET / s)) {
            return Some(Handle::Rotate);
        }
        if near(geo::tilt_handle(l, ROTATE_OFFSET / s)) {
            return Some(Handle::Tilt);
        }
        if let Some(c) = CORNERS.iter().find(|c| near(geo::corner(l, **c))) {
            return Some(Handle::Corner(*c));
        }
        if has_size {
            let (_, edges) = geo::crop_handles(l);
            if let Some((e, _)) = edges.iter().find(|(_, p)| near(*p)) {
                return Some(Handle::Edge(*e));
            }
            if alt && l.crop != lightair_core::Crop::default() && geo::contains(l, (x, y)) {
                return Some(Handle::Pan);
            }
        }
        None
    }

    /// Leaves crop view after `delay`: the ghost fades out, then the crop applies again. A crop
    /// drag in the meantime cancels it.
    fn leave_crop_view(&mut self, delay: std::time::Duration, cx: &mut Context<Self>) {
        if self.crop_mode.is_none() {
            return;
        }
        self.crop_epoch += 1;
        let epoch = self.crop_epoch;
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let fading = this.update(cx, |this, cx| {
                if this.crop_epoch != epoch || this.drag.as_ref().is_some_and(|d| d.mode.is_crop()) {
                    return false;
                }
                this.crop_leaving = true;
                let studio = this.studio.read(cx);
                let workspace = studio.workspace().clone();
                studio.engine().post(move |s| s.reconciler.fade_crop_preview(&workspace));
                cx.notify();
                true
            });
            if !matches!(fading, Ok(true)) {
                return;
            }
            cx.background_executor().timer(CROP_FADE).await;
            let _ = this.update(cx, |this, cx| {
                if this.crop_epoch == epoch && this.crop_leaving {
                    this.crop_mode = None;
                    this.crop_leaving = false;
                    this.sync_crop_preview(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The canvas's cursor: what a press would do here (or is doing).
    fn canvas_cursor(&self, workspace: &Workspace, cx: &App) -> CursorStyle {
        if self.panning.is_some() {
            return CursorStyle::ClosedHand;
        }
        if self.space_down {
            return CursorStyle::OpenHand;
        }
        let handle = match &self.drag {
            Some(d) if d.mode == DragMode::Move => return CursorStyle::ClosedHand,
            Some(d) => d.mode.handle(self.rotate_tool),
            None => self.hover_handle,
        };
        let rotation = {
            let studio = self.studio.read(cx);
            let canvas = self.canvas_id(workspace);
            studio
                .selected_scene
                .and_then(|id| workspace.scene(id))
                .and_then(|sc| studio.selected_object.and_then(|l| sc.world(l, canvas)))
                .map_or(0.0, |l| l.rotation)
        };
        let grabbing = self.drag.is_some();
        match handle {
            Some(Handle::Corner(c)) => resize_cursor(
                match c {
                    Corner::TopLeft | Corner::BottomRight => 45.0,
                    Corner::TopRight | Corner::BottomLeft => 135.0,
                } + rotation,
            ),
            Some(Handle::Edge(e)) => resize_cursor(
                match e {
                    geo::Edge::Left | geo::Edge::Right => 0.0,
                    geo::Edge::Top | geo::Edge::Bottom => 90.0,
                } + rotation,
            ),
            Some(_) if grabbing => CursorStyle::ClosedHand,
            Some(_) => CursorStyle::OpenHand,
            None => CursorStyle::Arrow,
        }
    }

    fn nudge(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let canvas = self.canvas_id(&workspace);
        let Some(scene_id) = studio.selected_scene else { return };
        let Some(scene) = workspace.scene(scene_id) else { return };
        let selection = studio.selection();
        let moved: Vec<(Id, Layout)> = selection
            .iter()
            .filter_map(|id| scene.object(*id).filter(|l| !scene.locked(l.id)))
            .filter(|l| !selection.iter().any(|a| scene.is_under(l.id, *a)))
            .filter_map(|l| {
                self.frame_of(scene, l, canvas).map(|lo| (l.id, Layout { x: lo.x + dx, y: lo.y + dy, ..lo }))
            })
            .collect();
        self.set_layouts(scene_id, canvas, moved, "Nudge objects", cx);
    }

    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let Some(scene) = studio.selected_scene else { return };
        let Some(sc) = studio.workspace().scene(scene) else { return };
        // A object inside another selected one goes with it.
        let selection = studio.selection();
        let mut commands: Vec<Command> = selection
            .iter()
            .filter(|l| !selection.iter().any(|a| sc.is_under(**l, *a)))
            .map(|object| Command::RemoveObject { scene, object: *object })
            .collect();
        match commands.len() {
            0 => {}
            1 => self.exec(commands.remove(0), cx),
            n => self.exec(Command::Batch { label: format!("Delete {n} objects"), commands }, cx),
        }
    }

    // ------------------------------------------------------------ adding things
    fn add_source_object(&mut self, name: &str, kind: &str, settings: serde_json::Value, size: (f32, f32), cx: &mut Context<Self>) {
        self.add_source_object_id(name, kind, settings, size, cx);
    }

    /// Adds a source and a object showing it to the selected scene: the source's id.
    fn add_source_object_id(
        &mut self,
        name: &str,
        kind: &str,
        settings: serde_json::Value,
        size: (f32, f32),
        cx: &mut Context<Self>,
    ) -> Option<Id> {
        // a source with sound of its own (a web page) also gets a mixer track
        let sound = kind == lightair_engine::web::KIND;
        self.add_placed_object(name, kind, settings, size, (0.5, 0.5), sound, cx)
    }

    /// `add_source_object_id`, put at `place` (across and down the room left: 0.5 centres it)
    /// and with a mixer track if it has `sound`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn add_placed_object(
        &mut self,
        name: &str,
        kind: &str,
        settings: serde_json::Value,
        size: (f32, f32),
        place: (f32, f32),
        sound: bool,
        cx: &mut Context<Self>,
    ) -> Option<Id> {
        self.add_menu = false;
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let scene = studio.selected_scene?;
        let (w, h) = size;
        let layouts: Vec<(Id, Layout)> = workspace
            .canvases
            .iter()
            .map(|f| {
                let s = (f.width as f32 / w).min(f.height as f32 / h).min(1.0);
                let (lw, lh) = (w * s, h * s);
                (f.id, Layout::boxed((f.width as f32 - lw) * place.0, (f.height as f32 - lh) * place.1, lw, lh))
            })
            .collect();
        let source = Source::new(name, kind, settings);
        let source_id = source.id;
        let object = Object::new(name, source.id, layouts);
        let object_id = object.id;
        let track = sound.then(|| lightair_core::Track::new(name, source_id));
        let mut commands = vec![Command::AddSource { source }, Command::AddObject { scene, object, index: None }];
        commands.extend(track.map(|track| Command::AddTrack { track, index: None }));
        let cmd = Command::Batch { label: format!("Add {name}"), commands };
        self.exec(cmd, cx);
        self.select_object(Some(object_id), cx);
        Some(source_id)
    }

    /// A camera object, set up with the camera defaults (Settings › Camera): with Beauty on by
    /// default, the source starts with the Beauty effect.
    fn add_camera(&mut self, label: &str, uid: &str, cx: &mut Context<Self>) {
        let defaults = self.prefs.camera;
        let source = self.add_source_object_id(label, lightair_core::kinds::CAMERA, defaults.settings(uid), (1280.0, 720.0), cx);
        if defaults.studio_on
            && let Some(source) = source
            && let Some(beauty) = lightair_core::catalog::def("studio.beauty")
        {
            let cmd = Command::AddSourceEffect { source, effect: beauty.create(), index: None };
            self.exec(cmd, cx);
        }
    }

    fn add_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_menu = false;
        let paths =
            cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path): Option<PathBuf> = paths.into_iter().next() else { return };
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Image".into());
            let _ = this.update(cx, |this, cx| {
                this.add_source_object(&name, "image_source", json!({"file": path.to_string_lossy()}), (640.0, 360.0), cx)
            });
        })
        .detach();
    }

    pub(super) fn add_text(&mut self, cx: &mut Context<Self>) {
        let settings = json!({
            "text": "Lightair",
            "font": {"face": crate::ui::TEXT_FACE, "size": 128, "flags": 1},
            "color1": 0xFFFFFFFFu32,
            "color2": 0xFFFFFFFFu32,
        });
        self.add_source_object("Text", "text_ft2_source_v2", settings, (720.0, 180.0), cx)
    }

    /// A 5-minute countdown that starts when its scene goes on air.
    fn add_timer(&mut self, cx: &mut Context<Self>) {
        let settings = json!({
            "mode": "countdown",
            "seconds": 300,
            "autostart": true,
            "font": {"face": crate::ui::TEXT_FACE, "size": 160, "flags": 1},
            "color1": 0xFFFFFFFFu32,
            "color2": 0xFFFFFFFFu32,
        });
        self.add_source_object("Countdown", lightair_engine::timers::TIMER_KIND, settings, (560.0, 200.0), cx)
    }

    fn add_colour(&mut self, cx: &mut Context<Self>) {
        let settings = json!({"color": 0xFF4A3BFFu32, "width": 400, "height": 400});
        self.add_source_object("Colour", "color_source_v3", settings, (400.0, 400.0), cx)
    }

    /// A video file object: plays when its scene goes on air, loops by default.
    fn add_video(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_menu = false;
        let paths =
            cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path): Option<PathBuf> = paths.into_iter().next() else { return };
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Video".into());
            let settings = json!({
                "local_file": path.to_string_lossy(),
                "is_local_file": true,
                "looping": true,
                "restart_on_activate": true,
            });
            let _ = this
                .update(cx, |this, cx| this.add_source_object(&name, "ffmpeg_source", settings, (1920.0, 1080.0), cx));
        })
        .detach();
    }

    /// Imports an OBS scene collection (OBS keeps them in
    /// ~/Library/Application Support/obs-studio/basic/scenes) as one undo step.
    fn import_obs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path): Option<PathBuf> = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.studio.update(cx, |s, cx| s.import_obs(&path, cx));
            });
        })
        .detach();
    }

    /// Picks the stinger video (cut point defaults to halfway through a typical 1 s stinger).
    /// Picks a stinger video: the workspace's, or a scene's own (`scene`).
    fn choose_stinger(&mut self, scene: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        let paths =
            cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path): Option<PathBuf> = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                let workspace = this.studio.read(cx).workspace().clone();
                // the cut point it had (the scene's, else the show's)
                let had = scene.and_then(|s| workspace.scene(s)).and_then(|s| s.stinger.clone()).or(workspace.stinger.clone());
                let stinger = lightair_core::Stinger { path: path.to_string_lossy().into_owned(), cut_ms: had.map_or(500, |s| s.cut_ms) };
                match scene {
                    Some(scene) => this.exec(Command::SetSceneStinger { scene, stinger: Some(stinger) }, cx),
                    None => this.exec(Command::SetStinger { stinger: Some(stinger) }, cx),
                }
            });
        })
        .detach();
    }

    /// ⌘1–⌘9: the nth scene goes on air (or is selected, in Studio mode).
    fn scene_shortcut(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let scene = self.studio.read(cx).workspace().scenes.get(index).map(|s| s.id);
        if let Some(id) = scene {
            self.run_palette_action(palette::PaletteAction::Scene(id), window, cx);
        }
    }

    /// The preview under the UI (native_preview): made with the window, each canvas's preview
    /// showing in its layer, shown in the Live place only. Whether it's there.
    fn sync_native(&mut self, window: &Window) -> bool {
        if !crate::native_preview::on() {
            return false;
        }
        let mut native = self.native.borrow_mut();
        // GPUI remade its composition (a lost device): the visuals go with it
        #[cfg(windows)]
        if native.as_ref().is_some_and(|n| !n.current()) {
            *native = None;
            self.native_layers.clear();
        }
        if native.is_none() {
            *native = crate::native_preview::NativePreview::attach(window, tok::bg_app(), tok::bg_canvas_well());
        }
        let Some(n) = native.as_mut() else { return false };
        for (id, preview) in &self.previews {
            if self.native_layers.insert(*id) {
                preview.set_layer(Some(n.picture(*id)));
            }
        }
        let gone: Vec<Id> = self.native_layers.iter().copied().filter(|id| !self.previews.contains_key(id)).collect();
        for id in gone {
            self.native_layers.remove(&id);
            n.forget(id);
        }
        n.set_visible(self.place == Place::Live);
        n.show_player(self.place == Place::Editor);
        true
    }

    /// Something on screen moves without input: redrawn 30 times a second.
    fn moving(&self) -> bool {
        self.studio_take_countdown.is_some()
            || self.thumbnail_shot.is_some()
            || self.add_object.as_ref().is_some_and(|a| a.screens.is_some())
    }

    /// Frames drawn so far by the live pictures a region shows (it redraws when this changes).
    fn live_frames(&self, region: docking::Region) -> u64 {
        match region {
            // drawn under the UI: a new frame needn't redraw the panel
            docking::Region::Canvas if self.native.borrow().is_some() => 0,
            docking::Region::Canvas => self.previews.values().map(|p| p.frames()).sum(),
            docking::Region::Inspector => self.program_monitor.as_ref().map_or(0, |p| p.frames()),
            _ => 0,
        }
    }

    fn add_scene(&mut self, cx: &mut Context<Self>) {
        let n = self.studio.read(cx).workspace().scenes.len() + 1;
        let scene = Scene::new(format!("Scene {n}"));
        let id = scene.id;
        self.exec(Command::AddScene { scene, index: None }, cx);
        self.studio.update(cx, |s, cx| {
            s.select_scene(id);
            cx.notify();
        });
    }
}

// ================================================================= render
impl Render for StudioWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let native = self.sync_native(window);
        self.tick_add_object();
        self.ensure_dock(window, cx);
        self.sync_crop_preview(cx);
        self.refresh_sizes(cx);
        self.sync_previews(cx);
        self.take_thumbnails(cx);
        self.sync_object_props(window, cx);
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();

        self.menu_actions(div(), cx)
            .relative()
            .key_context("Studio")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if this.place == Place::Editor {
                    this.editor.update(cx, |e, cx| e.undo(true, cx));
                } else {
                    this.studio.update(cx, |s, cx| s.undo(cx))
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if this.place == Place::Editor {
                    this.editor.update(cx, |e, cx| e.undo(false, cx));
                } else {
                    this.studio.update(cx, |s, cx| s.redo(cx))
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteSelection, window, cx| {
                // Backspace in an empty text field reaches here (the field passes it on)
                if typing(window) {
                    return;
                }
                if this.place == Place::Editor {
                    this.editor.update(cx, |e, cx| e.delete_selected(cx));
                } else {
                    this.delete_selection(cx)
                }
            }))
            // in the Editor: the playhead a frame, a cut
            .on_action(cx.listener(|this, _: &NudgeLeft, window, cx| {
                if typing(window) {
                    return;
                }
                if this.place == Place::Editor {
                    let k = Keystroke::parse("left").unwrap();
                    this.editor.update(cx, |e, cx| e.key(&k, cx));
                    return;
                }
                this.nudge(-1.0, 0.0, cx)
            }))
            .on_action(cx.listener(|this, _: &NudgeRight, window, cx| {
                if typing(window) {
                    return;
                }
                if this.place == Place::Editor {
                    let k = Keystroke::parse("right").unwrap();
                    this.editor.update(cx, |e, cx| e.key(&k, cx));
                    return;
                }
                this.nudge(1.0, 0.0, cx)
            }))
            .on_action(cx.listener(|this, _: &NudgeUp, window, cx| {
                if typing(window) {
                    return;
                }
                if this.place == Place::Editor {
                    let k = Keystroke::parse("up").unwrap();
                    this.editor.update(cx, |e, cx| e.key(&k, cx));
                    return;
                }
                this.nudge(0.0, -1.0, cx)
            }))
            .on_action(cx.listener(|this, _: &NudgeDown, window, cx| {
                if typing(window) {
                    return;
                }
                if this.place == Place::Editor {
                    let k = Keystroke::parse("down").unwrap();
                    this.editor.update(cx, |e, cx| e.key(&k, cx));
                    return;
                }
                this.nudge(0.0, 1.0, cx)
            }))
            .on_action(cx.listener(|this, _: &Deselect, window, cx| {
                if this.place == Place::Editor {
                    let k = Keystroke::parse("escape").unwrap();
                    this.editor.update(cx, |e, cx| e.key(&k, cx));
                    return;
                }
                // Esc in a rename field cancels it; otherwise it leaves crop mode or the
                // rotate gizmo first.
                if this.renaming.is_some() {
                    this.finish_rename(false, window, cx);
                } else if typing(window) {
                    // Esc in any other text field leaves the field, not the selection
                    window.blur(cx);
                } else if this.crop_mode.is_some() && !this.crop_leaving {
                    this.leave_crop_view(std::time::Duration::ZERO, cx);
                } else if this.rotate_tool {
                    this.rotate_tool = false;
                    cx.notify();
                } else {
                    this.select_object(None, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &RenameSelection, window, cx| {
                // Enter in a rename field reaches here too (the field passes it on): it
                // commits the new name rather than starting another rename.
                if this.renaming.is_some() {
                    this.finish_rename(true, window, cx);
                } else if window.context_stack().iter().any(|c| c.contains("Input")) {
                    // Enter in any other text field (Inspector, search…) is that field's.
                } else if this.crop_mode.is_some() && !this.crop_leaving {
                    this.leave_crop_view(std::time::Duration::ZERO, cx);
                } else {
                    this.rename_selection(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| this.open_palette(window, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.toggle_settings(cx)))
            .on_action(cx.listener(|this, _: &OpenAudioSettings, _, cx| this.open_settings(settings::Page::Audio, cx)))
            .on_action(cx.listener(|this, _: &ToggleRotateTool, window, cx| {
                // a plain letter: while a text field has the keyboard it's typing, not the tool
                // (passed on, so the field gets it)
                if typing(window) {
                    cx.propagate();
                    return;
                }
                this.rotate_tool = !this.rotate_tool;
                this.gizmo_hover = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleMics, _, cx| this.studio.update(cx, |s, cx| s.toggle_mics(cx))))
            .on_action(cx.listener(|this, _: &GroupSelection, window, cx| this.group_selection(window, cx)))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| {
                if this.place == Place::Editor {
                    this.editor.update(cx, |e, cx| e.copy_picked(cx));
                } else {
                    this.copy_selection(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CutSelection, _, cx| {
                if this.place == Place::Editor {
                    this.editor.update(cx, |e, cx| e.cut_picked(cx));
                } else {
                    this.cut_selection(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &PasteObjects, _, cx| {
                if this.place == Place::Editor {
                    this.editor.update(cx, |e, cx| e.paste_at(None, cx));
                } else {
                    this.paste_objects(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &DuplicateSelection, _, cx| {
                if this.place == Place::Editor {
                    this.editor.update(cx, |e, cx| e.duplicate_picked(cx));
                } else {
                    this.duplicate_selection(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &BringToFront, _, cx| this.arrange("Bring to front", |_, n| n - 1, cx)))
            .on_action(cx.listener(|this, _: &SendToBack, _, cx| this.arrange("Send to back", |_, _| 0, cx)))
            .on_action(cx.listener(|this, _: &BringForward, _, cx| this.arrange("Bring forward", |i, _| i + 1, cx)))
            .on_action(cx.listener(|this, _: &SendBackward, _, cx| {
                this.arrange("Send backward", |i, _| i.saturating_sub(1), cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleObjectVisible, _, cx| this.toggle_selection_visible(cx)))
            .on_action(cx.listener(|this, _: &ToggleObjectLocked, _, cx| this.toggle_selection_locked(cx)))
            .on_action(cx.listener(|this, _: &MoveOutOfParent, _, cx| this.move_out_of_parent(cx)))
            .on_action(cx.listener(|this, _: &FillCanvas, _, cx| this.fill_canvas(cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                if this.place == Place::Editor {
                    let k = Keystroke::parse(crate::ui::key("cmd-=")).unwrap();
                    this.editor.update(cx, |e, cx| e.key(&k, cx));
                    return;
                }
                this.zoom_by(1.25, None, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                if this.place == Place::Editor {
                    let k = Keystroke::parse(crate::ui::key("cmd--")).unwrap();
                    this.editor.update(cx, |e, cx| e.key(&k, cx));
                    return;
                }
                this.zoom_by(0.8, None, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| this.zoom_to_fit(cx)))
            .on_action(cx.listener(|this, _: &ZoomToActual, _, cx| this.zoom_to_actual(cx)))
            // Space held: dragging the canvas pans it.
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                // the Editor: space plays, and its tools' keys (unless a text field has the keys)
                if this.place == Place::Editor && !window.context_stack().iter().any(|c| c.contains("Input")) {
                    if ev.keystroke.key == "space" && !ev.keystroke.modifiers.modified() {
                        this.editor.update(cx, |e, cx| e.play(cx));
                        cx.stop_propagation();
                        return;
                    }
                    if this.editor.update(cx, |e, cx| e.key(&ev.keystroke, cx)) {
                        cx.stop_propagation();
                        return;
                    }
                }
                if ev.keystroke.key == "space" && !ev.keystroke.modifiers.modified() && !this.space_down {
                    this.space_down = true;
                    cx.notify();
                }
            }))
            .capture_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| {
                if ev.keystroke.key == "space" && this.space_down {
                    this.space_down = false;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &SaveAsPrefab, _, cx| {
                if let Some(object) = this.studio.read(cx).selected_object {
                    this.save_prefab(object, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &UngroupSelection, _, cx| this.ungroup_selection(cx)))
            .on_action(cx.listener(|this, _: &CreateEmpty, window, cx| this.create_empty(None, window, cx)))
            .on_action(cx.listener(|this, _: &CreateEmptyChild, window, cx| {
                let parent = this.studio.read(cx).selected_object;
                this.create_empty(parent, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Scene1, w, cx| this.scene_shortcut(0, w, cx)))
            .on_action(cx.listener(|this, _: &Scene2, w, cx| this.scene_shortcut(1, w, cx)))
            .on_action(cx.listener(|this, _: &Scene3, w, cx| this.scene_shortcut(2, w, cx)))
            .on_action(cx.listener(|this, _: &Scene4, w, cx| this.scene_shortcut(3, w, cx)))
            .on_action(cx.listener(|this, _: &Scene5, w, cx| this.scene_shortcut(4, w, cx)))
            .on_action(cx.listener(|this, _: &Scene6, w, cx| this.scene_shortcut(5, w, cx)))
            .on_action(cx.listener(|this, _: &Scene7, w, cx| this.scene_shortcut(6, w, cx)))
            .on_action(cx.listener(|this, _: &Scene8, w, cx| this.scene_shortcut(7, w, cx)))
            .on_action(cx.listener(|this, _: &Scene9, w, cx| this.scene_shortcut(8, w, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if this.place == Place::Editor {
                    this.editor.update(cx, |e, cx| e.select_all(cx));
                    return;
                }
                this.studio.update(cx, |s, cx| {
                    let p = s.workspace().clone();
                    let objects: Vec<Id> = s
                        .selected_scene
                        .and_then(|id| p.scene(id))
                        .map(|scene| scene.objects.iter().rev().map(|l| l.id).collect())
                        .unwrap_or_default();
                    s.selected_object = objects.first().copied();
                    s.also_selected = objects.into_iter().skip(1).collect();
                    cx.notify();
                })
            }))
            .on_action(cx.listener(|this, _: &AddMarker, _, cx| {
                if this.studio.read(cx).is_recording() {
                    this.studio.update(cx, |s, cx| s.add_marker(cx))
                }
            }))
            .on_action(cx.listener(|this, _: &Take, window, cx| {
                // ⌘↩ in a single-line field reaches here: never a take by accident
                if this.cue_scenes && !typing(window) {
                    this.take(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ShowLibrary, _, cx| this.show_place(Place::Library, cx)))
            .on_action(cx.listener(|this, _: &ShowLive, _, cx| this.show_place(Place::Live, cx)))
            .on_action(cx.listener(|this, _: &ShowEditor, _, cx| this.show_place(Place::Editor, cx)))
            .size_full()
            .flex()
            .flex_col()
            // drawn under the UI with the preview when it's native
            .when(!native, |d| d.bg(tok::bg_app()))
            .text_color(tok::text())
            .text_size(px(13.))
            .child(self.title_bar(&workspace, f32::from(window.viewport_size().width), cx))
            .child(match self.place {
                Place::Live => div().flex().flex_1().min_h_0().children(self.dock.clone()),
                Place::Library => div().flex().flex_1().min_h_0().child(self.library.clone()),
                Place::Editor => div().flex().flex_1().min_h_0().child(self.editor.clone()),
            })
            .child(self.status_bar(cx))
            .children(self.add_object_overlay(cx))
            .children(self.effect_catalog_overlay(cx))
            .children(self.source_picker_overlay(cx))
            .children(self.outputs.update(cx, |o, cx| o.dialog(cx)))
            .children(self.go_live_overlay(cx))
            .children(self.settings_overlay(cx))
            .children(self.palette_overlay(cx))
            .children(self.studio_take_countdown_overlay())
            .children(self.studio_take_card(cx))
    }
}

/// "ctrl-1" as the platform shows it: "⌃1", "Alt+1".
fn key_label(key: &str) -> String {
    let (m, k) = key.split_once('-').unwrap_or(("", key));
    match (m, cfg!(windows)) {
        ("ctrl", false) => format!("⌃{k}"),
        ("alt", true) => format!("Alt+{k}"),
        ("alt", false) => format!("⌥{k}"),
        _ => format!("Ctrl+{k}"),
    }
}

impl StudioWindow {
    pub(crate) fn show_place(&mut self, place: Place, cx: &mut Context<Self>) {
        self.place = place;
        if place == Place::Editor {
            self.editor.update(cx, |e, cx| e.show(cx));
        }
        if place == Place::Library {
            self.library.update(cx, |l, cx| l.reload(cx));
        }
        // the canvas previews rest while they aren't seen
        for p in self.previews.values() {
            p.set_active(place == Place::Live);
        }
        cx.notify();
    }

    /// Takes the Library's stills: a program preview, kept until it has drawn a few frames.
    fn take_thumbnails(&mut self, cx: &mut Context<Self>) {
        let wanted = self.studio.update(cx, |s, _| std::mem::take(&mut s.thumbnails));
        if !wanted.is_empty() {
            let shot = self.thumbnail_shot.get_or_insert_with(|| (Preview::attach(), Vec::new()));
            shot.1.extend(wanted);
        }
        let Some((preview, _)) = &self.thumbnail_shot else { return };
        if preview.frames() < 3 {
            return;
        }
        let Some((preview, requests)) = self.thumbnail_shot.take() else { return };
        let image = preview
            .latest_frame()
            .and_then(|f| lightair_preview::frame_rgba(&f, 480))
            .and_then(|(w, h, rgba)| image::RgbaImage::from_raw(w as u32, h as u32, rgba));
        let Some(image) = image else {
            tracing::warn!("no program frame for the Library's stills");
            return;
        };
        for r in requests {
            if let Some(dir) = r.path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = image.save_with_format(&r.path, image::ImageFormat::Png) {
                tracing::warn!(%e, path = %r.path.display(), "couldn't write a Library still");
            }
        }
        self.studio.update(cx, |s, _| s.library_version += 1);
    }

    /// Opens another workspace in this window (saved first; not while recording or live).
    pub(crate) fn open_workspace(&mut self, dir: std::path::PathBuf, cx: &mut Context<Self>) {
        let old = self.studio.read(cx).workspace().primary_canvas().id;
        let result = self.studio.update(cx, |s, cx| s.switch_to(&dir, cx));
        if let Err(e) = result {
            self.studio.update(cx, |s, _| s.issue = Some(e.into()));
            cx.notify();
            return;
        }
        // the main canvas's preview belongs to the new primary canvas now
        let new = self.studio.read(cx).workspace().primary_canvas().id;
        if let Some(p) = self.previews.remove(&old) {
            self.previews.retain(|_, _| false);
            self.previews.insert(new, p);
        }
        self.active_canvas = Some(new);
        self.crop_mode = None;
        self.crop_leaving = false;
        self.rotate_tool = false;
        self.renaming = None;
        self.source_picker = None;
        self.add_object = None;
        self.marquee = None;
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
        self.library.update(cx, |l, cx| l.reload(cx));
        cx.notify();
    }

    /// Makes a workspace called `name` and opens it.
    pub(crate) fn new_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        match self.studio.read(cx).create_workspace(name) {
            Ok(dir) => {
                self.open_workspace(dir, cx);
                self.show_place(Place::Live, cx);
            }
            Err(e) => {
                self.studio.update(cx, |s, _| s.issue = Some(format!("Couldn't make the workspace: {e}").into()));
                cx.notify();
            }
        }
    }

    /// Adds the screen recording scenes (screen, screen + camera, split, camera) on backdrops,
    /// from the workspace's screen (a display capture is added if it has none) and camera.
    pub(crate) fn add_screen_layouts(&mut self, cx: &mut Context<Self>) {
        use lightair_core::kinds;
        let workspace = self.studio.read(cx).workspace().clone();
        let mut commands = Vec::new();
        let screen = match workspace.sources.values().find(|i| i.kind == kinds::SCREEN) {
            Some(i) => i.id,
            None => {
                let source = lightair_core::Source::new("Display", kinds::SCREEN, display_settings());
                let id = source.id;
                commands.push(Command::AddSource { source });
                id
            }
        };
        let camera = workspace.sources.values().find(|i| i.kind == kinds::CAMERA).map(|i| i.id);
        let (_, _, dw, dh) = lightair_platform::input::main_display();
        let sources = lightair_core::screen_layouts::Sources {
            screen,
            screen_aspect: if dh > 0.0 { dw / dh } else { 16.0 / 10.0 },
            camera,
            preset: 0,
        };
        let mut with = workspace.as_ref().clone();
        for c in &commands {
            let _ = c.apply(&mut with);
        }
        commands.extend(lightair_core::screen_layouts::scenes(&with, &sources));
        self.exec(Command::Batch { label: "Add screen recording scenes".into(), commands }, cx);
    }

    /// Starts a Studio studio_take after a 3-second countdown (again during it: cancels), or stops one.
    pub(crate) fn studio_take(&mut self, cx: &mut Context<Self>) {
        if self.studio.read(cx).is_taking() {
            self.studio.update(cx, |s, cx| s.toggle_studio_take(cx));
        } else if self.studio_take_countdown.take().is_none() {
            self.studio_take_countdown = Some(Instant::now());
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.studio_take_countdown.take().is_some() {
                        this.studio.update(cx, |s, cx| s.toggle_studio_take(cx));
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// The countdown's number over the window.
    fn studio_take_countdown_overlay(&self) -> Option<AnyElement> {
        let started = self.studio_take_countdown?;
        let left = 3u64.saturating_sub(started.elapsed().as_secs()).max(1);
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .size(px(160.))
                        .justify_center()
                        .rounded_full()
                        .bg(rgba(0x0000_00b0))
                        .child(div().text_size(px(64.)).font_weight(FontWeight::BOLD).text_color(rgb(0xFFFFFF)).child(left.to_string()))
                        .child(div().text_size(px(11.)).text_color(rgb(0xDDDDDD)).child("Studio take")),
                )
                .into_any_element(),
        )
    }

    /// After a take: where it went.
    fn studio_take_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, id) = self.studio.read(cx).last_studio_take.clone()?;
        let _ = id;
        Some(
            div()
                .absolute()
                .right(px(16.))
                .bottom(px(40.))
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .rounded(px(12.))
                .bg(tok::bg_panel())
                .border_1()
                .border_color(tok::border_strong())
                .shadow_lg()
                .child(ui::icon("circle-check").size(px(18.)).text_color(tok::ok()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(FontWeight::MEDIUM).child("Studio take saved"))
                        .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("Screen, cameras, audio and pointer, in the Library")),
                )
                .child(Button::new("take-card-library").label("Show").small().outline().on_click(cx.listener(|this, _, _, cx| {
                    this.studio.update(cx, |s, _| s.last_studio_take = None);
                    this.show_place(Place::Library, cx);
                })))
                .child(ui::icon_button("take-card-close", "x", tok::text_tertiary()).on_click(cx.listener(|this, _, _, cx| {
                    this.studio.update(cx, |s, _| s.last_studio_take = None);
                    cx.notify();
                })))
                .into_any_element(),
        )
    }

    /// The record button: starts what the mode says, stops whatever is recording.
    pub(crate) fn record_click(&mut self, cx: &mut Context<Self>) {
        let busy = {
            let s = self.studio.read(cx);
            s.is_taking() || s.is_recording()
        };
        if self.studio_take_countdown.is_some() || self.studio.read(cx).is_taking() {
            self.studio_take(cx);
        } else if busy {
            self.studio.update(cx, |s, cx| s.toggle_recording(cx));
        } else {
            match self.prefs.record_mode {
                crate::prefs::RecordMode::Program => self.studio.update(cx, |s, cx| s.toggle_recording(cx)),
                crate::prefs::RecordMode::StudioTake => self.studio_take(cx),
            }
        }
    }

    /// [● Record | ⌄]: the record button, and the mode beside it.
    /// (`compact`: idle, just its symbol, the label in the tooltip.)
    fn record_control(&self, rec: &lightair_engine::outputs::OutputStatus, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        use crate::prefs::RecordMode;
        use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
        let studio = self.studio.read(cx);
        let clock = |t: u64| format!("{:02}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60);
        let mode = self.prefs.record_mode;
        // (symbol, label, tooltip)
        let (stop, label, tip): (bool, String, &str) = if let Some(t) = studio.studio_take_elapsed() {
            let time = clock(t.as_secs());
            (true, if studio.studio_take_paused() { format!("❚❚ {time}") } else { time }, "Stop the Studio take")
        } else if let Some(started) = self.studio_take_countdown {
            (true, format!("{}", 3u64.saturating_sub(started.elapsed().as_secs()).max(1)), "Cancel")
        } else {
            match rec.state {
                OutputState::Active { since } | OutputState::Reconnecting { since } => {
                    let time = clock(rec.recorded(since).as_secs());
                    (true, if rec.paused { format!("❚❚ {time}") } else { time }, "Stop recording")
                }
                OutputState::Starting | OutputState::Stopping => (true, "…".into(), ""),
                _ if studio.is_recording() => (true, "Audio".into(), "Stop recording"),
                _ => match mode {
                    RecordMode::Program => (false, "Record".into(), if cfg!(windows) { "Record the show as it's on air (Ctrl+R)" } else { "Record the show as it's on air (⌘R)" }),
                    RecordMode::StudioTake => (false, "Studio take".into(), if cfg!(windows) { "Record each screen, camera and the pointer, for the Editor (Ctrl+R)" } else { "Record each screen, camera and the pointer, for the Editor (⌘R)" }),
                },
            }
        };
        let active = stop;
        let symbol = if stop {
            div().size(px(9.)).rounded(px(2.)).bg(if active { rgb(0xFFFFFF).into() } else { tok::live() })
        } else {
            div().size(px(10.)).rounded_full().bg(tok::live())
        };
        let me = cx.entity().downgrade();
        let main = div()
            .id("record")
            .flex()
            .items_center()
            .gap_1p5()
            .h_full()
            .pl_2p5()
            .pr_2()
            .rounded_l(px(7.))
            .cursor_pointer()
            .when(active, |d| d.bg(tok::live()).text_color(rgb(0xFFFFFF)))
            .when(!active, |d| d.hover(|d| d.bg(tok::bg_selected())))
            .child(symbol)
            .when(!(compact && !stop), |d| d.child(div().text_size(px(13.)).when(active, |d| d.font_family(crate::ui::MONO)).child(label)))
            .when(!tip.is_empty(), |d| d.tooltip(move |w, cx| Tooltip::new(tip).build(w, cx)))
            .on_click(cx.listener(|this, _, _, cx| this.record_click(cx)));
        let chevron = Button::new("record-mode")
            .icon(ui::icon("chevron-down"))
            .xsmall()
            .ghost()
            .disabled(active)
            .tooltip("What Record records")
            .dropdown_menu(move |menu, _, _| {
                let pick = |m: RecordMode| {
                    let me = me.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        if let Some(me) = me.upgrade() {
                            me.update(cx, |this, cx| {
                                this.prefs.record_mode = m;
                                let _ = crate::prefs::save_to(&crate::prefs::path(), &this.prefs);
                                cx.notify();
                            });
                        }
                    }
                };
                menu.item(PopupMenuItem::new("Record").icon(ui::icon("circle")).checked(mode == RecordMode::Program).on_click(pick(RecordMode::Program)))
                    .item(
                        PopupMenuItem::new("Studio take (for the Editor)")
                            .icon(ui::icon("mouse-pointer-2"))
                            .checked(mode == RecordMode::StudioTake)
                            .on_click(pick(RecordMode::StudioTake)),
                    )
            });
        div()
            .flex()
            .items_center()
            .h(px(30.))
            .rounded(px(8.))
            .bg(tok::bg_panel())
            .border_1()
            .border_color(if active { tok::live() } else { tok::border() })
            .child(main)
            .child(div().w(px(1.)).h(px(16.)).bg(tok::border()))
            .child(div().px_0p5().child(chevron))
            .into_any_element()
    }

    /// [Clip | Replay]: saving the last seconds, and playing the last clip on air, in one
    /// container like Record's.
    fn clip_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let studio = self.studio.read(cx);
        let ready = studio.clips_ready();
        let replay = studio.last_clip.is_some();
        let clip = ui::icon_button("clip", "clapperboard", if ready { tok::text() } else { tok::text_tertiary() })
            .size(px(28.))
            .rounded(px(6.))
            .tooltip(ui::tip(if ready { "Save the last seconds as a clip" } else { "Clip: turn on Clips in Outputs to save the last seconds" }))
            .when(ready, |d| d.on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.save_clip(cx)))));
        div()
            .flex()
            .items_center()
            .h(px(30.))
            .px_0p5()
            .gap_0p5()
            .rounded(px(8.))
            .bg(tok::bg_panel())
            .border_1()
            .border_color(tok::border())
            .child(clip)
            .when(replay, |d| {
                d.child(div().w(px(1.)).h(px(16.)).bg(tok::border())).child(
                    ui::icon_button("replay", "rewind", tok::text())
                        .size(px(28.))
                        .rounded(px(6.))
                        .tooltip(ui::tip("Play the last clip on the live scene"))
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.replay_last_clip(cx)))),
                )
            })
            .into_any_element()
    }

    /// [camera | ⌄]: the virtual camera (Lightair as a camera in other apps). Off: the icon;
    /// on: lit, with a pulse dot; failed: the reason on hover, a click tries again. The menu
    /// picks which canvas it shows.
    fn virtual_camera_control(&self, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
        let studio = self.studio.read(cx);
        let state = studio.virtual_camera();
        let workspace = studio.workspace();
        let shown = workspace.outputs.virtual_camera.canvas.and_then(|c| workspace.canvas(c)).unwrap_or_else(|| workspace.primary_canvas());
        let canvas_name = shown.name.clone();
        let listed = studio.virtual_camera_name;
        let chosen_scene = workspace.outputs.virtual_camera.scene.and_then(|id| workspace.scene(id));
        // what it shows, said plainly: a scene, or the one on air (and on which canvas, if several)
        let mut shown_name = chosen_scene.map_or_else(|| "the scene on air".to_string(), |s| format!("“{}”", s.name));
        let scenes: Vec<(Id, String, bool)> =
            workspace.scenes.iter().map(|s| (s.id, s.name.clone(), chosen_scene.is_some_and(|c| c.id == s.id))).collect();
        let on_air = chosen_scene.is_none();
        let canvases: Vec<(Id, String, bool)> = workspace.canvases.iter().map(|c| (c.id, c.name.clone(), c.id == shown.id)).collect();
        let several = canvases.len() > 1;
        if several {
            shown_name = format!("{shown_name} ({canvas_name})");
        }
        let on = matches!(state, OutputState::Active { .. });
        let busy = matches!(state, OutputState::Starting | OutputState::Stopping);
        let installed = lightair_engine::vcam::installed();
        let ours_installable = lightair_engine::vcam::lightair_installable();
        let failed = match &state {
            OutputState::Failed(e) => Some(e.clone()),
            _ => None,
        };
        let tip: SharedString = if on {
            format!("Virtual camera on, showing {shown_name}. In other apps, choose “{listed}”. Click to stop.").into()
        } else if busy {
            "Virtual camera starting…".into()
        } else if let Some(e) = failed.as_ref().filter(|e| e.as_str() == lightair_engine::vcam::NOT_INSTALLED) {
            format!("Virtual camera: {e} Install it from the menu (⌄): Windows asks for an administrator's approval.").into()
        } else if let Some(e) = &failed {
            format!("Virtual camera: {e} Click to try again.").into()
        } else {
            "Start the virtual camera: Lightair as a camera in Zoom, Meet, FaceTime and other apps".into()
        };
        let colour = if on {
            rgb(0xFFFFFF).into()
        } else if failed.is_some() {
            tok::warn()
        } else if busy {
            tok::text_tertiary()
        } else {
            tok::text()
        };
        let main = div()
            .id("virtual-camera")
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .w(px(32.))
            .rounded_l(px(7.))
            .cursor_pointer()
            .when(on, |d| d.bg(tok::ok()))
            .when(!on, |d| d.hover(|d| d.bg(tok::bg_selected())))
            .child(ui::icon("lightair-virtual-camera").size(px(16.)).text_color(colour))
            // on: a small dot, like a camera's light
            .when(on, |d| d.child(div().absolute().top(px(5.)).right(px(5.)).size(px(5.)).rounded_full().bg(rgb(0xFFFFFF))))
            .tooltip(ui::tip(tip))
            .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.toggle_virtual_camera(cx))));
        let me = cx.entity().downgrade();
        let chevron = Button::new("virtual-camera-menu").icon(ui::icon("chevron-down")).xsmall().ghost().dropdown_menu(move |mut menu, _, _| {
            let act = |f: fn(&mut Studio, &mut Context<Studio>)| {
                let me = me.clone();
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |this, cx| this.studio.update(cx, f));
                    }
                }
            };
            menu = menu.item(PopupMenuItem::label("Virtual camera")).item(
                PopupMenuItem::new(if on || busy { "Stop" } else { "Start" })
                    .icon(ui::icon(if on || busy { "square" } else { "play" }))
                    .on_click(act(|s, cx| s.toggle_virtual_camera(cx))),
            );
            // Windows: Lightair's camera isn't installed (OBS's may be): install it
            if cfg!(windows) && (!installed || ours_installable) {
                menu = menu.item(
                    PopupMenuItem::new(if ours_installable { "Install Lightair Virtual Camera…" } else { "Install virtual camera…" })
                        .icon(ui::icon("download"))
                        .on_click(act(|s, cx| s.install_virtual_camera(cx))),
                );
            }
            // which scene: the one on air, or a scene of its own
            menu = menu.separator().item(PopupMenuItem::label("Shows")).item(
                PopupMenuItem::new("Scene on air").icon(ui::icon("radio")).checked(on_air).on_click({
                    let me = me.clone();
                    move |_, _, cx| {
                        if let Some(me) = me.upgrade() {
                            me.update(cx, |this, cx| this.studio.update(cx, |s, cx| s.set_virtual_camera_scene(None, cx)));
                        }
                    }
                }),
            );
            for (id, name, chosen) in scenes.clone() {
                let me = me.clone();
                menu = menu.item(PopupMenuItem::new(name).checked(chosen).on_click(move |_, _, cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |this, cx| this.studio.update(cx, |s, cx| s.set_virtual_camera_scene(Some(id), cx)));
                    }
                }));
            }
            if several {
                menu = menu.separator().item(PopupMenuItem::label("Canvas"));
                for (id, name, chosen) in canvases.clone() {
                    let me = me.clone();
                    menu = menu.item(PopupMenuItem::new(name).checked(chosen).on_click(move |_, _, cx| {
                        if let Some(me) = me.upgrade() {
                            me.update(cx, |this, cx| this.studio.update(cx, |s, cx| s.set_virtual_camera_canvas(Some(id), cx)));
                        }
                    }));
                }
            }
            menu.separator().item(PopupMenuItem::label(format!("In other apps: “{listed}”")))
        });
        div()
            .flex()
            .items_center()
            .h(px(30.))
            .rounded(px(8.))
            .bg(tok::bg_panel())
            .border_1()
            .border_color(if on { tok::ok() } else { tok::border() })
            .child(main)
            .child(div().w(px(1.)).h(px(16.)).bg(tok::border()))
            .child(div().px_0p5().child(chevron))
            .into_any_element()
    }

    /// The workspace's name, as a menu of the workspaces to open, and New Workspace.
    fn workspace_picker(&self, workspace: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
        let (me, current) = (cx.entity().downgrade(), self.studio.read(cx).workspace_dir());
        let button = Button::new("workspace-picker")
            .label(workspace.name.clone())
            .small()
            .ghost()
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                // read when the menu opens: workspaces come and go
                for p in crate::library::list_workspaces(current.as_deref()) {
                    let (me, dir) = (me.clone(), p.dir.clone());
                    let open = current.as_ref() == Some(&p.dir);
                    menu = menu.item(PopupMenuItem::new(p.name).checked(open).on_click(move |_, _, cx| {
                        if !open && let Some(me) = me.upgrade() {
                            me.update(cx, |this, cx| this.open_workspace(dir.clone(), cx));
                        }
                    }));
                }
                let (new, library) = (me.clone(), me.clone());
                menu.separator()
                    .item(PopupMenuItem::new("New Workspace…").icon(ui::icon("plus")).on_click(move |_, window, cx| {
                        if let Some(me) = new.upgrade() {
                            me.update(cx, |this, cx| {
                                this.show_place(Place::Library, cx);
                                this.library.update(cx, |l, cx| l.start_new_workspace(window, cx));
                            });
                        }
                    }))
                    .item(PopupMenuItem::new("All Workspaces in the Library").icon(ui::icon("library")).on_click(move |_, _, cx| {
                        if let Some(me) = library.upgrade() {
                            me.update(cx, |this, cx| this.show_place(Place::Library, cx));
                        }
                    }))
            });
        // in a container like the switcher's
        div().p_0p5().rounded(px(8.)).bg(tok::bg_panel()).border_1().border_color(tok::border()).child(button)
    }

    /// Library · Live · Editor.
    fn place_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str, label: &'static str, place: Place, key: &'static str| {
            let on = self.place == place;
            div()
                .id(id)
                .px_2p5()
                .py_0p5()
                .rounded(px(6.))
                .text_size(px(12.))
                .cursor_pointer()
                .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()).font_weight(FontWeight::MEDIUM))
                .when(!on, |d| d.text_color(tok::text_secondary()))
                .hover(|d| d.text_color(tok::text()))
                .tooltip(move |w, cx| Tooltip::new(format!("{label} ({})", key_label(key))).build(w, cx))
                .on_click(cx.listener(move |this, _, _, cx| this.show_place(place, cx)))
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded(px(8.))
            .bg(tok::bg_panel())
            .border_1()
            .border_color(tok::border())
            .child(tab("place-library", "Library", Place::Library, PLACE_KEYS[0]))
            .child(tab("place-live", "Live", Place::Live, PLACE_KEYS[1]))
            .child(tab("place-editor", "Editor", Place::Editor, PLACE_KEYS[2]))
    }

    /// The title bar, laid out for the window's `width`: full; compact (the performance readout
    /// as its dot, Record as its symbol) when that doesn't fit beside the centred Library · Live
    /// · Editor; and narrow (the switcher in the row, not centred) when even that doesn't.
    fn title_bar(&self, workspace: &Workspace, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.place == Place::Editor;
        let studio = self.studio.read(cx);
        let has_destination = workspace.outputs.destinations.iter().any(|d| d.enabled);

        let taking = studio.is_taking();
        let take_paused = studio.studio_take_paused();
        let rec_status = runtime().recording();
        let rec = rec_status.state.clone();
        let recording = matches!(rec, OutputState::Active { .. }) || taking;
        // what fits: each side's width, roughly, against half of what the switcher leaves
        let streaming_now = runtime().any_stream_running();
        let half = (width - 210.0) / 2.0;
        let left = 84.0 + 7.5 * workspace.name.chars().count() as f32 + 40.0 + if streaming_now { 52.0 } else { 0.0 } + if editor { 160.0 } else { 0.0 };
        let right = |compact: bool| -> f32 {
            if editor {
                return 260.0;
            }
            let perf = if compact { 28.0 } else { 150.0 };
            let live = if streaming_now { 130.0 } else { 76.0 };
            let tools = if recording && !taking { 84.0 } else { 0.0 };
            let record = if compact && !recording && !taking { 58.0 } else { 160.0 };
            // [Clip | Replay] and [virtual camera | ⌄]
            let clips = if self.studio.read(cx).last_clip.is_some() { 66.0 } else { 36.0 };
            12.0 + perf + live + tools + record + clips + 64.0 + 28.0 + 12.0 * 7.0
        };
        let compact = left > half || right(false) > half;
        let narrow = left > half || right(true) > half;
        let record = self.record_control(&rec_status, compact, cx);
        let paused = if taking { take_paused } else { rec_status.paused };
        let recording_tools = recording.then(|| {
            div()
                .flex()
                .items_center()
                .child(
                    ui::icon_button("rec-pause", if paused { "play" } else { "pause" }, tok::text_secondary())
                        .tooltip(move |w, cx| {
                            Tooltip::new(if paused { "Resume recording" } else { "Pause recording" }).build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.pause_recording(cx)))),
                )
                .child(
                    ui::icon_button("rec-marker", "bookmark", tok::text_secondary())
                        .tooltip(|w, cx| Tooltip::new(crate::ui::keys_label("Add marker (⌘M)").into_owned()).build(w, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.add_marker(cx)))),
                )
                .when(!taking, |d| d.child(
                    ui::icon_button("rec-split", "split", tok::text_secondary())
                        .tooltip(|w, cx| Tooltip::new("Continue in a new file").build(w, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.split_recording(cx)))),
                ))
        });

        let live_since = runtime().stream_since();
        let streaming = runtime().any_stream_running();
        let armed = self.confirm_end.is_some_and(|t| t.elapsed().as_secs_f32() < 3.0);
        let go_live = if streaming {
            let label = if armed {
                "Click again to end".to_string()
            } else {
                live_since.map(|t| format!("Live {}", elapsed(t))).unwrap_or_else(|| "Going live…".into())
            };
            Button::new("go-live").label(label).small().danger().on_click(cx.listener(|this, _, _, cx| {
                if !this.prefs.confirm_end_stream || this.confirm_end.is_some_and(|t| t.elapsed().as_secs_f32() < 3.0) {
                    this.confirm_end = None;
                    this.studio.update(cx, |s, cx| s.end_stream(cx));
                } else {
                    this.confirm_end = Some(Instant::now());
                    cx.notify();
                }
            }))
        } else {
            Button::new("go-live")
                .label("Go live")
                .small()
                .primary()
                .disabled(!has_destination)
                .tooltip(if has_destination {
                    "Start every enabled destination"
                } else {
                    "Add a destination in Outputs first"
                })
                .on_click(cx.listener(|this, _, _, cx| this.open_go_live(cx)))
        };

        div()
            .relative()
            .flex()
            .items_center()
            .h(px(44.))
            .pl(px(84.))
            .pr_3()
            .gap_3()
            .border_b_1()
            .border_color(tok::border())
            .bg(tok::bg_app())
            // Library · Live · Editor, in the middle of the bar whatever is either side (in the
            // row when the window is too narrow for that)
            .when(!narrow, |d| d.child(div().absolute().inset_0().flex().items_center().justify_center().child(self.place_switcher(cx))))
            .child(div().min_w_0().flex_shrink_1().overflow_hidden().child(self.workspace_picker(workspace, cx)))
            .when(narrow, |d| d.child(div().flex_none().child(self.place_switcher(cx))))
            // the Editor: its workspace beside the workspace
            .when(editor, |d| d.child(self.editor.update(cx, |e, cx| e.title_left(cx))))
            .when(streaming, |d| {
                d.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .bg(tok::live())
                        .text_size(px(10.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child("LIVE"),
                )
            })
            .child(div().flex_1())
            // the Editor: undo, redo and export here; what records and streams only while it does
            .when(editor, |d| d.child(self.editor.update(cx, |e, cx| e.title_right(cx))))
            .when(!editor || streaming, |d| d.child(go_live))
            .children(recording_tools.filter(|_| !taking))
            .when(!editor || recording || taking, |d| d.child(record))
            .when(!editor, |d| d.child(self.clip_control(cx)).child(self.virtual_camera_control(cx)))
            .child(
                ui::icon_button("open-settings", "settings", tok::text_secondary())
                    .tooltip(|w, cx| Tooltip::new(crate::ui::keys_label("Settings (⌘,)").into_owned()).build(w, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
            )
    }

    fn refresh_sizes(&self, cx: &mut Context<Self>) {
        let stale = self.sizes.borrow().0.is_none_or(|t| t.elapsed().as_millis() > 250);
        if stale {
            let sizes = self.studio.read(cx).engine().call(|s| s.reconciler.source_sizes());
            *self.sizes.borrow_mut() = (Some(Instant::now()), sizes);
        }
    }

    /// Where a object's picture actually is in a canvas: its box, narrowed to the picture in Fit
    /// mode. The frame, handles and dragging use this, so they stick to the source.
    /// In canvas terms (a child's layout is in its parent's space). An empty's frame is the box
    /// round what it holds (a small square at its pivot when it holds nothing).
    fn frame_of(&self, scene: &Scene, object: &Object, canvas: Id) -> Option<Layout> {
        let world = scene.world(object.id, canvas)?;
        if object.is_empty() {
            let frames: Vec<Layout> = scene
                .objects
                .iter()
                .filter(|l| !l.is_empty() && scene.is_under(l.id, object.id))
                .filter_map(|l| self.frame_of(scene, l, canvas))
                .collect();
            let bounds =
                frames.iter().map(geo::bounds).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
            let bounds = bounds.map(|(x0, y0, x1, y1)| Layout::boxed(x0, y0, x1 - x0, y1 - y0));
            let (px_, py_) = (world.x, world.y);
            return Some(Layout {
                visible: world.visible && scene.shown(object.id, canvas),
                ..bounds.unwrap_or(Layout::boxed(px_ - 20.0, py_ - 20.0, 40.0, 40.0))
            });
        }
        let size = object.source.and_then(|i| self.sizes.borrow().1.get(&i).copied()).map(|(w, h)| (w as f32, h as f32));
        let mut frame = geo::content_rect(&world, size);
        frame.visible = scene.shown(object.id, canvas);
        Some(frame)
    }

    /// Keeps libobs showing the object being cropped whole (crop mode), or not.
    fn sync_crop_preview(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let want = self
            .crop_mode
            .and_then(|object| studio.selected_scene.map(|scene| (scene, self.canvas_id(&workspace), object)));
        if want != self.crop_preview_sent {
            self.crop_preview_sent = want;
            studio.engine().post(move |s| s.reconciler.set_crop_preview(want, &workspace));
        }
    }

    fn toggle_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_add_object(window, cx);
    }

    /// A transition tile under the pointer plays its preview (redrawn each frame while it is).
    fn hover_transition(&mut self, i: usize, hovered: bool, cx: &mut Context<Self>) {
        if !hovered {
            if self.transition_hover.is_some_and(|(k, _)| k == i) {
                self.transition_hover = None;
                cx.notify();
            }
            return;
        }
        self.transition_hover = Some((i, Instant::now()));
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(16)).await;
                let still = this.update(cx, |this, cx| {
                    cx.notify();
                    this.transition_hover.is_some_and(|(k, _)| k == i)
                });
                if !matches!(still, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// How scenes change on air: a tile per kind, then its settings as rows (duration;
    /// direction for Slide and Wipe; the video and its cut point for Stinger).
    fn transition_picker(&self, workspace: &Workspace, scene: &Scene, cx: &mut Context<Self>) -> impl IntoElement {
        // this scene's own, or the default every switch uses (what the tiles below change)
        let own = scene.transition.is_some();
        let current = scene.transition.unwrap_or(workspace.transition);
        let sid = scene.id;
        let set = move |settings: TransitionSettings| {
            if own { Command::SetSceneTransition { scene: sid, transition: Some(settings) } } else { Command::SetTransition { settings } }
        };
        let scope = self.segmented(
            "tr-scope",
            vec![
                ("All scenes", Command::SetSceneTransition { scene: sid, transition: None }, !own),
                // its own starts as a copy of the default
                ("This scene", Command::SetSceneTransition { scene: sid, transition: Some(current) }, own),
            ],
            cx,
        );
        // the kinds, as tiles
        let tile = |i: usize, kind: TransitionKind| {
            let on = current.kind == kind;
            let about = match kind {
                TransitionKind::Cut => "Switches at once",
                TransitionKind::Fade => "Crossfades to the new scene",
                TransitionKind::Move => "Objects in both scenes glide to their new place; the rest fade",
                TransitionKind::Slide => "The new scene pushes the old one out",
                TransitionKind::Wipe => "The new scene wipes across the old one",
                TransitionKind::Dip => "Fades through black",
                TransitionKind::Stinger => "A video plays over the switch",
            };
            // hovered: it plays (A, the switch, B, again); otherwise it shows the switch midway
            let playing = self.transition_hover.filter(|(k, _)| *k == i).map(|(_, since)| {
                let t = since.elapsed().as_secs_f32() % 1.8;
                ((t - 0.3) / 1.0).clamp(0.0, 1.0)
            });
            let cmd = set(TransitionSettings { kind, ..current });
            div()
                .id(("tr-kind", i))
                .flex()
                .flex_col()
                .gap_1()
                .p(px(3.))
                .pb_1()
                .rounded(px(8.))
                .border_1()
                .cursor_pointer()
                .when(on, |d| d.bg(tok::bg_selected()).border_color(tok::accent_primary()))
                .when(!on, |d| d.bg(tok::bg_app()).border_color(tok::border()).hover(|d| d.border_color(tok::border_strong())))
                .child(transition_preview(kind, current.direction, playing))
                .child(
                    div()
                        .text_center()
                        .text_size(px(11.))
                        .text_color(if on { tok::text() } else { tok::text_secondary() })
                        .child(kind.label()),
                )
                .tooltip(ui::tip(about))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_transition(i, *hovered, cx)))
                .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx)))
        };
        let mut grid = div().grid().grid_cols(4).gap_1p5();
        for (i, k) in TransitionKind::ALL.iter().enumerate() {
            grid = grid.child(tile(i, *k));
        }
        let row = |name: &'static str, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_h(px(28.))
                .child(div().w(px(72.)).flex_none().text_size(px(12.)).text_color(tok::text_secondary()).child(name))
                .child(div().flex_1().min_w_0().flex().items_center().child(control))
        };
        let others = workspace.scenes.iter().filter(|s| s.id != sid && s.transition.is_some()).count();
        // (worth saying only when other scenes go their own way)
        let hint = (!own && others > 0).then(|| {
            format!("Except switching to the {others} scene{} with {} own", if others == 1 { "" } else { "s" }, if others == 1 { "its" } else { "their" })
        });
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(scope)
            .children(hint.map(|h| div().text_size(px(11.)).text_color(tok::text_tertiary()).child(h)))
            .child(grid);
        // duration (a cut has none)
        if current.kind != TransitionKind::Cut {
            let durations: Vec<(&'static str, Command, bool)> = [(300u32, "0.3 s"), (500, "0.5 s"), (800, "0.8 s"), (1200, "1.2 s")]
                .into_iter()
                .map(|(ms, label)| (label, set(TransitionSettings { duration_ms: ms, ..current }), current.duration_ms == ms))
                .collect();
            body = body.child(row("Duration", self.segmented("tr-duration", durations, cx).flex_1().into_any_element()));
        }
        if current.kind.directional() {
            let dirs: Vec<(&'static str, Command, bool)> = [(Direction::Left, "←"), (Direction::Right, "→"), (Direction::Up, "↑"), (Direction::Down, "↓")]
                .into_iter()
                .map(|(d, label)| (label, set(TransitionSettings { direction: d, ..current }), current.direction == d))
                .collect();
            body = body.child(row("Direction", self.segmented("tr-direction", dirs, cx).flex_1().into_any_element()));
        }
        // Stinger: its video, and when the scene changes underneath it
        if current.kind == TransitionKind::Stinger {
            // this scene's own video when it has its own transition (the show's until it picks one)
            let target = own.then_some(sid);
            let st = if own { scene.stinger.clone().or(workspace.stinger.clone()) } else { workspace.stinger.clone() };
            let shows_default = own && scene.stinger.is_none() && st.is_some();
            let name = st
                .as_ref()
                .and_then(|s| std::path::Path::new(&s.path).file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "Choose a video…".into());
            let name = if shows_default { format!("{name} (the show's)") } else { name };
            body = body.child(row(
                "Video",
                Button::new("stinger-file")
                    .label(name)
                    .icon(ui::icon("film"))
                    .small()
                    .outline()
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| this.choose_stinger(target, window, cx)))
                    .into_any_element(),
            ));
            if let Some(st) = st {
                let step = |delta: i64| {
                    let mut s = st.clone();
                    s.cut_ms = (s.cut_ms as i64 + delta).clamp(0, 10_000) as u32;
                    match target {
                        Some(scene) => Command::SetSceneStinger { scene, stinger: Some(s) },
                        None => Command::SetStinger { stinger: Some(s) },
                    }
                };
                let (earlier, later) = (step(-100), step(100));
                let stepper = div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .p(px(2.))
                    .rounded(px(7.))
                    .bg(tok::bg_app())
                    .child(
                        ui::icon_button("stinger-earlier", "minus", tok::text_secondary())
                            .on_click(cx.listener(move |this, _, _, cx| this.exec(earlier.clone(), cx))),
                    )
                    .child(div().w(px(56.)).text_center().text_size(px(12.)).font_family(crate::ui::MONO).child(format!("{:.1} s", st.cut_ms as f32 / 1000.0)))
                    .child(
                        ui::icon_button("stinger-later", "plus", tok::text_secondary())
                            .on_click(cx.listener(move |this, _, _, cx| this.exec(later.clone(), cx))),
                    );
                body = body.child(row(
                    "Cut at",
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(stepper)
                        .child(ui::info("stinger-cut-info", "When the scene changes underneath the video: when it covers the picture."))
                        .into_any_element(),
                ));
            }
        }
        body
    }

    fn scenes_panel(&self, workspace: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        let studio = self.studio.read(cx);
        let selected = studio.selected_scene;
        let program = workspace.program;
        let shell = cx.entity().downgrade();
        let rows = workspace.scenes.iter().enumerate().map(|(i, scene)| {
            let id = scene.id;
            let on_air = program == Some(id);
            let target = RowTarget::Scene(id);
            let dragged = DraggedRow { target, name: scene.name.clone().into(), icon: "layout-dashboard" };
            let name = scene.name.clone();
            div()
                .id(("scene", i))
                .relative()
                .flex()
                .items_center()
                .gap_2()
                .h(px(36.))
                .mx_2()
                .px_3()
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected == Some(id), |d| d.bg(tok::bg_selected()))
                .hover(|d| d.bg(tok::bg_raised()))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() == 2 {
                        this.start_rename(target, &name, window, cx);
                        return;
                    }
                    let cue_scenes = this.cue_scenes;
                    this.studio.update(cx, |s, cx| {
                        s.select_scene(id);
                        if cue_scenes {
                            cx.notify();
                        } else {
                            let _ = s.execute(Command::SetProgram { scene: Some(id) }, cx);
                        }
                    })
                }))
                .on_drag(dragged, |row, _, _, cx| cx.new(|_| row.clone()))
                .drag_over::<DraggedRow>(|style, _, _, _| style.bg(tok::bg_selected()))
                .on_drop(cx.listener(move |this, row: &DraggedRow, _, cx| this.drop_row(row, target, i, cx)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        this.studio.update(cx, |s, cx| {
                            s.select_scene(id);
                            cx.notify();
                        })
                    }),
                )
                .when(on_air, |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(0.))
                            .top(px(8.))
                            .bottom(px(8.))
                            .w(px(3.))
                            .rounded_full()
                            .bg(tok::live()),
                    )
                })
                .child(self.row_label(target, &scene.name, tok::text()))
                .when(on_air, |d| {
                    d.child(
                        div()
                            .text_size(px(10.))
                            .text_color(tok::live())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("ON AIR"),
                    )
                })
                .when(i < 9, |d| {
                    d.child(
                        div()
                            .text_size(px(10.))
                            .font_family(crate::ui::MONO)
                            .text_color(tok::text_tertiary())
                            .child(crate::ui::keys_label(&format!("⌘{}", i + 1)).into_owned()),
                    )
                })
                .context_menu({
                    let shell = shell.clone();
                    move |m, _, cx| menus::scene_menu(id, m, &shell, cx)
                })
        });
        div().flex().flex_col().size_full().pt_1().child(
            div()
                .id("scenes-scroll")
                // a scene from Assets: added as a new scene
                .drag_over::<assets::DraggedAsset>(|s, _, _, _| s.bg(tok::bg_raised()))
                .on_drop(cx.listener(|this, a: &assets::DraggedAsset, _, cx| {
                    if a.0.kind == assets::AssetKind::Scene {
                        this.add_scene_asset(&a.0, cx);
                    }
                }))
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .gap_0p5()
                .overflow_y_scroll()
                .children(rows)
                // Empty space below: its own menu.
                .child(
                    div()
                        .id("scenes-end")
                        .flex_1()
                        .min_h(px(24.))
                        .on_click(cx.listener(|this, _, _, cx| this.select_object(None, cx)))
                        .context_menu({
                            let shell = cx.entity().downgrade();
                            move |m, _, _| menus::scenes_blank_menu(m, &shell)
                        }),
                ),
        )
    }

    /// The canvas's floating controls, bottom right: side by side (with several canvases), zoom
    /// and safe areas.
    fn canvas_controls(&self, workspace: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        let divider = || div().w(px(1.)).h(px(16.)).mx_0p5().bg(tok::border_header());
        let mut bar = div()
            .id("canvas-controls")
            .absolute()
            .right(px(12.))
            .bottom(px(12.))
            .flex()
            .items_center()
            .gap_0p5()
            .p_1()
            .rounded(px(8.))
            .bg(tok::bg_header())
            .border_1()
            .border_color(tok::border_header())
            .shadow_md()
            // Clicks here are for the controls, not the canvas under them.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation());
        if workspace.canvases.len() > 1 {
            bar = bar
                .child(
                    ui::icon_button(
                        "side-by-side",
                        "columns-2",
                        if self.side_by_side { tok::text() } else { tok::text_secondary() },
                    )
                    .when(self.side_by_side, |d| d.bg(tok::bg_selected()))
                    .tooltip(|w, cx| Tooltip::new("Every canvas side by side").build(w, cx))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.side_by_side = !this.side_by_side;
                        cx.notify();
                    })),
                )
                .child(divider());
        }
        let shape = self.current_canvas(workspace).clone();
        let vertical = shape.height > shape.width;
        // Zoom: − 100% +, the number fits the canvas to the stage again.
        let percent = self.zoom_percent(workspace);
        let fitted = (self.zoom - 1.0).abs() < 0.001 && self.pan == (0.0, 0.0);
        let zoom = div()
            .flex()
            .items_center()
            .gap_0p5()
            .child(
                ui::icon_button("zoom-out", "minus", tok::text_secondary())
                    .tooltip(|w, cx| Tooltip::new(crate::ui::keys_label("Zoom out (⌘−)").into_owned()).build(w, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(0.8, None, cx))),
            )
            .child(
                div()
                    .id("zoom-fit")
                    .min_w(px(46.))
                    .h(px(22.))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .font_family(crate::ui::MONO)
                    .text_color(if fitted { tok::text_secondary() } else { tok::text() })
                    .hover(|d| d.bg(tok::bg_raised()))
                    .child(format!("{percent:.0}%"))
                    .tooltip(|w, cx| Tooltip::new(crate::ui::keys_label("Fit (⌘0) · double-click: 100% (⌥⌘0)").into_owned()).build(w, cx))
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                        if ev.click_count() == 2 { this.zoom_to_actual(cx) } else { this.zoom_to_fit(cx) }
                    })),
            )
            .child(
                ui::icon_button("zoom-in", "plus", tok::text_secondary())
                    .tooltip(|w, cx| Tooltip::new(crate::ui::keys_label("Zoom in (⌘=)").into_owned()).build(w, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.25, None, cx))),
            );
        let safe = if vertical { self.safe_vertical } else { self.safe_landscape };
        bar = bar.child(zoom).child(divider()).child(
            div()
                .id("safe-areas")
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(24.))
                .px_1p5()
                .rounded(px(5.))
                .cursor_pointer()
                .text_size(px(11.))
                .when(safe.is_some(), |d| d.bg(tok::bg_selected()))
                .hover(|d| d.bg(tok::bg_raised()))
                .child(ui::icon("lightair-safe-areas").size(px(14.)).text_color(tok::text_secondary()))
                // The icon alone until a profile is on; then its name.
                .children(safe.map(Profile::label))
                .tooltip(|w, cx| {
                    Tooltip::new("Where platforms draw their own buttons and captions (approximate)").build(w, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next = safe_areas::next(safe, shape.width, shape.height);
                    if vertical {
                        this.safe_vertical = next;
                    } else {
                        this.safe_landscape = next;
                    }
                    cx.notify();
                })),
        );
        bar
    }

    fn canvas_tabs(&self, workspace: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.canvas_id(workspace);
        let mut bar = div()
            .flex()
            .items_center()
            .gap_1()
            .h(px(36.))
            .px_3()
            .bg(tok::bg_header())
            .border_b_1()
            .border_color(tok::border_header());
        for (i, f) in workspace.canvases.iter().enumerate() {
            let id = f.id;
            let is_active = !self.side_by_side && id == active;
            let (gw, gh) = if f.width >= f.height { (14.0, 9.0) } else { (8.0, 13.0) };
            bar = bar.child(
                div()
                    .id(("canvas-tab", i))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(26.))
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(is_active, |d| d.bg(tok::bg_selected()))
                    .hover(|d| d.bg(tok::bg_raised()))
                    .child(div().w(px(gw)).h(px(gh)).rounded(px(2.)).border_1().border_color(tok::text_secondary()))
                    .child(format!("{} {}×{}", f.name, f.width, f.height))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_canvas = Some(id);
                        this.side_by_side = false;
                        cx.notify();
                    }))
                    .when(i > 0, |d| {
                        d.child(
                            ui::icon_button(("canvas-remove", i), "x", tok::text_tertiary())
                                .size(px(16.))
                                .tooltip(|w, cx| Tooltip::new("Remove this canvas (undo brings it back)").build(w, cx))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if this.active_canvas == Some(id) {
                                        this.active_canvas = None;
                                    }
                                    this.exec(Command::RemoveCanvas { canvas: id }, cx);
                                })),
                        )
                    }),
            );
        }
        // + right after the tabs: another shape for every scene.
        let open = self.canvas_menu.is_some();
        bar = bar.child(
            ui::icon_button("add-canvas", "plus", if open { tok::text() } else { tok::text_secondary() })
                .when(open, |d| d.bg(tok::bg_selected()))
                .tooltip(|w, cx| Tooltip::new("Add canvas (vertical, square, custom…)").build(w, cx))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.canvas_menu = match this.canvas_menu.take() {
                        Some(_) => None,
                        None => {
                            let w = cx
                                .new(|cx| gpui_kit::component::input::InputState::new(window, cx).placeholder("Width"));
                            let h = cx.new(|cx| {
                                gpui_kit::component::input::InputState::new(window, cx).placeholder("Height")
                            });
                            Some((w, h))
                        }
                    };
                    cx.notify();
                })),
        );
        bar = bar.child(div().flex_1());
        bar = bar.child(
            div()
                .id("studio-mode")
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(26.))
                .px_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(12.))
                .when(self.cue_scenes, |d| d.bg(tok::bg_selected()))
                .hover(|d| d.bg(tok::bg_raised()))
                .child(ui::icon("lightair-cue").size(px(14.)).text_color(tok::text_secondary()))
                .child("Cue scenes")
                .tooltip(|w, cx| {
                    Tooltip::new(crate::ui::keys_label("Edit the next scene without viewers seeing it, then Take it live (⌘↩)").into_owned()).build(w, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| this.set_cue_scenes(!this.cue_scenes, cx))),
        );
        let Some((w_in, h_in)) = &self.canvas_menu else { return div().w_full().child(bar) };
        let has = |w: u32, h: u32| workspace.canvases.iter().any(|f| f.width == w && f.height == h);
        let mut row =
            div().flex().flex_wrap().items_center().gap_1().px_3().py_2().border_b_1().border_color(tok::border());
        for (i, f) in [Canvas::vertical(), Canvas::square(), Canvas::portrait()].into_iter().enumerate() {
            if has(f.width, f.height) {
                continue;
            }
            let label = format!("{} {}×{}", f.name, f.width, f.height);
            row = row.child(
                Button::new(("canvas-preset", i))
                    .label(label)
                    .xsmall()
                    .outline()
                    .on_click(cx.listener(move |this, _, _, cx| this.add_canvas(f.clone(), cx))),
            );
        }
        let (w_in, h_in) = (w_in.clone(), h_in.clone());
        row = row
            .child(div().w(px(8.)))
            .child(div().w(px(72.)).child(gpui_kit::component::input::Input::new(&w_in).xsmall()))
            .child("×")
            .child(div().w(px(72.)).child(gpui_kit::component::input::Input::new(&h_in).xsmall()))
            .child(Button::new("canvas-custom").label("Add custom").xsmall().ghost().on_click(cx.listener(
                move |this, _, _, cx| {
                    let read = |e: &Entity<gpui_kit::component::input::InputState>, cx: &App| {
                        e.read(cx).value().trim().parse::<u32>().ok()
                    };
                    match (read(&w_in, cx), read(&h_in, cx)) {
                        (Some(w), Some(h)) if w >= 16 && h >= 16 => {
                            let f = Canvas::custom("Custom", w, h);
                            this.add_canvas(f, cx)
                        }
                        _ => this.studio.update(cx, |s, cx| {
                            s.issue = Some("Enter a width and height in pixels (16 or more).".into());
                            cx.notify();
                        }),
                    }
                },
            )));
        div().w_full().child(bar).child(row)
    }

    /// One canvas's stage: live preview plus selection overlay and guides.
    fn stage(&self, workspace: &Workspace, shown: &Canvas, cx: &mut Context<Self>) -> impl IntoElement {
        let studio = self.studio.read(cx);
        let bounds_map = self.stage_bounds.clone();
        let fid = shown.id;
        let native = self.native.borrow().is_some();
        // drawn under the UI (native_preview): only whether it has started is needed here, and
        // asking for a frame would have the preview read every frame back
        let frame = match self.previews.get(&fid) {
            Some(p) if native => (p.frames() > 0).then_some(None),
            Some(p) => p.latest_frame().map(Some),
            None => None,
        };
        let editing = fid == self.canvas_id(workspace);

        let (rects, zoom, pan, (cw, ch)) = (self.native_rects.clone(), self.zoom, self.pan, (shown.width as f32, shown.height as f32));
        let mut stage = div()
            .absolute()
            .top(px(16.))
            .left(px(16.))
            .right(px(16.))
            .bottom(px(16.))
            .child(
                canvas(
                    move |b, _, _| {
                        bounds_map.borrow_mut().insert(fid, b);
                        // where the picture is this frame (as `mapping` works it out)
                        let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
                        let s = (bw / cw).min(bh / ch) * zoom;
                        let x = f32::from(b.origin.x) + (bw - cw * s) / 2.0 + pan.0;
                        let y = f32::from(b.origin.y) + (bh - ch * s) / 2.0 + pan.1;
                        rects.borrow_mut().insert(fid, Bounds::new(point(px(x), px(y)), size(px(cw * s), px(ch * s))));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(match frame {
                // drawn under the UI (native_preview)
                Some(None) => div().into_any_element(),
                Some(Some(f)) => match (self.mapping(workspace, fid), self.stage_bounds.borrow().get(&fid).copied()) {
                    // Placed where the shown is (zoomed and panned).
                    (Some((ox, oy, s)), Some(b)) => crate::frame_view::frame(f)
                        .absolute()
                        .left(px(ox - f32::from(b.origin.x)))
                        .top(px(oy - f32::from(b.origin.y)))
                        .w(px(shown.width as f32 * s))
                        .h(px(shown.height as f32 * s))
                        .object_fit(ObjectFit::Fill)
                        .into_any_element(),
                    _ => crate::frame_view::frame(f).absolute().size_full().object_fit(ObjectFit::Contain).into_any_element(),
                },
                None => div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(tok::text_tertiary())
                    .child("Starting preview…")
                    .into_any_element(),
            });

        let bounds = self.stage_bounds.borrow().get(&fid).copied();
        if let (Some((ox, oy, s)), Some(b)) = (self.mapping(workspace, fid), bounds) {
            let (lx, ly) = (ox - f32::from(b.origin.x), oy - f32::from(b.origin.y));
            let profile = if shown.height > shown.width { self.safe_vertical } else { self.safe_landscape };
            let (cw, ch) = (shown.width as f32 * s, shown.height as f32 * s);
            for z in profile.map_or(&[][..], Profile::zones) {
                let (zx, zy, zw, zh) = z.rect;
                let zone = div()
                    .absolute()
                    .left(px(lx + zx * cw))
                    .top(px(ly + zy * ch))
                    .w(px(zw * cw))
                    .h(px(zh * ch))
                    .border_1()
                    .border_color(tok::canvas_safe_area())
                    .p_1()
                    .text_size(px(10.))
                    .text_color(tok::canvas_safe_area())
                    .child(z.label);
                stage =
                    stage.child(if z.outline { zone.border_dashed() } else { zone.bg(tok::canvas_safe_area_fill()) });
            }
            let selected = studio
                .selected_scene
                .and_then(|id| workspace.scene(id))
                .and_then(|scene| studio.selected_object.and_then(|l| scene.object(l)))
                .and_then(|l| {
                    let scene = studio.selected_scene.and_then(|id| workspace.scene(id))?;
                    self.frame_of(scene, l, fid).map(|lo| (lo, scene.locked(l.id) || l.is_empty(), l.id))
                });
            if let Some((mf, a, b, _)) = self.marquee
                && mf == fid
            {
                let (x0, y0) = (lx + a.0.min(b.0) * s, ly + a.1.min(b.1) * s);
                let (w, h) = ((a.0 - b.0).abs() * s, (a.1 - b.1).abs() * s);
                stage = stage.child(
                    div()
                        .absolute()
                        .left(px(x0))
                        .top(px(y0))
                        .w(px(w))
                        .h(px(h))
                        .border_1()
                        .border_color(tok::canvas_selection())
                        .bg(tok::canvas_safe_area_fill()),
                );
            }
            // Other selected objects: outlines only.
            let extras: Vec<[(f32, f32); 4]> = studio
                .selected_scene
                .and_then(|id| workspace.scene(id))
                .map(|scene| {
                    studio
                        .also_selected
                        .iter()
                        .filter_map(|id| scene.object(*id))
                        .filter_map(|l| self.frame_of(scene, l, fid))
                        .map(|lo| geo::outline(&lo).map(|(px_, py_)| (lx + px_ * s, ly + py_ * s)))
                        .collect()
                })
                .unwrap_or_default();
            if !extras.is_empty() {
                let colour = tok::canvas_selection();
                stage = stage.child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let o = bounds.origin;
                            for outline in &extras {
                                let mut path = PathBuilder::stroke(px(1.));
                                path.move_to(point(o.x + px(outline[0].0), o.y + px(outline[0].1)));
                                for p in &outline[1..] {
                                    path.line_to(point(o.x + px(p.0), o.y + px(p.1)));
                                }
                                path.close();
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, colour);
                                }
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                );
            }
            if let Some((l, locked, lid)) = selected {
                let colour = if editing { tok::canvas_selection() } else { tok::text_secondary() };
                let to_stage = move |(px_, py_): (f32, f32)| (lx + px_ * s, ly + py_ * s);
                let outline = geo::outline(&l).map(to_stage);
                let cropping = editing && !locked && self.crop_mode == Some(lid) && !self.crop_leaving;
                // The handle being dragged, or else the one under the pointer: lit up.
                let hot: Option<Handle> = match self.drag.as_ref().filter(|d| d.object == lid) {
                    Some(d) => d.mode.handle(self.rotate_tool),
                    None => self.hover_handle,
                };
                let grabbed = self.drag.as_ref().is_some_and(|d| d.object == lid);
                let grabs = self.grabs;
                // A handle's dot: lit when hot, and a quick pop when grabbed.
                let dot = move |(x, y): (f32, f32), base: f32, round: bool, this: Option<Handle>| -> AnyElement {
                    let is_hot = this.is_some() && this == hot;
                    let size = if is_hot { base * 1.3 } else { base };
                    let el = div()
                        .absolute()
                        .left(px(x - size / 2.))
                        .top(px(y - size / 2.))
                        .size(px(size))
                        .when(round, |d| d.rounded_full())
                        .bg(if is_hot { tok::accent_primary() } else { tok::canvas_selection() })
                        .border_1()
                        .border_color(if is_hot { tok::canvas_selection() } else { tok::canvas_selection_edge() });
                    if is_hot && grabbed {
                        el.with_animation(
                            ElementId::from(("grab", grabs)),
                            Animation::new(std::time::Duration::from_millis(260)),
                            move |el, t| {
                                let sz = size + base * 0.55 * bounce(ease_out_quint())(t);
                                el.left(px(x - sz / 2.)).top(px(y - sz / 2.)).size(px(sz))
                            },
                        )
                        .into_any_element()
                    } else {
                        el.into_any_element()
                    }
                };
                // Crop mode: the whole picture (where it would sit uncropped) with the cut-away
                // part as a ghost (drawn by the engine); the shape's outline; handles on it.
                let crop_view = cropping.then(|| {
                    // The full frame needs the source's size; the shape and handles don't.
                    let source = studio
                        .selected_scene
                        .and_then(|id| workspace.scene(id))
                        .and_then(|sc| sc.object(lid))
                        .and_then(|object| object.source);
                    let size = source.and_then(|i| studio.engine().call(move |s| s.reconciler.source_size(i)));
                    let full = size.map(|(w, h)| geo::outline(&l.uncropped((w as f32, h as f32))).map(to_stage));
                    let shape: Vec<(f32, f32)> = geo::shape_outline(&l).into_iter().map(to_stage).collect();
                    let (corners, edges) = geo::crop_handles(&l);
                    let handles: Vec<((f32, f32), Handle)> = corners
                        .iter()
                        .map(|(c, p)| (to_stage(*p), Handle::Corner(*c)))
                        .chain(edges.iter().map(|(e, p)| (to_stage(*p), Handle::Edge(*e))))
                        .collect();
                    (full, shape, handles)
                });
                let crop_paths = crop_view.as_ref().map(|(full, shape, _)| (*full, shape.clone()));
                let round_handles =
                    l.shape.is_some_and(|sh| sh.kind != lightair_core::ShapeKind::Rounded || sh.radius > 0.3);
                // The rotate gizmo (rings per axis) replaces the other handles while it's on.
                let gizmo_on = self.rotate_tool && !locked && editing && !cropping;
                let knob = (!locked && editing && !cropping && !gizmo_on).then(|| {
                    (to_stage(geo::rotate_handle(&l, 0.0)), to_stage(geo::rotate_handle(&l, ROTATE_OFFSET / s)))
                });
                // The 3D knob below, mirroring the rotate knob above (a click toggles the gizmo).
                let tilt_knob = (!locked && editing && !cropping)
                    .then(|| (to_stage(geo::tilt_handle(&l, 0.0)), to_stage(geo::tilt_handle(&l, ROTATE_OFFSET / s))));
                let active_axis = self
                    .drag
                    .as_ref()
                    .filter(|d| d.object == lid)
                    .and_then(|d| match d.mode {
                        DragMode::TiltAxis(a) => Some(a),
                        DragMode::Rotate if gizmo_on => Some(geo::Axis::Z),
                        _ => None,
                    })
                    .or(self.gizmo_hover);
                let rings: Vec<Ring> = if gizmo_on {
                    geo::gizmo_rings(&l, gizmo_radius(&l, s))
                        .into_iter()
                        .map(|(axis, pts)| {
                            let hot = active_axis == Some(axis);
                            let colour = if hot {
                                tok::gizmo_active()
                            } else {
                                match axis {
                                    geo::Axis::X => tok::gizmo_x(),
                                    geo::Axis::Y => tok::gizmo_y(),
                                    geo::Axis::Z => tok::gizmo_z(),
                                }
                            };
                            (colour, if hot { 3.5 } else { 2.5 }, pts.into_iter().map(to_stage).collect())
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                // Crop bars at the middle of each side (on the shape), along the side.
                let crop_bars: Vec<CropBar> = if knob.is_some() {
                    let (_, edges) = geo::crop_handles(&l);
                    let (sin, cos) = l.rotation.to_radians().sin_cos();
                    edges
                        .iter()
                        .map(|(e, p)| {
                            let (cx_, cy_) = to_stage(*p);
                            // Along the side: horizontal for top/bottom, vertical for left/right.
                            let (ux, uy) = match e {
                                geo::Edge::Top | geo::Edge::Bottom => (cos, sin),
                                geo::Edge::Left | geo::Edge::Right => (-sin, cos),
                            };
                            let lit = hot == Some(Handle::Edge(*e));
                            let half = if lit { 12.0 } else { 9.0 };
                            ((cx_ - ux * half, cy_ - uy * half), (cx_ + ux * half, cy_ + uy * half), lit)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                // How the picture looks tilted in 3D, when it is.
                let tilted =
                    (l.tilt_x.abs() > 0.01 || l.tilt_y.abs() > 0.01).then(|| geo::projected_outline(&l).map(to_stage));
                let edge = tok::canvas_selection_edge();
                // The outline turns with the object, so it's painted as a path.
                stage = stage.child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let o = bounds.origin;
                            let at = |(x, y): (f32, f32)| point(o.x + px(x), o.y + px(y));
                            // While cropping, the shape's outline replaces the box.
                            for (width, c) in [(3.0, edge), (1.0, colour)].into_iter().filter(|_| crop_paths.is_none())
                            {
                                let mut path = PathBuilder::stroke(px(width));
                                path.move_to(at(outline[0]));
                                for p in &outline[1..] {
                                    path.line_to(at(*p));
                                }
                                path.close();
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, c);
                                }
                            }
                            for (colour, width, pts) in &rings {
                                let mut path = PathBuilder::stroke(px(*width));
                                path.move_to(at(pts[0]));
                                for p in &pts[1..] {
                                    path.line_to(at(*p));
                                }
                                path.close();
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, *colour);
                                }
                            }
                            for (from, to) in knob.into_iter().chain(tilt_knob) {
                                let mut path = PathBuilder::stroke(px(1.));
                                path.move_to(at(from));
                                path.line_to(at(to));
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, colour);
                                }
                            }
                            if let Some(t) = &tilted {
                                let mut path = PathBuilder::stroke(px(1.5));
                                path.move_to(at(t[0]));
                                for p in &t[1..] {
                                    path.line_to(at(*p));
                                }
                                path.close();
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, tok::accent_primary());
                                }
                            }
                            for (width, c) in [(6.0, edge), (4.0, colour)] {
                                for (a, b, lit) in &crop_bars {
                                    let (width, c) = match (*lit, width > 5.0) {
                                        (true, true) => (8.0, edge),
                                        (true, false) => (6.0, tok::accent_primary()),
                                        _ => (width, c),
                                    };
                                    let mut path = PathBuilder::stroke(px(width));
                                    path.move_to(at(*a));
                                    path.line_to(at(*b));
                                    if let Ok(path) = path.build() {
                                        window.paint_path(path, c);
                                    }
                                }
                            }
                            if let Some((full, shape)) = &crop_paths {
                                // A hairline round the whole picture (its cut-away part is a ghost).
                                if let Some(full) = full {
                                    let mut line = PathBuilder::stroke(px(1.));
                                    line.move_to(at(full[0]));
                                    for p in &full[1..] {
                                        line.line_to(at(*p));
                                    }
                                    line.close();
                                    if let Ok(path) = line.build() {
                                        window.paint_path(path, tok::canvas_safe_area());
                                    }
                                }
                                for (width, c) in [(3.0, edge), (1.5, colour)] {
                                    let mut path = PathBuilder::stroke(px(width));
                                    path.move_to(at(shape[0]));
                                    for p in &shape[1..] {
                                        path.line_to(at(*p));
                                    }
                                    path.close();
                                    if let Ok(path) = path.build() {
                                        window.paint_path(path, c);
                                    }
                                }
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                );
                if let Some((_, _, handles)) = &crop_view {
                    for (p, h) in handles {
                        stage = stage.child(dot(*p, 10., round_handles, Some(*h)));
                    }
                }
                if gizmo_on {
                    // The angles under the gizmo, in the rings' colours.
                    // Above the frame, clear of the 3D knob below it.
                    let (gx, _) = to_stage(geo::centre(&l));
                    let top = outline.iter().map(|p| p.1).fold(f32::MAX, f32::min);
                    stage = stage.child(
                        div()
                            .absolute()
                            .left(px(gx - 75.))
                            .top(px(top - 30.))
                            .w(px(150.))
                            .flex()
                            .justify_center()
                            .gap_2()
                            .px_2()
                            .py_0p5()
                            .rounded(px(5.))
                            .bg(tok::scrim())
                            .text_size(px(11.))
                            .font_family(crate::ui::MONO)
                            .child(div().text_color(tok::gizmo_x()).child(format!("X {:.0}°", l.tilt_x)))
                            .child(div().text_color(tok::gizmo_y()).child(format!("Y {:.0}°", l.tilt_y)))
                            .child(div().text_color(tok::gizmo_z()).child(format!("Z {:.0}°", l.rotation))),
                    );
                }
                if let Some((_, (tx, ty))) = tilt_knob {
                    // A ring with a dot: "turn in space".
                    let k = if hot == Some(Handle::Tilt) { 18. } else { 14. };
                    stage = stage.child(
                        div()
                            .absolute()
                            .left(px(tx - k / 2.))
                            .top(px(ty - k / 2.))
                            .size(px(k))
                            .rounded_full()
                            .bg(if gizmo_on { tok::accent_primary() } else { tok::canvas_selection_edge() })
                            .border_2()
                            .border_color(tok::accent_primary())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(px(4.)).rounded_full().bg(tok::accent_primary())),
                    );
                }
                if let Some((_, k)) = knob {
                    stage = stage.child(dot(k, 10., true, Some(Handle::Rotate)));
                    // geo::outline goes round: top left, top right, bottom right, bottom left.
                    let corners = [Corner::TopLeft, Corner::TopRight, Corner::BottomRight, Corner::BottomLeft];
                    for (p, c) in outline.into_iter().zip(corners) {
                        stage = stage.child(dot(p, 8., false, Some(Handle::Corner(c))));
                    }
                }
                // What the drag is doing, by the pointer.
                if let (Some(d), Some((pf, p))) =
                    (self.drag.as_ref().filter(|d| d.object == lid && d.moved), self.pointer)
                    && pf == fid
                {
                    let text = match d.mode {
                        DragMode::Move => format!("{:.0}, {:.0}", l.x, l.y),
                        DragMode::Resize(_) => format!("{:.0} × {:.0}", l.width, l.height),
                        DragMode::Rotate => format!("{:.0}°", l.rotation),
                        DragMode::Crop(_, (w, h)) | DragMode::CropEdge(_, (w, h)) | DragMode::CropPan((w, h)) => {
                            let c = l.crop;
                            format!(
                                "{:.0} × {:.0} px",
                                (w - (c.left + c.right) as f32).max(0.0),
                                (h - (c.top + c.bottom) as f32).max(0.0)
                            )
                        }
                        DragMode::Tilt => format!("X {:.0}°  Y {:.0}°", l.tilt_x, l.tilt_y),
                        DragMode::TiltAxis(geo::Axis::X) => format!("X {:.0}°", l.tilt_x),
                        DragMode::TiltAxis(_) => format!("Y {:.0}°", l.tilt_y),
                    };
                    let (px_, py_) = to_stage(p);
                    stage = stage.child(
                        div()
                            .absolute()
                            .left(px(px_ + 14.))
                            .top(px(py_ + 18.))
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(5.))
                            .bg(tok::accent_primary())
                            .text_color(tok::canvas_selection())
                            .text_size(px(11.))
                            .font_family(crate::ui::MONO)
                            .whitespace_nowrap()
                            .child(text),
                    );
                }
            }
            // A selected empty: its pivot, as a small crosshair.
            if let Some(pivot) = studio.selected_scene.and_then(|id| workspace.scene(id)).and_then(|scene| {
                let l = studio.selected_object.and_then(|id| scene.object(id)).filter(|l| l.is_empty())?;
                scene.world(l.id, fid)
            }) {
                let (px_, py_) = (lx + pivot.x * s, ly + pivot.y * s);
                for (w, h) in [(18.0, 2.0), (2.0, 18.0)] {
                    stage = stage.child(
                        div()
                            .absolute()
                            .left(px(px_ - w / 2.))
                            .top(px(py_ - h / 2.))
                            .w(px(w))
                            .h(px(h))
                            .bg(tok::accent_primary()),
                    );
                }
                stage = stage.child(
                    div()
                        .absolute()
                        .left(px(px_ - 5.))
                        .top(px(py_ - 5.))
                        .size(px(10.))
                        .rounded_full()
                        .border_2()
                        .border_color(tok::accent_primary()),
                );
            }
            if editing {
                for (vertical, at) in &self.guides {
                    let g = div().absolute().bg(tok::canvas_guide());
                    stage = stage.child(if *vertical {
                        g.left(px(lx + at * s)).top(px(ly)).w(px(1.)).h(px(shown.height as f32 * s))
                    } else {
                        g.top(px(ly + at * s)).left(px(lx)).h(px(1.)).w(px(shown.width as f32 * s))
                    });
                }
            }
        }
        div().relative().flex_1().h_full().min_w_0().child(stage)
    }

    fn canvas_panel(&self, workspace: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        let live = runtime().any_stream_running();
        let native = self.native.borrow().is_some();
        let stages: Vec<AnyElement> =
            self.visible_canvases(workspace).iter().map(|f| self.stage(workspace, f, cx).into_any_element()).collect();
        let well =
            div().flex().flex_1().min_h_0().child(
                div()
                    .id("canvas")
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(!native, |d| d.bg(tok::bg_canvas_well()))
                    .when(live, |d| d.border_2().border_color(tok::live()))
                    .cursor(self.canvas_cursor(workspace, cx))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::canvas_right_down))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|this, ev: &MouseDownEvent, _, _| this.panning = Some((ev.position, this.pan))),
                    )
                    .on_mouse_up(MouseButton::Middle, cx.listener(Self::canvas_up))
                    .on_scroll(cx.listener(Self::canvas_scroll))
                    .on_pinch(cx.listener(Self::canvas_pinch))
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::canvas_down))
                    .on_mouse_move(cx.listener(Self::canvas_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::canvas_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::canvas_up))
                    .children(stages)
                    .children(self.canvas_text_overlay(cx))
                    // last: the stages have said where their pictures are this frame
                    .when(native, |d| {
                        let (n, rects) = (self.native.clone(), self.native_rects.clone());
                        let shown: Vec<Id> = self.visible_canvases(workspace).iter().map(|c| c.id).collect();
                        d.child(
                            canvas(
                                move |b, _, _| {
                                    let rects = rects.borrow();
                                    let pictures: Vec<(Id, Bounds<Pixels>)> =
                                        shown.iter().filter_map(|id| rects.get(id).map(|r| (*id, *r))).collect();
                                    if let Some(n) = n.borrow_mut().as_mut() {
                                        n.place(Some(b), &pictures);
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                    })
                    .child(self.canvas_controls(workspace, cx))
                    .drag_over::<assets::DraggedAsset>(|s, _, _, _| s.border_2().border_color(tok::accent_primary()))
                    .drag_over::<ExternalPaths>(|s, _, _, _| s.border_2().border_color(tok::accent_primary()))
                    .on_drop(cx.listener(|this, a: &assets::DraggedAsset, window, cx| {
                        this.drop_asset_on_canvas(&a.0, window, cx)
                    }))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                        this.drop_files_on_canvas(paths.paths(), window, cx)
                    }))
                    .context_menu({
                        let shell = cx.entity().downgrade();
                        move |m, window, cx| menus::object_menu(m, &shell, window, cx)
                    }),
            );
        // (the panel's own background where the preview doesn't show through)
        let well = match self.cue_scenes.then(|| self.program_column(workspace, cx)) {
            Some(column) => well.child(div().flex().h_full().bg(tok::bg_panel()).child(column)),
            None => well,
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(div().bg(tok::bg_panel()).child(self.canvas_tabs(workspace, cx)))
            .child(well)
    }

    /// Per-canvas visibility, and copying this object's layout from another canvas.
    fn canvases_section(&self, workspace: &Workspace, object: &Object, scene: Id, cx: &mut Context<Self>) -> AnyElement {
        let active = self.canvas_id(workspace);
        let mut col = div().flex().flex_col().gap_1p5().pt_3().mt_1().border_t_1().border_color(tok::border()).child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tok::text_secondary())
                .child("CANVASES"),
        );
        let Some(here) = workspace.canvas(active).cloned() else { return col.into_any_element() };
        for (i, f) in workspace.canvases.iter().enumerate() {
            let (fid, lid) = (f.id, object.id);
            let visible = object.layouts.get(&fid).is_some_and(|l| l.visible);
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    ui::icon_button(
                        ("canvas-eye", i),
                        if visible { "eye" } else { "eye-off" },
                        if visible { tok::text_secondary() } else { tok::text_tertiary() },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.exec(Command::SetObjectVisible { scene, object: lid, canvas: fid, visible: !visible }, cx)
                    })),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(if visible { tok::text() } else { tok::text_tertiary() })
                        .child(format!("{} {}×{}", f.name, f.width, f.height)),
                );
            // Copy this canvas's layout into the canvas being edited (reframed to its shape).
            if fid != active
                && let (Some(from), Some(current)) = (object.layouts.get(&fid), object.layouts.get(&active))
            {
                let layout = Layout { visible: current.visible, ..crate::reframe::reframe(from, f, &here) };
                let cmd = Command::SetLayout { scene, object: lid, canvas: active, layout };
                row = row.child(
                    Button::new(("copy-layout", i))
                        .label("Use this layout")
                        .xsmall()
                        .ghost()
                        .tooltip(format!("Copy the {} layout into {}", f.name, here.name))
                        .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx))),
                );
            }
            col = col.child(row);
        }
        col.into_any_element()
    }

    /// Studio mode's Program monitor and Take button.
    fn program_column(&self, workspace: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        let studio = self.studio.read(cx);
        let frame = self.program_monitor.as_ref().and_then(|p| p.latest_frame());
        let program_name = workspace.program.and_then(|id| workspace.scene(id)).map(|s| s.name.clone());
        let preview_name = studio.selected_scene.and_then(|id| workspace.scene(id)).map(|s| s.name.clone());
        let can_take = studio.selected_scene.is_some() && studio.selected_scene != workspace.program;
        let f = workspace.primary_canvas();
        let label = |text: &str, name: Option<String>, color: Hsla| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(11.))
                .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(color).child(text.to_string()))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(tok::text_secondary()).child(name.unwrap_or_else(|| "—".into())))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w(px(280.))
            .p_3()
            .border_l_1()
            .border_color(tok::border())
            .bg(tok::bg_panel())
            .child(label("CUED", preview_name, tok::text()))
            .child(label("ON AIR", program_name, tok::live()))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(256. * f.height as f32 / f.width.max(1) as f32))
                    .rounded(px(4.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(tok::live())
                    .bg(tok::bg_canvas_well())
                    .when_some(frame, |d, frame| {
                        d.child(crate::frame_view::frame(frame).absolute().size_full().object_fit(ObjectFit::Contain))
                    }),
            )
            .child(
                Button::new("take")
                    .label("Take")
                    .primary()
                    .w_full()
                    .disabled(!can_take)
                    .tooltip("Put the scene you're editing on air")
                    .on_click(cx.listener(|this, _, _, cx| this.take(cx))),
            )
    }

    /// Several objects selected: align them, or delete them.
    fn multi_inspector(
        &self,
        scene: &Scene,
        canvas: &Canvas,
        selection: &[Id],
        body: Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (sid, fid) = (scene.id, canvas.id);
        let objects: Vec<(Id, Layout)> = selection
            .iter()
            .filter_map(|id| scene.object(*id).filter(|l| !scene.locked(l.id)))
            .filter_map(|l| self.frame_of(scene, l, fid).map(|lo| (l.id, lo)))
            .collect();
        let align_button = |i: usize, icon: &'static str, tip: &'static str, how: geo::Align| {
            let objects = objects.clone();
            ui::icon_button(("align", i), icon, tok::text_secondary())
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let layouts: Vec<Layout> = objects.iter().map(|(_, l)| *l).collect();
                    let aligned = geo::align(&layouts, how);
                    let moved = objects.iter().map(|(id, _)| *id).zip(aligned).collect();
                    this.set_layouts(sid, fid, moved, "Align objects", cx);
                }))
        };
        let row = div()
            .flex()
            .gap_1()
            .child(align_button(0, "align-start-vertical", "Align left edges", geo::Align::Left))
            .child(align_button(1, "align-center-vertical", "Align centres", geo::Align::Centre))
            .child(align_button(2, "align-end-vertical", "Align right edges", geo::Align::Right))
            .child(div().w(px(8.)))
            .child(align_button(3, "align-start-horizontal", "Align top edges", geo::Align::Top))
            .child(align_button(4, "align-center-horizontal", "Align middles", geo::Align::Middle))
            .child(align_button(5, "align-end-horizontal", "Align bottom edges", geo::Align::Bottom));
        let distribute_button = |i: usize, icon: &'static str, tip: &'static str, horizontal: bool| {
            let objects = objects.clone();
            ui::icon_button(("distribute", i), icon, tok::text_secondary())
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let layouts: Vec<Layout> = objects.iter().map(|(_, l)| *l).collect();
                    let spaced = geo::distribute(&layouts, horizontal);
                    let moved = objects.iter().map(|(id, _)| *id).zip(spaced).collect();
                    this.set_layouts(sid, fid, moved, "Distribute objects", cx);
                }))
        };
        let row = row.child(div().w(px(8.))).when(objects.len() >= 3, |r| {
            r.child(distribute_button(0, "align-horizontal-distribute-center", "Space evenly across", true))
                .child(distribute_button(1, "align-vertical-distribute-center", "Space evenly down", false))
        });
        body.child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} objects selected", selection.len())))
            .child(div().text_color(tok::text_tertiary()).child("Drag any of them to move them together."))
            .child(row)
            .child(
                Button::new("delete-objects")
                    .label("Delete")
                    .small()
                    .danger()
                    .on_click(cx.listener(|this, _, _, cx| this.delete_selection(cx))),
            )
            .into_any_element()
    }

    fn inspector(&self, workspace: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        let studio = self.studio.read(cx);
        let canvas = self.current_canvas(workspace).clone();
        let scene = studio.selected_scene.and_then(|id| workspace.scene(id)).cloned();
        let object = scene.as_ref().and_then(|s| studio.selected_object.and_then(|l| s.object(l))).cloned();
        let selection = studio.selection();
        let track = studio.selected_track.and_then(|t| workspace.track(t)).cloned();
        let strip = studio.selected_track.and_then(|t| lightair_core::MixerStrip::find(workspace, t));
        let mut body = div().flex().flex_col().gap_3().px_3();
        if let (Some(sc), Some(l)) = (&scene, &object)
            && selection.len() == 1
            && let Some(bar) = self.prefab_bar(sc.id, l, cx)
        {
            body = body.child(div().pt_2().child(bar));
        }
        let content = match (scene, object) {
            (_, None) if track.is_some() => self.track_inspector(workspace, track.as_deref().unwrap(), body, cx),
            (_, None) if strip.is_some() => self.strip_inspector(workspace, strip.unwrap(), body, cx),
            (Some(scene), Some(_)) if selection.len() > 1 => {
                self.multi_inspector(&scene, &canvas, &selection, body, cx)
            }
            (Some(scene), Some(object)) if object.is_empty() => self.empty_inspector(&scene, &object, &canvas, body, cx),
            (Some(scene), Some(object)) => self.object_inspector(workspace, &scene, &object, &canvas, body, cx),
            (Some(scene), None) => {
                let (sid, on_air) = (scene.id, workspace.program == Some(scene.id));
                let header = div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_2()
                    .child(ui::icon("layout-dashboard").size(px(16.)).text_color(tok::text_secondary()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(scene.name.clone()),
                    )
                    .when(on_air, |d| {
                        d.child(
                            div()
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(tok::live())
                                .child("ON AIR"),
                        )
                    })
                    .child(
                        ui::icon_button("delete-scene", "trash", tok::text_secondary())
                            .tooltip(|w, cx| Tooltip::new("Delete scene").build(w, cx))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.exec(Command::RemoveScene { scene: sid }, cx)),
                            ),
                    );
                let info = div().text_size(px(12.)).text_color(tok::text_secondary()).child(format!(
                    "{} object{} · {}×{} @ {} fps",
                    scene.objects.len(),
                    if scene.objects.len() == 1 { "" } else { "s" },
                    canvas.width,
                    canvas.height,
                    canvas.fps_num / canvas.fps_den.max(1)
                ));
                // How switching to this scene looks on air: the show's default, or its own.
                let transition = self
                    .section(
                        "Transition",
                        Some(
                            ui::info(
                                "transition-info",
                                "How switching to this scene looks on air. All scenes: the default every switch uses. This scene: its own, used whenever you switch to it.",
                            )
                            .into_any_element(),
                        ),
                        cx,
                    )
                    .child(self.transition_picker(workspace, &scene, cx));
                let screen = self.screen_zoom_pointer_section(workspace, &scene, cx);
                body.child(header).child(info).child(screen).child(transition).into_any_element()
            }
            _ => body.child(div().text_color(tok::text_tertiary()).child("Nothing selected")).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .pt_2()
            .child(div().id("inspector-scroll").flex_1().min_h_0().w_full().pb_3().overflow_y_scroll().child(content))
    }

    /// The FPS readout in the top bar; a click shows the frame counts.
    /// The frame rate and frame time, or (`compact`) just their dot; either opens the details.
    fn performance(&self, dot: Hsla, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let studio = self.studio.read(cx);
        let stats = studio.stats;
        let recent = studio.recent_lagged;
        let preview = self.previews.values().map(|p| p.frames()).max().unwrap_or(0);
        let open = self.perf_open;
        // Fixed-width digits so the numbers don't jiggle as they change.
        let readout = div()
            .id("performance")
            .flex()
            .items_center()
            .gap_2()
            .h(px(24.))
            .px_2()
            .rounded(px(5.))
            .cursor_pointer()
            .text_size(px(11.))
            .font_family(crate::ui::MONO)
            .text_color(tok::text_secondary())
            .when(open, |d| d.bg(tok::bg_raised()))
            .hover(|d| d.bg(tok::bg_raised()))
            .child(div().size(px(8.)).rounded_full().bg(dot))
            .when(!compact, |d| d.child(format!("{:>4.1} FPS", stats.active_fps)).child(format!("{:>4.1} ms", stats.average_frame_time_ms)))
            .when(compact, |d| d.tooltip(ui::tip(format!("{:.1} FPS · {:.1} ms a frame", stats.active_fps, stats.average_frame_time_ms))))
            .on_click(cx.listener(|this, _, _, cx| {
                this.perf_open = !this.perf_open;
                cx.notify();
            }));
        let row = |label: &'static str, value: String| {
            div()
                .flex()
                .justify_between()
                .gap_6()
                .child(div().text_color(tok::text_secondary()).child(label))
                .child(div().font_family(crate::ui::MONO).text_color(tok::text()).child(value))
        };
        let share = if stats.total_frames > 0 {
            format!(" ({:.2}%)", stats.lagged_frames as f64 * 100.0 / stats.total_frames as f64)
        } else {
            String::new()
        };
        let panel = open.then(|| {
            deferred(
                div()
                    .id("performance-panel")
                    .absolute()
                    .bottom(px(30.))
                    .right_0()
                    .w(px(260.))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .rounded(px(8.))
                    .bg(tok::bg_header())
                    .border_1()
                    .border_color(tok::border_header())
                    .shadow_lg()
                    .text_size(px(12.))
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.perf_open = false;
                        cx.notify();
                    }))
                    .child(div().font_weight(FontWeight::SEMIBOLD).pb_1().child("Performance"))
                    .child(row("Frame rate", format!("{:.1} fps", stats.active_fps)))
                    .child(row("Render time", format!("{:.1} ms", stats.average_frame_time_ms)))
                    .child(row("Frames rendered", stats.total_frames.to_string()))
                    .child(row("Frames late", format!("{}{share}", stats.lagged_frames)))
                    .child(row("Late in the last second", recent.to_string()))
                    .child(row("Preview frames", preview.to_string()))
                    .child(div().pt_1().text_size(px(11.)).text_color(tok::text_tertiary()).child(
                        "Late frames are ones the renderer couldn't finish in time; a few at start-up are normal.",
                    )),
            )
            .with_priority(1)
        });
        div().relative().child(readout).children(panel)
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let studio = self.studio.read(cx);
        let issue = studio.issue.clone();
        let fps_color = if studio.stats.active_fps > 0.0 && studio.recent_lagged == 0 { tok::ok() } else { tok::warn() };
        div()
            .flex()
            .items_center()
            .gap_3()
            .h(px(26.))
            .px_3()
            .border_t_1()
            .border_color(tok::border())
            .text_size(px(11.))
            .text_color(tok::text_secondary())
            .children(self.console_chip(cx))
            .children(issue.map(|i| div().child(i)))
            .child(div().flex_1())
            // the frame rate and frame time (their details open upward)
            .child(self.performance(fps_color, false, cx))
            .child(div().text_color(tok::text_tertiary()).child(concat!("Lightair ", env!("CARGO_PKG_VERSION"))))
    }
}

/// A transition, drawn small: scene A (slate, a light card on the left) becoming scene B
/// (warm, a dark card on the right). `t`: how far through (0 A .. 1 B) while it plays;
/// `None`: a still that shows what the transition does.
fn transition_preview(kind: TransitionKind, direction: Direction, t: Option<f32>) -> Div {
    let still = t.is_none();
    let t = t.unwrap_or(match kind {
        TransitionKind::Slide | TransitionKind::Dip => 0.38,
        TransitionKind::Stinger => 0.45,
        _ => 0.5,
    });
    let ease = |x: f32| x * x * (3.0 - 2.0 * x);
    let e = ease(t);
    let a_bg: Hsla = rgb(0x3A4150).into();
    let b_bg: Hsla = rgb(0xE07B39).into();
    let card = |x: f32, y: f32, w: f32, h: f32, colour: Hsla| {
        div().absolute().left(relative(x)).top(relative(y)).w(relative(w)).h(relative(h)).rounded(px(2.)).bg(colour)
    };
    let a_card: Hsla = rgb(0xE9EAEE).into();
    let b_card: Hsla = rgb(0x24201E).into();
    // a whole scene, placed at (x, y) of the frame
    let scene = |b: bool, x: f32, y: f32| {
        div()
            .absolute()
            .left(relative(x))
            .top(relative(y))
            .size_full()
            .bg(if b { b_bg } else { a_bg })
            .child(if b { card(0.52, 0.24, 0.32, 0.52, b_card) } else { card(0.14, 0.3, 0.36, 0.4, a_card) })
    };
    let frame = div().relative().w_full().h(px(34.)).rounded(px(4.)).overflow_hidden();
    match kind {
        // a still cut: the two scenes as two frames, a gap between (no blend, a jump)
        TransitionKind::Cut if still => frame
            .child(scene(false, 0.0, 0.0))
            .child(scene(true, 0.54, 0.0))
            .child(div().absolute().left(relative(0.46)).top_0().w(relative(0.08)).h_full().bg(tok::bg_app())),
        TransitionKind::Cut => frame.child(scene(t >= 0.5, 0.0, 0.0)),
        TransitionKind::Fade => frame.child(scene(false, 0.0, 0.0)).child(scene(true, 0.0, 0.0).opacity(e)),
        // the backgrounds crossfade while one card glides to where it is in B
        TransitionKind::Move => {
            let lerp = |a: f32, b: f32| a + (b - a) * e;
            frame
                .child(div().absolute().size_full().bg(a_bg))
                .child(div().absolute().size_full().bg(b_bg).opacity(e))
                .child(card(lerp(0.14, 0.52), lerp(0.3, 0.24), lerp(0.36, 0.32), lerp(0.4, 0.52), a_card))
        }
        TransitionKind::Slide => {
            let (dx, dy) = match direction {
                Direction::Left => (-1.0, 0.0),
                Direction::Right => (1.0, 0.0),
                Direction::Up => (0.0, -1.0),
                Direction::Down => (0.0, 1.0),
            };
            frame.child(scene(false, dx * e, dy * e)).child(scene(true, -dx * (1.0 - e), -dy * (1.0 - e)))
        }
        // B revealed from the right edge, with a bright edge line
        TransitionKind::Wipe => {
            let w = e.max(0.001);
            frame
                .child(scene(false, 0.0, 0.0))
                .child(
                    div()
                        .absolute()
                        .left(relative(1.0 - w))
                        .top_0()
                        .w(relative(w))
                        .h_full()
                        .overflow_hidden()
                        .child(div().absolute().left(relative(-(1.0 - w) / w)).top_0().w(relative(1.0 / w)).h_full().child(scene(true, 0.0, 0.0))),
                )
                .child(div().absolute().left(relative(1.0 - w)).top_0().w(px(1.5)).h_full().bg(rgb(0xFFFFFF)))
        }
        // through black: A darkens, then B comes up
        TransitionKind::Dip => frame
            .child(scene(t >= 0.5, 0.0, 0.0))
            .child(div().absolute().size_full().bg(rgb(0x000000)).opacity(1.0 - (2.0 * t - 1.0).abs())),
        // a band sweeps across, the scenes swapping behind it
        TransitionKind::Stinger => frame
            .child(scene(t >= 0.5, 0.0, 0.0))
            .child(div().absolute().left(relative(-0.55 + 1.6 * t)).top_0().w(relative(0.5)).h_full().bg(rgb(0x7C5CFF))),
    }
}

#[cfg(test)]
mod keys {
    /// Every shortcut needs a modifier or a key that types nothing: a plain letter, digit or
    /// sign would be taken from text fields.
    #[test]
    fn no_shortcut_is_a_plain_typing_key() {
        for file in [include_str!("shell.rs"), include_str!("editor.rs")] {
            for (i, _) in file.match_indices("KeyBinding::new(\"") {
                let rest = &file[i + 17..];
                let key = &rest[..rest.find('"').unwrap()];
                let plain = key.chars().count() == 1 || (key.starts_with("shift-") && key.chars().count() == 7);
                assert!(!plain, "a shortcut on a typing key: {key:?}");
            }
        }
    }
}
