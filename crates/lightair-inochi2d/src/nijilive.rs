/*!
nijilive's deformers (nijigenerate's puppets): MeshGroups and PathDeformers bend the nodes under
them through filters, as nijilive (`core/nodes/node.d`, `deformable.d`, `meshgroup`,
`deformer/path.d`, `drawable.d` for welding) and its C++ port nicxlive do; both BSD-2-Clause,
© 2020 Inochi2D Project, © 2024 nijigenerate Project.

Which nodes a deformer reaches is decided once: walking down from its children, a node takes its
filter (a MeshGroup's: drawn nodes and, when it translates children, plain ones; a PathDeformer's:
drawn nodes only) and the walk goes on below Parts, Masks and PathDeformers, but not below plain
nodes, MeshGroups or DynamicComposites. A node reached by several deformers takes the outer one's
filter first.

Each frame, nodes are visited parents first: a plain node's translation is filtered, then its
transform found; a node with a mesh (or control points) has its params' deformation filtered,
then a deformer finds its own state (a MeshGroup its triangles, a PathDeformer its curve and
physics) for the nodes below. Dynamic filters run after every node's own, then welded vertices
are joined.
*/

mod curve;
mod path;

use std::collections::{HashMap, HashSet};

use glam::{Mat3, Mat4, Vec2, Vec3Swizzles};

use crate::meshgroup::MeshGroupCtx;
use crate::node::components::{
	Composite, DeformStack, Drawable, DynamicComposite, Mesh, MeshGroup, PathDeformer, SimplePhysics, TexturedMesh,
	TransformStore, Welding,
};
use crate::node::InoxNodeUuid;
use crate::puppet::{InoxNodeTree, World};
use crate::render::{TexturedMeshRenderCtx, VertexBuffers};

pub(crate) use path::PathCtx;

/// The deformers that bend a node, outer first.
#[derive(Default)]
struct Filters {
	/// Before the node's own work.
	pre: Vec<InoxNodeUuid>,
	/// Dynamic ones, once every node's deformation is found: (stage, deformer).
	post: Vec<(u8, InoxNodeUuid)>,
}

/// The order welded nodes are joined in (nijilive's tasks': parents first, children by zsort).
struct WeldOrder(Vec<InoxNodeUuid>);

/// A SimplePhysics node's transform as its physics sees it: where the deformers above put it in
/// the last frame (nijilive takes its anchor after its filters).
pub(crate) struct PhysicsAnchor(pub TransformStore);

fn is_path(id: InoxNodeUuid, comps: &World) -> bool {
	comps.get::<PathDeformer>(id).is_some()
}

/// A node deformed through its mesh (or control points): Parts, Masks, MeshGroups,
/// DynamicComposites, PathDeformers.
fn deformable(id: InoxNodeUuid, comps: &World) -> bool {
	comps.get::<Mesh>(id).is_some()
}

/// A drawn node in nijilive's terms (a PathDeformer isn't).
fn drawable(id: InoxNodeUuid, comps: &World) -> bool {
	deformable(id, comps) && !is_path(id, comps)
}

/// Whether a deformer's reach goes on below a node (nijilive's `mustPropagate`): Parts and
/// Masks, PathDeformers.
fn propagates(id: InoxNodeUuid, comps: &World) -> bool {
	comps.get::<TexturedMesh>(id).is_some() || is_path(id, comps)
}

fn linear(m: Mat4) -> Mat4 {
	Mat4::from_mat3(Mat3::from_mat4(m))
}

/// Every node's transform at rest.
fn rest_transforms(nodes: &InoxNodeTree) -> HashMap<InoxNodeUuid, Mat4> {
	let mut abs = HashMap::new();
	let root = nodes.root_node_id;
	for node in nodes.pre_order_iter() {
		let m = node.trans_offset.to_matrix();
		let base = if node.uuid == root {
			Mat4::IDENTITY
		} else if node.lock_to_root {
			abs[&root]
		} else {
			abs[&nodes.get_parent(node.uuid).uuid]
		};
		abs.insert(node.uuid, base * m);
	}
	abs
}

