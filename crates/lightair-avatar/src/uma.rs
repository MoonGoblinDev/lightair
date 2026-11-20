//! Umamo puppets (`.uma`, the native format of Umamo, a Cubism-compatible 2D puppet editor:
//! github.com/umamoorg/umamo). A `.uma` is a ZIP: a manifest, the puppet graph as JSON
//! (`model/puppet.json`) with its bulk arrays in a little-endian buffer (`model/buffers.bin`), and
//! the textures (`textures/index.json` and PNG entries). A [`Face`] sets the puppet's standard
//! parameters each frame, then its deformers are evaluated and its meshes posed, ready to draw as
//! textured triangles with clip masks, opacity, multiply and screen colours and blend modes.
//!
//! Puppets are Cubism-style, so the face drives Cubism's standard parameter ids, the way
//! [`crate::live2d`] does and with its sign conventions: `ParamAngleX` > 0 turns the face toward
//! the viewer's right, `ParamAngleY` > 0 looks up, `ParamAngleZ` > 0 tilts counter-clockwise as
//! seen, `ParamEyeLOpen` is the model's own left eye, `ParamEyeBallX` > 0 looks toward the
//! viewer's right and `ParamEyeBallY` > 0 up.
//!
//! The evaluation is a port of Umamo's own (Kotlin, GPL-3.0, as Lightair's desktop is), at commit
//! f7ef026 of its master:
//! - reading: `module/format/.../format/uma/` (`Uma.kt`, `UmaManifest.kt`, `UmaBuffers.kt`,
//!   `puppet/UmaPuppet.kt`, `puppet/UmaPuppetGeometry.kt`, `puppet/UmaPuppetEnums.kt`,
//!   `textures/UmaTextures.kt`) and `module/interop/.../interop/uma/UmaPuppetImport.kt`;
//! - keyform grids, channels and blend shapes: `module/runtime/.../runtime/eval/`
//!   (`KeyformGridSampling.kt`, `ChannelSampling.kt`, `BlendShapeSampling.kt`);
//! - deformers and meshes: `module/render/.../render/eval/` (`DeformerCascade.kt`,
//!   `DeformTransforms.kt`, `KeyformGridEval.kt`, `BlendShapeEval.kt`, `DeformPrepare.kt`,
//!   `RenderOrder.kt`), the render tree and visibility from `runtime/model/RenderRootDerive.kt` and
//!   `PuppetModel.kt`, the atlas mapping from `runtime/model/AtlasPlacement.kt`, and the colour
//!   maths from `render/glsl/PuppetShaders.kt`.
//!
//! Not supported: physics, pose groups, motions and expressions (a `.uma` carries none of them);
//! Cubism 5.3's advanced blend modes beyond add and multiply (drawn as [`Blend::Normal`]) and its
//! alpha blend modes; an isolated part's own offscreen layer (its opacity and colours are folded
//! into its drawables, its blend mode and masks are not applied). Parts marked as sketches (guide
//! images) and hidden parts or drawables aren't drawn.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use glam::Vec2;
use serde_json::{Map, Value};

use crate::Error;
use crate::rig::Face;

type Json = Map<String, Value>;

/// Umamo's default draw order (Cubism's).
const DEFAULT_DRAW_ORDER: f32 = 500.0;
/// The largest archive entry read.
const MAX_ENTRY: u64 = 1 << 30;
/// Key snapping and the smallest span between keys (Umamo's, its C++ runtime's).
const EPS_KEY: f32 = 0.001;
const EPS_SPAN: f32 = 0.0015;
/// The most cells a grid blends at once.
const MAX_CORNERS: usize = 16;
/// The float pi Umamo's runtime uses.
const PI_F: f32 = std::f32::consts::PI;

/// A texture, RGBA8 with its colour premultiplied by alpha (the file's PNGs are straight alpha;
/// they're premultiplied when read). Row 0 is the image's top.
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// How a drawable is laid over what's under it (premultiplied colour): `Normal` is
/// `src + dst·(1 − src.a)`, `Additive` is `src + dst`, `Multiply` is `src·dst + dst·(1 − src.a)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Additive,
    Multiply,
}

/// An art mesh: its texture coordinates and triangles, and how it's posed and drawn this frame.
///
/// Drawn: the texel `t` (premultiplied), its straight colour `c = t.rgb / t.a` tinted
/// `c·multiply`, then screened `c + screen − c·screen`, at alpha `t.a · opacity`, times the mask's
/// coverage (`1 − coverage` when `inverted_mask`) when it has masks.
pub struct Drawable {
    pub name: String,
    /// Into [`Puppet::textures`], `v` = 0 at the texture's top row.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub texture: usize,
    /// Posed this frame: model units, y up.
    pub positions: Vec<[f32; 2]>,
    pub opacity: f32,
    pub multiply: [f32; 3],
    pub screen: [f32; 3],
    pub blend: Blend,
    /// The drawables clipping this one: it's drawn only where they are (their texture's alpha, at
    /// full opacity, normal blend, unmasked, each where it's [`posed`](Drawable::posed)).
    pub masks: Vec<usize>,
    pub inverted_mask: bool,
    /// Whether it has geometry this frame (a keyform grid or deformer out of range hides it).
    pub posed: bool,
}

/// A puppet's parameter, and its range.
#[derive(Clone, Debug)]
pub struct Parameter {
    pub id: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

pub struct Puppet {
    textures: Vec<Texture>,
    drawables: Vec<Drawable>,
    defs: Vec<DrawableDef>,
    deformers: Vec<DeformerDef>,
    /// Parents before children; deformers in a parent cycle are left out (never drawn).
    deformer_order: Vec<usize>,
    parts: Vec<PartDef>,
    /// The render tree's top level, back to front before sorting.
    tree: Vec<Node>,
    glues: Vec<GlueDef>,
    params: Vec<Parameter>,
    values: Vec<f32>,
    driven: FaceParams,
    order: Vec<usize>,
    /// Each drawable's draw order this frame.
    sort_keys: Vec<f32>,
    /// World (canvas pixels, y up) to model units.
    origin: Vec2,
    pixels_per_unit: f32,
    time: f32,
    /// The model at rest: its lowest and highest corner (model units, y up).
    pub bounds: (Vec2, Vec2),
    /// Across the middle of its head (the top fifth of it), where a frame centres.
    pub centre_x: f32,
}

impl Puppet {
    /// Reads a `.uma`.
    pub fn load(path: &Path) -> Result<Puppet, Error> {
        let file = File::open(path)?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Other(format!("not a .uma (zip): {e}")))?;
        let manifest = entry(&mut zip, "manifest.json")?.ok_or(Error::Invalid("not a .uma: it has no manifest"))?;
        let manifest: Value =
            serde_json::from_slice(&manifest).map_err(|e| Error::Other(format!("manifest.json: {e}")))?;
        if manifest.get("format").and_then(Value::as_str) != Some("uma") {
            return Err(Error::Invalid("not a .uma: its manifest isn't one"));
        }
        if manifest.get("containerVersion").and_then(Value::as_i64).is_none_or(|v| v > 1) {
            return Err(Error::Invalid("this .uma is from a newer Umamo"));
        }
        let entry_path = |kind: &str, default: &str| -> Result<String, Error> {
            let found = manifest
                .get("entries")
                .and_then(Value::as_array)
                .and_then(|entries| entries.iter().find(|e| e.get("kind").and_then(Value::as_str) == Some(kind)));
            match found {
                Some(e) => {
                    if e.get("minVersion").and_then(Value::as_i64).is_some_and(|v| v > 1) {
                        return Err(Error::Other(format!("this .uma's {kind} entry is from a newer Umamo")));
                    }
                    Ok(e.get("path").and_then(Value::as_str).unwrap_or(default).to_string())
                }
                None => Ok(default.to_string()),
            }
        };
        let puppet_path = entry_path("puppet", "model/puppet.json")?;
        let textures_path = entry_path("textures", "textures/index.json")?;
        let puppet = entry(&mut zip, &puppet_path)?.ok_or(Error::Invalid("this .uma has no puppet"))?;
        let puppet: Value = serde_json::from_slice(&puppet).map_err(|e| Error::Other(format!("{puppet_path}: {e}")))?;
        let puppet = puppet.as_object().ok_or(Error::Invalid("the puppet entry isn't an object"))?;
        let index: Option<Value> = match entry(&mut zip, &textures_path)? {
            Some(b) => Some(serde_json::from_slice(&b).map_err(|e| Error::Other(format!("{textures_path}: {e}")))?),
            None => None,
        };

        let mut bufs = Buffers { zip: &mut zip, cache: HashMap::new() };
        let mut loader = Loader { bufs: &mut bufs, param_index: HashMap::new(), defaults: Vec::new() };

        // parameters
        let mut params = Vec::new();
        for p in arr(puppet.get("parameters")) {
            let Some(id) = p.get("id").and_then(Value::as_str) else { continue };
            let min = num(p.get("min")).unwrap_or(0.0);
            let max = num(p.get("max")).unwrap_or(0.0).max(min);
            let default = num(p.get("default")).unwrap_or(0.0).clamp(min, max);
            loader.param_index.entry(id.to_string()).or_insert(params.len());
            params.push(Parameter { id: id.to_string(), min, max, default });
        }
        loader.defaults = params.iter().map(|p| p.default).collect();

        // deformers
        let deformer_json = arr(puppet.get("deformers"));
        let deformer_index: HashMap<&str, usize> =
            deformer_json.iter().enumerate().filter_map(|(i, d)| Some((d.get("id")?.as_str()?, i))).rev().collect();
        let mut deformers = Vec::with_capacity(deformer_json.len());
        for d in deformer_json {
            let d = d.as_object().ok_or(Error::Invalid("a deformer isn't an object"))?;
            let parent = match d.get("parent").and_then(Value::as_str) {
                None => Parent::Root,
                Some(id) => deformer_index.get(id).map_or(Parent::Missing, |&i| Parent::Index(i)),
            };
            deformers.push(loader.deformer(d, parent)?);
        }
        let deformer_order = deformer_order(&deformers);

        // drawables
        let drawable_json = arr(puppet.get("drawables"));
        let drawable_index: HashMap<&str, usize> =
            drawable_json.iter().enumerate().filter_map(|(i, d)| Some((d.get("id")?.as_str()?, i))).rev().collect();
        let mut defs = Vec::with_capacity(drawable_json.len());
        let mut drawables = Vec::with_capacity(drawable_json.len());
        for d in drawable_json {
            let d = d.as_object().ok_or(Error::Invalid("a drawable isn't an object"))?;
            let parent = match d.get("parentDeformer").and_then(Value::as_str) {
                None => Parent::Root,
                // a drawable whose deformer doesn't exist draws nothing, as in Umamo
                Some(id) => deformer_index.get(id).map_or(Parent::Missing, |&i| Parent::Index(i)),
            };
            let (def, drawable) = loader.drawable(d, parent, &drawable_index)?;
            defs.push(def);
            drawables.push(drawable);
        }

