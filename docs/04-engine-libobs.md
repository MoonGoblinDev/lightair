# 04 · Engine: libobs Integration

Lightair uses **libobs** as its media engine and nothing else from OBS Studio's frontend. This document defines how the engine is built, loaded, driven and observed.

## Engine version policy

- `third_party/obs-studio` is a git submodule pinned to an **OBS release tag** (the baseline at design time is **33.0.0**, which introduces first-class multiple canvases, `obs_canvas_*`).
- Lightair builds **libobs, the graphics backends, and a curated set of OBS core plugins** from that tree. It never builds the OBS Qt frontend.
- Local changes live in `third_party/patches/` as small, upstreamable patches. Each patch has a reason, an upstream PR link once filed, and a removal condition. The target is zero permanent patches.
- Upgrading the engine is a deliberate change: bump the tag, rebuild, run the golden-render, A/V-sync and soak suites ([16](16-quality-performance-accessibility.md)), check third-party plugin ABI expectations.

## What ships in the bundle

| Component | From | Notes |
|---|---|---|
| `libobs` + `libobs-opengl` / `libobs-metal` (macOS), `libobs-d3d11` (Windows) | OBS tree | Graphics backend chosen at startup; Metal preferred on macOS once it is at parity for every module Lightair ships |
| `mac-capture`, `mac-avcapture`, `mac-videotoolbox`, `coreaudio-encoder`, `mac-syphon` | OBS tree | macOS capture & encode |
| `win-capture`, `win-dshow`, `win-wasapi`, `obs-nvenc`, `obs-qsv11`, `win-mf` (AMF) | OBS tree | Windows capture & encode |
| `obs-ffmpeg`, `obs-x264`, `obs-outputs`, `rtmp-services`, `obs-transitions`, `obs-filters`, `image-source`, `text-freetype2`, `vlc-video`, `obs-webrtc` | OBS tree | Shared core |
| `lightair-*` modules | Lightair | See [03 · Architecture](03-architecture.md#crate-graph) |
| CEF browser source | Lightair (`lightair-browser`, derived from `obs-browser`) | Rebuilt without Qt; helper processes signed in-bundle |
| `libobs-frontend-api` | Lightair (`lightair-compat`) | Same name and C ABI as OBS's; backed by Lightair ([13](13-extensibility-and-compatibility.md#frontend-api-shim)) |
| Qt runtime (QtCore, QtGui, QtWidgets, QtSvg, QtNetwork) | obs-deps release matching the pinned OBS | Loaded only when a Qt plugin is enabled ([13](13-extensibility-and-compatibility.md#qt-runtime)) |
| Virtual camera | Lightair | Camera Extension (macOS), DirectShow/MF (Windows) |

**Not shipped:** `frontend-tools`, `obs-websocket` (Lightair implements a compatible API natively), `decklink-output-ui`, `aja-output-ui`, `obs-vst` (replaced by Lightair's audio plugin host), Python/Lua scripting runtimes (replaced by Lightair's extension runtime; see [13](13-extensibility-and-compatibility.md)).

## Bootstrap sequence

Executed by the engine actor at launch. Every step reports progress to the splash/first-frame UI and fails into a specific Issue.

1. `obs_startup(locale, module_config_path, profiler_store)`; module config lives under Lightair's own app-support directory, never OBS's.
2. **Audio:** `obs_reset_audio2` with the project's sample rate (48 kHz default) and channel layout (stereo default, 5.1/7.1 allowed).
3. **Video:** `obs_reset_video` for the **primary canvas** (the "main" format). Graphics module, colour format (NV12/P010/I444), colour space (sRGB, Rec.709, Rec.2100 PQ/HLG), range and scale filter come from project settings.
4. **Modules:** on macOS, core modules load from `Contents/PlugIns` via `obs_load_core_modules`. Lightair registers an explicit allowlist with `obs_add_safe_module`: every module it ships, plus each third-party plugin the user has enabled on the Plugins settings page. Anything else is skipped and listed as disabled, so nothing unexpected loads. Third-party plugins load afterwards from the user plugin directory through `obs_load_plugins`, each one recorded with its load result for the Plugins settings page.
5. `obs_post_load_modules`, then **capability discovery**: enumerate source, filter, transition, encoder, output and service types into a `Capabilities` table that the UI uses (for example, to offer only encoders that exist on this machine).
6. **Additional canvases** (vertical and any custom format) via `obs_canvas_create`.
7. **Reconcile** the loaded project document into libobs (see below).
8. Start previews for visible views. The UI shows the first real frame. Startup is complete.

Shutdown reverses this: stop outputs gracefully (finishing files), release scenes and sources, remove canvases, `obs_shutdown`.

## Mapping the document onto libobs

| Lightair concept | libobs object(s) | Notes |
|---|---|---|
| **Project** | the libobs core instance | One project open at a time per process |
| **Format** (Landscape 16:9, Vertical 9:16, Square, custom) | `obs_canvas_t` (primary format uses the main canvas) | Each format has its own resolution, FPS divisor and colour settings |
| **Scene** | one `obs_scene_t` **per format** it is enabled for, created with `obs_canvas_scene_create` | All formats share the same underlying sources, so a camera is opened once |
| **Layer** | `obs_sceneitem_t` in each per-format scene, referencing a shared `obs_source_t` | Per-format transform, crop, visibility and blending are stored on the layer in the document and applied per scene item |
| **Group** | `obs_scene_t` group (`obs_scene_add_group2`) | Groups are layers with children; collapsed in UI by default |
| **Media input** (camera, display, window, app, file, browser…) | `obs_source_t` (input) | Created once per input; layers reference inputs. The same input can appear in many scenes ("clone" by construction) |
| **Layer effect** | filter `obs_source_t` attached with `obs_source_filter_add` | Effects on the *input* affect every layer using it; effects on the *layer* are applied via a per-layer private wrapper source (see "Input vs layer effects") |
| **Transition** | transition `obs_source_t` per format | Program output of each canvas goes through its own transition |
| **Track** (audio) | audio-capable `obs_source_t` + its audio filters, mixer-track bitmask | See [10 · Audio](10-audio.md) |
| **Mix** (Stream, Recording A/B/…, Monitor) | libobs audio mixes 1–6 (`MAX_AUDIO_MIXES`) | Lightair allocates mixes; the user never sees numbers |
| **Output destination** | `obs_output_t` + `obs_service_t` + `obs_encoder_t`s | Encoders are shared between destinations with identical settings (see [11](11-outputs-streaming-recording.md)) |
| **Hotkey** | not libobs hotkeys | Lightair owns key handling (global and in-app); libobs hotkeys are disabled to avoid double handling |

### Input vs layer effects

OBS attaches filters to sources, so a filter on a camera affects every scene that uses it. Users expect two different things, and Lightair offers both explicitly:

- **Input effects** (shown in the Inspector under the input's name): colour correction, Lightair Effect Stack, noise suppression on a mic. These are "fix the device once."
- **Layer effects** (shown under the layer): drop shadow, mask, blur, chroma key in *this* scene only. Implemented by wrapping the input in a lightweight private source (`lightair-sources: layer_wrap`) owned by the layer, carrying the layer's filter chain. Inputs without layer effects aren't wrapped, so there's no overhead.

### Reconciler

- Patches are applied in document order inside the engine actor.
- Every libobs object Lightair creates is tagged with the document **UUID** of its owner, so the reconciler can find and diff objects without name lookups. Names shown to users are display-only and may contain anything.
- Expensive operations (opening a device, loading a browser page) are started asynchronously; the layer shows a loading state in the UI while the runtime event is pending.
- Bulk changes (scene switch with 30 layers, template application) are wrapped in `obs_scene_atomic_update` so the audience never sees a half-applied state.
- The reconciler is **idempotent**: applying the full document to an empty engine produces the same libobs state as any sequence of patches. A test enforces this.

## Preview rendering

Requirements: native, no tearing, ≤ 1 frame of added latency, no work when hidden, overlays drawn by the UI on top.

### macOS (primary platform)

1. `lightair-preview` allocates a pool of **three IOSurfaces** per preview at the preview's on-screen pixel size (re-allocated on resize, debounced).
2. On the graphics thread (via `obs_add_main_rendered_callback` for the main canvas, and per-canvas render hooks for other formats), Lightair wraps each IOSurface as a texture with `gs_texture_create_from_iosurface`, sets it as the render target, and draws the canvas output texture scaled with the project's scale filter.
3. After the GPU fence, the buffer index is published atomically.
4. On the main thread, each frame, the canvas panel takes the newest published buffer, wraps the IOSurface in a `CVPixelBuffer` (`CVPixelBufferCreateWithIOSurface`, cached per surface) and paints it with GPUI's `surface()` element.
5. Overlays are ordinary GPUI elements stacked above the surface in the same element tree.

**Studio mode** uses two previews (Preview, Program). **Multiview** uses one small preview per scene, rendered by `obs_view` instances at thumbnail resolution, throttled to 15–30 fps.

### Windows and Linux

Same architecture, different handle type: DXGI shared NT handles (Windows) and DMA-BUF (Linux) imported into the GPUI renderer through a Lightair-maintained `surface()` backend, with a CPU-upload fallback. The canvas panel code is identical on every platform; only `lightair-preview` differs.

## Native modules: how Lightair extends libobs

Lightair features that run per frame or per audio buffer are libobs modules, so they work in every format, in recordings, in virtual camera output and through the remote API automatically:

- **Sources** (`obs_source_info` with `OBS_SOURCE_TYPE_INPUT`): text, counters, timers, shapes, chat overlay, alerts, gradients, phone camera receiver, image slideshow.
- **Filters** (`OBS_SOURCE_TYPE_FILTER`): every video effect, every audio effect, Lightair Effect Stack.
- **Transitions** (`OBS_SOURCE_TYPE_TRANSITION`): Move/Morph, wipes, matte stinger.
- **Outputs**: replay/clip writer, ISO recorder helpers.

Each module exposes its settings through `obs_properties` (so they are generically inspectable) **and** a Lightair-specific schema with richer UI hints (groups, units, ranges, curves, colour pickers, presets). The Inspector prefers the Lightair schema.

Rust modules use a thin `lightair-obs-module` crate that implements the `obs_source_info` vtable safely. C/ObjC modules (Lightair Effect Stack) keep their C core and are wrapped the same way.

## Telemetry from the engine

| Signal | Source | Delivery |
|---|---|---|
| Audio levels (peak, magnitude, input peak per channel) | `obs_volmeter` callbacks per track | Lock-free ring per track; UI samples at display rate |
| Render time, frame interval, lagged/skipped frames | `obs_get_average_frame_time_ns`, `obs_get_lagged_frames`, `obs_get_total_frames`, `video_output_get_skipped_frames` | Stats sampler (4 Hz) |
| Encoder load, dropped frames, bitrate, congestion, reconnecting | `obs_output_*` getters and signals | Stats sampler + runtime events |
| Disk space and recording size | OS calls on the recording volume | Stats sampler (1 Hz) |
| CPU and memory | `os_cpu_usage_info_query`, process RSS | Stats sampler (1 Hz) |
| Source state (active, showing, media state, device lost) | source signals | Runtime events |

Telemetry is **pull-based from the UI side**: values are produced cheaply and continuously, and only read when a widget is visible.

## Performance rules for code inside libobs threads

- No heap allocation per frame in hot paths; preallocate on settings change.
- No blocking locks shared with the main thread. Settings reach render code through double-buffered structs swapped atomically.
- Shaders are compiled once per settings "shape" and cached; uniforms change per frame.
- Every module reports its own GPU/CPU time in debug builds so the Stats panel can attribute cost per layer and per effect.
