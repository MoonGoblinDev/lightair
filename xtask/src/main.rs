//! Build orchestration (docs/15-platform-distribution-licensing.md, "Building").
//!
//!   cargo xtask modules    configure and build every in-tree libobs module (modules/*)
//!   cargo xtask bundle     assemble target/debug/Lightair.app for development (macOS)
//!   cargo xtask bundle --release   the same from an optimised build: target/release/Lightair.app
//!                              (for measuring performance)
//!   cargo xtask dist       the app to ship: target/dist/Lightair.app and Lightair-<version>.dmg,
//!                              everything it runs inside it, signed with a Developer ID and
//!                              notarized (LIGHTAIR_NOTARY_PROFILE) (macOS)
//!   cargo xtask dev        link OBS's helper programs next to target/debug for `cargo run`
//!   cargo xtask virtual-camera [--uninstall]  build "Lightair Virtual Camera" (the camera
//!                          extension and its installer, modules/lightair-virtual-camera),
//!                          put the installer in /Applications and open it (macOS)
//!
//! On Windows, OBS_DIR names the OBS build or install whose libobs Lightair links and runs
//! (docs/plans/windows.md); the tests run with its `bin/64bit` on PATH.
//!   cargo xtask test [--machine] [filter]  build every test program, then run them all at once
//!
//! The edit loop (docs/build-speed.md):
//!   cargo xtask check [--changed]  `cargo check` every target for this Mac (`--changed`: only
//!                              the crates changed since the last commit and those that use them)
//!   cargo xtask precommit      clippy for macOS and for Windows, then `cargo xtask test`: what a
//!                              commit needs (0 warnings, 0 failures)
//!   cargo xtask prune          delete object files no program in target/debug/deps uses

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "macos")]
mod dist;
mod rules;
mod update;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Links `to` at `link` (Windows: copies it; symbolic links need developer mode there).
fn link_file(to: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(to, link);
    #[cfg(not(unix))]
    return std::fs::copy(to, link).map(|_| ());
}

/// OBS's mux helper (clips and MKV recordings), from the OBS Lightair runs with.
fn obs_mux() -> PathBuf {
    if cfg!(windows) {
        obs_bin().join("obs-ffmpeg-mux.exe")
    } else {
        PathBuf::from(std::env::var("OBS_APP").unwrap_or_else(|_| "/Applications/OBS.app".into()))
            .join("Contents/MacOS/obs-ffmpeg-mux")
    }
}

/// Windows: the folder holding obs.dll (OBS_DIR's `bin/64bit`).
fn obs_bin() -> PathBuf {
    let dir = std::env::var("OBS_DIR").unwrap_or_else(|_| r"C:\Program Files\obs-studio".into());
    PathBuf::from(dir).join("bin").join("64bit")
}

/// Lower priority for this process and its children, so the machine stays responsive.
fn be_polite() {
    #[cfg(unix)]
    unsafe {
        setpriority(0, 0, 10)
    };
}

fn run(cmd: &mut Command) -> anyhow::Result<()> {
    eprintln!("$ {cmd:?}");
    let status = cmd.status()?;
    anyhow::ensure!(status.success(), "command failed: {status}");
    Ok(())
}

/// The Stream Deck plugin (streamdeck/), packed as the file Stream Deck installs when opened:
/// a zip of the `.sdPlugin` folder.
pub(crate) fn stream_deck_plugin(dest: &Path) -> anyhow::Result<()> {
    let _ = std::fs::remove_file(dest);
    run(Command::new("/usr/bin/ditto").args(["-c", "-k", "--keepParent", "--norsrc", "--noextattr", "--noqtn"]).arg(root().join("streamdeck/app.lightair.sdPlugin")).arg(dest))
}

/// The publisher's private release settings, from `release.env` at the repository's root (kept
/// out of git: .gitignore): `KEY=value` lines, set for the build unless already set. Lightair's
/// registered apps (LIGHTAIR_YOUTUBE_CLIENT_ID, …: compiled in, docs/official-apis.md) and
/// LIGHTAIR_NOTARY_PROFILE.
pub(crate) fn release_env() {
    let Ok(text) = std::fs::read_to_string(root().join("release.env")) else { return };
    let mut set = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim().trim_matches('"'));
        if v.is_empty() || std::env::var_os(k).is_some() {
            continue;
        }
        // SAFETY: set before any thread starts
        unsafe { std::env::set_var(k, v) };
        set.push(k.to_string());
    }
    if !set.is_empty() {
        eprintln!("release.env: {}", set.join(", "));
    }
}

/// The speech helper for voice-phrase automations (tools/speech, Swift: Apple's speech
/// recognition), built once into target/ and copied to `dest`. macOS only; without Swift it's
/// left out (voice phrases then say so).
pub(crate) fn speech_helper(dest: &Path) -> anyhow::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let src = root().join("tools/speech/main.swift");
    let built = root().join("target/lightair-speech");
    let stale = match (std::fs::metadata(&src).and_then(|m| m.modified()), std::fs::metadata(&built).and_then(|m| m.modified())) {
        (Ok(s), Ok(b)) => s > b,
        _ => true,
    };
    if stale {
        let ok = Command::new("swiftc").args(["-O", "-suppress-warnings"]).arg(&src).arg("-o").arg(&built).status().is_ok_and(|s| s.success());
        if !ok {
            eprintln!("left out the speech helper (no Swift compiler): voice-phrase automations won't hear");
            return Ok(());
        }
    }
    std::fs::copy(&built, dest)?;
    Ok(())
}

fn modules() -> anyhow::Result<()> {
    let root = root();
    for entry in std::fs::read_dir(root.join("modules"))? {
        let dir = entry?.path();
        if !dir.join("CMakeLists.txt").exists() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let build = root.join("target/modules").join(&name);
        let mut configure = Command::new("cmake");
        configure.arg("-S").arg(&dir).arg("-B").arg(&build);
        if cfg!(windows) {
            // Visual Studio's generator (Ninja needs a developer prompt for MSVC)
            configure.args(["-A", "x64"]);
        } else {
            configure.args(["-G", "Ninja", "-DCMAKE_BUILD_TYPE=RelWithDebInfo", "-DCMAKE_OSX_ARCHITECTURES=arm64"]);
        }
        run(&mut configure)?;
        run(Command::new("cmake").arg("--build").arg(&build).args(["--config", "RelWithDebInfo"]))?;
    }
    Ok(())
}

