//! The Inspector's Avatar section (docs/plans/avatars.md): the camera that follows the face,
//! Calibrate, framing, size, mirror, zoom and an expression. The model itself is the Source
//! section's file.

use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_model::{Command, Id, Source};
use serde_json::{Value, json};

use super::StudioWindow;
use crate::ui::{self, tok};

pub(super) const KIND: &str = lightair_engine::avatar::KIND;

/// Expressions every VRM model has: (setting, label).
const EXPRESSIONS: &[(&str, &str)] = &[
    ("", "None"),
    ("happy", "Happy"),
    ("angry", "Angry"),
    ("sad", "Sad"),
    ("relaxed", "Relaxed"),
    ("surprised", "Surprised"),
];

fn with(settings: &Value, key: &str, value: Value) -> Value {
    let mut s = if settings.is_object() { settings.clone() } else { json!({}) };
    s[key] = value;
    s
}

impl StudioWindow {
    /// Asks the engine what the selected avatar is doing (on the Inspector's timer).
    pub(super) fn refresh_avatar(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let Some(source) = studio
            .selected_object
            .and_then(|l| studio.selected_scene.and_then(|s| studio.workspace().scene(s)).and_then(|s| s.object(l)))
            .and_then(|l| l.source)
            .filter(|s| studio.workspace().sources.get(s).is_some_and(|i| i.kind == KIND))
        else {
            return;
        };
        let status = studio.engine().call(move |s| s.reconciler.avatar_status(source));
        if let Some(status) = status
            && self.avatar_status.get(&source) != Some(&status)
        {
            self.avatar_status.insert(source, status);
            cx.notify();
        }
    }

    fn calibrate_avatar(&mut self, source: Id, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let Some(settings) = studio.workspace().sources.get(&source).map(|i| i.settings.clone()) else { return };
        match studio.engine().call(move |s| s.reconciler.avatar_calibrate(source)) {
            Some(cal) => {
                self.avatar_note = Some((source, "Calibrated to your face now.".into()));
                self.exec(
                    Command::SetSourceSettings { source, settings: with(&settings, "calibration", json!(cal)) },
                    cx,
                );
            }
            None => self.avatar_note = Some((source, "No face in view: look at the camera and try again.".into())),
        }
        cx.notify();
    }

