//! VRM models (0.x and 1.0) read into one shape: nodes, skinned primitives with morph targets,
//! MToon materials, decoded textures, the humanoid's bones, expressions, look-at and spring
//! bones. VRM 0.x models face -Z; they're turned around ([`Model::root`]) so every model faces
//! +Z, toward the camera.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

use crate::Error;
use crate::glb::Glb;

pub struct Model {
    pub meta: Meta,
    /// 0 or 1.
    pub version: u8,
    pub nodes: Vec<Node>,
    /// Nodes without a parent.
    pub roots: Vec<usize>,
    /// Applied above the roots: VRM 0.x's half turn, so the model faces +Z.
    pub root: Quat,
    pub skins: Vec<Skin>,
    pub primitives: Vec<Primitive>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    /// Humanoid bone (VRM 1.0's names: "hips", "head", "leftUpperArm", …) → node.
    pub humanoid: HashMap<String, usize>,
    /// Expression (VRM 1.0's preset names for presets: "aa", "blink", "happy", …) → binds.
    pub expressions: HashMap<String, Expression>,
    pub look_at: LookAt,
    pub joints: Vec<SpringJoint>,
    pub colliders: Vec<Collider>,
}

#[derive(Default, Clone, Debug)]
pub struct Meta {
    pub name: String,
    pub authors: Vec<String>,
    pub license: String,
}

pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Node {
    pub fn local(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

pub struct Skin {
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

pub struct Morph {
    pub positions: Vec<(u32, Vec3)>,
    pub normals: Vec<(u32, Vec3)>,
}

pub struct Primitive {
    /// The glTF mesh it's part of (expressions bind to meshes' morph targets).
    pub mesh: usize,
    pub node: usize,
    pub skin: Option<usize>,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub morphs: Vec<Morph>,
    pub material: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    /// Cut out below this alpha.
    Mask(f32),
    Blend,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    pub base_texture: Option<usize>,
    pub shade_color: [f32; 3],
    pub shade_texture: Option<usize>,
    pub shading_toony: f32,
    pub shading_shift: f32,
    pub emissive: [f32; 3],
    pub emissive_texture: Option<usize>,
    pub rim_color: [f32; 3],
    pub rim_fresnel_power: f32,
    pub rim_lift: f32,
    pub matcap_texture: Option<usize>,
    pub matcap_factor: [f32; 3],
    /// World units (metres); 0 for none.
    pub outline_width: f32,
    pub outline_color: [f32; 3],
    pub outline_lighting_mix: f32,
    pub alpha: AlphaMode,
    pub double_sided: bool,
    pub z_write: bool,
    pub queue_offset: i32,
    /// Drawn flat (KHR_materials_unlit without MToon).
    pub unlit: bool,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            name: String::new(),
            base_color: [1.0; 4],
            base_texture: None,
            shade_color: [1.0; 3],
            shade_texture: None,
            shading_toony: 0.9,
            shading_shift: 0.0,
            emissive: [0.0; 3],
            emissive_texture: None,
            rim_color: [0.0; 3],
            rim_fresnel_power: 5.0,
            rim_lift: 0.0,
            matcap_texture: None,
            matcap_factor: [1.0; 3],
            outline_width: 0.0,
            outline_color: [0.0; 3],
            outline_lighting_mix: 1.0,
            alpha: AlphaMode::Opaque,
            double_sided: false,
            z_write: true,
            queue_offset: 0,
            unlit: false,
        }
    }
}

/// RGBA8, straight alpha.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Smaller copies, each half the last, down to 1×1 (power-of-two images only).
    pub mips: Vec<Vec<u8>>,
}

impl Image {
    fn new(width: u32, height: u32, rgba: Vec<u8>) -> Image {
        let mips = mip_chain(width, height, &rgba);
        Image { width, height, rgba, mips }
    }
}

/// Smaller copies of an RGBA8 picture, each half the last, down to 1×1 (box filtered); none for
/// a picture whose sides aren't powers of two.
pub fn mip_chain(width: u32, height: u32, rgba: &[u8]) -> Vec<Vec<u8>> {
    let mut mips: Vec<Vec<u8>> = Vec::new();
    if !(width.is_power_of_two() && height.is_power_of_two()) {
        return mips;
    }
    let (mut w, mut h) = (width as usize, height as usize);
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let prev: &[u8] = mips.last().map_or(rgba, Vec::as_slice);
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |xx: usize, yy: usize| prev[((yy.min(h - 1)) * w + xx.min(w - 1)) * 4 + c] as u32;
                    let sum = at(x * 2, y * 2) + at(x * 2 + 1, y * 2) + at(x * 2, y * 2 + 1) + at(x * 2 + 1, y * 2 + 1);
                    next[(y * nw + x) * 4 + c] = (sum / 4) as u8;
                }
            }
        }
        mips.push(next);
        (w, h) = (nw, nh);
    }
    mips
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Override {
    #[default]
    None,
    Block,
    Blend,
}

