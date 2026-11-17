//! Generates bindings for the pinned libobs (third_party/obs-studio) and links libobs.
//!
//! Linking: development builds link the `libobs.framework` of an installed OBS of the same
//! version (OBS_APP, default /Applications/OBS.app). Release builds will link the libobs built
//! from the submodule (IMPLEMENTATION.md, M13).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = plain(manifest.join("../..").canonicalize().expect("workspace root"));
    let libobs = root.join("third_party/obs-studio/libobs");
    let simde = root.join("third_party/simde");
    assert!(libobs.join("obs.h").exists(), "libobs headers missing: run `git submodule update --init --depth 1`");

    // libobs.h includes a CMake-generated obsconfig.h; provide the release values.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let include_dir = out.join("include");
    fs::create_dir_all(&include_dir).unwrap();
    // Written only when it differs: bindgen asks cargo to watch every header it reads, so a fresh
    // timestamp here would rebuild this crate and everything above it on every build.
    let config = include_dir.join("obsconfig.h");
    let contents = "#pragma once\n#define OBS_RELEASE_CANDIDATE 0\n#define OBS_BETA 0\n";
    if fs::read_to_string(&config).ok().as_deref() != Some(contents) {
        fs::write(&config, contents).unwrap();
    }

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-env-changed=OBS_APP");
    println!("cargo:rerun-if-env-changed=OBS_DIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Checking the Windows build from a Mac: there are no Windows SDK headers here, so the
    // headers are read as arm64 macOS (a `char*` va_list, as on Windows). Good enough to
    // type-check Windows code (`cargo check`); never for a real Windows build, which runs bindgen
    // natively (MSVC, LLVM).
    let cross_check = target_os == "windows" && !cfg!(windows);
    let mut builder = bindgen::Builder::default();
    if target_os == "windows" && !cross_check {
        // util_uint64.h's inline helper calls MSVC's `_udiv128`, which clang's own intrin.h
        // doesn't declare; only declarations are read, so the body needn't compile.
        builder = builder.clang_arg("-Wno-error=implicit-function-declaration").clang_arg("-Wno-implicit-function-declaration");
    }
    if cross_check {
        println!("cargo:warning=libobs bindings read as arm64 macOS: for checking the Windows build only");
        builder = builder.clang_arg("--target=arm64-apple-darwin");
    }

    let bindings = builder
        .header(manifest.join("wrapper.h").to_string_lossy())
        .clang_arg(format!("-I{}", libobs.display()))
        .clang_arg(format!("-I{}", include_dir.display()))
        .clang_arg(format!("-I{}", simde.display()))
        .allowlist_function("(obs|gs|os|calldata|signal_handler|proc_handler|video_output|audio_output|bmalloc|bfree|bzalloc|bstrdup|blog|base_set_log_handler|base_get_log_handler|profiler|config|vec4)_.*")
        .allowlist_function("blog|bfree|bmalloc|bstrdup|base_set_log_handler|base_get_log_handler|bnum_allocs")
        .allowlist_type("(obs|gs|video|audio|speaker)_.*")
        .allowlist_type("(calldata|signal_handler|proc_handler|profiler_name_store|config)_t")
        .allowlist_var("(OBS|GS|LOG|MODULE|LIBOBS|VIDEO|AUDIO|MAX)_.*")
        .default_enum_style(bindgen::EnumVariation::Consts)
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate_comments(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed for libobs");
    let mut code = bindings.to_string();
    if target_os == "windows" && !cross_check {
        code = unsigned_enums(&code);
        // libobs's functions imported from obs.dll by name, without an import library: any
        // OBS install (or the submodule's build) of the pinned version links as it is.
        code = code.replace("unsafe extern \"C\" {", "#[link(name = \"obs\", kind = \"raw-dylib\")]\nunsafe extern \"C\" {");
    }
    if cross_check {
        // libobs's Windows-only functions (inside `#ifdef _WIN32`, unseen when the headers are
        // read as macOS) that Lightair's Windows code calls
        code.push_str(
            "\nunsafe extern \"C\" {\n    pub fn gs_texture_open_nt_shared(handle: u32) -> *mut gs_texture_t;\n}\n",
        );
    }
    if fs::read_to_string(out.join("bindings.rs")).ok().as_deref() != Some(code.as_str()) {
        fs::write(out.join("bindings.rs"), code).expect("write bindings");
    }

    match target_os.as_str() {
        "macos" => {
            let obs_app = env::var("OBS_APP").unwrap_or_else(|_| "/Applications/OBS.app".into());
            println!("cargo:rustc-link-search=framework={obs_app}/Contents/Frameworks");
            println!("cargo:rustc-link-lib=framework=libobs");
            println!("cargo:obs_app={obs_app}");
        }
        // Windows: obs.dll is imported directly (raw-dylib, above); OBS_DIR names the OBS build
        // or install it runs from (`bin\64bit`), for the tools that need its folder.
        "windows" if !cross_check => {
            let obs_dir = env::var("OBS_DIR").unwrap_or_else(|_| r"C:\Program Files\obs-studio".into());
            println!("cargo:obs_dir={obs_dir}");
        }
        _ => {}
    }
}

/// A path without Windows's `\\?\` prefix, which `canonicalize` adds and clang can't resolve
/// the headers' own includes against.
fn plain(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

/// Enums as macOS has them: in the Microsoft ABI every C enum is an `int`, where elsewhere one
/// without negative values is an `unsigned int`. The same Rust types on both platforms keep the
/// code that compares them with libobs's `uint32_t` fields and flags the same.
fn unsigned_enums(code: &str) -> String {
    use std::collections::{HashMap, HashSet};
    // every enum constant: `pub const NAME: TYPE = VALUE;`
    let mut negative: HashSet<&str> = HashSet::new();
    let mut seen: HashMap<&str, ()> = HashMap::new();
    for line in code.lines() {
        let Some(rest) = line.trim().strip_prefix("pub const ") else { continue };
        let Some((_, rest)) = rest.split_once(": ") else { continue };
        let Some((ty, value)) = rest.split_once(" = ") else { continue };
        if ty.contains(':') || ty.contains(' ') {
            continue;
        }
        seen.insert(ty, ());
        if value.trim_end_matches(';').trim().starts_with('-') {
            negative.insert(ty);
        }
    }
    code.lines()
        .map(|line| {
            if let Some(rest) = line.strip_prefix("pub type ")
                && let Some((name, ty)) = rest.split_once(" = ")
                && ty == "::std::os::raw::c_int;"
                && seen.contains_key(name)
                && !negative.contains(name)
            {
                format!("pub type {name} = ::std::os::raw::c_uint;")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
