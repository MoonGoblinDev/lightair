//! Lightair's automations (docs/plans/live-integrations.md, 2.6): Streamer.bot's place, built in.
//! An automation is a trigger (an alert, a chat command, a chat message, a timer, or by hand),
//! conditions and steps; steps reach Lightair through the same methods as the remote API (so
//! every command is a step), plus chat, voice, sound, variables, points, web requests and other
//! automations. Runs in the same queue wait their turn; every run is kept with what each step did.
//!
//! The automations, global variables and points rules are one JSON file; viewers another.

pub mod model;
pub mod recipes;
pub mod streamerbot;
pub mod template;
pub mod viewers;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, unbounded};
use lightair_live::{ChatEvent, Hub, Kind, Role};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub use model::{Automation, Condition, Step, Trigger, Who};
use template::{render, render_value, text_of};
pub use viewers::{PointsRules, Viewer, Viewers};

/// Runs kept.
const HISTORY: usize = 200;
/// A run stops after this many steps (a loop of automations running each other).
const MAX_STEPS: usize = 500;

/// Calls a Lightair method (the remote API's) for a step: `chat.send`, `widgets.speak`,
/// `scene.switch`, …
pub type Host = Arc<dyn Fn(&str, Value) -> Result<Value, String> + Send + Sync>;

#[derive(Default, Serialize, Deserialize)]
struct Doc {
    #[serde(default)]
    automations: Vec<Automation>,
    #[serde(default)]
    vars: Map<String, Value>,
    #[serde(default)]
    points: PointsRules,
}

/// What one step did.
#[derive(Debug, Clone, Serialize)]
pub struct StepLog {
    pub step: String,
    pub ok: bool,
    pub detail: String,
}

/// One run of an automation.
#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub automation: String,
    pub name: String,
    pub started_ms: i64,
    /// What set it off ("NightOwl followed", "!so @pal", "Every 15 min").
    pub cause: String,
    pub steps: Vec<StepLog>,
    pub ok: bool,
    pub done: bool,
}

struct Inner {
    file: PathBuf,
    viewers_file: PathBuf,
    doc: Mutex<Doc>,
    viewers: Mutex<Viewers>,
    viewers_dirty: Mutex<bool>,
    history: Mutex<VecDeque<Run>>,
    last_run: Mutex<HashMap<String, Instant>>,
    user_last: Mutex<HashMap<(String, String), Instant>>,
    queues: Mutex<HashMap<String, Sender<Job>>>,
    timers: Mutex<HashMap<String, (Instant, u64)>>,
    host: Host,
    listeners: Mutex<Vec<Box<dyn Fn() + Send + Sync>>>,
}

struct Job {
    automation: Automation,
    ctx: Value,
    cause: String,
}

/// The automations (cheap to clone).
#[derive(Clone)]
pub struct Automations {
    inner: Arc<Inner>,
}

fn now_ms() -> i64 {
    lightair_live::event::now_ms()
}

pub fn new_id() -> String {
    format!("{:016x}", (now_ms() as u64) << 16 | template::random_below(65536))
}

fn rank(roles: &[Role]) -> Who {
    if roles.contains(&Role::Broadcaster) {
        Who::Broadcaster
    } else if roles.contains(&Role::Moderator) {
        Who::Moderators
    } else if roles.contains(&Role::Vip) {
        Who::Vips
    } else if roles.contains(&Role::Subscriber) || roles.contains(&Role::Member) {
        Who::Subscribers
    } else {
        Who::Everyone
    }
}

fn allowed(who: Who, roles: &[Role]) -> bool {
    let order = |w: Who| match w {
        Who::Everyone => 0,
        Who::Subscribers => 1,
        Who::Vips => 2,
        Who::Moderators => 3,
        Who::Broadcaster => 4,
    };
    order(rank(roles)) >= order(who)
}

fn amount_of(e: &ChatEvent) -> f64 {
    match &e.kind {
        Kind::Raid { viewers } => *viewers as f64,
        _ => e.paid.as_ref().map_or(0.0, |p| p.amount),
    }
}

