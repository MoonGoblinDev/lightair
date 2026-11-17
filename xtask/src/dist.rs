//! The app to ship (macOS): `target/dist/Lightair.app`, standalone, and `Lightair-<version>.dmg`.
//!
//! Everything Lightair runs is copied into the app: libobs and the OBS modules it loads (with the
//! frameworks and libraries they link, from the OBS 33 it builds against), Lightair's own modules
//! (Camera Studio), Chromium for web sources, and the command-line tools some features run
//! (FFmpeg, whisper.cpp) with their libraries. Nothing in it points outside it.
//!
//! Then every piece is signed with a Developer ID (hardened runtime, secure timestamp), inside
//! out, and the disk image is notarized and stapled when `LIGHTAIR_NOTARY_PROFILE` names a
//! notarytool keychain profile (`xcrun notarytool store-credentials <name> ...`).

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;

use super::{INFO_PLIST, bundle_chromium, modules, root, run};

/// The shipped app's bundle identifier.
const ID: &str = "app.lightair";

/// The main app: camera and microphone; code it didn't sign (Audio Unit and VST3 plugins, OBS
/// plugins users add); JIT and unsigned executable memory (Chromium, some audio plugins).
const APP_ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.security.device.camera</key><true/>
	<key>com.apple.security.device.audio-input</key><true/>
	<key>com.apple.security.automation.apple-events</key><true/>
	<key>com.apple.security.cs.disable-library-validation</key><true/>
	<key>com.apple.security.cs.allow-jit</key><true/>
	<key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
</dict>
</plist>
"#;

/// Chromium's helpers (V8 needs JIT).
const HELPER_ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.security.cs.allow-jit</key><true/>
	<key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
	<key>com.apple.security.cs.disable-library-validation</key><true/>
</dict>
</plist>
"#;

/// The command-line tools features run (found next to the executable first): exports and sound
/// tools (FFmpeg), transcription (whisper.cpp).
const TOOLS: &[&str] = &["ffmpeg", "ffprobe", "whisper-cli"];

/// Builds the app; a run that fails part way (a timestamp the network didn't give, a codesign
/// refusal) leaves no `Lightair.app` behind to install by mistake: what it built is moved to
/// `Lightair (failed).app`. That was the "sometimes an invalid signature" seen before: a run
/// stopped mid-signing (helpers signed alone, their bundles not yet sealed), its half-signed app
/// then installed and checked.
pub fn dist() -> anyhow::Result<()> {
    crate::release_env();
    let result = build();
    if result.is_err() {
        let out = root().join("target/dist");
        let app = out.join("Lightair.app");
        if app.exists() {
            let failed = out.join("Lightair (failed).app");
            let _ = std::fs::remove_dir_all(&failed);
            let _ = std::fs::rename(&app, &failed);
            eprintln!("dist failed: what it built is in {} (not to be installed)", failed.display());
        }
    }
    result
}

