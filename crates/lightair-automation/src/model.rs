//! An automation: when (its trigger), only if (its conditions), do (its steps), in a queue. All
//! of it is plain JSON, so the editor, the remote API, MCP agents and imports read and write the
//! same documents.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub trigger: Trigger,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Runs in the same queue wait for each other ("" runs at once, alongside anything).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub queue: String,
    /// Seconds before it runs again (for anyone).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cooldown: f64,
    /// Seconds before the same person can set it off again.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub user_cooldown: f64,
}

fn yes() -> bool {
    true
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Who may use a chat command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Who {
    #[default]
    Everyone,
    Subscribers,
    Vips,
    Moderators,
    Broadcaster,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum Trigger {
    /// Follows, subs, donations, cheers, raids (`kinds`; none: any of them), on these platforms
    /// (none: any), at least `min_amount`.
    Event {
        #[serde(default)]
        kinds: Vec<String>,
        #[serde(default)]
        platforms: Vec<String>,
        #[serde(default)]
        min_amount: f64,
        /// Test events (the test buttons) set it off too.
        #[serde(default = "yes")]
        tests: bool,
    },
    /// A chat command (`!name`, or one of `aliases`), from `who` and up.
    Command {
        name: String,
        #[serde(default)]
        aliases: Vec<String>,
        #[serde(default)]
        who: Who,
    },
    /// A chat message containing `contains` (any message when empty); `first_time`: only a
    /// person's first message ever.
    Chat {
        #[serde(default)]
        contains: String,
        #[serde(default)]
        first_time: bool,
    },
    /// Every `minutes`, when there have been at least `min_messages` chat messages since.
    Timer {
        minutes: f64,
        #[serde(default)]
        min_messages: u64,
    },
    /// Streaming or recording started or stopped (`what`: "stream" or "record"; `state`:
    /// "start" or "stop").
    Output { what: String, state: String },
    /// When Lightair starts.
    Start,
    /// A global hotkey ("cmd-alt-1"), working in any app.
    Hotkey { keys: String },
    /// A file changed (saved by another app: a game's stats, a bot's output). `{{ file.text }}`
    /// is what it holds now.
    File { path: String },
    /// A MIDI controller: a note pressed or a knob or fader moved (`message`: "note" or "cc";
    /// `number`: which one, -1 any; `channel` 1–16, 0 any).
    Midi {
        #[serde(default = "note")]
        message: String,
        #[serde(default = "any")]
        number: i32,
        #[serde(default)]
        channel: u8,
    },
    /// You say `phrase` (heard by the microphone, recognised on this computer where it can be).
    Voice { phrase: String },
    /// A game controller's button ("South", "Start", "DPadUp", …; empty: any).
    Controller {
        #[serde(default)]
        button: String,
    },
    /// Only when run by hand, by another automation, through the remote API or its web address.
    Manual,
}

/// `left op right`; both sides are templates (`{{ amount }}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub left: String,
    /// "is", "is not", ">", ">=", "<", "<=", "contains", "starts with", "chance" (left: a
    /// percentage, right unused).
    pub op: String,
    #[serde(default)]
    pub right: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Step {
    /// Any Lightair method (`scene.switch`, `source.update`, `record.start`, … as the remote API
    /// has them); text in `params` is filled in as templates.
    Call { method: String, #[serde(default)] params: Value },
    /// Sends to chat (where the trigger came from, unless `platform` says).
    Chat { text: String, #[serde(default, skip_serializing_if = "String::is_empty")] platform: String },
    /// Says it aloud on stream (the alert box's voice).
    Speak { text: String },
    /// Plays a sound on stream (the alert box's audio).
    Sound { file: String, #[serde(default = "full")] volume: f64 },
    Wait { seconds: f64 },
    /// Sets a variable ("set"), adds to it ("add") or removes it ("clear"). `user`: the person's
    /// own (per viewer) rather than everyone's.
    Var { name: String, #[serde(default = "set")] op: String, #[serde(default)] value: String, #[serde(default)] user: bool },
    /// Changes the person's points (`amount` may be negative; a template).
    Points { amount: String },
    If { when: Vec<Condition>, #[serde(default)] then: Vec<Step>, #[serde(default, rename = "else")] otherwise: Vec<Step> },
    /// One of the branches, at random.
    Random { choices: Vec<Vec<Step>> },
    /// Runs another automation (by name or id) with this one's context.
    Run { automation: String },
    /// A web request (`method` GET or POST); its answer is `{{ http }}` after.
    Http { url: String, #[serde(default = "get")] method: String, #[serde(default)] body: String },
    /// JavaScript (a function body): `ctx` is the run's context; what it returns is
    /// `{{ script.… }}` after, and `{stop: true}` ends the run. Ten seconds at most.
    Script { code: String },
    /// Ends the run here.
    Stop,
}

fn full() -> f64 {
    100.0
}

fn set() -> String {
    "set".into()
}

fn get() -> String {
    "GET".into()
}

impl Step {
    pub fn label(&self) -> String {
        match self {
            Step::Call { method, .. } => format!("Lightair: {method}"),
            Step::Chat { text, .. } => format!("Chat: {text}"),
            Step::Speak { text } => format!("Say: {text}"),
            Step::Sound { file, .. } => format!("Sound: {}", file.rsplit('/').next().unwrap_or(file)),
            Step::Wait { seconds } => format!("Wait {seconds} s"),
            Step::Var { name, op, value, .. } => format!("{op} {name} {value}"),
            Step::Points { amount } => format!("Points {amount}"),
            Step::If { .. } => "If…".into(),
            Step::Random { choices } => format!("One of {}", choices.len()),
            Step::Run { automation } => format!("Run {automation}"),
            Step::Http { method, url, .. } => format!("{method} {url}"),
            Step::Script { code } => format!("Script: {}", code.lines().next().unwrap_or("")),
            Step::Stop => "Stop".into(),
        }
    }
}

impl Trigger {
    pub fn summary(&self) -> String {
        match self {
            Trigger::Event { kinds, min_amount, .. } => {
                let k = if kinds.is_empty() { "any alert".to_string() } else { kinds.join(", ") };
                if *min_amount > 0.0 { format!("On {k} of {min_amount}+") } else { format!("On {k}") }
            }
            Trigger::Command { name, .. } => format!("Command {name}"),
            Trigger::Chat { contains, first_time } => match (contains.is_empty(), first_time) {
                (_, true) => "A first-time chatter".into(),
                (true, false) => "Any chat message".into(),
                (false, false) => format!("Chat containing “{contains}”"),
            },
            Trigger::Timer { minutes, .. } => format!("Every {minutes} min"),
            Trigger::Output { what, state } => format!(
                "When {} {}",
                if what == "record" { "recording" } else { "the stream" },
                if state == "stop" { "stops" } else { "starts" }
            ),
            Trigger::Start => "When Lightair starts".into(),
            Trigger::Hotkey { keys } => format!("Hotkey {keys}"),
            Trigger::File { path } => {
                let name = std::path::Path::new(path).file_name().map_or(path.as_str(), |n| n.to_str().unwrap_or(path));
                format!("When {name} changes")
            }
            Trigger::Midi { message, number, channel } => {
                let what = if message == "cc" { "MIDI control" } else { "MIDI note" };
                let which = if *number < 0 { "any".to_string() } else { number.to_string() };
                if *channel == 0 { format!("{what} {which}") } else { format!("{what} {which} (channel {channel})") }
            }
            Trigger::Voice { phrase } => format!("When you say “{phrase}”"),
            Trigger::Controller { button } => {
                if button.is_empty() { "Any controller button".into() } else { format!("Controller {button}") }
            }
            Trigger::Manual => "By hand".into(),
        }
    }
}

fn note() -> String {
    "note".into()
}

fn any() -> i32 {
    -1
}
