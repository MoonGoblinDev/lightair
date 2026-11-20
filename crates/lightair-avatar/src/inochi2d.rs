//! Inochi2D puppets (`.inp`, `.inx`) on Inox2D (third_party/inox2d, Lightair's fork), and
//! nijilive's (nijigenerate's; PathDeformers, DynamicComposites, Masks). A [`Face`] sets the
//! puppet's parameters each frame, then its physics run and its parts are posed; the result is a
//! list of draw operations ([`Op`]) in Inochi2D's order (masks, composites, blend modes), with
//! each part's vertices ready to draw (model units, y up).
//!
//! Inochi2D parameters have free names, 1D or 2D. Those a face drives are found by their names'
//! words, the way riggers name them in Inochi Creator ("Head:: Yaw-Pitch", "Head:: Roll",
//! "Eye:: Left:: Blink", "Mouth:: Shape", "Breath", …); the rest (physics, arms, a tail) keep their
//! defaults or follow physics. Inochi Session's own tracking bindings in a puppet, when there,
//! decide which way and how far: see [`Mapping`].

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use glam::{Mat4, Vec2, Vec3};
use inox2d::model::Model;
use inox2d::node::InoxNodeUuid;
use inox2d::node::components::{BlendMode, Mask, MaskMode, Masks};
use inox2d::node::drawables::{CompositeComponents, DynamicCompositeComponents, TexturedMeshComponents};
use inox2d::render::{
    CompositeRenderCtx, InoxRenderer, InoxRendererExt, MeshTexture, TexturedMeshRenderCtx, VertexBuffers,
};

use crate::Error;
use crate::rig::Face;

/// A texture, RGBA with its colour premultiplied by alpha.
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A part's mesh: its texture coordinates and triangles (its vertices change each frame).
pub struct Part {
    pub name: String,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Its texture in [`Puppet::textures`]. A nijilive Mask (a mesh only drawn into masks, whole)
    /// has a 1×1 opaque white one. A nijilive DynamicComposite's mesh has [`DYNAMIC`]: it's only
    /// drawn by [`Op::EndDynamic`], with the layer its children were drawn into.
    pub texture: usize,
    vert_offset: usize,
}

/// [`Part::texture`] of a DynamicComposite's mesh: no texture of the puppet's.
pub const DYNAMIC: usize = usize::MAX;

/// Where a DynamicComposite's texture lies on the puppet: its texture coordinate (u, v) (u to
/// the right, v down, 0..1 across the texture, as a [`Part`]'s `uvs`) shows what's drawn at
/// `origin + u · x_axis + v · y_axis` (model units, y up). The DynamicComposite's own transform
/// is in it, so it changes from frame to frame; at rest it's the rectangle its mesh's UVs span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureFrame {
    pub origin: [f32; 2],
    pub x_axis: [f32; 2],
    pub y_axis: [f32; 2],
}

impl TextureFrame {
    /// The point (model units, y up) a texture coordinate shows.
    pub fn at(&self, uv: [f32; 2]) -> [f32; 2] {
        [
            self.origin[0] + uv[0] * self.x_axis[0] + uv[1] * self.y_axis[0],
            self.origin[1] + uv[0] * self.x_axis[1] + uv[1] * self.y_axis[1],
        ]
    }
}

/// How a part or composite is laid over what's under it (Inochi2D's blend modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Multiply,
    ColorDodge,
    LinearDodge,
    Screen,
    /// Drawn only where something is already drawn.
    ClipToLower,
    /// Cut out of what's already drawn.
    SliceFromLower,
}

impl From<BlendMode> for Blend {
    fn from(b: BlendMode) -> Blend {
        match b {
            BlendMode::Normal => Blend::Normal,
            BlendMode::Multiply => Blend::Multiply,
            BlendMode::ColorDodge => Blend::ColorDodge,
            BlendMode::LinearDodge => Blend::LinearDodge,
            BlendMode::Screen => Blend::Screen,
            BlendMode::ClipToLower => Blend::ClipToLower,
            BlendMode::SliceFromLower => Blend::SliceFromLower,
        }
    }
}

