# 08 · Video Effects

Video effects are GPU shaders that run inside libobs as filters (the `lightair-video-fx` module), so they render identically in every format, recording, clip, virtual camera and NDI output.

## Where effects attach

| Attach point | Applies to | Typical use |
|---|---|---|
| **Input** | Every layer that uses this input, in every scene | Colour-correct a camera, Lightair Effect Stack, chroma key a green screen |
| **Layer** | This layer in this scene and format | Drop shadow, corner glow, blur behind a panel |
| **Group** | The composited group | Shadow behind a camera + frame group |
| **Scene** | The whole scene (as a nested scene or program) | "Dream" grade for a segment |
| **Format output** | Everything in a format, after compositing | Final LUT, sharpening for vertical |

The Inspector shows effects in sections by attach point, so it's always clear what an effect will touch.

## Effect stack UI

- The `EffectList` component: ordered rows, each with an **active dot** (click to bypass), name, and expand chevron. Drag to reorder. **+** opens the effect picker.
- **Effect picker**: searchable, grouped by category, each effect with an animated preview thumbnail of the selected layer.
- **Expanded effect**: parameters with Lightair controls (sliders, colour pickers, curves, gradients, points on canvas), **Reset**, **Presets ▾**, and **A/B** compare.
- **On-canvas handles** where parameters are spatial: corner pin corners, bulge centre/radius, gradient endpoints, directional blur angle, chroma key eyedropper, mask shapes.
- **Copy / paste effects** (one or the whole stack) between layers; **Save stack as preset**; import/export presets as files.
- **Disable all effects** on a layer temporarily (⌥-click the stack's master dot) to compare.

## Presets

Built-in stack presets ship at launch, including **Hologram**, **90s**, **Duotone**, **VHS**, **Neon outline**, **Soft focus**, **Cinematic**, **Noir**, **Comic**, **Security cam**, **Dream**, **Glitch**, **Frosted panel**, **Polaroid**, **Thermal**, **Night vision**, **Retro TV** and **Sunset**. Users save their own; presets are shareable files.

## Catalog

### Colour

| Effect | Parameters |
|---|---|
| **Colour correction** | Exposure, contrast, highlights, shadows, whites, blacks, temperature, tint, vibrance, saturation |
| **Lift / gamma / gain** | Colour wheels with master sliders |
| **Curves** | RGB + per-channel curves, hue-vs-hue/sat/luma curves |
| **Levels** | Input/output black and white points, gamma per channel |
| **Brightness & contrast** | Brightness, contrast |
| **Hue / saturation** | Hue shift, saturation, lightness, colorize |
| **Black & white** | Channel weights, tint |
| **Tint** | Map black to colour A and white to colour B |
| **Duotone / tritone** | Shadows, midtones, highlights colours; balance |
| **CC toner** | Toner colours by luminance band |
| **LUT** | `.cube` (1D/3D, sizes up to 128³) and `.png` LUT strips; intensity; stackable; large files streamed |
| **Invert** | Channels, blend |
| **Fill** | Replace colour while keeping alpha |
| **White balance (auto)** | Continuous or one-shot auto white balance from the image |
| **Channel mixer** | Matrix per output channel |

### Keying & compositing

| Effect | Parameters |
|---|---|
| **Chroma key** | Key colour (eyedropper, presets green/blue/magenta), similarity, smoothness, spill reduction, edge thin/feather, light-wrap, matte view |
| **Luma key** | Low/high thresholds, smoothness, invert |
| **Colour key** | Any colour, similarity, smoothness |
| **Despill** | Colour, amount |
| **Alpha tools** | Choke, feather, invert, premultiply fix |
| **Image mask** | See masks in [07](07-scenes-layers-canvas.md#masks) |

### Blur

| Effect | Parameters |
|---|---|
| **Gaussian blur** | Radius, quality, edge mode (clamp/repeat/transparent) |
| **Directional blur** | Angle (canvas handle), length |
| **Zoom / radial blur** | Centre (handle), amount, mode (zoom/spin) |
| **Box blur** | Radius, iterations |
| **Tilt-shift** | Focus band (handles), blur amount, falloff |
| **Masked / frosted blur** | Blur what's *behind* the layer within its shape (frosted-glass panels), tint, noise |
| **Pixelate / mosaic** | Cell size, shape (square/hex), region |

### Light & stylise

| Effect | Parameters |
|---|---|
| **Drop shadow** | Colour, opacity, angle, distance, blur, spread |
| **Shadow styles** | Inner, outer, multiple stacked shadows |
| **Glow styles** | Inner/outer glow, colour, size, intensity, threshold |
| **Outline / stroke** | Width, colour or gradient, position (inside/centre/outside), dash |
| **Gradient overlay** | Linear/radial/conic, stops, angle, blend mode, opacity |
| **Venetian blinds** | Count, angle, width, softness, animation |
| **Vignette** | Amount, roundness, feather, colour |
| **Film grain** | Amount, size, colour/mono, animated |
| **Scanlines / CRT** | Line density, curvature, phosphor glow, flicker |
| **VHS** | Tracking noise, chroma bleed, jitter, tape wear |
| **Halftone** | Dot size, angle, colour mode |
| **Posterize** | Levels per channel |
| **Chromatic aberration** | Offset, angle, radial mode |
| **Bloom** | Threshold, intensity, radius |
| **Light leaks** | Preset leaks, intensity, animation speed |
| **Neon edges** | Edge detection with glow and colour |

### Distort

| Effect | Parameters |
|---|---|
| **Transform** | Position, scale, rotation, skew, anchor (inside the layer) |
| **Corner pin** | Four corners (canvas handles), for screens in mockups |
| **3D transform** | Rotate X/Y/Z, perspective/field of view, depth |
| **Bulge / pinch** | Centre, radius, strength |
| **Swirl / twirl** | Centre, radius, angle |
| **Ripple / wave** | Amplitude, wavelength, speed, direction |
| **Displacement map** | Map image/layer, scale X/Y |
| **Mirror / kaleidoscope** | Axis, segments, rotation |
| **Lens distortion** | Barrel/pincushion, correct a GoPro-style lens |
| **Fisheye** | Strength, centre |

### Detail

| Effect | Parameters |
|---|---|
| **Sharpen** | Amount |
| **Unsharp mask** | Amount, radius, threshold |
| **Denoise (temporal)** | Strength, motion threshold (cleans low-light webcams) |
| **Deband** | Threshold, grain |

### Channels & stylised utility

| Effect | Parameters |
|---|---|
| **Shift channels** | Map each output channel from any input channel |
| **Threshold** | Level, softness, colour |
| **Edge detect** | Method, thickness, colour |
| **Fractal noise** | Type, scale, complexity, evolution speed, contrast, colour (as a generator layer or an overlay) |
| **Gradient** | Generator (linear/radial) as overlay |

### Time

| Effect | Parameters |
|---|---|
| **Freeze frame** | Freeze on hotkey/API, duration, fade |
| **Video delay** | Delay in ms/frames (for sync with delayed audio or "instant replay" effects) |
| **Echo / trails** | Decay, frames, blend |
| **Strobe** | Rate, duty |
| **Time remap** | Slow-motion of the last N seconds as a replay effect |

### Custom

| Effect | Description |
|---|---|
| **Custom shader** | Load a shader (Lightair shader format, a thin wrapper over libobs effect files) with declared parameters that appear as normal Lightair controls; hot reload in developer mode |
| **Plugin filters** | Compatible OBS filter plugins with generic properties UI |

## Animation of effects

Any numeric parameter can be:

- **Keyframed** in a layer animation (see [07](07-scenes-layers-canvas.md#layer-animation)).
- **Driven by audio** (a track's level, with attack/release and range mapping).
- **Driven by events** (an alert spikes the glow for 2 s).
- **Set by hotkey/API** with an optional ramp duration.

## Implementation notes

- Effects are written in libobs's effect language (`.effect`) or as Rust-driven multi-pass pipelines for complex effects (blur pyramids, bloom, denoise).
- Shaders compile once per variant and are cached on disk per GPU and driver.
- Each effect declares a **cost class** (cheap / moderate / heavy). The picker shows heavy effects with a hint, and the Stats panel shows measured cost per effect instance.
- **Bypassed effects cost nothing** (removed from the filter chain, not just set to zero).
- Golden-render tests cover every effect and every built-in preset ([16](16-quality-performance-accessibility.md)).
