//! Devices and apps on this computer or network, for automation steps (docs/plans/
//! live-integrations.md, 2.8): VTube Studio (its local API: the first time, VTube Studio asks
//! whether Lightair may), Elgato Key Lights (their local HTTP API), Philips Hue (the bridge's
//! local API: pairing is pressing its button). No accounts, no approval.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};
use tungstenite::Message;

use crate::net;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where integrations keep what they're given (VTube Studio's token).
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

fn token_file() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("vtube-studio.json"))
}

fn hue_file() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("hue.json"))
}

/// The paired Hue bridge: (address, the name the bridge gave Lightair).
pub fn hue_bridge() -> Option<(String, String)> {
    let v: Value = serde_json::from_slice(&std::fs::read(hue_file()?).ok()?).ok()?;
    Some((v["address"].as_str()?.to_string(), v["username"].as_str()?.to_string()))
}

/// Hue bridges on this network, through Philips' discovery service: their addresses.
pub fn hue_find() -> Result<Vec<String>, String> {
    let list = net::get_json(&net::agent(), "https://discovery.meethue.com/", &[])?;
    Ok(list.as_array().into_iter().flatten().filter_map(|b| b["internalipaddress"].as_str().map(String::from)).collect())
}

/// Asks the bridge at `address` to let Lightair in: `Ok(false)` until its button was pressed
/// (pressing it allows this for 30 seconds), `Ok(true)` once paired (kept for next time).
pub fn hue_pair(address: &str) -> Result<bool, String> {
    let address = address.trim().trim_start_matches("http://").trim_end_matches('/');
    if address.is_empty() {
        return Err("the bridge's address is needed".into());
    }
    let answer = net::post_json(&net::agent(), &format!("http://{address}/api"), &json!({"devicetype": "lightair#mac"}))?;
    let first = &answer[0];
    if let Some(name) = first["success"]["username"].as_str() {
        let file = hue_file().ok_or("nowhere to keep the pairing")?;
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&file, json!({"address": address, "username": name}).to_string()).map_err(|e| e.to_string())?;
        return Ok(true);
    }
    match first["error"]["type"].as_i64() {
        Some(101) => Ok(false),
        _ => Err(first["error"]["description"].as_str().map_or_else(|| format!("{address} didn't answer like a Hue bridge"), String::from)),
    }
}

pub fn hue_forget() {
    if let Some(f) = hue_file() {
        let _ = std::fs::remove_file(f);
    }
}

/// sRGB "#rrggbb" → the CIE xy Hue lights take (Philips' wide-gamut conversion).
fn hue_xy(colour: &str) -> Option<[f64; 2]> {
    let hex = colour.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|v| {
        let v = v as f64 / 255.0;
        if v > 0.04045 { ((v + 0.055) / 1.055).powf(2.4) } else { v / 12.92 }
    });
    let (r, g, b) = (c(0)?, c(2)?, c(4)?);
    let x = r * 0.664511 + g * 0.154324 + b * 0.162028;
    let y = r * 0.283881 + g * 0.668433 + b * 0.047685;
    let z = r * 0.000088 + g * 0.072310 + b * 0.986039;
    let sum = x + y + z;
    if sum == 0.0 {
        return Some([0.3127, 0.3290]);
    }
    Some([((x / sum) * 10000.0).round() / 10000.0, ((y / sum) * 10000.0).round() / 10000.0])
}

