/*!
Inochi2D 0.8's MeshGroups: a mesh that isn't drawn but deforms the nodes under it, ported from
the reference implementation (inochi2d v0.8.7, `core/nodes/meshgroup`).

A group's mesh is deformed by params like a Part's. Each frame, every triangle's affine map from
its rest shape to its deformed shape is found, and the group's nodes follow it:

- a drawn node (a Part, or another MeshGroup) moves its vertices: in a *dynamic* group a vertex
  goes where the triangle under it (as deformed) takes it, and the node is then drawn with the
  group's transform; otherwise each vertex moves by as much as the group's mesh moved where the
  vertex rests.
- other nodes move their translation the same way, when the group translates children.

Which group a node follows is decided once: walking down from each group's children, through
Parts (not into other groups) and through Composites that propagate MeshGroups.
*/

use glam::{Mat3, Mat4, Vec2, Vec3Swizzles};

use crate::node::components::{Composite, Drawable, Mesh, MeshGroup, TexturedMesh, TransformStore};
use crate::node::InoxNodeUuid;
use crate::puppet::{InoxNodeTree, World};

/// Which group deforms a node, and how.
pub(crate) struct GroupFilter {
	pub group: InoxNodeUuid,
	/// A drawn node (a Part or a MeshGroup): its vertices move; else its translation does.
	pub drawable: bool,
}

/// A group's state.
#[derive(Default)]
pub(crate) struct MeshGroupCtx {
	dynamic: bool,
	/// Each triangle at rest, and the inverse of its affine frame (None when degenerate).
	rest: Vec<[Vec2; 3]>,
	offsets: Vec<Option<Mat3>>,
	/// Each triangle's map from rest to deformed, this frame.
	transforms: Vec<Mat3>,
	/// The rest mesh's bounds, whole units.
	lo: Vec2,
	hi: Vec2,
	/// The group's deformation this frame.
	deform: Vec<Vec2>,
	forward: Mat4,
	inverse: Mat4,
}

fn frame(p1: Vec2, p2: Vec2, p3: Vec2) -> Mat3 {
	Mat3::from_cols((p2 - p1).extend(0.0), (p3 - p1).extend(0.0), p1.extend(1.0))
}

/// As the reference's `isPointInTriangle` (edges count).
fn in_triangle(pt: Vec2, t: &[Vec2; 3]) -> bool {
	let sign = |p1: Vec2, p2: Vec2, p3: Vec2| (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y);
	let d1 = sign(pt, t[0], t[1]);
	let d2 = sign(pt, t[1], t[2]);
	let d3 = sign(pt, t[2], t[0]);
	let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
	let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
	!(neg && pos)
}

impl MeshGroupCtx {
	pub(crate) fn new(mesh: &Mesh, dynamic: bool) -> Self {
		let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
		for v in &mesh.vertices {
			lo = lo.min(*v);
			hi = hi.max(*v);
		}
		let mut rest = Vec::new();
		let mut offsets = Vec::new();
		for t in mesh.indices.chunks_exact(3) {
			let corner = |i: u32| mesh.vertices.get(i as usize).copied().unwrap_or_default();
			let tri = [corner(t[0]), corner(t[1]), corner(t[2])];
			let m = frame(tri[0], tri[1], tri[2]);
			offsets.push((m.determinant().abs() > 1e-12).then(|| m.inverse()));
			rest.push(tri);
		}
		let n = rest.len();
		Self {
			dynamic,
			rest,
			offsets,
			transforms: vec![Mat3::IDENTITY; n],
			lo: lo.floor(),
			hi: hi.ceil(),
			deform: vec![Vec2::ZERO; mesh.vertices.len()],
			forward: Mat4::IDENTITY,
			inverse: Mat4::IDENTITY,
		}
	}

	/// The triangle a point (group space) falls in: the reference looks it up on whole units,
	/// the last triangle covering the unit winning.
	fn lookup(&self, c: Vec2) -> Option<usize> {
		if !(self.lo.x <= c.x && c.x < self.hi.x && self.lo.y <= c.y && c.y < self.hi.y) {
			return None;
		}
		let pt = c.floor();
		(0..self.rest.len())
			.rev()
			.find(|&i| self.offsets[i].is_some() && in_triangle(pt, &self.rest[i]))
	}

