//! Countdown, stopwatch and clock objects (docs/07-scenes-objects-canvas.md, "Timer"). The
//! document holds a `lightair_timer` source whose settings are text style plus timer settings;
//! libobs shows it with a text source whose text a tick callback rewrites once a second. Where
//! a timer is (running, paused, ended) is runtime state, never in the document or undo history.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use libobs_sys as sys;
use lightair_model::Id;
use parking_lot::Mutex;
use serde_json::Value;

use crate::obs::Data;

pub const TIMER_KIND: &str = "lightair_timer";
/// The libobs source that draws a timer.
pub const TIMER_SOURCE: &str = "text_ft2_source_v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerMode {
    Countdown,
    Stopwatch,
    Clock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimerConfig {
    pub mode: TimerMode,
    /// Countdown length.
    pub seconds: i64,
    /// Shown when a countdown ends (empty: 0:00).
    pub end_text: String,
    /// Start when the object first goes on air.
    pub autostart: bool,
    /// Clock: show seconds.
    pub show_seconds: bool,
}

impl TimerConfig {
    pub fn from_settings(s: &Value) -> Self {
        Self {
            mode: match s["mode"].as_str() {
                Some("stopwatch") => TimerMode::Stopwatch,
                Some("clock") => TimerMode::Clock,
                _ => TimerMode::Countdown,
            },
            seconds: s["seconds"].as_i64().unwrap_or(300).max(0),
            end_text: s["end_text"].as_str().unwrap_or("").to_string(),
            autostart: s["autostart"].as_bool().unwrap_or(true),
            show_seconds: s["show_seconds"].as_bool().unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimerAction {
    Start,
    Pause,
    Reset,
    /// Add (or with a negative value, remove) seconds.
    Add(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerStatus {
    pub running: bool,
    /// Countdown: seconds left. Stopwatch: seconds elapsed. Clock: 0.
    pub seconds: i64,
    pub ended: bool,
}

/// `h:mm:ss` from an hour up, `m:ss` below.
fn clock_text(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// What a timer shows. `elapsed` is whole seconds run; `now` is local (hour, minute, second).
pub fn timer_text(config: &TimerConfig, elapsed: i64, added: i64, ended: bool, now: (u32, u32, u32)) -> String {
    match config.mode {
        TimerMode::Countdown if ended && !config.end_text.is_empty() => config.end_text.clone(),
        // Round up so a 5:00 countdown shows 5:00 until a full second has passed.
        TimerMode::Countdown => clock_text(config.seconds + added - elapsed),
        TimerMode::Stopwatch => clock_text(elapsed + added),
        TimerMode::Clock if config.show_seconds => format!("{:02}:{:02}:{:02}", now.0, now.1, now.2),
        TimerMode::Clock => format!("{:02}:{:02}", now.0, now.1),
    }
}

struct Timer {
    /// Strong reference to the text source.
    source: *mut sys::obs_source_t,
    config: TimerConfig,
    running_since: Option<Instant>,
    banked: Duration,
    added: i64,
    ended: bool,
    started_once: bool,
    last_text: String,
}

impl Timer {
    fn elapsed(&self) -> Duration {
        self.banked + self.running_since.map_or(Duration::ZERO, |t| t.elapsed())
    }

    fn status(&self) -> TimerStatus {
        let e = self.elapsed().as_secs() as i64;
        let seconds = match self.config.mode {
            TimerMode::Countdown => (self.config.seconds + self.added - e).max(0),
            TimerMode::Stopwatch => e + self.added,
            TimerMode::Clock => 0,
        };
        TimerStatus { running: self.running_since.is_some(), seconds, ended: self.ended }
    }

    fn start(&mut self) {
        self.started_once = true;
        if self.running_since.is_none() && !self.ended {
            self.running_since = Some(Instant::now());
        }
    }

    fn pause(&mut self) {
        if let Some(t) = self.running_since.take() {
            self.banked += t.elapsed();
        }
    }

    /// Advances one tick: autostart, end detection, text. Returns true when a countdown just ended.
    fn tick(&mut self) -> bool {
        let mut just_ended = false;
        if self.config.autostart && !self.started_once && unsafe { sys::obs_source_active(self.source) } {
            self.start();
        }
        if self.config.mode == TimerMode::Countdown
            && self.running_since.is_some()
            && self.elapsed().as_secs() as i64 >= self.config.seconds + self.added
        {
            self.pause();
            self.ended = true;
            just_ended = true;
        }
        let now = {
            use chrono::Timelike;
            let t = chrono::Local::now();
            (t.hour(), t.minute(), t.second())
        };
        let text = timer_text(&self.config, self.elapsed().as_secs() as i64, self.added, self.ended, now);
        if text != self.last_text {
            let mut d = Data::new();
            d.set_str("text", &text);
            unsafe { sys::obs_source_update(self.source, d.as_ptr()) };
            self.last_text = text;
        }
        just_ended
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        unsafe { sys::obs_source_release(self.source) };
    }
}

#[derive(Default)]
pub(crate) struct Timers {
    map: Mutex<HashMap<Id, Timer>>,
    /// Countdowns that ended, for the app to run their end action.
    ended: Mutex<Vec<Id>>,
}

// SAFETY: the raw source pointers are strong libobs references; libobs sources are
// thread-safe to update and query.
unsafe impl Send for Timers {}
unsafe impl Sync for Timers {}

impl Timers {
    pub(crate) fn add(&self, id: Id, source: *mut sys::obs_source_t, settings: &Value) {
        let source = unsafe { sys::obs_source_get_ref(source) };
        if source.is_null() {
            return;
        }
        let timer = Timer {
            source,
            config: TimerConfig::from_settings(settings),
            running_since: None,
            banked: Duration::ZERO,
            added: 0,
            ended: false,
            started_once: false,
            last_text: String::new(),
        };
        self.map.lock().insert(id, timer);
    }

    pub(crate) fn configure(&self, id: Id, settings: &Value) {
        if let Some(t) = self.map.lock().get_mut(&id) {
            let config = TimerConfig::from_settings(settings);
            if config != t.config {
                if config.mode != t.config.mode || config.seconds != t.config.seconds {
                    t.ended = false;
                }
                t.config = config;
            }
            t.last_text.clear();
        }
    }

    pub(crate) fn remove(&self, id: Id) {
        self.map.lock().remove(&id);
    }

    pub(crate) fn clear(&self) {
        self.map.lock().clear();
        self.ended.lock().clear();
    }

    pub(crate) fn action(&self, id: Id, action: TimerAction) {
        let mut map = self.map.lock();
        let Some(t) = map.get_mut(&id) else { return };
        match action {
            TimerAction::Start => t.start(),
            TimerAction::Pause => t.pause(),
            TimerAction::Reset => {
                let running = t.running_since.is_some();
                t.running_since = running.then(Instant::now);
                t.banked = Duration::ZERO;
                t.added = 0;
                t.ended = false;
            }
            TimerAction::Add(s) => {
                t.added += s;
                if t.ended && t.config.mode == TimerMode::Countdown && t.status().seconds > 0 {
                    t.ended = false;
                }
            }
        }
        t.last_text.clear();
    }

    pub(crate) fn status(&self, id: Id) -> Option<TimerStatus> {
        self.map.lock().get(&id).map(Timer::status)
    }

    pub(crate) fn text(&self, id: Id) -> Option<String> {
        self.map.lock().get(&id).map(|t| t.last_text.clone())
    }

    pub(crate) fn take_ended(&self) -> Vec<Id> {
        std::mem::take(&mut *self.ended.lock())
    }

    /// Runs on libobs's tick (video thread); skips the frame rather than wait for a lock.
    fn tick(&self) {
        let Some(mut map) = self.map.try_lock() else { return };
        let ended: Vec<Id> = map.iter_mut().filter_map(|(id, t)| t.tick().then_some(*id)).collect();
        drop(map);
        if !ended.is_empty() {
            self.ended.lock().extend(ended);
        }
    }
}

pub(crate) unsafe extern "C" fn on_timers_tick(param: *mut std::ffi::c_void, _seconds: f32) {
    let timers = unsafe { &*(param as *const Timers) };
    timers.tick();
}

pub(crate) fn tick_param(timers: &Arc<Timers>) -> *mut std::ffi::c_void {
    Arc::as_ptr(timers) as *mut _
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text() {
        let c = TimerConfig::from_settings(&json!({"mode": "countdown", "seconds": 300}));
        assert_eq!(timer_text(&c, 0, 0, false, (0, 0, 0)), "5:00");
        assert_eq!(timer_text(&c, 61, 0, false, (0, 0, 0)), "3:59");
        assert_eq!(timer_text(&c, 400, 0, true, (0, 0, 0)), "0:00");
        let c = TimerConfig { end_text: "Starting!".into(), ..c };
        assert_eq!(timer_text(&c, 300, 0, true, (0, 0, 0)), "Starting!");
        let s = TimerConfig::from_settings(&json!({"mode": "stopwatch"}));
        assert_eq!(timer_text(&s, 3725, 0, false, (0, 0, 0)), "1:02:05");
        let k = TimerConfig::from_settings(&json!({"mode": "clock", "show_seconds": true}));
        assert_eq!(timer_text(&k, 0, 0, false, (9, 5, 7)), "09:05:07");
    }
}