        // parts and the render tree
        let part_json = arr(puppet.get("parts"));
        let part_index: HashMap<&str, usize> =
            part_json.iter().enumerate().filter_map(|(i, p)| Some((p.get("id")?.as_str()?, i))).rev().collect();
        let mut parts = Vec::with_capacity(part_json.len());
        for p in part_json {
            let p = p.as_object().ok_or(Error::Invalid("a part isn't an object"))?;
            parts.push(loader.part(p, &part_index, &drawable_index)?);
        }
        let root_children: Vec<Child> =
            arr(puppet.get("rootChildren")).iter().filter_map(|c| child_of(c, &part_index, &drawable_index)).collect();
        let tree = render_tree(&root_children, &parts, drawables.len());
        // what's shown: reached through visible parts (not sketches), and visible itself
        let mut shown = vec![false; drawables.len()];
        let mut entered = HashSet::new();
        mark_shown(&root_children, &parts, &defs, true, &mut entered, &mut shown);
        for (def, s) in defs.iter_mut().zip(shown) {
            def.shown = s;
        }

        // glues
        let mut glues = Vec::new();
        for g in arr(puppet.get("glues")) {
            let g = g.as_object().ok_or(Error::Invalid("a glue isn't an object"))?;
            let (Some(&a), Some(&b)) = (
                g.get("meshA").and_then(Value::as_str).and_then(|id| drawable_index.get(id)),
                g.get("meshB").and_then(Value::as_str).and_then(|id| drawable_index.get(id)),
            ) else {
                continue;
            };
            let pairs = g.get("pairs").and_then(Value::as_object).ok_or(Error::Invalid("a glue has no pairs"))?;
            let ia = loader.bufs.ints(pairs.get("indicesA"))?;
            let ib = loader.bufs.ints(pairs.get("indicesB"))?;
            let wa = loader.bufs.floats(pairs.get("weightsA"))?;
            let wb = loader.bufs.floats(pairs.get("weightsB"))?;
            let n = ia.len().min(ib.len()).min(wa.len()).min(wb.len());
            let pairs = (0..n)
                .filter(|&k| ia[k] >= 0 && ib[k] >= 0)
                .map(|k| (ia[k] as usize, ib[k] as usize, wa[k], wb[k]))
                .collect();
            glues.push(GlueDef {
                a,
                b,
                pairs,
                intensity: num(g.get("intensity")).unwrap_or(1.0),
                channels: loader.channels(g.get("channels"))?,
            });
        }

        // textures
        let textures = load_textures(loader.bufs.zip, index.as_ref(), &mut drawables, drawable_json)?;

        let canvas_width = num(puppet.get("canvasWidth")).unwrap_or(0.0);
        let pixels_per_unit = match num(puppet.get("pixelsPerUnit")) {
            Some(p) if p != 0.0 && p.is_finite() => p,
            _ if canvas_width > 0.0 => canvas_width,
            _ => 1.0,
        };
        let origin =
            Vec2::new(num(puppet.get("worldOriginX")).unwrap_or(0.0), num(puppet.get("worldOriginZ")).unwrap_or(0.0));
        let sort_keys = vec![DEFAULT_DRAW_ORDER; drawables.len()];
        let driven = FaceParams::find(&loader.param_index);
        let values = params.iter().map(|p| p.default).collect();

        let mut puppet = Puppet {
            textures,
            drawables,
            defs,
            deformers,
            deformer_order,
            parts,
            tree,
            glues,
            params,
            values,
            driven,
            order: Vec::new(),
            sort_keys,
            origin,
            pixels_per_unit,
            time: 0.0,
            bounds: (Vec2::splat(-1.0), Vec2::splat(1.0)),
            centre_x: 0.0,
        };
        puppet.pose();
        let bounds = puppet.visible_bounds();
        let band = bounds.1.y - (bounds.1.y - bounds.0.y) * 0.2;
        let (mut sum, mut n) = (0.0, 0usize);
        for &i in &puppet.order {
            let d = &puppet.drawables[i];
            if d.opacity <= 0.01 {
                continue;
            }
            for p in d.positions.iter().filter(|p| p[1] >= band) {
                sum += p[0];
                n += 1;
            }
        }
        puppet.bounds = bounds;
        puppet.centre_x = if n > 0 { sum / n as f32 } else { (bounds.0.x + bounds.1.x) * 0.5 };
        Ok(puppet)
    }

    pub fn textures(&self) -> &[Texture] {
        &self.textures
    }

    pub fn drawables(&self) -> &[Drawable] {
        &self.drawables
    }

    /// The drawables drawn this frame, back to front (shown, posed, any opacity).
    pub fn draw_order(&self) -> Vec<usize> {
        self.order.clone()
    }

    pub fn parameters(&self) -> &[Parameter] {
        &self.params
    }

    /// The standard parameters a face drives, those the puppet has.
    pub fn driven(&self) -> Vec<&str> {
        let p = &self.driven;
        let mut ids: Vec<usize> = Vec::new();
        ids.extend(p.angle.iter().chain(&p.body).chain(&p.eye_ball).chain(&p.eye_smile).chain(&p.brow).flatten());
        ids.extend(p.eye_open.iter().flatten());
        ids.extend(p.mouth_open.iter());
        ids.extend([p.mouth_form, p.breath].iter().chain(&p.vowels).flatten());
        ids.iter().filter_map(|&i| self.params.get(i)).map(|p| p.id.as_str()).collect()
    }

    /// The part of the model a framing shows, at an aspect (width / height): left, right, top,
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

    /// Poses the puppet for this frame: the face (mirrored first if wanted), `dt` seconds after
    /// the last.
    pub fn update(&mut self, face: &Face, dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
        self.time += dt;
        for (v, p) in self.values.iter_mut().zip(&self.params) {
            *v = p.default;
        }
        let p = self.driven.clone();
        let (yaw, pitch, roll) = face.angles();
        let params = &self.params;
        let values = &mut self.values;
        let mut set = |i: Option<usize>, v: f32| {
            if let (Some(i), true) = (i, v.is_finite())
                && let Some(param) = params.get(i)
            {
                values[i] = v.clamp(param.min, param.max);
            }
        };
        set(p.angle[0], yaw.clamp(-30.0, 30.0));
        set(p.angle[1], pitch.clamp(-30.0, 30.0));
        set(p.angle[2], roll.clamp(-30.0, 30.0));
        // the body follows the head a little
        set(p.body[0], (yaw * 0.3).clamp(-10.0, 10.0));
        set(p.body[1], (pitch * 0.3).clamp(-10.0, 10.0));
        set(p.body[2], (roll * 0.3).clamp(-10.0, 10.0));
        for side in 0..2 {
            for &i in &p.eye_open[side] {
                set(Some(i), face.eye_open[side]);
            }
        }
        set(p.eye_ball[0], face.gaze.x.clamp(-1.0, 1.0));
        set(p.eye_ball[1], face.gaze.y.clamp(-1.0, 1.0));
        set(p.brow[0], face.brow[0]);
        set(p.brow[1], face.brow[1]);
        if p.vowels.iter().any(Option::is_some) {
            for (i, v) in p.vowels.iter().zip(face.vowels()) {
                set(*i, v);
            }
        }
        for &i in &p.mouth_open {
            set(Some(i), face.mouth_open);
        }
        set(p.mouth_form, face.mouth_wide.clamp(-1.0, 1.0));
        // the eyes smile with a clear smile (not a little "ee")
        let smile = 0.8 * ((face.mouth_wide - 0.25) / 0.75).clamp(0.0, 1.0);
        set(p.eye_smile[0], smile);
        set(p.eye_smile[1], smile);
        set(p.breath, 0.5 + 0.5 * (self.time * std::f32::consts::TAU / 3.5).sin());
        self.pose();
    }

    /// Sets parameters by id (after [`update`](Puppet::update), then [`pose`](Puppet::pose) again).
    pub fn set_parameter(&mut self, id: &str, value: f32) {
        if let Some(i) = self.params.iter().position(|p| p.id == id)
            && value.is_finite()
        {
            self.values[i] = value.clamp(self.params[i].min, self.params[i].max);
        }
    }

