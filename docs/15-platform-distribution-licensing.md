# 15 · Platform, Distribution & Licensing

## Platform strategy

| Platform | Status in the design | Notes |
|---|---|---|
| **macOS 14+ on Apple Silicon** | Primary | First-class everything: zero-copy preview, Vision + Neural Engine for Camera Studio, ScreenCaptureKit per-app capture, Camera Extension virtual camera, VideoToolbox encoders |
| **macOS 14+ on Intel** | Supported | Same build (universal binary); Camera Studio defaults to the Balanced profile |
| **Windows 10 22H2+ / 11, x64** | Supported | D3D11 libobs backend, Windows `surface()` backend for GPUI, NVENC/QSV/AMF, WASAPI process loopback for per-app audio, ONNX Runtime + DirectML for Camera Studio |
| **Windows on ARM** | Later | Depends on libobs and encoder support |
| **Linux (x64, Wayland + X11)** | Supported after Windows | PipeWire capture, VA-API/NVENC, DMA-BUF preview import, Flatpak distribution |

Every feature in the catalog is designed cross-platform; the docs note where an implementation is platform-specific. A feature isn't marked complete until it works on every supported platform or has a documented platform exception.

## macOS bundle layout

```text
Lightair.app/Contents/
├─ MacOS/Lightair                         Rust binary (GPUI app)
├─ Frameworks/
│  ├─ libobs.framework                built from third_party/obs-studio
│  ├─ libobs-metal.dylib, libobs-opengl.dylib
│  ├─ ffmpeg dylibs, libdatachannel, librist, libsrt, mbedtls, …
│  ├─ libobs-frontend-api.dylib      Lightair's frontend API shim
│  ├─ Qt*.framework                  Qt runtime, loaded only when a Qt plugin is enabled
│  ├─ Chromium Embedded Framework.framework
│  └─ Lightair Helper (Renderer|GPU|Plugin).app    CEF helpers
├─ PlugIns/
│  ├─ mac-capture.plugin, mac-avcapture.plugin, obs-ffmpeg.plugin, …   OBS core modules
│  └─ lightair-sources.plugin, lightair-video-fx.plugin, lightair-camera-fx.plugin, lightair-audio-fx.plugin, …
├─ Library/SystemExtensions/
│  └─ Lightair Camera.systemextension     virtual camera (CoreMediaIO Camera Extension)
└─ Resources/
   ├─ assets/ (fonts, icons, templates, looks, LUTs, sounds)
   ├─ models/ (face landmarker, segmenter, YuNet)
   └─ licenses/ (GPL text, third-party notices, source offer)
```

This mirrors OBS.app's structure, so OBS core modules' `@executable_path/../Frameworks` rpaths resolve without patching.

## Permissions & entitlements (macOS)

| Capability | Mechanism | When requested |
|---|---|---|
| Camera | `NSCameraUsageDescription` | Onboarding device step, or first camera layer |
| Microphone | `NSMicrophoneUsageDescription` | Onboarding, or first mic track |
| Screen & system audio recording | ScreenCaptureKit / TCC | First display, window or app capture |
| Global hotkeys | Input Monitoring / Accessibility | Only when the user binds a global hotkey |
| Virtual camera | System extension activation (user approval in System Settings) | When the user enables Virtual Camera |
| Local network | `NSLocalNetworkUsageDescription` | NDI, LAN remote API, phone camera |
| Apple Events | Not used | |

