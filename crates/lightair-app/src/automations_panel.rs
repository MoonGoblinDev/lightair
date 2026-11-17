//! The Automations panel (docs/plans/live-integrations.md, 2.6): automations listed with their
//! switches, new ones from recipes, and the selected one's editor: when it runs, its steps
//! (each one's main field edited in place; anything else as JSON), cooldowns and queue, a button
//! to run it now, and its latest runs with what every step did.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, Input as TextInput, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_automation::{Automation, Automations, Condition, Step, Trigger, Who, recipes};
use serde_json::{Value, json};

use crate::ui::{self, tok};

pub struct AutomationsPanel {
    automations: Automations,
    /// For each automation's web address.
    widgets: Option<lightair_widgets::Widgets>,
    focus: FocusHandle,
    selected: Option<String>,
    /// The inputs of the automation they were made for (its id and step count).
    built_for: Option<(String, usize)>,
    inputs: Vec<(String, Entity<InputState>)>,
    json: Option<Entity<EditorState>>,
    show_json: bool,
    error: Option<String>,
    _events: Vec<Subscription>,
}

/// Fields edited in place: (key, label, number?).
fn fields(a: &Automation) -> Vec<(String, &'static str, bool)> {
    let mut f = vec![("name".to_string(), "Name", false)];
    match &a.trigger {
        Trigger::Command { .. } => f.push(("trigger.name".into(), "Command", false)),
        Trigger::Chat { .. } => f.push(("trigger.contains".into(), "Message contains (empty: any)", false)),
        Trigger::Timer { .. } => {
            f.push(("trigger.minutes".into(), "Every (minutes)", true));
            f.push(("trigger.min_messages".into(), "Only after this many chat messages", true));
        }
        Trigger::Event { .. } => f.push(("trigger.min_amount".into(), "At least (amount or raiders)", true)),
        Trigger::Hotkey { .. } => f.push(("trigger.keys".into(), "Keys (cmd-alt-1, ctrl-shift-f5, …)", false)),
        Trigger::File { .. } => f.push(("trigger.path".into(), "File (its full path; ~/ for your home folder)", false)),
        Trigger::Midi { .. } => {
            f.push(("trigger.message".into(), "note (a pad or key) or cc (a knob or fader)", false));
            f.push(("trigger.number".into(), "Which note or control (−1: any)", true));
            f.push(("trigger.channel".into(), "Channel (1–16, 0: any)", true));
        }
        Trigger::Controller { .. } => f.push(("trigger.button".into(), "Button (South, East, Start, DPadUp, LeftTrigger…; empty: any)", false)),
        Trigger::Manual | Trigger::Output { .. } | Trigger::Start => {}
    }
    step_fields(&a.steps, "steps", &mut f);
    f.push(("cooldown".into(), "Cooldown (seconds)", true));
    f.push(("user_cooldown".into(), "Per-person cooldown (seconds)", true));
    f.push(("queue".into(), "Queue (runs in one wait their turn)", false));
    f
}

/// A step's one field edited in its row: (field, label, number?).
fn step_field(s: &Step) -> Option<(&'static str, &'static str, bool)> {
    Some(match s {
        Step::Chat { .. } | Step::Speak { .. } => ("text", "Text", false),
        Step::Sound { .. } => ("file", "Sound file", false),
        Step::Wait { .. } => ("seconds", "Seconds", true),
        Step::Var { .. } => ("value", "Value", false),
        Step::Points { .. } => ("amount", "Points (− to take)", false),
        Step::Run { .. } => ("automation", "Automation", false),
        Step::Http { .. } => ("url", "Address", false),
        Step::Call { .. } => ("method", "Method", false),
        Step::Script { .. } => ("code", "Code (one line here; the JSON for more)", false),
        _ => return None,
    })
}

/// The fields of `steps` (at `path`, like "steps" or "steps.2.then"), and of the steps inside
/// If and Random steps.
fn step_fields(steps: &[Step], path: &str, f: &mut Vec<(String, &'static str, bool)>) {
    for (i, s) in steps.iter().enumerate() {
        let here = format!("{path}.{i}");
        if let Some((k, label, num)) = step_field(s) {
            f.push((format!("{here}.{k}"), label, num));
        }
        match s {
            Step::If { when, then, otherwise } => {
                for c in 0..when.len() {
                    for k in ["left", "op", "right"] {
                        f.push((format!("{here}.when.{c}.{k}"), "Condition", false));
                    }
                }
                step_fields(then, &format!("{here}.then"), f);
                step_fields(otherwise, &format!("{here}.else"), f);
            }
            Step::Random { choices } => {
                for (k, c) in choices.iter().enumerate() {
                    step_fields(c, &format!("{here}.choices.{k}"), f);
                }
            }
            _ => {}
        }
    }
}

/// Changes the list of steps at `path` ("steps", "steps.2.then", "steps.3.choices.1").
fn edit_list(a: &Automation, path: &str, f: impl FnOnce(&mut Vec<Value>)) -> Option<Automation> {
    let mut v = serde_json::to_value(a).ok()?;
    let mut cur = &mut v;
    for part in path.split('.') {
        cur = match part.parse::<usize>() {
            Ok(i) => cur.get_mut(i)?,
            Err(_) => cur.get_mut(part)?,
        };
    }
    f(cur.as_array_mut()?);
    serde_json::from_value(v).ok()
}

/// Every list of steps in `steps` (at `path`), with a name to show: "Steps", "2 › Then", ….
fn step_lists(steps: &[Step], path: &str, name: &str, out: &mut Vec<(String, String)>) {
    out.push((path.to_string(), name.to_string()));
    for (i, s) in steps.iter().enumerate() {
        let here = format!("{path}.{i}");
        let at = if name == "Steps" { format!("{}", i + 1) } else { format!("{name} › {}", i + 1) };
        match s {
            Step::If { then, otherwise, .. } => {
                step_lists(then, &format!("{here}.then"), &format!("{at} › Then"), out);
                step_lists(otherwise, &format!("{here}.else"), &format!("{at} › Otherwise"), out);
            }
            Step::Random { choices } => {
                for (k, c) in choices.iter().enumerate() {
                    step_lists(c, &format!("{here}.choices.{k}"), &format!("{at} › Choice {}", k + 1), out);
                }
            }
            _ => {}
        }
    }
}

fn list_mut<'a>(v: &'a mut Value, path: &str) -> Option<&'a mut Vec<Value>> {
    let mut cur = v;
    for part in path.split('.') {
        cur = match part.parse::<usize>() {
            Ok(n) => cur.get_mut(n)?,
            Err(_) => cur.get_mut(part)?,
        };
    }
    cur.as_array_mut()
}