/// Colour and opacity a part or composite is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub blend: Blend,
    pub opacity: f32,
    /// Multiplied with the texture's colour.
    pub tint: [f32; 3],
    /// Screened over it.
    pub screen: [f32; 3],
}

/// One step of drawing the puppet, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// A mask set begins: the mask is cleared to `clear` (1 when it only dodges, else 0); a pixel of
    /// a mask counts where its alpha is over `threshold`.
    BeginMasks { threshold: f32, clear: f32 },
    /// The parts drawn next as masks add to the mask (`dodge`: take away from it).
    Mask { dodge: bool },
    /// What follows is drawn only where the mask is set.
    MaskedContent,
    /// Masking ends.
    EndMasks,
    /// A part: as a mask (into the mask) or drawn.
    Part { part: usize, as_mask: bool, look: Look },
    /// A composite's children follow, drawn into a layer of their own…
    BeginComposite { as_mask: bool },
    /// …which is then laid down with this look.
    EndComposite { as_mask: bool, look: Look },
    /// A nijilive DynamicComposite begins. Not `as_mask`: the ops up to its matching
    /// [`Op::EndDynamic`] draw into a new layer: a transparent target of its own (premultiplied
    /// RGBA cleared to 0), drawn into exactly as the frame is (same projection, same blending),
    /// pushed on a stack of targets; they nest (a DynamicComposite inside another, or inside a
    /// composite) and may contain masks of their own (`BeginMasks`…`EndMasks`, which use the mask
    /// as usual, also inside the layer). `as_mask`: its children are drawn into the mask like any
    /// mask part (no layer); nothing to do.
    BeginDynamic { as_mask: bool },
    /// Its layer is finished: pop it (drawing goes back to the target below), then draw the
    /// DynamicComposite's mesh `part` (its posed vertices in [`Puppet::positions`], its triangles
    /// and UVs in [`Puppet::parts`]) into the target below, with `look` (tint, screen, opacity,
    /// blend mode) exactly as an [`Op::Part`] is drawn, the texel being the popped layer's:
    /// for a pixel whose interpolated UV is (u, v), sample the layer (bilinear, clamped to its
    /// edge) where `texture.at([u, v])` lands on it (the same projection as the frame). Masked when
    /// a `MaskedContent` is in force: its own masks (`BeginMasks`…`MaskedContent`) come just
    /// before this op, after its children, and their `EndMasks` right after it. (A renderer may
    /// equally draw the children into a texture of its own that `texture` maps onto [0,1]².)
    /// `as_mask`: nothing to draw.
    EndDynamic { part: usize, as_mask: bool, look: Look, texture: TextureFrame },
}

/// What a parameter's axis follows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    /// Degrees, scaled by `range` to -1..1.
    Yaw,
    Pitch,
    Roll,
    BodyYaw,
    BodyPitch,
    BodyRoll,
    /// 0 open to 1 shut: the model's right (0) or left (1) eye, or both.
    Blink(Option<usize>),
    /// 1 open to 0 shut.
    Open(Option<usize>),
    GazeX,
    GazeY,
    /// -1 lowered to 1 raised.
    Brow(Option<usize>),
    /// 0 shut to 1 open.
    MouthOpen,
    /// -1 pursed to 1 wide (a smile).
    MouthForm,
    /// 0 to 1, slowly.
    Breath,
}

impl Source {
    /// Centred (-1..1, mapped over the parameter's whole range) or a share (0..1 from its minimum).
    fn centred(self) -> bool {
        !matches!(self, Source::Blink(_) | Source::Open(_) | Source::MouthOpen | Source::Breath)
    }
}

/// A parameter axis a face drives: the parameter, its axis (0 x, 1 y), the source, its range
/// (degrees for the head's angles, else 1) and its direction.
#[derive(Clone, Debug)]
struct Drive {
    param: String,
    axis: usize,
    source: Source,
    range: f32,
    inverse: bool,
    min: f32,
    max: f32,
    /// Where a share (`Source::centred` false) starts: the minimum (for a nijilive puppet's blink,
    /// the parameter's default).
    rest: f32,
}

