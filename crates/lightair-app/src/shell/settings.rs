//! Settings (⌘,; docs/14-data-persistence-settings.md, "Settings"): a window with a page per
//! area. Recording, streaming, clips and canvases travel with the workspace; General is this
//! Mac's. Each destination has its own settings too, in Outputs (click it).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::{ClipSettings, Command, Canvas, Reconnect, RecordingSettings, VideoEncoder};

use gpui_kit::component::input::{Input, InputState};

use super::StudioWindow;
use crate::ui::{self, tok};

/// A canvas being edited: its name and size fields, and the frame rate picked.
pub(crate) struct CanvasEdit {
    id: lightair_core::Id,
    name: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    fps: u32,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Page {
    General,
    Audio,
    Camera,
    Recording,
    Streaming,
    Clips,
    Canvases,
    Shortcuts,
    Remote,
    Connections,
    Permissions,
    About,
}

const PAGES: &[(Page, &str, &str)] = &[
    (Page::General, "General", "settings"),
    (Page::Audio, "Audio", "audio-lines"),
    (Page::Recording, "Recording", "circle-dot"),
    (Page::Streaming, "Streaming", "radio"),
    (Page::Clips, "Clips", "scissors"),
    (Page::Canvases, "Canvases", "proportions"),
    (Page::Camera, "Camera", "camera"),
    (Page::Shortcuts, "Shortcuts", "keyboard"),
    (Page::Remote, "Remote API", "radio-tower"),
    (Page::Connections, "Connections", "plug"),
    (Page::Permissions, "Permissions", "shield-check"),
    (Page::About, "About", "info"),
];

/// Every keyboard shortcut, as the Shortcuts page lists them.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Objects",
        &[
            ("Cut, copy, paste", "⌘X  ⌘C  ⌘V"),
            ("Duplicate", "⌘D"),
            ("Delete", "⌫"),
            ("Rename", "↩"),
            ("Select all", "⌘A"),
            ("Deselect", "Esc"),
            ("Group / ungroup", "⌘G  ⇧⌘G"),
            ("Create empty / inside", "⇧⌘N  ⌥⇧N"),
            ("Bring forward / send backward", "⌘]  ⌘["),
            ("To front / to back", "⇧⌘]  ⇧⌘["),
            ("Show or hide / lock", "⇧⌘H  ⇧⌘L"),
            ("Nudge", "Arrow keys"),
            ("Rotate gizmo", "⇧⌘E"),
        ],
    ),
    (
        "Canvas",
        &[
            ("Zoom in / out", "⌘=  ⌘−"),
            ("Fit / actual size", "⌘0  ⌥⌘0"),
            ("Pan", "Scroll, Space-drag"),
            ("Crop", "⌥-drag a corner, drag an edge"),
        ],
    ),
    (
        "Show",
        &[
            ("Scene 1–9", "⌘1 … ⌘9"),
            ("Take (with Cue scenes)", "⌘↩"),
            ("Add marker", "⌘M"),
            ("Mute microphones", "⇧⌘M"),
            ("Undo / redo", "⌘Z  ⇧⌘Z"),
            ("Command palette", "⌘K"),
            ("Settings", "⌘,"),
        ],
    ),
];