/// Moves step `i` of the list at `from` to the end of the list at `to` (not into itself).
fn move_step(a: &Automation, from: &str, i: usize, to: &str) -> Option<Automation> {
    let here = format!("{from}.{i}");
    if to == from || to == here || to.starts_with(&format!("{here}.")) {
        return None;
    }
    let mut v = serde_json::to_value(a).ok()?;
    // taking the step out shifts later siblings: a target under one of them moves up by one
    let to = match to.strip_prefix(&format!("{from}.")) {
        Some(rest) => {
            let (n, tail) = rest.split_once('.').unwrap_or((rest, ""));
            match n.parse::<usize>() {
                Ok(n) if n > i => format!("{from}.{}{}{tail}", n - 1, if tail.is_empty() { "" } else { "." }),
                _ => to.to_string(),
            }
        }
        None => to.to_string(),
    };
    let step = {
        let src = list_mut(&mut v, from)?;
        if i >= src.len() {
            return None;
        }
        src.remove(i)
    };
    list_mut(&mut v, &to)?.push(step);
    serde_json::from_value(v).ok()
}

fn get(a: &Automation, key: &str) -> String {
    let v = serde_json::to_value(a).unwrap_or(Value::Null);
    let mut cur = &v;
    for part in key.split('.') {
        cur = match part.parse::<usize>() {
            Ok(i) => &cur[i],
            Err(_) => &cur[part],
        };
    }
    match cur {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => lightair_automation::template::text_of(other),
    }
}

fn set(a: &Automation, key: &str, text: &str, number: bool) -> Option<Automation> {
    let mut v = serde_json::to_value(a).ok()?;
    // whole numbers as integers, so integer fields (a MIDI note) take them
    let value = match text.trim().parse::<f64>() {
        Ok(n) if number && n.fract() == 0.0 && n.abs() < 1e15 => json!(n as i64),
        Ok(n) if number => json!(n),
        _ if number => json!(0),
        _ => json!(text),
    };
    let parts: Vec<&str> = key.split('.').collect();
    let (last, within) = parts.split_last()?;
    let mut cur = &mut v;
    for part in within {
        cur = match part.parse::<usize>() {
            Ok(i) => cur.get_mut(i)?,
            Err(_) => cur.get_mut(*part)?,
        };
    }
    *cur.get_mut(*last)? = value;
    serde_json::from_value(v).ok()
}

fn new_step(kind: &str) -> Step {
    match kind {
        "chat" => Step::Chat { text: "Hi {{ name }}!".into(), platform: String::new() },
        "speak" => Step::Speak { text: "{{ name }} says {{ message }}".into() },
        "sound" => Step::Sound { file: String::new(), volume: 100.0 },
        "wait" => Step::Wait { seconds: 2.0 },
        "var" => Step::Var { name: "count".into(), op: "add".into(), value: "1".into(), user: false },
        "points" => Step::Points { amount: "10".into() },
        "scene" => Step::Call { method: "scene.switch".into(), params: json!({"scene": "Main"}) },
        "show" => Step::Call { method: "object.update".into(), params: json!({"object": "Name of the object", "visible": true}) },
        "record" => Step::Call { method: "record.start".into(), params: json!({}) },
        "clip" => Step::Call { method: "clip.save".into(), params: json!({}) },
        "http" => Step::Http { url: "https://".into(), method: "GET".into(), body: String::new() },
        "vts" => Step::Call { method: "vts.hotkey".into(), params: json!({"hotkey": "Name of the hotkey"}) },
        "script" => Step::Script { code: "// ctx has name, message, amount, vars, …\nreturn { greeting: 'Hi ' + ctx.name };".into() },
        "light" => Step::Call { method: "light.set".into(), params: json!({"address": "192.168.1.20", "on": true, "brightness": 60, "temperature": 4500}) },
        "stream-edit" => Step::Call { method: "stream.edit".into(), params: json!({"platform": "twitch", "title": "{{ arg }}", "category": ""}) },
        "twitch-ad" => Step::Call { method: "twitch.ad".into(), params: json!({"seconds": 60}) },
        "twitch-poll" => Step::Call { method: "twitch.poll".into(), params: json!({"title": "What next?", "choices": "Option one, Option two", "seconds": 120}) },
        "twitch-prediction" => Step::Call { method: "twitch.prediction".into(), params: json!({"title": "Do we win?", "outcomes": "Yes, No", "seconds": 120}) },
        "twitch-raid" => Step::Call { method: "twitch.raid".into(), params: json!({"channel": "{{ target }}"}) },
        "twitch-shoutout" => Step::Call { method: "twitch.shoutout".into(), params: json!({"channel": "{{ target }}"}) },
        "twitch-clip" => Step::Call { method: "twitch.clip".into(), params: json!({}) },
        "twitch-marker" => Step::Call { method: "twitch.marker".into(), params: json!({"description": "{{ arg }}"}) },
        "if" => Step::If {
            when: vec![Condition { left: "{{ amount }}".into(), op: ">=".into(), right: "100".into() }],
            then: vec![],
            otherwise: vec![],
        },
        "random" => Step::Random { choices: vec![vec![], vec![]] },
        "hue" => Step::Call { method: "hue.set".into(), params: json!({"light": "Name of the light or room", "on": true, "brightness": 80, "colour": "#9146ff"}) },
        _ => Step::Stop,
    }
}

