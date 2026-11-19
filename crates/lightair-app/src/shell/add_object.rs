//! The Add Object picker (docs/05-interface-shell.md, "Layers"): a sheet with the object types
//! by group in a rail on the left, and the types themselves on the right (each with what it is),
//! searchable and walked with the arrow keys. A type that needs a choice opens a second step in
//! the same sheet: the camera, the display, window or app (each as a thumbnail: `screen_pick`),
//! or a web page's address and size.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input as TextInput, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::StudioWindow;
use crate::ui::{self, tok};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Types,
    Cameras,
    /// A display, window or app (the picker's mode).
    Screens,
    Web,
}

/// The rail's groups of types.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    All,
    Capture,
    Media,
    Graphics,
    Widgets,
}

/// (group, name, icon), in the rail's order.
const GROUPS: &[(Group, &str, &str)] = &[
    (Group::All, "All", "layers"),
    (Group::Capture, "Capture", "video"),
    (Group::Media, "Media", "image"),
    (Group::Graphics, "Graphics", "shapes"),
    (Group::Widgets, "Widgets", "puzzle"),
];

fn group_name(group: Group) -> &'static str {
    GROUPS.iter().find(|g| g.0 == group).map_or("", |g| g.1)
}

pub(super) struct AddObject {
    step: Step,
    /// The group shown (the rail's choice).
    group: Group,
    /// The type highlighted (Enter adds it): an index into those shown.
    sel: usize,
    search: Entity<InputState>,
    /// A web page's address, and its size (CSS pixels).
    web_url: Entity<InputState>,
    web_size: (u32, u32),
    pub(super) screens: Option<super::screen_pick::ScreenPick>,
    /// The types' list (scrolled to the highlight as it moves).
    scroll: ScrollHandle,
    _events: Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Camera,
    Display,
    Window,
    App,
    Image,
    Video,
    Text,
    Colour,
    Timer,
    Web,
}

/// (kind, group, icon, name, what it is), in the order shown.
const KINDS: &[(Kind, Group, &str, &str, &str)] = &[
    (Kind::Camera, Group::Capture, "camera", "Camera", "A webcam, capture card or phone"),
    (Kind::Display, Group::Capture, "monitor", "Display", "Everything on a screen"),
    (Kind::Window, Group::Capture, "app-window", "Window", "One window, even when it's covered"),
    (Kind::App, Group::Capture, "layout-grid", "App", "Every window of one app"),
    (Kind::Web, Group::Capture, "globe", "Browser", "A website, overlay or alert widget, live"),
    (Kind::Image, Group::Media, "image", "Image", "A picture or logo"),
    (Kind::Video, Group::Media, "clapperboard", "Video", "A video file, looped"),
    (Kind::Text, Group::Graphics, "type", "Text", "A title or caption"),
    (Kind::Colour, Group::Graphics, "square", "Colour", "A solid colour block"),
    (Kind::Timer, Group::Graphics, "timer", "Timer", "Countdown, stopwatch or clock"),
];

