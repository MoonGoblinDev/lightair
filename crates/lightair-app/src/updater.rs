//! Updates: the newest release's manifest (`latest.json` on GitHub Releases, made and signed by
//! `cargo xtask update-manifest` in the release workflow), checked at launch and every six
//! hours; installing it on request (Settings › About).
//!
//! Nothing is trusted that the update key didn't sign: the manifest's signature (Ed25519, the
//! public key below), the download's SHA-256 from it, and on macOS the new app's own code
//! signature, which must be valid and from the same team as the running app's. Then the app is
//! swapped once Lightair has quit, and opened again.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use sha2::Digest;

/// The update key's public half (hex), written by `cargo xtask update-key`. Empty: this build
/// doesn't update itself.
pub const PUBLIC_KEY: &str = "";

const MANIFEST: &str = "https://github.com/MoonGoblinDev/lightair/releases/latest/download/latest.json";

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(String),
    /// Ready: Lightair quits and the new version opens.
    Installing,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    url: String,
    sha256: String,
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn state() -> State {
    STATE.lock().clone()
}

fn set(s: State) {
    *STATE.lock() = s;
}

/// Whether this build can update itself (it has the update key).
pub fn enabled() -> bool {
    PUBLIC_KEY.len() == 64
}

/// "1.2.3" → (1, 2, 3); a newer one compares greater.
fn version(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim().trim_start_matches('v').split(['.', '-']).map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

fn platform() -> &'static str {
    if cfg!(windows) { "windows" } else { "macos" }
}

/// The manifest, once its signature checks out: the release for this platform, if newer.
fn newest() -> Result<Option<Release>, String> {
    let key: [u8; 32] = (0..32).filter_map(|i| u8::from_str_radix(PUBLIC_KEY.get(i * 2..i * 2 + 2)?, 16).ok()).collect::<Vec<u8>>().try_into().map_err(|_| "no update key in this build".to_string())?;
    let bytes = lightair_live::download(MANIFEST, 1 << 20)?;
    let sig = lightair_live::download(&format!("{MANIFEST}.sig"), 4096)?;
    if !lightair_live::rules::verify(&key, &bytes, &String::from_utf8_lossy(&sig)) {
        return Err("the update's manifest isn't signed by Lightair's key".into());
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let latest = v["version"].as_str().unwrap_or("");
    if version(latest) <= version(env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let a = &v["assets"][platform()];
    let (Some(url), Some(sha)) = (a["url"].as_str(), a["sha256"].as_str()) else { return Ok(None) };
    Ok(Some(Release {
        version: latest.to_string(),
        notes: v["notes"].as_str().unwrap_or("").to_string(),
        page: v["page"].as_str().unwrap_or("").to_string(),
        url: url.to_string(),
        sha256: sha.to_lowercase(),
    }))
}

/// Looks for an update now (in the background).
pub fn check() {
    if !enabled() || matches!(state(), State::Checking | State::Downloading(_) | State::Installing) {
        return;
    }
    set(State::Checking);
    std::thread::spawn(|| {
        set(match newest() {
            Ok(Some(r)) => State::Available(r),
            Ok(None) => State::UpToDate,
            Err(e) => {
                tracing::info!(%e, "update check");
                State::Failed(e)
            }
        });
    });
}

/// Checks at launch (after a moment) and every six hours while `auto()` says so.
pub fn start(auto: impl Fn() -> bool + Send + 'static) {
    if !enabled() || STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(15));
        loop {
            if auto() && !matches!(state(), State::Available(_) | State::Downloading(_) | State::Installing) {
                check();
            }
            std::thread::sleep(Duration::from_secs(6 * 3600));
        }
    });
}

/// Downloads, checks and stages the update, then starts the swap that runs once Lightair has
/// quit: Ok means quit now.
pub fn install() -> Result<(), String> {
    let State::Available(r) = state() else { return Err("no update to install".into()) };
    set(State::Downloading(r.version.clone()));
    let result = (|| -> Result<(), String> {
        let bytes = lightair_live::download(&r.url, 2 << 30)?;
        if format!("{:x}", sha2::Sha256::digest(&bytes)) != r.sha256 {
            return Err("the download doesn't match the signed manifest".into());
        }
        let dir = std::env::temp_dir().join(format!("lightair-update-{}", r.version));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        stage_and_swap(&dir, &bytes)
    })();
    match result {
        Ok(()) => {
            set(State::Installing);
            Ok(())
        }
        Err(e) => {
            set(State::Failed(e.clone()));
            Err(e)
        }
    }
}