    /// Evaluates the puppet at its current parameters: deformers, meshes, glue, draw order.
    pub fn pose(&mut self) {
        let vals = &self.values;
        // deformers, parents first
        let mut worlds: Vec<Option<World>> = (0..self.deformers.len()).map(|_| None).collect();
        for &i in &self.deformer_order {
            let d = &self.deformers[i];
            let world = match d.parent {
                Parent::Index(p) => match worlds.get(p).and_then(Option::as_ref) {
                    Some(parent) => d.world(vals, Some(parent)),
                    None => None,
                },
                Parent::Root | Parent::Missing => d.world(vals, None),
            };
            worlds[i] = world;
        }

        // drawables
        for ((def, out), key) in self.defs.iter().zip(self.drawables.iter_mut()).zip(self.sort_keys.iter_mut()) {
            out.posed = false;
            let parent = match def.parent {
                Parent::Root => None,
                Parent::Index(p) => match worlds.get(p).and_then(Option::as_ref) {
                    Some(w) => Some(w),
                    None => continue,
                },
                Parent::Missing => continue,
            };
            if def.base.is_empty() && out.indices.is_empty() {
                continue;
            }
            let corners = match &def.grid {
                Some(g) => match g.corners(vals) {
                    Some(c) => c,
                    None => continue,
                },
                None => Corners::rest(),
            };
            // channels, then blend shapes, then the deformer chain's
            let ch = &def.channels;
            let mut draw_order = scalar_at(&ch.draw_order, def.draw_order, vals);
            let mut opacity = scalar_at(&ch.opacity, def.opacity, vals);
            let mut multiply = color_at(&ch.multiply, def.multiply, vals);
            let mut screen = color_at(&ch.screen, def.screen, vals);
            let mut local = def.base.clone();
            if let Some(g) = &def.grid {
                for &(cell, w) in corners.list() {
                    if let Some(delta) = g.cells.get(&cell) {
                        for (o, d) in local.iter_mut().zip(delta) {
                            *o += w * d;
                        }
                    }
                }
            }
            if !def.shapes.is_empty() {
                for shape in &def.shapes {
                    let (keys, n) = shape.active(vals);
                    for &(k, w) in &keys[..n] {
                        let Some(Some(form)) = shape.forms.get(k) else { continue };
                        for (j, (o, d)) in local.iter_mut().zip(&form.deltas).enumerate() {
                            let reference = def.ref_deltas.as_ref().and_then(|r| r.get(j)).copied().unwrap_or(0.0);
                            *o += w * (d - reference);
                        }
                        draw_order += w * (form.draw_order - def.ref_draw_order);
                        opacity += w * (form.opacity - def.ref_opacity);
                        for c in 0..3 {
                            multiply[c] += w * (form.multiply[c] - def.ref_multiply[c]);
                            screen[c] += w * (form.screen[c] - def.ref_screen[c]);
                        }
                    }
                }
                opacity = opacity.clamp(0.0, 1.0);
                multiply = multiply.map(|c| c.clamp(0.0, 1.0));
                screen = screen.map(|c| c.clamp(0.0, 1.0));
            }
            if let Some(w) = parent {
                opacity *= w.opacity;
                for c in 0..3 {
                    multiply[c] = (multiply[c] * w.multiply[c]).clamp(0.0, 1.0);
                    screen[c] = screen_compose(screen[c], w.screen[c]).clamp(0.0, 1.0);
                }
            }
            out.positions.clear();
            out.positions.extend(local.chunks_exact(2).map(|p| {
                let (x, y) = match parent {
                    Some(w) => w.apply(p[0], p[1]),
                    None => (p[0], p[1]),
                };
                // canvas (y down) to world (y up)
                [x, -y]
            }));
            out.opacity = opacity;
            out.multiply = multiply;
            out.screen = screen;
            out.posed = true;
            *key = draw_order;
        }

        // glue, between posed meshes
        for g in &self.glues {
            if !(self.drawables[g.a].posed && self.drawables[g.b].posed) {
                continue;
            }
            let intensity = scalar_at(&g.channels.glue, g.intensity, vals);
            for &(ia, ib, wa, wb) in &g.pairs {
                let (Some(&[ax, ay]), Some(&[bx, by])) =
                    (self.drawables[g.a].positions.get(ia), self.drawables[g.b].positions.get(ib))
                else {
                    continue;
                };
                self.drawables[g.a].positions[ia] = [(bx - ax) * wa * intensity + ax, (by - ay) * wa * intensity + ay];
                self.drawables[g.b].positions[ib] = [(ax - bx) * wb * intensity + bx, (ay - by) * wb * intensity + by];
            }
        }

        // to model units
        let (o, s) = (self.origin, 1.0 / self.pixels_per_unit);
        for d in self.drawables.iter_mut().filter(|d| d.posed) {
            for p in &mut d.positions {
                *p = [(p[0] - o.x) * s, (p[1] - o.y) * s];
            }
        }

        // draw order: the render tree, each level sorted by draw order
        let part_orders: Vec<Option<f32>> = self.parts.iter().map(|p| p.draw_order_at(vals)).collect();
        let mut order = Vec::with_capacity(self.drawables.len());
        let ctx = Emit {
            parts: &self.parts,
            drawables: &self.drawables,
            sort_keys: &self.sort_keys,
            part_orders: &part_orders,
            vals,
        };
        ctx.emit(&self.tree, Look::IDENTITY, &mut order);
        self.order.clear();
        let mut seen = vec![false; self.drawables.len()];
        for (i, look) in order {
            if !(self.defs[i].shown && self.drawables[i].posed) || std::mem::replace(&mut seen[i], true) {
                continue;
            }
            let d = &mut self.drawables[i];
            if look != Look::IDENTITY {
                d.opacity *= look.opacity;
                for c in 0..3 {
                    d.multiply[c] = (d.multiply[c] * look.multiply[c]).clamp(0.0, 1.0);
                    d.screen[c] = screen_compose(d.screen[c], look.screen[c]).clamp(0.0, 1.0);
                }
            }
            self.order.push(i);
        }
    }

    fn visible_bounds(&self) -> (Vec2, Vec2) {
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for &i in &self.order {
            let d = &self.drawables[i];
            if d.opacity <= 0.01 {
                continue;
            }
            for p in &d.positions {
                let p = Vec2::from(*p);
                if p.is_finite() {
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
        if lo.x > hi.x { (Vec2::splat(-1.0), Vec2::splat(1.0)) } else { (lo, hi) }
    }
}

// ---------------------------------------------------------------------------------------------
// the archive

fn entry(zip: &mut zip::ZipArchive<File>, name: &str) -> Result<Option<Vec<u8>>, Error> {
    let mut f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(Error::Other(format!("{name}: {e}"))),
    };
    if f.size() > MAX_ENTRY {
        return Err(Error::Other(format!("{name} is too large")));
    }
    let mut v = Vec::with_capacity(f.size() as usize);
    (&mut f).take(MAX_ENTRY).read_to_end(&mut v)?;
    Ok(Some(v))
}

/// The archive's buffer entries, read once, and the accessors into them (UMA §4.9).
struct Buffers<'z> {
    zip: &'z mut zip::ZipArchive<File>,
    cache: HashMap<String, Vec<u8>>,
}

impl Buffers<'_> {
    fn words(&mut self, v: Option<&Value>, ty: &str) -> Result<Vec<u32>, Error> {
        let a = v.and_then(Value::as_object).ok_or(Error::Invalid("an array isn't a buffer accessor"))?;
        let buffer = a.get("buffer").and_then(Value::as_str).ok_or(Error::Invalid("an accessor names no buffer"))?;
        let size = |k: &str| a.get(k).and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok());
        let (Some(offset), Some(length), Some(count)) = (size("byteOffset"), size("byteLength"), size("count")) else {
            return Err(Error::Invalid("an accessor's sizes are missing"));
        };
        if a.get("componentType").and_then(Value::as_str) != Some(ty) {
            return Err(Error::Other(format!("an accessor isn't {ty}")));
        }
        if count.checked_mul(4) != Some(length) {
            return Err(Error::Invalid("an accessor's length doesn't match its count"));
        }
        if !self.cache.contains_key(buffer) {
            let bytes =
                entry(self.zip, buffer)?.ok_or_else(|| Error::Other(format!("the buffer {buffer} is missing")))?;
            self.cache.insert(buffer.to_string(), bytes);
        }
        let bytes = &self.cache[buffer];
        let end = offset
            .checked_add(length)
            .filter(|&e| e <= bytes.len())
            .ok_or(Error::Invalid("an accessor reaches past its buffer"))?;
        Ok(bytes[offset..end].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }

    fn floats(&mut self, v: Option<&Value>) -> Result<Vec<f32>, Error> {
        Ok(self.words(v, "float32")?.into_iter().map(f32::from_bits).collect())
    }

    fn ints(&mut self, v: Option<&Value>) -> Result<Vec<i32>, Error> {
        Ok(self.words(v, "int32")?.into_iter().map(|w| w as i32).collect())
    }
}

fn num(v: Option<&Value>) -> Option<f32> {
    v?.as_f64().map(|x| x as f32).filter(|x| x.is_finite())
}

fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn color(v: Option<&Value>, identity: [f32; 3]) -> [f32; 3] {
    match v.and_then(Value::as_array) {
        Some(a) if a.len() == 3 => std::array::from_fn(|k| num(a.get(k)).unwrap_or(identity[k])),
        _ => identity,
    }
}

const WHITE: [f32; 3] = [1.0; 3];
const BLACK: [f32; 3] = [0.0; 3];

fn screen_compose(a: f32, b: f32) -> f32 {
    a + b - a * b
}

// ---------------------------------------------------------------------------------------------
// keyform grids (KeyformGridSampling.kt)

struct Axis {
    param: Option<usize>,
    keys: Vec<f32>,
}

struct Grid<T> {
    axes: Vec<Axis>,
    /// Cells by their stride-folded index (the first axis varies fastest).
    cells: HashMap<u64, T>,
}

fn value(vals: &[f32], p: Option<usize>) -> f32 {
    p.and_then(|i| vals.get(i)).copied().unwrap_or(0.0)
}

/// The lower key and the fraction toward the next, or `None` out of range.
fn bracket(keys: &[f32], v: f32) -> Option<(usize, f32)> {
    let n = keys.len();
    if n <= 1 {
        if n == 0 {
            return Some((0, 0.0));
        }
        return (v > keys[0] - EPS_KEY && v < keys[0] + EPS_KEY).then_some((0, 0.0));
    }
    if v < keys[0] - EPS_KEY || v >= keys[n - 1] + EPS_KEY {
        return None;
    }
    if v < keys[0] + EPS_KEY {
        return Some((0, 0.0));
    }
    let mut hi = 1;
    while hi < n && keys[hi] + EPS_KEY <= v {
        hi += 1;
    }
    if hi >= n {
        return Some((n - 1, 0.0));
    }
    if v <= keys[hi] - EPS_KEY {
        let span = keys[hi] - keys[hi - 1];
        let f = if span >= EPS_SPAN { (v - keys[hi - 1]) / span } else { 0.0 };
        return Some((hi - 1, f));
    }
    Some((hi, 0.0))
}

/// The cells a grid blends at a pose and their weights (the first is the floor cell).
struct Corners {
    n: usize,
    c: [(u64, f32); MAX_CORNERS],
}

impl Corners {
    fn rest() -> Corners {
        let mut c = [(0, 0.0); MAX_CORNERS];
        c[0] = (0, 1.0);
        Corners { n: 1, c }
    }

    fn list(&self) -> &[(u64, f32)] {
        &self.c[..self.n]
    }
}

impl<T> Grid<T> {
    fn corners(&self, vals: &[f32]) -> Option<Corners> {
        let mut c = Corners::rest();
        let mut stride: u64 = 1;
        for axis in &self.axes {
            let (index, f) = bracket(&axis.keys, value(vals, axis.param))?;
            let index = index as u64;
            if f > 0.0 && c.n * 2 <= MAX_CORNERS {
                for i in (0..c.n).rev() {
                    let (li, w) = c.c[i];
                    c.c[2 * i] = (li.wrapping_add(index.wrapping_mul(stride)), w * (1.0 - f));
                    c.c[2 * i + 1] = (li.wrapping_add((index + 1).wrapping_mul(stride)), w * f);
                }
                c.n *= 2;
            } else {
                for corner in &mut c.c[..c.n] {
                    corner.0 = corner.0.wrapping_add(index.wrapping_mul(stride));
                }
            }
            stride = stride.wrapping_mul(axis.keys.len() as u64);
        }
        Some(c)
    }
}

// channels (ChannelSampling.kt)

#[derive(Default)]
struct Channels {
    draw_order: Option<Grid<f32>>,
    opacity: Option<Grid<f32>>,
    multiply: Option<Grid<[f32; 3]>>,
    screen: Option<Grid<[f32; 3]>>,
    flip_x: Option<Grid<bool>>,
    flip_y: Option<Grid<bool>>,
    glue: Option<Grid<f32>>,
}