/// The parameters a face drives, by name.
#[derive(Clone, Debug, Default)]
pub struct Mapping {
    drives: Vec<Drive>,
}

impl Mapping {
    /// The parameter names driven, with what by (for logs and tests).
    pub fn describe(&self) -> Vec<String> {
        self.drives
            .iter()
            .map(|d| {
                format!(
                    "{} [{}] ← {:?}{}",
                    d.param,
                    if d.axis == 0 { "x" } else { "y" },
                    d.source,
                    if d.inverse { " (inverse)" } else { "" }
                )
            })
            .collect()
    }
}

/// The words of a parameter's name, lower case ("Head:: Yaw-Pitch" → head, yaw, pitch).
fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    let mut prev_lower = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            // camelCase splits too ("EyeBlinkLeft")
            if c.is_uppercase() && prev_lower {
                flush(&mut cur, &mut out);
            }
            prev_lower = c.is_lowercase();
            cur.extend(c.to_lowercase());
        } else {
            prev_lower = false;
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// What a parameter's axes follow, from its name: (x, y).
fn by_name(name: &str, is_vec2: bool) -> [Option<Source>; 2] {
    let w = words(name);
    let has = |k: &str| w.iter().any(|x| x == k);
    let any = |ks: &[&str]| ks.iter().any(|k| has(k));
    let side = if any(&["left", "l"]) {
        Some(1)
    } else if any(&["right", "r"]) {
        Some(0)
    } else {
        None
    };
    if any(&[
        "physics",
        "phys",
        "sim",
        "arm",
        "hand",
        "leg",
        "tail",
        "ear",
        "hair",
        "skirt",
        "ribbon",
        "light",
        "size",
        "highlight",
    ]) {
        return [None, None];
    }
    let yaw = any(&["yaw", "turn", "x"]);
    let pitch = any(&["pitch", "nod", "y"]);
    let roll = any(&["roll", "tilt", "z"]);
    if any(&["head", "face", "neck"]) || (has("angle") && !any(&["body", "eye", "brow", "mouth"])) {
        if any(&["move", "position", "pos"]) {
            return [None, None];
        }
        if is_vec2 && (yaw || pitch) {
            return [Some(Source::Yaw), Some(Source::Pitch)];
        }
        if roll {
            return [Some(Source::Roll), None];
        }
        if yaw {
            return [Some(Source::Yaw), None];
        }
        if pitch {
            return [Some(Source::Pitch), None];
        }
        return [None, None];
    }
    if any(&["body", "torso", "chest", "upper"]) {
        if any(&["move", "position", "pos"]) {
            return [None, None];
        }
        if is_vec2 && (yaw || pitch) {
            return [Some(Source::BodyYaw), Some(Source::BodyPitch)];
        }
        if roll {
            return [Some(Source::BodyRoll), None];
        }
        if yaw {
            return [Some(Source::BodyYaw), None];
        }
        if pitch {
            return [Some(Source::BodyPitch), None];
        }
        return [None, None];
    }
    if any(&["brow", "brows", "eyebrow", "eyebrows"]) {
        // ("Eyebrow:: Emotion-UpDown": x its emotion, y how high)
        if is_vec2 && any(&["updown", "up", "down", "height", "y"]) {
            return [None, Some(Source::Brow(side))];
        }
        return [Some(Source::Brow(side)), None];
    }
    if any(&["eye", "eyes", "pupil", "pupils", "iris", "gaze", "look"]) {
        if any(&["blink", "close", "closed", "shut"]) {
            return [Some(Source::Blink(side)), None];
        }
        if any(&["open", "openness"]) {
            return [Some(Source::Open(side)), None];
        }
        if is_vec2 {
            return [Some(Source::GazeX), Some(Source::GazeY)];
        }
        if any(&["y", "pitch", "up", "vertical"]) {
            return [Some(Source::GazeY), None];
        }
        if any(&["x", "yaw", "move", "ball", "look", "gaze", "horizontal"]) {
            return [Some(Source::GazeX), None];
        }
        return [None, None];
    }
    if any(&["mouth", "lip", "lips", "jaw"]) {
        // (a mouth's "blink": a shape of its own, not how open or wide)
        if any(&["blink", "close", "closed"]) {
            return [None, None];
        }
        if is_vec2 {
            // Inochi Creator's "Mouth:: Shape": x its form, y how open
            return [Some(Source::MouthForm), Some(Source::MouthOpen)];
        }
        if any(&["open", "openness", "jaw"]) {
            return [Some(Source::MouthOpen), None];
        }
        if any(&["form", "width", "wide", "smile", "emotion", "shape"]) {
            return [Some(Source::MouthForm), None];
        }
        return [None, None];
    }
    if any(&["breath", "breathe", "breathing"]) {
        return [Some(Source::Breath), None];
    }
    [None, None]
}

/// Inochi Session's sources it binds, as ours.
fn session_source(source_name: &str, source_type: &str) -> Option<Source> {
    let w = words(source_name);
    let has = |k: &str| w.iter().any(|x| x == k);
    // (the tracker's left is the person's left: the model's right as it faces us… Inochi Session
    // binds them crosswise in its own puppets, so the binding's parameter decides the side)
    match source_type {
        "BoneRotYaw" => return Some(if has("head") { Source::Yaw } else { Source::BodyYaw }),
        "BoneRotPitch" => return Some(if has("head") { Source::Pitch } else { Source::BodyPitch }),
        "BoneRotRoll" => return Some(if has("head") { Source::Roll } else { Source::BodyRoll }),
        _ => {}
    }
    if has("blink") {
        return Some(Source::Blink(None));
    }
    if has("mouth") && has("open") || has("jaw") && has("open") {
        return Some(Source::MouthOpen);
    }
    if has("mouth") && (has("emotion") || has("smile") || has("wide")) {
        return Some(Source::MouthForm);
    }
    if has("eye") && has("x") {
        return Some(Source::GazeX);
    }
    if has("eye") && has("y") {
        return Some(Source::GazeY);
    }
    if has("brow") {
        return Some(Source::Brow(None));
    }
    None
}

/// Which way Inochi2D's head parameters turn, against ours (checked on Inochi2D's sample puppets
/// Aka and Midori): +x on "Head:: Yaw-Pitch" turns the face toward the viewer's right.
const YAW_SIGN: f32 = 1.0;
/// +y looks up.
const PITCH_SIGN: f32 = 1.0;
/// +roll tilts clockwise as seen.
const ROLL_SIGN: f32 = -1.0;

impl Mapping {
    fn new(model: &Model) -> Mapping {
        let mut drives = Vec::new();
        let nijilive = model.puppet.is_nijilive();
        let params = model.puppet.params();
        let mut names: Vec<&String> = params.keys().collect();
        names.sort();
        for name in names {
            let p = &params[name];
            let sources = by_name(name, p.is_vec2);
            for (axis, s) in sources.into_iter().enumerate() {
                let Some(source) = s else { continue };
                let range = match source {
                    Source::Yaw | Source::Roll | Source::BodyYaw | Source::BodyRoll => 30.0,
                    Source::Pitch | Source::BodyPitch => 15.0,
                    _ => 1.0,
                };
                let (min, max) = (p.min[axis], p.max[axis]);
                if (max - min).abs() < 1e-6 {
                    continue;
                }
                // (a nijilive rig's eyes rest at the blink's default: below it they open wide)
                let rest = if nijilive && matches!(source, Source::Blink(_)) {
                    p.defaults[axis].clamp(min.min(max), min.max(max))
                } else {
                    min
                };
                drives.push(Drive { param: name.clone(), axis, source, range, inverse: false, min, max, rest });
            }
        }
        // Inochi Session's bindings in the puppet, where it has them: which source, how far
        let by_uuid: HashMap<u32, &String> = params.iter().map(|(n, p)| (p.uuid.0, n)).collect();
        for v in model.vendors.iter().filter(|v| v.name == "com.inochi2d.inochi-session.bindings") {
            for b in v.payload.members() {
                if b["bindingType"].as_str() != Some("RatioBinding") {
                    continue;
                }
                let Some(name) = b["param"].as_u32().and_then(|u| by_uuid.get(&u)) else { continue };
                let axis = b["axis"].as_usize().unwrap_or(0).min(1);
                let Some(source) =
                    session_source(b["sourceName"].as_str().unwrap_or(""), b["sourceType"].as_str().unwrap_or(""))
                else {
                    continue;
                };
                let range_in = [b["inRange"][0].as_f32(), b["inRange"][1].as_f32()];
                let Some(d) = drives.iter_mut().find(|d| d.param == **name && d.axis == axis) else { continue };
                // keep the side the name gives; take the binding's reach
                let same_kind = std::mem::discriminant(&d.source) == std::mem::discriminant(&source);
                if !same_kind {
                    continue;
                }
                if let [Some(lo), Some(hi)] = range_in
                    && matches!(
                        source,
                        Source::Yaw
                            | Source::Pitch
                            | Source::Roll
                            | Source::BodyYaw
                            | Source::BodyPitch
                            | Source::BodyRoll
                    )
                {
                    d.range = ((hi - lo) * 0.5).abs().max(1.0);
                }
            }
        }
        Mapping { drives }
    }

    /// The parameters' values for a face: (name, axis, value).
    fn values(&self, face: &Face, time: f32) -> Vec<(&str, usize, f32)> {
        let (yaw, pitch, roll) = face.angles();
        self.drives
            .iter()
            .map(|d| {
                let side = |s: Option<usize>, v: [f32; 2]| s.map_or((v[0] + v[1]) * 0.5, |i| v[i]);
                let n = match d.source {
                    Source::Yaw => YAW_SIGN * yaw / d.range,
                    Source::Pitch => PITCH_SIGN * pitch / d.range,
                    Source::Roll => ROLL_SIGN * roll / d.range,
                    // the body follows the head a little
                    Source::BodyYaw => YAW_SIGN * yaw * 0.3 / d.range,
                    Source::BodyPitch => PITCH_SIGN * pitch * 0.3 / d.range,
                    Source::BodyRoll => ROLL_SIGN * roll * 0.3 / d.range,
                    Source::Blink(s) => 1.0 - side(s, face.eye_open),
                    Source::Open(s) => side(s, face.eye_open),
                    Source::GazeX => YAW_SIGN * face.gaze.x,
                    Source::GazeY => face.gaze.y,
                    Source::Brow(s) => side(s, face.brow),
                    Source::MouthOpen => face.mouth_open,
                    Source::MouthForm => face.mouth_wide,
                    Source::Breath => 0.5 + 0.5 * (time * std::f32::consts::TAU / 3.5).sin(),
                };
                let n = if d.inverse { if d.source.centred() { -n } else { 1.0 - n } } else { n };
                let v = if d.source.centred() {
                    (d.min + d.max) * 0.5 + n.clamp(-1.0, 1.0) * (d.max - d.min) * 0.5
                } else {
                    d.rest + n.clamp(0.0, 1.0) * (d.max - d.rest)
                };
                (d.param.as_str(), d.axis, v)
            })
            .collect()
    }
}

pub struct Puppet {
    model: Model,
    parts: Vec<Part>,
    part_of: HashMap<u32, usize>,
    textures: Vec<Texture>,
    /// Each part's vertices, posed (model units, y up).
    positions: Vec<Vec<[f32; 2]>>,
    ops: Vec<Op>,
    pub mapping: Mapping,
    time: f32,
    first: bool,
    /// Set when Inox2D failed on this puppet: it stays as it was.
    broken: bool,
    /// The puppet at rest: its lowest and highest corner (model units, y up).
    pub bounds: (Vec2, Vec2),
    /// Across the middle of its head (the top fifth of it), where a frame centres.
    pub centre_x: f32,
}

/// Reads Inox2D's draw calls into [`Op`]s and the parts' posed vertices.
struct Recorder<'a> {
    buffers: &'a VertexBuffers,
    part_of: &'a HashMap<u32, usize>,
    parts: &'a [Part],
    ops: std::cell::RefCell<Vec<Op>>,
    positions: std::cell::RefCell<&'a mut Vec<Vec<[f32; 2]>>>,
    /// The mask sets being drawn, innermost last.
    masks: std::cell::RefCell<Vec<MaskPhase>>,
}

