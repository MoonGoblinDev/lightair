//! Phones on the remote API (Lightair Live's Remote): lightair-remote's TLS port for paired
//! devices, beside the plain one. This file keeps what the app owns of it:
//!
//! - the desktop's id and name, and its TLS identity (certificate and key in the Keychain, as
//!   "Lightair remote TLS"), made the first time a phone pairs;
//! - the paired devices (`Phones/devices.json` in Lightair's data; keys are public keys);
//! - asking the person to allow a phone (a native alert: "Allow Sam's iPhone to control
//!   Lightair?");
//! - the port and its mDNS advertisement running while the remote API is on and a phone is
//!   paired or the pairing code is showing;
//! - `audio.levels` at 20 Hz while a phone is connected;
//! - Settings › Remote API › Phones: the pairing QR code and the paired devices.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::*;
use lightair_remote::mdns::Advertiser;
use lightair_remote::pairing::{Offer, devices_from_json, devices_to_json};
use lightair_remote::{
    DesktopInfo, DeviceRequest, Identity, Pairing, SECURE_PORT, Scope, SecureConfig, SecureListener,
};
use serde_json::{Value, json};

use super::StudioWindow;
use crate::ui::tok;

/// What phones are told this desktop answers (lightair-live's docs/05 §9).
const FEATURES: &[&str] = &[
    "audio.levels",
    "preview.jpeg",
    "scene.thumbnails",
    "panic",
    "staging",
    "chat",
    "activity",
    "deck",
    "stream.info",
    "outputs",
    "issues",
    "tally",
    "chat.moderate",
    "activity.replay",
    "stream.categories",
    "issues.fix",
];
/// How often meters go out while a phone is connected.
const METERS_EVERY: Duration = Duration::from_millis(50);
const KEYCHAIN_SERVICE: &str = "Lightair remote TLS";

pub(crate) struct Phones {
    pub pairing: Arc<Pairing>,
    pub desktop: DesktopInfo,
    identity: Option<Identity>,
    reading_identity: bool,
    listener: Option<SecureListener>,
    mdns: Option<Advertiser>,
    /// The offer the QR code shows, and its link.
    pub offer: Option<(Offer, String)>,
    /// The same pairing to type instead (05 §2, "Without a camera"): its offer, its six words,
    /// and this computer's fingerprint as four words to compare.
    words: Option<(Offer, String, String)>,
    pub error: Option<String>,
    meters: Option<Task<()>>,
    /// Previews for phones' pictures: the program's, and scenes' for thumbnails.
    program_preview: Option<PhonePreview>,
    scene_preview: Option<PhonePreview>,
    panic: Panic,
    /// The last chat event phones were told about, and the staging state they last saw.
    told_chat: u64,
    told_staging: Option<Value>,
    told_at: std::time::Instant,
    /// Each destination's bytes sent when last looked at, and the bitrate worked out from them.
    sent: std::collections::HashMap<lightair_model::Id, (u64, std::time::Instant, u32)>,
    /// The issues phones were told about, and when outputs' health was last sent.
    told_issues: Vec<Value>,
    told_health: std::time::Instant,
    /// Phone camera layers (`phone_camera`).
    pub cameras: super::phone_camera::Cameras,
    /// Each scene's last told thumbnail: (content hash, when, version).
    thumbs: std::collections::HashMap<lightair_model::Id, (u64, std::time::Instant, u64)>,
    /// Debug builds with `LIGHTAIR_DEV_PAIRING=1`: keeps a fresh control link written out.
    dev_link: Option<Task<()>>,
    /// Lightair Editor's round trip (`editor_sync`).
    pub editor_sync: Arc<super::editor_sync::EditorSync>,
}

fn folder() -> PathBuf {
    lightair_platform::paths::data_dir().join("Phones")
}

/// The desktop's id (made once) and the computer's name.
fn desktop_info() -> DesktopInfo {
    let file = folder().join("desktop-id");
    let id = std::fs::read_to_string(&file).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_else(
        || {
            let mut b = [0u8; 16];
            let _ = getrandom::fill(&mut b);
            let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
            let _ = std::fs::create_dir_all(folder());
            let _ = std::fs::write(&file, &id);
            id
        },
    );
    DesktopInfo { id, name: computer_name(), version: env!("CARGO_PKG_VERSION").into() }
}

fn computer_name() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/sbin/scutil").args(["--get", "ComputerName"]).output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !name.is_empty() {
            return name;
        }
    }
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Lightair".into())
}

/// The TLS identity from the Keychain, made (and saved) the first time.
fn load_identity(id: &str) -> Result<Identity, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    // a dev build (LIGHTAIR_DEV_PAIRING): its own identity in a file, not the Keychain
    if dev_pairing() {
        let file = folder().join("dev-identity.txt");
        if let Ok(saved) = std::fs::read_to_string(&file)
            && let Some((cert, key)) = saved.trim().split_once('.')
            && let (Ok(cert_der), Ok(key_der)) = (b64.decode(cert), b64.decode(key))
        {
            return Ok(Identity { cert_der, key_der });
        }
        let identity = Identity::generate(id)?;
        let _ = std::fs::create_dir_all(folder());
        std::fs::write(&file, format!("{}.{}", b64.encode(&identity.cert_der), b64.encode(&identity.key_der)))
            .map_err(|e| e.to_string())?;
        return Ok(identity);
    }
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, "identity").map_err(|e| e.to_string())?;
    if let Ok(saved) = entry.get_password()
        && let Some((cert, key)) = saved.split_once('.')
        && let (Ok(cert_der), Ok(key_der)) = (b64.decode(cert), b64.decode(key))
    {
        return Ok(Identity { cert_der, key_der });
    }
    let identity = Identity::generate(id)?;
    entry
        .set_password(&format!("{}.{}", b64.encode(&identity.cert_der), b64.encode(&identity.key_der)))
        .map_err(|e| e.to_string())?;
    Ok(identity)
}

impl Phones {
    /// Lets go of the port (the server it shares a handler with is restarting).
    pub(super) fn stop_listening(&mut self) {
        self.cameras.host.set_phones_port(0);
        self.listener = None;
        self.mdns = None;
    }

    /// The TLS identity's private key, once read (phone cameras' passphrases are made from it).
    pub(super) fn identity_key(&self) -> Option<Vec<u8>> {
        self.identity.as_ref().map(|i| i.key_der.clone())
    }

    /// Whether the phones' port is open.
    pub(super) fn listening(&self) -> bool {
        self.listener.is_some()
    }

    /// Whether a pairing code can be made now (the identity read and the port open).
    pub(super) fn ready(&self) -> bool {
        self.identity.is_some() && self.listener.is_some()
    }

    /// A camera pairing code for a layer (`c=<source>`): the offer and its link.
    pub(super) fn camera_offer(&self, source: &str) -> Option<(Offer, String)> {
        let identity = self.identity.as_ref()?;
        let offer = self.pairing.offer_camera(source);
        let addresses: Vec<String> = lightair_platform::net::local_ip().into_iter().collect();
        let port = self.listener.as_ref().map_or(SECURE_PORT, |l| l.addr().port());
        let link = self.pairing.link(&offer, &self.desktop, &addresses, port, &identity.fingerprint_b64());
        Some((offer, link))
    }
}