    pub(super) fn avatar_section(&self, source: &Source, cx: &mut Context<Self>) -> AnyElement {
        let id = source.id;
        let s = source.settings.clone();
        let set = |key: &str, v: Value| Command::SetSourceSettings { source: id, settings: with(&s, key, v) };
        let label = |text: &'static str| div().text_size(px(11.)).text_color(tok::text_tertiary()).child(text);
        let status = self.avatar_status.get(&id).cloned().unwrap_or_default();
        let device = s["device"].as_str().unwrap_or("").to_string();
        let file = s["file"].as_str().unwrap_or("");

        // what it's doing, in a line
        let (dot, line) = if file.is_empty() {
            (tok::text_tertiary(), "Choose a VRM model".to_string())
        } else if !status.error.is_empty() {
            (tok::error(), format!("The model didn't load: {}", status.error))
        } else if !status.loaded {
            (tok::text_tertiary(), "Loading the model…".to_string())
        } else if device.is_empty() {
            (tok::text_tertiary(), "Not following a face: choose a camera".to_string())
        } else if status.tracking {
            (tok::ok(), "Following your face".to_string())
        } else {
            (tok::warn(), "Looking for a face…".to_string())
        };
        let body_line =
            (!status.body.is_empty() && !device.is_empty() && status.loaded).then(|| format!("Body: {}", status.body));
        let state = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
                    .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(tok::text_secondary()).child(line)),
            )
            .children(
                body_line.map(|l| div().pl(px(15.)).text_size(px(11.)).text_color(tok::text_tertiary()).child(l)),
            );
        let show = s["show_tracking"].as_bool().unwrap_or(false);
        let show_cmd = set("show_tracking", json!(!show));

        // the camera that sees the face
        let cameras: Vec<(String, String)> = self.connected.as_ref().map(|c| c.cameras.clone()).unwrap_or_default();
        let shown = if device.is_empty() {
            "None".to_string()
        } else {
            cameras.iter().find(|(d, _)| *d == device).map_or_else(|| "Not connected".to_string(), |(_, n)| n.clone())
        };
        let me = cx.entity().downgrade();
        let base = s.clone();
        let current = device.clone();
        let camera =
            Button::new("avatar-camera").label(shown).small().outline().dropdown_menu(move |mut menu, _, _| {
                let mut choices = vec![(String::new(), "None (no face following)".to_string())];
                choices.extend(cameras.iter().cloned());
                for (value, name) in choices {
                    let me = me.clone();
                    let cmd = Command::SetSourceSettings { source: id, settings: with(&base, "device", json!(value)) };
                    menu = menu.item(PopupMenuItem::new(name).checked(value == current).on_click(move |_, _, cx| {
                        if let Some(me) = me.upgrade() {
                            me.update(cx, |this, cx| this.exec(cmd.clone(), cx));
                        }
                    }));
                }
                menu
            });

        let body = s["body"].as_str().unwrap_or("upper");
        let bodies = [("Face only", "off"), ("Upper body", "upper"), ("Full body", "full")]
            .into_iter()
            .map(|(l, v)| (l, set("body", json!(v)), body == v))
            .collect();
        let framing = s["framing"].as_str().unwrap_or("bust");
        let framings = [("Head", "head"), ("Bust", "bust"), ("Half", "half"), ("Full", "full")]
            .into_iter()
            .map(|(l, v)| (l, set("framing", json!(v)), framing == v))
            .collect();
        let (w, h) = (s["width"].as_i64().unwrap_or(1080), s["height"].as_i64().unwrap_or(1080));
        let sizes = [("Square", 1080, 1080), ("Portrait", 1080, 1920), ("Landscape", 1920, 1080)]
            .into_iter()
            .map(|(l, sw, sh)| {
                let mut both = with(&s, "width", json!(sw));
                both["height"] = json!(sh);
                (l, Command::SetSourceSettings { source: id, settings: both }, (w, h) == (sw, sh))
            })
            .collect();
        let expression = s["expression"].as_str().unwrap_or("");
        // a flat model's own expressions (Live2D; Inochi2D and Umamo puppets have none); a VRM's standard ones
        let lower = file.to_ascii_lowercase();
        let flat = [".model3.json", ".inp", ".inx", ".uma"].iter().any(|e| lower.ends_with(e));
        let no_expressions = flat && status.expressions.is_empty();
        let choices: Vec<(String, String)> = if flat {
            std::iter::once((String::new(), "None".to_string()))
                .chain(status.expressions.iter().map(|e| (e.clone(), e.clone())))
                .collect()
        } else {
            EXPRESSIONS.iter().map(|(v, n)| (v.to_string(), n.to_string())).collect()
        };
        let mut expressions = div().flex().flex_wrap().gap_1();
        for (i, (value, name)) in choices.into_iter().enumerate() {
            let on = expression == value;
            let cmd = set("expression", json!(value));
            expressions = expressions.child(
                div()
                    .id(("avatar-expression", i))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                    .when(!on, |d| {
                        d.bg(tok::bg_raised()).text_color(tok::text_secondary()).hover(|d| d.text_color(tok::text()))
                    })
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !on {
                            this.exec(cmd.clone(), cx)
                        }
                    })),
            );
        }
        let mirror = s["mirror"].as_bool().unwrap_or(true);
        let mirror_cmd = set("mirror", json!(!mirror));
        let zoom = s["zoom"].as_f64().unwrap_or(1.0) as f32;
        let base = s.clone();
        let zoom_slider = self.slider(
            format!("avatar-zoom-{id}"),
            (0.5, 2.0, 0.05),
            zoom,
            move |v| Command::SetSourceSettings { source: id, settings: with(&base, "zoom", json!(v)) },
            cx,
        );

        self.section("Avatar", None, cx)
            .child(state)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().flex().items_center().gap_1().child(label("Camera")).child(ui::info("avatar-camera-info", "The camera that sees your face. Its picture is never shown, only the avatar.")))
                    .child(camera),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("avatar-calibrate")
                            .icon(ui::icon("crosshair"))
                            .label("Calibrate")
                            .small()
                            .outline()
                            .disabled(device.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| this.calibrate_avatar(id, cx))),
                    )
                    .child(div().flex_1().min_w_0().text_size(px(11.)).line_height(px(15.)).text_color(tok::text_tertiary()).child(
                        self.avatar_note.as_ref().filter(|(s, _)| *s == id).map_or("Look at the camera, face relaxed, then click.".to_string(), |(_, n)| n.clone()),
                    )),
            )
            // (a flat model follows the face only)
            .when(!flat, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(label("Follows"))
                        .child(ui::info("avatar-body-info", if cfg!(target_os = "macos") {
                            "Upper body: your arms, hands and fingers too, when the camera sees them. Full body: your legs as well (stand back so the camera sees all of you)."
                        } else {
                            "Body following comes to Windows later: the face only, for now."
                        })),
                )
                .child(self.segmented("avatar-body", bodies, cx))
            })
            .child(label("Framing"))
            .child(self.segmented("avatar-framing", framings, cx))
            .child(div().flex().items_center().justify_between().child(label("Zoom")).child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(format!("{:.0}%", zoom * 100.0))))
            .child(zoom_slider)
            .child(label("Size"))
            .child(self.segmented("avatar-size", sizes, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_size(px(12.)).child("Mirror"))
                            .child(ui::info("avatar-mirror-info", "On: it moves like your reflection. Off: as the camera sees you.")),
                    )
                    .child(super::settings::switch("avatar-mirror", mirror).on_click(cx.listener(move |this, _, _, cx| this.exec(mirror_cmd.clone(), cx)))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_size(px(12.)).child("Show tracking"))
                            .child(ui::info("avatar-show-tracking-info", "The camera's picture in the avatar's corner, with what's followed marked: face green, body red (orange: flat, no depth), hands yellow. Turn it off before going live.")),
                    )
                    .child(super::settings::switch("avatar-show-tracking", show).on_click(cx.listener(move |this, _, _, cx| this.exec(show_cmd.clone(), cx)))),
            )
            .when(!no_expressions, |d| d.child(label("Expression")).child(expressions))
            .into_any_element()
    }
}
