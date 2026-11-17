//! obs-websocket v5 at `/` (protocol: github.com/obsproject/obs-websocket, docs/generated): the
//! requests OBS tools use most, answered from Lightair's methods, and Lightair's events in OBS's
//! names. In OBS's terms a Lightair scene is a scene, an object a scene item (its id a number
//! made from the object's), a source an input, and a mixer track its source's volume and mute.

use std::io;
use std::net::TcpStream;
use std::sync::Arc;

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::http::Request;
use crate::{ApiError, ErrorKind, Event, Shared};

/// The obs-websocket version this speaks like.
const VERSION: &str = "5.5.2";
const RPC: u64 = 1;

// close codes
const NOT_IDENTIFIED: u16 = 4007;
const AUTH_FAILED: u16 = 4009;
const UNSUPPORTED_RPC: u16 = 4010;
const DECODE: u16 = 4002;

// request status codes
const OK: u64 = 100;
const UNKNOWN_REQUEST: u64 = 204;
const MISSING_FIELD: u64 = 300;
const INVALID_FIELD: u64 = 402;
const OUTPUT_RUNNING: u64 = 500;
const OUTPUT_NOT_RUNNING: u64 = 501;
const STUDIO_MODE_OFF: u64 = 506;
const NOT_FOUND: u64 = 600;
const ACTION_FAILED: u64 = 702;

// event subscriptions
const SUB_GENERAL: u64 = 1;
const SUB_SCENES: u64 = 1 << 2;
const SUB_INPUTS: u64 = 1 << 3;
const SUB_OUTPUTS: u64 = 1 << 6;
const SUB_SCENE_ITEMS: u64 = 1 << 7;
const SUB_ALL: u64 = 0x7FF;

type Status = (u64, String);

pub(crate) fn run(stream: TcpStream, req: &Request, shared: &Arc<Shared>, peer: String) -> io::Result<()> {
    let ws = crate::ws::accept(stream, req, Some("obswebsocket.json"))?;
    let joined = shared.join("obs-websocket", peer);
    let auth = shared.password().map(|_| (crate::random_b64(32), crate::random_b64(32)));
    let session = Session { shared: shared.clone(), auth, identified: false, events: SUB_ALL };
    crate::ws::run(ws, joined, shared, session)
}

struct Session {
    shared: Arc<Shared>,
    /// The challenge and salt sent, when a password is asked.
    auth: Option<(String, String)>,
    identified: bool,
    events: u64,
}

fn b64_sha256(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

/// What obs-websocket clients send for `password` given the Hello's `challenge` and `salt`.
pub(crate) fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let secret = b64_sha256(&[password, salt]);
    b64_sha256(&[&secret, challenge])
}

fn msg(op: u64, d: Value) -> String {
    json!({"op": op, "d": d}).to_string()
}

impl crate::ws::Protocol for Session {
    fn greeting(&mut self) -> Option<String> {
        let mut d = json!({"obsWebSocketVersion": VERSION, "rpcVersion": RPC});
        if let Some((challenge, salt)) = &self.auth {
            d["authentication"] = json!({"challenge": challenge, "salt": salt});
        }
        Some(msg(0, d))
    }

    fn message(&mut self, text: &str) -> Result<Vec<String>, (u16, String)> {
        let m: Value = serde_json::from_str(text).map_err(|e| (DECODE, e.to_string()))?;
        let op = m.get("op").and_then(Value::as_u64).ok_or((DECODE, "no op".to_string()))?;
        let d = m.get("d").cloned().unwrap_or(json!({}));
        match op {
            // Identify, Reidentify
            1 | 3 => {
                if op == 1 {
                    if d.get("rpcVersion").and_then(Value::as_u64).is_some_and(|v| v != RPC) {
                        return Err((UNSUPPORTED_RPC, "rpcVersion 1 only".into()));
                    }
                    if let (Some((challenge, salt)), Some(password)) = (&self.auth, self.shared.password()) {
                        let given = d.get("authentication").and_then(Value::as_str).unwrap_or("");
                        if !crate::same(given.as_bytes(), auth_string(password, salt, challenge).as_bytes()) {
                            return Err((AUTH_FAILED, "Authentication failed.".into()));
                        }
                    }
                    self.identified = true;
                } else if !self.identified {
                    return Err((NOT_IDENTIFIED, "Identify first".into()));
                }
                if let Some(e) = d.get("eventSubscriptions").and_then(Value::as_u64) {
                    self.events = e;
                }
                Ok(vec![msg(2, json!({"negotiatedRpcVersion": RPC}))])
            }
            _ if !self.identified => Err((NOT_IDENTIFIED, "Identify first".into())),
            // Request
            6 => Ok(vec![msg(7, self.respond(&d))]),
            // RequestBatch
            8 => {
                let halt = d.get("haltOnFailure").and_then(Value::as_bool).unwrap_or(false);
                let mut results = vec![];
                for r in d.get("requests").and_then(Value::as_array).cloned().unwrap_or_default() {
                    let out = self.respond(&r);
                    let failed = out["requestStatus"]["result"] == false;
                    results.push(out);
                    if failed && halt {
                        break;
                    }
                }
                Ok(vec![msg(9, json!({"requestId": d.get("requestId").cloned().unwrap_or(Value::Null), "results": results}))])
            }
            _ => Ok(vec![]),
        }
    }

