//! The scene Inspector's "Screen zoom & pointer" (docs/plans/screen-studio.md, live): how the
//! scene's screen captures zoom (in to a click, or always following the pointer), and how the
//! pointer and clicks are drawn on them. The engine puts it on every screen capture in the
//! scene (`lightair_engine::cursor`).

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::slider::Slider;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::{Command, Workspace, Scene, ScreenZoomPointer};

use super::StudioWindow;
use crate::ui::{self, tok};

fn label(text: &'static str) -> Div {
    div().w(px(76.)).flex_none().text_size(px(12.)).text_color(tok::text_secondary()).child(text)
}

fn row(name: &'static str, control: impl IntoElement) -> Div {
    div().flex().items_center().gap_2().child(label(name)).child(div().flex_1().min_w_0().child(control))
}

fn slider_row(name: &'static str, slider: Slider, value: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(label(name))
        .child(div().flex_1().child(slider))
        .child(div().w(px(40.)).text_right().text_size(px(11.)).font_family(crate::ui::MONO).text_color(tok::text_tertiary()).child(value))
}

/// A label with its description on hover (an ⓘ beside it).
fn tip_row(name: &'static str, tip: &'static str, control: impl IntoElement) -> Div {
    let name_tip = div().w(px(76.)).flex_none().flex().items_center().gap_1().child(div().text_size(px(12.)).text_color(tok::text_secondary()).child(name)).child(ui::info(name, tip));
    div().flex().items_center().gap_2().child(name_tip).child(div().flex_1().min_w_0().child(control))
}

/// A part's heading ("ZOOM"), with what its choice does on hover.
fn heading(text: &'static str, tip: &'static str) -> Div {
    div()
        .pt_2()
        .flex()
        .items_center()
        .gap_1()
        .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text_tertiary()).child(text))
        .child(ui::info(text, tip))
}

fn note(text: &'static str) -> Div {
    // (min_w_0: in a row, it wraps instead of running past the edge)
    div().min_w_0().text_size(px(11.)).text_color(tok::text_tertiary()).child(text)
}

