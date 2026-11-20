//! Right-click menus (docs/05-interface-shell.md, "Context menus"). Right-clicking a object, a
//! scene or empty space selects what's under the pointer first, then offers what can be done to
//! it. Items run the same actions as the keyboard, so each menu shows its shortcuts.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use lightair_model::{Clip, Command, Id, Layout, Scene};

use super::*;
use crate::rows::RowTarget;
use crate::ui;

/// What a menu needs to know about the selection when it opens.
struct ObjectFacts {
    count: usize,
    empty: bool,
    has_parent: bool,
    visible: bool,
    locked: bool,
    /// The source whose source (display, camera, window…) can be changed.
    device: Option<Id>,
}

fn clip_on_clipboard(cx: &App) -> bool {
    cx.read_from_clipboard().and_then(|c| c.text()).is_some_and(|t| Clip::from_text(&t).is_some())
}

impl StudioWindow {
    fn object_facts(&self, cx: &App) -> Option<ObjectFacts> {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace();
        let scene = studio.selected_scene.and_then(|id| workspace.scene(id))?;
        let object = studio.selected_object.and_then(|id| scene.object(id))?;
        let canvas = self.canvas_id(workspace);
        let device = object
            .source
            .and_then(|i| workspace.sources.get(&i))
            .filter(|i| super::source::device_key(&i.kind, &i.settings).is_some())
            .map(|i| i.id);
        Some(ObjectFacts {
            count: studio.selection().len(),
            empty: object.is_empty(),
            has_parent: object.parent.is_some(),
            visible: object.layouts.get(&canvas).is_some_and(|l| l.visible),
            locked: object.locked,
            device,
        })
    }

    // ------------------------------------------------------------ what the menus do

    /// A copy of a scene right after it: the same sources, new objects.
    pub(super) fn duplicate_scene(&mut self, scene: Id, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(index) = workspace.scene_index(scene) else { return };
        let original = &workspace.scenes[index];
        let mut copy = Scene::new(format!("{} copy", original.name));
        let ids: std::collections::HashMap<Id, Id> = original.objects.iter().map(|l| (l.id, Id::new())).collect();
        copy.objects = original
            .objects
            .iter()
            .map(|l| lightair_model::Object { id: ids[&l.id], parent: l.parent.map(|p| ids[&p]), ..l.clone() })
            .collect();
        let id = copy.id;
        self.exec(Command::AddScene { scene: copy, index: Some(index + 1) }, cx);
        self.studio.update(cx, |s, cx| {
            s.select_scene(id);
            cx.notify();
        });
    }

    /// Moves each selected object among its siblings: `to` maps (index, sibling count) to the
    /// new index.
    pub(super) fn arrange(&mut self, label: &str, to: impl Fn(usize, usize) -> usize, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        let selection = studio.selection();
        let commands: Vec<Command> = scene
            .objects
            .iter()
            .filter(|l| selection.contains(&l.id) && !selection.iter().any(|a| scene.is_under(l.id, *a)))
            .filter_map(|l| {
                let siblings: Vec<Id> = scene.children(l.parent).map(|s| s.id).collect();
                let at = siblings.iter().position(|s| *s == l.id)?;
                let new = to(at, siblings.len()).min(siblings.len() - 1);
                (new != at).then_some(Command::MoveObject { scene: scene.id, object: l.id, to: new })
            })
            .collect();
        if !commands.is_empty() {
            self.exec(Command::Batch { label: label.into(), commands }, cx);
        }
    }

    /// Shows or hides the selection (by the first object's state).
    pub(super) fn toggle_selection_visible(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let canvas = self.canvas_id(&workspace);
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        let selection = studio.selection();
        let visible = !selection
            .first()
            .and_then(|id| scene.object(*id))
            .is_some_and(|l| l.layouts.get(&canvas).is_some_and(|lo| lo.visible));
        let commands: Vec<Command> = selection
            .iter()
            .map(|object| Command::SetObjectVisible { scene: scene.id, object: *object, canvas, visible })
            .collect();
        if !commands.is_empty() {
            let label = if visible { "Show objects" } else { "Hide objects" };
            self.exec(Command::Batch { label: label.into(), commands }, cx);
        }
    }

    pub(super) fn toggle_selection_locked(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        let selection = studio.selection();
        let locked = !selection.first().and_then(|id| scene.object(*id)).is_some_and(|l| l.locked);
        let commands: Vec<Command> =
            selection.iter().map(|object| Command::SetObjectLocked { scene: scene.id, object: *object, locked }).collect();
        if !commands.is_empty() {
            let label = if locked { "Lock objects" } else { "Unlock objects" };
            self.exec(Command::Batch { label: label.into(), commands }, cx);
        }
    }

