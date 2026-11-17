//! Donations from tip services, with the streamer's own token or link from each service's
//! dashboard (no app approval, no server): Streamlabs' socket (socket.io: donations),
//! StreamElements' realtime socket (Astro: tips), and Ko-fi's own stream alerts page (its link
//! opened in a hidden page, which Ko-fi sends each tip to live). Tokens are kept by the app (the
//! Keychain) and handed over here; the channel a reader is given only names the service.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};
use tungstenite::Message;

use crate::event::{ChatEvent, Kind, Paid, Status, User, text_fragments};
use crate::hub::Sink;
use crate::net;

static TOKENS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// Sets (or, empty, removes) a service's token ("streamlabs", "streamelements").
pub fn set_token(service: &str, token: &str) {
    let mut t = TOKENS.get_or_init(Default::default).lock();
    if token.trim().is_empty() {
        t.remove(service);
    } else {
        t.insert(service.into(), token.trim().into());
    }
}

/// Whether a service has its token (or, Ko-fi, its stream alerts link).
pub fn has_token(service: &str) -> bool {
    TOKENS.get_or_init(Default::default).lock().contains_key(service)
}

/// Ko-fi's stream alerts link ("https://ko-fi.com/streamalerts/overlay/sa_…"), when that's what
/// `text` is.
pub fn kofi_link(text: &str) -> Option<String> {
    let t = text.trim();
    let rest = t.strip_prefix("https://ko-fi.com/streamalerts/overlay/").or_else(|| t.strip_prefix("https://www.ko-fi.com/streamalerts/overlay/"))?;
    let key: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    (key.len() > 8).then(|| format!("https://ko-fi.com/streamalerts/overlay/{key}"))
}

fn token(service: &str) -> Result<String, String> {
    TOKENS.get_or_init(Default::default).lock().get(service).cloned().ok_or_else(|| format!("no {service} token (Settings › Connections)"))
}

fn money(amount: f64, currency: &str) -> Paid {
    let symbol = match currency {
        "USD" | "CAD" | "AUD" | "NZD" | "SGD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        "KRW" => "₩",
        "IDR" => "Rp",
        _ => "",
    };
    let display = if symbol.is_empty() { format!("{amount:.2} {currency}") } else { format!("{symbol}{amount:.2}") };
    Paid { display, amount, currency: currency.into() }
}

fn tip(service: &str, name: &str, amount: f64, currency: &str, message: &str) -> ChatEvent {
    let mut e = ChatEvent::new(service, "tips", Kind::Paid).with_fragments(text_fragments(message));
    e.user = Some(User { id: name.to_lowercase(), login: name.to_lowercase(), display_name: name.into(), ..Default::default() });
    e.paid = Some(money(amount, currency));
    e
}

fn num(v: &Value) -> f64 {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0)
}

/// Streamlabs: socket.io (Engine.IO 3) over a WebSocket; donations come as `42["event", …]`.
pub fn streamlabs(sink: &Sink, stop: &AtomicBool) -> Result<(), String> {
    let t = token("streamlabs")?;
    let mut socket = net::connect(&format!("wss://sockets.streamlabs.com/socket.io/?token={t}&EIO=3&transport=websocket"), Duration::from_millis(500))?;
    let mut ping = Duration::from_secs(25);
    let mut last = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if last.elapsed() > ping {
            socket.send(Message::text("2")).map_err(|e| e.to_string())?;
            last = Instant::now();
        }
        let Some(Message::Text(t)) = net::read(&mut socket)? else { continue };
        let t = t.as_str();
        if let Some(open) = t.strip_prefix('0') {
            if let Ok(v) = serde_json::from_str::<Value>(open) {
                ping = Duration::from_millis(v["pingInterval"].as_u64().unwrap_or(25_000));
            }
        } else if t == "40" {
            sink.status(Status::Connected);
        } else if t.starts_with("44") {
            return Err("Streamlabs refused the token: paste it again (Settings › Connections)".into());
        } else if let Some(payload) = t.strip_prefix("42")
            && let Ok(v) = serde_json::from_str::<Value>(payload)
        {
            for e in parse_streamlabs(&v[1]) {
                sink.emit(e);
            }
        }
    }
    let _ = socket.close(None);
    Ok(())
}

