//! The Go Live review sheet (docs/11-outputs-streaming-recording.md, "Go Live review"):
//! destinations to include, pre-flight checks in plain language, and "also record".

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_model::{Command, Id};

use super::StudioWindow;
use crate::preflight::{self, Level};
use crate::ui::{self, tok};

pub(super) struct GoLiveSheet {
    /// Whether each destination has a stream key (Keychain, looked up when the sheet opens).
    has_key: HashMap<Id, bool>,
    record_too: bool,
}

impl StudioWindow {
    pub(super) fn open_go_live(&mut self, cx: &mut Context<Self>) {
        // Preference: skip the review and go straight live.
        if !self.prefs.review_before_live {
            self.studio.update(cx, |s, cx| s.go_live(cx));
            return;
        }
        let workspace = self.studio.read(cx).workspace().clone();
        let has_key =
            workspace.outputs.destinations.iter().map(|d| (d.id, crate::secrets::stream_key(d.id).is_some())).collect();
        self.go_live_sheet = Some(GoLiveSheet { has_key, record_too: false });
        cx.notify();
    }

    fn close_go_live(&mut self, cx: &mut Context<Self>) {
        self.go_live_sheet = None;
        cx.notify();
    }

    fn confirm_go_live(&mut self, cx: &mut Context<Self>) {
        let record_too = self.go_live_sheet.as_ref().is_some_and(|s| s.record_too);
        self.go_live_sheet = None;
        self.studio.update(cx, |s, cx| {
            s.go_live(cx);
            if record_too && !s.is_recording() {
                s.toggle_recording(cx);
            }
        });
    }

    pub(super) fn go_live_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sheet = self.go_live_sheet.as_ref()?;
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let facts = studio.preflight_facts(&sheet.has_key, sheet.record_too);
        let recording = studio.is_recording();
        let checks = preflight::run(&facts);
        let ready = preflight::can_go_live(&checks);
        let record_too = sheet.record_too;

        let mut destinations = div().flex().flex_col().gap_1();
        if workspace.outputs.destinations.is_empty() {
            destinations = destinations
                .child(div().text_color(tok::text_tertiary()).child("No destinations yet. Add one in Outputs."));
        }
        for (i, d) in workspace.outputs.destinations.iter().enumerate() {
            let (id, enabled) = (d.id, d.enabled);
            // (no canvas chosen: the main one)
            let canvas = d.canvas.and_then(|f| workspace.canvas(f)).unwrap_or(workspace.primary_canvas()).name.clone();
            destinations = destinations.child(
                div()
                    .id(("golive-dest", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(32.))
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(tok::bg_raised()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.exec(Command::SetDestinationEnabled { destination: id, enabled: !enabled }, cx)
                    }))
                    .child(
                        div()
                            .size(px(14.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(if enabled { tok::accent_primary() } else { tok::border_strong() })
                            .when(enabled, |d| d.bg(tok::accent_primary())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(if enabled { tok::text() } else { tok::text_tertiary() })
                            .child(d.name.clone()),
                    )
                    .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(canvas)),
            );
        }

        let mut list = div().flex().flex_col().gap_1p5();
        for c in &checks {
            let (icon, colour) = match c.level {
                Level::Ok => ("circle-check", tok::ok()),
                Level::Warn => ("triangle-alert", tok::warn()),
                Level::Block => ("circle-x", tok::error()),
            };
            list =
                list.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(div().flex_none().child(ui::icon(icon).size(px(15.)).text_color(colour).mt(px(2.))))
                        .child(
                            div().flex_1().min_w_0().flex().flex_col().child(div().child(c.title.clone())).child(
                                div().text_size(px(11.)).text_color(tok::text_tertiary()).child(c.detail.clone()),
                            ),
                        ),
                );
        }

        let section = |title: &'static str| {
            div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text_secondary()).child(title)
        };
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
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_go_live(cx)))
                .child(
                    div()
                        .id("golive")
                        .flex()
                        .flex_col()
                        .gap_4()
                        .w(px(460.))
                        .max_h(px(640.))
                        .overflow_y_scroll()
                        .p_5()
                        .rounded(px(14.))
                        .bg(tok::bg_panel())
                        .border_1()
                        .border_color(tok::border_strong())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Go live"))
                        .child(div().flex().flex_col().gap_2().child(section("DESTINATIONS")).child(destinations))
                        .child(div().flex().flex_col().gap_2().child(section("CHECKS")).child(list))
                        .when(!recording, |d| {
                            d.child(
                                div()
                                    .id("golive-record")
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(s) = &mut this.go_live_sheet {
                                            s.record_too = !s.record_too;
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .size(px(14.))
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(if record_too {
                                                tok::accent_primary()
                                            } else {
                                                tok::border_strong()
                                            })
                                            .when(record_too, |d| d.bg(tok::accent_primary())),
                                    )
                                    .child("Also start recording"),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("golive-cancel")
                                        .label("Cancel")
                                        .small()
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| this.close_go_live(cx))),
                                )
                                .child(
                                    Button::new("golive-go")
                                        .label("Go live")
                                        .small()
                                        .danger()
                                        .disabled(!ready)
                                        .on_click(cx.listener(|this, _, _, cx| this.confirm_go_live(cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
