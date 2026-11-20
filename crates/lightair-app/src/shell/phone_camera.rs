//! The Phone camera layer (docs/07-scenes-layers-canvas.md, LY-16; lightair-live's docs/05 §6): a
//! phone running Lightair Live's Camera mode as a layer's picture and a mixer track.
//!
//! - **Pairing:** the layer's Inspector shows a camera pairing code (scope `camera`, `c=<source>`);
//!   the phone that scans it feeds that layer from then on, reconnecting by itself.
//! - **Control channel** (`wss://…:4456/camera`, lightair-remote's `CameraHost`): the desktop
//!   sends `configure` (lens, size, frame rate, codec, bitrate, orientation), `media` (where to send
//!   and the SRT passphrase), `tally`, `control` (zoom, torch, focus, exposure, white balance) and
//!   `keyframe`; the phone answers `hello` (its lenses and controls), `state` (its settings and
//!   the picture's rotation) and `health` every second.
//! - **Pictures and sound:** SRT to port 4461 (`lightair-phone-camera`), handed to the engine's
//!   `lightair_phone_camera` source over UDP on this machine, which decodes, turns the picture
//!   upright, shows a placeholder within a second of the phone going quiet, and passes the
//!   phone's microphone to the layer's track.
//! - **Tally** (MC-10): `program` (shown in the scene on air), `staged` (in the scene cued in
//!   studio mode), `live` (program while streaming or recording), sent to the phone on change and
//!   to remote clients as `source.tally`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use lightair_model::{Command, Id, Scene, Source};
use lightair_remote::pairing::Offer;
use lightair_remote::{ApiError, Call, CameraChannel, CameraHost, Device, Reply};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::StudioWindow;
use crate::ui::{self, tok};

pub(super) const KIND: &str = lightair_engine::phone_camera::KIND;

/// SRT's latency on a local network (lightair-live's docs/05 §6: about 4× the round trip).
const LATENCY_MS: u64 = 40;

/// What a new Phone camera layer asks of the phone.
pub(super) fn default_settings() -> Value {
    json!({"width": 1920, "height": 1080, "fps": 30, "codec": "h264", "bitrate_kbps": 12000, "orientation": "follow"})
}

/// The `configure` message for a layer's settings.
fn configure(settings: &Value) -> Value {
    let int = |k: &str, d: u64| settings[k].as_u64().unwrap_or(d);
    json!({
        "type": "configure",
        "lens": settings.get("lens").cloned().unwrap_or(Value::Null),
        "format": {"w": int("width", 1920), "h": int("height", 1080), "fps": int("fps", 30)},
        "codec": settings["codec"].as_str().unwrap_or("h264"),
        "bitrate_kbps": int("bitrate_kbps", 12000),
        "orientation": settings["orientation"].as_str().unwrap_or("follow"),
    })
}

/// Whether two configurations need a new decoder (another codec or picture size).
fn new_stream(a: &Value, b: &Value) -> bool {
    a["codec"] != b["codec"] || a["format"] != b["format"]
}

/// One Phone camera source, as the control channel knows it.
#[derive(Default)]
struct Cam {
    name: String,
    channel: Option<CameraChannel>,
    /// The phone feeding it now: (name, model, platform).
    phone: Option<(String, String, String)>,
    hello: Value,
    state: Value,
    health: Value,
    health_at: Option<Instant>,
    /// The SRT passphrase (made once per source while Lightair runs).
    passphrase: String,
    /// What the phone was last told: `configure` and `tally`.
    configured: Value,
    tally: Value,
    /// USB links up for it now.
    usb: u32,
}

/// The app's side of the phones' camera channel and SRT feeds: shared with lightair-remote's
/// server thread and the SRT thread.
#[derive(Default)]
pub(crate) struct Host {
    cams: Mutex<HashMap<String, Cam>>,
    srt_port: AtomicU16,
    /// Something the Inspector shows changed.
    changed: AtomicBool,
    /// For USB links: who's paired, this desktop's id, and the phones' TLS port.
    pairing: std::sync::OnceLock<Arc<lightair_remote::Pairing>>,
    desktop_id: std::sync::OnceLock<String>,
    phones_port: AtomicU16,
}

impl Host {
    /// What the app gives the host once it has it (the phones' pairing and the desktop's id).
    pub(crate) fn set_pairing(&self, pairing: Arc<lightair_remote::Pairing>, desktop_id: String) {
        let _ = self.pairing.set(pairing);
        let _ = self.desktop_id.set(desktop_id);
    }

    /// The phones' TLS port while it's open (0: closed), where USB links pipe their control channel.
    pub(crate) fn set_phones_port(&self, port: u16) {
        self.phones_port.store(port, Ordering::Release);
    }

    /// Where a layer's phone sends its pictures: over its USB link while one is up, else SRT.
    fn media(&self, cam: &Cam) -> Value {
        if cam.usb > 0 {
            return json!({"type": "media", "transport": "usb"});
        }
        json!({
            "type": "media", "transport": "srt", "port": self.srt_port.load(Ordering::Acquire),
            "passphrase": cam.passphrase, "latency_ms": LATENCY_MS,
        })
    }

