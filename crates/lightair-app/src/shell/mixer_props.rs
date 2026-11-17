//! Mixer strips in the Inspector (docs/plans/audio-engine.md, "Mixer UI"): a track's routing,
//! pan, trim and sends; buses, mixes and the master with their effects.

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::{AudioMix, Bus, Command, Id, MixerStrip, Workspace, Route, Send, Track};

use super::StudioWindow;
use crate::ui::{self, tok};

/// A row of choices: (label, chosen, what choosing runs).
fn choices(id: &'static str, options: Vec<(String, bool, Command)>, cx: &mut Context<StudioWindow>) -> Div {
    let mut row = div().flex().flex_wrap().gap_1();
    for (n, (label, on, cmd)) in options.into_iter().enumerate() {
        row = row.child(
            div()
                .id((id, n))
                .px_2()
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(11.))
                .cursor_pointer()
                .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                .when(!on, |d| d.text_color(tok::text_secondary()).hover(|d| d.bg(tok::bg_raised())))
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

fn labelled(label: &'static str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().w(px(72.)).flex_none().text_size(px(12.)).text_color(tok::text_secondary()).child(label))
        .child(div().flex_1().min_w_0().child(control))
}

fn header(icon: &'static str, name: String, kind: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .py_2()
        .child(ui::icon(icon).size(px(16.)).text_color(tok::text_secondary()))
        .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(name))
        .child(div().text_size(px(10.)).text_color(tok::text_tertiary()).child(kind))
}

impl StudioWindow {
    /// Where a strip's output goes: Program, a bus (not itself or one that feeds it), or nowhere.
    fn route_choices(
        &self,
        workspace: &Workspace,
        own: Option<Id>,
        current: Route,
        set: impl Fn(Route) -> Command,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut options = vec![("Master".to_string(), current == Route::Master, set(Route::Master))];
        for b in &workspace.audio.buses {
            if Some(b.id) == own {
                continue;
            }
            options.push((b.name.clone(), current == Route::Bus(b.id), set(Route::Bus(b.id))));
        }
        options.push(("Sends only".to_string(), current == Route::None, set(Route::None)));
        choices("route", options, cx)
    }

    /// Sends to each mix: on/off, level, before or after the fader.
    fn sends_rows(
        &self,
        workspace: &Workspace,
        key: Id,
        sends: &[Send],
        set: impl Fn(Vec<Send>) -> Command + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut col = div().flex().flex_col().gap_1p5();
        if workspace.audio.mixes.is_empty() {
            return col.child(
                div()
                    .text_size(px(11.))
                    .text_color(tok::text_tertiary())
                    .child("Add a mix in the mixer (+) to send to headphones, a VOD track or a guest."),
            );
        }
        for (n, mix) in workspace.audio.mixes.iter().enumerate() {
            let current = sends.iter().find(|s| s.mix == mix.id).copied();
            let others: Vec<Send> = sends.iter().filter(|s| s.mix != mix.id).copied().collect();
            let toggle = {
                let mut next = others.clone();
                if current.is_none() {
                    next.push(Send { mix: mix.id, level_db: 0.0, pre_fader: false });
                }
                set(next)
            };
            let mut row = div().flex().items_center().gap_2().child(
                super::settings::switch(("send-on", n), current.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| this.exec(toggle.clone(), cx))),
            );
            row = row.child(div().w(px(64.)).flex_none().truncate().text_size(px(12.)).child(mix.name.clone()));
            if let Some(s) = current {
                let (others2, set2) = (others.clone(), set.clone());
                let slider = self.slider(
                    format!("send:{key}:{}", mix.id),
                    (-60.0, 6.0, 0.5),
                    s.level_db,
                    move |v| {
                        let mut next = others2.clone();
                        next.push(Send { level_db: v, ..s });
                        set2(next)
                    },
                    cx,
                );
                let flip = {
                    let mut next = others.clone();
                    next.push(Send { pre_fader: !s.pre_fader, ..s });
                    set(next)
                };
                row = row
                    .child(div().flex_1().min_w_0().child(slider))
                    .child(
                        div()
                            .w(px(46.))
                            .flex_none()
                            .text_right()
                            .text_size(px(11.))
                            .font_family(crate::ui::MONO)
                            .text_color(tok::text_secondary())
                            .child(format!("{:+.1}", s.level_db)),
                    )
                    .child(
                        div()
                            .id(("send-pre", n))
                            .px_1p5()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .text_size(px(10.))
                            .cursor_pointer()
                            .bg(if s.pre_fader { tok::bg_selected() } else { tok::bg_raised() })
                            .text_color(if s.pre_fader { tok::text() } else { tok::text_tertiary() })
                            .child("PRE")
                            .on_click(cx.listener(move |this, _, _, cx| this.exec(flip.clone(), cx))),
                    );
            } else {
                row = row.child(div().flex_1());
            }
            col = col.child(row);
        }
        col
    }

    /// A track's place in the mix: output, pan, trim, polarity, mono, sends.
    pub(super) fn track_routing(&self, workspace: &Workspace, track: &Track, cx: &mut Context<Self>) -> AnyElement {
        let t = track.clone();
        let upd = move |f: &dyn Fn(&mut Track)| {
            let mut n = t.clone();
            f(&mut n);
            Command::UpdateTrack { track: n }
        };
        let route = {
            let t = track.clone();
            self.route_choices(workspace, None, track.route, move |r| Command::UpdateTrack { track: Track { route: r, ..t.clone() } }, cx)
        };
        let pan = {
            let t = track.clone();
            self.slider(format!("pan:{}", track.id), (-1.0, 1.0, 0.01), track.pan, move |v| {
                Command::UpdateTrack { track: Track { pan: v, ..t.clone() } }
            }, cx)
        };
        let trim = {
            let t = track.clone();
            self.slider(format!("trim:{}", track.id), (-24.0, 24.0, 0.5), track.trim_db, move |v| {
                Command::UpdateTrack { track: Track { trim_db: v, ..t.clone() } }
            }, cx)
        };
        let (invert, mono) = (track.invert, track.mono);
        let flip_invert = upd(&|n: &mut Track| n.invert = !invert);
        let flip_mono = upd(&|n: &mut Track| n.mono = !mono);
        let sends = {
            let t = track.clone();
            self.sends_rows(workspace, track.id, &track.sends, move |s| Command::UpdateTrack { track: Track { sends: s, ..t.clone() } }, cx)
        };
        let pan_label = if track.pan.abs() < 0.005 {
            "C".to_string()
        } else if track.pan < 0.0 {
            format!("L{:.0}", -track.pan * 100.0)
        } else {
            format!("R{:.0}", track.pan * 100.0)
        };
        self.section("Mix", None, cx)
            .child(labelled("Output", route))
            .child(labelled("Pan", div().flex().items_center().gap_2().child(div().flex_1().child(pan)).child(
                div().w(px(36.)).text_right().text_size(px(11.)).font_family(crate::ui::MONO).child(pan_label),
            )))
            .child(labelled("Trim", div().flex().items_center().gap_2().child(div().flex_1().child(trim)).child(
                div().w(px(36.)).text_right().text_size(px(11.)).font_family(crate::ui::MONO).child(format!("{:+.1}", track.trim_db)),
            )))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(
                        div().flex().items_center().gap_2().text_size(px(12.)).child("Polarity").child(
                            super::settings::switch("track-invert", invert)
                                .on_click(cx.listener(move |this, _, _, cx| this.exec(flip_invert.clone(), cx))),
                        ),
                    )
                    .child(
                        div().flex().items_center().gap_2().text_size(px(12.)).child("Mono").child(
                            super::settings::switch("track-mono", mono)
                                .on_click(cx.listener(move |this, _, _, cx| this.exec(flip_mono.clone(), cx))),
                        ),
                    ),
            )
            .child(div().pt_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text_tertiary()).child("SENDS"))
            .child(sends)
            .into_any_element()
    }

    /// A bus, a mix or the master.
    pub(super) fn strip_inspector(&self, workspace: &Workspace, strip: MixerStrip, body: Div, cx: &mut Context<Self>) -> AnyElement {
        match strip {
            MixerStrip::Bus(id) => {
                let Some(b) = workspace.audio.bus(id).cloned() else { return body.into_any_element() };
                let route = {
                    let b2 = b.clone();
                    self.route_choices(workspace, Some(id), b.route, move |r| Command::UpdateBus { bus: Bus { route: r, ..b2.clone() } }, cx)
                };
                let pan = {
                    let b2 = b.clone();
                    self.slider(format!("pan:{id}"), (-1.0, 1.0, 0.01), b.pan, move |v| Command::UpdateBus { bus: Bus { pan: v, ..b2.clone() } }, cx)
                };
                let sends = {
                    let b2 = b.clone();
                    self.sends_rows(workspace, id, &b.sends, move |s| Command::UpdateBus { bus: Bus { sends: s, ..b2.clone() } }, cx)
                };
                let rec = b.record;
                let flip_rec = Command::UpdateBus { bus: Bus { record: !rec, ..b.clone() } };
                body.child(header("group", b.name.clone(), "BUS"))
                    .child(
                        self.section("Mix", None, cx)
                            .child(labelled("Output", route))
                            .child(labelled("Pan", pan))
                            .child(labelled(
                                "Record",
                                super::settings::switch("bus-record", rec)
                                    .on_click(cx.listener(move |this, _, _, cx| this.exec(flip_rec.clone(), cx))),
                            ))
                            .child(div().pt_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text_tertiary()).child("SENDS"))
                            .child(sends),
                    )
                    .child(self.strip_effects_section(strip, &b.effects, cx))
                    .child(div().pt_2().child(
                        Button::new("remove-bus").label("Remove bus").small().danger().on_click(
                            cx.listener(move |this, _, _, cx| this.exec(Command::RemoveBus { bus: id }, cx)),
                        ),
                    ))
                    .into_any_element()
            }
            MixerStrip::Mix(id) => {
                let Some(m) = workspace.audio.mix(id).cloned() else { return body.into_any_element() };
                let fed = workspace.tracks.iter().filter(|t| t.sends.iter().any(|s| s.mix == id)).count()
                    + workspace.audio.buses.iter().filter(|b| b.sends.iter().any(|s| s.mix == id)).count();
                body.child(
                    header("split", m.name.clone(), "MIX").child(ui::info(
                        "mix-info",
                        format!(
                            "{fed} strip{} send{} to it. Use it for headphones, a Twitch VOD track, a guest's mix-minus or a music-free stream.",
                            if fed == 1 { "" } else { "s" },
                            if fed == 1 { "s" } else { "" }
                        ),
                    )),
                )
                    .child({
                        let mm = m.clone();
                        let pan = self.slider(format!("mixpan:{id}"), (-1.0, 1.0, 0.01), m.pan, move |v| {
                            Command::UpdateMix { mix: AudioMix { pan: v, ..mm.clone() } }
                        }, cx);
                        labelled("Balance", pan)
                    })
                    .child({
                        let on = m.to_program;
                        let cmd = Command::UpdateMix { mix: AudioMix { to_program: !on, ..m.clone() } };
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .py_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().text_size(px(12.)).child("Into Program too"))
                                    .child(ui::info("mix-to-program-info", "An effects return (a shared reverb), heard on the stream")),
                            )
                            .child(
                                super::settings::switch("mix-to-program", on)
                                    .on_click(cx.listener(move |this, _, _, cx| this.exec(cmd.clone(), cx))),
                            )
                    })
                    .child(self.strip_effects_section(strip, &m.effects, cx))
                    .child(div().pt_2().child(
                        Button::new("remove-mix").label("Remove mix").small().danger().on_click(
                            cx.listener(move |this, _, _, cx| this.exec(Command::RemoveMix { mix: id }, cx)),
                        ),
                    ))
                    .into_any_element()
            }
            MixerStrip::Master => body
                .child(header("radio", "Master".into(), "MASTER").child(ui::info(
                    "master-info",
                    "What the stream and the recording's main track carry. A limiter here keeps it from clipping.",
                )))
                .child(self.strip_effects_section(strip, &workspace.audio.master.effects, cx))
                .into_any_element(),
            MixerStrip::Track(_) => body.into_any_element(),
        }
    }
}

/// New strips from the mixer's add menu.
pub fn new_bus(workspace: &Workspace) -> Bus {
    Bus::new(format!("Bus {}", workspace.audio.buses.len() + 1))
}

pub fn new_mix(workspace: &Workspace) -> AudioMix {
    AudioMix::new(format!("Mix {}", workspace.audio.mixes.len() + 1))
}
