# 02 · Feature Catalog

The complete list of what Lightair does. Each feature has an ID used across the docs, issues and tests. Detailed behaviour lives in the linked documents.

**Origin tags**

| Tag | Meaning |
|---|---|
| **M** | Modern-studio parity (dedicated streaming apps have it) |
| **O** | OBS Studio parity (OBS has it natively) |
| **P** | Built-in replacement for a popular OBS plugin |
| **C** | Lightair Effect Stack (from the Dewy beauty pipeline) |
| **K** | Lightair original: not in dedicated streaming apps, nor in OBS without plugins |

A feature can carry several tags. The catalog has no priorities or dates; it is the definition of the finished product.

---

## 1. App shell & workspace ([05](05-interface-shell.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| SH-01 | Panel layout | Layers, Scenes/Templates, Canvas, Audio Mixer, Outputs, Inspector, Chat; title-bar stats and primary actions | M |
| SH-02 | Dock system | Resize, collapse, tab, rearrange and pop out any panel; layouts saved per project and per display setup | K |
| SH-03 | Workspaces | Named layouts (Edit, Live, Minimal, custom), switchable by hotkey | K |
| SH-04 | Swap layout | Mirror the whole layout left↔right | M |
| SH-05 | Window state memory | Window size, position and panel state restored on launch | M |
| SH-06 | Command palette | ⌘K to reach every command, scene, layer and setting | K |
| SH-07 | Context menus | Right-click menus on layers, canvas, scenes, tracks, outputs | M O |
| SH-08 | Floating layer toolbar | Quick actions above the selected layer on the canvas | M |
| SH-09 | Undo / redo | Unlimited, everything including effects and audio; persisted across restarts | M K |
| SH-10 | Onboarding | Permissions, devices, destinations, generated starter scenes and starter mixer | M |
| SH-11 | Auto-configuration | Bandwidth and encoder test proposes resolution, FPS, bitrate, encoder | O |
| SH-12 | Missing source assistant | Lists missing devices/windows/files and relinks them across all scenes at once | M |
| SH-13 | Permission indicators | Layers show missing OS permissions with a fix button | M |
| SH-14 | Issues & health | Plain-language issues with fix actions in the status bar and Activity | M K |
| SH-15 | Stats bar & Stats panel | Disk, FPS, CPU, render/encode time, dropped frames; per-layer and per-effect cost | M O K |
| SH-16 | Encoder view | What each encoder and output is doing, live | M |
| SH-17 | Capture device stats | FPS graph per capture device | M |
| SH-18 | Projectors | Fullscreen/windowed output of a canvas, scene, multiview or layer on any display | O |
| SH-19 | Multiview | Grid of scenes with program/preview markers; click to stage, double-click to take | O |
| SH-20 | Menu bar / tray control | Live state, timers, start/stop, mute, scene switch while hidden | K |
| SH-21 | Screenshot | Save a still of any canvas (hotkey) | M O |
| SH-22 | Multiple instances | Run a second instance with its own data folder | M O |
| SH-23 | Discord presence | Optional Discord Rich Presence | M |
| SH-24 | Themes | Dark, Light, High Contrast; custom accent; community themes | K |
| SH-25 | Localisation | 12 launch languages, RTL-ready | O K |
| SH-26 | In-app feedback | Feedback with optional diagnostics attachment | M |
| SH-27 | Auto-update | Stable/Beta/Nightly channels; never installs while live | M O |
| SH-28 | Panic controls | Mute all mics, privacy cut, stop all outputs (global hotkeys) | K |