impl StudioWindow {
    pub(super) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        cx.notify();
    }

    /// Opens Settings at a page.
    pub(super) fn open_settings(&mut self, page: Page, cx: &mut Context<Self>) {
        self.settings_open = true;
        self.settings_page = page;
        cx.notify();
    }

    fn set_recording(&mut self, f: impl FnOnce(&mut RecordingSettings), cx: &mut Context<Self>) {
        let mut settings = self.studio.read(cx).workspace().outputs.recording.clone();
        f(&mut settings);
        self.exec(Command::SetRecordingSettings { settings }, cx);
    }

    fn set_clips(&mut self, f: impl FnOnce(&mut ClipSettings), cx: &mut Context<Self>) {
        let mut settings = self.studio.read(cx).workspace().outputs.clips;
        f(&mut settings);
        self.exec(Command::SetClipSettings { settings }, cx);
    }

    fn choose_recording_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths =
            cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| this.set_recording(|r| r.folder = Some(dir), cx));
        })
        .detach();
    }

    pub(super) fn edit_canvas(&mut self, id: lightair_core::Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.studio.read(cx).workspace().canvas(id).cloned() else { return };
        let field = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            let e = cx.new(|cx| InputState::new(window, cx));
            e.update(cx, |i, cx| i.set_value(value, window, cx));
            e
        };
        let name = field(f.name.clone(), window, cx);
        let width = field(f.width.to_string(), window, cx);
        let height = field(f.height.to_string(), window, cx);
        let fps = f.fps_num / f.fps_den.max(1);
        self.canvas_edit = Some(CanvasEdit { id, name, width, height, fps, error: None });
        cx.notify();
    }

    /// Applies the canvas edit: the new size, with every top-level object reframed into it.
    fn apply_canvas_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = &mut self.canvas_edit else { return };
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(old) = workspace.canvas(edit.id).cloned() else { return };
        let num = |e: &Entity<InputState>, cx: &App| e.read(cx).value().trim().parse::<u32>().ok();
        let (Some(w), Some(h)) = (num(&edit.width, cx), num(&edit.height, cx)) else {
            edit.error = Some("Width and height are whole numbers of pixels.".into());
            cx.notify();
            return;
        };
        if !(16..=7680).contains(&w) || !(16..=7680).contains(&h) {
            edit.error = Some("A canvas is 16 to 7680 pixels each way.".into());
            cx.notify();
            return;
        }
        let name = edit.name.read(cx).value().trim().to_string();
        let new = Canvas {
            name: if name.is_empty() { old.name.clone() } else { name },
            width: w & !1,
            height: h & !1,
            fps_num: edit.fps,
            fps_den: 1,
            ..old.clone()
        };
        let mut commands = vec![Command::UpdateCanvas { canvas: new.clone() }];
        if (new.width, new.height) != (old.width, old.height) {
            for scene in &workspace.scenes {
                for object in scene.objects.iter().filter(|l| l.parent.is_none()) {
                    if let Some(l) = object.layouts.get(&old.id) {
                        let layout = crate::reframe::reframe(l, &old, &new);
                        commands.push(Command::SetLayout { scene: scene.id, object: object.id, canvas: old.id, layout });
                    }
                }
            }
        }
        self.canvas_edit = None;
        self.exec(Command::Batch { label: format!("Change {}", new.name.to_lowercase()), commands }, cx);
    }

    /// The keys pressed for the global shortcut being set.
    pub(super) fn capture_shortcut(&mut self, k: &Keystroke, cx: &mut Context<Self>) {
        let Some(action) = self.shortcut_capture else { return };
        if k.key == "escape" {
            self.shortcut_capture = None;
            cx.notify();
            return;
        }
        // Wait for a key, not just modifiers.
        if matches!(k.key.as_str(), "shift" | "control" | "alt" | "cmd" | "platform" | "function" | "") {
            return;
        }
        let mut parts = Vec::new();
        for (on, name) in [
            (k.modifiers.control, "ctrl"),
            (k.modifiers.alt, "alt"),
            (k.modifiers.shift, "shift"),
            (k.modifiers.platform, "cmd"),
        ] {
            if on {
                parts.push(name.to_string());
            }
        }
        parts.push(k.key.to_lowercase());
        let keys = parts.join("-");
        if crate::shortcuts::parse(&keys).is_none() {
            self.shortcut_note = Some(if cfg!(windows) { "Use Ctrl or Alt with a letter or number, or an F key.".into() } else { "Use ⌘, ⌃ or ⌥ with a letter or number, or an F key.".into() });
            cx.notify();
            return;
        }
        self.shortcut_capture = None;
        let mut list = self.prefs.global_shortcuts.clone();
        list.retain(|h| h.action != action && h.keys != keys);
        list.push(crate::prefs::GlobalShortcut { action, keys: keys.clone() });
        let refused = crate::shortcuts::register_all(&list.iter().map(|h| (h.action, h.keys.clone())).collect::<Vec<_>>());
        if refused.contains(&action) {
            list.retain(|h| h.action != action);
            crate::shortcuts::register_all(&list.iter().map(|h| (h.action, h.keys.clone())).collect::<Vec<_>>());
            self.shortcut_note = Some(format!("{} is taken by {} or another app.", crate::shortcuts::display(&keys), ui::OS));
        } else {
            self.shortcut_note = None;
        }
        self.set_pref(|p| p.global_shortcuts = list, cx);
    }

    fn clear_shortcut(&mut self, action: crate::shortcuts::GlobalAction, cx: &mut Context<Self>) {
        let mut list = self.prefs.global_shortcuts.clone();
        list.retain(|h| h.action != action);
        crate::shortcuts::register_all(&list.iter().map(|h| (h.action, h.keys.clone())).collect::<Vec<_>>());
        self.set_pref(|p| p.global_shortcuts = list, cx);
    }

    /// The audio engine's buffer (frames), now and from now on.
    fn set_audio_block(&mut self, frames: usize, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let done = self.studio.read(cx).engine().call(move |s| s.reconciler.set_audio_block(frames, &workspace));
        if done {
            self.set_pref(|p| p.audio_block = frames, cx);
        } else {
            self.studio.update(cx, |s, _| s.issue = Some("Change the buffer after the recording ends.".into()));
        }
    }

    /// Sets the mixer up as a template (one undo step), with the settings it wants.
    pub(super) fn use_audio_template(&mut self, t: lightair_core::templates::Template, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        self.exec(crate::mixer::template_command(&workspace, t), cx);
        if let Some(frames) = t.block() {
            self.set_audio_block(frames, cx);
        }
        if t == lightair_core::templates::Template::Podcast {
            crate::prefs::LOUDNESS_TARGET.store(-16, std::sync::atomic::Ordering::Relaxed);
            self.set_pref(|p| p.loudness_target = -16, cx);
        }
        cx.notify();
    }

    /// Tells the audio engine where and what to monitor, from the preferences.
    fn apply_headphones(&self, cx: &mut Context<Self>) {
        let p = &self.prefs;
        // the device by its id (the same one libobs's monitoring uses)
        let device = p.low_latency_headphones.then(|| p.headphones_device.clone().map_or(String::new(), |(_, id)| id));
        let program = p.headphones_hear_program;
        self.studio.read(cx).engine().call(move |s| s.reconciler.set_headphones(device, program));
    }

    fn set_pref(&mut self, f: impl FnOnce(&mut crate::prefs::Preferences), cx: &mut Context<Self>) {
        f(&mut self.prefs);
        if let Err(e) = crate::prefs::save_to(&crate::prefs::path(), &self.prefs) {
            self.studio.update(cx, |s, _| s.issue = Some(format!("Couldn't save preferences: {e}").into()));
        }
        cx.notify();
    }

    pub(super) fn settings_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.settings_open {
            return None;
        }
        let page = self.settings_page;
        let mut nav =
            div().flex().flex_col().gap_0p5().w(px(176.)).flex_none().p_2().border_r_1().border_color(tok::border());
        for (i, (p, label, icon)) in PAGES.iter().enumerate() {
            let on = *p == page;
            let p = *p;
            nav = nav.child(
                div()
                    .id(("settings-page", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(30.))
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                    .when(!on, |d| d.text_color(tok::text_secondary()).hover(|d| d.bg(tok::bg_raised())))
                    .child(ui::icon(icon).size(px(14.)))
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_page = p;
                        this.shortcut_capture = None;
                        if p == Page::Audio {
                            this.headphones_devices =
                                this.studio.read(cx).engine().call(|_| lightair_engine::monitoring::devices());
                        }
                        cx.notify();
                    })),
            );
        }
        let content = match page {
            Page::General => self.general_page(cx),
            Page::Audio => self.audio_page(cx),
            Page::Camera => self.camera_page(cx),
            Page::Recording => self.recording_page(cx),
            Page::Streaming => self.streaming_page(cx),
            Page::Clips => self.clips_page(cx),
            Page::Canvases => self.canvases_page(cx),
            Page::Shortcuts => self.shortcuts_page(cx),
            Page::Remote => self.remote_page(cx),
            Page::Connections => self.connections_page(cx),
            Page::Permissions => self.permissions_page(cx),
            Page::About => self.about_page(cx),
        };
        let title = PAGES.iter().find(|(p, ..)| *p == page).map_or("", |(_, l, _)| *l);
        let live = lightair_engine::outputs::runtime().any_stream_running()
            || lightair_engine::outputs::runtime().any_recording_running();
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_center()
                .bg(tok::scrim())
                // a sheet: nothing behind it takes the mouse (scrolling here never pans the canvas)
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    this.toggle_settings(cx);
                    window.focus(&this.focus, cx);
                }))
                .child(
                    div()
                        .id("settings")
                        .flex()
                        .w(px(780.))
                        .h(px(580.))
                        .rounded(px(14.))
                        .overflow_hidden()
                        .bg(tok::bg_panel())
                        .border_1()
                        .border_color(tok::border_strong())
                        .shadow_lg()
                        .track_focus(&self.settings_focus)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        // (a shortcut being set takes its keys first: `StudioWindow::_shortcut_keys`)
                        .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            if this.shortcut_capture.is_none() && ev.keystroke.key == "escape" {
                                this.toggle_settings(cx);
                                window.focus(&this.focus, cx);
                            }
                        }))
                        .child(
                            nav.child(div().flex_1()).child(
                                div()
                                    .px_2()
                                    .pb_1()
                                    .text_size(px(10.))
                                    .text_color(tok::text_tertiary())
                                    .child("Saved as you change them"),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .px_5()
                                        .pt_4()
                                        .pb_2()
                                        .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(title))
                                        .child(
                                            Button::new("settings-done")
                                                .label("Done")
                                                .small()
                                                .primary()
                                                .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
                                        ),
                                )
                                .when(live, |d| {
                                    d.child(
                                        div()
                                            .px_5()
                                            .pb_2()
                                            .text_size(px(11.))
                                            .text_color(tok::warn())
                                            .child("Recording or live: changes apply the next time it starts."),
                                    )
                                })
                                .child(
                                    div()
                                        .id("settings-content")
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .px_5()
                                        .pb_5()
                                        .child(content),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    // ------------------------------------------------------------ pages

    fn general_page(&self, cx: &mut Context<Self>) -> Div {
        let review = self.prefs.review_before_live;
        let confirm = self.prefs.confirm_end_stream;
        let studio = self.studio.read(cx);
        let workspace_dir = studio.workspace_dir();
        let name = studio.workspace().name.clone();
        pages()
            .child(
                group("Going live")
                    .child(row(
                        "Review before going live",
                        Some("Show the destinations and checks first."),
                        switch("pref-review", review).on_click(
                            cx.listener(move |this, _, _, cx| this.set_pref(|p| p.review_before_live = !review, cx)),
                        ),
                    ))
                    .child(row(
                        "Ask before ending a stream",
                        Some("Ending takes a second click on the Live button."),
                        switch("pref-confirm-end", confirm).on_click(
                            cx.listener(move |this, _, _, cx| this.set_pref(|p| p.confirm_end_stream = !confirm, cx)),
                        ),
                    )),
            )
            .child(group("Workspace").child(row(
                name.as_str(),
                workspace_dir.as_ref().map(|d| d.to_string_lossy().into_owned()).as_deref(),
                reveal_button("reveal-workspace", workspace_dir),
            )))
            .child(note(&format!("General settings belong to this {}. The rest are saved with the workspace.", ui::MACHINE)))
    }

    fn audio_page(&self, cx: &mut Context<Self>) -> Div {
        let prefs = &self.prefs;
        let current = prefs.headphones_device.clone().unwrap_or((crate::mixer::SYSTEM_DEFAULT.into(), "default".into()));
        let devices = self.headphones_devices.clone();
        let me = cx.entity().downgrade();
        let device = Button::new("headphones-device").label(current.0.clone()).small().outline().dropdown_menu(
            move |mut menu, _, _| {
                for (name, id) in devices.iter().cloned() {
                    let me = me.clone();
                    let on = id == current.1;
                    menu = menu.item(gpui_kit::component::menu::PopupMenuItem::new(name.clone()).checked(on).on_click(
                        move |_, _, cx| {
                            if let Some(me) = me.upgrade() {
                                let (n, i) = (name.clone(), id.clone());
                                me.update(cx, |this, cx| {
                                    let (n2, i2) = (n.clone(), i.clone());
                                    this.studio
                                        .read(cx)
                                        .engine()
                                        .call(move |_| lightair_engine::monitoring::set(&n2, &i2));
                                    this.set_pref(|p| p.headphones_device = (i != "default").then_some((n, i)), cx);
                                    this.apply_headphones(cx);
                                });
                            }
                        },
                    ));
                }
                menu
            },
        );
        let (direct, program) = (prefs.low_latency_headphones, prefs.headphones_hear_program);
        let mut hear = div().flex().gap_0p5().p_0p5().rounded(px(7.)).bg(tok::bg_panel());
        for (i, (p, label)) in [(false, "Cued tracks"), (true, "The stream")].into_iter().enumerate() {
            hear = hear.child(pref_chip(("headphones-hear", i), label.to_string(), program == p).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.set_pref(|pr| pr.headphones_hear_program = p, cx);
                    this.apply_headphones(cx);
                },
            )));
        }
        let rate = prefs.sample_rate;
        let running = crate::prefs::RUNNING_SAMPLE_RATE.load(std::sync::atomic::Ordering::Relaxed);
        let rates: Vec<(String, bool, bool, Option<u32>)> = [(48_000, "48 kHz"), (44_100, "44.1 kHz")]
            .into_iter()
            .map(|(r, l)| (l.to_string(), rate == r, true, Some(r)))
            .collect();
        let mut rate_chips = div().flex().gap_0p5().p_0p5().rounded(px(7.)).bg(tok::bg_panel());
        for (i, (label, on, _, r)) in rates.into_iter().enumerate() {
            rate_chips = rate_chips.child(pref_chip(("sample-rate", i), label, on).on_click(cx.listener(
                move |this, _, _, cx| {
                    if let Some(r) = r {
                        this.set_pref(|p| p.sample_rate = r, cx);
                    }
                },
            )));
        }
        let block = prefs.audio_block;
        let mut blocks = div().flex().gap_0p5().p_0p5().rounded(px(7.)).bg(tok::bg_panel());
        for (i, frames) in lightair_engine::audio_host::BLOCKS.into_iter().enumerate() {
            let ms = frames as f32 * 1000.0 / running.max(1) as f32;
            blocks = blocks.child(
                pref_chip(("audio-block", i), format!("{frames} · {ms:.1} ms"), block == frames)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_audio_block(frames, cx))),
            );
        }
        let mut templates = group("Start from a template");
        for (i, t) in lightair_core::templates::Template::ALL.into_iter().enumerate() {
            templates = templates.child(row(
                t.name(),
                Some(t.description()),
                Button::new(("audio-template", i)).label("Use").small().outline().on_click(
                    cx.listener(move |this, _, _, cx| this.use_audio_template(t, cx)),
                ),
            ));
        }
        templates = templates.child(note("Replaces the mixer's tracks, buses and mixes. Undo brings them back."));
        let target = prefs.loudness_target;
        let mut targets = div().flex().gap_0p5().p_0p5().rounded(px(7.)).bg(tok::bg_panel());
        for (i, (t, label)) in
            [(-14, "−14 Streaming"), (-16, "−16 Podcasts"), (-23, "−23 Broadcast")].into_iter().enumerate()
        {
            targets = targets.child(pref_chip(("loudness-target", i), label.to_string(), target == t).on_click(
                cx.listener(move |this, _, _, cx| {
                    crate::prefs::LOUDNESS_TARGET.store(t, std::sync::atomic::Ordering::Relaxed);
                    this.set_pref(|p| p.loudness_target = t, cx);
                }),
            ));
        }
        pages()
            .child(
                group("Listening")
                    .child(row("Headphones", Some("Where Listen in headphones plays."), device))
                    .child(row(
                        "Low-latency headphones",
                        Some("Straight from the mixer: a few milliseconds, for singing and playing along."),
                        switch("low-latency-headphones", direct).on_click(cx.listener(move |this, _, _, cx| {
                            this.set_pref(|p| p.low_latency_headphones = !direct, cx);
                            this.apply_headphones(cx);
                        })),
                    ))
                    .child(row("Hear", Some("The tracks you cue, or exactly what viewers hear."), hear)),
            )
            .child(templates)
            .child(
                group("Loudness")
                    .child(row("Target", Some("The mixer's LUFS readout turns green on it."), targets))
                    .child(note("YouTube, Twitch and Spotify even out to about −14 LUFS; Apple Podcasts −16; TV −23.")),
            )
            .child(
                group("Engine")
                    .child(row("Sample rate", Some("48 kHz suits video; 44.1 kHz matches music files."), rate_chips))
                    .child(row(
                        "Buffer",
                        Some(if cfg!(windows) {
                            "Smaller: less delay when you hear yourself. Larger: lighter on the PC."
                        } else {
                            "Smaller: less delay when you hear yourself. Larger: lighter on the Mac."
                        }),
                        blocks,
                    ))
                    .when(rate != running, |g| {
                        g.child(
                            div()
                                .px_1()
                                .text_size(px(11.))
                                .text_color(tok::warn())
                                .child("Takes effect the next time Lightair opens."),
                        )
                    }),
            )
    }

    fn camera_page(&self, cx: &mut Context<Self>) -> Div {
        let cam = self.prefs.camera;
        let mut sizes = div().flex().gap_0p5().p_0p5().rounded(px(7.)).bg(tok::bg_panel());
        for (i, (h, label)) in
            [(None, "Best"), (Some(720), "720p"), (Some(1080), "1080p"), (Some(2160), "4K")].into_iter().enumerate()
        {
            sizes = sizes.child(
                pref_chip(("camera-size", i), label.to_string(), cam.height == h)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_pref(|p| p.camera.height = h, cx))),
            );
        }
        let studio_on = cam.studio_on;
        pages()
            .child(
                group("New cameras")
                    .child(row("Resolution", Some("Best uses the most the camera offers."), sizes))
                    .child(row(
                        "Beauty on",
                        Some("Start with the Beauty effect (adjust it in the Inspector)."),
                        switch("camera-studio", studio_on).on_click(
                            cx.listener(move |this, _, _, cx| this.set_pref(|p| p.camera.studio_on = !studio_on, cx)),
                        ),
                    )),
            )
            .child(note("Cameras already added keep their settings; change them in their Inspector."))
    }

    fn shortcuts_page(&self, cx: &mut Context<Self>) -> Div {
        use crate::shortcuts::{GlobalAction, display};
        let mut global = group(if cfg!(windows) { "Anywhere on your PC" } else { "Anywhere on your Mac" });
        for (i, action) in GlobalAction::ALL.iter().copied().enumerate() {
            let keys = self.prefs.global_shortcuts.iter().find(|h| h.action == action).map(|h| h.keys.clone());
            let capturing = self.shortcut_capture == Some(action);
            let control = div()
                .flex()
                .items_center()
                .gap_1()
                .child(if capturing {
                    div().text_size(px(12.)).text_color(tok::warn()).child("Press keys… (Esc cancels)")
                } else {
                    value(&keys.as_deref().map(display).unwrap_or_else(|| "—".into()))
                })
                .child(
                    Button::new(("set-shortcut", i))
                        .label(if keys.is_some() { "Change" } else { "Set" })
                        .xsmall()
                        .outline()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.shortcut_capture = Some(action);
                            this.shortcut_note = None;
                            // the keys pressed next come here
                            window.focus(&this.settings_focus, cx);
                            cx.notify();
                        })),
                )
                .when(keys.is_some(), |d| {
                    d.child(
                        Button::new(("clear-shortcut", i))
                            .icon(ui::icon("x"))
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| this.clear_shortcut(action, cx))),
                    )
                });
            global = global.child(row(&action.label(), None, control));
        }
        if let Some(n) = &self.shortcut_note {
            global = global.child(div().px_1().text_size(px(11.)).text_color(tok::warn()).child(n.clone()));
        }
        let mut page = pages().child(global.child(note(
            "These work whichever app is in front: start a recording or switch scenes from a game or a call.",
        )));
        for (title, items) in SHORTCUTS {
            let mut g = group(title);
            for (what, keys) in *items {
                g = g.child(row(what, None, value(&crate::ui::keys_label(keys))));
            }
            page = page.child(g);
        }
        page
    }

    fn recording_page(&self, cx: &mut Context<Self>) -> Div {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let caps = studio.engine().capabilities.encoders.clone();
        let folder = studio.recording_folder();
        let rec = workspace.outputs.recording.clone();
        let targets = rec.targets(&workspace);

        let mut canvases = group("Canvases");
        for (i, f) in workspace.canvases.iter().enumerate() {
            let on = targets.contains(&f.id);
            let (fid, r2, p2) = (f.id, rec.clone(), workspace.clone());
            canvases = canvases.child(row(
                &format!("{} {}×{}", f.name, f.width, f.height),
                None,
                switch(("rec-canvas", i), on).on_click(cx.listener(move |this, _, _, cx| {
                    let settings = r2.with_canvas(&p2, fid, !on);
                    this.exec(Command::SetRecordingSettings { settings }, cx);
                })),
            ));
        }
        let prores = rec.encoder == VideoEncoder::ProRes;
        let encoder_choices: Vec<(String, bool, bool, Command)> =
            [VideoEncoder::H264, VideoEncoder::Hevc, VideoEncoder::ProRes, VideoEncoder::X264]
                .into_iter()
                .map(|e| {
                    // ProRes lives in MOV files.
                    let container = if e == VideoEncoder::ProRes { "mov".to_string() } else { rec.container.clone() };
                    let settings = RecordingSettings { encoder: e, container, ..rec.clone() };
                    (
                        e.label().to_string(),
                        rec.encoder == e,
                        crate::encoding::available(&caps, e),
                        Command::SetRecordingSettings { settings },
                    )
                })
                .collect();
        let containers: Vec<(String, bool, bool, Command)> = [("mp4", "MP4"), ("mov", "MOV"), ("mkv", "MKV")]
            .into_iter()
            .map(|(v, label)| {
                let settings = RecordingSettings { container: v.into(), ..rec.clone() };
                (
                    label.to_string(),
                    rec.container == v,
                    !prores || v == "mov",
                    Command::SetRecordingSettings { settings },
                )
            })
            .collect();
        let heights = height_choices(rec.height, |height| Command::SetRecordingSettings {
            settings: RecordingSettings { height, ..rec.clone() },
        });
        let audio: Vec<(String, bool, bool, Command)> = [128, 192, 256, 320]
            .into_iter()
            .map(|k| {
                let settings = RecordingSettings { audio_kbps: k, ..rec.clone() };
                (format!("{k} kbps"), rec.audio_kbps == k, true, Command::SetRecordingSettings { settings })
            })
            .collect();
        let splits: Vec<(String, bool, bool, Command)> = [None, Some(15), Some(30), Some(60)]
            .into_iter()
            .map(|m| {
                let settings = RecordingSettings { split_minutes: m, ..rec.clone() };
                (
                    m.map_or("Never".to_string(), |m| format!("{m} min")),
                    rec.split_minutes == m,
                    true,
                    Command::SetRecordingSettings { settings },
                )
            })
            .collect();
        let bitrate = |kbps: u32| Command::SetRecordingSettings {
            settings: RecordingSettings { bitrate_kbps: kbps.clamp(2_000, 150_000), ..rec.clone() },
        };
        pages()
            .child(canvases.child(note("Record records each canvas that's on, each to its own file.")))
            .child(
                group("Files")
                    .child(row(
                        "Folder",
                        Some(folder.to_string_lossy().as_ref()),
                        Button::new("rec-folder")
                            .label("Choose…")
                            .xsmall()
                            .outline()
                            .on_click(cx.listener(|this, _, window, cx| this.choose_recording_folder(window, cx)))
                            .into_any_element(),
                    ))
                    .child(row(
                        "File type",
                        Some(match rec.container.as_str() {
                            "mkv" => "Survives a crash; no markers.",
                            "mov" => "For editors; ProRes needs it.",
                            _ => "Crash-safe while recording, a normal MP4 after.",
                        }),
                        chips("rec-container", containers, cx),
                    ))
                    .child(row(
                        "New file every",
                        Some("Long shows in parts, without a gap."),
                        chips("rec-split", splits, cx),
                    )),
            )
            .child(
                group("Video")
                    .child(row(
                        "Encoder",
                        Some(match rec.encoder {
                            VideoEncoder::H264 if cfg!(windows) => "Plays everywhere. Uses the graphics card's encoder.",
                            VideoEncoder::H264 => "Plays everywhere. Uses the Mac's media engine.",
                            VideoEncoder::Hevc => "Half the size at the same quality.",
                            VideoEncoder::ProRes => "For editing: big files, no quality loss.",
                            VideoEncoder::X264 => "Software: uses the CPU.",
                        }),
                        chips("rec-encoder", encoder_choices, cx),
                    ))
                    .when(!prores, |g| {
                        g.child(row(
                            "Quality",
                            Some("Higher is sharper and bigger: about 90 MB a minute at 12 Mbps."),
                            stepper(
                                "rec-bitrate",
                                format!("{:.0} Mbps", rec.bitrate_kbps as f32 / 1000.0),
                                bitrate(rec.bitrate_kbps.saturating_sub(2_000)),
                                bitrate(rec.bitrate_kbps + 2_000),
                                cx,
                            ),
                        ))
                    })
                    .child(row(
                        "Resolution",
                        Some("Smaller than the canvas is scaled down."),
                        chips("rec-height", heights, cx),
                    )),
            )
            .child(group("Audio").child(row("Quality", None, chips("rec-audio", audio, cx))))
            .child(self.multitrack_group(&rec, cx))
    }

    /// Multitrack audio: each track (or those marked Record) to its own file.
    fn multitrack_group(&self, rec: &RecordingSettings, cx: &mut Context<Self>) -> Div {
        use lightair_core::{MultitrackFormat, MultitrackPoint};
        let set = |r: RecordingSettings| Command::SetRecordingSettings { settings: r };
        let (multi, only) = (rec.multitrack, rec.audio_only);
        let (m, o) = (set(RecordingSettings { multitrack: !multi, ..rec.clone() }), set(RecordingSettings { audio_only: !only, ..rec.clone() }));
        let formats: Vec<(String, bool, bool, Command)> = [(MultitrackFormat::Pcm24, "24-bit"), (MultitrackFormat::Float32, "32-bit float")]
            .into_iter()
            .map(|(f, l)| (l.to_string(), rec.multitrack_format == f, multi || only, set(RecordingSettings { multitrack_format: f, ..rec.clone() })))
            .collect();
        let points: Vec<(String, bool, bool, Command)> =
            [(MultitrackPoint::Raw, "Before effects"), (MultitrackPoint::PreFader, "After effects"), (MultitrackPoint::PostFader, "As mixed")]
                .into_iter()
                .map(|(p, l)| (l.to_string(), rec.multitrack_point == p, multi || only, set(RecordingSettings { multitrack_point: p, ..rec.clone() })))
                .collect();
        let workspace = self.studio.read(cx).workspace().clone();
        let mixes: Vec<(String, bool, bool, Command)> = workspace
            .audio
            .mixes
            .iter()
            .take(5)
            .map(|m| {
                let on = rec.file_mixes.contains(&m.id);
                let mut file_mixes = rec.file_mixes.clone();
                if on {
                    file_mixes.retain(|x| *x != m.id);
                } else {
                    file_mixes.push(m.id);
                }
                (m.name.clone(), on, true, set(RecordingSettings { file_mixes, ..rec.clone() }))
            })
            .collect();
        group("Multitrack audio")
            .when(!mixes.is_empty(), |g| {
                g.child(row(
                    "Mixes in the video file",
                    Some("Extra audio tracks after the program (MOV or MKV keep them all)."),
                    chips("rec-file-mixes", mixes, cx),
                ))
            })
            .child(row(
                "Each track to its own file",
                Some("WAV files beside the video, lined up to the sample. Tracks marked Record, or all of them."),
                switch("rec-multi", multi).on_click(cx.listener(move |this, _, _, cx| this.exec(m.clone(), cx))),
            ))
            .child(row(
                "Audio only",
                Some("Podcasts and music: the track files and the mix, no video."),
                switch("rec-audio-only", only).on_click(cx.listener(move |this, _, _, cx| this.exec(o.clone(), cx))),
            ))
            .child(row(
                "Format",
                Some(match rec.multitrack_format {
                    MultitrackFormat::Pcm24 => "Opens in every editor and DAW.",
                    MultitrackFormat::Float32 => "Can't clip: for music.",
                }),
                chips("rec-multitrack-format", formats, cx),
            ))
            .child(row(
                "Record",
                Some(match rec.multitrack_point {
                    MultitrackPoint::Raw => "The clean sources, to process later.",
                    MultitrackPoint::PreFader => "With each track's effects, at full level.",
                    MultitrackPoint::PostFader => "As the mix has them: faders and pan.",
                }),
                chips("rec-multitrack-point", points, cx),
            ))
    }

    fn streaming_page(&self, cx: &mut Context<Self>) -> Div {
        let workspace = self.studio.read(cx).workspace().clone();
        let kbps = workspace.outputs.stream_bitrate_kbps;
        let rc = workspace.outputs.reconnect;
        let set = |r: Reconnect| Command::SetReconnect { reconnect: r };
        let retries: Vec<(String, bool, bool, Command)> =
            [(0, "Off"), (5, "5 tries"), (20, "20 tries"), (100, "Keep trying")]
                .into_iter()
                .map(|(n, label)| (label.to_string(), rc.retries == n, true, set(Reconnect { retries: n, ..rc })))
                .collect();
        let delays: Vec<(String, bool, bool, Command)> = [2, 5, 10]
            .into_iter()
            .map(|d| (format!("{d} s"), rc.delay_s == d, rc.retries > 0, set(Reconnect { delay_s: d, ..rc })))
            .collect();
        let custom = workspace.outputs.destinations.iter().filter(|d| d.encoding.is_some()).count();
        pages()
            .child(
                group("Quality")
                    .child(row(
                        "Bitrate",
                        Some("For destinations on Automatic, capped at what each platform takes."),
                        stepper(
                            "stream-bitrate",
                            format!("{:.1} Mbps", kbps as f32 / 1000.0),
                            Command::SetStreamBitrate { kbps: kbps.saturating_sub(500).max(1_000) },
                            Command::SetStreamBitrate { kbps: (kbps + 500).min(51_000) },
                            cx,
                        ),
                    ))
                    .child(note(&format!(
                        "1080p60 wants 6–9 Mbps, 720p 3–5. {}",
                        if custom > 0 {
                            format!(
                                "{custom} destination{} use{} their own settings.",
                                plural(custom),
                                if custom == 1 { "s" } else { "" }
                            )
                        } else {
                            "Click a destination in Outputs to give it its own encoder, bitrate and size.".to_string()
                        }
                    ))),
            )
            .child(
                group("When the connection drops")
                    .child(row(
                        "Reconnect",
                        Some("Keep the stream going through short drop-outs."),
                        chips("reconnect", retries, cx),
                    ))
                    .child(row("Between tries", None, chips("reconnect-delay", delays, cx))),
            )
    }

    fn clips_page(&self, cx: &mut Context<Self>) -> Div {
        let workspace = self.studio.read(cx).workspace().clone();
        let clips = workspace.outputs.clips;
        let on = clips.enabled;
        let seconds: Vec<(String, bool, bool, Command)> = [15, 30, 60, 120, 300]
            .into_iter()
            .map(|s| {
                let settings = ClipSettings { seconds: s, ..clips };
                let label = if s >= 60 { format!("{} min", s / 60) } else { format!("{s} s") };
                (label, clips.seconds == s, true, Command::SetClipSettings { settings })
            })
            .collect();
        let canvases: Vec<(String, bool, bool, Command)> = workspace
            .canvases
            .iter()
            .map(|f| {
                let canvas = (f.id != workspace.primary_canvas().id).then_some(f.id);
                let settings = ClipSettings { canvas, ..clips };
                (f.name.clone(), clips.canvas == canvas, true, Command::SetClipSettings { settings })
            })
            .collect();
        pages().child(
            group("Clips")
                .child(row(
                    "Keep the last moments",
                    Some("The Clip button saves them as a file."),
                    switch("clips-on", on)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_clips(|c| c.enabled = !on, cx))),
                ))
                .child(row("Length", None, chips("clips-length", seconds, cx)))
                .when(workspace.canvases.len() > 1, |g| g.child(row("Canvas", None, chips("clips-canvas", canvases, cx)))),
        )
    }

    fn canvases_page(&self, cx: &mut Context<Self>) -> Div {
        let workspace = self.studio.read(cx).workspace().clone();
        let primary = workspace.primary_canvas().id;
        let busy = lightair_engine::outputs::runtime().any_stream_running()
            || lightair_engine::outputs::runtime().any_recording_running();
        let mut list = group("Canvases");
        for (i, f) in workspace.canvases.iter().enumerate() {
            let id = f.id;
            if let Some(edit) = self.canvas_edit.as_ref().filter(|e| e.id == id) {
                list = list.child(self.canvas_editor(f, edit, busy, cx));
                continue;
            }
            let detail = format!("{}×{} · {} fps", f.width, f.height, f.fps_num / f.fps_den.max(1));
            let actions = div()
                .flex()
                .gap_1()
                .child(
                    Button::new(("edit-canvas", i))
                        .label("Edit")
                        .xsmall()
                        .outline()
                        .on_click(cx.listener(move |this, _, window, cx| this.edit_canvas(id, window, cx))),
                )
                .child(if id == primary {
                    div().px_2().text_size(px(11.)).text_color(tok::text_tertiary()).child("Main").into_any_element()
                } else {
                    Button::new(("remove-canvas", i))
                        .label("Remove")
                        .xsmall()
                        .ghost()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.exec(Command::RemoveCanvas { canvas: id }, cx)),
                        )
                        .into_any_element()
                });
            list = list.child(row(&f.name, Some(&detail), actions));
        }
        let has = |w: u32, h: u32| workspace.canvases.iter().any(|f| f.width == w && f.height == h);
        let mut add = div().flex().flex_wrap().gap_1();
        for (i, f) in [Canvas::vertical(), Canvas::square(), Canvas::portrait()].into_iter().enumerate() {
            if has(f.width, f.height) {
                continue;
            }
            add = add.child(
                Button::new(("settings-add-canvas", i))
                    .label(format!("{} {}×{}", f.name, f.width, f.height))
                    .xsmall()
                    .outline()
                    .on_click(cx.listener(move |this, _, _, cx| this.add_canvas(f.clone(), cx))),
            );
        }
        pages()
            .child(list.child(note(
                "Each canvas is another shape of every scene (vertical for TikTok, say), recorded and streamed on its own.",
            )))
            .child(group("Add a canvas").child(div().px_3().py_2().child(add)))
    }

    /// Editing one canvas: name, size (presets for its shape, or typed), frame rate.
    fn canvas_editor(&self, f: &Canvas, edit: &CanvasEdit, busy: bool, cx: &mut Context<Self>) -> Div {
        let vertical = f.height > f.width;
        let square = f.height == f.width;
        let presets: &[(u32, u32)] = if square {
            &[(1080, 1080), (720, 720), (1440, 1440)]
        } else if vertical {
            &[(1080, 1920), (720, 1280), (1440, 2560), (2160, 3840)]
        } else {
            &[(1920, 1080), (1280, 720), (2560, 1440), (3840, 2160)]
        };
        let (fw, fh) = (edit.width.clone(), edit.height.clone());
        let mut sizes = div().flex().flex_wrap().gap_1();
        for (i, (w, h)) in presets.iter().copied().enumerate() {
            let (fw, fh) = (fw.clone(), fh.clone());
            sizes =
                sizes.child(Button::new(("canvas-preset", i)).label(format!("{w}×{h}")).xsmall().outline().on_click(
                    cx.listener(move |_, _, window, cx| {
                        fw.update(cx, |i, cx| i.set_value(w.to_string(), window, cx));
                        fh.update(cx, |i, cx| i.set_value(h.to_string(), window, cx));
                    }),
                ));
        }
        let mut rates = div().flex().gap_0p5().p_0p5().rounded(px(7.)).bg(tok::bg_panel());
        for (i, r) in [24, 25, 30, 50, 60].into_iter().enumerate() {
            let on = edit.fps == r;
            rates = rates.child(
                div()
                    .id(("canvas-fps", i))
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                    .when(!on, |d| d.text_color(tok::text_secondary()).hover(|d| d.bg(tok::bg_raised())))
                    .child(format!("{r} fps"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = &mut this.canvas_edit {
                            e.fps = r;
                        }
                        cx.notify();
                    })),
            );
        }
        let label = |t: &'static str| div().w(px(90.)).text_size(px(12.)).text_color(tok::text_secondary()).child(t);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(8.))
            .bg(tok::bg_raised())
            .border_1()
            .border_color(tok::border_strong())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label("Name"))
                    .child(div().flex_1().child(Input::new(&edit.name).small())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label("Size"))
                    .child(div().w(px(80.)).child(Input::new(&edit.width).small()))
                    .child("×")
                    .child(div().w(px(80.)).child(Input::new(&edit.height).small()))
                    .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("pixels")),
            )
            .child(div().flex().items_center().gap_2().child(div().w(px(90.))).child(sizes))
            .child(div().flex().items_center().gap_2().child(label("Frame rate")).child(rates))
            .child(note("Objects are reframed to keep their place when the size changes."))
            .when(busy, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(tok::warn())
                        .child("Stop recording and streaming to change a canvas."),
                )
            })
            .when_some(edit.error.clone(), |d, e| d.child(div().text_size(px(11.)).text_color(tok::error()).child(e)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("canvas-apply")
                            .label("Apply")
                            .small()
                            .primary()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_canvas_edit(cx))),
                    )
                    .child(Button::new("canvas-cancel").label("Cancel").small().ghost().on_click(cx.listener(
                        |this, _, _, cx| {
                            this.canvas_edit = None;
                            cx.notify();
                        },
                    ))),
            )
    }

    fn set_remote(&mut self, f: impl FnOnce(&mut crate::prefs::RemoteApi), cx: &mut Context<Self>) {
        self.set_pref(|p| f(&mut p.remote), cx);
        self.sync_remote(cx);
    }

    fn permissions_page(&self, cx: &mut Context<Self>) -> Div {
        use crate::permissions::State;
        // looked at again every second while this page is open (an answer in System Settings
        // shows without coming back)
        if !self.permissions_watch.get() {
            self.permissions_watch.set(true);
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                    let open = this
                        .update(cx, |this, cx| {
                            let open = this.settings_open && this.settings_page == Page::Permissions;
                            if open {
                                cx.notify();
                            } else {
                                this.permissions_watch.set(false);
                            }
                            open
                        })
                        .unwrap_or(false);
                    if !open {
                        break;
                    }
                }
            })
            .detach();
        }
        let mut list = group("What Lightair needs");
        for p in crate::permissions::all() {
            let (text, colour) = match p.state {
                State::Allowed => ("Allowed", tok::ok()),
                State::NotAsked => ("Not asked yet", tok::warn()),
                State::Denied => ("Not allowed", tok::error()),
                State::AskedOnUse => ("Asked on first use", tok::text_tertiary()),
                State::NotNow => ("Checked while the app is open", tok::text_tertiary()),
            };
            let key = p.key;
            let control = div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex().items_center().gap_1().child(div().size(px(7.)).rounded_full().bg(colour)).child(div().text_size(px(12.)).text_color(colour).child(text)))
                .when(p.state != State::Allowed, |d| {
                    let label = if p.state == State::NotAsked { "Allow…" } else { "Open Settings" };
                    d.child(Button::new(SharedString::from(format!("perm-{key}"))).label(label).xsmall().outline().on_click(move |_, _, _| {
                        if label == "Allow…" {
                            crate::permissions::ask(key)
                        } else {
                            crate::permissions::open_settings(key)
                        }
                    }))
                });
            list = list.child(row(p.name, Some(p.why), control));
        }
        pages().child(list).child(
            group("Good to know").child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(tok::text_tertiary())
                    .child(if cfg!(windows) { "Windows has a switch for desktop apps under each of these in Settings › Privacy & security: with it off, a camera shows black and a microphone stays silent." } else { "After allowing screen recording or input monitoring in System Settings, macOS may ask to quit and reopen Lightair before it takes effect." }),
            ),
        )
    }

    fn connections_page(&self, cx: &mut Context<Self>) -> Div {
        let status = self.studio.read(cx).chat.status();
        let mut tips = group("Tips and donations");
        for (service, name, hint) in super::connections::SERVICES {
            let channel = lightair_live::Channel { platform: (*service).into(), channel: "tips".into() };
            let connected = self.studio.read(cx).services.iter().any(|c| c.platform == *service);
            let (text, colour) = match (connected, status.get(&channel)) {
                (false, _) => ("Not connected".to_string(), tok::text_tertiary()),
                (true, Some(lightair_live::Status::Connected)) => ("Connected: tips show in Activity and alerts".into(), tok::ok()),
                (true, Some(lightair_live::Status::Error { message })) => (message.clone(), tok::error()),
                (true, _) => ("Connecting…".into(), tok::warn()),
            };
            let s = *service;
            let control = div()
                .flex()
                .items_center()
                .gap_1()
                .child(Button::new(SharedString::from(format!("connect-{s}"))).label(if connected { "Paste new token" } else { "Paste token" }).xsmall().outline().on_click(
                    cx.listener(move |this, _, _, cx| this.paste_connection(s, cx)),
                ))
                .when(connected, |d| {
                    d.child(Button::new(SharedString::from(format!("disconnect-{s}"))).label("Disconnect").xsmall().ghost().on_click(cx.listener(
                        move |this, _, _, cx| this.disconnect(s, cx),
                    )))
                });
            tips = tips.child(row(name, Some(&format!("{text}. {hint}")), control).text_color(colour));
        }
        // Ko-fi: its own stream alerts page, read on this computer
        let kofi_set = lightair_live::donations::has_token("kofi");
        let kofi_channel = lightair_live::Channel { platform: "kofi".into(), channel: "tips".into() };
        let kofi_status = match status.get(&kofi_channel) {
            Some(lightair_live::Status::Connected) => "Connected: Ko-fi tips, memberships and shop orders show in Activity and alerts".to_string(),
            Some(lightair_live::Status::Error { message }) => message.clone(),
            _ if !kofi_set => "Not connected. Its link: ko-fi.com › Stream Alerts › Copy the link (the one for OBS); no server or webhook needed.".into(),
            _ => "Connecting…".into(),
        };
        tips = tips.child(row(
            "Ko-fi",
            Some(&kofi_status),
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(Button::new("kofi-link").label(if kofi_set { "Paste new link" } else { "Paste stream alerts link" }).xsmall().outline().on_click(cx.listener(|this, _, _, cx| this.paste_kofi_link(cx))))
                .when(kofi_set, |d| d.child(Button::new("kofi-forget").label("Disconnect").xsmall().ghost().on_click(cx.listener(|this, _, _, cx| this.disconnect("kofi", cx))))),
        ));
        let heart = self.heart_rate.is_some();
        let bpm = self.studio.read(cx).widgets.as_ref().map(|w| w.data("heart_rate")).filter(|v| !v.is_null());
        let devices = group("Devices").child(row(
            "Heart rate (Pulsoid)",
            Some(&match (heart, bpm) {
                (true, Some(b)) => format!("Connected: {b} BPM. Add Widgets › Heart rate; automations read {{{{ vars.heart_rate }}}}."),
                (true, None) => "Connected, waiting for a reading (is the sensor on?)".to_string(),
                _ => "Not connected. Its token: pulsoid.net › Keys › Create new key (heart rate, read).".to_string(),
            }),
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(Button::new("pulsoid-paste").label(if heart { "Paste new token" } else { "Paste token" }).xsmall().outline().on_click(cx.listener(|this, _, _, cx| this.paste_pulsoid(cx))))
                .when(heart, |d| d.child(Button::new("pulsoid-forget").label("Disconnect").xsmall().ghost().on_click(cx.listener(|this, _, _, cx| this.forget_pulsoid(cx))))),
        ));
        let hue = lightair_live::integrations::hue_bridge();
        let pairing = self.hue_note.as_deref().is_some_and(|n| n.ends_with('…'));
        let devices = devices.child(row(
            "Philips Hue",
            Some(&match (&self.hue_note, &hue) {
                (Some(n), _) => n.clone(),
                (None, Some((a, _))) => format!("Paired with the bridge at {a}. Automations: Add step › Philips Hue light."),
                _ => "Not paired. Pair finds the bridge on this network, then asks you to press its button.".to_string(),
            }),
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(Button::new("hue-pair").label(if hue.is_some() { "Pair again" } else { "Pair" }).xsmall().outline().disabled(pairing).on_click(cx.listener(|this, _, _, cx| this.pair_hue(None, cx))))
                .child(Button::new("hue-paste").label("Paste address").xsmall().ghost().disabled(pairing).on_click(cx.listener(|this, _, _, cx| this.paste_hue_address(cx))))
                .when(hue.is_some(), |d| d.child(Button::new("hue-forget").label("Forget").xsmall().ghost().on_click(cx.listener(|this, _, _, cx| this.forget_hue(cx))))),
        ));
        let mut official = group("Official sign-in");
        for (provider, name, does) in [
            ("youtube", "YouTube", "title, description and category; chat; bans and timeouts; your Private and Unlisted streams"),
            ("twitch", "Twitch", "title and category; chat; delete, timeouts and bans"),
            ("kick", "Kick", "title and category; chat; timeouts and bans"),
            ("x", "X", "going live with your title when you stream, ending it after; chat (X's approval needed)"),
            ("spotify", "Spotify", "what's playing on any of your devices, for Now playing"),
        ] {
            let account = lightair_live::oauth::account(provider);
            let available = lightair_live::oauth::available(provider);
            let busy = self.oauth_notes.get(provider).is_some_and(|n| n.ends_with('…') || n.contains("code"));
            let status = match (self.oauth_notes.get(provider), &account, available) {
                (Some(n), _, _) => n.clone(),
                (None, Some((_, who)), _) => format!("Signed in as {who}: Lightair uses {name}'s own API for {does}."),
                (None, None, true) => format!("Sign in, as with OBS, and Lightair uses {name}'s own API for {does}."),
                (None, None, false) => format!("No {name} app in this copy of Lightair: register your own (docs/official-apis.md) and choose Use your own app; meanwhile the signed-in pages are used."),
            };
            official = official.child(row(
                name,
                Some(&status),
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(account.is_none(), |d| {
                        d.child(Button::new(SharedString::from(format!("oauth-in-{provider}"))).label("Sign in").xsmall().outline().disabled(!available || busy).on_click(cx.listener(
                            move |this, _, _, cx| this.official_sign_in(provider, cx),
                        )))
                    })
                    // the person's own registered app (docs/official-apis.md): its id, and secret
                    .when(account.is_none(), |d| {
                        let own = lightair_live::oauth::has_own_app(provider);
                        d.child(
                            Button::new(SharedString::from(format!("oauth-app-{provider}")))
                                .label(if own { "Your app ✓" } else { "Use your own app" })
                                .xsmall()
                                .ghost()
                                .dropdown_menu(move |menu, _, _| {
                                    let menu = menu
                                        .item(PopupMenuItem::new("Paste its client ID").on_click(move |_, _, cx| own_app_paste(provider, "id", cx)))
                                        .when(lightair_live::oauth::wants_secret(provider) || provider == "x", |m| {
                                            m.item(PopupMenuItem::new("Paste its client secret").on_click(move |_, _, cx| own_app_paste(provider, "secret", cx)))
                                        });
                                    if own {
                                        menu.separator().item(PopupMenuItem::new("Forget your app").on_click(move |_, _, _| {
                                            lightair_live::oauth::set_own_app(provider, "id", "");
                                            lightair_live::oauth::set_own_app(provider, "secret", "");
                                        }))
                                    } else {
                                        menu
                                    }
                                }),
                        )
                    })
                    .when(account.is_some(), |d| {
                        d.child(Button::new(SharedString::from(format!("oauth-out-{provider}"))).label("Sign out").xsmall().ghost().on_click(cx.listener(
                            move |this, _, _, cx| this.official_sign_out(provider, cx),
                        )))
                    }),
            ));
        }
        pages().child(official).child(tips).child(devices).child(
            group("How it works").child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(tok::text_tertiary())
                    .child("Copy the token from the service's own dashboard, then choose Paste token: Lightair keeps it in the system's credential store and never shows it. Tips arrive like any other event, in Activity, alert boxes, goals and automations."),
            ),
        )
    }

    fn remote_page(&self, cx: &mut Context<Self>) -> Div {
        let r = self.prefs.remote;
        let server = self.remote.running();
        let port = server.map_or(r.port, |s| s.addr().port());
        let host = if r.lan { lightair_platform::net::local_ip().unwrap_or_else(|| "this-mac".into()) } else { "127.0.0.1".into() };
        let asks = r.require_password || r.lan;
        let status = match (&self.remote.error, server) {
            (Some(e), _) => e.clone(),
            (None, Some(s)) => {
                let n = s.clients().len();
                let who = match (r.lan, cfg!(windows)) {
                    (true, true) => "this PC and your network",
                    (false, true) => "this PC only",
                    (true, false) => "this Mac and your network",
                    (false, false) => "this Mac only",
                };
                format!("On, for {who} · {n} client{} connected", plural(n))
            }
            (None, None) => "Off".into(),
        };
        let copy = |id: &'static str, text: String| {
            ui::icon_button(id, "copy", tok::text_secondary())
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
                .into_any_element()
        };
        let mut page = pages().child(
            group("Remote API")
                .child(row(
                    "Let other apps control Lightair",
                    Some(status.as_str()),
                    switch("remote-on", r.enabled)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_remote(|p| p.enabled = !r.enabled, cx))),
                ))
                .child(row(
                    "Port",
                    Some("4455 is where OBS tools look."),
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(Button::new("remote-port-down").icon(ui::icon("minus")).xsmall().ghost().on_click(cx.listener(
                            move |this, _, _, cx| this.set_remote(|p| p.port = p.port.saturating_sub(1).max(1024), cx),
                        )))
                        .child(div().w(px(56.)).text_center().text_size(px(12.)).font_family(crate::ui::MONO).child(r.port.to_string()))
                        .child(Button::new("remote-port-up").icon(ui::icon("plus")).xsmall().ghost().on_click(cx.listener(
                            move |this, _, _, cx| this.set_remote(|p| p.port = p.port.saturating_add(1), cx),
                        ))),
                ))
                .child(row(
                    "Other devices on your network",
                    Some("Phones, tablets, Stream Decks on other computers. Always asks for the password."),
                    switch("remote-lan", r.lan)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_remote(|p| p.lan = !r.lan, cx))),
                ))
                .child(row(
                    "Ask for the password",
                    Some(if r.lan { "Always, while other devices may connect." } else if cfg!(windows) { "Apps on this PC must send it too." } else { "Apps on this Mac must send it too." }),
                    switch("remote-password-on", asks).on_click(cx.listener(move |this, _, _, cx| {
                        if !r.lan {
                            this.set_remote(|p| p.require_password = !r.require_password, cx)
                        }
                    })),
                )),
        );
        if asks {
            if self.remote.password.is_none() {
                let this = cx.entity().downgrade();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |this, cx| this.read_remote_password(false, cx));
                });
            }
            let password = self.remote.password.clone().flatten().unwrap_or_default();
            let shown = if self.remote.show_password { password.clone() } else { "•".repeat(12) };
            let show = self.remote.show_password;
            page = page.child(
                group("Password").child(row(
                    &shown,
                    Some(if cfg!(windows) { "Kept in Windows Credential Manager." } else { "Kept in the Keychain." }),
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(ui::icon_button("remote-show", if show { "eye-off" } else { "eye" }, tok::text_secondary()).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.remote.show_password = !show;
                                cx.notify();
                            }),
                        ))
                        .child(copy("remote-copy-password", password))
                        .child(Button::new("remote-new-password").label("New").xsmall().ghost().on_click(cx.listener(
                            |this, _, _, cx| this.read_remote_password(true, cx),
                        ))),
                )),
            );
        }
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "lightair".into());
        let agent = format!("claude mcp add lightair -- \"{exe}\" mcp");
        let obs = format!("ws://{host}:{port}");
        let mcp_url = format!("http://{host}:{port}/mcp");
        let rpc = format!("ws://{host}:{port}/rpc");
        // Elgato's app, and the plugin packed into Lightair (macOS: in its Resources; Windows:
        // beside lightair.exe)
        let deck_app = if cfg!(windows) {
            std::env::var_os("ProgramFiles")
                .is_some_and(|p| std::path::Path::new(&p).join(r"Elgato\StreamDeck\StreamDeck.exe").exists())
        } else {
            std::path::Path::new("/Applications/Elgato Stream Deck.app").exists()
        };
        let deck_plugin = std::env::current_exe()
            .ok()
            .and_then(|e| {
                if cfg!(windows) {
                    Some(e.parent()?.join("Lightair.streamDeckPlugin"))
                } else {
                    Some(e.parent()?.parent()?.join("Resources/Lightair.streamDeckPlugin"))
                }
            })
            .filter(|p| p.exists());
        let deck_note = match (deck_app, &deck_plugin) {
            (false, _) => "Lightair's plugin: keys for scenes, going live, recording, clips, automations and mute, lit with what's on. Install Elgato's Stream Deck app first.".to_string(),
            (true, None) => "This copy of Lightair has no plugin to install (it's packed into the app when built).".to_string(),
            (true, Some(_)) => format!("Keys for scenes, going live, recording, clips, automations and mute, lit with what's on. Its key settings take this port ({port}){}.", if asks { " and the password" } else { "" }),
        };
        page.child(
            group("Connect")
                .child(row(
                    "Stream Deck",
                    Some(&deck_note),
                    Button::new("remote-stream-deck").label("Install plugin").xsmall().outline().disabled(!deck_app || deck_plugin.is_none()).on_click(move |_, _, _| {
                        if let Some(p) = &deck_plugin {
                            // Stream Deck installs a .streamDeckPlugin it's given (Explorer
                            // opens a file with the app it belongs to)
                            let opener = if cfg!(windows) { "explorer.exe" } else { "/usr/bin/open" };
                            let _ = std::process::Command::new(opener).arg(p).spawn();
                        }
                    }),
                ))
                .child(row("AI agents (MCP)", Some(&agent), copy("remote-copy-agent", agent.clone())))
                .child(row("MCP over HTTP", Some(&format!("{mcp_url} · Authorization: Bearer <password>")), copy("remote-copy-mcp", mcp_url.clone())))
                .child(row("OBS tools (obs-websocket 5)", Some(&format!("{obs} · connect as to OBS")), copy("remote-copy-obs", obs.clone())))
                .child(row("Lightair JSON-RPC", Some(&rpc), copy("remote-copy-rpc", rpc.clone()))),
        )
        .child(note("Agents and tools change the workspace like you do: every change can be undone, named after who made it."))
    }

    fn about_page(&self, cx: &mut Context<Self>) -> Div {
        let studio = self.studio.read(cx);
        let libobs = studio.engine().version.clone();
        let folders = [
            ("Workspace", studio.workspace_dir()),
            ("Recordings", Some(studio.recording_folder())),
            ("Preferences", crate::prefs::path().parent().map(|p| p.to_path_buf())),
        ];
        let mut where_ = group("Where things are");
        for (i, (label, dir)) in folders.into_iter().enumerate() {
            let shown = dir.as_ref().map(|d| d.to_string_lossy().into_owned());
            where_ = where_.child(row(label, shown.as_deref(), reveal_button(("reveal", i), dir)));
        }
        pages()
            .child(group("Lightair").child(row("Version", None, value(env!("CARGO_PKG_VERSION")))).child(row(
                "libobs",
                Some("The OBS engine Lightair runs on."),
                value(&libobs),
            )))
            .child(where_)
            .child(self.updates_group(cx))
    }

    /// Settings › About › Updates: the newest release, checked and installed from here.
    fn updates_group(&self, cx: &mut Context<Self>) -> Div {
        use crate::updater::{self, State};
        if !updater::enabled() {
            return group("Updates").child(row("This build doesn't update itself", Some("Development builds don't; releases from GitHub do."), div()));
        }
        let auto = self.prefs.check_updates;
        let state = updater::state();
        let (text, action): (String, Option<(&'static str, bool)>) = match &state {
            State::Idle => ("Not checked yet".into(), Some(("Check now", false))),
            State::Checking => ("Looking…".into(), None),
            State::UpToDate => (format!("Lightair {} is the newest", env!("CARGO_PKG_VERSION")), Some(("Check now", false))),
            State::Available(r) => (format!("Lightair {} is out{}", r.version, if r.notes.is_empty() { String::new() } else { format!(": {}", r.notes.lines().next().unwrap_or("")) }), Some(("Install and restart", true))),
            State::Downloading(v) => (format!("Downloading Lightair {v}…"), None),
            State::Installing => ("Restarting into the new version…".into(), None),
            State::Failed(e) => (format!("Couldn't update: {e}"), Some(("Check now", false))),
        };
        // while it works, look again shortly
        if matches!(state, State::Checking | State::Downloading(_)) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        let page = match &state {
            State::Available(r) if !r.page.is_empty() => Some(r.page.clone()),
            _ => None,
        };
        group("Updates")
            .child(row(
                "Lightair",
                Some(&text),
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when_some(page, |d, url| {
                        d.child(Button::new("update-notes").label("What's new").xsmall().ghost().on_click(move |_, _, cx| cx.open_url(&url)))
                    })
                    .when_some(action, |d, (label, install)| {
                        d.child(Button::new("update-action").label(label).xsmall().outline().on_click(cx.listener(move |this, _, _, cx| {
                            if !install {
                                updater::check();
                                cx.notify();
                                return;
                            }
                            // download and check in the background; quit once it's staged
                            cx.spawn(async move |this, cx| {
                                let r = cx.background_executor().spawn(async { updater::install() }).await;
                                let _ = this.update(cx, |_, cx| {
                                    if r.is_ok() {
                                        cx.quit();
                                    } else {
                                        cx.notify();
                                    }
                                });
                            })
                            .detach();
                            let _ = this;
                            cx.notify();
                        })))
                    }),
            ))
            .child(row(
                "Check for updates automatically",
                Some("At launch and every few hours, from Lightair's releases on GitHub."),
                switch("update-auto", auto).on_click(cx.listener(move |this, _, _, cx| this.set_pref(|p| p.check_updates = !auto, cx))),
            ))
    }
}

