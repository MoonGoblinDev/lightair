# Effects revamp: everything is an effect

Status: built (2026-11-12). Face effects follow Dewy's tabs: Beauty (Enhance), Face stickers, Relight, Colour grade, Ambience and Camera motion (Atmosphere), plus the new background effects. Supersedes the Lightair Effect Stack panel in
[09](../09-effect-stack.md) and extends the catalog in [08](../08-video-effects.md).

## Why

Lightair had two systems: a short list of libobs filters ("Effects") and Lightair Effect Stack, one
large face filter with its own panel and tabs. People don't think in pipelines; they think
"smooth my skin", "blur my background", "make it look like film". So every feature becomes an
**effect**: picked from one catalog, stacked on the source, each with its own on/off, controls,
reset and presets.

## Principles

- **One stack per source.** Effects live on the input (so they apply wherever it appears).
  Order is visible and draggable; face effects run in their natural order inside one engine pass.
- **Face effects share one engine.** Skin, shape, makeup, stickers, light, background and the
  rest are separate effects in the UI and the project file, but the engine runs one Camera
  Studio filter per source with one tracker and one segmenter, so ten face effects cost about
  what one does. The reconciler merges the source's face effects into that filter's settings.
- **Every control earns its place.** Defaults that look good, one big intensity where it
  helps, grouped detail behind disclosure, bipolar sliders centred at "unchanged", double-click
  to reset, values shown in human units.
- **Presets are first-class.** Effects with styles (grades, lights, makeup looks, stickers,
  ambience, motion) show them as tiles.
- **GPU only, real-time.** Every effect runs on the GPU in libobs, in every format, recording
  and stream.

## Architecture