## 2. Projects, scenes & templates ([07](07-scenes-layers-canvas.md), [14](14-data-persistence-settings.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| SC-01 | Projects | A show = scenes + inputs + audio + outputs + hotkeys; switch between projects | M O |
| SC-02 | Autosave & journal | Every change persisted; crash recovery replays the journal | M K |
| SC-03 | Backups & history | Automatic snapshots, browsable history, restore as copy, export backup | M K |
| SC-04 | Import / export project | Portable project package, assets collected, secrets excluded | M O |
| SC-05 | Import from OBS | Scene collection + profile, with a conversion report and plugin-to-built-in mapping | M K |
| SC-06 | Scenes | Add, duplicate (placed below), rename, reorder, folders, thumbnails | M O K |
| SC-07 | Scene hotkeys | Bind any scene to a key | M O |
| SC-08 | Studio mode | Preview/Program with Take, staging from multiview, Stream Deck and API | O M |
| SC-09 | Nested scenes | Use a scene as a layer in another scene | O M |
| SC-10 | Scene templates | Built-in and user templates, landscape + vertical versions, preview before use, add while live | M |
| SC-11 | Save as template | Turn scenes into reusable templates with device roles | K |
| SC-12 | Template packs | Install/share template packs | K |
| SC-13 | Scene automations | Switch on window focus, after media ends, after countdown, on schedule, on audio activity, on chat command | P K |
| SC-14 | Scene-entry actions | Play media, apply a camera look, set track volumes when a scene becomes active | M K |
| SC-15 | Starting Soon / BRB / Ending flow | Built-in show flow with countdown that auto-switches to the main scene | K |
| SC-16 | Global overlay layers | Layers that sit above every scene (downstream keyer) | P |
| SC-17 | Per-scene audio | Track volume/mute per scene when unlinked | M |

## 3. Formats / multi-canvas ([07](07-scenes-layers-canvas.md#formats))

| ID | Feature | Description | Origin |
|---|---|---|---|
| FM-01 | Multiple formats | Landscape + Vertical (+ Square, custom) canvases in one project | M P |
| FM-02 | Per-format layout | Each scene has a layout per format sharing the same inputs | M P |
| FM-03 | Side-by-side editing | Edit formats next to each other | K |
| FM-04 | Copy between formats | Copy layers/layouts from one format to another; auto-fit | M |
| FM-05 | Per-format visibility | Hide a layer in one format only | K |
| FM-06 | Format indicators | Distinct selection colour per format, active-format indicator | M |
| FM-07 | Per-format transitions | Mirror main transitions or choose separately | M |
| FM-08 | Safe-area overlays | Platform UI zones (TikTok, Shorts, Reels, Twitch) | K |
| FM-09 | Canvas presets | 4K, 1440p, 1080p, 864p, 720p, vertical 4K/1080p/720p, ultrawide, custom | M O |
| FM-10 | Frame rates | 24, 25, 29.97, 30, 48, 50, 59.94, 60, 120, per format divisors | O K |
| FM-11 | Colour management | sRGB/Rec.709/Rec.2100 (PQ/HLG), 8/10-bit, range | O |
| FM-12 | Auto-reframe | Generate a vertical layout from a landscape scene using face/subject positions | K |

## 4. Layers & inputs ([07](07-scenes-layers-canvas.md#layer-types))

