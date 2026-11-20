//! Words and looks in the Editor (docs/plans/video-editor.md, phase 3): text and shapes from
//! the media panel, captions made from the project's sound, and the inspector's text, shape,
//! filter, transition and keyframe controls.

use super::*;
use lightair_render::sequence::{Clip, Content, Filter, Keyframe, ShapeKind, ShapeStyle, TextStyle, Transition, TransitionKind};

/// Colours offered for words and shapes.
const PALETTE: [u32; 9] = [0xFFFFFF, 0x111111, 0xFACC15, 0xF97316, 0xF43F5E, 0xA855F7, 0x3B82F6, 0x22C55E, 0x67E8F9];

/// Stickers offered (emoji, in the system's colour emoji font).
const STICKERS: [&str; 40] = [
    "😀", "😂", "😍", "🥳", "😎", "🤯", "😱", "🤔", "👍", "👎", "👏", "🙌", "🙏", "💪", "👀", "🔥", "✨", "⭐", "💯", "❤️", "💡", "✅", "❌", "⚠️",
    "❓", "❗", "➡️", "⬅️", "⬆️", "⬇️", "🎉", "🎯", "🚀", "📌", "📣", "🔔", "💬", "🎬", "📸", "🏆",
];

/// Where the speech model is kept.
pub(super) fn model() -> PathBuf {
    lightair_render::speech::model_path(&lightair_platform::paths::data_dir())
}

/// The text styles offered, sized for a sequence of `size`: (name, style, where it goes).
pub(super) fn text_presets(size: (u32, u32)) -> Vec<(&'static str, TextStyle, Transform)> {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let k = h / 1080.0;
    let base = |text: &str, px: f32| TextStyle { text: text.into(), size: px * k, color: 0xFFFFFF, bold: false, align: 1, background: None, outline: None, width: w * 0.8, font: None };
    let at = |x: f32, y: f32| Transform { x: x * w, y: y * h, ..Transform::default() };
    vec![
        ("Title", TextStyle { bold: true, ..base("Title", 110.0) }, Transform::default()),
        ("Subtitle", TextStyle { outline: Some(0x000000), ..base("Subtitle", 60.0) }, at(0.0, 0.3)),
        ("Lower third", TextStyle { bold: true, align: 0, background: Some((0x111827, 0.85)), width: w * 0.4, ..base("Name · Role", 46.0) }, at(-0.26, 0.3)),
        ("Label", TextStyle { bold: true, color: 0x111111, background: Some((0xFACC15, 1.0)), ..base("NEW", 52.0) }, at(0.32, -0.32)),
        ("Neon", TextStyle { bold: true, color: 0x67E8F9, outline: Some(0x0E7490), ..base("Neon", 120.0) }, Transform::default()),
        ("Quote", TextStyle { background: Some((0x000000, 0.55)), ..base("“A few words”", 64.0) }, Transform::default()),
    ]
}

/// The shapes offered, sized for a sequence of `size`.
pub(super) fn shape_presets(size: (u32, u32)) -> Vec<(&'static str, ShapeStyle)> {
    let k = size.1 as f32 / 1080.0;
    let s = |shape, w: f32, h: f32, fill: Option<(u32, f32)>, stroke: Option<(u32, f32)>, radius: f32| ShapeStyle {
        shape,
        width: w * k,
        height: h * k,
        fill,
        stroke: stroke.map(|(c, l)| (c, l * k)),
        radius: radius * k,
    };
    vec![
        ("Box", s(ShapeKind::Rectangle, 600.0, 340.0, Some((0xFFFFFF, 1.0)), None, 0.0)),
        ("Rounded", s(ShapeKind::Rectangle, 600.0, 340.0, Some((0x3B82F6, 0.9)), None, 40.0)),
        ("Circle", s(ShapeKind::Ellipse, 360.0, 360.0, Some((0xF43F5E, 1.0)), None, 0.0)),
        ("Frame", s(ShapeKind::Rectangle, 640.0, 360.0, None, Some((0xFACC15, 12.0)), 16.0)),
        ("Ring", s(ShapeKind::Ellipse, 360.0, 360.0, None, Some((0xFFFFFF, 14.0)), 0.0)),
        ("Arrow", s(ShapeKind::Arrow, 420.0, 160.0, Some((0xF43F5E, 1.0)), None, 0.0)),
    ]
}

