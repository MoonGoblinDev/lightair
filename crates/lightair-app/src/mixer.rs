//! The audio mixer (docs/10-audio.md): one channel strip per track, with effect list, meter,
//! fader, Cue and Mute. Meters are read from the engine's lock-free cells every frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::component::Sizable;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::{AudioMix, Bus, Command, Effect, Id, Source, Master, MixerStrip, Workspace, Route, Send, Track, kinds};
use lightair_engine::meters::meters;
use serde_json::json;

use crate::studio::Studio;

/// What desktop audio takes, in this platform's words.
const EVERYTHING: &str = if cfg!(windows) { "Everything the PC plays" } else { "Everything the Mac plays" };
use crate::ui::{self, tok};

const STRIP_W: f32 = 166.0;
/// The effects list's height on a strip (the routing below it is added to this), and the least
/// it's shown at (a row and the +).
const FX_H: f32 = 76.0;
const FX_MIN: f32 = 50.0;
/// The meter area's height with the knobs, and without.
const LEVELS_H: f32 = 196.0;
const LEVELS_MIN: f32 = 110.0;
/// The source row at the foot.
const FOOT_H: f32 = 40.0;
const DB_MIN: f32 = -60.0;
const DB_MAX: f32 = 12.0;

/// Fader position (0 = bottom, 1 = top) for a dB value: linear in dB, −60…+12.
fn db_to_pos(db: f32) -> f32 {
    if !db.is_finite() { 0.0 } else { ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0) }
}

fn pos_to_db(pos: f32) -> f32 {
    if pos <= 0.001 { f32::NEG_INFINITY } else { DB_MIN + pos.clamp(0.0, 1.0) * (DB_MAX - DB_MIN) }
}

/// Meter height fraction for a level (scale 0 to −60 dBFS).
fn meter_frac(db: f32) -> f32 {
    if !db.is_finite() { 0.0 } else { ((db + 60.0) / 60.0).clamp(0.0, 1.0) }
}

/// The vocal chain new microphone tracks get (docs/10-audio.md, Voice Clarity): the engine's
/// one-knob chain (noise, expander, de-esser, EQ, compressor, limiter).
/// The commands that set the mixer up as a template. Podcasts take an interface's channels as
/// mics when there's one with more than two (a stereo USB mic isn't two people); music takes a
/// two-channel one too (vocal and instrument). Otherwise the default source.
pub fn template_command(workspace: &lightair_core::Workspace, t: lightair_core::templates::Template) -> Command {
    use lightair_core::templates::{Mics, Template};
    let least = match t {
        Template::Podcast => 3,
        Template::Music => 2,
        _ => u16::MAX,
    };
    let devices = lightair_engine::devices::input_devices();
    let mics = devices
        .iter()
        .filter(|d| d.channels >= least)
        .max_by_key(|d| d.channels)
        .map(|d| Mics { kind: lightair_engine::devices::DEVICE_KIND.into(), device: d.id.clone(), name: d.name.clone(), channels: d.channels })
        .unwrap_or(Mics { kind: lightair_engine::devices::DEVICE_KIND.into(), device: String::new(), name: SYSTEM_DEFAULT.into(), channels: 1 });
    lightair_core::templates::apply(workspace, t, &mics)
}

pub fn voice_clarity() -> Vec<Effect> {
    lightair_core::catalog::def("audio.voice").map(|d| vec![d.create()]).unwrap_or_default()
}

pub struct MixerPanel {
    studio: Entity<Studio>,
    faders: Rc<RefCell<HashMap<Id, Bounds<Pixels>>>>,
    dragging: Option<Id>,
    /// The small sliders (pan, trim, sends): where each is, and the one being dragged.
    knobs: Rc<RefCell<HashMap<Knob, Bounds<Pixels>>>>,
    knob_drag: Option<KnobDrag>,
    /// Each strip's routing section height (measured as it's drawn): every strip's top is as
    /// tall as the tallest, so meters and faders line up across the mixer.
    sections: Rc<RefCell<HashMap<Id, f32>>>,
    /// The panel's height (measured as it's drawn): short panels get compact strips.
    panel_h: Rc<std::cell::Cell<f32>>,
    /// A strip's name being edited.
    renaming: Option<crate::rows::Renaming>,
    /// Where each strip's meter is drawn (the meter overlay paints the levels there).
    spots: Spots,
    /// The window's native layer (macOS: meters drawn there, under the UI).
    native: crate::native_preview::Shared,
    add_track_menu: bool,
    /// When a click outside last closed the add menu (that click on + mustn't reopen it).
    menu_closed: Option<Instant>,
    focus: FocusHandle,
}

impl MixerPanel {
    pub fn new(studio: Entity<Studio>, native: crate::native_preview::Shared, cx: &mut Context<Self>) -> Self {
        cx.observe(&studio, |_, _, cx| cx.notify()).detach();
        Self {
            studio,
            faders: Rc::default(),
            dragging: None,
            knobs: Rc::default(),
            knob_drag: None,
            sections: Rc::default(),
            panel_h: Rc::default(),
            renaming: None,
            spots: Rc::default(),
            native,
            add_track_menu: false,
            menu_closed: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn toggle_add_track(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu_closed.take().is_some_and(|t| t.elapsed().as_millis() < 300) {
            return;
        }
        self.add_track_menu = !self.add_track_menu;
        cx.notify();
    }

    fn exec(&self, cmd: Command, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            let _ = s.execute(cmd, cx);
        });
    }

    fn fader_to(&self, track: Id, y: Pixels, cx: &mut Context<Self>) {
        let Some(b) = self.faders.borrow().get(&track).copied() else { return };
        let top = f32::from(b.origin.y);
        let h = f32::from(b.size.height).max(1.0);
        let pos = 1.0 - ((f32::from(y) - top) / h).clamp(0.0, 1.0);
        let db = (pos_to_db(pos) * 2.0).round() / 2.0;
        let workspace = self.studio.read(cx).workspace().clone();
        if let Some(cmd) = MixerStrip::find(&workspace, track).and_then(|k| volume_cmd(&workspace, k, db)) {
            self.exec(cmd, cx);
        }
    }

    fn add_track(
        &mut self,
        name: &str,
        kind: &str,
        settings: serde_json::Value,
        effects: Vec<Effect>,
        cx: &mut Context<Self>,
    ) {
        self.add_track_menu = false;
        let source = Source::new(name, kind, settings);
        let mut track = Track::new(name, source.id);
        track.effects = effects;
        let cmd = Command::Batch {
            label: format!("Add {name} track"),
            commands: vec![Command::AddSource { source }, Command::AddTrack { track, index: None }],
        };
        self.exec(cmd, cx);
    }
}