/// Where drawing is in a mask set. A mask source with masks of its own (nijilive puppets have
/// them) is drawn into the mask whole: its own set is skipped, so sets never nest in the ops.
#[derive(Clone, Copy, PartialEq)]
enum MaskPhase {
    Sources,
    Content,
    /// A set inside another's sources: left out; `true` while at its own sources.
    Skipped(bool),
}

impl Recorder<'_> {
    /// Whether what's drawn now is a skipped set's mask source.
    fn skipping(&self) -> bool {
        self.masks.borrow().last() == Some(&MaskPhase::Skipped(true))
    }

    fn push(&self, op: Op) {
        if !self.skipping() {
            self.ops.borrow_mut().push(op);
        }
    }
}

fn look(b: &inox2d::node::components::Blending) -> Look {
    let c = |v: Vec3| -> [f32; 3] { v.clamp(Vec3::ZERO, Vec3::ONE).to_array() };
    Look { blend: b.mode.into(), opacity: b.opacity.clamp(0.0, 1.0), tint: c(b.tint), screen: c(b.screen_tint) }
}

impl InoxRenderer for Recorder<'_> {
    fn on_begin_masks(&self, masks: &Masks) {
        let mut phases = self.masks.borrow_mut();
        if matches!(phases.last(), Some(MaskPhase::Sources | MaskPhase::Skipped(true))) {
            phases.push(MaskPhase::Skipped(true));
            return;
        }
        phases.push(MaskPhase::Sources);
        self.ops.borrow_mut().push(Op::BeginMasks {
            threshold: masks.threshold.clamp(0.0, 1.0),
            clear: if masks.has_masks() { 0.0 } else { 1.0 },
        });
    }
    fn on_begin_mask(&self, mask: &Mask) {
        if !matches!(self.masks.borrow().last(), Some(MaskPhase::Skipped(_))) {
            self.ops.borrow_mut().push(Op::Mask { dodge: mask.mode == MaskMode::Dodge });
        }
    }
    fn on_begin_masked_content(&self) {
        let mut phases = self.masks.borrow_mut();
        match phases.last_mut() {
            Some(p @ MaskPhase::Skipped(_)) => *p = MaskPhase::Skipped(false),
            Some(p) => {
                *p = MaskPhase::Content;
                self.ops.borrow_mut().push(Op::MaskedContent);
            }
            None => {}
        }
    }
    fn on_end_mask(&self) {
        let phase = self.masks.borrow_mut().pop();
        if !matches!(phase, Some(MaskPhase::Skipped(_))) {
            self.ops.borrow_mut().push(Op::EndMasks);
        }
    }
    fn draw_textured_mesh_content(
        &self,
        as_mask: bool,
        components: &TexturedMeshComponents,
        render_ctx: &TexturedMeshRenderCtx,
        id: InoxNodeUuid,
    ) {
        let Some(&part) = self.part_of.get(&uuid(id)) else { return };
        self.pose(part, components.transform, render_ctx);
        self.push(Op::Part { part, as_mask, look: look(&components.drawable.blending) });
    }
    fn begin_composite_content(&self, as_mask: bool, _: &CompositeComponents, _: &CompositeRenderCtx, _: InoxNodeUuid) {
        self.push(Op::BeginComposite { as_mask });
    }
    fn finish_composite_content(
        &self,
        as_mask: bool,
        components: &CompositeComponents,
        _: &CompositeRenderCtx,
        _: InoxNodeUuid,
    ) {
        self.push(Op::EndComposite { as_mask, look: look(&components.drawable.blending) });
    }
    fn begin_dynamic_composite(
        &self,
        as_mask: bool,
        _: &DynamicCompositeComponents,
        _: &TexturedMeshRenderCtx,
        _: InoxNodeUuid,
    ) {
        self.push(Op::BeginDynamic { as_mask });
    }
    fn finish_dynamic_composite(
        &self,
        as_mask: bool,
        components: &DynamicCompositeComponents,
        render_ctx: &TexturedMeshRenderCtx,
        id: InoxNodeUuid,
    ) {
        let Some(&part) = self.part_of.get(&uuid(id)) else { return };
        let m: &Mat4 = components.transform;
        self.pose(part, m, render_ctx);
        // the texture's corner and sides, from its centre and size in the composite's space
        let (centre, size) = (components.data.texture_offset, components.data.texture_size);
        let corner = m.transform_point3((centre - size * 0.5).extend(0.0));
        let x_axis = m.transform_vector3(Vec3::new(size.x, 0.0, 0.0));
        let y_axis = m.transform_vector3(Vec3::new(0.0, size.y, 0.0));
        let texture = TextureFrame {
            origin: [corner.x, -corner.y],
            x_axis: [x_axis.x, -x_axis.y],
            y_axis: [y_axis.x, -y_axis.y],
        };
        self.push(Op::EndDynamic { part, as_mask, look: look(&components.drawable.blending), texture });
    }
}

