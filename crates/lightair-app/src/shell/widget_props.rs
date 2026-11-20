//! The Inspector for a widget (docs/plans/live-integrations.md, 2.5): its settings, made from its
//! fields' descriptions (StreamElements' format: text, numbers, colours, choices, switches,
//! pictures and sounds, buttons), test events to try it with, and its address for OBS. Every
//! change is a source settings change: undoable, and the open page gets it at once.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, EditorState, Input as TextInput, InputEvent, InputState, TabSize};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_model::{Command, Id, Source};
use lightair_widgets::fields::{self, Field};
use lightair_widgets::{Custom, Instance, catalog, test_events};
use std::collections::HashSet;

use serde_json::{Value, json};

use super::StudioWindow;
use crate::ui::{self, tok};

/// The text-like fields' inputs for the selected widget.
pub(super) struct WidgetEdit {
    source: Id,
    inputs: Vec<(String, Entity<InputState>)>,
    /// Multi-line fields ("textarea"): set when the widget is selected, saved on leaving them.
    areas: Vec<(String, Entity<EditorState>)>,
    /// Its code, when it's its own (imported, or a built-in made one's own).
    code: Option<CodeEdit>,
    /// Counts keystrokes in each text field, so only the last one's pause keeps what's typed.
    typed: std::collections::HashMap<String, u64>,
    _events: Vec<Subscription>,
}

/// A widget's code being edited: a tab each for its HTML, CSS, JS and fields. Typing changes the
/// widget after a pause (and on leaving the editor); the page loads again with it.
pub(super) struct CodeEdit {
    tab: usize,
    /// Shown in the large editor (`widget_code_overlay`) instead of the Inspector.
    expanded: bool,
    editors: Vec<Entity<EditorState>>,
    /// Counts keystrokes, so only the last one's pause applies them.
    typed: u64,
    _events: Vec<Subscription>,
}

/// The code tabs: (name, the editor's language).
const TABS: &[(&str, &str)] = &[("HTML", "html"), ("CSS", "css"), ("JS", "javascript"), ("Fields", "json")];

/// What the code tab shown is about, under it.
fn code_hint(tab: usize, custom: &Custom) -> String {
    match (tab, &custom.folder) {
        (3, _) => "Its settings, as the Inspector shows them: each one's type, label, value and group. Use one in the code as {{name}}, or lightair.fields.name in JS.".to_string(),
        (_, Some(dir)) => format!("Also kept in {}: edits saved there show here.", std::path::Path::new(dir).file_name().map_or(dir.clone(), |n| n.to_string_lossy().into_owned())),
        _ => "Changes show on the canvas a moment after you stop typing. In JS, lightair.on('load' | 'fields' | 'event' | 'alert' | 'data', …) gets what Lightair sends.".to_string(),
    }
}

fn code_texts(c: &Custom) -> [&str; 4] {
    [&c.html, &c.css, &c.js, &c.fields]
}

/// Fields typed into (a line of text, a font's name, a file's path or address).
fn typed(kind: &str) -> bool {
    !matches!(kind, "checkbox" | "dropdown" | "button" | "hidden" | "textarea" | "colorpicker" | "fontpicker" | "number" | "slider")
}

/// Fonts a "fontpicker" offers: (name, the CSS that sets it).
const FONTS: &[(&str, &str)] = &[
    ("Inter", "Inter, -apple-system, \"Segoe UI\", sans-serif"),
    ("System", "-apple-system, BlinkMacSystemFont, \"Segoe UI\", sans-serif"),
    ("Helvetica", "\"Helvetica Neue\", Helvetica, Arial, sans-serif"),
    ("Rounded", "ui-rounded, \"SF Pro Rounded\", \"Arial Rounded MT Bold\", sans-serif"),
    ("Georgia", "Georgia, \"Times New Roman\", serif"),
    ("Impact", "Impact, \"Arial Black\", sans-serif"),
    ("Monospace", "ui-monospace, Menlo, Consolas, monospace"),
];

/// A font setting's name: one of `FONTS`, else its first family.
fn font_name(css: &str) -> String {
    FONTS.iter().find(|f| f.1 == css).map_or_else(
        || css.split(',').next().unwrap_or("").trim().trim_matches(['"', '\'']).to_string(),
        |f| f.0.to_string(),
    )
}

/// How widgets get what they show (the ⓘ beside a widget's description).
const HOW: &str = "Widgets show what happens on the platforms you've signed in to (Outputs): chat, follows, \
subs, donations and more, as it happens. Send a test to see one now: it plays on the canvas, and on \
stream if you're live. The link copies its address, to use it in OBS or a browser on this computer.";

fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(super) fn widget_of(source: &Source) -> Option<Instance> {
    Instance::from_settings(&source.settings)
}