impl Render for MixerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self.studio.read(cx).workspace().clone();
        let mut strips = Vec::new();
        // Program, pinned to the right edge (the others scroll beside it)
        let mut master = None;
        let views = strip_views(&workspace);
        self.sections.borrow_mut().retain(|id, _| views.iter().any(|v| v.id == *id));
        let layout = Layout::fit(self.panel_h.get(), self.sections.borrow().values().copied().fold(0.0, f32::max));
        for (i, view) in views.iter().enumerate() {
            let strip = self.strip(i, view, layout, cx);
            let menu = StripMenu::of(&workspace, view);
            let is_master = view.kind == MixerStrip::Master;
            if i > 0 && !is_master && views[i - 1].kind_order() != view.kind_order() {
                // buses and mixes each start a group
                strips.push(div().flex_none().w(px(1.)).h_full().bg(tok::border()).into_any_element());
            }
            let el = div()
                    .id(("strip-menu", i))
                    .flex_none()
                    .h_full()
                    .child(strip)
                    .on_mouse_down(MouseButton::Right, {
                        let (studio, id) = (self.studio.downgrade(), view.id);
                        move |_, _, cx| {
                            if let Some(s) = studio.upgrade() {
                                s.update(cx, |s, cx| {
                                    s.select_track(Some(id));
                                    cx.notify();
                                });
                            }
                        }
                    })
                    .context_menu({
                        let f = strip_menu(self.studio.downgrade(), cx.entity().downgrade(), menu);
                        move |m, window, cx| f(m, window, cx)
                    })
                    .into_any_element();
            if is_master {
                master = Some(el);
            } else {
                strips.push(el);
            }
        }
        self.spots.borrow_mut().retain(|id, _| views.iter().any(|v| v.id == *id));

        let panel_h = self.panel_h.clone();
        let native = crate::native_preview::on();
        let (spots, holes_spots) = (self.spots.clone(), self.spots.clone());
        let native_ref = self.native.clone();
        div()
            .id("mixer")
            .flex()
            .flex_col()
            .size_full()
            .child(
                canvas(move |b, _, _| panel_h.set(f32::from(b.size.height)), move |b, _, window, _| {
                    // the panel's own background, around the natively drawn meters
                    if native {
                        let holes: Vec<Bounds<Pixels>> = holes_spots.borrow().values().flat_map(|s| s.holes()).collect();
                        paint_except(window, b, &holes, tok::bg_panel(), px(0.));
                    }
                })
                .absolute()
                .size_full(),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                if let Some(track) = this.dragging {
                    this.fader_to(track, ev.position.y, cx);
                }
                match this.knob_drag {
                    Some(KnobDrag::Slide(knob)) => this.knob_to(knob, ev.position.x, cx),
                    Some(KnobDrag::Turn { knob, y0, v0 }) => {
                        // a full sweep over 160 px of travel, up to turn right
                        let (lo, hi) = knob.range();
                        let v = (v0 + (y0 - f32::from(ev.position.y)) / 160.0 * (hi - lo)).clamp(lo, hi);
                        this.knob_value(knob, v, cx);
                    }
                    None => {}
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
            .relative()
            // last laid out: the meters go where the strips left room for them
            .when(native, |d| {
                d.child(
                    canvas(
                        move |_, _, _| {
                            let places: Vec<(Id, crate::native_preview::MeterPlace)> =
                                spots.borrow().iter().filter_map(|(id, s)| s.place().map(|p| (*id, p))).collect();
                            if let Some(n) = native_ref.borrow_mut().as_mut() {
                                n.place_meters(&places);
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_0(),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("strips")
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .overflow_scroll()
                            .children(strips)
                            .when(workspace.tracks.is_empty(), |d| {
                                d.child(div().text_color(tok::text_tertiary()).child("No tracks"))
                            }),
                    )
                    .when_some(master, |d, m| {
                        d.child(
                            div()
                                .id("master-strip")
                                .flex()
                                .flex_none()
                                .px_3()
                                .py_2()
                                .border_l_1()
                                .border_color(tok::border())
                                .overflow_y_scroll()
                                .child(m),
                        )
                    }),
            )
            .when(self.add_track_menu, |d| d.child(self.add_menu(cx)))
    }
}

impl MixerPanel {
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        let knob = self.knob_drag.take().is_some();
        if self.dragging.take().is_some() || knob {
            self.studio.update(cx, |s, cx| {
                s.end_gesture();
                cx.notify();
            });
        }
    }

    /// The add menu: a card under the panel's + button, in two columns (what a track listens to,
    /// and the mixer's own strips and templates). Any click outside closes it.
    fn add_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        fn heading(label: &'static str) -> Div {
            div()
                .px_2()
                .pt_1()
                .pb_1()
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tok::text_tertiary())
                .child(label.to_uppercase())
        }
        fn entry(id: impl Into<ElementId>, icon: &str, title: impl Into<SharedString>, detail: impl Into<SharedString>) -> Stateful<Div> {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2p5()
                .px_2()
                .py_1p5()
                .rounded(px(7.))
                .cursor_pointer()
                .hover(|d| d.bg(tok::bg_selected()))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(28.))
                        .rounded(px(7.))
                        .bg(tok::bg_raised())
                        .child(ui::icon(icon).size(px(14.)).text_color(tok::text_secondary())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(12.)).text_color(tok::text()).truncate().child(title.into()))
                        .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).truncate().child(detail.into())),
                )
        }
        let column = || div().flex().flex_col().gap_0p5().flex_1().min_w_0();

        let sources = column()
            .child(heading("Sources"))
            .child(entry("add-mic", "mic", "Audio input", "Any mic or audio interface").on_click(cx.listener(
                |this, _, _, cx| {
                    // captured by the engine directly (low latency)
                    this.add_track(SYSTEM_DEFAULT, lightair_engine::devices::DEVICE_KIND, json!({"device": ""}), voice_clarity(), cx)
                },
            )))
            .child(entry("add-desktop", "volume-2", "Desktop audio", EVERYTHING).on_click(cx.listener(
                |this, _, _, cx| this.add_track("Desktop audio", kinds::DESKTOP_AUDIO, json!({}), vec![], cx),
            )))
            .child(entry("add-app", "app-window", "App audio", "One app: a game, music, a call").on_click(cx.listener(
                |this, _, _, cx| this.add_track("App audio", kinds::APP_AUDIO, json!({}), vec![], cx),
            )));

        let mixer = column()
            .child(heading("Mixer"))
            .child(entry("add-bus", "group", "Bus", "Group tracks, or share a reverb").on_click(cx.listener(
                |this, _, _, cx| {
                    this.add_track_menu = false;
                    let bus = crate::shell::mixer_props::new_bus(this.studio.read(cx).workspace());
                    let id = bus.id;
                    this.exec(Command::AddBus { bus, index: None }, cx);
                    this.studio.update(cx, |s, cx| {
                        s.select_track(Some(id));
                        cx.notify();
                    });
                },
            )))
            .child(entry("add-mix", "split", "Mix", "Headphones, VOD, a mix-minus").on_click(cx.listener(
                |this, _, _, cx| {
                    this.add_track_menu = false;
                    let mix = crate::shell::mixer_props::new_mix(this.studio.read(cx).workspace());
                    let id = mix.id;
                    this.exec(Command::AddMix { mix }, cx);
                    this.studio.update(cx, |s, cx| {
                        s.select_track(Some(id));
                        cx.notify();
                    });
                },
            )))
            .child(div().mx_2().my_1().h(px(1.)).bg(tok::border()))
            .child(entry("add-template", "layout-template", "Templates…", "A mixer set up for your show").on_click(
                cx.listener(|this, _, window, cx| {
                    this.add_track_menu = false;
                    window.dispatch_action(Box::new(crate::shell::OpenAudioSettings), cx);
                    cx.notify();
                }),
            ));

        // pinned under the + button, above everything, kept inside the window
        let card = div()
                    .id("add-menu")
                    .w(px(500.))
                    .flex()
                    .gap_1()
                    .p_1p5()
                    .rounded(px(12.))
                    .bg(tok::bg_panel())
                    .border_1()
                    .border_color(tok::border_strong())
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.add_track_menu = false;
                        this.menu_closed = Some(Instant::now());
                        cx.notify();
                    }))
                    .child(sources)
                    .child(div().w(px(1.)).my_1().bg(tok::border()))
                    .child(mixer);
        div().absolute().top(px(4.)).right(px(10.)).size_0().child(deferred(
            anchored().anchor(Anchor::TopRight).snap_to_window_with_margin(px(8.)).child(card),
        ))
    }

    /// Sets a small slider from where the pointer is along it.
    fn knob_to(&self, knob: Knob, x: Pixels, cx: &mut Context<Self>) {
        let Some(b) = self.knobs.borrow().get(&knob).copied() else { return };
        let frac = ((f32::from(x) - f32::from(b.origin.x)) / f32::from(b.size.width).max(1.0)).clamp(0.0, 1.0);
        let (lo, hi) = knob.range();
        self.knob_value(knob, lo + frac * (hi - lo), cx);
    }

    /// Sets a knob or slider to `v` (rounded to its step; pan catches the centre).
    fn knob_value(&self, knob: Knob, v: f32, cx: &mut Context<Self>) {
        let v = match knob {
            // the centre catches
            Knob::Pan(_) if v.abs() < 0.04 => 0.0,
            Knob::Pan(_) => (v * 100.0).round() / 100.0,
            _ => (v * 2.0).round() / 2.0,
        };
        let workspace = self.studio.read(cx).workspace().clone();
        if let Some(cmd) = knob.set(&workspace, v) {
            self.exec(cmd, cx);
        }
    }

    /// A small horizontal slider: a thin track filled from its centre (or left end), a dot.
    /// Drag or click to set; double-click resets.
    fn mini(&self, id: usize, knob: Knob, value: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let (lo, hi) = knob.range();
        let frac = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
        let origin = if knob.centred() { ((0.0 - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
        let (from, to) = (origin.min(frac), origin.max(frac));
        let knobs = self.knobs.clone();
        div()
            .id(("mini", id))
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(16.))
            .cursor_pointer()
            .child(
                canvas(
                    move |b, _, _| {
                        knobs.borrow_mut().insert(knob, b);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(div().absolute().left_0().right_0().top(px(6.5)).h(px(3.)).rounded_full().bg(tok::bg_app()))
            .child(
                div()
                    .absolute()
                    .left(relative(from))
                    .w(relative(to - from))
                    .top(px(6.5))
                    .h(px(3.))
                    .rounded_full()
                    .bg(tok::text_secondary()),
            )
            .when(knob.centred(), |d| {
                d.child(div().absolute().left(relative(origin)).top(px(4.)).w(px(1.)).h(px(8.)).bg(tok::text_tertiary()))
            })
            .child(
                div()
                    .absolute()
                    .left(relative(frac))
                    .ml(px(-5.))
                    .top(px(3.))
                    .size(px(10.))
                    .rounded_full()
                    .bg(tok::text())
                    .border_1()
                    .border_color(tok::bg_app()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if ev.click_count >= 2 {
                        let workspace = this.studio.read(cx).workspace().clone();
                        if let Some(cmd) = knob.set(&workspace, knob.reset()) {
                            this.exec(cmd, cx);
                        }
                        return;
                    }
                    this.studio.update(cx, |s, _| s.begin_gesture());
                    this.knob_drag = Some(KnobDrag::Slide(knob));
                    this.knob_to(knob, ev.position.x, cx);
                }),
            )
    }

    /// Edits a strip's name in place (Enter or clicking away keeps it; Escape doesn't).
    pub fn start_rename(&mut self, strip: MixerStrip, window: &mut Window, cx: &mut Context<Self>) {
        if strip == MixerStrip::Master {
            return;
        }
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(name) = strip_views(&workspace).into_iter().find(|v| v.kind == strip).map(|v| v.name) else { return };
        self.renaming = Some(crate::rows::Renaming::open(
            crate::rows::RowTarget::Strip(strip),
            &name,
            window,
            cx,
            |this, commit, _, cx| this.finish_rename(commit, cx),
        ));
        cx.notify();
    }

    fn finish_rename(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(r) = self.renaming.take() else { return };
        if commit && let Some(cmd) = r.command(cx) {
            self.exec(cmd, cx);
        }
        cx.notify();
    }

    /// A strip's routing section, its height noted for lining the strips up.
    fn measured(&self, id: Id, section: Div) -> Div {
        let sections = self.sections.clone();
        section.relative().flex_none().child(
            canvas(
                move |b, _, _| {
                    let h = f32::from(b.size.height);
                    let mut s = sections.borrow_mut();
                    if s.get(&id).is_none_or(|old| (old - h).abs() > 0.5) {
                        s.insert(id, h);
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }

    /// A mix's output: its own only, or into Program too (an effects return).
    fn mix_output(&self, i: usize, id: Id, on: bool, cx: &mut Context<Self>) -> Div {
        let me = cx.entity().downgrade();
        let button = gpui_kit::component::button::Button::new(("mix-out", i))
            .label(if on { "→ Program + own output" } else { "→ Own output" })
            .xsmall()
            .outline()
            .w_full()
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (to, label) in [(false, "Its own output only"), (true, "Into Program too")] {
                    let me = me.clone();
                    menu = menu.item(gpui_kit::component::menu::PopupMenuItem::new(label).checked(to == on).on_click(
                        move |_, _, cx| {
                            if let Some(me) = me.upgrade() {
                                me.update(cx, |this, cx| {
                                    let p = this.studio.read(cx).workspace().clone();
                                    if let Some(m) = p.audio.mix(id) {
                                        let mix = AudioMix { to_program: to, ..m.clone() };
                                        this.exec(Command::UpdateMix { mix }, cx);
                                    }
                                });
                            }
                        },
                    ));
                }
                menu
            });
        div().flex_none().px_2().py_1p5().border_b_1().border_color(tok::border()).child(button)
    }

    /// A rotary knob: its name, the knob (an arc from its centre or left end to the value, and
    /// a pointer), and the value. Drag up or down to turn; double-click resets.
    fn rotary(&self, id: usize, knob: Knob, value: f32, name: &'static str, text: String, cx: &mut Context<Self>) -> Div {
        let (lo, hi) = knob.range();
        let frac = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
        let origin = if knob.centred() { ((0.0 - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
        let (track, fill, pointer) = (tok::bg_app(), tok::accent_primary(), tok::text());
        let sweep = 135f32.to_radians();
        let angle = move |f: f32| -sweep + f * 2.0 * sweep;
        let dial = div()
            .id(("knob", id))
            .relative()
            .size(px(32.))
            .cursor(CursorStyle::ResizeUpDown)
            .child(
                div()
                    .absolute()
                    .top(px(5.))
                    .left(px(5.))
                    .size(px(22.))
                    .rounded_full()
                    .bg(tok::bg_panel())
                    .border_1()
                    .border_color(tok::border_strong()),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, _| {
                        let (cx0, cy0) = (
                            f32::from(b.origin.x) + f32::from(b.size.width) / 2.0,
                            f32::from(b.origin.y) + f32::from(b.size.height) / 2.0,
                        );
                        let at = |a: f32, r: f32| point(px(cx0 + r * a.sin()), px(cy0 - r * a.cos()));
                        let arc = |from: f32, to: f32, colour: Hsla, window: &mut Window| {
                            let (a, z) = (angle(from.min(to)), angle(from.max(to)));
                            let steps = (((z - a) / 0.08).ceil() as usize).max(1);
                            let mut path = PathBuilder::stroke(px(2.5));
                            path.move_to(at(a, 14.5));
                            for k in 1..=steps {
                                path.line_to(at(a + (z - a) * k as f32 / steps as f32, 14.5));
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, colour);
                            }
                        };
                        arc(0.0, 1.0, track, window);
                        if (frac - origin).abs() > 0.002 {
                            arc(origin, frac, fill, window);
                        }
                        let mut path = PathBuilder::stroke(px(2.));
                        path.move_to(at(angle(frac), 3.0));
                        path.line_to(at(angle(frac), 10.0));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, pointer);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if ev.click_count >= 2 {
                        this.knob_value(knob, knob.reset(), cx);
                        return;
                    }
                    this.studio.update(cx, |s, _| s.begin_gesture());
                    this.knob_drag = Some(KnobDrag::Turn { knob, y0: f32::from(ev.position.y), v0: value });
                }),
            )
            .tooltip(move |w, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Drag up or down; double-click resets").build(w, cx)
            });
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(div().text_size(px(8.5)).font_weight(FontWeight::BOLD).text_color(tok::text_tertiary()).child(name))
            .child(dial)
            .child(div().text_size(px(9.5)).font_family(crate::ui::MONO).text_color(tok::text_secondary()).child(text))
    }

    /// A track's or bus's place in the mix, on the strip: output, polarity, mono, recording,
    /// sends (pan and trim are knobs by the fader).
    fn channel(&self, i: usize, view: &StripView, ch: &Channel, workspace: &Workspace, cx: &mut Context<Self>) -> Div {
        let id = view.id;
        let label = |text: &'static str| {
            div()
                .w(px(30.))
                .flex_none()
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tok::text_tertiary())
                .child(text)
        };

        // output
        let route_name = match ch.route {
            Route::Master => "Master".to_string(),
            Route::Bus(b) => workspace.audio.bus(b).map_or("Bus".into(), |b| b.name.clone()),
            Route::None => "Sends only".into(),
        };
        let mut routes = vec![(Route::Master, "Master".to_string())];
        routes.extend(workspace.audio.buses.iter().filter(|b| b.id != id).map(|b| (Route::Bus(b.id), b.name.clone())));
        routes.push((Route::None, "Sends only".into()));
        let (me, current) = (cx.entity().downgrade(), ch.route);
        let output = gpui_kit::component::button::Button::new(("strip-route", i))
            .label(format!("→ {route_name}"))
            .xsmall()
            .outline()
            .w_full()
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (r, name) in routes.clone() {
                    let me = me.clone();
                    menu = menu.item(gpui_kit::component::menu::PopupMenuItem::new(name).checked(r == current).on_click(
                        move |_, _, cx| {
                            if let Some(me) = me.upgrade() {
                                me.update(cx, |this, cx| {
                                    let p = this.studio.read(cx).workspace().clone();
                                    if let Some(cmd) = strip_update(&p, id, |f| *f.route = r) {
                                        this.exec(cmd, cx);
                                    }
                                });
                            }
                        },
                    ));
                }
                menu
            });

        let mut col = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1p5()
            .border_b_1()
            .border_color(tok::border())
            .child(output);

        // switches
        let chip = |key: &'static str, text: &'static str, tip: &'static str, on: bool, on_bg: Hsla| {
            div()
                .id((key, i))
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .h(px(20.))
                .rounded(px(4.))
                .border_1()
                .border_color(if on { on_bg } else { tok::border_strong() })
                .bg(if on { on_bg } else { tok::bg_app() })
                .text_size(px(9.5))
                .font_weight(FontWeight::BOLD)
                .text_color(if on { tok::text_inverse() } else { tok::text_secondary() })
                .cursor_pointer()
                .child(text)
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
        };
        let toggle = move |f: fn(StripFields)| {
            move |this: &mut MixerPanel, _: &ClickEvent, _: &mut Window, cx: &mut Context<MixerPanel>| {
                cx.stop_propagation();
                let p = this.studio.read(cx).workspace().clone();
                if let Some(cmd) = strip_update(&p, id, f) {
                    this.exec(cmd, cx);
                }
            }
        };
        let mut switches = div().flex().gap_1();
        if let Some(invert) = ch.invert {
            switches = switches.child(
                chip("ch-invert", "Ø", "Polarity (flip the waveform)", invert, tok::accent_primary())
                    .on_click(cx.listener(toggle(|f| if let Some(v) = f.invert { *v = !*v }))),
            );
        }
        if let Some(mono) = ch.mono {
            switches = switches.child(
                chip("ch-mono", "MONO", "Sum to mono", mono, tok::accent_primary())
                    .on_click(cx.listener(toggle(|f| if let Some(v) = f.mono { *v = !*v }))),
            );
        }
        switches = switches.child(
            chip("ch-rec", "REC", "Record to its own file (multitrack recordings)", ch.record, tok::error())
                .on_click(cx.listener(toggle(|f| *f.record = !*f.record))),
        );
        col = col.child(switches);

        // sends
        if !workspace.audio.mixes.is_empty() {
            col = col.child(label("SENDS").w_full().pt_0p5());
            for (n, mix) in workspace.audio.mixes.iter().enumerate() {
                let send = ch.sends.iter().find(|s| s.mix == mix.id).copied();
                let mix_id = mix.id;
                let mut row = div().flex().items_center().gap_1().h(px(18.)).child(
                    div()
                        .id(("send-dot", i * 64 + n))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(14.))
                        .cursor_pointer()
                        .child(div().size(px(7.)).rounded_full().bg(if send.is_some() { tok::ok() } else { tok::text_tertiary() }))
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if send.is_some() { "Stop sending" } else { "Send to this mix" })
                                .build(w, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let p = this.studio.read(cx).workspace().clone();
                            let cmd = strip_update(&p, id, |f| {
                                if f.sends.iter().any(|s| s.mix == mix_id) {
                                    f.sends.retain(|s| s.mix != mix_id);
                                } else {
                                    f.sends.push(Send { mix: mix_id, level_db: 0.0, pre_fader: false });
                                }
                            });
                            if let Some(cmd) = cmd {
                                this.exec(cmd, cx);
                            }
                        })),
                );
                row = row.child(
                    div()
                        .w(px(40.))
                        .flex_none()
                        .truncate()
                        .text_size(px(10.5))
                        .text_color(if send.is_some() { tok::text() } else { tok::text_tertiary() })
                        .child(mix.name.clone()),
                );
                match send {
                    Some(s) => {
                        let pre = s.pre_fader;
                        row = row
                            .child(self.mini(i * 64 + 8 + n, Knob::Send(id, mix_id), s.level_db, cx))
                            .child(
                                div()
                                    .id(("send-pre", i * 64 + n))
                                    .flex_none()
                                    .px_1()
                                    .rounded(px(3.))
                                    .text_size(px(8.5))
                                    .font_weight(FontWeight::BOLD)
                                    .cursor_pointer()
                                    .bg(if pre { tok::bg_selected() } else { tok::bg_app() })
                                    .text_color(if pre { tok::text() } else { tok::text_tertiary() })
                                    .child(if pre { "PRE" } else { "POST" })
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Before the fader (a monitor mix the fader doesn't change) or after it",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let p = this.studio.read(cx).workspace().clone();
                                        let cmd = strip_update(&p, id, |f| {
                                            for x in f.sends.iter_mut().filter(|x| x.mix == mix_id) {
                                                x.pre_fader = !x.pre_fader;
                                            }
                                        });
                                        if let Some(cmd) = cmd {
                                            this.exec(cmd, cx);
                                        }
                                    })),
                            );
                    }
                    None => row = row.child(div().flex_1()),
                }
                col = col.child(row);
            }
        }
        col
    }

    fn strip(
        &self,
        i: usize,
        view: &StripView,
        layout: Layout,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Layout { fx_h, routing, section, foot, knobs } = layout;
        let compact = fx_h <= 0.0;
        let top_h = fx_h + if routing { section } else { 0.0 };
        let id = view.id;
        let kind = view.kind;
        let (muted, cue, volume) = (view.muted, view.cue, view.volume_db);
        let selected = self.studio.read(cx).selected_track == Some(id);
        let workspace = self.studio.read(cx).workspace().clone();
        let unity = volume_cmd(&workspace, kind, 0.0);
        let mute = mute_cmd(&workspace, kind, !muted);
        let studio = self.studio.downgrade();
        let request = move |req: crate::studio::UiRequest, cx: &mut App| {
            if let Some(s) = studio.upgrade() {
                s.update(cx, |s, cx| {
                    s.ui_request = Some(req);
                    cx.notify();
                });
            }
        };

        // ---- header: the name, and its menu
        let menu = strip_menu(self.studio.downgrade(), cx.entity().downgrade(), StripMenu::of(&workspace, view));
        // the name is centred on the strip; the tag, effect count (left) and menu (right) sit
        // over equal margins either side of it
        let fx_chip = compact && !view.effects.is_empty();
        let left_w: f32 = if view.badge.is_some() { 26.0 } else { 0.0 } + if fx_chip { 34.0 } else { 0.0 };
        let side = left_w.max(24.0) + 6.0;
        let (head_bg, head_fg) = match view.color {
            Some(c) => {
                let bg = strip_colour(c);
                (Some(bg), if luminance(c) > 0.6 { tok::bg_app() } else { gpui_kit::white() })
            }
            None => (None, tok::text()),
        };
        let name_el = match self.renaming.as_ref().filter(|r| r.target == crate::rows::RowTarget::Strip(kind)) {
            Some(r) => div()
                .id(("strip-rename", i))
                .w_full()
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                    if ev.keystroke.key == "escape" {
                        this.finish_rename(false, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(gpui_kit::component::input::Input::new(&r.input).xsmall())
                .into_any_element(),
            None => div()
                .id(("strip-name", i))
                .min_w_0()
                .text_center()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(head_fg)
                .truncate()
                .child(view.name.clone())
                .when(kind != MixerStrip::Master, |d| {
                    d.on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        if ev.click_count() == 2 {
                            cx.stop_propagation();
                            this.start_rename(kind, window, cx);
                        }
                    }))
                })
                .into_any_element(),
        };
        let mut left = div().flex().items_center().gap_1().pl_1();
        if let Some(b) = view.badge {
            left = left.child(
                div()
                    .text_size(px(8.5))
                    .font_weight(FontWeight::BOLD)
                    .whitespace_nowrap()
                    .text_color(if head_bg.is_some() { head_fg.opacity(0.7) } else { tok::text_tertiary() })
                    .child(b),
            );
        }
        if fx_chip {
            // compact: how many effects are on (they open in the Inspector)
            let on = view.effects.iter().filter(|e| e.2).count();
            let first = view.effects[0].0;
            let open = request.clone();
            left = left.child(
                div()
                    .id(("strip-fx-count", i))
                    .flex_none()
                    .px_1()
                    .rounded(px(3.))
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .bg(if on > 0 { tok::bg_selected() } else { tok::bg_app() })
                    .text_color(if on > 0 { tok::text_secondary() } else { tok::text_tertiary() })
                    .child(format!("FX {on}"))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        open(crate::studio::UiRequest::OpenEffect(kind, first), cx);
                    }),
            );
        }
        let header = div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(px(30.))
            .pr_1()
            .border_b_1()
            .border_color(tok::border())
            // the strip's corners (8, inside its 1 px border): a colour doesn't poke out of them
            .rounded_t(px(7.))
            .when_some(head_bg, |d, bg| d.bg(bg))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(side))
                    .child(name_el),
            )
            .child(left)
            .child(div().flex_1())
            .child(
                gpui_kit::component::button::Button::new(("strip-chevron", i))
                    .icon(ui::icon("chevron-down").text_color(head_fg))
                    .ghost()
                    .xsmall()
                    .dropdown_menu(move |m, window, cx| menu(m, window, cx)),
            );

        // ---- effects: a dot to switch each on or off, its name, a button to open it; + adds one
        // gives way to the meter in a short panel
        let mut fx = div()
            .id(("strip-fx", i))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .border_b_1()
            .border_color(tok::border());
        let home = crate::effect_drag::Home::Strip(kind);
        let drop_at = move |before: Option<Id>| {
            move |this: &mut MixerPanel, d: &crate::effect_drag::DraggedEffect, _: &mut Window, cx: &mut Context<MixerPanel>| {
                let p = this.studio.read(cx).workspace().clone();
                if let Some(cmd) = crate::effect_drag::move_cmd(&p, d, home, before) {
                    this.exec(cmd, cx);
                }
            }
        };
        for (n, (eid, name, on)) in view.effects.iter().cloned().enumerate() {
            let cmd = Command::SetStripEffectEnabled { strip: kind, effect: eid, enabled: !on };
            let open = request.clone();
            let dragged = crate::effect_drag::DraggedEffect { home, effect: eid, name: name.clone().into() };
            fx = fx.child(
                div()
                    .id(("fx-row", i * 64 + n))
                    // drag to reorder (or onto another strip); dropped here, it goes before this one
                    .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                    .drag_over::<crate::effect_drag::DraggedEffect>(|s, _, _, _| {
                        s.border_t_2().border_color(tok::accent_primary())
                    })
                    .on_drop(cx.listener(drop_at(Some(eid))))
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(24.))
                    .px_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(tok::border())
                    .child(
                        div()
                            .id(("fx-dot", i * 64 + n))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .cursor_pointer()
                            .child(div().size(px(8.)).rounded_full().bg(if on { tok::ok() } else { tok::text_tertiary() }))
                            .tooltip(move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if on { "Bypass" } else { "Turn on" }).build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.exec(cmd.clone(), cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_center()
                            .text_size(px(11.5))
                            .text_color(if on { tok::text() } else { tok::text_tertiary() })
                            .truncate()
                            .child(name),
                    )
                    .child(
                        div()
                            .id(("fx-edit", i * 64 + n))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|d| d.bg(tok::bg_selected()))
                            .child(ui::icon("sliders-horizontal").size(px(12.)).text_color(tok::text_secondary()))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                open(crate::studio::UiRequest::OpenEffect(kind, eid), cx);
                            }),
                    ),
            );
        }
        let add = request.clone();
        fx = fx.child(
            div()
                .id(("fx-add", i))
                .drag_over::<crate::effect_drag::DraggedEffect>(|s, _, _, _| {
                    s.border_t_2().border_color(tok::accent_primary())
                })
                .on_drop(cx.listener(drop_at(None)))
                .flex()
                .flex_1()
                .min_h(px(24.))
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|d| d.bg(tok::bg_selected()))
                .child(ui::icon("plus").size(px(14.)).text_color(tok::text_secondary()))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    add(crate::studio::UiRequest::AddEffect(kind), cx);
                }),
        );

        // ---- the stereo meter: two bars, a dB scale beside them, the peak above. The levels
        // themselves are painted by the meter overlay (MeterOverlay) where these say: they move
        // 30 times a second without the strips being laid out again.
        let spot = |what: Spot| {
            let spots = self.spots.clone();
            canvas(
                move |b, window, _| {
                    let mut spots = spots.borrow_mut();
                    let s = spots.entry(id).or_default();
                    match what {
                        Spot::Bar(c) => s.bars[c] = Some(b),
                        Spot::Readout => s.readout = Some(b),
                        Spot::Box => s.boxed = Some(b),
                    }
                    s.clip = Some(window.content_mask().bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
        };
        // natively drawn meters (native_preview): the strip leaves holes for them
        let native = crate::native_preview::on();
        let bar = |c: usize| div().relative().w(px(6.)).h_full().when(!native, |d| d.bg(tok::bg_panel())).child(spot(Spot::Bar(c)));
        let mut scale = div().relative().flex_1().h_full();
        // a label every 6 dB when the meter is tall enough to read them, every 12 when it's short
        // (its height: the fader's, beside it, from the last frame)
        let tall = self.faders.borrow().get(&id).is_none_or(|b| f32::from(b.size.height) >= 190.0);
        for db in (0..10).map(|k| -6 * k).filter(|db| tall || db % 12 == 0) {
            // each label centred on its level; the top one (0) sits inside the meter's edge
            let label = div().absolute().right(px(2.));
            let label = if db == 0 { label.top(px(-1.)) } else { label.bottom(relative(meter_frac(db as f32))).mb(px(-6.)) };
            scale = scale.child(
                label
                    .text_size(px(8.5))
                    .font_family(crate::ui::MONO)
                    .whitespace_nowrap()
                    .text_color(tok::text_tertiary())
                    .child(if db == 0 { "0".to_string() } else { format!("{db}") }),
            );
        }
        let peak_readout = div().relative().h(px(18.)).rounded(px(3.)).when(!native, |d| d.bg(tok::bg_app())).child(spot(Spot::Readout));
        let readout = |text: String| {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(18.))
                .rounded(px(3.))
                .text_size(px(10.))
                .font_family(crate::ui::MONO)
                .child(text)
        };
        let meter = div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(50.))
            .h_full()
            .child(peak_readout)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(2.))
                    .p(px(3.))
                    .rounded(px(3.))
                    .when(!native, |d| d.bg(tok::bg_app()))
                    .child(spot(Spot::Box))
                    .child(bar(0))
                    .child(bar(1))
                    .child(scale),
            );

        // ---- the fader: its value above, a long throw, a thumb you can grab
        let faders = self.faders.clone();
        let fader = div()
            .id(("fader", i))
            .relative()
            .flex_1()
            .min_h_0()
            .w(px(32.))
            .cursor_pointer()
            .child(
                canvas(
                    move |b, _, _| {
                        faders.borrow_mut().insert(id, b);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(div().absolute().left(px(14.)).top(px(4.)).bottom(px(4.)).w(px(4.)).rounded_full().bg(tok::bg_app()))
            // unity
            .child(div().absolute().left(px(6.)).w(px(20.)).h(px(1.)).top(relative(1.0 - db_to_pos(0.0))).bg(tok::text_tertiary()))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(relative(1.0 - db_to_pos(volume)))
                    .mt(px(-9.))
                    .w(px(32.))
                    .h(px(18.))
                    .rounded(px(3.))
                    .bg(tok::text_secondary())
                    .border_1()
                    .border_color(tok::text())
                    .shadow_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().w(px(18.)).h(px(2.)).rounded_full().bg(tok::bg_app())),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if ev.click_count >= 2 {
                        if let Some(cmd) = unity.clone() {
                            this.exec(cmd, cx);
                        }
                        return;
                    }
                    this.studio.update(cx, |s, _| s.begin_gesture());
                    this.dragging = Some(id);
                    this.fader_to(id, ev.position.y, cx);
                }),
            );
        let fader_col = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .h_full()
            .child(readout(if volume.is_finite() { format!("{volume:.1}") } else { "-inf".into() }).w(px(36.)).text_color(tok::text()))
            .child(fader);

        // ---- the buttons
        let button = |key: &'static str, label: &'static str, on: bool, on_bg: Hsla| {
            div()
                .id((key, i))
                .flex()
                .items_center()
                .justify_center()
                .w(px(44.))
                .h(px(26.))
                .rounded(px(5.))
                .border_1()
                .border_color(if on { on_bg } else { tok::border_strong() })
                .bg(if on { on_bg } else { tok::bg_app() })
                .text_size(px(10.5))
                .font_weight(FontWeight::BOLD)
                .text_color(if on { tok::text_inverse() } else { tok::text_secondary() })
                .cursor_pointer()
                .hover(|d| d.opacity(0.85))
                .child(label)
        };
        let pan_text = |pan: f32| {
            if pan.abs() < 0.005 {
                "C".to_string()
            } else if pan < 0.0 {
                format!("L{:.0}", -pan * 100.0)
            } else {
                format!("R{:.0}", pan * 100.0)
            }
        };
        let buttons = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1p5()
            .h_full()
            .when_some(view.pan.filter(|_| knobs), |d, pan| {
                d.child(self.rotary(i * 4, Knob::Pan(id), pan, "PAN", pan_text(pan), cx))
            })
            .when_some(view.trim.filter(|_| knobs), |d, t| {
                d.child(self.rotary(i * 4 + 1, Knob::Trim(id), t, "TRIM", format!("{t:+.1}"), cx))
            })
            .child(div().flex_1())
            .when_some(cue, |d, cue| {
                d.child(button("cue", "CUE", cue, tok::text()).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let p = this.studio.read(cx).workspace().clone();
                    if let Some(cmd) = cue_cmd(&p, kind, !cue) {
                        this.exec(cmd, cx);
                    }
                })))
            })
            .child(button("mute", "MUTE", muted, tok::live()).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if let Some(cmd) = mute.clone() {
                    this.exec(cmd, cx);
                }
            })));

        let levels = div()
            .flex()
            .flex_1()
            .min_h(px(if knobs { LEVELS_H } else { LEVELS_MIN }))
            .gap_2()
            .px_2()
            .py_2()
            .child(meter)
            .child(fader_col)
            .child(div().flex_1())
            .child(buttons);

        div()
            .id(("strip", i))
            .flex()
            .flex_col()
            .flex_none()
            .w(px(STRIP_W))
            .h_full()
            .min_h(px(
                30. + top_h + if knobs { LEVELS_H } else { LEVELS_MIN } + if foot { FOOT_H } else { 0. },
            ))
            .rounded(px(8.))
            .overflow_hidden()
            .when(!native, |d| d.bg(tok::bg_raised()))
            .when(native, |d| {
                let spots = self.spots.clone();
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |b, _, window, _| {
                            let holes: Vec<Bounds<Pixels>> = spots.borrow().get(&id).map(|s| s.holes().collect()).unwrap_or_default();
                            paint_except(window, b, &holes, tok::bg_raised(), px(8.));
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .border_1()
            .border_color(if selected { tok::accent_primary() } else { tok::border() })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.studio.update(cx, |s, cx| {
                    s.selected_object = None;
                    s.select_track(Some(id));
                    cx.notify();
                })
            }))
            .child(header)
            // effects and routing: the same height on every strip
            .when(top_h > 0.0, |d| d.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .h(px(top_h))
                    .when(!compact, |d| d.child(fx))
                    .when_some(view.channel.as_ref().filter(|_| routing), |d, ch| {
                        d.child(self.measured(id, self.channel(i, view, ch, &workspace, cx)))
                    })
                    .when_some(view.mix_to_program.filter(|_| routing), |d, on| {
                        d.child(self.measured(id, self.mix_output(i, id, on, cx)))
                    }),
            ))
            .child(levels)
                        // full width, centred in its row
            .when(foot, |d| {
                d.child(
                    div().flex().flex_col().flex_none().justify_center().h(px(FOOT_H)).px_2().pb_1().child(self.strip_source(i, view, cx)),
                )
            })
    }

    /// The strip's foot: a track's device (pick another here), or what the strip is.
    fn strip_source(&self, i: usize, view: &StripView, cx: &mut Context<Self>) -> AnyElement {
        let chip = |icon: &'static str, label: String| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(28.))
                .px_2()
                .rounded(px(6.))
                .border_1()
                .border_color(tok::border_strong())
                .text_size(px(11.5))
                .text_color(tok::text_secondary())
                .child(ui::icon(icon).size(px(13.)).flex_none())
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        match &view.source {
            Some(src) => {
                let (track, current) = (view.id, src.current.clone());
                let me = cx.entity().downgrade();
                gpui_kit::component::button::Button::new(("strip-source", i))
                    .icon(ui::icon(src.icon))
                    .label(src.label.clone())
                    .small()
                    .outline()
                    .w_full()
                    .dropdown_caret(true)
                    .dropdown_menu(move |menu, window, cx| {
                        use gpui_kit::component::menu::PopupMenuItem;
                        let pick = move |to: TrackSource, app: String, label: String, me: WeakEntity<MixerPanel>, on: bool| {
                            source_item(track, to, app, label, me, on)
                        };
                        let mut menu = menu.item(PopupMenuItem::label("Microphones and interfaces"));
                        let mut devices = vec![(String::new(), SYSTEM_DEFAULT.to_string())];
                        devices.extend(lightair_engine::devices::input_devices().into_iter().map(|d| {
                            let n = if d.channels == 1 { "1 input".to_string() } else { format!("{} inputs", d.channels) };
                            (d.id.clone(), format!("{} · {n}", d.name))
                        }));
                        for (value, label) in devices {
                            let to = TrackSource::Device(value);
                            let on = to == current;
                            menu = menu.item(pick(to, String::new(), label, me.clone(), on));
                        }
                        menu = menu.separator().item(pick(
                            TrackSource::Desktop,
                            String::new(),
                            "Desktop audio".into(),
                            me.clone(),
                            current == TrackSource::Desktop,
                        ));
                        let (me2, current2) = (me.clone(), current.clone());
                        menu.submenu("App", window, cx, move |mut sub, _, _| {
                            let apps = running_apps();
                            if apps.is_empty() {
                                sub = sub.item(PopupMenuItem::new("No apps open").disabled(true));
                            }
                            for (name, bundle) in apps {
                                let to = TrackSource::App(bundle);
                                let on = to == current2;
                                sub = sub.item(source_item(track, to, name.clone(), name, me2.clone(), on));
                            }
                            sub
                        })
                    })
                    .into_any_element()
            }
            None => chip(
                match view.kind {
                    MixerStrip::Bus(_) => "group",
                    MixerStrip::Mix(_) => "split",
                    _ => "radio",
                },
                match view.kind {
                    MixerStrip::Bus(_) => "Bus".into(),
                    MixerStrip::Mix(_) => "Mix".into(),
                    _ => "Stream & recording".into(),
                },
            )
            .into_any_element(),
        }
    }
}