/// What a run knows about what set it off.
pub fn context(e: Option<&ChatEvent>, args: &[String]) -> Value {
    let mut ctx = json!({"args": args, "arg": args.join(" "), "target": args.first().map(|a| a.trim_start_matches('@')).unwrap_or("")});
    if let Some(e) = e {
        let user = e.user.as_ref();
        ctx["event"] = serde_json::to_value(e).unwrap_or(Value::Null);
        ctx["name"] = json!(e.name());
        ctx["user"] = json!({
            "name": e.name(),
            "login": user.map_or("", |u| u.login.as_str()),
            "id": user.map_or("", |u| u.id.as_str()),
            "roles": user.map(|u| u.roles.clone()).unwrap_or_default(),
        });
        ctx["platform"] = json!(e.platform);
        // a channel-points redemption: which reward
        if let Some(p) = e.paid.as_ref().filter(|p| p.currency == "CHANNEL_POINTS") {
            ctx["reward"] = json!(p.display);
        }
        ctx["channel"] = json!(e.channel);
        ctx["message"] = json!(e.text);
        ctx["kind"] = json!(e.alert_kind().unwrap_or("message"));
        ctx["amount"] = json!(e.paid.as_ref().map_or_else(|| text_of(&json!(amount_of(e))), |p| p.display.clone()));
        ctx["value"] = json!(amount_of(e));
        ctx["viewers"] = json!(if let Kind::Raid { viewers } = e.kind { viewers } else { 0 });
        ctx["headline"] = json!(e.headline.clone().unwrap_or_default());
    }
    ctx
}

fn check(c: &Condition, ctx: &Value) -> bool {
    let (l, r) = (render(&c.left, ctx), render(&c.right, ctx));
    let nums = (l.trim().parse::<f64>(), r.trim().parse::<f64>());
    match c.op.as_str() {
        "is" | "=" | "==" => l.eq_ignore_ascii_case(&r),
        "is not" | "!=" => !l.eq_ignore_ascii_case(&r),
        ">" => matches!(nums, (Ok(a), Ok(b)) if a > b),
        ">=" => matches!(nums, (Ok(a), Ok(b)) if a >= b),
        "<" => matches!(nums, (Ok(a), Ok(b)) if a < b),
        "<=" => matches!(nums, (Ok(a), Ok(b)) if a <= b),
        "contains" => l.to_lowercase().contains(&r.to_lowercase()),
        "starts with" => l.to_lowercase().starts_with(&r.to_lowercase()),
        "is empty" => l.trim().is_empty(),
        "chance" => (template::random_below(10_000) as f64) < l.trim().parse::<f64>().unwrap_or(0.0) * 100.0,
        _ => false,
    }
}

impl Automations {
    /// Loads them from `dir` and starts listening to `hub`.
    pub fn start(hub: Hub, dir: PathBuf, host: Host) -> Automations {
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("automations.json");
        let viewers_file = dir.join("viewers.json");
        let doc: Doc = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let viewers: Viewers = std::fs::read(&viewers_file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let a = Automations {
            inner: Arc::new(Inner {
                file,
                viewers_file,
                doc: Mutex::new(doc),
                viewers: Mutex::new(viewers),
                viewers_dirty: Mutex::new(false),
                history: Mutex::new(VecDeque::new()),
                last_run: Mutex::new(HashMap::new()),
                user_last: Mutex::new(HashMap::new()),
                queues: Mutex::new(HashMap::new()),
                timers: Mutex::new(HashMap::new()),
                host,
                listeners: Mutex::new(Vec::new()),
            }),
        };
        // events
        {
            let (wake, woken) = crossbeam_channel::bounded::<()>(1);
            hub.on_change(move || {
                let _ = wake.try_send(());
            });
            let a = a.clone();
            let hub = hub.clone();
            let mut last = hub.history().last().map_or(0, |e| e.id);
            let _ = std::thread::Builder::new().name("lightair-automation".into()).spawn(move || {
                while woken.recv().is_ok() {
                    for e in hub.since(last) {
                        last = e.id;
                        a.event(&e);
                    }
                }
            });
        }
        // timers, and saving viewers now and then
        {
            let a = a.clone();
            let _ = std::thread::Builder::new().name("lightair-automation-timers".into()).spawn(move || {
                let mut ticks = 0u64;
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    a.tick();
                    ticks += 1;
                    if ticks.is_multiple_of(30) {
                        a.save_viewers();
                    }
                }
            });
        }
        a
    }

