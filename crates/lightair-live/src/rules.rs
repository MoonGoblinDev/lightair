//! Reading rules as data (docs/plans/live-integrations.md, 2.10): the selectors and addresses
//! readers and senders depend on, and a switch per platform, updated without an app release.
//! Sites change (YouTube's chat in October 2025, TikTok's signing several times); a fix reaches
//! people in a day by a new rules file.
//!
//! The rules in use are the built-in ones ([`text`]'s defaults), overridden by the newest
//! signed file Lightair has: one it fetched from [`address`] and kept (verified again when
//! read), never one it can't verify with [`PUBLIC_KEY`]. A file is
//! `{"version": n, "off": ["platform", …], "values": {"key": "value", …}}`, signed by
//! `cargo xtask rules-sign` (its key in the Keychain of whoever publishes), the signature
//! (base64 Ed25519 over the file's exact bytes) next to it as `<file>.sig`.

use std::path::PathBuf;
use std::sync::OnceLock;

use base64::Engine;
use parking_lot::RwLock;
use serde_json::Value;

use crate::net;

/// The rules' public key (Ed25519, 32 bytes as hex): `cargo xtask rules-key` made the pair (the
/// private half is in the publisher's Keychain).
pub const PUBLIC_KEY: Option<&str> = Some("a8bdd842fef6c26049fe19d7e7eef2d11f2dbb3b9a70df24493f22a1b98b9eb7");

/// Where the newest rules are published (`LIGHTAIR_RULES_URL` overrides): `rules/rules.json` in
/// Lightair's public repository, with its `.sig` next to it.
const ADDRESS: Option<&str> = Some("https://raw.githubusercontent.com/MoonGoblinDev/lightair/main/rules/rules.json");

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// The newest verified rules (None: built-in only).
static RULES: RwLock<Option<Value>> = RwLock::new(None);

pub fn address() -> Option<String> {
    std::env::var("LIGHTAIR_RULES_URL").ok().filter(|u| !u.is_empty()).or_else(|| ADDRESS.map(String::from))
}

fn key() -> Option<[u8; 32]> {
    let hex = std::env::var("LIGHTAIR_RULES_KEY").ok().filter(|k| !k.is_empty()).or_else(|| PUBLIC_KEY.map(String::from))?;
    let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
    bytes.try_into().ok()
}

/// Whether `signature` (base64) is `key`'s over `bytes`.
pub fn verify(key: &[u8; 32], bytes: &[u8], signature: &str) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(sig) = base64::engine::general_purpose::STANDARD.decode(signature.trim()) else { return false };
    let Ok(sig) = Signature::from_slice(&sig) else { return false };
    VerifyingKey::from_bytes(key).is_ok_and(|k| k.verify(bytes, &sig).is_ok())
}

/// The rules in `bytes` if `signature` checks out and they parse.
fn accept(bytes: &[u8], signature: &str) -> Option<Value> {
    let key = key()?;
    if !verify(&key, bytes, signature) {
        tracing::warn!("rules: a file whose signature doesn't check out, ignored");
        return None;
    }
    serde_json::from_slice::<Value>(bytes).ok().filter(|v| v["version"].is_u64())
}

fn version(v: &Option<Value>) -> u64 {
    v.as_ref().and_then(|v| v["version"].as_u64()).unwrap_or(0)
}

/// Where rules are kept (the data folder); reads the kept file, then fetches newer ones in the
/// background, now and every six hours.
pub fn set_dir(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("rules.json");
    if let (Ok(bytes), Ok(sig)) = (std::fs::read(&file), std::fs::read_to_string(dir.join("rules.json.sig"))) {
        *RULES.write() = accept(&bytes, &sig);
    }
    if DIR.set(dir).is_err() || address().is_none() || key().is_none() {
        return;
    }
    let _ = std::thread::Builder::new().name("lightair-rules".into()).spawn(|| {
        loop {
            if let Err(e) = refresh() {
                tracing::debug!(%e, "rules: no update");
            }
            std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
        }
    });
}

/// Fetches the published rules; keeps and uses them when they're signed and newer: whether they
/// were.
pub fn refresh() -> Result<bool, String> {
    let url = address().ok_or("no rules address")?;
    let agent = net::agent();
    let mut res = agent.get(&url).call().map_err(|e| e.to_string())?;
    let bytes = res.body_mut().read_to_vec().map_err(|e| e.to_string())?;
    let sig = net::get(&agent, &format!("{url}.sig"), &[])?;
    let rules = accept(&bytes, &sig).ok_or("the published rules aren't signed by Lightair's key")?;
    if rules["version"].as_u64().unwrap_or(0) <= version(&RULES.read()) {
        return Ok(false);
    }
    if let Some(dir) = DIR.get() {
        let _ = std::fs::write(dir.join("rules.json"), &bytes);
        let _ = std::fs::write(dir.join("rules.json.sig"), sig.trim());
    }
    tracing::info!(version = rules["version"].as_u64(), "rules: updated");
    *RULES.write() = Some(rules);
    Ok(true)
}

/// The rule `key` (like "tiktok.chat_container"), or `default`, the built-in one.
pub fn text(key: &str, default: &str) -> String {
    RULES.read().as_ref().and_then(|r| r["values"][key].as_str().map(String::from)).unwrap_or_else(|| default.to_string())
}

/// Whether `platform`'s reader and sender are on (the rules can turn one off while it's broken).
pub fn enabled(platform: &str) -> bool {
    !RULES.read().as_ref().is_some_and(|r| r["off"].as_array().is_some_and(|a| a.iter().any(|p| p.as_str() == Some(platform))))
}

/// The version in use (0: built-in).
pub fn current() -> u64 {
    version(&RULES.read())
}

#[cfg(test)]
pub(crate) fn use_for_test(rules: Option<Value>) {
    *RULES.write() = rules;
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn signed_rules_are_taken_and_others_are_not() {
        let signer = SigningKey::from_bytes(&[7u8; 32]);
        let public: String = signer.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let file = br#"{"version": 2, "off": ["linkedin"], "values": {"tiktok.chat_container": ".chat"}}"#;
        let sig = base64::engine::general_purpose::STANDARD.encode(signer.sign(file).to_bytes());
        let key: [u8; 32] = signer.verifying_key().to_bytes();
        assert!(super::verify(&key, file, &sig));
        assert!(!super::verify(&key, br#"{"version": 3}"#, &sig));
        assert!(!super::verify(&key, file, "bm90IGEgc2lnbmF0dXJl"));
        // SAFETY: tests in this crate don't read LIGHTAIR_RULES_KEY elsewhere
        unsafe { std::env::set_var("LIGHTAIR_RULES_KEY", &public) };
        let rules = super::accept(file, &sig).unwrap();
        assert!(super::accept(br#"{"version": 9}"#, &sig).is_none());
        super::use_for_test(Some(rules));
        assert_eq!(super::text("tiktok.chat_container", "[data-e2e]"), ".chat");
        assert_eq!(super::text("kick.input", "#in"), "#in");
        assert!(!super::enabled("linkedin") && super::enabled("twitch"));
        assert_eq!(super::current(), 2);
        super::use_for_test(None);
        assert!(super::enabled("linkedin"));
    }
}