fn run(cmd: &mut std::process::Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The running Lightair.app.
#[cfg(target_os = "macos")]
fn this_app() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()?.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Its code signature's team (None: not signed by a team, a development build).
#[cfg(target_os = "macos")]
fn team(app: &Path) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/codesign").args(["-dv", "--verbose=2"]).arg(app).output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).map(str::trim).filter(|t| *t != "not set").map(String::from)
}

#[cfg(target_os = "macos")]
fn stage_and_swap(dir: &Path, dmg_bytes: &[u8]) -> Result<(), String> {
    let app = this_app().ok_or("Lightair isn't running from an app bundle")?;
    let ours = team(&app).ok_or("this copy isn't signed for release: install the update from its page")?;
    let dmg = dir.join("Lightair.dmg");
    std::fs::write(&dmg, dmg_bytes).map_err(|e| e.to_string())?;
    let mount = dir.join("mount");
    run(std::process::Command::new("/usr/bin/hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mount).arg(&dmg))?;
    let staged = dir.join("Lightair.app");
    let copied = run(std::process::Command::new("/usr/bin/ditto").arg(mount.join("Lightair.app")).arg(&staged));
    let _ = run(std::process::Command::new("/usr/bin/hdiutil").args(["detach", "-quiet"]).arg(&mount));
    copied?;
    run(std::process::Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&staged)).map_err(|e| format!("the new version's signature isn't valid: {e}"))?;
    if team(&staged).as_deref() != Some(ours.as_str()) {
        return Err("the new version isn't signed by the same developer".into());
    }
    // the folder Lightair is in must take the new one (else: install it by hand)
    let parent = app.parent().ok_or("no folder")?;
    let probe = parent.join(".lightair-update-probe");
    if std::fs::write(&probe, b"").is_err() {
        let _ = std::process::Command::new("/usr/bin/open").arg(&dmg).spawn();
        return Err(format!("Lightair can't replace itself in {}: the disk image is open, drag Lightair to Applications", parent.display()));
    }
    let _ = std::fs::remove_file(probe);
    let script = dir.join("swap.sh");
    let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nwhile kill -0 {pid} 2>/dev/null; do sleep 0.2; done\nAPP={app}\nrm -rf \"$APP.old\"\nif mv \"$APP\" \"$APP.old\" && /usr/bin/ditto {new} \"$APP\"; then rm -rf \"$APP.old\"; else rm -rf \"$APP\"; mv \"$APP.old\" \"$APP\"; fi\n/usr/bin/xattr -dr com.apple.quarantine \"$APP\" 2>/dev/null\n/usr/bin/open \"$APP\"\n",
            pid = std::process::id(),
            app = q(&app),
            new = q(&staged),
        ),
    )
    .map_err(|e| e.to_string())?;
    std::process::Command::new("/bin/sh").arg(&script).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(windows)]
fn stage_and_swap(dir: &Path, zip_bytes: &[u8]) -> Result<(), String> {
    // the portable folder: …\Lightair\bin\64bit\lightair.exe
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = exe.parent().and_then(Path::parent).and_then(Path::parent).ok_or("no install folder")?.to_path_buf();
    let zip = dir.join("Lightair.zip");
    std::fs::write(&zip, zip_bytes).map_err(|e| e.to_string())?;
    let out = dir.join("new");
    run(std::process::Command::new("powershell").args(["-NoProfile", "-Command", &format!("Expand-Archive -Path '{}' -DestinationPath '{}' -Force", zip.display(), out.display())]))?;
    let new_root = out.join("Lightair");
    if !new_root.join(r"bin\64bit\lightair.exe").exists() {
        return Err("the download isn't a Lightair build".into());
    }
    let script = dir.join("swap.ps1");
    std::fs::write(
        &script,
        format!(
            "while (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 200 }}\nrobocopy '{new}' '{root}' /MIR /NFL /NDL /NJH /NJS | Out-Null\nStart-Process '{exe}'\n",
            pid = std::process::id(),
            new = new_root.display(),
            root = root.display(),
            exe = root.join(r"bin\64bit\lightair.exe").display(),
        ),
    )
    .map_err(|e| e.to_string())?;
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn stage_and_swap(_: &Path, _: &[u8]) -> Result<(), String> {
    Err("updates install on macOS and Windows".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions_compare() {
        assert!(super::version("0.2.0") > super::version("0.1.9"));
        assert!(super::version("v1.0.0") > super::version("0.99.99"));
        assert_eq!(super::version("0.1.0"), super::version("0.1.0"));
        assert!(super::version("0.1.1-beta") > super::version("0.1.0"));
    }
}
