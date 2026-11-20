//! Renaming and drag-reordering rows in the Objects and Scenes lists (docs/05-interface-shell.md).

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use lightair_model::{Command, Id};

use crate::ui::{self, tok};

/// What an inline rename field edits.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RowTarget {
    Scene(Id),
    Object { scene: Id, object: Id },
    /// A mixer strip (track, bus or mix).
    Strip(lightair_model::MixerStrip),
}

impl RowTarget {
    pub fn rename(self, name: String) -> Command {
        match self {
            RowTarget::Scene(scene) => Command::RenameScene { scene, name },
            RowTarget::Object { scene, object } => Command::RenameObject { scene, object, name },
            RowTarget::Strip(strip) => Command::RenameStrip { strip, name },
        }
    }
}

/// The open inline rename field.
pub struct Renaming {
    pub target: RowTarget,
    pub input: Entity<InputState>,
    pub original: String,
    _events: Subscription,
}

impl Renaming {
    /// Opens a field holding `name`, focused with everything selected. `on_done(commit)` runs on
    /// Enter (commit) or blur (commit).
    pub fn open<T: 'static>(
        target: RowTarget,
        name: &str,
        window: &mut Window,
        cx: &mut Context<T>,
        on_done: impl Fn(&mut T, bool, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(name.to_string(), window, cx);
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        let events = cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => on_done(this, true, window, cx),
            _ => {}
        });
        Self { target, input, original: name.to_string(), _events: events }
    }

    /// The command to run, if the name changed and isn't blank.
    pub fn command(&self, cx: &App) -> Option<Command> {
        let name = self.input.read(cx).value().trim().to_string();
        (!name.is_empty() && name != self.original).then(|| self.target.rename(name))
    }
}

/// A row being dragged to a new position. Rendered as the drag preview.
#[derive(Clone)]
pub struct DraggedRow {
    pub target: RowTarget,
    pub name: SharedString,
    pub icon: &'static str,
}

impl Render for DraggedRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(32.))
            .px_3()
            .rounded(px(6.))
            .bg(tok::bg_raised())
            .border_1()
            .border_color(tok::accent_primary())
            .text_color(tok::text())
            .shadow_md()
            .child(ui::icon(self.icon).size(px(15.)).text_color(tok::text_secondary()))
            .child(self.name.clone())
    }
}