const STEP_KINDS: &[(&str, &str)] = &[
    ("chat", "Send a chat message"),
    ("speak", "Say something on stream"),
    ("sound", "Play a sound"),
    ("wait", "Wait"),
    ("scene", "Switch scene"),
    ("show", "Show or hide an object"),
    ("record", "Start recording"),
    ("clip", "Save a clip"),
    ("var", "Change a variable"),
    ("points", "Give or take points"),
    ("vts", "VTube Studio hotkey"),
    ("light", "Elgato Key Light"),
    ("hue", "Philips Hue light"),
    ("http", "Web request"),
    ("script", "JavaScript"),
    ("stream-edit", "Change the stream's title or category"),
    ("twitch-ad", "Twitch: run an ad"),
    ("twitch-poll", "Twitch: start a poll"),
    ("twitch-prediction", "Twitch: start a prediction"),
    ("twitch-raid", "Twitch: raid a channel"),
    ("twitch-shoutout", "Twitch: shout out a channel"),
    ("twitch-clip", "Twitch: make a clip"),
    ("twitch-marker", "Twitch: add a stream marker"),
    ("if", "If… (then, otherwise)"),
    ("random", "One at random"),
    ("stop", "Stop"),
];

impl AutomationsPanel {
    pub fn new(automations: Automations, widgets: Option<lightair_widgets::Widgets>, cx: &mut Context<Self>) -> Self {
        let dirty = Arc::new(AtomicBool::new(true));
        {
            let dirty = dirty.clone();
            automations.on_change(move || dirty.store(true, Ordering::Release));
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                // (hotkeys registered on this, the main, thread)
                if dirty.swap(false, Ordering::AcqRel)
                    && this
                        .update(cx, |this, cx| {
                            let keys = this
                                .automations
                                .list()
                                .into_iter()
                                .filter(|a| a.enabled)
                                .filter_map(|a| match a.trigger {
                                    Trigger::Hotkey { keys } if crate::shortcuts::parse(&keys).is_some() => Some((a.id, keys)),
                                    _ => None,
                                })
                                .collect();
                            crate::shortcuts::set_automation_keys(keys);
                            cx.notify()
                        })
                        .is_err()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
            }
        })
        .detach();
        let selected = automations.list().first().map(|a| a.id.clone());
        AutomationsPanel { automations, widgets, focus: cx.focus_handle(), selected, built_for: None, inputs: Vec::new(), json: None, show_json: false, error: None, _events: Vec::new() }
    }

    fn current(&self) -> Option<Automation> {
        self.selected.as_ref().and_then(|id| self.automations.get(id))
    }

    fn save(&mut self, a: Automation, cx: &mut Context<Self>) {
        self.automations.set(a);
        self.error = None;
        cx.notify();
    }

    /// Brings in the Streamer.bot export on the clipboard: each action an automation; what
    /// didn't come over, said.
    fn import_streamerbot(&mut self, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        match lightair_automation::streamerbot::import(&text) {
            Ok(r) => {
                let n = r.automations.len();
                for a in r.automations {
                    self.selected.get_or_insert(a.id.clone());
                    self.automations.set(a);
                }
                let mut said = format!("Imported {n} automation{} from Streamer.bot.", if n == 1 { "" } else { "s" });
                if !r.notes.is_empty() {
                    said.push_str(" Not brought over: ");
                    said.push_str(&r.notes.iter().take(6).cloned().collect::<Vec<_>>().join("; "));
                    if r.notes.len() > 6 {
                        said.push_str(&format!("; and {} more", r.notes.len() - 6));
                    }
                }
                self.error = Some(said);
                self.built_for = None;
            }
            Err(e) => self.error = Some(format!("Couldn't import: {e}. In Streamer.bot: select actions › right-click › Export, then copy the string.")),
        }
        cx.notify();
    }

    /// Makes (or refreshes) the inputs for the selected automation.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.current() else {
            self.inputs.clear();
            self.built_for = None;
            return;
        };
        // rebuilt when the fields change (steps added, moved into an If, a trigger changed)
        let shape = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            fields(&a).iter().for_each(|(k, _, _)| k.hash(&mut h));
            (a.id.clone(), h.finish() as usize)
        };
        if self.built_for.as_ref() != Some(&shape) {
            self.inputs.clear();
            self._events.clear();
            for (key, _, number) in fields(&a) {
                let state = cx.new(|cx| InputState::new(window, cx));
                let k = key.clone();
                self._events.push(cx.subscribe_in(&state, window, move |this, state, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let text = state.read(cx).value().to_string();
                        if let Some(a) = this.current()
                            && get(&a, &k) != text
                            && let Some(next) = set(&a, &k, &text, number)
                        {
                            this.save(next, cx);
                        }
                    }
                }));
                self.inputs.push((key, state));
            }
            let doc = serde_json::to_string_pretty(&a).unwrap_or_default();
            let editor = cx.new(|cx| EditorState::new(window, cx).language("json").line_number(false).default_value(doc));
            self._events.push(cx.subscribe_in(&editor, window, |this, state, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Blur) {
                    let text = state.read(cx).value().to_string();
                    match serde_json::from_str::<Automation>(&text) {
                        Ok(mut a) => {
                            if let Some(id) = &this.selected {
                                a.id = id.clone();
                            }
                            this.built_for = None;
                            this.save(a, cx);
                        }
                        Err(e) => {
                            this.error = Some(format!("Not saved: {e}"));
                            cx.notify();
                        }
                    }
                }
            }));
            self.json = Some(editor);
            self.built_for = Some(shape);
        }
        for (key, state) in &self.inputs {
            let value = get(&a, key);
            if !state.read(cx).focus_handle(cx).is_focused(window) && state.read(cx).value() != value.as_str() {
                state.update(cx, |s, cx| s.set_value(value, window, cx));
            }
        }
        if let Some(editor) = &self.json
            && !editor.read(cx).focus_handle(cx).is_focused(window)
        {
            let doc = serde_json::to_string_pretty(&a).unwrap_or_default();
            if editor.read(cx).value() != doc.as_str() {
                editor.update(cx, |s, cx| s.set_value(doc, window, cx));
            }
        }
    }

    fn input(&self, key: &str) -> Option<Entity<InputState>> {
        self.inputs.iter().find(|(k, _)| k == key).map(|(_, s)| s.clone())
    }

    fn list(&self, cx: &mut Context<Self>) -> Div {
        let me = cx.entity().downgrade();
        let new = Button::new("automation-new").label("New").icon(ui::icon("plus")).xsmall().outline().dropdown_menu(move |mut menu, _, _| {
            for r in recipes::RECIPES {
                let me = me.clone();
                menu = menu.item(PopupMenuItem::new(r.name).on_click(move |_, _, cx| {
                    if let (Some(me), Some(a)) = (me.upgrade(), recipes::make(r.key)) {
                        me.update(cx, |this, cx| {
                            this.selected = Some(a.id.clone());
                            this.save(a, cx);
                        });
                    }
                }));
            }
            let me = me.clone();
            menu.separator().item(PopupMenuItem::new("Import from Streamer.bot (copy its export first)").on_click(move |_, _, cx| {
                if let Some(me) = me.upgrade() {
                    me.update(cx, |this, cx| this.import_streamerbot(cx));
                }
            }))
        });
        let mut list = div().id("automation-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll();
        for (i, a) in self.automations.list().into_iter().enumerate() {
            let on = Some(&a.id) == self.selected.as_ref();
            let (id, toggled) = (a.id.clone(), a.clone());
            list = list.child(
                div()
                    .id(("automation", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(tok::bg_selected()))
                    .hover(|d| d.bg(tok::bg_selected()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id.clone());
                        this.show_json = false;
                        cx.notify();
                    }))
                    .child(switch(("automation-on", i), a.enabled).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let mut a = toggled.clone();
                        a.enabled = !a.enabled;
                        this.save(a, cx);
                    })))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(div().text_size(px(12.)).text_color(tok::text()).truncate().child(a.name.clone()))
                            .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).truncate().child(a.trigger.summary())),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(260.))
            .flex_none()
            .h_full()
            .p_2()
            .border_r_1()
            .border_color(tok::border())
            .child(div().flex().items_center().justify_between().child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("AUTOMATIONS")).child(new))
            .child(list)
    }

    fn trigger_menu(&self, a: &Automation, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity().downgrade();
        let base = a.clone();
        Button::new("automation-trigger").label(a.trigger.summary()).small().outline().dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
            let options: Vec<(&str, Trigger)> = vec![
                ("Follows", Trigger::Event { kinds: vec!["follow".into()], platforms: vec![], min_amount: 0.0, tests: true }),
                ("Subscriptions", Trigger::Event { kinds: vec!["sub".into()], platforms: vec![], min_amount: 0.0, tests: true }),
                ("Donations and cheers", Trigger::Event { kinds: vec!["paid".into(), "cheer".into()], platforms: vec![], min_amount: 0.0, tests: true }),
                ("Raids", Trigger::Event { kinds: vec!["raid".into()], platforms: vec![], min_amount: 0.0, tests: true }),
                ("Channel-points redemptions (Twitch)", Trigger::Event { kinds: vec!["redeem".into()], platforms: vec![], min_amount: 0.0, tests: true }),
                ("Any alert", Trigger::Event { kinds: vec![], platforms: vec![], min_amount: 0.0, tests: true }),
                ("A chat command", Trigger::Command { name: "!command".into(), aliases: vec![], who: Who::Everyone }),
                ("A chat message", Trigger::Chat { contains: String::new(), first_time: false }),
                ("A first-time chatter", Trigger::Chat { contains: String::new(), first_time: true }),
                ("A timer", Trigger::Timer { minutes: 10.0, min_messages: 0 }),
                ("The stream starts", Trigger::Output { what: "stream".into(), state: "start".into() }),
                ("The stream stops", Trigger::Output { what: "stream".into(), state: "stop".into() }),
                ("Recording starts", Trigger::Output { what: "record".into(), state: "start".into() }),
                ("Recording stops", Trigger::Output { what: "record".into(), state: "stop".into() }),
                ("Lightair starts", Trigger::Start),
                ("A hotkey (any app)", Trigger::Hotkey { keys: "cmd-alt-1".into() }),
                ("A file changes", Trigger::File { path: "~/Documents/deaths.txt".into() }),
                ("A MIDI pad or key", Trigger::Midi { message: "note".into(), number: -1, channel: 0 }),
                ("A MIDI knob or fader", Trigger::Midi { message: "cc".into(), number: -1, channel: 0 }),
                ("A game controller button", Trigger::Controller { button: "South".into() }),
                ("Only by hand", Trigger::Manual),
            ];
            for (label, t) in options {
                let (me, base) = (me.clone(), base.clone());
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    if let Some(me) = me.upgrade() {
                        let mut a = base.clone();
                        a.trigger = t.clone();
                        me.update(cx, |this, cx| this.save(a, cx));
                    }
                }));
            }
            menu
        })
    }

    fn who_menu(&self, a: &Automation, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Trigger::Command { who, .. } = &a.trigger else { return None };
        let label = match who {
            Who::Everyone => "Anyone",
            Who::Subscribers => "Subscribers and up",
            Who::Vips => "VIPs and up",
            Who::Moderators => "Moderators and up",
            Who::Broadcaster => "Only you",
        };
        let me = cx.entity().downgrade();
        let base = a.clone();
        Some(
            Button::new("automation-who")
                .label(label)
                .small()
                .outline()
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for (l, w) in [("Anyone", Who::Everyone), ("Subscribers and up", Who::Subscribers), ("VIPs and up", Who::Vips), ("Moderators and up", Who::Moderators), ("Only you", Who::Broadcaster)] {
                        let (me, base) = (me.clone(), base.clone());
                        menu = menu.item(PopupMenuItem::new(l).on_click(move |_, _, cx| {
                            let mut a = base.clone();
                            if let Trigger::Command { who, .. } = &mut a.trigger {
                                *who = w;
                            }
                            if let Some(me) = me.upgrade() {
                                me.update(cx, |this, cx| this.save(a, cx));
                            }
                        }));
                    }
                    menu
                })
                .into_any_element(),
        )
    }

    fn steps(&self, a: &Automation, cx: &mut Context<Self>) -> Div {
        self.step_list(a, &a.steps, "steps", cx)
    }

    /// Saves `a` with the list at `path` changed.
    fn change_list(&mut self, a: &Automation, path: &str, f: impl FnOnce(&mut Vec<Value>), cx: &mut Context<Self>) {
        if let Some(next) = edit_list(a, path, f) {
            self.built_for = None;
            self.save(next, cx);
        }
    }

    /// A list of steps (the automation's, or inside an If or a Random), each with its own field,
    /// and the lists inside them, indented.
    fn step_list(&self, a: &Automation, steps: &[Step], path: &str, cx: &mut Context<Self>) -> Div {
        let mut col = div().flex().flex_col().gap_1();
        let n = steps.len();
        for (i, s) in steps.iter().enumerate() {
            let here = format!("{path}.{i}");
            let field = step_field(s).and_then(|(k, _, _)| self.input(&format!("{here}.{k}")));
            let id = |what: &str| SharedString::from(format!("step-{what}-{here}"));
            let (up, down, del) = (a.clone(), a.clone(), a.clone());
            let (p_up, p_down, p_del) = (path.to_string(), path.to_string(), path.to_string());
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_1p5()
                    .rounded(px(6.))
                    .bg(tok::bg_app())
                    .child(div().w(px(18.)).text_size(px(11.)).text_color(tok::text_tertiary()).child(format!("{}", i + 1)))
                    .child(div().w(px(150.)).flex_none().text_size(px(12.)).text_color(tok::text_secondary()).truncate().child(s.label().split(':').next().unwrap_or("").to_string()))
                    .child(div().flex_1().min_w_0().when_some(field, |d, f| d.child(TextInput::new(&f).small())))
                    .child(ui::icon_button(id("up"), "chevron-up", tok::text_tertiary()).when(i > 0, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| this.change_list(&up, &p_up, |l| l.swap(i, i - 1), cx)))
                    }))
                    .child(ui::icon_button(id("down"), "chevron-down", tok::text_tertiary()).when(i + 1 < n, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| this.change_list(&down, &p_down, |l| l.swap(i, i + 1), cx)))
                    }))
                    .child({
                        // move it into another list (an If's Then or Otherwise, a choice) or out
                        let mut lists = Vec::new();
                        step_lists(&a.steps, "steps", "Steps", &mut lists);
                        let here = format!("{path}.{i}");
                        let targets: Vec<(String, String)> = lists.into_iter().filter(|(p, _)| p != path && p != &here && !p.starts_with(&format!("{here}."))).collect();
                        let (me, base, from) = (cx.entity().downgrade(), a.clone(), path.to_string());
                        Button::new(id("move"))
                            .icon(ui::icon("corner-down-right"))
                            .xsmall()
                            .ghost()
                            .tooltip("Move into another list")
                            .dropdown_menu(move |mut menu, _, _| {
                                for (to, name) in &targets {
                                    let (me, base, from, to) = (me.clone(), base.clone(), from.clone(), to.clone());
                                    menu = menu.item(PopupMenuItem::new(if to == "steps" { "Out, to the automation's steps".to_string() } else { format!("Into {name}") }).on_click(move |_, _, cx| {
                                        if let (Some(me), Some(next)) = (me.upgrade(), move_step(&base, &from, i, &to)) {
                                            me.update(cx, |this, cx| {
                                                this.built_for = None;
                                                this.save(next, cx);
                                            });
                                        }
                                    }));
                                }
                                menu
                            })
                    })
                    .child(ui::icon_button(id("del"), "trash", tok::text_tertiary()).on_click(cx.listener(move |this, _, _, cx| {
                        this.change_list(&del, &p_del, |l| {
                            l.remove(i);
                        }, cx)
                    }))),
            );
            let nested = |label: &str, inner: Div| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .ml(px(18.))
                    .pl_2()
                    .border_l_1()
                    .border_color(tok::border())
                    .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(label.to_string()))
                    .child(inner)
            };
            match s {
                Step::If { when, then, otherwise } => {
                    col = col.child(nested("When all of these hold", self.conditions(a, when.len(), &here, cx)));
                    col = col.child(nested("Then", self.step_list(a, then, &format!("{here}.then"), cx)));
                    col = col.child(nested("Otherwise", self.step_list(a, otherwise, &format!("{here}.else"), cx)));
                }
                Step::Random { choices } => {
                    for (k, c) in choices.iter().enumerate() {
                        let (base, list) = (a.clone(), format!("{here}.choices"));
                        let head = div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().flex_1().child(self.step_list(a, c, &format!("{here}.choices.{k}"), cx)))
                            .when(choices.len() > 1, |d| {
                                d.child(ui::icon_button(SharedString::from(format!("choice-del-{here}-{k}")), "x", tok::text_tertiary()).tooltip(ui::tip("Remove this choice")).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.change_list(&base, &list, |l| {
                                            l.remove(k);
                                        }, cx)
                                    },
                                )))
                            });
                        col = col.child(nested(&format!("Choice {}", k + 1), head));
                    }
                    let (base, list) = (a.clone(), format!("{here}.choices"));
                    col = col.child(div().ml(px(18.)).child(Button::new(SharedString::from(format!("choice-add-{here}"))).label("Add choice").icon(ui::icon("plus")).xsmall().ghost().on_click(
                        cx.listener(move |this, _, _, cx| this.change_list(&base, &list, |l| l.push(json!([])), cx)),
                    )));
                }
                _ => {}
            }
        }
        let me = cx.entity().downgrade();
        let (base, list) = (a.clone(), path.to_string());
        col.child(Button::new(SharedString::from(format!("step-add-{path}"))).label("Add step").icon(ui::icon("plus")).xsmall().ghost().dropdown_menu(move |mut menu, _, _| {
            for (kind, label) in STEP_KINDS {
                let (me, base, list) = (me.clone(), base.clone(), list.clone());
                menu = menu.item(PopupMenuItem::new(*label).on_click(move |_, _, cx| {
                    let step = serde_json::to_value(new_step(kind)).unwrap_or_default();
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |this, cx| this.change_list(&base, &list, |l| l.push(step), cx));
                    }
                }));
            }
            menu
        }))
    }

    /// An If's conditions: left, how, right, each editable; add and remove.
    fn conditions(&self, a: &Automation, count: usize, step: &str, cx: &mut Context<Self>) -> Div {
        let mut col = div().flex().flex_col().gap_1();
        let list = format!("{step}.when");
        for c in 0..count {
            let input = |k: &str| self.input(&format!("{list}.{c}.{k}"));
            let (base, l) = (a.clone(), list.clone());
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(["left", "op", "right"].into_iter().filter_map(|k| {
                        input(k).map(|f| div().when(k == "op", |d| d.w(px(110.)).flex_none()).when(k != "op", |d| d.flex_1().min_w_0()).child(TextInput::new(&f).small()))
                    }))
                    .child(ui::icon_button(SharedString::from(format!("cond-del-{list}-{c}")), "x", tok::text_tertiary()).on_click(cx.listener(move |this, _, _, cx| {
                        this.change_list(&base, &l, |l| {
                            l.remove(c);
                        }, cx)
                    }))),
            );
        }
        let (base, l) = (a.clone(), list.clone());
        col.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Button::new(SharedString::from(format!("cond-add-{list}"))).label("Add condition").icon(ui::icon("plus")).xsmall().ghost().on_click(cx.listener(move |this, _, _, cx| {
                    this.change_list(&base, &l, |l| l.push(json!({"left": "{{ amount }}", "op": ">=", "right": "100"})), cx)
                })))
                .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("How: is, is not, >, >=, <, <=, contains, starts with, chance (left a percentage)")),
        )
    }

    fn runs(&self, a: &Automation) -> Div {
        let mut col = div().flex().flex_col().gap_1();
        let runs: Vec<_> = self.automations.history().into_iter().rev().filter(|r| r.automation == a.id).take(8).collect();
        if runs.is_empty() {
            return col.child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child("It hasn't run yet."));
        }
        for r in runs {
            let mut row = div().flex().flex_col().gap(px(2.)).p_1p5().rounded(px(6.)).bg(tok::bg_app()).child(
                div()
                    .flex()
                    .gap_2()
                    .text_size(px(11.))
                    .child(div().text_color(if r.ok { tok::ok() } else { tok::error() }).child(if !r.done { "…" } else if r.ok { "✓" } else { "✗" }))
                    .child(div().text_color(tok::text_secondary()).truncate().child(r.cause.clone())),
            );
            for s in r.steps {
                row = row.child(
                    div()
                        .flex()
                        .gap_2()
                        .pl_4()
                        .text_size(px(11.))
                        .child(div().text_color(if s.ok { tok::text_tertiary() } else { tok::error() }).child(if s.ok { "·" } else { "✗" }))
                        .child(div().text_color(tok::text_tertiary()).truncate().child(if s.detail.is_empty() { s.step } else { format!("{} → {}", s.step.split(':').next().unwrap_or(""), s.detail) })),
                );
            }
            col = col.child(row);
        }
        col
    }

    fn editor(&self, a: &Automation, cx: &mut Context<Self>) -> AnyElement {
        let label = |t: &str| div().text_size(px(11.)).text_color(tok::text_tertiary()).child(t.to_string());
        let field = |this: &Self, key: &str, l: &'static str| {
            this.input(key).map(|s| div().flex().flex_col().flex_1().min_w_0().gap_1().child(label(l)).child(TextInput::new(&s).small()))
        };
        let (run_id, del_id, dup) = (a.id.clone(), a.id.clone(), a.clone());
        let mut body = div()
            .id("automation-editor")
            .flex()
            .flex_col()
            .gap_3()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().children(field(self, "name", "Name")))
                    .child(Button::new("automation-run").label("Run now").icon(ui::icon("play")).small().on_click(cx.listener(move |this, _, _, cx| {
                        if let Err(e) = this.automations.run(&run_id, None, &[]) {
                            this.error = Some(e);
                        }
                        cx.notify();
                    })))
                    .when_some(self.widgets.as_ref().and_then(|w| w.run_url(&a.id)), |d, url| {
                        d.child(
                            ui::icon_button("automation-url", "link", tok::text_secondary())
                                .tooltip(ui::tip("Copy its web address: opening it runs this (Stream Deck's Website action, Touch Portal, a shortcut). This computer only."))
                                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(url.clone()))),
                        )
                    })
                    .child(ui::icon_button("automation-dup", "copy", tok::text_secondary()).tooltip(ui::tip("Duplicate")).on_click(cx.listener(move |this, _, _, cx| {
                        let mut a = dup.clone();
                        a.id = lightair_automation::new_id();
                        a.name = format!("{} copy", a.name);
                        this.selected = Some(a.id.clone());
                        this.save(a, cx);
                    })))
                    .child(ui::icon_button("automation-del", "trash", tok::text_secondary()).tooltip(ui::tip("Delete")).on_click(cx.listener(move |this, _, _, cx| {
                        this.automations.remove(&del_id);
                        this.selected = this.automations.list().first().map(|a| a.id.clone());
                        cx.notify();
                    }))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label("When"))
                    .child(div().flex().flex_wrap().gap_2().child(self.trigger_menu(a, cx)).children(self.who_menu(a, cx))),
            );
        for (key, l, _) in fields(a).into_iter().filter(|(k, _, _)| k.starts_with("trigger.")) {
            body = body.children(field(self, &key, l));
        }
        body = body.child(div().flex().flex_col().gap_1().child(label("Do")).child(self.steps(a, cx)));
        body = body.child(
            div()
                .flex()
                .gap_2()
                .children(field(self, "cooldown", "Cooldown (s)"))
                .children(field(self, "user_cooldown", "Per person (s)"))
                .children(field(self, "queue", "Queue")),
        );
        if !a.conditions.is_empty() {
            body = body.child(label(&format!("Only if: {}", a.conditions.iter().map(|c| format!("{} {} {}", c.left, c.op, c.right)).collect::<Vec<_>>().join(", "))));
        }
        body = body.child(
            div()
                .id("automation-json-toggle")
                .text_size(px(11.))
                .text_color(tok::accent_primary())
                .cursor_pointer()
                .child(if self.show_json { "Hide JSON" } else { "Edit everything as JSON (conditions, step details)" })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_json = !this.show_json;
                    cx.notify();
                })),
        );
        if self.show_json
            && let Some(editor) = &self.json
        {
            body = body.child(div().h(px(260.)).rounded(px(6.)).border_1().border_color(tok::border()).child(Editor::new(editor).h_full()));
        }
        body = body.when_some(self.error.clone(), |d, e| d.child(div().text_size(px(11.)).text_color(tok::error()).child(e)));
        body.child(div().flex().flex_col().gap_1().child(label("Latest runs")).child(self.runs(a))).into_any_element()
    }
}