impl Recorder<'_> {
    /// A mesh's vertices, posed (model units, y up).
    fn pose(&self, part: usize, m: &Mat4, render_ctx: &TexturedMeshRenderCtx) {
        let off = render_ctx.vert_offset as usize;
        debug_assert_eq!(off, self.parts[part].vert_offset);
        let mut positions = self.positions.borrow_mut();
        let out = &mut positions[part];
        out.clear();
        for k in 0..render_ctx.vert_len {
            let v = self.buffers.verts[off + k] + self.buffers.deforms[off + k];
            let p = m.transform_point3(v.extend(0.0));
            out.push([p.x, -p.y]);
        }
    }
}

fn uuid(id: InoxNodeUuid) -> u32 {
    id.raw()
}

fn failed(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Inox2D failed".into())
}

impl Puppet {
    /// Reads an `.inp` or `.inx` file.
    pub fn load(path: &Path) -> Result<Puppet, Error> {
        let data = std::fs::read(path)?;
        catch_unwind(AssertUnwindSafe(|| Puppet::from_bytes(&data))).unwrap_or_else(|e| Err(Error::Other(failed(e))))
    }

    fn from_bytes(data: &[u8]) -> Result<Puppet, Error> {
        let mut model = inox2d::formats::inp::parse_inp(data)
            .map_err(|e| Error::Other(e.to_string().lines().map(str::trim).collect::<Vec<_>>().join(" ")))?;
        let mut textures: Vec<Texture> = inox2d::texture::decode_model_textures(model.textures.iter())
            .into_iter()
            // (Inochi2D keeps its textures premultiplied already: every semi-transparent pixel of
            // its sample puppets has its colour at most its alpha)
            .map(|t| Texture { width: t.width(), height: t.height(), rgba: t.pixels().to_vec() })
            .collect();
        // nijilive's Masks: drawn whole
        let white = textures.len();
        let mut white_used = false;
        let mapping = Mapping::new(&model);
        let puppet = &mut model.puppet;
        puppet.init_transforms();
        puppet.init_rendering();
        puppet.init_params();
        puppet.init_physics();
        // the parts, from Inox2D's shared buffers
        let mut parts = Vec::new();
        let mut part_of = HashMap::new();
        {
            let rc = puppet.render_ctx.as_ref().ok_or(Error::Invalid("no render context"))?;
            let b = &rc.vertex_buffers;
            for (node, ctx, source) in puppet.textured_meshes() {
                let tex = match source {
                    MeshTexture::Albedo(t) => t,
                    MeshTexture::Mask => {
                        white_used = true;
                        white
                    }
                    MeshTexture::DynamicComposite => DYNAMIC,
                };
                let off = ctx.vert_offset as usize;
                let uvs = b.uvs[off..off + ctx.vert_len].iter().map(|v| v.to_array()).collect();
                let indices = b.indices[ctx.index_offset as usize..ctx.index_offset as usize + ctx.index_len]
                    .iter()
                    .map(|&i| i - ctx.vert_offset)
                    .collect();
                part_of.insert(uuid(node.uuid), parts.len());
                parts.push(Part {
                    name: node.name.trim_end_matches('\0').to_string(),
                    uvs,
                    indices,
                    texture: tex,
                    vert_offset: off,
                });
            }
        }
        if white_used {
            textures.push(Texture { width: 1, height: 1, rgba: vec![255; 4] });
        }
        let n = parts.len();
        let mut p = Puppet {
            model,
            parts,
            part_of,
            textures,
            positions: vec![Vec::new(); n],
            ops: Vec::new(),
            mapping,
            time: 0.0,
            first: true,
            broken: false,
            bounds: (Vec2::ZERO, Vec2::ZERO),
            centre_x: 0.0,
        };
        p.step(&[], 0.0);
        p.record();
        p.bounds = p.visible_bounds();
        let (lo, hi) = p.bounds;
        let band = hi.y - (hi.y - lo.y) * 0.2;
        let (mut sum, mut k) = (0.0, 0usize);
        for v in p.positions.iter().flatten().filter(|v| v[1] >= band) {
            sum += v[0];
            k += 1;
        }
        p.centre_x = if k > 0 { sum / k as f32 } else { (lo.x + hi.x) * 0.5 };
        Ok(p)
    }