/// What a made clip is called on the timeline.
pub(super) fn content_name(c: &Content) -> String {
    match c {
        Content::Text(t) => {
            let line = t.text.lines().next().unwrap_or("").trim();
            if line.is_empty() { "Text".into() } else { line.into() }
        }
        Content::Shape(s) => match s.shape {
            ShapeKind::Rectangle => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Arrow => "Arrow",
        }
        .into(),
    }
}

fn hex(c: u32) -> Rgba {
    rgb(c)
}

impl Editor {
    // ------------------------------------------------------------ the media panel's tabs

    /// A card for a text style or shape: a sample, added at the playhead on a click or dragged
    /// onto a track.
    fn look_card(&self, id: SharedString, name: &'static str, sample: AnyElement, source: MediaSource, cx: &mut Context<Self>) -> Stateful<Div> {
        let icon = match &source {
            MediaSource::Content(Content::Shape(_), _) => "shapes",
            _ => "type",
        };
        let drag = DraggedMedia::new(name, source.clone(), None, icon, &self.drag_hidden);
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap_1()
            .w(px(122.))
            .cursor_pointer()
            .child(
                div()
                    .w_full()
                    .h(px(68.))
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(rgb(0x1F2937))
                    .border_1()
                    .border_color(tok::border())
                    .hover(|d| d.border_color(tok::accent_primary()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(sample),
            )
            .child(div().text_size(px(11.)).child(name))
            .on_click(cx.listener(move |this, _, _, cx| this.add(source.clone(), None, None, cx)))
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
    }

    pub(super) fn text_tab(&self, cx: &mut Context<Self>) -> Div {
        let size = self.seq().map_or((1920, 1080), |s| s.size);
        let mut grid = div().flex().flex_wrap().gap_2();
        for (n, (name, style, place)) in text_presets(size).into_iter().enumerate() {
            let sample = div()
                .px_1p5()
                .py_0p5()
                .rounded(px(3.))
                .text_size(px(15.))
                .when(style.bold, |d| d.font_weight(FontWeight::BOLD))
                .text_color(hex(style.color))
                .when_some(style.background, |d, (c, a)| d.bg(Rgba { a, ..hex(c) }))
                .child(name)
                .into_any_element();
            grid = grid.child(self.look_card(SharedString::from(format!("ed-txt-{n}")), name, sample, MediaSource::Content(Content::Text(style), place), cx));
        }
        div().flex().flex_col().gap_2().child(heading("TEXT")).child(grid).child(hint("Click to add at the playhead, or drag onto a track."))
    }

    pub(super) fn shapes_tab(&self, cx: &mut Context<Self>) -> Div {
        let size = self.seq().map_or((1920, 1080), |s| s.size);
        let k = size.1 as f32 / 1080.0;
        // emoji stickers: big, with a white edge
        let mut stickers = div().flex().flex_wrap().gap_1();
        for (n, e) in STICKERS.iter().enumerate() {
            let style = TextStyle { text: (*e).into(), size: 220.0 * k, color: 0xFFFFFF, bold: false, align: 1, background: None, outline: Some(0xFFFFFF), width: size.0 as f32, font: None };
            let source = MediaSource::Content(Content::Text(style), Transform::default());
            let drag = DraggedMedia::new("Sticker", source.clone(), None, *e, &self.drag_hidden);
            stickers = stickers.child(
                div()
                    .id(("ed-stk", n))
                    .size(px(38.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(24.))
                    .cursor_pointer()
                    .hover(|d| d.bg(tok::bg_selected()))
                    .child(*e)
                    .on_click(cx.listener(move |this, _, _, cx| this.add(source.clone(), None, None, cx)))
                    .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone())),
            );
        }
        let mut grid = div().flex().flex_wrap().gap_2();
        for (n, (name, style)) in shape_presets(size).into_iter().enumerate() {
            let (w, h) = if style.width >= style.height { (44.0, 44.0 * style.height / style.width) } else { (44.0 * style.width / style.height, 44.0) };
            let sample = match style.shape {
                ShapeKind::Arrow => ui::icon("arrow-right").size(px(36.)).text_color(hex(style.fill.map_or(0xFFFFFF, |f| f.0))).into_any_element(),
                _ => div()
                    .w(px(w))
                    .h(px(h))
                    .when(style.shape == ShapeKind::Ellipse, |d| d.rounded_full())
                    .when(style.shape == ShapeKind::Rectangle, |d| d.rounded(px(style.radius / 16.0)))
                    .when_some(style.fill, |d, (c, a)| d.bg(Rgba { a, ..hex(c) }))
                    .when_some(style.stroke, |d, (c, _)| d.border_2().border_color(hex(c)))
                    .into_any_element(),
            };
            grid = grid.child(self.look_card(SharedString::from(format!("ed-shape-{n}")), name, sample, MediaSource::Content(Content::Shape(style), Transform::default()), cx));
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("STICKERS"))
            .child(stickers)
            .child(heading("SHAPES"))
            .child(grid)
            .child(hint("Click to add at the playhead, or drag onto a track. A sticker is text: its size and edge are in the inspector."))
    }

    pub(super) fn captions_tab(&self, cx: &mut Context<Self>) -> Div {
        let has = self.seq().is_some_and(|s| s.tracks.iter().any(|t| t.captions && !t.clips.is_empty()));
        let action = if self.captioning {
            Button::new("ed-cap").label("Listening…").small().outline().loading(true).into_any_element()
        } else if self.downloading {
            Button::new("ed-cap").label("Downloading the speech model…").small().outline().loading(true).into_any_element()
        } else if !model().is_file() {
            Button::new("ed-cap")
                .label("Get speech model")
                .icon(ui::icon("download"))
                .small()
                .outline()
                .tooltip("142 MB, once: speech is recognised on this computer, never sent anywhere")
                .on_click(cx.listener(|this, _, _, cx| this.download_model(cx)))
                .into_any_element()
        } else {
            Button::new("ed-cap")
                .label(if has { "Make captions again" } else { "Make captions" })
                .icon(ui::icon("captions"))
                .small()
                .primary()
                .on_click(cx.listener(|this, _, _, cx| this.make_captions(cx)))
                .into_any_element()
        };
        let styles: [(&'static str, i32); 3] = [("Boxed", 0), ("Outlined", 1), ("Yellow", 2)];
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("CAPTIONS"))
            .child(hint("The words heard in the project's sound become text clips on a captions track. Speech is recognised on this computer."))
            .child(div().flex().child(action))
            .when(has, |d| {
                d.child(heading("STYLE")).child(self.chips("ed-capstyle", &styles, -1, |this, v, cx| {
                    this.restyle_captions(
                        move |t| match v {
                            0 => {
                                t.color = 0xFFFFFF;
                                t.background = Some((0x000000, 0.6));
                                t.outline = None;
                            }
                            1 => {
                                t.color = 0xFFFFFF;
                                t.background = None;
                                t.outline = Some(0x000000);
                            }
                            _ => {
                                t.color = 0xFACC15;
                                t.background = None;
                                t.outline = Some(0x000000);
                            }
                        },
                        cx,
                    )
                }, cx))
                .child(hint("Select a caption to change its words, or its look for all of them."))
            })
    }

    // ------------------------------------------------------------ captions

    pub(super) fn download_model(&mut self, cx: &mut Context<Self>) {
        if self.downloading {
            return;
        }
        let (path, tx) = (model(), self.tx.clone());
        self.downloading = true;
        self.note = Some("Downloading the speech model (142 MB)…".into());
        std::thread::spawn(move || {
            let r = (|| {
                std::fs::create_dir_all(path.parent().ok_or("no folder")?).map_err(|e| e.to_string())?;
                let part = path.with_extension("part");
                let curl = if cfg!(windows) { "curl.exe" } else { "curl" };
                let ok = std::process::Command::new(curl)
                    .args(["-fsSL", "-o"])
                    .arg(&part)
                    .arg(lightair_render::speech::MODEL_URL)
                    .status()
                    .map_err(|e| e.to_string())?
                    .success();
                if !ok {
                    let _ = std::fs::remove_file(&part);
                    return Err("couldn't reach the download".to_string());
                }
                std::fs::rename(&part, &path).map_err(|e| e.to_string())
            })();
            let _ = tx.send(Done::Downloaded(r));
        });
        cx.notify();
    }

    /// Listens to the project's sound (on a thread) and makes its captions.
    pub(super) fn make_captions(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = self.seq().cloned() else { return };
        if self.captioning {
            return;
        }
        self.captioning = true;
        self.note = Some("Listening to the project…".into());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = (|| {
                let dir = std::env::temp_dir().join(format!("lightair-captions-{}", std::process::id()));
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let wav = dir.join("sound.wav");
                let heard = lightair_render::seqrender::sound_to(&seq, &wav).map_err(|e| e.to_string());
                let words = match heard {
                    Ok(true) => lightair_render::speech::transcribe(std::slice::from_ref(&wav), &model()).map_err(|e| e.to_string()),
                    Ok(false) => Err("the project has no sound to listen to".to_string()),
                    Err(e) => Err(e),
                };
                let _ = std::fs::remove_dir_all(&dir);
                Ok(lightair_render::speech::caption_lines(&words?, 42))
            })();
            let _ = tx.send(Done::Captions(r));
        });
        cx.notify();
    }