fn switch(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .w(px(30.))
        .h(px(18.))
        .rounded_full()
        .cursor_pointer()
        .bg(if on { tok::ok() } else { tok::border_strong() })
        .child(div().size(px(14.)).m(px(2.)).rounded_full().bg(rgb(0xFFFFFF)).when(on, |d| d.ml(px(14.))))
}

impl Render for AutomationsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // a deleted one: the first instead; none chosen (opened by the screenshot hook): the first
        let gone = self.selected.as_ref().is_some_and(|id| self.automations.get(id).is_none());
        let hook = self.selected.is_none() && std::env::var("LIGHTAIR_OPEN").is_ok_and(|h| h.starts_with("automations-"));
        if gone || hook {
            self.selected = self.automations.list().first().map(|a| a.id.clone());
        }
        self.sync(window, cx);
        let right = match self.current() {
            Some(a) => self.editor(&a, cx),
            None => div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .p_6()
                .child(ui::icon("workflow").size(px(28.)).text_color(tok::text_tertiary()))
                .child(div().text_size(px(13.)).text_color(tok::text_secondary()).child("No automations yet"))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(tok::text_tertiary())
                        .text_center()
                        .max_w(px(420.))
                        .child("Make Lightair act on its own: thank raiders, read donations aloud, run chat commands, switch scenes on a follow. Start from New."),
                )
                .into_any_element(),
        };
        div().flex().size_full().bg(tok::bg_panel()).child(self.list(cx)).child(right)
    }
}