fn scalar_or_none(grid: &Option<Grid<f32>>, vals: &[f32]) -> Option<f32> {
    let g = grid.as_ref()?;
    let corners = g.corners(vals)?;
    Some(corners.list().iter().filter_map(|(cell, w)| g.cells.get(cell).map(|v| w * v)).sum())
}

fn scalar_at(grid: &Option<Grid<f32>>, fallback: f32, vals: &[f32]) -> f32 {
    scalar_or_none(grid, vals).unwrap_or(fallback)
}

fn color_at(grid: &Option<Grid<[f32; 3]>>, fallback: [f32; 3], vals: &[f32]) -> [f32; 3] {
    let Some(g) = grid else { return fallback };
    let Some(corners) = g.corners(vals) else { return fallback };
    let mut o = [0.0; 3];
    for (cell, w) in corners.list() {
        if let Some(v) = g.cells.get(cell) {
            for k in 0..3 {
                o[k] += w * v[k];
            }
        }
    }
    o
}

fn flag_at(grid: &Option<Grid<bool>>, fallback: bool, vals: &[f32]) -> bool {
    let Some(g) = grid else { return fallback };
    let Some(corners) = g.corners(vals) else { return fallback };
    g.cells.get(&corners.c[0].0).copied().unwrap_or(fallback)
}

// blend shapes (BlendShapeEval.kt, BlendShapeSampling.kt)

struct Limit {
    param: Option<usize>,
    points: Vec<(f32, f32)>,
}

impl Limit {
    fn weight_at(&self, v: f32) -> f32 {
        let (Some(first), Some(last)) = (self.points.first(), self.points.last()) else { return 1.0 };
        if v <= first.0 {
            return first.1;
        }
        if v >= last.0 {
            return last.1;
        }
        let mut lower = 0;
        while lower + 1 < self.points.len() && self.points[lower + 1].0 <= v {
            lower += 1;
        }
        let (Some(lo), Some(up)) = (self.points.get(lower), self.points.get(lower + 1)) else { return last.1 };
        let span = up.0 - lo.0;
        if span > 0.0 { lo.1 + (v - lo.0) / span * (up.1 - lo.1) } else { lo.1 }
    }
}

struct Shape<F> {
    param: Option<usize>,
    keys: Vec<f32>,
    neutral: usize,
    forms: Vec<Option<F>>,
    limits: Vec<Limit>,
}

impl<F> Shape<F> {
    /// The keys that add to the pose, with their weights (at most two).
    fn active(&self, vals: &[f32]) -> ([(usize, f32); 2], usize) {
        let mut out = [(0, 0.0); 2];
        let mut n = 0;
        let mut multiplier: f32 = 1.0;
        for l in self.limits.iter().filter(|l| !l.points.is_empty()) {
            multiplier = multiplier.min(l.weight_at(value(vals, l.param)));
        }
        let keys = &self.keys;
        if multiplier <= 0.0 || keys.len() < 2 {
            return (out, 0);
        }
        let v = value(vals, self.param);
        let mut lower = 0;
        while lower + 1 < keys.len() && keys[lower + 1] <= v {
            lower += 1;
        }
        let span = if lower + 1 < keys.len() { keys[lower + 1] - keys[lower] } else { 0.0 };
        let f = if v <= keys[0] || lower + 1 >= keys.len() || span <= 0.0 { 0.0 } else { (v - keys[lower]) / span };
        let lower_w = (1.0 - f) * multiplier;
        if lower != self.neutral && lower_w > 0.0 {
            out[n] = (lower, lower_w);
            n += 1;
        }
        let upper_w = f * multiplier;
        if f > 0.0 && lower + 1 != self.neutral && upper_w > 0.0 {
            out[n] = (lower + 1, upper_w);
            n += 1;
        }
        (out, n)
    }
}

struct MeshForm {
    deltas: Vec<f32>,
    draw_order: f32,
    opacity: f32,
    multiply: [f32; 3],
    screen: [f32; 3],
}

struct DeformerForm {
    /// A warp's lattice, or a rotation's origin x, y, angle, scale.
    geometry: Vec<f32>,
    opacity: f32,
    multiply: [f32; 3],
    screen: [f32; 3],
}

// ---------------------------------------------------------------------------------------------
// deformers (DeformerCascade.kt, DeformTransforms.kt)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parent {
    Root,
    Index(usize),
    /// Names a deformer that isn't there.
    Missing,
}

enum Kind {
    Warp {
        rows: usize,
        cols: usize,
        bilinear: bool,
        grid: Option<Grid<Vec<f32>>>,
        reference: Option<Vec<f32>>,
    },
    /// Each cell: origin x, y, angle, scale.
    Rotation {
        base_angle: f32,
        flip: [bool; 2],
        grid: Option<Grid<[f32; 4]>>,
        reference: [f32; 4],
    },
}

struct DeformerDef {
    parent: Parent,
    kind: Kind,
    opacity: f32,
    multiply: [f32; 3],
    screen: [f32; 3],
    channels: Channels,
    shapes: Vec<Shape<DeformerForm>>,
    /// Opacity and colours at the default pose, what blend shapes add to.
    ref_look: Look,
}

/// Opacity, multiply and screen colours.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    opacity: f32,
    multiply: [f32; 3],
    screen: [f32; 3],
}

impl Look {
    const IDENTITY: Look = Look { opacity: 1.0, multiply: WHITE, screen: BLACK };

    /// This, under another (each multiplies, screens compose).
    fn under(self, parent: Look) -> Look {
        Look {
            opacity: self.opacity * parent.opacity,
            multiply: std::array::from_fn(|k| self.multiply[k] * parent.multiply[k]),
            screen: std::array::from_fn(|k| screen_compose(self.screen[k], parent.screen[k])),
        }
    }
}

enum Xform {
    Warp { cp: Vec<f32>, cols: usize, rows: usize, bilinear: bool },
    Rotation { c12: f32, c13: f32, c14: f32, c15: f32, ox: f32, oy: f32 },
}

/// A deformer posed: where it maps a point of its children's space (canvas pixels, y down), and
/// the opacity and colours it passes down.
struct World {
    xform: Xform,
    acc_y: f32,
    opacity: f32,
    multiply: [f32; 3],
    screen: [f32; 3],
}

impl World {
    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        match &self.xform {
            Xform::Warp { cp, cols, rows, bilinear } => warp_apply(cp, *cols, *rows, *bilinear, x, y),
            Xform::Rotation { c12, c13, c14, c15, ox, oy } => (c15 * y + c12 * x + ox, y * c13 + x * c14 + oy),
        }
    }

    fn is_rotation(&self) -> bool {
        matches!(self.xform, Xform::Rotation { .. })
    }
}

impl DeformerDef {
    /// Its look at a pose: channels, plus blend shapes, under its parent's.
    fn look(&self, vals: &[f32], parent: Option<&World>) -> Look {
        let ch = &self.channels;
        let mut look = Look {
            opacity: scalar_at(&ch.opacity, self.opacity, vals),
            multiply: color_at(&ch.multiply, self.multiply, vals),
            screen: color_at(&ch.screen, self.screen, vals),
        };
        let mut delta = Look { opacity: 0.0, multiply: BLACK, screen: BLACK };
        let mut contributed = false;
        for shape in &self.shapes {
            let (keys, n) = shape.active(vals);
            for &(k, w) in &keys[..n] {
                let Some(Some(form)) = shape.forms.get(k) else { continue };
                contributed = true;
                delta.opacity += w * (form.opacity - self.ref_look.opacity);
                for c in 0..3 {
                    delta.multiply[c] += w * (form.multiply[c] - self.ref_look.multiply[c]);
                    delta.screen[c] += w * (form.screen[c] - self.ref_look.screen[c]);
                }
            }
        }
        if contributed {
            look.opacity = (look.opacity + delta.opacity).clamp(0.0, 1.0);
            for c in 0..3 {
                look.multiply[c] = (look.multiply[c] + delta.multiply[c]).clamp(0.0, 1.0);
                look.screen[c] = (look.screen[c] + delta.screen[c]).clamp(0.0, 1.0);
            }
        }
        match parent {
            Some(p) => look.under(Look { opacity: p.opacity, multiply: p.multiply, screen: p.screen }),
            None => look,
        }
    }

    /// Posed at these parameters under its parent, or `None` when it's hidden (out of its grid's
    /// range, or unkeyed).
    fn world(&self, vals: &[f32], parent: Option<&World>) -> Option<World> {
        let parent_acc_y = parent.map_or(1.0, |p| p.acc_y);
        match &self.kind {
            Kind::Warp { rows, cols, bilinear, grid, reference } => {
                let (rows, cols) = (*rows, *cols);
                if rows == 0 || cols == 0 {
                    return None;
                }
                let grid = grid.as_ref()?;
                let corners = grid.corners(vals)?;
                let mut cp = vec![0.0; (rows + 1) * (cols + 1) * 2];
                for &(cell, w) in corners.list() {
                    if let Some(points) = grid.cells.get(&cell) {
                        for (o, p) in cp.iter_mut().zip(points) {
                            *o += w * p;
                        }
                    }
                }
                for shape in &self.shapes {
                    let (keys, n) = shape.active(vals);
                    for &(k, w) in &keys[..n] {
                        let Some(Some(form)) = shape.forms.get(k) else { continue };
                        for (j, (o, p)) in cp.iter_mut().zip(&form.geometry).enumerate() {
                            let r = reference.as_ref().and_then(|r| r.get(j)).copied().unwrap_or(0.0);
                            *o += w * (p - r);
                        }
                    }
                }
                let look = self.look(vals, parent);
                if let Some(p) = parent {
                    for point in cp.chunks_exact_mut(2) {
                        let (x, y) = p.apply(point[0], point[1]);
                        point[0] = x;
                        point[1] = y;
                    }
                }
                Some(World {
                    xform: Xform::Warp { cp, cols, rows, bilinear: *bilinear },
                    acc_y: parent_acc_y,
                    opacity: look.opacity,
                    multiply: look.multiply,
                    screen: look.screen,
                })
            }
            Kind::Rotation { base_angle, flip, grid, reference } => {
                let grid = grid.as_ref()?;
                let corners = grid.corners(vals)?;
                let mut f = [0.0f32; 4];
                for &(cell, w) in corners.list() {
                    if let Some(form) = grid.cells.get(&cell) {
                        for k in 0..4 {
                            f[k] += w * form[k];
                        }
                    }
                }
                let look = self.look(vals, parent);
                for shape in &self.shapes {
                    let (keys, n) = shape.active(vals);
                    for &(k, w) in &keys[..n] {
                        let Some(Some(form)) = shape.forms.get(k) else { continue };
                        for j in 0..4 {
                            f[j] += w * (form.geometry.get(j).copied().unwrap_or(reference[j]) - reference[j]);
                        }
                    }
                }
                let [origin_x, origin_y, angle, scale_acc] = f;
                let flip_x = flag_at(&self.channels.flip_x, flip[0], vals);
                let flip_y = flag_at(&self.channels.flip_y, flip[1], vals);
                let angle_deg = base_angle + angle;
                let scale = parent_acc_y * scale_acc;
                let (pivot, world_angle) = match parent {
                    None => ((origin_x, origin_y), angle_deg),
                    Some(p) => {
                        let pos = p.apply(origin_x, origin_y);
                        let inherited = inherited_angle(p, origin_x, origin_y, pos);
                        (pos, angle_deg - inherited * 180.0 / PI_F)
                    }
                };
                let radians = world_angle * PI_F / 180.0;
                let (sn, cs) = ((radians as f64).sin() as f32, (radians as f64).cos() as f32);
                let fx = if flip_x { -1.0 } else { 1.0 };
                let fy = if flip_y { -1.0 } else { 1.0 };
                Some(World {
                    xform: Xform::Rotation {
                        c12: cs * scale * fx,
                        c13: fy * cs * scale,
                        c14: fx * scale * sn,
                        c15: -sn * scale * fy,
                        ox: pivot.0,
                        oy: pivot.1,
                    },
                    acc_y: scale,
                    opacity: look.opacity,
                    multiply: look.multiply,
                    screen: look.screen,
                })
            }
        }
    }
}