| ID | Feature | Description | Origin |
|---|---|---|---|
| LY-01 | Camera / video device | Webcams, capture cards, virtual cameras; resolution, FPS, format, colour, alternate backend, device properties | M O |
| LY-02 | Display capture | Monitor capture (ScreenCaptureKit / DXGI / PipeWire), cursor options | M O |
| LY-03 | Window capture | Single window, keeps transparency on macOS | M O |
| LY-04 | Application capture | All windows of an app | O |
| LY-05 | Game capture | Automatic fullscreen/windowed game detection, no per-game setup | M O |
| LY-06 | Media | Video/audio files and playlists: play on show, hide on end, freeze last frame, loop, A/B loop, speed, hardware decode | M O |
| LY-07 | Image | PNG/JPG/GIF/WebP/SVG/AVIF/HEIC, drag-and-drop | M O |
| LY-08 | Image slideshow | Folder or list, transitions, timing | O |
| LY-09 | Browser | URL or local HTML, interact mode, responsive/locked size, audio track, devtools, auto-recover | M O |
| LY-10 | Text | Rich text: fonts (system + Google Fonts), weight, size, colour, gradient, outline, shadow, background, emoji, read from file, scrolling/marquee, typewriter | M O P |
| LY-11 | Markdown | Markdown text inline or from a live file | M |
| LY-12 | Shapes | Rectangle, rounded rectangle, ellipse, line, polygon; fill, gradient fill, border | M K |
| LY-13 | Colour / gradient | Solid or gradient fill layer | O P |
| LY-14 | Clone (duplicator) | Mirror a scene, layer or group, including across formats | M P |
| LY-15 | Group | Group/ungroup, nest, collapse, lock, hide, edit children in place | M O |
| LY-16 | Phone camera | Use a phone as a wireless/USB camera via a companion app (and Continuity Camera on macOS) | K |
| LY-17 | NDI input | Receive NDI sources | P |
| LY-18 | Network stream input | SRT, RTMP, RTSP, WHEP input | O K |
| LY-19 | Spout2 / Syphon input | Shared-texture input from other apps (VTube Studio etc.) | M O |
| LY-20 | Audio visualiser | Waveform/spectrum/bars layer from any track | P |
| LY-21 | Counter, timer, stopwatch, countdown, clock | Native, styleable, hotkey/API-controllable | M P |
| LY-22 | Screen draw | Annotation layer: pen, highlighter, stamps; shared drawing session | M |
| LY-23 | Replay layer | Plays the last clip with scrub, A/B loop, speed, own audio track | M |
| LY-24 | Widgets | Event overlays (see §12) as native layers | M |
| LY-25 | Plugin sources | Any compatible OBS source plugin | O |
| LY-26 | Automatic naming | Layers named from device/window/file | M |
| LY-27 | Device placeholders | Styled placeholder while a device is missing | K |

## 5. Canvas editing ([07](07-scenes-layers-canvas.md#canvas-editing))

| ID | Feature | Description | Origin |
|---|---|---|---|
| CV-01 | Direct manipulation | Move, resize (from centre with ⌥), rotate (⇧ snaps 15°), flip | M O |
| CV-02 | Numeric transform | Position, size, rotation, anchor, aspect lock in the Inspector | M O |
| CV-03 | Opacity | Per layer | M O |
| CV-04 | Corner radius | Uniform drag or per-corner with ⌥ | M |
| CV-05 | Crop | Non-destructive crop mode, reposition content inside the crop, quick-crop with ⌥-drag | M O |
| CV-06 | Camera quick shapes | Square, rounded, circle, vertical crop, reset | M |
| CV-07 | Fill modes | Fit, Fill, Stretch, Original | M O |
| CV-08 | Blend modes | Normal, Multiply, Screen, Overlay, Soft/Hard Light, Darken, Lighten, Color Dodge/Burn, Difference, Exclusion, Hue, Saturation, Color, Luminosity, Add, Subtract | M O |
| CV-09 | Masks | Shape masks, image/alpha masks, luma masks, layer-as-mask, feather | P K |
| CV-10 | Smart guides & snapping | Edges, centres, canvas, other layers, rounded corners, locked items; threshold and colour settings | M O |
| CV-11 | Align & distribute | Align selection to each other or the canvas; distribute evenly | M K |
| CV-12 | Rulers & guides | Draggable guides, grid, percent/pixel units | K |
| CV-13 | Selection | Click, ⇧/⌘-click, marquee, select all, Esc | M |
| CV-14 | Copy/paste layers | Within/across scenes and formats; paste style; duplicate with ⌥-drag | M O K |
| CV-15 | Copy/paste effects | Copy effect chains between layers | M |
| CV-16 | Lock / hide / rename / reorder | In panel and on canvas | M O |
| CV-17 | Zoom & pan | Fit, 50/100/200 %, pinch, space-drag | K |
| CV-18 | Keyboard editing | Nudge, resize, rotate, cycle selection from the keyboard | K |
| CV-19 | Layer animations | Keyframed position/scale/rotation/opacity/effect params with easing; triggered on show/hide or by hotkey | P K |
| CV-20 | Show/hide transitions per layer | Fade, slide, scale, wipe on visibility change | O P |
| CV-21 | 3D transform | Perspective rotation in X/Y with camera FOV | P |

