# 06 · Design System

Lightair's visual and interaction language. The system lives in the `lightair-ui` crate and is the only place colours, sizes and component styles are defined. Feature crates never hard-code a colour or a pixel value.

## Foundations

### Layering of the UI stack

| Layer | Role in Lightair | Web analogy |
|---|---|---|
| **GPUI** | Rendering, flex layout, styling primitives (`div().flex().gap_2()`), input | HTML + Tailwind |
| **gpui-base** | Unstyled behaviour: focus, keyboard nav, popovers, drag, selection, accessibility | Base UI / Radix |
| **gpui-component** | Styled standard controls, **restyled through Lightair's theme** | shadcn/ui |
| **lightair-ui** | Lightair tokens + theme for gpui-component + studio components built on gpui-base | Your design system |

**Rule:** use a gpui-component control when one exists and the theme can make it right (buttons, inputs, selects, menus, dialogs, tabs, tooltips, toasts, lists, tables, dock). Build on gpui-base when the control is specific to live production or needs custom painting (meters, faders, canvas handles, layer rows, scene cards, channel strips).

### Visual character

- **Dark, quiet, content-first.** The canvas and the creator's face are the brightest things on screen. Chrome is ink and grey. The interface is **monochrome plus one brand colour**: the on-air red from the logo, which means "live" and nothing else. Other colours appear only as status (healthy, warning, error).
- **Brand-aligned.** The UI uses the same ink, white and on-air red as the Lightair mark (see the [brand guide](../branding/README.md)). The primary action is the brightest control on screen: white when idle, on-air red when live.
- **Flat with depth through tone**, not shadows. Panels are separated by 1 px hairlines and slight tonal steps; floating surfaces (popovers, menus) get a single soft shadow.
- **Compact density** by default (streamers run Lightair next to games on one screen), with a *Comfortable* density option.
- **Motion is functional**: it explains where something came from or went. Nothing animates on a loop except live indicators and meters.

## Tokens

All tokens are semantic. Themes map semantic tokens to raw values. Values below are the **Dark (default)** theme.

### Colour: surfaces & text

| Token | Value | Use |
|---|---|---|
| `bg.app` | `#141414` | Window background, behind panels |
| `bg.panel` | `#1B1B1C` | Panels (layers, mixer, outputs) |
| `bg.raised` | `#232325` | Rows on hover, channel strips, inputs |
| `bg.selected` | `#2B2B2E` | Selected rows, active tab |
| `bg.overlay` | `#262628` | Popovers, menus, sheets |
| `bg.canvas-well` | `#0E0E0F` | Area around the canvas |
| `border.hairline` | `#2A2A2D` | Panel dividers, input borders |
| `border.strong` | `#3A3A3E` | Focused/hovered input borders, button outlines |
| `text.primary` | `#EDEDED` | Primary labels |
| `text.secondary` | `#A1A1A6` | Secondary labels, inactive rows |
| `text.tertiary` | `#6E6E73` | Placeholders, disabled, scale labels on meters |
| `text.inverse` | `#0E0E0F` | Text on bright fills |

### Colour: intent

| Token | Value | Use |
|---|---|---|
| `accent.primary` | `#FFFFFF` fill, `text.inverse` label | **Go Live** when idle, plus the primary button in onboarding and dialogs. One per view |
| `accent.interactive` | `#EDEDED` | Focus rings (2 px, with a 1 px `bg.app` gap), toggles on, selected segment, links (underlined) |
| `state.live` | `#FF3B4A` (on-air red, the brand colour) | Go Live while live, live badge, live canvas border, tally dots on scenes and outputs. Never decorative |
| `state.recording` | `#FF5A3D` | Recording dot and timer |
| `state.ok` | `#34D399` | Healthy indicators, active effect dot, meter green zone |
| `state.warn` | `#FBBF24` | Meter yellow zone, warnings, encoder strain |
| `state.error` | `#F87171` | Errors, missing devices, meter clip |
| `state.info` | `#60A5FA` | Informational badges |
| `canvas.selection` | `#FFFFFF` with a 1 px `#141414` outer edge | Selection outline and handles on the canvas; the double edge stays visible on any footage |
| `canvas.selection.alt` | `#A1A1A6` dashed | Selection in the second format during side-by-side editing |
| `canvas.guide` | `#FF4FD8` | Smart guides and snapping lines. A functional overlay colour chosen to contrast with any video; shown only while dragging |
| `canvas.safe-area` | `rgba(255,255,255,0.18)` | Safe-area overlays |

