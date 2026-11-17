//! Settings › Connections: tip services connected with the streamer's own token from each
//! service's dashboard (no app approval). Tokens go to the Keychain, read back off the main
//! thread at start (a Keychain prompt mustn't freeze the window).

use gpui_kit::*;

use super::StudioWindow;

/// (service, name, where its token is).
pub(super) const SERVICES: &[(&str, &str, &str)] = &[
    ("streamlabs", "Streamlabs", "Its Socket API token: streamlabs.com › Settings › API Settings › API Tokens"),
    ("streamelements", "StreamElements", "Its JWT token: streamelements.com › Account › Channels › Show secrets"),
];

impl StudioWindow {
    /// Connects the services that have a token saved (at start).
    pub(super) fn load_connections(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { SERVICES.iter().map(|(s, _, _)| *s).chain(["kofi", "pulsoid"]).filter_map(|s| crate::secrets::connection_token(s).map(|t| (s, t))).collect::<Vec<_>>() })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (service, token) in found {
                    if service == "pulsoid" {
                        this.start_heart_rate(token, cx);
                        continue;
                    }
                    lightair_live::donations::set_token(service, &token);
                    this.studio.update(cx, |s, _| s.set_service(service, true));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves the token on the clipboard for `service` and connects it.
    pub(super) fn paste_connection(&mut self, service: &'static str, cx: &mut Context<Self>) {
        let Some(token) = cx.read_from_clipboard().and_then(|c| c.text()).map(|t| t.trim().to_string()).filter(|t| t.len() > 10 && !t.contains(char::is_whitespace)) else {
            self.studio.update(cx, |s, cx| {
                s.issue = Some("Copy the token first (the clipboard doesn't hold one).".into());
                cx.notify();
            });
            return;
        };
        lightair_live::donations::set_token(service, &token);
        self.studio.update(cx, |s, _| {
            // a new token: the reader starts again with it
            s.set_service(service, false);
            s.set_service(service, true);
        });
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = crate::secrets::set_connection_token(service, &token) {
                    tracing::warn!(%e, "couldn't keep the token");
                }
            })
            .detach();
        cx.notify();
    }

    pub(super) fn disconnect(&mut self, service: &'static str, cx: &mut Context<Self>) {
        lightair_live::donations::set_token(service, "");
        self.studio.update(cx, |s, _| s.set_service(service, false));
        cx.background_executor().spawn(async move { crate::secrets::set_connection_token(service, "") }).detach();
        cx.notify();
    }
}

impl StudioWindow {
    /// Ko-fi: its stream alerts link from the clipboard (Ko-fi › Stream Alerts › Copy the link),
    /// kept in the Keychain; Lightair reads tips off that page, on this computer.
    pub(super) fn paste_kofi_link(&mut self, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        let Some(link) = lightair_live::donations::kofi_link(&text) else {
            self.studio.update(cx, |s, cx| {
                s.issue = Some("Copy Ko-fi's stream alerts link first (ko-fi.com › Stream Alerts › Copy the link: it starts https://ko-fi.com/streamalerts/overlay/).".into());
                cx.notify();
            });
            return;
        };
        lightair_live::donations::set_token("kofi", &link);
        self.studio.update(cx, |s, _| {
            s.set_service("kofi", false);
            s.set_service("kofi", true);
        });
        cx.background_executor().spawn(async move { crate::secrets::set_connection_token("kofi", &link) }).detach();
        cx.notify();
    }
}

