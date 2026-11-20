# 03 · System Architecture

This document describes how Lightair is put together: processes, threads, crates, the state model, and how data flows between the interface and the engine. Engine specifics live in [04 · Engine](04-engine-libobs.md).

## Stack at a glance

| Layer | Technology | Responsibility |
|---|---|---|
| Interface | Rust, **GPUI** via **gpui-kit** (pinned) | Windows, docks, panels, canvas editing, all interaction |
| Design system | `lightair-ui` on `gpui-base` + themed `gpui-component` | Tokens, studio-specific components |
| Application core | Rust (`lightair-model`) | Project document, commands, undo/redo, validation. No I/O, no libobs |
| Engine adapter | Rust (`lightair-engine`) over `libobs-sys` | Owns libobs; reconciles the document into libobs objects; streams runtime telemetry back |
| Engine | **libobs** (OBS Studio, vendored at a pinned release) | Capture, compositing, audio mixing, encoding, outputs |
| Native media modules | C/C++/ObjC/Rust compiled as libobs modules | Lightair's sources, filters, transitions, audio effects, beauty pipeline |
| Services | Rust (`lightair-services`) | Platform accounts (OAuth), stream metadata, chat, alerts, update checks |
| Remote | Rust (`lightair-remote`) | WebSocket API (Lightair-native + obs-websocket v5 compatible), Stream Deck, MIDI, OSC |

**Language policy.** Rust is the default. C/C++/Objective-C is used only where the code is a libobs module that benefits from sharing code with the OBS ecosystem (shaders, existing GPL code such as the beauty pipeline), or where a platform API is far simpler from Objective-C/C++ (Vision, Core ML, Media Foundation). Every C boundary is wrapped in a safe Rust API before the rest of the app sees it.

## Process model

```text
┌────────────────────────── Lightair (main process) ──────────────────────────┐
│                                                                             │
│  Main thread (GPUI) ── UI, input, layout, paint, preview surfaces           │
│        │  ▲                                                                 │
│  Commands  Snapshots / Events                                               │
│        ▼  │                                                                 │
│  Core actor ── document, undo, validation, autosave journal                 │
│        │  ▲                                                                 │
│   Patches  Runtime events                                                   │
│        ▼  │                                                                 │
│  Engine actor ── the only thread that calls mutating libobs APIs            │
│        │                                                                    │
│  libobs threads: graphics · video output · audio · encoders · outputs       │
│  Telemetry rings ◄── volmeters, stats, frame timings (lock-free)             │
│                                                                             │
│  Service tasks (async runtime): platform APIs, chat, remote API, updates    │
└─────────────────────────────────────────────────────────────────────────────┘
   │                       │                          │
   ▼                       ▼                          ▼
Browser helper        Virtual camera            Audio plugin host
(CEF, out-of-process)  (system extension)       (optional isolation for VST3/AU)
```

- **One main process** hosts the UI and libobs. Splitting libobs into a separate process (as Streamlabs does) costs a cross-process texture path and IPC for every interaction, and buys crash isolation that we get more cheaply by isolating the components that actually crash: browsers and third-party audio plugins.
- **Browser sources** run in CEF's own multi-process model (helper apps), feeding frames into libobs through shared GPU textures. A browser crash kills a helper, not the show.
- **Audio plugins** (VST3/AU) run in-process by default for latency. Each plugin can be switched to an isolated host process, at the cost of one buffer of latency. A plugin that crashes in-process is quarantined on next launch.
- **Virtual camera** is a system extension on macOS (CoreMediaIO Camera Extension) and a DirectShow/Media Foundation virtual camera on Windows, fed by a shared-memory/IOSurface frame queue.

## Threading model

| Thread / executor | Owner | Rules |
|---|---|---|
| **Main (GPUI)** | GPUI | Never blocks. Never calls libobs mutators. Reads immutable document snapshots and telemetry. |
| **Core actor** | `lightair-model` | Single-threaded owner of the authoritative document. Applies commands, produces patches, records undo. Runs on a dedicated thread; tasks arrive over a channel. |
| **Engine actor** | `lightair-engine` | The single caller of libobs mutating APIs. Applies patches in order, owns all `obs_*_t` handles, translates libobs signals into events. |
| **libobs internal threads** | libobs | Graphics, video, audio, encoder and output threads. Lightair code running inside them (native modules, render callbacks) must be real-time safe: no allocation in the hot path where avoidable, no locks shared with the UI, no I/O. |
| **Async runtime** | `lightair-services`, `lightair-remote` | Tokio. Network, OAuth, chat sockets, remote API, update checks. Talks to core via commands, never to libobs directly. |

**Why actors.** libobs is thread-safe in places and not in others. Funnelling every mutation through one engine thread makes ordering explicit, makes "apply this patch atomically" possible, and makes it trivial to keep the UI responsive while libobs does something slow (opening a capture device can take a second).

## Repositories