/// How far a parent turns what's at a point: a short step up from it, mapped (radians).
fn inherited_angle(parent: &World, ox: f32, oy: f32, pos: (f32, f32)) -> f32 {
    let disp: f32 = if parent.is_rotation() { -10.0 } else { -0.1 };
    let (mut dx, mut dy) = (0.0f32, 0.0f32);
    let mut probe = 1.0f32;
    for _ in 0..10 {
        let (x, y) = parent.apply(ox, oy + disp * probe);
        dx = x - pos.0;
        dy = y - pos.1;
        if dx != 0.0 || dy != 0.0 {
            break;
        }
        let (x, y) = parent.apply(ox, oy - disp * probe);
        dx = pos.0 - x;
        dy = pos.1 - y;
        if dx != 0.0 || dy != 0.0 {
            break;
        }
        probe *= 0.1;
    }
    wrap_radians((disp as f64).atan2(0.0) as f32 - (dy as f64).atan2(dx as f64) as f32)
}

fn wrap_radians(mut a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    while a > PI_F {
        a -= 2.0 * PI_F;
    }
    while a < -PI_F {
        a += 2.0 * PI_F;
    }
    a
}

/// A point of a warp's (u, v) space mapped through its lattice: inside it, the lattice cell's
/// triangles or bilinear patch; outside, Cubism's extrapolation.
fn warp_apply(cp: &[f32], cols: usize, rows: usize, bilinear: bool, u: f32, v: f32) -> (f32, f32) {
    if (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
        let row_len = cols + 1;
        let gu = cols as f32 * u;
        let gv = rows as f32 * v;
        let (iu, iv) = ((gu as usize).min(cols - 1), (gv as usize).min(rows - 1));
        let (cu, cv) = (gu - iu as f32, gv - iv as f32);
        return lattice_cell_point(cp, row_len, bilinear, iv * row_len + iu, cu, cv);
    }
    warp_extrap(cp, cols, rows, u, v)
}

fn lattice_cell_point(cp: &[f32], row_len: usize, bilinear: bool, cell: usize, cu: f32, cv: f32) -> (f32, f32) {
    let at = |i: usize| cp.get(i).copied().unwrap_or(0.0);
    let p00 = cell * 2;
    let p10 = (cell + 1) * 2;
    let p01 = (cell + row_len) * 2;
    let p11 = (cell + row_len + 1) * 2;
    if !bilinear {
        if cu + cv <= 1.0 {
            (
                at(p10) * cu + at(p00) * (1.0 - cu - cv) + at(p01) * cv,
                (1.0 - cu - cv) * at(p00 + 1) + at(p10 + 1) * cu + at(p01 + 1) * cv,
            )
        } else {
            let w = (cu - 1.0) + cv;
            (
                at(p01) * (1.0 - cu) + at(p11) * w + at(p10) * (1.0 - cv),
                w * at(p11 + 1) + (1.0 - cu) * at(p01 + 1) + (1.0 - cv) * at(p10 + 1),
            )
        }
    } else {
        let iv = 1.0 - cv;
        (
            at(p10) * cu * iv + at(p00) * (1.0 - cu) * iv + at(p01) * (1.0 - cu) * cv + at(p11) * cu * cv,
            iv * at(p10 + 1) * cu
                + at(p00 + 1) * (1.0 - cu) * iv
                + (1.0 - cu) * at(p01 + 1) * cv
                + cu * at(p11 + 1) * cv,
        )
    }
}

/// Outside a warp's lattice (DeformTransforms.kt's `warpExtrap`, Cubism's): the lattice's
/// corners' average frame, blended toward the nearest edge within two lattice widths.
fn warp_extrap(cp: &[f32], cols: usize, rows: usize, u: f32, v: f32) -> (f32, f32) {
    let at = |i: usize| cp.get(i).copied().unwrap_or(0.0);
    let row_len = cols + 1;
    let last_row = rows * row_len;
    let (tlx, tly) = (at(0), at(1));
    let (trx, try_) = (at(cols * 2), at(cols * 2 + 1));
    let (blx, bly) = (at(last_row * 2), at(last_row * 2 + 1));
    let (brx, bry) = (at((cols + last_row) * 2), at((cols + last_row) * 2 + 1));
    let gu = cols as f32 * u;
    let gv = rows as f32 * v;
    let dux = ((brx - tlx) + (trx - blx)) * 0.5;
    let dvx = ((brx - tlx) - (trx - blx)) * 0.5;
    let duy = ((bry - tly) + (try_ - bly)) * 0.5;
    let dvy = ((bry - tly) - (try_ - bly)) * 0.5;
    let ox = (tlx + trx + blx + brx) * 0.25 - (brx - tlx) * 0.5;
    let oy = (tly + try_ + bly + bry) * 0.25 - (bry - tly) * 0.5;
    if u <= -2.0 || u >= 3.0 || v <= -2.0 || v >= 3.0 || !u.is_finite() || !v.is_finite() {
        return (dux * u + ox + dvx * v, dvy * v + duy * u + oy);
    }
    // every branch below sets every one of these (or returns)
    let (bu, bv, c0x, c0y, c1x, c1y): (f32, f32, f32, f32, f32, f32);
    let (mut c2x, mut c2y, mut c3x, mut c3y): (f32, f32, f32, f32);
    // the lattice row (or column) a fraction falls in, and the next
    let lo_hi = |g: f32, n: usize| -> (usize, usize, f32) {
        let lo = (g.max(0.0) as usize).min(n);
        if lo == n { (n - 1, n, n as f32) } else { (lo, lo + 1, (lo + 1) as f32) }
    };
    if u <= 0.0 {
        bu = (u + 2.0) * 0.5;
        c3x = ox - 2.0 * dux;
        c3y = oy - 2.0 * duy;
        if v <= 0.0 {
            c2x = tlx;
            c2y = tly;
            bv = (v + 2.0) * 0.5;
            c1x = ox - 2.0 * dvx;
            c0x = c3x - 2.0 * dvx;
            c1y = oy - 2.0 * dvy;
            c0y = c3y - 2.0 * dvy;
        } else if v < 1.0 {
            let (lo, hi, edge_hi) = lo_hi(gv, rows);
            bv = gv - lo as f32;
            let edge_lo = lo as f32 / rows as f32;
            c1x = at(lo * row_len * 2);
            c1y = at(lo * row_len * 2 + 1);
            c2x = at(hi * row_len * 2);
            c2y = at(hi * row_len * 2 + 1);
            c0x = edge_lo * dvx + c3x;
            c0y = edge_lo * dvy + c3y;
            c3x += (edge_hi / rows as f32) * dvx;
            c3y += (edge_hi / rows as f32) * dvy;
        } else {
            c1x = blx;
            c1y = bly;
            bv = (v - 1.0) * 0.5;
            c0x = c3x + dvx;
            c0y = c3y + dvy;
            c3x += 3.0 * dvx;
            c2x = 3.0 * dvx + ox;
            c3y += 3.0 * dvy;
            c2y = 3.0 * dvy + oy;
        }
    } else if u < 1.0 {
        if v <= 0.0 {
            let (lo, hi, edge_hi) = lo_hi(gu, cols);
            let (ex, ey) = (at(hi * 2), at(hi * 2 + 1));
            c2y = ey;
            bv = (v + 2.0) * 0.5;
            bu = gu - lo as f32;
            let edge_lo = lo as f32 / cols as f32;
            c3x = at(lo * 2);
            c3y = at(lo * 2 + 1);
            c2x = ex;
            c0x = (edge_lo * dux + ox) - 2.0 * dvx;
            c0y = (edge_lo * duy + oy) - 2.0 * dvy;
            c1x = ((edge_hi / cols as f32) * dux + ox) - 2.0 * dvx;
            c1y = ((edge_hi / cols as f32) * duy + oy) - 2.0 * dvy;
        } else if v >= 1.0 {
            let (lo, hi, edge_hi) = lo_hi(gu, cols);
            bv = (v - 1.0) * 0.5;
            bu = gu - lo as f32;
            let edge_lo = lo as f32 / cols as f32;
            c0x = at((lo + last_row) * 2);
            c0y = at((lo + last_row) * 2 + 1);
            c1x = at((last_row + hi) * 2);
            c1y = at((last_row + hi) * 2 + 1);
            c2x = (edge_hi / cols as f32) * dux + ox + 3.0 * dvx;
            c3x = edge_lo * dux + ox + 3.0 * dvx;
            c3y = edge_lo * duy + oy + 3.0 * dvy;
            c2y = (edge_hi / cols as f32) * duy + oy + 3.0 * dvy;
        } else {
            return (ox + dux * u + dvx * v, oy + dvy * v + duy * u);
        }
    } else {
        bu = (u - 1.0) * 0.5;
        c2x = 3.0 * dux + ox;
        c2y = 3.0 * duy + oy;
        if v <= 0.0 {
            c3x = trx;
            c3y = try_;
            bv = (v + 2.0) * 0.5;
            c0x = (ox + dux) - 2.0 * dvx;
            c1x = c2x - 2.0 * dvx;
            c1y = c2y - 2.0 * dvy;
            c0y = (oy + duy) - 2.0 * dvy;
        } else if v < 1.0 {
            let (lo, hi, edge_hi) = lo_hi(gv, rows);
            bv = gv - lo as f32;
            c0x = at((lo * row_len + cols) * 2);
            c0y = at((lo * row_len + cols) * 2 + 1);
            c3x = at((hi * row_len + cols) * 2);
            c3y = at((hi * row_len + cols) * 2 + 1);
            let edge_lo = lo as f32 / rows as f32;
            c1x = edge_lo * dvx + c2x;
            c1y = edge_lo * dvy + c2y;
            c2x += (edge_hi / rows as f32) * dvx;
            c2y += (edge_hi / rows as f32) * dvy;
        } else {
            c0x = brx;
            c0y = bry;
            bv = (v - 1.0) * 0.5;
            c1x = c2x + dvx;
            c1y = c2y + dvy;
            c3x = ox + dux + 3.0 * dvx;
            c2x += 3.0 * dvx;
            c3y = oy + duy + 3.0 * dvy;
            c2y += 3.0 * dvy;
        }
    }
    if bu + bv <= 1.0 {
        ((c1x - c0x) * bu + c0x + (c3x - c0x) * bv, (c1y - c0y) * bu + c0y + (c3y - c0y) * bv)
    } else {
        (
            (c3x - c2x) * (1.0 - bu) + c2x + (c1x - c2x) * (1.0 - bv),
            (1.0 - bu) * (c3y - c2y) + c2y + (1.0 - bv) * (c1y - c2y),
        )
    }
}

