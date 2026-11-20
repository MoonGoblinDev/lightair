//! Live2D models (Cubism 3 to 5: `.model3.json`, `.moc3`, textures, physics, pose, expressions)
//! on mocari, a pure Rust runtime (third_party/mocari, Lightair's fork): no Cubism Core, no
//! Live2D SDK. A [`Face`] sets the model's standard parameters each frame, then its expression,
//! physics and pose are applied and its meshes rebuilt, ready to draw.
//!
//! Live2D's standard parameters read the face the way the rest of this crate does (checked on
//! Live2D's own sample model): `ParamAngleX` > 0 turns the face toward the viewer's right (the
//! model's left), `ParamAngleY` > 0 looks up, `ParamAngleZ` > 0 tilts counter-clockwise as seen,
//! `ParamEyeLOpen` is the model's own left eye, `ParamEyeBallX` > 0 looks toward the viewer's
//! right and `ParamEyeBallY` > 0 up.

use std::path::Path;

use glam::Vec2;
use mocari::assets::{DecodedTexture, RuntimeModel, load_model_runtime};
use mocari::expression::load_expression;
use mocari::json::Expression3;
use mocari::json::Motion3;
use mocari::moc3::Moc3DrawableMesh;
use mocari::motion::load_motion;
use mocari::render::common::{DrawableInfo, draw_order_indices};
use mocari::{ExpressionManager, MotionPlayer};

use crate::Error;
use crate::rig::Face;

/// The standard parameters a face sets, those the model has (by index).
#[derive(Clone, Debug, Default)]
struct Params {
    angle: [Option<usize>; 3],
    body: [Option<usize>; 3],
    eye_open: [Vec<usize>; 2],
    eye_ball: [Option<usize>; 2],
    /// Eyes narrowing in a smile: the model's right, left.
    eye_smile: [Option<usize>; 2],
    brow: [Option<usize>; 2],
    mouth_open: Vec<usize>,
    mouth_form: Option<usize>,
    vowels: [Option<usize>; 5],
    breath: Option<usize>,
    cheek: Option<usize>,
}

pub struct Puppet {
    model: RuntimeModel,
    params: Params,
    expressions: Vec<(String, Expression3)>,
    manager: ExpressionManager,
    playing: Option<String>,
    /// The model's Idle motions, played one after another under the face (which wins where both
    /// move a parameter: the head, eyes, mouth).
    idle: Vec<Motion3>,
    idle_player: Option<(usize, MotionPlayer)>,
    time: f32,
    /// The model at rest: its lowest and highest corner (model units, y up).
    pub bounds: (Vec2, Vec2),
    /// Across the middle of its head (the top fifth of it), where a frame centres.
    pub centre_x: f32,
}

