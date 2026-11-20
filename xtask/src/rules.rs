//! Publishing reading rules (lightair-core/crates/lightair-live/src/rules.rs): `rules-key` makes the signing
//! pair once (the private half in this Mac's Keychain, the public half to paste into rules.rs);
//! `rules-sign <file>` writes `<file>.sig` for publishing next to the file.

use std::process::Command;

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};

const SERVICE: &str = "Lightair rules signing key";

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn signing_key() -> anyhow::Result<SigningKey> {
    let out = Command::new("/usr/bin/security").args(["find-generic-password", "-w", "-s", SERVICE, "-a", "lightair"]).output()?;
    anyhow::ensure!(out.status.success(), "no signing key in the Keychain: run `cargo xtask rules-key` first");
    let text = String::from_utf8(out.stdout)?;
    let bytes: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&text.trim()[i * 2..i * 2 + 2], 16)).collect::<Result<_, _>>()?;
    Ok(SigningKey::from_bytes(&bytes.try_into().map_err(|_| anyhow::anyhow!("bad key"))?))
}

pub fn key() -> anyhow::Result<()> {
    if let Ok(k) = signing_key() {
        println!("a signing key exists already; its public key:\n{}", hex(&k.verifying_key().to_bytes()));
        return Ok(());
    }
    let mut seed = [0u8; 32];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut seed)?;
    let k = SigningKey::from_bytes(&seed);
    let status = Command::new("/usr/bin/security").args(["add-generic-password", "-s", SERVICE, "-a", "lightair", "-w", &hex(&seed)]).status()?;
    anyhow::ensure!(status.success(), "couldn't keep the key in the Keychain");
    println!("kept the private key in the Keychain ({SERVICE}). Public key, for PUBLIC_KEY in lightair-core/crates/lightair-live/src/rules.rs:\n{}", hex(&k.verifying_key().to_bytes()));
    Ok(())
}

pub fn sign(file: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rules: serde_json::Value = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(rules["version"].is_u64(), "a rules file needs a whole-number \"version\" (higher than the published one)");
    let sig = base64::engine::general_purpose::STANDARD.encode(signing_key()?.sign(&bytes).to_bytes());
    std::fs::write(format!("{file}.sig"), &sig)?;
    println!("wrote {file}.sig; publish both, at the rules address");
    Ok(())
}
