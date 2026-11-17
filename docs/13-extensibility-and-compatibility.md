# 13 · Extensibility & OBS Compatibility

Lightair's philosophy is **built in first**: anything streamers commonly add with a plugin should be a first-party feature. Extensibility exists for everything else: niche hardware, custom integrations, community creativity, and the existing OBS plugin ecosystem.

## Extension surfaces

| Surface | Who it's for | Runs where | Can do |
|---|---|---|---|
| **OBS plugins** (native) | Existing OBS ecosystem | In-process, libobs module | Sources, filters, transitions, encoders, outputs, services; frontend events, menu items and Qt panels through the [compatibility layer](#compatibility-layer-for-frontend-plugins) |
| **Lightair extensions** (JavaScript) | Integrators, community | Sandboxed JS runtime (`gpui-shell`) | Panels, commands, automation, overlays, service integrations, with explicitly granted capabilities |
| **Remote API** | External tools, Stream Deck, bots | Out-of-process (WebSocket) | Everything a user can do |
| **Content packs** | Creators | Data only | Templates, looks, stickers, LUTs, transitions (stinger media), themes, sound packs |

## OBS plugin compatibility

Lightair embeds libobs, so binary OBS plugins built for the same libobs major version load and work directly when they only use libobs APIs. Plugins that also use OBS's frontend or Qt go through the [compatibility layer](#compatibility-layer-for-frontend-plugins).

### Works

- **Sources** (inputs): appear in the Add Layer popover under "Plugins" with their own icon, or a generic icon.
- **Filters**: appear in the effect picker under "Plugins" for video or audio as appropriate.
- **Transitions**: appear in the transition picker.
- **Encoders, outputs, services**: appear in output and encoding settings.

### Properties UI

OBS plugins describe settings with `obs_properties`. Lightair renders them with a **generic property renderer** using Lightair components:

| obs_property type | Lightair control |
|---|---|
| bool | Toggle |
| int / float (slider) | Scrubbable slider with numeric field |
| int / float (spinner) | Scrubbable numeric field |
| text (default / password / multiline / info) | Input / secure input / text area / inline note |
| path (file / directory) | File picker with drag-and-drop |
| list (combo / editable / radio) | Select / combobox / segmented control |
| color / color_alpha | Colour picker |
| font | Font picker |
| editable list | Reorderable list editor |
| frame_rate | FPS picker |
| group (normal / checkable) | Collapsible section / section with toggle |
| button | Button (runs the plugin callback on the engine actor) |

Property modification callbacks are honoured: when a plugin's property callback changes visibility or options, the engine re-queries properties and the Inspector updates in place.

### Compatibility layer for frontend plugins

Many OBS plugins also talk to OBS's *frontend*, not just libobs. They call `obs-frontend-api` to listen to frontend events, add Tools menu items and save data into the scene collection, and they build docks, dialogs and property editors with Qt. Lightair supports them through a compatibility layer with three parts:

| Part | Status | Covers |
|---|---|---|
| **Frontend API shim** | Core, always on | Plugins that use `obs-frontend-api` without showing Qt UI: automation, event listeners, output helpers |
| **Custom browser docks** | Core, always on | URL docks (OBS's *Custom Browser Docks*): bot dashboards, activity feeds, third-party chat |
| **Qt runtime** | Opt-in, loaded only when a Qt plugin is enabled | Plugins that link Qt: docks, dialogs, custom property widgets, plugin-created browser docks |

#### Plugin classifier

Before loading a third-party plugin, Lightair reads its linked libraries (Mach-O load commands on macOS, PE imports on Windows, ELF `DT_NEEDED` on Linux) and classifies it:

| Links | Class | Loads with |
|---|---|---|
| libobs only | **libobs plugin** | Engine alone |
| + `obs-frontend-api` | **Frontend plugin** | Frontend API shim |
| + Qt | **Qt plugin** | Qt runtime (the user enables it per plugin) |

The classification, together with the compatibility database below, decides what the Plugins page offers. A Qt plugin can't load at all without Qt present (the OS loader would fail), so Lightair never attempts it until the user enables the Qt runtime for that plugin.

#### Frontend API shim

Lightair ships its own `libobs-frontend-api` with the same library name and C ABI as OBS's. Inside OBS that library is a thin layer over a table of frontend callbacks, and Lightair implements that table itself:

| Plugin calls | Lightair behaviour |
|---|---|
| Frontend events (streaming/recording/replay started and stopped, scene changed, transition changed, studio mode, virtual camera, exit, profile and collection changes) | Fired from Lightair runtime events, in the same order and on the same thread OBS uses |
| Current, preview and listed scenes; transitions | Mapped from the Lightair document. With several formats, the plugin sees the **primary format's** libobs scenes |
| Start/stop streaming, recording, replay buffer, virtual camera; save replay | Lightair commands, so they get undo labels where applicable and show in Activity with the plugin as their origin |
| Tools menu items | Entries in a **Plugins** menu and in the command palette |
| Save/load callbacks | The plugin's data is stored in the project package (`plugins/<id>.json`), so it travels with the show |
| Profile and global config | Per-plugin config files in Lightair's directories |
| Hotkeys the plugin registers with libobs | Shown in Lightair's Hotkeys page under the plugin's name and triggered through libobs |
| Main window, docks, browser panels (Qt-only calls) | Return null, unless the Qt runtime is enabled for that plugin |

#### Custom browser docks

Any URL can become a docked panel, rendered by Lightair's webview (`gpui-webview`), with no Qt involved. They dock, tab, pop out and save with the workspace like built-in panels. OBS's custom browser docks are imported along with a scene collection, so a migrating user's bot dashboard and activity feed reappear in the same place.

#### Qt runtime

For plugins that build Qt UI, Lightair hosts Qt inside its own process, beside GPUI.

- **Same Qt as OBS.** Lightair ships the Qt build from the obs-deps release that matches its pinned OBS version, so plugins built for that OBS link to it unchanged. Upgrading the engine upgrades Qt with it.
- **Loaded only when needed.** The Qt frameworks are in the app bundle, so the code signature covers them, but Lightair never loads them until a Qt plugin is enabled. Users without Qt plugins pay only disk space (about 40 MB) and no memory or CPU.
- **Shared event loop.** Lightair creates the `QApplication` but never calls `exec()`; GPUI owns the main event loop. On macOS, Qt's Cocoa event dispatcher runs off the same run loop, the way audio plugin hosts embed Qt-based plugin editors. On Windows, Lightair processes Qt's events from GPUI's message loop.
- **Docks become Lightair panels.** When a plugin adds a dock, Lightair takes the widget's native view (`NSView` or `HWND`) and places it in a Lightair dock panel, or a pop-out window. It docks, tabs and saves with the workspace like a built-in panel and carries a small **plugin** badge. Older plugins that add a `QDockWidget` straight to the main window are caught by an event filter on a hidden proxy main window and moved into Lightair panels the same way.
- **Dialogs** open as native Qt windows whose parent is a Qt wrapper around Lightair's main window, so they stay in front and centred.
- **Custom property widgets.** When a plugin replaces generic properties with its own Qt widget, the Inspector hosts that widget in place of the generic renderer.
- **Plugin browser docks.** Plugins that create browser panels through obs-browser's panel interface get Lightair's implementation of that interface, backed by Lightair's own CEF browser.
- **Theming.** Lightair generates a Qt stylesheet from its design tokens (colours, fonts, radii, control heights) and updates it when the theme changes, so plugin panels look close to native. They aren't pixel-identical, and the plugin badge is honest about that.

#### Compatibility database

A curated list, shipped with Lightair and updated with it, gives each known plugin a status:

| Status | Meaning |
|---|---|
| **Works** | Tested: libobs-only, or frontend API without Qt |
| **Works with Qt runtime** | Tested with the Qt runtime enabled |
| **Replaced by built-in** | Lightair has the feature natively; importing maps to the built-in ([below](#built-in-replacements-for-popular-plugins)). The plugin can still be enabled, but Lightair suggests the built-in |
| **Broken** | Depends on OBS frontend internals (see below) |
| **Unknown** | Not in the database; enabling shows a short warning |

The Plugins page shows the status before anything is enabled.

#### Crash quarantine

Frontend and Qt plugins run in Lightair's process, so a crash in one takes the app down. Lightair records which plugin was executing (from the crash report's stack and the last plugin call in flight). On relaunch it disables that plugin, explains why, and restores the project from the autosave journal ([14](14-data-persistence-settings.md#saving-autosave--recovery)). The user can re-enable it.

#### Not supported

Plugins that depend on **OBS frontend internals** can't work in any frontend except OBS's own: casting the main window to OBS's main window class, finding OBS widgets by object name, or calling private symbols from the OBS executable. The database marks them **Broken** and, where possible, points to a built-in alternative.

### Built-in replacements for popular plugins

When importing an OBS setup, Lightair maps these plugins to built-in features automatically:

| OBS plugin | Lightair built-in |
|---|---|
| Move Transition | Move / Morph transition, layer animations ([07](07-scenes-layers-canvas.md#transitions)) |
| Aitum Vertical / Vertical Canvas | Formats (native multi-canvas) |
| Aitum Multistream / Multiple RTMP outputs | Native multistreaming ([11](11-outputs-streaming-recording.md)) |
| Source Clone | Inputs shared across layers by design; Clone layer |
| StreamFX (blur, 3D transform, shaders) | Video effects catalog ([08](08-video-effects.md)) |
| Shaderfilter / user shaders | Custom Shader effect |
| Advanced Scene Switcher | Scene automations ([07](07-scenes-layers-canvas.md#automations)) |
| Background removal / NVIDIA Broadcast effects | Lightair Effect Stack background ([09](09-effect-stack.md)) |
| Beauty / face filters | Lightair Effect Stack |
| Noise suppression (RNNoise variants), compressor/gate stacks, VST hosts | Voice Clarity, audio effects, plugin host ([10](10-audio.md)) |
| Replay buffer helpers / "Clip" tools | Clips ([11](11-outputs-streaming-recording.md#clips)) |
| Source Record / ISO recording | ISO recording |
| Waveform / audio visualiser | Audio Visualiser layer |
| Text (GDI+/FreeType) with effects, scrolling text | Text layer |
| Countdown / timer scripts | Counter, Timer, Countdown layers |
| Chat overlays via browser source | Chat layer + Chat panel ([12](12-chat-alerts-integrations.md)) |
| obs-websocket | Native remote API (v5 protocol compatible) |
| Downstream Keyer | Global overlay layers |
| Transition table | Per-pair transition overrides |
| Freeze filter, Dynamic delay, Time warp | Freeze, Delay effects |
| 3D Effect, Composite Blur, Exeldro filters | Effects catalog |

## Lightair extensions (JavaScript)

Lightair hosts extensions through **gpui-shell**, gpui-kit's JavaScript extension runtime, so extensions can render **native panels** using Lightair's own component catalog rather than web views.

- **Manifest** (`lightair-extension.json`): id, name, version, entry script, requested capabilities, contributed commands, panels, layer types, settings.
- **Capabilities** are granted one by one by the user at install and can be revoked: `project.read`, `project.write`, `outputs.control`, `chat.read`, `chat.write`, `network:<host>`, `fs.read:<scope>`, `notifications`, `audio.levels`, `hotkeys`.
- **APIs** mirror the remote API (same command and event names), so an extension and an external tool can share code.
- **Panels** are declared in JS and rendered natively with Lightair components; they dock like built-in panels.
- **Overlay layers**: an extension can contribute a layer type rendered by the browser layer from bundled HTML (for visual overlays) with a message channel to the extension.
- **Isolation**: each extension runs in its own JS isolate on a worker thread. Long-running work can't block the UI or the engine; a misbehaving extension is suspended with a notice.
- **Distribution**: local install by drag-and-drop or from a directory; a curated extension index can be added later. Extensions are signed by their author key; unsigned extensions require developer mode.

## Remote API

See [12 · Chat, Alerts & Integrations](12-chat-alerts-integrations.md#remote-api). Two protocols on one server:

- **Lightair protocol**: JSON-RPC over WebSocket exposing the full command set and event stream, with typed schemas published for code generation.
- **obs-websocket v5 compatibility**: the request/event subset that maps cleanly (scenes, inputs, filters, outputs, recording, streaming, transitions, studio mode, media inputs), so existing tools (Stream Deck OBS plugin, Touch Portal, bots, overlay tools) work unchanged.

## Content packs

Packs are zip files with a manifest and assets, installable by drag-and-drop:

| Pack type | Contents |
|---|---|
| Template pack | Scenes with placeholder layers bound to device roles ("main camera", "game capture") |
| Look pack | Lightair Effect Stack looks, makeup looks, stickers, ambiences, lights, grades |
| Transition pack | Stinger media with track mattes and audio, move transition presets |
| Theme | Colour token mapping and optional font |
| Sound pack | Alert and UI sounds |
| Effect presets | Saved effect chains for video and audio |

## Stability contract

- The Lightair protocol and extension APIs are **versioned**; breaking changes require a new major version and a deprecation period during which both versions work.
- The project file format is versioned with forward migrations ([14](14-data-persistence-settings.md)).
- OBS plugin ABI compatibility follows the bundled libobs version; the Plugins page shows which plugins were built for an incompatible version.