/// Stream loudness (EBU R128): short-term and integrated, against the common −14 LUFS target.
/// Click to start integrated loudness over.
fn loudness_readout<T: 'static>(cx: &mut Context<T>) -> Option<impl IntoElement> {
    let r = lightair_engine::loudness::stream_loudness()?;
    let show = |v: f64| if v.is_finite() { format!("{v:.1}") } else { "—".into() };
    // Against the loudness target (Settings › Audio): within 2 LU is on target; well below
    // sounds quiet; above gets turned down by the platform.
    let target = crate::prefs::LOUDNESS_TARGET.load(std::sync::atomic::Ordering::Relaxed) as f64;
    let colour = match r.short_term {
        v if !v.is_finite() || v < target - 10.0 => tok::text_tertiary(),
        v if v > target + 4.0 => tok::warn(),
        v if (v - target).abs() <= 2.0 => tok::ok(),
        _ => tok::text_secondary(),
    };
    Some(
        div()
            .id("loudness")
            .flex()
            .items_baseline()
            .gap_2()
            .px_2()
            .py_0p5()
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|d| d.bg(tok::bg_raised()))
            .font_family(crate::ui::MONO)
            .text_size(px(11.))
            .child(div().text_color(tok::text_tertiary()).child("LUFS"))
            .child(div().text_color(colour).child(format!("S {}", show(r.short_term))))
            .child(div().text_color(tok::text_secondary()).child(format!("I {}", show(r.integrated))))
            .tooltip(|w, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "Stream loudness: short-term (3 s) and integrated, green on your target (Settings › Audio). Click to reset integrated.",
                )
                .build(w, cx)
            })
            .on_click(cx.listener(|_, _, _, cx| {
                lightair_engine::loudness::reset_stream_loudness();
                cx.notify();
            })),
    )
}