const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>Lightair</string>
	<key>CFBundleDisplayName</key><string>Lightair</string>
	<key>CFBundleIdentifier</key><string>app.lightair.dev</string>
	<key>CFBundleExecutable</key><string>lightair</string>
	<key>CFBundleIconFile</key><string>Lightair</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>0.1.0</string>
	<key>CFBundleVersion</key><string>1</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>NSCameraUsageDescription</key><string>Lightair shows your camera in your scenes and recordings.</string>
	<key>NSMicrophoneUsageDescription</key><string>Lightair records and streams your microphone.</string>
	<key>NSAudioCaptureUsageDescription</key><string>Lightair records and streams the sound your Mac and its apps play.</string>
	<key>NSBluetoothAlwaysUsageDescription</key><string>Signing in to a platform with a passkey on your phone uses Bluetooth to reach it.</string>
	<key>NSLocationUsageDescription</key><string>A web page in Lightair asked where you are.</string>
	<key>NSLocationWhenInUseUsageDescription</key><string>A web page in Lightair asked where you are.</string>
	<key>NSSpeechRecognitionUsageDescription</key><string>Lightair listens for the phrases your automations run on (on this Mac where the language allows), and web pages in Lightair may ask to recognise speech.</string>
	<key>NSAppleEventsUsageDescription</key><string>Lightair asks Spotify and Music what's playing, for the Now playing widget.</string>
	<key>NSLocalNetworkUsageDescription</key><string>Lightair finds network video sources and serves the remote control API.</string>
</dict>
</plist>
"#;

/// Development bundle: the debug binary inside a real .app (so macOS asks Lightair, not the
/// terminal, for camera and microphone access), with OBS's frameworks and plugins linked in
/// from an installed OBS 33 (docs/15-platform-distribution-licensing.md, "Building").
fn bundle(release: bool) -> anyhow::Result<()> {
    anyhow::ensure!(cfg!(target_os = "macos"), "the .app bundle is for macOS; on Windows, `cargo xtask dev`");
    let root = root();
    let profile = if release { "release" } else { "debug" };
    let mut build = Command::new("cargo");
    build.current_dir(&root).args(["build", "-p", "lightair-app", "-p", "lightair-web-helper"]);
    if release {
        build.arg("--release");
    }
    run(&mut build)?;
    let obs = PathBuf::from(std::env::var("OBS_APP").unwrap_or_else(|_| "/Applications/OBS.app".into()));
    let app = root.join(format!("target/{profile}/Lightair.app/Contents"));
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
    for dir in ["MacOS", "Frameworks", "PlugIns", "Resources"] {
        std::fs::create_dir_all(app.join(dir))?;
    }
    std::fs::copy(root.join(format!("target/{profile}/lightair")), app.join("MacOS/lightair"))?;
    // libobs runs this helper (from next to the executable) to write clips and MKV recordings.
    link_file(&obs.join("Contents/MacOS/obs-ffmpeg-mux"), &app.join("MacOS/obs-ffmpeg-mux"))?;
    std::fs::write(app.join("Info.plist"), INFO_PLIST)?;
    let icns = root.join("branding/final/icon/Lightair.icns");
    if icns.exists() {
        std::fs::copy(icns, app.join("Resources/Lightair.icns"))?;
    }
    stream_deck_plugin(&app.join("Resources/Lightair.streamDeckPlugin"))?;
    speech_helper(&app.join("MacOS/lightair-speech"))?;
    for (from, to) in
        [(obs.join("Contents/Frameworks"), app.join("Frameworks")), (obs.join("Contents/PlugIns"), app.join("PlugIns"))]
    {
        for entry in std::fs::read_dir(&from)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // OBS's Chromium (CEF) and its helpers: Lightair bundles its own, below.
            if name_str.starts_with("OBS Helper") || name_str == "Chromium Embedded Framework.framework" {
                continue;
            }
            link_file(&entry.path(), &to.join(&name))?;
        }
    }
    bundle_chromium(&root, &app, profile, false, "app.lightair.dev")?;
    // Sign the bundle itself so macOS privacy settings identify Lightair. Nested OBS code keeps
    // OBS's own signatures; nothing outside target/ is modified.
    //
    // Ad hoc signatures change with every build, so macOS asks for camera, microphone and screen
    // permission again after each rebuild. A real identity keeps them: the first "Apple
    // Development" identity in the keychain is used by default; LIGHTAIR_SIGN_IDENTITY overrides
    // it ("-" for ad hoc).
    let identity = std::env::var("LIGHTAIR_SIGN_IDENTITY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(apple_development_identity)
        .unwrap_or_else(|| "-".into());
    run(Command::new("codesign").args(["--force", "--sign", &identity]).arg(app.parent().unwrap()))?;
    if identity == "-" {
        eprintln!(
            "signed ad hoc: macOS will ask for permissions again after rebuilds (set LIGHTAIR_SIGN_IDENTITY to keep them)"
        );
    } else {
        eprintln!("signed with {identity}");
    }
    eprintln!("built {}", app.parent().unwrap().display());
    Ok(())
}