    /// The captions heard: on the captions track (in the look they had, if any).
    pub(super) fn take_captions(&mut self, r: Result<Vec<(u64, u64, String)>, String>, cx: &mut Context<Self>) {
        self.captioning = false;
        match r {
            Ok(lines) if lines.is_empty() => self.note = Some("No words were heard.".into()),
            Ok(lines) => {
                let n = lines.len();
                self.change(
                    |s| {
                        let had = s.tracks.iter().find(|t| t.captions).and_then(|t| t.clips.first()).and_then(|c| match &c.content {
                            Some(Content::Text(t)) => Some((t.clone(), c.transform)),
                            _ => None,
                        });
                        let (style, place) = had.unwrap_or_else(|| s.caption_style());
                        s.set_captions(&lines, &style, place);
                    },
                    cx,
                );
                self.note = Some(format!("{n} captions made").into());
            }
            Err(e) => self.note = Some(format!("Captions couldn't be made: {e}").into()),
        }
        cx.notify();
    }

    /// Changes the look of every caption.
    fn restyle_captions(&mut self, f: impl Fn(&mut TextStyle), cx: &mut Context<Self>) {
        self.change(
            |s| {
                for t in s.tracks.iter_mut().filter(|t| t.captions) {
                    for c in &mut t.clips {
                        if let Some(Content::Text(style)) = &mut c.content {
                            f(style);
                        }
                    }
                }
            },
            cx,
        );
    }

