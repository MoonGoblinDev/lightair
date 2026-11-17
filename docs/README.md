# Lightair Design Documentation

Lightair is a native live-production studio for streamers and creators, built on **libobs** (the engine inside OBS Studio) with an interface written in **Rust on GPUI and gpui-kit**. It aims for the feature scope and polish of the best dedicated streaming apps, keeps OBS's engine and plugin ecosystem, and ships the things OBS users install plugins for, including a face-aware **Lightair Effect Stack**, as built-in features.

These documents describe the **finished product**: what it does, how it looks and behaves, and how it's built. They're not a prototype plan and contain no schedule.

## Documents

| # | Document | What it covers |
|---|---|---|
| 01 | [Product Vision & Principles](01-product-vision.md) | Problem, audience, principles, non-goals, positioning, signature experiences |
| 02 | [Feature Catalog](02-feature-catalog.md) | Every feature with an ID and origin (modern studio / OBS / plugin replacement / Lightair Effect Stack / Lightair) |
| 03 | [System Architecture](03-architecture.md) | Processes, threads, crates, document/runtime/telemetry state, preview path, resilience |
| 04 | [Engine: libobs Integration](04-engine-libobs.md) | Engine build, bootstrap, document-to-libobs mapping, reconciler, preview rendering, native modules |
| 05 | [Interface: Shell, Layout & Interaction](05-interface-shell.md) | Window anatomy, panels, primary actions, command palette, live safety, onboarding, settings |
| 06 | [Design System](06-design-system.md) | Tokens, themes, typography, motion, studio components, patterns, writing style |
| 07 | [Scenes, Layers & Canvas](07-scenes-layers-canvas.md) | Formats, layer types, canvas editing, masks, animation, studio mode, automations, templates, transitions |
| 08 | [Video Effects](08-video-effects.md) | Attach points, effect stack UI, presets, full effect catalog |
| 09 | [Lightair Effect Stack](09-effect-stack.md) | Built-in beauty & camera pipeline: looks, skin, shape, makeup, effects, atmosphere, colour, light, background, framing |
| 10 | [Audio](10-audio.md) | Tracks, mixes, scene-aware tracks, mixer UI, effects, monitoring, routing |
| 11 | [Outputs](11-outputs-streaming-recording.md) | Streaming, multistreaming, stream manager, Go Live review, recording, ISO, clips, virtual camera, NDI |
| 12 | [Chat, Alerts, Creator Tools & Control](12-chat-alerts-integrations.md) | Unified chat, moderation, activity, alerts, widgets, captions, overlay builder, hotkeys, Stream Deck, MIDI, remote API |
| 13 | [Extensibility & OBS Compatibility](13-extensibility-and-compatibility.md) | OBS plugin support, built-in replacements, JS extensions, content packs |
| 14 | [Data, Persistence & Settings](14-data-persistence-settings.md) | Project format, autosave journal, undo, settings layers, OBS import, secrets |
| 15 | [Platform, Distribution & Licensing](15-platform-distribution-licensing.md) | Platforms, bundle layout, permissions, signing, updates, build, GPL-3.0, privacy |
| 16 | [Quality](16-quality-performance-accessibility.md) | Performance budgets, testing, observability, accessibility, localisation, definition of done |

**Suggested reading order:** 01 → 02 for what and why; 03 → 04 for how it's built; 05 → 06 for how it looks; 07–12 for each area in depth; 13–16 for cross-cutting concerns.

## Key decisions