/// What a type adds.
#[derive(Clone, Copy)]
enum Pick {
    Kind(Kind),
    /// A built-in widget (its kind).
    Widget(&'static str),
    /// A widget brought in from its files.
    Import,
}

/// A type as listed: what it adds, its group (and a widget's gallery group), icon, name, what it is.
#[derive(Clone, Copy)]
struct Entry {
    pick: Pick,
    group: Group,
    sub: Option<&'static str>,
    icon: &'static str,
    name: &'static str,
    about: &'static str,
}

/// A line of the types' list: a group's heading, a widget gallery group's, or a row of types
/// (indices into the entries).
enum Line {
    Heading(&'static str),
    Sub(&'static str),
    Row(usize, Option<usize>),
}

fn matches(query: &str, words: &[&str]) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || words.iter().any(|w| w.to_lowercase().contains(&q))
}

/// The types in `group` (All: every one) matching `query`, in the order shown.
fn entries(query: &str, group: Group) -> Vec<Entry> {
    let shown = |g: Group| group == Group::All || group == g;
    let mut out: Vec<Entry> = KINDS
        .iter()
        .filter(|k| shown(k.1) && matches(query, &[k.3, k.4, group_name(k.1)]))
        .map(|&(kind, group, icon, name, about)| Entry { pick: Pick::Kind(kind), group, sub: None, icon, name, about })
        .collect();
    if shown(Group::Widgets) {
        // Lightair's own by what they're for; then widgets brought in (.lwidget, StreamElements, Streamlabs)
        for &sub in WIDGET_GROUPS {
            for b in lightair_widgets::catalog::BUILTIN.iter().filter(|b| widget_group(b.kind) == sub) {
                if matches(query, &[b.name, b.about, sub, "widget", "overlay", "alert"]) {
                    out.push(Entry { pick: Pick::Widget(b.kind), group: Group::Widgets, sub: Some(sub), icon: b.icon, name: b.name, about: b.about });
                }
            }
        }
        if matches(query, &["Import a widget", "StreamElements", "Streamlabs", "custom widget", "lwidget"]) {
            out.push(Entry {
                pick: Pick::Import,
                group: Group::Widgets,
                sub: Some("Your own"),
                icon: "folder-open",
                name: "Import a widget",
                about: "A .lwidget package, or a StreamElements or Streamlabs widget's folder",
            });
        }
    }
    out
}

/// The list's lines: headings (each group's, when all are shown; each widget gallery group's)
/// and rows of two.
fn lines(entries: &[Entry], headings: bool) -> Vec<Line> {
    let mut out = vec![];
    let mut i = 0;
    while i < entries.len() {
        let (group, sub) = (entries[i].group, entries[i].sub);
        if headings && (i == 0 || entries[i - 1].group != group) {
            out.push(Line::Heading(group_name(group)));
        }
        if let Some(sub) = sub {
            out.push(Line::Sub(sub));
        }
        let end = (i..entries.len()).find(|&j| entries[j].group != group || entries[j].sub != sub).unwrap_or(entries.len());
        for r in (i..end).step_by(2) {
            out.push(Line::Row(r, (r + 1 < end).then_some(r + 1)));
        }
        i = end;
    }
    out
}

/// The highlight moved a column (`dx`) or a row (`dy`, keeping its column where it can), and the
/// line to bring into view.
fn moved(lines: &[Line], sel: usize, count: usize, dx: isize, dy: isize) -> (usize, usize) {
    let rows: Vec<(usize, usize, Option<usize>)> = lines
        .iter()
        .enumerate()
        .filter_map(|(ix, l)| match l {
            Line::Row(a, b) => Some((ix, *a, *b)),
            _ => None,
        })
        .collect();
    let row_of = |e: usize| rows.iter().position(|&(_, a, b)| a == e || b == Some(e)).unwrap_or(0);
    let next = if dy != 0 {
        let r = row_of(sel);
        let right = rows[r].1 != sel;
        let (_, a, b) = rows[(r as isize + dy).clamp(0, rows.len() as isize - 1) as usize];
        if right { b.unwrap_or(a) } else { a }
    } else {
        (sel as isize + dx).clamp(0, count as isize - 1) as usize
    };
    // the first row: from the top, so its heading shows too
    let r = row_of(next);
    (next, if r == 0 { 0 } else { rows[r].0 })
}

impl StudioWindow {
    pub(super) fn open_add_object(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_object.take().is_some() {
            cx.notify();
            return;
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search objects and widgets"));
        let web_url = cx.new(|cx| InputState::new(window, cx).placeholder("https://…"));
        search.update(cx, |s, cx| s.focus(window, cx));
        let events = cx.subscribe_in(&search, window, |this, search, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                let typed = !search.read(cx).value().trim().is_empty();
                if let Some(a) = &mut this.add_object {
                    // searching looks everywhere
                    if typed && a.step == Step::Types {
                        a.group = Group::All;
                    }
                    a.sel = 0;
                    a.scroll.set_offset(point(px(0.), px(0.)));
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.add_highlighted(window, cx),
            _ => {}
        });
        self.add_object = Some(AddObject {
            step: Step::Types,
            group: Group::All,
            sel: 0,
            search,
            web_url,
            web_size: (1920, 1080),
            screens: None,
            scroll: ScrollHandle::new(),
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_add_object(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_object = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Opens the display, window or app step (for screenshots).
    pub(super) fn pick_screen(&mut self, which: &str, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match which {
            "window" => Kind::Window,
            "app" => Kind::App,
            _ => Kind::Display,
        };
        self.pick(kind, window, cx);
    }

    /// Moves the sheet to `step`, with the search cleared and saying what it searches.
    fn add_object_step(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = &mut self.add_object else { return };
        a.step = step;
        if step != Step::Screens {
            a.screens = None;
        }
        let placeholder = match (step, a.screens.as_ref().map(|p| p.mode)) {
            (Step::Cameras, _) => "Search cameras",
            (Step::Screens, Some(lightair_core::kinds::DISPLAY)) => "Search displays",
            (Step::Screens, Some(lightair_core::kinds::APP)) => "Search apps",
            (Step::Screens, _) => "Search windows",
            _ => "Search objects and widgets",
        };
        let (search, web_url) = (a.search.clone(), a.web_url.clone());
        search.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.set_placeholder(placeholder, window, cx);
        });
        if step == Step::Web {
            web_url.update(cx, |i, cx| i.focus(window, cx));
        } else {
            search.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    /// For screenshots: shows a group ("widgets"), types a search ("?chat") or moves the
    /// highlight ("down", "right"), each separated by commas.
    pub(super) fn add_object_hook(&mut self, hook: &str, window: &mut Window, cx: &mut Context<Self>) {
        for part in hook.split(',') {
            if let Some(q) = part.strip_prefix('?') {
                if let Some(search) = self.add_object.as_ref().map(|a| a.search.clone()) {
                    search.update(cx, |s, cx| s.set_value(q, window, cx));
                }
            } else if let Some(&(group, ..)) = GROUPS.iter().find(|g| g.1.eq_ignore_ascii_case(part)) {
                self.show_group(group, window, cx);
            } else if let Some((dx, dy)) = match part {
                "left" => Some((-1, 0)),
                "right" => Some((1, 0)),
                "up" => Some((0, -1)),
                "down" => Some((0, 1)),
                _ => None,
            } {
                self.move_highlight(dx, dy, cx);
            } else if part == "web" {
                self.pick(Kind::Web, window, cx);
            } else if part == "camera" {
                self.pick(Kind::Camera, window, cx);
            }
        }
    }

    /// Shows a group of types (from any step).
    fn show_group(&mut self, group: Group, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_object.as_ref().is_some_and(|a| a.step != Step::Types) {
            self.add_object_step(Step::Types, window, cx);
        }
        if let Some(a) = &mut self.add_object {
            a.group = group;
            a.sel = 0;
            a.scroll.set_offset(point(px(0.), px(0.)));
        }
        cx.notify();
    }

    fn pick(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        match kind {
            Kind::Camera => {
                self.camera_devices = self.studio.read(cx).engine().call(|_| lightair_engine::kinds::cameras());
                self.add_object_step(Step::Cameras, window, cx);
                return;
            }
            Kind::Display | Kind::Window | Kind::App => {
                use lightair_core::kinds::{APP, DISPLAY, WINDOW};
                let mode = match kind {
                    Kind::Display => DISPLAY,
                    Kind::Window => WINDOW,
                    _ => APP,
                };
                let pick = super::screen_pick::ScreenPick::open(mode, self.studio.read(cx).engine());
                if let Some(a) = &mut self.add_object {
                    a.screens = Some(pick);
                }
                self.add_object_step(Step::Screens, window, cx);
                return;
            }
            Kind::Image => self.add_image(window, cx),
            Kind::Video => self.add_video(window, cx),
            Kind::Text => self.add_text(cx),
            Kind::Colour => self.add_colour(cx),
            Kind::Timer => self.add_timer(cx),
            Kind::Web => {
                self.add_object_step(Step::Web, window, cx);
                return;
            }
        }
        self.close_add_object(window, cx);
    }

    fn add_entry(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        match pick {
            Pick::Kind(kind) => self.pick(kind, window, cx),
            Pick::Widget(kind) => self.add_widget(kind, None, window, cx),
            Pick::Import => self.import_widget(window, cx),
        }
    }

    /// The types shown now.
    fn add_object_entries(&self, cx: &App) -> Vec<Entry> {
        let Some(a) = &self.add_object else { return vec![] };
        entries(&a.search.read(cx).value(), a.group)
    }

    /// Adds the highlighted type (Enter).
    fn add_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = &self.add_object else { return };
        if a.step != Step::Types {
            return;
        }
        let sel = a.sel;
        if let Some(e) = self.add_object_entries(cx).get(sel) {
            self.add_entry(e.pick, window, cx);
        }
    }

    /// The arrow keys move the highlight; Escape closes.
    fn add_object_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (dx, dy) = match ev.keystroke.key.as_str() {
            "escape" => {
                self.close_add_object(window, cx);
                cx.stop_propagation();
                return;
            }
            "left" => (-1, 0),
            "right" => (1, 0),
            "up" => (0, -1),
            "down" => (0, 1),
            _ => return,
        };
        if self.add_object.as_ref().is_none_or(|a| a.step != Step::Types) {
            return;
        }
        cx.stop_propagation();
        self.move_highlight(dx, dy, cx);
    }

    /// Moves the highlight a column (`dx`) or a row (`dy`), bringing it into view.
    fn move_highlight(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) {
        let entries = self.add_object_entries(cx);
        let Some(a) = &mut self.add_object else { return };
        if entries.is_empty() {
            return;
        }
        let lines = lines(&entries, a.group == Group::All);
        let (sel, line) = moved(&lines, a.sel.min(entries.len() - 1), entries.len(), dx, dy);
        a.sel = sel;
        a.scroll.scroll_to_item(line);
        cx.notify();
    }

    /// Adds a widget: a built-in one, or an imported one (`custom`).
    pub(super) fn add_widget(&mut self, kind: &str, custom: Option<lightair_widgets::Custom>, window: &mut Window, cx: &mut Context<Self>) {
        let builtin = lightair_widgets::catalog::by_kind(kind);
        let (w, h) = builtin.map_or((800, 600), |b| b.size);
        let name = builtin.map_or("Widget", |b| b.name).to_string();
        let instance = lightair_widgets::Instance::new(kind, custom);
        let settings = serde_json::json!({"url": instance.address(), "width": w, "height": h, "fps": 30, "widget": instance});
        let (place, sound) = builtin.map_or(((0.5, 0.5), true), |b| (b.place, b.sound));
        self.add_placed_object(&name, lightair_engine::web::KIND, settings, (w as f32, h as f32), place, sound, cx);
        self.close_add_object(window, cx);
    }

    /// Brings in a StreamElements custom widget from a folder of its files.
    fn import_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: true, multiple: false, prompt: Some("Import widget".into()) });
        self.close_add_object(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| match lightair_widgets::import(&dir) {
                Ok(custom) => this.add_widget("custom", Some(custom), window, cx),
                Err(e) => this.studio.update(cx, |s, cx| {
                    s.issue = Some(format!("Couldn't import that widget: {e}").into());
                    cx.notify();
                }),
            });
        })
        .detach();
    }

