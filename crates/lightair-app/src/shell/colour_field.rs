//! One colour control for every colour setting: the swatch opens a full picker (hue and
//! saturation, alpha, hex, Lightair's swatches as presets), the dropper picks a colour from
//! anywhere on screen, and the hex says what it is. Each picker is kept by a key (it needs the
//! window to be made: made or brought up to date in the Inspector's sync, drawn in its render).

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::StudioWindow;
use crate::ui::{self, tok};

/// What a change does (with the colour chosen).
pub(super) type Apply = Rc<dyn Fn(&mut StudioWindow, Hsla, &mut Context<StudioWindow>)>;

pub(super) struct Field {
    state: Entity<ColorPickerState>,
    shown: Hsla,
    apply: Apply,
    _events: Subscription,
}

#[derive(Default)]
pub(super) struct ColourFields {
    fields: HashMap<String, Field>,
    /// Keys used since the last `keep_only`.
    used: Vec<String>,
}

/// Presets: neutrals, then bright and soft ones.
const PRESETS: &[u32] = &[
    0xFFFFFF, 0xD9D9D9, 0x8C8C8C, 0x3A3A3A, 0x0B0B0F, 0xFF4A3B, 0xFF8A3D, 0xFFB020, 0xFFE14D, 0x2BD67B, 0x14B8A6, 0x3B82F6, 0x6366F1, 0x8B5CF6,
    0xD946EF, 0xF472B6,
];

/// "#RRGGBB", "#RRGGBBAA", "#RGB", "rgb(…)", "rgba(…)" → the colour.
pub(super) fn parse(text: &str) -> Option<Hsla> {
    let t = text.trim();
    if let Some(h) = t.strip_prefix('#') {
        let full = match h.len() {
            3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 | 8 => h.to_string(),
            _ => return None,
        };
        let v = u32::from_str_radix(&full, 16).ok()?;
        let (rgb, a) = if full.len() == 8 { (v >> 8, (v & 0xFF) as f32 / 255.0) } else { (v, 1.0) };
        return Some(with_alpha(rgb, a));
    }
    let inner = t.strip_prefix("rgba(").or_else(|| t.strip_prefix("rgb("))?.strip_suffix(')')?;
    let n: Vec<f32> = inner.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    let (r, g, b) = (*n.first()? as u32, *n.get(1)? as u32, *n.get(2)? as u32);
    Some(with_alpha((r.min(255) << 16) | (g.min(255) << 8) | b.min(255), n.get(3).copied().unwrap_or(1.0)))
}

fn with_alpha(rgb: u32, a: f32) -> Hsla {
    let mut c: Hsla = rgb_u32(rgb).into();
    c.a = a.clamp(0.0, 1.0);
    c
}

fn rgb_u32(rgb: u32) -> Rgba {
    gpui_kit::rgb(rgb)
}

/// (0xRRGGBB, alpha 0–1).
pub(super) fn rgb_alpha(c: Hsla) -> (u32, f32) {
    let r = Rgba::from(c);
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    ((ch(r.r) << 16) | (ch(r.g) << 8) | ch(r.b), r.a)
}

/// As CSS writes it: "#RRGGBB", or "rgba(…)" when it's see-through.
pub(super) fn css(c: Hsla) -> String {
    let (rgb, a) = rgb_alpha(c);
    if a >= 0.999 {
        format!("#{rgb:06X}")
    } else {
        format!("rgba({}, {}, {}, {:.2})", rgb >> 16, (rgb >> 8) & 0xFF, rgb & 0xFF, a)
    }
}

/// libobs's 0xAABBGGRR.
pub(super) fn abgr(c: Hsla) -> u32 {
    let (rgb, a) = rgb_alpha(c);
    (((a.clamp(0.0, 1.0) * 255.0).round() as u32) << 24) | ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | (rgb >> 16)
}

