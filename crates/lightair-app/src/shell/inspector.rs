//! The Inspector's building blocks (docs/05-interface-shell.md, "Inspector"): collapsible
//! sections, compact numeric fields, sliders, and the object view (Transform, content, Effects,
//! Lightair Effect Stack, Canvases).

use std::rc::Rc;

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input as TextInput, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_model::{Command, Fit, Canvas, Object, Layout, Workspace, Scene, Shape, ShapeKind};

use super::StudioWindow;
use crate::ui::{self, tok};

const HEADER_H: f32 = 34.0;

/// A slider bound to a document value: recreated when the value changes elsewhere.
pub(super) struct BoundSlider {
    state: Entity<SliderState>,
    value: f32,
    dragging: bool,
    on_change: Rc<dyn Fn(f32) -> Command>,
    _events: Subscription,
}

/// A number being dragged sideways (`number`): where the drag began, the value then, how much a
/// point of travel changes it, and the command a new value makes.
pub(super) struct Scrub {
    key: SharedString,
    x0: f32,
    v0: f32,
    step: f32,
    /// Values are multiples of this.
    snap: f32,
    moved: bool,
    apply: Rc<dyn Fn(f32) -> Command>,
}

/// A number being typed (`number`, clicked without dragging).
pub(super) struct NumberEdit {
    key: SharedString,
    input: Entity<InputState>,
    apply: Rc<dyn Fn(f32) -> Command>,
    _events: Subscription,
}

/// A number as the Inspector shows it: whole, without "-0".
fn whole(v: f32) -> String {
    let r = v.round();
    format!("{:.0}", if r == 0.0 { 0.0 } else { r })
}

