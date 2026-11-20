/*!
nijilive's PathDeformer: a curve through control points, deformed by params (deform bindings on
its control points), by the deformers above it and by its physics driver, that bends the drawn
nodes under it. Ported from nijilive (`core/nodes/deformer/path.d`,
`core/nodes/deformer/drivers/phys.d`) and its C++ port nicxlive (`core/nodes/path_deformer.cpp`,
`core/nodes/deformer/drivers/phys.cpp`); both BSD-2-Clause, © 2020 Inochi2D Project, © 2024
nijigenerate Project.

Each vertex of a node it deforms is tied to the point of the curve closest to where the vertex
rests; each frame the vertex keeps its distance along and across the curve there, as the curve
moves.
*/

use std::collections::HashMap;

use glam::{Mat3, Mat4, Vec2, Vec3Swizzles};

use crate::node::components::{CurveType, PathDeformer, PathPhysics};
use crate::node::InoxNodeUuid;

use super::curve::Curve;

fn finite(v: Vec2) -> bool {
	v.x.is_finite() && v.y.is_finite()
}

fn sanitize(values: &mut [Vec2]) {
	for v in values.iter_mut().filter(|v| !finite(**v)) {
		*v = Vec2::ZERO;
	}
}

fn linear(m: Mat4) -> Mat3 {
	Mat3::from_mat4(m)
}

/// Adds the linear part of `m` applied to `v`.
fn transform_vector(m: Mat4, v: Vec2) -> Vec2 {
	(linear(m) * v.extend(0.0)).xy()
}

/* --- PHYSICS DRIVERS --- */

/// Physics y goes up, the puppet's down.
fn to_physics(v: Vec2) -> Vec2 {
	Vec2::new(v.x, -v.y)
}

/// A chain of pendulums hanging from the first control point.
struct Pendulum {
	damping: f32,
	restore: f32,
	gravity: f32,
	input_scale: f32,
	propagate_scale: f32,
	angles: Vec<f32>,
	initial_angles: Vec<f32>,
	velocities: Vec<f32>,
	lengths: Vec<f32>,
	base: Vec2,
	external: Vec2,
	world_angle: f32,
}

impl Pendulum {
	/// The rest shape's angles and lengths (physics space); None when a segment has no length.
	fn shape(original: &[Vec2]) -> Option<(Vec<f32>, Vec<f32>)> {
		let mut angles = Vec::new();
		let mut lengths = Vec::new();
		for w in original.windows(2) {
			let v = to_physics(w[1]) - to_physics(w[0]);
			let len = v.length();
			if !len.is_finite() || len <= 1e-6 {
				return None;
			}
			angles.push(v.x.atan2(-v.y));
			lengths.push(len);
		}
		Some((angles, lengths))
	}

	/// Starts at rest. False when the curve can't hang (a segment has no length).
	fn setup(&mut self, original: &[Vec2]) -> bool {
		self.external = Vec2::ZERO;
		if original.len() < 2 {
			return false;
		}
		let Some((angles, lengths)) = Self::shape(original) else {
			return false;
		};
		self.initial_angles = angles.clone();
		self.angles = angles;
		self.lengths = lengths;
		self.velocities = vec![0.0; self.angles.len()];
		self.base = to_physics(original[0]);
		true
	}

	/// One step; false when it degenerates.
	fn step(&mut self, h: f32) -> bool {
		if self.lengths.is_empty() {
			return false;
		}
		let mut torque = self.external.x * h * self.lengths[0];
		for i in 0..self.angles.len() {
			let len = self.lengths[i];
			if !len.is_finite() || len <= 1e-6 {
				return false;
			}
			let restore = -(1.0 / h).min(self.restore) * (self.angles[i] - self.initial_angles[i]);
			let damping = -self.damping * self.velocities[i];
			let gravity = -self.gravity * (self.angles[i] + self.world_angle).sin();
			let acceleration = restore + damping + gravity + torque;
			if !acceleration.is_finite() {
				return false;
			}
			self.velocities[i] += acceleration * h;
			self.angles[i] += self.velocities[i] * h;
			if !self.velocities[i].is_finite() || !self.angles[i].is_finite() {
				return false;
			}
			torque += (-acceleration * self.angles[i].sin()) * h * len * self.propagate_scale;
		}
		true
	}