pub fn parse_streamlabs(v: &Value) -> Vec<ChatEvent> {
    if v["type"].as_str() != Some("donation") {
        return Vec::new();
    }
    let list = match &v["message"] {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    list.iter()
        .map(|m| tip("streamlabs", m["name"].as_str().or(m["from"].as_str()).unwrap_or("Someone"), num(&m["amount"]), m["currency"].as_str().unwrap_or("USD"), m["message"].as_str().unwrap_or("")))
        .collect()
}

/// The `channel` claim of StreamElements' JWT (its channel id).
fn jwt_channel(jwt: &str) -> Option<String> {
    use base64::Engine;
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()?["channel"].as_str().map(String::from)
}

/// StreamElements: the Astro socket, subscribed to the channel's tips.
pub fn streamelements(sink: &Sink, stop: &AtomicBool) -> Result<(), String> {
    let jwt = token("streamelements")?;
    let room = jwt_channel(&jwt).ok_or("that isn't a StreamElements JWT token")?;
    let mut socket = net::connect("wss://astro.streamelements.com", Duration::from_millis(500))?;
    let subscribe = json!({"type": "subscribe", "nonce": format!("lightair-{}", crate::event::now_ms()), "data": {"topic": "channel.tips", "room": room, "token": jwt, "token_type": "jwt"}});
    socket.send(Message::text(subscribe.to_string())).map_err(|e| e.to_string())?;
    while !stop.load(Ordering::Acquire) {
        let Some(Message::Text(t)) = net::read(&mut socket)? else { continue };
        let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
        match v["type"].as_str() {
            Some("response") if v["error"].is_string() => return Err(format!("StreamElements: {}", v["data"]["message"].as_str().or(v["error"].as_str()).unwrap_or("refused"))),
            Some("response") => sink.status(Status::Connected),
            Some("message") => {
                if let Some(e) = parse_streamelements(&v["data"]) {
                    sink.emit(e);
                }
            }
            _ => {}
        }
    }
    let _ = socket.close(None);
    Ok(())
}

pub fn parse_streamelements(d: &Value) -> Option<ChatEvent> {
    let don = if d["donation"].is_object() { &d["donation"] } else { d };
    let name = don["user"]["username"].as_str().or(don["username"].as_str()).or(d["displayName"].as_str())?;
    Some(tip("streamelements", name, num(&don["amount"]), don["currency"].as_str().unwrap_or("USD"), don["message"].as_str().unwrap_or("")))
}

/// Runs as Ko-fi's alerts page starts: the messages its live connection (SignalR) receives,
/// handed over as they come.
const KOFI_EARLY: &str = r#"(function () {
  if (window.__lightairWS) return;
  window.__lightairWS = true;
  var Native = window.WebSocket;
  function Hooked(url, protocols) {
    var ws = protocols === undefined ? new Native(url) : new Native(url, protocols);
    ws.addEventListener('message', function (ev) { if (typeof ev.data === 'string') lightair('ws:' + ev.data); });
    return ws;
  }
  Hooked.prototype = Native.prototype;
  ['CONNECTING', 'OPEN', 'CLOSING', 'CLOSED'].forEach(function (k) { Hooked[k] = Native[k]; });
  window.WebSocket = Hooked;
})();"#;

/// After the page loads: that it did, and whether Ko-fi took the link.
const KOFI_SCRIPT: &str = r#"(function () {
  setTimeout(function () {
    var text = (document.body && document.body.innerText || '').toLowerCase();
    lightair(JSON.stringify({ status: /not found|invalid|error/.test(text) && text.length < 400 ? 'refused' : 'ready' }));
  }, 3000);
})();"#;

/// Ko-fi, from its own stream alerts page (the link Ko-fi gives for OBS: Ko-fi › Stream Alerts ›
/// Copy the link), opened hidden: Ko-fi sends that page each tip, membership, shop order and
/// commission live, and Lightair reads them off the page's connection. Nothing to deploy;
/// what arrives while Lightair is closed isn't replayed.
pub fn kofi(sink: &Sink, stop: &AtomicBool) -> Result<(), String> {
    let link = token("kofi")?;
    let link = kofi_link(&link).ok_or("Ko-fi needs its stream alerts link (Settings › Connections › Ko-fi)")?;
    let host = crate::pages::host()?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = Mutex::new(tx);
    let early = format!("{}{KOFI_EARLY}", crate::pages::PRELUDE);
    let script = format!("{}{KOFI_SCRIPT}", crate::pages::PRELUDE);
    let page = host.open_early(&link, &early, &script, std::sync::Arc::new(move |m| {
        let _ = tx.lock().send(m);
    }));
    let mut seen = std::collections::HashSet::new();
    let mut heard = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let text = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(t) => t,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // a quiet connection for long: open the page again
                if heard.elapsed() > Duration::from_secs(30 * 60) {
                    break;
                }
                continue;
            }
            Err(_) => break,
        };
        heard = Instant::now();
        if let Some(frames) = text.strip_prefix("ws:") {
            for e in parse_kofi_frames(frames) {
                // memberships and the like can come twice (the activity and its alert)
                if let Some(id) = e.native_id.clone()
                    && !seen.insert(id)
                {
                    continue;
                }
                sink.emit(e);
            }
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            match v["status"].as_str() {
                Some("ready") => sink.status(Status::Connected),
                Some("refused") => {
                    drop(page);
                    return Err("Ko-fi didn't take that stream alerts link: copy it again from Ko-fi › Stream Alerts".into());
                }
                _ => {}
            }
        }
    }
    drop(page);
    Ok(())
}