/// Lightair's Chromium for web page sources (lightair-web): the pinned CEF framework the cef
/// crate fetched into target/cef, and Chromium's four helper apps, Lightair's own (each the
/// lightair-web-helper binary under the name Chromium looks for).
/// `copy`: the framework copied in (the shipped app) rather than linked; `id`: the app's bundle
/// identifier, the helpers' prefix.
fn bundle_chromium(root: &Path, app: &Path, profile: &str, copy: bool, id: &str) -> anyhow::Result<()> {
    let framework = std::fs::read_dir(root.join("target/cef"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|version| std::fs::read_dir(version.path()).into_iter().flatten().flatten())
        .map(|platform| platform.path().join("Chromium Embedded Framework.framework"))
        .find(|p| p.exists());
    let Some(framework) = framework else {
        anyhow::ensure!(!copy, "no Chromium in target/cef (build lightair-web once)");
        eprintln!("no Chromium in target/cef (build lightair-web once): web pages won't paint");
        return Ok(());
    };
    let into = app.join("Frameworks/Chromium Embedded Framework.framework");
    if copy {
        run(Command::new("ditto").arg(&framework).arg(&into))?;
    } else {
        link_file(&framework, &into)?;
    }
    let prefix = id;
    let helper = root.join(format!("target/{profile}/lightair-web-helper"));
    for (suffix, id) in [("", ""), (" (GPU)", ".gpu"), (" (Renderer)", ".renderer"), (" (Plugin)", ".plugin")] {
        let name = format!("Lightair Helper{suffix}");
        let contents = app.join("Frameworks").join(format!("{name}.app")).join("Contents");
        std::fs::create_dir_all(contents.join("MacOS"))?;
        std::fs::copy(&helper, contents.join("MacOS").join(&name))?;
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>{name}</string>
	<key>CFBundleExecutable</key><string>{name}</string>
	<key>CFBundleIdentifier</key><string>{prefix}.helper{id}</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>0.1.0</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>LSUIElement</key><true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
"#
            ),
        )?;
    }
    Ok(())
}

/// Links OBS's helper programs next to the debug binary (development without the bundle).
/// Windows: Chromium's runtime too (lightair.exe loads libcef.dll as it starts, and Chromium
/// finds its resources beside it), for `target/debug`, or `target/release` with `--release`.
fn dev(release: bool) -> anyhow::Result<()> {
    let mux = obs_mux();
    let profile = if release { "release" } else { "debug" };
    let out = root().join("target").join(profile);
    // Windows: the encoders' test programs too (NVENC, Quick Sync and AMF register only once
    // theirs, run from beside the executable, says the hardware works)
    let mut helpers = vec![mux];
    if cfg!(windows) {
        helpers.extend(["obs-nvenc-test.exe", "obs-qsv-test.exe", "obs-amf-test.exe"].map(|h| obs_bin().join(h)));
    }
    for dir in [out.clone(), out.join("deps")] {
        std::fs::create_dir_all(&dir)?;
        for helper in helpers.iter().filter(|h| h.exists()) {
            let link = dir.join(helper.file_name().unwrap());
            if !link.exists() {
                link_file(helper, &link)?;
            }
        }
        if cfg!(windows) {
            chromium_runtime(&dir)?;
        }
    }
    Ok(())
}

/// The OBS modules the Windows app ships (lightair-engine's DEFAULT_MODULES for Windows, but
/// Lightair's own Camera Studio, built here): keep the two lists alike.
#[cfg(windows)]
const WINDOWS_MODULES: &[&str] = &[
    "aja", "decklink", "image-source", "obs-ffmpeg", "obs-filters", "obs-nvenc", "obs-outputs", "obs-qsv11",
    "obs-transitions", "obs-webrtc", "obs-x264", "rtmp-services", "text-freetype2", "vlc-video", "win-capture",
    "win-dshow", "win-wasapi",
];

/// Windows: the app to ship, `target\dist\Lightair` and `Lightair-<version>-windows-x64.zip`,
/// self-contained, laid out as OBS lays itself out (what libobs and its modules expect):
///
/// ```text
/// Lightair\bin\64bit\    lightair.exe, Chromium's helper and runtime, libobs and its DLLs,
///                        OBS's helper programs, the MSVC runtime
/// Lightair\core\<m>\     OBS's modules Lightair uses, each with its data
/// Lightair\data\libobs\  libobs's effects
/// Lightair\obs-plugins\64bit\, Lightair\data\obs-plugins\   Camera Studio
/// ```
///
/// libobs and its modules come from the OBS install it's built against (OBS_DIR). Not signed:
/// Authenticode needs a certificate (docs/15).
#[cfg(windows)]
fn dist_windows() -> anyhow::Result<()> {
    release_env();
    let root = root();
    run(cargo().current_dir(&root).args(["build", "--release", "-p", "lightair-app", "-p", "lightair-web-helper", "-p", "lightair-camera-win"]))?;
    modules()?;
    let obs = PathBuf::from(std::env::var("OBS_DIR").unwrap_or_else(|_| r"C:\Program Files\obs-studio".into()));
    anyhow::ensure!(obs.join(r"bin\64bit\obs.dll").exists(), "no OBS at {} (set OBS_DIR)", obs.display());
    let out = root.join(r"target\dist\Lightair");
    let _ = std::fs::remove_dir_all(&out);
    let bin = out.join(r"bin\64bit");
    std::fs::create_dir_all(&bin)?;
    let release = root.join(r"target\release");
    // (and Lightair Virtual Camera's media source: the Frame Server loads it from here once
    // it's registered, crates/lightair-camera-win)
    for exe in ["lightair.exe", "lightair-web-helper.exe", "lightair_camera.dll"] {
        std::fs::copy(release.join(exe), bin.join(exe))?;
    }
    // libobs, its graphics backend, its DLLs and helper programs (not OBS's own UI: Qt, scripting)
    let skip = |name: &str| {
        let n = name.to_ascii_lowercase();
        n.starts_with("qt6") || n.ends_with(".pdb") || n == "obs64.exe" || n == "obs-frontend-api.dll"
            || n == "obs-scripting.dll" || n == "lua51.dll" || n == "libobs-opengl.dll"
    };
    for entry in std::fs::read_dir(obs.join(r"bin\64bit"))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && !skip(&name) {
            std::fs::copy(entry.path(), bin.join(&name))?;
        }
    }
    chromium_runtime(&bin)?;
    // the Stream Deck plugin, as the file Stream Deck installs when opened (a zip of the
    // `.sdPlugin` folder; PowerShell makes only `.zip`s, so renamed)
    let deck_zip = bin.join("Lightair.streamDeckPlugin.zip");
    run(Command::new("powershell").args(["-NoProfile", "-Command"]).arg(format!(
        "Compress-Archive -Path '{}' -DestinationPath '{}'",
        root.join(r"streamdeck\app.lightair.sdPlugin").display(),
        deck_zip.display()
    )))?;
    std::fs::rename(&deck_zip, bin.join("Lightair.streamDeckPlugin"))?;
    // the MSVC runtime, beside the programs (Microsoft's redistributable files)
    match msvc_runtime() {
        Some(crt) => {
            for entry in std::fs::read_dir(&crt)? {
                let entry = entry?;
                std::fs::copy(entry.path(), bin.join(entry.file_name()))?;
            }
        }
        None => eprintln!("no MSVC redistributable found: the app needs the Visual C++ runtime installed"),
    }
    // OBS's modules, each with its data
    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let (src, dst) = (entry.path(), to.join(entry.file_name()));
            if entry.file_type()?.is_dir() {
                copy_tree(&src, &dst)?;
            } else if !src.extension().is_some_and(|e| e == "pdb") {
                std::fs::copy(&src, &dst)?;
            }
        }
        Ok(())
    }
    for m in WINDOWS_MODULES {
        let dir = obs.join("core").join(m);
        if dir.exists() {
            copy_tree(&dir, &out.join("core").join(m))?;
        }
    }
    copy_tree(&obs.join(r"data\libobs"), &out.join(r"data\libobs"))?;
    // Camera Studio (cargo xtask modules)
    let fx = root.join(r"target\modules\lightair-camera-fx");
    if fx.join(r"obs-plugins\64bit\lightair-camera-fx.dll").exists() {
        std::fs::create_dir_all(out.join(r"obs-plugins\64bit"))?;
        std::fs::copy(fx.join(r"obs-plugins\64bit\lightair-camera-fx.dll"), out.join(r"obs-plugins\64bit\lightair-camera-fx.dll"))?;
        copy_tree(&fx.join(r"data\obs-plugins\lightair-camera-fx"), &out.join(r"data\obs-plugins\lightair-camera-fx"))?;
    } else {
        eprintln!("Camera Studio isn't built (cargo xtask modules): left out");
    }
    let version = env!("CARGO_PKG_VERSION");
    let zip = root.join(format!(r"target\dist\Lightair-{version}-windows-x64.zip"));
    let _ = std::fs::remove_file(&zip);
    run(Command::new("powershell")
        .args(["-NoProfile", "-Command"])
        .arg(format!("Compress-Archive -Path '{}' -DestinationPath '{}' -CompressionLevel Optimal", out.display(), zip.display())))?;
    eprintln!("built {} and {}", out.display(), zip.display());
    Ok(())
}