impl StudioWindow {
    /// Keeps the selected widget's inputs in step (called from render, with the selection).
    pub(super) fn sync_widget_props(&mut self, source: Option<&Source>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((source, instance)) = source.and_then(|s| widget_of(s).map(|i| (s, i))) else {
            self.props.widget = None;
            return;
        };
        let list = fields::parse(instance.schema());
        let values = fields::values(&list, &instance.fields);
        if !self.props.widget.as_ref().is_some_and(|w| w.source == source.id) {
            let mut inputs = Vec::new();
            let mut events = Vec::new();
            for f in list.iter().filter(|f| typed(&f.kind)) {
                let state = cx.new(|cx| InputState::new(window, cx));
                let (id, key) = (source.id, f.key.clone());
                events.push(cx.subscribe_in(&state, window, move |this, state, ev: &InputEvent, _, cx| match ev {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        let text = state.read(cx).value().to_string();
                        this.set_widget_field(id, &key, json!(text), cx);
                    }
                    // kept once typing pauses too (leaving a field isn't always seen)
                    InputEvent::Change => {
                        let Some(w) = this.props.widget.as_mut().filter(|w| w.source == id) else { return };
                        let n = w.typed.entry(key.clone()).or_default();
                        *n += 1;
                        let (n, key, state) = (*n, key.clone(), state.clone());
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(600)).await;
                            let _ = this.update(cx, |this, cx| {
                                if this.props.widget.as_ref().is_some_and(|w| w.source == id && w.typed.get(&key) == Some(&n)) {
                                    let text = state.read(cx).value().to_string();
                                    this.set_widget_field(id, &key, json!(text), cx);
                                }
                            });
                        })
                        .detach();
                    }
                    _ => {}
                }));
                inputs.push((f.key.clone(), state));
            }
            let mut areas = Vec::new();
            for f in list.iter().filter(|f| f.kind == "textarea") {
                let text = values.get(&f.key).map(shown).unwrap_or_default();
                let state = cx.new(|cx| EditorState::new(window, cx).line_number(false).default_value(text));
                let (id, key) = (source.id, f.key.clone());
                events.push(cx.subscribe_in(&state, window, move |this, state, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::Blur) {
                        let text = state.read(cx).value().to_string();
                        this.set_widget_field(id, &key, json!(text), cx);
                    }
                }));
                areas.push((f.key.clone(), state));
            }
            self.props.widget = Some(WidgetEdit { source: source.id, inputs, areas, code: None, typed: Default::default(), _events: events });
        }
        // colour fields: a full picker each
        for f in list.iter().filter(|f| f.kind == "colorpicker") {
            let current = values.get(&f.key).and_then(|v| v.as_str()).and_then(super::colour_field::parse).unwrap_or_else(|| gpui_kit::rgb(0xFFFFFF).into());
            let (id, key) = (source.id, f.key.clone());
            let apply: super::colour_field::Apply = std::rc::Rc::new(move |this: &mut StudioWindow, c, cx| {
                this.set_widget_field(id, &key, json!(super::colour_field::css(c)), cx);
            });
            self.sync_colour(&format!("w-{}-{}", source.id, f.key), current, apply, window, cx);
        }
        // its code's editors
        let w = self.props.widget.as_mut().unwrap();
        match &instance.custom {
            Some(c) => {
                if w.code.is_none() {
                    let mut editors = Vec::new();
                    let mut events = Vec::new();
                    for (&(_, language), text) in TABS.iter().zip(code_texts(c)) {
                        let state = cx.new(|cx| EditorState::new(window, cx).language(language).line_number(true).indent_guides(true).folding(true).tab_size(TabSize { tab_size: 2, hard_tabs: false }).default_value(text.to_string()));
                        let id = source.id;
                        events.push(cx.subscribe_in(&state, window, move |this, _, ev: &InputEvent, _, cx| match ev {
                            InputEvent::Change => this.code_typed(id, cx),
                            InputEvent::Blur => this.apply_code(id, cx),
                            _ => {}
                        }));
                        editors.push(state);
                    }
                    w.code = Some(CodeEdit { tab: 0, expanded: false, editors, typed: 0, _events: events });
                }
                // changed elsewhere (undone, its folder edited): shown, unless it's being typed in
                let code = w.code.as_ref().unwrap();
                for (state, text) in code.editors.iter().zip(code_texts(c)) {
                    let focused = state.read(cx).focus_handle(cx).is_focused(window);
                    if !focused && state.read(cx).value() != text {
                        state.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
                    }
                }
            }
            None => w.code = None,
        }
        let w = self.props.widget.as_ref().unwrap();
        for (key, state) in &w.inputs {
            let value = values.get(key).map(shown).unwrap_or_default();
            let focused = state.read(cx).focus_handle(cx).is_focused(window);
            if !focused && state.read(cx).value() != value.as_str() {
                state.update(cx, |s, cx| s.set_value(value, window, cx));
            }
        }
    }

    pub(super) fn set_widget_field(&mut self, source: Id, key: &str, value: Value, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(s) = workspace.sources.get(&source) else { return };
        let Some(instance) = widget_of(s) else { return };
        let list = fields::parse(instance.schema());
        if fields::values(&list, &instance.fields).get(key) == Some(&value) {
            return;
        }
        self.exec(widget_command(s, key, value), cx);
    }

    /// For screenshots: the selected widget's code made its own (and kept in its folder).
    pub(super) fn customize_selected_widget(&mut self, folder: bool, cx: &mut Context<Self>) {
        let found = {
            let s = self.studio.read(cx);
            let workspace = s.workspace();
            let object = s.selected_scene.and_then(|id| workspace.scene(id)).and_then(|sc| s.selected_object.and_then(|o| sc.object(o)));
            object.and_then(|o| o.source).and_then(|id| workspace.sources.get(&id)).and_then(|src| widget_of(src).map(|i| (src.id, i.kind)))
        };
        if let Some((id, kind)) = found {
            self.set_widget_code(id, lightair_widgets::customize(&kind), cx);
            if folder {
                self.keep_widget_in_folder(id, cx);
            }
        }
    }

    /// A keystroke in the code: applied once typing pauses.
    fn code_typed(&mut self, source: Id, cx: &mut Context<Self>) {
        let Some(code) = self.props.widget.as_mut().filter(|w| w.source == source).and_then(|w| w.code.as_mut()) else { return };
        code.typed += 1;
        let n = code.typed;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
            let _ = this.update(cx, |this, cx| {
                if this.props.widget.as_ref().and_then(|w| w.code.as_ref()).is_some_and(|c| c.typed == n) {
                    this.apply_code(source, cx);
                }
            });
        })
        .detach();
    }

    /// The code in the editors, given to the widget.
    fn apply_code(&mut self, source: Id, cx: &mut Context<Self>) {
        let Some(code) = self.props.widget.as_ref().filter(|w| w.source == source).and_then(|w| w.code.as_ref()) else { return };
        let v: Vec<String> = code.editors.iter().map(|e| e.read(cx).value().to_string()).collect();
        let next = Custom { html: v[0].clone(), css: v[1].clone(), js: v[2].clone(), fields: v[3].clone(), folder: None };
        self.set_widget_code(source, Some(next), cx);
    }

    /// Sets a widget's code (`None`: a built-in's own again), keeping where it's kept.
    fn set_widget_code(&mut self, source: Id, code: Option<Custom>, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(s) = workspace.sources.get(&source) else { return };
        let Some(mut instance) = widget_of(s) else { return };
        let code = code.map(|c| Custom { folder: instance.custom.as_ref().and_then(|o| o.folder.clone()), ..c });
        if match (&instance.custom, &code) {
            (Some(a), Some(b)) => a.same_code(b),
            (a, b) => a.is_none() && b.is_none(),
        } {
            return;
        }
        instance.custom = code;
        let mut settings = s.settings.clone();
        settings["widget"] = json!(instance);
        self.exec(Command::SetSourceSettings { source, settings }, cx);
    }

    /// Keeps its code in a folder too (made, if new) and opens it: edits saved there show at once.
    fn open_widget_folder(&mut self, source: Id, cx: &mut Context<Self>) {
        if let Some(dir) = self.keep_widget_in_folder(source, cx) {
            cx.open_with_system(&dir);
        }
    }

    /// Keeps a widget's code in its folder too (made, and written, if new): the folder.
    fn keep_widget_in_folder(&mut self, source: Id, cx: &mut Context<Self>) -> Option<std::path::PathBuf> {
        let workspace = self.studio.read(cx).workspace().clone();
        let s = workspace.sources.get(&source)?;
        let mut instance = widget_of(s)?;
        let c = instance.custom.as_mut()?;
        let dir = match &c.folder {
            Some(dir) => std::path::PathBuf::from(dir),
            None => {
                let name: String = s.name.chars().map(|ch| if ch.is_alphanumeric() || " -_".contains(ch) { ch } else { '-' }).collect();
                lightair_platform::paths::data_dir().join("Widgets").join(format!("{} {}", name.trim(), instance.id))
            }
        };
        if let Err(e) = lightair_widgets::write_folder(c, &dir) {
            self.studio.update(cx, |s, cx| {
                s.issue = Some(format!("Couldn't write the widget's files: {e}").into());
                cx.notify();
            });
            return None;
        }
        self.widget_folders.insert(source, c.clone());
        if c.folder.is_none() {
            c.folder = Some(dir.to_string_lossy().into_owned());
            let mut settings = s.settings.clone();
            settings["widget"] = json!(instance);
            self.exec(Command::SetSourceSettings { source, settings }, cx);
        }
        Some(dir)
    }

    /// Starts looking at widgets' folders (once).
    pub(super) fn watch_widget_folders(&mut self, cx: &mut Context<Self>) {
        if self.folder_watch.is_some() {
            return;
        }
        self.folder_watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                if this.update(cx, |this, cx| this.sync_widget_folders(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Each widget kept in a folder, in step with it both ways: files edited there come in;
    /// changes made here (typed, undone) are written there.
    fn sync_widget_folders(&mut self, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let mut kept = HashSet::new();
        for s in workspace.sources.values() {
            let Some(c) = widget_of(s).and_then(|i| i.custom) else { continue };
            let Some(dir) = c.folder.as_deref().map(std::path::PathBuf::from) else { continue };
            kept.insert(s.id);
            let disk = lightair_widgets::import_folder(&dir).ok();
            let last = self.widget_folders.get(&s.id);
            match disk {
                // edited there
                Some(d) if last.is_none_or(|l| !l.same_code(&d)) && !d.same_code(&c) => {
                    self.widget_folders.insert(s.id, d.clone());
                    self.set_widget_code(s.id, Some(d), cx);
                }
                // in step
                Some(d) if d.same_code(&c) => {
                    self.widget_folders.insert(s.id, d);
                }
                // changed here (or its files are gone): written there
                _ => {
                    if lightair_widgets::write_folder(&c, &dir).is_ok() {
                        self.widget_folders.insert(s.id, c);
                    }
                }
            }
        }
        self.widget_folders.retain(|id, _| kept.contains(id));
    }

    /// The platform test events say they're from: the first destination's, else Twitch.
    fn test_platform(&self, cx: &App) -> String {
        self.studio.read(cx).workspace().outputs.destinations.iter().find_map(|d| d.platform.clone()).unwrap_or_else(|| "twitch".into())
    }

    pub(super) fn widget_section(&self, source: &Source, cx: &mut Context<Self>) -> Option<AnyElement> {
        let instance = widget_of(source)?;
        let widgets = self.studio.read(cx).widgets.clone()?;
        let list = fields::parse(instance.schema());
        let values = fields::values(&list, &instance.fields);
        let builtin = catalog::by_kind(&instance.kind);
        let title: &'static str = builtin.map_or("Widget", |b| b.name);
        let about = builtin.map_or("A widget you brought in", |b| b.about);

        let url = widgets.url(&instance.id, &instance.key);
        let copy = ui::icon_button("widget-copy-url", "link", tok::text_secondary())
            .tooltip(ui::tip("Copy its address (for OBS or a browser on this computer)"))
            .on_click(cx.listener(move |_, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(url.clone()))));
        let mut body = self.section(title, Some(copy.into_any_element()), cx);

        // what it is (and how widgets work), then: send it a test event; export an imported one
        body = body.child(
            div()
                .flex()
                .items_start()
                .gap_1p5()
                .child(div().flex_1().min_w_0().text_size(px(12.)).line_height(px(17.)).text_color(tok::text_secondary()).child(about))
                .child(div().pt(px(2.)).child(ui::info("widget-how", HOW))),
        );
        // (tests come from a platform it listens to)
        let platform = instance.platforms.first().cloned().unwrap_or_else(|| self.test_platform(cx));
        let w = widgets.clone();
        let test = Button::new("widget-test")
            .label("Send a test")
            .icon(ui::icon("zap"))
            .small()
            .outline()
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for &(kind, name) in test_events::KINDS {
                    let (w, platform) = (w.clone(), platform.clone());
                    menu = menu.item(PopupMenuItem::new(name).on_click(move |_, _, _| w.test(kind, &platform)));
                }
                menu
            });
        let actions = div().flex().items_center().gap_1().child(test);
        body = body.child(self.widget_platforms(source, &instance, cx)).child(actions);

        // the fields, in their groups (a group named like the widget needs no heading)
        let mut group = String::new();
        for f in list.iter().filter(|f| f.is_shown()) {
            if !f.group.is_empty() && f.group != group {
                group = f.group.clone();
                if !group.eq_ignore_ascii_case(title) {
                    body = body.child(
                        div()
                            .pt_2()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tok::text_tertiary())
                            .child(group.to_uppercase()),
                    );
                }
            }
            body = body.child(self.widget_field(source, &instance, f, values.get(&f.key).cloned().unwrap_or(Value::Null), cx));
        }
        Some(body.child(self.widget_code(source, &instance, builtin.is_some(), cx)).into_any_element())
    }

    /// Where its events come from: each platform set up (in Outputs) with how its chat
    /// connection is doing, and whether this widget listens to it (all, until some are picked).
    fn widget_platforms(&self, source: &Source, instance: &Instance, cx: &mut Context<Self>) -> AnyElement {
        use lightair_live::Status;
        let id = source.id;
        // each platform's channels, at their best ("connected" if any is)
        let mut status: Vec<(String, Vec<(String, Status)>)> = Vec::new();
        for (channel, s) in self.studio.read(cx).chat.status() {
            match status.iter_mut().find(|(p, _)| *p == channel.platform) {
                Some((_, list)) => list.push((channel.channel, s)),
                None => status.push((channel.platform, vec![(channel.channel, s)])),
            }
        }
        // platforms it was set to that aren't set up now still show (off, and unset)
        for p in &instance.platforms {
            if !status.iter().any(|(q, _)| q == p) {
                status.push((p.clone(), Vec::new()));
            }
        }
        status.sort_by_key(|(p, _)| crate::platforms::by_id(p).map_or(p.clone(), |x| x.name.to_string()));
        let known: Vec<String> = status.iter().map(|(p, _)| p.clone()).collect();
        let rank = |s: &Status| match s {
            Status::Connected => 0,
            Status::Connecting => 1,
            Status::Offline => 2,
            Status::Error { .. } => 3,
        };
        let label = div().text_size(px(12.)).text_color(tok::text_secondary()).child("Gets events from");
        let mut col = div().flex().flex_col().gap_1p5().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(label)
                .when(!instance.platforms.is_empty(), |d| {
                    d.child(
                        div()
                            .id("widget-platforms-all")
                            .text_size(px(11.))
                            .text_color(tok::text_tertiary())
                            .cursor_pointer()
                            .hover(|d| d.text_color(tok::text()))
                            .child("Every platform")
                            .on_click(cx.listener(move |this, _, _, cx| this.set_widget_platforms(id, Vec::new(), cx))),
                    )
                }),
        );
        if status.is_empty() {
            return col
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .rounded(px(6.))
                        .border_1()
                        .border_dashed()
                        .border_color(tok::border_strong())
                        .text_size(px(11.))
                        .line_height(px(15.))
                        .text_color(tok::text_tertiary())
                        .child(ui::icon("unplug").size(px(14.)).flex_none())
                        .child(div().flex_1().min_w_0().child("No platform connected yet. Add a destination with its chat in Outputs and its events come here; until then, send a test.")),
                )
                .into_any_element();
        }
        let mut chips = div().flex().flex_wrap().gap_1();
        for (i, (platform, channels)) in status.into_iter().enumerate() {
            let on = instance.hears(&platform);
            let best = channels.iter().map(|(_, s)| s).min_by_key(|s| rank(s));
            let (dot, state) = match best {
                Some(Status::Connected) => (tok::ok(), "connected"),
                Some(Status::Connecting) => (tok::warn(), "connecting…"),
                Some(Status::Offline) => (tok::text_tertiary(), "not live"),
                Some(Status::Error { .. }) => (tok::error(), "can't read its chat"),
                None => (tok::border_strong(), "not set up in Outputs"),
            };
            let name = crate::platforms::by_id(&platform).map_or(platform.clone(), |p| p.name.to_string());
            let mut tip = format!("{name}: {state}");
            for (channel, s) in &channels {
                if let Status::Error { message } = s {
                    tip.push_str(&format!("\n{channel}: {message}"));
                } else if channels.len() > 1 || !channel.is_empty() {
                    tip.push_str(&format!("\n{channel}"));
                }
            }
            tip.push_str(if on { "\nClick to stop this widget getting its events" } else { "\nClick to get its events" });
            // clicked: listened to, or not (every platform again when all are on)
            let next: Vec<String> = {
                let mut chosen: Vec<String> = known.iter().filter(|p| instance.hears(p)).cloned().collect();
                if on {
                    chosen.retain(|p| *p != platform);
                } else {
                    chosen.push(platform.clone());
                }
                if known.iter().all(|p| chosen.contains(p)) { Vec::new() } else { chosen }
            };
            chips = chips.child(
                div()
                    .id(("widget-platform", i))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(26.))
                    .pl_1()
                    .pr_2()
                    .rounded(px(6.))
                    .border_1()
                    .cursor_pointer()
                    .when(on, |d| d.bg(tok::bg_raised()).border_color(tok::border_strong()))
                    .when(!on, |d| d.border_color(tok::border()).opacity(0.55).hover(|d| d.opacity(0.85)))
                    .map(|d| match crate::platforms::by_id(&platform) {
                        Some(p) => d.child(crate::outputs_panel::badge(p, 18.)),
                        None => d.child(ui::icon("radio").size(px(14.))),
                    })
                    .child(div().text_size(px(12.)).text_color(if on { tok::text() } else { tok::text_secondary() }).child(name))
                    .child(div().size(px(6.)).rounded_full().bg(dot))
                    .tooltip(ui::tip(tip))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_widget_platforms(id, next.clone(), cx))),
            );
        }
        col = col.child(chips);
        if instance.platforms.is_empty() {
            col = col.child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("All of them, and any added later. Click one to leave it out."));
        }
        col.into_any_element()
    }

    /// Sets the platforms a widget gets events from (none: every one).
    fn set_widget_platforms(&mut self, source: Id, platforms: Vec<String>, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(s) = workspace.sources.get(&source) else { return };
        let Some(mut instance) = widget_of(s) else { return };
        if instance.platforms == platforms {
            return;
        }
        instance.platforms = platforms;
        let mut settings = s.settings.clone();
        settings["widget"] = json!(instance);
        self.exec(Command::SetSourceSettings { source, settings }, cx);
    }

    /// Its look and code: a built-in's custom CSS, and making its code one's own; one's own
    /// code's editors (HTML, CSS, JS, fields), its folder, exporting it, putting a built-in back.
    fn widget_code(&self, source: &Source, instance: &Instance, builtin: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = source.id;
        let heading = |text: &'static str| {
            div().pt_2().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text_tertiary()).child(text)
        };
        let hint = |text: SharedString| div().text_size(px(11.)).line_height(px(15.)).text_color(tok::text_tertiary()).child(text);
        let editor_box = |editor: Editor, h: f32| {
            div().h(px(h)).rounded(px(5.)).overflow_hidden().bg(tok::bg_raised()).border_1().border_color(tok::border()).child(editor.h(px(h)).bordered(false).xsmall().text_size(px(11.)))
        };
        let mut col = div().flex().flex_col().gap_2();
        let Some(custom) = &instance.custom else {
            // a built-in: restyle it, or make its code one's own
            col = col.child(heading("STYLE AND CODE"));
            if let Some(t) = self.props.web_css.as_ref().filter(|t| t.source == id) {
                col = col
                    .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child("Custom CSS"))
                    .child(editor_box(Editor::new(&t.state), 110.))
                    .child(hint("Applies over its own style as you type. Each part has a class or id to aim at: \"Customize code\" shows them.".into()));
            }
            let kind = instance.kind.clone();
            col = col.child(
                Button::new("widget-customize")
                    .label("Customize code…")
                    .icon(ui::icon("code"))
                    .small()
                    .outline()
                    .w_full()
                    .tooltip("Your own copy of its HTML, CSS and JavaScript, to change anything about it")
                    .on_click(cx.listener(move |this, _, _, cx| this.set_widget_code(id, lightair_widgets::customize(&kind), cx))),
            );
            return col.into_any_element();
        };

        // its own code: tabs, and what to do with it; edited here, or in the large editor
        let code = self.props.widget.as_ref().filter(|w| w.source == id).and_then(|w| w.code.as_ref());
        let (tab, expanded) = code.map_or((0, false), |c| (c.tab, c.expanded));
        col = col.child(heading("CODE")).child(self.code_bar(source, custom, builtin, false, cx));
        if expanded {
            col = col.child(
                div()
                    .id("widget-code-elsewhere")
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .h(px(64.))
                    .rounded(px(5.))
                    .border_1()
                    .border_dashed()
                    .border_color(tok::border_strong())
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(tok::text_tertiary())
                    .hover(|d| d.text_color(tok::text_secondary()))
                    .child(ui::icon("panel-left-open").size(px(14.)))
                    .child("Open in the large editor")
                    .on_click(cx.listener(move |this, _, _, cx| this.expand_code(id, true, cx))),
            );
        } else if let Some(editor) = code.and_then(|c| c.editors.get(tab)) {
            col = col.child(editor_box(Editor::new(editor), 260.));
        }
        col.child(hint(code_hint(tab, custom).into())).into_any_element()
    }

    /// The code's tabs and actions: its folder, exporting it, putting a built-in back, and
    /// the large editor (opening it, or, in it, closing it).
    fn code_bar(&self, source: &Source, custom: &Custom, builtin: bool, large: bool, cx: &mut Context<Self>) -> Div {
        let id = source.id;
        let code = self.props.widget.as_ref().filter(|w| w.source == id).and_then(|w| w.code.as_ref());
        let tab = code.map_or(0, |c| c.tab);
        let mut tabs = div().flex().gap_0p5().p_0p5().rounded(px(6.)).bg(tok::bg_raised());
        for (i, (name, _)) in TABS.iter().enumerate() {
            let on = i == tab;
            tabs = tabs.child(
                div()
                    .id(("widget-code-tab", i))
                    .px_2()
                    .py(px(3.))
                    .rounded(px(4.))
                    .text_size(px(if large { 12. } else { 11. }))
                    .cursor_pointer()
                    .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                    .when(!on, |d| d.text_color(tok::text_tertiary()).hover(|d| d.text_color(tok::text_secondary())))
                    .child(*name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(c) = this.props.widget.as_mut().and_then(|w| w.code.as_mut()) {
                            c.tab = i;
                        }
                        cx.notify();
                    })),
            );
        }
        let size = (source.settings["width"].as_u64().unwrap_or(800) as u32, source.settings["height"].as_u64().unwrap_or(600) as u32);
        let name = source.name.clone();
        let export_code = custom.clone();
        let mut bar = div()
            .flex()
            .items_center()
            .gap_1()
            .child(tabs)
            .child(div().flex_1())
            .child(
                ui::icon_button(if large { "widget-folder-large" } else { "widget-folder" }, "folder-open", tok::text_secondary())
                    .tooltip(ui::tip("Open its files in a folder: edit them in any editor, and each save shows here at once"))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_widget_folder(id, cx))),
            )
            .child(
                ui::icon_button(if large { "widget-export-large" } else { "widget-export" }, "package", tok::text_secondary())
                    .tooltip(ui::tip("Export as a .lwidget package, to share or use elsewhere"))
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let path = cx.prompt_for_new_path(&std::env::home_dir().unwrap_or_default().join("Desktop"), Some(&format!("{name}.lwidget")));
                        let (custom, name) = (export_code.clone(), name.clone());
                        cx.spawn_in(window, async move |_, _| {
                            if let Ok(Ok(Some(path))) = path.await
                                && let Err(e) = lightair_widgets::export(&custom, &name, size, &path)
                            {
                                tracing::warn!(%e, "widget export");
                            }
                        })
                        .detach();
                    })),
            );
        if builtin {
            bar = bar.child(
                ui::icon_button(if large { "widget-revert-large" } else { "widget-revert" }, "rotate-ccw", tok::text_secondary())
                    .tooltip(ui::tip("Put back the original code (your changes can be undone)"))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_widget_code(id, None, cx))),
            );
        }
        let (icon, tip) = if large { ("minimize-2", "Back to the Inspector") } else { ("maximize-2", "Edit in a large editor, beside the canvas") };
        bar.child(
            ui::icon_button(if large { "widget-code-small" } else { "widget-code-large" }, icon, tok::text_secondary())
                .tooltip(ui::tip(tip))
                .on_click(cx.listener(move |this, _, _, cx| this.expand_code(id, !large, cx))),
        )
    }

    /// For screenshots: the selected widget listening to `platforms` only.
    pub(super) fn set_selected_widget_platforms(&mut self, platforms: Vec<String>, cx: &mut Context<Self>) {
        let id = {
            let s = self.studio.read(cx);
            let w = s.workspace();
            s.selected_scene.and_then(|id| w.scene(id)).and_then(|sc| s.selected_object.and_then(|o| sc.object(o))).and_then(|o| o.source)
        };
        if let Some(id) = id {
            self.set_widget_platforms(id, platforms, cx);
        }
    }

    /// For screenshots: the selected widget's code in the large editor, on its CSS tab.
    pub(super) fn expand_selected_code(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.props.widget.as_mut().and_then(|w| w.code.as_mut()) {
            c.expanded = true;
            c.tab = 1;
        }
        cx.notify();
    }

    /// Opens (or closes) the large code editor for the widget `source`.
    fn expand_code(&mut self, source: Id, open: bool, cx: &mut Context<Self>) {
        if let Some(c) = self.props.widget.as_mut().filter(|w| w.source == source).and_then(|w| w.code.as_mut()) {
            c.expanded = open;
        }
        cx.notify();
    }

    /// The large code editor: a panel at the left, the canvas still in view, for the selected
    /// widget whose code is its own.
    pub(super) fn widget_code_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let w = self.props.widget.as_ref()?;
        let code = w.code.as_ref().filter(|c| c.expanded)?;
        let workspace = self.studio.read(cx).workspace().clone();
        let source = workspace.sources.get(&w.source)?.clone();
        let instance = widget_of(&source)?;
        let custom = instance.custom.clone()?;
        let builtin = catalog::by_kind(&instance.kind).is_some();
        let editor = code.editors.get(code.tab)?.clone();
        let id = source.id;
        Some(
            div()
                .id("widget-code-large")
                .absolute()
                .left(px(10.))
                .top(px(54.))
                .bottom(px(34.))
                .w(relative(0.5))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(tok::bg_panel())
                .border_1()
                .border_color(tok::border_strong())
                .shadow_lg()
                .occlude()
                .capture_key_down(cx.listener(move |this, ev: &KeyDownEvent, _, cx| {
                    if ev.keystroke.key == "escape" {
                        this.expand_code(id, false, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .px_3()
                        .pt_3()
                        .pb_2()
                        .border_b_1()
                        .border_color(tok::border())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(ui::icon("code").size(px(15.)).text_color(tok::text_secondary()))
                                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(source.name.clone()))
                                .child(div().text_size(px(12.)).text_color(tok::text_tertiary()).child("code")),
                        )
                        .child(self.code_bar(&source, &custom, builtin, true, cx)),
                )
                .child(div().flex_1().min_h_0().p_1().child(Editor::new(&editor).h(relative(1.)).bordered(false).text_size(px(12.5))))
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(tok::border())
                        .text_size(px(11.))
                        .text_color(tok::text_tertiary())
                        .child(code_hint(code.tab, &custom)),
                )
                .into_any_element(),
        )
    }

    fn widget_field(&self, source: &Source, instance: &Instance, f: &Field, value: Value, cx: &mut Context<Self>) -> AnyElement {
        let id = source.id;
        let key = f.key.clone();
        // its name at the left (wrapped when long), its control at the right: a small control
        // (a switch, a number, a menu) leaves the name the room; a wide one (text, a colour, a
        // slider) takes what a column of names leaves
        let line = |wide: bool, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_h(px(26.))
                .child(
                    div()
                        .min_w_0()
                        .map(|d| if wide { d.flex_none().w(px(92.)) } else { d.flex_1() })
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(tok::text_secondary())
                        .child(f.label.clone()),
                )
                .child(div().flex().min_w_0().justify_end().map(|d| if wide { d.flex_1() } else { d.flex_none().max_w(px(160.)) }).child(control))
                .into_any_element()
        };
        let row = |control: AnyElement| line(false, control);
        let wide = |control: AnyElement| line(true, control);
        // its name above, its control the whole width
        let stacked = |control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child(f.label.clone()))
                .child(control)
                .into_any_element()
        };
        let menu = |current: String, options: Vec<(String, String)>, cx: &mut Context<Self>| {
            let me = cx.entity().downgrade();
            let key = key.clone();
            Button::new(SharedString::from(format!("wf-{key}")))
                .label(current)
                .xsmall()
                .outline()
                .w_full()
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for (v, l) in options.iter().cloned() {
                        let (me, key) = (me.clone(), key.clone());
                        menu = menu.item(PopupMenuItem::new(l).on_click(move |_, _, cx| {
                            if let Some(me) = me.upgrade() {
                                me.update(cx, |this, cx| this.set_widget_field(id, &key, json!(v.clone()), cx));
                            }
                        }));
                    }
                    menu
                })
                .into_any_element()
        };
        match f.kind.as_str() {
            "checkbox" => {
                let on = value.as_bool().unwrap_or(false);
                row(super::settings::switch(SharedString::from(format!("wf-{key}")), on)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_widget_field(id, &key, json!(!on), cx)))
                    .into_any_element())
            }
            "dropdown" => {
                let current = shown(&value);
                let label = f.options.iter().find(|(v, _)| *v == current).map_or(current.clone(), |(_, l)| l.clone());
                row(menu(label, f.options.clone(), cx))
            }
            "fontpicker" => {
                let current = shown(&value);
                let mut options: Vec<(String, String)> = FONTS.iter().map(|(n, css)| (css.to_string(), n.to_string())).collect();
                if !FONTS.iter().any(|f| f.1 == current) && !current.is_empty() {
                    options.insert(0, (current.clone(), font_name(&current)));
                }
                row(menu(font_name(&current), options, cx))
            }
            "colorpicker" => wide(self.colour_field(&format!("w-{id}-{}", f.key), cx)),
            "slider" => {
                let (min, max) = (f.min.unwrap_or(0.0) as f32, f.max.unwrap_or(100.0) as f32);
                let step = f.step.unwrap_or(1.0) as f32;
                let v = value.as_f64().unwrap_or(min as f64) as f32;
                let src = source.clone();
                let k = key.clone();
                let slider = self.slider(format!("wf:{id}:{key}"), (min, max, step), v, move |v| widget_command(&src, &k, json!(v)), cx);
                wide(div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(slider))
                    .child(div().w(px(32.)).text_right().text_size(px(11.)).font_family(crate::ui::MONO).text_color(tok::text_tertiary()).child(format!("{v:.0}")))
                    .into_any_element())
            }
            "number" => {
                let (min, max) = (f.min.map(|m| m as f32), f.max.map(|m| m as f32));
                let snap = f.step.unwrap_or(1.0).max(1.0) as f32;
                // a drag across the field's range in about 150 points
                let step = match (min, max) {
                    (Some(lo), Some(hi)) => ((hi - lo) / 150.0).max(0.05),
                    _ => snap * 0.5,
                };
                let v = value.as_f64().unwrap_or(0.0) as f32;
                let src = source.clone();
                let k = key.clone();
                let apply = move |v: f32| {
                    let v = v.max(min.unwrap_or(f32::MIN)).min(max.unwrap_or(f32::MAX));
                    widget_command(&src, &k, json!(v.round() as i64))
                };
                row(self.number_snapped(format!("wf-{id}-{key}"), div().into_any_element(), v, "", step, snap, apply, cx).w(px(80.)).into_any_element())
            }
            "button" => {
                let (widgets, wid) = (self.studio.read(cx).widgets.clone(), instance.id.clone());
                Button::new(SharedString::from(format!("wf-{key}")))
                    .label(f.label.clone())
                    .small()
                    .outline()
                    .w_full()
                    .on_click(move |_, _, _| {
                        if let Some(w) = &widgets {
                            w.button(&wid, &key);
                        }
                    })
                    .into_any_element()
            }
            "textarea" => match self.props.widget.as_ref().filter(|w| w.source == id).and_then(|w| w.areas.iter().find(|(k, _)| *k == f.key)) {
                Some((_, state)) => stacked(
                    div()
                        .rounded(px(5.))
                        .bg(tok::bg_raised())
                        .border_1()
                        .border_color(tok::border())
                        .child(Editor::new(state).h(px(96.)).bordered(false))
                        .into_any_element(),
                ),
                None => div().into_any_element(),
            },
            kind => {
                let Some(state) = self.props.widget.as_ref().filter(|w| w.source == id).and_then(|w| w.inputs.iter().find(|(k, _)| *k == f.key)).map(|(_, s)| s.clone())
                else {
                    return div().into_any_element();
                };
                if !matches!(kind, "image-input" | "sound-input" | "video-input") {
                    return wide(div().flex_1().min_w_0().child(TextInput::new(&state).xsmall()).into_any_element());
                }
                // a picture, sound or video: its file (or address), and a button to choose one
                let pick = Button::new(SharedString::from(format!("wf-pick-{key}"))).label("Choose…").xsmall().outline().on_click(cx.listener(
                    move |_, _, window, cx| {
                        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
                        let key = key.clone();
                        cx.spawn_in(window, async move |this, cx| {
                            let Ok(Ok(Some(paths))) = paths.await else { return };
                            let Some(path) = paths.into_iter().next() else { return };
                            let _ = this.update(cx, |this, cx| this.set_widget_field(id, &key, json!(path.to_string_lossy()), cx));
                        })
                        .detach();
                    },
                ));
                stacked(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().min_w_0().child(TextInput::new(&state).xsmall()))
                        .child(pick)
                        .into_any_element(),
                )
            }
        }
    }
}

/// The settings change that sets a widget's field `key` to `value`.
fn widget_command(source: &Source, key: &str, value: Value) -> Command {
    let mut settings = source.settings.clone();
    if let Some(mut instance) = widget_of(source) {
        if !instance.fields.is_object() {
            instance.fields = json!({});
        }
        instance.fields[key] = value;
        settings["widget"] = json!(instance);
    }
    Command::SetSourceSettings { source: source.id, settings }
}