// ---------------------------------------------------------------- dock panel
/// Where a strip's meter is: its two bars, the peak readout above them, and the area that clips
/// them (the strips scroll), in window pixels, as the strip was last laid out.
#[derive(Clone, Copy, Default)]
pub(crate) struct MeterSpot {
    bars: [Option<Bounds<Pixels>>; 2],
    readout: Option<Bounds<Pixels>>,
    clip: Option<Bounds<Pixels>>,
    /// The box the bars are in (drawn natively: native_preview).
    boxed: Option<Bounds<Pixels>>,
}

impl MeterSpot {
    /// Where the native meter goes, once all of it is laid out.
    fn place(&self) -> Option<crate::native_preview::MeterPlace> {
        Some(crate::native_preview::MeterPlace {
            boxed: self.boxed?,
            bars: [self.bars[0]?, self.bars[1]?],
            readout: self.readout?,
            clip: self.clip?,
        })
    }

    /// The holes the strip leaves for it (the box and the readout).
    fn holes(&self) -> impl Iterator<Item = Bounds<Pixels>> {
        self.boxed.into_iter().chain(self.readout)
    }
}

/// Paints `bounds` (corners rounded `radius`) in `colour` except where `holes` are: where a
/// native meter under the window shows through (native_preview).
fn paint_except(window: &mut Window, bounds: Bounds<Pixels>, holes: &[Bounds<Pixels>], colour: Hsla, radius: Pixels) {
    let holes: Vec<Bounds<Pixels>> = holes.iter().filter_map(|h| h.intersect(&bounds).filter_empty()).collect();
    if holes.is_empty() {
        window.paint_quad(fill(bounds, colour).corner_radii(radius));
        return;
    }
    // bands between the holes' top and bottom edges; in each, what the holes leave of the width
    let mut ys = vec![bounds.top(), bounds.bottom()];
    for h in &holes {
        ys.push(h.top());
        ys.push(h.bottom());
    }
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.dedup();
    for w in ys.windows(2) {
        let (y0, y1) = (w[0], w[1]);
        if y1 <= y0 {
            continue;
        }
        let mut xs: Vec<(Pixels, Pixels)> = holes
            .iter()
            .filter(|h| h.top() < y1 && h.bottom() > y0)
            .map(|h| (h.left(), h.right()))
            .collect();
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut x = bounds.left();
        let mut pieces = vec![];
        for (l, r) in xs {
            if l > x {
                pieces.push((x, l));
            }
            x = x.max(r);
        }
        if x < bounds.right() {
            pieces.push((x, bounds.right()));
        }
        for (l, r) in pieces {
            let piece = Bounds::from_corners(point(l, y0), point(r, y1));
            // the outer corners stay round
            let top = y0 == bounds.top();
            let bottom = y1 == bounds.bottom();
            let corners = Corners {
                top_left: if top && l == bounds.left() { radius } else { px(0.) },
                top_right: if top && r == bounds.right() { radius } else { px(0.) },
                bottom_left: if bottom && l == bounds.left() { radius } else { px(0.) },
                bottom_right: if bottom && r == bounds.right() { radius } else { px(0.) },
            };
            window.paint_quad(fill(piece, colour).corner_radii(corners));
        }
    }
}