/// The MSVC runtime's redistributable folder (`Microsoft.VC143.CRT`, x64) of an installed
/// Visual Studio or its Build Tools.
#[cfg(windows)]
fn msvc_runtime() -> Option<PathBuf> {
    let vs = PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio");
    let alt = PathBuf::from(r"C:\Program Files\Microsoft Visual Studio");
    let mut found: Vec<PathBuf> = Vec::new();
    for base in [vs, alt] {
        for year in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            for edition in std::fs::read_dir(year.path()).into_iter().flatten().flatten() {
                let redist = edition.path().join(r"VC\Redist\MSVC");
                for version in std::fs::read_dir(&redist).into_iter().flatten().flatten() {
                    let crt = version.path().join(r"x64\Microsoft.VC143.CRT");
                    if crt.join("vcruntime140.dll").exists() {
                        found.push(crt);
                    }
                }
            }
        }
    }
    found.sort();
    found.pop()
}

/// Windows: copies Chromium's runtime (CEF's Release folder and its Resources, as fetched
/// into target/cef) into `dir`, where only what's changed is copied again.
fn chromium_runtime(dir: &Path) -> anyhow::Result<()> {
    let Some(cef) = std::fs::read_dir(root().join("target/cef"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|version| std::fs::read_dir(version.path()).into_iter().flatten().flatten())
        .map(|platform| platform.path())
        .find(|p| p.join("libcef.dll").exists())
    else {
        eprintln!("no Chromium in target/cef (build lightair-web once): web pages won't paint");
        return Ok(());
    };
    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let (src, dst) = (entry.path(), to.join(entry.file_name()));
            if entry.file_type()?.is_dir() {
                copy_tree(&src, &dst)?;
            } else {
                let same = std::fs::metadata(&dst).ok().zip(entry.metadata().ok()).is_some_and(|(d, s)| d.len() == s.len() && d.modified().ok() >= s.modified().ok());
                if !same {
                    std::fs::copy(&src, &dst)?;
                }
            }
        }
        Ok(())
    }
    for entry in std::fs::read_dir(&cef)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let wanted = name.ends_with(".dll") || name.ends_with(".pak") || name.ends_with(".dat") || name.ends_with(".bin") || name.ends_with(".json");
        if entry.file_type()?.is_dir() && name == "locales" {
            copy_tree(&entry.path(), &dir.join("locales"))?;
        } else if wanted && name != "archive.json" {
            let dst = dir.join(&name);
            let same = std::fs::metadata(&dst).ok().zip(entry.metadata().ok()).is_some_and(|(d, s)| d.len() == s.len());
            if !same {
                std::fs::copy(entry.path(), dst)?;
            }
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("modules") => modules(),
        Some("bundle") => bundle(std::env::args().any(|a| a == "--release")),
        Some("dev") => dev(std::env::args().any(|a| a == "--release")),
        Some("virtual-camera") => virtual_camera(std::env::args().any(|a| a == "--uninstall")),
        Some("test") => test(std::env::args().skip(2).collect()),
        Some("check") => check(std::env::args().any(|a| a == "--changed")),
        Some("precommit") => precommit(),
        #[cfg(target_os = "macos")]
        Some("dist") => dist::dist(),
        #[cfg(windows)]
        Some("dist") => dist_windows(),
        #[cfg(not(any(target_os = "macos", windows)))]
        Some("dist") => anyhow::bail!("the shipped app is built on macOS and Windows"),
        Some("rules-key") => rules::key(),
        Some("update-key") => update::key(),
        Some("update-manifest") => {
            let a: Vec<String> = std::env::args().skip(2).collect();
            anyhow::ensure!(a.len() >= 2, "usage: cargo xtask update-manifest <version> <folder> [notes]");
            update::manifest(&a[0], &a[1], a.get(2).map(String::as_str))
        }
        Some("ci-secrets") => update::ci_secrets(),
        Some("rules-sign") => rules::sign(&std::env::args().nth(2).ok_or_else(|| anyhow::anyhow!("usage: cargo xtask rules-sign <rules.json>"))?),
        Some("prune") => {
            be_polite();
            let (files, bytes) = prune(None)?;
            eprintln!("deleted {files} unused object files ({:.0} MB)", bytes as f64 / 1e6);
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: cargo xtask (modules | bundle | dist | dev | virtual-camera [--uninstall] | test [--machine] [filter] | check [--changed] | precommit | prune | rules-key | rules-sign <file> | update-key | update-manifest <version> <folder> [notes] | ci-secrets)"
            );
            Ok(())
        }
    }
}

