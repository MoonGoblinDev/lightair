//! The Library (docs/plans/screen-studio.md, "Library"): every recording Lightair made, by
//! workspace. Workspaces down the side, the selection's items as cards grouped by day, and what can
//! be done with them: play, reveal, rename, duplicate, move to another workspace, locate a moved
//! file, move to the Trash (asked first), and bring in older recordings.
//!
//! Each workspace's items come from its recordings index (`lightair_store::recordings`); the
//! media stays where it was recorded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Datelike, Local, TimeZone};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input as TextInput, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_model::Id;
use lightair_engine::player::Player;
use lightair_preview::Preview;
use lightair_store::recordings::{self, Index, Item, Kind};

use crate::studio::Studio;
use crate::ui::{self, tok};

mod inside;
mod pick;
pub(crate) use inside::part_icon;

/// Where each card is drawn (for picking with a box).
type CardBounds = std::rc::Rc<std::cell::RefCell<HashMap<(PathBuf, Id), Bounds<Pixels>>>>;

/// A workspace in the side list.
#[derive(Clone)]
pub(crate) struct WorkspaceEntry {
    pub name: String,
    pub dir: PathBuf,
}

/// The file playing in the player.
struct Playing {
    player: Arc<Player>,
    preview: Preview,
    title: String,
    /// The seek bar's place on screen, for clicks.
    bar: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
}

pub struct Library {
    studio: Entity<Studio>,
    workspaces: Vec<WorkspaceEntry>,
    /// `None`: All recordings.
    selected: Option<PathBuf>,
    indexes: HashMap<PathBuf, Index>,
    /// Bytes per workspace (counted on reload).
    sizes: HashMap<PathBuf, u64>,
    seen_version: Option<u64>,
    search: Entity<InputState>,
    kind: Option<Kind>,
    renaming: Option<(PathBuf, Id, Entity<InputState>)>,
    /// What's to go to the Trash (asked first): one item or several.
    confirm_trash: Vec<(PathBuf, Id)>,
    playing: Option<Playing>,
    note: Option<SharedString>,
    /// Asked for by "Open in Editor": the window switches to the Editor.
    pub open_in_editor: Option<(PathBuf, Id)>,
    /// Items asked for in the Editor together (the window takes them there).
    pub open_many: Option<Vec<Id>>,
    /// The items picked, the last one picked alone (for ⇧-click runs), a box being dragged to
    /// pick, and where the cards and the cards' area are.
    picked: Vec<(PathBuf, Id)>,
    anchor: Option<(PathBuf, Id)>,
    marquee: Option<pick::Marquee>,
    card_bounds: CardBounds,
    body_bounds: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
    /// Parts of a studio_take asked for in the Editor (the window takes them there).
    pub open_parts: Option<Vec<lightair_render::sequence::Part>>,
    /// Takes opened to show their parts, and their parts (`None`: being read).
    inside_open: std::collections::HashSet<Id>,
    inside: HashMap<Id, Option<Vec<lightair_render::sequence::Part>>>,
    /// Asked for: open this workspace / make one with this name (the window does it).
    pub open_workspace: Option<PathBuf>,
    pub new_workspace: Option<String>,
    /// The new workspace's name, being typed.
    naming: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
}

/// The workspaces in Lightair's Workspaces folder (packages with a workspace.json), by name; and
/// the open one, wherever it is.
pub(crate) fn list_workspaces(current: Option<&Path>) -> Vec<WorkspaceEntry> {
    let name_of = |dir: &Path| -> Option<String> {
        let text = std::fs::read_to_string(lightair_store::manifest(dir)).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        v["name"].as_str().map(String::from)
    };
    // the Workspaces folder, and the open workspace's own folder (where its new workspaces go)
    let mut folders = vec![lightair_platform::paths::workspaces_dir()];
    if let Some(parent) = current.and_then(Path::parent).filter(|p| *p != folders[0]) {
        folders.push(parent.to_path_buf());
    }
    let mut out: Vec<WorkspaceEntry> = folders
        .iter()
        .flat_map(|f| std::fs::read_dir(f).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|d| d.extension().is_some_and(|e| e == "lightair"))
        .filter_map(|dir| Some(WorkspaceEntry { name: name_of(&dir)?, dir }))
        .collect();
    if let Some(cur) = current
        && !out.iter().any(|p| p.dir == cur)
        && let Some(name) = name_of(cur)
    {
        out.push(WorkspaceEntry { name, dir: cur.to_path_buf() });
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    // workspaces sharing a name are told apart by their folder
    let names: Vec<String> = out.iter().map(|p| p.name.clone()).collect();
    for p in &mut out {
        if names.iter().filter(|n| **n == p.name).count() > 1
            && let Some(folder) = p.dir.file_name()
        {
            p.name = format!("{} ({})", p.name, folder.to_string_lossy());
        }
    }
    out
}

/// "Today", "Yesterday", or the date.
fn day_label(ms: i64) -> String {
    let Some(t) = Local.timestamp_millis_opt(ms).single() else { return "Unknown date".into() };
    let today = Local::now().date_naive();
    match (today - t.date_naive()).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if t.year() == today.year() => t.format("%A %-d %B").to_string(),
        _ => t.format("%-d %B %Y").to_string(),
    }
}

