//! Releases and updates: the update signing key, the signed manifest Lightair's updater reads
//! (`latest.json`, attached to each GitHub Release), and the GitHub secrets the release workflow
//! needs (.github/workflows/release.yml, docs/releasing.md).

use std::path::Path;
use std::process::Command;

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use sha2::Digest;

const SERVICE: &str = "Lightair update signing key";
const REPO: &str = "MoonGoblinDev/lightair";
/// The app's copy of the public key (filled by `update-key`).
const UPDATER: &str = "crates/lightair-app/src/updater.rs";

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> anyhow::Result<SigningKey> {
    let t = text.trim();
    anyhow::ensure!(t.len() == 64, "the update key is 64 hex digits");
    let bytes: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&t[i * 2..i * 2 + 2], 16)).collect::<Result<_, _>>()?;
    Ok(SigningKey::from_bytes(&bytes.try_into().map_err(|_| anyhow::anyhow!("bad key"))?))
}

/// The private key: `LIGHTAIR_UPDATE_KEY` (CI's secret), else this Mac's Keychain.
fn signing_key() -> anyhow::Result<SigningKey> {
    if let Ok(k) = std::env::var("LIGHTAIR_UPDATE_KEY")
        && !k.trim().is_empty()
    {
        return from_hex(&k);
    }
    let out = Command::new("/usr/bin/security").args(["find-generic-password", "-w", "-s", SERVICE, "-a", "lightair"]).output()?;
    anyhow::ensure!(out.status.success(), "no update key: set LIGHTAIR_UPDATE_KEY or run `cargo xtask update-key`");
    from_hex(&String::from_utf8(out.stdout)?)
}

/// Makes the update signing key once (kept in the Keychain) and writes its public half into
/// the app's updater.
pub fn key() -> anyhow::Result<()> {
    let k = match signing_key() {
        Ok(k) => {
            println!("an update key exists already");
            k
        }
        Err(_) => {
            let mut seed = [0u8; 32];
            std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut seed)?;
            let status = Command::new("/usr/bin/security").args(["add-generic-password", "-s", SERVICE, "-a", "lightair", "-w", &hex(&seed)]).status()?;
            anyhow::ensure!(status.success(), "couldn't keep the key in the Keychain");
            println!("kept the private key in the Keychain ({SERVICE})");
            SigningKey::from_bytes(&seed)
        }
    };
    let public = hex(&k.verifying_key().to_bytes());
    let path = crate::root().join(UPDATER);
    let text = std::fs::read_to_string(&path)?;
    let start = text.find("pub const PUBLIC_KEY: &str = \"").ok_or_else(|| anyhow::anyhow!("no PUBLIC_KEY in {UPDATER}"))?;
    let from = start + "pub const PUBLIC_KEY: &str = \"".len();
    let end = from + text[from..].find('"').unwrap_or(0);
    std::fs::write(&path, format!("{}{public}{}", &text[..from], &text[end..]))?;
    println!("public key {public}, written into {UPDATER}");
    Ok(())
}

/// One downloadable file of a release.
fn asset(dir: &Path, name: &str, tag: &str) -> anyhow::Result<serde_json::Value> {
    let bytes = std::fs::read(dir.join(name))?;
    Ok(serde_json::json!({
        "url": format!("https://github.com/{REPO}/releases/download/{tag}/{name}"),
        "sha256": hex(&sha2::Sha256::digest(&bytes)),
        "size": bytes.len(),
    }))
}

/// `latest.json` (and `.sig`) for `version` from the files in `dir`: the macOS disk image and the
/// Windows zip, whichever are there.
pub fn manifest(version: &str, dir: &str, notes: Option<&str>) -> anyhow::Result<()> {
    let dir = Path::new(dir);
    let tag = format!("v{version}");
    let mut assets = serde_json::Map::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == format!("Lightair-{version}.dmg") {
            assets.insert("macos".into(), asset(dir, &name, &tag)?);
        } else if name == format!("Lightair-{version}-windows-x64.zip") {
            assets.insert("windows".into(), asset(dir, &name, &tag)?);
        }
    }
    anyhow::ensure!(!assets.is_empty(), "no Lightair-{version}.dmg or Lightair-{version}-windows-x64.zip in {}", dir.display());
    let doc = serde_json::json!({
        "version": version,
        "notes": notes.unwrap_or(""),
        "page": format!("https://github.com/{REPO}/releases/tag/{tag}"),
        "assets": assets,
    });
    let bytes = serde_json::to_vec_pretty(&doc)?;
    let sig = base64::engine::general_purpose::STANDARD.encode(signing_key()?.sign(&bytes).to_bytes());
    std::fs::write(dir.join("latest.json"), &bytes)?;
    std::fs::write(dir.join("latest.json.sig"), sig)?;
    println!("wrote latest.json ({}) and its signature", assets.keys().cloned().collect::<Vec<_>>().join(", "));
    Ok(())
}