/// SignalR messages (JSON, each ended by U+001E) → Ko-fi events: `updateAlertActivity`
/// carries the structured activity.
pub fn parse_kofi_frames(frames: &str) -> Vec<ChatEvent> {
    frames
        .split('\u{1e}')
        .filter_map(|f| serde_json::from_str::<Value>(f.trim()).ok())
        .filter(|m| m["type"].as_i64() == Some(1) && m["target"].as_str() == Some("updateAlertActivity"))
        .filter_map(|m| {
            let a = &m["arguments"][0];
            // the activity comes as JSON text or as an object
            let a = match a {
                Value::String(s) => serde_json::from_str::<Value>(s).ok()?,
                other => other.clone(),
            };
            parse_kofi(&a)
        })
        .collect()
}

/// A Ko-fi activity: a tip, a membership (each payment), a shop order, a commission.
pub fn parse_kofi(a: &Value) -> Option<ChatEvent> {
    let name = a["UserName"].as_str().filter(|n| !n.is_empty()).unwrap_or("Someone");
    let amount = num(&a["Amount"]);
    let public = a["IsMessagePublic"].as_bool() != Some(false);
    let message = if public { a["Message"].as_str().unwrap_or("") } else { "" };
    let mut e = tip("kofi", name, amount, a["Currency"].as_str().unwrap_or("USD"), message);
    match a["TransactionType"].as_str().unwrap_or("Donation") {
        "Membership" | "Subscription" => {
            e.kind = Kind::Membership;
            let tier = a["MembershipTierName"].as_str().unwrap_or("");
            e.headline = Some(if tier.is_empty() { format!("{name} is a member on Ko-fi") } else { format!("{name} is a {tier} member on Ko-fi") });
        }
        "Shop item" | "Shop Order" => {
            let item = a["ShopItemName"].as_str().unwrap_or("");
            e.headline = Some(if item.is_empty() { format!("{name} bought from the shop") } else { format!("{name} bought {item}") });
        }
        "Commission" => e.headline = Some(format!("{name} paid for a commission")),
        _ => {}
    }
    e.test = a["IsTestActivity"].as_bool().unwrap_or(false);
    e.native_id = a["TransactionId"].as_str().filter(|t| !t.is_empty()).map(String::from);
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamlabs_and_streamelements_tips() {
        let v: Value = serde_json::from_str(r#"{"type":"donation","message":[{"name":"Fan","amount":"5.0000000000","currency":"USD","message":"hi"}]}"#).unwrap();
        let e = &parse_streamlabs(&v)[0];
        assert_eq!((e.name(), e.paid.as_ref().unwrap().amount, e.paid.as_ref().unwrap().display.as_str()), ("Fan", 5.0, "$5.00"));
        let d: Value = serde_json::from_str(r#"{"donation":{"user":{"username":"Pal"},"amount":10,"currency":"EUR","message":"gg"}}"#).unwrap();
        assert_eq!(parse_streamelements(&d).unwrap().paid.unwrap().display, "€10.00");
        // {"alg":"none"}.{"channel":"abc"}.
        assert_eq!(jwt_channel("eyJhbGciOiJub25lIn0.eyJjaGFubmVsIjoiYWJjIn0.x").as_deref(), Some("abc"));
        // Ko-fi's alerts page: SignalR messages, each ended by U+001E, the activity as JSON text
        let activity = json!({"TransactionType": "Donation", "UserName": "Kim", "Amount": "3.00", "Currency": "GBP", "Message": "ty",
            "IsMessagePublic": true, "TransactionId": "t1", "IsTestActivity": false}).to_string();
        let frames = format!("{}\u{1e}{}\u{1e}{}\u{1e}",
            json!({"type": 6}),
            json!({"type": 1, "target": "updateAlertActivity", "arguments": [activity]}),
            json!({"type": 1, "target": "newStreamAlert", "arguments": ["<div class='sa-label'>Got £3.00 from Kim!</div>", ""]}));
        let got = parse_kofi_frames(&frames);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].paid.clone().unwrap().display, got[0].text.as_str(), got[0].native_id.as_deref()), ("£3.00".to_string(), "ty", Some("t1")));
        // a membership with a private message, a shop item, a test
        let m = parse_kofi(&json!({"TransactionType": "Membership", "UserName": "Ana", "Amount": "5", "Currency": "USD", "Message": "secret",
            "IsMessagePublic": false, "MembershipTierName": "Gold", "IsTestActivity": true})).unwrap();
        assert_eq!((m.kind.clone(), m.text.as_str(), m.headline.as_deref(), m.test), (Kind::Membership, "", Some("Ana is a Gold member on Ko-fi"), true));
        let shop = parse_kofi(&json!({"TransactionType": "Shop item", "UserName": "Bo", "Amount": "12", "Currency": "EUR", "ShopItemName": "Sticker pack"})).unwrap();
        assert_eq!(shop.headline.as_deref(), Some("Bo bought Sticker pack"));
        // the link, however it was copied
        assert_eq!(kofi_link(" https://ko-fi.com/streamalerts/overlay/sa_1234-abcd-5678 "), Some("https://ko-fi.com/streamalerts/overlay/sa_1234-abcd-5678".into()));
        assert_eq!(kofi_link("https://ko-fi.com/yourname"), None);
    }
}
