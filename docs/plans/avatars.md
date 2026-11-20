# Avatars (VTubing)

Status (2026-11-20): **VRM** and **Live2D** avatars work, following the face; VRM also follows
the body and hands (macOS), still jittery. Next: the remaining 2D formats. **To continue in
another session, start at "Handoff" below.**

An **Avatar** is an object like a camera: a character model that moves with the person in front
of a camera. The camera's picture is never shown; only the character is. Any camera works
(a webcam, an iPhone by Continuity Camera or cable, a capture card).

## Formats, in order

| # | Format | Files | Runtime | Notes |
|---|---|---|---|---|
| 1 | **VRM** 0.x and 1.0 | `.vrm` (glTF) | Ours | Open; humanoid bones, expressions, MToon, spring bones |
| 2 | **Live2D** Cubism 3–5 | `.model3.json`, `.moc3`, textures, `.physics3.json`, `.exp3.json`, `.motion3.json` | mocari (pure Rust, MIT; Lightair's fork at third_party/mocari, github.com/MoonGoblinDev/Mocari) — no Cubism Core or SDK | In: face → standard parameters (angles, body, eyes, gaze, brows, vowels or mouth, breath), the model's own EyeBlink and LipSync groups, physics, pose, expressions; drawn with clip masks, blend modes, multiply and screen colours (`lightair-engine::avatar2d`). See "Licensing" |
| 3 | **Inochi2D** | `.inp`, `.inx` | Inox2D (Rust, BSD-2: github.com/Inochi2D/inox2d), to fork as mocari was | Open format. Not started |
| 4 | **nijigenerate** | nijilive puppets (`.inp`/`.inx`, derived from Inochi2D 0.8) | Inox2D, extended for nijilive's differences (reference: github.com/nijigenerate/nijilive, D, BSD-2) | Not started |
| 5 | **Ayatsuri 2D** | its own models (it also reads `.moc3` v3 to v5.3) | github.com/Ayatsuri2D/ayatsuri_2d (Rust, GPL-3.0: fine for Lightair) | Not started |
| 6 | **Umamo** | its own format | github.com/future2d/umamo (Kotlin editor, GPL-3.0): format to read in Rust | Not started |

Each format is a loader plus a poser; tracking, the mapping from tracking to the model, the
object, the inspector and drawing are shared.

## Pieces

```
camera (private capture) ──► face tracker (effect stack, MediaPipe 478) ──► face rig
                                                                            │
                                         head yaw/pitch/roll, eyes, gaze, brows, mouth, vowels
                                                                            │
model file ──► loader (per format) ──► model ──► poser (per format) ◄──────┘
                                                   │  bones, morphs/parameters, physics
                                                   ▼
                                     mesh ──► drawn by libobs (lightair_avatar source)
```

- **Tracking** (`modules/lightair-effect-stack/src/face-api.c`): the effect stack's face tracker
  as libobs global procs (`lightair_face_tracker_new/submit/get/free`). The avatar renders its
  camera small off screen each frame, reads it back and submits it; landmarks come back
  smoothed and interpolated to the render frame.
- **Face rig** (`lightair-avatar::rig`): from the 2D landmarks, head rotation (fitted to
  MediaPipe's canonical face), eye openness, gaze (irises), brows, mouth open and width, and
  the vowels (A I U E O). Calibrated to a neutral face (Calibrate), smoothed, mirrored.
- **Models** (`lightair-avatar::vrm`, `live2d`; later `inochi2d`, …): parsed once, off the
  graphics thread.
- **Posing** (`lightair-avatar::pose`): bones from the rig (head and neck, a little spine),
  arms down from the T-pose, idle breathing; expressions (blink, vowels, look); spring bones;
  CPU skinning and morphs, so drawing is one dynamic vertex buffer per mesh.
- **Drawing** (`lightair-engine::avatar`): libobs's graphics (Metal/OpenGL/Direct3D alike):
  a toon shader close to MToon (shade, shade shift and toony, rim, emission, outline as an
  inverted hull), alpha modes (opaque, cutout, blend sorted after), into a texture with depth,
  then drawn as the source. Transparent around the character.
- **Object** (`lightair_avatar`): settings `file`, `device` (camera's unique id), `mirror`,
  `framing` (head, bust, half, full), `width`, `height`, `calibration`.


## Handoff (2026-11-20)

### What's built

- **The Avatar object** (`lightair_avatar`): Add Object › Avatar, or Object › Add Avatar…, takes
  a `.vrm` or a Live2D `.model3.json`. Inspector (`crates/lightair-app/src/shell/avatar_props.rs`):
  status line (+ "Body: …" for VRM), Camera (a device; its picture is never shown), Calibrate,
  Follows (VRM: face only / upper body / full body), Framing, Zoom, Size, Mirror, Show tracking
  (the trackers' picture and points in the avatar's corner), Expression (VRM's standard ones,
  or a Live2D model's own). The model file is the Source section's "Replace…".
- **Face** (every model): the Effect Stack's MediaPipe face tracker through libobs global procs
  (`modules/lightair-effect-stack/src/face-api.c`), measured into a `Face`
  (`crates/lightair-avatar/src/rig.rs`: head fitted to MediaPipe's canonical face, eyes, gaze,
  brows, mouth, vowels), calibrated, smoothed, mirrored.
- **Body and hands** (VRM, macOS; Windows written, never compiled): MediaPipe BlazePose (person
  detector, then the full landmark model following from its own alignment points, the detector
  again every 30 frames) and hand landmarks cropped from the pose's wrist and knuckles
  (`src/tracker/body-mediapipe.cpp`, `body-coreml.mm`, `body-onnx.cpp`, `body-tracker.cpp`,
  procs `lightair_body_tracker_*` in `face-api.c`). Solved onto bones in
  `crates/lightair-avatar/src/body.rs` and `pose.rs` (`Avatar::body_bones`).
- **VRM** (`crates/lightair-avatar/src/vrm.rs`, `glb.rs`, `pose.rs`): 0.x and 1.0, humanoid,
  expressions, look-at, spring bones, MToon-like toon shader with outlines, drawn by
  `crates/lightair-engine/src/avatar.rs`. Only VRoid 1.0 exports have been tried (0.x untested).
- **Live2D** (`crates/lightair-avatar/src/live2d.rs` on mocari; drawn by
  `crates/lightair-engine/src/avatar2d.rs`): standard parameters from the face, the model's
  EyeBlink/LipSync groups, physics, pose, expressions; clip masks (a texture each time the mask
  set changes), normal/additive/multiply blends, multiply and screen colours. Tried on Live2D's
  sample "Niziiro Mao" (moc3 v5, blend shapes): correct.

### Where the code is

| Piece | Files |
|---|---|
| Source, camera, trackers, VRM drawing, status/calibrate procs | `crates/lightair-engine/src/avatar.rs` |
| Live2D drawing | `crates/lightair-engine/src/avatar2d.rs` |
| Models, face rig, body solve, posing | `crates/lightair-avatar/src/{vrm,glb,rig,body,pose,live2d}.rs` |
| Trackers (C/C++/ObjC++) | `modules/lightair-effect-stack/src/{face-api.c,body-tracker.h}`, `src/tracker/body-*.{cpp,mm,hpp}` |
| Models for the trackers | `modules/lightair-effect-stack/data/models/` (+ `NOTICE.md`); Core ML conversions by `tools/convert-coreml.py body` |
| App | `crates/lightair-app/src/shell/avatar_props.rs`, `add_avatar` in `shell.rs`, Kind::Avatar in `shell/add_object.rs`, Object › Add Avatar… in `shell/app_menu.rs` |
| Live2D runtime | `third_party/mocari` (submodule: github.com/MoonGoblinDev/Mocari, a fork of Eatgrapes/Mocari) |

### Running and testing

- Sample models (not in the repository): VRM `~/Development/vroid-re/reference/exports/base-female.vrm`,
  `base-male.vrm`, `~/Development/vroid-re/exports/live-test.vrm` (its bow sits over the mouth
  at rest: the model's own); Live2D `~/Downloads/mao_en/runtime/mao_pro.model3.json`.
- `LIGHTAIR_VRM=<.vrm> LIGHTAIR_VRM_OUT=<dir> cargo test -p lightair-avatar --release --test vrm`:
  loading, posing, software renders; `LIGHTAIR_LIVE2D=<.model3.json> cargo test -p lightair-avatar --release --test live2d`.
- Through real libobs: `cargo test -p lightair-preview --test preview avatar` with `LIGHTAIR_VRM`,
  `LIGHTAIR_LIVE2D`, `LIGHTAIR_VRM_OUT` (pictures), `LIGHTAIR_BODY_IMAGE` (the body tracker on a
  picture), `LIGHTAIR_BODY_JSON_OUT`.
- Live tracking recorded: run Lightair with `LIGHTAIR_AVATAR_DUMP=<file>` (e.g.
  `open -g --env LIGHTAIR_AVATAR_DUMP=/tmp/body.jsonl /Applications/Lightair.app`), then
  `LIGHTAIR_BODY_JSONL=<file> cargo test -p lightair-avatar --release --test vrm replayed -- --nocapture`
  replays it and prints the arms' shake (`SHOW_JUMPS=1` lists jumps).
- Checking the live app without touching the screen: Lightair's MCP server
  (`/Applications/Lightair.app/Contents/MacOS/lightair mcp`): `add_object` (kind
  `lightair_avatar`, settings as above), `screenshot`, `undo`, `get_state`.
- The ONNX model files are git-ignored (`/modules/*/data/models/*.onnx`); only the Core ML
  conversions are committed. The three body models come from OpenCV Zoo on Hugging Face (see
  `NOTICE.md`); a Windows build needs them in `data/models/` next to the face ones.

### The mocari fork

- Changes Lightair needs from mocari go in `third_party/mocari`: commit there, push to
  `origin` (MoonGoblinDev/Mocari), then commit the new submodule pointer in Lightair. Upstream
  fixes: `git -C third_party/mocari fetch upstream && git -C third_party/mocari merge upstream/master`.
  Nothing has had to change in it yet (pinned at upstream 29a4792).

### Next, in order

1. **Live2D polish**: check Calibrate and Mirror with a person (signs and ranges were checked on
   renders only); play the model's Idle motion under tracking (mocari has `MotionPlayer`; motion
   groups are in `model3.json`); `ParamEyeLSmile`/`ParamEyeRSmile` from a smile; expressions with
   fades (mocari's ExpressionManager fades; a weight setting exists but isn't used for Live2D);
   framing of wide models (centred on the head's top fifth today); more test models (Cubism 3 and
   4, several textures, inverted masks, offscreen parts).
2. **Inochi2D**: fork Inox2D into `third_party/inox2d` the way mocari was; a `crates/lightair-avatar/src/inochi2d.rs`
   puppet (load `.inp`/`.inx`, set parameters from the `Face`: Inochi2D parameters are 1D or 2D
   with arbitrary names, so map the conventional ones (head, eyes, mouth) and let the rest be;
   Inochi Session's tracking bindings are the reference); draw like Live2D in a
   `crates/lightair-engine` module (Inochi2D has masks, blend modes and composites). Sample
   models: Inochi2D's own (Aka, Midori) from its site.
3. **nijigenerate / nijilive**: compare nijilive's puppet format with Inochi2D 0.8 and extend
   the Inox2D fork for the differences.
4. **Ayatsuri 2D**: read its own format with its Rust crates (GPL-3.0); its `.moc3` reader is a
   second opinion for Live2D models mocari draws wrongly.
5. **Umamo**: its format, read in Rust (port from its Kotlin; GPL-3.0).
6. **VRM tracking jitter** (see "Known issues").
7. **Windows**: compile and try the body tracker's ONNX path (`body-onnx.cpp`); body following
   is off there in the inspector's words until then.
8. **Later** (below).

## Known issues (to come back to)

- **VRM body and hand tracking still jitter** (2026-11-20, set aside to start on the 2D
  formats). What's in: MediaPipe BlazePose and hands (Core ML / ONNX), joints built from the
  picture's points plus the model's depth, One Euro filtering (tuned on a replayed recording),
  arms that follow only while their hand is in view, a three-answer debounce, arms that fade back
  to rest from where they were last seen, hands turned in 3D with the forearm taking the roll.
  Still to try: filtering in bone-angle space rather than on joints, BlazePose's heavy model or
  its heatmap refinement, holding the elbow from the model's 3D only when the wrist is steady,
  per-finger filtering, and a recorded clip with the person in view to judge against
  (`LIGHTAIR_AVATAR_DUMP=<file>` records the tracker's frames; lightair-avatar's
  `replayed_tracking_is_steady` test replays them and measures the shake).

## Later

- Lip-sync from a microphone when the mouth can't be seen.
- Expression hotkeys and automation (happy, angry, …).
- iPhone face tracking (ARKit) and the VMC protocol, in and out.
- Body tracking for 2D models (Live2D's body angle and arm parameters from the body tracker).

## Licensing

VRM, Inochi2D, nijilive: open formats. Live2D: Lightair reads `.moc3` with mocari (MIT, an
independent pure Rust runtime) and never uses, downloads or bundles Cubism Core or the SDK. Live2D's Editor licence (§5.1.9) restricts its
customers from using their output files with software that competes with Live2D's; that binds
model makers, not Lightair. Decision (2026-11-20): build it; if Live2D objects, comply then.