impl StudioWindow {
    /// Heart rate from Pulsoid: to widgets ("heart_rate") and automations' `{{ vars.heart_rate }}`.
    pub(super) fn start_heart_rate(&mut self, token: String, cx: &mut Context<Self>) {
        if let Some(old) = self.heart_rate.take() {
            old.store(true, std::sync::atomic::Ordering::Release);
        }
        if token.is_empty() {
            cx.notify();
            return;
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (widgets, automations) = (self.studio.read(cx).widgets.clone(), self.automations.clone());
        let last = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        lightair_live::integrations::pulsoid(
            token,
            move |bpm| {
                if let Some(w) = &widgets {
                    w.set_data("heart_rate", serde_json::json!(bpm));
                }
                // the variable (and its file) only when it changes by more than a little
                if last.swap(bpm, std::sync::atomic::Ordering::Relaxed).abs_diff(bpm) >= 2 {
                    automations.set_var("heart_rate", serde_json::json!(bpm));
                }
            },
            stop.clone(),
        );
        self.heart_rate = Some(stop);
        cx.notify();
    }

    pub(super) fn paste_pulsoid(&mut self, cx: &mut Context<Self>) {
        let Some(token) = cx.read_from_clipboard().and_then(|c| c.text()).map(|t| t.trim().to_string()).filter(|t| t.len() > 10 && !t.contains(char::is_whitespace)) else {
            self.studio.update(cx, |s, cx| {
                s.issue = Some("Copy the token first (the clipboard doesn't hold one).".into());
                cx.notify();
            });
            return;
        };
        self.start_heart_rate(token.clone(), cx);
        cx.background_executor().spawn(async move { crate::secrets::set_connection_token("pulsoid", &token) }).detach();
    }

    pub(super) fn forget_pulsoid(&mut self, cx: &mut Context<Self>) {
        self.start_heart_rate(String::new(), cx);
        cx.background_executor().spawn(async move { crate::secrets::set_connection_token("pulsoid", "") }).detach();
    }

    /// Finds the Hue bridge (or uses `address`) and waits up to 40 seconds for its button.
    pub(super) fn pair_hue(&mut self, address: Option<String>, cx: &mut Context<Self>) {
        self.hue_note = Some("Looking for the bridge…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let note = |text: String, cx: &mut gpui::AsyncApp| {
                let _ = this.update(cx, |this, cx| {
                    this.hue_note = Some(text);
                    cx.notify();
                });
            };
            let address = match address {
                Some(a) => a,
                None => match cx.background_executor().spawn(async { lightair_live::integrations::hue_find() }).await {
                    Ok(list) if !list.is_empty() => list[0].clone(),
                    Ok(_) => return note("No bridge found on this network. Copy its address (Hue app › Settings › Bridges › ⓘ), then Paste address.".into(), cx),
                    Err(e) => return note(format!("Couldn't look for the bridge ({e}). Copy its address from the Hue app, then Paste address."), cx),
                },
            };
            note(format!("Press the button on the bridge at {address}…"), cx);
            for _ in 0..40 {
                let a = address.clone();
                match cx.background_executor().spawn(async move { lightair_live::integrations::hue_pair(&a) }).await {
                    Ok(true) => {
                        let _ = this.update(cx, |this, cx| {
                            this.hue_note = None;
                            cx.notify();
                        });
                        return;
                    }
                    Ok(false) => cx.background_executor().timer(std::time::Duration::from_secs(1)).await,
                    Err(e) => return note(e, cx),
                }
            }
            note("The bridge's button wasn't pressed in time: choose Pair again, then press it.".into(), cx);
        })
        .detach();
    }

    pub(super) fn paste_hue_address(&mut self, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|c| c.text()).map(|t| t.trim().to_string()).unwrap_or_default();
        if text.is_empty() || !text.chars().all(|c| c.is_ascii_alphanumeric() || ".:-[]".contains(c)) {
            self.hue_note = Some("Copy the bridge's address first (like 192.168.1.10).".into());
            cx.notify();
            return;
        }
        self.pair_hue(Some(text), cx);
    }

    pub(super) fn forget_hue(&mut self, cx: &mut Context<Self>) {
        lightair_live::integrations::hue_forget();
        self.hue_note = None;
        cx.notify();
    }

    /// Official sign-in (OAuth) to `provider`, in the person's browser.
    pub(super) fn official_sign_in(&mut self, provider: &'static str, cx: &mut Context<Self>) {
        self.oauth_notes.insert(provider.into(), "Opening your browser…".into());
        cx.notify();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        type Outcome = std::sync::Arc<std::sync::Mutex<Option<Result<String, String>>>>;
        let done: Outcome = Default::default();
        let out = done.clone();
        std::thread::spawn(move || {
            let tx = std::sync::Mutex::new(tx);
            let r = lightair_live::oauth::sign_in(provider, Box::new(move |m| {
                let _ = tx.lock().map(|t| t.send(m.to_string()));
            }));
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        });
        // what the sign-in asks for (Twitch's code) while it waits, then how it ended
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
            while let Ok(m) = rx.try_recv() {
                let _ = this.update(cx, |this, cx| {
                    this.oauth_notes.insert(provider.into(), m);
                    cx.notify();
                });
            }
            let finished = done.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(result) = finished {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(_) => this.oauth_notes.remove(provider),
                        Err(e) => this.oauth_notes.insert(provider.into(), e),
                    };
                    cx.notify();
                });
                break;
            }
        })
        .detach();
    }

    pub(super) fn official_sign_out(&mut self, provider: &str, cx: &mut Context<Self>) {
        lightair_live::oauth::forget(provider);
        self.oauth_notes.remove(provider);
        cx.notify();
    }
}
