//! The project's own settings (its size and frame rate: any), and exporting it: a panel with
//! ready-made settings for where it's going and every choice (the format, its codecs and
//! quality, the size and rate, the sound, the stretch of time, where it goes), then its
//! progress and what came of it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input as TextInput, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_render::exporter::{AudioCodec, Container, ExportSettings, Rate, VideoCodec, PRESETS};
use lightair_render::seqrender::SequenceRenderer;
use lightair_store::recordings::{File as ItemFile, Index, Item, Kind};

use super::looks::{hint, label};
use super::{Done, Editor, fps_label};
use crate::ui::{self, tok};

/// Frame sizes: (name, size).
const SIZES: &[(&str, (u32, u32))] = &[
    ("720p HD", (1280, 720)),
    ("1080p Full HD", (1920, 1080)),
    ("1440p QHD", (2560, 1440)),
    ("4K UHD", (3840, 2160)),
    ("DCI 4K", (4096, 2160)),
    ("Vertical 1080p", (1080, 1920)),
    ("Vertical 4K", (2160, 3840)),
    ("Square", (1080, 1080)),
    ("Portrait 4:5", (1080, 1350)),
    ("Ultrawide 21:9", (2560, 1080)),
];

/// Frame rates: (name, rate).
const RATES: &[(&str, f64)] = &[("23.976", 23.976), ("24", 24.0), ("25", 25.0), ("29.97", 29.97), ("30", 30.0), ("50", 50.0), ("59.94", 59.94), ("60", 60.0)];

/// The formats offered first: (name, the preset that makes it).
const FORMATS: &[(&str, usize)] = &[
    ("MP4 · H.264", 0),
    ("MP4 · H.264, high bitrate (YouTube)", 1),
    ("MP4 · HEVC, smaller file", 3),
    ("MOV · ProRes 422 HQ (for editing)", 4),
    ("MOV · DNxHR HQ (for editing)", 5),
    ("WebM · VP9 (for the web)", 6),
    ("MKV · lossless (archive)", 7),
    ("MOV · ProRes 4444 (transparent)", 8),
    ("MOV · HEVC (transparent, Mac)", 9),
    ("WebM · VP9 (transparent)", 10),
    ("GIF (animated)", 11),
    ("PNG frames", 12),
    ("WAV (sound only)", 13),
    ("MP3 (sound only)", 14),
];

/// Whether `s` is `preset`'s kind of file (its container and codecs: the rest may differ).
fn same_format(preset: &ExportSettings, s: &ExportSettings) -> bool {
    preset.container == s.container
        && preset.keeps_alpha() == s.keeps_alpha()
        && (!s.container.has_video() || (preset.video == s.video && (preset.profile == s.profile || preset.video.profiles().is_empty())))
        && (preset.audio.is_some() == s.audio.is_some() || !s.container.has_video())
        && (preset.video != VideoCodec::H264 || preset.rate == s.rate || matches!((preset.rate, s.rate), (Rate::Quality(70), Rate::Quality(q)) if q != 85))
}

/// What's under Advanced, in short (it's folded).
fn advanced_summary(s: &ExportSettings) -> String {
    let mut parts = Vec::new();
    if s.container.has_video() && s.video.hardware_possible() {
        parts.push(if s.hardware { "Hardware" } else { "Software" }.to_string());
    }
    match s.audio.filter(|_| s.container.has_audio()) {
        Some(a) if a.has_bitrate() => parts.push(format!("{} {} kbps", a.name(), s.audio_kbps)),
        Some(a) => parts.push(a.name().to_string()),
        None if s.container.has_audio() => parts.push("No sound".into()),
        None => {}
    }
    parts.join("  ·  ")
}

/// The text fields of the project's settings and the export panel.
const FIELDS: &[&str] = &["proj-w", "proj-h", "proj-fps", "exp-name", "exp-w", "exp-h", "exp-kbps"];

/// The stretch of time exported.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Range {
    All,
    FromPlayhead,
    Markers,
}

/// The export panel, open.
pub(super) struct ExportPanel {
    settings: ExportSettings,
    folder: PathBuf,
    range: Range,
    /// The size typed (its fields show).
    custom_size: bool,
    /// Its name field wants the project's name.
    fresh: bool,
    /// Advanced settings unfolded.
    advanced: bool,
    started: Option<Instant>,
    stop: Option<Arc<AtomicBool>>,
    result: Option<Result<PathBuf, String>>,
}