trait FilterEmpty: Sized {
    fn filter_empty(self) -> Option<Self>;
}

impl FilterEmpty for Bounds<Pixels> {
    fn filter_empty(self) -> Option<Self> {
        (self.size.width > px(0.) && self.size.height > px(0.)).then_some(self)
    }
}

pub(crate) type Spots = Rc<RefCell<HashMap<Id, MeterSpot>>>;

#[derive(Clone, Copy)]
enum Spot {
    Bar(usize),
    Readout,
    Box,
}

/// How often the meters move (ms): 30 times a second, always (they must stay fluid).
fn meter_ms() -> u64 {
    33
}

/// Peak hold (the marker every console leaves at the recent peak): held this long, then it
/// falls this fast.
const HOLD_S: f32 = 1.5;
const HOLD_FALL_DB_S: f32 = 12.0;
/// The peak readout: the held peak, changing at most this often (a number that moved every
/// frame couldn't be read).
const READOUT_S: f32 = 0.25;

/// One strip's meter as displayed: the bars (rise at once, fall at 24 dB/s), each channel's
/// peak hold and when it was set, and the readout's number and when it last changed.
#[derive(Clone, Copy)]
struct Shown {
    bars: [f32; 2],
    hold: [(f32, Instant); 2],
    readout: (f32, Instant),
}

