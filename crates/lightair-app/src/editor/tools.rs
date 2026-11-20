//! The timeline's tools, as CapCut has them: the select and split tools (and picking all to
//! the left or right of a point), undo and redo, split, delete left and right of the playhead,
//! delete, markers, freeze frame, reverse, mirror, rotate and crop;
//! snapping, linkage and the preview axis (the player following the pointer over the
//! timeline); zoom, by a slider or to fit; copy, cut, paste and duplicate; and their keys.

use super::*;
use lightair_render::sequence::Clip;

/// A clip's crop: left, top, right, bottom.
type Crop = (f32, f32, f32, f32);

/// What a click on the timeline does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tool {
    Select,
    /// Splits the clip where it's clicked.
    Split,
    /// Picks everything to the left (or right) of where it's clicked, on every track.
    Leftward,
    Rightward,
}

impl Tool {
    fn icon(self) -> &'static str {
        match self {
            Tool::Select => "mouse-pointer-2",
            Tool::Split => "scissors-line-dashed",
            Tool::Leftward => "chevrons-left",
            Tool::Rightward => "chevrons-right",
        }
    }
}

/// Clips copied: each with its medium, its track, and its start from the first one's.
#[derive(Clone)]
pub(super) struct Copied {
    clip: Clip,
    media: Option<Media>,
    track: usize,
    offset: u64,
}

fn tip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView {
    move |w, cx| gpui_kit::component::tooltip::Tooltip::new(text).build(w, cx)
}

impl Editor {
    /// The clips the tools work on: what's picked, else what's under the playhead.
    fn targets(&self) -> Vec<Id> {
        let picked = self.picked_clips();
        if !picked.is_empty() {
            return picked;
        }
        let t = self.playhead;
        self.seq().map(|s| s.tracks.iter().filter(|tr| !tr.locked).flat_map(|tr| tr.clips.iter()).filter(|c| c.start_ms <= t && t < c.end_ms()).map(|c| c.id).collect()).unwrap_or_default()
    }

