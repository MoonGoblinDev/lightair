//! The Inspector's building blocks (docs/05-interface-shell.md, "Inspector"): collapsible
//! sections, compact numeric fields, sliders, and the object view (Transform, content, Effects,
//! Lightair Effect Stack, Canvases).

use std::rc::Rc;

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::{Command, Fit, Canvas, Object, Layout, Workspace, Scene, Shape, ShapeKind};

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

    /// A labelled number with small −/+ (shift-free steps of `step`).
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
                .size(px(20.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(tok::text_tertiary())
                .hover(|d| d.bg(tok::bg_selected()).text_color(tok::text()))
                .child(ui::icon(icon).size(px(12.)))
                .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx)))
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .h(px(30.))
            .px_2()
            .rounded(px(6.))
            .bg(tok::bg_raised())
            .child(div().w(px(16.)).text_size(px(11.)).text_color(tok::text_tertiary()).child(label))
            .child(div().flex_1().text_size(px(12.)).font_family(crate::ui::MONO).child(value))
            .child(small(0, "minus", minus))
            .child(small(1, "plus", plus))
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
        Slider::new(&sliders[&key].state)
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

        // Transform.
        let grid = div()
            .grid()
            .grid_cols(2)
            .gap_1()
            .child(self.field("tx", "X", format!("{:.0}", lo.x), set(&|l| l.x -= 10.0), set(&|l| l.x += 10.0), cx))
            .child(self.field("ty", "Y", format!("{:.0}", lo.y), set(&|l| l.y -= 10.0), set(&|l| l.y += 10.0), cx))
            .child(self.field(
                "tw",
                "W",
                format!("{:.0}", lo.width),
                set(&|l| l.width = (l.width - 10.0).max(8.0)),
                set(&|l| l.width += 10.0),
                cx,
            ))
            .child(self.field(
                "th",
                "H",
                format!("{:.0}", lo.height),
                set(&|l| l.height = (l.height - 10.0).max(8.0)),
                set(&|l| l.height += 10.0),
                cx,
            ))
            .child(self.field(
                "tr",
                "↻",
                format!("{:.0}°", lo.rotation),
                set(&|l| l.rotation -= 15.0),
                set(&|l| l.rotation += 15.0),
                cx,
            ));
        let mut fits = div().flex().gap_0p5().p_0p5().rounded(px(6.)).bg(tok::bg_raised());
        for (i, (fit, label, tip)) in [
            (Fit::Fit, "Fit", "Whole picture, inside the box"),
            (Fit::Fill, "Fill", "Cover the box, crop the overflow"),
            (Fit::Stretch, "Stretch", "Stretch to the box"),
            (Fit::Original, "1:1", "Original size"),
        ]
        .into_iter()
        .enumerate()
        {
            let cmd = Command::SetLayout { scene: sid, object: lid, canvas: fid, layout: Layout { fit, ..lo } };
            let on = lo.fit == fit;
            fits = fits.child(
                div()
                    .id(("fit", i))
                    .flex_1()
                    .text_center()
                    .py_1()
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                    .when(!on, |d| d.text_color(tok::text_tertiary()))
                    .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx))),
            );
        }
        // Crop is on the canvas (the side handles); here, undo it.
        let cropped = lo.crop != lightair_core::Crop::default();
        let reset = set(&|l| l.crop = lightair_core::Crop::default());
        let crop_actions = div().flex().gap_1().when(cropped, |d| {
            d.child(
                Button::new("reset-crop")
                    .label("Uncrop")
                    .xsmall()
                    .ghost()
                    .tooltip("Drag a side handle on the canvas to crop")
                    .on_click(cx.listener(move |this, _, _, cx| this.exec(reset.clone(), cx))),
            )
        });
        let mut transform =
            self.section("Transform", Some(crop_actions.into_any_element()), cx).child(grid).child(fits);
        // Flip and 3D.
        let toggle = |id: &'static str,
                      icon: &'static str,
                      tip: &'static str,
                      on: bool,
                      cmd: Command,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded(px(6.))
                .cursor_pointer()
                .bg(if on { tok::bg_selected() } else { tok::bg_raised() })
                .hover(|d| d.bg(tok::bg_selected()))
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .child(ui::icon(icon).size(px(14.)).text_color(if on { tok::text() } else { tok::text_secondary() }))
                .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx)))
        };
        let flip_h = set(&|l| l.flip_h = !l.flip_h);
        let flip_v = set(&|l| l.flip_v = !l.flip_v);
        let flat = set(&|l| {
            l.tilt_x = 0.0;
            l.tilt_y = 0.0;
        });
        let tilted = lo.tilt_x.abs() > 0.01 || lo.tilt_y.abs() > 0.01;
        transform = transform.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(toggle("flip-h", "flip-horizontal-2", "Flip left↔right", lo.flip_h, flip_h, cx))
                .child(toggle("flip-v", "flip-vertical-2", "Flip top↕bottom", lo.flip_v, flip_v, cx))
                .child(div().flex_1())
                .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("3D: drag the ⊙ handle"))
                .when(tilted, |d| {
                    d.child(
                        Button::new("flat")
                            .label("Flat")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| this.exec(flat.clone(), cx))),
                    )
                }),
        );
        if tilted {
            let row = |label: &'static str, slider: Slider, value: String| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(64.)).text_size(px(12.)).text_color(tok::text_secondary()).child(label))
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
            };
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
            transform = transform.child(row("Tilt ↕", tx, format!("{:.0}°", lo.tilt_x))).child(row(
                "Tilt ↔",
                ty,
                format!("{:.0}°", lo.tilt_y),
            ));
        }
        // Shape: part of the object's frame, cut from the cropped picture.
        let shape = lo.shape;
        let with_shape = |sh: Option<Shape>| Command::SetLayout {
            scene: sid,
            object: lid,
            canvas: fid,
            layout: Layout { shape: sh, ..lo },
        };
        let keep = |kind: ShapeKind| Some(Shape { kind, ..shape.unwrap_or(Shape::new(kind)) });
        transform = transform.child(self.segmented(
            "shape",
            vec![
                ("Rectangle", with_shape(None), shape.is_none()),
                ("Rounded", with_shape(keep(ShapeKind::Rounded)), shape.is_some_and(|s| s.kind == ShapeKind::Rounded)),
                ("Circle", with_shape(keep(ShapeKind::Circle)), shape.is_some_and(|s| s.kind == ShapeKind::Circle)),
                ("Pill", with_shape(keep(ShapeKind::Pill)), shape.is_some_and(|s| s.kind == ShapeKind::Pill)),
            ],
            cx,
        ));
        if let Some(sh) = shape {
            let row = |label: &'static str, slider: Slider, value: String| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(64.)).text_size(px(12.)).text_color(tok::text_secondary()).child(label))
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
            };
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
                transform = transform.child(row("Corners", s, format!("{:.0}%", sh.radius * 100.0)));
            }
            let s = self.slider(key("feather"), (0.0, 40.0, 1.0), sh.feather, set_shape(|s, v| s.feather = v), cx);
            transform = transform.child(row("Softness", s, format!("{:.0} px", sh.feather)));
            let s = self.slider(key("border"), (0.0, 40.0, 1.0), sh.border, set_shape(|s, v| s.border = v), cx);
            transform = transform.child(row("Border", s, format!("{:.0} px", sh.border)));
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
                transform = transform.child(swatches);
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
