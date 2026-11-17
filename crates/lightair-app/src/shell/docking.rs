//! The dock layout (docs/05-interface-shell.md, "Window anatomy"): every region is a gpui-kit
//! dock panel, so it can be resized, collapsed, tabbed with others or moved. `StudioWindow`
//! keeps the state and handlers; each region's panel renders through it.

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{BasePanel, DockLayout, DockPlacement, DockSkin, Panel, PanelEvent, panel_handle};
use gpui_kit::*;

use super::StudioWindow;
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Region {
    Hierarchy,
    Scenes,
    Canvas,
    Inspector,
    Assets,
}

pub(super) struct ShellPanel {
    shell: WeakEntity<StudioWindow>,
    region: Region,
    focus: FocusHandle,
}

impl ShellPanel {
    fn new(shell: Entity<StudioWindow>, region: Region, cx: &mut Context<Self>) -> Self {
        cx.observe(&shell, |_, _, cx| cx.notify()).detach();
        // a live picture is redrawn when it has a new frame (the canvas's rate), not at every
        // display refresh, and only its own panel
        if matches!(region, Region::Canvas | Region::Inspector) {
            cx.spawn(async move |this, cx| {
                let mut seen = 0;
                loop {
                    cx.background_executor().timer(std::time::Duration::from_millis(8)).await;
                    let frames = this.update(cx, |p, cx| p.shell.upgrade().map(|s| s.read(cx).live_frames(p.region)));
                    match frames {
                        Ok(Some(n)) if n != seen => {
                            seen = n;
                            let _ = this.update(cx, |p, cx| {
                                cx.notify();
                                // the meters move in the same redraw
                                if p.region == Region::Canvas
                                    && let Some(shell) = p.shell.upgrade()
                                {
                                    let mixer = shell.read(cx).mixer_dock.clone();
                                    crate::mixer::MixerDock::tick_meters(&mixer, cx);
                                }
                            });
                        }
                        Ok(Some(_)) => {}
                        _ => break,
                    }
                }
            })
            .detach();
        }
        Self { shell: shell.downgrade(), region, focus: cx.focus_handle() }
    }
}

impl BasePanel for ShellPanel {
    fn panel_name(&self) -> &'static str {
        match self.region {
            Region::Hierarchy => "layers",
            Region::Scenes => "scenes",
            Region::Canvas => "canvas",
            Region::Inspector => "inspector",
            Region::Assets => "assets",
        }
    }
}

impl Panel for ShellPanel {
    /// None: tabs show `title` (icon and name).
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        None
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match self.region {
            Region::Hierarchy => ui::panel_title("list-tree", "Hierarchy"),
            Region::Scenes => ui::panel_title("layout-dashboard", "Scenes"),
            Region::Canvas => ui::panel_title("monitor", "Canvas"),
            Region::Inspector => ui::panel_title("sliders-horizontal", "Inspector"),
            Region::Assets => ui::panel_title("folder", "Assets"),
        }
    }

    fn title_style(&self, _: &App) -> Option<gpui_kit::component::dock::TitleStyle> {
        Some(ui::panel_header_style())
    }

    fn toolbar_buttons(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<Vec<Button>> {
        let shell = self.shell.clone();
        let button = move |id: &'static str, icon: &'static str, tip: &'static str| {
            Button::new(id).icon(ui::icon(icon)).ghost().xsmall().tooltip(tip)
        };
        match self.region {
            Region::Hierarchy => {
                let empty = shell.clone();
                Some(vec![
                    button("create-empty", "box", crate::ui::keys_static("Create empty (⇧⌘N)")).on_click(move |_, window, cx| {
                        if let Some(s) = empty.upgrade() {
                            s.update(cx, |w, cx| w.create_empty(None, window, cx));
                        }
                    }),
                    button("add-object", "plus", "Add object").on_click(move |_, window, cx| {
                        if let Some(s) = shell.upgrade() {
                            s.update(cx, |w, cx| w.toggle_add_menu(window, cx));
                        }
                    }),
                ])
            }
            Region::Scenes => {
                let import = shell.clone();
                Some(vec![
                    button("import-obs", "import", "Import OBS scenes…").on_click(move |_, window, cx| {
                        if let Some(s) = import.upgrade() {
                            s.update(cx, |w, cx| w.import_obs(window, cx));
                        }
                    }),
                    button("add-scene", "plus", "Add scene").on_click(move |_, _, cx| {
                        if let Some(s) = shell.upgrade() {
                            s.update(cx, |w, cx| w.add_scene(cx));
                        }
                    }),
                ])
            }
            Region::Assets => {
                let reveal = shell.clone();
                Some(vec![
                    button("reveal-assets", "folder-open", "Show in Finder").on_click(move |_, _, cx| {
                        if let Some(s) = reveal.upgrade() {
                            s.update(cx, |w, cx| w.reveal_assets(cx));
                        }
                    }),
                    button("import-assets", "plus", "Import files…").on_click(move |_, window, cx| {
                        if let Some(s) = shell.upgrade() {
                            s.update(cx, |w, cx| w.import_assets(window, cx));
                        }
                    }),
                ])
            }
            Region::Canvas | Region::Inspector => None,
        }
    }

    fn zoom_control(&self, _: &App) -> Option<gpui_kit::component::dock::PanelControl> {
        None
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    /// The canvas: its preview is drawn under the window's UI (native_preview), showing
    /// through where the canvas paints nothing.
    fn transparent(&self, _: &App) -> bool {
        self.region == Region::Canvas && crate::native_preview::on()
    }
}

