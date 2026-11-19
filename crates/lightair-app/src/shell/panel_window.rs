//! A dock panel in a window of its own: "Open in New Window" on its tab (right-click). The
//! panel moves, it isn't copied: it leaves the dock, and closing its window docks it again in
//! the region it came from. The canvas too: its preview then shows in a layer under its own
//! window (and back under the main window's when it docks again).

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{BasePanelView, DockPlacement, PanelHandle};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::StudioWindow;
use crate::ui::tok;

/// The window's root: the panel, on the panel's background (window roots draw none: the
/// main window's preview shows through them).
pub(super) struct PanelWindow {
    panel: Arc<dyn BasePanelView>,
    home: Home,
    /// The canvas's window: the layer its preview shows in, under the UI (made on first draw).
    native: Option<crate::native_preview::Shared>,
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
        // the canvas: its preview's layer under the UI, which shows through where nothing's drawn
        let under = self.native.as_ref().is_some_and(|shared| {
            let mut n = shared.borrow_mut();
            if n.is_none() {
                *n = crate::native_preview::NativePreview::attach(window, tok::bg_panel(), tok::bg_canvas_well());
                // made: the previews move to it now (the main window, covered, may not draw),
                // and the canvas redraws
                if n.is_some() {
                    let me = self.home.me.clone();
                    cx.defer(move |cx| {
                        if let Some(me) = me.upgrade() {
                            me.update(cx, |s, cx| {
                                s.hand_previews();
                                cx.notify();
                            });
                        }
                    });
                }
            }
            n.is_some()
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(!under, |d| d.bg(tok::bg_panel()))
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
        "canvas" => "Canvas",
        _ => "Panel",
    }
}

/// Whether a panel can have a window of its own (every one: the canvas's preview moves to
/// a layer under its window).
pub(super) fn can_pop_out(_: &str) -> bool {
    true
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
        // (the canvas stays in the dock, which shows where it went, `canvas_away`; its window
        // gets a canvas panel of its own)
        let panel = if name == "canvas" {
            self.new_canvas_panel(cx)
        } else {
            dock.update(cx, |area, cx| area.remove_panel_view(id, window, cx));
            panel
        };
        let id = panel.panel_id(cx);
        self.own_window(name, true, cx);
        let main = window.window_handle();
        let me = cx.entity().downgrade();
        // beside the main window, the size of a side panel (the canvas: as big as it was)
        let at = window.bounds();
        let canvas = name == "canvas";
        let size = if canvas {
            size((at.size.width * 0.6).max(px(640.)), (at.size.height * 0.7).max(px(400.)))
        } else {
            size(px(380.), at.size.height.min(px(760.)))
        };
        let origin = point(at.right() - size.width - px(40.), at.top() + px(80.));
        let native = (canvas && crate::native_preview::on()).then(|| self.canvas_native.clone());
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size })),
            // the canvas's preview is drawn under its UI, showing where the UI leaves room
            window_background: if native.is_some() { WindowBackgroundAppearance::Transparent } else { WindowBackgroundAppearance::Opaque },
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
                cx.new(|_| PanelWindow { panel, home, native })
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
        if name == "canvas" {
            if own {
                self.canvas_out = true;
            } else {
                self.canvas_home();
            }
            cx.notify();
        }
    }

    /// Puts a panel whose window closed back in the dock, in `region`.
    fn dock_again(&mut self, panel: Arc<dyn BasePanelView>, region: DockPlacement, window: &mut Window, cx: &mut Context<Self>) {
        let id = panel.panel_id(cx);
        self.panel_windows.retain(|(p, ..)| *p != id);
        self.own_window(panel.panel_name(cx), false, cx);
        // the canvas never left the dock (its window had a panel of its own)
        if panel.panel_name(cx) == "canvas" {
            return;
        }
        let Some(dock) = self.dock.clone() else { return };
        dock.update(cx, |area, cx| {
            area.add_panel_view(panel, region, None, window, cx);
            if region != DockPlacement::Center && !area.is_dock_open(region) {
                area.toggle_dock(region, window, cx);
            }
            area.select_panel(id, window, cx);
        });
    }

    /// The canvas's own window, if it has one: its handle and view.
    fn canvas_window(&self, cx: &App) -> Option<(AnyWindowHandle, Entity<PanelWindow>)> {
        self.panel_windows.iter().find(|(_, _, v)| v.read(cx).panel.panel_name(cx) == "canvas").map(|(_, h, v)| (*h, v.clone()))
    }

    /// The canvas full screen (only its picture, on black, on this window's display), or back.
    /// In a window of its own, it goes back to the dock first.
    pub(super) fn toggle_canvas_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(full) = self.canvas_full {
            close_fullscreen(full, cx.entity().downgrade(), cx);
            self.fullscreen_home(cx);
            return;
        }
        let display = window.display(cx);
        let me = cx.entity().downgrade();
        let popped = self.canvas_window(cx);
        cx.defer(move |cx| {
            if let Some((handle, view)) = popped {
                let _ = handle.update(cx, |_, w, cx| view.update(cx, |v, cx| v.back(w, cx)));
            }
            let Some(this) = me.upgrade() else { return };
            let bounds = display.as_ref().map(|d| d.bounds()).unwrap_or(Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.))));
            // the whole screen, menu bar, Dock and notch area too ("simple" full screen: no Space
            // of its own, no animation)
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                display_id: display.as_ref().map(|d| d.id()),
                window_background: WindowBackgroundAppearance::Transparent,
                focus: true,
                ..Default::default()
            };
            let native = this.read(cx).canvas_native.clone();
            this.update(cx, |s, cx| {
                s.canvas_out = true;
                cx.notify();
            });
            let me2 = me.clone();
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                let closing = me2.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    if let Some(me) = closing.upgrade() {
                        me.update(cx, |s, cx| s.fullscreen_home(cx));
                    }
                    true
                });
                enter_fullscreen(window);
                let focus = cx.focus_handle();
                cx.new(|cx| {
                    let observed = me2.upgrade().map(|me| cx.observe(&me, |_, _, cx| cx.notify()));
                    CanvasScreen { me: me2, native, focus, focused: false, bare: false, _observe: observed }
                })
            });
            this.update(cx, |s, cx| match opened {
                Ok((handle, _)) => {
                    s.canvas_full = Some(handle);
                    crate::native_preview::hide_cursor();
                    // the menu bar and Dock hidden outright once it's full screen (it goes full
                    // screen just after it opens)
                    cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
                        let _ = handle.update(cx, |_, w, _| {
                            if is_fullscreen(w) {
                                crate::native_preview::hide_menu_bar();
                            }
                        });
                    })
                    .detach();
                }
                Err(e) => {
                    tracing::warn!(%e, "full screen canvas");
                    s.fullscreen_home(cx);
                }
            });
        });
    }

    /// The canvas back from full screen: its previews in this window's layer again.
    pub(super) fn fullscreen_home(&mut self, cx: &mut Context<Self>) {
        if self.canvas_full.take().is_some() || self.canvas_out {
            self.canvas_home();
            cx.notify();
        }
    }

    /// Where the canvas is while it's away from the main window: its own window, or full screen.
    pub(super) fn canvas_away(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let full = self.canvas_full.is_some();
        let (icon, title, hint) = if full {
            ("fullscreen", "The canvas is full screen", "Press Esc there, or ⇧⌘F, to bring it back.")
        } else {
            ("app-window", "The canvas is in its own window", "Close that window, or bring it back here.")
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(tok::bg_canvas_well())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(48.))
                    .rounded(px(12.))
                    .bg(tok::bg_raised())
                    .child(crate::ui::icon(icon).size(px(22.)).text_color(tok::text_secondary())),
            )
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_size(px(12.)).text_color(tok::text_tertiary()).child(crate::ui::keys_label(hint).into_owned()))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .when(!full, |d| {
                        d.child(Button::new("canvas-show-window").label("Show it").small().outline().on_click(cx.listener(|this, _, _, cx| {
                            if let Some((handle, _)) = this.canvas_window(cx) {
                                let _ = handle.update(cx, |_, w, _| w.activate_window());
                            }
                        })))
                    })
                    .child(Button::new("canvas-bring-back").label(if full { "Leave full screen" } else { "Bring it back" }).small().primary().on_click(cx.listener(
                        move |this, _, window, cx| {
                            if full {
                                this.toggle_canvas_fullscreen(window, cx);
                            } else if let Some((handle, view)) = this.canvas_window(cx) {
                                cx.defer(move |cx| {
                                    let _ = handle.update(cx, |_, w, cx| view.update(cx, |v, cx| v.back(w, cx)));
                                });
                            }
                        },
                    ))),
            )
    }

    /// The canvas as the full screen shows it: only the picture, fitted, on black (drawn under
    /// the window's UI, which draws nothing here).
    pub(super) fn canvas_presentation(&self, cx: &mut Context<Self>) -> AnyElement {
        let workspace = self.studio.read(cx).workspace().clone();
        let shown = self.current_canvas(&workspace).clone();
        let (id, cw, ch) = (shown.id, shown.width as f32, shown.height as f32);
        let layer = self.canvas_native.clone();
        div()
            .size_full()
            .relative()
            .child(
                canvas(
                    move |b, _, _| {
                        let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
                        let s = (bw / cw).min(bh / ch);
                        let (w, h) = (cw * s, ch * s);
                        let at = point(b.origin.x + px((bw - w) / 2.0), b.origin.y + px((bh - h) / 2.0));
                        if let Some(n) = layer.borrow_mut().as_mut() {
                            n.place(Some(b), &[(id, Bounds::new(at, size(px(w), px(h))))]);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }
}

/// The canvas full screen: only its picture, on black. Esc (or ⇧⌘F, or a double-click)
/// brings it back.
pub(super) struct CanvasScreen {
    me: WeakEntity<StudioWindow>,
    native: crate::native_preview::Shared,
    focus: FocusHandle,
    focused: bool,
    /// The menu bar and Dock hidden outright.
    bare: bool,
    _observe: Option<Subscription>,
}

impl CanvasScreen {
    /// Leaves full screen: the main window takes the picture back, then this window closes.
    fn exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(me) = self.me.upgrade() {
            me.update(cx, |s, cx| s.fullscreen_home(cx));
        }
        close_fullscreen(window.window_handle(), self.me.clone(), cx);
    }
}

/// Full screen: macOS's "simple" kind (the whole screen, menu bar, Dock and notch area too, no
/// Space of its own, no animation); elsewhere the window's own (Windows: no frame, over the
/// whole monitor, the taskbar too; simple full screen does nothing there).
fn enter_fullscreen(window: &mut Window) {
    if cfg!(target_os = "macos") {
        window.toggle_simple_fullscreen();
    } else {
        window.toggle_fullscreen();
    }
}

fn is_fullscreen(window: &Window) -> bool {
    window.is_simple_fullscreen() || window.is_fullscreen()
}

/// Closes the full screen canvas's window: out of full screen first (so the menu bar and Dock
/// come back; a window closed while it's full screen can stay), then closed.
fn close_fullscreen(handle: AnyWindowHandle, me: WeakEntity<StudioWindow>, cx: &mut App) {
    let mut wait = 50;
    let _ = handle.update(cx, |_, w, _| {
        if w.is_simple_fullscreen() {
            w.toggle_simple_fullscreen();
            wait = 300;
        } else if w.is_fullscreen() {
            w.toggle_fullscreen();
            wait = 900;
        }
    });
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(wait)).await;
        let _ = handle.update(cx, |_, w, _| w.remove_window());
        // the main window, in view again, draws the canvas back in it
        let _ = me.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

impl Render for CanvasScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        {
            let mut n = self.native.borrow_mut();
            if n.is_none() {
                let black: Hsla = gpui_kit::rgb(0x000000).into();
                *n = crate::native_preview::NativePreview::attach(window, black, black);
                // made: the previews move to it now (this window covers the main one, which
                // then doesn't draw), and this window draws the picture
                if n.is_some() {
                    let me = self.me.clone();
                    cx.defer(move |cx| {
                        if let Some(me) = me.upgrade() {
                            me.update(cx, |s, cx| {
                                s.hand_previews();
                                cx.notify();
                            });
                        }
                    });
                    cx.notify();
                }
            }
        }
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        // once full screen: nothing of the system's shows either, not even on the pointer's way
        if !self.bare && is_fullscreen(window) {
            self.bare = true;
            crate::native_preview::hide_menu_bar();
        }
        let picture = self.me.upgrade().map(|me| me.update(cx, |s, cx| s.canvas_presentation(cx)));
        div()
            .id("canvas-fullscreen")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let k = &ev.keystroke;
                if k.key == "escape" || (k.key == "f" && k.modifiers.shift && (k.modifiers.platform || k.modifiers.control)) {
                    this.exit(window, cx);
                }
            }))
            .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                if ev.click_count() == 2 {
                    this.exit(window, cx);
                }
            }))
            .children(picture)
    }
}
