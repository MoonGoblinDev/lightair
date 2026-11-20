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
const FEATURES: &[&str] =
    &["audio.levels", "preview.jpeg", "scene.thumbnails", "panic", "staging", "chat", "activity", "deck", "stream.info"];
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
    /// Previews for phones' pictures: the program's, and scenes' for thumbnails.
    program_preview: Option<PhonePreview>,
    scene_preview: Option<PhonePreview>,
    panic: Panic,
    /// The last chat event phones were told about, and the staging state they last saw.
    told_chat: u64,
    told_staging: Option<Value>,
    told_at: std::time::Instant,
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
            program_preview: None,
            scene_preview: None,
            panic: Panic::default(),
            told_chat: u64::MAX,
            told_staging: None,
            told_at: std::time::Instant::now(),
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
                let (w, h, rgba) = Self::phone_frame(this, true, None, width, cx).await?;
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
                    let Ok((w, h, rgba)) = Self::phone_frame(this.clone(), false, Some(id), width, cx).await else {
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

    /// The latest picture of the program (`program`) or of `scene`, at most `width` wide.
    async fn phone_frame(
        this: WeakEntity<Self>,
        program: bool,
        scene: Option<lightair_model::Id>,
        width: usize,
        cx: &mut AsyncWindowContext,
    ) -> Result<(usize, usize, Vec<u8>), lightair_remote::ApiError> {
        use lightair_remote::ApiError;
        let start = this
            .update(cx, |view, cx| {
                let studio = view.studio.read(cx);
                let w = studio.workspace().clone();
                let canvas = w.primary_canvas().id;
                let engine = studio.engine().clone();
                let p = &mut view.remote.phones;
                let slot = if program { &mut p.program_preview } else { &mut p.scene_preview };
                let shot = slot.get_or_insert_with(|| PhonePreview {
                    preview: lightair_preview::Preview::attach(),
                    scene: None,
                    used: std::time::Instant::now(),
                });
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
                    let _ = self.remote_call(&Call { method: method.into(), params: json!({}), client: who }, cx);
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

impl StudioWindow {
    fn staging_json(&self, cx: &App) -> Value {
        let studio = self.studio.read(cx);
        let w = studio.workspace();
        let staged = studio.selected_scene.filter(|s| self.cue_scenes && Some(*s) != w.program);
        json!({"enabled": self.cue_scenes, "staged": staged, "program": w.program})
    }

    /// New chat, activity and staging, to phones (lightair-live's docs/05 §4.2).
    fn tell_phones(&mut self, cx: &mut Context<Self>) {
        let hub = self.studio.read(cx).chat.clone();
        let staging = self.staging_json(cx);
        let mut out: Vec<(&str, Value)> = vec![];
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
        if let Some(server) = self.remote.running() {
            for (event, data) in out {
                server.emit(event, data);
            }
        }
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
                self.remote_exec(call.client, &format!("Take {name}"), vec![Command::SetProgram { scene: Some(staged) }], cx)
                    .map(|_| self.staging_json(cx))
            }
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