fn build() -> anyhow::Result<()> {
    anyhow::ensure!(cfg!(target_os = "macos"), "the shipped app is built on macOS");
    let root = root();
    let version = env!("CARGO_PKG_VERSION");
    run(Command::new("cargo").current_dir(&root).args(["build", "--release", "-p", "lightair-app", "-p", "lightair-web-helper"]))?;
    modules()?;
    let obs = PathBuf::from(std::env::var("OBS_APP").unwrap_or_else(|_| "/Applications/OBS.app".into()));
    let out = root.join("target/dist");
    let _ = std::fs::remove_dir_all(&out);
    let app = out.join("Lightair.app");
    let c = app.join("Contents");
    for dir in ["MacOS", "Frameworks", "PlugIns", "Resources"] {
        std::fs::create_dir_all(c.join(dir))?;
    }

    // the app
    std::fs::copy(root.join("target/release/lightair"), c.join("MacOS/lightair"))?;
    std::fs::write(c.join("Info.plist"), INFO_PLIST.replace("app.lightair.dev", ID).replace("0.1.0", version))?;
    std::fs::copy(root.join("branding/final/icon/Lightair.icns"), c.join("Resources/Lightair.icns"))?;
    std::fs::copy(root.join("branding/final/icon/lightair-icon-512.png"), c.join("Resources/lightair-icon-512.png"))?;
    crate::stream_deck_plugin(&c.join("Resources/Lightair.streamDeckPlugin"))?;
    crate::speech_helper(&c.join("MacOS/lightair-speech"))?;

    // libobs, its graphics backends and mux helper, the modules Lightair loads
    let obs_frameworks = obs.join("Contents/Frameworks");
    ditto(&obs_frameworks.join("libobs.framework"), &c.join("Frameworks/libobs.framework"))?;
    for lib in ["libobs-opengl.dylib", "libobs-metal.dylib"] {
        std::fs::copy(obs_frameworks.join(lib), c.join("Frameworks").join(lib))?;
    }
    std::fs::copy(obs.join("Contents/MacOS/obs-ffmpeg-mux"), c.join("MacOS/obs-ffmpeg-mux"))?;
    for module in default_modules(&root)? {
        let plugin = obs.join("Contents/PlugIns").join(format!("{module}.plugin"));
        if plugin.exists() {
            ditto(&plugin, &c.join("PlugIns").join(format!("{module}.plugin")))?;
        }
    }
    ditto(
        &root.join("target/modules/lightair-camera-fx/lightair-camera-fx.plugin"),
        &c.join("PlugIns/lightair-camera-fx.plugin"),
    )?;

    // Chromium
    bundle_chromium(&root, &c, "release", true, ID)?;

    // the tools, from wherever they're installed (Homebrew)
    let mut origins = std::collections::HashMap::new();
    for tool in TOOLS {
        let found = which(tool).ok_or_else(|| anyhow::anyhow!("{tool} isn't installed (brew install ffmpeg whisper-cpp)"))?;
        let found = std::fs::canonicalize(found)?;
        std::fs::copy(&found, c.join("MacOS").join(tool))?;
        origins.insert(c.join("MacOS").join(tool), found);
        run(Command::new("install_name_tool").args(["-add_rpath", "@executable_path/../Frameworks"]).arg(c.join("MacOS").join(tool)))?;
    }

    gather_libraries(&c, &obs_frameworks, &mut origins)?;

    // only the app's own Frameworks: no falling back to an installed OBS or Homebrew
    drop_outside_rpaths(&app)?;
    check_self_contained(&app)?;

    let identity = std::env::var("LIGHTAIR_DIST_IDENTITY").ok().filter(|s| !s.is_empty()).or_else(developer_id).ok_or_else(|| {
        anyhow::anyhow!("no \"Developer ID Application\" identity in the keychain (or set LIGHTAIR_DIST_IDENTITY)")
    })?;
    sign_all(&app, &identity, &out)?;
    run(Command::new("codesign").args(["--verify", "--deep", "--strict", "--verbose=1"]).arg(&app))?;

    // the disk image: the app and a link to /Applications
    let stage = out.join("dmg");
    std::fs::create_dir_all(&stage)?;
    ditto(&app, &stage.join("Lightair.app"))?;
    std::os::unix::fs::symlink("/Applications", stage.join("Applications"))?;
    let dmg = out.join(format!("Lightair-{version}.dmg"));
    run(Command::new("hdiutil")
        .args(["create", "-quiet", "-volname", "Lightair", "-fs", "HFS+", "-format", "UDZO", "-ov", "-srcfolder"])
        .arg(&stage)
        .arg(&dmg))?;
    std::fs::remove_dir_all(&stage)?;
    run(Command::new("codesign").args(["--force", "--timestamp", "--sign", &identity]).arg(&dmg))?;

    match std::env::var("LIGHTAIR_NOTARY_PROFILE").ok().filter(|s| !s.is_empty()) {
        Some(profile) => {
            run(Command::new("xcrun").args(["notarytool", "submit", "--wait", "--keychain-profile", &profile]).arg(&dmg))?;
            run(Command::new("xcrun").args(["stapler", "staple"]).arg(&dmg))?;
            run(Command::new("xcrun").args(["stapler", "staple"]).arg(&app))?;
            eprintln!("notarized and stapled");
        }
        None => eprintln!(
            "not notarized: store credentials once with\n  xcrun notarytool store-credentials lightair --apple-id <Apple ID> --team-id <team>\nthen run again with LIGHTAIR_NOTARY_PROFILE=lightair"
        ),
    }
    eprintln!("built {}\n      {}", app.display(), dmg.display());
    Ok(())
}

fn ditto(from: &Path, to: &Path) -> anyhow::Result<()> {
    run(Command::new("ditto").arg(from).arg(to))
}

/// The OBS modules Lightair loads on macOS: `lightair_engine::DEFAULT_MODULES` (its first list).
fn default_modules(root: &Path) -> anyhow::Result<Vec<String>> {
    let lib = std::fs::read_to_string(root.join("crates/lightair-engine/src/lib.rs"))?;
    let start = lib.find("pub const DEFAULT_MODULES").ok_or_else(|| anyhow::anyhow!("no DEFAULT_MODULES"))?;
    let list = &lib[start..];
    let list = &list[list.find("&[").unwrap() + 2..list.find("];").unwrap()];
    Ok(list.split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect())
}

fn which(tool: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .map(|d| d.join(tool))
        .find(|p| p.is_file())
}

fn is_macho(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok()
        && matches!(magic, [0xcf, 0xfa, 0xed, 0xfe] | [0xca, 0xfe, 0xba, 0xbe] | [0xce, 0xfa, 0xed, 0xfe])
}