    /// Adds a capture of the display, window or app chosen (`size`: its picture's, if known).
    fn add_screen(&mut self, mode: i64, name: &str, value: &serde_json::Value, size: Option<(u32, u32)>, window: &mut Window, cx: &mut Context<Self>) {
        let settings = super::screen_pick::settings(mode, value);
        let (w, h) = size.map_or((1920.0, 1080.0), |(w, h)| (w as f32, h as f32));
        let name: String = name.chars().take(48).collect();
        self.add_source_object(&name, lightair_core::kinds::SCREEN, settings, (w, h), cx);
        self.close_add_object(window, cx);
    }

    /// Moves the screen picker on (new captures, stills). Called each frame from render.
    pub(super) fn tick_add_object(&mut self) {
        if let Some(pick) = self.add_object.as_mut().and_then(|a| a.screens.as_mut()) {
            pick.tick();
        }
    }

    /// Adds the web page typed in (an address without a scheme gets https://).
    fn add_web_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = &self.add_object else { return };
        let typed = a.web_url.read(cx).value().trim().to_string();
        if typed.is_empty() {
            return;
        }
        let url = super::object_props::web_address(&typed);
        let (w, h) = a.web_size;
        let name = url.split("://").nth(1).and_then(|r| r.split('/').next()).filter(|h| !h.is_empty()).unwrap_or("Browser").to_string();
        let settings = serde_json::json!({"url": url, "width": w, "height": h, "fps": 30});
        self.add_source_object(&name, lightair_engine::web::KIND, settings, (w as f32, h as f32), cx);
        self.close_add_object(window, cx);
    }

    pub(super) fn add_object_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.add_object.as_ref()?;
        let query = state.search.read(cx).value().to_string();
        let studio = self.studio.read(cx);
        let scene: Option<String> = studio.selected_scene.and_then(|id| studio.workspace().scene(id)).map(|s| s.name.clone());
        let step = state.step;
        let screen_mode = state.screens.as_ref().map(|p| p.mode);

        // the rail: the groups (with how many types each shows), and the scene they're added to
        let mut rail = div().flex().flex_col().flex_none().w(px(184.)).p_2().gap(px(2.)).border_r_1().border_color(tok::border());
        for &(group, name, icon) in GROUPS {
            let on = step == Step::Types && state.group == group;
            let count = entries(&query, group).len();
            rail = rail.child(
                div()
                    .id(("add-object-group", group as usize))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(30.))
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                    .when(!on, |d| d.text_color(tok::text_secondary()).hover(|d| d.bg(tok::bg_raised()).text_color(tok::text())))
                    .child(ui::icon(icon).size(px(15.)))
                    .child(div().flex_1().child(name))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(if count == 0 { tok::border_strong() } else { tok::text_tertiary() })
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.show_group(group, window, cx))),
            );
        }
        rail = rail.child(div().flex_1()).when_some(scene, |d, scene| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px_2()
                    .py_1p5()
                    .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("Adds to"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(tok::text_secondary())
                            .child(ui::icon("layout-dashboard").size(px(13.)).flex_none())
                            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(scene)),
                    ),
            )
        });

        let list = || div().id("add-object-list").flex().flex_col().flex_1().min_h_0().min_w_0().overflow_y_scroll().p_3();
        let content: AnyElement = match step {
            Step::Types => {
                let shown = entries(&query, state.group);
                let sel = state.sel.min(shown.len().saturating_sub(1));
                let mut body = list().track_scroll(&state.scroll).gap(px(4.));
                if shown.is_empty() {
                    body = body.child(empty_state(
                        "search",
                        format!("Nothing matches “{}”", query.trim()),
                        "Try “camera”, “chat” or “timer”",
                    ));
                }
                let lines = lines(&shown, state.group == Group::All);
                for (ix, line) in lines.iter().enumerate() {
                    body = body.child(match *line {
                        Line::Heading(text) => div()
                            .pt(px(if ix == 0 { 0. } else { 14. }))
                            .pb(px(2.))
                            .px_1()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tok::text_tertiary())
                            .child(text.to_uppercase())
                            .into_any_element(),
                        Line::Sub(text) => div()
                            .pt(px(if ix == 0 { 0. } else { 8. }))
                            .px_1()
                            .text_size(px(12.))
                            .text_color(tok::text_secondary())
                            .child(text)
                            .into_any_element(),
                        Line::Row(a, b) => {
                            let card = |i: usize| {
                                let e = shown[i];
                                tile(("add-object-type", i), e.icon, e.name, e.about, Some(i == sel))
                                    .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                                        if let Some(a) = this.add_object.as_mut().filter(|a| a.sel != i) {
                                            a.sel = i;
                                            cx.notify();
                                        }
                                    }))
                                    .on_click(cx.listener(move |this, _, window, cx| this.add_entry(e.pick, window, cx)))
                            };
                            div()
                                .flex()
                                .gap(px(4.))
                                .child(card(a))
                                .map(|d| match b {
                                    Some(b) => d.child(card(b)),
                                    None => d.child(div().flex_1()),
                                })
                                .into_any_element()
                        }
                    });
                }
                body.into_any_element()
            }
            Step::Web => {
                let size = state.web_size;
                let mut sizes = div().flex().gap(px(2.)).p(px(2.)).rounded(px(7.)).bg(tok::bg_app()).border_1().border_color(tok::border());
                for (i, (label, detail, wh)) in [
                    ("Landscape", "1920 × 1080", (1920u32, 1080u32)),
                    ("HD", "1280 × 720", (1280, 720)),
                    ("Vertical", "1080 × 1920", (1080, 1920)),
                    ("Square", "1080 × 1080", (1080, 1080)),
                ]
                .into_iter()
                .enumerate()
                {
                    let on = size == wh;
                    sizes = sizes.child(
                        div()
                            .id(("web-size", i))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(1.))
                            .h(px(44.))
                            .rounded(px(5.))
                            .cursor_pointer()
                            .when(on, |d| d.bg(tok::bg_selected()))
                            .when(!on, |d| d.hover(|d| d.bg(tok::bg_raised())))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if on { tok::text() } else { tok::text_secondary() })
                                    .child(label),
                            )
                            .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(detail))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(a) = &mut this.add_object {
                                    a.web_size = wh;
                                }
                                cx.notify();
                            })),
                    );
                }
                let field = |label: &'static str, hint: &'static str, control: AnyElement| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap_2()
                                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(tok::text()).child(label))
                                .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(hint)),
                        )
                        .child(control)
                };
                list()
                    .gap_5()
                    .child(field(
                        "Address",
                        "A site, or an overlay or alert widget's link",
                        div()
                            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                                if ev.keystroke.key == "enter" {
                                    this.add_web_page(window, cx);
                                }
                            }))
                            .child(TextInput::new(&state.web_url))
                            .into_any_element(),
                    ))
                    .child(field("Size", "The page's size, in pixels", sizes.into_any_element()))
                    .child(div().flex_1())
                    .child(
                        div().flex().justify_end().child(
                            Button::new("add-web-page")
                                .label("Add browser")
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| this.add_web_page(window, cx))),
                        ),
                    )
                    .into_any_element()
            }
            Step::Screens => {
                use lightair_core::kinds::{APP, DISPLAY};
                let pick = state.screens.as_ref()?;
                let mode = pick.mode;
                let (icon, what) = match mode {
                    DISPLAY => ("monitor", "displays"),
                    APP => ("layout-grid", "apps with windows open"),
                    _ => ("app-window", "windows open"),
                };
                let shown: Vec<_> = pick
                    .items
                    .iter()
                    .zip(&pick.thumbs)
                    .enumerate()
                    .filter(|(_, (item, _))| matches(&query, &[&item.title, &item.detail]))
                    .collect();
                let mut body = list();
                if !pick.listed {
                    body = body.child(empty_state(icon, "Looking…".into(), ""));
                } else if shown.is_empty() {
                    body = body.child(if query.trim().is_empty() {
                        empty_state(icon, format!("No {what}"), "")
                    } else {
                        empty_state("search", format!("Nothing matches “{}”", query.trim()), "")
                    });
                }
                if mode != DISPLAY {
                    // windows and apps: by name
                    let mut grid = div().grid().grid_cols(2).gap(px(4.));
                    for (i, (item, _)) in shown {
                        let (title, value) = (item.title.clone(), item.value.clone());
                        grid = grid.child(
                            tile(("screen", i), icon, item.title.clone(), item.detail.clone(), None)
                                .on_click(cx.listener(move |this, _, window, cx| this.add_screen(mode, &title, &value, None, window, cx))),
                        );
                    }
                    body = body.child(grid);
                } else {
                    // displays: each as a picture, in its own shape
                    let mut grid = div().grid().grid_cols(2).gap_3();
                    for (i, (item, thumb)) in shown {
                        let size = thumb.size;
                        let ratio = size.map_or(10.0 / 16.0, |(w, h)| h as f32 / w.max(1) as f32);
                        let detail = match (size, item.main) {
                            (Some((w, h)), main) => format!("{w} × {h}{}", if main { " · Main" } else { "" }),
                            (None, true) => "Main".into(),
                            (None, false) => String::new(),
                        };
                        let (title, value) = (item.title.clone(), item.value.clone());
                        grid = grid.child(
                            div()
                                .id(("screen", i))
                                .group("screen")
                                .flex()
                                .flex_col()
                                .gap_2()
                                .p_2()
                                .min_w_0()
                                .rounded(px(10.))
                                .border_1()
                                .border_color(transparent_black())
                                .cursor_pointer()
                                .hover(|d| d.bg(tok::bg_raised()).border_color(tok::border_strong()))
                                .child(
                                    div()
                                        .relative()
                                        .w_full()
                                        .pb(relative(ratio))
                                        .rounded(px(6.))
                                        .overflow_hidden()
                                        .bg(tok::bg_app())
                                        .border_1()
                                        .border_color(tok::border())
                                        .child(match thumb.frame() {
                                            Some(f) => crate::frame_view::frame(f).absolute().size_full().object_fit(ObjectFit::Contain).into_any_element(),
                                            None => div()
                                                .absolute()
                                                .size_full()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(ui::icon(icon).size(px(20.)).text_color(tok::text_tertiary()))
                                                .into_any_element(),
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .justify_between()
                                        .gap_2()
                                        .min_w_0()
                                        .px_0p5()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(item.title.clone()),
                                        )
                                        .child(div().flex_none().text_size(px(11.)).text_color(tok::text_tertiary()).child(detail)),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| this.add_screen(mode, &title, &value, size, window, cx))),
                        );
                    }
                    body = body.child(grid);
                }
                body.into_any_element()
            }
            Step::Cameras => {
                let cams: Vec<_> = self.camera_devices.iter().filter(|(label, _)| matches(&query, &[label])).cloned().collect();
                let mut body = list();
                if cams.is_empty() {
                    body = body.child(if query.trim().is_empty() {
                        empty_state("camera-off", "No cameras found".into(), "Connect a webcam, capture card or phone")
                    } else {
                        empty_state("search", format!("Nothing matches “{}”", query.trim()), "")
                    });
                }
                let mut grid = div().grid().grid_cols(2).gap(px(4.));
                for (i, (label, uid)) in cams.into_iter().enumerate() {
                    // (by the device's transport type, not its name)
                    let hint = if lightair_platform::devices::camera_is_virtual(&uid) { "Virtual camera" } else { "Camera" };
                    grid = grid.child(tile(("camera", i), "camera", label.clone(), hint, None).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.add_camera(&label, &uid, cx);
                            this.close_add_object(window, cx);
                        },
                    )));
                }
                body.child(grid).into_any_element()
            }
        };

        // the header: back (in a second step) and what it's choosing, then the search
        let title = match step {
            Step::Types => None,
            Step::Cameras => Some("Camera"),
            Step::Screens => Some(match screen_mode {
                Some(lightair_core::kinds::DISPLAY) => "Display",
                Some(lightair_core::kinds::APP) => "App",
                _ => "Window",
            }),
            Step::Web => Some("Browser"),
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(52.))
            .pl_3()
            .pr_3()
            .border_b_1()
            .border_color(tok::border())
            .when_some(title, |d, title| {
                d.child(
                    div()
                        .id("add-object-back")
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .size(px(26.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_color(tok::text_secondary())
                        .hover(|d| d.bg(tok::bg_raised()).text_color(tok::text()))
                        .child(ui::icon("chevron-left").size(px(16.)))
                        .tooltip(ui::tip("All objects"))
                        .on_click(cx.listener(|this, _, window, cx| this.add_object_step(Step::Types, window, cx))),
                )
                .child(div().flex_none().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .when(step != Step::Web, |d| d.child(div().flex_none().w(px(1.)).h(px(18.)).mx_1().bg(tok::border())))
            })
            .map(|d| {
                if step == Step::Web {
                    d.child(div().flex_1())
                } else {
                    d.child(
                        div().flex_1().min_w_0().child(
                            TextInput::new(&state.search)
                                .appearance(false)
                                .prefix(ui::icon("search").size(px(15.)).text_color(tok::text_tertiary()))
                                .cleanable(true),
                        ),
                    )
                }
            })
            .child(key_cap("esc"));

        // the footer: the keys
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_4()
            .h(px(34.))
            .px_3()
            .border_t_1()
            .border_color(tok::border())
            .bg(tok::bg_app().opacity(0.35))
            .map(|d| match step {
                Step::Types => d.child(key_hint("↑ ↓ ← →", "Move")).child(key_hint("↵", "Add")).child(key_hint("esc", "Close")),
                Step::Web => d.child(key_hint("↵", "Add")).child(key_hint("esc", "Close")),
                _ => d.child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("Click one to add it")).child(key_hint("esc", "Close")),
            });

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(72.))
                .pb(px(24.))
                .bg(tok::scrim())
                // a sheet: nothing behind it takes the mouse (scrolling here never pans the canvas)
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_add_object(window, cx)))
                .child(
                    div()
                        .id("add-object")
                        .flex()
                        .flex_col()
                        .w(px(760.))
                        .h(px(560.))
                        .flex_shrink(1.)
                        .min_h(px(320.))
                        .overflow_hidden()
                        .rounded(px(12.))
                        .bg(tok::bg_panel())
                        .border_1()
                        .border_color(tok::border_strong())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.add_object_key(ev, window, cx)))
                        .child(header)
                        .child(div().flex().flex_1().min_h_0().child(rail).child(content))
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}