impl StudioWindow {
    /// A section with a header that collapses it (remembered per title). `action` sits at the
    /// header's right.
    pub(super) fn section(&self, title: &'static str, action: Option<AnyElement>, cx: &mut Context<Self>) -> Div {
        let open = !self.collapsed.borrow().contains(title);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .pb_3()
            .border_b_1()
            .border_color(tok::border())
            .when(!open, |d| d.h(px(HEADER_H)).pb_0().overflow_hidden())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(HEADER_H))
                    .flex_none()
                    .child(
                        div()
                            .id(SharedString::from(format!("section-{title}")))
                            .flex()
                            .flex_1()
                            .items_center()
                            .gap_1()
                            .h_full()
                            .cursor_pointer()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tok::text_secondary())
                            .hover(|d| d.text_color(tok::text()))
                            .child(ui::icon(if open { "chevron-down" } else { "chevron-right" }).size(px(12.)))
                            .child(title.to_uppercase())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mut c = this.collapsed.borrow_mut();
                                if !c.remove(title) {
                                    c.insert(title);
                                }
                                drop(c);
                                cx.notify();
                            })),
                    )
                    .children(action),
            )
    }

    /// A labelled number: drag it sideways to change it (⇧ ten times faster, ⌥ finer; one undo
    /// step per drag), or click it to type one (Enter or clicking away keeps it, Escape doesn't).
    /// `step`: the change per point dragged.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn number(
        &self,
        key: impl Into<SharedString>,
        label: AnyElement,
        value: f32,
        unit: &'static str,
        step: f32,
        apply: impl Fn(f32) -> Command + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.number_snapped(key, label, value, unit, step, 1.0, apply, cx)
    }

    /// `number`, its values multiples of `snap`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn number_snapped(
        &self,
        key: impl Into<SharedString>,
        label: AnyElement,
        value: f32,
        unit: &'static str,
        step: f32,
        snap: f32,
        apply: impl Fn(f32) -> Command + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key: SharedString = key.into();
        let apply: Rc<dyn Fn(f32) -> Command> = Rc::new(apply);
        let editing = self.number_edit.as_ref().filter(|e| e.key == key).map(|e| e.input.clone());
        let scrubbing = self.scrub.as_ref().is_some_and(|s| s.key == key);
        let weak = cx.weak_entity();
        div()
            .id(ElementId::Name(key.clone()))
            .relative()
            .flex()
            .items_center()
            .gap_1p5()
            .h(px(26.))
            .min_w_0()
            .pl_2()
            .pr_1()
            .rounded(px(5.))
            .bg(tok::bg_raised())
            .border_1()
            .border_color(if editing.is_some() || scrubbing { tok::border_strong() } else { tok::bg_raised() })
            .hover(|d| d.border_color(tok::border_strong()))
            .child(div().flex().flex_none().items_center().text_size(px(11.)).text_color(tok::text_tertiary()).child(label))
            .map(|d| match editing {
                Some(input) => d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            if ev.keystroke.key == "escape" {
                                cx.stop_propagation();
                                this.finish_number(false, window, cx);
                            }
                        }))
                        .child(TextInput::new(&input).appearance(false).xsmall()),
                ),
                None => d
                    .cursor(CursorStyle::ResizeLeftRight)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .font_family(crate::ui::MONO)
                            .child(format!("{}{unit}", whole(value))),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let x0 = f32::from(ev.position.x);
                            let key = key.clone();
                            this.scrub = Some(Scrub { key, x0, v0: value, step, snap, moved: false, apply: apply.clone() });
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| this.scrub_up(window, cx))),
            })
            // while dragged: the pointer followed anywhere in the window
            .when(scrubbing, |d| {
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let w = weak.clone();
                            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    let _ = w.update(cx, |this, cx| this.scrub_move(ev, cx));
                                }
                            });
                            let w = weak.clone();
                            window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                                if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                                    let _ = w.update(cx, |this, cx| this.scrub_up(window, cx));
                                }
                            });
                        },
                    )
                    .absolute()
                    .size_0(),
                )
            })
    }

    /// A labelled value with small −/+ (for one that isn't a plain number: a duration, a zoom).
    pub(super) fn field(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        minus: Command,
        plus: Command,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let small = |i: usize, icon: &'static str, cmd: Command| {
            div()
                .id((id, i))
                .flex()
                .items_center()
                .justify_center()
                .size(px(18.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(tok::text_tertiary())
                .hover(|d| d.bg(tok::bg_selected()).text_color(tok::text()))
                .child(ui::icon(icon).size(px(11.)))
                .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx)))
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .h(px(26.))
            .pl_2()
            .pr_1()
            .rounded(px(5.))
            .bg(tok::bg_raised())
            .when(!label.is_empty(), |d| d.child(div().flex_none().text_size(px(11.)).text_color(tok::text_tertiary()).child(label)))
            .child(div().flex_1().min_w_0().text_size(px(12.)).font_family(crate::ui::MONO).child(value))
            .child(small(0, "minus", minus))
            .child(small(1, "plus", plus))
    }

    fn scrub_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(s) = &mut self.scrub else { return };
        let dx = f32::from(ev.position.x) - s.x0;
        let first = !s.moved;
        if first {
            if dx.abs() < 3.0 {
                return;
            }
            s.moved = true;
        }
        let rate = if ev.modifiers.shift { 10.0 } else if ev.modifiers.alt { 0.2 } else { 1.0 };
        let snap = s.snap.max(f32::EPSILON);
        let cmd = (s.apply)(((s.v0 + dx * s.step * rate) / snap).round() * snap);
        if first {
            self.studio.update(cx, |st, _| st.begin_gesture());
        }
        self.exec(cmd, cx);
    }

    /// The drag ends; a click (no drag) opens the number for typing.
    fn scrub_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.scrub.take() else { return };
        if s.moved {
            self.studio.update(cx, |st, _| st.end_gesture());
        } else {
            let input = cx.new(|cx| InputState::new(window, cx));
            input.update(cx, |state, cx| {
                state.set_value(whole(s.v0), window, cx);
                state.focus(window, cx);
                state.select_all(window, cx);
            });
            let events = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.finish_number(true, window, cx),
                InputEvent::Blur => this.finish_number(true, window, cx),
                _ => {}
            });
            self.number_edit = Some(NumberEdit { key: s.key, input, apply: s.apply, _events: events });
        }
        cx.notify();
    }

    /// Ends typing a number: the value typed is kept (`commit`), if it's a number.
    fn finish_number(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.number_edit.take() else { return };
        let typed = e.input.read(cx).value().to_string();
        let typed = typed.trim().trim_end_matches(['°', '%']).trim_end_matches("px").trim();
        if commit && let Ok(v) = typed.parse::<f32>() && v.is_finite() {
            self.exec((e.apply)(v), cx);
        }
        if e.input.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// For screenshots: the selected object rounded, with a border, tilted and cropped (so every
    /// Transform row shows).
    pub(super) fn inspect_shape_hook(&mut self, cx: &mut Context<Self>) {
        let cmd = {
            let s = self.studio.read(cx);
            let workspace = s.workspace();
            let canvas = self.canvas_id(workspace);
            let scene = s.selected_scene.and_then(|id| workspace.scene(id));
            let object = scene.and_then(|sc| s.selected_object.and_then(|id| sc.object(id)));
            match (scene, object) {
                (Some(sc), Some(o)) => o.layouts.get(&canvas).map(|lo| {
                    let mut l = *lo;
                    l.shape = Some(Shape { border: 6.0, ..Shape::new(ShapeKind::Rounded) });
                    l.tilt_y = 18.0;
                    l.crop.left = 40;
                    Command::SetLayout { scene: sc.id, object: o.id, canvas, layout: l }
                }),
                _ => None,
            }
        };
        if let Some(cmd) = cmd {
            self.exec(cmd, cx);
        }
    }

    /// A row of mutually exclusive options; `options` are (label, command, selected).
    pub(super) fn segmented(
        &self,
        id: &'static str,
        options: Vec<(&'static str, Command, bool)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = div().flex().gap_0p5().p_0p5().rounded(px(6.)).bg(tok::bg_raised());
        for (i, (label, cmd, on)) in options.into_iter().enumerate() {
            row = row.child(
                div()
                    .id((id, i))
                    .flex_1()
                    .text_center()
                    .py_1()
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                    .when(!on, |d| d.text_color(tok::text_tertiary()).hover(|d| d.text_color(tok::text_secondary())))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !on {
                            this.exec(cmd.clone(), cx)
                        }
                    })),
            );
        }
        row
    }

    /// A slider whose changes are one undo step per drag.
    pub(super) fn slider(
        &self,
        key: String,
        range: (f32, f32, f32),
        value: f32,
        on_change: impl Fn(f32) -> Command + 'static,
        cx: &mut Context<Self>,
    ) -> Slider {
        let on_change: Rc<dyn Fn(f32) -> Command> = Rc::new(on_change);
        let mut sliders = self.sliders.borrow_mut();
        let stale = sliders.get(&key).is_none_or(|s| !s.dragging && (s.value - value).abs() > f32::EPSILON);
        if stale {
            let (min, max, step) = range;
            let state = cx.new(|_| SliderState::new().min(min).max(max).step(step).default_value(value));
            let k = key.clone();
            let events = cx.subscribe(&state, move |this, _, ev: &SliderEvent, cx| {
                let (v, release) = match ev {
                    SliderEvent::Change(v) => (v.end(), false),
                    SliderEvent::Release(v) => (v.end(), true),
                };
                let cmd = {
                    let mut sliders = this.sliders.borrow_mut();
                    let Some(s) = sliders.get_mut(&k) else { return };
                    if !s.dragging {
                        s.dragging = true;
                        this.studio.update(cx, |st, _| st.begin_gesture());
                    }
                    s.value = v;
                    if release {
                        s.dragging = false;
                    }
                    (s.on_change)(v)
                };
                this.exec(cmd, cx);
                if release {
                    this.studio.update(cx, |st, _| st.end_gesture());
                }
            });
            sliders.insert(
                key.clone(),
                BoundSlider { state, value, dragging: false, on_change: on_change.clone(), _events: events },
            );
        } else if let Some(s) = sliders.get_mut(&key) {
            s.on_change = on_change;
        }
        let slider = Slider::new(&sliders[&key].state);
        // a range around zero (pan, trim, bipolar effects): filled outward from the centre
        if range.0 < 0. && range.1 > 0. { slider.fill_from(0.) } else { slider }
    }

    /// The Inspector for one object.
    pub(super) fn object_inspector(
        &self,
        workspace: &Workspace,
        scene: &Scene,
        object: &Object,
        canvas: &Canvas,
        body: Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lo = object.layouts.get(&canvas.id).copied().unwrap_or(Layout::boxed(0., 0., 100., 100.));
        let (sid, lid, fid) = (scene.id, object.id, canvas.id);
        let source = object.source.and_then(|i| workspace.sources.get(&i)).cloned();
        let kind = source.as_ref().map(|i| i.kind.clone()).unwrap_or_default();
        let set = move |f: &dyn Fn(&mut Layout)| {
            let mut l = lo;
            f(&mut l);
            Command::SetLayout { scene: sid, object: lid, canvas: fid, layout: l }
        };

        // Header: what it is, and the two actions people reach for.
        let full = set(&|l| {
            *l =
                Layout { x: 0.0, y: 0.0, width: canvas.width as f32, height: canvas.height as f32, rotation: 0.0, ..*l }
        });
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .py_2()
            .child(ui::icon(ui::source_icon(&kind)).size(px(16.)).text_color(tok::text_secondary()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(object.name.clone()),
            )
            .child(
                ui::icon_button("fill-canvas", "maximize", tok::text_secondary())
                    .tooltip(|w, cx| Tooltip::new("Fill the canvas").build(w, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.exec(full.clone(), cx))),
            )
            .child(
                ui::icon_button("delete-object", "trash", tok::text_secondary())
                    .tooltip(|w, cx| Tooltip::new("Delete object").build(w, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.exec(Command::RemoveObject { scene: sid, object: lid }, cx)
                    })),
            );

        // Transform: position and rotation, then size and flips, three to a row; then how the
        // picture fits its box, and its shape.
        let at = move |f: fn(&mut Layout, f32)| {
            move |v: f32| {
                let mut l = lo;
                f(&mut l, v);
                Command::SetLayout { scene: sid, object: lid, canvas: fid, layout: l }
            }
        };
        let letter = |text: &'static str| div().w(px(10.)).child(text).into_any_element();
        let flip = |id: &'static str, icon: &'static str, tip: &'static str, on: bool, cmd: Command, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .h(px(26.))
                .rounded(px(5.))
                .cursor_pointer()
                .bg(if on { tok::bg_selected() } else { tok::bg_raised() })
                .hover(|d| d.bg(tok::bg_selected()))
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .child(ui::icon(icon).size(px(14.)).text_color(if on { tok::text() } else { tok::text_secondary() }))
                .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx)))
        };
        let flip_h = set(&|l| l.flip_h = !l.flip_h);
        let flip_v = set(&|l| l.flip_v = !l.flip_v);
        let grid = div()
            .grid()
            .grid_cols(3)
            .gap_1()
            .child(self.number("tx", letter("X"), lo.x, "", 1.0, at(|l, v| l.x = v), cx))
            .child(self.number("ty", letter("Y"), lo.y, "", 1.0, at(|l, v| l.y = v), cx))
            .child(self.number(
                "tr",
                ui::icon("rotate-cw").size(px(11.)).into_any_element(),
                lo.rotation,
                "°",
                0.5,
                at(|l, v| l.rotation = v),
                cx,
            ))
            .child(self.number("tw", letter("W"), lo.width, "", 1.0, at(|l, v| l.width = v.max(8.0)), cx))
            .child(self.number("th", letter("H"), lo.height, "", 1.0, at(|l, v| l.height = v.max(8.0)), cx))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(flip("flip-h", "flip-horizontal-2", "Flip left↔right", lo.flip_h, flip_h, cx))
                    .child(flip("flip-v", "flip-vertical-2", "Flip top↕bottom", lo.flip_v, flip_v, cx)),
            );
        let fit_to = |fit: Fit| Command::SetLayout { scene: sid, object: lid, canvas: fid, layout: Layout { fit, ..lo } };
        let fits = self.segmented(
            "fit",
            vec![
                ("Fit", fit_to(Fit::Fit), lo.fit == Fit::Fit),
                ("Fill", fit_to(Fit::Fill), lo.fit == Fit::Fill),
                ("Stretch", fit_to(Fit::Stretch), lo.fit == Fit::Stretch),
                ("1:1", fit_to(Fit::Original), lo.fit == Fit::Original),
            ],
            cx,
        );
        // Shape: part of the object's frame, cut from the cropped picture.
        let shape = lo.shape;
        let with_shape = |sh: Option<Shape>| Command::SetLayout {
            scene: sid,
            object: lid,
            canvas: fid,
            layout: Layout { shape: sh, ..lo },
        };
        let keep = |kind: ShapeKind| Some(Shape { kind, ..shape.unwrap_or(Shape::new(kind)) });
        let shapes = self.segmented(
            "shape",
            vec![
                ("None", with_shape(None), shape.is_none()),
                ("Rounded", with_shape(keep(ShapeKind::Rounded)), shape.is_some_and(|s| s.kind == ShapeKind::Rounded)),
                ("Circle", with_shape(keep(ShapeKind::Circle)), shape.is_some_and(|s| s.kind == ShapeKind::Circle)),
                ("Pill", with_shape(keep(ShapeKind::Pill)), shape.is_some_and(|s| s.kind == ShapeKind::Pill)),
            ],
            cx,
        );
        // a setting's name, then its control
        let row = |label: &'static str, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_none().w(px(56.)).text_size(px(12.)).text_color(tok::text_secondary()).child(label))
                .child(div().flex_1().min_w_0().child(control))
        };
        // a slider's row: its value at the right
        let slider_row = |label: &'static str, slider: Slider, value: String| {
            row(
                label,
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(slider))
                    .child(
                        div()
                            .w(px(40.))
                            .text_right()
                            .text_size(px(11.))
                            .font_family(crate::ui::MONO)
                            .text_color(tok::text_tertiary())
                            .child(value),
                    )
                    .into_any_element(),
            )
        };

        // In the header: undo a crop or a tilt (both made on the canvas), and how to make them.
        let cropped = lo.crop != lightair_model::Crop::default();
        let reset = set(&|l| l.crop = lightair_model::Crop::default());
        let flat = set(&|l| {
            l.tilt_x = 0.0;
            l.tilt_y = 0.0;
        });
        let tilted = lo.tilt_x.abs() > 0.01 || lo.tilt_y.abs() > 0.01;
        let actions = div()
            .flex()
            .items_center()
            .gap_1()
            .when(cropped, |d| {
                d.child(
                    Button::new("reset-crop")
                        .label("Uncrop")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| this.exec(reset.clone(), cx))),
                )
            })
            .when(tilted, |d| {
                d.child(
                    Button::new("flat")
                        .label("Flat")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| this.exec(flat.clone(), cx))),
                )
            })
            .child(ui::info("transform-info", "On the canvas: drag a side handle to crop, and the ⊙ handle to tilt in 3D"));
        let mut transform = self
            .section("Transform", Some(actions.into_any_element()), cx)
            .child(grid)
            .child(row("Fit", fits.into_any_element()));
        if tilted {
            let key = |name: &str| format!("tilt:{lid}:{fid}:{name}");
            let tx = self.slider(
                key("x"),
                (-75.0, 75.0, 1.0),
                lo.tilt_x,
                move |v| Command::SetLayout { scene: sid, object: lid, canvas: fid, layout: Layout { tilt_x: v, ..lo } },
                cx,
            );
            let ty = self.slider(
                key("y"),
                (-75.0, 75.0, 1.0),
                lo.tilt_y,
                move |v| Command::SetLayout { scene: sid, object: lid, canvas: fid, layout: Layout { tilt_y: v, ..lo } },
                cx,
            );
            transform = transform
                .child(slider_row("Tilt ↕", tx, format!("{:.0}°", lo.tilt_x)))
                .child(slider_row("Tilt ↔", ty, format!("{:.0}°", lo.tilt_y)));
        }
        transform = transform.child(row("Shape", shapes.into_any_element()));
        if let Some(sh) = shape {
            let set_shape = move |f: fn(&mut Shape, f32)| {
                move |v: f32| {
                    let mut s = sh;
                    f(&mut s, v);
                    Command::SetLayout { scene: sid, object: lid, canvas: fid, layout: Layout { shape: Some(s), ..lo } }
                }
            };
            let key = |name: &str| format!("shape:{lid}:{fid}:{name}");
            if sh.kind == ShapeKind::Rounded {
                let s = self.slider(key("radius"), (0.0, 1.0, 0.01), sh.radius, set_shape(|s, v| s.radius = v), cx);
                transform = transform.child(slider_row("Corners", s, format!("{:.0}%", sh.radius * 100.0)));
            }
            let s = self.slider(key("feather"), (0.0, 40.0, 1.0), sh.feather, set_shape(|s, v| s.feather = v), cx);
            transform = transform.child(slider_row("Softness", s, format!("{:.0} px", sh.feather)));
            let s = self.slider(key("border"), (0.0, 40.0, 1.0), sh.border, set_shape(|s, v| s.border = v), cx);
            transform = transform.child(slider_row("Border", s, format!("{:.0} px", sh.border)));
            if sh.border > 0.0 {
                let current = super::object_props::rgb_of(sh.border_color);
                let mut swatches = div().flex().flex_wrap().gap_1p5();
                for (i, &rgb) in super::object_props::PALETTE.iter().enumerate() {
                    let colour = Shape { border_color: super::object_props::abgr(rgb), ..sh };
                    let cmd = with_shape(Some(colour));
                    swatches = swatches.child(
                        div()
                            .id(("border-swatch", i))
                            .size(px(20.))
                            .rounded_full()
                            .cursor_pointer()
                            .bg(super::object_props::rgb_to_hsla(rgb))
                            .border_2()
                            .border_color(if current == rgb { tok::text() } else { tok::border() })
                            .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx))),
                    );
                }
                transform = transform.child(row("", swatches.into_any_element()));
            }
        }

        let mut col = body.gap_0().child(header);
        if let Some(source) = source.as_ref().and_then(|i| self.source_section(i, cx)) {
            col = col.child(source);
        }
        col = col.child(transform);
        if let Some(source) = &source {
            if let Some(content) = self.content_section(source, cx) {
                col = col.child(content);
            }
            col = col.child(self.effects_section(source, cx));
        }
        if workspace.canvases.len() > 1 {
            col = col.child(self.canvases_section(workspace, object, sid, cx));
        }
        col.into_any_element()
    }
}