    /// Whether an Android phone is paired to a layer (then phones plugged in by USB are asked to
    /// be Lightair's accessory).
    fn wants_android(&self) -> bool {
        let Some(p) = self.pairing.get() else { return false };
        let cams = self.cams.lock();
        p.devices().iter().any(|d| {
            d.platform.to_ascii_lowercase().contains("android")
                && d.source.as_ref().is_some_and(|s| cams.contains_key(s))
        })
    }

    /// The layer's name for a source id (the consent alert says which layer a phone would feed).
    pub(crate) fn layer_name(&self, source: &str) -> Option<String> {
        self.cams.lock().get(source).map(|c| c.name.clone())
    }

    fn send(&self, source: &str, message: Value) {
        if let Some(c) = self.cams.lock().get(source).and_then(|c| c.channel.clone()) {
            c.send(message);
        }
    }
}

/// A layer's SRT passphrase: made from this desktop's private TLS key and the layer, so it stays
/// the same when Lightair restarts (a phone still sending reconnects at once) and changes when the
/// identity is reset. 32 hex digits (SRT takes 10–79 characters).
fn passphrase(key_der: &[u8], source: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"lightair phone camera srt passphrase\0");
    h.update(key_der);
    h.update(source.as_bytes());
    h.finalize()[..16].iter().map(|x| format!("{x:02x}")).collect()
}

impl CameraHost for Host {
    fn connected(&self, source: &str, device: &Device, channel: CameraChannel) -> bool {
        let mut cams = self.cams.lock();
        let Some(cam) = cams.get_mut(source) else { return false };
        tracing::info!(source, phone = device.name, "phone camera connected");
        channel.send(cam.configured.clone());
        channel.send(cam.tally.clone());
        channel.send(self.media(cam));
        cam.channel = Some(channel);
        cam.phone = Some((device.name.clone(), device.model.clone(), device.platform.clone()));
        drop(cams);
        self.changed.store(true, Ordering::Release);
        true
    }

    fn message(&self, source: &str, message: &Value) {
        let mut cams = self.cams.lock();
        let Some(cam) = cams.get_mut(source) else { return };
        match message["type"].as_str() {
            Some("hello") => cam.hello = message.clone(),
            Some("state") => {
                if let Some(r) = message["rotation"].as_i64() {
                    lightair_engine::phone_camera::feed(source).set_rotation(r);
                }
                // (a state may carry only what changed)
                match (cam.state.as_object_mut(), message.as_object()) {
                    (Some(old), Some(new)) => old.extend(new.iter().map(|(k, v)| (k.clone(), v.clone()))),
                    _ => cam.state = message.clone(),
                }
            }
            Some("health") => {
                cam.health = message.clone();
                cam.health_at = Some(Instant::now());
            }
            _ => return,
        }
        drop(cams);
        self.changed.store(true, Ordering::Release);
    }

    fn disconnected(&self, source: &str) {
        if let Some(cam) = self.cams.lock().get_mut(source) {
            tracing::info!(source, "phone camera left");
            cam.channel = None;
        }
        self.changed.store(true, Ordering::Release);
    }
}

impl lightair_phone_camera::link::LinkHost for Host {
    fn desktop_id(&self) -> String {
        self.desktop_id.get().cloned().unwrap_or_default()
    }

    fn phones_port(&self) -> Option<u16> {
        Some(self.phones_port.load(Ordering::Acquire)).filter(|p| *p != 0)
    }

    fn knows(&self, source: &str, device: &str) -> bool {
        self.cams.lock().contains_key(source)
            && self.pairing.get().is_some_and(|p| {
                p.devices().iter().any(|d| {
                    d.id == device && d.scope == lightair_remote::Scope::Camera && d.source.as_deref() == Some(source)
                })
            })
    }

    fn signed_in(&self, source: &str) -> bool {
        self.cams.lock().get(source).is_some_and(|c| c.channel.is_some())
    }

    fn link(&self, source: &str, up: bool) {
        let mut cams = self.cams.lock();
        let Some(cam) = cams.get_mut(source) else { return };
        cam.usb = if up { cam.usb + 1 } else { cam.usb.saturating_sub(1) };
        // a phone signed in already (it moved from Wi-Fi) hears where to send now
        if let Some(c) = &cam.channel {
            c.send(self.media(cam));
        }
        drop(cams);
        self.changed.store(true, Ordering::Release);
    }
}

impl lightair_phone_camera::Feeds for Host {
    fn passphrase(&self, source: &str) -> Option<String> {
        self.cams.lock().get(source).map(|c| c.passphrase.clone()).filter(|p| !p.is_empty())
    }

    fn started(&self, source: &str, _peer: SocketAddr) -> Option<SocketAddr> {
        let feed = lightair_engine::phone_camera::feed(source);
        feed.new_stream();
        // the decoder is new: a keyframe now, and again once it listens
        self.send(source, json!({"type": "keyframe"}));
        let channel = self.cams.lock().get(source).and_then(|c| c.channel.clone());
        if let Some(c) = channel {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(600));
                c.send(json!({"type": "keyframe"}));
            });
        }
        self.changed.store(true, Ordering::Release);
        Some(feed.addr())
    }

    fn data(&self, source: &str, n: usize) {
        lightair_engine::phone_camera::feed(source).data(n);
    }

    fn ended(&self, _source: &str) {
        self.changed.store(true, Ordering::Release);
    }
}

