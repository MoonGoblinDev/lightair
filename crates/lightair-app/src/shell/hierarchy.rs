//! The Hierarchy (docs/07-scenes-objects-canvas.md, "Hierarchy"): the scene's objects as a tree.
//! Empties hold other objects, any object can hold more, and rows drag above, below or into one
//! another. The list shows the front first, a parent above the objects it holds.

use std::collections::HashSet;

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_core::{Clip, Command, Id, Object, Layout, Workspace, Scene};

use super::StudioWindow;
use super::assets::DraggedAsset;
use crate::rows::{DraggedRow, RowTarget};
use crate::ui::{self, tok};

/// Where a dragged row lands relative to the row under the pointer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum TreeDrop {
    /// In front of it (above it in the list), beside it.
    Above,
    /// Inside it, in front of what it holds.
    Into,
    /// Behind it (below it in the list), beside it.
    Below,
}

const ROW: f32 = 30.0;
const INDENT: f32 = 14.0;

/// The rows to show, front first: (object, depth, holds objects).
fn rows<'a>(scene: &'a Scene, collapsed: &HashSet<Id>) -> Vec<(&'a Object, usize, bool)> {
    fn visit<'a>(
        scene: &'a Scene,
        parent: Option<Id>,
        depth: usize,
        collapsed: &HashSet<Id>,
        out: &mut Vec<(&'a Object, usize, bool)>,
    ) {
        let kids: Vec<&Object> = scene.children(parent).collect();
        for k in kids.into_iter().rev() {
            let holds = scene.children(Some(k.id)).next().is_some();
            out.push((k, depth, holds));
            if holds && !collapsed.contains(&k.id) {
                visit(scene, Some(k.id), depth + 1, collapsed, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(scene, None, 0, collapsed, &mut out);
    out
}

/// The command for dropping `object` at `zone` of `target` (`None`: below everything, at the
/// top level, behind the rest). `None` when it would go nowhere or inside itself.
pub(super) fn drop_command(
    scene: &Scene,
    object: Id,
    target: Option<(Id, TreeDrop)>,
    collapsed: &HashSet<Id>,
) -> Option<Command> {
    let (parent, index) = match target {
        None => (None, Some(0)),
        Some((t, zone)) => {
            if t == object || scene.is_under(t, object) {
                return None;
            }
            let t_object = scene.object(t)?;
            // Siblings without the dragged object: indices as they'll be once it's lifted out.
            let siblings: Vec<Id> = scene.children(t_object.parent).map(|l| l.id).filter(|id| *id != object).collect();
            let at = siblings.iter().position(|id| *id == t)?;
            let open_parent = scene.children(Some(t)).next().is_some() && !collapsed.contains(&t);
            match zone {
                TreeDrop::Into => (Some(t), None),
                // Right under an open parent's row: its first (frontmost) child.
                TreeDrop::Below if open_parent => (Some(t), None),
                TreeDrop::Above => (t_object.parent, Some(at + 1)),
                TreeDrop::Below => (t_object.parent, Some(at)),
            }
        }
    };
    Some(Command::ReparentObject { scene: scene.id, object, parent, index })
}

impl StudioWindow {
    pub(super) fn hierarchy_panel(&self, workspace: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        let studio = self.studio.read(cx);
        let canvas = self.canvas_id(workspace);
        let scene = studio.selected_scene.and_then(|id| workspace.scene(id)).cloned();
        let selection = studio.selection();
        let col = div().flex().flex_col().size_full().pt_1();
        let Some(scene) = scene else {
            return col.child(div().px_3().text_color(tok::text_tertiary()).child("No scene selected"));
        };
        let scene_id = scene.id;
        let dragging = cx.has_active_drag();
        let hint = self.tree_drop.filter(|_| dragging);
        let list = rows(&scene, &self.collapsed_objects);
        // the rows' order, for ⇧-click runs
        let order: std::sync::Arc<Vec<Id>> = std::sync::Arc::new(list.iter().map(|(l, _, _)| l.id).collect());
        let shell = cx.entity().downgrade();
        let mut items = Vec::new();
        for (i, (object, depth, holds)) in list.into_iter().enumerate() {
            let id = object.id;
            let kind = object.source.and_then(|i| workspace.sources.get(&i)).map(|i| i.kind.clone()).unwrap_or_default();
            let prefab = object.prefab.is_some();
            let icon = if prefab {
                "package"
            } else if object.is_empty() {
                "box"
            } else {
                ui::source_icon(&kind)
            };
            let own_visible = object.layouts.get(&canvas).is_some_and(|l| l.visible);
            let shown = scene.shown(id, canvas);
            let locked = object.locked;
            let inherited_lock = !locked && scene.locked(id);
            let is_sel = selection.contains(&id);
            let target = RowTarget::Object { scene: scene_id, object: id };
            let dragged = DraggedRow { target, name: object.name.clone().into(), icon };
            let name = object.name.clone();
            let collapsed = self.collapsed_objects.contains(&id);
            let zone = hint.filter(|(t, _)| *t == id).map(|(_, z)| z);
            // Prefab instances' names in the accent colour, as in Unity.
            let text = match (shown, prefab) {
                (false, _) => tok::text_tertiary(),
                (true, true) => tok::prefab(),
                (true, false) => tok::text(),
            };
            let row = div()
                .id(("tree", i))
                .relative()
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(ROW))
                .mx_1()
                .pr_1()
                .pl(px(6.0 + depth as f32 * INDENT))
                .rounded(px(5.))
                .cursor_pointer()
                .when(is_sel, |d| d.bg(tok::bg_selected()))
                .when(!is_sel, |d| d.hover(|d| d.bg(tok::bg_raised())))
                .when(zone == Some(TreeDrop::Into), |d| d.border_1().border_color(tok::accent_primary()))
                .on_click({
                    let order = order.clone();
                    cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    let m = ev.modifiers();
                    // ⌘: add or take away; ⇧: the run of rows from the one picked to this one
                    if m.platform || m.control {
                        this.toggle_selected(id, cx);
                        return;
                    }
                    if m.shift {
                        let anchor = this.studio.read(cx).selected_object;
                        match anchor.and_then(|a| order.iter().position(|x| *x == a)).zip(order.iter().position(|x| *x == id)) {
                            Some((a, b)) => {
                                let run: Vec<Id> = order[a.min(b)..=a.max(b)].to_vec();
                                this.studio.update(cx, |s, cx| {
                                    for l in run {
                                        s.add_selected(l);
                                    }
                                    cx.notify();
                                });
                            }
                            None => this.toggle_selected(id, cx),
                        }
                        return;
                    }
                    this.select_object(Some(id), cx);
                    if ev.click_count() == 2 {
                        this.start_rename(target, &name, window, cx);
                    }
                })
                })
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.row_right_down(id, cx)))
                .on_drag(dragged, |row, _, _, cx| cx.new(|_| row.clone()))
                .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<DraggedRow>, _, cx| {
                    if !ev.bounds.contains(&ev.event.position) {
                        return;
                    }
                    let f = (ev.event.position.y - ev.bounds.origin.y) / ev.bounds.size.height;
                    let zone = if f < 0.28 {
                        TreeDrop::Above
                    } else if f > 0.72 {
                        TreeDrop::Below
                    } else {
                        TreeDrop::Into
                    };
                    if this.tree_drop != Some((id, zone)) {
                        this.tree_drop = Some((id, zone));
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener(move |this, row: &DraggedRow, _, cx| {
                    let zone = this.tree_drop.filter(|(t, _)| *t == id).map_or(TreeDrop::Into, |(_, z)| z);
                    this.tree_drop = None;
                    this.drop_on_tree(row, Some((id, zone)), cx);
                }))
                // Assets dropped here: placed above, below or into this row.
                .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<DraggedAsset>, _, cx| {
                    if !ev.bounds.contains(&ev.event.position) {
                        return;
                    }
                    let f = (ev.event.position.y - ev.bounds.origin.y) / ev.bounds.size.height;
                    let zone = if f < 0.28 {
                        TreeDrop::Above
                    } else if f > 0.72 {
                        TreeDrop::Below
                    } else {
                        TreeDrop::Into
                    };
                    if this.tree_drop != Some((id, zone)) {
                        this.tree_drop = Some((id, zone));
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener(move |this, a: &DraggedAsset, _, cx| {
                    let zone = this.tree_drop.filter(|(t, _)| *t == id).map_or(TreeDrop::Into, |(_, z)| z);
                    this.tree_drop = None;
                    this.drop_asset_on_tree(&a.0, Some((id, zone)), cx);
                }))
                // Fold arrow, or room for one.
                .child(div().id(("fold", i)).size(px(14.)).flex().items_center().justify_center().when(holds, |d| {
                    d.child(
                        ui::icon(if collapsed { "chevron-right" } else { "chevron-down" })
                            .size(px(12.))
                            .text_color(tok::text_tertiary()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if !this.collapsed_objects.remove(&id) {
                            this.collapsed_objects.insert(id);
                        }
                        cx.notify();
                    }))
                }))
                .child(ui::icon(icon).size(px(14.)).text_color(match (shown, prefab) {
                    (false, _) => tok::text_tertiary(),
                    (true, true) => tok::prefab(),
                    (true, false) => tok::text_secondary(),
                }))
                .child(self.row_label(target, &object.name, text))
                .child(
                    ui::icon_button(
                        ("lock", i),
                        if locked || inherited_lock { "lock" } else { "lock-open" },
                        if locked { tok::text() } else { tok::text_tertiary() },
                    )
                    .when(!locked && !is_sel, |d| d.opacity(if inherited_lock { 0.5 } else { 0.0 }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.exec(Command::SetObjectLocked { scene: scene_id, object: id, locked: !locked }, cx)
                    })),
                )
                .child(
                    ui::icon_button(
                        ("eye", i),
                        if own_visible { "eye" } else { "eye-off" },
                        if own_visible { tok::text_secondary() } else { tok::text_tertiary() },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.exec(
                            Command::SetObjectVisible { scene: scene_id, object: id, canvas, visible: !own_visible },
                            cx,
                        )
                    })),
                )
                // Drop line: above or below this row, indented to where the object would land.
                .when_some(zone.filter(|z| *z != TreeDrop::Into), |d, z| {
                    let line = div()
                        .absolute()
                        .left(px(6.0 + depth as f32 * INDENT))
                        .right_1()
                        .h(px(2.))
                        .rounded_full()
                        .bg(tok::accent_primary());
                    d.child(if z == TreeDrop::Above { line.top(px(-1.)) } else { line.bottom(px(-1.)) })
                })
                .context_menu({
                    let shell = shell.clone();
                    move |m, window, cx| super::menus::object_menu(m, &shell, window, cx)
                });
            items.push(row);
        }
        let empty_scene = scene.objects.is_empty();
        col.child(
            div()
                .id("hierarchy-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .overflow_y_scroll()
                .children(items)
                .when(empty_scene, |d| {
                    d.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_size(px(12.))
                            .text_color(tok::text_tertiary())
                            .child("Empty scene — add an object with +"),
                    )
                })
                // Below the rows: to the top level, at the back.
                .child(
                    div()
                        .id("tree-end")
                        .flex_1()
                        .min_h(px(24.))
                        .drag_over::<DraggedRow>(|s, _, _, _| s.bg(tok::bg_raised()))
                        .on_click(cx.listener(|this, _, _, cx| this.select_object(None, cx)))
                        .on_drop(cx.listener(move |this, row: &DraggedRow, _, cx| {
                            this.tree_drop = None;
                            this.drop_on_tree(row, None, cx);
                        }))
                        .drag_over::<DraggedAsset>(|s, _, _, _| s.bg(tok::bg_raised()))
                        .on_drop(cx.listener(|this, a: &DraggedAsset, _, cx| {
                            this.tree_drop = None;
                            this.drop_asset_on_tree(&a.0, None, cx);
                        }))
                        .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.select_object(None, cx)))
                        .context_menu({
                            let shell = shell.clone();
                            move |m, _, cx| super::menus::blank_menu(m, &shell, cx)
                        }),
                ),
        )
    }

    fn drop_on_tree(&mut self, row: &DraggedRow, target: Option<(Id, TreeDrop)>, cx: &mut Context<Self>) {
        let RowTarget::Object { scene, object } = row.target else { return };
        let workspace = self.studio.read(cx).workspace().clone();
        let Some(sc) = workspace.scene(scene) else { return };
        if let Some(cmd) = drop_command(sc, object, target, &self.collapsed_objects) {
            // Show where it went.
            if let Some((parent, TreeDrop::Into)) = target {
                self.collapsed_objects.remove(&parent);
            }
            self.exec(cmd, cx);
        }
    }

    /// The Inspector for an empty: where its pivot is, how it turns and scales what it holds.
    pub(super) fn empty_inspector(
        &self,
        scene: &Scene,
        object: &Object,
        canvas: &lightair_core::Canvas,
        body: Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lo = object.layouts.get(&canvas.id).copied().unwrap_or(Layout::boxed(0., 0., 0., 0.));
        let (sid, lid, fid) = (scene.id, object.id, canvas.id);
        let set = move |f: &dyn Fn(&mut Layout)| {
            let mut l = lo;
            f(&mut l);
            Command::SetLayout { scene: sid, object: lid, canvas: fid, layout: l }
        };
        let count = scene.objects.iter().filter(|l| scene.is_under(l.id, lid)).count();
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .py_2()
            .child(ui::icon("box").size(px(16.)).text_color(tok::text_secondary()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(object.name.clone()),
            )
            .child(ui::icon_button("delete-empty", "trash", tok::text_secondary()).on_click(
                cx.listener(move |this, _, _, cx| this.exec(Command::RemoveObject { scene: sid, object: lid }, cx)),
            ));
        let scale = |l: &mut Layout, by: f32| {
            l.scale_x = (l.scale_x * by).clamp(0.05, 20.0);
            l.scale_y = (l.scale_y * by).clamp(0.05, 20.0);
        };
        let at = move |f: fn(&mut Layout, f32)| {
            move |v: f32| {
                let mut l = lo;
                f(&mut l, v);
                set(&|m| *m = l)
            }
        };
        let letter = |text: &'static str| div().w(px(10.)).child(text).into_any_element();
        let grid = div()
            .grid()
            .grid_cols(2)
            .gap_1()
            .child(self.number("ex", letter("X"), lo.x, "", 1.0, at(|l, v| l.x = v), cx))
            .child(self.number("ey", letter("Y"), lo.y, "", 1.0, at(|l, v| l.y = v), cx))
            .child(self.number(
                "er",
                ui::icon("rotate-cw").size(px(11.)).into_any_element(),
                lo.rotation,
                "°",
                0.5,
                at(|l, v| l.rotation = v),
                cx,
            ))
            .child(self.field(
                "es",
                "%",
                if (lo.scale_x - lo.scale_y).abs() < 0.001 {
                    format!("{:.0}", lo.scale_x * 100.0)
                } else {
                    format!("{:.0}×{:.0}", lo.scale_x * 100.0, lo.scale_y * 100.0)
                },
                set(&|l| scale(l, 1.0 / 1.1)),
                set(&|l| scale(l, 1.1)),
                cx,
            ));
        let reset = set(&|l| {
            l.rotation = 0.0;
            l.scale_x = 1.0;
            l.scale_y = 1.0;
        });
        let transform = self
            .section(
                "Transform",
                Some(
                    Button::new("empty-reset")
                        .label("Reset")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| this.exec(reset.clone(), cx)))
                        .into_any_element(),
                ),
                cx,
            )
            .child(grid)
            .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(format!(
                "Holds {count} object{}. Moving, turning or scaling it carries them.",
                if count == 1 { "" } else { "s" }
            )));
        let actions = div()
            .flex()
            .gap_1()
            .child(
                Button::new("empty-child")
                    .label("Add empty inside")
                    .xsmall()
                    .outline()
                    .on_click(cx.listener(move |this, _, window, cx| this.create_empty(Some(lid), window, cx))),
            )
            .when(count > 0, |d| {
                d.child(
                    Button::new("empty-ungroup")
                        .label("Ungroup")
                        .xsmall()
                        .outline()
                        .on_click(cx.listener(|this, _, _, cx| this.ungroup_selection(cx))),
                )
            });
        body.child(header).child(transform).child(actions).into_any_element()
    }

    // ------------------------------------------------------------ copy, paste, duplicate

    /// The selected objects (with what's under them) as a clip, also put on the clipboard.
    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) -> Option<Clip> {
        let studio = self.studio.read(cx);
        let clip = Clip::copy(studio.workspace(), studio.selected_scene?, &studio.selection())?;
        cx.write_to_clipboard(ClipboardItem::new_string(clip.to_text()));
        Some(clip)
    }

    pub(super) fn cut_selection(&mut self, cx: &mut Context<Self>) {
        if self.copy_selection(cx).is_some() {
            self.delete_selection(cx);
        }
    }

    /// Pastes the clipboard's objects next to the selection (in front of it, beside it), or at
    /// the front of the scene.
    pub(super) fn paste_objects(&mut self, cx: &mut Context<Self>) {
        let Some(clip) = cx.read_from_clipboard().and_then(|c| c.text()).and_then(|t| Clip::from_text(&t)) else {
            return;
        };
        self.place_clip(&clip, "Paste", cx);
    }

    /// Copies the selection in place, in front of it (⌘D), leaving the clipboard alone.
    pub(super) fn duplicate_selection(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let Some(scene) = studio.selected_scene else { return };
        let Some(clip) = Clip::copy(studio.workspace(), scene, &studio.selection()) else { return };
        self.place_clip(&clip, "Duplicate", cx);
    }

    fn place_clip(&mut self, clip: &Clip, label: &str, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        // Beside the frontmost selected object, in front of it.
        let selection = studio.selection();
        let anchor = scene.objects.iter().rev().find(|l| selection.contains(&l.id));
        let (parent, index) = match anchor {
            Some(a) => {
                let siblings: Vec<Id> = scene.children(a.parent).map(|l| l.id).collect();
                (a.parent, siblings.iter().position(|s| *s == a.id).map(|i| i + 1))
            }
            None => (None, None),
        };
        let (commands, roots) = clip.paste(&workspace, scene.id, parent, index);
        if commands.is_empty() {
            return;
        }
        let n = roots.len();
        let label = if n == 1 { label.to_string() } else { format!("{label} {n} objects") };
        self.exec(Command::Batch { label, commands }, cx);
        self.studio.update(cx, |s, cx| {
            s.selected_object = roots.first().copied();
            s.also_selected = roots.iter().skip(1).copied().collect();
            cx.notify();
        });
    }

    /// A new empty, inside `parent` (or at the top level, at the canvas centre), selected and
    /// ready to rename.
    pub(super) fn create_empty(&mut self, parent: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        // At the parent's pivot, or the middle of each canvas.
        let at: Vec<(Id, (f32, f32))> = workspace
            .canvases
            .iter()
            .map(|f| (f.id, if parent.is_some() { (0.0, 0.0) } else { (f.width as f32 / 2.0, f.height as f32 / 2.0) }))
            .collect();
        let mut object = Object::empty("Empty", at);
        object.parent = parent;
        let (id, scene_id) = (object.id, scene.id);
        if let Some(p) = parent {
            self.collapsed_objects.remove(&p);
        }
        self.exec(Command::AddObject { scene: scene_id, object, index: None }, cx);
        self.select_object(Some(id), cx);
        self.start_rename(RowTarget::Object { scene: scene_id, object: id }, "Empty", window, cx);
    }

    /// Puts the selected objects in a new empty at the middle of their box (⌘G).
    pub(super) fn group_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        // Outermost selected objects, back to front as they're stored.
        let selection = studio.selection();
        let picked: Vec<&Object> = scene
            .objects
            .iter()
            .filter(|l| selection.contains(&l.id) && !selection.iter().any(|a| scene.is_under(l.id, *a)))
            .collect();
        if picked.is_empty() {
            return;
        }
        // Their shared parent, if they have one; otherwise the top level.
        let parent = picked[0].parent.filter(|p| picked.iter().all(|l| l.parent == Some(*p)));
        // The pivot: the middle of their frames, per canvas, in the parent's space.
        let mut layouts = Vec::new();
        for f in &workspace.canvases {
            let frames: Vec<Layout> = picked.iter().filter_map(|l| self.frame_of(scene, l, f.id)).collect();
            let Some((x0, y0, x1, y1)) = frames
                .iter()
                .map(crate::geometry::bounds)
                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
            else {
                continue;
            };
            let Some(frame) = scene.frame_of(parent, f.id) else { continue };
            let (px_, py_) = frame.unapply(((x0 + x1) / 2.0, (y0 + y1) / 2.0));
            layouts.push((f.id, (px_, py_)));
        }
        let mut group = Object::empty("Group", layouts);
        group.parent = parent;
        let gid = group.id;
        // In front of the frontmost picked object, among the parent's children.
        let siblings: Vec<Id> = scene.children(parent).map(|l| l.id).collect();
        let front = picked.iter().filter_map(|l| siblings.iter().position(|s| *s == l.id)).max();
        let mut commands = vec![Command::AddObject { scene: scene.id, object: group, index: front.map(|i| i + 1) }];
        commands.extend(picked.iter().map(|l| Command::ReparentObject {
            scene: scene.id,
            object: l.id,
            parent: Some(gid),
            index: None,
        }));
        let scene_id = scene.id;
        self.exec(Command::Batch { label: "Group objects".into(), commands }, cx);
        self.select_object(Some(gid), cx);
        self.start_rename(RowTarget::Object { scene: scene_id, object: gid }, "Group", window, cx);
    }

    /// Takes the objects out of the selected empty and removes it (⇧⌘G). Objects that aren't
    /// empties keep what they hold.
    pub(super) fn ungroup_selection(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let Some(scene) = studio.selected_scene.and_then(|id| workspace.scene(id)) else { return };
        let mut commands = Vec::new();
        let mut freed = Vec::new();
        for id in studio.selection() {
            let Some(group) = scene.object(id).filter(|l| l.is_empty()) else { continue };
            let siblings: Vec<Id> = scene.children(group.parent).map(|l| l.id).collect();
            let at = siblings.iter().position(|s| *s == id).unwrap_or(0);
            // Back to front, each in turn in front of the last, where the empty was.
            for (k, child) in scene.children(Some(id)).enumerate() {
                commands.push(Command::ReparentObject {
                    scene: scene.id,
                    object: child.id,
                    parent: group.parent,
                    index: Some(at + 1 + k),
                });
                freed.push(child.id);
            }
            commands.push(Command::RemoveObject { scene: scene.id, object: id });
        }
        if commands.is_empty() {
            return;
        }
        self.exec(Command::Batch { label: "Ungroup".into(), commands }, cx);
        self.studio.update(cx, |s, cx| {
            s.selected_object = freed.first().copied();
            s.also_selected = freed.iter().skip(1).copied().collect();
            cx.notify();
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{TreeDrop, drop_command, rows};
    use lightair_core::{Command, Document, Id, Source, Object, Layout, Workspace, Scene};

    /// Back, Group{A, B}, Front — built through commands like the app does.
    fn scene() -> (Document, Id, [Id; 5]) {
        let mut doc = Document::new(Workspace::new("T"));
        let f = doc.workspace().primary_canvas().id;
        let source = Source::new("I", "color_source_v3", serde_json::json!({}));
        let iid = source.id;
        doc.apply(&Command::AddSource { source }).unwrap();
        let sc = Scene::new("S");
        let sid = sc.id;
        doc.apply(&Command::AddScene { scene: sc, index: None }).unwrap();
        let pic = |n: &str| Object::new(n, iid, [(f, Layout::boxed(0., 0., 10., 10.))]);
        let back = pic("Back");
        let group = Object::empty("Group", [(f, (0., 0.))]);
        let a = pic("A").under(group.id);
        let b = pic("B").under(group.id);
        let front = pic("Front");
        let ids = [back.id, group.id, a.id, b.id, front.id];
        for l in [back, group, a, b, front] {
            doc.apply(&Command::AddObject { scene: sid, object: l, index: None }).unwrap();
        }
        (doc, sid, ids)
    }

    fn names(doc: &Document, sid: Id) -> Vec<String> {
        doc.workspace().scene(sid).unwrap().objects.iter().map(|l| l.name.clone()).collect()
    }

    #[test]
    fn rows_show_the_front_first_with_children_under_their_parent() {
        let (doc, sid, [_, group, ..]) = scene();
        let sc = doc.workspace().scene(sid).unwrap();
        let shown = |c: &HashSet<Id>| {
            rows(sc, c).iter().map(|(l, d, _)| format!("{}{}", " ".repeat(*d), l.name)).collect::<Vec<_>>()
        };
        assert_eq!(shown(&HashSet::new()), ["Front", "Group", " B", " A", "Back"]);
        assert_eq!(shown(&HashSet::from([group])), ["Front", "Group", "Back"]);
    }

    #[test]
    fn drops_land_above_below_and_into() {
        let (mut doc, sid, [back, group, a, b, front]) = scene();
        let none = HashSet::new();
        let drop = |doc: &mut Document, object: Id, target: Option<(Id, TreeDrop)>| {
            let sc = doc.workspace().scene(sid).unwrap().clone();
            let cmd = drop_command(&sc, object, target, &none).expect("a move");
            doc.apply(&cmd).unwrap();
            assert!(doc.workspace().scene(sid).unwrap().is_well_formed());
        };
        // Front above B (in front of B, inside the group).
        drop(&mut doc, front, Some((b, TreeDrop::Above)));
        assert_eq!(names(&doc, sid), ["Back", "Group", "A", "B", "Front"]);
        assert_eq!(doc.workspace().scene(sid).unwrap().object(front).unwrap().parent, Some(group));
        // Back below A (behind A, inside the group).
        drop(&mut doc, back, Some((a, TreeDrop::Below)));
        assert_eq!(names(&doc, sid), ["Group", "Back", "A", "B", "Front"]);
        // A into B.
        drop(&mut doc, a, Some((b, TreeDrop::Into)));
        assert_eq!(names(&doc, sid), ["Group", "Back", "B", "A", "Front"]);
        // Just below the open group's row: its frontmost child.
        drop(&mut doc, back, Some((group, TreeDrop::Below)));
        assert_eq!(names(&doc, sid), ["Group", "B", "A", "Front", "Back"]);
        // Into empty space: the top level, at the back.
        drop(&mut doc, front, None);
        assert_eq!(names(&doc, sid), ["Front", "Group", "B", "A", "Back"]);
    }

    #[test]
    fn a_object_cant_be_dropped_into_itself() {
        let (doc, sid, [_, group, a, ..]) = scene();
        let sc = doc.workspace().scene(sid).unwrap();
        assert!(drop_command(sc, group, Some((a, TreeDrop::Into)), &HashSet::new()).is_none());
        assert!(drop_command(sc, group, Some((group, TreeDrop::Above)), &HashSet::new()).is_none());
    }
}