	/// Runs `dt` seconds and adds how far the chain is from rest to `deform`.
	fn update(&mut self, original: &[Vec2], deform: &mut [Vec2], dt: f32) -> bool {
		let mut h = dt.min(10.0);
		while h > 0.01 {
			if !self.step(0.01) {
				return false;
			}
			h -= 0.01;
		}
		if !self.step(h) {
			return false;
		}
		let mut p = self.base;
		for i in 0..original.len().min(deform.len()) {
			if i > 0 {
				let a = self.angles[i - 1];
				p.x += self.lengths[i - 1] * a.sin();
				p.y -= self.lengths[i - 1] * a.cos();
			}
			let delta = to_physics(p) - original[i];
			if finite(delta) {
				deform[i] += delta;
			}
		}
		true
	}
}

/// Masses on springs, from the first control point.
struct SpringPendulum {
	damping: f32,
	spring: f32,
	restoration: f32,
	gravity: Vec2,
	positions: Vec<Vec2>,
	velocities: Vec<Vec2>,
	initial: Vec<Vec2>,
	lengths: Vec<f32>,
	external: Vec2,
}

impl SpringPendulum {
	fn setup(&mut self, original: &[Vec2]) -> bool {
		self.external = Vec2::ZERO;
		self.initial = original.iter().map(|p| to_physics(*p)).collect();
		self.positions = self.initial.clone();
		self.velocities = vec![Vec2::ZERO; self.positions.len()];
		self.lengths = self.positions.windows(2).map(|w| (w[1] - w[0]).length()).collect();
		!self.lengths.is_empty() && self.lengths.iter().all(|l| l.is_finite() && *l > 1e-6)
	}

	fn step(&mut self, h: f32) -> bool {
		let n = self.positions.len();
		for i in 1..n {
			let mut force = Vec2::ZERO;
			let prev = self.positions[i - 1] + if i == 1 { self.external * h } else { Vec2::ZERO };
			let diff = self.positions[i] - prev;
			let len = diff.length();
			if !len.is_finite() || len <= 1e-6 {
				return false;
			}
			force += -self.spring * (diff * (len - self.lengths[i - 1]) / len);
			if i < n - 1 {
				let diff = self.positions[i] - self.positions[i + 1];
				let len = diff.length();
				if !len.is_finite() || len <= 1e-6 {
					return false;
				}
				force += -self.spring * (diff * (len - self.lengths[i]) / len);
			}
			let restoration = -self.restoration * (self.positions[i] - self.initial[i]);
			let damping = -self.damping * self.velocities[i];
			let acceleration = force + damping + restoration + self.gravity;
			if !finite(acceleration) {
				return false;
			}
			self.velocities[i] += acceleration * h;
			self.positions[i] += self.velocities[i] * h;
			if !finite(self.positions[i]) {
				return false;
			}
		}
		true
	}

	fn update(&mut self, original: &[Vec2], deform: &mut [Vec2], dt: f32) -> bool {
		let mut h = dt.min(10.0);
		while h > 0.01 {
			if !self.step(0.01) {
				return false;
			}
			h -= 0.01;
		}
		if !self.step(h) {
			return false;
		}
		for (i, p) in self.positions.iter().enumerate().take(original.len().min(deform.len())) {
			let delta = to_physics(*p) - original[i];
			if finite(delta) {
				deform[i] += delta;
			}
		}
		true
	}
}

enum Driver {
	Pendulum(Pendulum),
	Spring(SpringPendulum),
}