    // ------------------------------------------------------------ the selected text's words

    /// Keeps the text box in step with the selected text clip (made when one is selected).
    pub(super) fn sync_text_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = self.selected.and_then(|id| self.seq()?.clip(id)).and_then(|c| match &c.content {
            Some(Content::Text(t)) => Some((c.id, t.text.clone())),
            _ => None,
        });
        let Some((id, text)) = want else {
            self.text_edit = None;
            return;
        };
        if let Some((cur, input)) = &self.text_edit
            && *cur == id
        {
            // changed elsewhere (undo): shown, unless it's being typed
            if self.text_before.is_none() && input.read(cx).value() != text {
                input.update(cx, |s, cx| s.set_value(text, window, cx));
            }
            return;
        }
        self.text_before = None;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Words"));
        input.update(cx, |s, cx| s.set_value(text, window, cx));
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, _, cx| match ev {
            InputEvent::Change => {
                let value = input.read(cx).value().to_string();
                let Some(p) = &mut this.project else { return };
                let same = p.sequence.clip(id).is_none_or(|c| matches!(&c.content, Some(Content::Text(t)) if t.text == value));
                if same {
                    return;
                }
                // typing is one undo step, kept until the box is left
                if this.text_before.is_none() {
                    this.text_before = Some(p.sequence.clone());
                }
                if let Some(Content::Text(t)) = p.sequence.clip_mut(id).and_then(|c| c.content.as_mut()) {
                    t.text = value;
                }
                this.settle(cx);
            }
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.text_typed(),
            _ => {}
        });
        self._subscriptions.push(sub);
        self.text_edit = Some((id, input));
    }

    /// Typing ended: what it changed is one undo step.
    pub(super) fn text_typed(&mut self) {
        if let (Some(before), Some(p)) = (self.text_before.take(), &self.project)
            && before != p.sequence
        {
            self.undo.push(before);
            self.redo.clear();
        }
    }

    // ------------------------------------------------------------ the inspector

    /// Changes the selected made picture's content.
    fn set_content(&mut self, clip: Id, f: impl FnOnce(&mut Content), cx: &mut Context<Self>) {
        self.change(
            |s| {
                if let Some(c) = s.clip_mut(clip).and_then(|c| c.content.as_mut()) {
                    f(c);
                }
            },
            cx,
        );
    }

    fn swatches(&self, id: &'static str, current: Option<u32>, none: bool, on: impl Fn(&mut Self, Option<u32>, &mut Context<Self>) + Copy + 'static, cx: &mut Context<Self>) -> Div {
        let mut row = div().flex().flex_wrap().gap_1p5();
        let mut options: Vec<Option<u32>> = PALETTE.iter().map(|c| Some(*c)).collect();
        if none {
            options.insert(0, None);
        }
        for (n, c) in options.into_iter().enumerate() {
            let sel = c == current;
            row = row.child(
                div()
                    .id((id, n))
                    .size(px(22.))
                    .rounded(px(5.))
                    .border_2()
                    .border_color(if sel { tok::accent_primary() } else { tok::border() })
                    .cursor_pointer()
                    .map(|d| match c {
                        Some(c) => d.bg(hex(c)),
                        None => d.flex().items_center().justify_center().child(ui::icon("x").size(px(12.)).text_color(tok::text_tertiary())),
                    })
                    .on_click(cx.listener(move |this, _, _, cx| on(this, c, cx))),
            );
        }
        row
    }

    /// The text or shape tab of a made picture.
    pub(super) fn content_section(&self, c: &Clip, mut body: Div, cx: &mut Context<Self>) -> Div {
        let cid = c.id;
        let k = self.seq().map_or(1.0, |s| s.size.1 as f32 / 1080.0);
        let on_captions = self.seq().and_then(|s| s.find(cid)).is_some_and(|(t, _)| self.seq().is_some_and(|s| s.tracks[t].captions));
        match &c.content {
            Some(Content::Text(t)) => {
                // a caption's look goes to all of them
                let set = move |this: &mut Self, f: &dyn Fn(&mut TextStyle), cx: &mut Context<Self>| {
                    if on_captions {
                        this.restyle_captions(f, cx);
                    } else {
                        this.set_content(cid, |c| if let Content::Text(t) = c { f(t) }, cx);
                    }
                };
                if let Some((_, input)) = self.text_edit.as_ref().filter(|(id, _)| *id == cid) {
                    body = body.child(label("WORDS")).child(TextInput::new(input).small());
                }
                body = body
                    .when(on_captions, |d| d.child(hint("A caption: its look changes every caption.")))
                    .child(label("LOOK"))
                    .child(self.stepper("ed-tsize", "Size", format!("{:.0}", t.size / k), move |this, d, cx| set(this, &|t| t.size = (t.size + d * 4.0 * k).clamp(8.0 * k, 400.0 * k), cx), cx))
                    .child(self.stepper("ed-twidth", "Line width", format!("{:.0}%", t.width / self.seq().map_or(1920.0, |s| s.size.0 as f32) * 100.0), move |this, d, cx| {
                        let w = this.seq().map_or(1920.0, |s| s.size.0 as f32);
                        set(this, &|t| t.width = (t.width + d * w * 0.05).clamp(w * 0.1, w), cx)
                    }, cx))
                    .child(self.swatches("ed-tcol", Some(t.color), false, move |this, c, cx| set(this, &|t| t.color = c.unwrap_or(0xFFFFFF), cx), cx))
                    .child(self.chips("ed-tbold", &[("Regular", false), ("Bold", true)], t.bold, move |this, v, cx| set(this, &|t| t.bold = v, cx), cx))
                    .child(self.chips("ed-talign", &[("Left", 0u8), ("Centre", 1), ("Right", 2)], t.align, move |this, v, cx| set(this, &|t| t.align = v, cx), cx))
                    .child(label("BOX BEHIND"))
                    .child(self.swatches("ed-tbg", t.background.map(|b| b.0), true, move |this, c, cx| set(this, &|t| t.background = c.map(|c| (c, t.background.map_or(0.75, |b| b.1))), cx), cx))
                    .child(label("OUTLINE"))
                    .child(self.swatches("ed-tline", t.outline, true, move |this, c, cx| set(this, &|t| t.outline = c, cx), cx));
            }
            Some(Content::Shape(s)) => {
                let set = move |this: &mut Self, f: &dyn Fn(&mut ShapeStyle), cx: &mut Context<Self>| this.set_content(cid, |c| if let Content::Shape(s) = c { f(s) }, cx);
                body = body
                    .child(label("SHAPE"))
                    .child(self.chips("ed-skind", &[("Rectangle", ShapeKind::Rectangle), ("Ellipse", ShapeKind::Ellipse), ("Arrow", ShapeKind::Arrow)], s.shape, move |this, v, cx| set(this, &|s| s.shape = v, cx), cx))
                    .child(self.stepper("ed-sw", "Width", format!("{:.0}", s.width / k), move |this, d, cx| set(this, &|s| s.width = (s.width + d * 20.0 * k).max(8.0), cx), cx))
                    .child(self.stepper("ed-sh", "Height", format!("{:.0}", s.height / k), move |this, d, cx| set(this, &|s| s.height = (s.height + d * 20.0 * k).max(8.0), cx), cx))
                    .when(s.shape == ShapeKind::Rectangle, |d| d.child(self.stepper("ed-sr", "Corners", format!("{:.0}", s.radius / k), move |this, d, cx| set(this, &|s| s.radius = (s.radius + d * 8.0 * k).clamp(0.0, s.width.min(s.height) / 2.0), cx), cx)))
                    .child(label("FILL"))
                    .child(self.swatches("ed-sfill", s.fill.map(|f| f.0), true, move |this, c, cx| set(this, &|s| s.fill = c.map(|c| (c, s.fill.map_or(1.0, |f| f.1))), cx), cx))
                    .child(label("LINE"))
                    .child(self.swatches("ed-sline", s.stroke.map(|f| f.0), true, move |this, c, cx| set(this, &|s| s.stroke = c.map(|c| (c, s.stroke.map_or(10.0 * k, |f| f.1))), cx), cx))
                    .when(s.stroke.is_some(), |d| {
                        d.child(self.stepper("ed-slw", "Line width", format!("{:.0}", s.stroke.map_or(0.0, |f| f.1) / k), move |this, d, cx| {
                            set(this, &|s| {
                                if let Some(f) = &mut s.stroke {
                                    f.1 = (f.1 + d * 2.0 * k).clamp(k, 80.0 * k);
                                }
                            }, cx)
                        }, cx))
                    });
            }
            None => {}
        }
        body
    }

    /// Position, scale, rotation and opacity at the playhead, and the clip's keyframes.
    pub(super) fn transform_section(&self, c: &Clip, body: Div, cx: &mut Context<Self>) -> Div {
        use std::rc::Rc;
        let cid = c.id;
        let inside = c.start_ms <= self.playhead && self.playhead < c.end_ms();
        body.child(self.keyed_title(c, "TRANSFORM", &lightair_render::keys::TRANSFORM, cx))
            .child(self.keyed(c, "scale", "Scale", Rc::new(|v| format!("{:.0}%", v * 100.0)), 0.05, (0.05, 10.0), cx))
            .child(self.keyed(c, "x", "Position X", Rc::new(|v| format!("{v:.0}")), 10.0, (-20_000.0, 20_000.0), cx))
            .child(self.keyed(c, "y", "Position Y", Rc::new(|v| format!("{v:.0}")), 10.0, (-20_000.0, 20_000.0), cx))
            .child(self.keyed(c, "rotation", "Rotate", Rc::new(|v| format!("{v:.0}°")), 5.0, (-3_600.0, 3_600.0), cx))
            .child(self.keyed(c, "opacity", "Opacity", Rc::new(|v| format!("{:.0}%", v * 100.0)), 0.1, (0.0, 1.0), cx))
            .child(Button::new("ed-reset").label("Reset").icon(ui::icon("rotate-ccw")).xsmall().ghost().on_click(cx.listener(move |this, _, _, cx| {
                this.change(
                    |s| {
                        if let Some(c) = s.clip_mut(cid) {
                            c.transform = Transform::default();
                            c.keyframes.clear();
                            for p in lightair_render::keys::TRANSFORM {
                                c.anim.remove(p);
                            }
                        }
                    },
                    cx,
                )
            })))
            .child(hint(if inside {
                "◇ keys a value at the playhead; with keys, a change sets the one there. The ◇ by the title keys them all."
            } else {
                "Move the playhead over the clip to key its values."
            }))
            .map(|b| self.fades_section(c, b, cx))
    }


    pub(super) fn filter_section(&self, c: &Clip, body: Div, cx: &mut Context<Self>) -> Div {
        let cid = c.id;
        let f = c.filter;
        // the filter is chosen in the media panel's Filters; here, how it's set
        let name = match Filter::PRESETS.iter().find(|p| p.1 == f) {
            Some((n, _)) => (*n).to_string(),
            None => "Custom".to_string(),
        };
        use std::rc::Rc;
        body.child(label("FILTER"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().text_size(px(12.)).child(name))
                    .when(!f.is_none(), |d| {
                        d.child(Button::new("ed-fclear").label("Reset").xsmall().ghost().on_click(cx.listener(move |this, _, _, cx| {
                            this.change(
                                |s| {
                                    if let Some(c) = s.clip_mut(cid) {
                                        c.filter = Filter::default();
                                        for p in lightair_render::keys::FILTER {
                                            c.anim.remove(p);
                                        }
                                    }
                                },
                                cx,
                            )
                        })))
                    }),
            )
            .child(hint("Choose one in the Filters tab on the left."))
            .child(self.keyed_title(c, "ADJUST", &lightair_render::keys::FILTER, cx))
            .child(self.keyed(c, "brightness", "Brightness", Rc::new(|v| format!("{:+.0}", v * 100.0)), 0.05, (-1.0, 1.0), cx))
            .child(self.keyed(c, "contrast", "Contrast", Rc::new(|v| format!("{:+.0}", (v - 1.0) * 100.0)), 0.05, (0.0, 2.0), cx))
            .child(self.keyed(c, "saturation", "Saturation", Rc::new(|v| format!("{:+.0}", (v - 1.0) * 100.0)), 0.1, (0.0, 2.0), cx))
            .child(self.keyed(c, "temperature", "Warmth", Rc::new(|v| format!("{:+.0}", v * 100.0)), 0.1, (-1.0, 1.0), cx))
            .child(self.keyed(c, "vignette", "Vignette", Rc::new(|v| format!("{:.0}", v * 100.0)), 0.1, (0.0, 1.0), cx))
            .child(self.keyed(c, "blur", "Blur", Rc::new(|v| format!("{:.0}", v * 100.0)), 0.1, (0.0, 1.0), cx))
    }

    pub(super) fn transition_section(&self, c: &Clip, body: Div, cx: &mut Context<Self>) -> Div {
        let cid = c.id;
        let before = self.seq().and_then(|s| s.before(cid)).cloned();
        let Some(prev) = before else {
            return body.child(label("TRANSITION")).child(hint("A transition plays across a cut: put this clip right after another on its track."));
        };
        let longest = (prev.len_ms().min(c.len_ms())).max(100);
        let kind = c.transition.map(|t| t.kind);
        let dur = c.transition.map_or(500, |t| t.duration_ms);
        let set = move |this: &mut Self, kind: Option<TransitionKind>, dur: u64, cx: &mut Context<Self>| {
            this.change(|s| if let Some(c) = s.clip_mut(cid) { c.transition = kind.map(|kind| Transition { kind, duration_ms: dur.min(longest) }) }, cx)
        };
        body.child(label("TRANSITION FROM THE CLIP BEFORE"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().text_size(px(12.)).child(kind.map_or("None", TransitionKind::name)))
                    .when(kind.is_some(), |d| d.child(Button::new("ed-tnone").label("Remove").icon(ui::icon("x")).xsmall().ghost().on_click(cx.listener(move |this, _, _, cx| set(this, None, dur, cx))))),
            )
            .child(hint("Choose one in the Transitions tab on the left."))
            .when(kind.is_some(), |d| {
                d.child(label("LENGTH")).child(self.chips(
                    "ed-td",
                    &[("0.3 s", 300u64), ("0.5 s", 500), ("1 s", 1_000), ("1.5 s", 1_500), ("2 s", 2_000)],
                    dur,
                    move |this, v, cx| set(this, kind, v, cx),
                    cx,
                ))
            })
            .when(kind.is_some(), |d| {
                d.child(Button::new("ed-tall").label("Use at every cut").icon(ui::icon("blend")).xsmall().ghost().on_click(cx.listener(move |this, _, _, cx| {
                    this.change(
                        |s| {
                            let cuts: Vec<Id> = s.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.captions).flat_map(|t| t.clips.iter().map(|c| c.id)).filter(|c| s.before(*c).is_some()).collect();
                            for id in cuts {
                                let len = s.before(id).map_or(0, Clip::len_ms).min(s.clip(id).map_or(0, Clip::len_ms)).max(100);
                                if let Some(c) = s.clip_mut(id) {
                                    c.transition = kind.map(|kind| Transition { kind, duration_ms: dur.min(len) });
                                }
                            }
                        },
                        cx,
                    )
                })))
            })
    }
}

