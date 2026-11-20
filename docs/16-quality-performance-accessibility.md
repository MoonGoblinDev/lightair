# 16 · Quality: Performance, Testing, Accessibility, Localisation

## Performance budgets

Budgets are enforced by automated benchmarks. A change that breaks a budget fails CI.

### Live-path budgets (non-negotiable)

| Metric | Budget |
|---|---|
| Frames skipped or lagged caused by UI activity | **0** in a 4-hour soak with continuous UI interaction scripts |
| Audio glitches (buffer underruns) caused by UI activity | **0** |
| Main-thread work inside libobs threads | **None** (enforced by thread assertions in debug builds) |
| Engine actor mutation latency (command → libobs applied) | p99 < 8 ms for single-layer edits |
| Scene switch to first program frame | ≤ 1 frame + transition duration |

### Interface budgets

| Metric | Budget |
|---|---|
| UI frame time (typical panel interaction) | < 8 ms on Apple M1, < 12 ms on a 2019 mid-range Windows laptop |
| Canvas drag latency (pointer → overlay moved) | Same frame |
| Canvas drag latency (pointer → video moved in preview) | ≤ 2 frames |
| Preview latency added by `lightair-preview` | ≤ 1 frame |
| Cold start to first live preview frame | < 2.5 s (M1, warm disk) |
| Open a 50-scene project | < 1.5 s to interactive |
| Command palette results | < 16 ms per keystroke |
| Memory (UI + engine, simple scene: camera, display, mic) | < 400 MB RSS, excluding CEF helpers |

### Overhead vs OBS

Lightair is measured against OBS Studio of the same engine version with an equivalent scene. Target: **lower total CPU and memory** than OBS's Qt frontend in steady state, with previews visible.

### Feature budgets

| Feature | Budget (M1, 1080p60 canvas) |
|---|---|
| Lightair Effect Stack, all tabs on | ≤ 1.0 ms render time, ≤ 20 % of one core, 0 skipped frames |
| Voice Clarity on one track | ≤ 3 % of one core, ≤ 10 ms added latency |
| Each preview (thumbnail size) | ≤ 0.15 ms GPU |
| Chat panel at 50 messages/s | No UI frame over budget |
| Meter rendering, 12 tracks | ≤ 0.5 ms per UI frame |

## Testing strategy

Run everything with `cargo xtask test` (a filter or `--nocapture` after it goes to every test
program). It builds all test programs, then runs them side by side, slowest first, from small
folders of their own, without the camera and capture-card modules that wait seconds on macOS
services (the smoke test still loads every module). Tests that count dropped frames skip that
check while sharing the GPU (plain `cargo test` still checks it). The suite runs in under twenty
seconds, where `cargo test` runs the programs one after another for minutes.

Each crate's integration tests are one test program (`tests/<crate>/main.rs`, a module per
file), built and linked once. libobs runs once per process, so an engine or preview test starts
with `lightair_engine::testing::ran_in_own_process()`: `cargo xtask test` runs each such test as
a process of its own, and plain `cargo test` re-runs the program for it. Tests that depend on
this Mac (its audio devices, its macOS version) or change it (a global shortcut) are ignored;
`cargo xtask test --machine` runs them too. `cargo xtask check` is the edit loop and `cargo xtask
precommit` what a commit needs (docs/build-speed.md).