## 6. Transitions ([07](07-scenes-layers-canvas.md#transitions))

| ID | Feature | Description | Origin |
|---|---|---|---|
| TR-01 | Cut | Instant | M O |
| TR-02 | Fade | Duration, fade through colour | M O |
| TR-03 | Move | Matching layers animate between scenes; easing, duration, reorder modes (none/midpoint/continuous); match by ID, name or rule | M P |
| TR-04 | Morph | Cross-morph with easing and strength | M |
| TR-05 | Stinger | WebM/MP4/MOV with track matte (packed horizontal/vertical, matte only, chroma), invert, audio, auto midpoint | M O |
| TR-06 | Wipes | Linear, radial, clock, venetian blinds, luma wipe with custom images | M O |
| TR-07 | Slide / swipe / zoom | Directional transitions | O |
| TR-08 | Per-scene overrides | Transition per destination scene and per scene pair | O P |
| TR-09 | Per-format transitions | Vertical mirrors main or uses its own | M |
| TR-10 | Transition preview | Preview a transition in the Inspector without going to air | K |

## 7. Video effects ([08](08-video-effects.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| VF-01 | Effect stack | Ordered, stackable, per-effect bypass and reset, drag reorder | M O |
| VF-02 | Input vs layer effects | Fix the device once, or style this layer in this scene | K |
| VF-03 | Effect presets | Built-in presets (Hologram, 90s, Duotone, …), save/import/export shareable presets | M |
| VF-04 | Colour effects | Black & white, brightness/contrast, hue/saturation, tint, invert, tritone, duotone, LUT (.cube/.png, intensity, stackable, large LUTs), CC toner, fill, colour correction (lift/gamma/gain, curves, white balance), levels | M O P |
| VF-05 | Keying | Chroma key, luma key, colour key, despill, edge refine | M O |
| VF-06 | Blur | Gaussian, directional, zoom/radial, box, tilt-shift, mask blur | M P |
| VF-07 | Stylise | Glow styles, shadow styles (inner/outer/multiple), drop shadow, outline/stroke, gradient overlay, venetian blinds, pixelate, posterize, halftone, scanlines, VHS, film grain, vignette, chromatic aberration | M P |
| VF-08 | Distort | Bulge, corner pin, transform (skew/anchor), displacement, ripple, swirl, mirror/kaleidoscope | M P |
| VF-09 | Generators | Fractal noise, gradient, solid | M |
| VF-10 | Detail | Sharpen, unsharp mask, denoise (temporal) | M O |
| VF-11 | Channels | Shift channels, threshold, channel mixer | M |
| VF-12 | Time | Freeze frame, delay (video), strobe, echo/trails | P |
| VF-13 | Custom shader | Load a shader with exposed parameters | P |
| VF-14 | Effect parameter animation | Keyframe or audio-drive any numeric effect parameter | K |

## 8. Lightair Effect Stack ([09](09-effect-stack.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| CS-01 | Looks | 26 one-click looks (Everyday, Glam, Studio, Mood, Men) + saved looks, hotkeys, per-scene looks with cross-fade | C K |
| CS-02 | Skin | Smooth (keeps texture), foundation, skin tone, redness, shadows, smile lines, teeth, bright eyes, clarity, all-visible-skin | C |
| CS-03 | Pigment-model skin | Melanin/hemoglobin calibration every 2 s; shading preserved (98–102 %) | C |
| CS-04 | Shape | 27 reshape controls across face, eyes, nose, mouth, brows | C |
| CS-05 | Makeup | 15 looks; lips, gloss, blush (3 placements), eyeshadow, liner, contour, highlight, brow tint, grade/LUT | C |
| CS-06 | Face effects | 23 tracked stickers/distortions; anchors, animation, blend; import custom stickers | C K |
| CS-07 | Atmosphere | 13 particle ambiences (person-aware) + 6 camera motions (incl. audio-driven Beat Pulse) | C |
| CS-08 | Colour | 26 grades, custom grade controls, `.cube` import | C |
| CS-09 | Light | 9 silhouette-following virtual lights with key/fill/rim, colour, direction, background dim | C |
| CS-10 | Background | Blur, remove, replace (image/video/layer), dim; feather, shift, hair detail; quality tiers; per-layer | M C K |
| CS-11 | Auto-frame | Face-following framing with shot sizes, per format | K |
| CS-12 | Before/after | Split view on canvas | C |
| CS-13 | Multi-face | Up to 4 faces; per-face targeting for shape/stickers | K |
| CS-14 | Performance profiles | Quality / Balanced / Light | M C |
| CS-15 | Look packs | `.lightairlook` import/export | K |
| CS-16 | Remote control | Every control addressable via API/Stream Deck | C K |