/// The Phone camera layers' state in the window.
#[derive(Default)]
pub(crate) struct Cameras {
    pub host: Arc<Host>,
    receiver: Option<lightair_phone_camera::srt::Receiver>,
    /// Open pairing codes, by layer: the offer and its link.
    offers: HashMap<Id, (Offer, String)>,
    /// Bytes received when last looked at, and the rate worked out (kb/s).
    rates: HashMap<Id, (u64, Instant, u32)>,
    ticking: Option<Task<()>>,
    error: Option<String>,
    /// Phones by USB (iPhones through usbmuxd, Android accessories) and, for developers, at TCP
    /// addresses (`LIGHTAIR_PHONE_LINK=127.0.0.1:4457`: the iOS simulator, `adb forward`).
    usb: Vec<Box<dyn std::any::Any>>,
}

impl Cameras {
    /// Whether the phones' port must run for cameras: a layer exists or a code is showing.
    pub(crate) fn wanted(&self) -> bool {
        !self.host.cams.lock().is_empty() || !self.offers.is_empty()
    }
}

/// Whether a (flattened) scene shows a source in any canvas.
fn shows(scene: Option<&Scene>, source: Id) -> bool {
    scene.is_some_and(|s| s.objects.iter().any(|o| o.source == Some(source) && o.layouts.values().any(|l| l.visible)))
}