    /// Each selected object out of its parent, in front of it, staying put on the canvas.
    pub(super) fn move_out_of_parent(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        let commands: Vec<Command> = studio
            .selection()
            .iter()
            .filter_map(|id| {
                let parent = scene.object(scene.object(*id)?.parent?)?;
                let siblings: Vec<Id> = scene.children(parent.parent).map(|l| l.id).collect();
                let at = siblings.iter().position(|s| *s == parent.id)?;
                Some(Command::ReparentObject { scene: scene.id, object: *id, parent: parent.parent, index: Some(at + 1) })
            })
            .collect();
        if !commands.is_empty() {
            self.exec(Command::Batch { label: "Move out".into(), commands }, cx);
        }
    }

    /// The selected pictures fill the canvas (in the canvas being edited).
    pub(super) fn fill_canvas(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let canvas = self.current_canvas(&workspace).clone();
        let Some(scene) = studio.selected_scene else { return };
        let Some(sc) = workspace.scene(scene) else { return };
        let layouts: Vec<(Id, Layout)> = studio
            .selection()
            .iter()
            .filter_map(|id| sc.object(*id).filter(|l| !l.is_empty()))
            .filter_map(|l| {
                let world = sc.world(l.id, canvas.id)?;
                Some((
                    l.id,
                    Layout {
                        x: 0.0,
                        y: 0.0,
                        width: canvas.width as f32,
                        height: canvas.height as f32,
                        rotation: 0.0,
                        ..world
                    },
                ))
            })
            .collect();
        self.set_layouts(scene, canvas.id, layouts, "Fill canvas", cx);
    }

    /// Before a right-click menu opens on the canvas: select the picture under the pointer
    /// (keeping the selection if it's already in it), or nothing.
    pub(super) fn canvas_right_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(canvas) = self.stage_at(&workspace, ev.position) else { return };
        let Some(p) = self.to_canvas(&workspace, canvas, ev.position) else { return };
        let studio = self.studio.read(cx);
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        let hit = scene
            .objects
            .iter()
            .rev()
            .filter(|l| !l.is_empty())
            .find(|l| self.frame_of(scene, l, canvas).is_some_and(|lo| lo.visible && geo::contains(&lo, p)))
            .map(|l| l.id);
        let selection = studio.selection();
        match hit {
            Some(id) if selection.contains(&id) => {}
            Some(id) => self.select_object(Some(id), cx),
            None => self.select_object(None, cx),
        }
    }

    /// Before a row's menu opens: select the row unless it's already in the selection.
    pub(super) fn row_right_down(&mut self, object: Id, cx: &mut Context<Self>) {
        if !self.studio.read(cx).selection().contains(&object) {
            self.select_object(Some(object), cx);
        }
    }
}

/// The menu for the selected objects (right-clicked in the Hierarchy or on the canvas).
pub(super) fn object_menu(
    menu: PopupMenu,
    shell: &WeakEntity<StudioWindow>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(w) = shell.upgrade() else { return menu };
    let (facts, focus) = {
        let w = w.read(cx);
        (w.object_facts(cx), w.focus.clone())
    };
    let Some(f) = facts else { return blank_menu(menu, shell, cx) };
    let shell = shell.clone();
    let paste = clip_on_clipboard(cx);
    let mut menu = menu
        .action_context(focus)
        .menu_with_icon("Cut", ui::icon("scissors"), Box::new(CutSelection))
        .menu_with_icon("Copy", ui::icon("copy"), Box::new(CopySelection))
        .menu_with_icon_and_disabled("Paste", ui::icon("clipboard-paste"), Box::new(PasteObjects), !paste)
        .menu_with_icon("Duplicate", ui::icon("copy-plus"), Box::new(DuplicateSelection))
        .menu_with_icon("Delete", ui::icon("trash"), Box::new(DeleteSelection))
        .separator();
    if f.count == 1 {
        menu = menu.menu("Rename", Box::new(RenameSelection));
    }
    if let Some(source) = f.device.filter(|_| f.count == 1) {
        let s = shell.clone();
        menu =
            menu.item(PopupMenuItem::new("Change source…").icon(ui::icon("monitor")).on_click(move |_, window, cx| {
                if let Some(s) = s.upgrade() {
                    s.update(cx, |w, cx| w.open_source_picker(source, window, cx));
                }
            }));
    }
    menu = menu.separator().menu_with_icon("Group", ui::icon("group"), Box::new(GroupSelection)).menu_with_icon(
        "Create empty inside",
        ui::icon("box"),
        Box::new(CreateEmptyChild),
    );
    if f.empty {
        menu = menu.menu_with_icon("Ungroup", ui::icon("ungroup"), Box::new(UngroupSelection));
    }
    if f.has_parent {
        menu = menu.menu("Move out of parent", Box::new(MoveOutOfParent));
    }
    menu = menu
        .separator()
        .submenu("Arrange", window, cx, |m, _, _| {
            m.menu_with_icon("Bring to front", ui::icon("bring-to-front"), Box::new(BringToFront))
                .menu("Bring forward", Box::new(BringForward))
                .menu("Send backward", Box::new(SendBackward))
                .menu_with_icon("Send to back", ui::icon("send-to-back"), Box::new(SendToBack))
        })
        .separator()
        .menu_with_icon(
            if f.visible { "Hide" } else { "Show" },
            ui::icon(if f.visible { "eye-off" } else { "eye" }),
            Box::new(ToggleObjectVisible),
        )
        .menu_with_icon(
            if f.locked { "Unlock" } else { "Lock" },
            ui::icon(if f.locked { "lock-open" } else { "lock" }),
            Box::new(ToggleObjectLocked),
        );
    if !f.empty {
        menu = menu.separator().menu_with_icon("Fill canvas", ui::icon("maximize"), Box::new(FillCanvas));
    }
    if f.count == 1 {
        menu = menu.separator().menu_with_icon("Save as prefab", ui::icon("package-plus"), Box::new(SaveAsPrefab));
    }
    menu
}