fn cargo() -> Command {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root());
    cmd
}

/// Workspace packages: name, folder (relative to the root), and the workspace packages each one
/// uses (dev-dependencies too: `--all-targets` builds the tests).
fn packages() -> anyhow::Result<Vec<(String, PathBuf, Vec<String>)>> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps", "--offline"]).output()?;
    anyhow::ensure!(out.status.success(), "cargo metadata failed");
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let root = root();
    Ok(meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap_or_default());
            let dir = manifest.parent().unwrap_or(&root).strip_prefix(&root).unwrap_or(Path::new("")).to_path_buf();
            let uses = p["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| d["path"].is_string())
                .filter_map(|d| d["name"].as_str().map(str::to_string))
                .collect();
            (p["name"].as_str().unwrap_or_default().to_string(), dir, uses)
        })
        .collect())
}

/// The packages changed since the last commit (staged, unstaged or new files), and every package
/// that uses one of them; `None` when a change touches the whole workspace (Cargo.toml at the
/// root, Cargo.lock, .cargo, the toolchain).
fn changed_packages() -> anyhow::Result<Option<Vec<String>>> {
    let out = Command::new("git").current_dir(root()).args(["status", "--porcelain", "-uall"]).output()?;
    anyhow::ensure!(out.status.success(), "git status failed");
    let paths: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.get(3..))
        // a rename: "old -> new"; both changed
        .flat_map(|p| p.split(" -> ").map(|p| PathBuf::from(p.trim_matches('"'))).collect::<Vec<_>>())
        .collect();
    let packages = packages()?;
    let mut changed: Vec<String> = Vec::new();
    for path in &paths {
        // the deepest package folder holding it (the workspace root is "")
        let owner = packages
            .iter()
            .filter(|(_, dir, _)| !dir.as_os_str().is_empty() && path.starts_with(dir))
            .max_by_key(|(_, dir, _)| dir.components().count());
        match owner {
            Some((name, ..)) => changed.push(name.clone()),
            None if path.starts_with("docs") || path.extension().is_some_and(|e| e == "md") => {}
            None if path.starts_with("modules") || path.starts_with("branding") || path.starts_with("screenshots") => {}
            None => return Ok(None),
        }
    }
    // and whatever uses them, until nothing new is added
    loop {
        let more: Vec<String> = packages
            .iter()
            .filter(|(name, _, uses)| !changed.contains(name) && uses.iter().any(|u| changed.contains(u)))
            .map(|(name, ..)| name.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        changed.extend(more);
    }
    changed.sort();
    changed.dedup();
    Ok(Some(changed))
}

/// The inner loop: `cargo check` (all targets, so tests too) for this Mac; clippy and Windows
/// are for `precommit`. The whole workspace by default: crates that didn't change cost almost
/// nothing to check, while checking a subset (`--changed`) unifies dependencies' features over
/// that subset only, so each new subset first rebuilds some dependencies (docs/build-speed.md).
fn check(changed: bool) -> anyhow::Result<()> {
    be_polite();
    // Plain `cargo build` and `cargo test` leave object files behind too: past 50,000 files in
    // the folder (a few seconds per crate again), clear them first.
    let crowded = std::fs::read_dir(root().join("target/debug/deps")).map_or(0, |d| d.count()) > 50_000;
    if crowded {
        let (files, bytes) = prune(None)?;
        eprintln!("deleted {files} unused object files ({:.0} MB)", bytes as f64 / 1e6);
    }
    let mut cmd = cargo();
    cmd.args(["check", "--all-targets"]);
    match if changed { changed_packages()? } else { None } {
        None => {
            cmd.arg("--workspace");
        }
        Some(packages) if packages.is_empty() => {
            eprintln!("nothing changed since the last commit (`cargo xtask check` checks everything)");
            return Ok(());
        }
        Some(packages) => {
            for p in packages {
                cmd.args(["-p", &p]);
            }
        }
    }
    run(&mut cmd)
}

/// Runs clippy for the workspace and all targets, counting warnings (cached results are
/// replayed, so a clean build with nothing to redo still reports them).
fn clippy(target: Option<&str>) -> anyhow::Result<usize> {
    use std::io::BufRead;
    let mut cmd = cargo();
    cmd.args(["clippy", "--workspace", "--all-targets", "--message-format=json-render-diagnostics"]);
    if let Some(t) = target {
        cmd.args(["--target", t]);
        // checking Windows from a Mac: lightair-web leaves Chromium out (Windows CEF can't be
        // built here; on Windows itself it's built as on macOS)
        if t.contains("windows") && !cfg!(windows) {
            cmd.env("RUSTFLAGS", "--cfg lightair_cross_check");
        }
        // gpui's build script compiles a Windows resource file with llvm-rc; Homebrew's LLVM
        // keeps it off PATH.
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        if !dirs.iter().any(|d| d.join("llvm-rc").exists())
            && let Some(llvm) = ["/opt/homebrew/opt/llvm/bin", "/usr/local/opt/llvm/bin"]
                .map(PathBuf::from)
                .into_iter()
                .find(|d| d.join("llvm-rc").exists())
        {
            dirs.push(llvm);
            cmd.env("PATH", std::env::join_paths(dirs)?);
        }
    }
    eprintln!(
        "$ cargo clippy --workspace --all-targets{}",
        target.map(|t| format!(" --target {t}")).unwrap_or_default()
    );
    let mut child = cmd.stdout(std::process::Stdio::piped()).spawn()?;
    let mut warnings = 0;
    for line in std::io::BufReader::new(child.stdout.take().unwrap()).lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line?) else { continue };
        if msg["reason"] == "compiler-message" && msg["message"]["level"] == "warning" {
            // rustc's own "N warnings emitted" summary has no code and no spans
            if msg["message"]["spans"].as_array().is_some_and(|s| !s.is_empty()) {
                warnings += 1;
            }
        }
    }
    anyhow::ensure!(child.wait()?.success(), "clippy failed");
    Ok(warnings)
}