    /// Called (from any thread) when automations, runs or variables change.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        self.inner.listeners.lock().push(Box::new(f));
    }

    fn changed(&self) {
        for l in self.inner.listeners.lock().iter() {
            l();
        }
    }

    fn save(&self) {
        let text = serde_json::to_vec_pretty(&*self.inner.doc.lock()).unwrap_or_default();
        let tmp = self.inner.file.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &self.inner.file);
        }
        self.changed();
    }

    fn save_viewers(&self) {
        let mut dirty = self.inner.viewers_dirty.lock();
        if !*dirty {
            return;
        }
        *dirty = false;
        let text = serde_json::to_vec(&*self.inner.viewers.lock()).unwrap_or_default();
        let tmp = self.inner.viewers_file.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &self.inner.viewers_file);
        }
    }

    pub fn list(&self) -> Vec<Automation> {
        self.inner.doc.lock().automations.clone()
    }

    pub fn get(&self, key: &str) -> Option<Automation> {
        self.inner.doc.lock().automations.iter().find(|a| a.id == key || a.name.eq_ignore_ascii_case(key)).cloned()
    }

    /// Adds it, or replaces the one with its id.
    pub fn set(&self, a: Automation) {
        {
            let mut doc = self.inner.doc.lock();
            match doc.automations.iter_mut().find(|x| x.id == a.id) {
                Some(x) => *x = a,
                None => doc.automations.push(a),
            }
        }
        self.save();
    }

    pub fn remove(&self, id: &str) -> bool {
        let removed = {
            let mut doc = self.inner.doc.lock();
            let before = doc.automations.len();
            doc.automations.retain(|a| a.id != id);
            before != doc.automations.len()
        };
        if removed {
            self.save();
        }
        removed
    }

    pub fn vars(&self) -> Map<String, Value> {
        self.inner.doc.lock().vars.clone()
    }

    pub fn set_var(&self, name: &str, value: Value) {
        {
            let mut doc = self.inner.doc.lock();
            if value.is_null() {
                doc.vars.remove(name);
            } else {
                doc.vars.insert(name.into(), value);
            }
        }
        self.save();
    }

    pub fn points_rules(&self) -> PointsRules {
        self.inner.doc.lock().points.clone()
    }

    pub fn set_points_rules(&self, rules: PointsRules) {
        self.inner.doc.lock().points = rules;
        self.save();
    }

    pub fn history(&self) -> Vec<Run> {
        self.inner.history.lock().iter().cloned().collect()
    }

    pub fn top_viewers(&self, n: usize) -> Vec<Viewer> {
        self.inner.viewers.lock().top(n)
    }

    /// Gives (or, negative, takes) a viewer's points: their balance after, or None when they
    /// haven't enough to take (a prediction's stake).
    pub fn change_points(&self, platform: &str, id: &str, name: &str, delta: f64) -> Option<f64> {
        let balance = {
            let mut viewers = self.inner.viewers.lock();
            let have = viewers.get(platform, id).map_or(0.0, |v| v.points);
            if delta < 0.0 && have + delta < -1e-9 {
                return None;
            }
            viewers.add_points(platform, id, name, delta)
        };
        *self.inner.viewers_dirty.lock() = true;
        self.save_viewers();
        self.changed();
        Some(balance)
    }

    /// Runs it now (by hand, the remote API), as if `event` had set it off.
    pub fn run(&self, key: &str, event: Option<&ChatEvent>, args: &[String]) -> Result<(), String> {
        let a = self.get(key).ok_or_else(|| format!("no automation “{key}”"))?;
        let ctx = self.with_state(context(event, args), event);
        self.queue(a, ctx, "Run by hand".into());
        Ok(())
    }

    fn with_state(&self, mut ctx: Value, e: Option<&ChatEvent>) -> Value {
        ctx["vars"] = Value::Object(self.vars());
        let rules = self.points_rules();
        ctx["points_name"] = json!(rules.name);
        if let Some(u) = e.and_then(|e| e.user.as_ref().map(|u| (e.platform.clone(), u.id.clone()))) {
            let v = self.inner.viewers.lock().get(&u.0, &u.1).cloned();
            ctx["viewer"] = json!({
                "points": v.as_ref().map_or(0.0, |v| v.points),
                "messages": v.as_ref().map_or(0, |v| v.messages),
                "first_seen": v.as_ref().map_or(0, |v| v.first_seen),
            });
            ctx["user_vars"] = self.inner.doc.lock().vars.get(&format!("user:{}:{}", u.0, u.1)).cloned().unwrap_or(json!({}));
        }
        ctx
    }

    fn event(&self, e: &ChatEvent) {
        if e.deleted || matches!(e.kind, Kind::Deletion { .. }) {
            return;
        }
        let mut first_time = false;
        if e.kind == Kind::Message
            && !e.test
            && let Some(u) = &e.user
        {
            let rules = self.points_rules();
            first_time = self.inner.viewers.lock().chatted(&e.platform, &u.id, &u.display_name, now_ms(), &rules);
            *self.inner.viewers_dirty.lock() = true;
            for t in self.inner.timers.lock().values_mut() {
                t.1 += 1;
            }
        }
        let roles = e.user.as_ref().map(|u| u.roles.clone()).unwrap_or_default();
        let words: Vec<String> = e.text.split_whitespace().map(String::from).collect();
        for a in self.list().into_iter().filter(|a| a.enabled) {
            let (fires, args, cause) = match &a.trigger {
                Trigger::Event { kinds, platforms, min_amount, tests } => {
                    let Some(kind) = e.alert_kind() else { continue };
                    let ok = (kinds.is_empty() || kinds.iter().any(|k| k == kind || (k == "paid" && kind == "cheer")))
                        && (platforms.is_empty() || platforms.contains(&e.platform))
                        && amount_of(e) >= *min_amount
                        && (*tests || !e.test);
                    (ok, Vec::new(), format!("{} ({kind})", e.name()))
                }
                Trigger::Command { name, aliases, who } => {
                    if e.kind != Kind::Message {
                        continue;
                    }
                    let Some(first) = words.first() else { continue };
                    let hit = first.eq_ignore_ascii_case(name) || aliases.iter().any(|al| first.eq_ignore_ascii_case(al));
                    (hit && allowed(*who, &roles), words[1..].to_vec(), format!("{}: {}", e.name(), e.text))
                }
                Trigger::Chat { contains, first_time: only_first } => {
                    if e.kind != Kind::Message {
                        continue;
                    }
                    let ok = (contains.is_empty() || e.text.to_lowercase().contains(&contains.to_lowercase())) && (!only_first || first_time);
                    (ok, words.clone(), format!("{}: {}", e.name(), e.text))
                }
                Trigger::Timer { .. }
                | Trigger::Manual
                | Trigger::Output { .. }
                | Trigger::Start
                | Trigger::Hotkey { .. }
                | Trigger::File { .. }
                | Trigger::Midi { .. }
                | Trigger::Voice { .. }
                | Trigger::Controller { .. } => continue,
            };
            if !fires || !self.cooled(&a, e) {
                continue;
            }
            let ctx = self.with_state(context(Some(e), &args), Some(e));
            if a.conditions.iter().all(|c| check(c, &ctx)) {
                self.queue(a, ctx, cause);
            }
        }
    }

    /// Past its cooldowns (and starts them).
    fn cooled(&self, a: &Automation, e: &ChatEvent) -> bool {
        let now = Instant::now();
        if a.cooldown > 0.0
            && let Some(t) = self.inner.last_run.lock().get(&a.id)
            && now.duration_since(*t).as_secs_f64() < a.cooldown
        {
            return false;
        }
        let who = e.user.as_ref().map(|u| (a.id.clone(), format!("{}:{}", e.platform, u.id)));
        if a.user_cooldown > 0.0
            && let Some(k) = &who
            && let Some(t) = self.inner.user_last.lock().get(k)
            && now.duration_since(*t).as_secs_f64() < a.user_cooldown
        {
            return false;
        }
        self.inner.last_run.lock().insert(a.id.clone(), now);
        if let Some(k) = who {
            self.inner.user_last.lock().insert(k, now);
        }
        true
    }

    /// Streaming or recording started or stopped: the automations waiting for it.
    pub fn output_changed(&self, what: &str, started: bool) {
        let state = if started { "start" } else { "stop" };
        for a in self.list().into_iter().filter(|a| a.enabled) {
            if matches!(&a.trigger, Trigger::Output { what: w, state: s } if w == what && s == state) {
                let ctx = self.with_state(context(None, &[]), None);
                if a.conditions.iter().all(|c| check(c, &ctx)) {
                    let cause = a.trigger.summary();
                    self.queue(a, ctx, cause);
                }
            }
        }
    }

    /// Runs the enabled automations whose trigger `hit` accepts, with `extra` merged into what
    /// their steps see.
    fn signal(&self, hit: impl Fn(&Trigger) -> bool, extra: Value, cause: &str) {
        for a in self.list().into_iter().filter(|a| a.enabled && hit(&a.trigger)) {
            let mut ctx = self.with_state(context(None, &[]), None);
            if let (Some(c), Value::Object(e)) = (ctx.as_object_mut(), &extra) {
                c.extend(e.clone());
            }
            if a.conditions.iter().all(|c| check(c, &ctx)) {
                self.queue(a, ctx, cause.to_string());
            }
        }
    }

    /// The files automations watch.
    pub fn watched_files(&self) -> Vec<String> {
        self.list().into_iter().filter(|a| a.enabled).filter_map(|a| match a.trigger {
            Trigger::File { path } if !path.trim().is_empty() => Some(path),
            _ => None,
        }).collect()
    }

    /// Whether any automation listens to MIDI / game controllers (devices are opened only then).
    pub fn wants(&self, what: &str) -> bool {
        self.list().iter().any(|a| {
            a.enabled && matches!((&a.trigger, what), (Trigger::Midi { .. }, "midi") | (Trigger::Controller { .. }, "controller") | (Trigger::Voice { .. }, "voice"))
        })
    }

    /// `path` changed; `text`: what it holds now (the start of it).
    pub fn file_changed(&self, path: &str, text: &str) {
        let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string();
        self.signal(|t| matches!(t, Trigger::File { path: p } if p == path), json!({"file": {"path": path, "name": name, "text": text}}), &format!("{name} changed"));
    }

    /// A MIDI message: `kind` "note" (pressed) or "cc", `channel` 1–16.
    pub fn midi(&self, kind: &str, channel: u8, number: u8, value: u8, device: &str) {
        self.signal(
            |t| matches!(t, Trigger::Midi { message, number: n, channel: c } if message == kind && (*n < 0 || *n == number as i32) && (*c == 0 || *c == channel)),
            json!({"midi": {"kind": kind, "channel": channel, "number": number, "value": value, "device": device}}),
            &format!("MIDI {kind} {number}"),
        );
    }

    /// The phrases automations listen for (lowercase).
    pub fn voice_phrases(&self) -> Vec<String> {
        self.list()
            .into_iter()
            .filter(|a| a.enabled)
            .filter_map(|a| match a.trigger {
                Trigger::Voice { phrase } if !phrase.trim().is_empty() => Some(phrase.trim().to_lowercase()),
                _ => None,
            })
            .collect()
    }

    /// `phrase` was said (`heard`: the whole utterance).
    pub fn said(&self, phrase: &str, heard: &str) {
        self.signal(
            |t| matches!(t, Trigger::Voice { phrase: p } if p.trim().eq_ignore_ascii_case(phrase)),
            json!({"voice": {"phrase": phrase, "heard": heard}}),
            &format!("You said “{phrase}”"),
        );
    }

    /// A game controller's button was pressed.
    pub fn controller(&self, button: &str, device: &str) {
        self.signal(
            |t| matches!(t, Trigger::Controller { button: b } if b.is_empty() || b.eq_ignore_ascii_case(button)),
            json!({"controller": {"button": button, "device": device}}),
            &format!("Controller {button}"),
        );
    }

    /// Lightair started: the automations waiting for it.
    pub fn started(&self) {
        for a in self.list().into_iter().filter(|a| a.enabled && matches!(a.trigger, Trigger::Start)) {
            let ctx = self.with_state(context(None, &[]), None);
            if a.conditions.iter().all(|c| check(c, &ctx)) {
                self.queue(a, ctx, "Lightair started".into());
            }
        }
    }

    fn tick(&self) {
        let now = Instant::now();
        for a in self.list().into_iter().filter(|a| a.enabled) {
            let Trigger::Timer { minutes, min_messages } = a.trigger else { continue };
            let due = {
                let mut timers = self.inner.timers.lock();
                let t = timers.entry(a.id.clone()).or_insert((now, 0));
                if now.duration_since(t.0).as_secs_f64() >= minutes.max(0.1) * 60.0 {
                    let enough = t.1 >= min_messages;
                    *t = (now, 0);
                    enough
                } else {
                    false
                }
            };
            if due {
                let ctx = self.with_state(context(None, &[]), None);
                if a.conditions.iter().all(|c| check(c, &ctx)) {
                    let cause = a.trigger.summary();
                    self.queue(a, ctx, cause);
                }
            }
        }
    }

    fn queue(&self, automation: Automation, ctx: Value, cause: String) {
        let job = Job { automation, ctx, cause };
        if job.automation.queue.is_empty() {
            let me = self.clone();
            let _ = std::thread::Builder::new().name("lightair-automation-run".into()).spawn(move || me.execute(job));
            return;
        }
        let mut queues = self.inner.queues.lock();
        let tx = queues.entry(job.automation.queue.clone()).or_insert_with(|| {
            let (tx, rx) = unbounded::<Job>();
            let me = self.clone();
            let _ = std::thread::Builder::new().name("lightair-automation-queue".into()).spawn(move || {
                while let Ok(job) = rx.recv() {
                    me.execute(job);
                }
            });
            tx
        });
        let _ = tx.send(job);
    }

    fn execute(&self, job: Job) {
        let Job { automation, mut ctx, cause } = job;
        let index = {
            let mut h = self.inner.history.lock();
            h.push_back(Run { automation: automation.id.clone(), name: automation.name.clone(), started_ms: now_ms(), cause, steps: Vec::new(), ok: true, done: false });
            while h.len() > HISTORY {
                h.pop_front();
            }
            h.len() - 1
        };
        let started = self.inner.history.lock().get(index).map(|r| r.started_ms);
        let mut count = 0;
        let ok = self.steps(&automation.steps, &mut ctx, &mut count, started.unwrap_or(0));
        let mut h = self.inner.history.lock();
        if let Some(r) = h.iter_mut().rev().find(|r| Some(r.started_ms) == started && r.automation == automation.id) {
            r.ok = ok.is_ok();
            r.done = true;
        }
        drop(h);
        self.changed();
    }

    fn log(&self, started: i64, step: &Step, ok: bool, detail: String) {
        let mut h = self.inner.history.lock();
        if let Some(r) = h.iter_mut().rev().find(|r| r.started_ms == started && !r.done) {
            r.steps.push(StepLog { step: step.label(), ok, detail });
        }
    }

    /// Runs `steps`; `Err(())` stops the run (a Stop step, or too many steps).
    fn steps(&self, steps: &[Step], ctx: &mut Value, count: &mut usize, started: i64) -> Result<(), ()> {
        for step in steps {
            *count += 1;
            if *count > MAX_STEPS {
                self.log(started, step, false, "too many steps: stopped".into());
                return Err(());
            }
            let result = self.step(step, ctx, count, started);
            match result {
                Ok(Some(detail)) => self.log(started, step, true, detail),
                Ok(None) => {}
                Err(StepError::Stop) => {
                    self.log(started, step, true, "stopped here".into());
                    return Err(());
                }
                Err(StepError::Failed(m)) => self.log(started, step, false, m),
            }
        }
        Ok(())
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, StepError> {
        (self.inner.host)(method, params).map_err(StepError::Failed)
    }

    fn step(&self, step: &Step, ctx: &mut Value, count: &mut usize, started: i64) -> Result<Option<String>, StepError> {
        match step {
            Step::Call { method, params } => {
                let out = self.call(method, render_value(params, ctx))?;
                ctx["result"] = out.clone();
                Ok(Some(text_of(&out).chars().take(200).collect()))
            }
            Step::Chat { text, platform } => {
                let text = render(text, ctx);
                let platform = if platform.is_empty() { ctx["platform"].as_str().unwrap_or("").to_string() } else { platform.clone() };
                let reply = ctx["event"]["native_id"].clone();
                self.call("chat.send", json!({"text": text, "platform": platform, "channel": ctx["channel"], "reply_to": reply}))?;
                Ok(Some(text))
            }
            Step::Speak { text } => {
                let text = render(text, ctx);
                self.call("widgets.speak", json!({"text": text}))?;
                Ok(Some(text))
            }
            Step::Sound { file, volume } => {
                self.call("widgets.sound", json!({"file": render(file, ctx), "volume": volume}))?;
                Ok(Some(String::new()))
            }
            Step::Wait { seconds } => {
                std::thread::sleep(Duration::from_secs_f64(seconds.clamp(0.0, 3600.0)));
                Ok(None)
            }
            Step::Var { name, op, value, user } => {
                let value = render(value, ctx);
                let key = if *user {
                    format!("user:{}:{}", ctx["platform"].as_str().unwrap_or(""), ctx["user"]["id"].as_str().unwrap_or(""))
                } else {
                    String::new()
                };
                let mut doc = self.inner.doc.lock();
                let vars = if *user {
                    let entry = doc.vars.entry(key).or_insert(json!({}));
                    if !entry.is_object() {
                        *entry = json!({});
                    }
                    entry.as_object_mut().unwrap()
                } else {
                    &mut doc.vars
                };
                let next = match op.as_str() {
                    "add" => {
                        let now = vars.get(name).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0.0);
                        Some(json!(now + value.trim().parse::<f64>().unwrap_or(0.0)))
                    }
                    "clear" => None,
                    "push" => {
                        let mut list = vars.get(name).and_then(|v| v.as_array().cloned()).unwrap_or_default();
                        list.push(json!(value));
                        Some(Value::Array(list))
                    }
                    _ => Some(value.trim().parse::<f64>().map_or_else(|_| json!(value), |n| json!(n))),
                };
                match &next {
                    Some(v) => {
                        vars.insert(name.clone(), v.clone());
                    }
                    None => {
                        vars.remove(name);
                    }
                }
                let shown = next.as_ref().map(text_of).unwrap_or_default();
                let all = doc.vars.clone();
                drop(doc);
                self.save();
                if *user {
                    ctx["user_vars"][name.as_str()] = next.unwrap_or(Value::Null);
                } else {
                    ctx["vars"] = Value::Object(all);
                }
                Ok(Some(format!("{name} = {shown}")))
            }
            Step::Points { amount } => {
                let amount: f64 = render(amount, ctx).trim().parse().map_err(|_| StepError::Failed("not a number".into()))?;
                let (p, id, name) = (ctx["platform"].as_str().unwrap_or("").to_string(), ctx["user"]["id"].as_str().unwrap_or("").to_string(), ctx["name"].as_str().unwrap_or("").to_string());
                if id.is_empty() {
                    return Err(StepError::Failed("no one to give points to".into()));
                }
                let total = self.inner.viewers.lock().add_points(&p, &id, &name, amount);
                *self.inner.viewers_dirty.lock() = true;
                ctx["viewer"]["points"] = json!(total);
                Ok(Some(format!("{name}: {total}")))
            }
            Step::If { when, then, otherwise } => {
                let yes = when.iter().all(|c| check(c, ctx));
                let branch = if yes { then } else { otherwise };
                self.steps(branch, ctx, count, started).map_err(|_| StepError::Stop)?;
                Ok(Some(if yes { "yes".into() } else { "no".into() }))
            }
            Step::Random { choices } => {
                if choices.is_empty() {
                    return Ok(None);
                }
                let i = template::random_below(choices.len() as u64) as usize;
                self.steps(&choices[i], ctx, count, started).map_err(|_| StepError::Stop)?;
                Ok(Some(format!("choice {}", i + 1)))
            }
            Step::Run { automation } => {
                let a = self.get(&render(automation, ctx)).ok_or_else(|| StepError::Failed(format!("no automation “{automation}”")))?;
                self.steps(&a.steps, ctx, count, started).map_err(|_| StepError::Stop)?;
                Ok(Some(a.name))
            }
            Step::Http { url, method, body } => {
                let url = render(url, ctx);
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
                    .timeout_global(Some(Duration::from_secs(15)))
                    .http_status_as_error(false)
                    .build()
                    .into();
                let res = if method.eq_ignore_ascii_case("POST") {
                    agent.post(&url).send(render(body, ctx))
                } else {
                    agent.get(&url).call()
                };
                let mut res = res.map_err(|e| StepError::Failed(e.to_string()))?;
                let status = res.status().as_u16();
                let text = res.body_mut().read_to_string().unwrap_or_default();
                ctx["http"] = json!(text);
                ctx["http_json"] = serde_json::from_str(&text).unwrap_or(Value::Null);
                if status >= 400 {
                    return Err(StepError::Failed(format!("HTTP {status}")));
                }
                Ok(Some(format!("HTTP {status}")))
            }
            Step::Script { code } => {
                let out = self.call("script.run", json!({"code": code, "ctx": ctx.clone()}))?;
                ctx["script"] = out.clone();
                if out["stop"].as_bool() == Some(true) {
                    return Err(StepError::Stop);
                }
                Ok(Some(text_of(&out).chars().take(200).collect()))
            }
            Step::Stop => Err(StepError::Stop),
        }
    }
}