Brand platform colours (YouTube red, Twitch purple, …) appear only inside platform icons.

**Light theme** inverts the neutrals: `accent.primary` becomes an Ink (`#141414`) fill with a white label, `accent.interactive` becomes `#141414`, and canvas selection keeps its white-and-ink double edge. On-air red is the same in every theme.

### Typography

| Token | Font | Size / line | Weight | Use |
|---|---|---|---|---|
| `type.title` | Inter (bundled) | 15 / 20 | 600 | Panel titles ("Layers", "Outputs") |
| `type.body` | Inter | 13 / 18 | 450 | Rows, labels, inputs |
| `type.body-strong` | Inter | 13 / 18 | 600 | Selected rows, primary buttons |
| `type.caption` | Inter | 11 / 14 | 500 | Badges, stats, secondary info |
| `type.micro` | Inter | 9 / 12 | 600, tabular | Meter scales, timecode in tight spaces |
| `type.mono` | JetBrains Mono (bundled) | 12 / 16 | 450 | Timecode, bitrate, logs, numeric readouts |

Numbers that update live always use **tabular figures** so they don't jitter.

### Spacing, sizing, radius

- **Spacing scale** (px): `2, 4, 6, 8, 12, 16, 20, 24, 32, 48`.
- **Control heights**: compact `24`, default `28`, large `36` (primary actions in the title bar are `32`).
- **Row height**: layer/scene rows `36` (compact `30`).
- **Radius**: `sm 4` (inputs, chips), `md 6` (buttons, rows), `lg 10` (popovers, cards, channel strips), `full` (toggles, avatars, round camera masks).
- **Hit targets**: never smaller than 24×24 even when the visual is smaller.

### Elevation

| Level | Use | Treatment |
|---|---|---|
| 0 | Panels | Tone only |
| 1 | Hover cards, channel strips | `bg.raised` |
| 2 | Popovers, menus, palette | `bg.overlay`, 1 px `border.hairline`, shadow `0 8 24 rgba(0,0,0,.45)` |
| 3 | Sheets (Go Live review), modals | Same as 2 plus a 40 % black scrim |

### Motion

| Token | Duration | Curve | Use |
|---|---|---|---|
| `motion.instant` | 0 ms | – | Live-critical state (live/record changes) |
| `motion.fast` | 120 ms | ease-out | Hover, press, toggles |
| `motion.base` | 180 ms | ease-in-out | Panels expanding, popovers |
| `motion.slow` | 260 ms | spring (stiff) | Docking, reordering, sheets |

Respect "Reduce motion": everything becomes a cross-fade or instant.

### Iconography

- **Lucide** (bundled by gpui-kit) as the base set, 16 px default, 1.5 px stroke.
- Lightair-specific glyphs drawn to the same grid: layer types (counter, chat, phone camera), effect categories, Lightair Effect Stack tabs, platform marks (official assets, unmodified).
- Status glyphs pair colour with shape (⚠ triangle, ● dot, ✕) so state never relies on colour alone.

## Themes

- **Dark** (default), **Light**, **High Contrast Dark**, **High Contrast Light**.
- Users can set a custom interaction accent (it recolours `accent.interactive` only) and load community themes (a JSON file mapping semantic tokens; schema versioned). Themes can't change sizes or layout, only colour and font choice, so they can't break the UI. `state.live` is locked to on-air red in every theme, so "live" always looks the same.
- Canvas overlay colours stay distinct in every theme.

## Studio components (lightair-ui)

Custom components built on gpui-base. Each has a spec covering anatomy, states, keyboard behaviour and accessibility role.