| Decision | Choice | Why | Where |
|---|---|---|---|
| Engine | libobs at a pinned OBS release, built from source | Industry-standard capture/encode/output stack and plugin ecosystem | [04](04-engine-libobs.md) |
| UI technology | Rust + GPUI + gpui-kit | Native GPU rendering, low overhead next to games, preview composited inside the UI scene (handles and menus draw over video) | [03](03-architecture.md) |
| Process model | One process for UI + engine; isolate browsers and audio plugins | Zero-copy preview and simple interaction; isolate the parts that actually crash | [03](03-architecture.md#process-model) |
| Source of truth | Lightair document; libobs is reconciled to match | Undo, autosave, remote control and recovery come for free | [03](03-architecture.md#state-model) |
| Preview | libobs → IOSurface → `CVPixelBuffer` → GPUI `surface()` | Zero-copy on macOS; same design with DXGI/DMA-BUF elsewhere | [04](04-engine-libobs.md#preview-rendering) |
| Multi-format | libobs canvases (`obs_canvas_*`, OBS 33+) with per-format layouts sharing inputs | Native vertical without plugins; one camera opened once | [07](07-scenes-layers-canvas.md#formats) |
| Effects | Lightair libobs modules (filters, transitions, sources) | Work in every output path automatically | [08](08-video-effects.md) |
| Beauty | Dewy pipeline absorbed as Lightair Effect Stack, unbranded | Pigment-accurate skin, frame-synced tracking, already libobs-native | [09](09-effect-stack.md) |
| OBS frontend plugins | Frontend API shim and custom browser docks built in; Qt runtime opt-in | Migrating users keep their plugins without making Qt part of the core app | [13](13-extensibility-and-compatibility.md#compatibility-layer-for-frontend-plugins) |
| Remote API | Lightair JSON-RPC + obs-websocket v5 compatibility | New capabilities without breaking existing tools | [12](12-chat-alerts-integrations.md#remote-api) |
| Licence | GPL-3.0-or-later | Required to combine GPL-2.0-or-later libobs with Apache-2.0 GPUI | [15](15-platform-distribution-licensing.md#licensing) |
| Platforms | macOS first, then Windows, then Linux | Best zero-copy path and on-device ML on Apple Silicon | [15](15-platform-distribution-licensing.md#platform-strategy) |
| Accounts | Not required; no paid tiers in the design | Offline-first; cloud models are bring-your-own | [15](15-platform-distribution-licensing.md#privacy) |

## Glossary

| Term (in the app and the code) | Meaning |
|---|---|
| **Workspace** (`Workspace`) | A complete show setup: canvases, scenes, sources, audio, outputs; a `.lightair` package (`workspace.json`) |
| **Canvas** (`Canvas`) | An output picture's shape and size (Landscape, Vertical, Square, custom); a libobs canvas (`ObsCanvas`) |
| **Scene** (`Scene`) | An ordered stack of objects, laid out per canvas |
| **Object** (`Object`) | A source or generated element placed in a scene, with per-canvas transform, crop, visibility and effects |
| **Source** (`Source`) | What feeds objects and tracks (camera, screen, picture, mic, file); opened once, used by many objects |
| **Group / Empty** | An object holding other objects |
| **Template** | Reusable scenes whose sources are roles bound to real devices on use |
| **Effect** | A video or audio processor on a source, track or mix |
| **Lightair Effect Stack** | The built-in face-aware camera pipeline (beauty, shape, makeup, AR, atmosphere, light, background) |
| **Look** | A saved Lightair Effect Stack configuration applied in one click |
| **Track** | A channel strip in the mixer: one source, its effects, fader, mute, routing |
| **Mix** | A summed audio bus (headphones, VOD, a mix-minus) |
| **Master** (`MixerStrip::Master`) | The mixer's main strip: what the stream and the recording's main track carry |
| **Output / destination** | Anything leaving Lightair: stream destination, recording, clips |
| **Program** | The scene on air |
| **Cue scenes** (`cue_scenes`) / **Take** | Editing the next scene off air / putting it on air |
| **Studio take** (`StudioTake`) | A recording of each source on its own, for the Editor (a `.lightair-take` folder) |
| **Multitrack audio** (`multitrack`) | Each mixer track recorded to its own file (track files) |
| **Screen zoom & pointer** (`ScreenZoomPointer`) | A scene's live zoom, drawn pointer and clicks on its screen captures |
| **Drawn pointer** (`pointer`) | Lightair's own pointer drawn on a screen capture |
| **Headphones** | Where Lightair plays what you hear (`headphones_device`, low-latency headphones) |
| **Shortcuts** (`shortcuts`) | Keys that work anywhere on the computer |
| **Clip** | A saved slice of the replay buffer, per canvas |
| **Project** (`Project`) | An Editor project: a timeline of clips |
| **Command** | A data description of a change to the document; the unit of undo and remote control |
| **Patch** | The minimal change set a command produces, applied to libobs by the reconciler |
| **Reconciler** | The engine component that makes libobs match the document |
| **Runtime state / telemetry** | Live state (is it streaming) / high-rate measurements (levels, FPS), never in undo |
| **Issue** | A user-facing problem with a plain-language cause and a fix action |

Code uses the same words as the app. Saved files and the undo journal still read under the
older names (Project, Format, Layer, Input, …) through serde aliases.

## Sources

- OBS Studio 33.0.0 source (libobs headers and module loader).
- Lightair Effect Stack: the Dewy project (`obs-beauty`), its README, data files and pigment research.
- gpui-kit v0.9.1 and gpui-pre 0.3.12 sources (surface element, window handles).