impl StudioWindow {
    /// The section, for `scene`.
    pub(super) fn screen_zoom_pointer_section(&self, workspace: &Workspace, scene: &Scene, cx: &mut Context<Self>) -> Div {
        let sid = scene.id;
        let on = scene.screen.is_some();
        let look = scene.screen.unwrap_or_default();
        let set = move |l: ScreenZoomPointer| Command::SetScreenZoomPointer { scene: sid, screen: Some(l) };
        let toggle = super::settings::switch("screen-look-on", on)
            .tooltip(ui::tip("Zoom in where you click and draw a bigger, smoother pointer with click effects on this scene's screen, live, as Screen Studio does."))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.exec(Command::SetScreenZoomPointer { scene: sid, screen: if on { None } else { Some(ScreenZoomPointer::default()) } }, cx)
            }));
        let mut body = self.section("Screen zoom & pointer", Some(toggle.into_any_element()), cx);
        let screens = scene
            .objects
            .iter()
            .filter_map(|l| l.source.and_then(|i| workspace.sources.get(&i)))
            .filter(|i| i.kind == lightair_core::kinds::SCREEN)
            .count();
        if !on {
            return body;
        }
        if screens == 0 {
            body = body.child(note("Add a screen to this scene: it zooms and draws the pointer there."));
        }
        // clicks (for the zoom and their effects) need the OS to let Lightair see them
        let wants_clicks = look.zoom == ScreenZoomPointer::ZOOM_CLICKS || (look.pointer_drawn && !look.pointer_hidden && look.clicks > 0);
        if wants_clicks && !lightair_platform::input::can_listen() {
            body = body.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .p_2()
                    .rounded(px(6.))
                    .bg(tok::bg_raised())
                    .child(div().flex_none().pt(px(1.)).child(ui::icon("info").size(px(14.)).text_color(tok::text_secondary())))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_2()
                            .child(note("Clicks need Input Monitoring for Lightair. Until it's on, zooms don't follow clicks and clicks aren't drawn."))
                            .child(
                                // asks macOS (which lists Lightair in the pane, off), then shows
                                // the pane to turn it on there
                                Button::new("screen-input-monitoring")
                                    .label("Allow in Settings…")
                                    .icon(ui::icon("external-link"))
                                    .xsmall()
                                    .outline()
                                    .on_click(|_, _, _| {
                                        if !lightair_platform::input::request_listen() {
                                            lightair_platform::input::open_input_monitoring_settings();
                                        }
                                    }),
                            )
                            .child(note("Switch Lightair on in the list. macOS may ask you to quit and reopen it.")),
                    ),
            );
        }
        // ---- zoom
        body = body
            .child(heading(
                "ZOOM",
                match look.zoom {
                    ScreenZoomPointer::ZOOM_CLICKS => "On clicks: zooms in where you click (and, when typing, where you clicked last) and follows the pointer, then back out once you stop.",
                    ScreenZoomPointer::ZOOM_TOGGLE => "Toggle: press the key to zoom in and follow the pointer; press it again to go back to zooming on clicks.",
                    ScreenZoomPointer::ZOOM_ALWAYS => "Always: zoomed in, the view following the pointer.",
                    _ => "Off: the screen as it is. On clicks zooms in where you click; Always stays in, following the pointer; Toggle follows while a key has it on.",
                },
            ))
            .child(self.segmented(
                "screen-zoom",
                vec![
                    ("Off", set(ScreenZoomPointer { zoom: ScreenZoomPointer::ZOOM_OFF, ..look }), look.zoom == ScreenZoomPointer::ZOOM_OFF),
                    ("On clicks", set(ScreenZoomPointer { zoom: ScreenZoomPointer::ZOOM_CLICKS, ..look }), look.zoom == ScreenZoomPointer::ZOOM_CLICKS),
                    ("Always", set(ScreenZoomPointer { zoom: ScreenZoomPointer::ZOOM_ALWAYS, ..look }), look.zoom == ScreenZoomPointer::ZOOM_ALWAYS),
                    ("Toggle", set(ScreenZoomPointer { zoom: ScreenZoomPointer::ZOOM_TOGGLE, ..look }), look.zoom == ScreenZoomPointer::ZOOM_TOGGLE),
                ],
                cx,
            ));
        if look.zoom != ScreenZoomPointer::ZOOM_OFF {
            let scales: Vec<(&'static str, Command, bool)> = [("1.5×", 1.5f32), ("2×", 2.0), ("2.5×", 2.5), ("3×", 3.0)]
                .into_iter()
                .map(|(n, v)| (n, set(ScreenZoomPointer { zoom_scale: v, ..look }), (look.zoom_scale - v).abs() < 0.01))
                .collect();
            body = body.child(row("How far", self.segmented("screen-zoom-scale", scales, cx)));
            // toggle: on clicks while the key hasn't turned following on
            if look.zoom == ScreenZoomPointer::ZOOM_CLICKS || look.zoom == ScreenZoomPointer::ZOOM_TOGGLE {
                let s = self.slider(format!("screen-hold:{sid}"), (0.5, 8.0, 0.25), look.zoom_hold_s, move |v| set(ScreenZoomPointer { zoom_hold_s: v, ..look }), cx);
                body = body.child(slider_row("Stays in", s, format!("{:.1} s", look.zoom_hold_s)));
                let typing = super::settings::switch("screen-zoom-typing", look.zoom_typing)
                    .on_click(cx.listener(move |this, _, _, cx| this.exec(set(ScreenZoomPointer { zoom_typing: !look.zoom_typing, ..look }), cx)));
                body = body.child(tip_row("Typing", "Typing zooms in too, at the text field (where you clicked last), and stays in while the keys keep coming.", div().flex().child(typing)));
            }
            body = body.child(row(
                "Motion",
                self.segmented(
                    "screen-zoom-motion",
                    vec![
                        ("Focused", set(ScreenZoomPointer { zoom_motion: 0.15, ..look }), look.zoom_motion < 0.34),
                        ("Balanced", set(ScreenZoomPointer { zoom_motion: 0.5, ..look }), (0.34..0.67).contains(&look.zoom_motion)),
                        ("Smooth", set(ScreenZoomPointer { zoom_motion: 0.85, ..look }), look.zoom_motion >= 0.67),
                    ],
                    cx,
                ),
            ));
            let badge = super::settings::switch("screen-zoom-badge", look.zoom_badge)
                .on_click(cx.listener(move |this, _, _, cx| this.exec(set(ScreenZoomPointer { zoom_badge: !look.zoom_badge, ..look }), cx)));
            body = body.child(tip_row("Badge", "While viewers see the zoom, a badge by your pointer says so. Only you see it: it's never in the picture.", div().flex().child(badge)));

        }
        // ---- toggle: its key, and whether it's in now
        if look.zoom == ScreenZoomPointer::ZOOM_TOGGLE {
            let keys = self.prefs.global_shortcuts.iter().find(|h| h.action == crate::shortcuts::GlobalAction::FollowZoom).map(|h| crate::shortcuts::display(&h.keys));
            let following = lightair_engine::pointer::following();
            let control = div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .bg(tok::bg_raised())
                        .text_size(px(11.))
                        .font_family(crate::ui::MONO)
                        .text_color(if keys.is_some() { tok::text() } else { tok::text_tertiary() })
                        .child(keys.clone().unwrap_or_else(|| "No key".into())),
                )
                .child(
                    Button::new("screen-follow-key")
                        .label(if keys.is_some() { "Change…" } else { "Set a key…" })
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.open_settings(super::settings::Page::Shortcuts, cx))),
                )
                .child(div().flex_1())
                .when(following, |d| {
                    d.child(
                        Button::new("screen-follow-stop")
                            .label("Zoomed in · Out")
                            .xsmall()
                            .outline()
                            .on_click(cx.listener(|_, _, _, cx| {
                                lightair_engine::pointer::toggle_follow();
                                cx.notify();
                            })),
                    )
                });
            body = body.child(row("Key", control));
        }
        // ---- pointer
        body = body
            .child(heading("POINTER", "As captured: the capture's own pointer. Drawn: Lightair draws it, bigger, smoother and styled. Hidden: no pointer on the screen, and no clicks drawn."))
            .child(self.segmented(
                "screen-cursor",
                vec![
                    ("As captured", set(ScreenZoomPointer { pointer_drawn: false, pointer_hidden: false, ..look }), !look.pointer_drawn && !look.pointer_hidden),
                    ("Drawn", set(ScreenZoomPointer { pointer_drawn: true, pointer_hidden: false, ..look }), look.pointer_drawn && !look.pointer_hidden),
                    ("Hidden", set(ScreenZoomPointer { pointer_hidden: true, ..look }), look.pointer_hidden),
                ],
                cx,
            ));
        if look.pointer_drawn && !look.pointer_hidden {
            let style = self.segmented(
                "screen-cursor-style",
                vec![
                    ("Pointer", set(ScreenZoomPointer { pointer_style: 0, ..look }), look.pointer_style == 0),
                    ("Dot", set(ScreenZoomPointer { pointer_style: 1, ..look }), look.pointer_style == 1),
                    ("Ring", set(ScreenZoomPointer { pointer_style: 2, ..look }), look.pointer_style == 2),
                ],
                cx,
            );
            let size = self.slider(format!("screen-size:{sid}"), (0.5, 4.0, 0.1), look.pointer_size, move |v| set(ScreenZoomPointer { pointer_size: v, ..look }), cx);
            let smooth = self.slider(format!("screen-smooth:{sid}"), (0.0, 1.0, 0.05), look.pointer_smoothing, move |v| set(ScreenZoomPointer { pointer_smoothing: v, ..look }), cx);
            let idle = self.slider(format!("screen-idle:{sid}"), (0.0, 10.0, 0.5), look.pointer_idle_s, move |v| set(ScreenZoomPointer { pointer_idle_s: v, ..look }), cx);
            let tilt = super::settings::switch("screen-tilt", look.pointer_tilt).on_click(cx.listener(move |this, _, _, cx| this.exec(set(ScreenZoomPointer { pointer_tilt: !look.pointer_tilt, ..look }), cx)));
            let current = super::object_props::rgb_of(look.pointer_color);
            let mut swatches = div().flex().flex_wrap().gap_1p5();
            for (i, &rgb) in super::object_props::PALETTE.iter().enumerate() {
                let cmd = set(ScreenZoomPointer { pointer_color: super::object_props::abgr(rgb), ..look });
                swatches = swatches.child(
                    div()
                        .id(("screen-colour", i))
                        .size(px(20.))
                        .rounded(px(5.))
                        .border_2()
                        .border_color(if current == rgb { tok::accent_primary() } else { tok::border() })
                        .bg(rgb_colour(rgb))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx))),
                );
            }
            body = body
                .child(row("Look", style))
                .child(slider_row("Size", size, format!("{:.1}×", look.pointer_size)))
                .child(slider_row("Smoothing", smooth, format!("{:.0}%", look.pointer_smoothing * 100.0)))
                .child(slider_row("Hide when still", idle, if look.pointer_idle_s <= 0.0 { "never".into() } else { format!("{:.1} s", look.pointer_idle_s) }))
                .child(row("Tilt moving", div().flex().child(tilt)))
                .child(row("Colour", swatches))
                // ---- clicks
                .child(heading("CLICKS", "How a click shows: a circle under the pointer, or a ripple."))
                .child(self.segmented(
                    "screen-clicks",
                    vec![
                        ("None", set(ScreenZoomPointer { clicks: 0, ..look }), look.clicks == 0),
                        ("Circle", set(ScreenZoomPointer { clicks: 1, ..look }), look.clicks == 1),
                        ("Ripple", set(ScreenZoomPointer { clicks: 2, ..look }), look.clicks == 2),
                    ],
                    cx,
                ));
        }
        body
    }
}

fn rgb_colour(rgb: u32) -> Rgba {
    gpui_kit::rgb(rgb)
}
