# Windows: every Lightair feature on both platforms

Status (17 November 2026): **built, tested and run on Windows** (Windows 11, i5-9300H, GTX 1050 Ti
with Intel UHD 630, against an installed OBS 33.0.1): the whole suite passes there (316 tests),
the app runs, records and is benchmarked against OBS ([../benchmarks/lightair-vs-obs-windows.md](../benchmarks/lightair-vs-obs-windows.md)).
What the first real Windows run found and fixed is in [§6](#6-the-first-run-on-windows). Lightair is
designed for macOS and Windows (docs/15: Windows 10 22H2+ and 11, x64; Linux after Windows). **Every
feature is designed for both**: a new feature names its Windows path before it's built, and nothing
lands that would stop the Windows build.

Building on Windows (development): Rust (rustup reads `rust-toolchain.toml`), Visual Studio 2022
Build Tools (C++), LLVM (bindgen's libclang: `LIBCLANG_PATH`), CMake and Ninja (Chromium's
wrapper), and OBS Studio 33 installed (`OBS_DIR`, default `C:\Program Files\obs-studio`): libobs
is imported from its `obs.dll` directly (raw-dylib, no import library needed), and the Camera
Studio module makes its import libraries from the DLLs.

```
git submodule update --init --depth 1
cargo build -p lightair-app -p lightair-web-helper
cargo xtask modules          # Lightair Effect Stack (shape masks, 3D transforms, effects)
cargo xtask dev              # Chromium's runtime and OBS's helper programs beside the exe
cargo xtask test
```

Run `target\debug\lightair.exe` with OBS's `bin\64bit` on `PATH` (development only: the shipped
app carries libobs beside it, in OBS's own layout).

## 1. Principles

- **One product, two platforms.** The same features, the same project files, the same UI. Where an
  OS offers something the other doesn't, Lightair builds its own (an eyedropper on Windows) or
  says plainly that the feature is for one platform (Audio Units).
- **Projects are portable.** A project made on a Mac opens on Windows with its scenes, layouts,
  mixer and effects intact. Machine-specific choices (which camera, which display, which app) are
  re-picked, not broken.
- **The platform is behind one seam.** OS calls live in a `lightair-platform` crate with a macOS
  and a Windows backend. The rest of the code is cross-platform Rust, libobs and GPUI.
- **Same code, same tests.** The suite runs on both, in CI, from the first Windows phase.

## 2. Where Lightair is macOS-only today

| Area | Today (macOS) | Windows | Kind of work |
|---|---|---|---|
| **libobs build** | Links an installed `OBS.app`'s frameworks (`libobs-sys/build.rs`) | Build libobs from the pinned `third_party/obs-studio` (D3D11 backend) or link a matching OBS install's `obs.dll`; bindgen as now | Build |
| **Preview** | IOSurface → `CVPixelBuffer` → GPUI `surface()` (`lightair-preview/mac.rs`) | DXGI shared NT handles from libobs (D3D11) into GPUI's Windows renderer; GPUI's `surface()` is macOS-only, so Lightair adds a Windows backend (docs/03, docs/04) | New backend |
| **Project source kinds** | Projects store libobs ids: `screen_capture`, `sck_audio_capture`, `av_capture_input_v2`, `coreaudio_input_capture`, … | **Portable kinds** in the project (display, window, app audio, desktop audio, camera, mic, interface), mapped to libobs ids per platform when played: `monitor_capture`/`window_capture`/`game_capture`, `wasapi_output_capture`, `wasapi_process_output_capture`, `dshow_input`, `wasapi_input_capture`. Settings translated (display UUID ↔ monitor id, app bundle id ↔ executable). Existing projects migrated once | Model + engine |
| **Encoders** | VideoToolbox (H.264, HEVC, ProRes) | NVENC, AMF, QSV, x264 (libobs `obs-nvenc`, `obs-qsv11`, `win-mf`); ProRes via FFmpeg (software) | Engine settings |
| **Lightair Effect Stack** (`modules/lightair-effect-stack`) | Apple Vision / Core ML segmentation and tracking (`backend-vision.mm`, `segmenter-coreml.mm`) | The module already has ONNX backends (`backend-onnx.cpp`, `mediapipe.cpp`): ONNX Runtime with DirectML on Windows. The module's quality picker shows what each platform has | Build + test |
| **macOS camera effects panel** (`system_effects.rs`) | Portrait, Backgrounds, Studio Light | Windows Studio Effects where the camera and PC support them (blur, eye contact, framing); hidden otherwise | Platform |
| **Audio engine** | cpal on CoreAudio; real-time thread via Mach time-constraint policy (`audio_host.rs`, not guarded) | cpal on WASAPI; real-time thread via MMCSS ("Pro Audio"); guard the Mach code | Platform |
| **App and desktop audio** | ScreenCaptureKit audio (`sck_audio_capture`) | WASAPI process loopback (per app) and desktop loopback | Kinds (above) |
| **Running apps list** (source menu, `mixer.rs`) | NSWorkspace | Top-level windows' processes (for process loopback) | Platform |
| **Plugins** | Audio Units (AudioToolbox) and VST3 (CFBundle loading) | VST3 only: load the bundle's `Contents/x86_64-win/*.vst3` DLL (`InitDll`/`ExitDll`), editors attach to an `HWND`. Audio Units are macOS-only by nature; the Plugins page shows what the platform has | Platform |
| **Plugin editor windows** (`plugin_editor.rs`) | NSWindow | Win32 window hosting the VST3 view | Platform |
| **Main-thread dispatch** (`dispatch2` in `main.rs`) | GCD main queue | GPUI's foreground executor (cross-platform; replaces GCD on both) | Refactor |
| **Global hotkeys** (`hotkeys.rs`) | Carbon `RegisterEventHotKey` | `RegisterHotKey` | Platform |
| **Eyedropper** (`eyedropper.rs`) | NSColorSampler | Lightair's own: a full-screen capture with a magnifier loupe | New |
| **Stream keys** | Keychain (`keyring` with `apple-native`) | Credential Manager (`keyring` with `windows-native`) | Feature flag |
| **Paths** | `~/Library/Application Support/Lightair`, `~/Movies/Lightair` | `%APPDATA%\Lightair`, the user's Videos folder (Known Folders) | Platform |
| **Reveal, Trash** | Finder reveal, macOS Trash | Explorer `/select`, Recycle Bin | Platform |
| **Displays** (`displays.rs`, guarded) | CoreGraphics display UUIDs | DXGI outputs / monitor ids | Platform |
| **Virtual camera** (designed) | Camera Extension | DirectShow / Media Foundation virtual camera (docs/03) | Already planned |
| **Updates, signing** (designed) | Sparkle, notarisation | Signed feed updater, Authenticode, MSIX (docs/15) | Already planned |
| **Fonts, colours, UI** | GPUI | GPUI on Windows (Direct Write); check gpui-kit's Windows support at the pinned version | Verify |

Already cross-platform: the core model and commands, the store and journal, the audio graph and
DSP (`lightair-audio`), templates, the effect catalog and its libobs shaders (libobs compiles them
for D3D11), scenes, layers, transitions, formats and the mixer UI.

## 3. The `lightair-platform` crate

One module per concern, each with `macos.rs` and `windows.rs` behind `#[cfg]`, and the
same public functions:

- `paths`: config, projects, recordings, caches.
- `files`: reveal, move to trash, open with.
- `hotkeys`: global shortcuts.
- `credentials`: secrets.
- `apps`: running apps with windows and audio.
- `displays`: monitors with stable ids.
- `realtime`: real-time audio thread priority.
- `pick_color`: the eyedropper.
- `camera_effects`: the OS's own camera effects.
- `input`: pointer, clicks, keys, cursor shape (for the Screen Studio work).
- `plugins`: plugin folders and loaders.

The app and engine call these instead of OS APIs. A test per module runs on both platforms.

## 4. Phases

1. **It builds on Windows:**
   - guard every macOS call;
   - `lightair-platform` with paths, files, hotkeys, credentials, realtime, displays;
   - GPUI's foreground executor instead of GCD;
   - libobs: OBS 33.0.0's official Windows build (the submodule's headers for the bindings);
   - CI on `windows-latest` running `cargo xtask test` (tests that need macOS frameworks marked
     per platform).

   *Done when:* the Windows CI builds and runs the platform-neutral suite.

   Done: `lightair-platform` (paths, files, apps, realtime); hotkeys on `RegisterHotKey`;
   GPUI's foreground executor for plugin jobs; VST3 DLL loading and a Win32 editor window; a
   portable preview path (frames read back as BGRA, shown as images) until DXGI; the eyedropper
   hidden and camera effects stubbed until phase 3; Audio Unit tests marked macOS-only;
   `.github/workflows/ci.yml` (OBS 33.0.0's official build, clippy, `cargo xtask test`).
   Checking from a Mac: `PATH="/opt/homebrew/opt/llvm/bin:$PATH" cargo check --workspace
   --all-targets --target x86_64-pc-windows-msvc` (libobs-sys reads the headers as macOS's
   there and links nothing).
2. **It runs on Windows:**
   - DXGI preview into GPUI;
   - portable source kinds with migration;
   - capture (display, window, game), cameras, WASAPI mic, desktop and app audio;
   - NVENC/AMF/QSV;
   - audio engine on WASAPI.

   *Done when:* a project made on a Mac opens on Windows, plays, records and streams, and the
   screenshot hooks produce Windows screenshots.

   Done: portable kinds (`lightair_core::kinds`: screen, camera, mic, desktop audio, app
   audio) with their own setting names; the engine maps them per platform
   (`lightair_engine::kinds`) and gives an input a new source when an edit maps it to another
   libobs type; projects move to format 2 on open (and OBS imports arrive portable). Game
   capture is a fourth screen mode on Windows. Hardware encoders: NVENC, AMF and Quick Sync
   by the same choices as VideoToolbox (H.264, HEVC; ProRes stays macOS-only, and the choice
   says so). Audio: cpal opens WASAPI devices; libobs's monitoring is per platform already.

   Not done, and why: the zero-copy preview. GPUI's Windows renderer draws no surfaces
   (`draw_surfaces` is empty in gpui-pre-windows 0.3.12) and its `PaintSurface` carries only
   a `CVPixelBuffer`, so DXGI shared textures need a GPUI change (a Windows surface holding a
   shared NT handle, opened with `OpenSharedResource1` and drawn as a textured quad), made and
   measured on a Windows machine. Until then Windows previews through the read-back path:
   correct, one frame later, with a copy per frame. Running on Windows, recording, streaming
   and Windows screenshots also wait for a Windows machine or the CI runner.
3. **Parity:**
   - Lightair Effect Stack on ONNX/DirectML;
   - VST3 hosting with editors;
   - eyedropper;
   - Windows Studio Effects;
   - running apps;
   - virtual camera;
   - installer, signing, updates.

   *Done when:* every feature in docs/02's catalog is marked for both platforms or has a stated
   one-platform reason.

   Done: Lightair Effect Stack's Windows build (`cmake/windows.cmake`: the ONNX backends, ONNX Runtime
   and DirectML from Microsoft's NuGet packages, OBS's module layout; MediaPipe segmentation
   on Windows); VST3 editors in Win32 windows (phase 1); Lightair's own eyedropper
   (`lightair_platform::pick_color`: a snapshot of every display with a loupe); running apps
   (phase 1); docs/02 lists the one-platform features and why.

   Not done here: Windows Studio Effects (reading a camera's effect state needs its Media
   Foundation extended controls, and a camera that has them); the virtual camera, installer,
   signing and updates are not built on macOS either and come with docs/15's distribution work
   for both platforms at once.