// ---------------------------------------------------------------- building blocks

fn pages() -> Div {
    div().flex().flex_col().gap_5().pt_1()
}

/// A titled card of rows.
fn group(title: &str) -> Div {
    div().flex().flex_col().gap_1p5().child(
        div()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(tok::text_tertiary())
            .child(title.to_uppercase()),
    )
}

/// A setting: its name (and what it does) on the left, its control on the right.
fn row(label: &str, detail: Option<&str>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap_4()
        .min_h(px(44.))
        .px_3()
        .py_2()
        .rounded(px(8.))
        .bg(tok::bg_raised())
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(13.)).child(label.to_string()))
                .children(detail.map(|d| {
                    div()
                        .text_size(px(11.))
                        .text_color(tok::text_tertiary())
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(d.to_string())
                })),
        )
        .child(div().flex_none().child(control))
}

fn note(text: &str) -> Div {
    div().px_1().text_size(px(11.)).text_color(tok::text_tertiary()).child(text.to_string())
}

fn value(text: &str) -> Div {
    div().text_size(px(12.)).font_family(crate::ui::MONO).text_color(tok::text_secondary()).child(text.to_string())
}

fn pref_chip(id: impl Into<ElementId>, label: String, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded(px(5.))
        .text_size(px(11.))
        .cursor_pointer()
        .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
        .when(!on, |d| d.text_color(tok::text_secondary()).hover(|d| d.bg(tok::bg_raised())))
        .child(label)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub(crate) fn switch(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .w(px(34.))
        .h(px(20.))
        .rounded_full()
        .cursor_pointer()
        .bg(if on { tok::ok() } else { tok::border_strong() })
        .child(div().size(px(16.)).m(px(2.)).rounded_full().bg(rgb(0xFFFFFF)).when(on, |d| d.ml(px(16.))))
}

/// A row of choices: (label, chosen, available, what choosing runs).
fn chips(id: &'static str, options: Vec<(String, bool, bool, Command)>, cx: &mut Context<StudioWindow>) -> AnyElement {
    let mut row = div().flex().flex_wrap().justify_end().gap_0p5().p_0p5().rounded(px(7.)).bg(tok::bg_panel());
    for (i, (label, on, enabled, cmd)) in options.into_iter().enumerate() {
        row = row.child(
            div()
                .id((id, i))
                .px_2()
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(11.))
                .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                .when(!on && enabled, |d| {
                    d.cursor_pointer().text_color(tok::text_secondary()).hover(|d| d.bg(tok::bg_raised()))
                })
                .when(!enabled, |d| d.text_color(tok::text_tertiary()).opacity(0.5))
                .child(label)
                .when(enabled && !on, |d| d.on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx)))),
        );
    }
    row.into_any_element()
}