| Component | Description |
|---|---|
| `ChannelStrip` | Track header (name, input picker), effect list, `Meter`, `Fader`, Edit/Cue/Mute/Link buttons. Narrow and wide variants. |
| `Meter` | Vertical or horizontal; peak + RMS bars, peak-hold tick, clip latch (click to reset), dB scale (0 to −60, labelled at 0/−6/−12/−18/−24/−30/−36/−42/−48/−54), green/yellow/red zones at −20/−9 dBFS. Rendered with GPU quads, updated every frame while visible. |
| `Fader` | Logarithmic taper, unity mark, fine mode with ⇧, double-click to unity, scroll to adjust, numeric readout that's editable. |
| `Knob` | For effect parameters where space is tight; drag vertically, ⇧ fine, double-click reset. |
| `LayerRow` | Type icon, name, badges, hover actions (visibility, lock, effects), drag handle, nesting indent for groups. |
| `SceneCard` | Row or tile with thumbnail, name, program/preview markers, hotkey badge. |
| `CanvasOverlay` | Selection box, 8 resize handles, rotation handle, crop handles (⌥), anchor point, smart guides, distance labels, safe areas, rulers. |
| `FormatTabs` | Landscape / Vertical / custom tabs with aspect icons and a side-by-side toggle. |
| `EffectList` | Ordered list with drag reorder, per-item bypass dot, expand to edit, "+" with searchable effect picker. Shared by layers, inputs and tracks. |
| `OutputRow` | Platform icon, account/destination name, format badge, live toggle, health dot, bitrate sparkline when live. |
| `HealthStrip` | Title-bar cluster: overall health ring, disk, FPS, CPU; click opens Stats. |
| `PrimaryAction` | Go Live / Record / Clip buttons with idle, armed, active, stopping and error states plus elapsed time. Go Live: `accent.primary` when idle, `state.live` with a white label and timer while live. Record: outlined, with a red dot and timer while recording. Clip: outlined |
| `LookTile` | Lightair Effect Stack thumbnail rendered from the user's own camera, with intensity ring and selected state. |
| `TimecodeLabel` | Tabular mono timer with colour by state. |
| `CurveEditor` | Bézier curves for colour grading and easing; shared by effects and transitions. |
| `GradientPicker`, `ColorPicker` | HSV/RGB/hex with eyedropper (samples from the canvas), saved swatches. |
| `Scrubbable` | Wraps any numeric field to add label-drag scrubbing and expression input. |

## Interaction patterns

- **Inline over modal**: properties edit in the Inspector or in popovers anchored to what you clicked.
- **Progressive disclosure**: rows show essentials; hover reveals actions; "Edit" expands detail; the command palette reaches everything.
- **Immediate apply with undo**: no "OK/Apply/Cancel". Changes apply live; undo reverts.
- **Presets everywhere**: every effect and Lightair Effect Stack section has built-in presets and "Save as preset".
- **Empty states teach**: an empty Layers panel shows the three most likely layers for this scene type as buttons.
- **Errors are actionable**: every error states what happened and offers the fix as a button.

## Writing style

- Sentence case everywhere ("Add layer", not "Add Layer").
- Verbs on buttons ("Go live", "Save clip"), nouns on panels ("Layers").
- Plain language for health ("Your upload can't keep up. Lower the bitrate to 4,500 kbps?"), not error codes. Codes go in the diagnostics detail.
- No jargon where a common word works: "Effects", not "Filters"; "Tracks", not "Audio sources"; "Destinations", not "Services".

## Accessibility

- Every interactive component has an AccessKit role, name, value and actions (gpui-base provides these; studio components must too).
- Meters expose level as a value with a throttled announcement only on clipping.
- All colour pairs meet WCAG AA contrast for text; state never relies on colour alone.
- Full keyboard operation including canvas editing (select with Tab, move with arrows, resize with ⌥arrows, rotate with ⌘⌥arrows).
- Text scaling 85–150 % without truncating primary actions.
- Details in [16 · Quality](16-quality-performance-accessibility.md#accessibility).
