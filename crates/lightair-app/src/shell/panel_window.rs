//! A dock panel in a window of its own: "Open in New Window" on its tab (right-click). The
//! panel moves, it isn't copied: it leaves the dock, and closing its window docks it again in
//! the region it came from. The canvas stays: its preview is drawn under the main window.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{BasePanelView, DockPlacement, PanelHandle};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use super::StudioWindow;
use crate::ui::tok;

/// The window's root: the panel, on the panel's background (window roots draw none: the
/// main window's preview shows through them).
pub(super) struct PanelWindow {
    panel: Arc<dyn BasePanelView>,
    home: Home,
}

/// Where a panel goes back to: the main window's dock, in the region it came from.
#[derive(Clone)]
struct Home {
    me: WeakEntity<StudioWindow>,
    main: AnyWindowHandle,
    region: DockPlacement,
}

impl Home {
    fn take_back(&self, panel: Arc<dyn BasePanelView>, cx: &mut App) {
        let (me, region) = (self.me.clone(), self.region);
        let _ = self.main.update(cx, |_, window, cx| {
            if let Some(me) = me.upgrade() {
                me.update(cx, |this, cx| this.dock_again(panel, region, window, cx));
            }
        });
    }
}

impl PanelWindow {
    /// Back in the main window's dock; this window closes.
    pub(super) fn back(&mut self, window: &mut Window, cx: &mut App) {
        self.home.take_back(self.panel.clone(), cx);
        window.remove_window();
    }
}

impl Render for PanelWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // its header, as in the dock: its title, what it shows beside it, its buttons
        let handle = PanelHandle::of(&self.panel);
        let title = handle.map(|h| h.title(window, cx));
        let suffix = handle.and_then(|h| h.title_suffix(window, cx));
        let buttons = handle.and_then(|h| h.toolbar_buttons(window, cx)).unwrap_or_default();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(tok::bg_panel())
            .text_color(tok::text())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .h(px(36.))
                    .px_3()
                    .border_b_1()
                    .border_color(tok::border())
                    .children(title)
                    .child(div().flex_1())
                    .children(suffix)
                    .children(buttons.into_iter().map(|b| b.xsmall().ghost()))
                    .child(
                        Button::new("panel-window-back")
                            .icon(crate::ui::icon("arrow-down-left"))
                            .xsmall()
                            .ghost()
                            .tooltip("Back to the main window")
                            .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().relative().child(div().absolute().size_full().child(self.panel.view())))
    }
}

/// What a panel's window is called.
fn title(name: &str) -> &'static str {
    match name {
        "layers" => "Hierarchy",
        "scenes" => "Scenes",
        "inspector" => "Inspector",
        "assets" => "Assets",
        "mixer" => "Audio Mixer",
        "outputs" => "Outputs",
        "chat" => "Chat",
        "activity" => "Activity",
        "stream" => "Stream",
        "automations" => "Automations",
        _ => "Panel",
    }
}

/// Whether a panel can have a window of its own (not the canvas: its preview is the main
/// window's).
pub(super) fn can_pop_out(name: &str) -> bool {
    name != "canvas"
}

impl StudioWindow {
    /// A tab's right-click menu: open its panel in a window of its own.
    pub(super) fn tab_menu(me: WeakEntity<Self>) -> gpui_kit::component::dock::TabMenu {
        std::rc::Rc::new(move |menu: PopupMenu, panel: Arc<dyn BasePanelView>, _: &mut Window, cx: &mut App| {
            if !can_pop_out(panel.panel_name(cx)) {
                return menu;
            }
            let me = me.clone();
            menu.item(PopupMenuItem::new("Open in New Window").on_click(move |_, window, cx| {
                if let Some(me) = me.upgrade() {
                    me.update(cx, |this, cx| this.pop_out(panel.clone(), window, cx));
                }
            }))
        })
    }