/// The menu for empty space in the Hierarchy or on the canvas.
pub(super) fn blank_menu(menu: PopupMenu, shell: &WeakEntity<StudioWindow>, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let Some(w) = shell.upgrade() else { return menu };
    let focus = w.read(cx).focus.clone();
    let paste = clip_on_clipboard(cx);
    let s = shell.clone();
    menu.action_context(focus)
        .menu_with_icon_and_disabled("Paste", ui::icon("clipboard-paste"), Box::new(PasteObjects), !paste)
        .separator()
        .item(PopupMenuItem::new("Add object…").icon(ui::icon("plus")).on_click(move |_, window, cx| {
            if let Some(s) = s.upgrade() {
                s.update(cx, |w, cx| w.open_add_object(window, cx));
            }
        }))
        .menu_with_icon("Create empty", ui::icon("box"), Box::new(CreateEmpty))
        .separator()
        .menu("Select all", Box::new(SelectAll))
}

/// The menu for a scene in the Scenes list.
pub(super) fn scene_menu(
    scene: Id,
    menu: PopupMenu,
    shell: &WeakEntity<StudioWindow>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(w) = shell.upgrade() else { return menu };
    let workspace = w.read(cx).studio.read(cx).workspace().clone();
    let Some(index) = workspace.scene_index(scene) else { return menu };
    let count = workspace.scenes.len();
    let on_air = workspace.program == Some(scene);
    let name = workspace.scenes[index].name.clone();
    let s = shell.clone();
    let run = move |cmd: Command| {
        let s = s.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            if let Some(s) = s.upgrade() {
                s.update(cx, |w, cx| w.exec(cmd.clone(), cx));
            }
        }
    };
    let (s1, s2) = (shell.clone(), shell.clone());
    menu.item(
        PopupMenuItem::new("Go on air")
            .icon(ui::icon("radio"))
            .disabled(on_air)
            .on_click(run(Command::SetProgram { scene: Some(scene) })),
    )
    .separator()
    .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
        if let Some(s) = s1.upgrade() {
            s.update(cx, |w, cx| w.start_rename(RowTarget::Scene(scene), &name, window, cx));
        }
    }))
    .item(PopupMenuItem::new("Duplicate").icon(ui::icon("copy-plus")).on_click(move |_, _, cx| {
        if let Some(s) = s2.upgrade() {
            s.update(cx, |w, cx| w.duplicate_scene(scene, cx));
        }
    }))
    .item(
        PopupMenuItem::new("Delete")
            .icon(ui::icon("trash"))
            .disabled(count < 2)
            .on_click(run(Command::RemoveScene { scene })),
    )
    .separator()
    .item(
        PopupMenuItem::new("Move up")
            .icon(ui::icon("arrow-up"))
            .disabled(index == 0)
            .on_click(run(Command::MoveScene { scene, to: index.saturating_sub(1) })),
    )
    .item(
        PopupMenuItem::new("Move down")
            .icon(ui::icon("arrow-down"))
            .disabled(index + 1 >= count)
            .on_click(run(Command::MoveScene { scene, to: index + 1 })),
    )
}

/// The menu for empty space in the Scenes list.
pub(super) fn scenes_blank_menu(menu: PopupMenu, shell: &WeakEntity<StudioWindow>) -> PopupMenu {
    let (s1, s2) = (shell.clone(), shell.clone());
    menu.item(PopupMenuItem::new("Add scene").icon(ui::icon("plus")).on_click(move |_, _, cx| {
        if let Some(s) = s1.upgrade() {
            s.update(cx, |w, cx| w.add_scene(cx));
        }
    }))
    .item(PopupMenuItem::new("Import OBS scenes…").icon(ui::icon("import")).on_click(move |_, window, cx| {
        if let Some(s) = s2.upgrade() {
            s.update(cx, |w, cx| w.import_obs(window, cx));
        }
    }))
}