/// The meters' levels, painted over the mixer where its strips left room for them. Only this
/// redraws 30 times a second; the strips (cached) are laid out again only when they change. In
/// GPUI a view that redraws takes its parents with it, but not their other children.
pub struct MeterOverlay {
    spots: Spots,
    /// Each strip's meter as displayed.
    display: HashMap<Id, Shown>,
    last_frame: Instant,
    /// When the meters last moved.
    moved: Instant,
}

impl MeterOverlay {
    fn new(spots: Spots, cx: &mut Context<Self>) -> Self {
        // about 30 times a second, in step with the canvas's frames when it shows a live picture
        // (`tick`: one window redraw for both); on its own when it doesn't
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(33)).await;
                let alive = this.update(cx, |m, cx| {
                    if m.moved.elapsed() >= std::time::Duration::from_millis(meter_ms() + 17) {
                        m.moved = Instant::now();
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self { spots, display: HashMap::new(), last_frame: Instant::now(), moved: Instant::now() }
    }

    /// The canvas drew a frame: the meters move with it if they're due (same redraw).
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.moved.elapsed() >= std::time::Duration::from_millis(meter_ms() - 3) {
            self.moved = Instant::now();
            cx.notify();
        }
    }
}

impl Render for MeterOverlay {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let spots: Vec<(Id, MeterSpot)> = self.spots.borrow().iter().map(|(id, s)| (*id, *s)).collect();
        self.display.retain(|id, _| spots.iter().any(|(s, _)| s == id));
        let mut paint = Vec::with_capacity(spots.len());
        for (id, spot) in spots {
            let levels = meters().get(id).map(|c| c.levels());
            let quiet = f32::NEG_INFINITY;
            let shown = self.display.entry(id).or_insert(Shown { bars: [quiet; 2], hold: [(quiet, now); 2], readout: (quiet, now) });
            for c in 0..2 {
                let target = levels.map_or(quiet, |l| l.peak[c]);
                // the bar: rises at once, falls at 24 dB/s
                let value = &mut shown.bars[c];
                let fallen = if value.is_finite() { *value - 24.0 * dt } else { quiet };
                *value = if target > fallen { target } else { fallen.max(-90.0) };
                // the hold: set by a new peak, kept a while, then falling slowly, never below the bar
                let (level, since) = &mut shown.hold[c];
                if target >= *level {
                    (*level, *since) = (target, now);
                } else if now.duration_since(*since).as_secs_f32() > HOLD_S {
                    *level = (*level - HOLD_FALL_DB_S * dt).max(shown.bars[c]);
                    if *level <= -90.0 {
                        *level = quiet;
                    }
                }
            }
            // the readout: the held peak, a few times a second
            let held = shown.hold[0].0.max(shown.hold[1].0);
            if now.duration_since(shown.readout.1).as_secs_f32() >= READOUT_S || held > shown.readout.0 {
                shown.readout = (held, now);
            }
            paint.push((spot, *shown));
        }
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                // green below −20, yellow −20…−9, red above −9 dBFS
                let (g_end, y_end) = (meter_frac(-20.0), meter_frac(-9.0));
                for (spot, shown) in &paint {
                    let clip = spot.clip.map(|bounds| ContentMask { bounds });
                    window.with_content_mask(clip, |window| {
                        let colour_at = |frac: f32| if frac > y_end { tok::error() } else if frac > g_end { tok::warn() } else { tok::ok() };
                        for (c, b) in spot.bars.iter().enumerate() {
                            let Some(b) = b else { continue };
                            // the peak hold: a 2-pixel line where the recent peak was
                            let held = shown.hold[c].0;
                            if held > -90.0 {
                                let f = meter_frac(held);
                                let y = b.origin.y + b.size.height * (1.0 - f);
                                window.paint_quad(fill(Bounds::from_corners(point(b.origin.x, y), point(b.origin.x + b.size.width, (y + px(2.)).min(b.origin.y + b.size.height))), colour_at(f)));
                            }
                            let frac = meter_frac(shown.bars[c]);
                            for (from, to, colour) in [(0.0, g_end, tok::ok()), (g_end, y_end, tok::warn()), (y_end, 1.0, tok::error())] {
                                let top = frac.min(to);
                                if top <= from {
                                    continue;
                                }
                                let h = b.size.height;
                                let y0 = b.origin.y + h * (1.0 - top);
                                let y1 = b.origin.y + h * (1.0 - from);
                                window.paint_quad(fill(Bounds::from_corners(point(b.origin.x, y0), point(b.origin.x + b.size.width, y1)), colour));
                            }
                        }
                        if let Some(b) = spot.readout {
                            let peak = shown.readout.0;
                            let text: SharedString = if peak > -90.0 { format!("{peak:.1}").into() } else { "-inf".into() };
                            let colour = if peak > -1.0 { tok::error() } else { tok::text_secondary() };
                            let run = TextRun { len: text.len(), font: font(crate::ui::MONO), color: colour, background_color: None, underline: None, strikethrough: None };
                            let size = px(10.);
                            let line = window.text_system().shape_line(text, size, &[run], None);
                            let line_h = px(12.);
                            let origin = point(b.origin.x, b.origin.y + (b.size.height - line_h) / 2.0);
                            let _ = line.paint(origin, line_h, TextAlign::Center, Some(b.size.width), window, cx);
                        }
                    });
                }
            },
        )
        .size_full()
    }
}

/// The mixer as a dock panel: its strips (cached: redrawn only when they change) and the meter
/// overlay above them.
pub struct MixerDock {
    body: Entity<MixerPanel>,
    /// GPUI's meters (where they aren't drawn natively).
    overlay: Option<Entity<MeterOverlay>>,
}

impl MixerDock {
    pub fn new(body: Entity<MixerPanel>, cx: &mut Context<Self>) -> Self {
        let spots = body.read(cx).spots.clone();
        let overlay = (!crate::native_preview::on()).then(|| cx.new(|cx| MeterOverlay::new(spots, cx)));
        Self { body, overlay }
    }

    /// The canvas drew a frame: the meters move with it if they're due.
    pub fn tick_meters(this: &Entity<Self>, cx: &mut App) {
        if let Some(overlay) = this.read(cx).overlay.clone() {
            overlay.update(cx, |m, cx| m.tick(cx));
        }
    }
}

impl Render for MixerDock {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(self.body.clone().cached(StyleRefinement::default().size_full()))
            .children(self.overlay.clone().map(|o| div().absolute().inset_0().child(o)))
    }
}

impl gpui_kit::component::dock::BasePanel for MixerDock {
    fn panel_name(&self) -> &'static str {
        "mixer"
    }

    /// The natively drawn meters show with the mixer's tab.
    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        let native = self.body.read(cx).native.clone();
        if let Some(n) = native.borrow_mut().as_mut() {
            n.show_meters(active);
        }
    }
}

impl gpui_kit::component::dock::Panel for MixerDock {
    /// None: the tab shows `title` (icon and name).
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        None
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::panel_title("audio-lines", "Audio Mixer")
    }

    fn title_style(&self, _: &App) -> Option<gpui_kit::component::dock::TitleStyle> {
        Some(ui::panel_header_style())
    }

    fn toolbar_buttons(
        &mut self,
        _: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<gpui_kit::component::button::Button>> {
        use gpui_kit::component::Sizable;
        use gpui_kit::component::button::ButtonVariants;
        let body = self.body.clone();
        Some(vec![
            gpui_kit::component::button::Button::new("mixer-add")
                .icon(ui::icon("plus"))
                .ghost()
                .xsmall()
                .tooltip("Add a track, bus or mix")
                .on_click(move |_, window, cx| body.update(cx, |m, cx| m.toggle_add_track(window, cx))),
        ])
    }

    fn title_suffix(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        loudness_readout(cx)
    }

    fn zoom_control(&self, _: &App) -> Option<gpui_kit::component::dock::PanelControl> {
        None
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    /// Not cached as a whole: the meter overlay moves 30 times a second and would take the
    /// strips with it; the strips are cached on their own.
    fn cached(&self, _: &App) -> bool {
        false
    }

    /// Meters drawn under the window (native_preview) show through the mixer's holes.
    fn transparent(&self, _: &App) -> bool {
        crate::native_preview::on()
    }
}

impl EventEmitter<gpui_kit::component::dock::PanelEvent> for MixerDock {}

impl Focusable for MixerDock {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.body.read(cx).focus.clone()
    }
}