impl Driver {
	fn new(physics: &PathPhysics) -> Self {
		match *physics {
			PathPhysics::Pendulum {
				damping,
				restore_constant,
				gravity,
				input_scale,
				propagate_scale,
			} => Driver::Pendulum(Pendulum {
				damping,
				restore: restore_constant,
				gravity,
				input_scale,
				propagate_scale,
				angles: Vec::new(),
				initial_angles: Vec::new(),
				velocities: Vec::new(),
				lengths: Vec::new(),
				base: Vec2::ZERO,
				external: Vec2::ZERO,
				world_angle: 0.0,
			}),
			// (nijilive saves none of a spring pendulum's settings: its defaults)
			PathPhysics::SpringPendulum => Driver::Spring(SpringPendulum {
				damping: 0.3,
				spring: 10.0,
				restoration: 0.0,
				gravity: Vec2::new(0.0, 9.8),
				positions: Vec::new(),
				velocities: Vec::new(),
				initial: Vec::new(),
				lengths: Vec::new(),
				external: Vec2::ZERO,
			}),
		}
	}

	fn setup(&mut self, original: &[Vec2]) -> bool {
		match self {
			Driver::Pendulum(p) => p.setup(original),
			Driver::Spring(s) => s.setup(original),
		}
	}

	/// The deformer's root moved by `force` (world units) since the last frame, and is turned by
	/// `angle` (radians).
	fn enforce(&mut self, force: Vec2, angle: f32) {
		match self {
			Driver::Pendulum(p) => {
				if finite(force * p.input_scale) {
					p.external = force * p.input_scale;
				}
				if angle.is_finite() {
					p.world_angle = -angle;
				}
			}
			Driver::Spring(s) => {
				if finite(force) {
					s.external = force;
				}
			}
		}
	}

	fn update(&mut self, original: &[Vec2], deform: &mut [Vec2], dt: f32) -> bool {
		match self {
			Driver::Pendulum(p) => p.update(original, deform, dt),
			Driver::Spring(s) => s.update(original, deform, dt),
		}
	}
}

/* --- THE DEFORMER --- */

/// A PathDeformer's state.
pub(crate) struct PathCtx {
	curve_type: CurveType,
	original: Curve,
	/// What the nodes' vertices are measured against: the rest curve, or (with physics) the rest
	/// curve as the deformers above moved it this frame.
	prev: Curve,
	deformed: Curve,
	/// The inverse of the deformer's transform this frame.
	inverse: Mat4,
	driver: Option<Driver>,
	driver_ready: bool,
	prev_root: Option<Vec2>,
	/// Each deformed node's vertices' places on the curve (found at rest; without physics only).
	cache: HashMap<InoxNodeUuid, Vec<f32>>,
}

impl PathCtx {
	pub fn new(def: &PathDeformer, points: &[Vec2]) -> Self {
		let original = Curve::new(def.curve, points.to_vec());
		Self {
			curve_type: def.curve,
			prev: original.clone(),
			deformed: original.clone(),
			original,
			inverse: Mat4::IDENTITY,
			driver: def.physics.as_ref().map(Driver::new),
			driver_ready: false,
			prev_root: None,
			cache: HashMap::new(),
		}
	}

	pub fn has_physics(&self) -> bool {
		self.driver.is_some()
	}

	/// Where a node's vertices (`vertices`, its transform `absolute`) rest on the curve.
	pub fn closest(&self, vertices: &[Vec2], absolute: Mat4) -> Vec<f32> {
		let to_curve = self.inverse * absolute;
		vertices
			.iter()
			.map(|v| self.prev.closest_point(to_curve.transform_point3(v.extend(0.0)).xy()))
			.collect()
	}

	/// Remembers where a node's vertices rest on the curve (`absolute` the node's transform at
	/// rest, `rest` the deformer's).
	pub fn cache_rest(&mut self, node: InoxNodeUuid, vertices: &[Vec2], absolute: Mat4, rest: Mat4) {
		if self.driver.is_some() {
			return;
		}
		self.inverse = rest.inverse();
		let t = self.closest(vertices, absolute);
		self.cache.insert(node, t);
	}

