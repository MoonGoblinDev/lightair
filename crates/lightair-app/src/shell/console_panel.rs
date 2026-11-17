//! The footer's console (crate::console): how many things need attention, at the footer's
//! left; opened, each one with its fix.

use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::StudioWindow;
use crate::console::{self, Fix};
use crate::ui::{self, tok};

impl StudioWindow {
    /// Looks at what's connected every few seconds (off the UI thread), from launch on.
    pub(super) fn watch_devices(&mut self, cx: &mut Context<Self>) {
        let engine = self.studio.read(cx).engine().clone();
        cx.spawn(async move |this, cx| {
            loop {
                let e = engine.clone();
                let now = cx.background_executor().spawn(async move { console::Connected::now(&e) }).await;
                tracing::debug!(mics = now.mics.len(), cameras = now.cameras.len(), "connected devices");
                let alive = this
                    .update(cx, |this, cx| {
                        if this.connected.as_ref() != Some(&now) {
                            this.connected = Some(now);
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
            }
        })
        .detach();
    }

    /// The footer's left: what needs attention (a click opens the console).
    pub(super) fn console_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let n = console::look(self.studio.read(cx).workspace(), self.connected.as_ref()).len();
        if n == 0 && !self.console_open {
            return None;
        }
        let open = self.console_open;
        Some(
            div()
                .relative()
                .child(
                    div()
                        .id("console-chip")
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(20.))
                        .px_1p5()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(if n > 0 { tok::warn() } else { tok::text_secondary() })
                        .when(open, |d| d.bg(tok::bg_raised()))
                        .hover(|d| d.bg(tok::bg_raised()))
                        .child(ui::icon("triangle-alert").size(px(13.)))
                        .child(if n == 0 { "Console".to_string() } else if n == 1 { "1 needs attention".to_string() } else { format!("{n} need attention") })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.console_open = !this.console_open;
                            cx.notify();
                        })),
                )
                .children(open.then(|| self.console_overlay(cx)))
                .into_any_element(),
        )
    }

    fn console_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let workspace = self.studio.read(cx).workspace().clone();
        let items = console::look(&workspace, self.connected.as_ref());
        let connected = self.connected.clone().unwrap_or_default();
        let mut list = div().flex().flex_col().gap_2();
        if items.is_empty() {
            list = list.child(div().py_3().text_color(tok::text_tertiary()).child("Nothing needs attention."));
        }
        for item in items {
            let fix = match item.fix {
                Fix::Device { source, camera } => {
                    let me = cx.entity().downgrade();
                    let current = workspace.sources.get(&source).map(|s| (*s.clone()).clone());
                    let mut choices: Vec<(String, String)> = if camera { connected.cameras.clone() } else { connected.mics.clone() };
                    if !camera {
                        choices.insert(0, (String::new(), crate::mixer::SYSTEM_DEFAULT.to_string()));
                    }
                    Some(
                        Button::new(SharedString::from(format!("console-fix-{}", item.key)))
                            .label("Replace with…")
                            .xsmall()
                            .outline()
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut menu, _, _| {
                                if choices.is_empty() {
                                    return menu.item(PopupMenuItem::new(if camera { "No camera connected" } else { "No microphone connected" }).disabled(true));
                                }
                                for (id, name) in &choices {
                                    let (me, current, id, name) = (me.clone(), current.clone(), id.clone(), name.clone());
                                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                                        let (Some(me), Some(src)) = (me.upgrade(), current.clone()) else { return };
                                        let mut settings = src.settings.clone();
                                        settings["device"] = serde_json::json!(id);
                                        let name = if camera { name.clone() } else { console::mic_name(&id, &name) };
                                        me.update(cx, |this, cx| {
                                            let w = this.studio.read(cx).workspace().clone();
                                            if let Some(cmd) = console::repoint(&w, src.id, settings, &name) {
                                                this.exec(cmd, cx);
                                            }
                                        });
                                    }));
                                }
                                menu
                            })
                            .into_any_element(),
                    )
                }
                Fix::None => None,
            };
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .p_2()
                    .rounded(px(6.))
                    .bg(tok::bg_app())
                    .child(ui::icon("triangle-alert").size(px(14.)).text_color(tok::warn()).mt(px(1.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(tok::text()).child(item.title))
                            .child(div().text_size(px(11.)).text_color(tok::text_secondary()).child(item.detail)),
                    )
                    .children(fix),
            );
        }
        deferred(
            div()
                .id("console-panel")
                .absolute()
                .bottom(px(26.))
                .left_0()
                .w(px(520.))
                .max_h(px(420.))
                .overflow_y_scroll()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(8.))
                .bg(tok::bg_header())
                .border_1()
                .border_color(tok::border_header())
                .shadow_lg()
                .text_size(px(12.))
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.console_open = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).text_color(tok::text()).child("Console"))
                        .child(ui::icon_button("console-close", "x", tok::text_tertiary()).on_click(cx.listener(|this, _, _, cx| {
                            this.console_open = false;
                            cx.notify();
                        }))),
                )
                .child(list),
        )
        .with_priority(1)
        .into_any_element()
    }
}
