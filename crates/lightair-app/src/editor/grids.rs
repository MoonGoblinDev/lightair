//! CapCut's grids of looks: the media panel's Effects, Transitions and Filters, and the
//! inspector's Animation (in, out, combo). Each is a thumbnail of what it does to sample
//! pictures (made once, `lightair_render::thumbs`); those that move play while the pointer is
//! over them. A click puts it on the picked clips; one dragged onto a clip puts it on that one.

use super::*;
use lightair_render::motion::Motion;
use lightair_render::sequence::{Clip, ClipEffect, Filter, Transition, TransitionKind};
use lightair_render::thumbs::{self, Slot, FRAMES, STILL};

/// A look a card puts on clips.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Look {
    Filter(usize),
    Effect(&'static str),
    /// A transition from the clip before (`None`: none).
    Transition(Option<TransitionKind>),
}

/// A look being dragged onto a clip.
#[derive(Clone)]
pub struct DraggedLook {
    pub(super) look: Look,
    name: SharedString,
    thumb: Option<PathBuf>,
}

impl Render for DraggedLook {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .rounded(px(8.))
            .bg(tok::bg_raised())
            .border_2()
            .border_color(tok::accent_primary())
            .shadow_lg()
            .text_size(px(11.))
            .text_color(tok::text())
            .child(div().w(px(80.)).h(px(45.)).rounded(px(4.)).overflow_hidden().bg(tok::bg_canvas_well()).children(self.thumb.clone().map(|p| img(p).size_full().object_fit(ObjectFit::Cover))))
            .child(self.name.clone())
    }
}

/// The thumbnails: where, whether they're made, and the card playing under the pointer.
pub(super) struct Thumbs {
    pub(super) dir: PathBuf,
    ready: bool,
    making: bool,
    hover: Option<(SharedString, usize)>,
    _play: Option<Task<()>>,
}

impl Thumbs {
    pub(super) fn new() -> Thumbs {
        // (the version: made anew when what they show changes)
        let dir = lightair_platform::paths::data_dir().join("cache").join("looks").join("v1");
        Thumbs { ready: thumbs::ready(&dir), dir, making: false, hover: None, _play: None }
    }
}

/// Cards, three a row (as wide as the panel lets them be).
fn cards() -> Div {
    div().grid().grid_cols(3).gap_2()
}

