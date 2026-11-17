# 09 · Lightair Effect Stack

In the app it's simply **Effects** (the ✦ panel, the Inspector's section, the effect's name in an object's chain); "Lightair Effect Stack" is its name in these documents and the code.

**Lightair Effect Stack** is Lightair's built-in, face-aware camera pipeline: skin, reshape, makeup, face-tracked effects, atmosphere, colour, virtual lighting, and background, applied live to any camera and edited from one native panel. It is a first-class feature of Lightair, not a plugin: it uses Lightair's design system, undo, presets, remote API and project file like everything else.

> **Origin.** Lightair Effect Stack is built from the Dewy beauty pipeline (GPL-2.0-or-later, same author), absorbed into Lightair with its Qt dock replaced by native Lightair UI and its branding removed. The technology described here (pigment-model skin, frame-synchronised tracking, learned segmentation) carries over unchanged.

## Why it's different

Most beauty filters paint over the skin: blur it, then blend a skin colour on top. That flattens faces because the paint also covers the light and shadow that give a face its shape.

Lightair Effect Stack edits skin the way skin works. In optical density, each skin pixel is **melanin + hemoglobin + shading**. Lightair calibrates the melanin and hemoglobin directions on *this* person's skin and *this* camera every two seconds, and edits only pigment:

- **Skin tone** changes melanin.
- **Foundation** evens out melanin and hemoglobin blotches.
- **Redness** removes excess hemoglobin.
- **Makeup** is deposited as a pigment layer.
- **Shading** (the lighting, shadows and 3-D form of the face) is never touched. Measured, it keeps 98–102 % of the original shadow depth, versus 59–67 % for a paint-over approach.

The result: it still looks like you, in your light, just on a good day.

## Where it lives in the UI

- **On the layer:** camera layers show a ✦ badge when Lightair Effect Stack is on. Clicking the badge toggles it; ⌥-click opens the panel.
- **In the Inspector:** a camera *input* has a **Lightair Effect Stack** section at the top of its input effects, with the on/off switch, the current look, and an intensity slider.
- **Lightair Effect Stack panel** (✦ on the activity rail): the full editor, shown in the right column or expanded to a large workspace with a before/after split on the canvas.
- **In onboarding:** the camera step offers a row of Looks rendered from the user's own face.

Because it is an **input effect**, a look applies everywhere that camera appears: every scene, every format, recordings, clips, virtual camera, NDI.

## Panel structure

```text
✦ Lightair Effect Stack — [MacBook Pro Camera ▾]               [On ●]  [Before/After]
──────────────────────────────────────────────────────────────────────────
 Looks │ Skin │ Shape │ Makeup │ Effects │ Atmosphere │ Color │ Light │ Background
──────────────────────────────────────────────────────────────────────────
 (tab content: tiles rendered from your face, or grouped sliders)
──────────────────────────────────────────────────────────────────────────
 Intensity ━━━━━━━●━━  │ Reset tab │ Save look…
```

Every tab has its own on/off switch, so a creator can keep their makeup but turn off the atmosphere for a serious segment. Thumbnails are rendered **from the live camera**, not stock photos.

### Looks

One-click full looks that set every tab at once, plus the user's saved looks.

| Category | Built-in looks |
|---|---|
| **Everyday** | Natural, Fresh face, Dewy (the signature look), … |
| **Glam** | Full-coverage looks with makeup, filter and light |
| **Studio** | Looks built around virtual key/fill lighting |
| **Mood** | Stylised colour and atmosphere |
| **Men** | Grooming-oriented looks: even skin, no makeup, light shaping |

26 built-in looks ship at launch. Saving a look stores only what differs from defaults; applying a look resets everything else, so looks are predictable. Right-click a look to rename, duplicate, delete, or **assign a hotkey** (switching looks live is a common Stream Deck action).

Looks are part of the **scene system** too: a scene can specify a look for a camera ("Late-night chill" scene switches the camera to a Mood look), applied with a smooth cross-fade.

### Skin