pub(super) fn of_abgr(v: u32) -> Hsla {
    let rgb = ((v & 0xFF) << 16) | (v & 0xFF00) | ((v >> 16) & 0xFF);
    // (0 alpha is "unset" in many sources: opaque)
    let a = if v >> 24 == 0 { 1.0 } else { (v >> 24) as f32 / 255.0 };
    with_alpha(rgb, a)
}

impl StudioWindow {
    /// Makes (or brings up to date) the picker for `key`, showing `current`; `apply` runs when
    /// it changes. Called while syncing, with the window.
    pub(super) fn sync_colour(&mut self, key: &str, current: Hsla, apply: Apply, window: &mut Window, cx: &mut Context<Self>) {
        self.colours.used.push(key.to_string());
        match self.colours.fields.get_mut(key) {
            Some(f) => {
                f.apply = apply;
                if f.shown != current {
                    f.shown = current;
                    f.state.update(cx, |s, cx| s.set_value(current, window, cx));
                }
            }
            None => {
                let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(current));
                let k = key.to_string();
                let events = cx.subscribe_in(&state, window, move |this, _, ev: &ColorPickerEvent, _, cx| {
                    if let ColorPickerEvent::Change(Some(c)) = ev
                        && let Some(f) = this.colours.fields.get_mut(&k)
                    {
                        f.shown = *c;
                        let apply = f.apply.clone();
                        apply(this, *c, cx);
                    }
                });
                self.colours.fields.insert(key.to_string(), Field { state, shown: current, apply, _events: events });
            }
        }
    }

    /// Forgets the pickers not synced since the last call (the selection moved on).
    pub(super) fn keep_synced_colours(&mut self) {
        let used = std::mem::take(&mut self.colours.used);
        self.colours.fields.retain(|k, _| used.contains(k));
    }

    /// The control: picker, hex, dropper.
    pub(super) fn colour_field(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = self.colours.fields.get(key) else {
            return div().into_any_element();
        };
        let hex = css(f.shown);
        let picker = ColorPicker::new(&f.state)
            .featured_colors(PRESETS.iter().map(|rgb| super::object_props::rgb_to_hsla(*rgb)).collect())
            .anchor(Anchor::TopRight)
            .small();
        let k = key.to_string();
        div()
            .flex()
            .flex_1()
            .items_center()
            .gap_2()
            .child(picker)
            .child(div().flex_1().text_size(px(11.)).font_family(crate::ui::MONO).text_color(tok::text_secondary()).child(hex))
            .when(crate::eyedropper::AVAILABLE, |row| {
                row.child(
                    ui::icon_button(SharedString::from(format!("drop-{key}")), "pipette", tok::text_secondary())
                        .tooltip(ui::tip("Pick a colour from the screen"))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let picked = crate::eyedropper::pick();
                            let k = k.clone();
                            cx.spawn(async move |this, cx| {
                                let rgb = loop {
                                    match picked.try_recv() {
                                        Ok(r) => break r,
                                        Err(std::sync::mpsc::TryRecvError::Empty) => cx.background_executor().timer(std::time::Duration::from_millis(30)).await,
                                        Err(_) => break None,
                                    }
                                };
                                if let Some(rgb) = rgb {
                                    let _ = this.update(cx, |this, cx| {
                                        if let Some(apply) = this.colours.fields.get(&k).map(|f| f.apply.clone()) {
                                            apply(this, with_alpha(rgb, 1.0), cx);
                                        }
                                    });
                                }
                            })
                            .detach();
                        })),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{abgr, css, of_abgr, parse};

    #[test]
    fn css_colours_round_trip() {
        for t in ["#FF4A3B", "rgba(0, 0, 0, 0.45)"] {
            assert_eq!(css(parse(t).unwrap()), t);
        }
        assert_eq!(css(parse("#fff").unwrap()), "#FFFFFF");
        assert_eq!(abgr(parse("#112233").unwrap()), 0xFF33_2211);
        assert_eq!(css(of_abgr(0xFF33_2211)), "#112233");
    }
}