    /// The docked panel called `name` (`BasePanel::panel_name`), if it's in the dock.
    pub(super) fn docked_panel(&self, name: &str, cx: &App) -> Option<Arc<dyn BasePanelView>> {
        let area = self.dock.as_ref()?.read(cx);
        [DockPlacement::Center, DockPlacement::Left, DockPlacement::Right, DockPlacement::Bottom]
            .into_iter()
            .filter_map(|p| area.layout(p))
            .flat_map(|tree| tree.panels().collect::<Vec<_>>())
            .filter_map(|id| area.panel(id).cloned())
            .find(|panel| panel.panel_name(cx) == name)
    }

    /// Moves `panel` out of the dock into a window of its own.
    pub(super) fn pop_out(&mut self, panel: Arc<dyn BasePanelView>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = self.dock.clone() else { return };
        let id = panel.panel_id(cx);
        let name = panel.panel_name(cx);
        let Some(region) = dock.read(cx).region_of_panel(id) else { return };
        if !can_pop_out(name) {
            return;
        }
        dock.update(cx, |area, cx| area.remove_panel_view(id, window, cx));
        self.own_window(name, true, cx);
        let main = window.window_handle();
        let me = cx.entity().downgrade();
        // beside the main window, the size of a side panel
        let at = window.bounds();
        let size = size(px(380.), at.size.height.min(px(760.)));
        let origin = point(at.right() - size.width - px(40.), at.top() + px(80.));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size })),
            titlebar: Some(TitlebarOptions { title: Some(format!("{} · Lightair", title(name)).into()), ..Default::default() }),
            window_min_size: Some(gpui_kit::size(px(240.), px(200.))),
            ..Default::default()
        };
        // Opened once this update is over: a new window draws at once, and a panel drawn
        // through this window's view (the Inspector) can't while it's being updated.
        let home = Home { me: me.clone(), main, region };
        cx.defer(move |cx| {
            let back = panel.clone();
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                // closing it docks the panel again, where it was
                let (closing, docked) = (home.clone(), panel.clone());
                window.on_window_should_close(cx, move |_, cx| {
                    closing.take_back(docked.clone(), cx);
                    true
                });
                cx.new(|_| PanelWindow { panel, home })
            });
            let _ = main.update(cx, |_, window, cx| {
                let Some(me) = me.upgrade() else { return };
                me.update(cx, |this, cx| match opened {
                    Ok((handle, view)) => this.panel_windows.push((id, handle, view)),
                    // no window: it goes back where it was
                    Err(e) => {
                        tracing::warn!(%e, "panel window");
                        this.dock_again(back, region, window, cx);
                    }
                })
            });
        });
    }

    /// Every panel in a window of its own back in the dock (a test hook's).
    pub(super) fn all_back(this: WeakEntity<Self>, cx: &mut AsyncApp) {
        let Ok(windows) = this.update(cx, |this, _| this.panel_windows.clone()) else { return };
        for (_, handle, view) in windows {
            let _ = handle.update(cx, |_, window, cx| view.update(cx, |w, cx| w.back(window, cx)));
        }
    }

    /// What a panel does differently in a window of its own: the mixer's meters.
    fn own_window(&mut self, name: &str, own: bool, cx: &mut Context<Self>) {
        if name == "mixer" {
            self.mixer_dock.update(cx, |m, cx| m.set_own_window(own, cx));
        }
    }

    /// Puts a panel whose window closed back in the dock, in `region`.
    fn dock_again(&mut self, panel: Arc<dyn BasePanelView>, region: DockPlacement, window: &mut Window, cx: &mut Context<Self>) {
        let id = panel.panel_id(cx);
        self.panel_windows.retain(|(p, ..)| *p != id);
        self.own_window(panel.panel_name(cx), false, cx);
        let Some(dock) = self.dock.clone() else { return };
        dock.update(cx, |area, cx| {
            area.add_panel_view(panel, region, None, window, cx);
            if region != DockPlacement::Center && !area.is_dock_open(region) {
                area.toggle_dock(region, window, cx);
            }
            area.select_panel(id, window, cx);
        });
    }
}