## 9. Audio ([10](10-audio.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| AU-01 | Track mixer | Channel strips: input, effects, meter, fader, Edit/Cue/Mute, link | M |
| AU-02 | Track sources | Mic/interface (channel select, 32-ch remap), system audio, per-app audio, output loopback, layer audio | M O |
| AU-03 | Global vs scene tracks | Global tracks across scenes; layer tracks belong to their scene; promote to global | M |
| AU-04 | Per-scene volume/mute | Unlink a track to give each scene its own level and mute | M |
| AU-05 | Track organisation | Reorder (global or per-scene), sort, folders, show current vs all, reveal source layer | M K |
| AU-06 | Meters | Peak/RMS/LUFS, peak hold, clip latch, loudness targets per platform | O K |
| AU-07 | Cue monitoring | Cue mix to headphones, stream-mix monitoring | M O |
| AU-08 | Mixes & routing | Stream, Recording(s), Cue; per-track routing; master chains | O K |
| AU-09 | Voice Clarity | One-knob vocal chain with calibration; levels Low → Very high; auto gain | M K |
| AU-10 | Noise suppression | Neural and light DSP modes | O M |
| AU-11 | Dynamics | Gate/expander, compressor with sidechain, limiter | O M |
| AU-12 | EQ & de-esser | 8-band parametric with analyser; de-esser | O K |
| AU-13 | Ducking | Sidechain ducking from one or more tracks | M P |
| AU-14 | Creative | Reverb, delay, pitch/voice changer, stereo panner/width, channel tools | M P |
| AU-15 | Audio plugins | VST3 and AU hosting, isolated mode | M O |
| AU-16 | Sync | Per-track delay; automatic clap-test sync | M O K |
| AU-17 | Push-to-talk/mute | Per-track hotkeys | O |
| AU-18 | Twitch VOD track | VOD-exclusive mix and per-track exclude-from-VOD | M O |
| AU-19 | Virtual microphone | Stream mix as a system microphone for calls | K |
| AU-20 | Hardware mixer integration | BEACN, Elgato Wave Link, GoXLR integration | M K |
| AU-21 | Silence/clipping warnings | Live mic silent, clipping with suggested fix | K |
| AU-22 | Audio bitrate | 96–320 kbps AAC, Opus where supported | M O |

## 10. Outputs: streaming ([11](11-outputs-streaming-recording.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| ST-01 | Platform sign-in | Twitch, YouTube, Kick, TikTok, X, Facebook, LinkedIn, Trovo (OAuth) | M |
| ST-02 | Custom destinations | RTMP/RTMPS, SRT, WHIP, RIST, HLS (YouTube) | M O K |
| ST-03 | Multistreaming | Unlimited destinations, per-destination toggle, add/stop mid-stream | M P |
| ST-04 | Per-destination format | Each destination picks a format (landscape/vertical) and quality | M P |
| ST-05 | Encoder sharing | Destinations with identical settings share one encoder | M |
| ST-06 | Twitch Enhanced Broadcasting | Multi-track encoding with Twitch ladder; Dual Format | M O |
| ST-07 | Stream manager | Title, category, tags, classifications, description, privacy, thumbnail per platform; edit several at once | M P |
| ST-08 | Go Live review | Pre-flight sheet: destinations, metadata, health checks, record-too | M |
| ST-09 | Reconnect | Automatic reconnect with backoff, per-destination status and banners | M O |
| ST-10 | Live bitrate change | Adjust bitrate while live | M |
| ST-11 | Network optimisation | Dynamic bitrate, low-latency mode, bind to network interface | O |
| ST-12 | Stream delay | Delay with preserve-cutoff option | O |
| ST-13 | Auto start/stop | Auto-start YouTube broadcasts, stop on end | M O |
| ST-14 | Bandwidth test | Test without going public | O |
| ST-15 | Relay mode | Send one upstream to a relay service that fans out to platforms | M K |
| ST-16 | Stream markers | Platform markers (Twitch) and local markers | O K |
| ST-17 | Scheduled streams | Create/attach to scheduled YouTube broadcasts | K |

