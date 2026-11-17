# 05 · Interface: Shell, Layout & Interaction

This document defines the app's structure on screen and how people move through it. The visual language (colour, type, components) is in [06 · Design System](06-design-system.md).

## Window anatomy

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ ● ● ●  [Project ▾]  [Feedback]          Health ● Disk  FPS  CPU  │ Go Live │ Record │ Clip │  Title bar
├───────────────┬───────────────────────────────────────────────────┬──────────────────┬─┤
│ LAYERS     +  │                                                   │ OUTPUTS       +  │▣│
│  T Title      │                                                   │  ▶ YouTube  ◯    │☰│
│  ⌗ Counter    │              CANVAS (format tabs ▸ 16:9 | 9:16)   │  ◎ Virtual Cam   │💬│
│  ▣ Display    │                                                   │                  │⚡│
│  ◉ Camera  ✦  │                                                   │──────────────────│ │
│  ▶ Media      │                                                   │ INSPECTOR        │ │
│               │                                                   │  (selection)     │ │
├───────────────┤                                                   │                  │ │
│ SCENES  TEMPL │───────────────────────────────────────────────────│                  │ │
│  Intro        │ AUDIO MIXER  +                                 ⌄  │                  │ │
│ ▌Screen View  │ ┌Track─┐ ┌Track─┐ ┌Track─┐                        │                  │ │
│  Full camera  │ │ fx   │ │ fx   │ │ fx   │   Master: Stream ▾     │                  │ │
│  BRB          │ │meter │ │meter │ │meter │                        │                  │ ⚙│
│               │ └──────┘ └──────┘ └──────┘                        │                  │ ◉│
├───────────────┴───────────────────────────────────────────────────┴──────────────────┴─┤
│ ⚠ 1 issue · Rec 00:12:31 · 4.2 GB · Stream 6,000 kbps · 0 dropped           v1.0 · ⌘K   │  Status bar
└────────────────────────────────────────────────────────────────────────────────────────┘
```

| Region | Purpose | Default |
|---|---|---|
| **Title bar** | Project switcher, feedback, live health strip, primary actions (Go Live, Record, Clip) | Always visible |
| **Left column** | **Layers** (top) for the current scene; **Scenes / Templates** (bottom), split with a draggable divider | Visible |
| **Centre** | **Canvas** with format tabs; **Audio Mixer** below, collapsible to a slim meter strip | Visible |
| **Right column** | Context panel switched by the **activity rail**: Outputs, Inspector, Chat, Activity, Stats | Outputs + Inspector |
| **Activity rail** | Icons switching the right column: Outputs ◇, Inspector ☰, Chat 💬, Activity ⚡ (events, alerts, issues), Lightair Effect Stack ✦, Settings ⚙, Account | Visible |
| **Status bar** | Issues, timers, file size, bitrate, dropped frames, version, command palette hint | Visible |

The whole layout is a **gpui-kit dock layout**: every panel can be resized, collapsed, rearranged, tabbed together, or popped out to its own window (Chat and Multiview are the common pop-outs). Layouts are saved per project and per display configuration, with named **Workspaces**: *Edit* (large inspector), *Live* (large chat and activity, compact mixer), *Minimal* (canvas + primary actions only). Switching workspace never changes the show, only the window.

## Primary actions

| Action | Behaviour |
|---|---|
| **Go Live** | Opens the **Go Live review** sheet: destinations with toggles, title/category/tags per platform, format per destination, health pre-check (encoder, bandwidth estimate, mic levels in the last 10 s, camera present), "Record too" toggle. One button starts everything. While live the button turns red, shows elapsed time, and its menu offers per-destination stop. |
| **Record** | Starts recording with the project's recording profile. While recording: elapsed time, size, and a pause control. Its menu offers **Test recording** (see below). |
| **Clip** | Saves the replay buffer (default 60 s) for every enabled format. Shows a toast with a thumbnail, name, and "Reveal / Share / Trim". Disabled with an explanation if the replay buffer is off. |

**Test recording** ("Rehearse"): records a short clip using the exact stream encoder settings, then opens an in-app review player with level meters and a waveform, so the user can see and hear what viewers will get before going live.

## Core panels

### Layers

- Rows show type icon, name, inline status badges (missing device ⚠, live audio, Lightair Effect Stack ✦ on), and on hover: visibility 👁, lock 🔒, and a quick-effects button.
- Drag to reorder; drag onto another layer to group; drag from Finder/Explorer to add media.
- Multi-select with ⇧/⌘; selection is shared with the canvas.
- Double-click renames. Right-click gives the full layer menu (also available via the command palette).
- **+** opens the **Add Layer** popover: searchable, grouped (Video, Capture, Media, Text & Graphics, Interactive, Audio, Templates), with live device thumbnails and "recently used".
- A layer shows which formats it appears in (landscape, vertical) and can be hidden per format.

### Scenes & Templates

- Scenes list with live thumbnails on hover (or always, as a setting), drag to reorder, folders for organising large shows.
- The active (program) scene is marked with a bright bar; in studio mode the staged (preview) scene gets a second, distinct marker.
- **Templates** tab: built-in and user templates (layouts like "Camera + Screen", "Just Chatting", "BRB", "Starting Soon", "Podcast 2-up") that instantiate layers wired to the user's actual devices. "Save scene as template" turns any scene into a template.
- Each scene has optional **scene settings**: transition override, hotkey, auto-switch rules (for example, switch when a game window gains focus or after a countdown ends).

### Canvas

Covered in detail in [07 · Scenes, Layers & Canvas](07-scenes-layers-canvas.md). Key shell behaviours:

- **Format tabs** at the top (Landscape, Vertical, …) plus a **Side by side** mode that shows formats next to each other, each editable.
- Zoom (fit, 50/100/200 %, pinch, ⌘+scroll), pan (space-drag), rulers and guides toggled from a canvas toolbar that appears on hover.
- Safe-area overlays per destination (TikTok UI zones, YouTube Shorts buttons, Twitch overlay zones).
- **Studio mode** toggle splits the canvas into Preview (editable) and Program (live) with a **Take** button and transition picker between them.

### Audio Mixer

Covered in [10 · Audio](10-audio.md). A horizontal strip of **tracks** under the canvas, each a channel strip: name and input selector, effects list with per-effect bypass dots and **+** to add, meter, fader, and **Edit / Cue / Mute** buttons plus link. A collapsed mode shows only names, meters and mute.

### Outputs

Destinations (streaming platforms, custom RTMP/SRT/WHIP, recordings, virtual camera, NDI), each with a toggle, format badge, and health. Add with **+**. Covered in [11 · Outputs](11-outputs-streaming-recording.md).

### Inspector

Context-sensitive properties for the current selection: layer (transform, crop, blending, per-format visibility, effects), input (device, resolution, input effects), track, transition, output, or scene. Sections are collapsible and remember their state. Multi-selection edits shared properties. Every numeric field supports scrubbing (drag the label), arrow-key nudging, typed maths (`+10`, `*2`), and resetting to default (double-click the label).

### Chat & Activity

Unified multi-platform chat with moderation actions; Activity shows follows, subs, raids, donations, alerts, markers, clips and issues in one timeline. See [12 · Chat, Alerts & Integrations](12-chat-alerts-integrations.md).

### Lightair Effect Stack

The beauty and camera look workspace. It opens as a right-column panel when a camera layer is selected (✦ on the rail), or full-height for deep editing. See [09 · Lightair Effect Stack](09-effect-stack.md).

### Plugin panels

Custom browser docks and panels from OBS plugins (through the opt-in Qt runtime) appear as ordinary dock panels: they tab, pop out and save with the workspace. Plugin panels carry a small **plugin** badge, and plugin menu items appear under **Plugins** and in the command palette. See [13 · Extensibility](13-extensibility-and-compatibility.md#compatibility-layer-for-frontend-plugins).

## Command palette & keyboard

- **⌘K / Ctrl+K** opens the command palette: every command, every scene, every layer, every setting page, fuzzy-searchable, with shortcuts shown beside each result.
- Every command can be bound to a key (in-app or **global**, so it works while a game has focus), a MIDI message, a Stream Deck key, or an OSC address. One registry, so a binding change is reflected everywhere.
- Default bindings follow platform conventions (⌘Z undo, ⌘D duplicate, ⌘G group, arrows nudge, ⇧arrows nudge ×10, ⌥-drag duplicate, Delete removes, ⌘1–9 switch scene).
- Keyboard focus is always visible and follows a predictable order: rail → left column → canvas → mixer → right column.

## Live-safety interactions

Because the canvas is live by default (outside studio mode), editing has guard rails:

- **Live badge**: when outputs are active, the canvas frame shows a thin red border and "LIVE" so it's obvious that edits are visible to the audience.
- **Soft confirmations** only for destructive changes visible on air: deleting the program scene, removing a live layer's input, stopping a stream. Confirmations are inline popovers, not modal dialogs, and can be disabled.
- **Panic controls**: global hotkeys for "Mute all mics", "Privacy cut" (switch to a chosen safe scene instantly, mute everything, and blur the canvas), and "Stop all outputs".
- **Undo everywhere**, including during live, with the undo toast naming what changed ("Undo: Move 'Camera'").

## Onboarding

1. **Welcome**: choose a starting point: *Streaming*, *Recording*, *Podcast*, *Import from OBS*.
2. **Permissions**: one screen explaining camera, microphone, screen recording and (optional) accessibility/input monitoring for global hotkeys, with buttons that trigger each OS prompt and live status ticks.
3. **Devices**: camera with live preview (and a Lightair Effect Stack look picker), microphone with a level meter and an automatic Voice Clarity calibration (record 5 s of speech, 5 s of silence), display selection.
4. **Destinations**: sign in to platforms (optional), or skip.
5. **Starter scenes**: generated from the choices above (Starting Soon, Main, BRB, Ending) using built-in templates.
6. **Auto-config**: a bandwidth and encoder test proposes resolution, FPS, bitrate and encoder, with plain-language reasoning.

Import from OBS reads a scene collection and profile, maps sources and filters (see [14 · Data](14-data-persistence-settings.md#importing-from-obs)), and shows a report of what was converted, what was replaced by a built-in feature, and what needs attention.

## Windows & multi-display

- **Pop-out panels**: any dock panel but the canvas can become its own window (right-click its tab › **Open in New Window**) and dock back (its window's ↙ button, or closing the window: it returns to the region it came from).
- **Docking beside a whole column**: dropping a tab in the thin band along a panel's outer edge puts it beside the whole column (or row) that edge belongs to, full height: two sidebars side by side, the dock widening by a column.
- **Projectors**: fullscreen or windowed output of any canvas, scene, multiview, or a single layer, on any display. Used for confidence monitors and capture by other tools.
- **Multiview**: grid of scenes with program/preview markers, click to stage, double-click to take; configurable layouts (2×2, 3×3, 1+4, 1+7).
- **Menu bar / tray extra**: live state, timers, start/stop, mute, scene switch, for use while Lightair is hidden.

## Settings

A single Settings window (or panel in the Minimal workspace) with search, organised as: **General**, **Appearance**, **Video & Formats**, **Audio**, **Outputs & Encoding**, **Recording & Clips**, **Accounts**, **Hotkeys & Controllers**, **Remote API**, **Lightair Effect Stack**, **Plugins**, **Advanced**, **About & Diagnostics**. Every setting has a one-line explanation and a reset. Settings that are risky while live are disabled with an explanation instead of failing.
