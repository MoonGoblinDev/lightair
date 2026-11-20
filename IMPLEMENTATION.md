# Lightair · Implementation Plan

The design docs in [`docs/`](docs/README.md) describe the finished product. This file is the **build order**: milestones that each end in something that runs, in an order where each one stands on the last. No dates; a milestone is done when its checklist is.

Status key: ✅ done · 🔨 in progress · ⬜ not started

## M0 · Foundation ✅
- Cargo workspace with the crate layout from [03 · Architecture](docs/03-architecture.md#crate-graph).
- GPL-3.0-or-later licence, `.gitignore`, README.
- `cargo xtask` entry point.

## M1 · Engine bootstrap ✅
- `libobs-sys`: bindgen bindings against the pinned libobs headers (33.0.0).
- **Dev runtime**: link against an installed OBS 33 (`/Applications/OBS.app`) and assemble a dev `Lightair.app` bundle whose `Frameworks/` and `PlugIns/` point at OBS's, so OBS core modules resolve their rpaths ([04](docs/04-engine-libobs.md), [15](docs/15-platform-distribution-licensing.md#building)).
- `lightair-engine`: engine actor thread; `obs_startup`, audio/video reset, core module loading with the safe-module allowlist, capability discovery, clean shutdown.
- Headless smoke test: start the engine, create a colour source in a scene, render frames, shut down.

## M2 · Native window with live preview ✅
- `lightair-app` opens a GPUI window (gpui-kit root).
- `lightair-preview`: libobs renders the program canvas into its own render target, a GL blit copies it into a BGRA IOSurface, and VideoToolbox converts it on the GPU to full-range NV12 (the format GPUI's `surface()` draws). No CPU copies.
- A display capture or camera visible live in the window.

## M3 · Core document ✅
- `lightair-model`: Project / Format / Scene / Layer / Input / Track / Output model with stable IDs, commands, validation, undo/redo with gesture coalescing.
- Engine reconciler applies patches to libobs (idempotent).
- `lightair-store`: project package read/write, autosave journal, recovery.

## M4 · Studio shell 🔨
Done:
- Design tokens applied to gpui-component's theme; Lucide icons.
- Window anatomy from [05](docs/05-interface-shell.md): title bar with health strip and primary actions, Layers, Scenes (ON AIR tally), live Canvas, Mixer strip, Outputs, Inspector, status bar.
- Canvas editing: click to select (topmost visible layer), drag to move with snapping guides to canvas edges and centre (⌘ disables), corner-handle resize (⇧ keeps aspect), arrow-key nudge, delete, Esc to deselect.
- Every edit is a command: journaled, undoable (⌘Z / ⇧⌘Z), one undo step per drag gesture; scene switching is not undone.
- Add layer: display, image (file picker), text, colour. Add scene. Layer visibility and lock.
- Inspector: position, size, rotation steppers; fit mode; fill canvas; delete.
- Rename layers and scenes inline (double-click, or Enter on the selection; Enter or clicking away commits); drag rows to reorder layers and scenes.
- Rotation handle above the selection (⇧ snaps to 15°) and crop by ⌥-dragging a corner (the picture keeps its scale; Reset crop in the Inspector). Rotated layers are outlined, hit-tested, resized and cropped in their own frame (the opposite corner stays put); the geometry is unit-tested.
- Multi-select: ⇧/⌘-click in Layers, ⇧-click on the canvas, ⌘A; dragging any selected layer moves them all (one undo step), arrow keys nudge them, Delete removes them, and the Inspector aligns and evenly spaces them by their visible bounds. Dragging on empty canvas draws a selection box (⇧ adds).
- Settings sheet (⌘, or the gear): recording folder, container and bitrate, stream bitrate (project), and "review before going live" (this Mac, in `preferences.json`, tested round trip).
- Inspector Source section: change a layer's camera, display/window/app (screen capture), microphone or audio device, or replace its image/video file. Lists are read from the input itself with its settings applied (`Reconciler::input_options`, tested). Mixer tracks show the same section.
- Shape and crop, together in Transform: a layer (per format) can be a rectangle, rounded box, circle or pill, with softness and a coloured border. A shaped layer is drawn through a one-item libobs group: the crop on the item inside, the GPU shape mask (`lightair_shape_mask`, Lightair's module) on the group, so the shape is cut from the cropped picture and belongs to that layer alone; Move keeps shapes while gliding. A render test checks per-layer shapes, a circle following the crop, and the border.
- The frame hugs the picture: outlines, handles, clicks and drags use the rectangle the (cropped) source actually fills (letterboxed in Fit, own size in Original), from input sizes cached a few times a second; a drag snaps the box to it and Fit resizes keep the picture's shape. Switching to a source with another shape moves the handles with it.
- One set of handles on the canvas: corners resize (⌥ crops), side bars crop (showing the whole picture dimmed while dragging), the top knob rotates, the bottom knob tilts in 3D (drag right/down turns that side away; ⇧ snaps; double-click flattens), ⌥-drag inside slides a cropped picture under its crop. A tilted layer also shows its perspective outline, from the same maths as the filter (unit-tested).
- Flip and 3D tilt: Flip H/V and Tilt sliders in Transform. A GPU filter in Lightair's module (`lightair_transform`) draws the picture through one perspective matrix (perspective divide on the GPU, so texturing is correct), after the shape mask on the layer's group. A render test checks flip, both tilt directions and tilt with a circle.
- Command palette (⌘K): fuzzy search over scenes and actions (Take, Studio mode, record, go live, clip, add layers and scenes, add vertical, import, transitions, undo/redo); ↑↓ ↩ Esc.

Remaining:
- `lightair-ui` crate with the studio components (scrubbable fields, `LayerRow`, `SceneCard`, `FormatTabs`).
- gpui-kit dock layout (resize, collapse, pop-out, workspaces).
- Groups, rulers and guides; per-layer (rather than per-input) shapes.
- Onboarding; the full settings window (appearance, audio devices, hotkeys, advanced encoding).

## M5 · Audio 🔨
Done:
- `Track` and `Effect` in the document (commands, undo, diff, persisted in `audio.json`).
- Reconciler: each global track's input sits on its own output channel; volume, mute and cue (monitoring) map to libobs; effects are libobs audio filters, rebuilt when the chain's shape changes and updated in place otherwise.
- Lock-free meter cells fed by libobs volume meters; the end-to-end test checks a −6 dBFS tone reads −6.0 and drops by exactly the fader amount.
- Mixer panel: channel strips with effect list (bypass dot, remove, presets), L/R meters with zones and falling peaks, dB-taper fader (one undo step per drag, double-click for unity), Cue, Mute, input name. Microphone and desktop-audio tracks; new mic tracks get the Voice Clarity chain.

- Stream loudness (EBU R128): K-weighted momentary, short-term and gated integrated LUFS on the stream mix, from libobs's raw audio callback; the Mixer shows short-term and integrated (click resets; going live resets). Reference tones from BS.1770 read exactly (−23 LUFS for 1 kHz at −20 dBFS on one channel), and the engine test reads −6.0 / −26.0 LUFS through a real track and fader.

Remaining:
- Voice Clarity as one native effect with calibration (`lightair-audio-fx`), plus EQ, de-esser, ducking, reverb, delay, pitch.
- Mixes and routing (Stream, Recording n, Cue, VOD), master chains, true peak, per-platform loudness targets and Normalise.
- Scene tracks, per-scene volume (link/unlink), folders, channel remap, sync offset and auto-sync, VST3/AU host, hardware mixer integrations.

## M6 · Outputs 🔨
Done:
- Engine output manager: recording (hybrid MP4, MKV, MOV) and RTMP(S) streaming, each with its own encoders; Apple hardware H.264 picked automatically. libobs output signals drive a runtime status table (starting, active, reconnecting, stopping, failed with a plain-language reason).
- End-to-end test, verified with ffprobe: a recording has H.264 + AAC of the right length, and a stream reaches a local ffmpeg RTMP server.
- Output settings and destinations in the document (`outputs.json`); stream keys only in the Keychain.
- Clips: a libobs replay buffer kept running while Clips is on (length and format per project); the Clip button saves the last seconds; a test saves a 3 s clip and probes it. libobs writes clips through OBS's `obs-ffmpeg-mux` helper, which `cargo xtask dev` / `bundle` link next to the executable.
- Destinations by URL: rtmp(s):// (RTMP), srt:// / rist:// / udp:// / tcp:// (MPEG-TS; the key is the SRT stream ID) and http(s):// (WHIP, with Opus audio and no B-frames). A test streams over real SRT to a listener in the same engine, over UDP to ffmpeg (H.264 + AAC received), and checks an unreachable WHIP endpoint fails in plain language.
- Go Live review sheet: choose destinations, then plain-language checks (stream keys, encoder, rendering keeping up, upload estimate with shared encoders, each mic's level over the last 10 s for silence or clipping, disk space when also recording); blocking problems disable Go live. The checks are a pure, tested function; mic windows come from a lock-free loudest-peak value on each meter.
- Encoder sharing: destinations with identical encoder settings on the same canvas share one video and audio encoder (the Outputs panel shows "N destinations · M encoders" while live). A test streams to three local servers: two identical ones share, a different bitrate gets its own, and stopping one shared stream leaves the other running.
- Recording pause/resume (the timer shows recorded time), markers as MP4/MOV chapters (⌘M), and split into a new file without a gap, from the title bar and the palette. A test checks with ffprobe that paused time is left out, the chapter is there, and the split produced a second file.
- Replay: after a clip is saved, Replay (title bar or palette) plays it full-frame on top of the live scene; the replay layer removes itself when the clip ends.
- A headless self-test inside the app bundle (`LIGHTAIR_SELFTEST=camera`) confirms camera frames with Lightair Effect Stack attached.
- UI: Record with live timer, Go live for every enabled destination, LIVE badge and live canvas border, two-click End stream, Outputs panel with recording status, destination rows (toggle, status, remove) and an add-destination form.

Remaining:
- Per-destination bitrates and encoder settings (sharing then splits automatically).
- Stream manager (titles, categories) and platform sign-in (with M11); camera-frozen and bandwidth checks in the Go Live sheet; a setting to skip the sheet.
- Clip library (browse, trim, share), replay speed and scrubbing; virtual camera, NDI, SRT listener mode and latency settings, a WHIP test against a real server, ISO and multitrack recording, markers for MKV (sidecar file), test recording.

## M7 · Formats 🔨
Done:
- Every scene exists once per format: one libobs scene per (scene, format) in that format's canvas (`obs_canvas_create`, activate + scene-ref, audio mixed once), sharing inputs. Program switches every canvas. The reconciler syncs formats from the snapshot, and a test checks incremental reconciliation matches a full rebuild across two canvases.
- Previews for any canvas (`obs_canvas_render` into the preview pipeline).
- Format tabs, side-by-side editing (the pointer edits whichever stage it is over), "Add vertical" with auto-reframe (backgrounds fill, other layers keep their arrangement in a centred band). Layers panel and inspector follow the active format; new layers get a layout in every format.
- Recording and each destination can target a format (aspect badge); a vertical recording is 1080×1920.
- Add format: Vertical 9:16, Square 1:1, Portrait 4:5 or a custom size (even pixels, 16–7680), each with every layer reframed; extra formats can be removed from their tab (undoable; outputs that targeted them fall back to the primary format).
- Inspector "Formats" section: show or hide the layer per format, and "Use this layout" copies another format's layout into the one being edited (reframed to its shape).
- Safe-area overlays: a chip cycles Title/Action safe and Twitch player controls on landscape stages, TikTok, Shorts and Reels UI zones on vertical ones (approximate zones, drawn as GPUI overlays).

Remaining:
- Per-destination safe areas (from the destination's platform), smarter reframe (face-aware with Lightair Effect Stack), Twitch Dual Format.

## M8 · Effects & transitions 🔨
Done:
- Transition settings in the document (Cut, Fade, Move; duration). Every format's output channel goes through its own libobs transition.
- Move: a temporary scene per format where layers that continue (same input, then same name) glide from A's layout to B's with easing, driven by a libobs tick callback; entering layers fade in, leaving layers fade out; the canvas then lands on B. A test samples the glide halfway and checks the landing.
- Transition picker under Scenes: Cut, Fade, Move, Slide, Wipe, Dip (through black), with duration and a direction for Slide and Wipe. Each format's output transition swaps to the right libobs type when the kind changes; a test samples Slide, Wipe and Dip halfway through. Imported OBS transitions map onto these.
- Video effects on any layer (Inspector → Effects): Colour correction, Green screen, Luma key, Sharpen, Scroll, Delay from libobs's core filters, each with the few controls that matter; on/off, remove, and imported plugin filters listed as plugins. A render test checks Colour correction and Green screen with the exact settings the app sends.
- Stinger transition: the project's stinger video (with its cut point) plays over the switch through libobs's stinger; the transition is set up when chosen so the video is loaded before the switch. OBS stingers import with their video and cut point. A render test plays a generated stinger and checks it shows, then lands.
- Studio mode: the stage shows the scene you're editing (rendered off air from its libobs scene, held "showing" so its inputs run), a Program monitor beside it, and Take (⌘↩) puts it on air with the current transition. A test checks the preview renders an off-air scene while Program stays put.

Remaining:
- Morph, stinger track mattes, more wipe shapes; per-scene and per-pair overrides; per-format transitions; studio mode for additional formats (they show Program today).
- More effects (blur, LUT and mask with file pickers, presets, drag to reorder the stack); masks; layer animations.

## M9 · Lightair Effect Stack 🔨
Done:
- `modules/lightair-effect-stack`: the Dewy pipeline (pigment-model skin, Vision tracking, Core ML segmentation, makeup, effects, atmosphere, colour, light) built in-tree as a libobs module without Qt or the frontend API, rebranded (`lightair_camera_studio` filter, "Lightair Effect Stack"); `cargo xtask modules` builds it. A test loads it, renders through it, and checks it adds no lag once warm.
- Input effects in the document (applied wherever the input appears), with replace-not-merge settings so a look resets what it doesn't set.
- Camera layers from the device list; new cameras get Lightair Effect Stack (off until a look is chosen). Inspector section: on/off, every look by category, and skin controls.

Remaining:
- The full Lightair Effect Stack panel (Shape, Makeup, Effects, Atmosphere, Colour, Light tabs; thumbnails rendered from the user's face; before/after; saved looks; look packs).
- Background blur/remove/replace, auto-frame, multi-face (new features beyond Dewy).
- Verify with a live camera (needs the macOS camera permission for Lightair).

## M10 · Layers & creator features 🔨
Text, shapes, counters/timers, browser layer (CEF), media, phone camera, NDI, screen draw, templates, automations ([07](docs/07-scenes-layers-canvas.md)).

Done:
- Video layer (`ffmpeg_source`, loops and restarts when its scene goes on air) with transport in the Inspector: play/pause, restart, loop, position and duration. Transport is runtime state through the engine (`media_status`, `media_action`); a test plays, pauses, seeks and restarts a file.
- Inspector content sections: text (content, size, font, bold/italic, outline, shadow, colour swatches) and colour layers; edits are undoable input-settings changes.
- Timer layers: countdown (starts when its scene goes on air; start, pause, reset, +1 min; end text; "then go to" a scene), stopwatch and clock. The document holds the settings; a libobs tick rewrites a text source once a second from runtime state, so ticking never touches undo. A test covers autostart on air, pause, adding time, the end event and reset.

Remaining:
- Text backgrounds, gradients and any installed font, shapes, counters, timer hotkeys, browser layer (CEF), phone camera, NDI, screen draw, templates, automations; playlists and media scrubbing.

## M11 · Audience & control 🔨
Platform sign-in, stream manager, unified chat, activity, alerts, widgets, captions, hotkeys, Stream Deck, MIDI/OSC, remote API ([12](docs/12-chat-alerts-integrations.md)).

Done:
- In-app shortcuts: ⌘1–⌘9 for scenes 1–9 (shown on the scene rows; Studio mode selects instead), ⇧⌘M mutes or unmutes every microphone, plus ⌘K, ⌘↩, ⌘M, ⌘A, ⌘, from earlier slices.

Remaining:
- Global hotkeys and a hotkey settings page, Stream Deck, MIDI/OSC, remote API (Lightair protocol and obs-websocket compatibility), platform sign-in and the rest of the audience features.

## M12 · Compatibility & import 🔨
OBS plugin classifier, frontend API shim, custom browser docks, Qt runtime, OBS scene collection import ([13](docs/13-extensibility-and-compatibility.md), [14](docs/14-data-persistence-settings.md#importing-from-obs)).

Done:
- OBS scene collection import (`lightair_store::import_obs_collection`): sources become inputs of the same libobs type and settings, filters become input effects, items become layers (alignment point + scale/bounds → box, crop, visibility, lock), global and in-scene audio sources become mixer tracks (volume in dB, mute, monitoring → cue), transition and program scene carry over, scene names stay unique. Dewy becomes Lightair Effect Stack with its settings. One undo step, with a report: converted, replaced by built-in features, kept as plugins, needs attention. "Import OBS scenes…" button in the Scenes header. Dry run: `cargo run -p lightair-store --example obs_import_report -- <collection.json>`.
- Layers whose source size OBS didn't save are corrected after import: the app holds those inputs showing off air until they report a size (or 8 s pass), then refits every such layer the user hasn't touched, as one undo step ("Size imported layers"). A test reads a real image's size off air and checks the refit box.

Remaining:
- Import: nested scenes and groups, per-scene audio once scene tracks exist, importing OBS profiles (outputs, encoders, stream service).
- Plugin classifier, frontend API shim, custom browser docks, Qt runtime.

## M13 · Engine from source & distribution 🔨
Done:
- `cargo xtask bundle`: a development `Lightair.app` (Info.plist with camera/microphone usage strings, the brand icon, OBS frameworks and plugins linked from an installed OBS 33, ad hoc signature), so macOS asks Lightair itself for privacy permissions.
- Windows port: builds, passes the suite and runs on Windows 11; `cargo xtask dist` makes the self-contained portable app and zip (OBS's layout, libobs and modules from an installed OBS 33, the MSVC runtime beside it) ([plans/windows.md](docs/plans/windows.md)).

Remaining:
- Build libobs and core modules from the submodule; release bundle with everything copied in; Developer ID signing and notarisation (needs your certificate and Apple ID); updater; a Windows installer and Authenticode signing; the Linux port.

## M14 · Quality
Performance budgets in CI, golden renders, A/V sync, soak tests, accessibility and localisation passes ([16](docs/16-quality-performance-accessibility.md)).