use crate::secrets::dev_pairing;

/// Whether `peer` is this computer (a simulator runs on it).
fn from_this_computer(peer: std::net::IpAddr) -> bool {
    peer.is_loopback()
        || peer.to_canonical().is_loopback()
        || lightair_platform::net::local_ip().is_some_and(|ip| ip == peer.to_canonical().to_string())
}

impl StudioWindow {
    pub(super) fn new_phones(window: &mut Window, cx: &mut Context<Self>) -> Phones {
        // asking the person happens on the main thread, as a native alert; the phone's connection
        // waits for the answer (a minute at most)
        let (asks, mut queue) = futures_channel::mpsc::unbounded::<(DeviceRequest, std::sync::mpsc::Sender<bool>)>();
        let cameras = super::phone_camera::Cameras::default();
        let host = cameras.host.clone();
        cx.spawn_in(window, async move |_, cx| {
            use futures_util::StreamExt;
            while let Some((request, reply)) = queue.next().await {
                let scope = match request.scope {
                    Scope::Watch => "watch your show (it can't change anything)",
                    _ => "control Lightair",
                };
                let message = if request.scope == Scope::Camera {
                    super::phone_camera::consent_question(&host, &request.name, request.source.as_deref())
                } else {
                    format!("Allow {} to {scope}?", request.name)
                };
                let detail = format!(
                    "{} · {} · from {}. You can remove it any time in Settings › Remote API.",
                    if request.model.is_empty() { "A phone" } else { request.model.as_str() },
                    request.platform,
                    request.peer
                );
                let answer = cx.update(|window, cx| {
                    window.prompt(PromptLevel::Info, &message, Some(&detail), &["Allow", "Don't Allow"], cx)
                });
                let allowed = match answer {
                    Ok(rx) => rx.await.ok() == Some(0),
                    Err(_) => false,
                };
                let _ = reply.send(allowed);
            }
        })
        .detach();
        let confirm = move |request: &DeviceRequest| {
            if dev_pairing() && request.scope != Scope::Camera && from_this_computer(request.peer) {
                tracing::warn!(name = %request.name, "phones: paired without asking (LIGHTAIR_DEV_PAIRING)");
                return true;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            if asks.unbounded_send((request.clone(), tx)).is_err() {
                return false;
            }
            rx.recv_timeout(Duration::from_secs(60)).unwrap_or(false)
        };
        let devices =
            std::fs::read_to_string(folder().join("devices.json")).map(|t| devices_from_json(&t)).unwrap_or_default();
        let save = |devices: &[lightair_remote::Device]| {
            let _ = std::fs::create_dir_all(folder());
            let _ = std::fs::write(folder().join("devices.json"), devices_to_json(devices));
        };
        let pairing = Pairing::new(devices, confirm, save);
        let desktop = desktop_info();
        cameras.host.set_pairing(pairing.clone(), desktop.id.clone());
        Phones {
            pairing,
            desktop,
            identity: None,
            reading_identity: false,
            listener: None,
            mdns: None,
            offer: None,
            words: None,
            error: None,
            meters: None,
            program_preview: None,
            scene_preview: None,
            panic: Panic::default(),
            told_chat: u64::MAX,
            told_staging: None,
            told_at: std::time::Instant::now(),
            sent: Default::default(),
            told_issues: vec![],
            told_health: std::time::Instant::now(),
            cameras,
            editor_sync: Default::default(),
            thumbs: Default::default(),
            dev_link: None,
        }
    }

    /// Runs the phones' port while the remote API is on and a phone is paired or the pairing code
    /// shows; stops it otherwise.
    pub(super) fn sync_phones(&mut self, cx: &mut Context<Self>) {
        // Phone camera layers want the port too
        self.sync_phone_cameras(cx);
        // the Editor's round trip answers from the workspace open now
        self.remote.phones.editor_sync.set_workspace(self.studio.read(cx).workspace_dir());
        let running = self.remote.running().is_some();
        let p = &mut self.remote.phones;
        if p.offer.as_ref().is_some_and(|(o, _)| !p.pairing.open() && o.expires_at > 0) {
            // the code expired
            p.offer = None;
        }
        let wanted =
            running && (!p.pairing.devices().is_empty() || p.offer.is_some() || p.cameras.wanted() || dev_pairing());
        if !wanted {
            p.cameras.host.set_phones_port(0);
            p.listener = None;
            p.mdns = None;
            p.meters = None;
            self.sync_srt(cx);
            return;
        }
        let Some(identity) = p.identity.clone() else {
            if !p.reading_identity {
                p.reading_identity = true;
                let id = p.desktop.id.clone();
                cx.spawn(async move |this, cx| {
                    let identity = cx.background_executor().spawn(async move { load_identity(&id) }).await;
                    let _ = this.update(cx, |this, cx| {
                        let p = &mut this.remote.phones;
                        p.reading_identity = false;
                        match identity {
                            Ok(i) => p.identity = Some(i),
                            Err(e) => p.error = Some(format!("Lightair couldn't make its identity for phones: {e}")),
                        }
                        this.sync_phones(cx);
                        cx.notify();
                    });
                })
                .detach();
            }
            return;
        };
        if p.listener.is_none() {
            let port = std::env::var("LIGHTAIR_PHONES_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(SECURE_PORT);
            let config = SecureConfig {
                port,
                identity: identity.clone(),
                desktop: p.desktop.clone(),
                features: FEATURES.iter().chain(super::editor_sync::FEATURES).map(|f| f.to_string()).collect(),
            };
            let started = self.remote.running().map(|s| s.start_secure(config, self.remote.phones.pairing.clone()));
            let transfers = self.remote.running().map(|s| s.transfers());
            let p = &mut self.remote.phones;
            match started {
                Some(Ok(l)) => {
                    // phone cameras' control channel (lightair-live's docs/05 §6)
                    l.cameras(p.cameras.host.clone());
                    if let Some(t) = transfers {
                        p.editor_sync.connect(t, p.pairing.clone());
                    }
                    p.cameras.host.set_phones_port(l.addr().port());
                    p.listener = Some(l);
                    p.error = None;
                }
                Some(Err(e)) => {
                    p.error = Some(if e.kind() == std::io::ErrorKind::AddrInUse {
                        format!("Port {port}, which phones use, is taken by another app.")
                    } else {
                        e.to_string()
                    });
                    return;
                }
                None => return,
            }
        }
        let p = &mut self.remote.phones;
        if p.mdns.is_none() {
            let port = p.listener.as_ref().map_or(SECURE_PORT, |l| l.addr().port());
            match Advertiser::start(p.desktop.clone(), port, &identity.short_fingerprint()) {
                Ok(a) => p.mdns = Some(a),
                Err(e) => tracing::warn!("phones: mDNS: {e}"),
            }
        }
        let live = lightair_engine::outputs::runtime().any_stream_running()
            || lightair_engine::outputs::runtime().recording().state.is_running();
        if let Some(m) = &p.mdns {
            m.update(p.pairing.open(), live);
        }
        if dev_pairing() && p.dev_link.is_none() {
            p.dev_link = Some(cx.spawn(async move |this, cx| {
                loop {
                    if this.update(cx, |this, cx| this.write_dev_link(cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_secs(5)).await;
                }
            }));
        }
        if p.meters.is_none() {
            p.meters = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(METERS_EVERY).await;
                    if this.update(cx, |this, cx| this.emit_levels(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        self.sync_srt(cx);
    }

    /// `LIGHTAIR_DEV_PAIRING`: a control link to `<workspace>/dev-pairing-link.txt` (and
    /// stdout) whenever none is open, for a simulator to pair with no one at the computer.
    fn write_dev_link(&mut self, cx: &mut Context<Self>) {
        let p = &self.remote.phones;
        if p.pairing.open() {
            return;
        }
        let (Some(identity), Some(listener)) = (p.identity.as_ref(), p.listener.as_ref()) else { return };
        let offer = p.pairing.offer(Scope::Control);
        let addresses: Vec<String> = lightair_platform::net::local_ip().into_iter().chain(["127.0.0.1".to_string()]).collect();
        let link = p.pairing.link(&offer, &p.desktop, &addresses, listener.addr().port(), &identity.fingerprint_b64());
        println!("LIGHTAIR_DEV_PAIRING {link}");
        if let Some(dir) = self.studio.read(cx).workspace_dir() {
            let _ = std::fs::write(dir.join("dev-pairing-link.txt"), format!("{link}\n"));
        }
    }

    /// `audio.levels` to the phones that asked (lightair-live's docs/05 §4.2): dBFS per channel,
    /// silence as null.
    fn emit_levels(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.remote.running() else { return };
        if !server.clients().iter().any(|c| c.protocol == "Lightair phone") {
            return;
        }
        // chat, activity and staging four times a second
        if self.remote.phones.told_at.elapsed() >= Duration::from_millis(250) {
            self.remote.phones.told_at = std::time::Instant::now();
            self.tell_phones(cx);
        }
        let Some(server) = self.remote.running() else { return };
        let db = |v: f32| if v.is_finite() { json!(v) } else { Value::Null };
        let w = self.studio.read(cx).workspace().clone();
        let tracks: Vec<Value> = w
            .tracks
            .iter()
            .map(|t| {
                let l = lightair_engine::meters::meters().get(t.id).map(|c| c.levels());
                let (peak, rms) = l.map_or(([f32::NEG_INFINITY; 2], [f32::NEG_INFINITY; 2]), |l| (l.peak, l.magnitude));
                json!({
                    "track": t.id,
                    "peak": [db(peak[0]), db(peak[1])],
                    "rms": [db(rms[0]), db(rms[1])],
                    "clip": peak.iter().any(|p| *p >= 0.0),
                })
            })
            .collect();
        server.emit("audio.levels", json!({"tracks": tracks}));
    }

    /// Shows a pairing code (Settings › Remote API › Pair a phone).
    pub(super) fn pair_phone(&mut self, cx: &mut Context<Self>) {
        // the identity is needed for the code's fingerprint: start everything first
        let p = &mut self.remote.phones;
        if p.identity.is_none() {
            p.offer = Some((Offer { secret: [0; 16], scope: Scope::Control, expires_at: 0 }, String::new()));
            self.sync_phones(cx);
            let this = cx.entity().downgrade();
            cx.spawn(async move |_, cx| {
                for _ in 0..100 {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    let ready = this.update(cx, |this, _| this.remote.phones.identity.is_some()).unwrap_or(true);
                    if ready {
                        let _ = this.update(cx, |this, cx| this.make_offer(cx));
                        return;
                    }
                }
            })
            .detach();
            return;
        }
        self.make_offer(cx);
    }

    fn make_offer(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.remote.phones;
        let Some(identity) = &p.identity else { return };
        let offer = p.pairing.offer(Scope::Control);
        let addresses: Vec<String> = lightair_platform::net::local_ip().into_iter().collect();
        let port = p.listener.as_ref().map_or(SECURE_PORT, |l| l.addr().port());
        let link = p.pairing.link(&offer, &p.desktop, &addresses, port, &identity.fingerprint_b64());
        p.offer = Some((offer, link));
        let (typed, words) = p.pairing.offer_words(Scope::Control);
        let fp = lightair_remote::pairing::fingerprint_words(&identity.fingerprint()).join(" ");
        p.words = Some((typed, words.join(" "), fp));
        self.sync_phones(cx);
        // the code lasts two minutes; the sheet follows it
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(lightair_remote::pairing::OFFER_FOR + Duration::from_secs(1)).await;
            let _ = this.update(cx, |this, cx| {
                this.sync_phones(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn stop_pairing(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.remote.phones;
        if let Some((offer, _)) = p.offer.take() {
            p.pairing.withdraw(&offer);
        }
        if let Some((offer, ..)) = p.words.take() {
            p.pairing.withdraw(&offer);
        }
        self.sync_phones(cx);
        cx.notify();
    }

    pub(super) fn forget_phone(&mut self, id: &str, cx: &mut Context<Self>) {
        self.remote.phones.pairing.remove(id);
        self.sync_phones(cx);
        cx.notify();
    }

    /// Settings › Remote API › Phones.
    pub(super) fn phones_group(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.remote.phones;
        let devices = p.pairing.devices();
        let on = self.remote.running().is_some();
        let detail = if !on {
            "Turn on the remote API to pair Lightair Live on a phone or tablet.".to_string()
        } else if let Some(e) = &p.error {
            e.clone()
        } else if devices.is_empty() {
            "Lightair Live controls your show from a phone: scenes, audio, chat and going live.".to_string()
        } else {
            format!("{} paired · Lightair Live finds this computer on your network.", devices.len())
        };
        let mut group = super::settings::group("Phones").child(super::settings::row(
            "Pair a phone",
            Some(&detail),
            if p.offer.is_some() {
                Button::new("phones-cancel")
                    .label("Done")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| this.stop_pairing(cx)))
            } else {
                Button::new("phones-pair")
                    .label("Pair a phone")
                    .xsmall()
                    .outline()
                    .disabled(!on)
                    .on_click(cx.listener(|this, _, _, cx| this.pair_phone(cx)))
            },
        ));
        // a code that has run out isn't shown (its timer tidies up)
        if let Some((offer, link)) = &p.offer
            && (offer.expires_at == 0 || p.pairing.open())
        {
            group = group.child(pairing_code(link, offer, false));
            if let Some((_, words, fp)) = &p.words {
                group = group.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .rounded(px(8.))
                        .bg(tok::bg_raised())
                        .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child(
                            "No camera on the phone? In Lightair Live › Found on this network, choose this computer and type:",
                        ))
                        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(words.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(tok::text_tertiary())
                                .child(format!("The phone should show this computer as: {fp}")),
                        ),
                );
            }
        }
        for d in devices {
            let id = d.id.clone();
            let seen = d.last_seen.map(seen_ago).unwrap_or_else(|| "never connected".into());
            let what = [d.model.as_str(), d.platform.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" · ");
            let line = format!("{}{}{} · {}", what, if what.is_empty() { "" } else { " · " }, d.scope.as_str(), seen);
            // unseen for 180 days: suggest removing it (05 §2)
            let unseen = d.last_seen.or(Some(d.paired_at)).is_some_and(|t| unix_now().saturating_sub(t) > 180 * 86_400);
            let line = if unseen { format!("{line} · Not seen for 6 months: remove it?") } else { line };
            group = group.child(super::settings::row(
                &d.name,
                Some(&line),
                Button::new(SharedString::from(format!("phones-remove-{}", d.id)))
                    .label("Remove")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(move |this, _, _, cx| this.forget_phone(&id, cx))),
            ));
        }
        group
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A transition kind as phones write it.
fn transition_name(k: lightair_model::TransitionKind) -> &'static str {
    use lightair_model::TransitionKind as T;
    match k {
        T::Cut => "cut",
        T::Fade => "fade",
        T::Move => "move",
        T::Slide => "slide",
        T::Wipe => "wipe",
        T::Dip => "dip",
        T::Stinger => "stinger",
    }
}

fn transition_kind(s: &str) -> Option<lightair_model::TransitionKind> {
    use lightair_model::TransitionKind as T;
    Some(match s.to_ascii_lowercase().as_str() {
        "cut" => T::Cut,
        "fade" => T::Fade,
        "move" => T::Move,
        "slide" => T::Slide,
        "wipe" => T::Wipe,
        "dip" => T::Dip,
        "stinger" => T::Stinger,
        _ => return None,
    })
}

fn seen_ago(unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    match now.saturating_sub(unix) {
        s if s < 120 => "seen just now".into(),
        s if s < 3600 => format!("seen {} minutes ago", s / 60),
        s if s < 86_400 => format!("seen {} hours ago", s / 3600),
        s => format!("seen {} days ago", s / 86_400),
    }
}

/// The QR code the phone scans, with where to find it in Lightair Live (`camera`: for a Phone
/// camera layer).
pub(super) fn pairing_code(link: &str, offer: &Offer, camera: bool) -> Div {
    let body = div().flex().items_center().gap_4().px_3().py_3().rounded(px(8.)).bg(tok::bg_raised());
    if link.is_empty() {
        return body.child(
            div().text_size(px(12.)).text_color(tok::text_secondary()).child("Making this computer's identity…"),
        );
    }
    let Ok(code) = qrcode::QrCode::new(link.as_bytes()) else {
        return body.child(div().text_size(px(12.)).child("The pairing code didn't fit in a QR code."));
    };
    let width = code.width();
    let dark: Vec<bool> = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    let side = 220.0f32;
    let quiet = 4.0;
    let module = side / (width as f32 + 2.0 * quiet);
    let minutes = offer.expires_at.saturating_sub(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    );
    body.child(
        div().flex_none().size(px(side)).rounded(px(6.)).bg(gpui_kit::white()).child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    for (i, d) in dark.iter().enumerate() {
                        if !*d {
                            continue;
                        }
                        let (x, y) = ((i % width) as f32 + quiet, (i / width) as f32 + quiet);
                        let origin = point(b.origin.x + px(x * module), b.origin.y + px(y * module));
                        window.paint_quad(fill(
                            Bounds::new(origin, size(px(module + 0.25), px(module + 0.25))),
                            gpui_kit::black(),
                        ));
                    }
                },
            )
            .size_full(),
        ),
    )
    .child(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .flex_1()
            .child(div().text_size(px(13.)).child("Scan this with Lightair Live"))
            .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child(if camera {
                "On the phone: Camera, then scan this. Lightair asks you to allow the phone before it sends anything."
            } else {
                "On the phone: Home › Pair a computer. Lightair asks you to allow the phone before it can do anything."
            }))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(tok::text_tertiary())
                    .child(format!("The code works once, for {} more seconds.", minutes)),
            )
            .child({
                // for a phone or tablet that can't scan this screen: send it the link
                let link = link.to_owned();
                div().child(
                    Button::new("phones-copy-link")
                        .label("Copy link")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))),
                )
            }),
    )
}