/// Deformers, parents first; those in a parent cycle are left out.
fn deformer_order(deformers: &[DeformerDef]) -> Vec<usize> {
    let mut depth: Vec<(usize, usize)> = Vec::new();
    for i in 0..deformers.len() {
        let mut at = i;
        let mut steps = 0;
        let ok = loop {
            match deformers[at].parent {
                Parent::Index(p) => {
                    at = p;
                    steps += 1;
                    if steps > deformers.len() {
                        break false;
                    }
                }
                Parent::Root | Parent::Missing => break true,
            }
        };
        if ok {
            depth.push((steps, i));
        }
    }
    depth.sort_by_key(|&(d, _)| d);
    depth.into_iter().map(|(_, i)| i).collect()
}

// ---------------------------------------------------------------------------------------------
// drawables, parts, the render tree

struct DrawableDef {
    parent: Parent,
    /// The keyform space's rest positions (its parent deformer's space), x, y.
    base: Vec<f32>,
    grid: Option<Grid<Vec<f32>>>,
    shapes: Vec<Shape<MeshForm>>,
    ref_deltas: Option<Vec<f32>>,
    ref_draw_order: f32,
    ref_opacity: f32,
    ref_multiply: [f32; 3],
    ref_screen: [f32; 3],
    draw_order: f32,
    opacity: f32,
    multiply: [f32; 3],
    screen: [f32; 3],
    channels: Channels,
    visible: bool,
    /// Reached through visible parts, and visible.
    shown: bool,
}

struct PartDef {
    children: Vec<Child>,
    visible: bool,
    sketch: bool,
    /// Grouped or isolated (sorted as one); else its children are sorted with its parent's.
    grouped: bool,
    isolated: bool,
    draw_order: i32,
    /// Its draw order at the default pose, what blend shapes add to.
    ref_draw_order: f32,
    composite: Look,
    channels: Channels,
    shapes: Vec<Shape<f32>>,
}

impl PartDef {
    /// Its draw order when it's animated (a track or a blend shape), else `None`.
    fn draw_order_at(&self, vals: &[f32]) -> Option<f32> {
        let tracked = scalar_or_none(&self.channels.draw_order, vals);
        let mut delta = None;
        for shape in &self.shapes {
            let (keys, n) = shape.active(vals);
            for &(k, w) in &keys[..n] {
                let Some(Some(form)) = shape.forms.get(k) else { continue };
                let d: &mut f32 = delta.get_or_insert(0.0);
                *d += w * (form - self.ref_draw_order);
            }
        }
        if tracked.is_none() && delta.is_none() {
            return None;
        }
        Some(tracked.unwrap_or(self.draw_order as f32) + delta.unwrap_or(0.0))
    }
}

#[derive(Clone, Copy)]
enum Child {
    Part(usize),
    Drawable(usize),
}

enum Node {
    Drawable(usize),
    Group { part: usize, children: Vec<Node> },
}

fn child_of(c: &Value, parts: &HashMap<&str, usize>, drawables: &HashMap<&str, usize>) -> Option<Child> {
    if let Some(id) = c.get("part").and_then(Value::as_str) {
        return parts.get(id).map(|&i| Child::Part(i));
    }
    let id = c.get("drawable").and_then(Value::as_str)?;
    drawables.get(id).map(|&i| Child::Drawable(i))
}

/// The render tree (RenderRootDerive.kt): back to front (the outliner's top is the front),
/// pass-through parts' children lifted into their parent's level.
fn render_tree(root: &[Child], parts: &[PartDef], drawable_count: usize) -> Vec<Node> {
    fn collect(
        children: &[Child],
        parts: &[PartDef],
        entered: &mut Vec<usize>,
        placed: &mut [bool],
        into: &mut Vec<Node>,
    ) {
        for child in children.iter().rev() {
            match *child {
                Child::Drawable(i) => {
                    if let Some(p) = placed.get_mut(i) {
                        *p = true;
                    }
                    into.push(Node::Drawable(i));
                }
                Child::Part(i) => {
                    if entered.contains(&i) {
                        continue;
                    }
                    let Some(part) = parts.get(i) else { continue };
                    entered.push(i);
                    if part.grouped {
                        let mut group = Vec::new();
                        collect(&part.children, parts, entered, placed, &mut group);
                        into.push(Node::Group { part: i, children: group });
                    } else {
                        collect(&part.children, parts, entered, placed, into);
                    }
                    entered.pop();
                }
            }
        }
    }
    let mut placed = vec![false; drawable_count];
    let mut tree = Vec::new();
    collect(root, parts, &mut Vec::new(), &mut placed, &mut tree);
    tree.extend(placed.iter().enumerate().filter(|(_, p)| !**p).map(|(i, _)| Node::Drawable(i)));
    tree
}

fn mark_shown(
    children: &[Child],
    parts: &[PartDef],
    defs: &[DrawableDef],
    visible: bool,
    entered: &mut HashSet<usize>,
    shown: &mut [bool],
) {
    for child in children {
        match *child {
            Child::Drawable(i) => {
                if visible && defs.get(i).is_some_and(|d| d.visible) {
                    shown[i] = true;
                }
            }
            Child::Part(i) => {
                if !entered.insert(i) {
                    continue;
                }
                if let Some(p) = parts.get(i) {
                    mark_shown(&p.children, parts, defs, visible && p.visible && !p.sketch, entered, shown);
                }
            }
        }
    }
}

/// Walks the render tree into draw order.
struct Emit<'a> {
    parts: &'a [PartDef],
    drawables: &'a [Drawable],
    sort_keys: &'a [f32],
    part_orders: &'a [Option<f32>],
    vals: &'a [f32],
}

impl Emit<'_> {
    fn key(&self, node: &Node) -> i32 {
        match node {
            Node::Drawable(i) => {
                let posed = self.drawables.get(*i).is_some_and(|d| d.posed);
                let order = if posed {
                    self.sort_keys.get(*i).copied().unwrap_or(DEFAULT_DRAW_ORDER)
                } else {
                    DEFAULT_DRAW_ORDER
                };
                (order + 0.001) as i32
            }
            Node::Group { part, .. } => match self.part_orders.get(*part).copied().flatten() {
                Some(o) => (o + 0.001) as i32,
                None => self.parts.get(*part).map_or(DEFAULT_DRAW_ORDER as i32, |p| p.draw_order),
            },
        }
    }

    fn emit(&self, nodes: &[Node], look: Look, out: &mut Vec<(usize, Look)>) {
        let mut sorted: Vec<(i32, &Node)> = nodes.iter().map(|n| (self.key(n), n)).collect();
        sorted.sort_by_key(|&(k, _)| k);
        for (_, node) in sorted {
            match node {
                Node::Drawable(i) => out.push((*i, look)),
                Node::Group { part, children } => {
                    let inner = match self.parts.get(*part) {
                        Some(p) if p.isolated => {
                            let ch = &p.channels;
                            let own = Look {
                                opacity: scalar_at(&ch.opacity, p.composite.opacity, self.vals),
                                multiply: color_at(&ch.multiply, p.composite.multiply, self.vals),
                                screen: color_at(&ch.screen, p.composite.screen, self.vals),
                            };
                            own.under(look)
                        }
                        _ => look,
                    };
                    self.emit(children, inner, out);
                }
            }
        }
    }
}

struct GlueDef {
    a: usize,
    b: usize,
    pairs: Vec<(usize, usize, f32, f32)>,
    intensity: f32,
    channels: Channels,
}

// ---------------------------------------------------------------------------------------------
// reading the puppet entry (UmaPuppetImport.kt)

struct Loader<'b, 'z> {
    bufs: &'b mut Buffers<'z>,
    param_index: HashMap<String, usize>,
    /// Each parameter's default, by index.
    defaults: Vec<f32>,
}

impl Loader<'_, '_> {
    fn axes(&self, v: Option<&Value>) -> Vec<Axis> {
        arr(v)
            .iter()
            .map(|a| Axis {
                param: a.get("parameter").and_then(Value::as_str).and_then(|id| self.param_index.get(id).copied()),
                keys: arr(a.get("keys")).iter().map(|k| num(Some(k)).unwrap_or(0.0)).collect(),
            })
            .collect()
    }

    /// A grid: its axes, and its cells read by `form` (cells it can't read are left out).
    fn grid<T>(
        &mut self,
        v: Option<&Value>,
        mut form: impl FnMut(&mut Self, &Json) -> Result<Option<T>, Error>,
    ) -> Result<Option<Grid<T>>, Error> {
        let Some(g) = v.and_then(Value::as_object) else { return Ok(None) };
        let axes = self.axes(g.get("axes"));
        let mut cells = HashMap::new();
        for c in arr(g.get("cells")) {
            let Some(c) = c.as_object() else { continue };
            let coordinate: Vec<u64> = arr(c.get("coordinate")).iter().map(|k| k.as_u64().unwrap_or(0)).collect();
            let mut index: u64 = 0;
            let mut stride: u64 = 1;
            for (axis, k) in axes.iter().zip(&coordinate) {
                index = index.wrapping_add(k.wrapping_mul(stride));
                stride = stride.wrapping_mul(axis.keys.len() as u64);
            }
            if let Some(f) = form(self, c)? {
                cells.insert(index, f);
            }
        }
        Ok(Some(Grid { axes, cells }))
    }