	fn map(&self, c: Vec2) -> Vec2 {
		match self.lookup(c) {
			Some(i) => (self.transforms[i] * c.extend(1.0)).xy(),
			None => c,
		}
	}

	/// This frame's triangles, from the mesh and its deformation (`deform`, kept).
	pub(crate) fn update_with(&mut self, mesh: &Mesh, deform: &[Vec2], absolute: Mat4) {
		self.deform.clear();
		self.deform.extend_from_slice(deform);
		self.deform.resize(mesh.vertices.len(), Vec2::ZERO);
		self.update(mesh, absolute);
	}

	/// nijilive's MeshGroup filter (`filterChildren`): moves a node's points (`vertices` in its
	/// own space, `deform` their deformation, `absolute` the node's transform) by as much as the
	/// group's mesh moved where they are (dynamic: with their deformation; else at rest).
	/// Whether it changed anything.
	pub(crate) fn filter_nijilive(&self, vertices: &[Vec2], deform: &mut [Vec2], absolute: Mat4) -> bool {
		let center = self.inverse * absolute;
		let to_local = Mat4::from_mat3(Mat3::from_mat4(center).inverse());
		let mut changed = false;
		for (v, d) in vertices.iter().zip(deform.iter_mut()) {
			let c = if self.dynamic {
				center.transform_point3((*v + *d).extend(0.0)).xy()
			} else {
				center.transform_point3(v.extend(0.0)).xy()
			};
			let mapped = self.map(c);
			if mapped != c {
				changed = true;
				*d += to_local.transform_vector3((mapped - c).extend(0.0)).xy();
			}
		}
		changed
	}

	/// This frame's triangles, from the mesh and its deformation.
	fn update(&mut self, mesh: &Mesh, absolute: Mat4) {
		for (i, t) in mesh.indices.chunks_exact(3).enumerate() {
			let corner = |k: u32| {
				let k = k as usize;
				mesh.vertices.get(k).copied().unwrap_or_default() + self.deform.get(k).copied().unwrap_or_default()
			};
			self.transforms[i] = match self.offsets[i] {
				Some(offset) => frame(corner(t[0]), corner(t[1]), corner(t[2])) * offset,
				None => Mat3::IDENTITY,
			};
		}
		self.forward = absolute;
		self.inverse = absolute.inverse();
	}

	/// Moves a node's points (`vertices` in its own space, `deform` their deformation, `absolute`
	/// the node's transform). For a dynamic group, the transform to draw them with instead.
	fn filter(&self, vertices: &[Vec2], deform: &mut [Vec2], absolute: Mat4) -> Option<Mat4> {
		let center = self.inverse * absolute;
		let to_local = Mat4::from_mat3(Mat3::from_mat4(center).inverse());
		for (v, d) in vertices.iter().zip(deform.iter_mut()) {
			if self.dynamic {
				let c = center.transform_point3((*v + *d).extend(0.0)).xy();
				*d = self.map(c) - *v;
			} else {
				let c = center.transform_point3(v.extend(0.0)).xy();
				*d += to_local.transform_vector3((self.map(c) - c).extend(0.0)).xy();
			}
		}
		self.dynamic.then_some(self.forward)
	}
}

fn is_drawable(id: InoxNodeUuid, comps: &World) -> bool {
	comps.get::<TexturedMesh>(id).is_some() || comps.get::<MeshGroup>(id).is_some()
}

