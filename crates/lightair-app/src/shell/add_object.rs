//! The Add Object picker (docs/05-interface-shell.md, "Layers"): searchable object types as
//! tiles in groups (what each is, on hover), with a second step where a type needs one: the
//! camera, the display, window or app (each as a thumbnail: `screen_pick`), or a web page's
//! address and size.

use gpui_kit::component::input::{Input as TextInput, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::Sizable;

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

pub(super) struct AddObject {
    step: Step,
    search: Entity<InputState>,
    /// A web page's address, and its size (CSS pixels).
    web_url: Entity<InputState>,
    web_size: (u32, u32),
    pub(super) screens: Option<super::screen_pick::ScreenPick>,
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

/// (kind, group, icon, name, what it is)
const KINDS: &[(Kind, &str, &str, &str, &str)] = &[
    (Kind::Camera, "Capture", "camera", "Camera", "A webcam, capture card or phone"),
    (Kind::Display, "Capture", "monitor", "Display", "Everything on a screen"),
    (Kind::Window, "Capture", "app-window", "Window", "One window, even when others cover it"),
    (Kind::App, "Capture", "layout-grid", "App", "Every window of one app"),
    (Kind::Web, "Capture", "globe", "Browser", "A website, an overlay or an alert widget, live"),
    (Kind::Image, "Media", "image", "Image", "A picture or logo"),
    (Kind::Video, "Media", "clapperboard", "Video", "A video file, looped"),
    (Kind::Text, "Graphics", "type", "Text", "A title or caption"),
    (Kind::Colour, "Graphics", "square", "Colour", "A solid colour block"),
    (Kind::Timer, "Graphics", "timer", "Timer", "Countdown, stopwatch or clock"),
];

fn matches(query: &str, words: &[&str]) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || words.iter().any(|w| w.to_lowercase().contains(&q))
}

impl StudioWindow {
    pub(super) fn open_add_object(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_object.take().is_some() {
            cx.notify();
            return;
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let web_url = cx.new(|cx| InputState::new(window, cx).placeholder("https://…"));
        search.update(cx, |s, cx| s.focus(window, cx));
        let events = cx.subscribe_in(&search, window, |_, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        });
        self.add_object = Some(AddObject { step: Step::Types, search, web_url, web_size: (1920, 1080), screens: None, _events: events });
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

    fn pick(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        match kind {
            Kind::Camera => {
                self.camera_devices = self.studio.read(cx).engine().call(|_| lightair_engine::kinds::cameras());
                if let Some(a) = &mut self.add_object {
                    a.step = Step::Cameras;
                }
                cx.notify();
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
                    a.step = Step::Screens;
                    a.screens = Some(pick);
                }
                cx.notify();
                return;
            }
            Kind::Image => self.add_image(window, cx),
            Kind::Video => self.add_video(window, cx),
            Kind::Text => self.add_text(cx),
            Kind::Colour => self.add_colour(cx),
            Kind::Timer => self.add_timer(cx),
            Kind::Web => {
                if let Some(a) = &mut self.add_object {
                    a.step = Step::Web;
                    a.web_url.update(cx, |i, cx| i.focus(window, cx));
                }
                cx.notify();
                return;
            }
        }
        self.close_add_object(window, cx);
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
        let workspace = studio.workspace().clone();
        let scene = studio.selected_scene.and_then(|id| workspace.scene(id)).cloned();

        let card = |id: ElementId, icon: &'static str, name: String, hint: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_3()
                .p_2()
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|d| d.bg(tok::bg_selected()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .size(px(36.))
                        .rounded(px(8.))
                        .bg(tok::bg_app())
                        .child(ui::icon(icon).size(px(18.)).text_color(tok::text())),
                )
                .child(
                    div().flex().flex_col().min_w_0().child(div().font_weight(FontWeight::MEDIUM).child(name)).child(
                        div()
                            .text_size(px(11.))
                            .text_color(tok::text_tertiary())
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(hint),
                    ),
                )
        };
        // a type: its icon in a well, its name under it
        let tile = |id: ElementId, icon: &'static str, name: &'static str| {
            div()
                .id(id)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .h(px(84.))
                .rounded(px(10.))
                .bg(tok::bg_app())
                .border_1()
                .border_color(tok::border())
                .cursor_pointer()
                .hover(|d| d.bg(tok::bg_selected()).border_color(tok::border_strong()))
                .child(ui::icon(icon).size(px(22.)).text_color(tok::text()))
                .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child(name))
        };
        let heading = |text: &str| {
            div()
                .pt_2()
                .pb_1()
                .px_2()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tok::text_tertiary())
                .child(text.to_uppercase())
        };

        // back to the types
        let back = |cx: &mut Context<Self>| {
            div()
                .id("add-object-back")
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(tok::text_secondary())
                .hover(|d| d.text_color(tok::text()))
                .child(ui::icon("chevron-left").size(px(14.)))
                .child("All types")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(a) = &mut this.add_object {
                        a.step = Step::Types;
                        a.screens = None;
                    }
                    cx.notify();
                }))
        };
        let mut body =
            div().id("add-object-body").flex().flex_col().flex_shrink(1.).min_h_0().overflow_y_scroll().px_2();
        match state.step {
            Step::Types => {
                for group in ["Capture", "Media", "Graphics"] {
                    let kinds: Vec<_> =
                        KINDS.iter().filter(|k| k.1 == group && matches(&query, &[k.3, k.4, k.1])).collect();
                    if kinds.is_empty() {
                        continue;
                    }
                    let mut grid = div().grid().grid_cols(3).gap_2();
                    for (i, (kind, _, icon, name, hint)) in kinds.into_iter().enumerate() {
                        let kind = *kind;
                        grid = grid.child(
                            tile((group, i).into(), icon, name)
                                .tooltip(ui::tip(*hint))
                                .on_click(cx.listener(move |this, _, window, cx| this.pick(kind, window, cx))),
                        );
                    }
                    body = body.child(heading(group)).child(grid);
                }
                // the widget gallery: Lightair's own by what they're for, each with what it does;
                // and widgets brought in (.lwidget, StreamElements, Streamlabs)
                let builtins: Vec<_> = lightair_widgets::catalog::BUILTIN
                    .iter()
                    .filter(|b| matches(&query, &[b.name, b.about, "widget", "overlay", "alert", widget_group(b.kind)]))
                    .collect();
                let import = matches(&query, &["StreamElements", "Streamlabs", "custom widget", "import", "widget"]);
                if !builtins.is_empty() || import {
                    body = body.child(heading("Widgets"));
                    for group in WIDGET_GROUPS {
                        let members: Vec<_> = builtins.iter().filter(|b| widget_group(b.kind) == *group).collect();
                        if members.is_empty() {
                            continue;
                        }
                        let mut grid = div().grid().grid_cols(2).gap_1();
                        for b in members {
                            let kind = b.kind;
                            grid = grid.child(
                                card(SharedString::from(format!("widget-{kind}")).into(), b.icon, b.name.to_string(), b.about.to_string())
                                    .on_click(cx.listener(move |this, _, window, cx| this.add_widget(kind, None, window, cx))),
                            );
                        }
                        body = body.child(div().px_2().pt_1().text_size(px(11.)).text_color(tok::text_tertiary()).child(*group)).child(grid);
                    }
                    if import {
                        body = body.child(
                            card(
                                "widget-import".into(),
                                "folder-open",
                                "Import a widget".into(),
                                "A .lwidget package, or a StreamElements or Streamlabs custom widget's folder".into(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.import_widget(window, cx))),
                        );
                    }
                }
            }
            Step::Web => {
                let size = state.web_size;
                let mut sizes = div().flex().flex_wrap().gap(px(2.)).p(px(2.)).rounded(px(7.)).bg(tok::bg_app());
                for (i, (label, wh)) in [("Landscape 1920×1080", (1920u32, 1080u32)), ("1280×720", (1280, 720)), ("Vertical 1080×1920", (1080, 1920)), ("Square 1080×1080", (1080, 1080))]
                    .into_iter()
                    .enumerate()
                {
                    let on = size == wh;
                    sizes = sizes.child(
                        div()
                            .id(("web-size", i))
                            .px_2p5()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()).font_weight(FontWeight::MEDIUM))
                            .when(!on, |d| d.text_color(tok::text_secondary()).hover(|d| d.text_color(tok::text())))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(a) = &mut this.add_object {
                                    a.web_size = wh;
                                }
                                cx.notify();
                            })),
                    );
                }
                body = body
                    .child(back(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .p_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1p5()
                                    .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child("Address"))
                                    .child(
                                        div().on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                                            if ev.keystroke.key == "enter" {
                                                this.add_web_page(window, cx);
                                            }
                                        }))
                                        .child(TextInput::new(&state.web_url)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1p5()
                                    .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child("Size"))
                                    .child(sizes),
                            )
                            .child(
                                div().flex().justify_end().child(
                                    Button::new("add-web-page")
                                        .label("Add browser")
                                        .small()
                                        .primary()
                                        .on_click(cx.listener(|this, _, window, cx| this.add_web_page(window, cx))),
                                ),
                            ),
                    );
            }
            Step::Screens => {
                use lightair_core::kinds::{APP, DISPLAY};
                body = body.child(back(cx));
                let pick = state.screens.as_ref()?;
                let mode = pick.mode;
                let (icon, nothing, cols) = match mode {
                    DISPLAY => ("monitor", "No displays found", 2),
                    APP => ("layout-grid", "No apps with windows open", 3),
                    _ => ("app-window", "No windows open", 3),
                };
                let shown: Vec<_> = pick
                    .items
                    .iter()
                    .zip(&pick.thumbs)
                    .enumerate()
                    .filter(|(_, (item, _))| matches(&query, &[&item.title, &item.detail]))
                    .collect();
                if !pick.listed {
                    body = body.child(div().p_3().text_color(tok::text_tertiary()).child("Looking…"));
                } else if shown.is_empty() {
                    body = body.child(div().p_3().text_color(tok::text_tertiary()).child(nothing));
                }
                if mode != DISPLAY {
                    // windows and apps: a plain list, by name
                    let mut grid = div().grid().grid_cols(2).gap_1().pb_2();
                    for (i, (item, _)) in shown {
                        let (title, value) = (item.title.clone(), item.value.clone());
                        grid = grid.child(card(("screen", i).into(), icon, item.title.clone(), item.detail.clone()).on_click(
                            cx.listener(move |this, _, window, cx| this.add_screen(mode, &title, &value, None, window, cx)),
                        ));
                    }
                    body = body.child(grid);
                } else {
                    let mut grid = div().grid().grid_cols(cols).gap_2().pb_2();
                    for (i, (item, thumb)) in shown {
                        let size = thumb.size;
                        // a display in its own shape; windows and apps in one shape, so rows line up
                        let ratio = match (mode, size) {
                            (DISPLAY, Some((w, h))) => h as f32 / w.max(1) as f32,
                            _ => 10.0 / 16.0,
                        };
                        let detail = match (mode, size, item.main) {
                            (DISPLAY, Some((w, h)), main) => format!("{w} × {h}{}", if main { " · Main" } else { "" }),
                            (DISPLAY, None, true) => "Main".into(),
                            _ => item.detail.clone(),
                        };
                        let (title, value) = (item.title.clone(), item.value.clone());
                        grid = grid.child(
                            div()
                                .id(("screen", i))
                                .flex()
                                .flex_col()
                                .gap_1p5()
                                .p_1p5()
                                .min_w_0()
                                .rounded(px(10.))
                                .cursor_pointer()
                                .hover(|d| d.bg(tok::bg_selected()))
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
                                            Some(f) => crate::frame_view::frame(f)
                                                .absolute()
                                                .size_full()
                                                .object_fit(ObjectFit::Contain)
                                                .into_any_element(),
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
                                        .flex_col()
                                        .min_w_0()
                                        .px_0p5()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(item.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(tok::text_tertiary())
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(detail),
                                        ),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.add_screen(mode, &title, &value, size, window, cx)
                                })),
                        );
                    }
                    body = body.child(grid);
                }
            }
            Step::Cameras => {
                body = body.child(back(cx));
                let cams: Vec<_> =
                    self.camera_devices.iter().filter(|(label, _)| matches(&query, &[label])).cloned().collect();
                if cams.is_empty() {
                    body = body.child(div().p_3().text_color(tok::text_tertiary()).child("No cameras found"));
                }
                let mut grid = div().grid().grid_cols(2).gap_1();
                for (i, (label, uid)) in cams.into_iter().enumerate() {
                    // (by the device's transport type, not its name)
                    let virtual_cam = lightair_platform::devices::camera_is_virtual(&uid);
                    let hint = if virtual_cam { "Virtual camera" } else { "Camera" };
                    grid = grid.child(card(("camera", i).into(), "camera", label.clone(), hint.into()).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.add_camera(&label, &uid, cx);
                            this.close_add_object(window, cx);
                        }),
                    ));
                }
                body = body.child(grid);
            }
        }

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(80.))
                .bg(tok::scrim())
                // a sheet: nothing behind it takes the mouse (scrolling here never pans the canvas)
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_add_object(window, cx)))
                .child(
                    div()
                        .id("add-object")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(560.))
                        .max_h(px(560.))
                        .p_3()
                        .rounded(px(14.))
                        .bg(tok::bg_panel())
                        .border_1()
                        .border_color(tok::border_strong())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            if ev.keystroke.key == "escape" {
                                this.close_add_object(window, cx);
                                cx.stop_propagation();
                            }
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(div().flex_none().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(
                                    match state.step {
                                        Step::Types => "Add object",
                                        Step::Cameras => "Choose a camera",
                                        Step::Screens => match state.screens.as_ref().map(|p| p.mode) {
                                            Some(lightair_core::kinds::DISPLAY) => "Choose a display",
                                            Some(lightair_core::kinds::APP) => "Choose an app",
                                            _ => "Choose a window",
                                        },
                                        Step::Web => "Browser",
                                    },
                                ))
                                .when_some(scene.as_ref(), |d, s| {
                                    d.child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_size(px(11.))
                                            .text_color(tok::text_tertiary())
                                            .child(format!("to {}", s.name)),
                                    )
                                }),
                        )
                        .when(state.step != Step::Web, |d| d.child(TextInput::new(&state.search)))
                        .child(body),
                )
                .into_any_element(),
        )
    }
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