/// A type, camera, window or app: its icon in a well, its name, and what it is. `on`: whether
/// it's highlighted, for a list walked by keys (where the pointer moves the highlight too);
/// `None` for one that just lights up under the pointer.
fn tile(id: impl Into<ElementId>, icon: &str, name: impl Into<SharedString>, about: impl Into<SharedString>, on: Option<bool>) -> Stateful<Div> {
    let (hover, on) = (on.is_none(), on.unwrap_or(false));
    div()
        .id(id)
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(10.))
        .min_h(px(54.))
        .px(px(8.))
        .py(px(7.))
        .rounded(px(8.))
        .border_1()
        .cursor_pointer()
        .when(on, |d| d.bg(tok::bg_raised()).border_color(tok::border_strong()))
        .when(!on, |d| d.border_color(transparent_black()))
        .when(hover, |d| d.hover(|d| d.bg(tok::bg_raised())))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(34.))
                .rounded(px(8.))
                .bg(if on { tok::bg_selected() } else { tok::bg_raised() })
                .border_1()
                .border_color(tok::border())
                .child(ui::icon(icon).size(px(16.)).text_color(tok::text())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(tok::text())
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(name.into()),
                )
                .child(
                    // up to two lines (a widget's runs long)
                    div()
                        .text_size(px(11.5))
                        .line_height(px(15.))
                        .text_color(tok::text_tertiary())
                        .line_clamp(2)
                        .child(about.into()),
                ),
        )
}