| Layer | Change |
|---|---|
| `lightair-effect-stack` (C) | Port Dewy's latest pipeline: MediaPipe 478-point mesh on Core ML, mesh-drawn contour, highlight, eyeliner styles and brow, lash-line eyelids, Finish pass for light, faster and edge-snapped segmentation. Add a **background** stage (blur, remove, replace) driven by the person mask. Add **`lightair_fx`**, a data-driven GPU effect filter: one filter type, one `.effect` file per look (`data/fx/*.effect`), parameters from settings. |
| `lightair-core` | Effects keep `kind` + `settings`. Face effects use kinds `studio.*`; generic shader effects `fx.*`; libobs filters keep their ids. |
| `lightair-engine` | Input effect chains: consecutive `studio.*` effects become one `lightair_camera_studio` filter with merged settings (each effect's switch and intensity applied); `fx.*` become `lightair_fx` with `fx` set; the rest pass through. |
| `lightair-app` | A typed catalog (`effects/catalog.rs`): kind, name, category, description, icon, controls (slider, bipolar slider, choice, colour, preset grid, file, switch), groups, defaults. The **Effect catalog sheet** (search, categories, tiles, descriptions) and the **effect stack** in the Inspector with per-effect controls. Old projects' single Lightair Effect Stack effect is split into the new face effects on load. |

## The effect list

### Face (needs a face in view; shared tracker)

| Effect | Kind | Controls |
|---|---|---|
| **Beauty** | `studio.beauty` | Dewy's Enhance tab as one effect, in groups: **Skin** (Smooth, Foundation, Skin tone ±, Redness, Dark circles, Brighten teeth, Bright eyes, Clarity, Whole body skin) · **Face** (Natural, Slim, V-shape, Narrow, Small, Cheekbones, Jawbone, Chin ±, Forehead ±, Head size) · **Eyes** (Enlarge, Distance ±, Position ±, Tilt ±, Inner corner, Outer corner ±, Lower lid, Iris size) · **Nose** (Narrow, Lift ±, Bridge, Tip) · **Mouth** (Size ±, Position ±, Smile) · **Brows** (Thickness ±, Position ±) · **Makeup** (Look: None + 15 tiles, Intensity, lip colour + amount, gloss, blush colour + amount + placement, eyeshadow colour + amount, eyeliner style, contour, highlight, brow tint, look colour) |
| **Face stickers** | `studio.stickers` | Sticker set (23 tiles: Cute, Cool, Glam, Funny) |
| **Relight** | `studio.light` | Light (9 tiles: Soft Box, Butterfly, Rembrandt, Split, Golden Hour, Moonlight, Spotlight, Neon, Rim Glow) · Intensity |

### Background (needs a person in view; shared segmenter)

| Effect | Kind | Controls |
|---|---|---|
| **Background blur** | `studio.bg_blur` | Amount, Edge softness |
| **Background removal** | `studio.bg_remove` | Edge softness, Edge shift (±) — transparent background, layers show through |
| **Background replace** | `studio.bg_replace` | Colour or image (file), Edge softness |

### Colour

| Effect | Kind | Controls |
|---|---|---|
| **Colour grade** | `studio.grade` | Grade (26 tiles: Natural, Emotional, Vivid, Film, B&W) · Intensity |
| **Colour correction** | `color_filter_v2` | Brightness, Contrast, Saturation, Gamma, Hue, Opacity |
| **LUT** | `clut_filter` | .cube / .png file, Amount |
| **Black & white** | `fx.mono` | Amount, Tint colour, Tint amount |
| **Duotone** | `fx.duotone` | Shadow colour, Highlight colour, Amount |
| **Posterize** | `fx.posterize` | Levels |
| **Invert** | `fx.invert` | Amount |
| **Vignette** | `fx.vignette` | Amount, Size, Softness, Roundness |

### Stylize

| Effect | Kind | Controls |
|---|---|---|
| **Film grain** | `fx.grain` | Amount, Size, Colour (switch) |
| **VHS** | `fx.vhs` | Amount, Tracking noise, Colour bleed |
| **CRT scanlines** | `fx.scanlines` | Amount, Line size, Curvature |
| **Halftone** | `fx.halftone` | Dot size, Angle, Colour (switch) |
| **Pixelate** | `fx.pixelate` | Block size |
| **Neon edges** | `fx.edges` | Amount, Glow colour, Keep image (amount) |
| **Sketch** | `fx.sketch` | Amount, Line weight |
| **Chromatic aberration** | `fx.chroma` | Amount, Angle |
| **Glow** | `fx.glow` | Amount, Threshold, Size |
| **Emboss** | `fx.emboss` | Amount, Angle |

### Blur & sharpen

| Effect | Kind | Controls |
|---|---|---|
| **Blur** | `fx.blur` | Amount |
| **Tilt-shift** | `fx.tiltshift` | Amount, Focus position, Focus size |
| **Sharpen** | `sharpness_filter_v2` | Amount |

### Distort & motion

| Effect | Kind | Controls |
|---|---|---|
| **Camera motion** | `studio.motion` | Motion (6 tiles: Handheld, Beat Pulse, RGB Split, Slow Drift, Glitch, Wobble) · Intensity |
| **Ambience** | `studio.ambience` | Particles (13 tiles: Sparkles, Golden Dust, Bokeh, Hearts, Sakura, Snow, Fireflies, Light Leak, Purple Mist, Night Fog, Dreamy, Confetti, Bubbles) · Intensity |
| **Mirror** | `fx.mirror` | Axis (Left→right, Right→left, Top→bottom, Bottom→top) |
| **Kaleidoscope** | `fx.kaleido` | Segments, Rotation |
| **Bulge** | `fx.bulge` | Amount (±), Size |
| **Swirl** | `fx.swirl` | Amount (±), Size |
| **Wave** | `fx.wave` | Amount, Frequency, Speed |
| **Scroll** | `scroll_filter` | Horizontal, Vertical |

### Keying & utility

| Effect | Kind | Controls |
|---|---|---|
| **Green screen** | `chroma_key_filter_v2` | Key colour (Green, Blue, Magenta), Similarity, Smoothness, Spill |
| **Colour key** | `color_key_filter_v2` | Colour, Similarity, Smoothness |
| **Luma key** | `luma_key_filter_v2` | Remove darks, Keep up to |
| **Delay** | `gpu_delay` | Milliseconds |

## UI

- **Inspector › Effects**: the stack. Each effect is a card: icon, name, a one-line summary of
  what it's doing ("Smooth 50 · Foundation 20"), an on/off switch, a chevron. Expanded: its
  controls, grouped, with Reset and a ⋯ menu (duplicate, copy, move up/down, remove). Drag to
  reorder. A master switch bypasses the whole stack to compare.
- **Add effect** opens the **Effect catalog** sheet: categories down the side (All, Face,
  Background, Colour, Stylize, Blur, Distort & motion, Keying & utility), a search field, and a
  grid of tiles (icon, name, one-line description; "Needs a face" / "Needs a person" badges).
  Choosing a tile adds it and opens its controls.
- **Controls**: slider rows (label, slider, value; bipolar ones centred), choice chips, colour
  swatches with a picker, preset tile grids, a file chooser, switches.

## Plan

1. **Engine port** (C): Dewy's latest tracker and pipeline into `lightair-effect-stack`, renamed;
   Core ML models built with the module; render tests still pass.
2. **Background stage** (C + shader) in the Lightair Effect Stack filter.
3. **`lightair_fx`** filter (C) with the shader effects above; a render test per family.
4. **Catalog and model** (Rust): typed catalog, `studio.*` merging in the reconciler, `fx.*`
   mapping, migration of the old single Lightair Effect Stack effect; unit tests.
5. **UI**: effect stack and controls in the Inspector; the Effect catalog sheet. (Whole-stack looks were built, then removed: not wanted.)
6. Screenshots, the full suite, commit after each step.