    pub(super) fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = tool;
        cx.notify();
    }

    /// A click on the timeline at `t` with the split or pick-a-side tool: true if it was used.
    pub(super) fn tool_click(&mut self, clip: Option<Id>, t: u64, cx: &mut Context<Self>) -> bool {
        match self.tool {
            Tool::Select => false,
            Tool::Split => {
                let Some(c) = clip else { return true };
                let t = self.snap(t as i64, None);
                let linked = if self.linkage { self.seq().map(|s| s.linked(c)).unwrap_or_else(|| vec![c]) } else { vec![c] };
                self.change(|s| split_together(s, &linked, t), cx);
                true
            }
            Tool::Leftward | Tool::Rightward => {
                let left = self.tool == Tool::Leftward;
                let all: Vec<Id> = self
                    .seq()
                    .map(|s| s.tracks.iter().flat_map(|tr| tr.clips.iter()).filter(|c| if left { c.start_ms < t } else { c.end_ms() > t }).map(|c| c.id).collect())
                    .unwrap_or_default();
                self.selected = all.last().copied();
                self.picked = all;
                cx.notify();
                true
            }
        }
    }

    /// Cuts away what's before (or after) the playhead of the targets.
    pub(super) fn delete_side(&mut self, left: bool, cx: &mut Context<Self>) {
        let (t, only) = (self.playhead, self.picked_clips());
        let mut first: Option<Id> = None;
        self.change(
            |s| {
                let done = if left { s.delete_left(t, &only) } else { s.delete_right(t, &only) };
                first = done.first().copied();
            },
            cx,
        );
        // the playhead to where the cut was
        if left
            && let Some(start) = first.and_then(|c| self.seq()?.clip(c)).map(|c| c.start_ms)
        {
            self.seek(start, cx);
        }
    }

    pub(super) fn toggle_marker(&mut self, cx: &mut Context<Self>) {
        let (t, near) = (self.playhead, (6.0 / self.pps() * 1000.0) as u64);
        self.change(|s| s.toggle_marker(t, near), cx);
    }

    /// Mirrors (or turns a quarter) the targets' pictures.
    pub(super) fn mirror(&mut self, cx: &mut Context<Self>) {
        let targets = self.targets();
        self.change(|s| targets.iter().for_each(|c| if let Some(c) = s.clip_mut(*c) { c.flip_h = !c.flip_h }), cx);
    }

    pub(super) fn rotate(&mut self, cx: &mut Context<Self>) {
        let (targets, t) = (self.targets(), self.playhead);
        self.change(
            |s| {
                for c in &targets {
                    if let Some(c) = s.clip_mut(*c) {
                        let r = ((c.transform_at(t).rotation + 90.0) / 90.0).round() * 90.0 % 360.0;
                        kit::set_prop(c, "rotation", t, r);
                    }
                }
            },
            cx,
        );
    }

    /// The picked video clip's frame at the playhead, held for 3 s (made on a thread).
    pub(super) fn freeze(&mut self, cx: &mut Context<Self>) {
        let t = self.playhead;
        let Some(seq) = self.seq() else { return };
        let target = self.targets().into_iter().find_map(|id| {
            let c = seq.clip(id)?;
            let m = seq.media(c.media)?;
            (c.content.is_none() && m.kind == MediaKind::Video && c.start_ms <= t && t < c.end_ms()).then(|| (c.clone(), m.clone()))
        });
        let (Some((clip, media)), Some(ws)) = (target, self.workspace.clone()) else {
            self.note = Some("Put the playhead over a video clip to freeze its frame.".into());
            cx.notify();
            return;
        };
        let at = clip.source_held(t);
        let tx = self.tx.clone();
        self.note = Some("Freezing the frame…".into());
        std::thread::spawn(move || {
            let r = (|| {
                let size = (media.width.max(2) & !1, media.height.max(2) & !1);
                let rgba = lightair_render::media::frame_at(&media.path, at, size).map_err(|e| e.to_string())?;
                let dir = ws.join("edits").join("frames");
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let out = dir.join(format!("{} {at}.png", media.name.replace(['/', ':', '\\'], "-")));
                let png = image::RgbaImage::from_raw(size.0, size.1, rgba).ok_or("the frame couldn't be read")?;
                png.save(&out).map_err(|e| e.to_string())?;
                let mut m = Media::from_file(&out).map_err(|e| e.to_string())?;
                m.name = format!("{} (frozen)", media.name);
                Ok(m)
            })();
            let _ = tx.send(Done::Frozen(clip.id, t, r));
        });
        cx.notify();
    }

    pub(super) fn take_frozen(&mut self, clip: Id, t: u64, r: Result<Media, String>, cx: &mut Context<Self>) {
        let m = match r {
            Ok(m) => m,
            Err(e) => {
                self.note = Some(format!("The frame couldn't be frozen: {e}").into());
                cx.notify();
                return;
            }
        };
        self.note = None;
        let mut made = None;
        self.change(
            |s| {
                let Some((track, _)) = s.find(clip) else { return };
                if s.clip(clip).is_some_and(|c| c.start_ms < t) {
                    s.split(clip, t);
                }
                // what's after moves on by the frame's 3 s
                for c in s.tracks[track].clips.iter_mut().filter(|c| c.start_ms >= t) {
                    c.start_ms += 3_000;
                }
                let id = s.add_media(m);
                let mut still = Clip::new(id, t, 3_000);
                still.out_ms = 3_000;
                made = Some(still.id);
                s.tracks[track].clips.push(still);
                s.tracks[track].sort();
            },
            cx,
        );
        self.pick_only(made);
        cx.notify();
    }

    /// The picked clip played backwards (its piece made into a file on a thread).
    pub(super) fn reverse(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = self.seq() else { return };
        let target = self.picked_clips().into_iter().find_map(|id| {
            let c = seq.clip(id)?;
            let m = seq.media(c.media)?;
            (c.content.is_none() && m.studio_take.is_none() && m.kind != MediaKind::Image).then(|| (c.clone(), m.clone()))
        });
        let (Some((clip, media)), Some(ws)) = (target, self.workspace.clone()) else {
            self.note = Some("Pick a video or sound clip to reverse.".into());
            cx.notify();
            return;
        };
        let tx = self.tx.clone();
        self.note = Some("Reversing…".into());
        std::thread::spawn(move || {
            let r = (|| {
                let ff = lightair_render::media::ffmpeg().map_err(|e| e.to_string())?;
                let dir = ws.join("edits").join("reversed");
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let ext = if media.kind == MediaKind::Audio { "wav" } else { "mp4" };
                let out = dir.join(format!("{} {}-{} reversed.{ext}", media.name.replace(['/', ':', '\\'], "-"), clip.in_ms, clip.out_ms));
                let mut cmd = std::process::Command::new(ff);
                cmd.args(["-v", "error", "-y", "-ss", &format!("{:.3}", clip.in_ms as f64 / 1000.0), "-to", &format!("{:.3}", clip.out_ms as f64 / 1000.0), "-i"]).arg(&media.path);
                if media.kind == MediaKind::Video {
                    cmd.args(["-vf", "reverse"]);
                }
                if media.kind == MediaKind::Audio || !media.audio.is_empty() {
                    cmd.args(["-af", "areverse"]);
                }
                let ok = cmd.arg(&out).status().map_err(|e| e.to_string())?.success();
                if !ok {
                    return Err("FFmpeg couldn't reverse it".to_string());
                }
                let mut m = Media::from_file(&out).map_err(|e| e.to_string())?;
                m.name = format!("{} (reversed)", media.name);
                Ok(m)
            })();
            let _ = tx.send(Done::Reversed(clip.id, r));
        });
        cx.notify();
    }

    pub(super) fn take_reversed(&mut self, clip: Id, r: Result<Media, String>, cx: &mut Context<Self>) {
        match r {
            Ok(m) => {
                self.note = None;
                self.change(
                    |s| {
                        let len = m.duration_ms;
                        let id = s.add_media(m);
                        if let Some(c) = s.clip_mut(clip) {
                            let keep = c.len_ms();
                            c.media = id;
                            c.in_ms = 0;
                            c.out_ms = len.min(((keep as f64) * c.speed.max(0.05) as f64).round() as u64).max(1);
                        }
                    },
                    cx,
                );
            }
            Err(e) => self.note = Some(format!("It couldn't be reversed: {e}").into()),
        }
        cx.notify();
    }

    // ------------------------------------------------------------ copy, paste, duplicate

    /// What's picked, with the clips linked to it (linkage on).
    pub(super) fn picked_with_links(&self) -> Vec<Id> {
        let Some(seq) = self.seq() else { return Vec::new() };
        let mut all = self.picked_clips();
        if self.linkage {
            for c in all.clone() {
                for l in seq.linked(c) {
                    if !all.contains(&l) {
                        all.push(l);
                    }
                }
            }
        }
        all
    }

    pub fn copy_picked(&mut self, cx: &mut Context<Self>) {
        let picked = self.picked_with_links();
        let Some(seq) = self.seq() else { return };
        let mut copied: Vec<Copied> = picked
            .iter()
            .filter_map(|id| {
                let (t, _) = seq.find(*id)?;
                let c = seq.clip(*id)?.clone();
                Some(Copied { media: seq.media(c.media).cloned(), clip: c, track: t, offset: 0 })
            })
            .collect();
        let first = copied.iter().map(|c| c.clip.start_ms).min().unwrap_or(0);
        for c in &mut copied {
            c.offset = c.clip.start_ms - first;
        }
        self.note = Some(format!("{} copied", if copied.len() == 1 { "1 clip".to_string() } else { format!("{} clips", copied.len()) }).into());
        self.clipboard = copied;
        cx.notify();
    }

    pub fn cut_picked(&mut self, cx: &mut Context<Self>) {
        self.copy_picked(cx);
        // what was copied goes: linked clips too
        let gone = self.picked_with_links();
        self.picked = gone;
        self.delete_selected(cx);
    }

    /// The copied clips from `at` (the playhead): on their tracks where free, else on tracks
    /// of their own.
    pub fn paste_at(&mut self, at: Option<u64>, cx: &mut Context<Self>) {
        if self.clipboard.is_empty() {
            return;
        }
        let (copied, at) = (self.clipboard.clone(), at.unwrap_or(self.playhead));
        let mut made = Vec::new();
        self.change(
            |s| {
                // new links for the copies, so they don't go with the originals
                let mut links: std::collections::HashMap<Id, Id> = Default::default();
                for c in copied {
                    let mut clip = c.clip.clone();
                    clip.id = Id::new();
                    clip.start_ms = at + c.offset;
                    clip.link = clip.link.map(|l| *links.entry(l).or_default());
                    if let Some(m) = &c.media
                        && s.media(m.id).is_none()
                    {
                        s.add_media(m.clone());
                    }
                    let len = clip.len_ms();
                    let kind = s.tracks.get(c.track).map_or(TrackKind::Video, |t| t.kind);
                    let track = if s.tracks.get(c.track).is_some_and(|t| !t.locked && t.free(clip.start_ms, clip.start_ms + len, None)) {
                        c.track
                    } else {
                        let t = lightair_render::sequence::Track::empty(kind);
                        match kind {
                            TrackKind::Video => {
                                s.tracks.insert(0, t);
                                0
                            }
                            TrackKind::Audio => {
                                s.tracks.push(t);
                                s.tracks.len() - 1
                            }
                        }
                    };
                    made.push(clip.id);
                    s.tracks[track].clips.push(clip);
                    s.tracks[track].sort();
                }
            },
            cx,
        );
        self.selected = made.last().copied();
        self.picked = made;
        cx.notify();
    }

    /// Copies of the picked clips right after them.
    pub fn duplicate_picked(&mut self, cx: &mut Context<Self>) {
        let end = self.picked_with_links().iter().filter_map(|c| self.seq()?.clip(*c)).map(|c| c.end_ms()).max();
        let Some(end) = end else { return };
        self.copy_picked(cx);
        self.paste_at(Some(end), cx);
    }

    // ------------------------------------------------------------ the playhead

    /// Moves the playhead by frames (or to the previous or next cut, or an end).
    pub(super) fn step(&mut self, frames: i64, cx: &mut Context<Self>) {
        let frame = 1000.0 / self.seq().map_or(30.0, |s| s.fps.max(1.0));
        let t = (self.playhead as f64 + frames as f64 * frame).max(0.0) as u64;
        let end = self.seq().map_or(0, |s| s.duration_ms());
        self.seek(t.min(end), cx);
    }

    pub(super) fn jump_cut(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(seq) = self.seq() else { return };
        let mut cuts = seq.snap_points(None);
        cuts.extend(seq.markers.iter().copied());
        cuts.sort();
        cuts.dedup();
        let t = self.playhead;
        let to = if forward { cuts.into_iter().find(|c| *c > t + 1) } else { cuts.into_iter().rev().find(|c| *c + 1 < t) };
        if let Some(to) = to {
            self.seek(to, cx);
        }
    }

    // ------------------------------------------------------------ zoom

    /// Zoom from a slider's place (0..1, on a log scale).
    fn zoom_from(&mut self, f: f32, cx: &mut Context<Self>) {
        let (lo, hi) = (0.05f32.ln(), 40f32.ln());
        self.zoom = (lo + f.clamp(0.0, 1.0) * (hi - lo)).exp();
        cx.notify();
    }

    fn zoom_place(&self) -> f32 {
        let (lo, hi) = (0.05f32.ln(), 40f32.ln());
        (self.zoom.ln() - lo) / (hi - lo)
    }

    /// The whole sequence in view.
    pub(super) fn zoom_to_fit(&mut self, cx: &mut Context<Self>) {
        let width = self.timeline_bounds.borrow().map_or(1200.0, |b| f32::from(b.size.width) - HEADER_W - 40.0);
        let len = self.seq().map_or(0, |s| s.duration_ms()).max(1_000) as f32 / 1000.0;
        self.zoom = (width / len / PPS).clamp(0.05, 40.0);
        self.scroll_ms = 0.0;
        cx.notify();
    }

    // ------------------------------------------------------------ the preview axis

    /// The pointer over the timeline: the player shows that moment (preview axis on).
    pub(super) fn hover_at(&mut self, x: Option<Pixels>, cx: &mut Context<Self>) {
        if !self.axis || self.playing.is_some() || self.drag.is_some() || self.marquee.is_some() {
            return;
        }
        let t = x.and_then(|x| self.time_at(x)).map(|t| t.min(self.seq().map_or(0, |s| s.duration_ms())));
        if t == self.hover {
            return;
        }
        self.hover = t;
        self.request(t.unwrap_or(self.playhead));
        cx.notify();
    }

    // ------------------------------------------------------------ keys

    /// The Editor's keys (when no text field has them): true if it took the key.
    pub fn key(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = &k.modifiers;
        let cmd = if cfg!(target_os = "macos") { m.platform } else { m.control };
        match (k.key.as_str(), cmd, m.shift) {
            ("a", false, false) => self.set_tool(Tool::Select, cx),
            ("b", false, false) => self.set_tool(Tool::Split, cx),
            ("[", false, false) => self.set_tool(Tool::Leftward, cx),
            ("]", false, false) => self.set_tool(Tool::Rightward, cx),
            ("b", true, false) => self.split(cx),
            ("q", false, false) => self.delete_side(true, cx),
            ("w", false, false) => self.delete_side(false, cx),
            ("m", false, false) => self.toggle_marker(cx),
            ("s", false, false) => {
                self.snapping = !self.snapping;
                cx.notify();
            }
            ("left", false, s) => self.step(if s { -10 } else { -1 }, cx),
            ("right", false, s) => self.step(if s { 10 } else { 1 }, cx),
            ("up", false, false) => self.jump_cut(false, cx),
            ("down", false, false) => self.jump_cut(true, cx),
            ("home", _, _) => self.seek(0, cx),
            ("end", _, _) => {
                let end = self.seq().map_or(0, |s| s.duration_ms());
                self.seek(end, cx)
            }
            ("=", true, _) | ("+", true, _) => {
                self.zoom = (self.zoom * 1.5).min(40.0);
                cx.notify();
            }
            ("-", true, _) => {
                self.zoom = (self.zoom / 1.5).max(0.05);
                cx.notify();
            }
            ("0", false, true) => self.zoom_to_fit(cx),
            ("escape", _, _) => {
                self.pick_only(None);
                self.tool = Tool::Select;
                cx.notify();
            }
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------ the bar

    pub(super) fn tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity().downgrade();
        let tool = self.tool;
        let picker = Button::new("ed-tool").icon(ui::icon(tool.icon())).xsmall().ghost().dropdown_caret(true).dropdown_menu(move |menu, _, _| {
            let item = |label: &'static str, key: &'static str, t: Tool| {
                let me = me.clone();
                PopupMenuItem::new(format!("{label}    {key}")).icon(ui::icon(t.icon())).checked(tool == t).on_click(move |_, _, cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |this, cx| this.set_tool(t, cx));
                    }
                })
            };
            menu.item(item("Select", "A", Tool::Select))
                .item(item("Split", "B", Tool::Split))
                .item(item("Select leftward", "[", Tool::Leftward))
                .item(item("Select rightward", "]", Tool::Rightward))
        });
        let sep = || div().w(px(1.)).h(px(16.)).mx_1().bg(tok::border());
        let button = |id: &'static str, icon: &'static str, label: &'static str| ui::icon_button(id, icon, tok::text_secondary()).tooltip(tip(label));
        let toggle = |id: &'static str, icon: &'static str, label: &'static str, on: bool| {
            ui::icon_button(id, icon, if on { tok::accent_primary() } else { tok::text_tertiary() }).tooltip(tip(label)).when(on, |d| d.bg(tok::bg_selected()))
        };
        let slider = {
            let bounds = self.zoom_bar.clone();
            let place = self.zoom_place();
            div()
                .id("ed-zoom-bar")
                .relative()
                .w(px(110.))
                .h(px(16.))
                .flex()
                .items_center()
                .cursor_pointer()
                .child(canvas(move |b, _, _| *bounds.borrow_mut() = Some(b), |_, _, _, _| {}).absolute().size_full())
                .child(div().w_full().h(px(2.)).rounded_full().bg(tok::border_strong()))
                .child(div().absolute().left(px(place * 104.0)).size(px(10.)).rounded_full().bg(tok::text()))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.zoom_dragging = true;
                    this.zoom_at(ev.position.x, cx);
                }))
                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                    if this.zoom_dragging {
                        this.zoom_at(ev.position.x, cx);
                    }
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.zoom_dragging = false))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.zoom_dragging = false))
        };
        div()
            .flex()
            .items_center()
            .gap_0p5()
            .px_2()
            .h(px(36.))
            .border_t_1()
            .border_b_1()
            .border_color(tok::border())
            .child(button("ed-t-add", "plus", "Import media").on_click(cx.listener(|this, _, w, cx| this.import(w, cx))))
            .child(picker)
            .child(sep())
            .child(button("ed-t-undo", "undo-2", "Undo").on_click(cx.listener(|this, _, _, cx| this.undo(true, cx))))
            .child(button("ed-t-redo", "redo-2", "Redo").on_click(cx.listener(|this, _, _, cx| this.undo(false, cx))))
            .child(sep())
            .child(button("ed-split", "scissors", crate::ui::keys_static("Split at the playhead (⌘B)")).on_click(cx.listener(|this, _, _, cx| this.split(cx))))
            .child(button("ed-t-dl", "arrow-left-to-line", "Delete left of the playhead (Q)").on_click(cx.listener(|this, _, _, cx| this.delete_side(true, cx))))
            .child(button("ed-t-dr", "arrow-right-to-line", "Delete right of the playhead (W)").on_click(cx.listener(|this, _, _, cx| this.delete_side(false, cx))))
            .child(button("ed-del", "trash", "Delete (⌫)").on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx))))
            .child(sep())
            .child(button("ed-t-mark", "flag", "Marker at the playhead (M)").on_click(cx.listener(|this, _, _, cx| this.toggle_marker(cx))))
            .child(button("ed-t-freeze", "snowflake", "Freeze frame: 3 s of the frame at the playhead").on_click(cx.listener(|this, _, _, cx| this.freeze(cx))))
            .child(button("ed-t-rev", "rewind", "Reverse").on_click(cx.listener(|this, _, _, cx| this.reverse(cx))))
            .child(button("ed-t-mirror", "flip-horizontal-2", "Mirror").on_click(cx.listener(|this, _, _, cx| this.mirror(cx))))
            .child(button("ed-t-rotate", "rotate-cw", "Rotate 90°").on_click(cx.listener(|this, _, _, cx| this.rotate(cx))))
            .child(button("ed-t-crop", "crop", "Crop (in the inspector)").on_click(cx.listener(|this, _, _, cx| {
                this.tab = InspectorTab::Video;
                this.note = Some("Crop: in the inspector, under the transform.".into());
                cx.notify();
            })))
            .child(sep())
            .child(
                ui::icon_button("ed-vo", if self.recorder.is_some() { "square" } else { "mic" }, if self.recorder.is_some() { rgb(0xEF4444).into() } else { tok::text_secondary() })
                    .tooltip(tip("Record a voice-over from the playhead"))
                    .on_click(cx.listener(|this, _, _, cx| this.voice_over(cx))),
            )
            .when_some(self.recorder.as_ref(), |d, r| {
                let s = r.began.elapsed().as_secs();
                d.child(div().text_size(px(12.)).text_color(rgb(0xEF4444)).child(format!("● {}:{:02}", s / 60, s % 60)))
            })
            .child(div().flex_1())
            .when_some(self.note.clone(), |d, n| d.child(div().text_size(px(12.)).text_color(tok::text_secondary()).overflow_hidden().text_ellipsis().whitespace_nowrap().child(n)))
            .when_some(self.proxying.clone(), |d, (name, f)| {
                d.child(div().pl_3().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).text_color(tok::text_tertiary()).child(format!("Proxy for “{name}” {:.0}%", f * 100.0)))
            })
            .child(div().flex_1())
            .child(toggle("ed-t-snap", "between-horizontal-start", "Snapping (S)", self.snapping).on_click(cx.listener(|this, _, _, cx| {
                this.snapping = !this.snapping;
                cx.notify();
            })))
            .child(toggle("ed-t-link", if self.linkage { "link" } else { "unlink" }, "Linkage: linked clips go together (⌥-drag moves one alone)", self.linkage).on_click(cx.listener(|this, _, _, cx| {
                let on = !this.linkage;
                this.set_linkage(on, cx)
            })))
            .child(toggle("ed-t-axis", "scan-line", "Preview axis: the player follows the pointer", self.axis).on_click(cx.listener(|this, _, _, cx| {
                this.axis = !this.axis;
                cx.notify();
            })))
            .child(toggle("ed-scrub", "audio-waveform", "Hear the sound while scrubbing", self.scrub_audio).on_click(cx.listener(|this, _, _, cx| {
                this.scrub_audio = !this.scrub_audio;
                cx.notify();
            })))
            .child(sep())
            .child(button("ed-zout", "zoom-out", crate::ui::keys_static("Zoom out (⌘−)")).on_click(cx.listener(|this, _, _, cx| {
                this.zoom = (this.zoom / 1.5).max(0.05);
                cx.notify();
            })))
            .child(slider)
            .child(button("ed-zin", "zoom-in", crate::ui::keys_static("Zoom in (⌘=)")).on_click(cx.listener(|this, _, _, cx| {
                this.zoom = (this.zoom * 1.5).min(40.0);
                cx.notify();
            })))
            .child(button("ed-zfit", "maximize-2", crate::ui::keys_static("Fit the timeline (⇧0)")).on_click(cx.listener(|this, _, _, cx| this.zoom_to_fit(cx))))
    }

    fn zoom_at(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(b) = *self.zoom_bar.borrow() else { return };
        let f = f32::from(x - b.origin.x) / f32::from(b.size.width).max(1.0);
        self.zoom_from(f, cx);
    }

    /// The inspector's crop.
    pub(super) fn crop_section(&self, c: &Clip, body: Div, cx: &mut Context<Self>) -> Div {
        let cid = c.id;
        let set = move |this: &mut Self, f: &dyn Fn(&mut Crop), cx: &mut Context<Self>| this.change(|s| if let Some(c) = s.clip_mut(cid) { f(&mut c.crop) }, cx);
        use std::rc::Rc;
        let _ = set;
        let pct: Rc<dyn Fn(f32) -> String> = Rc::new(|v| format!("{:.0}%", v * 100.0));
        body.child(self.keyed_title(c, "CROP", &lightair_render::keys::CROP, cx))
            .child(self.keyed(c, "crop.l", "Left", pct.clone(), 0.05, (0.0, 0.9), cx))
            .child(self.keyed(c, "crop.r", "Right", pct.clone(), 0.05, (0.0, 0.9), cx))
            .child(self.keyed(c, "crop.t", "Top", pct.clone(), 0.05, (0.0, 0.9), cx))
            .child(self.keyed(c, "crop.b", "Bottom", pct, 0.05, (0.0, 0.9), cx))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(self.chips("ed-flip", &[("Mirror", 0u8), ("Flip", 1)], if c.flip_h { 0 } else if c.flip_v { 1 } else { 9 }, move |this, v, cx| {
                        this.change(|s| if let Some(c) = s.clip_mut(cid) { if v == 0 { c.flip_h = !c.flip_h } else { c.flip_v = !c.flip_v } }, cx)
                    }, cx))
                    .child(Button::new("ed-crop-reset").label("Reset").xsmall().ghost().on_click(cx.listener(move |this, _, _, cx| {
                        this.change(|s| if let Some(c) = s.clip_mut(cid) {
                            c.crop = (0.0, 0.0, 0.0, 0.0);
                            c.flip_h = false;
                            c.flip_v = false;
                        }, cx)
                    }))),
            )
    }
}