/// Gives groups their state and every node it deforms its filter. (Needs Meshes and transforms.)
pub(crate) fn init(nodes: &InoxNodeTree, comps: &mut World) {
	let groups: Vec<InoxNodeUuid> = nodes
		.pre_order_iter()
		.filter(|n| comps.get::<MeshGroup>(n.uuid).is_some())
		.filter(|n| comps.get::<Mesh>(n.uuid).is_some_and(|m| !m.indices.is_empty()))
		.map(|n| n.uuid)
		.collect();
	for g in groups {
		let (dynamic, translate) = {
			let mg = comps.get::<MeshGroup>(g).unwrap();
			(mg.dynamic, mg.translate_children)
		};
		let ctx = MeshGroupCtx::new(comps.get::<Mesh>(g).unwrap(), dynamic);
		comps.add(g, ctx);
		let mut stack: Vec<InoxNodeUuid> = nodes.get_children(g).map(|n| n.uuid).collect();
		while let Some(id) = stack.pop() {
			let drawable = is_drawable(id, comps);
			let group = comps.get::<MeshGroup>(id).is_some();
			let composite = comps.get::<Composite>(id).is_some_and(|c| c.propagate_meshgroup);
			// (a composite that propagates takes no filter itself)
			if (translate || drawable) && !composite {
				let filter = GroupFilter { group: g, drawable };
				match comps.get_mut::<GroupFilter>(id) {
					Some(f) => *f = filter,
					None => comps.add(id, filter),
				}
			}
			if (drawable && !group) || composite {
				stack.extend(nodes.get_children(id).map(|n| n.uuid));
			}
		}
	}
}

/// While transforms are found, before a node's own: a node that isn't drawn moves its
/// translation (`parent` its parent's transform).
pub(crate) fn before_transform(node: &crate::node::InoxNode, parent: Mat4, comps: &mut World) {
	let Some(f) = comps.get::<GroupFilter>(node.uuid) else {
		return;
	};
	if f.drawable {
		return;
	}
	let group = f.group;
	let Some(ctx) = comps.get::<MeshGroupCtx>(group) else {
		return;
	};
	let rest = node.trans_offset.translation.truncate();
	let current = comps
		.get::<TransformStore>(node.uuid)
		.unwrap()
		.relative
		.translation
		.truncate();
	// (the reference keeps a node's own translation and its params' offset apart)
	let mut offset = [current - rest];
	ctx.filter(&[rest], &mut offset, parent);
	let t = &mut comps.get_mut::<TransformStore>(node.uuid).unwrap().relative.translation;
	t.x = rest.x + offset[0].x;
	t.y = rest.y + offset[0].y;
}

/// While transforms are found, after a node's own: a group finds its triangles for the frame.
/// `deform` combines its params' deformation into a buffer.
pub(crate) fn after_transform(node: InoxNodeUuid, comps: &mut World, deform: impl FnOnce(&World, &mut [Vec2]) -> bool) {
	let Some(mut ctx) = comps.get_mut::<MeshGroupCtx>(node).map(std::mem::take) else {
		return;
	};
	if !deform(comps, &mut ctx.deform) {
		ctx.deform.iter_mut().for_each(|d| *d = Vec2::ZERO);
	}
	let absolute = comps.get::<TransformStore>(node).unwrap().absolute;
	// moved by a group above it (the reference only does this for groups that aren't dynamic)
	if let Some(f) = comps.get::<GroupFilter>(node) {
		if let (Some(outer), Some(mesh)) = (comps.get::<MeshGroupCtx>(f.group), comps.get::<Mesh>(node)) {
			if !outer.dynamic {
				outer.filter(&mesh.vertices, &mut ctx.deform, absolute);
			}
		}
	}
	if let Some(mesh) = comps.get::<Mesh>(node) {
		ctx.update(mesh, absolute);
	}
	*comps.get_mut::<MeshGroupCtx>(node).unwrap() = ctx;
}

/// Once deformations are combined: a drawn Part in a group moves its vertices (`deform`, written
/// in place) and, in a dynamic group, is drawn with the group's transform.
pub(crate) fn filter_part(node: InoxNodeUuid, comps: &mut World, deform: &mut [Vec2]) {
	let Some(f) = comps.get::<GroupFilter>(node) else {
		return;
	};
	let (Some(ctx), Some(mesh), Some(store)) = (
		comps.get::<MeshGroupCtx>(f.group),
		comps.get::<Mesh>(node),
		comps.get::<TransformStore>(node),
	) else {
		return;
	};
	if comps.get::<Drawable>(node).is_none() {
		return;
	}
	if let Some(m) = ctx.filter(&mesh.vertices, deform, store.absolute) {
		comps.get_mut::<TransformStore>(node).unwrap().absolute = m;
	}
}

/// Whether a node is deformed by a group.
pub(crate) fn has_filter(node: InoxNodeUuid, comps: &World) -> bool {
	comps.get::<GroupFilter>(node).is_some_and(|f| f.drawable)
}