impl Puppet {
    /// Reads a `.model3.json` and everything it names.
    pub fn load(path: &Path) -> Result<Puppet, Error> {
        let mut model = load_model_runtime(path).map_err(|e| Error::Other(e.to_string()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let expressions = model
            .runtime()
            .model()
            .expressions()
            .iter()
            .filter_map(|e| load_expression(dir.join(e.file())).ok().map(|x| (e.name().to_string(), x)))
            .collect();
        let idle: Vec<Motion3> = model
            .runtime()
            .model()
            .motions()
            .iter()
            .filter(|(group, _)| group.eq_ignore_ascii_case("idle"))
            .flat_map(|(_, list)| list.iter())
            .filter_map(|m| load_motion(dir.join(m.file())).ok())
            .collect();
        let rt = model.runtime_mut();
        let find = |id: &str| rt.parameter_index(id);
        let group = |name: &str| -> Vec<usize> {
            rt.model()
                .groups()
                .iter()
                .filter(|g| g.name() == name && g.target() == "Parameter")
                .flat_map(|g| g.ids().iter().filter_map(|id| rt.parameter_index(id)))
                .collect()
        };
        let vowels = ["ParamA", "ParamI", "ParamU", "ParamE", "ParamO"].map(find);
        let mut mouth_open = group("LipSync");
        if mouth_open.is_empty() {
            mouth_open.extend(find("ParamMouthOpenY"));
        }
        // (a model that speaks in vowels opens its mouth with them)
        if vowels.iter().any(Option::is_some) {
            mouth_open.retain(|i| !vowels.contains(&Some(*i)));
        }
        let blink = group("EyeBlink");
        let eye = |id: &str| -> Vec<usize> {
            let mut v: Vec<usize> = find(id).into_iter().collect();
            if v.is_empty() {
                v = blink
                    .iter()
                    .copied()
                    .filter(|&i| rt.parameter_ids()[i].contains(if id.contains('L') { "L" } else { "R" }))
                    .collect();
            }
            v
        };
        let params = Params {
            angle: ["ParamAngleX", "ParamAngleY", "ParamAngleZ"].map(find),
            body: ["ParamBodyAngleX", "ParamBodyAngleY", "ParamBodyAngleZ"].map(find),
            // the model's right, left
            eye_open: [eye("ParamEyeROpen"), eye("ParamEyeLOpen")],
            eye_ball: ["ParamEyeBallX", "ParamEyeBallY"].map(find),
            eye_smile: ["ParamEyeRSmile", "ParamEyeLSmile"].map(find),
            brow: ["ParamBrowRY", "ParamBrowLY"].map(find),
            mouth_open,
            mouth_form: find("ParamMouthForm"),
            vowels,
            breath: find("ParamBreath"),
            cheek: find("ParamCheek"),
        };
        rt.reset_parameters();
        rt.stabilize_physics();
        rt.update_meshes();
        let bounds = visible_bounds(rt.meshes());
        let band = bounds.1.y - (bounds.1.y - bounds.0.y) * 0.2;
        let (mut sum, mut n) = (0.0, 0usize);
        for m in rt.meshes().iter().filter(|m| m.opacity() > 0.01) {
            for v in m.vertices().iter().filter(|v| v.position()[1] >= band) {
                sum += v.position()[0];
                n += 1;
            }
        }
        let centre_x = if n > 0 { sum / n as f32 } else { (bounds.0.x + bounds.1.x) * 0.5 };
        Ok(Puppet {
            model,
            params,
            expressions,
            manager: ExpressionManager::new(),
            playing: None,
            idle,
            idle_player: None,
            time: 0.0,
            bounds,
            centre_x,
        })
    }

    pub fn textures(&self) -> &[DecodedTexture] {
        self.model.textures()
    }

    pub fn meshes(&self) -> &[Moc3DrawableMesh] {
        self.model.runtime().meshes()
    }

    /// The meshes' indices in the order they're drawn (back to front).
    pub fn draw_order(&self) -> Vec<usize> {
        let infos: Vec<DrawableInfo> = self.meshes().iter().map(DrawableInfo::from_mesh).collect();
        draw_order_indices(&infos)
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

    /// How many Idle motions the model has (played under the face).
    pub fn idle_motions(&self) -> usize {
        self.idle.len()
    }

    /// The model's expressions, by name.
    pub fn expression_names(&self) -> Vec<String> {
        self.expressions.iter().map(|(n, _)| n.clone()).collect()
    }

    /// Poses the model for this frame: the face (mirrored first if wanted), an expression by
    /// name (or none), `dt` seconds after the last.
    pub fn update(&mut self, face: &Face, expression: Option<&str>, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        if self.playing.as_deref() != expression {
            self.manager.stop_all();
            if let Some(e) = expression.and_then(|name| self.expressions.iter().find(|(n, _)| n == name)) {
                self.manager.play(e.1.clone());
            }
            self.playing = expression.map(str::to_string);
        }
        let p = self.params.clone();
        let (yaw, pitch, roll) = face.angles();
        // the next Idle motion when one ends
        if !self.idle.is_empty() && self.idle_player.as_ref().is_none_or(|(_, m)| m.is_finished()) {
            let next = self.idle_player.as_ref().map_or(0, |(i, _)| (i + 1) % self.idle.len());
            self.idle_player = Some((next, MotionPlayer::new_once(self.idle[next].clone())));
        }
        let rt = self.model.runtime_mut();
        rt.reset_parameters();
        if let Some((_, m)) = self.idle_player.as_mut() {
            m.tick(dt);
            m.apply(rt);
        }
        let mut set = |i: Option<usize>, v: f32| {
            if let Some(i) = i {
                rt.set_parameter_by_index(i, v);
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
        let _ = p.cheek;
        self.manager.tick(dt);
        self.manager.apply(rt);
        rt.apply_physics(dt);
        rt.apply_pose(dt);
        rt.update_meshes();
    }
}

fn visible_bounds(meshes: &[Moc3DrawableMesh]) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for m in meshes.iter().filter(|m| m.opacity() > 0.01) {
        for v in m.vertices() {
            let p = Vec2::from(v.position());
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    if lo.x > hi.x { (Vec2::splat(-1.0), Vec2::splat(1.0)) } else { (lo, hi) }
}