    fn event(&mut self, event: &Event) -> Option<String> {
        if !self.identified {
            return None;
        }
        let (sub, kind, data) = translate(event)?;
        (self.events & sub != 0).then(|| msg(5, json!({"eventType": kind, "eventIntent": sub, "eventData": data})))
    }
}

/// Lightair's event in OBS's name, with OBS's fields.
fn translate(e: &Event) -> Option<(u64, &'static str, Value)> {
    let d = &e.data;
    let s = |k: &str| d.get(k).cloned().unwrap_or(Value::Null);
    let output = |active: bool, paused: Option<bool>| {
        let state = match (active, paused) {
            (true, Some(true)) => "OBS_WEBSOCKET_OUTPUT_PAUSED",
            (true, _) => "OBS_WEBSOCKET_OUTPUT_STARTED",
            (false, _) => "OBS_WEBSOCKET_OUTPUT_STOPPED",
        };
        json!({"outputActive": active, "outputState": state})
    };
    let active = d.get("active").and_then(Value::as_bool).unwrap_or(false);
    Some(match e.name.as_str() {
        "scene.program" => {
            (SUB_SCENES, "CurrentProgramSceneChanged", json!({"sceneName": s("name"), "sceneUuid": s("id")}))
        }
        "scene.list" => {
            let scenes = scene_list(d.get("scenes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]));
            (SUB_SCENES, "SceneListChanged", json!({"scenes": scenes}))
        }
        "record.state" => {
            let mut o = output(active, d.get("paused").and_then(Value::as_bool));
            o["outputPath"] = s("path");
            (SUB_OUTPUTS, "RecordStateChanged", o)
        }
        "stream.state" => (SUB_OUTPUTS, "StreamStateChanged", output(active, None)),
        "vcam.state" => (SUB_OUTPUTS, "VirtualcamStateChanged", output(active, None)),
        "clip.saved" => (SUB_OUTPUTS, "ReplayBufferSaved", json!({"savedReplayPath": s("path")})),
        "audio.mute" => (
            SUB_INPUTS,
            "InputMuteStateChanged",
            json!({"inputName": s("source_name"), "inputUuid": s("source"), "inputMuted": s("muted")}),
        ),
        "audio.volume" => {
            let db = d.get("volume_db").and_then(Value::as_f64).unwrap_or(0.0);
            (
                SUB_INPUTS,
                "InputVolumeChanged",
                json!({"inputName": s("source_name"), "inputUuid": s("source"), "inputVolumeDb": db, "inputVolumeMul": mul(db)}),
            )
        }
        "object.visible" => (
            SUB_SCENE_ITEMS,
            "SceneItemEnableStateChanged",
            json!({"sceneName": s("scene_name"), "sceneUuid": s("scene"), "sceneItemId": item_id(d.get("object").and_then(Value::as_str).unwrap_or("")), "sceneItemEnabled": s("visible")}),
        ),
        "app.exit" => (SUB_GENERAL, "ExitStarted", json!({})),
        _ => return None,
    })
}

fn mul(db: f64) -> f64 {
    if db <= -100.0 { 0.0 } else { 10f64.powf(db / 20.0) }
}

fn db(mul: f64) -> f64 {
    if mul <= 0.0 { -100.0 } else { 20.0 * mul.log10() }
}

/// A scene item's number: the object's id folded to 31 bits (the same every time).
pub(crate) fn item_id(object: &str) -> u64 {
    // FNV-1a
    let mut h: u64 = 0xcbf29ce484222325;
    for b in object.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h % 0x7FFF_FFFE) + 1
}

/// OBS lists scenes bottom first, each with its index from the bottom.
fn scene_list(scenes: &[Value]) -> Vec<Value> {
    let n = scenes.len();
    scenes
        .iter()
        .enumerate()
        .rev()
        .map(|(i, s)| json!({"sceneIndex": n - 1 - i, "sceneName": s["name"], "sceneUuid": s["id"]}))
        .collect()
}

/// OBS's names for Lightair's own kinds (as OBS on this platform calls them).
fn obs_kind(kind: &str) -> &str {
    if cfg!(windows) {
        return match kind {
            "lightair_screen" => "monitor_capture",
            "lightair_camera" => "dshow_input",
            "lightair_mic" | "lightair_audio_device" => "wasapi_input_capture",
            "lightair_desktop_audio" => "wasapi_output_capture",
            "lightair_app_audio" => "wasapi_process_output_capture",
            "lightair_web" => "browser_source",
            k => k,
        };
    }
    match kind {
        "lightair_screen" => "screen_capture",
        "lightair_camera" => "macos-avcapture",
        "lightair_mic" | "lightair_audio_device" => "coreaudio_input_capture",
        "lightair_desktop_audio" | "lightair_app_audio" => "sck_audio_capture",
        "lightair_web" => "browser_source",
        k => k,
    }
}

fn status_of(e: ApiError) -> Status {
    let code = match e.kind {
        ErrorKind::Unknown => UNKNOWN_REQUEST,
        ErrorKind::Invalid => INVALID_FIELD,
        ErrorKind::NotFound => NOT_FOUND,
        ErrorKind::State => ACTION_FAILED,
        ErrorKind::Failed => ACTION_FAILED,
    };
    (code, e.message)
}

fn field<'a>(d: &'a Value, name: &str) -> Result<&'a Value, Status> {
    d.get(name).filter(|v| !v.is_null()).ok_or((MISSING_FIELD, format!("Your request is missing the `{name}` field.")))
}