/// The text fields, made once (they need the window), and keystrokes counted so only the last
/// one's pause applies what's typed.
#[derive(Default)]
pub(super) struct Fields {
    inputs: HashMap<&'static str, Entity<InputState>>,
    typed: HashMap<&'static str, u64>,
    _subs: Vec<Subscription>,
}

/// "16:9" for 1920×1080.
fn aspect((w, h): (u32, u32)) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(w, h).max(1);
    let (a, b) = (w / g, h / g);
    // ratios that don't reduce well, as a number
    if a > 64 { format!("{:.2}:1", w as f32 / h.max(1) as f32) } else { format!("{a}:{b}") }
}

fn even(v: u32) -> u32 {
    v.clamp(16, 8192) & !1
}

/// "1:23" or "1:02:03".
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1000.0) } else if mb >= 10.0 { format!("{mb:.0} MB") } else { format!("{mb:.1} MB") }
}

impl Editor {
    /// Keeps the text fields made and showing what they stand for (unless being typed in).
    pub(super) fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for &key in FIELDS {
            if self.fields.inputs.contains_key(key) {
                continue;
            }
            let input = cx.new(|cx| InputState::new(window, cx));
            let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, _, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    let text = input.read(cx).value().to_string();
                    this.commit_field(key, &text, cx);
                }
                InputEvent::Change => {
                    let n = this.fields.typed.entry(key).or_default();
                    *n += 1;
                    let (n, input) = (*n, input.clone());
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_millis(700)).await;
                        let _ = this.update(cx, |this, cx| {
                            if this.fields.typed.get(key) == Some(&n) {
                                let text = input.read(cx).value().to_string();
                                this.commit_field(key, &text, cx);
                            }
                        });
                    })
                    .detach();
                }
                _ => {}
            });
            self.fields.inputs.insert(key, input);
            self.fields._subs.push(sub);
        }
        let seq = self.seq().cloned();
        let fresh = self.export_panel.as_ref().is_some_and(|p| p.fresh);
        for &key in FIELDS {
            let want = match (key, &seq, &self.export_panel) {
                ("proj-w", Some(s), _) => s.size.0.to_string(),
                ("proj-h", Some(s), _) => s.size.1.to_string(),
                ("proj-fps", Some(s), _) => fps_label(s.fps),
                ("exp-name", _, Some(_)) if fresh => self.project.as_ref().map_or("Export".into(), |p| p.name.clone()),
                ("exp-w", Some(s), Some(p)) => p.settings.size_for(s.size).0.to_string(),
                ("exp-h", Some(s), Some(p)) => p.settings.size_for(s.size).1.to_string(),
                ("exp-kbps", _, Some(p)) => match p.settings.rate {
                    Rate::Bitrate(k) => fps_label(k as f64 / 1000.0),
                    Rate::Quality(_) => "16".into(),
                },
                _ => continue,
            };
            let input = self.fields.inputs[key].clone();
            let focused = input.read(cx).focus_handle(cx).is_focused(window);
            if (!focused || key == "exp-name") && input.read(cx).value() != want.as_str() && (key != "exp-name" || fresh) {
                input.update(cx, |s, cx| s.set_value(want, window, cx));
            }
        }
        if let Some(p) = &mut self.export_panel {
            p.fresh = false;
        }
    }

    /// A field's value, typed: applied.
    fn commit_field(&mut self, key: &'static str, text: &str, cx: &mut Context<Self>) {
        let number = text.trim().trim_end_matches("fps").trim_end_matches("Mbps").trim().replace(',', ".");
        match key {
            "proj-w" | "proj-h" => {
                let Ok(v) = number.parse::<f64>() else { return };
                let v = even(v.round() as u32);
                self.change(|s| if key == "proj-w" { s.size.0 = v } else { s.size.1 = v }, cx);
            }
            "proj-fps" => {
                let Ok(v) = number.parse::<f64>() else { return };
                if (1.0..=240.0).contains(&v) {
                    self.change(|s| s.fps = v, cx);
                }
            }
            "exp-w" | "exp-h" => {
                let Ok(v) = number.parse::<f64>() else { return };
                let project = self.seq().map_or((1920, 1080), |s| s.size);
                if let Some(p) = &mut self.export_panel {
                    let (mut w, mut h) = p.settings.size_for(project);
                    if key == "exp-w" { w = even(v.round() as u32) } else { h = even(v.round() as u32) }
                    p.settings.size = Some((w, h));
                }
            }
            "exp-kbps" => {
                let Ok(v) = number.parse::<f64>() else { return };
                if let Some(p) = &mut self.export_panel
                    && v > 0.0
                {
                    p.settings.rate = Rate::Bitrate((v * 1000.0).round().clamp(100.0, 500_000.0) as u32);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn text_field(&self, key: &str) -> Option<Entity<InputState>> {
        self.fields.inputs.get(key).cloned()
    }

    /// The project's own settings, in the Inspector when no clip is selected.
    pub(super) fn project_section(&self, body: Div, cx: &mut Context<Self>) -> Div {
        let Some(seq) = self.seq() else { return body };
        let (size, fps) = (seq.size, seq.fps);
        let me = cx.entity().downgrade();
        let name = SIZES.iter().find(|(_, s)| *s == size).map_or("Custom".to_string(), |(n, _)| n.to_string());
        let presets = Button::new("ed-size-preset").label(name).small().outline().w_full().dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
            for (i, &(n, s)) in SIZES.iter().enumerate() {
                if i == 5 || i == 7 {
                    menu = menu.separator();
                }
                let me = me.clone();
                menu = menu.item(PopupMenuItem::new(format!("{n}  ·  {}×{}", s.0, s.1)).checked(s == size).on_click(move |_, _, cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |this, cx| this.change(|q| q.size = s, cx));
                    }
                }));
            }
            menu
        });
        let input = |key: &str| self.text_field(key).map(|i| div().flex_1().min_w_0().child(TextInput::new(&i).small()));
        let dims = div()
            .flex()
            .items_center()
            .gap_1p5()
            .children(input("proj-w"))
            .child(div().text_size(px(12.)).text_color(tok::text_tertiary()).child("×"))
            .children(input("proj-h"))
            .child(
                ui::icon_button("ed-size-rotate", "rotate-cw", tok::text_secondary())
                    .tooltip(ui::tip("Turn it: landscape ↔ portrait"))
                    .on_click(cx.listener(|this, _, _, cx| this.change(|s| s.size = (s.size.1, s.size.0), cx))),
            );
        let rate = RATES.iter().find(|(_, r)| (r - fps).abs() < 0.001).map_or(-1.0, |(_, r)| *r);
        body.child(label("SIZE"))
            .child(presets)
            .child(dims)
            .child(hint(format!("{}×{} · {}", size.0, size.1, aspect(size))))
            .child(label("FRAME RATE"))
            // in two even rows: film and PAL rates, then NTSC and high
            .child(self.segmented("ed-fps".into(), &RATES[..4], rate, |this, v, cx| this.change(|s| s.fps = v, cx), cx))
            .child(self.segmented("ed-fps2".into(), &RATES[4..], rate, |this, v, cx| this.change(|s| s.fps = v, cx), cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child("Any other"))
                    .children(self.text_field("proj-fps").map(|i| div().w(px(72.)).child(TextInput::new(&i).small())))
                    .child(div().text_size(px(12.)).text_color(tok::text_tertiary()).child("fps")),
            )
    }

    /// Opens the export panel (as the project was last exported, or the standard settings).
    pub(super) fn open_export(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        if p.sequence.duration_ms() == 0 {
            self.note = Some("Put something on the timeline first.".into());
            cx.notify();
            return;
        }
        let settings = p.sequence.export.unwrap_or_default().fixed();
        self.stop(cx);
        self.export_panel = Some(ExportPanel {
            settings,
            folder: self.studio.read(cx).recording_folder(),
            range: Range::All,
            custom_size: settings.size.is_some_and(|s| !SIZES.iter().any(|(_, z)| *z == s)),
            fresh: true,
            advanced: false,
            started: None,
            stop: None,
            result: None,
        });
        cx.notify();
    }

    fn close_export(&mut self, cx: &mut Context<Self>) {
        if self.export_panel.as_ref().is_some_and(|p| p.started.is_some()) {
            return;
        }
        self.export_panel = None;
        cx.notify();
    }

    fn set_export(&mut self, f: impl FnOnce(&mut ExportSettings), cx: &mut Context<Self>) {
        if let Some(p) = &mut self.export_panel {
            f(&mut p.settings);
            p.settings = p.settings.fixed();
        }
        cx.notify();
    }

    /// `start..end` of the result the panel's range is.
    fn export_range(&self) -> (u64, u64) {
        let Some(seq) = self.seq() else { return (0, 0) };
        let end = seq.duration_ms();
        match self.export_panel.as_ref().map_or(Range::All, |p| p.range) {
            Range::All => (0, end),
            Range::FromPlayhead => (self.playhead.min(end), end),
            Range::Markers => match (seq.markers.first(), seq.markers.last()) {
                (Some(&a), Some(&b)) if b > a => (a, b.min(end)),
                _ => (0, end),
            },
        }
    }

    /// Exports as the panel says, in the background.
    fn run_export(&mut self, cx: &mut Context<Self>) {
        let (Some(p), Some(ws)) = (&self.project, self.workspace.clone()) else { return };
        let Some(panel) = &self.export_panel else { return };
        if panel.started.is_some() {
            return;
        }
        let settings = panel.settings;
        let typed = self.text_field("exp-name").map(|i| i.read(cx).value().trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| p.name.clone());
        let base = lightair_core::file_safe(&typed);
        let ext = settings.container.ext();
        let folder = panel.folder.clone();
        let taken = |path: &std::path::Path| if settings.container == Container::Png { path.with_extension("").exists() } else { path.exists() };
        let out = (1..)
            .map(|n| if n == 1 { folder.join(format!("{base}.{ext}")) } else { folder.join(format!("{base} {n}.{ext}")) })
            .find(|p| !taken(p))
            .unwrap_or_else(|| folder.join(format!("{base}.{ext}")));
        let range = self.export_range();
        let (seq, tx, title) = (p.sequence.clone(), self.tx.clone(), typed);
        // remembered with the project, for next time
        self.change(|s| s.export = Some(settings), cx);
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(panel) = &mut self.export_panel {
            panel.started = Some(Instant::now());
            panel.stop = Some(stop.clone());
            panel.result = None;
        }
        self.exporting = Some(0.0);
        std::thread::spawn(move || {
            let result = (|| {
                let mut r = SequenceRenderer::new(&seq).map_err(|e| e.to_string())?;
                let progress = tx.clone();
                let written = r
                    .export_with(&out, &settings, range, |f| {
                        let _ = progress.send(Done::Progress(f));
                        !stop.load(Ordering::Relaxed)
                    })
                    .map_err(|e| e.to_string())?;
                let mut item = Item::new(Kind::Export, title, vec![ItemFile::new(written.clone())], chrono::Utc::now().timestamp_millis());
                item.duration_ms = Some(range.1.saturating_sub(range.0));
                let _ = Index::add_to(&ws, item);
                Ok(written)
            })();
            let _ = tx.send(Done::Exported(result));
        });
        cx.notify();
    }

    /// The export ended (`Done::Exported`): the panel says how.
    pub(super) fn export_ended(&mut self, result: Result<PathBuf, String>) {
        match &mut self.export_panel {
            Some(p) => {
                p.started = None;
                p.stop = None;
                p.result = Some(result);
            }
            None => {
                self.note = Some(match result {
                    Ok(p) => format!("Exported {} into the Library", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).into(),
                    Err(e) => format!("The export stopped: {e}").into(),
                });
            }
        }
    }

    /// A choice as a menu button: its current option's name, the options in its menu.
    #[allow(clippy::too_many_arguments)]
    fn menu_button<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        options: Vec<(String, T)>,
        current: T,
        on: fn(&mut Editor, T, &mut Context<Editor>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.menu_button_named(id, None, options, current, on, cx)
    }

    /// `menu_button`, showing `name` (when none of the options is current).
    fn menu_button_named<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        name: Option<String>,
        options: Vec<(String, T)>,
        current: T,
        on: fn(&mut Editor, T, &mut Context<Editor>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let me = cx.entity().downgrade();
        let shown = name.unwrap_or_else(|| options.iter().find(|(_, v)| *v == current).map_or("Custom".to_string(), |(n, _)| n.clone()));
        Button::new(id).label(shown).small().outline().dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
            for (name, v) in options.iter().cloned() {
                let me = me.clone();
                menu = menu.item(PopupMenuItem::new(name).checked(v == current).on_click(move |_, _, cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |this, cx| on(this, v, cx));
                    }
                }));
            }
            menu
        })
        .into_any_element()
    }

    /// The export panel, when open, over the Editor: the name and where it goes, then the four
    /// choices that matter (format, size, rate, quality), the rest folded under Advanced.
    pub(super) fn export_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.export_panel.as_ref()?;
        let seq = self.seq()?;
        let s = panel.settings;
        let (project, project_fps) = (seq.size, seq.fps);
        let busy = panel.started.is_some();
        let me = cx.entity().downgrade();
        let row = |name: &'static str, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .min_h(px(30.))
                .child(div().flex_none().w(px(92.)).text_size(px(12.)).text_color(tok::text_secondary()).child(name))
                .child(div().flex().flex_1().min_w_0().child(control))
        };
        let mut body = div().id("exp-body").flex().flex_col().gap_2().px_5().py_4().overflow_y_scroll().flex_1().min_h_0();

        // the name, and where it goes
        let ext = if s.container == Container::Png { "/".to_string() } else { format!(".{}", s.container.ext()) };
        let home = std::env::home_dir().unwrap_or_default();
        let folder = panel.folder.strip_prefix(&home).map_or(panel.folder.display().to_string(), |p| format!("~/{}", p.display()));
        body = body
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .children(self.text_field("exp-name").map(|i| div().flex_1().child(TextInput::new(&i))))
                    .child(div().text_size(px(13.)).font_family(ui::MONO).text_color(tok::text_tertiary()).child(ext)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pb_2()
                    .text_size(px(11.5))
                    .text_color(tok::text_tertiary())
                    .child(ui::icon("folder").size(px(12.)))
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(folder))
                    .child(
                        div()
                            .id("exp-folder")
                            .flex_none()
                            .text_color(tok::text_secondary())
                            .cursor_pointer()
                            .hover(|d| d.text_color(tok::text()))
                            .child("Change…")
                            .on_click(cx.listener(|_, _, window, cx| {
                                let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Export here".into()) });
                                cx.spawn_in(window, async move |this, cx| {
                                    let Ok(Ok(Some(paths))) = paths.await else { return };
                                    let Some(dir) = paths.into_iter().next() else { return };
                                    let _ = this.update(cx, |this, cx| {
                                        if let Some(p) = &mut this.export_panel {
                                            p.folder = dir;
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                            })),
                    ),
            );

        // the format: what kind of file, by name (a preset's codecs, keeping the size and rate)
        let formats: Vec<(String, usize)> = FORMATS.iter().map(|(n, i)| (n.to_string(), *i)).collect();
        let current = FORMATS.iter().map(|(_, i)| *i).find(|i| same_format(&PRESETS[*i].2, &s)).unwrap_or(usize::MAX);
        let format_name = if current == usize::MAX {
            format!("{} · {}{}", s.container.name(), s.video.name(), if s.keeps_alpha() { " (transparent)" } else { "" })
        } else {
            String::new()
        };
        let format = self.menu_button_named("exp-format", (!format_name.is_empty()).then_some(format_name), formats, current, |this, i, cx| {
            if let Some(p) = &mut this.export_panel {
                let preset = PRESETS[i].2;
                p.settings = ExportSettings { size: p.settings.size, fps: preset.fps.or(p.settings.fps).filter(|_| preset.video != VideoCodec::Gif || preset.fps.is_some()), ..preset };
                if preset.video == VideoCodec::Gif {
                    p.settings.fps = preset.fps;
                }
                p.result = None;
            }
            cx.notify();
        }, cx);
        body = body.child(row("Format", format));

        if s.container.has_video() {
            // the size: the project's, a common one, or typed
            let mut sizes: Vec<(String, Option<(u32, u32)>)> = vec![(format!("{}×{}  (project)", project.0, project.1), None)];
            sizes.extend(SIZES.iter().filter(|(_, z)| *z != project).map(|(n, z)| (format!("{}×{}  {n}", z.0, z.1), Some(*z))));
            let chosen = s.size.filter(|z| *z != project);
            let typed = panel.custom_size || chosen.is_some_and(|z| !SIZES.iter().any(|(_, x)| *x == z));
            sizes.push(("Custom…".into(), Some((0, 0))));
            let out = s.size_for(project);
            let size = self.menu_button_named("exp-size", typed.then(|| format!("{}×{}  custom", out.0, out.1)), sizes, if typed { Some((0, 0)) } else { chosen }, |this, v, cx| {
                let project = this.seq().map_or((1920, 1080), |s| s.size);
                if let Some(p) = &mut this.export_panel {
                    p.custom_size = v == Some((0, 0));
                    p.settings.size = if p.custom_size { Some(p.settings.size_for(project)) } else { v };
                }
                cx.notify();
            }, cx);
            let mut size_cell = div().flex().items_center().gap_2().child(size);
            if typed {
                size_cell = size_cell
                    .children(self.text_field("exp-w").map(|i| div().w(px(64.)).child(TextInput::new(&i).small())))
                    .child(div().text_size(px(12.)).text_color(tok::text_tertiary()).child("×"))
                    .children(self.text_field("exp-h").map(|i| div().w(px(64.)).child(TextInput::new(&i).small())));
            }
            body = body.child(row("Resolution", size_cell.into_any_element()));
            if aspect(out) != aspect(project) {
                body = body.child(row("", hint(format!("Fitted inside, with {} bars ({} into {})", if s.keeps_alpha() { "see-through" } else { "black" }, aspect(project), aspect(out))).flex_1().min_w_0().into_any_element()));
            }

            // the rate
            let mut rates: Vec<(String, Option<f64>)> = vec![(format!("{} fps  (project)", fps_label(project_fps)), None)];
            let list: &[f64] = if s.video == VideoCodec::Gif { &[10.0, 15.0, 20.0, 24.0, 30.0] } else { &[23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 59.94, 60.0] };
            rates.extend(list.iter().filter(|r| (**r - project_fps).abs() > 0.001 || s.video == VideoCodec::Gif).map(|r| (format!("{} fps", fps_label(*r)), Some(*r))));
            let fps = s.fps.filter(|f| (f - project_fps).abs() > 0.001 || s.video == VideoCodec::Gif);
            body = body.child(row("Frame rate", self.menu_button("exp-fps", rates, fps, |this, v, cx| this.set_export(|x| x.fps = v, cx), cx).into_any_element()));

            // the quality (codecs that have a choice)
            if s.video.rate_controlled() {
                let quality = match s.rate {
                    Rate::Quality(q) => self.segmented("exp-quality".into(), &[("Smaller", 45u8), ("Balanced", 70), ("High", 85), ("Maximum", 95)], q, |this, v, cx| this.set_export(|x| x.rate = Rate::Quality(v), cx), cx).into_any_element(),
                    Rate::Bitrate(k) => div().text_size(px(12.)).text_color(tok::text_secondary()).child(format!("{} Mbps (Advanced)", fps_label(k as f64 / 1000.0))).into_any_element(),
                };
                body = body.child(row("Quality", div().flex_1().child(quality).into_any_element()));
            }
        }

        // see-through where nothing covers the frame (formats that can carry it)
        if s.alpha_possible() {
            body = body.child(row(
                "Background",
                div()
                    .flex_1()
                    .child(self.segmented("exp-alpha".into(), &[("Black", false), ("Transparent", true)], s.alpha, |this, v, cx| this.set_export(|x| x.alpha = v, cx), cx))
                    .into_any_element(),
            ));
            if s.alpha && s.video == VideoCodec::Hevc {
                body = body.child(row("", hint("HEVC with transparency plays in Final Cut, Motion, Safari and Keynote").flex_1().min_w_0().into_any_element()));
            }
        }

        // the rest, folded
        let open = panel.advanced;
        body = body.child(
            div()
                .id("exp-advanced")
                .flex()
                .items_center()
                .gap_1()
                .mt_2()
                .pt_3()
                .border_t_1()
                .border_color(tok::border())
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(tok::text_secondary())
                .hover(|d| d.text_color(tok::text()))
                .child(ui::icon(if open { "chevron-down" } else { "chevron-right" }).size(px(13.)))
                .child("Advanced")
                .child(div().flex_1())
                .when(!open, |d| d.child(div().font_weight(FontWeight::NORMAL).text_color(tok::text_tertiary()).child(advanced_summary(&s))))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(p) = &mut this.export_panel {
                        p.advanced = !p.advanced;
                    }
                    cx.notify();
                })),
        );
        if open {
            let containers: Vec<(String, Container)> = Container::ALL.iter().map(|c| (c.name().to_string(), *c)).collect();
            body = body.child(row("File type", self.menu_button("exp-container", containers, s.container, |this, v, cx| this.set_export(|x| x.container = v, cx), cx).into_any_element()));
            if s.container.has_video() {
                let codecs: Vec<(String, VideoCodec)> = s.container.video_codecs().iter().map(|c| (c.name().to_string(), *c)).collect();
                if codecs.len() > 1 {
                    body = body.child(row("Video codec", self.menu_button("exp-vcodec", codecs, s.video, |this, v, cx| this.set_export(|x| x.video = v, cx), cx).into_any_element()));
                }
                let profiles: Vec<(String, u8)> = s.video.profiles().iter().enumerate().map(|(i, p)| (p.to_string(), i as u8)).collect();
                if !profiles.is_empty() {
                    body = body.child(row("Profile", self.menu_button("exp-profile", profiles, s.profile, |this, v, cx| this.set_export(|x| x.profile = v, cx), cx).into_any_element()));
                }
                if s.video.hardware_possible() {
                    body = body.child(row(
                        "Encoder",
                        self.menu_button("exp-hw", vec![("Hardware (faster)".into(), true), ("Software (smaller files)".into(), false)], s.hardware, |this, v, cx| this.set_export(|x| x.hardware = v, cx), cx).into_any_element(),
                    ));
                }
                if matches!(s.video, VideoCodec::Hevc | VideoCodec::Av1) {
                    body = body.child(row("Colour", self.menu_button("exp-bits", vec![("8-bit".into(), false), ("10-bit (HDR-ready)".into(), true)], s.ten_bit, |this, v, cx| this.set_export(|x| x.ten_bit = v, cx), cx).into_any_element()));
                }
                if s.video.rate_controlled() {
                    let by_rate = matches!(s.rate, Rate::Bitrate(_));
                    let mut cell = div().flex().items_center().gap_2().child(self.menu_button(
                        "exp-ratemode",
                        vec![("By quality".into(), false), ("Fixed bitrate".into(), true)],
                        by_rate,
                        |this, v, cx| this.set_export(|x| x.rate = if v { Rate::Bitrate(16_000) } else { Rate::Quality(70) }, cx),
                        cx,
                    ));
                    if by_rate {
                        cell = cell
                            .children(self.text_field("exp-kbps").map(|i| div().w(px(64.)).child(TextInput::new(&i).small())))
                            .child(div().text_size(px(12.)).text_color(tok::text_tertiary()).child("Mbps"));
                    }
                    body = body.child(row("Bitrate", cell.into_any_element()));
                }
            }
            if s.container.has_audio() {
                let mut codecs: Vec<(String, Option<AudioCodec>)> = Vec::new();
                if s.container.has_video() {
                    codecs.push(("No sound".into(), None));
                }
                codecs.extend(s.container.audio_codecs().iter().map(|c| (c.name().to_string(), Some(*c))));
                let mut cell = div().flex().items_center().gap_2().child(self.menu_button("exp-acodec", codecs, s.audio, |this, v, cx| this.set_export(|x| x.audio = v, cx), cx));
                if let Some(a) = s.audio {
                    if a.has_bitrate() {
                        let kbps: Vec<(String, u32)> = [96u32, 128, 192, 256, 320].iter().map(|k| (format!("{k} kbps"), *k)).collect();
                        cell = cell.child(self.menu_button("exp-akbps", kbps, s.audio_kbps, |this, v, cx| this.set_export(|x| x.audio_kbps = v, cx), cx));
                    }
                    if a != AudioCodec::Opus {
                        let rates: Vec<(String, u32)> = vec![("44.1 kHz".into(), 44_100), ("48 kHz".into(), 48_000), ("96 kHz".into(), 96_000)];
                        cell = cell.child(self.menu_button("exp-arate", rates, s.sample_rate, |this, v, cx| this.set_export(|x| x.sample_rate = v, cx), cx));
                    }
                    cell = cell.child(self.menu_button("exp-ch", vec![("Stereo".into(), 2u8), ("Mono".into(), 1)], s.channels, |this, v, cx| this.set_export(|x| x.channels = v, cx), cx));
                }
                body = body.child(row("Audio", cell.flex_wrap().into_any_element()));
            }
            let mut ranges: Vec<(String, Range)> = vec![("Whole project".into(), Range::All), ("From the playhead".into(), Range::FromPlayhead)];
            if seq.markers.len() >= 2 {
                ranges.push(("Between the markers".into(), Range::Markers));
            }
            let (a, b) = self.export_range();
            body = body.child(row(
                "Range",
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.menu_button("exp-range", ranges, panel.range, |this, v, cx| {
                        if let Some(p) = &mut this.export_panel {
                            p.range = v;
                        }
                        cx.notify();
                    }, cx))
                    .child(div().text_size(px(11.5)).text_color(tok::text_tertiary()).child(format!("{} – {}", clock(a), clock(b))))
                    .into_any_element(),
            ));
        }

        // how long and how big it'll be, and the buttons; or how far it's got; or how it went
        let (a, b) = self.export_range();
        let estimate = s.estimate(project, project_fps, b.saturating_sub(a));
        let footer = div().flex().items_center().gap_2().px_5().py_3().border_t_1().border_color(tok::border()).bg(tok::bg_app().opacity(0.35));
        let footer = match (&panel.result, busy) {
            (_, true) => {
                let f = self.exporting.unwrap_or(0.0).clamp(0.0, 1.0);
                let left = panel.started.map(|t| t.elapsed().as_secs_f32()).filter(|_| f > 0.02).map(|e| format!("  ·  {} left", clock(((e / f - e).max(0.0) * 1000.0) as u64)));
                footer
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap_1p5()
                            .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child(format!("Exporting  {:.0}%{}", f * 100.0, left.unwrap_or_default())))
                            .child(div().h(px(4.)).w_full().rounded_full().bg(tok::bg_raised()).child(div().h_full().rounded_full().bg(tok::accent_primary()).w(relative(f)))),
                    )
                    .child(Button::new("exp-stop").label("Stop").small().ghost().on_click(cx.listener(|this, _, _, cx| {
                        if let Some(stop) = this.export_panel.as_ref().and_then(|p| p.stop.clone()) {
                            stop.store(true, Ordering::Relaxed);
                        }
                        cx.notify();
                    })))
            }
            (Some(Ok(path)), false) => {
                let (shown, opened) = (path.clone(), path.clone());
                footer
                    .child(ui::icon("circle-check").size(px(15.)).text_color(tok::ok()))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).child("Exported"))
                    .child(Button::new("exp-reveal").label("Show in Finder").small().ghost().on_click(move |_, _, cx| cx.reveal_path(&shown)))
                    .child(Button::new("exp-open").label("Open").small().ghost().on_click(move |_, _, cx| cx.open_with_system(&opened)))
                    .child(Button::new("exp-done").label("Done").small().primary().on_click(cx.listener(|this, _, _, cx| this.close_export(cx))))
            }
            (Some(Err(e)), false) => {
                let stopped = e.ends_with("stopped");
                footer
                    .child(ui::icon(if stopped { "circle-slash" } else { "triangle-alert" }).size(px(15.)).text_color(if stopped { tok::text_tertiary() } else { tok::error() }))
                    .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(tok::text_secondary()).child(if stopped { "Stopped. Nothing was kept.".to_string() } else { format!("It couldn't be exported: {e}") }))
                    .child(Button::new("exp-close").label("Close").small().ghost().on_click(cx.listener(|this, _, _, cx| this.close_export(cx))))
                    .child(Button::new("exp-again").label("Try again").small().primary().on_click(cx.listener(|this, _, _, cx| this.run_export(cx))))
            }
            (None, false) => footer
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .text_color(tok::text_tertiary())
                        .child(match estimate {
                            Some(e) => format!("{}  ·  about {}", clock(b.saturating_sub(a)), megabytes(e)),
                            None => clock(b.saturating_sub(a)),
                        }),
                )
                .child(Button::new("exp-cancel").label("Cancel").small().ghost().on_click(cx.listener(|this, _, _, cx| this.close_export(cx))))
                .child(Button::new("exp-go").label("Export").small().primary().on_click(cx.listener(|this, _, _, cx| this.run_export(cx)))),
        };
        let _ = me;

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                // the Editor dimmed behind it (its picture is bright)
                .bg(hsla(0., 0., 0., 0.6))
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_export(cx)))
                .child(
                    div()
                        .id("export-panel")
                        .flex()
                        .flex_col()
                        .w(px(500.))
                        .max_h(relative(0.9))
                        .overflow_hidden()
                        .rounded(px(12.))
                        .bg(tok::bg_panel())
                        .border_1()
                        .border_color(tok::border_strong())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                            match ev.keystroke.key.as_str() {
                                "escape" => this.close_export(cx),
                                "enter" if this.export_panel.as_ref().is_some_and(|p| p.started.is_none() && p.result.is_none()) => this.run_export(cx),
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .h(px(48.))
                                .px_5()
                                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Export"))
                                .child(div().flex_1())
                                .when(!busy, |d| d.child(ui::icon_button("exp-x", "x", tok::text_tertiary()).on_click(cx.listener(|this, _, _, cx| this.close_export(cx))))),
                        )
                        .child(body)
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    /// For screenshots: the export panel open (`preset`: one of the presets, by number, with
    /// Advanced unfolded).
    pub fn hook_export(&mut self, preset: Option<usize>, cx: &mut Context<Self>) {
        self.open_export(cx);
        if let (Some(p), Some(i)) = (&mut self.export_panel, preset) {
            p.settings = PRESETS.get(i).map_or(p.settings, |x| x.2);
            p.advanced = true;
            // (HEVC with transparency: in a 4:5 frame, so every note shows)
            if i == 9 {
                p.settings.size = Some((1080, 1350));
                p.advanced = false;
            }
        }
        cx.notify();
    }
}