/// 1:05, 1:02:03.
fn duration_label(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// 210 KB, 812 MB, 3.4 GB.
fn bytes_label(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", (b / 1e3).ceil())
    }
}

fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Recording => "video",
        Kind::StudioTake => "mouse-pointer-2",
        Kind::Clip => "scissors",
        Kind::Multitrack => "audio-lines",
        Kind::Export => "film",
    }
}

impl Library {
    pub fn new(studio: Entity<Studio>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search titles, files and markers"));
        let subs = vec![cx.subscribe(&search, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        })];
        let selected = studio.read(cx).workspace_dir();
        Library {
            studio,
            workspaces: Vec::new(),
            selected,
            indexes: HashMap::new(),
            sizes: HashMap::new(),
            seen_version: None,
            search,
            kind: None,
            renaming: None,
            confirm_trash: Vec::new(),
            playing: None,
            note: None,
            open_in_editor: None,
            open_parts: None,
            open_many: None,
            picked: Vec::new(),
            anchor: None,
            marquee: None,
            card_bounds: Default::default(),
            body_bounds: Default::default(),
            inside_open: Default::default(),
            inside: HashMap::new(),
            open_workspace: None,
            new_workspace: None,
            naming: None,
            _subscriptions: subs,
        }
    }

    /// Reads the workspaces and their indexes again (when shown, and when a recording finishes).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let current = self.studio.read(cx).workspace_dir();
        self.workspaces = list_workspaces(current.as_deref());
        self.indexes = self.workspaces.iter().map(|p| (p.dir.clone(), Index::load(&p.dir))).collect();
        self.sizes = self.indexes.iter().map(|(d, i)| (d.clone(), i.bytes())).collect();
        if self.selected.as_ref().is_some_and(|s| !self.workspaces.iter().any(|p| &p.dir == s)) {
            self.selected = None;
        }
        self.seen_version = Some(self.studio.read(cx).library_version);
        cx.notify();
    }

    fn save(&mut self, workspace: &Path) {
        if let Some(index) = self.indexes.get(workspace)
            && let Err(e) = index.save(workspace)
        {
            self.note = Some(format!("Couldn't save the Library: {e}").into());
        }
        self.sizes.insert(workspace.to_path_buf(), self.indexes.get(workspace).map_or(0, Index::bytes));
    }

    /// The items shown: the selection's, filtered, newest first, with their workspace.
    fn shown(&self, cx: &App) -> Vec<(PathBuf, Item)> {
        let query = self.search.read(cx).value().to_string();
        let mut out: Vec<(PathBuf, Item)> = self
            .indexes
            .iter()
            .filter(|(dir, _)| self.selected.as_ref().is_none_or(|s| s == *dir))
            .flat_map(|(dir, index)| index.items.iter().map(move |i| (dir.clone(), i.clone())))
            .filter(|(_, i)| self.kind.is_none_or(|k| i.kind == k) && i.matches(&query))
            .collect();
        out.sort_by_key(|(_, i)| std::cmp::Reverse(i.started_ms));
        out
    }

    // ------------------------------------------------------------ actions

    fn play(&mut self, workspace: &Path, id: Id, cx: &mut Context<Self>) {
        let Some(item) = self.indexes.get(workspace).and_then(|i| i.get(id)).cloned() else { return };
        // the first file that's there: the program's video, or the first audio file
        let Some(path) = item.files.iter().map(|f| f.path.clone()).find(|p| p.exists() && !p.extension().is_some_and(|e| e == "json")) else {
            self.note = Some("Its files aren't where they were. Locate them first.".into());
            cx.notify();
            return;
        };
        let opened = self.studio.read(cx).engine().call(move |_| Player::open(&path).map(Arc::new));
        let Some(player) = opened else {
            self.note = Some("Lightair couldn't open it.".into());
            cx.notify();
            return;
        };
        let preview = Preview::attach();
        preview.set_own_size(true);
        preview.set_size(Some((1280, 720)));
        preview.set_source(Some(player.source_ref()));
        self.playing = Some(Playing { player, preview, title: item.title, bar: Default::default() });
        cx.notify();
    }

    fn close_player(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.playing.take() {
            p.preview.set_source(None);
            // the source goes on the engine thread, with libobs's other work
            let player = p.player;
            self.studio.read(cx).engine().post(move |_| drop(player));
        }
        cx.notify();
    }

    fn reveal(&mut self, workspace: &Path, id: Id, cx: &mut Context<Self>) {
        let Some(item) = self.indexes.get(workspace).and_then(|i| i.get(id)) else { return };
        let target = item.files.iter().map(|f| f.path.clone()).find(|p| p.exists());
        match target {
            Some(p) => {
                if let Err(e) = lightair_platform::files::reveal(&p) {
                    self.note = Some(format!("Couldn't show it: {e}").into());
                }
            }
            None => self.note = Some("Its files aren't where they were. Locate them first.".into()),
        }
        cx.notify();
    }

    fn start_rename(&mut self, workspace: PathBuf, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self.indexes.get(&workspace).and_then(|i| i.get(id)).map(|i| i.title.clone()) else { return };
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |s, cx| {
            s.set_value(title, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.finish_rename(cx);
            }
        });
        self._subscriptions.push(sub);
        self.renaming = Some((workspace, id, input));
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some((workspace, id, input)) = self.renaming.take() else { return };
        let title = input.read(cx).value().trim().to_string();
        if !title.is_empty()
            && let Some(item) = self.indexes.get_mut(&workspace).and_then(|i| i.get_mut(id))
        {
            item.title = title;
            self.save(&workspace);
        }
        cx.notify();
    }

    fn duplicate(&mut self, workspace: PathBuf, id: Id, cx: &mut Context<Self>) {
        let result = self.indexes.get_mut(&workspace).map(|i| i.duplicate(id));
        match result {
            Some(Ok(copy)) => {
                self.save(&workspace);
                self.note = Some(format!("Made “{}”", copy.title).into());
            }
            Some(Err(e)) => self.note = Some(format!("Couldn't duplicate it: {e}").into()),
            None => {}
        }
        cx.notify();
    }

    fn move_to(&mut self, from: PathBuf, to: PathBuf, id: Id, cx: &mut Context<Self>) {
        // the open copies of both indexes are what's saved
        for dir in [&from, &to] {
            self.save(dir);
        }
        match recordings::move_item(&from, &to, id) {
            Ok(_) => {
                let name = self.workspaces.iter().find(|p| p.dir == to).map(|p| p.name.clone()).unwrap_or_default();
                self.note = Some(format!("Moved to {name}").into());
            }
            Err(e) => self.note = Some(format!("Couldn't move it: {e}").into()),
        }
        self.reload(cx);
    }

    fn locate(&mut self, workspace: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Look in this folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(folder) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                let found = this.indexes.get_mut(&workspace).map_or(0, |i| i.locate_in(&folder));
                this.save(&workspace);
                this.note = Some(match found {
                    0 => "Nothing missing was in that folder.".into(),
                    1 => "Found 1 file.".into(),
                    n => format!("Found {n} files.").into(),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// A studio_take, with its edit, copied into a folder the user picks, to give or keep.
    fn export_package(&mut self, workspace: PathBuf, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.indexes.get(&workspace).and_then(|i| i.get(id)).cloned() else { return };
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Put the package here".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(folder) = paths.into_iter().next() else { return };
            let edit = workspace.join("recordings").join("edits").join(format!("{id}.json"));
            let result = recordings::export_package(&item, Some(&edit), &folder);
            let _ = this.update(cx, |this, cx| {
                this.note = Some(match result {
                    Ok(p) => format!("Saved {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).into(),
                    Err(e) => format!("Couldn't save the package: {e}").into(),
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn add_existing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.selected.clone().or_else(|| self.studio.read(cx).workspace_dir()) else { return };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add the recordings in this folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(folder) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                let index = this.indexes.entry(workspace.clone()).or_default();
                let found = index.existing_in(&folder);
                let n = found.len();
                index.items.extend(found);
                this.save(&workspace);
                this.note = Some(match n {
                    0 => "No new recordings in that folder.".into(),
                    1 => "Added 1 recording.".into(),
                    n => format!("Added {n} recordings.").into(),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn trash(&mut self, cx: &mut Context<Self>) {
        let items = std::mem::take(&mut self.confirm_trash);
        let (mut moved, mut failed) = (0, None);
        for (workspace, id) in &items {
            match self.indexes.get_mut(workspace).map(|i| i.trash(*id, lightair_platform::files::trash)) {
                Some(Ok(())) => moved += 1,
                Some(Err(e)) => failed = Some(e.to_string()),
                None => {}
            }
            self.picked.retain(|p| p != &(workspace.clone(), *id));
        }
        let workspaces: std::collections::HashSet<PathBuf> = items.iter().map(|(p, _)| p.clone()).collect();
        for p in workspaces {
            self.save(&p);
        }
        self.note = Some(match failed {
            Some(e) => format!("Couldn't move it to the {}: {e}", trash_name()).into(),
            None if moved == 1 => format!("Moved to the {}", trash_name()).into(),
            None => format!("{moved} items moved to the {}", trash_name()).into(),
        });
        cx.notify();
    }

    /// A field for the new workspace's name; Enter makes it, Escape (or leaving it empty) doesn't.
    pub(crate) fn start_new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name"));
        input.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let name = input.read(cx).value().trim().to_string();
                this.naming = None;
                if !name.is_empty() {
                    this.new_workspace = Some(name);
                }
                cx.notify();
            }
            InputEvent::Blur if input.read(cx).value().trim().is_empty() => {
                this.naming = None;
                cx.notify();
            }
            _ => {}
        });
        self._subscriptions.push(sub);
        self.naming = Some(input);
        cx.notify();
    }

    // ------------------------------------------------------------ view

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = |dir: Option<&PathBuf>| -> usize {
            self.indexes.iter().filter(|(d, _)| dir.is_none_or(|x| x == *d)).map(|(_, i)| i.items.len()).sum()
        };
        let row = |id: SharedString, icon: &'static str, label: String, n: usize, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .h(px(30.))
                .px_2()
                .rounded(px(6.))
                .cursor_pointer()
                .when(on, |d| d.bg(tok::bg_selected()))
                .hover(|d| d.bg(tok::bg_selected()))
                .child(ui::icon(icon).size(px(14.)).text_color(tok::text_secondary()))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(label))
                .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(n.to_string()))
        };
        let total: u64 = self.sizes.values().sum();
        let mut list = div().flex().flex_col().gap_0p5().child(
            row("lib-all".into(), "library", "All recordings".into(), count(None), self.selected.is_none())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.selected = None;
                    cx.notify();
                })),
        );
        list = list.child(
            div()
                .flex()
                .items_center()
                .pt_3()
                .pb_1()
                .pl_2()
                .child(div().flex_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text_tertiary()).child("WORKSPACES"))
                .child(
                    ui::icon_button("lib-new-workspace", "plus", tok::text_secondary())
                        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("New workspace").build(w, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.start_new_workspace(window, cx))),
                ),
        );
        if let Some(input) = &self.naming {
            list = list.child(div().px_1().pb_1().child(TextInput::new(input).small()));
        }
        let current = self.studio.read(cx).workspace_dir();
        for (n, p) in self.workspaces.iter().enumerate() {
            let dir = p.dir.clone();
            let on = self.selected.as_ref() == Some(&p.dir);
            let open = current.as_ref() == Some(&p.dir);
            let menu = {
                let (me, dir) = (cx.entity().downgrade(), p.dir.clone());
                move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                    let (me2, dir2) = (me.clone(), dir.clone());
                    menu.item(PopupMenuItem::new("Open workspace").icon(ui::icon("folder-open")).disabled(open).on_click(move |_, _, cx| {
                        if let Some(me) = me2.upgrade() {
                            me.update(cx, |this, cx| {
                                this.open_workspace = Some(dir2.clone());
                                cx.notify();
                            });
                        }
                    }))
                    .item(PopupMenuItem::new(if cfg!(windows) { "Show in Explorer" } else { "Show in Finder" }).icon(ui::icon("folder")).on_click({
                        let dir = dir.clone();
                        move |_, _, _| {
                            let _ = lightair_platform::files::reveal(&dir);
                        }
                    }))
                }
            };
            list = list.child(
                row(SharedString::from(format!("lib-workspace-{n}")), if open { "folder-open" } else { "folder" }, p.name.clone(), count(Some(&p.dir)), on)
                    .when(open, |d| d.child(div().size(px(6.)).rounded_full().bg(tok::ok())))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if open { "Open now" } else { "Double-click to open" }).build(w, cx)
                    })
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        this.selected = Some(dir.clone());
                        if ev.click_count() >= 2 && !open {
                            this.open_workspace = Some(dir.clone());
                        }
                        cx.notify();
                    }))
                    .context_menu(menu),
            );
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(232.))
            .h_full()
            .p_2()
            .gap_2()
            .border_r_1()
            .border_color(tok::border())
            .bg(tok::bg_panel())
            .child(div().id("lib-workspaces").flex_1().min_h_0().overflow_y_scroll().child(list))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(tok::border())
                    .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(format!(
                        "{} in {} {}",
                        bytes_label(total),
                        count(None),
                        if count(None) == 1 { "item" } else { "items" }
                    )))
                    .child(
                        Button::new("lib-add-existing")
                            .label("Add existing recordings…")
                            .icon(ui::icon("folder-plus"))
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| this.add_existing(window, cx))),
                    ),
            )
    }

    fn filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chip = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(px(6.))
                .text_size(px(12.))
                .cursor_pointer()
                .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                .when(!on, |d| d.text_color(tok::text_secondary()))
                .hover(|d| d.bg(tok::bg_selected()))
                .child(label)
        };
        let mut row = div().flex().items_center().gap_1().child(chip("lib-kind-all", "All", self.kind.is_none()).on_click(
            cx.listener(|this, _, _, cx| {
                this.kind = None;
                cx.notify();
            }),
        ));
        for (n, k) in Kind::ALL.into_iter().enumerate() {
            let ids = ["lib-kind-0", "lib-kind-1", "lib-kind-2", "lib-kind-3", "lib-kind-4"];
            row = row.child(chip(ids[n], k.label(), self.kind == Some(k)).on_click(cx.listener(move |this, _, _, cx| {
                this.kind = Some(k);
                cx.notify();
            })));
        }
        row
    }

    fn card(&self, workspace: &Path, item: &Item, n: usize, cx: &mut Context<Self>) -> AnyElement {
        let (dir, id) = (workspace.to_path_buf(), item.id);
        let missing = !item.missing().is_empty();
        let thumbnail = item.thumbnail.clone().filter(|t| t.exists());
        let when = Local.timestamp_millis_opt(item.started_ms).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
        // its canvases (a recording of several), or what a studio_take recorded (screen, camera)
        let canvases: Vec<String> = item.files.iter().filter_map(|f| f.canvas.clone().or_else(|| f.track.clone())).collect();
        let mut detail = vec![when, item.kind.label().to_string()];
        if !canvases.is_empty() {
            detail.push(canvases.join(" + "));
        }
        let renaming = self.renaming.as_ref().filter(|(d, i, _)| *d == dir && *i == id).map(|(_, _, input)| input.clone());
        let workspace_name = (self.selected.is_none())
            .then(|| self.workspaces.iter().find(|p| p.dir == dir).map(|p| p.name.clone()))
            .flatten();

        let thumb = div()
            .relative()
            .w_full()
            .h(px(135.))
            .rounded(px(8.))
            .overflow_hidden()
            .bg(tok::bg_canvas_well())
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match &thumbnail {
                Some(path) => d.child(img(path.clone()).size_full().object_fit(ObjectFit::Cover)),
                None => d.child(ui::icon(kind_icon(item.kind)).size(px(28.)).text_color(tok::text_tertiary())),
            })
            .when_some(item.duration_ms, |d, ms| {
                d.child(
                    div()
                        .absolute()
                        .right(px(6.))
                        .bottom(px(6.))
                        .px_1p5()
                        .rounded(px(4.))
                        .bg(rgba(0x0000_00b0))
                        .text_size(px(11.))
                        .font_family(crate::ui::MONO)
                        .text_color(rgb(0xFFFFFF))
                        .child(duration_label(ms)),
                )
            })
            .when(!item.markers.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(6.))
                        .bottom(px(6.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded(px(4.))
                        .bg(rgba(0x0000_00b0))
                        .text_size(px(11.))
                        .text_color(rgb(0xFFFFFF))
                        .child(ui::icon("bookmark").size(px(10.)))
                        .child(item.markers.len().to_string()),
                )
            })
            .when(missing, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .bg(rgba(0x0000_00c0))
                        .child(ui::icon("file-question").size(px(20.)).text_color(tok::warn()))
                        .child(div().text_size(px(12.)).text_color(rgb(0xFFFFFF)).child("Missing")),
                )
            });

        let menu = {
            let (me, dir, workspaces, title, kind) = (cx.entity().downgrade(), dir.clone(), self.workspaces.clone(), item.title.clone(), item.kind);
            move |mut menu: PopupMenu, window: &mut Window, cx: &mut Context<PopupMenu>| {
                let act = |f: fn(&mut Library, PathBuf, Id, &mut Window, &mut Context<Library>)| {
                    let (me, dir) = (me.clone(), dir.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        if let Some(me) = me.upgrade() {
                            me.update(cx, |this, cx| f(this, dir.clone(), id, window, cx));
                        }
                    }
                };
                menu = menu
                    .item(PopupMenuItem::new("Play").icon(ui::icon("play")).on_click(act(|t, d, i, _, cx| t.play(&d, i, cx))))
                    .item(PopupMenuItem::new("Open in Editor").icon(ui::icon("scissors")).on_click(act(|t, d, i, _, cx| {
                        t.open_in_editor = Some((d, i));
                        cx.notify();
                    })))
                    .item(
                        PopupMenuItem::new(if cfg!(windows) { "Show in Explorer" } else { "Show in Finder" })
                            .icon(ui::icon("folder-open"))
                            .on_click(act(|t, d, i, _, cx| t.reveal(&d, i, cx))),
                    )
                    .separator()
                    .item(PopupMenuItem::new("Rename…").icon(ui::icon("pencil")).on_click(act(|t, d, i, w, cx| t.start_rename(d, i, w, cx))))
                    .item(PopupMenuItem::new("Duplicate").icon(ui::icon("copy")).on_click(act(|t, d, i, _, cx| t.duplicate(d, i, cx))));
                let others: Vec<WorkspaceEntry> = workspaces.iter().filter(|p| p.dir != dir).cloned().collect();
                if !others.is_empty() {
                    let (me, dir) = (me.clone(), dir.clone());
                    menu = menu.submenu_with_icon(Some(ui::icon("folder-input")), "Move to workspace", window, cx, move |mut sub, _, _| {
                        for p in &others {
                            let (me, from, to) = (me.clone(), dir.clone(), p.dir.clone());
                            sub = sub.item(PopupMenuItem::new(p.name.clone()).on_click(move |_, _, cx| {
                                if let Some(me) = me.upgrade() {
                                    me.update(cx, |this, cx| this.move_to(from.clone(), to.clone(), id, cx));
                                }
                            }));
                        }
                        sub
                    });
                }
                if kind == Kind::StudioTake {
                    menu = menu.item(PopupMenuItem::new("Export package…").icon(ui::icon("package")).on_click(act(|t, d, i, w, cx| t.export_package(d, i, w, cx))));
                }
                if missing {
                    menu = menu.item(PopupMenuItem::new("Locate…").icon(ui::icon("search")).on_click(act(|t, d, _, w, cx| t.locate(d, w, cx))));
                }
                let _ = &title;
                menu.separator().item(
                    PopupMenuItem::new(format!("Move to {}…", trash_name())).icon(ui::icon("trash")).on_click(act(|t, d, i, _, cx| {
                        t.confirm_trash = vec![(d, i)];
                        cx.notify();
                    })),
                )
            }
        };

        let picked = self.is_picked(&dir, id);
        let (bounds, key, pdir) = (self.card_bounds.clone(), (dir.clone(), id), dir.clone());
        div()
            .id(("lib-card", n))
            .relative()
            .flex()
            .flex_col()
            .gap_1p5()
            .w(px(240.))
            .p_1p5()
            .rounded(px(10.))
            .cursor_pointer()
            .border_2()
            .border_color(if picked { tok::accent_primary() } else { rgba(0x00000000).into() })
            .when(picked, |d| d.bg(tok::bg_selected()))
            .hover(|d| d.bg(tok::bg_raised()))
            .child(canvas(move |b, _, _| {
                bounds.borrow_mut().insert(key.clone(), b);
            }, |_, _, _, _| {}).absolute().size_full())
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.pick_card(&pdir, id, ev.modifiers, cx);
            }))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() >= 2 {
                    let dir = dir.clone();
                    this.play(&dir, id, cx);
                }
            }))
            .child(thumb)
            .child(match renaming {
                Some(input) => TextInput::new(&input).small().into_any_element(),
                None => div()
                    .px_0p5()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::MEDIUM)
                    .child(item.title.clone())
                    .into_any_element(),
            })
            .child(
                div()
                    .px_0p5()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .text_color(tok::text_tertiary())
                    .child(match workspace_name {
                        Some(p) => format!("{} · {p}", detail.join(" · ")),
                        None => detail.join(" · "),
                    }),
            )
            .when(item.kind == Kind::StudioTake, |d| {
                let open = self.inside_open.contains(&item.id);
                let it = item.clone();
                d.child(
                    div()
                        .id(("lib-inside", n))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_0p5()
                        .text_size(px(12.))
                        .text_color(tok::text_secondary())
                        .cursor_pointer()
                        .hover(|d| d.text_color(tok::text()))
                        .child(ui::icon(if open { "chevron-down" } else { "chevron-right" }).size(px(12.)))
                        .child(match self.inside.get(&item.id).and_then(Option::as_ref) {
                            Some(p) => format!("{} parts", p.len()),
                            None => "What's inside".to_string(),
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_inside(&it, cx)
                        })),
                )
                .children(self.inside_list(item, n, cx))
            })
            .context_menu(menu)
            .into_any_element()
    }

    fn empty(&self, searching: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .size_full()
            .p_8()
            .child(ui::icon("library").size(px(36.)).text_color(tok::text_tertiary()))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(if searching {
                "Nothing matches"
            } else {
                "No recordings yet"
            }))
            .child(
                div()
                    .max_w(px(380.))
                    .text_center()
                    .text_color(tok::text_secondary())
                    .child(if searching {
                        "Try other words, or another kind."
                    } else {
                        "Recordings, clips and multitrack audio from this workspace appear here as soon as they finish."
                    }),
            )
            .when(!searching, |d| {
                d.child(
                    Button::new("lib-empty-add")
                        .label("Add existing recordings…")
                        .small()
                        .outline()
                        .on_click(cx.listener(|this, _, window, cx| this.add_existing(window, cx))),
                )
            })
    }

    fn trash_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let items: Vec<&Item> = self.confirm_trash.iter().filter_map(|(p, id)| self.indexes.get(p)?.get(*id)).collect();
        let item = *items.first()?;
        let files: usize = items.iter().map(|i| i.files.iter().filter(|f| f.path.exists()).count()).sum();
        let title = if items.len() == 1 { format!("Move “{}” to the {}?", item.title, trash_name()) } else { format!("Move {} items to the {}?", items.len(), trash_name()) };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(tok::scrim())
                // a sheet: nothing behind it takes the mouse (scrolling here never pans the canvas)
                .occlude()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(400.))
                        .p_4()
                        .rounded(px(14.))
                        .bg(tok::bg_panel())
                        .border_1()
                        .border_color(tok::border_strong())
                        .shadow_lg()
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(div().text_color(tok::text_secondary()).child(format!(
                            "Its {} go to the {}, where you can put {} back. It leaves the Library.",
                            if files == 1 { "file".to_string() } else { format!("{files} files") },
                            trash_name(),
                            if files == 1 { "it" } else { "them" },
                        )))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(Button::new("lib-trash-cancel").label("Cancel").small().ghost().on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.confirm_trash.clear();
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    Button::new("lib-trash-ok")
                                        .label(format!("Move to {}", trash_name()))
                                        .small()
                                        .danger()
                                        .on_click(cx.listener(|this, _, _, cx| this.trash(cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn player(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.playing.as_ref()?;
        // the preview follows the picture's shape
        let (w, h) = p.player.size();
        if w > 0 && h > 0 {
            let scale = (1280.0 / w as f32).min(720.0 / h as f32).min(1.0);
            p.preview.set_size(Some(((w as f32 * scale) as u32, (h as f32 * scale) as u32)));
        }
        let status = p.player.status();
        let playing = status.state == lightair_engine::media::MediaState::Playing;
        let fraction =
            if status.duration_ms > 0 { (status.time_ms as f32 / status.duration_ms as f32).clamp(0.0, 1.0) } else { 0.0 };
        let bar = p.bar.clone();
        let frame = p.preview.latest_frame();
        let sound_only = w == 0 || h == 0;
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(tok::scrim())
                // a sheet: nothing behind it takes the mouse (scrolling here never pans the canvas)
                .occlude()
                .child(
                    div()
                        .id("lib-player")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(880.))
                        .p_3()
                        .rounded(px(14.))
                        .bg(tok::bg_panel())
                        .border_1()
                        .border_color(tok::border_strong())
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(p.title.clone()))
                                .child(ui::icon_button("lib-player-close", "x", tok::text_secondary()).on_click(cx.listener(
                                    |this, _, _, cx| this.close_player(cx),
                                ))),
                        )
                        .child(
                            div()
                                .relative()
                                .w_full()
                                .h(px(480.))
                                .rounded(px(8.))
                                .overflow_hidden()
                                .bg(rgb(0x000000))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(sound_only, |d| d.child(ui::icon("audio-lines").size(px(40.)).text_color(tok::text_tertiary())))
                                .when_some(frame.filter(|_| !sound_only), |d, f| {
                                    d.child(crate::frame_view::frame(f).absolute().size_full().object_fit(ObjectFit::Contain))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    ui::icon_button("lib-player-play", if playing { "pause" } else { "play" }, tok::text())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(p) = &this.playing {
                                                p.player.toggle();
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id("lib-player-bar")
                                        .relative()
                                        .flex_1()
                                        .h(px(16.))
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .child(
                                            canvas(move |b, _, _| *bar.borrow_mut() = Some(b), |_, _, _, _| {})
                                                .absolute()
                                                .size_full(),
                                        )
                                        .child(
                                            div()
                                                .relative()
                                                .w_full()
                                                .h(px(4.))
                                                .rounded_full()
                                                .bg(tok::bg_raised())
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .left_0()
                                                        .top_0()
                                                        .h_full()
                                                        .w(relative(fraction))
                                                        .rounded_full()
                                                        .bg(tok::accent_primary()),
                                                ),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                                let Some(p) = &this.playing else { return };
                                                let Some(b) = *p.bar.borrow() else { return };
                                                let x = f32::from(ev.position.x - b.origin.x) / f32::from(b.size.width).max(1.0);
                                                let d = p.player.status().duration_ms;
                                                p.player.seek((x.clamp(0.0, 1.0) * d as f32) as i64);
                                                cx.notify();
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_family(crate::ui::MONO)
                                        .text_color(tok::text_secondary())
                                        .child(format!(
                                            "{} / {}",
                                            duration_label(status.time_ms.max(0) as u64),
                                            duration_label(status.duration_ms.max(0) as u64)
                                        )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// What the OS calls its bin.
fn trash_name() -> &'static str {
    if cfg!(windows) { "Recycle Bin" } else { "Trash" }
}

impl Render for Library {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // the cards drawn this time say where they are
        self.card_bounds.borrow_mut().clear();
        if self.seen_version != Some(self.studio.read(cx).library_version) {
            self.reload(cx);
        }
        if self.playing.is_some() {
            window.request_animation_frame();
        }
        let shown = self.shown(cx);
        let searching = !self.search.read(cx).value().is_empty() || self.kind.is_some();
        let title = match &self.selected {
            None => "All recordings".to_string(),
            Some(dir) => self.workspaces.iter().find(|p| &p.dir == dir).map(|p| p.name.clone()).unwrap_or_default(),
        };
        let size = match &self.selected {
            None => self.sizes.values().sum(),
            Some(d) => self.sizes.get(d).copied().unwrap_or(0),
        };

        // cards, under their day
        let mut body = div().flex().flex_col().gap_4().p_4();
        let mut day = String::new();
        let mut row: Vec<AnyElement> = Vec::new();
        for (n, (dir, item)) in shown.iter().enumerate() {
            let d = day_label(item.started_ms);
            if d != day {
                if !row.is_empty() {
                    body = body.child(div().flex().flex_wrap().gap_3().children(std::mem::take(&mut row)));
                }
                body = body.child(
                    div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(tok::text_secondary()).child(d.clone()),
                );
                day = d;
            }
            row.push(self.card(dir, item, n, cx));
        }
        if !row.is_empty() {
            body = body.child(div().flex().flex_wrap().gap_3().children(row));
        }

        div()
            .relative()
            .flex()
            .size_full()
            .bg(tok::bg_app())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    if this.playing.is_some() {
                        this.close_player(cx);
                    } else if !this.picked.is_empty() && this.confirm_trash.is_empty() {
                        this.picked.clear();
                        cx.notify();
                    } else if !std::mem::take(&mut this.confirm_trash).is_empty() {
                        cx.notify();
                    }
                } else if ev.keystroke.key == "space"
                    && let Some(p) = &this.playing
                {
                    p.player.toggle();
                    cx.notify();
                }
            }))
            .child(self.sidebar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .px_4()
                            .pt_3()
                            .pb_2()
                            .border_b_1()
                            .border_color(tok::border())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(title))
                                    .when(size > 0, |d| d.child(div().text_size(px(12.)).text_color(tok::text_tertiary()).child(bytes_label(size))))
                                    .child(div().flex_1())
                                    .child(div().w(px(280.)).child(TextInput::new(&self.search).small())),
                            )
                            .child(self.filters(cx)),
                    )
                    .children(self.picked_bar(cx))
                    .child({
                        let bounds = self.body_bounds.clone();
                        div()
                            .id("lib-body")
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(canvas(move |b, _, _| *bounds.borrow_mut() = Some(b), |_, _, _, _| {}).absolute().size_full())
                            .map(|d| if shown.is_empty() { d.child(self.empty(searching, cx)) } else { d.child(body) })
                            .children(self.marquee_overlay())
                            // the space between cards: a box to pick with
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                this.begin_marquee(ev.position, ev.modifiers);
                                cx.notify();
                            }))
                            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                                if this.marquee.is_some() {
                                    this.marquee_to(ev.position, cx);
                                }
                            }))
                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_marquee(cx)))
                            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_marquee(cx)))
                    })
                    .when_some(self.note.clone(), |d, note| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_4()
                                .py_1p5()
                                .border_t_1()
                                .border_color(tok::border())
                                .text_size(px(12.))
                                .text_color(tok::text_secondary())
                                .child(div().flex_1().child(note))
                                .child(ui::icon_button("lib-note-close", "x", tok::text_tertiary()).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.note = None;
                                        cx.notify();
                                    },
                                ))),
                        )
                    }),
            )
            .children(self.trash_dialog(cx))
            .children(self.player(cx))
    }
}

#[cfg(test)]
mod tests {
    // (not `super::*`: gpui's own `test` macro would come with it)
    use super::{Local, bytes_label, day_label, duration_label};

    #[test]
    fn labels_read_naturally() {
        assert_eq!(duration_label(65_000), "1:05");
        assert_eq!(duration_label(3_723_000), "1:02:03");
        assert_eq!(bytes_label(812_000_000), "812 MB");
        assert_eq!(bytes_label(209_362), "210 KB");
        assert_eq!(bytes_label(3_400_000_000), "3.4 GB");
        assert_eq!(day_label(Local::now().timestamp_millis()), "Today");
    }
}
