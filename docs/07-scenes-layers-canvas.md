# 07 · Scenes, Layers & Canvas

## Concepts

| Term | Meaning |
|---|---|
| **Input** | A media source: a camera, a display, a file, a browser page. Opened once, reusable anywhere |
| **Layer** | An input (or a generated element like text) placed in a scene, with a transform, crop, blending, effects and visibility, **per format** |
| **Scene** | An ordered stack of layers. One scene is on air (*program*); in studio mode another is *staged* (*preview*) |
| **Format** | An output canvas shape: Landscape 16:9, Vertical 9:16, Square, or custom. Every scene can have a layout in every format |
| **Group** | A layer containing layers, with its own transform and effects |
| **Template** | A reusable scene (or set of scenes) whose inputs are *roles* ("main camera", "game"), bound to real devices when used |

The input/layer split is the core of the model. It's what makes "clones" unnecessary (the same camera appears in ten scenes as ten layers, opened once), and it lets formats share inputs while arranging them differently.

## Formats

- A project has a **primary format** (the libobs main canvas) and any number of **additional formats** (libobs canvases via `obs_canvas_create`; see [04](04-engine-libobs.md)). The common setup is Landscape 1920×1080 + Vertical 1080×1920.
- Every scene has a **layout per enabled format**. A layout holds each layer's transform, crop, visibility and layer effects for that format. Inputs are shared.
- **Adding a format** to an existing project runs **Auto-reframe** on each scene: layers are placed in the new shape using heuristics (camera layers keep the face centred using the face tracker, text scales to fit width, full-frame captures crop to the region of interest) and marked "auto" until the user edits them.
- **Format tabs** switch which format the canvas edits. **Side by side** shows formats next to each other and both are editable; selection is shared, so selecting a layer in one highlights it in the other.
- Each format has a distinct **selection colour** and an **active format indicator**, so it's always clear which layout you're changing.
- **Copy layout from…** copies positions from another format (with auto-fit), per layer or for the whole scene.
- A layer can be **hidden in one format only** (the chat box that only makes sense vertically).
- All formats switch scenes together. Each format's transition can mirror the primary format's or be set separately.
- **Safe areas** show the UI overlays of each destination (TikTok side buttons and caption area, Shorts, Reels, Twitch's player controls) on the relevant format.

## Layer types

Grouped as they appear in the **Add Layer** popover.

### Video

| Layer | Key settings |
|---|---|
| **Camera** | Device, resolution, FPS, pixel format, colour space/range, alternate backend, device controls (exposure, focus, white balance, zoom where supported), Continuity Camera options (Center Stage, Portrait, Studio Light), **Lightair Effect Stack** |
| **Phone camera** | Pair a phone via QR code (companion app); Wi-Fi or USB; lens selection, resolution, FPS, phone mic as track; low-latency SRT transport |
| **Capture card** | Same as Camera, plus audio channel mapping and input format detection |
| **NDI** | Source picker on the LAN, bandwidth (full/low), audio |
| **Network stream** | SRT (listener/caller), RTMP (local ingest server), RTSP, WHEP; latency and reconnect options |
| **Spout2 / Syphon** | Shared-texture inputs from other apps (VTubing software), with transparency |

### Capture

| Layer | Key settings |
|---|---|
| **Display** | Display, cursor show/hide, highlight clicks, exclude Lightair's own windows, crop to region |
| **Window** | Window picker with live thumbnails, match rules (title/app/class) so it re-finds the window, keep transparency |
| **Application** | All windows of an app, auto-follow focused window |
| **Game** | Automatic: captures the active fullscreen or windowed game without per-game setup; manual: pick a game; anti-cheat-safe capture methods; game audio offered as a track |

### Media

| Layer | Key settings |
|---|---|
| **Media** | File or playlist; play when shown / always; restart on show; hide when finished; freeze last frame; loop; A/B loop points; speed; hardware decode; own audio track; transport controls in the Inspector and on Stream Deck |
| **Image** | PNG, JPEG, GIF (animated), WebP, SVG (vector, re-rasterised at any size), AVIF, HEIC; drag-and-drop |
| **Slideshow** | Folder or list, order, duration, transition, loop |
| **Browser** | URL or local HTML; width/height or responsive to layer size; custom CSS; **Interact** mode (click, type, scroll inside the layer); audio as a track; reload; devtools; auto-recover after a crash; page-to-Lightair message channel |

### Text & graphics

| Layer | Key settings |
|---|---|
| **Text** | Rich text with per-run styling; fonts (system and Google Fonts, previewed); weight, size, tracking, leading; fill (colour or gradient); outline; shadow; background box with padding and radius; alignment; auto-size or fixed box with wrap/shrink; emoji; read from file (live); scrolling/marquee; typewriter reveal |
| **Markdown** | Rendered Markdown inline or from a file (live), themed |
| **Shape** | Rectangle (corner radius), ellipse, line, polygon, star; fill, gradient fill, stroke, dash |
| **Gradient / colour** | Linear, radial, conic gradients with multiple stops; solid colour |
| **Counter** | Value, step, prefix/suffix, format, increment/decrement hotkeys |
| **Timer / stopwatch / countdown / clock** | Target time or duration, format, end action (switch scene, play media, show layer), pause/resume/add time via hotkey/API |
| **Audio visualiser** | Source track; bars, wave, circle, spectrum; colours, smoothing, sensitivity |
| **Screen draw** | Annotation layer: pen, highlighter, calligraphy, stamps (arrow, circle, square, ✕), clear; drawing from Lightair or a shared guest link |

### Interactive

| Layer | Key settings |
|---|---|
| **Chat** | On-stream chat (see [12](12-chat-alerts-integrations.md)) |
| **Alerts** | Native alert box |
| **Widgets** | Goals, progress bar, recent activity, spotlight, combo, wheel spin, social handles, chat highlight, subathon timer, confetti, polls, captions |
| **Replay** | Plays the latest clip with scrubbing, A/B loop, speed, own audio track; auto-dismiss |

### Structure

| Layer | Key settings |
|---|---|
| **Group** | Children, collapse, nested up to any depth (UI collapses beyond 3) |
| **Scene** | Another scene as a layer (nesting); live, so changes propagate |
| **Clone** | Mirror a specific layer, group or scene, optionally from another format, with its own transform and effects |
| **Plugin source** | Any compatible OBS input plugin, with generic properties UI |

## Canvas editing

The canvas behaves like a design tool, rendered over the live preview ([03](03-architecture.md#preview--rendering-path)).

### Selection & transform

- Click to select; ⇧/⌘-click to add; marquee-drag on empty canvas; Tab cycles; Esc deselects; ⌘A selects all.
- **Handles**: 8 resize handles, rotation handle above, anchor point (draggable). ⇧ keeps aspect (or frees it when locked); ⌥ resizes from centre; ⇧ while rotating snaps to 15°.
- **Crop**: ⌥-drag a handle for quick crop, or enter crop mode (double-click or **C**) to crop with shaded outside area and drag content inside the crop. Reset crop in one click.
- **Corner radius**: a radius handle at each corner; drag one for all, ⌥-drag for one.
- **Fill mode**: Fit, Fill, Stretch, Original, applied when an input's resolution changes so layouts survive device swaps.
- **Quick shapes for cameras** in the floating toolbar: Square, Rounded, Circle, Vertical crop, Reset.
- **Flip** horizontal/vertical, **opacity**, **blend mode**.
- **Numeric** Inspector fields for X, Y, W, H, rotation, anchor, crop (each scrubbable, accepts maths).
- **Keyboard**: arrows nudge 1 px (⇧ = 10 px), ⌥arrows resize, ⌘⌥arrows rotate, ⌘[ / ⌘] reorder.

### Snapping & alignment

- **Smart guides** to canvas edges and centre, other layers' edges and centres, equal spacing, and user guides; distance labels while dragging. Snapping respects rounded corners and includes locked layers. Hold ⌘ to suspend snapping.
- Threshold and guide colour are settings.
- **Align** (left, centre, right, top, middle, bottom) to selection or canvas; **distribute** horizontally/vertically; **match size**.
- **Rulers**, draggable **guides**, optional **grid** with subdivisions; units in px or %.

### Floating toolbar

Appears above the selection: crop, quick shapes, fill mode, flip, effects (opens the effect picker), Lightair Effect Stack (for cameras), visibility per format, more (…). It never covers the selection; it flips below when near the top edge.

### Masks

- **Shape masks**: rectangle, rounded rectangle, ellipse, polygon, drawn directly on the layer, with feather and invert.
- **Image / alpha masks**: any image, video or layer as an alpha or luma mask.
- **Layer as mask** (clipping): a layer clips the layers above it inside a group, like design tools.
- Masks are part of the layer, edited in place with their own handles.

### Layer animation

- **Show/hide transitions** per layer: fade, slide (direction), scale, wipe, blur-in, with duration and easing. Separate show and hide.
- **Keyframe animations**: animate transform, opacity, crop, corner radius and any numeric effect parameter over a timeline with easing curves (`CurveEditor`). Triggered on show, on scene entry, by hotkey/API, or looped.
- **Audio-driven parameters**: bind any numeric parameter to a track's level (bounce the camera on the beat, pulse a logo).

### Copy, paste & duplicate

- Copy/paste layers within or across scenes and formats; paste keeps position when the target format matches, otherwise auto-fits.
- **Paste style** applies transform-independent styling (effects, blend, radius) from the clipboard.
- **Copy/paste effects** between layers.
- ⌥-drag duplicates; ⌘D duplicates in place.

### Live editing safeguards

When an output is active and studio mode is off, the canvas has a live border, and deleting a visible layer or input asks for inline confirmation (configurable). Undo is always available.

## Scenes

- **Scenes panel**: add, duplicate (inserted below the original), rename, reorder by drag, **folders**, colour labels, hover thumbnails.
- **Scene settings** (Inspector when a scene is selected): transition override, hotkey, entry actions, automations, per-format enablement, notes (visible to the streamer only).
- **Entry actions**: when the scene becomes active: play media, apply a Lightair Effect Stack look, set track volumes/mutes, start a timer, show a layer after a delay, send a chat message, trigger a webhook.
- **Per-scene audio** with unlinked tracks (see [10](10-audio.md#scene-aware-tracks)).

### Studio mode

- Toggle in the canvas toolbar. The canvas splits into **Preview** (staged, editable) and **Program** (on air, read-only), side by side or stacked.
- Clicking a scene stages it; **Take** (or Space / hotkey / Stream Deck) transitions it to program using the selected transition. **Quick transitions** buttons for Cut, Fade and Move.
- **Swap** option: after a take, the previous program scene becomes staged.
- Editing in Preview never touches program unless both are the same scene (shown with a warning badge).

### Automations

Rules attached to scenes or global, built in a simple rule editor (When → If → Then):

| When | Examples |
|---|---|
| Window/app focus | Game window focused → switch to "Gameplay" |
| Media ended | Intro video ends → switch to "Main" |
| Timer/countdown ended | Starting Soon countdown → "Main" |
| Audio activity | Guest mic active for 1 s → switch to guest camera |
| Time / schedule | 21:00 → "Ending" |
| Chat command / event | `!brb` from a mod → "BRB"; raid → play the raid scene |
| Output state | Stream starts → "Starting Soon"; stream stops → stop recording |
| Idle | No mic activity for 10 min → "AFK" |

Conditions can check current scene, live state, time and variables. Actions are any command. Automations are visible and togglable from the Activity panel during a show ("Automation switched to Gameplay · Undo").

### Templates

- **Templates tab** next to Scenes: browse by style and purpose (Starting Soon, Main, BRB, Ending, Just Chatting, Gameplay, Podcast, Tutorial), preview with the user's own devices bound, use with one click.
- Templates come in **landscape and vertical** versions; using one in a project without a vertical format offers to add it.
- **Add while live**: templates insert without switching the current scene.
- **Save as template**: any scenes, with inputs converted to roles; shared as template packs.
- If the project has only one empty scene, using a template fills it instead of adding a new one.

## Transitions

| Transition | Settings |
|---|---|
| **Cut** | – |
| **Fade** | Duration; fade through colour |
| **Move** | Duration, easing (in, out, in-out, custom curve), layer matching (by ID, then name, then rule), reorder mode (none, midpoint, continuous), unmatched layers' behaviour (fade, slide, scale), per-layer overrides (exclude, custom curve) |
| **Morph** | Duration, easing, strength (cross-morph of matched layers plus content dissolve) |
| **Stinger** | Media (WebM/MP4/MOV with alpha), midpoint (auto-detected from the file, adjustable), track matte (none, packed horizontal, packed vertical, separate matte file, chroma key) with invert and key colour, audio on/off and monitoring |
| **Wipes** | Linear (angle), radial, clock, venetian blinds, luma wipe with custom images; softness |
| **Slide / Swipe / Zoom** | Direction, easing |

- **Default transition** per format, **per-scene override** (the transition used when switching *to* this scene), and a **transition table** for specific scene pairs.
- **Preview a transition** inside the Inspector without going to air.
- Transitions are libobs transitions (`lightair-video-fx`), so they apply identically in recordings and every format.

### Move transition details

Matching layers between two scenes is what makes Move feel magical, so it's deterministic and visible:

1. Layers referencing the **same input** match first (the camera in "Main" and in "Just Chatting").
2. Then layers with the **same name**.
3. Then explicit **match rules** (Inspector → Transition → "Match this layer with…").

In the Inspector's transition preview, matched pairs are shown as lines between the two scene thumbnails. Unmatched layers use the "unmatched" behaviour. Crop, corner radius, opacity and effect parameters interpolate too, so a circle camera can grow into a full-frame camera.
