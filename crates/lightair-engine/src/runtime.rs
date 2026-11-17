//! Where the engine's binaries live: libobs, graphics backends and OBS plugins.
//!
//! The location is discovered from the libobs image that is actually loaded, so the same code
//! works inside `Lightair.app` (Contents/Frameworks + Contents/PlugIns) and in development,
//! where libobs comes from an installed OBS.

#[cfg(not(windows))]
use std::ffi::CStr;
use std::ffi::c_void;
use std::path::{Path, PathBuf};

use libobs_sys as sys;

#[derive(Debug, Clone)]
pub struct Runtime {
    /// Directory holding libobs.framework and the graphics backends.
    pub frameworks: PathBuf,
    /// Directory holding `<module>.plugin` bundles.
    pub plugins: PathBuf,
    /// True when the running executable is inside the same bundle as libobs.
    pub in_bundle: bool,
}

impl Runtime {
    #[cfg(target_os = "macos")]
    pub fn detect() -> Result<Self, String> {
        let lib = loaded_libobs_path().ok_or("could not locate the loaded libobs image")?;
        let frameworks = lib
            .ancestors()
            .find(|p| p.file_name().is_some_and(|n| n == "libobs.framework"))
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("libobs is not inside a libobs.framework: {}", lib.display()))?;
        let contents = frameworks.parent().unwrap_or(&frameworks).to_path_buf();
        let plugins = contents.join("PlugIns");
        let in_bundle = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .is_some_and(|dir| dir == contents.join("MacOS"));
        Ok(Self { frameworks, plugins, in_bundle })
    }

    /// Windows: OBS's layout, `bin/64bit/obs.dll` (and the graphics backends beside it),
    /// `core/<module>/<module>.dll` with `core/<module>/data/` (OBS 33's layout; plugins of
    /// the older flat layout are loaded from `obs-plugins/64bit`, `module_patterns`).
    ///
    /// libobs loads its graphics backend, `libobs-winrt` and each module's dependencies from
    /// the default DLL folders only (not `PATH`), so libobs's folder is added to them: needed
    /// where libobs isn't beside the executable (development, tests).
    #[cfg(windows)]
    pub fn detect() -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        let lib = loaded_libobs_path().ok_or("could not locate the loaded libobs image")?;
        let frameworks = lib.parent().map(Path::to_path_buf).ok_or("libobs has no folder")?;
        let root = frameworks.parent().and_then(Path::parent).unwrap_or(&frameworks).to_path_buf();
        let plugins = root.join("core");
        let in_bundle = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .is_some_and(|dir| dir == frameworks);
        static ADDED: std::sync::Once = std::sync::Once::new();
        ADDED.call_once(|| {
            let wide: Vec<u16> = frameworks.as_os_str().encode_wide().chain([0]).collect();
            // SAFETY: a NUL-terminated path; the cookie is kept for the process's life.
            unsafe { windows_sys::Win32::System::LibraryLoader::AddDllDirectory(wide.as_ptr()) };
        });
        Ok(Self { frameworks, plugins, in_bundle })
    }

    /// libobs's own data (its effects): Windows, `data\libobs` in OBS's layout; macOS finds it
    /// in libobs.framework itself.
    pub fn libobs_data(&self) -> Option<PathBuf> {
        if !cfg!(windows) {
            return None;
        }
        let root = self.frameworks.parent().and_then(Path::parent)?;
        Some(root.join("data").join("libobs")).filter(|d| d.join("default.effect").exists())
    }

    /// The graphics backend's library: Metal or OpenGL on macOS, Direct3D 11 on Windows.
    pub fn graphics_module(&self, graphics: crate::engine::Graphics) -> PathBuf {
        use crate::engine::Graphics;
        self.frameworks.join(match graphics {
            Graphics::Metal => "libobs-metal.dylib",
            Graphics::OpenGl => "libobs-opengl.dylib",
            Graphics::D3D11 => "libobs-d3d11.dll",
        })
    }

    /// `obs_load_plugins` path patterns for the modules in `dir`: where the binary and the data
    /// of `%module%` are.
    pub fn module_patterns(&self, dir: &Path) -> (String, String) {
        if cfg!(windows) {
            // forward slashes, which libobs splits the module's name off by
            let slashes = |p: &Path| p.display().to_string().replace('\\', "/");
            if dir.file_name().is_some_and(|n| n == "64bit") {
                // the flat layout: `<dir>/<module>.dll`, data in `<root>/data/obs-plugins/`
                // (the folder alone: libobs then looks for `*.dll` in it)
                let root = dir.parent().and_then(Path::parent).unwrap_or(dir);
                (slashes(dir), format!("{}/data/obs-plugins/%module%/", slashes(root)))
            } else {
                // a folder per module: `<dir>/<module>/<module>.dll`, `<dir>/<module>/data/`
                (format!("{}/%module%/", slashes(dir)), format!("{}/%module%/data/", slashes(dir)))
            }
        } else {
            (
                format!("{}/%module%.plugin/Contents/MacOS/", dir.display()),
                format!("{}/%module%.plugin/Contents/Resources/", dir.display()),
            )
        }
    }
}

#[cfg(not(windows))]
fn loaded_libobs_path() -> Option<PathBuf> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    let addr = sys::obs_startup as *const c_void;
    // SAFETY: dladdr only reads the address and fills `info`.
    if unsafe { libc::dladdr(addr, &mut info) } == 0 || info.dli_fname.is_null() {
        return None;
    }
    let path = unsafe { CStr::from_ptr(info.dli_fname) };
    Some(PathBuf::from(path.to_string_lossy().into_owned()))
}

#[cfg(windows)]
fn loaded_libobs_path() -> Option<PathBuf> {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };
    let addr = sys::obs_startup as *const c_void;
    let mut module = std::ptr::null_mut();
    // SAFETY: looks up which loaded module holds `addr`, then its file name.
    unsafe {
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            addr as *const u16,
            &mut module,
        ) == 0
        {
            return None;
        }
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize;
        (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}