The crates every Lightair app shares (`lightair-model`, `-store`, `-audio`, `-render`, `-fonts`,
`-live`, `-widgets`, `-automation`, `-remote`, `-platform`) live in their own repository,
**lightair-core** (`MoonGoblinDev/lightair-core`, public, GPL-3.0-or-later), checked out beside this
one. This repository holds the desktop app: the GPUI shell, the libobs engine adapter, the preview
bridge, the browser, the native modules. Lightair Live and Lightair Editor, the mobile apps, use
lightair-core the same way: every repository pins it to one commit on GitHub. Here,
`cargo xtask core-pin [<rev>]` moves the pin (the commit must be pushed first);
`cargo xtask core-link` builds against a checkout beside this repo (`../lightair-core`) for
changing both at once, and `cargo xtask core-unlink` undoes it (`cargo xtask check` refuses while
it's linked).

## Crate graph

```text
lightair-app ─────────────────────────────── binary, composition root
 ├─ lightair-shell           windows, docks, command palette, menus, keybindings
 │   └─ lightair-features/*  one crate per panel/feature (see below)
 ├─ lightair-ui              design system: tokens, theme, studio components
 ├─ lightair-model           document model, commands, undo, validation (pure)
 ├─ lightair-engine          libobs adapter, reconciler, telemetry
 │   ├─ libobs-sys           bindgen FFI for the vendored libobs
 │   └─ lightair-preview     canvas → GPU surface bridge (IOSurface / DXGI)
 ├─ lightair-modules/*       native libobs modules shipped in-bundle
 ├─ lightair-services        accounts, platform APIs, chat, alerts, updates
 ├─ lightair-remote          WebSocket API, Stream Deck, MIDI, OSC
 ├─ lightair-store           persistence, migrations, import (OBS, JSON)
 ├─ lightair-platform        permissions, keychain, extensions, OS integration
 └─ lightair-compat          OBS plugin compatibility: frontend API shim, Qt plugin host, plugin classifier
```

**Feature crates** (`lightair-features/*`) each own one area of the UI and depend only on `lightair-ui`, `lightair-model` (types + command constructors) and a read-only telemetry interface. They never depend on `lightair-engine`:

`layers`, `scenes`, `canvas`, `mixer`, `outputs`, `inspector`, `effect-stack`, `chat`, `activity`, `clips`, `settings`, `onboarding`, `multiview`, `stats`.

**Native modules** (`lightair-modules/*`) are libobs plugins built in-tree and loaded from the app bundle like OBS's core plugins:

| Module | Contents |
|---|---|
| `lightair-sources` | Text (rich), Counter/Timer/Countdown, Shapes, Chat overlay, Alerts overlay, Color/Gradient, Clone (layer reference), Image slideshow, Phone camera receiver |
| `lightair-video-fx` | The full video effect catalog ([08 · Effects](08-video-effects.md)), Lightair transitions (Move/Morph, wipes, stinger with matte) |
| `lightair-effect-stack` | The face-aware Lightair Effect Stack pipeline (beauty, reshape, makeup, AR, atmosphere, relight, background) ([09 · Lightair Effect Stack](09-effect-stack.md)) |
| `lightair-audio-fx` | Voice Clarity, noise suppression, gate, compressor, limiter, EQ, de-esser, ducking, reverb, delay, plugin host bridge ([10 · Audio](10-audio.md)) |
| `lightair-capture-mac` / `-win` | Capture improvements where OBS's are insufficient: per-app audio via ScreenCaptureKit/WASAPI process loopback, Continuity Camera handling |
| `lightair-outputs` | Multistream coordinator helpers, clip/replay writer, ISO recorder, NDI/SRT/WHIP glue where not upstream |

## State model

Lightair distinguishes **three kinds of state**. Mixing them is the most common way UI frameworks over libobs go wrong.

| Kind | Examples | Owner | Persisted | Undoable | Rate |
|---|---|---|---|---|---|
| **Document** | Scenes, layers, transforms, effect settings, tracks, outputs config, hotkeys | Core actor | Yes (project file) | Yes | Human speed |
| **Runtime** | Is the output live, encoder in use, device connected, current program scene, recording path | Engine → Core | No | No | Event-driven |
| **Telemetry** | Audio levels, FPS, render/encode times, bitrate, dropped frames, disk space | Engine → rings | No | No | 30–120 Hz |

### Document flow (unidirectional)

```text
UI gesture ─► Command ─► Core: validate + apply ─► new Document snapshot (Arc)
                              │                         │
                              ├─► Undo stack            └─► UI re-renders from snapshot
                              └─► Patch list ─► Engine reconciler ─► libobs
```

- **Commands** are plain data (`SetLayerTransform { layer, format, transform }`, `AddEffect { target, kind, index }`, …). Every UI action, hotkey, remote API call and script produces commands, so all of them are undoable, recordable and testable the same way.
- **Snapshots** are immutable, structurally shared (`Arc` + persistent collections). The UI holds the latest snapshot; diffing two snapshots is cheap pointer comparison.
- **Patches** are the minimal change set produced by applying a command. The engine's **reconciler** maps patches onto libobs operations (create source, set settings, reorder scene item, …).
- **Coalescing.** Continuous gestures (dragging a fader or a layer) send high-rate commands that the core coalesces into one undo entry when the gesture ends. During the gesture, the engine receives "live" patches so the audience sees motion in real time.
- **Transactions.** Multi-step operations (paste 5 layers, apply a template, apply a Lightair Effect Stack look) are one command and one undo entry.

### libobs is not the source of truth

The document is. libobs is a rendering target that is reconciled to match. Consequences:

- Undo/redo, autosave, remote-control and multi-window consistency fall out naturally.
- The engine can be torn down and rebuilt from the document (device lost, GPU reset, crash recovery).
- Third-party plugin sources store their settings as opaque `obs_data` JSON inside the document. Lightair can render their properties generically (see [13](13-extensibility-and-compatibility.md)) without understanding them.
- When libobs changes something on its own (a plugin updates its own settings, a device disappears), the engine emits a **runtime event**. If that change belongs in the document, the engine sends an `ExternalChange` command that core applies without an undo entry.

### Runtime and telemetry flow

- Engine translates libobs signals (`source_activate`, output `start`/`stop`/`reconnect`, device changes, …) into typed `RuntimeEvent`s on a channel to core, which folds them into a `Runtime` snapshot the UI observes.
- High-rate values never go through channels per sample. Each producer (volmeter callback, stats sampler) writes into a **lock-free single-producer ring or atomic cell**. The UI samples them once per frame while a meter or stat is visible. Nothing is computed for invisible widgets.

## Preview & rendering path

The canvas preview must be native, tear-free, low latency, and composited *inside* the GPUI scene so the UI can draw handles, guides, and menus on top of it.

```text
libobs graphics thread
  obs_canvas / obs_view render ─► render target texture (IOSurface-backed on macOS,
                                   DXGI shared on Windows), pooled ×3
            │ fence + publish index (atomic)
            ▼
lightair-preview (main thread, per frame)
  newest completed buffer ─► CVPixelBuffer wrapping the IOSurface (macOS)
            │
            ▼
GPUI `surface()` element inside the canvas panel, with overlays painted above it
```

- One preview stream per visible view (main canvas, vertical canvas, scene thumbnails in multiview, studio-mode preview and program). Each has its own resolution, sized to its on-screen pixels, so a small thumbnail never costs a full-resolution render.
- Hidden views stop rendering immediately.
- **Zero copy on macOS:** libobs renders into textures created from IOSurfaces; GPUI's Metal renderer samples the same IOSurface via `CVPixelBuffer`. A CPU-readback path exists as a fallback for unsupported GPU/driver combinations and for diagnostics.
- **Windows:** libobs (D3D11) renders into shared textures. GPUI's surface primitive is macOS-only today, so Lightair contributes or maintains a Windows `surface()` backend that imports DXGI shared handles. Fallback: per-frame image upload.
- **Linux:** DMA-BUF import into the GPUI renderer, fallback image upload.
- **Overlays** (selection, handles, snapping guides, safe areas, crop shading, rulers) are GPUI elements drawn in canvas coordinates using the document's transforms, not libobs drawing. This is what makes the editor feel like a design tool.

Details in [04 · Engine](04-engine-libobs.md#preview-rendering).

## Error handling & resilience

| Failure | Behaviour |
|---|---|
| Device disconnects (camera, mic, capture card) | Layer shows a "disconnected" placeholder on the canvas (audience sees last frame or a styled card, user's choice). Auto-reconnects when the device returns. |
| Output disconnects | Automatic reconnect with backoff, visible in the output row, without stopping other outputs. |
| Encoder overload | Health indicator turns amber, then the app suggests a specific remedy (lower preset, hardware encoder, lower resolution). |
| Browser helper crash | Source shows a reload affordance; helper restarts automatically up to N times. |
| Audio plugin crash | Track bypasses the plugin, marks it red, offers isolated mode. |
| Lightair crash | Crash reporter (opt-in upload). On relaunch, the document is restored from the autosave journal and the user is offered to resume outputs that were live. |
| GPU reset / device lost | Engine tears down graphics, rebuilds from the document, previews resume. |

All errors are **typed** (`thiserror`) inside crates and converted to user-facing **Issues** at the shell: plain-language title, cause, suggested fix action, and a "copy diagnostics" button. Issues show in the status bar and in the Activity panel.

## Repository layout

```text
lightair/
├─ Cargo.toml                workspace
├─ crates/                   lightair-* Rust crates (above)
├─ modules/                  native libobs modules (CMake, built by build.rs/xtask)
├─ third_party/
│  ├─ obs-studio/            git submodule pinned to an OBS release tag
│  └─ patches/               minimal, upstreamable patches to libobs
├─ assets/                   icons, fonts, sounds, default templates, LUTs, models
├─ xtask/                    build orchestration: libobs, modules, bundle, sign
├─ tests/                    integration and golden-render tests
└─ docs/                     this documentation
```

`cargo xtask build` builds libobs and the selected OBS core plugins from the submodule, builds Lightair's modules, compiles the Rust workspace, and assembles the platform bundle (see [15 · Platform](15-platform-distribution-licensing.md)).
