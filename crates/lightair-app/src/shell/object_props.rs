//! Inspector sections for what a object shows (docs/07-scenes-objects-canvas.md): media
//! transport, text content and style, colour. Content edits are source settings changes, so
//! they're undoable and apply wherever the source appears.

use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::input::{Editor, EditorState, Input as TextInput, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::{Command, Id, Source};
use lightair_engine::media::{MediaAction, MediaState, MediaStatus};
use lightair_engine::timers::{TIMER_KIND, TimerAction, TimerStatus};
use serde_json::{Value, json};

use super::StudioWindow;
use crate::ui::tok;

/// The text field for the selected text object.
pub(super) struct TextEdit {
    source: Id,
    state: Entity<InputState>,
    _events: Subscription,
}

/// The custom CSS editor for the selected web page.
pub(super) struct CssEdit {
    source: Id,
    state: Entity<EditorState>,
    _events: Subscription,
}

/// One-click additions to a web page's custom CSS.
const CSS_SNIPPETS: &[(&str, &str, &str)] = &[
    ("Transparent background", "Shows the scene through the page", "html, body { background: transparent !important; }"),
    ("Hide scrollbars", "No scrollbars, the page still scrolls", "html, body { overflow: hidden !important; }\n::-webkit-scrollbar { display: none; }"),
];

/// Inspector state for content sections.
#[derive(Default)]
pub(super) struct ObjectProps {
    text: Option<TextEdit>,
    media: Option<(Id, MediaStatus, Instant)>,
    /// The effect whose controls are shown.
    pub(super) open_effect: Option<Id>,
    timer: Option<(Id, TimerStatus, Instant)>,
    /// The address field for the selected web page.
    web: Option<TextEdit>,
    /// Its custom CSS.
    web_css: Option<CssEdit>,
    /// The selected widget's settings.
    pub(super) widget: Option<super::widget_props::WidgetEdit>,
}

pub(super) fn is_text(kind: &str) -> bool {
    kind.starts_with("text_ft2_source") || kind.starts_with("text_gdiplus")
}
pub(super) fn is_colour(kind: &str) -> bool {
    kind.starts_with("color_source")
}
pub(super) fn is_web(kind: &str) -> bool {
    kind == lightair_engine::web::KIND
}
/// An address as typed, made loadable: without a scheme it gets https://.
pub(super) fn web_address(typed: &str) -> String {
    let typed = typed.trim();
    if typed.is_empty() || typed.contains("://") || typed.starts_with("data:") || typed.starts_with("about:") {
        typed.to_string()
    } else {
        format!("https://{typed}")
    }
}
pub(super) fn is_media(kind: &str) -> bool {
    kind == "ffmpeg_source" || kind == "vlc_source"
}

/// libobs colours are 0xAABBGGRR.
pub(super) fn abgr(rgb: u32) -> u32 {
    0xFF00_0000 | ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}
pub(super) fn rgb_of(abgr: u32) -> u32 {
    ((abgr & 0xFF) << 16) | (abgr & 0xFF00) | ((abgr >> 16) & 0xFF)
}

/// Swatches offered for text and colour objects (RGB).
pub(super) const PALETTE: &[u32] = &[0xFFFFFF, 0x0B0B0F, 0xFF4A3B, 0xFFB020, 0x2BD67B, 0x3B82F6, 0x8B5CF6, 0xF472B6];

/// `settings` with `key` set (a merged copy: the reconciler updates libobs from the whole object).
fn with(settings: &Value, key: &str, value: Value) -> Value {
    let mut s = settings.clone();
    if !s.is_object() {
        s = json!({});
    }
    s[key] = value;
    s
}

fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

impl StudioWindow {
    /// Keeps the text field and the media status in step with the selection. Called from render.
    pub(super) fn sync_object_props(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let source = studio
            .selected_scene
            .and_then(|s| workspace.scene(s))
            .and_then(|s| studio.selected_object.and_then(|l| s.object(l)))
            .and_then(|l| l.source.and_then(|i| workspace.sources.get(&i)))
            .cloned();
        let engine = studio.engine().clone();

        // Text field: one per selected text source; refreshed when the text changes elsewhere
        // (undo) and the field isn't being typed in.
        match source.as_ref().filter(|i| is_text(&i.kind)) {
            Some(i) => {
                let text = i.settings["text"].as_str().unwrap_or("").to_string();
                let same = self.props.text.as_ref().is_some_and(|t| t.source == i.id);
                if !same {
                    let state = cx.new(|cx| InputState::new(window, cx));
                    let id = i.id;
                    let events = cx.subscribe_in(&state, window, move |this, state, ev: &InputEvent, _, cx| {
                        if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                            let value = state.read(cx).value().to_string();
                            this.set_text(id, value, cx);
                        }
                    });
                    self.props.text = Some(TextEdit { source: i.id, state, _events: events });
                }
                let t = self.props.text.as_ref().unwrap();
                let focused = t.state.read(cx).focus_handle(cx).is_focused(window);
                if !focused && t.state.read(cx).value() != text.as_str() {
                    t.state.update(cx, |s, cx| s.set_value(text, window, cx));
                }
            }
            None => self.props.text = None,
        }

        // Web page address: the same, applied on Enter or when the field loses focus.
        match source.as_ref().filter(|i| is_web(&i.kind)) {
            Some(i) => {
                let url = i.settings["url"].as_str().unwrap_or("").to_string();
                if !self.props.web.as_ref().is_some_and(|t| t.source == i.id) {
                    let state = cx.new(|cx| InputState::new(window, cx).placeholder("https://…"));
                    let id = i.id;
                    let events = cx.subscribe_in(&state, window, move |this, state, ev: &InputEvent, _, cx| {
                        if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                            let value = state.read(cx).value().to_string();
                            this.set_web_url(id, value, cx);
                        }
                    });
                    self.props.web = Some(TextEdit { source: i.id, state, _events: events });
                }
                let t = self.props.web.as_ref().unwrap();
                let focused = t.state.read(cx).focus_handle(cx).is_focused(window);
                if !focused && t.state.read(cx).value() != url.as_str() {
                    t.state.update(cx, |s, cx| s.set_value(url, window, cx));
                }
            }
            None => self.props.web = None,
        }

        // Its custom CSS: the page restyles as it's typed; kept (undoably) when the field is
        // left, or when the Inspector moves on without it being left (what was typed stays).
        let shown = source.as_ref().filter(|i| is_web(&i.kind)).map(|i| i.id);
        if let Some(old) = self.props.web_css.as_ref().filter(|t| Some(t.source) != shown) {
            let (id, css) = (old.source, old.state.read(cx).value().to_string());
            self.set_web_css(id, css, cx);
        }
        match source.as_ref().filter(|i| is_web(&i.kind)) {
            Some(i) => {
                let css = i.settings["css"].as_str().unwrap_or("").to_string();
                if !self.props.web_css.as_ref().is_some_and(|t| t.source == i.id) {
                    let state = cx.new(|cx| {
                        EditorState::new(window, cx)
                            .language("css")
                            .line_number(false)
                            .placeholder("Type CSS to restyle this page")
                            .default_value(css.clone())
                    });
                    let id = i.id;
                    let events = cx.subscribe_in(&state, window, move |this, state, ev: &InputEvent, _, cx| match ev {
                        InputEvent::Change => {
                            let css = state.read(cx).value().to_string();
                            this.studio.read(cx).engine().post(move |s| s.reconciler.web_restyle(id, css));
                        }
                        InputEvent::Blur => {
                            let css = state.read(cx).value().to_string();
                            this.set_web_css(id, css, cx);
                        }
                        _ => {}
                    });
                    self.props.web_css = Some(CssEdit { source: i.id, state, _events: events });
                }
                let t = self.props.web_css.as_ref().unwrap();
                let focused = t.state.read(cx).focus_handle(cx).is_focused(window);
                if !focused && t.state.read(cx).value() != css.as_str() {
                    t.state.update(cx, |s, cx| s.set_value(css, window, cx));
                }
            }
            None => self.props.web_css = None,
        }

        // colour controls: the text's, the colour object's (widgets' are synced with them)
        if let Some(i) = source.as_ref().filter(|i| is_text(&i.kind) || i.kind == TIMER_KIND || is_colour(&i.kind)) {
            let (key, keys): (&str, &'static [&'static str]) = if is_colour(&i.kind) { ("object-colour", &["color"]) } else { ("text-colour", &["color1", "color2"]) };
            let current = super::colour_field::of_abgr(i.settings[keys[0]].as_u64().unwrap_or(0xFFFF_FFFF) as u32);
            let id = i.id;
            let apply: super::colour_field::Apply = std::rc::Rc::new(move |this: &mut StudioWindow, c, cx| {
                let workspace = this.studio.read(cx).workspace().clone();
                let Some(src) = workspace.sources.get(&id) else { return };
                let v = json!(super::colour_field::abgr(c));
                if keys.iter().all(|k| src.settings[*k] == v) {
                    return;
                }
                let mut s = src.settings.clone();
                for k in keys {
                    s = with(&s, k, v.clone());
                }
                this.exec(Command::SetSourceSettings { source: id, settings: s }, cx);
            });
            self.sync_colour(key, current, apply, window, cx);
        }
        self.sync_widget_props(source.as_deref(), window, cx);
        self.keep_synced_colours();

        // Timer status, polled the same way.
        match source.as_ref().filter(|i| i.kind == TIMER_KIND) {
            Some(i) => {
                let stale = self
                    .props
                    .timer
                    .as_ref()
                    .is_none_or(|(id, _, at)| *id != i.id || at.elapsed() > Duration::from_millis(200));
                if stale {
                    let id = i.id;
                    self.props.timer =
                        engine.call(move |s| s.reconciler.timer_status(id)).map(|status| (id, status, Instant::now()));
                }
            }
            None => self.props.timer = None,
        }

        // Media status, polled a few times a second while a media object is selected.
        match source.as_ref().filter(|i| is_media(&i.kind)) {
            Some(i) => {
                let stale = self
                    .props
                    .media
                    .as_ref()
                    .is_none_or(|(id, _, at)| *id != i.id || at.elapsed() > Duration::from_millis(200));
                if stale {
                    let id = i.id;
                    self.props.media =
                        engine.call(move |s| s.reconciler.media_status(id)).map(|status| (id, status, Instant::now()));
                }
            }
            None => self.props.media = None,
        }
    }

    pub(super) fn set_text(&mut self, source: Id, text: String, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(i) = workspace.sources.get(&source) else { return };
        if i.settings["text"].as_str() == Some(text.as_str()) {
            return;
        }
        self.exec(Command::SetSourceSettings { source, settings: with(&i.settings, "text", json!(text)) }, cx);
    }

    fn set_web_url(&mut self, source: Id, typed: String, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(i) = workspace.sources.get(&source) else { return };
        let url = web_address(&typed);
        if url.is_empty() || i.settings["url"].as_str() == Some(url.as_str()) {
            return;
        }
        self.exec(Command::SetSourceSettings { source, settings: with(&i.settings, "url", json!(url)) }, cx);
    }

    fn set_web_css(&mut self, source: Id, css: String, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(i) = workspace.sources.get(&source) else { return };
        if i.settings["css"].as_str().unwrap_or("") == css {
            return;
        }
        self.exec(Command::SetSourceSettings { source, settings: with(&i.settings, "css", json!(css)) }, cx);
    }

    fn media(&self, source: Id, action: MediaAction, cx: &mut Context<Self>) {
        self.studio.read(cx).engine().post(move |s| s.reconciler.media_action(source, action));
    }

    /// The content section for a object's source, if its kind has one.
    pub(super) fn content_section(&self, source: &Source, cx: &mut Context<Self>) -> Option<AnyElement> {
        if source.kind == TIMER_KIND {
            Some(div().child(self.timer_section(source, cx)).child(self.text_section(source, cx)).into_any_element())
        } else if is_text(&source.kind) {
            Some(self.text_section(source, cx).into_any_element())
        } else if is_colour(&source.kind) {
            Some(self.colour_section(source, cx).into_any_element())
        } else if is_media(&source.kind) {
            Some(self.media_section(source, cx).into_any_element())
        } else if is_web(&source.kind) && source.settings.get("widget").is_some() {
            self.widget_section(source, cx)
        } else if is_web(&source.kind) {
            Some(self.web_section(source, cx).into_any_element())
        } else {
            None
        }
    }

    fn text_section(&self, source: &Source, cx: &mut Context<Self>) -> impl IntoElement {
        // the system's own faces (what every Mac, or every Windows PC, has)
        #[cfg(not(windows))]
        const FONTS: &[&str] = &[
            "Helvetica Neue",
            "Avenir Next",
            "Futura",
            "Gill Sans",
            "Georgia",
            "Didot",
            "Menlo",
            "Impact",
            "Marker Felt",
        ];
        #[cfg(windows)]
        const FONTS: &[&str] = &[
            "Segoe UI",
            "Arial",
            "Bahnschrift",
            "Calibri",
            "Georgia",
            "Trebuchet MS",
            "Consolas",
            "Impact",
            "Segoe Print",
        ];
        let (id, settings) = (source.id, source.settings.clone());
        let font = if settings["font"].is_object() {
            settings["font"].clone()
        } else {
            json!({"face": crate::ui::TEXT_FACE, "size": 128, "flags": 0})
        };
        let size = font["size"].as_i64().unwrap_or(128);
        let face = font["face"].as_str().unwrap_or(crate::ui::TEXT_FACE).to_string();
        let flags = font["flags"].as_i64().unwrap_or(0);
        let set_font = {
            let (settings, font) = (settings.clone(), font.clone());
            move |key: &str, value: Value| {
                let mut f = font.clone();
                f[key] = value;
                Command::SetSourceSettings { source: id, settings: with(&settings, "font", f) }
            }
        };
        let next_face = FONTS[(FONTS.iter().position(|f| *f == face).map_or(0, |i| i + 1)) % FONTS.len()];
        let cycle_face = set_font("face", json!(next_face));
        let (smaller, bigger) =
            (set_font("size", json!((size - 8).clamp(8, 1024))), set_font("size", json!((size + 8).clamp(8, 1024))));

        // Style toggles: bold and italic are font flags (1, 2); outline and shadow are settings.
        let mut toggles = div().flex().gap_0p5().p_0p5().rounded(px(6.)).bg(tok::bg_raised());
        let toggle = |i: usize, label: &'static str, on: bool, cmd: Command, cx: &mut Context<Self>| {
            div()
                .id(("text-toggle", i))
                .flex_1()
                .text_center()
                .py_1()
                .rounded(px(4.))
                .text_size(px(11.))
                .cursor_pointer()
                .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                .when(!on, |d| d.text_color(tok::text_tertiary()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx)))
        };
        for (i, (label, bit)) in [("Bold", 1i64), ("Italic", 2)].into_iter().enumerate() {
            toggles = toggles.child(toggle(i, label, flags & bit != 0, set_font("flags", json!(flags ^ bit)), cx));
        }
        for (i, (label, key)) in [("Outline", "outline"), ("Shadow", "drop_shadow")].into_iter().enumerate() {
            let on = settings[key].as_bool().unwrap_or(false);
            let cmd = Command::SetSourceSettings { source: id, settings: with(&settings, key, json!(!on)) };
            toggles = toggles.child(toggle(2 + i, label, on, cmd, cx));
        }

        let font_chip = div()
            .id("text-font")
            .flex()
            .items_center()
            .gap_1()
            .h(px(30.))
            .px_2()
            .rounded(px(6.))
            .bg(tok::bg_raised())
            .cursor_pointer()
            .hover(|d| d.bg(tok::bg_selected()))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_size(px(12.)).child(face))
            .child(crate::ui::icon("chevrons-up-down").size(px(12.)).text_color(tok::text_tertiary()))
            .on_click(cx.listener(move |this, _, _, cx| this.exec(cycle_face.clone(), cx)));
        self.section("Text", None, cx)
            .when_some(self.props.text.as_ref().filter(|t| t.source == source.id), |d, t| {
                d.child(TextInput::new(&t.state).small())
            })
            .child(div().grid().grid_cols(2).gap_1().child(font_chip).child(self.field(
                "text-size",
                "Aa",
                size.to_string(),
                smaller,
                bigger,
                cx,
            )))
            .child(toggles)
            .child(self.colour_field("text-colour", cx))
    }

    fn timer_section(&self, source: &Source, cx: &mut Context<Self>) -> impl IntoElement {
        let id = source.id;
        let settings = source.settings.clone();
        let mode = settings["mode"].as_str().unwrap_or("countdown").to_string();
        let seconds = settings["seconds"].as_i64().unwrap_or(300);
        let status = self.props.timer.as_ref().filter(|(i, ..)| *i == id).map(|(_, s, _)| *s);
        let running = status.is_some_and(|s| s.running);
        let set = move |key: &str, value: Value| Command::SetSourceSettings {
            source: id,
            settings: with(&settings, key, value),
        };
        let modes = self.segmented(
            "timer-mode",
            [("countdown", "Countdown"), ("stopwatch", "Stopwatch"), ("clock", "Clock")]
                .into_iter()
                .map(|(value, label)| (label, set("mode", json!(value)), mode == value))
                .collect(),
            cx,
        );
        let action = move |a: TimerAction| {
            move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.studio.read(cx).engine().post(move |s| s.reconciler.timer_action(id, a));
                this.props.timer = None;
                cx.notify();
            }
        };
        let mut col = self.section("Timer", None, cx).child(modes);
        if mode == "countdown" {
            let (shorter, longer) =
                (set("seconds", json!((seconds - 60).max(10))), set("seconds", json!(seconds + 60)));
            // Then: which scene to go to when it ends (click to cycle).
            let workspace = self.studio.read(cx).workspace().clone();
            let end = source.settings["end_scene"].as_str().map(str::to_string);
            let current = workspace.scenes.iter().position(|sc| Some(sc.id.to_string()) == end);
            let next = match current {
                None => workspace.scenes.first().map(|sc| json!(sc.id.to_string())),
                Some(i) => workspace.scenes.get(i + 1).map(|sc| json!(sc.id.to_string())),
            }
            .unwrap_or(Value::Null);
            let then = current.map_or("Stay".to_string(), |i| format!("→ {}", workspace.scenes[i].name));
            let cycle = set("end_scene", next);
            col = col.child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_1()
                    .child(self.field(
                        "timer-length",
                        "⏱",
                        format!("{}:{:02}", seconds / 60, seconds % 60),
                        shorter,
                        longer,
                        cx,
                    ))
                    .child(
                        div()
                            .id("timer-end")
                            .flex()
                            .items_center()
                            .gap_1()
                            .h(px(30.))
                            .px_2()
                            .rounded(px(6.))
                            .bg(tok::bg_raised())
                            .cursor_pointer()
                            .hover(|d| d.bg(tok::bg_selected()))
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("When it ends (click to change)")
                                    .build(w, cx)
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .child(then),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.exec(cycle.clone(), cx))),
                    ),
            );
        }
        if mode == "clock" {
            let secs = source.settings["show_seconds"].as_bool().unwrap_or(false);
            col = col.child(self.segmented(
                "clock-seconds",
                vec![
                    ("12:30", set("show_seconds", json!(false)), !secs),
                    ("12:30:45", set("show_seconds", json!(true)), secs),
                ],
                cx,
            ));
        } else {
            let icon_button = |id: &'static str, icon: &'static str, tip: &'static str| {
                crate::ui::icon_button(id, icon, tok::text_secondary())
                    .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
            };
            col = col.child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        icon_button(
                            "timer-run",
                            if running { "pause" } else { "play" },
                            if running { "Pause" } else { "Start" },
                        )
                        .on_click(cx.listener(action(if running {
                            TimerAction::Pause
                        } else {
                            TimerAction::Start
                        }))),
                    )
                    .child(
                        icon_button("timer-reset", "rotate-ccw", "Reset")
                            .on_click(cx.listener(action(TimerAction::Reset))),
                    )
                    .child(
                        icon_button("timer-add", "plus", "Add a minute")
                            .on_click(cx.listener(action(TimerAction::Add(60)))),
                    ),
            );
        }
        col
    }

    /// Which of an interface's channels a track takes: all, one (mono) or a pair (stereo).
    fn track_channels(
        &self,
        workspace: &lightair_core::Workspace,
        track: &lightair_core::Track,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use lightair_core::TrackChannels;
        let source = workspace.sources.get(&track.source)?;
        if source.kind != lightair_engine::devices::DEVICE_KIND {
            return None;
        }
        let device = source.settings["device"].as_str().unwrap_or("").to_string();
        let devices = {
            let mut cache = self.interfaces.borrow_mut();
            if cache.0.elapsed() > std::time::Duration::from_secs(5) || cache.1.is_empty() {
                *cache = (std::time::Instant::now(), lightair_engine::devices::input_devices());
            }
            cache.1.clone()
        };
        let channels = devices
            .iter()
            .find(|d| if lightair_engine::devices::is_system_default(&device) { d.default } else { d.id == device || d.name == device })
            .map_or(2, |d| d.channels);
        let shown = lightair_engine::devices::device_name(&device).unwrap_or_else(|| crate::mixer::SYSTEM_DEFAULT.to_string());
        // the device: the system default, or any source the Mac has
        let picker = {
            let me = cx.entity().downgrade();
            let (source_id, track) = (source.id, track.clone());
            let base = source.settings.clone();
            let current = device.clone();
            Button::new("track-device")
                .label(shown)
                .small()
                .outline()
                .dropdown_menu(move |mut menu, _, _| {
                    let mut choices = vec![(String::new(), crate::mixer::SYSTEM_DEFAULT.to_string())];
                    choices.extend(devices.iter().map(|d| {
                        let n = if d.channels == 1 { "1 input".to_string() } else { format!("{} inputs", d.channels) };
                        (d.id.clone(), format!("{} · {n}", d.name))
                    }));
                    for (value, label) in choices {
                        let (me, track) = (me.clone(), track.clone());
                        let mut settings = base.clone();
                        settings["device"] = serde_json::Value::String(value.clone());
                        let on = value == current;
                        menu = menu.item(gpui_kit::component::menu::PopupMenuItem::new(label).checked(on).on_click(
                            move |_, _, cx| {
                                // another device has other channels: start from all of them
                                // the source takes the device's name (System Default for the system's); the
                                // track keeps its own (crate::console::repoint)
                                let name = crate::console::mic_name(&value, &lightair_engine::devices::device_name(&value).unwrap_or_default());
                                let t = lightair_core::Track { channels: TrackChannels::All, ..track.clone() };
                                let cmd = Command::Batch {
                                    label: "Change input device".into(),
                                    commands: vec![
                                        Command::SetSourceSettings { source: source_id, settings: settings.clone() },
                                        Command::RenameSource { source: source_id, name },
                                        Command::UpdateTrack { track: t },
                                    ],
                                };
                                if let Some(me) = me.upgrade() {
                                    me.update(cx, |this, cx| this.exec(cmd, cx));
                                }
                            },
                        ));
                    }
                    menu
                })
        };
        let set = |c: TrackChannels| Command::UpdateTrack { track: lightair_core::Track { channels: c, ..track.clone() } };
        let mut options = vec![("All".to_string(), TrackChannels::All)];
        options.extend((0..channels).map(|c| (format!("{}", c + 1), TrackChannels::Mono(c))));
        options.extend((0..channels / 2).map(|p| (format!("{}–{}", p * 2 + 1, p * 2 + 2), TrackChannels::Stereo(p * 2, p * 2 + 1))));
        let mut row = div().flex().flex_wrap().gap_1();
        for (n, (label, c)) in options.into_iter().enumerate() {
            let on = track.channels == c;
            let cmd = set(c);
            row = row.child(
                div()
                    .id(("track-ch", n))
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
                    .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx))),
            );
        }
        Some(
            self.section("Input", None, cx)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(div().text_size(px(12.)).child("Device"))
                        .child(picker),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(tok::text_tertiary())
                        .child(if channels == 1 { "1 channel".to_string() } else { format!("{channels} channels: all, one, or a pair") }),
                )
                .when(channels > 1, |d| d.child(row))
                .into_any_element(),
        )
    }

    /// A mixer track: level, mute, and its effect chain (the mixer strip stays compact).
    pub(super) fn track_inspector(
        &self,
        workspace: &lightair_core::Workspace,
        track: &lightair_core::Track,
        body: Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tid = track.id;
        body.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_none().child(crate::ui::icon("audio-lines").size(px(16.)).text_color(tok::text_secondary())))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().font_weight(FontWeight::SEMIBOLD).child(track.name.clone())),
        )
        .children(workspace.sources.get(&track.source).and_then(|i| self.source_section(&i.clone(), cx)))
        .children(self.track_channels(workspace, track, cx))
        .child(self.track_routing(workspace, track, cx))
        .child({
            let rec = track.record;
            let cmd = Command::UpdateTrack { track: lightair_core::Track { record: !rec, ..track.clone() } };
            div()
                .flex()
                .items_center()
                .justify_between()
                .py_1()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(12.)).child("Record to its own file"))
                        .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("In multitrack recordings")),
                )
                .child(super::settings::switch("track-record", rec).on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx))))
        })
        .child(self.track_effects_section(track, cx))
        .child(
            div().pt_2().child(
                Button::new("remove-track")
                    .label("Remove track")
                    .small()
                    .danger()
                    .on_click(cx.listener(move |this, _, _, cx| this.exec(Command::RemoveTrack { track: tid }, cx))),
            ),
        )
        .into_any_element()
    }

    /// A web page: its address, its size (the page's layout width, and the frame's), how often it
    /// may repaint, and a reload.
    fn web_section(&self, source: &Source, cx: &mut Context<Self>) -> impl IntoElement {
        const SIZES: &[(&str, &str, (i64, i64))] = &[
            ("1080p", "Landscape, 1920×1080", (1920, 1080)),
            ("720p", "Landscape, 1280×720", (1280, 720)),
            ("Vertical", "Portrait, 1080×1920", (1080, 1920)),
            ("Square", "1080×1080", (1080, 1080)),
        ];
        let (id, settings) = (source.id, source.settings.clone());
        let width = settings["width"].as_i64().unwrap_or(1920);
        let height = settings["height"].as_i64().unwrap_or(1080);
        let fps = settings["fps"].as_i64().unwrap_or(30);
        // a browser's zoom steps
        const ZOOMS: &[i64] = &[25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500];
        let zoom = settings["zoom"].as_i64().unwrap_or(100);
        let zoom_out = ZOOMS.iter().rev().copied().find(|z| *z < zoom).unwrap_or(ZOOMS[0]);
        let zoom_in = ZOOMS.iter().copied().find(|z| *z > zoom).unwrap_or(ZOOMS[ZOOMS.len() - 1]);
        let set = |pairs: &[(&str, Value)]| {
            let mut s = settings.clone();
            for (key, value) in pairs {
                s = with(&s, key, value.clone());
            }
            Command::SetSourceSettings { source: id, settings: s }
        };
        let sizes = SIZES
            .iter()
            .map(|(label, _, (w, h))| {
                let page = set(&[("width", json!(w)), ("height", json!(h))]);
                (*label, self.with_web_box(page, (*w, *h), cx), (*w, *h) == (width, height))
            })
            .collect();
        let rates = [30i64, 60]
            .into_iter()
            .map(|r| (if r == 30 { "30 fps" } else { "60 fps" }, set(&[("fps", json!(r))]), fps == r))
            .collect();
        let custom = !SIZES.iter().any(|(_, _, wh)| *wh == (width, height));
        let label = |text: &'static str| div().text_size(px(11.)).text_color(tok::text_tertiary()).child(text);
        self.section(
            "Browser",
            Some(
                crate::ui::icon_button("web-reload", "refresh-cw", tok::text_secondary())
                    .tooltip(crate::ui::tip("Reload the page"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.studio.read(cx).engine().post(move |s| s.reconciler.web_reload(id));
                    }))
                    .into_any_element(),
            ),
            cx,
        )
        .when_some(self.props.web.as_ref().filter(|t| t.source == source.id), |d, t| d.child(TextInput::new(&t.state).small()))
        .child(label("Size"))
        .child(self.segmented("web-size", sizes, cx))
        .when(custom, |d| {
            d.child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(format!("Custom, {width}×{height}")))
        })
        .child(label("Frame rate"))
        .child(self.segmented("web-fps", rates, cx))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(label("Zoom"))
                .when(zoom != 100, |d| {
                    let reset = set(&[("zoom", json!(100))]);
                    d.child(
                        div()
                            .id("web-zoom-reset")
                            .text_size(px(11.))
                            .text_color(tok::text_tertiary())
                            .cursor_pointer()
                            .hover(|d| d.text_color(tok::text()))
                            .child("Reset")
                            .on_click(cx.listener(move |this, _, _, cx| this.exec(reset.clone(), cx))),
                    )
                }),
        )
        .child(self.field(
            "web-zoom",
            "",
            format!("{zoom}%"),
            set(&[("zoom", json!(zoom_out))]),
            set(&[("zoom", json!(zoom_in))]),
            cx,
        ))
        .child(label("Custom CSS"))
        .when_some(self.props.web_css.as_ref().filter(|t| t.source == source.id), |d, t| {
            d.child(
                div()
                    .h(px(120.))
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(tok::bg_raised())
                    .text_size(px(11.))
                    .child(Editor::new(&t.state).h(px(120.)).bordered(false)),
            )
        })
        .child({
            let css = settings["css"].as_str().unwrap_or("").to_string();
            let mut row = div().flex().flex_wrap().gap_1();
            for (i, (name, tip, snippet)) in CSS_SNIPPETS.iter().enumerate() {
                let added = css.contains(snippet);
                let next = if added {
                    css.replace(snippet, "").trim().to_string()
                } else if css.trim().is_empty() {
                    snippet.to_string()
                } else {
                    format!("{}\n{snippet}", css.trim_end())
                };
                let cmd = set(&[("css", json!(next))]);
                row = row.child(
                    div()
                        .id(("web-css-snippet", i))
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .cursor_pointer()
                        .when(added, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                        .when(!added, |d| {
                            d.bg(tok::bg_raised()).text_color(tok::text_tertiary()).hover(|d| d.text_color(tok::text_secondary()))
                        })
                        .child(*name)
                        .tooltip(crate::ui::tip(*tip))
                        .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx))),
                );
            }
            row
        })
    }

    /// `page` (a web page's new size, `w × h`) with the selected object's box resized to match
    /// on the canvas shown, around its centre: the box is the page's window.
    fn with_web_box(&self, page: Command, (w, h): (i64, i64), cx: &mut Context<Self>) -> Command {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace();
        let canvas = self.canvas_id(workspace);
        let layout = studio.selected_scene.and_then(|scene| {
            let sc = workspace.scene(scene)?;
            let object = studio.selected_object?;
            let world = sc.world(object, canvas)?;
            let c = world.crop;
            let (bw, bh) = ((w - (c.left + c.right) as i64).max(16) as f32, (h - (c.top + c.bottom) as i64).max(16) as f32);
            let (cx_, cy_) = (world.x + world.width / 2.0, world.y + world.height / 2.0);
            let resized = lightair_core::Layout { x: cx_ - bw / 2.0, y: cy_ - bh / 2.0, width: bw, height: bh, ..world };
            Some(Command::SetLayout { scene, object, canvas, layout: sc.local(object, canvas, &resized)? })
        });
        match layout {
            Some(layout) => Command::Batch { label: "Resize browser".into(), commands: vec![page, layout] },
            None => page,
        }
    }

    fn colour_section(&self, _source: &Source, cx: &mut Context<Self>) -> impl IntoElement {
        self.section("Colour", None, cx).child(self.colour_field("object-colour", cx))
    }

    fn media_section(&self, source: &Source, cx: &mut Context<Self>) -> impl IntoElement {
        let id = source.id;
        let status = self.props.media.as_ref().filter(|(i, ..)| *i == id).map(|(_, s, _)| *s);
        let playing = status
            .is_some_and(|s| matches!(s.state, MediaState::Playing | MediaState::Opening | MediaState::Buffering));
        let looping = source.settings["looping"].as_bool().unwrap_or(false);
        let file = source.settings["local_file"]
            .as_str()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No file".into());
        let (time, duration) = status.map_or((0, 0), |s| (s.time_ms, s.duration_ms));
        let progress = if duration > 0 { (time as f32 / duration as f32).clamp(0.0, 1.0) } else { 0.0 };
        let state_label = match status.map(|s| s.state) {
            Some(MediaState::Error) => "Couldn't play this file",
            Some(MediaState::Ended) => "Ended",
            Some(MediaState::Stopped) => "Stopped",
            _ => "",
        };
        let loop_settings = with(&source.settings, "looping", json!(!looping));
        self.section("Media", None, cx)
            .child(div().text_color(tok::text_secondary()).overflow_hidden().text_ellipsis().child(file))
            .child(
                div()
                    .id("media-progress")
                    .h(px(4.))
                    .w_full()
                    .rounded_full()
                    .bg(tok::border())
                    .child(div().h_full().rounded_full().bg(tok::accent_primary()).w(relative(progress))),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(11.))
                    .font_family(crate::ui::MONO)
                    .text_color(tok::text_tertiary())
                    .child(clock(time))
                    .child(state_label)
                    .child(clock(duration)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        Button::new("media-play")
                            .label(if playing { "Pause" } else { "Play" })
                            .small()
                            .outline()
                            .disabled(status.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let action = if playing { MediaAction::Pause } else { MediaAction::Play };
                                this.media(id, action, cx)
                            })),
                    )
                    .child(
                        Button::new("media-restart")
                            .label("Restart")
                            .small()
                            .ghost()
                            .disabled(status.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| this.media(id, MediaAction::Restart, cx))),
                    )
                    .child({
                        let b = Button::new("media-loop").label("Loop").small().on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.exec(Command::SetSourceSettings { source: id, settings: loop_settings.clone() }, cx)
                            },
                        ));
                        if looping { b.primary() } else { b.ghost() }
                    }),
            )
    }
}

pub(super) fn rgb_to_hsla(rgb_value: u32) -> Hsla {
    rgb(rgb_value).into()
}

#[cfg(test)]
mod tests {
    use super::{abgr, clock, rgb_of, with};
    use serde_json::json;

    #[test]
    fn colour_order() {
        assert_eq!(abgr(0xFF4A3B), 0xFF3B4AFF);
        assert_eq!(rgb_of(abgr(0x123456)), 0x123456);
        assert_eq!(clock(83_500), "1:23");
        let s = with(&json!({"text": "a", "font": {"size": 10}}), "text", json!("b"));
        assert_eq!(s, json!({"text": "b", "font": {"size": 10}}));
    }
}