impl StudioWindow {
    /// Keeps the camera host's layers, the SRT listener, phones' settings and tally in step
    /// with the workspace (on every change, and twice a second while a layer exists).
    pub(super) fn sync_phone_cameras(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let w = studio.workspace();
        let layers: Vec<(Id, String, Value)> =
            w.sources.values().filter(|s| s.kind == KIND).map(|s| (s.id, s.name.clone(), s.settings.clone())).collect();
        let c = &self.remote.phones.cameras;
        if layers.is_empty() && c.ticking.is_none() && c.host.cams.lock().is_empty() {
            return;
        }
        let staged_scene = studio.selected_scene.filter(|s| self.cue_scenes && Some(*s) != w.program);
        let program_scene = w.program.and_then(|s| w.scene(s)).map(|s| s.flatten());
        let staged_scene = staged_scene.and_then(|s| w.scene(s)).map(|s| s.flatten());
        let on_air = lightair_engine::outputs::runtime().any_stream_running()
            || lightair_engine::outputs::runtime().recording().state.is_running();
        let devices = self.remote.phones.pairing.devices();
        let key_der = self.remote.phones.identity_key();
        let mut tallies = vec![];
        {
            let host = self.remote.phones.cameras.host.clone();
            let mut cams = host.cams.lock();
            let keep: Vec<String> = layers.iter().map(|(id, ..)| id.to_string()).collect();
            cams.retain(|k, _| {
                let kept = keep.contains(k);
                if !kept {
                    lightair_engine::phone_camera::forget(k);
                }
                kept
            });
            for (id, name, settings) in &layers {
                let key = id.to_string();
                let cam = cams.entry(key.clone()).or_default();
                cam.name = name.clone();
                // (once the identity is read; a phone can't connect before)
                if cam.passphrase.is_empty()
                    && let Some(k) = &key_der
                {
                    cam.passphrase = passphrase(k, &key);
                }
                // the phone's settings: sent when they change
                let wanted = configure(settings);
                if cam.configured != wanted {
                    let restart = !cam.configured.is_null() && new_stream(&cam.configured, &wanted);
                    cam.configured = wanted.clone();
                    if let Some(c) = &cam.channel {
                        c.send(wanted);
                        if restart {
                            lightair_engine::phone_camera::feed(&key).restart();
                            c.send(json!({"type": "keyframe"}));
                        }
                    }
                }
                // tally
                let program = shows(program_scene.as_ref(), *id);
                let staged = shows(staged_scene.as_ref(), *id);
                let tally = json!({"type": "tally", "program": program, "staged": staged, "live": program && on_air});
                if cam.tally != tally {
                    if let Some(c) = &cam.channel {
                        c.send(tally.clone());
                    }
                    cam.tally = tally;
                    tallies
                        .push(json!({"source": id, "program": program, "staged": staged, "live": program && on_air}));
                }
                // what the placeholder says
                let feed = lightair_engine::phone_camera::feed(&key);
                let paired = devices.iter().find(|d| d.source.as_deref() == Some(key.as_str())).map(|d| d.name.clone());
                let message = match (&cam.channel, &cam.phone, paired) {
                    (Some(_), Some((phone, ..)), _) => format!("Waiting for {phone}'s picture"),
                    (None, _, Some(phone)) => format!("Waiting for {phone}"),
                    _ => "Phone camera".to_string(),
                };
                feed.set_message(&message);
            }
        }
        if let Some(server) = self.remote.running() {
            for t in tallies {
                server.emit("source.tally", t);
            }
        }
        self.sync_srt(cx);
        // twice a second while a layer exists: health, rates, placeholders
        let c = &mut self.remote.phones.cameras;
        if layers.is_empty() {
            c.ticking = None;
            c.rates.clear();
        } else if c.ticking.is_none() {
            c.ticking = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    if this.update(cx, |this, cx| this.tick_phone_cameras(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
    }

    /// The SRT listener runs while the phones' port does and a layer exists.
    pub(super) fn sync_srt(&mut self, _cx: &mut Context<Self>) {
        let wanted = self.remote.phones.listening() && !self.remote.phones.cameras.host.cams.lock().is_empty();
        let c = &mut self.remote.phones.cameras;
        if !wanted {
            c.receiver = None;
            c.usb.clear();
            return;
        }
        if c.usb.is_empty() {
            c.usb = usb_watchers(&c.host);
        }
        if c.receiver.is_some() {
            return;
        }
        let port = std::env::var("LIGHTAIR_PHONE_SRT_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(lightair_phone_camera::SRT_PORT);
        let feeds: Arc<dyn lightair_phone_camera::Feeds> = c.host.clone();
        match lightair_phone_camera::srt::Receiver::start(port, Duration::from_millis(LATENCY_MS), feeds) {
            Ok(r) => {
                let port = r.port();
                c.host.srt_port.store(port, Ordering::Release);
                c.receiver = Some(r);
                c.error = None;
                // phones already connected learn the port
                let cams = c.host.cams.lock();
                for cam in cams.values() {
                    if let Some(ch) = &cam.channel {
                        ch.send(c.host.media(cam));
                    }
                }
            }
            Err(e) => c.error = Some(format!("Phone cameras can't receive pictures: {e}")),
        }
    }

    fn tick_phone_cameras(&mut self, cx: &mut Context<Self>) {
        // the placeholder and tally follow what happens on their own (going live, a phone dropping)
        self.sync_phone_cameras(cx);
        let ids: Vec<Id> =
            self.studio.read(cx).workspace().sources.values().filter(|s| s.kind == KIND).map(|s| s.id).collect();
        let c = &mut self.remote.phones.cameras;
        for id in ids {
            let bytes = lightair_engine::phone_camera::feed(&id.to_string()).bytes();
            let now = Instant::now();
            let e = c.rates.entry(id).or_insert((bytes, now, 0));
            let secs = now.duration_since(e.1).as_secs_f64();
            if secs >= 1.0 {
                e.2 = ((bytes.saturating_sub(e.0)) as f64 * 8.0 / 1000.0 / secs) as u32;
                *e = (bytes, now, e.2);
            }
        }
        if c.host.changed.swap(false, Ordering::AcqRel) || self.selected_phone_camera(cx).is_some() {
            cx.notify();
        }
    }

    fn selected_phone_camera(&self, cx: &App) -> Option<Id> {
        let studio = self.studio.read(cx);
        studio
            .selected_object
            .and_then(|l| studio.selected_scene.and_then(|s| studio.workspace().scene(s)).and_then(|s| s.object(l)))
            .and_then(|l| l.source)
            .filter(|s| studio.workspace().sources.get(s).is_some_and(|i| i.kind == KIND))
    }

    /// Shows a pairing code for a layer: the phone that scans it becomes the layer's camera.
    pub(super) fn pair_camera(&mut self, source: Id, cx: &mut Context<Self>) {
        // the phones' port and identity first (the code carries the fingerprint)
        self.sync_phones(cx);
        if !self.remote.phones.ready() {
            let this = cx.entity().downgrade();
            cx.spawn(async move |_, cx| {
                for _ in 0..100 {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    let ready = this
                        .update(cx, |this, cx| {
                            this.sync_phones(cx);
                            this.remote.phones.ready()
                        })
                        .unwrap_or(true);
                    if ready {
                        let _ = this.update(cx, |this, cx| this.make_camera_offer(source, cx));
                        return;
                    }
                }
            })
            .detach();
            return;
        }
        self.make_camera_offer(source, cx);
    }

    fn make_camera_offer(&mut self, source: Id, cx: &mut Context<Self>) {
        let Some((offer, link)) = self.remote.phones.camera_offer(&source.to_string()) else { return };
        self.remote.phones.cameras.offers.insert(source, (offer, link));
        self.sync_phones(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(lightair_remote::pairing::OFFER_FOR + Duration::from_secs(1)).await;
            let _ = this.update(cx, |this, cx| {
                let p = &mut this.remote.phones;
                if !p.pairing.open() || p.cameras.offers.get(&source).is_some_and(|(o, _)| o.expires_at <= unix_now()) {
                    p.cameras.offers.remove(&source);
                }
                this.sync_phones(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn stop_camera_pairing(&mut self, source: Id, cx: &mut Context<Self>) {
        let p = &mut self.remote.phones;
        if let Some((offer, _)) = p.cameras.offers.remove(&source) {
            p.pairing.withdraw(&offer);
        }
        self.sync_phones(cx);
        cx.notify();
    }

    fn forget_camera_phone(&mut self, device: &str, cx: &mut Context<Self>) {
        self.remote.phones.pairing.remove(device);
        self.sync_phones(cx);
        cx.notify();
    }

    /// Sends a `control` message to a layer's phone (zoom, torch, focus, exposure, white balance).
    fn camera_control(&self, source: Id, control: Value) {
        let mut m = json!({"type": "control"});
        if let (Some(m), Some(c)) = (m.as_object_mut(), control.as_object()) {
            m.extend(c.clone());
        }
        self.remote.phones.cameras.host.send(&source.to_string(), m);
    }

    /// `camera.pair {source}` (a pairing link for a Phone camera layer; the person still allows
    /// the phone) and `camera.status {source?}` (each layer's phone, picture and health).
    pub(super) fn camera_call(&mut self, call: &Call, cx: &mut Context<Self>) -> Option<Reply> {
        if !call.method.starts_with("camera.") {
            return None;
        }
        let w = self.studio.read(cx).workspace().clone();
        let layer = |key: Option<&str>| -> Result<Vec<Id>, ApiError> {
            let all = w.sources.values().filter(|s| s.kind == KIND);
            match key {
                None => Ok(all.map(|s| s.id).collect()),
                Some(k) => all
                    .clone()
                    .find(|s| s.id.to_string() == k)
                    .or_else(|| all.clone().find(|s| s.name == k))
                    .map(|s| vec![s.id])
                    .ok_or_else(|| ApiError::not_found(format!("no phone camera {k}"))),
            }
        };
        let key = call.params.get("source").and_then(Value::as_str);
        Some(match call.method.as_str() {
            "camera.pair" => (|| {
                let id = layer(Some(key.ok_or_else(|| ApiError::invalid("`source` is needed"))?))?[0];
                self.sync_phones(cx);
                if !self.remote.phones.ready() {
                    // the identity is read on a thread: started now, ready in a moment
                    return Err(ApiError::state("the phones' port is starting: ask again in a moment"));
                }
                let (offer, link) = self.remote.phones.camera_offer(&id.to_string()).ok_or_else(|| ApiError::failed("no identity"))?;
                self.remote.phones.cameras.offers.insert(id, (offer.clone(), link.clone()));
                cx.notify();
                Ok(json!({"source": id, "link": link, "expires_at": offer.expires_at}))
            })(),
            "camera.status" => layer(key).map(|ids| {
                let cams = self.remote.phones.cameras.host.cams.lock();
                let list: Vec<Value> = ids
                    .iter()
                    .map(|id| {
                        let k = id.to_string();
                        let feed = lightair_engine::phone_camera::feed(&k);
                        let c = cams.get(&k);
                        json!({
                            "source": id,
                            "connected": c.is_some_and(|c| c.channel.is_some()),
                            "phone": c.and_then(|c| c.phone.as_ref()).map(|(n, m, p)| json!({"name": n, "model": m, "platform": p})),
                            "picture": feed.live(),
                            "usb": c.is_some_and(|c| c.usb > 0),
                            "rotation": feed.rotation(),
                            "received_kbps": self.remote.phones.cameras.rates.get(id).map_or(0, |r| r.2),
                            "state": c.map_or(Value::Null, |c| c.state.clone()),
                            "health": c.map_or(Value::Null, |c| c.health.clone()),
                            "tally": c.map_or(Value::Null, |c| c.tally.clone()),
                        })
                    })
                    .collect();
                json!({"cameras": list, "srt_port": self.remote.phones.cameras.host.srt_port.load(Ordering::Acquire)})
            }),
            "camera.control" => (|| {
                let id = layer(Some(key.ok_or_else(|| ApiError::invalid("`source` is needed"))?))?[0];
                let control = call.params.get("control").cloned().ok_or_else(|| ApiError::invalid("`control` is needed"))?;
                self.camera_control(id, control);
                Ok(json!({}))
            })(),
            _ => Err(ApiError::new(lightair_remote::ErrorKind::Unknown, format!("no method {}", call.method))),
        })
    }

    /// The Inspector's Phone camera section.
    pub(super) fn phone_camera_section(&self, source: &Source, cx: &mut Context<Self>) -> AnyElement {
        let id = source.id;
        let key = id.to_string();
        let s = source.settings.clone();
        let label = |text: &'static str| div().text_size(px(11.)).text_color(tok::text_tertiary()).child(text);
        let p = &self.remote.phones;
        let feed = lightair_engine::phone_camera::feed(&key);
        let live = feed.live();
        let usb = p.cameras.host.cams.lock().get(&key).is_some_and(|c| c.usb > 0);
        let (channel, phone, hello, state, health, fresh) = {
            let cams = p.cameras.host.cams.lock();
            match cams.get(&key) {
                Some(c) => (
                    c.channel.is_some(),
                    c.phone.clone(),
                    c.hello.clone(),
                    c.state.clone(),
                    c.health.clone(),
                    c.health_at.is_some_and(|t| t.elapsed() < Duration::from_secs(3)),
                ),
                None => (false, None, Value::Null, Value::Null, Value::Null, false),
            }
        };
        let paired: Vec<Device> =
            p.pairing.devices().into_iter().filter(|d| d.source.as_deref() == Some(key.as_str())).collect();
        let rate = p.cameras.rates.get(&id).map_or(0, |r| r.2);
        let remote_on = self.remote.running().is_some();

        // what it's doing, in a line
        let phone_name = phone.as_ref().map(|p| p.0.clone()).or_else(|| paired.first().map(|d| d.name.clone()));
        let (dot, line) = if !remote_on {
            (tok::warn(), "Turn on the remote API (Settings › Remote API) to use a phone as a camera".to_string())
        } else if let Some(e) = p.error.clone().or_else(|| p.cameras.error.clone()) {
            (tok::error(), e)
        } else if live {
            (
                tok::ok(),
                format!(
                    "Live from {}{} · {:.1} Mb/s",
                    phone_name.as_deref().unwrap_or("the phone"),
                    if usb { " by USB" } else { "" },
                    rate as f64 / 1000.0
                ),
            )
        } else if channel {
            (
                tok::warn(),
                format!("{} is connected, waiting for its picture", phone_name.as_deref().unwrap_or("The phone")),
            )
        } else if let Some(n) = &phone_name {
            (tok::text_tertiary(), format!("Waiting for {n}: open Camera in Lightair Live"))
        } else {
            (tok::text_tertiary(), "Pair a phone to use it as this camera".to_string())
        };
        let status = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
            .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(tok::text_secondary()).child(line));

        // the phone's health (MC-13)
        let health_line = (channel && fresh).then(|| {
            let mut parts = vec![];
            if let Some(b) = health["battery"].as_f64() {
                let charging = health["charging"].as_bool() == Some(true);
                parts.push(format!("Battery {:.0}%{}", b * 100.0, if charging { " (charging)" } else { "" }));
            }
            if let Some(t) = health["thermal"].as_str() {
                parts.push(format!("thermal {t}"));
            }
            if let Some(k) = health["bitrate_kbps"].as_u64() {
                parts.push(format!("sending {:.1} Mb/s", k as f64 / 1000.0));
            }
            if let Some(r) = health["rtt_ms"].as_u64() {
                parts.push(format!("{r} ms away"));
            }
            if let Some(d) = health["dropped"].as_u64().filter(|d| *d > 0) {
                parts.push(format!("{d} dropped"));
            }
            if health["screen_off"].as_bool() == Some(true) {
                parts.push("screen off".into());
            }
            let worry = matches!(health["thermal"].as_str(), Some("serious" | "critical"))
                || health["battery"].as_f64().is_some_and(|b| b < 0.15) && health["charging"].as_bool() != Some(true);
            div()
                .pl(px(15.))
                .text_size(px(11.))
                .text_color(if worry { tok::warn() } else { tok::text_tertiary() })
                .child(parts.join(" · "))
        });

        // pairing: the code, or the phones paired to this layer
        let offer = p.cameras.offers.get(&id).filter(|(o, _)| o.expires_at > unix_now()).cloned();
        let mut pairing = div().flex().flex_col().gap_2();
        if let Some((offer, link)) = &offer {
            pairing = pairing.child(super::phones::pairing_code(link, offer, true)).child(
                Button::new("phone-camera-pair-done")
                    .label("Done")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(move |this, _, _, cx| this.stop_camera_pairing(id, cx))),
            );
        } else {
            for d in &paired {
                let device = d.id.clone();
                let what = [d.model.as_str(), d.platform.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" · ");
                pairing = pairing.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(ui::icon("smartphone").small().text_color(tok::text_tertiary()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(12.)).child(d.name.clone()))
                                .child(div().text_size(px(11.)).text_color(tok::text_tertiary()).child(what)),
                        )
                        .child(
                            Button::new(SharedString::from(format!("phone-camera-forget-{}", d.id)))
                                .label("Forget")
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| this.forget_camera_phone(&device, cx))),
                        ),
                );
            }
            pairing = pairing.child(
                div().child(
                    Button::new("phone-camera-pair")
                        .icon(ui::icon("qr-code"))
                        .label(if paired.is_empty() { "Pair a phone" } else { "Pair another phone" })
                        .small()
                        .outline()
                        .disabled(!remote_on)
                        .on_click(cx.listener(move |this, _, _, cx| this.pair_camera(id, cx))),
                ),
            );
        }

        // the picture asked of the phone
        let set = |key: &str, v: Value| Command::SetSourceSettings { source: id, settings: with(&s, key, v) };
        let (w, h, fps) = (
            s["width"].as_u64().unwrap_or(1920),
            s["height"].as_u64().unwrap_or(1080),
            s["fps"].as_u64().unwrap_or(30),
        );
        let formats =
            [("720p", 1280, 720, 30), ("1080p", 1920, 1080, 30), ("1080p60", 1920, 1080, 60), ("4K", 3840, 2160, 30)]
                .into_iter()
                .map(|(l, fw, fh, ff)| {
                    let mut v = with(&s, "width", json!(fw));
                    v["height"] = json!(fh);
                    v["fps"] = json!(ff);
                    (l, Command::SetSourceSettings { source: id, settings: v }, (w, h, fps) == (fw, fh, ff))
                })
                .collect();
        let codec = s["codec"].as_str().unwrap_or("h264");
        let codecs = [("H.264", "h264"), ("HEVC", "hevc")]
            .into_iter()
            .map(|(l, v)| (l, set("codec", json!(v)), codec == v))
            .collect();
        let kbps = s["bitrate_kbps"].as_u64().unwrap_or(12000);
        let bitrates = [("6 Mb/s", 6000), ("12 Mb/s", 12000), ("20 Mb/s", 20000)]
            .into_iter()
            .map(|(l, v)| (l, set("bitrate_kbps", json!(v)), kbps == v))
            .collect();
        let orientation = s["orientation"].as_str().unwrap_or("follow");
        let orientations = [("As held", "follow"), ("Landscape", "landscape"), ("Portrait", "portrait")]
            .into_iter()
            .map(|(l, v)| (l, set("orientation", json!(v)), orientation == v))
            .collect();

        // the phone's lenses (from its hello)
        let lenses: Vec<(String, String)> = hello["capabilities"]["lenses"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|l| {
                        Some((
                            l["id"].as_str()?.to_string(),
                            l["name"].as_str().unwrap_or(l["id"].as_str()?).to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let lens_now = state["lens"].as_str().or(s["lens"].as_str()).unwrap_or("").to_string();
        let lens_shown = lenses
            .iter()
            .find(|(l, _)| *l == lens_now)
            .map_or_else(|| "The phone's choice".to_string(), |(_, n)| n.clone());
        let me = cx.entity().downgrade();
        let base = s.clone();
        let lens_menu = {
            let lenses = lenses.clone();
            Button::new("phone-camera-lens").label(lens_shown).small().outline().dropdown_menu(move |mut menu, _, _| {
                for (value, name) in &lenses {
                    let me = me.clone();
                    let cmd = Command::SetSourceSettings { source: id, settings: with(&base, "lens", json!(value)) };
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(*value == lens_now).on_click(
                        move |_, _, cx| {
                            if let Some(me) = me.upgrade() {
                                me.update(cx, |this, cx| this.exec(cmd.clone(), cx));
                            }
                        },
                    ));
                }
                menu
            })
        };

        // live controls (MC-06, MC-11): sent to the phone, its state shown back
        let controls: Vec<&str> = hello["capabilities"]["controls"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let has = |c: &str| controls.is_empty() || controls.contains(&c);
        let zoom = state["zoom"].as_f64().unwrap_or(1.0);
        let torch = state["torch"].as_f64().unwrap_or(0.0) > 0.0;
        let locked = |k: &str| state[k]["mode"].as_str() == Some("locked");
        let chip = |key: &'static str, i: usize, text: String, on: bool, msg: Value, cx: &mut Context<Self>| {
            div()
                .id((key, i))
                .px_2()
                .py_1()
                .rounded(px(6.))
                .text_size(px(11.))
                .cursor_pointer()
                .when(on, |d| d.bg(tok::bg_selected()).text_color(tok::text()))
                .when(!on, |d| {
                    d.bg(tok::bg_raised()).text_color(tok::text_secondary()).hover(|d| d.text_color(tok::text()))
                })
                .child(text)
                .on_click(cx.listener(move |this, _, _, _| this.camera_control(id, msg.clone())))
        };
        let mut zooms = div().flex().flex_wrap().gap_1();
        for (i, z) in [1.0, 2.0, 3.0, 5.0].into_iter().enumerate() {
            zooms = zooms.child(chip(
                "phone-camera-zoom",
                i,
                format!("{z}×"),
                (zoom - z).abs() < 0.05,
                json!({"zoom": z}),
                cx,
            ));
        }
        let mut locks = div().flex().flex_wrap().gap_1();
        for (i, (name, key, on)) in [
            ("Focus", "focus", locked("focus")),
            ("Exposure", "exposure", locked("exposure")),
            ("White balance", "white_balance", locked("white_balance")),
        ]
        .into_iter()
        .enumerate()
        .filter(|(_, (_, k, _))| has(k))
        {
            let msg = json!({ key: {"mode": if on { "auto" } else { "locked" }} });
            locks = locks.child(chip(
                "phone-camera-lock",
                i,
                format!("{name} {}", if on { "locked" } else { "auto" }),
                on,
                msg,
                cx,
            ));
        }
        if has("torch") {
            locks = locks.child(chip(
                "phone-camera-torch",
                0,
                "Torch".into(),
                torch,
                json!({"torch": if torch { 0.0 } else { 1.0 }}),
                cx,
            ));
        }

        let row = |name: &'static str, control: AnyElement| {
            div().flex().items_center().justify_between().gap_2().child(label(name)).child(control)
        };
        self.section("Phone camera", None, cx)
            .child(status)
            .children(health_line)
            .child(pairing)
            .child(label("Picture"))
            .child(self.segmented("phone-camera-format", formats, cx))
            .child(self.segmented("phone-camera-codec", codecs, cx))
            .child(self.segmented("phone-camera-bitrate", bitrates, cx))
            .child(div().flex().items_center().gap_1().child(label("Orientation")).child(ui::info(
                "phone-camera-orientation-info",
                "As held: the picture turns with the phone. Landscape or Portrait: it stays that way however the phone is held.",
            )))
            .child(self.segmented("phone-camera-orientation", orientations, cx))
            .when(!lenses.is_empty(), |d| d.child(row("Lens", lens_menu.into_any_element())))
            .when(channel, |d| d.child(label("Zoom")).child(zooms).child(label("Camera")).child(locks))
            .into_any_element()
    }
}

/// The watchers that keep USB links to phones (and to TCP addresses, for developers).
fn usb_watchers(host: &Arc<Host>) -> Vec<Box<dyn std::any::Any>> {
    use lightair_phone_camera::{aoa, tcp, usbmux};
    let links: Arc<dyn lightair_phone_camera::link::LinkHost> = host.clone();
    let mut out: Vec<Box<dyn std::any::Any>> = vec![];
    match usbmux::Watcher::start(usbmux::Address::system(), links.clone()) {
        Ok(w) => out.push(Box::new(w)),
        Err(e) => tracing::warn!(%e, "phone cameras: iPhones by USB"),
    }
    let android = host.clone();
    let serial = lightair_phone_camera::link::LinkHost::desktop_id(&**host);
    match aoa::Watcher::start(links.clone(), serial, Arc::new(move || android.wants_android())) {
        Ok(w) => out.push(Box::new(w)),
        Err(e) => tracing::warn!(%e, "phone cameras: Android phones by USB"),
    }
    let addresses: Vec<std::net::SocketAddr> = std::env::var("LIGHTAIR_PHONE_LINK")
        .unwrap_or_default()
        .split(',')
        .filter_map(|a| a.trim().parse().ok())
        .collect();
    if !addresses.is_empty() {
        match tcp::Watcher::start(&addresses, links) {
            Ok(w) => out.push(Box::new(w)),
            Err(e) => tracing::warn!(%e, "phone cameras: links to TCP addresses"),
        }
    }
    out
}

fn with(settings: &Value, key: &str, value: Value) -> Value {
    let mut s = if settings.is_object() { settings.clone() } else { json!({}) };
    s[key] = value;
    s
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What the consent alert asks for a camera: "Allow Sam's iPhone to be the camera for "Desk"?"
pub(super) fn consent_question(host: &Host, name: &str, source: Option<&str>) -> String {
    match source.and_then(|s| host.layer_name(s)) {
        Some(layer) => format!("Allow {name} to be the camera for \u{201c}{layer}\u{201d}?"),
        None => format!("Allow {name} to be a camera for Lightair?"),
    }
}

#[cfg(test)]
mod tests {
    // (not `super::*`: gpui's `test` attribute would shadow the standard one)
    use super::{Cam, Host, configure, consent_question, default_settings, new_stream, passphrase};
    use lightair_remote::CameraHost;
    use serde_json::json;

    #[test]
    fn configure_follows_the_layer() {
        let c = configure(&default_settings());
        assert_eq!(c["type"], "configure");
        assert_eq!(c["format"], json!({"w": 1920, "h": 1080, "fps": 30}));
        assert_eq!(c["codec"], "h264");
        assert_eq!(c["orientation"], "follow");
        assert!(c["lens"].is_null());
        let mut s = default_settings();
        s["codec"] = json!("hevc");
        assert!(new_stream(&c, &configure(&s)));
        s["codec"] = json!("h264");
        s["bitrate_kbps"] = json!(6000);
        assert!(!new_stream(&c, &configure(&s)));
    }

    #[test]
    fn the_host_refuses_phones_for_unknown_layers() {
        let host = Host::default();
        assert_eq!(lightair_phone_camera::Feeds::passphrase(&host, "nope"), None);
        host.cams
            .lock()
            .insert("cam".into(), Cam { name: "Desk".into(), passphrase: "p".repeat(16), ..Default::default() });
        host.cams.lock().insert("new".into(), Cam::default());
        assert_eq!(lightair_phone_camera::Feeds::passphrase(&host, "new"), None, "no passphrase before the identity");
        assert_eq!(lightair_phone_camera::Feeds::passphrase(&host, "cam").as_deref(), Some("pppppppppppppppp"));
        assert_eq!(
            consent_question(&host, "Sam's iPhone", Some("cam")),
            "Allow Sam's iPhone to be the camera for \u{201c}Desk\u{201d}?"
        );
    }

    #[test]
    fn passphrases_are_per_layer_and_stay_put() {
        let a = passphrase(b"key one", "cam-1");
        assert_eq!(a.len(), 32);
        assert_eq!(a, passphrase(b"key one", "cam-1"));
        assert_ne!(a, passphrase(b"key one", "cam-2"));
        assert_ne!(a, passphrase(b"key two", "cam-1"));
    }

    #[test]
    fn state_messages_merge() {
        let host = Host::default();
        host.cams.lock().insert("cam".into(), Cam::default());
        host.message("cam", &json!({"type": "state", "lens": "back-wide", "zoom": 1.0}));
        host.message("cam", &json!({"type": "state", "zoom": 2.0}));
        let cams = host.cams.lock();
        assert_eq!(cams["cam"].state["lens"], "back-wide");
        assert_eq!(cams["cam"].state["zoom"], 2.0);
    }
}