	/// This frame's curve (the reference's `applyPathDeform`): `deform` the control points'
	/// deformation (params, then the deformers above), `orig` it before those deformers,
	/// `absolute` the deformer's transform and `angle` its rotation; physics run `dt` seconds.
	pub fn update(&mut self, deform: &mut [Vec2], orig: &[Vec2], absolute: Mat4, angle: f32, dt: f32) {
		self.inverse = absolute.inverse();
		sanitize(deform);
		let rest = self.original.points().to_vec();
		let n = rest.len();
		if self.driver.is_some() && n >= 2 && deform.len() == n {
			let baseline: Vec<Vec2> = if orig.len() == n {
				orig.to_vec()
			} else {
				deform.to_vec()
			};
			let driver = self.driver.as_mut().unwrap();
			let mut ok = true;
			if !self.driver_ready {
				ok = driver.setup(&rest);
				self.driver_ready = true;
			}
			// the deformers above move the curve the vertices are measured against
			let moved: Vec<Vec2> = (0..n)
				.map(|i| {
					let d = deform[i] - baseline[i];
					rest[i] + if finite(d) { d } else { Vec2::ZERO }
				})
				.collect();
			self.prev = Curve::new(self.curve_type, moved);
			let root = absolute.transform_point3((rest[0] + deform[0]).extend(0.0)).xy();
			let root = if finite(root) { root } else { Vec2::ZERO };
			if ok {
				if let Some(prev) = self.prev_root {
					driver.enforce(root - prev, angle);
					ok = driver.update(&rest, deform, dt);
				}
			}
			self.prev_root = Some(root);
			if !ok {
				// (the reference stops a driver that degenerates)
				self.driver = None;
				self.prev = self.original.clone();
			}
		}
		if n >= 2 && deform.len() == n {
			let mut points: Vec<Vec2> = rest.iter().zip(deform.iter()).map(|(p, d)| *p + *d).collect();
			sanitize(&mut points);
			self.deformed = Curve::new(self.curve_type, points);
		}
	}

	/// Bends a node's vertices (`vertices` in its space, `deform` their deformation so far,
	/// `absolute` its transform) the way the curve bent (the reference's `deformChildren`).
	pub fn filter(&self, node: InoxNodeUuid, vertices: &[Vec2], deform: &mut [Vec2], absolute: Mat4) -> bool {
		if self.original.points().len() < 2 || deform.len() < vertices.len() {
			return false;
		}
		let center = self.inverse * absolute;
		if !center.is_finite() {
			return false;
		}
		sanitize(deform);
		let centered: Vec<Vec2> = vertices
			.iter()
			.zip(deform.iter())
			.map(|(v, d)| center.transform_point3(v.extend(0.0)).xy() + transform_vector(center, *d))
			.collect();
		if !centered.iter().all(|c| finite(*c)) {
			return false;
		}
		let computed;
		let ts: &[f32] = match self.cache.get(&node) {
			Some(t) if t.len() >= vertices.len() && self.driver.is_none() => t,
			_ => {
				computed = self.closest(vertices, absolute);
				&computed
			}
		};
		let inverse = center.inverse();
		let to_local = Mat4::from_mat3(linear(inverse));
		for i in 0..vertices.len() {
			let t = ts[i];
			let on_rest = self.prev.point(t);
			let on_deformed = self.deformed.point(t);
			if !finite(on_deformed) {
				return false;
			}
			let normalize = |v: Vec2, fallback: Vec2| {
				let len2 = v.length_squared();
				if len2 > 1e-8 {
					v / len2.sqrt()
				} else {
					fallback
				}
			};
			let tangent_rest = normalize(self.prev.derivative(t), Vec2::X);
			let tangent_deformed = normalize(self.deformed.derivative(t), tangent_rest);
			let normal_rest = Vec2::new(-tangent_rest.y, tangent_rest.x);
			let normal_deformed = Vec2::new(-tangent_deformed.y, tangent_deformed.x);
			let rel = centered[i] - on_rest;
			let (across, along) = (rel.dot(normal_rest), rel.dot(tangent_rest));
			let moved = on_deformed + normal_deformed * across + tangent_deformed * along;
			let offset = to_local.transform_vector3((moved - centered[i]).extend(0.0)).xy();
			if finite(offset) {
				deform[i] += offset;
			}
		}
		true
	}
}