## 11. Outputs: recording, clips, virtual camera ([11](11-outputs-streaming-recording.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| RC-01 | Recording | MP4, MOV, MKV, FLV, fragmented MP4/MOV (crash-safe), hybrid MP4 | M O |
| RC-02 | Codecs | H.264, HEVC, AV1 (hardware and software), ProRes (macOS), lossless | M O K |
| RC-03 | Independent recording encoder | "Same as stream" or own settings | M O |
| RC-04 | Record any format | Landscape, vertical, or both simultaneously | M P |
| RC-05 | Multitrack audio | Up to 6 named audio tracks + master + VOD | M O |
| RC-06 | ISO recording | Separate files per camera/input and per track | P K |
| RC-07 | Pause / split | Pause and resume; split by time/size; manual split | O |
| RC-08 | Chapter markers | Hotkey/API/Stream Deck markers saved in file and sidecar | M O |
| RC-09 | Test recording | Record with stream settings and review inside the app | M K |
| RC-10 | Remux | Convert MKV to MP4/MOV after recording | O |
| RC-11 | Clips (replay buffer) | 10–300 s rolling buffer, Clip button/hotkey/API, per-format clips | M O |
| RC-12 | Clip library | Browse, trim in/out, rename, tag, reveal, share to platform | K |
| RC-13 | Instant replay on canvas | See LY-23 | M P |
| RC-14 | Screenshots | Per canvas, hotkey | M O |
| RC-15 | Virtual camera | Any format, system extension on macOS, Windows virtual camera | M O |
| RC-16 | NDI output | Program, preview, or any scene/format | P |
| RC-17 | Auto-record | Start recording when streaming starts | O |
| RC-18 | File naming | Templates with date, scene, format, counters | O |

## 12. Chat, events & widgets ([12](12-chat-alerts-integrations.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| CH-01 | Unified chat | Twitch, YouTube, Kick, TikTok, X, Facebook in one feed with badges, emotes, per-platform filters | M |
| CH-02 | Reply & send | Send to all or one platform | M |
| CH-03 | Moderation | Delete, timeout, ban, mod tools, slow/emote/sub-only toggles where APIs allow | K |
| CH-04 | Chat layout options | Font size, alternating backgrounds, invert direction, reverse layout, split activity, pop-out | M |
| CH-05 | Custom chat dock | Replace chat with any URL | M |
| CH-06 | Activity feed | Follows, subs, gifts, bits/super chats, raids, merch orders, channel points in one timeline with replay-alert | M |
| CH-07 | Chat layer | On-stream chat with styles, animations, hide after N s, filters | M |
| CH-08 | Event widgets | Activity, recent activity, combo, spotlight, goals, progress bar, wheel spin, social handles, counter, chat highlight, subathon timer, confetti, polls | M K |
| CH-09 | Alerts | Native alerts per event type with media, TTS, animation, conditions, queue | P K |
| CH-10 | Captions | On-device speech-to-text for live captions layer and platform captions | M K |
| CH-11 | Chat commands | Built-in bot commands (!uptime, !socials, custom) and command-triggered actions | K |
| CH-12 | Merch events | Fourthwall and similar store integrations | M |
| CH-13 | Shareable widget configs | Export/import widget styles | M |

## 13. Control & integrations ([12](12-chat-alerts-integrations.md#control))