impl EventEmitter<PanelEvent> for ShellPanel {}

impl Focusable for ShellPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ShellPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shell) = self.shell.upgrade() else { return div().into_any_element() };
        let region = self.region;
        shell.update(cx, |w, cx| {
            let workspace = w.studio.read(cx).workspace().clone();
            match region {
                Region::Hierarchy => w.hierarchy_panel(&workspace, cx).into_any_element(),
                Region::Scenes => w.scenes_panel(&workspace, cx).into_any_element(),
                Region::Canvas => w.canvas_panel(&workspace, cx).into_any_element(),
                Region::Inspector => {
                    w.ensure_colour_pickers(window, cx);
                    w.inspector(&workspace, cx).into_any_element()
                }
                Region::Assets => w.assets_panel(&workspace, cx).into_any_element(),
            }
        })
    }
}

impl StudioWindow {
    /// Builds the dock on first render (panels need this window's entity).
    pub(super) fn ensure_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dock.is_some() {
            return;
        }
        let me = cx.entity();
        let panel = |region: Region, cx: &mut Context<Self>| cx.new(|cx| ShellPanel::new(me.clone(), region, cx));
        let (objects, scenes) = (panel(Region::Hierarchy, cx), panel(Region::Scenes, cx));
        let (canvas, inspector) = (panel(Region::Canvas, cx), panel(Region::Inspector, cx));
        let assets = panel(Region::Assets, cx);
        let (mixer, outputs, chat) = (self.mixer_dock.clone(), self.outputs.clone(), self.chat.clone());
        let activity = self.activity.clone();
        let stream = self.stream.clone();
        let automations = self.automations_panel.clone();
        // Every group shows a tab bar, even with one panel, so any panel can be dropped onto
        // any header to join it as a tab (and the canvas gets a header like the rest).
        let (dock, skin) = DockSkin::dock_area("lightair-studio", Some(1), window, cx);
        skin.set_panel_style(gpui_kit::component::dock::PanelStyle::TabBar, cx);
        // No "…" menu in panel headers: each panel's actions are its own buttons.
        skin.set_menu_button_visible(false, cx);
        dock.update(cx, |area, cx| {
            area.set_center(DockLayout::tabs().panel_view(panel_handle(canvas), cx), window, cx);
            // the mixer and assets: a dock of their own, so they hide like the side panels
            area.set_dock(
                DockPlacement::Bottom,
                DockLayout::tabs()
                    .panel_view(panel_handle(mixer), cx)
                    .panel_view(panel_handle(assets), cx)
                    .panel_view(panel_handle(automations), cx)
                    .active_index(match std::env::var("LIGHTAIR_OPEN").as_deref() {
                        Ok(h) if h.starts_with("assets") => 1,
                        Ok(h) if h.starts_with("automations") => 2,
                        _ => 0,
                    }),
                window,
                cx,
            );
            // tall enough for a full channel strip (taller for the audio screenshot hooks)
            let bottom = if std::env::var("LIGHTAIR_OPEN").is_ok_and(|h| h.starts_with("audio")) { 600. } else { 420. };
            area.set_dock_size(DockPlacement::Bottom, px(bottom), window, cx);
            area.set_dock(
                DockPlacement::Left,
                DockLayout::v_split()
                    .child(DockLayout::tabs().panel_view(panel_handle(objects), cx), None)
                    .child(DockLayout::tabs().panel_view(panel_handle(scenes), cx), Some(px(300.))),
                window,
                cx,
            );
            area.set_dock(
                DockPlacement::Right,
                DockLayout::v_split()
                    .child(
                        DockLayout::tabs()
                            .panel_view(panel_handle(outputs), cx)
                            .panel_view(panel_handle(chat), cx)
                            .panel_view(panel_handle(activity), cx)
                            .panel_view(panel_handle(stream), cx)
                            .active_index(match std::env::var("LIGHTAIR_OPEN").as_deref() {
                                Ok("chat") => 1,
                                Ok(h) if h.starts_with("activity") => 2,
                                Ok("stream") => 3,
                                _ => 0,
                            }),
                        Some(px(320.)),
                    )
                    .child(DockLayout::tabs().panel_view(panel_handle(inspector), cx), None),
                window,
                cx,
            );
            area.set_dock_size(DockPlacement::Left, px(260.), window, cx);
            area.set_dock_size(DockPlacement::Right, px(320.), window, cx);
        });
        self.dock = Some(dock);
        // Development: open a dialog on launch for screenshots (no clicking needed).
        match std::env::var("LIGHTAIR_OPEN").as_deref() {
            Ok("add-object") => self.open_add_object(window, cx),
            // a text object being typed into on the canvas
            Ok("canvas-text") => {
                self.add_text(cx);
                let studio = self.studio.read(cx);
                let w = studio.workspace().clone();
                let found = studio.selected_scene.and_then(|s| w.scene(s)).zip(studio.selected_object).and_then(|(scene, o)| scene.object(o).and_then(|o| o.source.map(|s| (o.id, s))));
                let canvas = w.primary_canvas().id;
                if let Some((object, source)) = found {
                    self.start_canvas_text(object, source, canvas, window, cx);
                }
            }
            // a platform's sign-in window (kept open until closed)
            Ok(hook) if hook.starts_with("signin-") => {
                static KEPT: parking_lot::Mutex<Option<Box<dyn lightair_live::pages::Page>>> = parking_lot::Mutex::new(None);
                let closed: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(|| tracing::info!("CHECK sign-in window closed"));
                if let Ok(url) = std::env::var("LIGHTAIR_SIGNIN_URL") {
                    *KEPT.lock() = Some(crate::chat_panel::window_page(&url, closed));
                } else {
                    match lightair_live::accounts::sign_in(&hook[7..], closed) {
                        Ok(page) => *KEPT.lock() = Some(page),
                        Err(e) => tracing::warn!(%e, "CHECK sign-in"),
                    }
                }
            }
            // recipes added (and an alert box, for their voice), then the test events sent
            Ok(hook) if hook.starts_with("automations-") => {
                for key in hook[12..].split(',') {
                    if let Some(a) = lightair_automation::recipes::make(key) {
                        self.automations.set(a);
                    }
                }
                self.add_widget("alert-box", None, window, cx);
                self.send_test_events(window, cx);
            }
            // a widget added and selected; LIGHTAIR_TEST_EVENTS ("sub,paid,…") then sent to it
            Ok(hook) if hook.trim_start_matches("activity-").starts_with("widget-") => {
                for kind in hook.trim_start_matches("activity-")[7..].split(',') {
                    match kind.strip_prefix("import:") {
                        Some(dir) => match lightair_widgets::import_folder(std::path::Path::new(dir)) {
                            Ok(custom) => self.add_widget("custom", Some(custom), window, cx),
                            Err(e) => tracing::warn!(%e, "widget import"),
                        },
                        None => self.add_widget(kind, None, window, cx),
                    }
                }
                self.send_test_events(window, cx);
            }
            Ok(open @ ("add-display" | "add-window" | "add-app")) => {
                self.open_add_object(window, cx);
                self.pick_screen(open.trim_start_matches("add-"), window, cx);
            }
            Ok("settings") => self.toggle_settings(cx),
            Ok("virtual-camera-on") => self.studio.update(cx, |s, cx| s.toggle_virtual_camera(cx)),
            Ok("bottom-closed") => {
                if let Some(dock) = self.dock.clone() {
                    dock.update(cx, |area, cx| area.toggle_dock(DockPlacement::Bottom, window, cx));
                }
            }
            Ok("shortcut-capture") => {
                // Settings › Shortcuts listening for a key, then Esc sent through the window (no
                // shortcut saved): whether it reached Settings, logged
                self.open_settings(super::settings::Page::Shortcuts, cx);
                self.shortcut_capture = Some(crate::shortcuts::GlobalAction::FollowZoom);
                window.focus(&self.settings_focus, cx);
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
                    let before = this.update(cx, |this, _| this.shortcut_capture.is_some()).unwrap_or(false);
                    let _ = this.update_in(cx, |this, window, cx| {
                        tracing::info!(settings_focused = this.settings_focus.is_focused(window), root = this.focus.is_focused(window), within = this.settings_focus.contains_focused(window, cx), "CHECK focus before");
                    });
                    let _ = cx.update(|window, cx| window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx));
                    let _ = this.update_in(cx, |this, window, _| {
                        tracing::info!(listening = before, after_escape = this.shortcut_capture.is_some(), back_to_window = this.focus.is_focused(window), "CHECK settings key");
                    });
                })
                .detach();
            }
            Ok(hook) if hook.starts_with("settings-") => {
                use super::settings::Page;
                let page = match &hook[9..] {
                    "audio" => Page::Audio,
                    "camera" => Page::Camera,
                    "recording" => Page::Recording,
                    "streaming" => Page::Streaming,
                    "clips" => Page::Clips,
                    "canvases" | "canvases-edit" => Page::Canvases,
                    "shortcuts" => Page::Shortcuts,
                    "remote" => Page::Remote,
                    "connections" => Page::Connections,
                    "permissions" => Page::Permissions,
                    "about" => Page::About,
                    _ => Page::General,
                };
                if page == Page::Audio {
                    self.headphones_devices =
                        self.studio.read(cx).engine().call(|_| lightair_engine::monitoring::devices());
                }
                if hook == "settings-canvases-edit" {
                    let id = self.studio.read(cx).workspace().primary_canvas().id;
                    self.edit_canvas(id, window, cx);
                }
                self.open_settings(page, cx);
            }
            Ok("palette") => self.open_palette(window, cx),
            Ok("scene-screen") => {
                // the scene's Inspector with its screen zoom & pointer on (the zoom always in)
                self.show_place(super::Place::Live, cx);
                let scene = self.studio.read(cx).selected_scene;
                if let Some(scene) = scene {
                    let look = lightair_core::ScreenZoomPointer { zoom: lightair_core::ScreenZoomPointer::ZOOM_TOGGLE, ..Default::default() };
                    self.exec(lightair_core::Command::SetScreenZoomPointer { scene, screen: Some(look) }, cx);
                    self.select_object(None, cx);
                }
            }
            Ok("library") => self.show_place(super::Place::Library, cx),
            Ok(hook @ ("new-workspace" | "new-workspace-live")) => {
                // a second workspace, made and opened in place, then the Library (or Live)
                self.new_workspace("Second show", cx);
                if hook == "new-workspace" {
                    self.show_place(super::Place::Library, cx);
                }
            }
            Ok("editor-latest") => {
                // the Editor with the workspace's newest recording added to a workspace
                if let Some(dir) = self.studio.read(cx).workspace_dir() {
                    let index = lightair_store::recordings::Index::load(&dir);
                    if let Some(item) = index.newest_first().first() {
                        let id = item.id;
                        self.show_place(super::Place::Editor, cx);
                        self.editor.update(cx, |e, cx| e.add_item(id, cx));
                    }
                }
            }
            Ok("screen-layouts") => {
                // the layout scenes, the display blank (an app that isn't there), Screen on air
                let p = self.studio.read(cx).workspace().clone();
                if let Some(display) = p.sources.values().find(|i| i.kind == lightair_core::kinds::SCREEN) {
                    let s = serde_json::json!({"mode": lightair_core::kinds::APP, "app": "app.lightair.nothing"});
                    self.exec(lightair_core::Command::SetSourceSettings { source: display.id, settings: s }, cx);
                }
                self.add_screen_layouts(cx);
                let p = self.studio.read(cx).workspace().clone();
                if let Some(scene) = p.scenes.iter().find(|s| s.name == "Screen") {
                    self.exec(lightair_core::Command::SetProgram { scene: Some(scene.id) }, cx);
                    self.studio.update(cx, |s, _| s.select_scene(scene.id));
                }
            }
            Ok("studio-take") => {
                // a 4-second take of the Main scene, its display pointed at an app that isn't
                // there (nobody's screen in the files), into a folder beside the workspace
                let p = self.studio.read(cx).workspace().clone();
                let mut settings = p.outputs.recording.clone();
                if let Some(dir) = self.studio.read(cx).workspace_dir().and_then(|d| d.parent().map(|p| p.join("shot-recordings"))) {
                    settings.folder = Some(dir);
                }
                self.exec(lightair_core::Command::SetRecordingSettings { settings }, cx);
                if let Some(display) = p.sources.values().find(|i| i.kind == lightair_core::kinds::SCREEN) {
                    let s = serde_json::json!({"mode": lightair_core::kinds::APP, "app": "app.lightair.nothing", "cursor": false});
                    self.exec(lightair_core::Command::SetSourceSettings { source: display.id, settings: s }, cx);
                }
                if let Some(main) = p.scenes.iter().find(|s| s.name == "Main") {
                    self.exec(lightair_core::Command::SetProgram { scene: Some(main.id) }, cx);
                }
                self.studio_take(cx);
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_secs(8)).await;
                    let _ = this.update(cx, |this, cx| this.studio_take(cx));
                })
                .detach();
            }
            Ok("cursor") => {
                // the drawn pointer, big, on the background colour (not on a screen capture: no
                // one's screen in the picture)
                let p = self.studio.read(cx).workspace().clone();
                if let Some(bg) = p.sources.values().find(|i| i.name == "Background") {
                    let mut e = lightair_core::catalog::def(lightair_core::catalog::POINTER_KIND).unwrap().create();
                    e.settings["size"] = serde_json::json!(4.0);
                    e.settings["idle_s"] = serde_json::json!(0.0);
                    self.exec(lightair_core::Command::AddSourceEffect { source: bg.id, effect: e, index: None }, cx);
                }
                // the logo sits where the pointer usually is
                if let Some(scene) = p.program.and_then(|s| p.scene(s)) {
                    let f = p.primary_canvas().id;
                    for l in scene.objects.iter().filter(|l| l.name == "Logo") {
                        self.exec(lightair_core::Command::SetObjectVisible { scene: scene.id, object: l.id, canvas: f, visible: false }, cx);
                    }
                }
            }
            Ok("editor") => self.show_place(super::Place::Editor, cx),
            Ok(hook @ ("editor-proxy" | "editor-play")) => {
                self.show_place(super::Place::Editor, cx);
                let play = hook == "editor-play";
                self.editor.update(cx, |e, cx| e.hook_speed(play, cx));
            }
            Ok("editor-ghost") => {
                self.show_place(super::Place::Editor, cx);
                self.editor.update(cx, |e, cx| e.hook_ghost(cx));
            }
            Ok("library-inside") => {
                self.show_place(super::Place::Library, cx);
                self.library.update(cx, |l, cx| l.hook_inside(cx));
            }
            Ok("editor-take-open") => {
                self.show_place(super::Place::Editor, cx);
                self.editor.update(cx, |e, cx| e.hook_open_studio_take(cx));
            }
            Ok("editor-trim") => {
                self.show_place(super::Place::Editor, cx);
                self.editor.update(cx, |e, cx| e.hook_trim(cx));
            }
            Ok("editor-zooms") => {
                // the newest recording in a workspace, then a zoom added on its zoom track
                if let Some(dir) = self.studio.read(cx).workspace_dir() {
                    let index = lightair_store::recordings::Index::load(&dir);
                    if let Some(item) = index.newest_first().first() {
                        let id = item.id;
                        self.show_place(super::Place::Editor, cx);
                        self.editor.update(cx, |e, cx| e.add_item(id, cx));
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(2500)).await;
                            let _ = this.update(cx, |this, cx| this.editor.update(cx, |e, cx| e.hook_zooms(cx)));
                        })
                        .detach();
                    }
                }
            }
            Ok("editor-alone") => {
                self.show_place(super::Place::Editor, cx);
                self.editor.update(cx, |e, cx| e.hook_alone(cx));
            }
            Ok("editor-tools") => {
                self.show_place(super::Place::Editor, cx);
                self.editor.update(cx, |e, cx| e.hook_tools(cx));
            }
            Ok("library-pick") => {
                self.show_place(super::Place::Library, cx);
                self.library.update(cx, |l, cx| l.hook_pick(cx));
            }
            Ok("editor-more") => {
                self.show_place(super::Place::Editor, cx);
                self.editor.update(cx, |e, cx| e.hook_more(cx));
            }
            Ok("editor-sound") => {
                self.show_place(super::Place::Editor, cx);
                self.editor.update(cx, |e, cx| e.hook_sound(cx));
            }
            Ok(hook @ ("editor-grids" | "editor-grids-2")) => {
                self.show_place(super::Place::Editor, cx);
                let second = hook == "editor-grids-2";
                self.editor.update(cx, |e, cx| e.hook_grids(second, cx));
            }
            Ok(hook @ ("editor-looks" | "editor-looks-2")) => {
                self.show_place(super::Place::Editor, cx);
                let second = hook == "editor-looks-2";
                self.editor.update(cx, |e, cx| e.hook_looks(second, cx));
            }
            Ok("library-record") => {
                // a short recording (into a folder beside the workspace, not the user's), a
                // marker, then the Library once it's in
                let mut settings = self.studio.read(cx).workspace().outputs.recording.clone();
                if let Some(dir) = self.studio.read(cx).workspace_dir().and_then(|d| d.parent().map(|p| p.join("shot-recordings"))) {
                    settings.folder = Some(dir);
                }
                self.exec(lightair_core::Command::SetRecordingSettings { settings }, cx);
                self.studio.update(cx, |s, cx| s.toggle_recording(cx));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                    let _ = this.update(cx, |this, cx| this.studio.update(cx, |s, cx| s.add_marker(cx)));
                    cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                    let _ = this.update(cx, |this, cx| this.studio.update(cx, |s, cx| s.toggle_recording(cx)));
                    cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                    let _ = this.update(cx, |this, cx| this.show_place(super::Place::Library, cx));
                })
                .detach();
            }
            Ok("perf") => self.perf_open = true,
            Ok("two-canvases") => self.add_canvas(lightair_core::Canvas::vertical(), cx),
            Ok("dests") => {
                // Sample destinations (no keys) to see the list.
                for (name, server, platform, on) in [
                    ("YouTube", "rtmps://a.rtmps.youtube.com:443/live2", "youtube", true),
                    ("Twitch", "rtmp://aps10.contribute.live-video.net/app/", "twitch", true),
                    ("Kick", "rtmps://example.global-contribute.live-video.net/app/", "kick", false),
                ] {
                    let destination = lightair_core::Destination {
                        id: lightair_core::Id::new(),
                        name: name.into(),
                        server: server.into(),
                        enabled: on,
                        canvas: None,
                        platform: Some(platform.into()),
                        encoding: None,
                        audio_mix: None,
                        vod_mix: None,
                        chat: None,
                    };
                    self.exec(lightair_core::Command::AddDestination { destination }, cx);
                }
            }
            Ok("edit-dest") => {
                let destination = lightair_core::Destination {
                    id: lightair_core::Id::new(),
                    name: "Twitch".into(),
                    server: "rtmp://aps10.contribute.live-video.net/app/".into(),
                    enabled: true,
                    canvas: None,
                    platform: Some("twitch".into()),
                    encoding: Some(lightair_core::Encoding {
                        bitrate_kbps: 8000,
                        height: Some(720),
                        ..Default::default()
                    }),
                    audio_mix: None,
                    vod_mix: None,
                    chat: None,
                };
                let id = destination.id;
                self.exec(lightair_core::Command::AddDestination { destination }, cx);
                self.outputs.update(cx, |o, cx| o.open_edit(id, window, cx));
            }
            Ok("dest-tiktok-vertical") => {
                if let Some(p) = crate::platforms::by_id("tiktok") {
                    self.outputs.update(cx, |o, cx| {
                        o.choose(p, window, cx);
                        o.request_vertical(cx);
                    });
                }
            }
            Ok(hook) if hook.starts_with("dest-search-") => {
                self.outputs.update(cx, |o, cx| o.open_picker(window, cx));
                self.outputs.update(cx, |o, cx| o.type_search(&hook[12..], window, cx));
            }
            Ok(hook) if hook.starts_with("dest-") => match crate::platforms::by_id(&hook[5..]) {
                Some(p) => self.outputs.update(cx, |o, cx| o.choose(p, window, cx)),
                None => self.outputs.update(cx, |o, cx| o.open_picker(window, cx)),
            },
            Ok("rename") => {
                // Rename the top object to "Renamed" and press Enter, as a person would.
                let top = {
                    let s = self.studio.read(cx);
                    s.selected_scene.and_then(|id| s.workspace().scene(id)).and_then(|sc| sc.objects.last().map(|l| l.id))
                };
                self.select_object(top, cx);
                self.rename_selection(window, cx);
                if let Some(r) = &self.renaming {
                    r.input.update(cx, |i, cx| i.set_value("Renamed", window, cx));
                }
                cx.spawn_in(window, async move |_, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                    let _ = cx.update(|window, cx| {
                        window.dispatch_keystroke(gpui_kit::Keystroke::parse("enter").unwrap(), cx);
                    });
                })
                .detach();
            }
            Ok("source") => {
                let source = {
                    let s = self.studio.read(cx);
                    s.workspace().sources.values().find(|i| i.kind == lightair_core::kinds::SCREEN).map(|i| i.id)
                };
                if let Some(id) = source {
                    self.open_source_picker(id, window, cx);
                }
            }
            Ok("crop") => {
                let top = {
                    let s = self.studio.read(cx);
                    s.selected_scene.and_then(|id| s.workspace().scene(id)).and_then(|sc| sc.objects.last().map(|l| l.id))
                };
                self.select_object(top, cx);
                self.crop_mode = top;
            }
            Ok("assets") => {
                // A prefab from the top object, and an instance of it.
                let top = {
                    let s = self.studio.read(cx);
                    s.selected_scene.and_then(|id| s.workspace().scene(id)).and_then(|sc| sc.objects.last().map(|l| l.id))
                };
                if let Some(top) = top {
                    self.select_object(Some(top), cx);
                    self.save_prefab(top, cx);
                    let assets = self.assets(cx);
                    if let Some(a) = assets.iter().find(|a| a.kind == super::assets::AssetKind::Prefab) {
                        self.place_asset(
                            &a.clone(),
                            super::assets::Placement::At(
                                self.canvas_id(&self.studio.read(cx).workspace().clone()),
                                (400.0, 300.0),
                            ),
                            cx,
                        );
                    }
                }
            }
            Ok("group") => {
                // The top two objects into a group, then an empty inside it.
                let top: Vec<lightair_core::Id> = {
                    let s = self.studio.read(cx);
                    s.selected_scene
                        .and_then(|id| s.workspace().scene(id))
                        .map(|sc| sc.objects.iter().rev().take(2).map(|l| l.id).collect())
                        .unwrap_or_default()
                };
                self.studio.update(cx, |s, _| {
                    s.selected_object = top.first().copied();
                    s.also_selected = top.iter().skip(1).copied().collect();
                });
                self.group_selection(window, cx);
                self.renaming = None;
            }
            Ok("zoom") => {
                let top = {
                    let s = self.studio.read(cx);
                    s.selected_scene.and_then(|id| s.workspace().scene(id)).and_then(|sc| sc.objects.last().map(|l| l.id))
                };
                self.select_object(top, cx);
                self.zoom = 2.5;
                self.pan = (-700.0, -350.0);
            }
            Ok("gizmo") => {
                let top = {
                    let s = self.studio.read(cx);
                    s.selected_scene.and_then(|id| s.workspace().scene(id)).and_then(|sc| sc.objects.last().map(|l| l.id))
                };
                self.select_object(top, cx);
                self.rotate_tool = true;
            }
            Ok(hook @ ("fx-stack" | "fx-sheet" | "fx-shape" | "fx-none" | "fx-bg" | "fx-system" | "fx-system-remove" | "fx-key" | "fx-key-open")) => {
                // A picture with a few effects, its stack open (or the catalog over it).
                let board = concat!(env!("CARGO_MANIFEST_DIR"), "/../../branding/final/board.png");
                self.add_source_object("Board", "image_source", serde_json::json!({"file": board}), (1920.0, 1080.0), cx);
                let (source, object) = {
                    let s = self.studio.read(cx);
                    let object = s.selected_scene.and_then(|id| s.workspace().scene(id)).and_then(|sc| sc.objects.last().cloned());
                    (object.as_ref().and_then(|l| l.source), object.map(|l| l.id))
                };
                self.select_object(object, cx);
                self.collapsed.borrow_mut().extend(["Source", "Transform"]);
                if let Some(iid) = source {
                    let kinds: &[&str] = if hook.starts_with("fx-key") {
                        &["chroma_key_filter_v2"]
                    } else if hook == "fx-system" {
                        &["studio.bg_replace"]
                    } else if hook == "fx-system-remove" {
                        &["studio.bg_remove"]
                    } else if hook == "fx-bg" {
                        &["studio.bg_blur"]
                    } else if hook == "fx-none" {
                        &[]
                    } else if hook == "fx-shape" {
                        &["studio.light", "studio.beauty"]
                    } else {
                        &["studio.beauty", "studio.bg_blur", "studio.light", "fx.vignette", "color_filter_v2"]
                    };
                    let mut first = None;
                    for kind in kinds {
                        if let Some(d) = lightair_core::catalog::def(kind) {
                            let mut effect = d.create();
                            if hook.starts_with("fx-key") {
                                effect.settings["key_color_type"] = serde_json::json!("custom");
                            }
                            if hook.starts_with("fx-system") {
                                effect.settings["bg_model"] = serde_json::json!(lightair_core::catalog::BG_MODEL_SYSTEM);
                            }
                            first.get_or_insert(effect.id);
                            self.exec(lightair_core::Command::AddSourceEffect { source: iid, effect, index: None }, cx);
                        }
                    }
                    self.props.open_effect = first;
                    if hook == "fx-sheet" {
                        self.open_effect_catalog(super::effects::Owner::Source(iid), super::effects::Page::All, window, cx);
                    }
                }
            }
            Ok(hook @ ("audio-plugin" | "audio-plugins" | "audio-vst3")) => {
                // the first track: Apple's AUDelay added (its editor opens), or the plugins page
                let track = self.studio.read(cx).workspace().tracks.first().map(|t| t.id);
                if let Some(t) = track {
                    self.studio.update(cx, |s, cx| {
                        s.selected_object = None;
                        s.selected_track = Some(t);
                        cx.notify();
                    });
                    let owner = super::effects::Owner::Track(t);
                    if hook == "audio-plugins" {
                        self.open_effect_catalog(owner, super::effects::Page::Plugins, window, cx);
                    } else if let Some(info) = if hook == "audio-vst3" {
                        crate::plugin_scan::vst3().into_iter().find(|p| p.name.contains("Surge"))
                    } else {
                        lightair_engine::plugins::scan_audio_units()
                            .into_iter()
                            .find(|p| p.vendor == "Apple" && p.name == "AUDelay")
                    } {
                        self.add_plugin(owner, &info, window, cx);
                    }
                }
            }
            Ok("audio-rename") => {
                // the first track's name, being edited on its strip
                if let Some(t) = self.studio.read(cx).workspace().tracks.first().map(|t| t.id) {
                    self.mixer.update(cx, |m, cx| m.start_rename(lightair_core::MixerStrip::Track(t), window, cx));
                }
            }
            Ok("audio-add") => {
                // the mixer's add menu, open
                self.mixer.update(cx, |m, cx| m.toggle_add_track(window, cx));
            }
            Ok("assets-scene") => {
                // the scene being edited, saved into Assets, and added back as a new scene
                if let Some(scene) = self.studio.read(cx).selected_scene {
                    self.save_scene_asset(scene, cx);
                    if let Some(a) = self.assets(cx).into_iter().find(|a| a.kind == super::assets::AssetKind::Scene) {
                        self.add_scene_asset(&a, cx);
                    }
                }
            }
            Ok("audio-colours") => {
                // the Streaming mixer with coloured strips (Mic red, Desktop blue, Music amber)
                self.use_audio_template(lightair_core::templates::Template::Streaming, cx);
                let tracks: Vec<lightair_core::Id> = self.studio.read(cx).workspace().tracks.iter().map(|t| t.id).collect();
                for (t, c) in tracks.into_iter().zip([0xE5484D, 0x0090FF, 0xFFB224]) {
                    self.exec(
                        lightair_core::Command::SetStripColor { strip: lightair_core::MixerStrip::Track(t), color: Some(c) },
                        cx,
                    );
                }
                self.exec(lightair_core::Command::SetStripColor { strip: lightair_core::MixerStrip::Master, color: Some(0x30A46C) }, cx);
            }
            Ok("console") => {
                // the console open, the first track selected (its pan in the Inspector)
                self.console_open = true;
                let first = self.studio.read(cx).workspace().tracks.first().map(|t| t.id);
                self.studio.update(cx, |s, cx| {
                    s.selected_object = None;
                    s.select_track(first);
                    cx.notify();
                });
            }
            Ok("audio-template") => {
                // the Streaming template (it changes no preferences), the mic selected
                self.use_audio_template(lightair_core::templates::Template::Streaming, cx);
                let first = self.studio.read(cx).workspace().tracks.first().map(|t| t.id);
                self.studio.update(cx, |s, cx| {
                    s.selected_object = None;
                    s.select_track(first);
                    cx.notify();
                });
            }
            Ok("audio-eq") => {
                // an EQ on the master, its card open: the response curve in view
                if let Some(d) = lightair_core::catalog::def("audio.eq") {
                    let mut effect = d.create();
                    effect.settings["b1_on"] = serde_json::json!(true);
                    effect.settings["b5_on"] = serde_json::json!(true);
                    effect.settings["b5_gain"] = serde_json::json!(4.0);
                    effect.settings["b8_on"] = serde_json::json!(true);
                    self.props.open_effect = Some(effect.id);
                    self.exec(
                        lightair_core::Command::AddStripEffect { strip: lightair_core::MixerStrip::Master, effect, index: None },
                        cx,
                    );
                    self.studio.update(cx, |s, cx| {
                        s.selected_object = None;
                        s.select_track(Some(lightair_core::MASTER_ID));
                        cx.notify();
                    });
                }
            }
            Ok(hook @ ("audio-mixer" | "audio-bus")) => {
                // a Guests bus, a Headphones mix, the mic routed to the bus and sent to the mix
                let workspace = self.studio.read(cx).workspace().clone();
                if let Some(t) = workspace.tracks.first().map(|t| (**t).clone()) {
                    let bus = lightair_core::Bus::new("Guests");
                    let mix = lightair_core::AudioMix::new("Headphones");
                    let (bid, _mid) = (bus.id, mix.id);
                    self.exec(lightair_core::Command::AddBus { bus, index: None }, cx);
                    self.exec(lightair_core::Command::AddMix { mix: mix.clone() }, cx);
                    let track = lightair_core::Track {
                        route: lightair_core::Route::Bus(bid),
                        sends: vec![lightair_core::Send { mix: mix.id, level_db: -3.0, pre_fader: true }],
                        pan: -0.3,
                        ..t.clone()
                    };
                    self.exec(lightair_core::Command::UpdateTrack { track }, cx);
                    if let Some(d) = lightair_core::catalog::def("audio.limiter") {
                        self.exec(
                            lightair_core::Command::AddStripEffect {
                                strip: lightair_core::MixerStrip::Master,
                                effect: d.create(),
                                index: None,
                            },
                            cx,
                        );
                    }
                    let sel = if hook == "audio-bus" { bid } else { t.id };
                    self.studio.update(cx, |s, cx| {
                        s.selected_object = None;
                        s.select_track(Some(sel));
                        cx.notify();
                    });
                }
            }
            Ok(hook @ ("audio-track" | "audio-sheet")) => {
                // the first track, its effects open (or the audio catalog over it)
                let track = self.studio.read(cx).workspace().tracks.first().map(|t| (**t).clone());
                if let Some(t) = track {
                    let mut first = None;
                    for kind in ["audio.eq", "audio.compressor"] {
                        if let Some(d) = lightair_core::catalog::def(kind) {
                            let mut effect = d.create();
                            if kind == "audio.eq" {
                                effect.settings["b1_on"] = serde_json::json!(true);
                                effect.settings["b5_on"] = serde_json::json!(true);
                                effect.settings["b5_gain"] = serde_json::json!(3.0);
                            }
                            first.get_or_insert(effect.id);
                            self.exec(lightair_core::Command::AddEffect { track: t.id, effect, index: None }, cx);
                        }
                    }
                    self.props.open_effect = first;
                    self.studio.update(cx, |s, cx| {
                        s.selected_object = None;
                        s.selected_track = Some(t.id);
                        cx.notify();
                    });
                    if hook == "audio-sheet" {
                        self.open_effect_catalog(super::effects::Owner::Track(t.id), super::effects::Page::All, window, cx);
                    }
                }
            }
            Ok("select") => {
                let top = {
                    let s = self.studio.read(cx);
                    s.selected_scene.and_then(|id| s.workspace().scene(id)).and_then(|sc| sc.objects.last().map(|l| l.id))
                };
                self.select_object(top, cx);
            }
            _ => {}
        }
    }
}