/// What a strip's menu offers (its commands, worked out from the workspace).
#[derive(Clone)]
struct StripMenu {
    muted: bool,
    mute: Option<Command>,
    cue: Option<(bool, Command)>,
    reset: Option<Command>,
    remove: Option<(&'static str, Command)>,
    /// The strip to rename (Program keeps its name).
    rename: Option<MixerStrip>,
    strip: MixerStrip,
    color: Option<u32>,
}

impl StripMenu {
    fn of(p: &Workspace, v: &StripView) -> Self {
        Self {
            muted: v.muted,
            mute: mute_cmd(p, v.kind, !v.muted),
            cue: v.cue.and_then(|c| Some((c, cue_cmd(p, v.kind, !c)?))),
            reset: (v.volume_db.abs() >= 0.05).then(|| volume_cmd(p, v.kind, 0.0)).flatten(),
            remove: match v.kind {
                MixerStrip::Track(track) => Some(("Remove track", Command::RemoveTrack { track })),
                MixerStrip::Bus(bus) => Some(("Remove bus", Command::RemoveBus { bus })),
                MixerStrip::Mix(mix) => Some(("Remove mix", Command::RemoveMix { mix })),
                MixerStrip::Master => None,
            },
            rename: (v.kind != MixerStrip::Master).then_some(v.kind),
            strip: v.kind,
            color: v.color,
        }
    }
}

/// A strip's menu (its ⌄ and right-click): mute, listen, back to 0 dB, remove.
fn strip_menu(
    studio: WeakEntity<Studio>,
    me: WeakEntity<MixerPanel>,
    m: StripMenu,
) -> impl Fn(
    gpui_kit::component::menu::PopupMenu,
    &mut Window,
    &mut Context<gpui_kit::component::menu::PopupMenu>,
) -> gpui_kit::component::menu::PopupMenu
+ 'static {
    use gpui_kit::component::menu::PopupMenuItem;
    move |mut menu, window, cx| {
        let run = |cmd: Command| {
            let studio = studio.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                if let Some(s) = studio.upgrade() {
                    s.update(cx, |s, cx| {
                        let _ = s.execute(cmd.clone(), cx);
                    });
                }
            }
        };
        if let Some(cmd) = m.mute.clone() {
            menu = menu.item(
                PopupMenuItem::new(if m.muted { "Unmute" } else { "Mute" })
                    .icon(crate::ui::icon(if m.muted { "volume-2" } else { "volume-x" }))
                    .on_click(run(cmd)),
            );
        }
        if let Some((on, cmd)) = m.cue.clone() {
            menu = menu.item(
                PopupMenuItem::new("Listen in headphones").icon(crate::ui::icon("headphones")).checked(on).on_click(run(cmd)),
            );
        }
        if let Some(strip) = m.rename {
            let me = me.clone();
            menu = menu.item(PopupMenuItem::new("Rename…").icon(crate::ui::icon("pencil")).on_click(
                move |_, window, cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |this, cx| this.start_rename(strip, window, cx));
                    }
                },
            ));
        }
        // the header's colour
        {
            let (studio, strip, current) = (studio.clone(), m.strip, m.color);
            menu = menu.submenu_with_icon(Some(crate::ui::icon("palette")), "Colour", window, cx, move |mut sub, _, _| {
                let set = |color: Option<u32>| {
                    let studio = studio.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        if let Some(s) = studio.upgrade() {
                            s.update(cx, |s, cx| {
                                let _ = s.execute(Command::SetStripColor { strip, color }, cx);
                            });
                        }
                    }
                };
                sub = sub.item(PopupMenuItem::new("None").checked(current.is_none()).on_click(set(None)));
                for &(name, c) in STRIP_COLOURS {
                    sub = sub.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().size(px(11.)).rounded_full().bg(strip_colour(c)))
                                .child(name)
                        })
                        .checked(current == Some(c))
                        .on_click(set(Some(c))),
                    );
                }
                sub
            });
        }
        menu = match m.reset.clone() {
            Some(cmd) => menu.item(PopupMenuItem::new("Reset to 0 dB").on_click(run(cmd))),
            None => menu.item(PopupMenuItem::new("Reset to 0 dB").disabled(true)),
        };
        if let Some((label, cmd)) = m.remove.clone() {
            menu = menu.separator().item(PopupMenuItem::new(label).icon(crate::ui::icon("trash")).on_click(run(cmd)));
        }
        menu
    }
}

/// What a mixer strip shows: a track, a bus, the master or a mix.
struct StripView {
    id: Id,
    kind: MixerStrip,
    name: String,
    volume_db: f32,
    muted: bool,
    /// Tracks have a cue button.
    cue: Option<bool>,
    /// Its effects: id, name, on.
    effects: Vec<(Id, String, bool)>,
    badge: Option<&'static str>,
    /// A track's source (buses, mixes and the master have none).
    source: Option<StripSource>,
    /// Where a track or bus goes and what it sends (mixes and the master have none).
    channel: Option<Channel>,
    /// A mix: whether it's also part of Program.
    mix_to_program: Option<bool>,
    /// Pan (balance, for a mix); none for the master.
    pan: Option<f32>,
    /// Gain before the effects.
    trim: Option<f32>,
    /// The header's colour (0xRRGGBB).
    color: Option<u32>,
}

/// A track's or bus's routing, as the strip shows it.
struct Channel {
    route: Route,
    /// Tracks only.
    invert: Option<bool>,
    mono: Option<bool>,
    record: bool,
    sends: Vec<Send>,
}

/// How the strips are laid out: the same for all of them, so meters line up. A short panel
/// keeps what matters most (name, meter, fader, Cue and Mute), then the effects (a row and the
/// +), then fits in turn the knobs, the source and the routing; what's left grows the effects
/// list to its full height, then lengthens the meters.
#[derive(Clone, Copy)]
struct Layout {
    /// The effects list's height (0: hidden; the header shows how many are on).
    fx_h: f32,
    /// The routing section (output, switches, sends) is shown, this tall.
    routing: bool,
    section: f32,
    /// The source at the foot.
    foot: bool,
    /// The pan and trim knobs.
    knobs: bool,
}

impl Layout {
    /// For a panel `height` tall (0: not measured yet) whose tallest routing section is `section`.
    fn fit(height: f32, section: f32) -> Self {
        let full = Self { fx_h: FX_H, routing: true, section, foot: true, knobs: true };
        if height < 1.0 {
            return full;
        }
        // the panel's padding and the strip's border, and the header; the meter's least
        let mut room = height - 18.0 - 30.0 - LEVELS_MIN;
        // the effects first (what's on a voice matters more than where it's sent)
        // (and nothing else shows before them: a strip too short for them is meter and fader)
        if room < FX_MIN {
            return Self { fx_h: 0.0, routing: false, section, foot: false, knobs: false };
        }
        let mut fx_h = FX_MIN;
        room -= FX_MIN;
        let knobs = room >= LEVELS_H - LEVELS_MIN;
        if knobs {
            room -= LEVELS_H - LEVELS_MIN;
        }
        let foot = room >= FOOT_H;
        if foot {
            room -= FOOT_H;
        }
        let routing = room >= section;
        if routing {
            room -= section;
        }
        // what's left: the effects list at its full height
        fx_h += room.clamp(0.0, FX_H - FX_MIN);
        Self { fx_h, routing, section, foot, knobs }
    }
}

/// What's being dragged: a slider along, or a knob up and down from where it started.
#[derive(Clone, Copy)]
enum KnobDrag {
    Slide(Knob),
    Turn { knob: Knob, y0: f32, v0: f32 },
}

/// A small slider on a strip.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Knob {
    Pan(Id),
    Trim(Id),
    /// A strip's send to a mix.
    Send(Id, Id),
}

impl Knob {
    fn range(self) -> (f32, f32) {
        match self {
            Knob::Pan(_) => (-1.0, 1.0),
            Knob::Trim(_) => (-24.0, 24.0),
            Knob::Send(..) => (-60.0, 6.0),
        }
    }

    /// Filled from the middle (pan, trim) rather than the left end (a send's level).
    fn centred(self) -> bool {
        !matches!(self, Knob::Send(..))
    }

    fn reset(self) -> f32 {
        0.0
    }

    fn set(self, p: &Workspace, v: f32) -> Option<Command> {
        match self {
            Knob::Pan(id) => strip_update(p, id, |f| *f.pan = v).or_else(|| {
                let m = p.audio.mix(id)?;
                Some(Command::UpdateMix { mix: AudioMix { pan: v, ..m.clone() } })
            }),
            Knob::Trim(id) => strip_update(p, id, |f| {
                if let Some(t) = f.trim_db {
                    *t = v
                }
            })
            .or_else(|| {
                if id == lightair_core::MASTER_ID {
                    return Some(Command::UpdateMaster { master: Master { trim_db: v, ..p.audio.master.clone() } });
                }
                let m = p.audio.mix(id)?;
                Some(Command::UpdateMix { mix: AudioMix { trim_db: v, ..m.clone() } })
            }),
            Knob::Send(id, mix) => strip_update(p, id, |f| {
                for s in f.sends.iter_mut().filter(|s| s.mix == mix) {
                    s.level_db = v;
                }
            }),
        }
    }
}

/// The fields a track and a bus share, to change either the same way.
struct StripFields<'a> {
    route: &'a mut Route,
    pan: &'a mut f32,
    record: &'a mut bool,
    sends: &'a mut Vec<Send>,
    trim_db: Option<&'a mut f32>,
    invert: Option<&'a mut bool>,
    mono: Option<&'a mut bool>,
}

/// The command that changes a track or bus through `f`.
fn strip_update(p: &Workspace, id: Id, f: impl FnOnce(StripFields)) -> Option<Command> {
    if let Some(t) = p.track(id) {
        let mut t = (**t).clone();
        f(StripFields {
            route: &mut t.route,
            pan: &mut t.pan,
            record: &mut t.record,
            sends: &mut t.sends,
            trim_db: Some(&mut t.trim_db),
            invert: Some(&mut t.invert),
            mono: Some(&mut t.mono),
        });
        return Some(Command::UpdateTrack { track: t });
    }
    let mut b = p.audio.bus(id)?.clone();
    f(StripFields {
        route: &mut b.route,
        pan: &mut b.pan,
        record: &mut b.record,
        sends: &mut b.sends,
        trim_db: Some(&mut b.trim_db),
        invert: None,
        mono: None,
    });
    Some(Command::UpdateBus { bus: b })
}

/// What a track listens to, for the strip's foot.
struct StripSource {
    icon: &'static str,
    label: String,
    current: TrackSource,
}

/// A track's source, as the strip's source menu offers them.
#[derive(Clone, PartialEq)]
enum TrackSource {
    /// An audio interface the engine opens ("" for the system default).
    Device(String),
    /// Everything the computer plays.
    Desktop,
    /// One app (its bundle id; "" until one is chosen).
    App(String),
    /// Something else (a media object, an older capture): kept until another is chosen.
    Other,
}

impl TrackSource {
    fn of(i: &Source) -> Self {
        match i.kind.as_str() {
            // ("default", as some were saved, is the system's too)
            lightair_engine::devices::DEVICE_KIND => {
                let d = i.settings["device"].as_str().unwrap_or("");
                TrackSource::Device(if lightair_engine::devices::is_system_default(d) { String::new() } else { d.into() })
            }
            kinds::APP_AUDIO => TrackSource::App(i.settings["app"].as_str().unwrap_or("").into()),
            kinds::DESKTOP_AUDIO => TrackSource::Desktop,
            _ => TrackSource::Other,
        }
    }

    /// The source it needs: name, kind, settings.
    fn source(&self, app_name: &str) -> (String, &'static str, serde_json::Value) {
        match self {
            TrackSource::Device(d) => (
                lightair_engine::devices::device_name(d).unwrap_or_else(|| SYSTEM_DEFAULT.into()),
                lightair_engine::devices::DEVICE_KIND,
                json!({ "device": d }),
            ),
            TrackSource::Desktop => ("Desktop audio".into(), kinds::DESKTOP_AUDIO, json!({})),
            TrackSource::App(b) => (app_name.to_string(), kinds::APP_AUDIO, json!({ "app": b })),
            TrackSource::Other => (String::new(), "", json!({})),
        }
    }
}