/// Splits `clips` at `t`; the right halves linked together anew (apart from the left ones).
pub(super) fn split_together(s: &mut Sequence, clips: &[Id], t: u64) {
    let link = (clips.len() > 1).then(Id::new);
    for c in clips {
        if let Some(right) = s.split(*c, t)
            && let Some(r) = s.clip_mut(right)
        {
            r.link = link.or(r.link.map(|_| Id::new()));
        }
    }
}

impl Editor {
    /// For the screenshot hook: every clip picked, two markers, the split tool.
    pub fn hook_tools(&mut self, cx: &mut Context<Self>) {
        self.change(
            |s| {
                s.toggle_marker(4_000, 10);
                s.toggle_marker(15_000, 10);
            },
            cx,
        );
        self.select_all(cx);
        self.tool = Tool::Split;
        self.seek(6_000, cx);
    }
}

impl Editor {
    /// For the check hook: the first clip's right edge dragged 100 points left, as a drag
    /// would (once the timeline is laid out); its length before and after, logged.
    pub fn hook_trim(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                let Some(b) = *this.timeline_bounds.borrow() else { return };
                let Some(c) = this.seq().and_then(|s| s.tracks.iter().find(|t| t.kind == TrackKind::Video)).and_then(|t| t.clips.first()).cloned() else { return };
                let before = c.len_ms();
                let x = f32::from(b.origin.x) + HEADER_W + this.x_of(c.end_ms()) - 100.0;
                let y = this.track_top(this.seq().and_then(|s| s.find(c.id)).map_or(0, |f| f.0)).unwrap_or(0.0) + 20.0;
                this.begin_drag(Drag::TrimEnd(c.id));
                this.drag_to(point(px(x), px(y)), cx);
                this.drag_end(cx);
                let after = this.seq().and_then(|s| s.clip(c.id)).map_or(0, |c| c.len_ms());
                tracing::info!(before, after, "CHECK trim end by 100 points");
            });
        })
        .detach();
    }
}