/// Nothing to show: an icon, what's missing, and what to try.
fn empty_state(icon: &str, title: String, hint: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .py(px(64.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(40.))
                .rounded(px(10.))
                .bg(tok::bg_raised())
                .child(ui::icon(icon).size(px(18.)).text_color(tok::text_tertiary())),
        )
        .child(div().text_size(px(13.)).text_color(tok::text_secondary()).child(title))
        .when(!hint.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(tok::text_tertiary()).child(hint)))
}

/// A key, drawn as a key cap.
fn key_cap(keys: &'static str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(20.))
        .h(px(20.))
        .px_1p5()
        .rounded(px(4.))
        .bg(tok::bg_raised())
        .border_1()
        .border_color(tok::border())
        .text_size(px(10.5))
        .text_color(tok::text_secondary())
        .child(keys)
}

/// A key and what it does.
fn key_hint(keys: &'static str, label: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(key_cap(keys))
        .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(label))
}

/// The widget gallery's groups, in order.
const WIDGET_GROUPS: &[&str] = &["Alerts and events", "Chat", "Goals and timers", "Viewers play", "Info"];

/// Which gallery group a built-in widget is in.
fn widget_group(kind: &str) -> &'static str {
    match kind {
        "alert-box" | "event-list" | "credits" | "featured" => "Alerts and events",
        "chat-box" | "emote-wall" | "emote-combo" => "Chat",
        "goal" | "counter" | "subathon" => "Goals and timers",
        "poll" | "prediction" | "wheel" | "leaderboard" => "Viewers play",
        _ => "Info",
    }
}