fn str_field<'a>(d: &'a Value, name: &str) -> Result<&'a str, Status> {
    field(d, name)?.as_str().ok_or((INVALID_FIELD, format!("`{name}` should be a string.")))
}

/// `sceneUuid` or `sceneName` (or `sourceName`/`sourceUuid` naming a scene).
fn scene_ref(d: &Value) -> Result<String, Status> {
    for k in ["sceneUuid", "sceneName", "sourceUuid", "sourceName"] {
        if let Some(v) = d.get(k).and_then(Value::as_str) {
            return Ok(v.to_string());
        }
    }
    Err((MISSING_FIELD, "Your request is missing the `sceneName` field.".into()))
}

fn input_ref(d: &Value) -> Result<String, Status> {
    for k in ["inputUuid", "inputName", "sourceUuid", "sourceName"] {
        if let Some(v) = d.get(k).and_then(Value::as_str) {
            return Ok(v.to_string());
        }
    }
    Err((MISSING_FIELD, "Your request is missing the `inputName` field.".into()))
}

fn named<'a>(list: &'a [Value], what: &str, key: &str) -> Option<&'a Value> {
    list.iter().find(|v| v["id"] == key).or_else(|| list.iter().find(|v| v["name"] == key)).filter(|_| !what.is_empty())
}

impl Session {
    fn call(&self, method: &str, params: Value) -> Result<Value, Status> {
        self.shared.call("obs-websocket", method, params).map_err(status_of)
    }

    fn state(&self) -> Result<Value, Status> {
        self.call("state.get", json!({}))
    }

    fn respond(&self, r: &Value) -> Value {
        let kind = r.get("requestType").and_then(Value::as_str).unwrap_or("");
        let data = r.get("requestData").cloned().unwrap_or(json!({}));
        let mut out = json!({"requestType": kind, "requestId": r.get("requestId").cloned().unwrap_or(Value::Null)});
        match self.request(kind, &data) {
            Ok(response) => {
                out["requestStatus"] = json!({"result": true, "code": OK});
                if !response.is_null() {
                    out["responseData"] = response;
                }
            }
            Err((code, comment)) => out["requestStatus"] = json!({"result": false, "code": code, "comment": comment}),
        }
        out
    }