enum StepError {
    Stop,
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightair_live::{Channel, User};

    fn wait_for(f: impl Fn() -> bool) {
        for _ in 0..200 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    #[test]
    fn commands_run_steps_with_their_arguments() {
        let dir = std::env::temp_dir().join(format!("lightair-automation-test-{}", new_id()));
        let calls = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let seen = calls.clone();
        let host: Host = Arc::new(move |m, p| {
            seen.lock().push((m.to_string(), p));
            Ok(json!({}))
        });
        let hub = Hub::new();
        let a = Automations::start(hub.clone(), dir.clone(), host);
        a.set(Automation {
            id: "so".into(),
            name: "Shoutout".into(),
            enabled: true,
            trigger: Trigger::Command { name: "!so".into(), aliases: vec![], who: Who::Moderators },
            conditions: vec![],
            steps: vec![
                Step::Chat { text: "Go follow {{ target }}!".into(), platform: String::new() },
                Step::Var { name: "shoutouts".into(), op: "add".into(), value: "1".into(), user: false },
            ],
            queue: String::new(),
            cooldown: 0.0,
            user_cooldown: 0.0,
        });
        let channel = Channel { platform: "twitch".into(), channel: "me".into() };
        let msg = |roles: Vec<Role>| {
            let mut e = ChatEvent::new("twitch", "me", Kind::Message).with_fragments(lightair_live::event::text_fragments("!so @pal"));
            e.user = Some(User { id: "1".into(), display_name: "Mod".into(), roles, ..Default::default() });
            e
        };
        // not a moderator: nothing
        hub.inject(channel.clone(), msg(vec![]));
        hub.inject(channel, msg(vec![Role::Moderator]));
        wait_for(|| a.vars().contains_key("shoutouts"));
        let calls = calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "chat.send");
        assert_eq!(calls[0].1["text"], "Go follow pal!");
        assert_eq!(a.vars()["shoutouts"], json!(1.0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_midi_and_controllers_set_automations_off() {
        let dir = std::env::temp_dir().join(format!("lightair-automation-test-{}", new_id()));
        let host: Host = Arc::new(|_, _| Ok(json!({})));
        let a = Automations::start(Hub::new(), dir.clone(), host);
        let make = |id: &str, trigger: Trigger, value: &str| Automation {
            id: id.into(),
            name: id.into(),
            enabled: true,
            trigger,
            conditions: vec![],
            steps: vec![Step::Var { name: id.into(), op: "set".into(), value: value.into(), user: false }],
            queue: String::new(),
            cooldown: 0.0,
            user_cooldown: 0.0,
        };
        a.set(make("file", Trigger::File { path: "/tmp/deaths.txt".into() }, "{{ file.text }}"));
        a.set(make("pad", Trigger::Midi { message: "note".into(), number: 36, channel: 0 }, "{{ midi.value }}"));
        a.set(make("fader", Trigger::Midi { message: "cc".into(), number: -1, channel: 2 }, "{{ midi.number }}"));
        a.set(make("start", Trigger::Controller { button: "Start".into() }, "{{ controller.device }}"));
        assert_eq!(a.watched_files(), vec!["/tmp/deaths.txt".to_string()]);
        assert!(a.wants("midi") && a.wants("controller"));
        a.file_changed("/tmp/other.txt", "x");
        a.file_changed("/tmp/deaths.txt", "7");
        a.midi("note", 10, 37, 100, "Pads");
        a.midi("note", 10, 36, 90, "Pads");
        a.midi("cc", 1, 7, 50, "Faders");
        a.midi("cc", 2, 7, 50, "Faders");
        a.controller("South", "Pad");
        a.controller("start", "Pad");
        wait_for(|| ["file", "pad", "fader", "start"].iter().all(|k| a.vars().contains_key(*k)));
        let v = a.vars();
        assert_eq!((v["file"].clone(), v["pad"].clone(), v["fader"].clone(), v["start"].clone()), (json!(7.0), json!(90.0), json!(7.0), json!("Pad")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn conditions() {
        let ctx = json!({"value": 25, "name": "Mochi"});
        assert!(check(&Condition { left: "{{ value }}".into(), op: ">=".into(), right: "10".into() }, &ctx));
        assert!(!check(&Condition { left: "{{ name }}".into(), op: "is".into(), right: "sora".into() }, &ctx));
        assert!(check(&Condition { left: "{{ name }}".into(), op: "contains".into(), right: "och".into() }, &ctx));
    }
}