impl gpui_kit::component::dock::BasePanel for AutomationsPanel {
    fn panel_name(&self) -> &'static str {
        "automations"
    }
}

impl gpui_kit::component::dock::Panel for AutomationsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        None
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::panel_title("workflow", "Automations")
    }

    fn title_style(&self, _: &App) -> Option<gpui_kit::component::dock::TitleStyle> {
        Some(ui::panel_header_style())
    }

    fn zoom_control(&self, _: &App) -> Option<gpui_kit::component::dock::PanelControl> {
        None
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl EventEmitter<gpui_kit::component::dock::PanelEvent> for AutomationsPanel {}

impl Focusable for AutomationsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod checks {
    use super::{edit_list, fields, get, set};
    use lightair_automation::{Automation, Condition, Step, Trigger};
    use serde_json::json;

    fn sample() -> Automation {
        serde_json::from_value(json!({
            "id": "a", "name": "Big tips", "enabled": true, "trigger": {"on": "manual"},
            "steps": [{"do": "if", "when": [{"left": "{{ amount }}", "op": ">=", "right": "100"}], "then": [{"do": "chat", "text": "Wow"}], "else": []},
                      {"do": "random", "choices": [[{"do": "speak", "text": "a"}], []]}]
        }))
        .unwrap()
    }

    #[std::prelude::v1::test]
    fn nested_steps_are_edited_in_place() {
        let a = sample();
        let keys: Vec<String> = fields(&a).into_iter().map(|f| f.0).collect();
        for k in ["steps.0.when.0.left", "steps.0.when.0.op", "steps.0.then.0.text", "steps.1.choices.0.0.text"] {
            assert!(keys.contains(&k.to_string()), "{k} in {keys:?}");
        }
        assert_eq!(get(&a, "steps.0.then.0.text"), "Wow");
        let a = set(&a, "steps.0.then.0.text", "Thank you!", false).unwrap();
        let a = set(&a, "steps.0.when.0.right", "500", false).unwrap();
        let Step::If { when, then, .. } = &a.steps[0] else { panic!() };
        assert_eq!((when[0].clone(), then[0].clone()), (Condition { left: "{{ amount }}".into(), op: ">=".into(), right: "500".into() }, Step::Chat { text: "Thank you!".into(), platform: String::new() }));
        // a step added into Otherwise, a choice removed
        let a = edit_list(&a, "steps.0.else", |l| l.push(json!({"do": "wait", "seconds": 2.0}))).unwrap();
        let a = edit_list(&a, "steps.1.choices", |l| {
            l.remove(1);
        })
        .unwrap();
        let Step::If { otherwise, .. } = &a.steps[0] else { panic!() };
        assert_eq!(otherwise, &vec![Step::Wait { seconds: 2.0 }]);
        let Step::Random { choices } = &a.steps[1] else { panic!() };
        assert_eq!(choices.len(), 1);
        // the If moved into the Random's choice, then a step moved back out
        let moved = super::move_step(&a, "steps", 0, "steps.1.choices.0").unwrap();
        assert_eq!(moved.steps.len(), 1);
        let Step::Random { choices } = &moved.steps[0] else { panic!() };
        assert!(matches!(choices[0].last(), Some(Step::If { .. })));
        let out = super::move_step(&moved, "steps.0.choices.0", 0, "steps").unwrap();
        assert_eq!(out.steps.len(), 2);
        // never into itself
        assert!(super::move_step(&a, "steps", 0, "steps.0.then").is_none());
        assert!(matches!(a.trigger, Trigger::Manual));
    }
}