fn hue_call(method: &str, p: &Value) -> Result<Value, String> {
    let (address, user) = hue_bridge().ok_or("Hue isn't paired: Settings › Connections › Philips Hue")?;
    let base = format!("http://{address}/api/{user}");
    let agent = net::agent();
    let lights = net::get_json(&agent, &format!("{base}/lights"), &[])?;
    let groups = net::get_json(&agent, &format!("{base}/groups"), &[])?;
    if let Some(e) = lights[0]["error"]["description"].as_str() {
        return Err(format!("the bridge said: {e} (pair again)"));
    }
    let named = |list: &Value, kind: &str| -> Vec<Value> {
        list.as_object().into_iter().flatten().map(|(id, v)| json!({"id": id, "name": v["name"], "kind": kind})).collect()
    };
    if method == "hue.lights" {
        let mut all = named(&lights, "light");
        all.extend(named(&groups, "room"));
        return Ok(json!({"lights": all}));
    }
    let want = p["light"].as_str().unwrap_or("").trim();
    let find = |list: &Value| {
        list.as_object()
            .into_iter()
            .flatten()
            .find(|(id, v)| id.as_str() == want || v["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(want)))
            .map(|(id, _)| id.clone())
    };
    let path = if want.is_empty() || want.eq_ignore_ascii_case("all") {
        "groups/0/action".to_string()
    } else if let Some(id) = find(&lights) {
        format!("lights/{id}/state")
    } else if let Some(id) = find(&groups) {
        format!("groups/{id}/action")
    } else {
        return Err(format!("the bridge has no light or room “{want}”"));
    };
    let mut state = json!({});
    if let Some(on) = p["on"].as_bool() {
        state["on"] = json!(on);
    }
    if let Some(b) = p["brightness"].as_f64() {
        state["bri"] = json!((b.clamp(0.0, 100.0) * 2.54).round().max(1.0) as u64);
    }
    if let Some(xy) = p["colour"].as_str().or(p["color"].as_str()).and_then(hue_xy) {
        state["xy"] = json!(xy);
    } else if let Some(k) = p["temperature"].as_f64() {
        state["ct"] = json!((1_000_000.0 / k.clamp(2000.0, 6500.0)).round().clamp(153.0, 500.0) as u64);
    }
    if let Some(t) = p["transition"].as_f64() {
        state["transitiontime"] = json!((t.max(0.0) * 10.0).round() as u64);
    }
    let mut res = agent.put(&format!("{base}/{path}")).header("Content-Type", "application/json").send(state.to_string()).map_err(|e| e.to_string())?;
    let answer: Value = serde_json::from_str(&res.body_mut().read_to_string().unwrap_or_default()).unwrap_or(Value::Null);
    if let Some(e) = answer.as_array().into_iter().flatten().find_map(|r| r["error"]["description"].as_str()) {
        return Err(format!("the bridge said: {e}"));
    }
    Ok(answer)
}

/// One VTube Studio connection at a time (its API answers in order).
static VTS: Mutex<()> = Mutex::new(());

fn vts_request(socket: &mut net::Socket, kind: &str, data: Value) -> Result<Value, String> {
    let id = format!("lightair-{}", crate::event::now_ms());
    let req = json!({"apiName": "VTubeStudioPublicAPI", "apiVersion": "1.0", "requestID": id, "messageType": kind, "data": data});
    socket.send(Message::text(req.to_string())).map_err(|e| e.to_string())?;
    // the token request waits for the person to answer in VTube Studio
    let wait = if kind == "AuthenticationTokenRequest" { 60 } else { 8 };
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(wait) {
        let Some(Message::Text(t)) = net::read(socket)? else { continue };
        let v: Value = serde_json::from_str(t.as_str()).map_err(|e| e.to_string())?;
        if v["requestID"] != id.as_str() {
            continue;
        }
        if v["messageType"] == "APIError" {
            return Err(format!("VTube Studio: {}", v["data"]["message"].as_str().unwrap_or("refused")));
        }
        return Ok(v["data"].clone());
    }
    Err("VTube Studio didn't answer".into())
}

/// A connection to VTube Studio, signed in as Lightair (asking for a token the first time).
fn vts_connect() -> Result<net::Socket, String> {
    let port = std::env::var("LIGHTAIR_VTS_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(8001);
    let mut socket = net::connect(&format!("ws://127.0.0.1:{port}"), Duration::from_millis(300))
        .map_err(|_| "VTube Studio isn't running (or its API is off: Settings › Start API)".to_string())?;
    let plugin = json!({"pluginName": "Lightair", "pluginDeveloper": "Lightair"});
    let saved = token_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["token"].as_str().map(String::from));
    let token = match saved {
        Some(t) => t,
        None => {
            let v = vts_request(&mut socket, "AuthenticationTokenRequest", plugin.clone())?;
            let t = v["authenticationToken"].as_str().ok_or("VTube Studio gave no token (Allow Lightair in VTube Studio)")?.to_string();
            if let Some(f) = token_file() {
                let _ = std::fs::create_dir_all(f.parent().unwrap_or(&f));
                let _ = std::fs::write(f, json!({"token": t}).to_string());
            }
            t
        }
    };
    let mut auth = plugin;
    auth["authenticationToken"] = json!(token);
    let v = vts_request(&mut socket, "AuthenticationRequest", auth)?;
    if v["authenticated"].as_bool() != Some(true) {
        if let Some(f) = token_file() {
            let _ = std::fs::remove_file(f);
        }
        return Err("VTube Studio didn't accept Lightair: run it again and choose Allow".into());
    }
    Ok(socket)
}

/// A method for an automation step or remote client, if it's one of these:
/// `vts.hotkeys`, `vts.hotkey` {hotkey: name or id}, `vts.expression` {file, active},
/// `light.get` / `light.set` {address, on, brightness, temperature}, `hue.lights`,
/// `hue.set` {light: a light's or room's name (or "all"), on, brightness, colour, temperature,
/// transition}.
pub fn call(method: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        "vts.hotkeys" | "vts.hotkey" | "vts.expression" => {
            let _one = VTS.lock();
            (|| {
                let mut s = vts_connect()?;
                let list = vts_request(&mut s, "HotkeysInCurrentModelRequest", json!({}))?;
                let out = match method {
                    "vts.hotkeys" => Ok(json!({"hotkeys": list["availableHotkeys"].as_array().map(|a| a.iter().map(|h| json!({"name": h["name"], "id": h["hotkeyID"], "type": h["type"]})).collect::<Vec<_>>()).unwrap_or_default()})),
                    "vts.hotkey" => {
                        let want = p["hotkey"].as_str().unwrap_or("");
                        let id = list["availableHotkeys"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|h| h["hotkeyID"] == want || h["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(want)))
                            .and_then(|h| h["hotkeyID"].as_str().map(String::from))
                            .ok_or_else(|| format!("the model has no hotkey “{want}”"))?;
                        vts_request(&mut s, "HotkeyTriggerRequest", json!({"hotkeyID": id}))
                    }
                    _ => vts_request(&mut s, "ExpressionActivationRequest", json!({"expressionFile": p["file"], "active": p["active"].as_bool().unwrap_or(true)})),
                };
                let _ = s.close(None);
                out
            })()
        }
        "light.get" | "light.set" => (|| {
            let address = p["address"].as_str().filter(|a| !a.is_empty()).ok_or("the light's address is needed (its IP, from Elgato Control Center)")?;
            let host = if address.contains(':') { address.to_string() } else { format!("{address}:9123") };
            let url = format!("http://{host}/elgato/lights");
            let agent = net::agent();
            if method == "light.get" {
                return net::get_json(&agent, &url, &[]);
            }
            let mut light = json!({});
            if let Some(on) = p["on"].as_bool() {
                light["on"] = json!(on as u8);
            }
            if let Some(b) = p["brightness"].as_f64() {
                light["brightness"] = json!(b.clamp(0.0, 100.0) as u64);
            }
            // Kelvin (2900–7000) → Elgato's own scale (143–344)
            if let Some(k) = p["temperature"].as_f64() {
                light["temperature"] = json!((1_000_000.0 / k.clamp(2900.0, 7000.0)).round() as u64);
            }
            let body = json!({"numberOfLights": 1, "lights": [light]});
            let mut res = agent.put(&url).header("Content-Type", "application/json").send(body.to_string()).map_err(|e| e.to_string())?;
            let text = res.body_mut().read_to_string().unwrap_or_default();
            Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
        })(),
        "hue.lights" | "hue.set" => hue_call(method, p),
        m if crate::official::is_action(m) => crate::official::action(m, p),
        _ => return None,
    })
}

/// Heart rate from Pulsoid (the person's own token from pulsoid.net › Keys): `on_bpm` with each
/// reading, until `stop`. Reconnects with a pause after a failure.
pub fn pulsoid(token: String, on_bpm: impl Fn(u32) + Send + 'static, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    let _ = std::thread::Builder::new().name("lightair-heart-rate".into()).spawn(move || {
        while !stop.load(Ordering::Acquire) {
            let result = (|| -> Result<(), String> {
                let mut socket = net::connect(&format!("wss://dev.pulsoid.net/api/v1/data/real_time?access_token={token}"), Duration::from_millis(500))?;
                while !stop.load(Ordering::Acquire) {
                    let Some(Message::Text(t)) = net::read(&mut socket)? else { continue };
                    if let Ok(v) = serde_json::from_str::<Value>(t.as_str())
                        && let Some(bpm) = v["data"]["heart_rate"].as_u64()
                    {
                        on_bpm(bpm as u32);
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                tracing::info!(%e, "heart rate");
                crate::hub::sleep_unless(&stop, Duration::from_secs(15));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_methods_are_left_alone() {
        assert!(super::call("scene.switch", &serde_json::json!({})).is_none());
        assert!(super::call("light.set", &serde_json::json!({})).unwrap().is_err());
    }

    #[test]
    fn colours_become_hue_xy() {
        assert_eq!(super::hue_xy("#ff0000"), Some([0.7006, 0.2993]));
        assert_eq!(super::hue_xy("#000000"), Some([0.3127, 0.329]));
        assert!(super::hue_xy("red").is_none());
    }

    /// A stand-in bridge: refuses the first pairing (button not pressed), then pairs; lists a
    /// light and a room; takes a room's colour.
    #[test]
    fn pairs_and_sets_a_room_on_a_stand_in_bridge() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut pairs = 0;
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let mut req = String::new();
                let mut buf = vec![0u8; 8192];
                // the body may come in a second read
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    let Some((head, body)) = req.split_once("\r\n\r\n") else {
                        if n == 0 { break } else { continue }
                    };
                    let want = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                    if n == 0 || body.len() >= want {
                        break;
                    }
                }
                let line = req.lines().next().unwrap_or("").to_string();
                let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                log.lock().push(format!("{line} {body}"));
                let reply = if line.starts_with("POST /api ") {
                    pairs += 1;
                    if pairs == 1 { r#"[{"error":{"type":101,"description":"link button not pressed"}}]"# } else { r#"[{"success":{"username":"abc"}}]"# }
                } else if line.contains("/lights ") {
                    r#"{"1":{"name":"Desk"}}"#
                } else if line.contains("/groups ") {
                    r#"{"3":{"name":"Office"}}"#
                } else {
                    r#"[{"success":{}}]"#
                };
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
            }
        });
        super::set_dir(std::env::temp_dir().join(format!("lightair-hue-{}", std::process::id())));
        assert_eq!(super::hue_pair(&address), Ok(false));
        assert_eq!(super::hue_pair(&address), Ok(true));
        assert_eq!(super::hue_bridge().unwrap().1, "abc");
        let list = super::call("hue.lights", &serde_json::json!({})).unwrap().unwrap();
        assert_eq!(list["lights"].as_array().unwrap().len(), 2);
        super::call("hue.set", &serde_json::json!({"light": "office", "on": true, "brightness": 50, "colour": "#ff0000"})).unwrap().unwrap();
        assert!(super::call("hue.set", &serde_json::json!({"light": "Kitchen"})).unwrap().is_err());
        let put = seen.lock().iter().find(|l| l.starts_with("PUT")).cloned().unwrap();
        assert!(put.starts_with("PUT /api/abc/groups/3/action"), "{put}");
        assert!(put.contains(r#""bri":127"#) && put.contains(r#""on":true"#) && put.contains("0.7006"), "{put}");
        super::hue_forget();
        assert!(super::hue_bridge().is_none());
    }
}

#[cfg(test)]
mod vts_check {
    /// With LIGHTAIR_VTS_PORT set to a VTube Studio (or a stand-in): the whole sign-in and a hotkey.
    #[test]
    fn hotkey_against_a_running_vtube_studio() {
        if std::env::var("LIGHTAIR_VTS_PORT").is_err() {
            return;
        }
        super::set_dir(std::env::temp_dir().join(format!("lightair-vts-{}", std::process::id())));
        let list = super::call("vts.hotkeys", &serde_json::json!({})).unwrap().unwrap();
        assert_eq!(list["hotkeys"][0]["name"], "Wave");
        super::call("vts.hotkey", &serde_json::json!({"hotkey": "wave"})).unwrap().unwrap();
    }
}