/// Every file under `dir` (not following links).
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if meta.is_dir() {
                todo.push(p);
            } else if meta.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The libraries a binary links (not its own install name, which `otool -L` lists too).
fn linked(binary: &Path) -> Vec<String> {
    let text = |args: &[&str]| {
        Command::new("otool").args(args).arg(binary).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    // (universal binaries: a header line per architecture, ending in ':')
    let own = text(&["-D"]).lines().find(|l| !l.ends_with(':')).map(|l| l.trim().to_string());
    text(&["-L"])
        .lines()
        .filter(|l| !l.ends_with(':'))
        .filter_map(|l| l.trim().split(" (").next().map(str::to_string))
        .filter(|s| !s.is_empty() && Some(s) != own.as_ref())
        .collect()
}

/// Copies in every library the app's code links that isn't in it yet (OBS's frameworks and
/// libraries, or installed ones), and points installed ones (absolute paths) at the copies.
/// `origins`: where copied-in installed binaries came from (their `@loader_path` search paths
/// are relative to it).
fn gather_libraries(c: &Path, obs_frameworks: &Path, origins: &mut std::collections::HashMap<PathBuf, PathBuf>) -> anyhow::Result<()> {
    let frameworks = c.join("Frameworks");
    let mut todo: VecDeque<PathBuf> = files(c).into_iter().filter(|p| is_macho(p)).collect();
    let mut done = BTreeSet::new();
    while let Some(binary) = todo.pop_front() {
        if !done.insert(binary.clone()) {
            continue;
        }
        for dep in linked(&binary) {
            if dep.starts_with("/System/") || dep.starts_with("/usr/lib/") {
                continue;
            }
            if let Some(rel) = dep.strip_prefix("@rpath/").or_else(|| dep.strip_prefix("@executable_path/../Frameworks/")) {
                let inside = frameworks.join(rel);
                if inside.exists() {
                    continue;
                }
                // OBS's: a framework (copy all of it) or a library
                let top = rel.split('/').next().unwrap();
                let from = obs_frameworks.join(top);
                if !from.exists() {
                    // an installed library linked through its own search paths (Homebrew)
                    let origin = origins.get(&binary).cloned();
                    let found = rpaths(&binary)
                        .into_iter()
                        .filter_map(|r| match (r.strip_prefix("@loader_path/"), &origin) {
                            (Some(sub), Some(o)) => Some(o.parent()?.join(sub)),
                            (None, _) if r.starts_with('/') => Some(PathBuf::from(r)),
                            _ => None,
                        })
                        .map(|dir| dir.join(rel))
                        .find(|p| p.exists());
                    let found = found.ok_or_else(|| anyhow::anyhow!("{} links {dep}, which isn't in the app, OBS or its search paths", binary.display()))?;
                    let copy = frameworks.join(rel);
                    copy_library(&found, &copy)?;
                    origins.insert(copy.clone(), std::fs::canonicalize(&found)?);
                    todo.push_back(copy);
                    continue;
                }
                if top.ends_with(".framework") {
                    ditto(&from, &frameworks.join(top))?;
                    todo.extend(files(&frameworks.join(top)).into_iter().filter(|p| is_macho(p)));
                } else {
                    std::fs::copy(&from, frameworks.join(top)).with_context(|| format!("{}", from.display()))?;
                    todo.push_back(frameworks.join(top));
                }
                continue;
            }
            if dep.starts_with('@') {
                // @loader_path and friends: resolved inside the copied bundle already
                continue;
            }
            // an installed library (Homebrew): copied in, linked through @rpath
            let name = Path::new(&dep).file_name().unwrap().to_string_lossy().into_owned();
            let copy = frameworks.join(&name);
            if !copy.exists() {
                copy_library(Path::new(&dep), &copy)?;
                origins.insert(copy.clone(), std::fs::canonicalize(&dep)?);
                todo.push_back(copy);
            }
            run(Command::new("install_name_tool").args(["-change", &dep, &format!("@rpath/{name}")]).arg(&binary))?;
        }
    }
    Ok(())
}

/// An installed library copied in as `@rpath/<name>`, finding its neighbours beside it.
fn copy_library(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::copy(std::fs::canonicalize(from).with_context(|| format!("{}", from.display()))?, to).with_context(|| format!("{} -> {}", from.display(), to.display()))?;
    let mut perms = std::fs::metadata(to)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(to, perms)?;
    let name = to.file_name().unwrap().to_string_lossy();
    run(Command::new("install_name_tool").args(["-id", &format!("@rpath/{name}")]).arg(to))?;
    let _ = Command::new("install_name_tool").args(["-add_rpath", "@loader_path"]).arg(to).output();
    Ok(())
}

/// A binary's library search paths (LC_RPATH).
fn rpaths(binary: &Path) -> Vec<String> {
    let out = Command::new("otool").arg("-l").arg(binary).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut lines = out.lines();
    let mut found = Vec::new();
    while let Some(l) = lines.next() {
        if l.trim() == "cmd LC_RPATH"
            && let Some(path) = lines.nth(1).and_then(|l| l.trim().strip_prefix("path ")).and_then(|p| p.split(" (offset").next())
        {
            found.push(path.to_string());
        }
    }
    found
}

/// Drops search paths outside the app (an installed OBS, Homebrew's folders) from all its code.
fn drop_outside_rpaths(app: &Path) -> anyhow::Result<()> {
    for f in files(app).into_iter().filter(|p| is_macho(p)) {
        for r in rpaths(&f).into_iter().filter(|r| r.starts_with('/') && !r.starts_with("/usr/lib/") && !r.starts_with("/System/")) {
            run(Command::new("install_name_tool").args(["-delete_rpath", &r]).arg(&f))?;
        }
    }
    Ok(())
}

/// No link pointing outside the app, no library linked from outside it.
fn check_self_contained(app: &Path) -> anyhow::Result<()> {
    let mut todo = vec![app.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&p)?;
                anyhow::ensure!(target.is_relative(), "{} links outside the app ({})", p.display(), target.display());
                let resolved = std::fs::canonicalize(&p)?;
                anyhow::ensure!(resolved.starts_with(std::fs::canonicalize(app)?), "{} links outside the app", p.display());
            } else if meta.is_dir() {
                todo.push(p);
            }
        }
    }
    for f in files(app).into_iter().filter(|p| is_macho(p)) {
        for dep in linked(&f) {
            anyhow::ensure!(
                dep.starts_with('@') || dep.starts_with("/System/") || dep.starts_with("/usr/lib/"),
                "{} links {dep} outside the app",
                f.display()
            );
        }
    }
    Ok(())
}