    fn scene<'a>(&self, state: &'a Value, d: &Value) -> Result<&'a Value, Status> {
        let key = scene_ref(d)?;
        named(state["scenes"].as_array().map(Vec::as_slice).unwrap_or(&[]), "scene", &key)
            .ok_or((NOT_FOUND, format!("No scene was found by the name of `{key}`.")))
    }

    fn item<'a>(&self, scene: &'a Value, d: &Value) -> Result<&'a Value, Status> {
        let id = field(d, "sceneItemId")?.as_u64().ok_or((INVALID_FIELD, "`sceneItemId` should be a number.".into()))?;
        scene["objects"]
            .as_array()
            .and_then(|o| o.iter().find(|o| item_id(o["id"].as_str().unwrap_or("")) == id))
            .ok_or((NOT_FOUND, format!("No scene items were found in the specified scene by that ID ({id}).")))
    }

    fn track<'a>(&self, state: &'a Value, d: &Value) -> Result<(&'a Value, &'a Value), Status> {
        let key = input_ref(d)?;
        let input = named(state["sources"].as_array().map(Vec::as_slice).unwrap_or(&[]), "input", &key)
            .ok_or((NOT_FOUND, format!("No source was found by the name of `{key}`.")))?;
        let track = state["tracks"]
            .as_array()
            .and_then(|t| t.iter().find(|t| t["source"] == input["id"]))
            .ok_or((INVALID_FIELD, format!("The input `{key}` has no sound.")))?;
        Ok((input, track))
    }

    fn output_status(&self, which: &str) -> Result<Value, Status> {
        let s = self.state()?;
        Ok(s["outputs"][which].clone())
    }

    /// Starts/stops an output with OBS's checks (500 already running, 501 not running).
    fn output(&self, which: &str, method: &str, want: Option<bool>) -> Result<Value, Status> {
        let active = self.output_status(which)?["active"].as_bool().unwrap_or(false);
        let action = match want {
            Some(true) if active => return Err((OUTPUT_RUNNING, "The output is already running.".into())),
            Some(false) if !active => return Err((OUTPUT_NOT_RUNNING, "The output is not running.".into())),
            Some(true) => "start",
            Some(false) => "stop",
            None if active => "stop",
            None => "start",
        };
        let r = self.call(&format!("{method}.{action}"), json!({}))?;
        Ok(json!({"outputActive": action == "start", "outputPath": r.get("path").cloned().unwrap_or(Value::Null)}))
    }

    fn request(&self, kind: &str, d: &Value) -> Result<Value, Status> {
        Ok(match kind {
            "GetVersion" => {
                let v = self.call("app.version", json!({}))?;
                json!({
                    "obsVersion": "31.0.0",
                    "obsWebSocketVersion": VERSION,
                    "rpcVersion": RPC,
                    "availableRequests": REQUESTS,
                    "supportedImageFormats": ["png"],
                    "platform": std::env::consts::OS,
                    "platformDescription": format!("Lightair {}", v["version"].as_str().unwrap_or("")),
                })
            }
            "GetStats" => {
                let s = self.call("stats.get", json!({}))?;
                json!({
                    "cpuUsage": s["cpu_percent"],
                    "memoryUsage": s["memory_mb"],
                    "availableDiskSpace": s["disk_free_mb"],
                    "activeFps": s["fps"],
                    "averageFrameRenderTime": s["frame_time_ms"],
                    "renderSkippedFrames": s["lagged_frames"],
                    "renderTotalFrames": s["total_frames"],
                    "outputSkippedFrames": s["output_skipped_frames"],
                    "outputTotalFrames": s["output_total_frames"],
                    "webSocketSessionIncomingMessages": Value::Null,
                    "webSocketSessionOutgoingMessages": Value::Null,
                })
            }
            "Sleep" => {
                let ms = d.get("sleepMillis").and_then(Value::as_u64).unwrap_or(0).min(50_000);
                std::thread::sleep(std::time::Duration::from_millis(ms));
                Value::Null
            }
            "GetSceneList" => {
                let s = self.state()?;
                let scenes = scene_list(s["scenes"].as_array().map(Vec::as_slice).unwrap_or(&[]));
                json!({
                    "currentProgramSceneName": s["program"]["name"],
                    "currentProgramSceneUuid": s["program"]["id"],
                    "currentPreviewSceneName": Value::Null,
                    "currentPreviewSceneUuid": Value::Null,
                    "scenes": scenes,
                })
            }
            "GetGroupList" => json!({"groups": []}),
            "GetCurrentProgramScene" => {
                let s = self.state()?;
                let p = &s["program"];
                json!({"sceneName": p["name"], "sceneUuid": p["id"], "currentProgramSceneName": p["name"], "currentProgramSceneUuid": p["id"]})
            }
            "SetCurrentProgramScene" => {
                self.call("scene.switch", json!({"scene": scene_ref(d)?}))?;
                Value::Null
            }
            "GetStudioModeEnabled" => json!({"studioModeEnabled": false}),
            "GetCurrentPreviewScene" | "SetCurrentPreviewScene" | "TriggerStudioModeTransition" => {
                return Err((STUDIO_MODE_OFF, "Studio mode is not active.".into()));
            }
            "CreateScene" => {
                let r = self.call("scene.create", json!({"name": str_field(d, "sceneName")?}))?;
                json!({"sceneUuid": r["id"]})
            }
            "RemoveScene" => {
                self.call("scene.remove", json!({"scene": scene_ref(d)?}))?;
                Value::Null
            }
            "SetSceneName" => {
                self.call("scene.rename", json!({"scene": scene_ref(d)?, "name": str_field(d, "newSceneName")?}))?;
                Value::Null
            }
            "GetSceneItemList" | "GetGroupSceneItemList" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let sources = s["sources"].as_array().cloned().unwrap_or_default();
                let items: Vec<Value> = scene["objects"]
                    .as_array()
                    .map(|o| o.iter().filter(|o| o["parent"].is_null()).enumerate().map(|(i, o)| scene_item(o, i, &sources)).collect())
                    .unwrap_or_default();
                json!({"sceneItems": items})
            }
            "GetSceneItemId" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let name = str_field(d, "sourceName")?;
                let sources = s["sources"].as_array().cloned().unwrap_or_default();
                let found = scene["objects"].as_array().and_then(|o| {
                    o.iter().find(|o| {
                        o["name"] == name || sources.iter().any(|src| src["id"] == o["source"] && src["name"] == name)
                    })
                });
                let o = found.ok_or((NOT_FOUND, format!("No scene items were found in scene with the name `{name}`.")))?;
                json!({"sceneItemId": item_id(o["id"].as_str().unwrap_or(""))})
            }
            "GetSceneItemSource" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let o = self.item(scene, d)?;
                let src = named(s["sources"].as_array().map(Vec::as_slice).unwrap_or(&[]), "source", o["source"].as_str().unwrap_or(""));
                json!({"sourceName": src.map(|s| s["name"].clone()), "sourceUuid": o["source"]})
            }
            "GetSceneItemEnabled" => {
                let s = self.state()?;
                let o = self.item(self.scene(&s, d)?, d)?;
                json!({"sceneItemEnabled": o["visible"]})
            }
            "SetSceneItemEnabled" | "SetSceneItemLocked" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let o = self.item(scene, d)?;
                let (field_name, key) = if kind == "SetSceneItemEnabled" {
                    ("sceneItemEnabled", "visible")
                } else {
                    ("sceneItemLocked", "locked")
                };
                let on = field(d, field_name)?.as_bool().ok_or((INVALID_FIELD, format!("`{field_name}` should be a boolean.")))?;
                let mut p = json!({"scene": scene["id"], "object": o["id"]});
                p[key] = on.into();
                self.call("object.update", p)?;
                Value::Null
            }
            "GetSceneItemLocked" => {
                let s = self.state()?;
                let o = self.item(self.scene(&s, d)?, d)?;
                json!({"sceneItemLocked": o["locked"]})
            }
            "GetSceneItemIndex" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let o = self.item(scene, d)?;
                let i = scene["objects"].as_array().and_then(|l| l.iter().filter(|x| x["parent"].is_null()).position(|x| x["id"] == o["id"]));
                json!({"sceneItemIndex": i})
            }
            "SetSceneItemIndex" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let o = self.item(scene, d)?;
                let to = field(d, "sceneItemIndex")?.as_u64().ok_or((INVALID_FIELD, "`sceneItemIndex` should be a number.".into()))?;
                self.call("object.move", json!({"scene": scene["id"], "object": o["id"], "to": to}))?;
                Value::Null
            }
            "GetSceneItemTransform" => {
                let s = self.state()?;
                let o = self.item(self.scene(&s, d)?, d)?;
                json!({"sceneItemTransform": transform(o)})
            }
            "SetSceneItemTransform" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let o = self.item(scene, d)?;
                let t = field(d, "sceneItemTransform")?;
                let mut p = json!({"scene": scene["id"], "object": o["id"]});
                let num = |k: &str| t.get(k).and_then(Value::as_f64);
                if let Some(x) = num("positionX") {
                    p["x"] = x.into();
                }
                if let Some(y) = num("positionY") {
                    p["y"] = y.into();
                }
                if let Some(r) = num("rotation") {
                    p["rotation"] = r.into();
                }
                let (w, h) = (o["width"].as_f64().unwrap_or(0.0), o["height"].as_f64().unwrap_or(0.0));
                if let Some(bw) = num("boundsWidth").filter(|v| *v > 1.0) {
                    p["width"] = bw.into();
                } else if let Some(sx) = num("scaleX") {
                    p["width"] = (w * sx).into();
                }
                if let Some(bh) = num("boundsHeight").filter(|v| *v > 1.0) {
                    p["height"] = bh.into();
                } else if let Some(sy) = num("scaleY") {
                    p["height"] = (h * sy).into();
                }
                self.call("object.update", p)?;
                Value::Null
            }
            "RemoveSceneItem" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let o = self.item(scene, d)?;
                self.call("object.remove", json!({"scene": scene["id"], "object": o["id"]}))?;
                Value::Null
            }
            "CreateSceneItem" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let r = self.call("object.add", json!({"scene": scene["id"], "source": input_ref(d)?}))?;
                if d.get("sceneItemEnabled").and_then(Value::as_bool) == Some(false) {
                    self.call("object.update", json!({"scene": scene["id"], "object": r["object"], "visible": false}))?;
                }
                json!({"sceneItemId": item_id(r["object"].as_str().unwrap_or(""))})
            }
            "GetInputList" => {
                let s = self.state()?;
                let want = d.get("inputKind").and_then(Value::as_str);
                let inputs: Vec<Value> = s["sources"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .map(|i| (i, obs_kind(i["kind"].as_str().unwrap_or(""))))
                            .filter(|(_, k)| want.is_none_or(|w| w == *k))
                            .map(|(i, k)| json!({"inputName": i["name"], "inputUuid": i["id"], "inputKind": k, "unversionedInputKind": k}))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({"inputs": inputs})
            }
            "GetInputKindList" => {
                if cfg!(windows) {
                    json!({"inputKinds": ["browser_source", "color_source_v3", "ffmpeg_source", "image_source", "text_ft2_source_v2", "monitor_capture", "window_capture", "game_capture", "dshow_input", "wasapi_input_capture", "wasapi_output_capture", "wasapi_process_output_capture"]})
                } else {
                    json!({"inputKinds": ["browser_source", "color_source_v3", "ffmpeg_source", "image_source", "text_ft2_source_v2", "screen_capture", "macos-avcapture", "coreaudio_input_capture"]})
                }
            }
            "GetSpecialInputs" => json!({"desktop1": Value::Null, "desktop2": Value::Null, "mic1": Value::Null, "mic2": Value::Null, "mic3": Value::Null, "mic4": Value::Null}),
            "GetInputSettings" => {
                let src = self.call("source.get", json!({"source": input_ref(d)?}))?;
                json!({"inputSettings": src["settings"], "inputKind": obs_kind(src["kind"].as_str().unwrap_or(""))})
            }
            "SetInputSettings" => {
                let settings = field(d, "inputSettings")?.clone();
                let overlay = d.get("overlay").and_then(Value::as_bool).unwrap_or(true);
                self.call("source.update", json!({"source": input_ref(d)?, "settings": settings, "replace": !overlay}))?;
                Value::Null
            }
            "SetInputName" => {
                self.call("source.update", json!({"source": input_ref(d)?, "name": str_field(d, "newInputName")?}))?;
                Value::Null
            }
            "CreateInput" => {
                let s = self.state()?;
                let scene = self.scene(&s, d)?;
                let mut p = json!({
                    "scene": scene["id"],
                    "kind": str_field(d, "inputKind")?,
                    "name": str_field(d, "inputName")?,
                });
                if let Some(settings) = d.get("inputSettings").filter(|v| v.is_object()) {
                    p["settings"] = settings.clone();
                }
                let r = self.call("object.add", p)?;
                if d.get("sceneItemEnabled").and_then(Value::as_bool) == Some(false) {
                    self.call("object.update", json!({"scene": scene["id"], "object": r["object"], "visible": false}))?;
                }
                json!({"inputUuid": r["source"], "sceneItemId": item_id(r["object"].as_str().unwrap_or(""))})
            }
            "RemoveInput" => {
                self.call("source.remove", json!({"source": input_ref(d)?}))?;
                Value::Null
            }
            "GetMediaInputStatus" => {
                let r = self.call("media.status", json!({"source": input_ref(d)?}))?;
                let state = match r["state"].as_str().unwrap_or("none") {
                    "playing" => "OBS_MEDIA_STATE_PLAYING",
                    "opening" => "OBS_MEDIA_STATE_OPENING",
                    "buffering" => "OBS_MEDIA_STATE_BUFFERING",
                    "paused" => "OBS_MEDIA_STATE_PAUSED",
                    "stopped" => "OBS_MEDIA_STATE_STOPPED",
                    "ended" => "OBS_MEDIA_STATE_ENDED",
                    "error" => "OBS_MEDIA_STATE_ERROR",
                    _ => "OBS_MEDIA_STATE_NONE",
                };
                let n = |k: &str| r[k].as_i64().filter(|v| *v >= 0);
                json!({"mediaState": state, "mediaDuration": n("duration_ms"), "mediaCursor": n("time_ms")})
            }
            "TriggerMediaInputAction" => {
                let action = match str_field(d, "mediaAction")? {
                    "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_PLAY" => "play",
                    "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_PAUSE" => "pause",
                    "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_RESTART" => "restart",
                    "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_STOP" => "stop",
                    a => return Err((INVALID_FIELD, format!("Lightair doesn't do `{a}`."))),
                };
                self.call("media.action", json!({"source": input_ref(d)?, "action": action}))?;
                Value::Null
            }
            "SetMediaInputCursor" => {
                let ms = field(d, "mediaCursor")?.as_i64().ok_or((INVALID_FIELD, "`mediaCursor` should be a number.".into()))?;
                self.call("media.action", json!({"source": input_ref(d)?, "action": "seek", "ms": ms}))?;
                Value::Null
            }
            "GetInputMute" => {
                let s = self.state()?;
                let (_, t) = self.track(&s, d)?;
                json!({"inputMuted": t["muted"]})
            }
            "SetInputMute" | "ToggleInputMute" => {
                let s = self.state()?;
                let (_, t) = self.track(&s, d)?;
                let muted = if kind == "ToggleInputMute" {
                    !t["muted"].as_bool().unwrap_or(false)
                } else {
                    field(d, "inputMuted")?.as_bool().ok_or((INVALID_FIELD, "`inputMuted` should be a boolean.".into()))?
                };
                self.call("audio.set", json!({"track": t["id"], "muted": muted}))?;
                if kind == "ToggleInputMute" { json!({"inputMuted": muted}) } else { Value::Null }
            }
            "GetInputVolume" => {
                let s = self.state()?;
                let (_, t) = self.track(&s, d)?;
                let v = t["volume_db"].as_f64().unwrap_or(0.0);
                json!({"inputVolumeDb": v, "inputVolumeMul": mul(v)})
            }
            "SetInputVolume" => {
                let s = self.state()?;
                let (_, t) = self.track(&s, d)?;
                let v = match (d.get("inputVolumeDb").and_then(Value::as_f64), d.get("inputVolumeMul").and_then(Value::as_f64)) {
                    (Some(v), _) => v,
                    (None, Some(m)) => db(m),
                    _ => return Err((MISSING_FIELD, "Your request is missing the `inputVolumeDb` field.".into())),
                };
                self.call("audio.set", json!({"track": t["id"], "volume_db": v.clamp(-100.0, 26.0)}))?;
                Value::Null
            }
            "GetRecordStatus" => {
                let r = self.output_status("recording")?;
                let secs = r["seconds"].as_f64().unwrap_or(0.0);
                json!({
                    "outputActive": r["active"],
                    "outputPaused": r["paused"],
                    "outputTimecode": timecode(secs),
                    "outputDuration": (secs * 1000.0) as u64,
                    "outputBytes": r["bytes"],
                })
            }
            "StartRecord" => {
                self.output("recording", "record", Some(true))?;
                Value::Null
            }
            "StopRecord" => {
                let r = self.output("recording", "record", Some(false))?;
                json!({"outputPath": r["outputPath"]})
            }
            "ToggleRecord" => json!({"outputActive": self.output("recording", "record", None)?["outputActive"]}),
            "PauseRecord" => {
                self.call("record.pause", json!({}))?;
                Value::Null
            }
            "ResumeRecord" => {
                self.call("record.resume", json!({}))?;
                Value::Null
            }
            "ToggleRecordPause" => {
                let paused = self.output_status("recording")?["paused"].as_bool().unwrap_or(false);
                self.call(if paused { "record.resume" } else { "record.pause" }, json!({}))?;
                Value::Null
            }
            "SplitRecordFile" => {
                self.call("record.split", json!({}))?;
                Value::Null
            }
            "CreateRecordChapter" => {
                self.call("record.marker", json!({}))?;
                Value::Null
            }
            "GetStreamStatus" => {
                let r = self.output_status("streaming")?;
                let secs = r["seconds"].as_f64().unwrap_or(0.0);
                json!({
                    "outputActive": r["active"],
                    "outputReconnecting": false,
                    "outputTimecode": timecode(secs),
                    "outputDuration": (secs * 1000.0) as u64,
                    "outputCongestion": 0,
                    "outputBytes": 0,
                    "outputSkippedFrames": 0,
                    "outputTotalFrames": 0,
                })
            }
            "StartStream" => {
                self.output("streaming", "stream", Some(true))?;
                Value::Null
            }
            "StopStream" => {
                self.output("streaming", "stream", Some(false))?;
                Value::Null
            }
            "ToggleStream" => json!({"outputActive": self.output("streaming", "stream", None)?["outputActive"]}),
            "GetVirtualCamStatus" => json!({"outputActive": self.output_status("virtual_camera")?["active"]}),
            "StartVirtualCam" => {
                self.output("virtual_camera", "vcam", Some(true))?;
                Value::Null
            }
            "StopVirtualCam" => {
                self.output("virtual_camera", "vcam", Some(false))?;
                Value::Null
            }
            "ToggleVirtualCam" => json!({"outputActive": self.output("virtual_camera", "vcam", None)?["outputActive"]}),
            "GetReplayBufferStatus" => json!({"outputActive": self.output_status("clips")?["active"]}),
            "StartReplayBuffer" | "StopReplayBuffer" | "ToggleReplayBuffer" => Value::Null,
            "SaveReplayBuffer" => {
                self.call("clip.save", json!({}))?;
                Value::Null
            }
            "GetLastReplayBufferReplay" => {
                let r = self.call("clip.status", json!({}))?;
                json!({"savedReplayPath": r["last"]})
            }
            "GetSourceScreenshot" | "SaveSourceScreenshot" => {
                let scene = scene_ref(d)?;
                let width = d.get("imageWidth").and_then(Value::as_u64);
                let r = self.call("screenshot", json!({"scene": scene, "width": width}))?;
                let data = r["data"].as_str().unwrap_or_default();
                if kind == "SaveSourceScreenshot" {
                    let path = str_field(d, "imageFilePath")?;
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| (ACTION_FAILED, e.to_string()))?;
                    std::fs::write(path, bytes).map_err(|e| (ACTION_FAILED, e.to_string()))?;
                    Value::Null
                } else {
                    json!({"imageData": format!("data:image/png;base64,{data}")})
                }
            }
            "GetSceneCollectionList" => {
                let v = self.call("app.version", json!({}))?;
                json!({"currentSceneCollectionName": v["workspace"], "sceneCollections": [v["workspace"]]})
            }
            "GetProfileList" => json!({"currentProfileName": "Lightair", "profiles": ["Lightair"]}),
            "GetCurrentSceneTransition" => {
                json!({"transitionName": "Fade", "transitionUuid": Value::Null, "transitionKind": "fade_transition", "transitionFixed": false, "transitionDuration": 300, "transitionConfigurable": false, "transitionSettings": Value::Null})
            }
            "GetSceneTransitionList" => {
                json!({"currentSceneTransitionName": "Fade", "currentSceneTransitionUuid": Value::Null, "currentSceneTransitionKind": "fade_transition", "transitions": [{"transitionName": "Fade", "transitionUuid": Value::Null, "transitionKind": "fade_transition", "transitionFixed": false, "transitionConfigurable": false}]})
            }
            "GetHotkeyList" => json!({"hotkeys": []}),
            "GetSourceActive" => json!({"videoActive": true, "videoShowing": true}),
            "GetVideoSettings" => {
                let s = self.state()?;
                let c = &s["canvases"][0];
                let fps = c["fps"].as_f64().unwrap_or(30.0);
                json!({"fpsNumerator": fps.round() as u64, "fpsDenominator": 1, "baseWidth": c["width"], "baseHeight": c["height"], "outputWidth": c["width"], "outputHeight": c["height"]})
            }
            "GetRecordDirectory" => {
                let r = self.output_status("recording")?;
                json!({"recordDirectory": r["folder"]})
            }
            "GetCurrentSceneTransitionCursor" => json!({"transitionCursor": 0.0}),
            _ => return Err((UNKNOWN_REQUEST, format!("Lightair doesn't do `{kind}` yet."))),
        })
    }
}