impl Editor {
    /// Makes the thumbnails, if they aren't (on a thread).
    pub(super) fn want_thumbs(&mut self) {
        if self.thumbs.ready || self.thumbs.making {
            return;
        }
        self.thumbs.making = true;
        let (dir, tx) = (self.thumbs.dir.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Done::Thumbs(thumbs::make(&dir)));
        });
    }

    pub(super) fn take_thumbs(&mut self, r: Result<(), String>, cx: &mut Context<Self>) {
        self.thumbs.making = false;
        match r {
            Ok(()) => self.thumbs.ready = true,
            Err(e) => {
                tracing::warn!("looks' thumbnails: {e}");
                self.note = Some("The looks' thumbnails couldn't be made.".into());
            }
        }
        cx.notify();
    }

    /// The pointer over (or off) a card that moves: it plays.
    fn hover_card(&mut self, key: SharedString, over: bool, cx: &mut Context<Self>) {
        if !over {
            if self.thumbs.hover.as_ref().is_some_and(|(k, _)| *k == key) {
                self.thumbs.hover = None;
                self.thumbs._play = None;
                cx.notify();
            }
            return;
        }
        self.thumbs.hover = Some((key, 0));
        self.thumbs._play = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(130)).await;
                let going = this.update(cx, |this, cx| match &mut this.thumbs.hover {
                    Some((_, f)) => {
                        *f = (*f + 1) % FRAMES;
                        cx.notify();
                        true
                    }
                    None => false,
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// A card: its thumbnail (`frames`: one, or the frames it plays), its name; `on`: what the
    /// picked clip has.
    #[allow(clippy::too_many_arguments)]
    fn thumb_card(&self, key: impl Into<SharedString>, name: &'static str, (frames, still): (Vec<PathBuf>, usize), on: bool, drag: Option<Look>, click: impl Fn(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> Stateful<Div> {
        let key: SharedString = key.into();
        let moving = frames.len() > 1;
        let frame = match &self.thumbs.hover {
            Some((k, f)) if *k == key && moving => *f,
            _ if moving => still,
            _ => 0,
        };
        let picture = frames.get(frame).cloned().filter(|_| self.thumbs.ready);
        let thumb = frames.get(still.min(frames.len().saturating_sub(1))).cloned().filter(|_| self.thumbs.ready);
        let hover_key = key.clone();
        div()
            .id(key.clone())
            .flex()
            .flex_col()
            .gap_1()
            .w_full().min_w_0()
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .w_full()
                    .aspect_ratio(16.0 / 9.0)
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(tok::bg_canvas_well())
                    .border_2()
                    .border_color(if on { tok::accent_primary() } else { rgba(0x00000000).into() })
                    .hover(|d| d.border_color(tok::accent_primary()))
                    .map(|d| match picture {
                        Some(p) => d.child(img(p).size_full().object_fit(ObjectFit::Cover)),
                        None => d.flex().items_center().justify_center().child(ui::icon("loader").size(px(14.)).text_color(tok::text_tertiary())),
                    })
                    .when(on, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(3.))
                                .top(px(3.))
                                .size(px(14.))
                                .rounded_full()
                                .bg(tok::accent_primary())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(ui::icon("check").size(px(10.)).text_color(rgb(0xFFFFFF))),
                        )
                    }),
            )
            .child(div().text_size(px(11.)).overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(if on { tok::accent_primary() } else { tok::text_secondary() }).child(name))
            .when(moving, |d| d.on_hover(cx.listener(move |this, over: &bool, _, cx| this.hover_card(hover_key.clone(), *over, cx))))
            .on_click(cx.listener(move |this, _, _, cx| click(this, cx)))
            .when_some(drag, |d, look| {
                let dragged = DraggedLook { look, name: name.into(), thumb };
                d.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
            })
    }

    /// A "none" card (takes it off).
    fn none_card(&self, key: &'static str, on: bool, click: impl Fn(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(key)
            .flex()
            .flex_col()
            .gap_1()
            .w_full().min_w_0()
            .cursor_pointer()
            .child(
                div()
                    .w_full()
                    .aspect_ratio(16.0 / 9.0)
                    .rounded(px(6.))
                    .bg(tok::bg_canvas_well())
                    .border_2()
                    .border_color(if on { tok::accent_primary() } else { rgba(0x00000000).into() })
                    .hover(|d| d.border_color(tok::accent_primary()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::icon("ban").size(px(20.)).text_color(tok::text_secondary())),
            )
            .child(div().text_size(px(11.)).text_color(if on { tok::accent_primary() } else { tok::text_secondary() }).child("None"))
            .on_click(cx.listener(move |this, _, _, cx| click(this, cx)))
    }

    /// The picked clips that have pictures.
    fn picked_pictures(&self) -> Vec<Id> {
        let Some(seq) = self.seq() else { return Vec::new() };
        self.picked_clips().into_iter().filter(|c| seq.find(*c).is_some_and(|(t, _)| seq.tracks[t].kind == TrackKind::Video)).collect()
    }

    /// The first picked clip with a picture.
    fn picked_picture(&self) -> Option<Clip> {
        let id = *self.picked_pictures().first()?;
        self.seq()?.clip(id).cloned()
    }

    /// Puts `look` on `clips` (the picked ones if empty).
    pub(super) fn apply_look(&mut self, look: Look, clips: Vec<Id>, cx: &mut Context<Self>) {
        let clips = if clips.is_empty() { self.picked_pictures() } else { clips };
        if clips.is_empty() {
            self.note = Some("Pick a clip on the timeline first (or drag this onto one).".into());
            cx.notify();
            return;
        }
        match look {
            Look::Filter(i) => {
                let f = Filter::PRESETS[i].1;
                self.change(|s| clips.iter().for_each(|c| if let Some(c) = s.clip_mut(*c) { c.filter = f }), cx);
            }
            Look::Effect(kind) => {
                // on all of them already: taken off; else put on those without it
                let all = self.seq().is_some_and(|s| clips.iter().filter_map(|c| s.clip(*c)).all(|c| c.effects.iter().any(|e| e.kind == kind)));
                let settings = catalog_defaults(kind);
                self.change(
                    |s| {
                        for c in &clips {
                            let Some(c) = s.clip_mut(*c) else { continue };
                            if all {
                                c.effects.retain(|e| e.kind != kind);
                            } else if !c.effects.iter().any(|e| e.kind == kind) {
                                c.effects.push(ClipEffect::new(kind, settings.clone()));
                            }
                        }
                    },
                    cx,
                );
                if !all {
                    self.tab = InspectorTab::Effects;
                }
            }
            Look::Transition(kind) => {
                let ok: Vec<Id> = self.seq().map(|s| clips.iter().copied().filter(|c| s.before(*c).is_some()).collect()).unwrap_or_default();
                if ok.is_empty() && kind.is_some() {
                    self.note = Some("A transition goes across a cut: pick a clip right after another on its track.".into());
                    cx.notify();
                    return;
                }
                self.change(
                    |s| {
                        for id in &ok {
                            let longest = s.before(*id).map_or(0, Clip::len_ms).min(s.clip(*id).map_or(0, Clip::len_ms)).max(100);
                            if let Some(c) = s.clip_mut(*id) {
                                let dur = c.transition.map_or(500, |t| t.duration_ms);
                                c.transition = kind.map(|kind| Transition { kind, duration_ms: dur.min(longest) });
                            }
                        }
                    },
                    cx,
                );
            }
        }
    }

    // ------------------------------------------------------------ the media panel's tabs

    fn grid_head(&self, title: &'static str, text: &'static str) -> Div {
        div().flex().flex_col().gap_1().child(heading(title)).child(hint(text))
    }

    pub(super) fn filters_tab(&self, cx: &mut Context<Self>) -> Div {
        let current = self.picked_picture().map(|c| c.filter);
        let mut grid = cards();
        for (i, (name, f)) in Filter::PRESETS.iter().enumerate() {
            let on = current == Some(*f);
            let card = if i == 0 {
                self.none_card("ed-filter-0", on, move |this, cx| this.apply_look(Look::Filter(0), Vec::new(), cx), cx)
            } else {
                self.thumb_card(format!("ed-filter-{i}"), name, (vec![thumbs::filter(&self.thumbs.dir, i)], 0), on, Some(Look::Filter(i)), move |this, cx| this.apply_look(Look::Filter(i), Vec::new(), cx), cx)
            };
            grid = grid.child(card);
        }
        self.grid_head("FILTERS", "Click to put one on the picked clips, or drag it onto a clip. Fine-tune it in Adjust.").gap_2().child(grid)
    }

    pub(super) fn effects_tab(&self, cx: &mut Context<Self>) -> Div {
        let has: Vec<String> = self.picked_picture().map(|c| c.effects.iter().map(|e| e.kind.clone()).collect()).unwrap_or_default();
        let mut body = self.grid_head("EFFECTS", "Click to add one to the picked clips (again to take it off), or drag it onto a clip.").gap_2();
        let all = thumbs::effects();
        for cat in lightair_model::catalog::Category::ALL {
            let defs: Vec<_> = all.iter().filter(|d| d.category == cat).collect();
            if defs.is_empty() {
                continue;
            }
            let mut grid = cards();
            for d in defs {
                let kind = d.kind;
                let on = has.iter().any(|k| k == kind);
                grid = grid.child(self.thumb_card(format!("ed-fx-{kind}"), d.name, (vec![thumbs::effect(&self.thumbs.dir, kind)], 0), on, Some(Look::Effect(kind)), move |this, cx| this.apply_look(Look::Effect(kind), Vec::new(), cx), cx));
            }
            body = body.child(label(cat.name())).child(grid);
        }
        body
    }

    pub(super) fn transitions_tab(&self, cx: &mut Context<Self>) -> Div {
        let current = self.picked_picture().and_then(|c| c.transition.map(|t| t.kind));
        let has_clip = self.picked_picture().is_some();
        let mut grid = cards().child(self.none_card("ed-tr-none", has_clip && current.is_none(), |this, cx| this.apply_look(Look::Transition(None), Vec::new(), cx), cx));
        for (i, (name, kind)) in TransitionKind::ALL.iter().enumerate() {
            let kind = *kind;
            let frames = (0..FRAMES).map(|f| thumbs::transition(&self.thumbs.dir, i, f)).collect();
            grid = grid.child(self.thumb_card(format!("ed-tr-{i}"), name, (frames, STILL), current == Some(kind), Some(Look::Transition(Some(kind))), move |this, cx| this.apply_look(Look::Transition(Some(kind)), Vec::new(), cx), cx));
        }
        self.grid_head("TRANSITIONS", "Across the cut before the picked clip: click one, or drag it onto the clip after a cut.").gap_2().child(grid)
    }

    // ------------------------------------------------------------ the inspector's Animation

    pub(super) fn animation_section(&self, c: &Clip, body: Div, cx: &mut Context<Self>) -> Div {
        let (cid, slot) = (c.id, self.anim_slot);
        let current = match slot {
            Slot::In => c.motion.enter,
            Slot::Out => c.motion.exit,
            Slot::Combo => c.motion.combo,
        };
        let set = move |this: &mut Self, m: Option<Motion>, cx: &mut Context<Self>| {
            this.change(
                |s| {
                    if let Some(c) = s.clip_mut(cid) {
                        match slot {
                            Slot::In => c.motion.enter = m,
                            Slot::Out => c.motion.exit = m,
                            Slot::Combo => c.motion.combo = m,
                        }
                        // a combo is the whole clip; in and out are its ends
                        if m.is_some() {
                            if slot == Slot::Combo {
                                c.motion.enter = None;
                                c.motion.exit = None;
                            } else {
                                c.motion.combo = None;
                            }
                        }
                    }
                },
                cx,
            )
        };
        let len = c.len_ms();
        let mut grid = cards().child(self.none_card("ed-anim-none", current.is_none(), move |this, cx| set(this, None, cx), cx));
        for (i, (name, kind)) in slot.kinds().iter().enumerate() {
            let kind = *kind;
            let frames = (0..FRAMES).map(|f| thumbs::animation(&self.thumbs.dir, slot, i, f)).collect();
            let on = current.is_some_and(|m| m.kind == kind);
            let keep = current.map(|m| m.ms);
            grid = grid.child(self.thumb_card(format!("ed-anim-{slot:?}-{i}"), name, (frames, slot.still()), on, None, move |this, cx| {
                let ms = keep.unwrap_or(if slot == Slot::Combo { 2_000 } else { 500.min(len / 2).max(100) });
                set(this, Some(Motion { kind, ms }), cx)
            }, cx));
        }
        let slots = [("In", Slot::In), ("Out", Slot::Out), ("Combo", Slot::Combo)];
        let mut body = body
            .child(div().pt_2().child(self.segmented("ed-anim-slot".into(), &slots, slot, |this, v, cx| {
                this.anim_slot = v;
                cx.notify();
            }, cx)))
            .child(grid);
        if let Some(m) = current {
            body = match slot {
                Slot::Combo => body.child(label("SPEED")).child(self.chips("ed-anim-speed", &[("Slow", 4_000u64), ("Normal", 2_000), ("Fast", 1_000)], m.ms, move |this, v, cx| set(this, Some(Motion { ms: v, ..m }), cx), cx)),
                _ => {
                    let longest = (len / 2).max(100);
                    let options: Vec<(&'static str, u64)> = [("0.3 s", 300u64), ("0.5 s", 500), ("1 s", 1_000), ("1.5 s", 1_500), ("2 s", 2_000)].into_iter().filter(|(_, v)| *v <= longest).collect();
                    body.child(label("DURATION")).child(self.chips("ed-anim-ms", &options, m.ms, move |this, v, cx| set(this, Some(Motion { ms: v, ..m }), cx), cx))
                }
            };
        }
        body.child(hint(match slot {
            Slot::In => "How it comes in, over its first moments. Point at one to see it move.",
            Slot::Out => "How it goes out, over its last moments.",
            Slot::Combo => "One movement across the whole clip (instead of in and out).",
        }))
    }
}

fn catalog_defaults(kind: &str) -> serde_json::Map<String, serde_json::Value> {
    lightair_model::catalog::def(kind).map(|d| d.defaults()).unwrap_or_default()
}

impl Editor {
    /// For the check hook: a transition and an effect put on from the grids (the Transitions
    /// tab showing); or (`second`) an animation coming in, in the inspector, with Effects shown.
    pub fn hook_grids(&mut self, second: bool, cx: &mut Context<Self>) {
        let videos: Vec<Id> = self.seq().map(|s| s.tracks.iter().filter(|t| t.kind == TrackKind::Video).flat_map(|t| t.clips.iter().map(|c| c.id)).collect()).unwrap_or_default();
        let Some(first) = videos.first().copied() else { return };
        if second {
            self.pick_only(Some(first));
            self.apply_look(Look::Effect("fx.vhs"), vec![first], cx);
            self.change(|s| if let Some(c) = s.clip_mut(first) { c.motion.enter = Some(Motion { kind: lightair_render::motion::MotionKind::ZoomIn, ms: 1_000 }) }, cx);
            self.left = LeftTab::Effects;
            self.tab = InspectorTab::Animation;
            self.seek(400, cx);
        } else {
            let after = videos.iter().copied().find(|c| self.seq().is_some_and(|s| s.before(*c).is_some()));
            let pick = after.unwrap_or(first);
            self.pick_only(Some(pick));
            self.apply_look(Look::Transition(Some(TransitionKind::Spin)), vec![pick], cx);
            self.apply_look(Look::Filter(3), vec![pick], cx);
            self.left = LeftTab::Transitions;
            self.tab = InspectorTab::Transition;
        }
        cx.notify();
    }
}