/// What a commit needs: clippy clean for macOS and for Windows, and every test passing.
fn precommit() -> anyhow::Result<()> {
    be_polite();
    let started = std::time::Instant::now();
    let mac = clippy(None)?;
    let mac_took = started.elapsed();
    let windows = clippy(Some("x86_64-pc-windows-msvc"))?;
    let windows_took = started.elapsed() - mac_took;
    eprintln!(
        "clippy: {mac} warning(s) for macOS ({:.1}s), {windows} for Windows ({:.1}s)",
        mac_took.as_secs_f64(),
        windows_took.as_secs_f64()
    );
    anyhow::ensure!(mac + windows == 0, "clippy warnings");
    // the Stream Deck plugin, between a stand-in Stream Deck and Lightair (needs Node)
    if Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success()) {
        run(Command::new("node").arg(root().join("streamdeck/test/run.js")))?;
    } else {
        eprintln!("skipped the Stream Deck plugin's test: no node");
    }
    test(Vec::new())
}

/// Object files are left in target/debug/deps by every incremental build of a test program or
/// binary (macOS keeps them for their debug info, under new names each build), and nothing
/// deletes the old ones. rustc lists that folder each time it starts, so a few hundred thousand
/// of them cost seconds per crate. This deletes those no program there refers to; with
/// `programs`, only the leftovers of those programs (just built). Returns (files, bytes).
fn prune(programs: Option<&[PathBuf]>) -> anyhow::Result<(usize, u64)> {
    if !cfg!(target_os = "macos") {
        return Ok((0, 0));
    }
    let deps = root().join("target/debug/deps");
    let mut objects: Vec<String> = Vec::new();
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&deps)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.ends_with(".rcgu.o") {
            objects.push(name);
        } else if !name.contains('.') && name.rsplit_once('-').is_some_and(|(_, h)| h.len() == 16) {
            found.push(deps.join(&name));
        }
    }
    // a program's own objects are named after it: `name-hash.<unit>.<build>.rcgu.o`
    let only: Option<Vec<String>> = programs.map(|programs| {
        programs.iter().filter_map(|p| Some(format!("{}.", p.file_name()?.to_string_lossy()))).collect()
    });
    let programs = programs.map(<[PathBuf]>::to_vec).unwrap_or(found);
    let mut used = std::collections::HashSet::new();
    let lists = std::thread::scope(|s| {
        let jobs: Vec<_> = programs
            .chunks(programs.len().div_ceil(8).max(1))
            .map(|chunk| {
                s.spawn(move || {
                    let mut names = Vec::new();
                    for p in chunk {
                        let Ok(out) = Command::new("nm").arg("-ap").arg(p).output() else { continue };
                        for line in String::from_utf8_lossy(&out.stdout).lines() {
                            if let Some((_, path)) = line.split_once(" OSO ") {
                                names.push(path.rsplit('/').next().unwrap_or(path).to_string());
                            }
                        }
                    }
                    names
                })
            })
            .collect();
        jobs.into_iter().flat_map(|j| j.join().unwrap_or_default()).collect::<Vec<_>>()
    });
    used.extend(lists);
    let (mut files, mut bytes) = (0, 0);
    for name in objects {
        let theirs = only.as_ref().is_none_or(|only| only.iter().any(|p| name.starts_with(p.as_str())));
        if used.contains(&name) || !theirs {
            continue;
        }
        let path = deps.join(&name);
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        if std::fs::remove_file(&path).is_ok() {
            files += 1;
            bytes += size;
        }
    }
    Ok((files, bytes))
}