/// Puts this Mac's release secrets into the repository's GitHub secrets (through `gh`): the
/// update key, and release.env's app ids. Says which ones only you can add.
pub fn ci_secrets() -> anyhow::Result<()> {
    // GitHub's API times out now and then: each secret is tried a few times
    let set = |name: &str, value: &str| -> anyhow::Result<()> {
        for attempt in 1..=4 {
            let mut child = Command::new("gh").args(["secret", "set", name, "--repo", REPO]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn()?;
            std::io::Write::write_all(child.stdin.as_mut().unwrap(), value.as_bytes())?;
            drop(child.stdin.take());
            if child.wait()?.success() {
                println!("set {name}");
                return Ok(());
            }
            if attempt < 4 {
                eprintln!("trying {name} again…");
                std::thread::sleep(std::time::Duration::from_secs(3 * attempt));
            }
        }
        anyhow::bail!("gh couldn't set {name} (GitHub didn't answer): run `cargo xtask ci-secrets` again")
    };
    set("LIGHTAIR_UPDATE_KEY", &hex(&signing_key()?.to_bytes()))?;
    if let Ok(text) = std::fs::read_to_string(crate::root().join("release.env")) {
        for line in text.lines().map(str::trim).filter(|l| l.starts_with("LIGHTAIR_") && !l.starts_with("LIGHTAIR_NOTARY")) {
            if let Some((k, v)) = line.split_once('=')
                && !v.trim().is_empty()
            {
                set(k.trim(), v.trim().trim_matches('"'))?;
            }
        }
    }
    println!(
        "\nStill to add yourself (Settings › Secrets and variables › Actions on GitHub, or `gh secret set NAME`):\n  \
         MACOS_CERTIFICATE_P12       your Developer ID Application certificate exported as .p12, base64 (`base64 -i cert.p12 | pbcopy`)\n  \
         MACOS_CERTIFICATE_PASSWORD  the password you gave that .p12\n  \
         APPLE_ID                    your Apple ID email\n  \
         APPLE_TEAM_ID               your team ID\n  \
         APPLE_APP_PASSWORD          an app-specific password (appleid.apple.com)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signature, Verifier};

    #[test]
    fn a_manifest_signed_and_read_back() {
        let dir = std::env::temp_dir().join(format!("lightair-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Lightair-9.9.9.dmg"), b"a disk image").unwrap();
        std::fs::write(dir.join("Lightair-9.9.9-windows-x64.zip"), b"a zip").unwrap();
        let seed = [9u8; 32];
        // SAFETY: only this test reads it
        unsafe { std::env::set_var("LIGHTAIR_UPDATE_KEY", super::hex(&seed)) };
        super::manifest("9.9.9", dir.to_str().unwrap(), Some("Faster, calmer")).unwrap();
        let bytes = std::fs::read(dir.join("latest.json")).unwrap();
        let sig = std::fs::read_to_string(dir.join("latest.json.sig")).unwrap();
        let sig = Signature::from_slice(&base64::Engine::decode(&base64::engine::general_purpose::STANDARD, sig).unwrap()).unwrap();
        ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().verify(&bytes, &sig).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["notes"], "Faster, calmer");
        assert_eq!(v["assets"]["macos"]["url"], "https://github.com/MoonGoblinDev/lightair/releases/download/v9.9.9/Lightair-9.9.9.dmg");
        assert_eq!(v["assets"]["windows"]["sha256"].as_str().unwrap().len(), 64);
        let _ = std::fs::remove_dir_all(dir);
    }
}
