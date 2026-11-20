# Avatars (VTubing)

Status (2026-11-20): **VRM**, **Live2D** and **Inochi2D** avatars work, following the face; VRM
also follows the body and hands (macOS), still jittery. Next: nijigenerate (nijilive) puppets,
then Ayatsuri 2D and Umamo. **To continue in another session, start at "Handoff" below.**

An **Avatar** is an object like a camera: a character model that moves with the person in front
of a camera. The camera's picture is never shown; only the character is. Any camera works
(a webcam, an iPhone by Continuity Camera or cable, a capture card).

## Formats, in order

| # | Format | Files | Runtime | Notes |
|---|---|---|---|---|
| 1 | **VRM** 0.x and 1.0 | `.vrm` (glTF) | Ours | Open; humanoid bones, expressions, MToon, spring bones |
| 2 | **Live2D** Cubism 3–5 | `.model3.json`, `.moc3`, textures, `.physics3.json`, `.exp3.json`, `.motion3.json` | mocari (pure Rust, MIT; Lightair's fork at third_party/mocari, github.com/MoonGoblinDev/Mocari) — no Cubism Core or SDK | In: face → standard parameters (angles, body, eyes, gaze, brows, vowels or mouth, breath), the model's own EyeBlink and LipSync groups, physics, pose, expressions; drawn with clip masks, blend modes, multiply and screen colours (`lightair-engine::avatar2d`). See "Licensing" |
| 3 | **Inochi2D** | `.inp`, `.inx` | Inox2D (Rust, BSD-2; Lightair's fork at third_party/inox2d, github.com/MoonGoblinDev/inox2d), with Inochi2D 0.8's MeshGroups added | Done: parameters found by name from the face (head, body, eyes, gaze, brows, mouth, breath), physics, MeshGroups, masks, composites, blend modes (`lightair-avatar::inochi2d`, drawn by `lightair-engine::avatar_inochi2d`) |
| 4 | **nijigenerate** | nijilive puppets (`.inp`/`.inx`, derived from Inochi2D 0.8) | the Inox2D fork, extended for nijilive's differences (reference: github.com/nijigenerate/nijilive, D, BSD-2) | MeshGroups (shared with Inochi2D 0.8) done; nijilive's own nodes not started |
| 5 | **Ayatsuri 2D** | its own models (it also reads `.moc3` v3 to v5.3) | github.com/Ayatsuri2D/ayatsuri_2d (Rust, GPL-3.0: fine for Lightair) | Not started |
| 6 | **Umamo** | `.uma` (its native format: a ZIP with the puppet graph as JSON, buffers, the texture atlas); it also exports `.moc3`, which Live2D's path reads | github.com/umamoorg/umamo (Kotlin, GPL-3.0; future2d/umamo is an old fork without `.uma`): reader and evaluator ported to Rust | Done: `lightair-avatar::uma` (keyform grids, warp and rotation deformers, blend shapes, glue, clip masks), drawn by `lightair-engine::avatar_uma` |

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
- **Models** (`lightair-avatar::vrm`, `live2d`, `inochi2d`; later nijilive, …): parsed once,
  off the graphics thread.
- **Posing** (`lightair-avatar::pose`): bones from the rig (head and neck, a little spine),
  arms down from the T-pose, idle breathing; expressions (blink, vowels, look); spring bones;
  CPU skinning and morphs, so drawing is one dynamic vertex buffer per mesh.
- **Drawing** (`lightair-engine::avatar`): libobs's graphics (Metal/OpenGL/Direct3D alike):
  a toon shader close to MToon (shade, shade shift and toony, rim, emission, outline as an
  inverted hull), alpha modes (opaque, cutout, blend sorted after), into a texture with depth,
  then drawn as the source. Transparent around the character.
- **Object** (`lightair_avatar`): settings `file`, `device` (camera's unique id), `mirror`,
  `framing` (head, bust, half, full), `width`, `height`, `calibration`.


## Handoff (2026-11-20, Inochi2D added the same day)

### What's built

- **The Avatar object** (`lightair_avatar`): Add Object › Avatar, or Object › Add Avatar…, takes
  a `.vrm`, a Live2D `.model3.json` or an Inochi2D `.inp`/`.inx`. Inspector (`crates/lightair-app/src/shell/avatar_props.rs`):
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
- **Umamo** (`crates/lightair-avatar/src/uma.rs`, drawn by `crates/lightair-engine/src/avatar_uma.rs`):
  `.uma` read (manifest, `model/puppet.json` + `model/buffers.bin`, textures as atlas pages or
  separate tiles) and evaluated as Umamo's runtime does (ported from its Kotlin at commit
  f7ef026, files listed in the module's doc): keyform grids, warp lattices, rotation deformers,
  blend shapes, glue, draw order, part visibility, masks. The face drives Cubism's standard
  parameters with Live2D's conventions. A `.uma` carries no physics, pose groups, motions or
  expressions (Umamo keeps those only for moc3 export): so Mao shows both arm sets, as Umamo
  does. Not done: Cubism 5.3's advanced blend modes (drawn normal), an isolated part's own layer.
  Tried on Mao turned into `.uma` by Umamo 0.5.0 itself: correct, through libobs too.
- **Live2D** (`crates/lightair-avatar/src/live2d.rs` on mocari; drawn by
  `crates/lightair-engine/src/avatar2d.rs`): standard parameters from the face, the model's
  EyeBlink/LipSync groups, physics, pose, expressions; clip masks (a texture each time the mask
  set changes), normal/additive/multiply blends, multiply and screen colours. Tried on Live2D's
  sample "Niziiro Mao" (moc3 v5, blend shapes): correct.
- **Inochi2D** (`crates/lightair-avatar/src/inochi2d.rs` on the Inox2D fork; drawn by
  `crates/lightair-engine/src/avatar_inochi2d.rs`): parameters a face drives are found by their
  names' words (`by_name`: "Head:: Yaw-Pitch", "Head:: Roll", "Body:: …", "Eye:: Left:: Blink",
  "… Move" for gaze, brows, "Mouth:: Shape" x form / y open, "Mouth:: Open", "Breath"; physics,
  arms, hair, tail are left alone); Inochi Session's bindings in a puppet, when there, set the
  head's reach in degrees. Signs checked on renders: +x on the head's yaw-pitch turns toward the
  viewer's right, +y looks up, roll is the opposite of ours. Inox2D's draw calls are recorded into
  a list of `Op`s (masks, mask content, parts, composites) with each part's vertices posed, which
  the libobs renderer replays: masks into a texture (libobs's Metal shaders can't `discard`, so a
  mask's threshold is a step and masks add up or dodge by blending), a composite's children into
  a layer laid down after with its tint and blend, Inochi2D's seven blend modes as blend
  functions. Inochi2D's textures are stored premultiplied (no premultiplying again). Inox2D
  panics on bad puppets: loading and posing are inside `catch_unwind` (a puppet that panics while
  posing stops where it was). Tried on Inochi2D's samples Aka (0.7) and Midori (0.8, MeshGroups):
  correct, through libobs too.

### Where the code is

| Piece | Files |
|---|---|
| Source, camera, trackers, VRM drawing, status/calibrate procs | `crates/lightair-engine/src/avatar.rs` |
| Live2D drawing; `Flat`/`FlatGpu`, either flat kind | `crates/lightair-engine/src/avatar2d.rs` |
| Inochi2D drawing | `crates/lightair-engine/src/avatar_inochi2d.rs` |
| Umamo model and drawing | `crates/lightair-avatar/src/uma.rs`, `crates/lightair-engine/src/avatar_uma.rs` |
| Models, face rig, body solve, posing | `crates/lightair-avatar/src/{vrm,glb,rig,body,pose,live2d,inochi2d}.rs` |
| Trackers (C/C++/ObjC++) | `modules/lightair-effect-stack/src/{face-api.c,body-tracker.h}`, `src/tracker/body-*.{cpp,mm,hpp}` |
| Models for the trackers | `modules/lightair-effect-stack/data/models/` (+ `NOTICE.md`); Core ML conversions by `tools/convert-coreml.py body` |
| App | `crates/lightair-app/src/shell/avatar_props.rs`, `add_avatar` in `shell.rs`, Kind::Avatar in `shell/add_object.rs`, Object › Add Avatar… in `shell/app_menu.rs` |
| Live2D runtime | `third_party/mocari` (submodule: github.com/MoonGoblinDev/Mocari, a fork of Eatgrapes/Mocari) |
| Inochi2D runtime | `third_party/inox2d` (submodule: github.com/MoonGoblinDev/inox2d, a fork of Inochi2D/inox2d); MeshGroups in `inox2d/src/meshgroup.rs` |

### Running and testing

- Sample models (not in the repository): VRM `~/Development/vroid-re/reference/exports/base-female.vrm`,
  `base-male.vrm`, `~/Development/vroid-re/exports/live-test.vrm` (its bow sits over the mouth
  at rest: the model's own); Live2D `~/Downloads/mao_en/runtime/mao_pro.model3.json`; Inochi2D
  `~/Downloads/inochi2d-samples/Aka.inx`, `Midori.inx` (CC BY 4.0, from
  media.githubusercontent.com/media/Inochi2D/example-models/main/<name>.inx; the repository holds
  only LFS pointers); Umamo `~/Downloads/umamo-samples/mao_pro.uma` (one atlas page),
  `mao_pro_t06.uma` (two pages), `mao_pro_t06_tiles.uma` (separate tiles), made from Live2D's Mao
  by Umamo 0.5.0's own classes run headlessly (`java -cp umamo-macos-arm64-0.5.0.jar
  MakeUma.java`; Umamo's CLI can't write `.uma`): Moc3 document → `UmaDocumentBridge.documentOf` →
  `Uma.write`.
- `LIGHTAIR_VRM=<.vrm> LIGHTAIR_VRM_OUT=<dir> cargo test -p lightair-avatar --release --test vrm`:
  loading, posing, software renders; `LIGHTAIR_LIVE2D=<.model3.json> cargo test -p lightair-avatar --release --test live2d`.
- `LIGHTAIR_INOCHI2D=<.inx> LIGHTAIR_INOCHI2D_OUT=<dir> cargo test -p lightair-avatar --release --test inochi2d -- --nocapture`:
  the parameters found (printed), poses drawn in software the way libobs draws them;
  `LIGHTAIR_INOCHI2D_PROBE=x,y` lists the parts drawing that pixel, `LIGHTAIR_INOCHI2D_ONLY=<name>`
  draws only parts so named. `cargo run -p lightair-avatar --release --example inochi2d-dump <.inx>`
  prints a puppet's meta, parameters and vendor data (Inochi Session's bindings).
- `LIGHTAIR_UMA=<.uma> LIGHTAIR_UMA_OUT=<dir> cargo test -p lightair-avatar --release --test uma -- --nocapture`:
  poses drawn in software; `LIGHTAIR_UMA_LIVE2D=<model3.json>` draws the same poses from the moc3
  through mocari, to compare.
- Through real libobs: `cargo test -p lightair-preview --test preview avatar` with `LIGHTAIR_VRM`,
  `LIGHTAIR_LIVE2D`, `LIGHTAIR_INOCHI2D`, `LIGHTAIR_UMA`, `LIGHTAIR_VRM_OUT` (pictures), `LIGHTAIR_BODY_IMAGE` (the body tracker on a
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

### The Inox2D fork

- Same arrangement as mocari: `third_party/inox2d` (origin MoonGoblinDev/inox2d, upstream
  Inochi2D/inox2d, branch `main`); Lightair depends on `third_party/inox2d/inox2d`. Upstream:
  `git -C third_party/inox2d fetch upstream && git -C third_party/inox2d merge upstream/main`.
- Lightair's commit `37bc27a` on upstream d4dd9dd: MeshGroups (ported from inochi2d v0.8.7's
  `core/nodes/meshgroup`: fixed and dynamic groups, translated children, propagating
  composites; a node follows the nearest group reached through Parts), params applied at (0, 0)
  too, `Puppet::params()`, `Puppet::textured_meshes()`, `ParamCtx::get()`,
  `InoxNodeUuid::raw()`, glam 0.30, warnings fixed. **Not pushed yet** (needs the owner's word;
  until then the submodule points at a commit only this checkout has).

### Next, in order

1. **Live2D polish**: check Calibrate and Mirror with a person (signs and ranges were checked on
   renders only). Done since: the model's Idle motions play under the face (one after another;
   the face's parameters win), `ParamEyeLSmile`/`ParamEyeRSmile` from a clear smile; expressions
   already fade in and out (mocari's ExpressionManager). Still: the expression weight setting
   for Live2D (it needs the manager's players' `set_weight`); framing of wide models (centred on
   the head's top fifth today); more test models (Cubism 3 and 4, several textures, inverted
   masks, offscreen parts).
2. **Inochi2D** (done; polish when someone uses it): check with a person (signs and ranges were
   checked on renders only); try more puppets, especially ones rigged by others with other
   parameter names (`by_name` in `inochi2d.rs`); Inochi Session's ExpressionBindings (Lua-like
   expressions) and its Blendshape sources beyond the ones read; opacity and tint bindings (Inox2D
   ignores them: `BindingValues::Opacity`); disabled nodes (`enabled: false`, drawn today).
3. **nijigenerate / nijilive**: compare nijilive's puppet format with Inochi2D 0.8 and extend
   the Inox2D fork for the differences. MeshGroups (from nijigenerate's author, in Inochi2D
   0.8) are in already; nijilive adds deformers (`core/nodes/deformer`: grid, path/curve,
   physics drivers), filters and more node types: read `source/nijilive/core/nodes/` and port
   what puppets use. Sample puppets: nijigenerate's own.
4. **Ayatsuri 2D**: read its own format with its Rust crates (GPL-3.0); its `.moc3` reader is a
   second opinion for Live2D models mocari draws wrongly.
5. **Umamo** (done; polish): try puppets made in Umamo from a PSD (only Mao imported into Umamo
   so far); isolated parts' layers, flip channels, part-level and warp blend shapes are ported but
   untested; Cubism 5.3's advanced blend modes.
6. **VRM tracking jitter** (see "Known issues").
7. **Windows**: compile and try the body tracker's ONNX path (`body-onnx.cpp`); body following
   is off there in the inspector's words until then.
8. **Later** (below).

## Known issues (to come back to)

- **Inochi2D, small differences from the source art** (2026-11-20): on Midori a sliver of the back
  hair's texture shows above the head where the leaf covers it in the art, and a thin line or two
  beside the bow; the same parts drawn in software look the same, so it's in posing (Inox2D's
  transforms or physics against the reference), not drawing.
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