/// The SHA-1 of the first valid "Apple Development" code signing identity, if any.
/// The team of the keychain's Apple Development certificate (its subject's OU).
fn development_team() -> Option<String> {
    let pem = Command::new("security").args(["find-certificate", "-c", "Apple Development", "-p"]).output().ok()?;
    let mut x509 = Command::new("openssl")
        .args(["x509", "-noout", "-subject"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    use std::io::Write;
    x509.stdin.take()?.write_all(&pem.stdout).ok()?;
    let out = x509.wait_with_output().ok()?;
    let subject = String::from_utf8_lossy(&out.stdout).into_owned();
    let ou = subject.split(['/', ',']).map(str::trim).find_map(|p| p.strip_prefix("OU=").or_else(|| p.strip_prefix("OU = ")))?;
    let team: String = ou.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    (team.len() == 10).then_some(team)
}

/// "Lightair Virtual Camera" (modules/lightair-virtual-camera): XcodeGen makes the Xcode project
/// in target/virtual-camera, xcodebuild builds and signs it for the team (automatic signing makes
/// the provisioning profiles the extension needs), and the installer goes to /Applications, the
/// only place macOS installs camera extensions from, then opens (with `--uninstall`, to remove
/// the camera). macOS asks the user to allow the extension in System Settings the first time.
fn virtual_camera(uninstall: bool) -> anyhow::Result<()> {
    anyhow::ensure!(cfg!(target_os = "macos"), "the virtual camera extension is for macOS");
    let root = root();
    let team = std::env::var("LIGHTAIR_TEAM").ok().or_else(development_team).ok_or_else(|| {
        anyhow::anyhow!("no Apple Development certificate in the keychain (or set LIGHTAIR_TEAM)")
    })?;
    let out = root.join("target/virtual-camera");
    std::fs::create_dir_all(&out)?;
    run(Command::new("xcodegen")
        .args(["generate", "--quiet", "--spec"])
        .arg(root.join("modules/lightair-virtual-camera/project.yml"))
        .arg("--project")
        .arg(&out)
        .arg("--project-root")
        .arg(root.join("modules/lightair-virtual-camera")))?;
    run(Command::new("xcodebuild")
        .arg("-project")
        .arg(out.join("LightairVirtualCamera.xcodeproj"))
        .args(["-scheme", "Installer", "-configuration", "Release", "-quiet", "-allowProvisioningUpdates", "-derivedDataPath"])
        .arg(out.join("build"))
        .arg(format!("DEVELOPMENT_TEAM={team}"))
        .arg(format!("LIGHTAIR_VC_SRC={}", root.join("modules/lightair-virtual-camera").display()))
        .arg("build"))?;
    let built = out.join("build/Build/Products/Release/Lightair Virtual Camera.app");
    let installed = PathBuf::from("/Applications/Lightair Virtual Camera.app");
    if installed.exists() {
        std::fs::remove_dir_all(&installed)?;
    }
    run(Command::new("ditto").arg(&built).arg(&installed))?;
    let mut open = Command::new("open");
    open.arg("-n").arg(&installed);
    if uninstall {
        open.args(["--args", "--uninstall"]);
    }
    run(&mut open)?;
    eprintln!("opened {}", installed.display());
    Ok(())
}

fn apple_development_identity() -> Option<String> {
    let out = Command::new("security").args(["find-identity", "-v", "-p", "codesigning"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.contains("\"Apple Development:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .filter(|h| h.len() == 40)
        .map(str::to_string)
}

/// A test program to run: its name, path, package directory (cargo runs tests from there), and
/// arguments of its own (one test of a program whose tests each run in a process of their own).
struct TestBin {
    name: String,
    exe: PathBuf,
    dir: PathBuf,
    args: Vec<String>,
}

/// Ignored tests that measure rather than test (run by hand, optimised): `--machine` leaves
/// them out.
const MEASUREMENTS: &[&str] = &["podcast_graph_cost", "frames_per_second", "swizzle_cost"];

/// Test programs whose tests start libobs (process-global): each test runs as a process of its
/// own (lightair_engine::testing).
const OWN_PROCESS: &[&str] = &["lightair-engine/engine", "lightair-preview/preview"];

/// `cargo test` runs test programs one after another, and most of ours start libobs and record
/// real seconds of video, so they spend their time waiting. This builds them all, then runs them
/// side by side, slowest first (timings from the last run), so the whole suite takes about as
/// long as its slowest program. Arguments after `test` are passed to every program (a filter,
/// `--nocapture`, ...). `--machine` also runs the tests that depend on this Mac (its audio
/// devices, its settings), which are ignored otherwise. Doc tests aren't run; `cargo test --doc`
/// does those.
fn test(args: Vec<String>) -> anyhow::Result<()> {
    use std::io::BufRead;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let started = Instant::now();
    let machine = args.iter().any(|a| a == "--machine");
    let mut args: Vec<String> = args.into_iter().filter(|a| a != "--machine").collect();
    // the filter (the first argument that isn't a flag)
    let filter = args.iter().position(|a| !a.starts_with('-')).map(|i| args.remove(i));
    if machine {
        args.push("--include-ignored".into());
        for m in MEASUREMENTS {
            args.extend(["--skip".into(), m.to_string()]);
        }
    }
    let flags = args.clone();
    args.extend(filter.clone());
    // Polite: the build and the tests run at a lower priority, so the Mac stays responsive
    // (typing elsewhere keeps up) while they run.
    be_polite();
    let mut build = cargo()
        .args(["test", "--workspace", "--no-run", "--message-format=json-render-diagnostics"])
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut bins = Vec::new();
    for line in std::io::BufReader::new(build.stdout.take().unwrap()).lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line?) else { continue };
        if msg["reason"] != "compiler-artifact" || msg["profile"]["test"] != true {
            continue;
        }
        let Some(exe) = msg["executable"].as_str() else { continue };
        let manifest = PathBuf::from(msg["manifest_path"].as_str().unwrap_or_default());
        let package = manifest.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned());
        let target = msg["target"]["name"].as_str().unwrap_or_default();
        let kind = msg["target"]["kind"][0].as_str().unwrap_or_default();
        let name = match kind {
            "test" => format!("{}/{target}", package.unwrap_or_default()),
            _ => format!("{} ({kind})", package.unwrap_or_default()),
        };
        bins.push(TestBin { name, exe: exe.into(), dir: manifest.parent().unwrap().to_path_buf(), args: Vec::new() });
    }
    anyhow::ensure!(build.wait()?.success(), "build failed");
    let built = started.elapsed();
    let built_programs: Vec<PathBuf> = bins.iter().map(|b| b.exe.clone()).collect();
    let pruning = std::thread::spawn(move || prune(Some(&built_programs)));

    // One test per process where libobs starts: the filter picks tests here, as the program
    // would.
    let mut expanded = Vec::new();
    for bin in bins {
        if !OWN_PROCESS.contains(&bin.name.as_str()) {
            expanded.push(bin);
            continue;
        }
        let out = Command::new(&bin.exe).args(["--list", "--format", "terse"]).current_dir(&bin.dir).output()?;
        anyhow::ensure!(out.status.success(), "{} --list failed", bin.name);
        for test in String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.strip_suffix(": test")) {
            if filter.as_deref().is_some_and(|f| !test.contains(f)) {
                continue;
            }
            let mut args = vec![test.to_string(), "--exact".into()];
            args.extend(flags.iter().cloned());
            expanded.push(TestBin {
                name: format!("{}/{test}", bin.name.split('/').next().unwrap_or_default()),
                exe: bin.exe.clone(),
                dir: bin.dir.clone(),
                args,
            });
        }
    }
    let mut bins = expanded;

    // Slowest first, so the long ones aren't left starting last.
    let times_path = root().join("target/test-times.json");
    let mut times: serde_json::Map<String, serde_json::Value> =
        std::fs::read_to_string(&times_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    bins.sort_by(|a, b| {
        let t = |n: &str| times.get(n).and_then(|v| v.as_f64()).unwrap_or(f64::MAX);
        t(&b.name).total_cmp(&t(&a.name))
    });

    // Each program runs from a folder of its own (a hard link, no copy). macOS treats a bare
    // program's folder as its bundle and lists it while libobs starts, and target/debug/deps
    // holds thousands of files (more when builds' leftovers pile up), which costs seconds per
    // program. The recording tests need OBS's mux helper next to the program.
    let run_dir = root().join("target/test-run");
    let mux = obs_mux();
    let mut linked_to = std::collections::HashMap::new();
    for bin in &mut bins {
        if let Some(linked) = linked_to.get(&bin.exe) {
            bin.exe = PathBuf::clone(linked);
            continue;
        }
        let file = bin.exe.file_name().unwrap().to_owned();
        let dir = run_dir.join(&file);
        std::fs::create_dir_all(&dir)?;
        let linked = dir.join(&file);
        let _ = std::fs::remove_file(&linked);
        if std::fs::hard_link(&bin.exe, &linked).is_err() {
            std::fs::copy(&bin.exe, &linked)?;
        }
        let helper = dir.join(mux.file_name().unwrap());
        if !helper.exists() && mux.exists() {
            let _ = link_file(&mux, &helper);
        }
        linked_to.insert(std::mem::replace(&mut bin.exe, linked.clone()), linked);
    }

    // half the cores: each program runs libobs and real-time audio threads of its own
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8).div_ceil(2).max(2);
    let results = Arc::new(Mutex::new(Vec::new()));
    run_tests(bins, jobs, &args, &results);
    if let Ok(Ok((files, bytes))) = pruning.join()
        && files > 0
    {
        eprintln!("deleted {files} object files earlier builds left ({:.0} MB)", bytes as f64 / 1e6);
    }

    let results = std::mem::take(&mut *results.lock().unwrap());
    for (name, _, took, _, _) in &results {
        times.insert(name.clone(), serde_json::json!(took.as_secs_f64()));
    }
    let _ = std::fs::write(&times_path, serde_json::to_string_pretty(&times)?);
    let failed: Vec<_> = results.iter().filter(|r| !r.1).collect();
    for (name, _, _, _, log) in &failed {
        eprintln!("\n──── {name} ────\n{log}");
    }
    let passed: usize = results.iter().map(|r| r.3).sum();
    let serial: Duration = results.iter().map(|r| r.2).sum();
    eprintln!(
        "\n{} processes, {passed} tests, {} failed · build {:.1}s, run {:.1}s (one after another: {:.1}s)",
        results.len(),
        failed.len(),
        built.as_secs_f64(),
        (started.elapsed() - built).as_secs_f64(),
        serial.as_secs_f64(),
    );
    anyhow::ensure!(failed.is_empty(), "{} test program(s) failed", failed.len());
    Ok(())
}

/// A test program's result: name, passed, time, tests passed, output.
type TestResult = (String, bool, std::time::Duration, usize, String);

/// Runs `bins` on `jobs` threads, adding to `results`.
fn run_tests(
    bins: Vec<TestBin>,
    jobs: usize,
    args: &[String],
    results: &std::sync::Arc<std::sync::Mutex<Vec<TestResult>>>,
) {
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    let queue = Arc::new(Mutex::new(bins.into_iter().collect::<std::collections::VecDeque<_>>()));
    let workers: Vec<_> = (0..jobs)
        .map(|_| {
            let (queue, results, args) = (queue.clone(), results.clone(), args.to_vec());
            std::thread::spawn(move || {
                loop {
                    // Take the next one and let go of the lock before running it.
                    let next = queue.lock().unwrap().pop_front();
                    let Some(bin) = next else { break };
                    let t = Instant::now();
                    let mut cmd = Command::new(&bin.exe);
                    // Windows finds obs.dll (and libobs its data) through PATH
                    if cfg!(windows) {
                        let path = std::env::var_os("PATH").unwrap_or_default();
                        let mut dirs = vec![obs_bin()];
                        dirs.extend(std::env::split_paths(&path));
                        if let Ok(joined) = std::env::join_paths(dirs) {
                            cmd.env("PATH", joined);
                        }
                    }
                    let own_process = !bin.args.is_empty();
                    let out = cmd
                        .args(if own_process { &bin.args } else { &args })
                        .envs(own_process.then_some(("LIGHTAIR_TEST_OWN_PROCESS", "1")))
                        .current_dir(&bin.dir)
                        .env("CARGO_MANIFEST_DIR", &bin.dir)
                        // Tests that measure frame timing skip that when sharing the GPU.
                        .envs((jobs > 1).then_some(("LIGHTAIR_TESTS_SHARE_GPU", "1")))
                        // Camera and capture-card modules wait seconds on system services as
                        // they load; tests don't use them (the smoke test asks for every module).
                        .env(
                            "LIGHTAIR_SKIP_MODULES",
                            std::env::var("LIGHTAIR_SKIP_MODULES").unwrap_or_else(|_| {
                                "mac-avcapture,mac-avcapture-legacy,mac-syphon,aja,decklink,vlc-video".into()
                            }),
                        )
                        .output();
                    let took = t.elapsed();
                    let (ok, log) = match out {
                        Ok(o) => (
                            o.status.success(),
                            format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
                        ),
                        Err(e) => (false, e.to_string()),
                    };
                    let passed: usize = log
                        .lines()
                        .filter_map(|l| l.strip_prefix("test result: "))
                        .filter_map(|l| l.split("; ").next()?.split_whitespace().nth(1)?.parse::<usize>().ok())
                        .sum();
                    // a test of its own that didn't run (a name that no longer matches) isn't a pass
                    let ok = ok && (!own_process || passed == 1 || log.contains(" 1 ignored"));
                    eprintln!(
                        "{} {:>6.1}s  {} ({passed})",
                        if ok { "ok  " } else { "FAIL" },
                        took.as_secs_f64(),
                        bin.name
                    );
                    results.lock().unwrap().push((bin.name, ok, took, passed, log));
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}

#[cfg(unix)]
unsafe extern "C" {
    /// POSIX: (PRIO_PROCESS = 0, this process = 0, niceness). Children inherit it.
    fn setpriority(which: i32, who: u32, prio: i32) -> i32;
}