    fn channels(&mut self, v: Option<&Value>) -> Result<Channels, Error> {
        let mut ch = Channels::default();
        let Some(map) = v.and_then(Value::as_object) else { return Ok(ch) };
        let scalar = |_: &mut Self, c: &Json| Ok(num(c.get("value")));
        let rgb = |_: &mut Self, c: &Json| {
            Ok(c.get("value")
                .and_then(Value::as_array)
                .filter(|a| a.len() == 3)
                .map(|a| std::array::from_fn(|k| num(a.get(k)).unwrap_or(0.0))))
        };
        let flag = |_: &mut Self, c: &Json| Ok(c.get("value").and_then(Value::as_bool));
        for (name, grid) in map {
            match name.as_str() {
                "drawOrder" => ch.draw_order = self.grid(Some(grid), scalar)?,
                "opacity" => ch.opacity = self.grid(Some(grid), scalar)?,
                "glueIntensity" => ch.glue = self.grid(Some(grid), scalar)?,
                "multiplyColor" => ch.multiply = self.grid(Some(grid), rgb)?,
                "screenColor" => ch.screen = self.grid(Some(grid), rgb)?,
                "flipX" => ch.flip_x = self.grid(Some(grid), flag)?,
                "flipY" => ch.flip_y = self.grid(Some(grid), flag)?,
                _ => {}
            }
        }
        Ok(ch)
    }

    fn shapes<F>(
        &mut self,
        v: Option<&Value>,
        mut form: impl FnMut(&mut Self, &Json) -> Result<F, Error>,
    ) -> Result<Vec<Shape<F>>, Error> {
        let mut out = Vec::new();
        for b in arr(v) {
            let Some(b) = b.as_object() else { continue };
            let mut forms = Vec::new();
            for f in arr(b.get("forms")) {
                forms.push(match f.as_object() {
                    Some(f) => Some(form(self, f)?),
                    None => None,
                });
            }
            let limits = arr(b.get("limits"))
                .iter()
                .map(|l| Limit {
                    param: l.get("parameter").and_then(Value::as_str).and_then(|id| self.param_index.get(id).copied()),
                    points: arr(l.get("points"))
                        .iter()
                        .map(|p| (num(p.get("value")).unwrap_or(0.0), num(p.get("weight")).unwrap_or(1.0)))
                        .collect(),
                })
                .collect();
            out.push(Shape {
                param: b.get("parameter").and_then(Value::as_str).and_then(|id| self.param_index.get(id).copied()),
                keys: arr(b.get("keys")).iter().map(|k| num(Some(k)).unwrap_or(0.0)).collect(),
                neutral: b.get("neutralIndex").and_then(Value::as_u64).map_or(usize::MAX, |n| n as usize),
                forms,
                limits,
            });
        }
        Ok(out)
    }

    fn look_form(f: &Json) -> (f32, [f32; 3], [f32; 3]) {
        (num(f.get("opacity")).unwrap_or(1.0), color(f.get("multiplyColor"), WHITE), color(f.get("screenColor"), BLACK))
    }

    fn deformer(&mut self, d: &Json, parent: Parent) -> Result<DeformerDef, Error> {
        let channels = self.channels(d.get("channels"))?;
        let opacity = num(d.get("opacity")).unwrap_or(1.0);
        let multiply = color(d.get("multiplyColor"), WHITE);
        let screen = color(d.get("screenColor"), BLACK);
        let defaults: Vec<f32> = self.defaults.clone();
        let warp = d.get("kind").and_then(Value::as_str) == Some("warp");
        let (kind, shapes) = if warp {
            let rows = d.get("rows").and_then(Value::as_u64).unwrap_or(0) as usize;
            let cols = d.get("columns").and_then(Value::as_u64).unwrap_or(0) as usize;
            if (rows as u64 + 1).saturating_mul(cols as u64 + 1) > 1 << 20 {
                return Err(Error::Invalid("a warp's lattice is too large"));
            }
            let grid = self.grid(d.get("geometry"), |s, c| match c.get("controlPoints") {
                Some(cp) => Ok(Some(s.bufs.floats(Some(cp))?)),
                None => Ok(None),
            })?;
            let shapes = self.shapes(d.get("blendShapes"), |s, f| {
                let (opacity, multiply, screen) = Self::look_form(f);
                let geometry = match f.get("controlPoints") {
                    Some(cp) => s.bufs.floats(Some(cp))?,
                    None => Vec::new(),
                };
                Ok(DeformerForm { geometry, opacity, multiply, screen })
            })?;
            // the lattice at the default pose, what blend shapes add to
            let reference = grid.as_ref().and_then(|g| {
                let corners = g.corners(&defaults)?;
                let mut out: Option<Vec<f32>> = None;
                for &(cell, w) in corners.list() {
                    let Some(points) = g.cells.get(&cell) else { continue };
                    let o = out.get_or_insert_with(|| vec![0.0; points.len()]);
                    for (o, p) in o.iter_mut().zip(points) {
                        *o += w * p;
                    }
                }
                out
            });
            let bilinear = d.get("isQuadTransform").and_then(Value::as_bool).unwrap_or(false);
            (Kind::Warp { rows, cols, bilinear, grid, reference }, shapes)
        } else {
            let pivot = |c: &Json| -> Option<[f32; 4]> {
                Some([num(c.get("originX"))?, num(c.get("originY"))?, num(c.get("angle"))?, num(c.get("scale"))?])
            };
            let grid = self.grid(d.get("geometry"), |_, c| Ok(pivot(c)))?;
            let shapes = self.shapes(d.get("blendShapes"), |_, f| {
                let (opacity, multiply, screen) = Self::look_form(f);
                Ok(DeformerForm { geometry: pivot(f).map(Vec::from).unwrap_or_default(), opacity, multiply, screen })
            })?;
            let reference = grid
                .as_ref()
                .and_then(|g| {
                    let corners = g.corners(&defaults)?;
                    let mut out = [0.0f32; 4];
                    let mut any = false;
                    for &(cell, w) in corners.list() {
                        let Some(f) = g.cells.get(&cell) else { continue };
                        any = true;
                        for k in 0..4 {
                            out[k] += w * f[k];
                        }
                    }
                    any.then_some(out)
                })
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let flip = [
                d.get("flipX").and_then(Value::as_bool).unwrap_or(false),
                d.get("flipY").and_then(Value::as_bool).unwrap_or(false),
            ];
            let base_angle = num(d.get("baseAngle")).unwrap_or(0.0);
            (Kind::Rotation { base_angle, flip, grid, reference }, shapes)
        };
        let ref_look = Look {
            opacity: scalar_at(&channels.opacity, opacity, &defaults),
            multiply: color_at(&channels.multiply, multiply, &defaults),
            screen: color_at(&channels.screen, screen, &defaults),
        };
        Ok(DeformerDef { parent, kind, opacity, multiply, screen, channels, shapes, ref_look })
    }

    fn drawable(
        &mut self,
        d: &Json,
        parent: Parent,
        drawables: &HashMap<&str, usize>,
    ) -> Result<(DrawableDef, Drawable), Error> {
        let name =
            d.get("name").and_then(Value::as_str).or(d.get("id").and_then(Value::as_str)).unwrap_or("").to_string();
        let defaults = self.defaults.clone();
        let (base, uvs, indices) = match d.get("mesh").and_then(Value::as_object) {
            Some(m) => {
                let canvas = self.bufs.floats(m.get("canvasPositions"))?;
                let local = self.bufs.floats(m.get("localPositions"))?;
                let uvs = self.bufs.floats(m.get("uvs"))?;
                let indices = self.bufs.ints(m.get("indices"))?;
                let n = canvas.len() / 2;
                if canvas.len() % 2 != 0
                    || local.len() != canvas.len()
                    || uvs.len() != canvas.len()
                    || indices.len() % 3 != 0
                {
                    return Err(Error::Other(format!("the drawable {name}'s mesh doesn't add up")));
                }
                if indices.iter().any(|&i| i < 0 || i as usize >= n) {
                    return Err(Error::Other(format!("the drawable {name}'s triangles name vertices it doesn't have")));
                }
                let uvs = uvs.chunks_exact(2).map(|p| [p[0], p[1]]).collect();
                (local, uvs, indices.into_iter().map(|i| i as u32).collect())
            }
            None => (Vec::new(), Vec::new(), Vec::new()),
        };
        let channels = self.channels(d.get("channels"))?;
        let grid = self.grid(d.get("geometry"), |s, c| Ok(Some(s.bufs.floats(c.get("positionDeltas"))?)))?;
        let shapes = self.shapes(d.get("blendShapes"), |s, f| {
            let (opacity, multiply, screen) = Self::look_form(f);
            Ok(MeshForm {
                deltas: s.bufs.floats(f.get("positionDeltas"))?,
                draw_order: num(f.get("drawOrder")).unwrap_or(DEFAULT_DRAW_ORDER),
                opacity,
                multiply,
                screen,
            })
        })?;
        let draw_order = num(d.get("drawOrder")).unwrap_or(DEFAULT_DRAW_ORDER);
        let opacity = num(d.get("opacity")).unwrap_or(1.0);
        let multiply = color(d.get("multiplyColor"), WHITE);
        let screen = color(d.get("screenColor"), BLACK);
        // the grid at the default pose, what blend shapes add to
        let ref_deltas = grid.as_ref().and_then(|g| {
            let corners = g.corners(&defaults)?;
            let len = g.cells.values().next().map_or(0, Vec::len);
            let mut out = vec![0.0; len];
            for &(cell, w) in corners.list() {
                if let Some(delta) = g.cells.get(&cell) {
                    for (o, x) in out.iter_mut().zip(delta) {
                        *o += w * x;
                    }
                }
            }
            Some(out)
        });
        let blend = match d.get("blendMode").and_then(Value::as_str) {
            Some("additivePremultiplied" | "additive" | "additiveGlow") => Blend::Additive,
            Some("multiplyPremultiplied" | "multiply") => Blend::Multiply,
            _ => Blend::Normal,
        };
        let def = DrawableDef {
            parent,
            ref_draw_order: scalar_at(&channels.draw_order, draw_order, &defaults),
            ref_opacity: scalar_at(&channels.opacity, opacity, &defaults),
            ref_multiply: color_at(&channels.multiply, multiply, &defaults),
            ref_screen: color_at(&channels.screen, screen, &defaults),
            base,
            grid,
            shapes,
            ref_deltas,
            draw_order,
            opacity,
            multiply,
            screen,
            channels,
            visible: d.get("isVisible").and_then(Value::as_bool).unwrap_or(true),
            shown: false,
        };
        let drawable = Drawable {
            name,
            uvs,
            indices,
            texture: 0,
            positions: Vec::new(),
            opacity,
            multiply,
            screen,
            blend,
            masks: arr(d.get("maskedBy")).iter().filter_map(|m| drawables.get(m.as_str()?).copied()).collect(),
            inverted_mask: d.get("invertMask").and_then(Value::as_bool).unwrap_or(false),
            posed: false,
        };
        Ok((def, drawable))
    }

