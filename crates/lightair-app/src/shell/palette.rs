//! The command palette (⌘K, docs/05-interface-shell.md): type to find any action or scene.

use gpui_kit::component::input::{Input as TextInput, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_model::{Command, Id, Workspace, TransitionKind, TransitionSettings};

use super::StudioWindow;
use crate::ui::{self, tok};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum PaletteAction {
    Scene(Id),
    Take,
    CueScenes,
    Record,
    PauseRecording,
    Marker,
    SplitRecording,
    GoLive,
    Clip,
    Replay,
    AddScene,
    AddImage,
    AddVideo,
    AddText,
    AddColour,
    AddTimer,
    AddVertical,
    AddScreenLayouts,
    StudioTake,
    ImportObs,
    Transition(TransitionKind),
    Undo,
    Redo,
    Settings,
    Mics,
}

pub(super) struct Entry {
    pub label: String,
    pub hint: &'static str,
    pub icon: &'static str,
    pub action: PaletteAction,
}

pub(super) struct Palette {
    input: Entity<InputState>,
    selected: usize,
    _events: Subscription,
}

/// Fuzzy match: every query character appears in order. Lower scores are better; `None` is
/// no match. Prefix and word-start matches rank first, then compact matches.
pub(super) fn score(query: &str, label: &str) -> Option<u32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let l: Vec<char> = label.to_lowercase().chars().collect();
    let mut qi = 0;
    let (mut first, mut last, mut word_starts) = (None, 0, 0);
    for (i, c) in l.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            first.get_or_insert(i);
            last = i;
            if i == 0 || !l[i - 1].is_alphanumeric() {
                word_starts += 1;
            }
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    let first = first.unwrap_or(0) as u32;
    let spread = (last as u32 + 1).saturating_sub(first) - q.len() as u32;
    Some(first * 4 + spread * 2 + (q.len() as u32).saturating_sub(word_starts))
}

fn entries(workspace: &Workspace, cue_scenes: bool) -> Vec<Entry> {
    use PaletteAction::*;
    let mut out: Vec<Entry> = workspace
        .scenes
        .iter()
        .map(|s| Entry {
            label: s.name.clone(),
            hint: if cue_scenes { "Select scene" } else { "Go to scene" },
            icon: "layout-dashboard",
            action: Scene(s.id),
        })
        .collect();
    let fixed: &[(&str, &'static str, &'static str, PaletteAction)] = &[
        ("Take", "⌘↩", "play", Take),
        (if cue_scenes { "Cue scenes: turn off" } else { "Cue scenes" }, "", "lightair-cue", CueScenes),
        ("Start or stop recording", "", "circle", Record),
        ("Pause or resume recording", "", "pause", PauseRecording),
        ("Add marker", "⌘M", "bookmark", Marker),
        ("Continue recording in a new file", "", "split", SplitRecording),
        ("Go live", "", "radio", GoLive),
        ("Save clip", "", "scissors", Clip),
        ("Replay last clip", "", "rotate-ccw", Replay),
        ("Add scene", "", "plus", AddScene),
        ("Add image object", "", "image", AddImage),
        ("Add video object", "", "clapperboard", AddVideo),
        ("Add text object", "", "type", AddText),
        ("Add colour object", "", "square", AddColour),
        ("Add timer object", "", "timer", AddTimer),
        ("Add vertical canvas", "", "smartphone", AddVertical),
        ("Add screen recording scenes", "", "monitor", AddScreenLayouts),
        ("Start or stop a Studio take", "", "mouse-pointer-2", StudioTake),
        ("Import OBS scenes", "", "import", ImportObs),
        ("Undo", "⌘Z", "undo-2", Undo),
        ("Redo", "⇧⌘Z", "redo-2", Redo),
        ("Settings", "⌘,", "settings", Settings),
        ("Mute or unmute microphones", "⇧⌘M", "mic-off", Mics),
    ];
    out.extend(fixed.iter().map(|(label, hint, icon, action)| Entry {
        label: label.to_string(),
        hint,
        icon,
        action: *action,
    }));
    out.extend(TransitionKind::ALL.iter().map(|k| Entry {
        label: format!("Transition: {}", k.label()),
        hint: "",
        icon: "arrow-right-left",
        action: Transition(*k),
    }));
    out
}

/// Entries matching `query`, best first.
pub(super) fn matches(workspace: &Workspace, cue_scenes: bool, query: &str) -> Vec<Entry> {
    let mut scored: Vec<(u32, usize, Entry)> = entries(workspace, cue_scenes)
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| score(query, &e.label).map(|s| (s, i, e)))
        .collect();
    scored.sort_by_key(|(s, i, _)| (*s, *i));
    scored.into_iter().map(|(.., e)| e).collect()
}