fn stepper(
    id: &'static str,
    value: String,
    minus: Command,
    plus: Command,
    cx: &mut Context<StudioWindow>,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            Button::new((id, 0usize))
                .icon(ui::icon("minus"))
                .xsmall()
                .ghost()
                .on_click(cx.listener(move |this, _, _, cx| this.exec(minus.clone(), cx))),
        )
        .child(div().w(px(80.)).text_center().text_size(px(12.)).font_family(crate::ui::MONO).child(value))
        .child(
            Button::new((id, 1usize))
                .icon(ui::icon("plus"))
                .xsmall()
                .ghost()
                .on_click(cx.listener(move |this, _, _, cx| this.exec(plus.clone(), cx))),
        )
        .into_any_element()
}

/// Canvas size, 1080p, 720p, 480p.
pub(crate) fn height_choices(
    current: Option<u32>,
    make: impl Fn(Option<u32>) -> Command,
) -> Vec<(String, bool, bool, Command)> {
    [None, Some(1080), Some(720), Some(480)]
        .into_iter()
        .map(|h| (h.map_or("Canvas".to_string(), |h| format!("{h}p")), current == h, true, make(h)))
        .collect()
}

fn reveal_button(id: impl Into<ElementId>, dir: Option<std::path::PathBuf>) -> AnyElement {
    match dir {
        Some(dir) => ui::icon_button(id, "folder-open", tok::text_secondary())
            .on_click(move |_, _, cx| {
                let _ = std::fs::create_dir_all(&dir);
                cx.reveal_path(&dir)
            })
            .into_any_element(),
        None => div().into_any_element(),
    }
}

/// Copies the person's own app's id or secret from the clipboard (never shown again).
fn own_app_paste(provider: &str, part: &str, cx: &mut App) {
    let text = cx.read_from_clipboard().and_then(|c| c.text()).map(|t| t.trim().to_string()).unwrap_or_default();
    if !text.is_empty() && !text.contains(char::is_whitespace) {
        lightair_live::oauth::set_own_app(provider, part, &text);
    }
}