#[derive(Clone, Debug, Default)]
pub struct Expression {
    /// (mesh, morph target, weight 0..1).
    pub binds: Vec<(usize, usize, f32)>,
    pub binary: bool,
    pub override_blink: Override,
    pub override_look_at: Override,
    pub override_mouth: Override,
}

#[derive(Clone, Copy, Debug)]
pub struct RangeMap {
    /// Degrees of gaze at which the output is full.
    pub input_max: f32,
    /// Degrees (bone look-at) or weight (expression look-at) at full.
    pub output_scale: f32,
}

impl RangeMap {
    pub fn map(&self, degrees: f32) -> f32 {
        (degrees.abs() / self.input_max.max(1e-3)).min(1.0) * self.output_scale
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LookAtKind {
    Bone,
    Expression,
}

#[derive(Clone, Copy, Debug)]
pub struct LookAt {
    pub kind: LookAtKind,
    pub offset: Vec3,
    pub horizontal_inner: RangeMap,
    pub horizontal_outer: RangeMap,
    pub vertical_down: RangeMap,
    pub vertical_up: RangeMap,
}

#[derive(Clone, Copy, Debug)]
pub enum Tail {
    Node(usize),
    /// A point in the joint's own space (a chain's last bone, which has no child).
    Local(Vec3),
}

#[derive(Clone, Debug)]
pub struct SpringJoint {
    pub node: usize,
    pub tail: Tail,
    pub hit_radius: f32,
    pub stiffness: f32,
    pub gravity_power: f32,
    pub gravity_dir: Vec3,
    pub drag: f32,
    /// Indices into [`Model::colliders`].
    pub colliders: Vec<usize>,
}

#[derive(Clone, Copy, Debug)]
pub enum Shape {
    Sphere { offset: Vec3, radius: f32 },
    Capsule { offset: Vec3, radius: f32, tail: Vec3 },
}

#[derive(Clone, Copy, Debug)]
pub struct Collider {
    pub node: usize,
    pub shape: Shape,
}

fn f(v: &Value, default: f32) -> f32 {
    v.as_f64().map_or(default, |x| x as f32)
}

fn vec3(v: &Value, default: [f32; 3]) -> Vec3 {
    match v.as_array() {
        Some(a) if a.len() >= 3 => Vec3::new(f(&a[0], default[0]), f(&a[1], default[1]), f(&a[2], default[2])),
        _ => match (v.get("x"), v.get("y"), v.get("z")) {
            (Some(x), Some(y), Some(z)) => Vec3::new(f(x, 0.0), f(y, 0.0), f(z, 0.0)),
            _ => Vec3::from(default),
        },
    }
}

fn rgb(v: &Value, default: [f32; 3]) -> [f32; 3] {
    match v.as_array() {
        Some(a) if a.len() >= 3 => [f(&a[0], default[0]), f(&a[1], default[1]), f(&a[2], default[2])],
        _ => match (v.get("r"), v.get("g"), v.get("b")) {
            (Some(r), Some(g), Some(b)) => [f(r, 0.0), f(g, 0.0), f(b, 0.0)],
            _ => default,
        },
    }
}

fn idx(v: &Value) -> Option<usize> {
    v.as_u64().map(|i| i as usize)
}

impl Model {
    /// Reads a `.vrm` (or a plain `.glb`, without the humanoid parts).
    pub fn load(bytes: &[u8]) -> Result<Model, Error> {
        let glb = Glb::parse(bytes)?;
        let j = &glb.json;
        let ext = &j["extensions"];
        let version = if ext.get("VRMC_vrm").is_some() {
            1
        } else if ext.get("VRM").is_some() {
            0
        } else {
            return Err(Error::Invalid("not a VRM model (no VRM or VRMC_vrm extension)"));
        };

        // nodes
        let empty = Vec::new();
        let jnodes = j["nodes"].as_array().unwrap_or(&empty);
        let mut nodes: Vec<Node> = jnodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let (translation, rotation, scale) = if let Some(m) = n["matrix"].as_array().filter(|m| m.len() == 16) {
                    let m: Vec<f32> = m.iter().map(|x| f(x, 0.0)).collect();
                    let (s, r, t) = Mat4::from_cols_slice(&m).to_scale_rotation_translation();
                    (t, r, s)
                } else {
                    let r = n["rotation"].as_array().filter(|r| r.len() == 4);
                    (
                        vec3(&n["translation"], [0.0; 3]),
                        r.map_or(Quat::IDENTITY, |r| Quat::from_xyzw(f(&r[0], 0.0), f(&r[1], 0.0), f(&r[2], 0.0), f(&r[3], 1.0)).normalize()),
                        vec3(&n["scale"], [1.0; 3]),
                    )
                };
                Node {
                    name: n["name"].as_str().map_or_else(|| format!("node{i}"), str::to_string),
                    parent: None,
                    children: n["children"].as_array().map(|c| c.iter().filter_map(idx).filter(|&c| c < jnodes.len()).collect()).unwrap_or_default(),
                    translation,
                    rotation,
                    scale,
                }
            })
            .collect();
        for i in 0..nodes.len() {
            for c in nodes[i].children.clone() {
                nodes[c].parent = Some(i);
            }
        }
        let roots: Vec<usize> = (0..nodes.len()).filter(|&i| nodes[i].parent.is_none()).collect();
        let root = if version == 0 { Quat::from_rotation_y(std::f32::consts::PI) } else { Quat::IDENTITY };

        // skins
        let mut skins = Vec::new();
        for s in j["skins"].as_array().unwrap_or(&empty) {
            let joints: Vec<usize> = s["joints"].as_array().map(|a| a.iter().filter_map(idx).collect()).unwrap_or_default();
            let inverse_bind = match idx(&s["inverseBindMatrices"]) {
                Some(a) => glb.floats(a)?.as_chunks::<16>().0.iter().map(Mat4::from_cols_array).collect(),
                None => vec![Mat4::IDENTITY; joints.len()],
            };
            skins.push(Skin { joints, inverse_bind });
        }

        // images and textures
        let mut images = Vec::new();
        for i in 0..j["images"].as_array().map_or(0, Vec::len) {
            let decoded = glb.image_bytes(i).ok().and_then(|b| image::load_from_memory(b).ok()).map(|im| im.to_rgba8());
            images.push(match decoded {
                Some(im) => Image::new(im.width(), im.height(), im.into_raw()),
                None => Image::new(1, 1, vec![255; 4]),
            });
        }
        let texture = |t: &Value| -> Option<usize> { idx(&t["index"]).and_then(|t| idx(&j["textures"][t]["source"])).filter(|&s| s < images.len()) };

        // materials
        let mut materials: Vec<Material> = j["materials"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .map(|m| {
                let pbr = &m["pbrMetallicRoughness"];
                let bc = pbr["baseColorFactor"].as_array();
                let mut mat = Material {
                    name: m["name"].as_str().unwrap_or("").to_string(),
                    base_color: bc.filter(|a| a.len() == 4).map_or([1.0; 4], |a| [f(&a[0], 1.0), f(&a[1], 1.0), f(&a[2], 1.0), f(&a[3], 1.0)]),
                    base_texture: texture(&pbr["baseColorTexture"]),
                    emissive: rgb(&m["emissiveFactor"], [0.0; 3]),
                    emissive_texture: texture(&m["emissiveTexture"]),
                    alpha: match m["alphaMode"].as_str() {
                        Some("MASK") => AlphaMode::Mask(f(&m["alphaCutoff"], 0.5)),
                        Some("BLEND") => AlphaMode::Blend,
                        _ => AlphaMode::Opaque,
                    },
                    double_sided: m["doubleSided"].as_bool().unwrap_or(false),
                    unlit: m["extensions"].get("KHR_materials_unlit").is_some(),
                    ..Material::default()
                };
                mat.shade_color = [mat.base_color[0] * 0.7, mat.base_color[1] * 0.7, mat.base_color[2] * 0.75];
                mat.shade_texture = mat.base_texture;
                if let Some(t) = m["extensions"].get("VRMC_materials_mtoon") {
                    mat.unlit = false;
                    mat.shade_color = rgb(&t["shadeColorFactor"], [1.0; 3]);
                    mat.shade_texture = texture(&t["shadeMultiplyTexture"]).or(mat.base_texture);
                    mat.shading_toony = f(&t["shadingToonyFactor"], 0.9);
                    mat.shading_shift = f(&t["shadingShiftFactor"], 0.0);
                    mat.rim_color = rgb(&t["parametricRimColorFactor"], [0.0; 3]);
                    mat.rim_fresnel_power = f(&t["parametricRimFresnelPowerFactor"], 5.0);
                    mat.rim_lift = f(&t["parametricRimLiftFactor"], 0.0);
                    mat.matcap_texture = texture(&t["matcapTexture"]);
                    mat.matcap_factor = rgb(&t["matcapFactor"], [1.0; 3]);
                    mat.outline_width = match t["outlineWidthMode"].as_str() {
                        Some("worldCoordinates") => f(&t["outlineWidthFactor"], 0.0),
                        // (screen widths are relative to the screen's height: about the same
                        // at a bust shot)
                        Some("screenCoordinates") => f(&t["outlineWidthFactor"], 0.0) * 0.5,
                        _ => 0.0,
                    };
                    mat.outline_color = rgb(&t["outlineColorFactor"], [0.0; 3]);
                    mat.outline_lighting_mix = f(&t["outlineLightingMixFactor"], 1.0);
                    mat.z_write = mat.alpha != AlphaMode::Blend || t["transparentWithZWrite"].as_bool().unwrap_or(false);
                    mat.queue_offset = t["renderQueueOffsetNumber"].as_i64().unwrap_or(0) as i32;
                } else {
                    mat.z_write = mat.alpha != AlphaMode::Blend;
                }
                mat
            })
            .collect();
        if materials.is_empty() {
            materials.push(Material::default());
        }
        if version == 0 {
            apply_v0_materials(&ext["VRM"], &mut materials, &|i: usize| j["textures"][i]["source"].as_u64().map(|s| s as usize).filter(|&s| s < images.len()));
        }

        // primitives: one per (node, primitive)
        let mut primitives = Vec::new();
        for (ni, n) in jnodes.iter().enumerate() {
            let Some(mi) = idx(&n["mesh"]) else { continue };
            let skin = idx(&n["skin"]).filter(|&s| s < skins.len());
            for p in j["meshes"][mi]["primitives"].as_array().unwrap_or(&empty) {
                if p["mode"].as_u64().unwrap_or(4) != 4 {
                    continue;
                }
                let a = &p["attributes"];
                let Some(pos) = idx(&a["POSITION"]) else { continue };
                let positions: Vec<Vec3> = glb.floats(pos)?.as_chunks::<3>().0.iter().map(|v| Vec3::from_array(*v)).collect();
                let count = positions.len();
                let normals = match idx(&a["NORMAL"]) {
                    Some(n) => glb.floats(n)?.as_chunks::<3>().0.iter().map(|v| Vec3::from_array(*v).normalize_or_zero()).collect(),
                    None => vec![Vec3::Z; count],
                };
                let uvs = match idx(&a["TEXCOORD_0"]) {
                    Some(t) => glb.floats(t)?.as_chunks::<2>().0.to_vec(),
                    None => vec![[0.0; 2]; count],
                };
                let (joints, weights) = match (skin, idx(&a["JOINTS_0"]), idx(&a["WEIGHTS_0"])) {
                    (Some(_), Some(jt), Some(w)) => (
                        glb.ints(jt)?.as_chunks::<4>().0.iter().map(|v| [v[0] as u16, v[1] as u16, v[2] as u16, v[3] as u16]).collect(),
                        glb.floats(w)?
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|v| {
                                let s = v[0] + v[1] + v[2] + v[3];
                                let s = if s > 0.0 { s } else { 1.0 };
                                [v[0] / s, v[1] / s, v[2] / s, v[3] / s]
                            })
                            .collect(),
                    ),
                    _ => (Vec::new(), Vec::new()),
                };
                let indices = match idx(&p["indices"]) {
                    Some(i) => glb.ints(i)?,
                    None => (0..count as u32).collect(),
                };
                let mut morphs = Vec::new();
                for t in p["targets"].as_array().unwrap_or(&empty) {
                    let sparse = |acc: Option<usize>| -> Result<Vec<(u32, Vec3)>, Error> {
                        Ok(match acc {
                            Some(acc) => glb
                                .floats(acc)?
                                .as_chunks::<3>()
                                .0
                                .iter()
                                .enumerate()
                                .map(|(i, v)| (i as u32, Vec3::from_array(*v)))
                                .filter(|(_, v)| v.length_squared() > 1e-14)
                                .collect(),
                            None => Vec::new(),
                        })
                    };
                    morphs.push(Morph { positions: sparse(idx(&t["POSITION"]))?, normals: sparse(idx(&t["NORMAL"]))? });
                }
                if joints.len() != count && skin.is_some() || normals.len() != count || uvs.len() != count {
                    continue;
                }
                primitives.push(Primitive {
                    mesh: mi,
                    node: ni,
                    skin: if joints.is_empty() { None } else { skin },
                    positions,
                    normals,
                    uvs,
                    joints,
                    weights,
                    indices: indices.into_iter().filter(|&i| (i as usize) < count).collect(),
                    morphs,
                    material: idx(&p["material"]).filter(|&m| m < materials.len()).unwrap_or(0),
                });
            }
        }

        let mut model = Model {
            meta: Meta::default(),
            version,
            nodes,
            roots,
            root,
            skins,
            primitives,
            materials,
            images,
            humanoid: HashMap::new(),
            expressions: HashMap::new(),
            look_at: LookAt {
                kind: LookAtKind::Bone,
                offset: Vec3::new(0.0, 0.06, 0.0),
                horizontal_inner: RangeMap { input_max: 90.0, output_scale: 10.0 },
                horizontal_outer: RangeMap { input_max: 90.0, output_scale: 10.0 },
                vertical_down: RangeMap { input_max: 90.0, output_scale: 10.0 },
                vertical_up: RangeMap { input_max: 90.0, output_scale: 10.0 },
            },
            joints: Vec::new(),
            colliders: Vec::new(),
        };
        if version == 1 {
            model.read_v1(&ext["VRMC_vrm"], &ext["VRMC_springBone"], jnodes);
        } else {
            model.read_v0(&ext["VRM"], jnodes);
        }
        if !model.humanoid.contains_key("head") || !model.humanoid.contains_key("hips") {
            return Err(Error::Invalid("the model has no humanoid head or hips"));
        }
        Ok(model)
    }

