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
const FEATURES: &[&str] = &["audio.levels"];
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
    pub error: Option<String>,
    meters: Option<Task<()>>,
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
        self.listener = None;
        self.mdns = None;
    }
}

impl StudioWindow {
    pub(super) fn new_phones(window: &mut Window, cx: &mut Context<Self>) -> Phones {
        // asking the person happens on the main thread, as a native alert; the phone's connection
        // waits for the answer (a minute at most)
        let (asks, mut queue) = futures_channel::mpsc::unbounded::<(DeviceRequest, std::sync::mpsc::Sender<bool>)>();
        cx.spawn_in(window, async move |_, cx| {
            use futures_util::StreamExt;
            while let Some((request, reply)) = queue.next().await {
                let scope = match request.scope {
                    Scope::Watch => "watch your show (it can't change anything)",
                    _ => "control Lightair",
                };
                let message = format!("Allow {} to {scope}?", request.name);
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
        Phones {
            pairing: Pairing::new(devices, confirm, save),
            desktop: desktop_info(),
            identity: None,
            reading_identity: false,
            listener: None,
            mdns: None,
            offer: None,
            error: None,
            meters: None,
        }
    }

    /// Runs the phones' port while the remote API is on and a phone is paired or the pairing code
    /// shows; stops it otherwise.
    pub(super) fn sync_phones(&mut self, cx: &mut Context<Self>) {
        let running = self.remote.running().is_some();
        let p = &mut self.remote.phones;
        if p.offer.as_ref().is_some_and(|(o, _)| !p.pairing.open() && o.expires_at > 0) {
            // the code expired
            p.offer = None;
        }
        let wanted = running && (!p.pairing.devices().is_empty() || p.offer.is_some());
        if !wanted {
            p.listener = None;
            p.mdns = None;
            p.meters = None;
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
                features: FEATURES.iter().map(|f| f.to_string()).collect(),
            };
            let started = self.remote.running().map(|s| s.start_secure(config, self.remote.phones.pairing.clone()));
            let p = &mut self.remote.phones;
            match started {
                Some(Ok(l)) => {
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
    }

    /// `audio.levels` to the phones that asked (lightair-live's docs/05 §4.2): dBFS per channel,
    /// silence as null.
    fn emit_levels(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.remote.running() else { return };
        if !server.clients().iter().any(|c| c.protocol == "Lightair phone") {
            return;
        }
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
            group = group.child(pairing_code(link, offer));
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

fn seen_ago(unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    match now.saturating_sub(unix) {
        s if s < 120 => "seen just now".into(),
        s if s < 3600 => format!("seen {} minutes ago", s / 60),
        s if s < 86_400 => format!("seen {} hours ago", s / 3600),
        s => format!("seen {} days ago", s / 86_400),
    }
}

/// The QR code the phone scans, with where to find it in Lightair Live.
fn pairing_code(link: &str, offer: &Offer) -> Div {
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
            .child(div().text_size(px(12.)).text_color(tok::text_secondary()).child(
                "On the phone: Home › Pair a computer. Lightair asks you to allow the phone before it can do anything.",
            ))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(tok::text_tertiary())
                    .child(format!("The code works once, for {} more seconds.", minutes)),
            ),
    )
}