| Control | What it does |
|---|---|
| **Smooth** | Frequency-separated smoothing (base / blemish band / fine grain). Removes blemishes, keeps pores and film grain |
| **Foundation** | Evens melanin and hemoglobin variation |
| **Skin tone** | Lighter ↔ deeper, by melanin only |
| **Redness** | Removes excess hemoglobin (flush, irritation) |
| **Shadows** | Lifts dark circles under the eyes |
| **Smile lines** | Softens nasolabial folds |
| **Brighten teeth** | Whitens teeth inside the tracked mouth region only |
| **Bright eyes** | Lifts and clarifies the eye whites and iris |
| **Clarity** | Local contrast for definition without sharpening artefacts |
| **Smooth all visible skin** | Extends the skin edits to neck, arms and hands using the learned body-skin mask |

### Shape

27 reshape controls, each a smooth, landmark-driven warp, bounded so faces stay human:

| Region | Controls |
|---|---|
| **Face** | Natural, Slim, V-shape, Narrow, Small face, Cheekbones, Jawbone, Chin length, Forehead, Head size |
| **Eyes** | Enlarge, Distance, Position, Tilt, Inner corner, Outer corner, Lower eyelid, Iris size |
| **Nose** | Narrow, Lift, Bridge, Tip |
| **Mouth** | Size, Position, Smile |
| **Brows** | Thickness, Position |

Warps are anchored to the face and fade out to zero at the face boundary, so backgrounds, door frames and hands don't bend.

### Makeup

15 built-in makeup looks, or build your own:

- **Lips**: colour, opacity, gloss.
- **Blush**: colour, opacity, three placements (apple, lifted, draped).
- **Eyeshadow**: colour, opacity.
- **Eyeliner**: style, thickness.
- **Contour** and **Highlight**.
- **Brow tint**.
- **Grade**: a colour grade bundled with the makeup look, or a user `.cube` LUT.

Makeup is deposited as pigment in optical density, so it sits *under* the existing light and shadow like real makeup.

### Effects

23 face-tracked stickers and distortions across **Cute**, **Cool**, **Glam** and **Funny** (heart eyes, ears, glasses, crowns, freckles, …). Sprites anchor to tracked points (eyes, each eye, nose, mouth, chin, forehead, top of head, each cheek), sized in eye-distance units so they scale with distance from the camera, with optional animation (bob, pulse, spin, sway, twinkle) and blend modes (normal, add, multiply).

Users can **import their own stickers** (PNG/SVG) and place them by dragging on a face diagram; the anchor and offsets are saved as a custom effect.

### Atmosphere

- **Ambience** (13 GPU particle systems): Sparkles, Golden Dust, Bokeh, Hearts, Sakura, Snow, Fireflies, Light Leak, Purple Mist, Night Fog, Dreamy, Confetti, Bubbles. Particles respect the person mask, so they can fall *behind* the person.
- **Motion** (6 camera-motion effects): Handheld, Beat Pulse (driven by a chosen audio track), RGB Split, Slow Drift, Glitch, Wobble.

### Color

26 colour grades in **Natural**, **Emotional**, **Vivid**, **Film** and **B&W** (Clear, Fresh, Soft, Bright, Honey, Milk, …), baked into 3D LUTs at runtime. Custom grade controls: exposure, contrast, curve, saturation, vibrance, warmth, tint, hue, fade, roll-off, per-channel gamma, shadow/highlight tint, mono with tint. Users can import `.cube` LUTs.

### Light

9 whole-body virtual lights that follow the real silhouette using the person mask: **Soft Box**, **Butterfly**, **Rembrandt**, **Split**, **Golden Hour**, **Moonlight**, **Spotlight**, **Neon**, **Rim Glow**. Each exposes key/fill/rim amounts, colour, direction, and background dim. Light is applied in linear light so it reads as real illumination rather than a colour overlay.

### Background

Built on the same person segmentation the skin pipeline already runs:

| Mode | Description |
|---|---|
| **Off** | |
| **Blur** | Lens-style bokeh blur with adjustable strength; edges refined to the frame's real edges |
| **Remove** | Transparent background, so the camera can be composited over any layer (a green screen without the green) |
| **Replace** | Image, video, colour, or another Lightair layer/scene behind the person |
| **Dim** | Darken and desaturate the room to pull focus |