impl StudioWindow {
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return self.close_palette(window, cx);
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command or scene…"));
        input.update(cx, |s, cx| s.focus(window, cx));
        let events = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                if let Some(p) = &mut this.palette {
                    p.selected = 0;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.run_selected(window, cx),
            InputEvent::Blur => this.close_palette(window, cx),
            _ => {}
        });
        self.palette = Some(Palette { input, selected: 0, _events: events });
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn palette_matches(&self, cx: &App) -> Vec<Entry> {
        let Some(p) = &self.palette else { return vec![] };
        let query = p.input.read(cx).value().to_string();
        matches(self.studio.read(cx).workspace(), self.cue_scenes, &query)
    }

    fn run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.palette.as_ref().map_or(0, |p| p.selected);
        let Some(entry) = self.palette_matches(cx).into_iter().nth(selected) else { return };
        self.close_palette(window, cx);
        self.run_palette_action(entry.action, window, cx);
    }

    pub(super) fn run_palette_action(&mut self, action: PaletteAction, window: &mut Window, cx: &mut Context<Self>) {
        use PaletteAction::*;
        match action {
            Scene(id) => {
                let cue_scenes = self.cue_scenes;
                self.studio.update(cx, |s, cx| {
                    s.select_scene(id);
                    if cue_scenes {
                        cx.notify();
                    } else {
                        let _ = s.execute(Command::SetProgram { scene: Some(id) }, cx);
                    }
                });
            }
            Take => self.take(cx),
            CueScenes => self.set_cue_scenes(!self.cue_scenes, cx),
            Record => self.studio.update(cx, |s, cx| s.toggle_recording(cx)),
            PauseRecording => self.studio.update(cx, |s, cx| s.pause_recording(cx)),
            Marker => self.studio.update(cx, |s, cx| s.add_marker(cx)),
            SplitRecording => self.studio.update(cx, |s, cx| s.split_recording(cx)),
            GoLive => self.open_go_live(cx),
            Clip => self.studio.update(cx, |s, cx| s.save_clip(cx)),
            Replay => self.studio.update(cx, |s, cx| s.replay_last_clip(cx)),
            AddScene => self.add_scene(cx),
            AddImage => self.add_image(window, cx),
            AddVideo => self.add_video(window, cx),
            AddText => self.add_text(cx),
            AddColour => self.add_colour(cx),
            AddTimer => self.add_timer(cx),
            AddVertical => self.add_vertical(cx),
            AddScreenLayouts => self.add_screen_layouts(cx),
            StudioTake => self.studio_take(cx),
            ImportObs => self.import_obs(window, cx),
            Transition(kind) => {
                let current = self.studio.read(cx).workspace().transition;
                self.exec(Command::SetTransition { settings: TransitionSettings { kind, ..current } }, cx);
            }
            Undo => self.studio.update(cx, |s, cx| s.undo(cx)),
            Redo => self.studio.update(cx, |s, cx| s.redo(cx)),
            Settings => self.toggle_settings(cx),
            Mics => self.studio.update(cx, |s, cx| s.toggle_mics(cx)),
        }
    }

    /// The palette overlay, when open.
    pub(super) fn palette_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.palette.as_ref()?;
        let list = self.palette_matches(cx);
        let count = list.len();
        let selected = p.selected.min(count.saturating_sub(1));
        let rows = list.into_iter().take(12).enumerate().map(|(i, e)| {
            let action = e.action;
            div()
                .id(("palette-row", i))
                .flex()
                .items_center()
                .gap_2()
                .h(px(34.))
                .px_3()
                .rounded(px(6.))
                .cursor_pointer()
                .when(i == selected, |d| d.bg(tok::bg_selected()))
                .hover(|d| d.bg(tok::bg_raised()))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_palette(window, cx);
                    this.run_palette_action(action, window, cx);
                }))
                .child(ui::icon(e.icon).size(px(15.)).text_color(tok::text_secondary()))
                .child(div().flex_1().child(e.label))
                .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(crate::ui::keys_label(e.hint).into_owned()))
        });
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .pt(px(96.))
                .bg(tok::scrim())
                // a sheet: nothing behind it takes the mouse (scrolling here never pans the canvas)
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_palette(window, cx)))
                .child(
                    div()
                        .id("palette")
                        .flex()
                        .flex_col()
                        .gap_1()
                        .w(px(520.))
                        .max_h(px(480.))
                        .p_2()
                        .rounded(px(12.))
                        .bg(tok::bg_raised())
                        .border_1()
                        .border_color(tok::border_strong())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                            let step: isize = match ev.keystroke.key.as_str() {
                                "down" => 1,
                                "up" => -1,
                                "escape" => {
                                    this.close_palette(window, cx);
                                    cx.stop_propagation();
                                    return;
                                }
                                _ => return,
                            };
                            if let Some(p) = &mut this.palette {
                                let last = count.saturating_sub(1).min(11) as isize;
                                p.selected = (p.selected as isize + step).clamp(0, last) as usize;
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .child(TextInput::new(&p.input))
                        .when(count == 0, |d| {
                            d.child(div().px_3().py_2().text_color(tok::text_tertiary()).child("No matches"))
                        })
                        .children(rows)
                        .tooltip(|w, cx| Tooltip::new("↑↓ to choose, ↩ to run, Esc to close").build(w, cx)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{matches, score};
    use lightair_model::{Workspace, Scene};

    #[test]
    fn scoring() {
        assert_eq!(score("", "Anything"), Some(0));
        assert!(score("rec", "Start or stop recording").is_some());
        assert!(score("xyz", "Take").is_none());
        assert!(score("ta", "Take").unwrap() < score("ta", "Rotate all").unwrap(), "prefix first");
    }

    #[test]
    fn scenes_and_commands_are_found() {
        let mut p = Workspace::new("P");
        p.scenes.push(std::sync::Arc::new(Scene::new("Be Right Back")));
        assert_eq!(matches(&p, false, "brb")[0].label, "Be Right Back");
        assert_eq!(matches(&p, false, "cue")[0].label, "Cue scenes");
        assert_eq!(matches(&p, true, "cue")[0].label, "Cue scenes: turn off");
        assert!(matches(&p, false, "slide").iter().any(|e| e.label == "Transition: Slide"));
    }
}