// ---------------------------------------------------------------- pictures and panic for phones

/// A preview kept for phones' pictures, let go of when they stop asking (it costs a render).
pub(crate) struct PhonePreview {
    preview: lightair_preview::Preview,
    scene: Option<lightair_model::Id>,
    /// The canvas whose program it shows (`None`: the primary one).
    canvas: Option<lightair_model::Id>,
    used: std::time::Instant,
}

/// What panic controls changed, to put back (lightair-live's docs/05 §4.2, Panic).
#[derive(Default)]
pub(crate) struct Panic {
    /// Tracks muted by "mute all", to unmute.
    muted: Vec<lightair_model::Id>,
    /// The scene before privacy, to restore.
    before_privacy: Option<lightair_model::Id>,
}

/// JPEG of an RGB picture, off the main thread.
fn jpeg(w: usize, h: usize, rgba: Vec<u8>, quality: u8) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("bad picture")?;
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// A microphone, by its source's kind.
fn is_microphone(kind: &str) -> bool {
    let k = kind.to_ascii_lowercase();
    k.contains("input_capture") || k.contains("microphone") || k.contains("mic")
}

impl StudioWindow {
    /// `preview.frame` (the program, for the preview stream) and `scene.thumbnails`: pictures
    /// from previews of their own, encoded as JPEG off the main thread.
    pub(super) async fn phone_picture(
        this: WeakEntity<Self>,
        call: lightair_remote::Call,
        cx: &mut AsyncWindowContext,
    ) -> lightair_remote::Reply {
        use lightair_remote::ApiError;
        let p = &call.params;
        match call.method.as_str() {
            "preview.frame" => {
                let width = p.get("max_width").and_then(Value::as_u64).unwrap_or(720).clamp(64, 1920) as usize;
                // another format's program (MR-05): a canvas id from state.get's canvases
                let canvas: Option<lightair_model::Id> =
                    p.get("canvas").filter(|c| c.is_string()).and_then(|c| serde_json::from_value(c.clone()).ok());
                let (w, h, rgba) = Self::phone_frame(this, true, None, canvas, width, cx).await?;
                let data = cx
                    .background_executor()
                    .spawn(async move { jpeg(w, h, rgba, 70) })
                    .await
                    .map_err(ApiError::failed)?;
                Ok(
                    json!({"mime": "image/jpeg", "width": w, "height": h, "data": base64::engine::general_purpose::STANDARD.encode(data)}),
                )
            }
            "scene.thumbnails" => {
                let width = p.get("width").and_then(Value::as_u64).unwrap_or(240).clamp(64, 640) as usize;
                let wanted: Option<Vec<String>> = p
                    .get("scenes")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect());
                let scenes: Vec<(lightair_model::Id, String)> = this
                    .update(cx, |view, cx| {
                        view.studio.read(cx).workspace().scenes.iter().map(|s| (s.id, s.name.clone())).collect()
                    })
                    .map_err(|_| ApiError::failed("Lightair's window is closed"))?;
                let mut out = vec![];
                for (id, name) in scenes {
                    let key = id.to_string();
                    if wanted.as_ref().is_some_and(|w| !w.iter().any(|x| *x == key || *x == name)) {
                        continue;
                    }
                    let Ok((w, h, rgba)) = Self::phone_frame(this.clone(), false, Some(id), None, width, cx).await else {
                        continue;
                    };
                    let Ok(data) = cx.background_executor().spawn(async move { jpeg(w, h, rgba, 65) }).await else {
                        continue;
                    };
                    out.push(json!({"scene": id, "mime": "image/jpeg", "width": w, "height": h, "data": base64::engine::general_purpose::STANDARD.encode(data)}));
                }
                Ok(json!({"thumbnails": out}))
            }
            _ => Err(ApiError::new(lightair_remote::ErrorKind::Unknown, "no such method")),
        }
    }

    /// The latest picture of the program (`program`; of `canvas`, else the primary canvas) or of
    /// `scene`, at most `width` wide.
    async fn phone_frame(
        this: WeakEntity<Self>,
        program: bool,
        scene: Option<lightair_model::Id>,
        canvas: Option<lightair_model::Id>,
        width: usize,
        cx: &mut AsyncWindowContext,
    ) -> Result<(usize, usize, Vec<u8>), lightair_remote::ApiError> {
        use lightair_remote::ApiError;
        let start = this
            .update(cx, |view, cx| {
                let studio = view.studio.read(cx);
                let w = studio.workspace().clone();
                let primary = w.primary_canvas().id;
                // (an unknown or the primary canvas: the primary one, as before)
                let other = canvas.filter(|c| *c != primary && w.canvas(*c).is_some());
                let canvas = primary;
                let engine = studio.engine().clone();
                let p = &mut view.remote.phones;
                let slot = if program { &mut p.program_preview } else { &mut p.scene_preview };
                if program && slot.as_ref().is_some_and(|s| s.canvas != other) {
                    *slot = None;
                }
                let made = slot.is_none();
                let shot = slot.get_or_insert_with(|| {
                    let preview = match other {
                        Some(c) => engine
                            .call(move |e| e.reconciler.canvas_ref(c).map(|p| p as usize))
                            .map(|p| lightair_preview::Preview::attach_canvas(p as *mut _))
                            .unwrap_or_else(lightair_preview::Preview::attach),
                        None => lightair_preview::Preview::attach(),
                    };
                    PhonePreview { preview, scene: None, canvas: other, used: std::time::Instant::now() }
                });
                if made {
                    return shot.preview.frames() + 3;
                }
                shot.used = std::time::Instant::now();
                // the program scene is the program's own picture
                let scene = scene.filter(|s| Some(*s) != w.program);
                if shot.scene != scene {
                    let source = scene.and_then(|s| {
                        engine.call(move |e| e.reconciler.scene_source_ref(s, canvas).map(|p| p as usize))
                    });
                    shot.preview.set_source(source.map(|p| p as *mut lightair_engine::sys::obs_source_t));
                    shot.scene = scene;
                    return shot.preview.frames() + 3;
                }
                shot.preview.frames().max(if program { 2 } else { 6 })
            })
            .map_err(|_| ApiError::failed("Lightair's window is closed"))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let frame = this
                .update(cx, |view, _| {
                    let p = &view.remote.phones;
                    let shot = if program { p.program_preview.as_ref() } else { p.scene_preview.as_ref() }?;
                    (shot.preview.frames() >= start).then(|| shot.preview.latest_frame()).flatten()
                })
                .ok()
                .flatten();
            if let Some(frame) = frame {
                let picture =
                    lightair_preview::frame_rgba(&frame, width).ok_or_else(|| ApiError::failed("no picture"))?;
                // let go of the previews once phones stop asking
                let weak = this.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(Duration::from_secs(10)).await;
                    let _ = weak.update(cx, |view, _| {
                        let p = &mut view.remote.phones;
                        for slot in [&mut p.program_preview, &mut p.scene_preview] {
                            if slot.as_ref().is_some_and(|s| s.used.elapsed() >= Duration::from_secs(9)) {
                                *slot = None;
                            }
                        }
                    });
                })
                .detach();
                return Ok(picture);
            }
            if std::time::Instant::now() > deadline {
                return Err(ApiError::failed("the picture didn't come in time"));
            }
            cx.background_executor().timer(Duration::from_millis(15)).await;
        }
    }

    /// Panic controls (lightair-live's docs/05 §4.2): `panic.mute_all`, `panic.privacy`,
    /// `panic.stop_all`, `panic.get`. Each is one undoable step named after the phone.
    pub(super) fn phone_call(
        &mut self,
        call: &lightair_remote::Call,
        cx: &mut Context<Self>,
    ) -> Option<lightair_remote::Reply> {
        use lightair_model::Command;
        use lightair_remote::{ApiError, Call};
        let on = call.params.get("on").and_then(Value::as_bool).unwrap_or(true);
        let who = call.client;
        let w = self.studio.read(cx).workspace().clone();
        let mics: Vec<lightair_model::Id> = w
            .tracks
            .iter()
            .filter(|t| w.sources.get(&t.source).is_some_and(|s| is_microphone(&s.kind)))
            .map(|t| t.id)
            .collect();
        let reply = match call.method.as_str() {
            "panic.get" => Ok(json!({})),
            "panic.mute_all" => {
                let cmds: Vec<Command> = if on {
                    let to_mute: Vec<_> =
                        mics.iter().copied().filter(|id| w.tracks.iter().any(|t| t.id == *id && !t.muted)).collect();
                    self.remote.phones.panic.muted = to_mute.clone();
                    to_mute.into_iter().map(|track| Command::SetTrackMuted { track, muted: true }).collect()
                } else {
                    std::mem::take(&mut self.remote.phones.panic.muted)
                        .into_iter()
                        .map(|track| Command::SetTrackMuted { track, muted: false })
                        .collect()
                };
                self.remote_exec(who, if on { "Mute all microphones" } else { "Unmute microphones" }, cmds, cx)
                    .map(|_| json!({}))
            }
            "panic.privacy" => {
                if on {
                    let named = call.params.get("scene").and_then(Value::as_str);
                    let privacy = w
                        .scenes
                        .iter()
                        .find(|s| named.is_some_and(|n| s.name == n || s.id.to_string() == n))
                        .or_else(|| {
                            w.scenes.iter().find(|s| {
                                let n = s.name.to_ascii_lowercase();
                                n.contains("be right back") || n == "brb" || n.contains("privacy")
                            })
                        })
                        .map(|s| s.id);
                    match privacy {
                        None => Err(ApiError::state("Make a \"Be right back\" scene to use privacy mode.")),
                        Some(scene) => {
                            self.remote.phones.panic.before_privacy = w.program.filter(|p| *p != scene);
                            let to_mute: Vec<_> = mics
                                .iter()
                                .copied()
                                .filter(|id| w.tracks.iter().any(|t| t.id == *id && !t.muted))
                                .collect();
                            self.remote.phones.panic.muted.extend(to_mute.iter().copied());
                            let mut cmds = vec![Command::SetProgram { scene: Some(scene) }];
                            cmds.extend(to_mute.into_iter().map(|track| Command::SetTrackMuted { track, muted: true }));
                            self.remote_exec(who, "Privacy", cmds, cx).map(|_| json!({}))
                        }
                    }
                } else {
                    let mut cmds: Vec<Command> = self
                        .remote
                        .phones
                        .panic
                        .before_privacy
                        .take()
                        .map(|s| Command::SetProgram { scene: Some(s) })
                        .into_iter()
                        .collect();
                    cmds.extend(
                        std::mem::take(&mut self.remote.phones.panic.muted)
                            .into_iter()
                            .map(|track| Command::SetTrackMuted { track, muted: false }),
                    );
                    self.remote_exec(who, "End privacy", cmds, cx).map(|_| json!({}))
                }
            }
            "panic.stop_all" => {
                for method in ["stream.stop", "record.stop", "vcam.stop"] {
                    // "not live" and the like: nothing to stop
                    let _ = self.remote_call(&Call { method: method.into(), params: json!({}), client: who, device: None }, cx);
                }
                Ok(json!({}))
            }
            _ => return self.phone_more(call, cx),
        };
        let p = &self.remote.phones.panic;
        let state = json!({"muted_all": !p.muted.is_empty() && p.before_privacy.is_none(), "privacy": p.before_privacy.is_some()});
        if let Some(server) = self.remote.running() {
            server.emit("panic.state", state.clone());
        }
        Some(reply.map(|r| if call.method == "panic.get" { state } else { r }))
    }
}