Edge quality controls: feather, shift (shrink/grow), hair detail, and temporal stability.

### Framing

Uses the face tracker:

- **Auto-frame**: smoothly keeps the face framed (crop-zoom inside the camera image) with selectable shot sizes (close, medium, wide) and follow speed. Works per format: the vertical format can auto-frame tighter than landscape from the same camera.
- **Level horizon**: corrects small camera roll.

## Pipeline

```text
Camera frame (GPU texture)
   │
   ├─► Tracker (async, frame-tagged)        ├─► Segmenter (≈15 Hz, async)
   │     macOS: Apple Vision, 76 landmarks   │     multiclass: face skin, body skin,
   │     Windows/Linux: MediaPipe Face        │     hair, clothes, person
   │     Landmarker (478 pts) + YuNet        │     → GPU joint-bilateral upsample
   │     → One-Euro smoothing, history,      │       to the frame's real edges
   │       interpolation, frame sync         │
   ▼                                         ▼
Pigment calibration (every 2 s): melanin & hemoglobin axes for this face + camera
   │
   ▼
GPU passes inside libobs (single filter, ordered):
  1. Frequency-separated smoothing      6. Atmosphere (behind/in front of person)
  2. Pigment edits (tone, foundation,    7. Colour grade (LUT)
     redness, shadows, teeth, eyes)      8. Relight
  3. Face warp (shape)                  9. Background
  4. Makeup deposit                     10. Framing (crop/zoom)
  5. Stickers & distortions
```

- **Frame synchronisation.** Landmarks are tagged with the frame they were computed from; the filter delays output by 1–3 frames (configurable, "Lock effects to the face") so makeup and stickers never slide. Repeated-frame detection prevents double processing when the camera runs slower than the canvas.
- **Single pass budget.** All tabs share one tracking and segmentation result and one filter instance; enabling more tabs adds shader work, not more inference.
- **Placement in the engine.** Implemented as the `lightair-effect-stack` libobs module. The tracker core is portable C++ (`tracker-core`) with platform backends (Vision on macOS; ONNX Runtime with Core ML / DirectML / CPU execution providers elsewhere). ONNX Runtime ships privately named and loaded by full path so it can't clash with other plugins.
- **Shared face service.** The tracker is a Lightair-wide service: other features (auto-framing, "speaker is looking at camera" heuristics for auto-switch, face-anchored layer effects) subscribe to the same results instead of running their own models.

## Performance budget

Measured baseline (M4 Pro, 1080p30 camera, 1080p60 canvas): **+0.6–0.8 ms render time per frame, about +15 % of one CPU core, +7–9 % GPU, zero skipped frames** with every tab active. Tracking runs on Apple Vision and segmentation mostly on the Neural Engine, so little competes with a game for the GPU.

Lightair enforces this budget in CI with a recorded-camera benchmark. Lightair Effect Stack has a **Performance** setting (Quality / Balanced / Light) that changes segmentation rate and resolution, smoothing kernel size and particle counts, and the Stats panel shows its cost per camera.

## Multiple people & multiple cameras

- Each camera input has its own Lightair Effect Stack instance and look.
- **Multiple faces** in one frame: the tracker follows up to 4 faces; skin and makeup apply to all, reshape and stickers can be limited to the largest face or a selected face.

## Privacy

All processing is **on-device**. No frames, landmarks or masks leave the machine. Models ship in the app bundle (MediaPipe Face Landmarker and selfie multiclass segmentation, Apache-2.0; OpenCV YuNet, MIT) with their licences in the third-party notices.

## Content & extensibility

Looks, makeup looks, stickers, ambiences, lights and colour grades are **data** (versioned JSON + assets), not code:

- Built-in content ships in `assets/effect-stack/`.
- User content lives in the user library and syncs across projects.
- Packs can be shared as a single `.lightairlook` file (zip of JSON + assets) and imported by drag-and-drop.

## Remote control

Every Lightair Effect Stack control is addressable through the remote API and the command system: apply look, set any slider, toggle a tab, cycle looks, set intensity. The Stream Deck plugin ships with Lightair Effect Stack actions (look buttons with face thumbnails, toggle background blur).