    pub fn name(&self) -> Option<&str> {
        self.model.puppet.meta.name.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

    pub fn textures(&self) -> &[Texture] {
        &self.textures
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Each part's vertices, posed (model units, y up).
    pub fn positions(&self) -> &[Vec<[f32; 2]>] {
        &self.positions
    }

    /// How to draw it, in order.
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// Parameters set by name for one frame, then physics, then posed.
    fn step(&mut self, values: &[(&str, usize, f32)], dt: f32) {
        let puppet = &mut self.model.puppet;
        puppet.begin_frame();
        if let Some(ctx) = puppet.param_ctx.as_mut() {
            for &(name, axis, v) in values {
                let mut cur = ctx.get(name).unwrap_or(Vec2::ZERO);
                cur[axis] = v;
                let _ = ctx.set(name, cur);
            }
        }
        // (the first frame's physics settle from rest)
        puppet.end_frame(if self.first { 0.0 } else { dt });
        self.first = false;
    }

    fn record(&mut self) {
        let Some(rc) = self.model.puppet.render_ctx.as_ref() else { return };
        let rec = Recorder {
            buffers: &rc.vertex_buffers,
            part_of: &self.part_of,
            parts: &self.parts,
            ops: Default::default(),
            positions: std::cell::RefCell::new(&mut self.positions),
            masks: Default::default(),
        };
        rec.draw(&self.model.puppet);
        self.ops = rec.ops.into_inner();
    }

    /// Poses the puppet for this frame from a face (mirrored first if wanted), `dt` seconds after
    /// the last.
    pub fn update(&mut self, face: &Face, dt: f32) {
        if self.broken {
            return;
        }
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let mapping = self.mapping.clone();
            let values = mapping.values(face, self.time);
            self.step(&values, dt);
            self.record();
        }));
        if let Err(e) = ok {
            tracing::warn!(error = failed(e), "Inochi2D puppet stopped");
            self.broken = true;
        }
    }

    /// The part of the puppet a framing shows, at an aspect (width / height): left, right, top,
    /// bottom (model units, y up).
    pub fn frame(&self, framing: crate::Framing, aspect: f32, zoom: f32) -> (f32, f32, f32, f32) {
        let (lo, hi) = self.bounds;
        let tall = hi.y - lo.y;
        let share = match framing {
            crate::Framing::Head => 0.3,
            crate::Framing::Bust => 0.45,
            crate::Framing::Half => 0.62,
            crate::Framing::Full => 1.0,
        };
        let h = tall * share * 1.04 / zoom.clamp(0.25, 4.0);
        let top = hi.y + tall * 0.02;
        let w = h * aspect.max(0.05);
        (self.centre_x - w * 0.5, self.centre_x + w * 0.5, top, top - h)
    }

    fn visible_bounds(&self) -> (Vec2, Vec2) {
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for op in &self.ops {
            if let Op::Part { part, as_mask: false, look } = op
                && look.opacity > 0.01
            {
                for v in &self.positions[*part] {
                    let p = Vec2::from(*v);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
        if lo.x > hi.x { (Vec2::splat(-1.0), Vec2::splat(1.0)) } else { (lo, hi) }
    }
}
