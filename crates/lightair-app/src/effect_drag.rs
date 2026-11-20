//! Dragging an effect to reorder its chain (mixer strips and Inspector cards), or to move an
//! audio effect to another strip. The engine runs a chain in this order.

use gpui_kit::*;
use lightair_model::{Command, Effect, Id, MixerStrip, Workspace};

use crate::ui::{self, tok};

/// Where an effect lives: a source (video) or a mixer strip (audio).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Home {
    Source(Id),
    Strip(MixerStrip),
}

impl Home {
    fn effects(self, p: &Workspace) -> Option<&[Effect]> {
        Some(match self {
            Home::Source(id) => &p.sources.get(&id)?.effects,
            Home::Strip(MixerStrip::Track(id)) => &p.track(id)?.effects,
            Home::Strip(MixerStrip::Bus(id)) => &p.audio.bus(id)?.effects,
            Home::Strip(MixerStrip::Mix(id)) => &p.audio.mix(id)?.effects,
            Home::Strip(MixerStrip::Master) => &p.audio.master.effects,
        })
    }

    fn remove(self, effect: Id) -> Command {
        match self {
            Home::Source(source) => Command::RemoveSourceEffect { source, effect },
            Home::Strip(strip) => Command::RemoveStripEffect { strip, effect },
        }
    }

    fn add(self, effect: Effect, index: usize) -> Command {
        match self {
            Home::Source(source) => Command::AddSourceEffect { source, effect, index: Some(index) },
            Home::Strip(strip) => Command::AddStripEffect { strip, effect, index: Some(index) },
        }
    }
}

/// The effect being dragged.
#[derive(Clone)]
pub struct DraggedEffect {
    pub home: Home,
    pub effect: Id,
    pub name: SharedString,
}

impl Render for DraggedEffect {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(28.))
            .px_3()
            .rounded(px(6.))
            .bg(tok::bg_raised())
            .border_1()
            .border_color(tok::accent_primary())
            .text_size(px(12.))
            .text_color(tok::text())
            .shadow_md()
            .child(ui::icon("grip-vertical").size(px(13.)).text_color(tok::text_tertiary()))
            .child(self.name.clone())
    }
}

/// The command that puts the dragged effect before `before` in `to`'s chain (`None`: at the
/// end). `None` when nothing would change, or the move makes no sense (audio onto video).
pub fn move_cmd(p: &Workspace, d: &DraggedEffect, to: Home, before: Option<Id>) -> Option<Command> {
    // audio onto video, or an effect dropped on itself
    if matches!(d.home, Home::Source(_)) != matches!(to, Home::Source(_)) || before == Some(d.effect) {
        return None;
    }
    let from = d.home.effects(p)?;
    let at = from.iter().position(|e| e.id == d.effect)?;
    let effect = from[at].clone();
    // the target chain as it is once the effect has left it
    let rest: Vec<Id> = to.effects(p)?.iter().map(|e| e.id).filter(|id| *id != d.effect).collect();
    let index = before.and_then(|b| rest.iter().position(|id| *id == b)).unwrap_or(rest.len());
    if to == d.home && index == at {
        return None;
    }
    let label = format!("Move {}", effect.name);
    Some(Command::Batch { label, commands: vec![d.home.remove(d.effect), to.add(effect, index)] })
}

#[cfg(test)]
mod tests {
    // not `super::*`: gpui's `test` attribute would shadow the standard one
    use super::{DraggedEffect, Home, move_cmd};
    use lightair_model::{Command, Document, Effect, Id, Source, MixerStrip, Workspace, Track};
    use serde_json::json;

    fn chain(doc: &Document, t: Id) -> Vec<String> {
        doc.workspace().track(t).unwrap().effects.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn dragging_reorders_and_moves_between_strips() {
        let mut doc = Document::new(Workspace::new("Drag"));
        let mic = Source::new("Mic", "lightair_audio_device", json!({}));
        let mut a = Track::new("A", mic.id);
        a.effects = ["Gate", "EQ", "Comp"].iter().map(|n| Effect::new(*n, "audio.gain", json!({}))).collect();
        let b = Track::new("B", mic.id);
        let (ta, tb) = (a.id, b.id);
        let ids: Vec<Id> = a.effects.iter().map(|e| e.id).collect();
        doc.apply(&Command::AddSource { source: mic }).unwrap();
        doc.apply(&Command::AddTrack { track: a, index: None }).unwrap();
        doc.apply(&Command::AddTrack { track: b, index: None }).unwrap();
        let home = Home::Strip(MixerStrip::Track(ta));
        let drag = |id: Id| DraggedEffect { home, effect: id, name: "x".into() };

        // Comp before Gate
        let cmd = move_cmd(doc.workspace(), &drag(ids[2]), home, Some(ids[0])).unwrap();
        doc.apply(&cmd).unwrap();
        assert_eq!(chain(&doc, ta), ["Comp", "Gate", "EQ"]);
        // Comp to the end (dropped on +), downwards
        let cmd = move_cmd(doc.workspace(), &drag(ids[2]), home, None).unwrap();
        doc.apply(&cmd).unwrap();
        assert_eq!(chain(&doc, ta), ["Gate", "EQ", "Comp"]);
        // onto itself, or just before the one after it: nothing to do
        assert!(move_cmd(doc.workspace(), &drag(ids[1]), home, Some(ids[1])).is_none());
        assert!(move_cmd(doc.workspace(), &drag(ids[1]), home, Some(ids[2])).is_none());
        // EQ to the other track
        let cmd = move_cmd(doc.workspace(), &drag(ids[1]), Home::Strip(MixerStrip::Track(tb)), None).unwrap();
        doc.apply(&cmd).unwrap();
        assert_eq!(chain(&doc, ta), ["Gate", "Comp"]);
        assert_eq!(chain(&doc, tb), ["EQ"]);
        // audio never lands on a video source
        assert!(move_cmd(doc.workspace(), &drag(ids[0]), Home::Source(Id::new()), None).is_none());
    }
}