/// Gives deformers their state and nodes their filters; DynamicComposites their texture.
/// (Needs Meshes and the render context's buffers.)
pub(crate) fn init(nodes: &InoxNodeTree, comps: &mut World) {
	let rest = rest_transforms(nodes);
	let order: Vec<InoxNodeUuid> = nodes.pre_order_iter().map(|n| n.uuid).collect();

	for node in nodes.pre_order_iter() {
		if comps.get::<SimplePhysics>(node.uuid).is_some() {
			let store = TransformStore {
				absolute: rest[&node.uuid],
				relative: node.trans_offset.clone(),
			};
			comps.add(node.uuid, PhysicsAnchor(store));
		}
	}
	for &id in &order {
		if let (Some(group), Some(mesh)) = (comps.get::<MeshGroup>(id), comps.get::<Mesh>(id)) {
			if !mesh.indices.is_empty() {
				let ctx = MeshGroupCtx::new(mesh, group.dynamic);
				comps.add(id, ctx);
			}
		}
		if let (Some(def), Some(mesh)) = (comps.get::<PathDeformer>(id), comps.get::<Mesh>(id)) {
			let ctx = PathCtx::new(def, &mesh.vertices);
			comps.add(id, ctx);
		}
	}

	// filters, outer deformers first
	let mut filters: HashMap<InoxNodeUuid, Filters> = HashMap::new();
	for &d in &order {
		let group = comps.get::<MeshGroupCtx>(d).is_some();
		let path = comps.get::<PathCtx>(d).is_some();
		if !group && !path {
			continue;
		}
		let (dynamic, translate) = match comps.get::<MeshGroup>(d) {
			Some(g) => (g.dynamic, g.translate_children),
			None => (comps.get::<PathDeformer>(d).is_some_and(|p| p.dynamic), false),
		};
		let mut stack: Vec<InoxNodeUuid> = nodes.get_children(d).map(|n| n.uuid).collect();
		stack.reverse();
		while let Some(id) = stack.pop() {
			let takes = if group {
				let blocked = comps.get::<Composite>(id).is_some_and(|c| c.propagate_meshgroup);
				!blocked && (translate || deformable(id, comps))
			} else {
				drawable(id, comps)
			};
			if takes {
				let f = filters.entry(id).or_default();
				if dynamic && deformable(id, comps) {
					f.post.push((if group { 0 } else { 1 }, d));
				} else {
					f.pre.push(d);
				}
				if path {
					if let Some(mesh) = comps.get::<Mesh>(id) {
						let vertices = mesh.vertices.clone();
						let ctx = comps.get_mut::<PathCtx>(d).unwrap();
						ctx.cache_rest(id, &vertices, rest[&id], rest[&d]);
					}
				}
			}
			if propagates(id, comps) {
				let mut children: Vec<InoxNodeUuid> = nodes.get_children(id).map(|n| n.uuid).collect();
				children.reverse();
				stack.extend(children);
			}
		}
	}
	for (id, mut f) in filters {
		f.post.sort_by_key(|p| p.0);
		comps.add(id, f);
	}

	// DynamicComposites' textures: their mesh's bounds at rest (nijilive's `initTarget`)
	for &id in &order {
		if comps.get::<DynamicComposite>(id).is_none() {
			continue;
		}
		let Some(mesh) = comps.get::<Mesh>(id) else { continue };
		let m = rest[&id];
		let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
		let (mut wlo, mut whi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
		for v in &mesh.vertices {
			lo = lo.min(*v);
			hi = hi.max(*v);
			let w = m.transform_point3(v.extend(0.0)).xy();
			wlo = wlo.min(w);
			whi = whi.max(w);
		}
		if lo.x > hi.x {
			continue;
		}
		let size = hi - lo;
		let data = DynamicComposite {
			texture_offset: (wlo + whi) * 0.5 - m.w_axis.truncate().truncate(),
			texture_size: Vec2::new(size.x.ceil() + 1.0, size.y.ceil() + 1.0),
		};
		*comps.get_mut::<DynamicComposite>(id).unwrap() = data;
	}

	// welding, both ends drawn
	let mut weld = Vec::new();
	let mut stack = vec![nodes.root_node_id];
	while let Some(id) = stack.pop() {
		if comps.get::<Welding>(id).is_some() && comps.get::<TexturedMeshRenderCtx>(id).is_some() {
			weld.push(id);
		}
		let mut children: Vec<(InoxNodeUuid, f32)> = nodes.get_children(id).map(|n| (n.uuid, n.zsort)).collect();
		children.sort_by(|a, b| b.1.total_cmp(&a.1));
		stack.extend(children.iter().rev().map(|c| c.0));
	}
	comps.add(nodes.root_node_id, WeldOrder(weld));
}

/// Applies a deformer's filter to a node's points; whether it changed them.
fn apply(
	deformer: InoxNodeUuid,
	node: InoxNodeUuid,
	comps: &World,
	vertices: &[Vec2],
	deform: &mut [Vec2],
	absolute: Mat4,
) -> bool {
	if let Some(group) = comps.get::<MeshGroupCtx>(deformer) {
		// (nijilive's MeshGroups move only the PathDeformers that have physics)
		if comps.get::<PathCtx>(node).is_some_and(|p| !p.has_physics()) {
			return false;
		}
		return group.filter_nijilive(vertices, deform, absolute);
	}
	if let Some(path) = comps.get::<PathCtx>(deformer) {
		return path.filter(node, vertices, deform, absolute);
	}
	false
}

/// One frame: every node's transform and deformation (into the buffers, for drawn meshes), with
/// physics run `dt` seconds.
pub(crate) fn update(nodes: &InoxNodeTree, comps: &mut World, buffers: &mut VertexBuffers, dt: f32) {
	let root = nodes.root_node_id;
	let mut angles: HashMap<InoxNodeUuid, f32> = HashMap::new();
	let mut posted = Vec::new();
	for node in nodes.pre_order_iter() {
		let id = node.uuid;
		let (base, base_angle) = if id == root {
			(Mat4::IDENTITY, 0.0)
		} else {
			let from = if node.lock_to_root {
				root
			} else {
				nodes.get_parent(id).uuid
			};
			(
				comps.get::<TransformStore>(from).unwrap().absolute,
				angles.get(&from).copied().unwrap_or(0.0),
			)
		};
		let pre: Vec<InoxNodeUuid> = comps.get::<Filters>(id).map(|f| f.pre.clone()).unwrap_or_default();
		if comps.get::<Filters>(id).is_some_and(|f| !f.post.is_empty()) {
			posted.push(id);
		}
		let has_mesh = deformable(id, comps);

		// a plain node's translation (its rest place, and its params' offset from it)
		if !has_mesh && !pre.is_empty() {
			let rest = node.trans_offset.translation.truncate();
			let current = comps.get::<TransformStore>(id).unwrap().relative.translation.truncate();
			let mut offset = [current - rest];
			for d in &pre {
				apply(*d, id, comps, &[rest], &mut offset, base);
			}
			let t = &mut comps.get_mut::<TransformStore>(id).unwrap().relative.translation;
			t.x = rest.x + offset[0].x;
			t.y = rest.y + offset[0].y;
		}
		let store = comps.get_mut::<TransformStore>(id).unwrap();
		let absolute = if id == root {
			store.relative.to_matrix()
		} else {
			base * store.relative.to_matrix()
		};
		store.absolute = absolute;
		let angle = base_angle + store.relative.rotation.z;
		angles.insert(id, angle);
		if comps.get::<PhysicsAnchor>(id).is_some() {
			let store = comps.get::<TransformStore>(id).unwrap().clone();
			comps.get_mut::<PhysicsAnchor>(id).unwrap().0 = store;
		}
		if !has_mesh {
			continue;
		}

		// the node's deformation: its params', then its filters
		let vertices = comps.get::<Mesh>(id).unwrap().vertices.clone();
		let mut deform = vec![Vec2::ZERO; vertices.len()];
		if let Some(stack) = comps.get::<DeformStack>(id) {
			if stack.deform_len == deform.len() {
				stack.combine(nodes, comps, &mut deform);
			}
		}
		let orig = deform.clone();
		for d in &pre {
			apply(*d, id, comps, &vertices, &mut deform, absolute);
		}

		// a deformer's state for the nodes below
		if let Some(path) = comps.get_mut::<PathCtx>(id) {
			path.update(&mut deform, &orig, absolute, angle, dt);
		}
		if comps.get::<MeshGroupCtx>(id).is_some() {
			let mut ctx = std::mem::take(comps.get_mut::<MeshGroupCtx>(id).unwrap());
			ctx.update_with(comps.get::<Mesh>(id).unwrap(), &deform, absolute);
			*comps.get_mut::<MeshGroupCtx>(id).unwrap() = ctx;
		}
		if let Some(ctx) = comps.get::<TexturedMeshRenderCtx>(id) {
			let off = ctx.vert_offset as usize;
			if let Some(out) = buffers.deforms.get_mut(off..off + ctx.vert_len) {
				if out.len() == deform.len() {
					out.copy_from_slice(&deform);
				}
			}
		}
	}

	// dynamic filters, once every deformer's state is found
	for stage in 0..2u8 {
		for &id in &posted {
			let Some(ctx) = comps.get::<TexturedMeshRenderCtx>(id) else {
				continue;
			};
			let (off, len) = (ctx.vert_offset as usize, ctx.vert_len);
			let deformers: Vec<InoxNodeUuid> = comps
				.get::<Filters>(id)
				.map(|f| f.post.iter().filter(|p| p.0 == stage).map(|p| p.1).collect())
				.unwrap_or_default();
			if deformers.is_empty() || off + len > buffers.deforms.len() {
				continue;
			}
			let vertices = &comps.get::<Mesh>(id).unwrap().vertices;
			let absolute = comps.get::<TransformStore>(id).unwrap().absolute;
			let deform = &mut buffers.deforms[off..off + len];
			for d in deformers {
				apply(d, id, comps, vertices, deform, absolute);
			}
		}
	}

	weld_all(comps, buffers, root);
}

/* --- WELDING --- */

/// A drawn mesh's vertices, deformation slot and transform.
fn mesh_of(comps: &World, id: InoxNodeUuid) -> Option<(&[Vec2], usize, Mat4)> {
	let ctx = comps.get::<TexturedMeshRenderCtx>(id)?;
	let mesh = comps.get::<Mesh>(id)?;
	(mesh.vertices.len() == ctx.vert_len).then_some(())?;
	comps.get::<Drawable>(id)?;
	Some((
		&mesh.vertices,
		ctx.vert_offset as usize,
		comps.get::<TransformStore>(id)?.absolute,
	))
}

fn weld_all(comps: &World, buffers: &mut VertexBuffers, root: InoxNodeUuid) {
	let Some(WeldOrder(order)) = comps.get::<WeldOrder>(root) else {
		return;
	};
	let mut done: HashSet<InoxNodeUuid> = HashSet::new();
	let mut applied: HashSet<(u32, u32)> = HashSet::new();
	for &target in order {
		done.insert(target);
		let Some(welding) = comps.get::<Welding>(target) else {
			continue;
		};
		for link in &welding.links {
			let source = link.target;
			let key = (source.raw().min(target.raw()), source.raw().max(target.raw()));
			if !done.contains(&source) || source == target || applied.contains(&key) {
				continue;
			}
			applied.insert(key);
			weld(comps, buffers, source, target);
		}
	}
}

/// Joins `source`'s welded vertices with `target`'s (the reference's `weldingProcessor`, run
/// on `source` for `target`).
fn weld(comps: &World, buffers: &mut VertexBuffers, source: InoxNodeUuid, target: InoxNodeUuid) {
	let (Some(sw), Some(tw)) = (comps.get::<Welding>(source), comps.get::<Welding>(target)) else {
		return;
	};
	let (Some(link), Some(counter)) = (
		sw.links.iter().find(|l| l.target == target),
		tw.links.iter().find(|l| l.target == source),
	) else {
		return;
	};
	let (Some((s_verts, s_off, s_mat)), Some((t_verts, t_off, t_mat))) =
		(mesh_of(comps, source), mesh_of(comps, target))
	else {
		return;
	};
	let (s_len, t_len) = (s_verts.len(), t_verts.len());
	if s_off + s_len > buffers.deforms.len() || t_off + t_len > buffers.deforms.len() {
		return;
	}

	let mut self_idx = Vec::new();
	let mut target_idx = Vec::new();
	for i in 0..link.indices.len().min(s_len) {
		let mapped = link.indices[i];
		if mapped < 0 || mapped as usize >= t_len {
			continue;
		}
		let t = mapped as usize;
		if t >= counter.indices.len() || counter.indices[t] < 0 {
			continue;
		}
		self_idx.push(i);
		target_idx.push(t);
	}
	// the denser seam, whichever end's list it is
	let (mut counter_self, mut counter_target) = (Vec::new(), Vec::new());
	for (i, &mapped) in counter.indices.iter().enumerate() {
		if i < t_len
			&& mapped >= 0
			&& (mapped as usize) < s_len
			&& (mapped as usize) < link.indices.len()
			&& link.indices[mapped as usize] >= 0
		{
			counter_self.push(mapped as usize);
			counter_target.push(i);
		}
	}
	if counter_self.len() > self_idx.len() || (counter_self.len() == self_idx.len() && source.raw() > target.raw()) {
		self_idx = counter_self;
		target_idx = counter_target;
	}
	let n = self_idx.len();
	if n == 0 {
		return;
	}

	let world = |verts: &[Vec2], off: usize, m: Mat4, i: usize| {
		m.transform_point3((verts[i] + buffers.deforms[off + i]).extend(0.0))
			.xy()
	};
	let self_world: Vec<Vec2> = self_idx.iter().map(|&i| world(s_verts, s_off, s_mat, i)).collect();
	let target_world: Vec<Vec2> = target_idx.iter().map(|&i| world(t_verts, t_off, t_mat, i)).collect();

	// every connected group shares one place
	let mut groups: Vec<usize> = (0..n).collect();
	fn root_of(groups: &[usize], mut i: usize) -> usize {
		while groups[i] != i {
			i = groups[i];
		}
		i
	}
	let mut first_self: HashMap<usize, usize> = HashMap::new();
	let mut first_target: HashMap<usize, usize> = HashMap::new();
	for i in 0..n {
		if let Some(&p) = first_self.get(&self_idx[i]) {
			let (a, b) = (root_of(&groups, i), root_of(&groups, p));
			groups[a] = b;
		} else {
			first_self.insert(self_idx[i], i);
		}
		if let Some(&p) = first_target.get(&target_idx[i]) {
			let (a, b) = (root_of(&groups, i), root_of(&groups, p));
			groups[a] = b;
		} else {
			first_target.insert(target_idx[i], i);
		}
	}
	let mut self_sum = vec![Vec2::ZERO; n];
	let mut target_sum = vec![Vec2::ZERO; n];
	let mut self_count = vec![0usize; n];
	let mut target_count = vec![0usize; n];
	for i in 0..n {
		let g = root_of(&groups, i);
		if first_self[&self_idx[i]] == i {
			self_sum[g] += self_world[i];
			self_count[g] += 1;
		}
		if first_target[&target_idx[i]] == i {
			target_sum[g] += target_world[i];
			target_count[g] += 1;
		}
	}
	let weight = link.weight.clamp(0.0, 1.0);
	let blended: Vec<Vec2> = (0..n)
		.map(|i| {
			let g = root_of(&groups, i);
			self_sum[g] / self_count[g].max(1) as f32 * weight
				+ target_sum[g] / target_count[g].max(1) as f32 * (1.0 - weight)
		})
		.collect();

	// each end's vertices move there, a shared vertex by the mean of its moves
	let s_inv = linear(s_mat).inverse();
	let t_inv = linear(t_mat).inverse();
	let mut scatter = |idx: &[usize], world: &[Vec2], inv: Mat4, off: usize, len: usize| {
		let mut sums = vec![Vec2::ZERO; len];
		let mut counts = vec![0usize; len];
		for (k, &i) in idx.iter().enumerate() {
			sums[i] += inv.transform_vector3((blended[k] - world[k]).extend(0.0)).xy();
			counts[i] += 1;
		}
		for i in 0..len {
			if counts[i] > 0 {
				let d = sums[i] / counts[i] as f32;
				if d.x.is_finite() && d.y.is_finite() {
					buffers.deforms[off + i] += d;
				}
			}
		}
	};
	scatter(&self_idx, &self_world, s_inv, s_off, s_len);
	scatter(&target_idx, &target_world, t_inv, t_off, t_len);
}