| Level | What | Tooling |
|---|---|---|
| **Unit** | `lightair-model` commands, validation, undo coalescing, migrations, reconciler diffing | `cargo test`, property tests (`proptest`) for command sequences and undo/redo round trips |
| **Engine integration** | Reconciler against real libobs: create, mutate and tear down every entity type; idempotence (full apply == incremental) | Headless libobs with null graphics + test sources |
| **UI** | Components and panels in headless GPUI windows: pointer/keyboard input, focus order, layout assertions, accessibility tree | gpui-kit UI testing harness |
| **Golden render** | Each video effect, transition, Lightair Effect Stack tab, text style and template rendered to images and compared to approved goldens (perceptual diff with tolerance) | Offscreen libobs render, per-GPU-vendor goldens where needed |
| **Audio** | Each audio effect against reference signals: frequency response, gain, latency, null tests; Voice Clarity against a speech/noise corpus | Offline processing harness |
| **A/V sync** | Clapper test: synthetic flash + beep through capture, effects, encoders and outputs; measure offset in recordings and on a local RTMP server | Automated, every engine change |
| **Outputs** | Stream to local RTMP/SRT/WHIP servers, verify continuity, reconnect behaviour under induced packet loss and outages | Local test servers + network shaping |
| **Soak** | 4–12 hour sessions: streaming + recording + replay buffer + scripted UI interaction; watch memory growth, frame stats, file integrity | Nightly on dedicated machines |
| **Compatibility** | Popular third-party OBS plugins load and work, including frontend-API plugins on the shim and Qt plugins in the Qt runtime (docks embedded, dialogs, events, save/load round trips); classifier accuracy; crash quarantine; OBS import fixtures from real user collections (anonymised) | Plugin matrix feeding the compatibility database |
| **Lightair Effect Stack** | Tracking jitter and lag harness on recorded footage; pigment model validation across a broad, consented skin-tone dataset | Offline harness |

**Fuzzing** covers the project loader, OBS importer, remote API message parser and pack importer.

## Observability

- Structured logging (`tracing`) with per-subsystem levels, written to rotating files and viewable in *Diagnostics → Logs* with filtering.
- **Stats panel**: per-layer and per-effect GPU/CPU time, per-output encoder and network stats, per-track DSP time, render/encode timelines.
- **Performance HUD** (developer setting): UI frame time graph, engine actor queue depth, preview publish rate.
- Crash reports (opt-in) with symbolicated stacks across Rust, C/C++ and ObjC frames.

## Accessibility

Lightair targets **WCAG 2.2 AA** equivalence for desktop and the platform accessibility guidelines.

- **Screen readers**: every control has an AccessKit role, name, value and actions; panels are landmarks; the canvas exposes layers as a list of objects with position and size, and supports keyboard editing.
- **Keyboard**: everything operable without a pointer, including canvas edits, mixer faders and the effect stack. Focus is always visible.
- **Live announcements** (opt-in): "Stream started", "Recording stopped", "Microphone muted", "Dropped frames" spoken through the screen reader, throttled.
- **Visual**: high-contrast themes; state uses shape plus colour; text scaling 85–150 %; reduce-motion support; no information conveyed only by colour on meters (clip latch shows ✕).
- **Motor**: all drag interactions have keyboard and numeric alternatives; hit targets ≥ 24 px; sticky hover menus.
- **Hearing**: captions for the user's own stream (on-device transcription; see [12](12-chat-alerts-integrations.md#captions)) and visual alerts for audio events (clipping, silence on a live mic).

## Localisation

- All strings in Fluent (`.ftl`) files, with plural and gender rules; no strings concatenated in code.
- Launch languages chosen by user base: English, Spanish, Portuguese (BR), German, French, Japanese, Korean, Simplified Chinese, Traditional Chinese, Indonesian, Russian, Turkish.
- Right-to-left layout support is part of the design system (GPUI flex direction mirrors; icons with direction are flipped).
- Number, date, duration and unit formatting follows locale; bitrates and dB values keep technical units.
- Pseudo-localisation build in CI to catch truncation and hard-coded strings.

## Definition of done (every feature)

A feature is done when:

1. It's modelled in `lightair-model` as commands with undo, validation and persistence (with migration tests if the schema changed).
2. It works in every format, in recordings, clips, virtual camera and remote API.
3. It has UI in the design system, keyboard access, AccessKit semantics and localised strings.
4. It has unit, integration and (if visual or audible) golden tests.
5. It meets its performance budget and doesn't regress live-path budgets.
6. It's documented in the user guide and, if it replaces an OBS plugin, in the import mapping.