// ---------------------------------------------------------------- chat, activity, staging, deck

/// What counts as activity (lightair-live's docs/05 §4.2): money, memberships, raids, follows.
fn is_activity(kind: &lightair_live::event::Kind) -> bool {
    use lightair_live::event::Kind;
    matches!(kind, Kind::Paid | Kind::Membership | Kind::Raid { .. } | Kind::Follow)
}

fn deck_file() -> PathBuf {
    folder().join("deck.json")
}

/// The deck's pages and version (`deck.json` in Phones/).
fn read_deck() -> (Value, u64) {
    let v: Value = std::fs::read_to_string(deck_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    (v.get("pages").cloned().unwrap_or_else(|| json!([])), v.get("version").and_then(Value::as_u64).unwrap_or(0))
}

/// `stream.info.get`: each connected channel's title, category and viewers, from the platforms
/// (on the remote server's thread: it waits on the network). `platform` narrows it to one.
pub(crate) fn stream_info(hub: &lightair_live::Hub, params: &Value) -> lightair_remote::Reply {
    let only = params.get("platform").and_then(Value::as_str);
    let mut channels: Vec<_> = hub.status().into_keys().filter(|c| only.is_none_or(|p| p == c.platform)).collect();
    channels.sort_by(|a, b| (&a.platform, &a.channel).cmp(&(&b.platform, &b.channel)));
    let out: Vec<Value> = channels
        .into_iter()
        .map(|c| match lightair_live::stream_info::info(&c.platform, &c.channel) {
            Ok(i) => json!({
                "platform": c.platform, "channel": c.channel, "live": i.live, "title": i.title,
                "category": {"name": i.category}, "tags": [], "viewers": i.viewers, "started_ms": i.started_ms,
                "editable": lightair_live::stream_info::can_edit(&c.platform),
            }),
            Err(e) => json!({"platform": c.platform, "channel": c.channel, "error": e}),
        })
        .collect();
    Ok(json!(out))
}

/// `stream.categories {platform, query}`: categories to choose from before `stream.info.set`
/// (on the server's thread: it waits on the platform).
pub(crate) fn stream_categories(params: &Value) -> lightair_remote::Reply {
    use lightair_remote::ApiError;
    let platform = params.get("platform").and_then(Value::as_str).ok_or_else(|| ApiError::invalid("platform is required"))?;
    let query = params.get("query").and_then(Value::as_str).unwrap_or("");
    lightair_live::stream_info::categories(platform, query).map(|c| json!(c)).map_err(ApiError::state)
}

/// `chat.moderate {message, action, seconds?, reason?}`: deletes a chat message, or times out
/// or bans its sender, as the account signed in to that platform in Lightair (as the Chat panel
/// does), answering `{done: true}`. `message` is the message's `id` (or its platform id).
/// Timeouts last `seconds` (`duration_s` too), 600 by default. Unbanning isn't offered by any platform's path here yet: it says so.
pub(crate) fn chat_moderate(hub: &lightair_live::Hub, params: &Value) -> lightair_remote::Reply {
    use lightair_live::moderate::{Action, act, can};
    use lightair_remote::ApiError;
    let action = params.get("action").and_then(Value::as_str).ok_or_else(|| ApiError::invalid("action is required"))?;
    let e = find_event(hub, params.get("message")).ok_or_else(|| ApiError::not_found("no such chat message"))?;
    if !can(&e.platform) {
        return Err(ApiError::state(format!("moderating {} chat from Lightair isn't supported", e.platform)));
    }
    let reason = params.get("reason").and_then(Value::as_str).unwrap_or("");
    let user = e.user.as_ref();
    let (user_id, login) = (user.map_or("", |u| u.id.as_str()), user.map_or("", |u| u.login.as_str()));
    let native = e.native_id.as_deref().unwrap_or("");
    let a = match action {
        "delete" if !native.is_empty() => Action::Delete { message_id: native },
        "delete" => return Err(ApiError::state("this message can't be deleted (the platform gave it no id)")),
        "timeout" | "ban" if user_id.is_empty() => return Err(ApiError::state("this message has no sender to act on")),
        "timeout" => {
            let given = params.get("seconds").or_else(|| params.get("duration_s")).and_then(Value::as_u64);
            let seconds = given.unwrap_or(600).clamp(1, 1_209_600);
            Action::Timeout { user_id, login, seconds, reason }
        }
        "ban" => Action::Ban { user_id, login, reason },
        "unban" => return Err(ApiError::state("unban them on the platform: Lightair can't yet")),
        other => return Err(ApiError::invalid(format!("unknown action {other:?}"))),
    };
    act(&e.platform, &e.channel, a).map(|_| json!({"done": true})).map_err(ApiError::state)
}

/// A chat or activity event by its `id` (a number) or its platform id (a string).
pub(crate) fn find_event(hub: &lightair_live::Hub, id: Option<&Value>) -> Option<lightair_live::event::ChatEvent> {
    let id = id?;
    let (n, native) = (id.as_u64(), id.as_str());
    hub.history().into_iter().rev().find(|e| Some(e.id) == n || native.is_some_and(|s| e.native_id.as_deref() == Some(s) || s.parse().ok() == Some(e.id)))
}

impl StudioWindow {
    fn staging_json(&self, cx: &App) -> Value {
        let studio = self.studio.read(cx);
        let w = studio.workspace();
        let staged = studio.selected_scene.filter(|s| self.cue_scenes && Some(*s) != w.program);
        // the transition a take would use: the staged scene's own, else the workspace's
        let t = staged.and_then(|s| w.scene(s)).and_then(|s| s.transition).unwrap_or(w.transition);
        json!({"enabled": self.cue_scenes, "staged": staged, "program": w.program,
            "transition": {"kind": transition_name(t.kind), "duration_ms": t.duration_ms}})
    }

    /// New chat, activity and staging, to phones (lightair-live's docs/05 §4.2).
    fn tell_phones(&mut self, cx: &mut Context<Self>) {
        let hub = self.studio.read(cx).chat.clone();
        let staging = self.staging_json(cx);
        let mut out: Vec<(&str, Value)> = self.remote.phones.editor_sync.take_events();
        let p = &mut self.remote.phones;
        // the first look only notes where chat is: history comes from chat.history
        let events = if p.told_chat == u64::MAX { vec![] } else { hub.since(p.told_chat) };
        if p.told_chat == u64::MAX {
            p.told_chat = hub.history().last().map_or(0, |e| e.id);
        }
        for e in events {
            p.told_chat = p.told_chat.max(e.id);
            match &e.kind {
                lightair_live::event::Kind::Deletion { .. } => out.push(("chat.removed", json!(e))),
                k if is_activity(k) => {
                    out.push(("activity.item", json!(e)));
                    if matches!(k, lightair_live::event::Kind::Paid) {
                        out.push(("chat.message", json!(e)));
                    }
                }
                _ => out.push(("chat.message", json!(e))),
            }
        }
        if p.told_staging.as_ref() != Some(&staging) {
            p.told_staging = Some(staging.clone());
            out.push(("staging.state", staging));
        }
        // outputs' health once a second while live, and issues as they come and go
        if self.remote.phones.told_health.elapsed() >= Duration::from_secs(1) {
            self.remote.phones.told_health = std::time::Instant::now();
            if runtime().any_stream_running() {
                for d in self.outputs_json(cx)["destinations"].as_array().cloned().unwrap_or_default() {
                    if d["enabled"].as_bool() == Some(true) {
                        out.push(("output.health", d));
                    }
                }
            }
            let issues = self.issues_json(cx);
            let p = &mut self.remote.phones;
            for i in issues.iter().filter(|i| !p.told_issues.contains(i)) {
                out.push(("issue.raised", i.clone()));
            }
            for gone in p.told_issues.iter().filter(|o| !issues.iter().any(|i| i["id"] == o["id"])) {
                out.push(("issue.cleared", json!({"id": gone["id"]})));
            }
            p.told_issues = issues;
        }
        out.extend(self.scene_thumbnail_events(cx));
        if let Some(server) = self.remote.running() {
            for (event, data) in out {
                server.emit(event, data);
            }
        }
    }

    /// `scene.thumbnail {scene, version}` when a scene's picture may have changed: the scene on
    /// air every 5 s (its sources move), another when its objects change; one per scene per 5 s
    /// at most (05 §4.2). Phones fetch `scene.thumbnails` for the ones they show.
    fn scene_thumbnail_events(&mut self, cx: &mut Context<Self>) -> Vec<(&'static str, Value)> {
        let w = self.studio.read(cx).workspace().clone();
        let now = std::time::Instant::now();
        let p = &mut self.remote.phones;
        let mut out = vec![];
        p.thumbs.retain(|id, _| w.scene(*id).is_some());
        for scene in &w.scenes {
            let hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                serde_json::to_string(&**scene).unwrap_or_default().hash(&mut h);
                h.finish()
            };
            let on_air = w.program == Some(scene.id);
            let t = p.thumbs.entry(scene.id).or_insert((hash, now, 0));
            let due = now.duration_since(t.1) >= Duration::from_secs(5);
            if due && (on_air || t.0 != hash) {
                *t = (hash, now, t.2 + 1);
                out.push(("scene.thumbnail", json!({"scene": scene.id, "version": t.2})));
            }
        }
        out
    }

    /// Staging, activity and the deck for phones (lightair-live's docs/05 §4.2).
    pub(super) fn phone_more(&mut self, call: &lightair_remote::Call, cx: &mut Context<Self>) -> Option<lightair_remote::Reply> {
        use lightair_model::Command;
        use lightair_remote::ApiError;
        let p = &call.params;
        Some(match call.method.as_str() {
            "staging.get" => Ok(self.staging_json(cx)),
            "staging.enable" => {
                let on = p.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                if on != self.cue_scenes {
                    self.set_cue_scenes(on, cx);
                }
                Ok(self.staging_json(cx))
            }
            "staging.stage" => {
                let w = self.studio.read(cx).workspace().clone();
                let key = p.get("scene").and_then(Value::as_str).unwrap_or("");
                let Some(scene) = w.scenes.iter().find(|s| s.id.to_string() == key || s.name == key).map(|s| s.id) else {
                    return Some(Err(ApiError::not_found(format!("no scene {key}"))));
                };
                if !self.cue_scenes {
                    self.set_cue_scenes(true, cx);
                }
                self.studio.update(cx, |s, _| s.select_scene(scene));
                self.sync_studio_preview(cx);
                cx.notify();
                Ok(self.staging_json(cx))
            }
            "staging.take" => {
                let studio = self.studio.read(cx);
                let w = studio.workspace().clone();
                let Some(staged) = studio.selected_scene.filter(|s| self.cue_scenes && Some(*s) != w.program) else {
                    return Some(Err(ApiError::state("Nothing is staged.")));
                };
                let name = w.scene(staged).map(|s| s.name.clone()).unwrap_or_default();
                // a transition for this take only: {kind, duration_ms?}
                if let Some(t) = p.get("transition").filter(|t| t.is_object()) {
                    let Some(kind) = t["kind"].as_str().and_then(transition_kind) else {
                        return Some(Err(ApiError::invalid("transition.kind: cut, fade, move, slide, wipe, dip or stinger")));
                    };
                    let base = w.scene(staged).and_then(|s| s.transition).unwrap_or(w.transition);
                    let duration_ms = t["duration_ms"].as_u64().map_or(base.duration_ms, |d| d.min(10_000) as u32);
                    let settings = lightair_model::TransitionSettings { kind, duration_ms, direction: base.direction };
                    self.studio.read(cx).engine().call(move |s| s.reconciler.next_transition = Some(settings));
                }
                self.remote_exec(call.client, &format!("Take {name}"), vec![Command::SetProgram { scene: Some(staged) }], cx)
                    .map(|_| self.staging_json(cx))
            }
            "outputs.list" => Ok(self.outputs_json(cx)),
            "issues.list" => Ok(json!({"issues": self.issues_json(cx)})),
            "issues.fix" => (|| {
                let issue = p.get("issue").and_then(Value::as_str).unwrap_or("");
                let fix = p.get("fix").and_then(Value::as_str).unwrap_or("");
                // a destination's: try again, or stop it
                let id = issue
                    .strip_prefix("output-")
                    .and_then(|i| serde_json::from_value::<lightair_model::Id>(json!(i)).ok())
                    .ok_or_else(|| ApiError::state("This issue has no fix to run from a phone; fix it on the computer."))?;
                match fix {
                    "retry" => {
                        self.studio.update(cx, |s, cx| s.go_live_to(Some(id), cx));
                        Ok(json!({"done": true}))
                    }
                    "stop" => {
                        self.studio.read(cx).engine().post(move |s| s.outputs.stop_stream(id));
                        Ok(json!({"done": true}))
                    }
                    other => Err(ApiError::invalid(format!("no fix {other} for this issue"))),
                }
            })(),
            "activity.replay" => (|| {
                let studio = self.studio.read(cx);
                let event = find_event(&studio.chat, p.get("item")).ok_or_else(|| ApiError::not_found("no such activity item"))?;
                let widgets = studio.widgets.clone().ok_or_else(|| ApiError::state("Alerts aren't running."))?;
                widgets.replay(&event);
                Ok(json!({"replayed": true}))
            })(),
            "activity.history" => {
                let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
                let items: Vec<_> = self.studio.read(cx).chat.history().into_iter().filter(|e| is_activity(&e.kind)).collect();
                let start = items.len().saturating_sub(limit);
                Ok(json!({"items": &items[start..]}))
            }
            "deck.get" => {
                let (pages, version) = read_deck();
                Ok(json!({"pages": pages, "version": version}))
            }
            "deck.set" => {
                let (_, version) = read_deck();
                if p.get("base_version").and_then(Value::as_u64) != Some(version) {
                    return Some(Err(ApiError::state("The deck changed on another device. Reload it and try again.")));
                }
                let Some(pages) = p.get("pages").filter(|v| v.is_array()).cloned() else {
                    return Some(Err(ApiError::invalid("`pages` is needed")));
                };
                let next = version + 1;
                let _ = std::fs::create_dir_all(folder());
                if let Err(e) = std::fs::write(deck_file(), json!({"pages": pages, "version": next}).to_string()) {
                    return Some(Err(ApiError::failed(e.to_string())));
                }
                if let Some(server) = self.remote.running() {
                    server.emit("deck.changed", json!({"version": next}));
                }
                Ok(json!({"version": next}))
            }
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------- outputs and issues

use lightair_engine::outputs::{OutputKey, OutputState, runtime};

/// An output's state in phones' words (lightair-live's docs/05 §4.2, Outputs).
fn output_state(s: &OutputState) -> &'static str {
    match s {
        OutputState::Idle | OutputState::Stopping => "off",
        OutputState::Starting => "connecting",
        OutputState::Active { .. } => "live",
        OutputState::Reconnecting { .. } => "reconnecting",
        OutputState::Failed(_) => "error",
    }
}

impl StudioWindow {
    /// `outputs.list`: each destination's state, bitrate and dropped frames, and the recording.
    fn outputs_json(&mut self, cx: &App) -> Value {
        let studio = self.studio.read(cx);
        let w = studio.workspace().clone();
        let recording = studio.is_recording();
        let now = std::time::Instant::now();
        let sent = &mut self.remote.phones.sent;
        let destinations: Vec<Value> = w
            .outputs
            .destinations
            .iter()
            .map(|d| {
                let st = runtime().get(OutputKey::Stream(d.id));
                // bits since the last look, as the Stream panel works it out
                let kbps = match sent.get(&d.id) {
                    Some(&(_, at, kbps)) if now.duration_since(at) < Duration::from_millis(900) => kbps,
                    Some(&(bytes, at, _)) if st.bytes >= bytes => {
                        ((st.bytes - bytes) as f64 * 8.0 / 1000.0 / now.duration_since(at).as_secs_f64()) as u32
                    }
                    _ => 0,
                };
                if !sent.get(&d.id).is_some_and(|&(_, at, _)| now.duration_since(at) < Duration::from_millis(900)) {
                    sent.insert(d.id, (st.bytes, now, kbps));
                }
                let running = st.state.is_running();
                let canvas = d.canvas.and_then(|c| w.canvases.iter().find(|x| x.id == c)).unwrap_or(w.primary_canvas());
                let dropped = if st.total_frames > 0 { st.dropped_frames as f64 * 100.0 / st.total_frames as f64 } else { 0.0 };
                let mut v = json!({
                    "id": d.id, "platform": d.platform.as_deref().unwrap_or("custom"), "name": d.name, "format": canvas.name,
                    "enabled": d.enabled, "state": output_state(&st.state),
                    "bitrate_kbps": if running { kbps } else { 0 }, "dropped_pct": (dropped * 10.0).round() / 10.0,
                });
                if let OutputState::Failed(m) = &st.state {
                    v["message"] = m.clone().into();
                }
                v
            })
            .collect();
        let rec = runtime().recording();
        let elapsed = match rec.state {
            OutputState::Active { since } | OutputState::Reconnecting { since } => rec.recorded(since).as_millis() as u64,
            _ => 0,
        };
        json!({"destinations": destinations, "recording": {"active": recording, "paused": rec.paused, "elapsed_ms": elapsed}})
    }

    /// What's wrong now, worst first (lightair-live's docs/05 §4.2, Issues): outputs that failed
    /// or are reconnecting, dropped and lagged frames, and a disk running out.
    fn issues_json(&mut self, cx: &App) -> Vec<Value> {
        let issue = |id: String, severity: &str, title: String, detail: String| {
            json!({"id": id, "severity": severity, "title": title, "detail": detail, "fixes": []})
        };
        // what a phone may do about a destination's trouble (issues.fix)
        let fixes = |state: &str| match state {
            "error" => json!([{"id": "retry", "label": "Try again"}, {"id": "stop", "label": "Stop it"}]),
            _ => json!([{"id": "stop", "label": "Stop it"}]),
        };
        let mut out = vec![];
        let outputs = self.outputs_json(cx);
        for d in outputs["destinations"].as_array().into_iter().flatten() {
            let name = d["name"].as_str().unwrap_or("A destination");
            match d["state"].as_str() {
                Some("error") => out.push({
                    let mut i = issue(
                        format!("output-{}", d["id"].as_str().unwrap_or("")),
                        "error",
                        format!("{name} stopped"),
                        d["message"].as_str().unwrap_or("The stream to it ended with an error.").to_owned(),
                    );
                    i["fixes"] = fixes("error");
                    i
                }),
                Some("reconnecting") => out.push({
                    let mut i = issue(
                        format!("output-{}", d["id"].as_str().unwrap_or("")),
                        "warn",
                        format!("{name} is reconnecting"),
                        "The connection dropped; Lightair is trying again.".into(),
                    );
                    i["fixes"] = fixes("reconnecting");
                    i
                }),
                Some("live") if d["dropped_pct"].as_f64().unwrap_or(0.0) >= 1.0 => out.push(issue(
                    format!("dropped-{}", d["id"]),
                    "warn",
                    format!("{name} is dropping frames"),
                    format!("{}% of frames didn't make it: the upload can't keep up. A lower bitrate helps.", d["dropped_pct"]),
                )),
                _ => {}
            }
        }
        let studio = self.studio.read(cx);
        if studio.recent_lagged > 2 {
            out.push(issue(
                "lagged".into(),
                "warn",
                "Frames are being skipped".into(),
                "The computer can't draw every frame in time. Fewer sources or a lower frame rate helps.".into(),
            ));
        }
        if let Some((free, _)) = lightair_platform::files::space(&studio.recording_folder()) {
            let gb = free as f64 / (1u64 << 30) as f64;
            if gb < 5.0 {
                out.push(issue(
                    "disk".into(),
                    if gb < 1.0 { "error" } else { "warn" },
                    "The disk is nearly full".into(),
                    format!("{gb:.1} GB left where recordings are saved."),
                ));
            }
        }
        let rank = |v: &Value| match v["severity"].as_str() {
            Some("error") => 0,
            Some("warn") => 1,
            _ => 2,
        };
        out.sort_by_key(rank);
        out
    }
}