4. **Ongoing:** every new feature (the Screen Studio plan first) is built with both backends, and
   CI gates on both.

## 5. Decisions

Recommended, to confirm as the work starts:

- **libobs on Windows:** OBS 33.0.0's official build, as on macOS (its OBS.app): the pinned
  submodule's headers make the bindings, unmodified, and the release bundles OBS's own binaries,
  so Lightair's users never need OBS. (CI built libobs from the submodule at first; switched on
  17 November 2026, it saved about an hour a run for the same version.)
- **Portable project kinds:** the project stores Lightair's own kinds, not libobs ids. This is the
  one change that touches the model, so it comes early (phase 2).
- **One-platform features** say so in the UI rather than disappearing: Audio Units (macOS); later,
  iPhone/iPad over USB (macOS; Android over USB on both, see the Screen Studio plan).
- **Testing on Windows:** CI runs the suite; screenshots come from a Windows machine or VM through
  the same hooks.

## 6. The first run on Windows

What it took to go from "compiles from a Mac" to running and passing on a Windows PC. Each is a
real bug a Windows user would have hit:

**Building**
- `canonicalize` gives `\\?\` paths on Windows, which clang can't resolve libobs's own includes
  against (`libobs-sys/build.rs`, `plain`).
- MSVC makes every C enum an `int` where macOS clang makes a non-negative one `unsigned`: the
  bindings are made unsigned as on macOS, so code comparing them with `uint32_t` fields is the same.
- libobs is imported from `obs.dll` by name (`raw-dylib`): an installed OBS ships no `obs.lib`.
  Lightair Effect Stack's CMake makes `obs.lib` and `w32-pthreads.lib` from the DLLs' exports.
- `vsnprintf` is inline in the UCRT's headers: linked from `legacy_stdio_definitions`.

**Starting**
- libobs looked for its effects and OBS's modules for their files relative to the working folder
  (OBS runs from `bin\64bit`): the engine registers libobs's data folder and works from libobs's
  folder (game capture's hooks, `..\..\core\win-capture\data`).
- OBS 33 moved its modules to `core\<module>\<module>.dll` with `core\<module>\data`.
- libobs loads `libobs-winrt` and modules' dependencies from the default DLL folders only
  (not `PATH`): libobs's folder is added (`AddDllDirectory`).
- The encoders' test programs (`obs-nvenc-test.exe`, `obs-qsv-test.exe`, `obs-amf-test.exe`) run
  from beside the executable: without them NVENC, Quick Sync and AMF never registered.
- On a laptop with two GPUs libobs rendered on the integrated one (adapter 0): the engine picks
  the GPU Windows ranks highest for performance (`gpu.rs`; `LIGHTAIR_GPU_ADAPTER` overrides), and
  Chromium renders on the same one (`use-adapter-luid`) so its shared textures open on libobs's.
- Chromium started Lightair itself for each of its processes: they run as
  `lightair-web-helper.exe` (a window app: no console per process).

**Running**
- The workspace journal couldn't be emptied ("Access is denied"): Windows opens an append-only
  file without the right to truncate it. Every compaction failed (after a crash, a migration).
- The input recorder could hang for good when stopped right after starting.
- The Editor's export chose the first hardware encoder FFmpeg lists; on a PC whose NVIDIA driver
  is older than FFmpeg's NVENC wants, every export failed. Encoders are tried before use.
- FFmpeg, ffprobe and the other helpers run without a console window.
- macOS words in the UI: shortcuts read Ctrl+/Alt+ (`ui::keys_label`), fonts are Windows's
  (Consolas, Segoe UI), "Keychain" is Windows Credential Manager, "Mac" is "PC".
- Camera and microphone access: Windows's privacy switches (whole PC, user, desktop apps) are read
  from where Windows keeps them; a blocked camera says so, and the Permissions page opens the
  right Settings page instead of listing macOS's permissions.
- A display capture with no display chosen (new, or a project from a Mac) captured nothing: it
  takes the primary monitor, as macOS takes the main display. Lists of choices skipped nothing,
  so OBS's disabled "[Select a display to capture]" placeholder was offered as a display.
- A camera asked for a height it has no format for showed nothing ("video format match failed"):
  its formats are read from DirectShow's properties and the nearest one it has is used.
- Lightair Effect Stack's face maps (makeup, liner, light) never loaded: libobs's `GS_BUILD_MIPMAPS` asks
  Direct3D 11 for a whole mip chain with one level's data, which D3D11 refuses; and the module
  crashed about one run in three. The mip levels are made on the CPU now (every backend).

**Performance** (measured: [../benchmarks/lightair-vs-obs-windows.md](../benchmarks/lightair-vs-obs-windows.md))
- The canvas preview is drawn by DirectComposition under GPUI's UI, as macOS draws it with Core
  Animation: GPUI (third_party/gpui-pre-windows, patched) shows its swap chain over an underlay
  visual; each canvas's picture is a swap chain libobs's graphics thread presents to, straight
  from its own texture when the size matches. No frame is read back and no UI redraws for it.
- The preview presents only when its swap chain's frame-latency waitable object says the
  compositor took the last frame: a present that waited (while a display capture starts and
  Windows reconfigures composition) made the program skip ~17 frames on each switch to a scene
  with a display capture; now 3-4, as OBS.
- The canvas preview renders at the size it's shown (on a laptop whose screen hangs off the other
  GPU each frame crosses between them), with libobs's area filter when much smaller.
- Media files decode on the GPU's video decoder by default (OBS's media source has it, off):
  about 40% less CPU idle with two videos playing, 25% less recording.
- The mixer's meters are DirectComposition visuals moved by a meter thread at 60 fps (glyph
  surfaces made once spell the peak readout): the window doesn't redraw for a moving meter.
- Web pages: Chromium shares each frame as a D3D11 shared texture, opened by libobs (OBS's own
  browser source does the same); no frame is copied through memory.
- Cameras stop while off air (DirectShow's own "deactivate when not showing"); display capture
  already stops while hidden on Windows.

**Left** (by impact)
- Distribution: an installer (MSIX or Inno Setup), Authenticode signing, updates, and the app laid
  out as OBS's (`bin\64bit\lightair.exe` beside libobs, `core\`, `data\`), with CEF beside it.
- Windows Studio Effects: Windows applies them in the camera driver (so OBS's capture already has
  them) and the button opens Settings › Camera; reading or switching them from Lightair
  (KSPROPERTY_CAMERACONTROL_EXTENDED_BACKGROUNDSEGMENTATION) needs a Copilot+ PC to try on.
- Google sign-ins from Chrome/Edge 127+: their cookies use app-bound encryption, which only the
  browser itself can open; Lightair says so and asks for signing in in its own window (on
  purpose: getting around it is what cookie-stealing malware does).
- Lightair Virtual Camera in Teams, Zoom or a browser (checked with ffmpeg through DirectShow,
  `LIGHTAIR_SELFTEST=virtual-camera`: 20 frames of the magenta program, 247/0/244 back). It
  has to be registered in HKLM once (`regsvr32 lightair_camera.dll` as administrator: the Frame
  Server refuses a per-user registration, "path not found"); an installer should do that.
  Windows lists it as "Lightair Virtual Camera (Windows Virtual Camera)".
- The zoom badge by the pointer; the system pointer's picture; display capture's pointer and
  follow-zoom on a second monitor (`display_frame`).

**Done since** (17 November 2026)
- The virtual camera: Lightair's own on Windows 11, "Lightair Virtual Camera"
  (crates/lightair-camera-win: a Media Foundation camera source, `lightair_camera.dll`, added
  with `MFCreateVirtualCamera`; 1080p30 NV12 through shared memory, written only while an app
  reads it), installed from the menu ("Install Lightair Virtual Camera…", one administrator
  approval). Where it can't be (Windows 10, not installed): OBS's (`win-dshow`'s
  `virtualcam_output`), which ffmpeg read the 1920×1080 program from. Colours: BT.601 limited
  range, said in the media type (what apps assume of a webcam's NV12 when they don't look).
- "Now playing" from Windows' media sessions (WinRT `GlobalSystemMediaTransportControls`): title,
  artist, album, app and artwork. Checked with Media Player.
- "Use my browser's" sign-in from Chrome, Edge and Brave (the key unsealed with DPAPI, cookies
  AES-GCM). Checked on this PC's Chrome and Edge.
- The Editor player is presented under the UI (wgpu on Direct3D 12 into a DirectComposition
  visual), no read back; paced to the clock as it's drawn. `LIGHTAIR_EDITOR_VULKAN=1` goes back to
  Vulkan and read back.
