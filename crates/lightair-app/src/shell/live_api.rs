//! The remote API's live methods (docs/plans/live-integrations.md): chat, widgets and
//! automations. Automations' steps come through here too (as the "Automation" client), so a
//! step can do anything a remote client can.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use lightair_automation::{Automation, Automations, Host, recipes};
use lightair_remote::{ApiError, Call, ErrorKind, Reply};
use serde_json::{Value, json};

use super::StudioWindow;

fn text<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Starts the automations; their steps call Lightair through `jobs` (the remote API's queue).
pub(super) fn start_automations(hub: &lightair_live::Hub, jobs: super::remote::JobSender) -> Automations {
    // Twitch channel-points redemptions, while signed in to Twitch officially
    {
        let hub = hub.clone();
        let _ = std::thread::Builder::new()
            .name("lightair-twitch-events".into())
            .spawn(move || lightair_live::official::watch_twitch_events(hub, Arc::new(std::sync::atomic::AtomicBool::new(false))));
    }
    let chat = hub.clone();
    let host: Host = Arc::new(move |method: &str, params: Value| {
        // (sending takes a moment: on the automation's own thread)
        if method == "chat.send" {
            return crate::chat_send::send(&chat, &params);
        }
        if let Some(out) = lightair_live::integrations::call(method, &params) {
            return out;
        }
        if method == "script.run" {
            return lightair_live::pages::run_script(params["code"].as_str().unwrap_or(""), &params["ctx"], Duration::from_secs(10));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let call = Call { method: method.to_string(), params, client: "Automation", device: None };
        jobs.unbounded_send((call, tx)).map_err(|_| "Lightair is closing".to_string())?;
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e.message),
            Err(_) => Err("Lightair didn't answer in time".into()),
        }
    });
    lightair_live::integrations::set_dir(lightair_platform::paths::data_dir().join("integrations"));
    lightair_live::rules::set_dir(lightair_platform::paths::data_dir().join("rules"));
    // official sign-ins' tokens: the Keychain
    lightair_live::oauth::set_store(
        Box::new(crate::secrets::connection_token),
        Box::new(|k, v| {
            let _ = crate::secrets::set_connection_token(k, v);
        }),
    );
    Automations::start(hub.clone(), lightair_platform::paths::data_dir().join("automations"), host)
}

/// The points leaderboard and the giveaway's entrants, for the widgets that show them (only
/// while one exists).
pub(super) fn feed_widgets(automations: Automations, widgets: Option<lightair_widgets::Widgets>) {
    crate::triggers::watch(automations.clone());
    // automations: when streaming or recording starts or stops, and once at start
    {
        let a = automations.clone();
        let _ = std::thread::Builder::new().name("lightair-automation-outputs".into()).spawn(move || {
            use lightair_engine::outputs::runtime;
            std::thread::sleep(Duration::from_secs(2));
            a.started();
            let (mut streaming, mut recording) = (runtime().any_stream_running(), runtime().any_recording_running());
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let (s, r) = (runtime().any_stream_running(), runtime().any_recording_running());
                if s != streaming {
                    streaming = s;
                    a.output_changed("stream", s);
                    // X: going live there (with its title) once it sees the video; ending it
                    if lightair_live::official::signed_in("x") {
                        std::thread::spawn(move || {
                            let r = if s { lightair_live::official::x_go_live().map(|_| ()) } else { lightair_live::official::x_end() };
                            if let Err(e) = r {
                                tracing::warn!(%e, "X broadcast");
                            }
                        });
                    }
                }
                if r != recording {
                    recording = r;
                    a.output_changed("record", r);
                }
            }
        });
    }
    let Some(widgets) = widgets else { return };
    // an automation's web address (for Stream Deck's Website action and the like)
    {
        let file = lightair_platform::paths::data_dir().join("automations").join("run-key");
        let key = std::fs::read_to_string(&file).ok().map(|k| k.trim().to_string()).filter(|k| k.len() >= 32).unwrap_or_else(|| {
            let mut b = [0u8; 16];
            let _ = getrandom::fill(&mut b);
            let k: String = b.iter().map(|x| format!("{x:02x}")).collect();
            let _ = std::fs::write(&file, &k);
            k
        });
        let a = automations.clone();
        widgets.set_run_hook(key, Box::new(move |name| a.run(name, None, &[])));
    }
    {
        let a = automations.clone();
        widgets.set_points_hook(Box::new(move |platform, id, name, delta| a.change_points(platform, id, name, delta)));
    }
    let _ = std::thread::Builder::new().name("lightair-widget-feed".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(4));
            if widgets.has_kind("leaderboard") {
                let top: Vec<Value> = automations.top_viewers(20).into_iter().map(|v| json!({"name": v.name, "points": v.points, "platform": v.platform})).collect();
                widgets.set_data("leaderboard", Value::Array(top));
            }
            if widgets.has_kind("wheel") {
                widgets.set_data("giveaway", automations.vars().get("giveaway").cloned().unwrap_or(json!([])));
            }
        }
    });
}