Hardened runtime entitlements: camera, audio input, JIT for the extension runtime, and `com.apple.security.cs.disable-library-validation`. That last one is required so that third-party OBS plugins, which other developers sign, can load at all. Entitlements are fixed at signing time, so Lightair enforces the policy itself: third-party plugins are **off by default** and load only after the user enables each one on the Plugins page, which explains the trade-off. Until then, only Lightair-signed modules are on the libobs allowlist ([04](04-engine-libobs.md#bootstrap-sequence)).

The Permissions page in Settings shows every permission's status, what uses it, and a button to open the right System Settings pane.

## Code signing, notarisation, updates

- **macOS:** Developer ID signed, hardened runtime, notarised and stapled. Every helper, plugin, framework and the system extension is signed in-bundle.
- **Windows:** Authenticode-signed installer and binaries (MSIX and classic installer), with a portable zip for advanced users. Built today: the portable app and its zip (`cargo xtask dist` on Windows: `target\dist\Lightair`, laid out as OBS is, `bin\64bit\lightair.exe` beside libobs, its modules in `core\`, the MSVC runtime beside the programs); not yet signed or installed by an installer.
- **Linux:** Flatpak on Flathub, plus AppImage.
- **Updates:** Sparkle 2 (macOS) and an equivalent signed-feed updater on Windows, with EdDSA-signed appcasts. Channels: **Stable**, **Beta**, **Nightly**. Updates never install while live or recording; the app offers to install on quit.
- **Release notes** are shown in-app after an update, linked to the changelog.

## Building

- `cargo xtask bootstrap`: fetches toolchains and prebuilt OBS dependencies (obs-deps) for the platform.
- `cargo xtask engine`: builds libobs, graphics backends and the curated OBS core modules from `third_party/obs-studio` (CMake + Ninja), applies `third_party/patches`.
- `cargo xtask modules`: builds `lightair-modules/*`.
- `cargo build`: the Rust workspace (`libobs-sys` bindgen runs against the built headers).
- `cargo xtask bundle --sign`: assembles, signs and notarises the platform package.

**Development mode** can link against an installed OBS of the same version for fast iteration on UI work, but CI and releases always build the engine from source.

CI builds every platform on every change, runs unit tests, headless GPUI UI tests, engine integration tests with a virtual camera/microphone, and the golden-render suite ([16](16-quality-performance-accessibility.md)).

## Licensing

| Component | Licence | Implication |
|---|---|---|
| libobs, OBS core plugins | GPL-2.0-or-later | Linking makes Lightair a GPL derivative |
| GPUI, gpui-kit (gpui-base, gpui-component, gpui-shell) | Apache-2.0 | Compatible with GPL-3.0 (not GPL-2.0-only) |
| Camera Studio (from Dewy) | GPL-2.0-or-later | Same author; relicensed into Lightair as GPL-3.0-or-later |
| CEF / Chromium | BSD-style | Notices |
| Qt (runtime for plugin UI) | LGPL-3.0 | Dynamically linked, unmodified; notices and Qt source offer |
| FFmpeg (LGPL/GPL build as in OBS) | LGPL/GPL | Notices + source offer |
| ONNX Runtime | MIT | Notices |
| MediaPipe models | Apache-2.0 | Notices |
| OpenCV YuNet | MIT | Notices |
| Inter, JetBrains Mono | SIL OFL 1.1 | Notices |
| Lucide icons | ISC | Notices |

**Lightair is licensed GPL-3.0-or-later.** GPL-3.0 is required because Apache-2.0 code (GPUI) is not compatible with GPL-2.0-only, and libobs's "or later" clause allows GPL-3.0.

Obligations built into the release process:

- Complete corresponding source for every release (tagged repo + engine submodule + patches + build scripts).
- Licence texts and third-party notices in the bundle and in *About → Licences*.
- A written source offer in the installer and About window.
- Code signing doesn't restrict users from running modified builds (GPL-3.0 installation information is satisfied by public, documented build instructions; no locked-down hardware is involved).

### Trademarks

- "OBS" and "OBS Studio" are trademarks of the OBS Project. Lightair doesn't use them in its name or logo. It may say factually that it is "built on libobs, the engine of OBS Studio" in About and documentation.
- Lightair doesn't copy another app's name, logo, icons or visual design; its visual identity is its own.
- Platform logos (YouTube, Twitch, …) are used per each platform's brand guidelines, only to identify destinations.

## Privacy

- **No account required.** Lightair is fully functional offline.
- **Telemetry is opt-in**, anonymous, documented field by field, viewable before sending, and never includes frames, audio, chat contents, titles, keys or file paths.
- **Crash reports** are opt-in per crash (or "always"), with the report contents viewable.
- Platform sign-in uses OAuth with PKCE in the system browser; tokens live in the OS credential store.
- Camera Studio, noise suppression, captions/transcription and background effects run **on-device**.
- The only feature that can send data to a model provider is the **Overlay builder**, and only after the user connects a provider of their choice (cloud or local). The panel names the provider and what is sent.