fn developer_id() -> Option<String> {
    let out = Command::new("security").args(["find-identity", "-v", "-p", "codesigning"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.contains("\"Developer ID Application:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .filter(|h| h.len() == 40)
        .map(str::to_string)
}

/// Signs inside out: every piece of code, then every nested bundle (deepest first), then the
/// app, all with the hardened runtime and a secure timestamp.
fn sign_all(app: &Path, identity: &str, out: &Path) -> anyhow::Result<()> {
    let app_ent = out.join("app.entitlements");
    let helper_ent = out.join("helper.entitlements");
    std::fs::write(&app_ent, APP_ENTITLEMENTS)?;
    std::fs::write(&helper_ent, HELPER_ENTITLEMENTS)?;
    let sign = |path: &Path, entitlements: Option<&Path>| -> anyhow::Result<()> {
        let mut cmd = Command::new("codesign");
        cmd.args(["--force", "--timestamp", "--options", "runtime", "--sign", identity]);
        if let Some(e) = entitlements {
            cmd.arg("--entitlements").arg(e);
        }
        cmd.arg(path);
        // the secure timestamp comes from Apple over the network: a moment's failure is retried
        for attempt in 1.. {
            let output = cmd.output()?;
            if output.status.success() {
                return Ok(());
            }
            let err = String::from_utf8_lossy(&output.stderr).into_owned();
            let transient = ["timestamp", "network", "Internet", "not available", "timed out"].iter().any(|w| err.contains(w));
            anyhow::ensure!(transient && attempt < 4, "codesign {}: {err}", path.display());
            eprintln!("codesign {}: {} (trying again)", path.display(), err.trim());
            std::thread::sleep(std::time::Duration::from_secs(3 * attempt));
        }
        unreachable!()
    };
    let is_bundle = |p: &Path| p.extension().is_some_and(|e| e == "framework" || e == "plugin" || e == "app" || e == "bundle");
    // bundles inside the app, deepest first
    let mut bundles = Vec::new();
    let mut todo = vec![app.join("Contents")];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.is_dir() {
                if is_bundle(&p) {
                    bundles.push(p.clone());
                }
                todo.push(p);
            }
        }
    }
    bundles.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    let code: Vec<PathBuf> = files(app).into_iter().filter(|p| is_macho(p)).collect();
    eprintln!("signing {} pieces of code and {} bundles with the hardened runtime", code.len(), bundles.len());
    for f in &code {
        let helper = f.to_string_lossy().contains("Lightair Helper");
        sign(f, helper.then_some(helper_ent.as_path()))?;
    }
    for b in &bundles {
        let helper = b.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Lightair Helper"));
        sign(b, helper.then_some(helper_ent.as_path()))?;
    }
    sign(app, Some(&app_ent))
}