| ID | Feature | Description | Origin |
|---|---|---|---|
| CT-01 | Hotkeys | Every command; in-app and global; enable/disable; conflicts shown | M O |
| CT-02 | Stream Deck | Official plugin: scenes, studio staging, mute/cue/volume (dials), layer/effect visibility, record, go live, clip, replay, media, widgets, markers, camera background, looks | M K |
| CT-03 | Remote API | Lightair JSON-RPC over WebSocket + obs-websocket v5 compatibility; LAN opt-in, auth | M O K |
| CT-04 | MIDI & OSC | Map controllers to commands and faders with feedback | K |
| CT-05 | Third-party bots | Streamer.bot, Lumia Stream, Mix It Up, Touch Portal via API | M |
| CT-06 | Macros | Sequences of commands with waits and conditions | P K |
| CT-07 | Mobile remote | Companion app/web remote for scenes, audio, chat, start/stop | K |

## 14. Creator tools

| ID | Feature | Description | Origin |
|---|---|---|---|
| CR-01 | Overlay builder (AI) | Describe an overlay, alert, widget or scene; it's generated as an editable browser layer, testable with sample events; bring-your-own model (local or cloud provider, opt-in) | M K |
| CR-02 | Overlay gallery | Starter overlays and elements, customisable | M |
| CR-03 | LUT tools | Build, convert and preview LUTs in-app | M |
| CR-04 | Screen draw sessions | Guests draw on a layer through a link | M |

## 15. Extensibility ([13](13-extensibility-and-compatibility.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| EX-01 | OBS plugin support | Source, filter, transition, encoder, output, service plugins with generic properties UI | O K |
| EX-02 | Frontend API shim | Plugins using `obs-frontend-api` without Qt UI: events, menu items, save data, hotkeys | K |
| EX-03 | JS extensions | Native panels, commands, automations, capabilities-based sandbox | K |
| EX-04 | Content packs | Templates, looks, transitions, themes, sounds, effect presets | K |
| EX-05 | Custom browser docks | Any URL as a docked panel; imported from OBS | O M |
| EX-06 | Qt runtime for plugin UI | Opt-in: Qt docks, dialogs and property widgets from OBS plugins hosted as Lightair panels | K |
| EX-07 | Plugin compatibility database | Per-plugin status (works, needs Qt runtime, replaced by built-in, broken) and crash quarantine | K |

## 16. Platform ([15](15-platform-distribution-licensing.md))

| ID | Feature | Description | Origin |
|---|---|---|---|
| PL-01 | macOS (Apple Silicon + Intel) | Primary | M O |
| PL-02 | Windows x64 | Supported | M O |
| PL-03 | Linux | Supported (Flatpak, AppImage) | O |
| PL-04 | Hardware encoders | VideoToolbox, NVENC, QSV, AMF, VA-API | M O |
| PL-05 | Offline-first, no account | Fully functional without sign-in | O K |
| PL-06 | Privacy | On-device processing; opt-in telemetry and crash reports | K |

### Which platform has what

Every feature in this catalog is for macOS and Windows alike (docs/plans/windows.md) unless it is
listed here with its reason. Each runs on the platform's own parts: ScreenCaptureKit or Windows
Graphics Capture, VideoToolbox or NVENC/AMF/Quick Sync, Core ML or ONNX Runtime with DirectML.

| Feature | Platform | Why |
|---|---|---|
| Audio Units (AU-, plugin hosting) | macOS | An Apple format; VST3 runs on both |
| ProRes recording (RC-) | macOS | libobs encodes ProRes through VideoToolbox only; Windows offers H.264 and HEVC |
| macOS camera effects (Portrait, Backgrounds) as Lightair Effect Stack's background model | macOS | Apple's; Windows segments with MediaPipe (Windows Studio Effects to follow where the camera has them) |
| Apple Vision person segmentation | macOS | Apple's; Windows uses MediaPipe on DirectML |
| Game capture (screen mode) | Windows | libobs's game capture hooks Direct3D/Vulkan games; on macOS a game is a window |
| iPhone/iPad as a camera over USB | macOS | Apple's continuity APIs; Android over USB on both |
