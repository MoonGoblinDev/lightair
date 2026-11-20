//! Typing into a text object on the canvas: double-click it and a field opens right over it,
//! at its size and the canvas's zoom, with its text. Clicking elsewhere (or ⌘↩) keeps what was
//! typed as one undoable change; Esc leaves it as it was.

use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::*;
use lightair_model::Id;

use super::StudioWindow;
use crate::ui::tok;

pub(super) struct CanvasText {
    source: Id,
    object: Id,
    canvas: Id,
    state: Entity<EditorState>,
    _events: Subscription,
}

impl StudioWindow {
    pub(super) fn start_canvas_text(&mut self, object: Id, source: Id, canvas: Id, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.studio.read(cx).workspace().sources.get(&source).and_then(|s| s.settings["text"].as_str().map(String::from)).unwrap_or_default();
        let state = cx.new(|cx| EditorState::new(window, cx).line_number(false).default_value(text));
        state.update(cx, |s, cx| s.focus(window, cx));
        let events = cx.subscribe_in(&state, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Blur) {
                this.finish_canvas_text(true, window, cx);
            }
        });
        self.canvas_text = Some(CanvasText { source, object, canvas, state, _events: events });
        cx.notify();
    }

    /// Ends typing: `keep` what was typed, or leave the text as it was.
    pub(super) fn finish_canvas_text(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.canvas_text.take() else { return };
        if keep {
            let text = t.state.read(cx).value().to_string();
            self.set_text(t.source, text, cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The field, over the object being typed into.
    pub(super) fn canvas_text_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.canvas_text.as_ref()?;
        let studio = self.studio.read(cx);
        let workspace = studio.workspace().clone();
        let scene = studio.selected_scene.and_then(|s| workspace.scene(s))?;
        let object = scene.object(t.object)?;
        let frame = self.frame_of(scene, object, t.canvas)?;
        let (ox, oy, s) = self.mapping(&workspace, t.canvas)?;
        let size = workspace.sources.get(&t.source).and_then(|i| i.settings["font"]["size"].as_f64()).unwrap_or(72.0) as f32;
        let (x, y) = (ox + frame.x * s, oy + frame.y * s);
        let (w, h) = ((frame.width * s).max(160.0), (frame.height * s).max(36.0));
        let state = t.state.clone();
        Some(
            deferred(
                anchored().position(point(px(x), px(y))).child(
                    div()
                        .id("canvas-text")
                        .w(px(w))
                        .min_h(px(h))
                        .p_1()
                        .rounded(px(4.))
                        .bg(tok::bg_panel())
                        .border_2()
                        .border_color(tok::accent_primary())
                        .text_size(px((size * s).clamp(11.0, 200.0)))
                        .text_color(tok::text())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                            "escape" => {
                                cx.stop_propagation();
                                this.finish_canvas_text(false, window, cx);
                            }
                            "enter" if ev.keystroke.modifiers.platform => {
                                cx.stop_propagation();
                                this.finish_canvas_text(true, window, cx);
                            }
                            _ => {}
                        }))
                        .child(Editor::new(&state).h(px(h - 8.0)).appearance(false)),
                ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