/// A source menu entry: picking it points `track` at `to`.
fn source_item(
    track: Id,
    to: TrackSource,
    app: String,
    label: String,
    me: WeakEntity<MixerPanel>,
    on: bool,
) -> gpui_kit::component::menu::PopupMenuItem {
    gpui_kit::component::menu::PopupMenuItem::new(label).checked(on).on_click(move |_, _, cx| {
        if let Some(me) = me.upgrade() {
            me.update(cx, |this, cx| {
                let p = this.studio.read(cx).workspace().clone();
                if let Some(cmd) = source_cmd(&p, track, &to, &app) {
                    this.exec(cmd, cx);
                }
            });
        }
    })
}

/// The microphone the OS is set to use (macOS's or Windows' default source).
pub(crate) const SYSTEM_DEFAULT: &str = "System Default";

/// The apps that are open (the Dock's or taskbar's), name and capture id, by name; not
/// Lightair itself.
fn running_apps() -> Vec<(String, String)> {
    lightair_platform::apps::running().into_iter().map(|a| (a.name, a.id)).collect()
}

/// The commands that make `track` listen to `to`: the same kind of source changes its settings;
/// another kind replaces the source (the old one goes if nothing else uses it). A track still
/// named after its old source takes the new one's name.
fn source_cmd(p: &Workspace, track: Id, to: &TrackSource, app_name: &str) -> Option<Command> {
    let t = p.track(track)?;
    let old = p.sources.get(&t.source)?;
    let (name, kind, settings) = to.source(app_name);
    if kind.is_empty() {
        return None;
    }
    // (a name the user gave it stays)
    let renamed = |t: &Track| if t.named { t.name.clone() } else { name.clone() };
    let mut commands = Vec::new();
    let mut new_track = Track { channels: lightair_core::TrackChannels::All, ..(**t).clone() };
    new_track.name = renamed(t);
    if old.kind == kind {
        let mut s = old.settings.clone();
        if let (Some(s), Some(n)) = (s.as_object_mut(), settings.as_object()) {
            for (k, v) in n {
                s.insert(k.clone(), v.clone());
            }
        }
        commands.push(Command::SetSourceSettings { source: old.id, settings: s });
        if old.name != name {
            commands.push(Command::RenameSource { source: old.id, name: name.clone() });
        }
    } else {
        let source = Source::new(name.clone(), kind, settings);
        new_track.source = source.id;
        commands.push(Command::AddSource { source });
    }
    let switched = new_track.source != t.source;
    commands.push(Command::UpdateTrack { track: new_track });
    let shown = p.scenes.iter().any(|s| s.objects.iter().any(|l| l.source == Some(old.id)));
    let shared = p.tracks.iter().any(|o| o.id != track && o.source == old.id);
    if switched && !shown && !shared {
        commands.push(Command::RemoveSource { source: old.id });
    }
    Some(Command::Batch { label: "Change audio source".into(), commands })
}

impl StripView {
    fn kind_order(&self) -> u8 {
        match self.kind {
            MixerStrip::Track(_) => 0,
            MixerStrip::Bus(_) => 1,
            MixerStrip::Mix(_) => 2,
            MixerStrip::Master => 3,
        }
    }
}

/// Tracks, then buses, the master, and the mixes.
fn strip_views(p: &Workspace) -> Vec<StripView> {
    let on = |e: &[Effect]| e.iter().map(|e| (e.id, e.name.clone(), e.enabled)).collect::<Vec<_>>();
    let source = |t: &Track| {
        let i = p.sources.get(&t.source)?;
        let current = TrackSource::of(i);
        let (icon, label) = match &current {
            TrackSource::Device(d) => ("mic", lightair_engine::devices::device_name(d).unwrap_or_else(|| SYSTEM_DEFAULT.into())),
            TrackSource::Desktop => ("volume-2", "Desktop audio".into()),
            TrackSource::App(b) if b.is_empty() => ("app-window", "Pick an app".into()),
            TrackSource::App(_) => ("app-window", i.name.clone()),
            TrackSource::Other => (ui::source_icon(&i.kind), i.name.clone()),
        };
        Some(StripSource { icon, label, current })
    };
    let mut v: Vec<StripView> = p
        .tracks
        .iter()
        .map(|t| StripView {
            id: t.id,
            kind: MixerStrip::Track(t.id),
            name: t.name.clone(),
            volume_db: t.volume_db,
            muted: t.muted,
            cue: Some(t.cue),
            effects: on(&t.effects),
            badge: None,
            source: source(t),
            channel: Some(Channel {
                route: t.route,
                invert: Some(t.invert),
                mono: Some(t.mono),
                record: t.record,
                sends: t.sends.clone(),
            }),
            mix_to_program: None,
            pan: Some(t.pan),
            trim: Some(t.trim_db),
            color: t.color,
        })
        .collect();
    v.extend(p.audio.buses.iter().map(|b| StripView {
        id: b.id,
        kind: MixerStrip::Bus(b.id),
        name: b.name.clone(),
        volume_db: b.volume_db,
        muted: b.muted,
        cue: Some(b.cue),
        effects: on(&b.effects),
        badge: Some("BUS"),
        source: None,
        channel: Some(Channel {
            route: b.route,
            invert: None,
            mono: None,
            record: b.record,
            sends: b.sends.clone(),
        }),
        mix_to_program: None,
        pan: Some(b.pan),
        trim: Some(b.trim_db),
        color: b.color,
    }));
    v.extend(p.audio.mixes.iter().map(|x| StripView {
        id: x.id,
        kind: MixerStrip::Mix(x.id),
        name: x.name.clone(),
        volume_db: x.volume_db,
        muted: x.muted,
        cue: Some(x.cue),
        effects: on(&x.effects),
        badge: Some("MIX"),
        source: None,
        channel: None,
        mix_to_program: Some(x.to_program),
        pan: Some(x.pan),
        trim: Some(x.trim_db),
        color: x.color,
    }));
    // the master last: always the right-most strip
    let m = &p.audio.master;
    v.push(StripView {
        id: lightair_core::MASTER_ID,
        kind: MixerStrip::Master,
        name: "Master".into(),
        volume_db: m.volume_db,
        muted: m.muted,
        cue: Some(m.cue),
        effects: on(&m.effects),
        badge: None, // "Master" says it
        source: None,
        channel: None,
        mix_to_program: None,
        pan: None,
        trim: Some(m.trim_db),
        color: m.color,
    });
    v
}

/// The command that sets a strip's fader.
fn volume_cmd(p: &Workspace, kind: MixerStrip, db: f32) -> Option<Command> {
    Some(match kind {
        MixerStrip::Track(track) => Command::SetTrackVolume { track, volume_db: db },
        MixerStrip::Bus(id) => Command::UpdateBus { bus: Bus { volume_db: db, ..p.audio.bus(id)?.clone() } },
        MixerStrip::Mix(id) => Command::UpdateMix { mix: AudioMix { volume_db: db, ..p.audio.mix(id)?.clone() } },
        MixerStrip::Master => Command::UpdateMaster { master: Master { volume_db: db, ..p.audio.master.clone() } },
    })
}

/// Strip colours offered in its menu (0xRRGGBB): readable as a header with white or dark text.
const STRIP_COLOURS: &[(&str, u32)] = &[
    ("Red", 0xE5484D),
    ("Orange", 0xF76B15),
    ("Amber", 0xFFB224),
    ("Green", 0x30A46C),
    ("Teal", 0x12A594),
    ("Blue", 0x0090FF),
    ("Indigo", 0x3E63DD),
    ("Purple", 0x8E4EC6),
    ("Pink", 0xD6409F),
    ("Grey", 0x8B8D98),
];

fn strip_colour(c: u32) -> Hsla {
    Rgba { r: ((c >> 16) & 0xff) as f32 / 255.0, g: ((c >> 8) & 0xff) as f32 / 255.0, b: (c & 0xff) as f32 / 255.0, a: 1.0 }
        .into()
}

/// Relative luminance (0..1): above about 0.6, dark text reads better on it.
fn luminance(c: u32) -> f32 {
    let ch = |v: u32| {
        let x = (v & 0xff) as f32 / 255.0;
        if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * ch(c >> 16) + 0.7152 * ch(c >> 8) + 0.0722 * ch(c)
}

/// The command that puts a strip in headphones (the cue) or takes it out.
fn cue_cmd(p: &Workspace, kind: MixerStrip, cue: bool) -> Option<Command> {
    Some(match kind {
        MixerStrip::Track(track) => Command::SetTrackCue { track, cue },
        MixerStrip::Bus(id) => Command::UpdateBus { bus: Bus { cue, ..p.audio.bus(id)?.clone() } },
        MixerStrip::Mix(id) => Command::UpdateMix { mix: AudioMix { cue, ..p.audio.mix(id)?.clone() } },
        MixerStrip::Master => Command::UpdateMaster { master: Master { cue, ..p.audio.master.clone() } },
    })
}

/// The command that mutes or unmutes a strip.
fn mute_cmd(p: &Workspace, kind: MixerStrip, muted: bool) -> Option<Command> {
    Some(match kind {
        MixerStrip::Track(track) => Command::SetTrackMuted { track, muted },
        MixerStrip::Bus(id) => Command::UpdateBus { bus: Bus { muted, ..p.audio.bus(id)?.clone() } },
        MixerStrip::Mix(id) => Command::UpdateMix { mix: AudioMix { muted, ..p.audio.mix(id)?.clone() } },
        MixerStrip::Master => Command::UpdateMaster { master: Master { muted, ..p.audio.master.clone() } },
    })
}

#[cfg(test)]
mod layout_tests {
    use super::{FX_H, FX_MIN, Layout};

    #[test]
    fn effects_stay_in_view_before_the_rest() {
        let section = 120.0;
        // tall: everything
        let tall = Layout::fit(900.0, section);
        assert!(tall.knobs && tall.foot && tall.routing && tall.fx_h == FX_H);
        // middling: the effects stay, routing gives way first
        let mid = Layout::fit(420.0, section);
        assert!(mid.fx_h >= FX_MIN, "effects shown at mid height");
        assert!(!mid.routing, "routing gives way before the effects");
        // too short for anything but the meter and fader
        let short = Layout::fit(170.0, section);
        assert_eq!(short.fx_h, 0.0);
        assert!(!short.knobs && !short.routing && !short.foot);
        // wherever the effects show, nothing else shows without them
        for h in (150..1000).step_by(10) {
            let l = Layout::fit(h as f32, section);
            if l.knobs || l.foot || l.routing {
                assert!(l.fx_h >= FX_MIN, "at {h}: something shown while the effects aren't");
            }
        }
    }
}