impl StudioWindow {
    /// LIGHTAIR_TEST_EVENTS ("sub,paid,…"), sent 4 s apart (the screenshot hooks).
    pub(super) fn send_test_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tests: Vec<String> = std::env::var("LIGHTAIR_TEST_EVENTS").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(String::from).collect();
        cx.spawn_in(window, async move |this, cx| {
            for kind in tests {
                cx.background_executor().timer(Duration::from_millis(4000)).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(w) = &this.studio.read(cx).widgets {
                        w.test(&kind, "twitch");
                    }
                });
            }
        })
        .detach();
    }

    /// A live method's answer, or `None` (not one of these).
    pub(super) fn live_call(&mut self, call: &Call, cx: &mut Context<Self>) -> Option<Reply> {
        let p = &call.params;
        let studio = self.studio.read(cx);
        let widgets = studio.widgets.clone();
        let hub = studio.chat.clone();
        let no_widgets = || ApiError::failed("widgets aren't available");
        Some(match call.method.as_str() {
            "chat.history" => {
                let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
                let h = hub.history();
                let start = h.len().saturating_sub(limit);
                Ok(json!({"events": &h[start..], "channels": hub.status().into_iter().map(|(c, s)| json!({"platform": c.platform, "channel": c.channel, "status": s})).collect::<Vec<_>>()}))
            }
            // the server and automations send on their own threads; anything else, off this one
            "chat.send" => {
                let (hub, p) = (hub.clone(), p.clone());
                std::thread::spawn(move || {
                    if let Err(e) = crate::chat_send::send(&hub, &p) {
                        tracing::warn!(%e, "chat not sent");
                    }
                });
                Ok(json!({"queued": true}))
            }
            "music.now" => Ok(widgets.as_ref().map(|w| w.data("now_playing")).filter(|v| !v.is_null()).unwrap_or_else(crate::now_playing::now)),
            "widgets.list" => widgets.map(|w| json!({"widgets": w.list().into_iter().map(|(id, kind)| json!({"id": id, "kind": kind})).collect::<Vec<_>>(), "session": w.session()})).ok_or_else(no_widgets),
            "widgets.test" => {
                let w = widgets.ok_or_else(no_widgets);
                w.map(|w| {
                    w.test(text(p, "kind").unwrap_or("follow"), text(p, "platform").unwrap_or("twitch"));
                    json!({})
                })
            }
            "widgets.button" => {
                let (Some(w), Some(id), Some(button)) = (widgets, text(p, "widget"), text(p, "button")) else {
                    return Some(Err(ApiError::invalid("widget and button are needed")));
                };
                w.button(id, button);
                Ok(json!({}))
            }
            "widgets.speak" => match widgets {
                Some(w) if w.speak(text(p, "text").unwrap_or("")) => Ok(json!({})),
                Some(_) => Err(ApiError::new(ErrorKind::State, "there's no alert box on air to say it (add one in Add Object › Widgets)")),
                None => Err(no_widgets()),
            },
            "widgets.sound" => match widgets {
                Some(w) if w.sound(text(p, "file").unwrap_or(""), p.get("volume").and_then(Value::as_f64).unwrap_or(100.0)) => Ok(json!({})),
                Some(_) => Err(ApiError::new(ErrorKind::State, "there's no alert box on air to play it (add one in Add Object › Widgets)")),
                None => Err(no_widgets()),
            },
            "automation.list" => {
                let a = &self.automations;
                Ok(json!({
                    "automations": a.list(),
                    "recipes": recipes::RECIPES.iter().map(|r| json!({"key": r.key, "name": r.name, "about": r.about})).collect::<Vec<_>>(),
                    "vars": a.vars(),
                    "runs": a.history().into_iter().rev().take(30).collect::<Vec<_>>(),
                }))
            }
            "automation.set" => {
                let made = match (text(p, "recipe"), p.get("automation")) {
                    (Some(key), _) => recipes::make(key).ok_or_else(|| ApiError::not_found(format!("no recipe {key}"))),
                    (None, Some(v)) => {
                        let mut v = v.clone();
                        if v.get("id").and_then(Value::as_str).is_none_or(str::is_empty) {
                            v["id"] = json!(lightair_automation::new_id());
                        }
                        serde_json::from_value::<Automation>(v).map_err(|e| ApiError::invalid(format!("that isn't an automation: {e}")))
                    }
                    _ => Err(ApiError::invalid("automation or recipe is needed")),
                };
                made.map(|a| {
                    self.automations.set(a.clone());
                    cx.notify();
                    json!(a)
                })
            }
            "automation.remove" => {
                let key = text(p, "automation").unwrap_or("");
                match self.automations.get(key) {
                    Some(a) => {
                        self.automations.remove(&a.id);
                        cx.notify();
                        Ok(json!({}))
                    }
                    None => Err(ApiError::not_found(format!("no automation {key}"))),
                }
            }
            "automation.run" => {
                let args: Vec<String> = p.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                self.automations.run(text(p, "automation").unwrap_or(""), None, &args).map(|_| json!({})).map_err(ApiError::not_found)
            }
            "vars.set" => {
                let Some(name) = text(p, "name") else { return Some(Err(ApiError::invalid("name is needed"))) };
                self.automations.set_var(name, p.get("value").cloned().unwrap_or(Value::Null));
                Ok(json!({}))
            }
            _ => return None,
        })
    }
}