pub(super) fn heading(t: &'static str) -> Div {
    div().pt_2().pb_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text_tertiary()).child(t)
}

/// A section's title: a line above it, its name in sentence case.
pub(super) fn label(t: &'static str) -> Div {
    let mut name = t.to_lowercase();
    if let Some(first) = name.get(0..1) {
        name = first.to_uppercase() + &name[1..];
    }
    div().mt_2().pt_3().border_t_1().border_color(tok::border()).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text()).child(name)
}

pub(super) fn hint(t: impl Into<SharedString>) -> Div {
    div().text_size(px(11.5)).line_height(px(16.)).text_color(tok::text_tertiary()).child(t.into())
}

impl Editor {
    /// For the screenshot hooks: a title (moving), a shape, a filter and a crossfade on the open
    /// project; `second` shows the second clip's transition and the captions tab instead.
    pub fn hook_looks(&mut self, second: bool, cx: &mut Context<Self>) {
        let Some(seq) = self.seq() else { return };
        let size = seq.size;
        let videos: Vec<Id> = seq.tracks.iter().filter(|t| t.kind == TrackKind::Video).flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
        let (_, mut title, place) = text_presets(size).swap_remove(0);
        title.text = "Lightair".into();
        let (_, shape) = shape_presets(size).swap_remove(1);
        let mut made = None;
        self.change(
            |s| {
                let t = s.place_content(Content::Text(title), 1_000, 6_000);
                if let Some(c) = s.clip_mut(t) {
                    c.transform = place;
                    c.keyframes = vec![
                        Keyframe { at_ms: 0, transform: Transform { y: -200.0, opacity: 0.0, ..place } },
                        Keyframe { at_ms: 1_000, transform: place },
                    ];
                }
                let sh = s.place_content(Content::Shape(shape), 1_000, 6_000);
                if let Some(c) = s.clip_mut(sh) {
                    c.transform = Transform { x: 0.0, y: 330.0, scale: 0.5, opacity: 0.8, ..Transform::default() };
                }
                if let Some(first) = videos.first().and_then(|id| s.clip_mut(*id)) {
                    first.filter = Filter::PRESETS[2].1;
                }
                if let Some(c) = videos.get(1).and_then(|id| s.clip_mut(*id)) {
                    c.transition = Some(Transition { kind: TransitionKind::Crossfade, duration_ms: 1_000 });
                }
                made = Some(t);
            },
            cx,
        );
        if second {
            self.pick_only(videos.get(1).copied());
            self.tab = InspectorTab::Transition;
            self.left = LeftTab::Captions;
            self.seek(11_800, cx);
        } else {
            self.pick_only(made);
            self.tab = InspectorTab::Text;
            self.left = LeftTab::Text;
            self.seek(2_500, cx);
        }
    }
}