impl Editor {
    /// For the check hook: the first two video clips linked, then the second dragged 100 points
    /// right three ways (as it is; with ⌥; linkage off), and where both went, logged.
    pub fn hook_alone(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                let Some(b) = *this.timeline_bounds.borrow() else { return };
                let ids: Vec<Id> = this.seq().and_then(|s| s.tracks.iter().find(|t| t.kind == TrackKind::Video)).map(|t| t.clips.iter().map(|c| c.id).collect()).unwrap_or_default();
                let [a, second, ..] = ids[..] else { return };
                let link = Id::new();
                this.change(|s| [a, second].iter().for_each(|c| if let Some(c) = s.clip_mut(*c) { c.link = Some(link) }), cx);
                for (way, alone, linkage) in [("linked", false, true), ("option", true, true), ("linkage off", false, false)] {
                    this.pick_only(None);
                    this.set_linkage(true, cx);
                    // picked as a click would (with linkage, the pair), then linkage switched
                    this.pick_for_drag(second, false);
                    this.set_linkage(linkage, cx);
                    let start = |this: &Self, c: Id| this.seq().and_then(|s| s.clip(c)).map_or(0, |c| c.start_ms);
                    let (a0, s0) = (start(this, a), start(this, second));
                    this.drag_alone = alone;
                    this.pick_for_drag(second, alone);
                    let x = f32::from(b.origin.x) + HEADER_W + this.x_of(s0) + 100.0;
                    let y = this.track_top(this.seq().and_then(|s| s.find(second)).map_or(0, |f| f.0)).unwrap_or(0.0) + 20.0;
                    this.begin_drag(Drag::Move { clip: second, grab_ms: 0 });
                    this.drag_to(point(px(x), px(y)), cx);
                    this.drag_end(cx);
                    let (a1, s1) = (start(this, a), start(this, second));
                    tracing::info!(way, other_moved = a1 as i64 - a0 as i64, dragged_moved = s1 as i64 - s0 as i64, "CHECK drag one of a linked pair");
                }
                this.set_linkage(true, cx);
            });
        })
        .detach();
    }
}