    fn read_v1(&mut self, vrm: &Value, springs: &Value, jnodes: &[Value]) {
        let meta = &vrm["meta"];
        self.meta = Meta {
            name: meta["name"].as_str().unwrap_or("").to_string(),
            authors: meta["authors"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            license: meta["licenseUrl"].as_str().unwrap_or("").to_string(),
        };
        if let Some(bones) = vrm["humanoid"]["humanBones"].as_object() {
            for (name, b) in bones {
                if let Some(n) = idx(&b["node"]).filter(|&n| n < self.nodes.len()) {
                    self.humanoid.insert(name.clone(), n);
                }
            }
        }
        let node_mesh = |n: usize| jnodes.get(n).and_then(|n| idx(&n["mesh"]));
        let ov = |v: &Value| match v.as_str() {
            Some("block") => Override::Block,
            Some("blend") => Override::Blend,
            _ => Override::None,
        };
        for group in ["preset", "custom"] {
            if let Some(exprs) = vrm["expressions"][group].as_object() {
                for (name, e) in exprs {
                    let binds = e["morphTargetBinds"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|b| Some((node_mesh(idx(&b["node"])?)?, idx(&b["index"])?, f(&b["weight"], 1.0)))).collect())
                        .unwrap_or_default();
                    self.expressions.insert(
                        name.clone(),
                        Expression {
                            binds,
                            binary: e["isBinary"].as_bool().unwrap_or(false),
                            override_blink: ov(&e["overrideBlink"]),
                            override_look_at: ov(&e["overrideLookAt"]),
                            override_mouth: ov(&e["overrideMouth"]),
                        },
                    );
                }
            }
        }
        let la = &vrm["lookAt"];
        let range = |v: &Value, d: RangeMap| RangeMap { input_max: f(&v["inputMaxValue"], d.input_max), output_scale: f(&v["outputScale"], d.output_scale) };
        let kind = if la["type"].as_str() == Some("expression") { LookAtKind::Expression } else { LookAtKind::Bone };
        let d = if kind == LookAtKind::Bone { RangeMap { input_max: 90.0, output_scale: 10.0 } } else { RangeMap { input_max: 90.0, output_scale: 1.0 } };
        self.look_at = LookAt {
            kind,
            offset: vec3(&la["offsetFromHeadBone"], [0.0, 0.06, 0.0]),
            horizontal_inner: range(&la["rangeMapHorizontalInner"], d),
            horizontal_outer: range(&la["rangeMapHorizontalOuter"], d),
            vertical_down: range(&la["rangeMapVerticalDown"], d),
            vertical_up: range(&la["rangeMapVerticalUp"], d),
        };

        // spring bones
        let mut collider_index = HashMap::new();
        for (i, c) in springs["colliders"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
            let Some(node) = idx(&c["node"]).filter(|&n| n < self.nodes.len()) else { continue };
            let s = &c["shape"];
            let shape = if let Some(sp) = s.get("sphere") {
                Shape::Sphere { offset: vec3(&sp["offset"], [0.0; 3]), radius: f(&sp["radius"], 0.0) }
            } else if let Some(cp) = s.get("capsule") {
                Shape::Capsule { offset: vec3(&cp["offset"], [0.0; 3]), radius: f(&cp["radius"], 0.0), tail: vec3(&cp["tail"], [0.0; 3]) }
            } else {
                continue;
            };
            collider_index.insert(i, self.colliders.len());
            self.colliders.push(Collider { node, shape });
        }
        let groups: Vec<Vec<usize>> = springs["colliderGroups"]
            .as_array()
            .map(|g| g.iter().map(|g| g["colliders"].as_array().map(|c| c.iter().filter_map(|c| idx(c).and_then(|c| collider_index.get(&c).copied())).collect()).unwrap_or_default()).collect())
            .unwrap_or_default();
        for spring in springs["springs"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let colliders: Vec<usize> = spring["colliderGroups"].as_array().map(|g| g.iter().filter_map(idx).filter_map(|g| groups.get(g)).flatten().copied().collect()).unwrap_or_default();
            let joints: Vec<&Value> = spring["joints"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
            for (k, jt) in joints.iter().enumerate() {
                let Some(node) = idx(&jt["node"]).filter(|&n| n < self.nodes.len()) else { continue };
                let tail = match joints.get(k + 1).and_then(|n| idx(&n["node"])).filter(|&n| n < self.nodes.len()) {
                    Some(next) => Tail::Node(next),
                    // (a chain's last joint is only its predecessor's tail)
                    None if k > 0 => continue,
                    None => Tail::Local(self.virtual_tail(node)),
                };
                self.joints.push(SpringJoint {
                    node,
                    tail,
                    hit_radius: f(&jt["hitRadius"], 0.0),
                    stiffness: f(&jt["stiffness"], 1.0),
                    gravity_power: f(&jt["gravityPower"], 0.0),
                    gravity_dir: vec3(&jt["gravityDir"], [0.0, -1.0, 0.0]),
                    drag: f(&jt["dragForce"], 0.5),
                    colliders: colliders.clone(),
                });
            }
        }
        self.order_joints();
    }

    fn read_v0(&mut self, vrm: &Value, jnodes: &[Value]) {
        let meta = &vrm["meta"];
        self.meta = Meta {
            name: meta["title"].as_str().unwrap_or("").to_string(),
            authors: meta["author"].as_str().map(|a| vec![a.to_string()]).unwrap_or_default(),
            license: meta["licenseName"].as_str().unwrap_or("").to_string(),
        };
        for b in vrm["humanoid"]["humanBones"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            if let (Some(name), Some(n)) = (b["bone"].as_str(), idx(&b["node"]).filter(|&n| n < self.nodes.len())) {
                self.humanoid.insert(name.to_string(), n);
            }
        }
        let _ = jnodes;
        for g in vrm["blendShapeMaster"]["blendShapeGroups"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let preset = g["presetName"].as_str().unwrap_or("unknown");
            let name = match preset {
                "a" => "aa",
                "i" => "ih",
                "u" => "ou",
                "e" => "ee",
                "o" => "oh",
                "blink" => "blink",
                "blink_l" => "blinkLeft",
                "blink_r" => "blinkRight",
                "joy" => "happy",
                "angry" => "angry",
                "sorrow" => "sad",
                "fun" => "relaxed",
                "surprised" => "surprised",
                "lookup" => "lookUp",
                "lookdown" => "lookDown",
                "lookleft" => "lookLeft",
                "lookright" => "lookRight",
                "neutral" => "neutral",
                _ => g["name"].as_str().unwrap_or("custom"),
            };
            let binds = g["binds"]
                .as_array()
                .map(|a| a.iter().filter_map(|b| Some((idx(&b["mesh"])?, idx(&b["index"])?, f(&b["weight"], 100.0) / 100.0))).collect())
                .unwrap_or_default();
            let overrides = matches!(name, "happy" | "angry" | "sad" | "relaxed" | "surprised");
            self.expressions.insert(
                name.to_string(),
                Expression {
                    binds,
                    binary: g["isBinary"].as_bool().unwrap_or(false),
                    override_blink: if overrides { Override::Blend } else { Override::None },
                    override_look_at: Override::None,
                    override_mouth: if overrides { Override::Blend } else { Override::None },
                },
            );
        }
        let fp = &vrm["firstPerson"];
        let kind = if fp["lookAtTypeName"].as_str() == Some("BlendShape") { LookAtKind::Expression } else { LookAtKind::Bone };
        let range = |v: &Value| RangeMap { input_max: f(&v["xRange"], 90.0), output_scale: f(&v["yRange"], if kind == LookAtKind::Bone { 10.0 } else { 1.0 }) };
        let o = vec3(&fp["firstPersonBoneOffset"], [0.0, 0.06, 0.0]);
        self.look_at = LookAt {
            kind,
            offset: Vec3::new(-o.x, o.y, -o.z),
            horizontal_inner: range(&fp["lookAtHorizontalInner"]),
            horizontal_outer: range(&fp["lookAtHorizontalOuter"]),
            vertical_down: range(&fp["lookAtVerticalDown"]),
            vertical_up: range(&fp["lookAtVerticalUp"]),
        };

        // spring bones: every node under each group's roots, its first child its tail
        let sa = &vrm["secondaryAnimation"];
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for g in sa["colliderGroups"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let mut list = Vec::new();
            if let Some(node) = idx(&g["node"]).filter(|&n| n < self.nodes.len()) {
                for c in g["colliders"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    let o = vec3(&c["offset"], [0.0; 3]);
                    list.push(self.colliders.len());
                    self.colliders.push(Collider { node, shape: Shape::Sphere { offset: Vec3::new(-o.x, o.y, -o.z), radius: f(&c["radius"], 0.0) } });
                }
            }
            groups.push(list);
        }
        for g in sa["boneGroups"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let colliders: Vec<usize> = g["colliderGroups"].as_array().map(|a| a.iter().filter_map(idx).filter_map(|i| groups.get(i)).flatten().copied().collect()).unwrap_or_default();
            let gd = vec3(&g["gravityDir"], [0.0, -1.0, 0.0]);
            let params = (
                f(&g["hitRadius"], 0.02),
                f(&g["stiffiness"], 1.0),
                f(&g["gravityPower"], 0.0),
                Vec3::new(-gd.x, gd.y, -gd.z),
                f(&g["dragForce"], 0.4),
            );
            let mut stack: Vec<usize> = g["bones"].as_array().map(|a| a.iter().filter_map(idx).filter(|&n| n < self.nodes.len()).collect()).unwrap_or_default();
            while let Some(node) = stack.pop() {
                let tail = match self.nodes[node].children.first() {
                    Some(&c) => Tail::Node(c),
                    None => Tail::Local(self.virtual_tail(node)),
                };
                self.joints.push(SpringJoint {
                    node,
                    tail,
                    hit_radius: params.0,
                    stiffness: params.1,
                    gravity_power: params.2,
                    gravity_dir: params.3,
                    drag: params.4,
                    colliders: colliders.clone(),
                });
                stack.extend(self.nodes[node].children.iter().copied());
            }
        }
        self.order_joints();
    }

    /// A chain end's tail: 7 cm on, the way its bone points.
    fn virtual_tail(&self, node: usize) -> Vec3 {
        let t = self.nodes[node].translation;
        let dir = if t.length_squared() > 1e-10 { t.normalize() } else { Vec3::Y };
        // (in the node's own space, the bone continues the way it came from its parent)
        let inv = self.nodes[node].rotation.inverse();
        inv * dir * 0.07 / self.nodes[node].scale.max(Vec3::splat(1e-4))
    }

    /// Joints parents first, so one pass simulates a chain from its root.
    fn order_joints(&mut self) {
        let depth = |mut n: usize| {
            let mut d = 0;
            while let Some(p) = self.nodes[n].parent {
                n = p;
                d += 1;
            }
            d
        };
        let depths: Vec<usize> = (0..self.nodes.len()).map(depth).collect();
        self.joints.sort_by_key(|j| depths[j.node]);
        let mut seen = std::collections::HashSet::new();
        self.joints.retain(|j| seen.insert(j.node));
    }

    /// Rest-pose world matrices (with [`Model::root`]).
    pub fn rest_world(&self) -> Vec<Mat4> {
        let locals: Vec<Mat4> = self.nodes.iter().map(Node::local).collect();
        self.world(&locals)
    }

    /// World matrices from local ones.
    pub fn world(&self, locals: &[Mat4]) -> Vec<Mat4> {
        let mut out = vec![Mat4::IDENTITY; self.nodes.len()];
        let root = Mat4::from_quat(self.root);
        let mut stack: Vec<(usize, Mat4)> = self.roots.iter().map(|&r| (r, root)).collect();
        while let Some((n, parent)) = stack.pop() {
            let w = parent * locals[n];
            out[n] = w;
            for &c in &self.nodes[n].children {
                stack.push((c, w));
            }
        }
        out
    }
}

/// VRM 0.x keeps MToon's settings beside the glTF materials, as Unity shader properties.
fn apply_v0_materials(vrm: &Value, materials: &mut [Material], texture: &dyn Fn(usize) -> Option<usize>) {
    let Some(props) = vrm["materialProperties"].as_array() else { return };
    for (m, p) in materials.iter_mut().zip(props) {
        if !p["shader"].as_str().unwrap_or("").contains("MToon") {
            continue;
        }
        let fl = &p["floatProperties"];
        let ve = &p["vectorProperties"];
        let tx = &p["textureProperties"];
        let v4 = |k: &str, d: [f32; 4]| -> [f32; 4] {
            ve[k].as_array().filter(|a| a.len() >= 4).map_or(d, |a| [f(&a[0], d[0]), f(&a[1], d[1]), f(&a[2], d[2]), f(&a[3], d[3])])
        };
        let tex = |k: &str| tx[k].as_u64().and_then(|t| texture(t as usize));
        m.unlit = false;
        m.base_color = v4("_Color", m.base_color);
        m.base_texture = tex("_MainTex").or(m.base_texture);
        let s = v4("_ShadeColor", [0.97, 0.81, 0.86, 1.0]);
        m.shade_color = [s[0], s[1], s[2]];
        m.shade_texture = tex("_ShadeTexture").or(m.base_texture);
        // (0.x's toony and shift, to 1.0's: three-vrm's conversion)
        let toony = f(&fl["_ShadeToony"], 0.9);
        let shift = f(&fl["_ShadeShift"], 0.0);
        m.shading_toony = ((toony - shift) / (2.0 - (1.0 - toony) - shift).max(1e-4) * 2.0 - 1.0).clamp(0.0, 1.0);
        m.shading_shift = -((1.0 - toony) + shift);
        let e = v4("_EmissionColor", [0.0; 4]);
        m.emissive = [e[0], e[1], e[2]];
        m.emissive_texture = tex("_EmissionMap");
        let r = v4("_RimColor", [0.0; 4]);
        m.rim_color = [r[0], r[1], r[2]];
        m.rim_fresnel_power = f(&fl["_RimFresnelPower"], 1.0);
        m.rim_lift = f(&fl["_RimLift"], 0.0);
        m.matcap_texture = tex("_SphereAdd");
        m.outline_width = match fl["_OutlineWidthMode"].as_f64().unwrap_or(0.0) as i32 {
            1 => f(&fl["_OutlineWidth"], 0.0) * 0.01,
            2 => f(&fl["_OutlineWidth"], 0.0) * 0.005,
            _ => 0.0,
        };
        let o = v4("_OutlineColor", [0.0, 0.0, 0.0, 1.0]);
        m.outline_color = [o[0], o[1], o[2]];
        m.outline_lighting_mix = f(&fl["_OutlineLightingMix"], 1.0);
        m.alpha = match fl["_BlendMode"].as_f64().unwrap_or(0.0) as i32 {
            1 => AlphaMode::Mask(f(&fl["_Cutoff"], 0.5)),
            2 | 3 => AlphaMode::Blend,
            _ => AlphaMode::Opaque,
        };
        m.z_write = fl["_BlendMode"].as_f64().unwrap_or(0.0) as i32 != 2;
        m.double_sided = fl["_CullMode"].as_f64().unwrap_or(2.0) as i32 == 0;
        m.queue_offset = p["renderQueue"].as_i64().map_or(0, |q| (q - if m.alpha == AlphaMode::Blend { 3000 } else { 2000 }) as i32);
    }
}