    fn part(
        &mut self,
        p: &Json,
        parts: &HashMap<&str, usize>,
        drawables: &HashMap<&str, usize>,
    ) -> Result<PartDef, Error> {
        let mode = p.get("groupMode").and_then(Value::as_str);
        let composite = p.get("composite").and_then(Value::as_object);
        let draw_order = p
            .get("drawOrder")
            .and_then(Value::as_i64)
            .map_or(DEFAULT_DRAW_ORDER as i32, |o| o.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        let channels = self.channels(p.get("channels"))?;
        let shapes =
            self.shapes(p.get("blendShapes"), |_, f| Ok(num(f.get("drawOrder")).unwrap_or(DEFAULT_DRAW_ORDER)))?;
        let defaults = self.defaults.clone();
        Ok(PartDef {
            children: arr(p.get("children")).iter().filter_map(|c| child_of(c, parts, drawables)).collect(),
            visible: p.get("isVisible").and_then(Value::as_bool).unwrap_or(true),
            sketch: p.get("isSketch").and_then(Value::as_bool).unwrap_or(false),
            grouped: matches!(mode, Some("grouped" | "isolated")),
            isolated: mode == Some("isolated"),
            draw_order,
            composite: Look {
                opacity: num(composite.and_then(|c| c.get("opacity"))).unwrap_or(1.0),
                multiply: color(composite.and_then(|c| c.get("multiplyColor")), WHITE),
                screen: color(composite.and_then(|c| c.get("screenColor")), BLACK),
            },
            ref_draw_order: scalar_at(&channels.draw_order, draw_order as f32, &defaults),
            channels,
            shapes,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// textures (UmaTextures.kt, AtlasPlacement.kt)

fn decode_png(zip: &mut zip::ZipArchive<File>, path: &str) -> Result<image::RgbaImage, Error> {
    let bytes = entry(zip, path)?.ok_or_else(|| Error::Other(format!("the texture {path} is missing")))?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|e| Error::Other(format!("{path}: {e}")))?;
    Ok(img.to_rgba8())
}

fn premultiplied(img: image::RgbaImage, pad: u32) -> Texture {
    let (w, h) = img.dimensions();
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mut rgba = vec![0u8; pw as usize * ph as usize * 4];
    for (y, row) in img.rows().enumerate() {
        let start = ((y + pad as usize) * pw as usize + pad as usize) * 4;
        for (x, px) in row.enumerate() {
            let [r, g, b, a] = px.0;
            let pm = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
            rgba[start + x * 4..start + x * 4 + 4].copy_from_slice(&[pm(r), pm(g), pm(b), a]);
        }
    }
    Texture { width: pw, height: ph, rgba }
}

/// The textures the drawables sample, and each drawable's texture and texture coordinates: the
/// render pages when the file stores them (an imported MOC3's or CMO3's), else each drawable's
/// own tile of source art (its coordinates mapped from the packed page into the tile, the tile
/// given a transparent pixel's border).
fn load_textures(
    zip: &mut zip::ZipArchive<File>,
    index: Option<&Value>,
    drawables: &mut [Drawable],
    drawable_json: &[Value],
) -> Result<Vec<Texture>, Error> {
    let mut textures = Vec::new();
    let mut blank: Option<usize> = None;
    let mut blank_texture = |textures: &mut Vec<Texture>| -> usize {
        *blank.get_or_insert_with(|| {
            textures.push(Texture { width: 1, height: 1, rgba: vec![0; 4] });
            textures.len() - 1
        })
    };
    let id_of = |j: usize| drawable_json.get(j).and_then(|d| d.get("id")).and_then(Value::as_str);
    let field = |j: usize, k: &str| drawable_json.get(j).and_then(|d| d.get(k));
    let by_id: HashMap<&str, usize> = (0..drawable_json.len()).filter_map(|j| Some((id_of(j)?, j))).rev().collect();
    // the drawable whose texture a drawable shares (a duplicate's source), else itself
    let source = |j: usize| -> usize {
        field(j, "textureSource").and_then(Value::as_str).and_then(|id| by_id.get(id).copied()).unwrap_or(j)
    };

    let render_pages = index.and_then(|i| i.get("renderPages")).filter(|r| !arr(r.get("pages")).is_empty());
    if let Some(rp) = render_pages {
        for page in arr(rp.get("pages")) {
            let path =
                page.get("path").and_then(Value::as_str).ok_or(Error::Invalid("a render page names no image"))?;
            textures.push(premultiplied(decode_png(zip, path)?, 0));
        }
        let count = textures.len();
        let map = rp.get("drawablePages").and_then(Value::as_object);
        for (j, d) in drawables.iter_mut().enumerate() {
            let key = id_of(source(j)).unwrap_or("");
            let page = map
                .and_then(|m| m.get(key))
                .and_then(Value::as_u64)
                .or_else(|| field(j, "texturePage").and_then(Value::as_u64))
                .map(|p| p as usize)
                .filter(|&p| p < count);
            d.texture = page.unwrap_or(0);
        }
        return Ok(textures);
    }

    // pages derived from the tiles: each drawable samples its tile directly
    let index = index.and_then(Value::as_object);
    let pages: Vec<(f32, f32)> = arr(index.and_then(|i| i.get("pages")))
        .iter()
        .map(|p| (num(p.get("width")).unwrap_or(0.0), num(p.get("height")).unwrap_or(0.0)))
        .collect();
    let uvs_address_pages = index.and_then(|i| i.get("storedUvsAddressPages")).and_then(Value::as_bool).unwrap_or(true);
    let tiles: HashMap<&str, &Value> =
        arr(index.and_then(|i| i.get("tiles"))).iter().filter_map(|t| Some((t.get("id")?.as_str()?, t))).collect();
    let mut tile_textures: HashMap<String, usize> = HashMap::new();
    for (j, d) in drawables.iter_mut().enumerate() {
        let tile_id = field(j, "atlasTile").or_else(|| field(source(j), "atlasTile")).and_then(Value::as_str);
        let tile = tile_id.and_then(|id| tiles.get(id).map(|t| (id, *t)));
        let Some((tile_id, tile)) = tile else {
            d.texture = blank_texture(&mut textures);
            continue;
        };
        let texture = match tile_textures.get(tile_id) {
            Some(&t) => t,
            None => {
                let path = tile.get("path").and_then(Value::as_str).ok_or(Error::Invalid("a tile names no image"))?;
                textures.push(premultiplied(decode_png(zip, path)?, 1));
                tile_textures.insert(tile_id.to_string(), textures.len() - 1);
                textures.len() - 1
            }
        };
        d.texture = texture;
        let (tw, th) = (textures[texture].width as f32 - 2.0, textures[texture].height as f32 - 2.0);
        // from the page's coordinates to the tile's (AtlasPlacement.kt's layerUvAffineOf)
        let placement = tile.get("placement").filter(|_| uvs_address_pages);
        let affine = placement.and_then(|p| {
            let page = pages.get(p.get("page")?.as_u64()? as usize)?;
            let (px, py) = (num(p.get("positionX"))?, num(p.get("positionY"))?);
            let (sx, sy) = (num(p.get("scaleX"))?, num(p.get("scaleY"))?);
            if sx == 0.0 || sy == 0.0 || tw <= 0.0 || th <= 0.0 {
                return None;
            }
            let r = (num(p.get("rotationDegrees"))? as f64).to_radians();
            let (c, s) = (r.cos(), r.sin());
            let (m00, m01) = ((c / sx as f64) as f32, (s / sx as f64) as f32);
            let (m10, m11) = ((-s / sy as f64) as f32, (c / sy as f64) as f32);
            let (m02, m12) = (-(m00 * px + m01 * py), -(m10 * px + m11 * py));
            Some([m00 * page.0 / tw, m01 * page.1 / tw, m02 / tw, m10 * page.0 / th, m11 * page.1 / th, m12 / th])
        });
        for uv in &mut d.uvs {
            let [u, v] = *uv;
            let [u, v] = match affine {
                Some(m) => [m[0] * u + m[1] * v + m[2], m[3] * u + m[4] * v + m[5]],
                None => [u, v],
            };
            // into the bordered texture
            *uv = [(u * tw + 1.0) / (tw + 2.0), (v * th + 1.0) / (th + 2.0)];
        }
    }
    Ok(textures)
}

// ---------------------------------------------------------------------------------------------
// the face

/// The standard parameters a face sets, those the puppet has (by index).
#[derive(Clone, Debug, Default)]
struct FaceParams {
    angle: [Option<usize>; 3],
    body: [Option<usize>; 3],
    /// The model's right, left.
    eye_open: [Vec<usize>; 2],
    eye_ball: [Option<usize>; 2],
    /// Eyes narrowing in a smile: the model's right, left.
    eye_smile: [Option<usize>; 2],
    brow: [Option<usize>; 2],
    mouth_open: Vec<usize>,
    mouth_form: Option<usize>,
    vowels: [Option<usize>; 5],
    breath: Option<usize>,
}

impl FaceParams {
    fn find(index: &HashMap<String, usize>) -> FaceParams {
        let find = |id: &str| index.get(id).copied();
        FaceParams {
            angle: ["ParamAngleX", "ParamAngleY", "ParamAngleZ"].map(find),
            body: ["ParamBodyAngleX", "ParamBodyAngleY", "ParamBodyAngleZ"].map(find),
            eye_open: [find("ParamEyeROpen").into_iter().collect(), find("ParamEyeLOpen").into_iter().collect()],
            eye_ball: ["ParamEyeBallX", "ParamEyeBallY"].map(find),
            eye_smile: ["ParamEyeRSmile", "ParamEyeLSmile"].map(find),
            brow: ["ParamBrowRY", "ParamBrowLY"].map(find),
            mouth_open: find("ParamMouthOpenY").into_iter().collect(),
            mouth_form: find("ParamMouthForm"),
            vowels: ["ParamA", "ParamI", "ParamU", "ParamE", "ParamO"].map(find),
            breath: find("ParamBreath"),
        }
    }
}