fn timecode(secs: f64) -> String {
    let ms = (secs * 1000.0) as u64;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn transform(o: &Value) -> Value {
    let f = |k: &str| o[k].as_f64().unwrap_or(0.0);
    json!({
        "positionX": f("x"), "positionY": f("y"), "rotation": f("rotation"),
        "scaleX": 1.0, "scaleY": 1.0,
        "width": f("width"), "height": f("height"),
        "sourceWidth": f("width"), "sourceHeight": f("height"),
        "alignment": 5, "boundsType": "OBS_BOUNDS_NONE", "boundsAlignment": 0,
        "boundsWidth": 0.0, "boundsHeight": 0.0,
        "cropLeft": 0, "cropRight": 0, "cropTop": 0, "cropBottom": 0, "cropToBounds": false,
    })
}

fn scene_item(o: &Value, index: usize, sources: &[Value]) -> Value {
    let src = sources.iter().find(|s| s["id"] == o["source"]);
    let kind = src.and_then(|s| s["kind"].as_str()).map(obs_kind);
    json!({
        "sceneItemId": item_id(o["id"].as_str().unwrap_or("")),
        "sceneItemIndex": index,
        "sceneItemEnabled": o["visible"],
        "sceneItemLocked": o["locked"],
        "sceneItemTransform": transform(o),
        "sceneItemBlendMode": "OBS_BLEND_NORMAL",
        "sourceName": src.map_or(o["name"].clone(), |s| s["name"].clone()),
        "sourceUuid": o["source"],
        "sourceType": if src.is_some() { "OBS_SOURCE_TYPE_INPUT" } else { "OBS_SOURCE_TYPE_SCENE" },
        "inputKind": kind,
        "isGroup": src.is_none(),
    })
}

/// What `GetVersion` says this answers.
const REQUESTS: &[&str] = &[
    "GetVersion", "GetStats", "Sleep", "GetSceneList", "GetGroupList", "GetCurrentProgramScene",
    "SetCurrentProgramScene", "GetStudioModeEnabled", "CreateScene", "RemoveScene", "SetSceneName",
    "GetSceneItemList", "GetGroupSceneItemList", "GetSceneItemId", "GetSceneItemSource", "GetSceneItemEnabled",
    "SetSceneItemEnabled", "GetSceneItemLocked", "SetSceneItemLocked", "GetSceneItemIndex", "SetSceneItemIndex",
    "GetSceneItemTransform", "SetSceneItemTransform", "RemoveSceneItem", "CreateSceneItem", "GetInputList",
    "GetInputKindList", "GetSpecialInputs", "GetInputSettings", "SetInputSettings", "SetInputName", "CreateInput",
    "RemoveInput", "GetInputMute", "SetInputMute", "ToggleInputMute", "GetInputVolume", "SetInputVolume",
    "GetRecordStatus", "StartRecord", "StopRecord", "ToggleRecord", "PauseRecord", "ResumeRecord",
    "ToggleRecordPause", "SplitRecordFile", "CreateRecordChapter", "GetStreamStatus", "StartStream", "StopStream",
    "ToggleStream", "GetVirtualCamStatus", "StartVirtualCam", "StopVirtualCam", "ToggleVirtualCam",
    "GetReplayBufferStatus", "SaveReplayBuffer", "GetLastReplayBufferReplay", "GetSourceScreenshot",
    "SaveSourceScreenshot", "GetSceneCollectionList", "GetProfileList", "GetCurrentSceneTransition",
    "GetSceneTransitionList", "GetHotkeyList", "GetSourceActive", "GetVideoSettings", "GetRecordDirectory", "GetMediaInputStatus",
    "TriggerMediaInputAction", "SetMediaInputCursor",
];
