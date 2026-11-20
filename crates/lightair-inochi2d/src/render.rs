mod deform_stack;
mod vertex_buffers;

use std::collections::{HashMap, HashSet};
use std::mem::swap;

use crate::node::{
	components::{DeformStack, Mask, MaskSource, Masks, Mesh, MeshGroup, PathDeformer, ZSort},
	drawables::{CompositeComponents, DrawableKind, DynamicCompositeComponents, TexturedMeshComponents},
	InoxNodeUuid,
};
use crate::params::BindingValues;
use crate::puppet::{InoxNodeTree, Puppet, World};

pub use vertex_buffers::VertexBuffers;

/// Additional info per node for rendering a TexturedMesh:
/// - offset and length of array for mesh point coordinates
/// - offset and length of array for indices of mesh points defining the mesh
///
/// inside `puppet.render_ctx_vertex_buffers`.
pub struct TexturedMeshRenderCtx {
	pub index_offset: u32,
	pub vert_offset: u32,
	pub index_len: usize,
	pub vert_len: usize,
}

/// Additional info per node for rendering a Composite.
pub struct CompositeRenderCtx {
	pub zsorted_children_list: Vec<InoxNodeUuid>,
}

/// Additional info per node for rendering a nijilive DynamicComposite: what's drawn into its
/// texture (every drawn node under it, through plain nodes and Parts, but not into another
/// DynamicComposite, which is drawn as one), in zsort order. (Its mesh is a
/// `TexturedMeshRenderCtx` too.)
pub struct DynamicCompositeRenderCtx {
	pub zsorted_children_list: Vec<InoxNodeUuid>,
}

/// What a mesh in the vertex buffers is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshTexture {
	/// A Part: its albedo texture's index.
	Albedo(usize),
	/// A nijilive Mask: only drawn into masks, wholly (it has no texture).
	Mask,
	/// A nijilive DynamicComposite: drawn with the texture its children are drawn into.
	DynamicComposite,
}

/// Additional struct attached to a puppet for rendering.
pub struct RenderCtx {
	/// General compact data buffers for interfacing with the GPU.
	pub vertex_buffers: VertexBuffers,
	/// All nodes that need respective draw method calls:
	/// - including standalone parts and composite parents,
	/// - excluding (TODO: plain mesh masks) and composite children.
	root_drawables_zsorted: Vec<InoxNodeUuid>,
	/// Drawables not drawn on their own: nijilive's Masks, and what DynamicComposites draw.
	hidden: HashSet<InoxNodeUuid>,
	/// nijilive draws nodes of the same zsort in this order (its tasks': nodes before their
	/// children, children by zsort).
	order: HashMap<InoxNodeUuid, usize>,
}

/// What a DynamicComposite draws, in tree order (nijilive's `scanPartsRecurse`): Parts and the
/// nodes under them, through nodes that aren't drawn; a DynamicComposite inside is drawn as one.
fn scan_dynamic(nodes: &InoxNodeTree, comps: &World, id: InoxNodeUuid, out: &mut Vec<InoxNodeUuid>) {
	for child in nodes.get_children(id) {
		let c = child.uuid;
		match DrawableKind::new(c, comps, false) {
			Some(DrawableKind::TexturedMesh(_)) => {
				if comps.get::<MaskSource>(c).is_none() {
					out.push(c);
				}
				scan_dynamic(nodes, comps, c, out);
			}
			Some(_) => out.push(c),
			None => scan_dynamic(nodes, comps, c, out),
		}
	}
}

impl RenderCtx {
	/// MODIFIES puppet. In addition to initializing self, installs render contexts in the World of components
	pub(super) fn new(puppet: &mut Puppet) -> Self {
		let nodes = &puppet.nodes;
		let comps = &mut puppet.node_comps;

		let mut nodes_to_deform = HashSet::new();
		for param in &puppet.params {
			param.1.bindings.iter().for_each(|b| {
				if matches!(b.values, BindingValues::Deform(_)) {
					nodes_to_deform.insert(b.node);
				}
			});
		}
		// TODO: Further fill the set when Meshgroup is implemented.

		let mut vertex_buffers = VertexBuffers::default();
		let mut hidden = HashSet::new();

		let mut root_drawables_count: usize = 0;
		for node in nodes.iter() {
			let drawable_kind = DrawableKind::new(node.uuid, comps, true);
			if let Some(drawable_kind) = drawable_kind {
				root_drawables_count += 1;

				match drawable_kind {
					DrawableKind::TexturedMesh(components) => {
						let (index_offset, vert_offset) = vertex_buffers.push(components.mesh);
						let (index_len, vert_len) = (components.mesh.indices.len(), components.mesh.vertices.len());

						comps.add(
							node.uuid,
							TexturedMeshRenderCtx {
								index_offset,
								vert_offset,
								index_len,
								vert_len,
							},
						);

						// TexturedMesh not deformed by any source does not need a DeformStack
						if nodes_to_deform.contains(&node.uuid) {
							comps.add(node.uuid, DeformStack::new(vert_len));
						}
						// a nijilive Mask is only drawn into masks
						if comps.get::<MaskSource>(node.uuid).is_some() {
							hidden.insert(node.uuid);
						}
					}
					DrawableKind::DynamicComposite(components) => {
						let (index_offset, vert_offset) = vertex_buffers.push(components.mesh);
						let (index_len, vert_len) = (components.mesh.indices.len(), components.mesh.vertices.len());
						let mut children = Vec::new();
						scan_dynamic(nodes, comps, node.uuid, &mut children);
						hidden.extend(children.iter().copied());

						comps.add(
							node.uuid,
							TexturedMeshRenderCtx {
								index_offset,
								vert_offset,
								index_len,
								vert_len,
							},
						);
						if nodes_to_deform.contains(&node.uuid) {
							comps.add(node.uuid, DeformStack::new(vert_len));
						}
						comps.add(
							node.uuid,
							DynamicCompositeRenderCtx {
								zsorted_children_list: children,
							},
						);
					}
					DrawableKind::Composite { .. } => {
						// exclude non-drawable children
						let children_list: Vec<InoxNodeUuid> = nodes
							.get_children(node.uuid)
							.filter_map(|n| {
								if DrawableKind::new(n.uuid, comps, false).is_some() {
									Some(n.uuid)
								} else {
									None
								}
							})
							.collect();

						// composite children are excluded from root_drawables_zsorted
						root_drawables_count -= children_list.len();

						comps.add(
							node.uuid,
							CompositeRenderCtx {
								// sort later, before render
								zsorted_children_list: children_list,
							},
						);
					}
				};
			}
		}

		// MeshGroups (and nijilive's PathDeformers) deformed by params
		for id in &nodes_to_deform {
			let deformer = comps.get::<MeshGroup>(*id).is_some() || comps.get::<PathDeformer>(*id).is_some();
			if deformer && comps.get::<DeformStack>(*id).is_none() {
				if let Some(len) = comps.get::<Mesh>(*id).map(|m| m.vertices.len()) {
					comps.add(*id, DeformStack::new(len));
				}
			}
		}

		let mut root_drawables_zsorted = Vec::new();
		// similarly, populate later, before render
		root_drawables_zsorted.resize(root_drawables_count, InoxNodeUuid(0));

		let mut order = HashMap::new();
		if puppet.nijilive {
			// nodes before their children, children by zsort (at rest, as nijilive's tasks are)
			let mut stack = vec![nodes.root_node_id];
			while let Some(id) = stack.pop() {
				order.insert(id, order.len());
				let mut children: Vec<(InoxNodeUuid, f32)> =
					nodes.get_children(id).map(|n| (n.uuid, n.zsort)).collect();
				children.sort_by(|a, b| b.1.total_cmp(&a.1));
				stack.extend(children.iter().rev().map(|c| c.0));
			}
		}

		Self {
			vertex_buffers,
			root_drawables_zsorted,
			hidden,
			order,
		}
	}

	/// Reset all `DeformStack`.
	pub(crate) fn reset(&mut self, nodes: &InoxNodeTree, comps: &mut World) {
		for node in nodes.iter() {
			if let Some(deform_stack) = comps.get_mut::<DeformStack>(node.uuid) {
				deform_stack.reset();
			}
		}
	}

	/// Update zsort-ordered info and deform buffer content inside self, according to updated puppet.
	/// (`nijilive`: deformed by nijilive's rules; physics there run `dt` seconds.)
	pub(crate) fn update(&mut self, nodes: &InoxNodeTree, comps: &mut World, nijilive: bool, dt: f32) {
		let mut root_drawable_uuid_zsort_vec = Vec::<(InoxNodeUuid, f32)>::new();
		let mut in_groups = Vec::new();
		if nijilive {
			crate::nijilive::update(nodes, comps, &mut self.vertex_buffers, dt);
		}

		// root is definitely not a drawable.
		for node in nodes.iter().skip(1) {
			if let Some(drawable_kind) = DrawableKind::new(node.uuid, comps, false) {
				let parent = nodes.get_parent(node.uuid);
				let node_zsort = comps.get::<ZSort>(node.uuid).unwrap().0;

				if !self.hidden.contains(&node.uuid)
					&& !matches!(
						DrawableKind::new(parent.uuid, comps, false),
						Some(DrawableKind::Composite(_))
					) {
					// exclude composite children
					root_drawable_uuid_zsort_vec.push((node.uuid, node_zsort));
				}

				match drawable_kind {
					// for Composite, update zsorted children list
					DrawableKind::Composite { .. } => {
						// `swap()` usage is a trick that both:
						// - returns mut borrowed comps early
						// - does not involve any heap allocations
						let mut zsorted_children_list = Vec::new();
						swap(
							&mut zsorted_children_list,
							&mut comps
								.get_mut::<CompositeRenderCtx>(node.uuid)
								.unwrap()
								.zsorted_children_list,
						);

						zsorted_children_list.sort_by(|a, b| {
							let zsort_a = comps.get::<ZSort>(*a).unwrap();
							let zsort_b = comps.get::<ZSort>(*b).unwrap();
							zsort_a.total_cmp(zsort_b).reverse()
						});

						swap(
							&mut zsorted_children_list,
							&mut comps
								.get_mut::<CompositeRenderCtx>(node.uuid)
								.unwrap()
								.zsorted_children_list,
						);
					}
					DrawableKind::DynamicComposite { .. } => {
						let mut list = Vec::new();
						swap(
							&mut list,
							&mut comps
								.get_mut::<DynamicCompositeRenderCtx>(node.uuid)
								.unwrap()
								.zsorted_children_list,
						);
						let order = &self.order;
						list.sort_by(|a, b| {
							let za = comps.get::<ZSort>(*a).unwrap();
							let zb = comps.get::<ZSort>(*b).unwrap();
							za.total_cmp(zb).reverse().then_with(|| order.get(a).cmp(&order.get(b)))
						});
						swap(
							&mut list,
							&mut comps
								.get_mut::<DynamicCompositeRenderCtx>(node.uuid)
								.unwrap()
								.zsorted_children_list,
						);
					}
					// (nijilive's deforms are in the buffers already)
					DrawableKind::TexturedMesh(..) if nijilive => {}
					// for TexturedMesh, obtain and write deforms into vertex_buffer
					DrawableKind::TexturedMesh(..) => {
						// A TexturedMesh not having an associated DeformStack means it will not be deformed at all, skip.
						// (unless a MeshGroup deforms it)
						let render_ctx = comps.get::<TexturedMeshRenderCtx>(node.uuid).unwrap();
						let vert_offset = render_ctx.vert_offset as usize;
						let vert_len = render_ctx.vert_len;
						let deforms = &mut self.vertex_buffers.deforms[vert_offset..(vert_offset + vert_len)];
						if let Some(deform_stack) = comps.get::<DeformStack>(node.uuid) {
							deform_stack.combine(nodes, comps, deforms);
						} else if crate::meshgroup::has_filter(node.uuid, comps) {
							deforms.iter_mut().for_each(|d| *d = glam::Vec2::ZERO);
						}
						if crate::meshgroup::has_filter(node.uuid, comps) {
							in_groups.push((node.uuid, vert_offset, vert_len));
						}
					}
				}
			}
		}

		// Parts in MeshGroups follow them
		for (id, offset, len) in in_groups {
			crate::meshgroup::filter_part(id, comps, &mut self.vertex_buffers.deforms[offset..offset + len]);
		}

		if nijilive {
			let order = &self.order;
			root_drawable_uuid_zsort_vec.sort_by(|a, b| {
				a.1.total_cmp(&b.1)
					.reverse()
					.then_with(|| order.get(&a.0).cmp(&order.get(&b.0)))
			});
			self.root_drawables_zsorted = root_drawable_uuid_zsort_vec.iter().map(|d| d.0).collect();
			return;
		}
		root_drawable_uuid_zsort_vec.sort_by(|a, b| a.1.total_cmp(&b.1).reverse());
		self.root_drawables_zsorted
			.iter_mut()
			.zip(root_drawable_uuid_zsort_vec.iter())
			.for_each(|(old, new)| *old = new.0);
	}
}

/// Same as the reference Inochi2D implementation, Inox2D also aims for a "bring your own rendering backend" design.
/// A custom backend shall implement this trait.
///
/// It is perfectly fine that the trait implementation does not contain everything needed to display a puppet as:
/// - The renderer may not be directly rendering to the screen for flexibility.
/// - The renderer may want platform-specific optimizations, e.g. batching, and the provided implementation is merely for collecting puppet info.
/// - The renderer may be a debug/just-for-fun renderer intercepting draw calls for other purposes.
///
/// Either way, the point is Inox2D will implement a `draw()` method for any `impl InoxRenderer`, dispatching calls based on puppet structure according to Inochi2D standard.
pub trait InoxRenderer {
	/// Begin masking.
	///
	/// Ref impl: Clear and start writing to the stencil buffer, lock the color buffer.
	fn on_begin_masks(&self, masks: &Masks);
	/// Get prepared for rendering a singular Mask.
	fn on_begin_mask(&self, mask: &Mask);
	/// Get prepared for rendering masked content.
	///
	/// Ref impl: Read only from the stencil buffer, unlock the color buffer.
	fn on_begin_masked_content(&self);
	/// End masking.
	///
	/// Ref impl: Disable the stencil buffer.
	fn on_end_mask(&self);

	/// Draw TexturedMesh content.
	// TODO: TexturedMesh without any texture (usually for mesh masks)?
	fn draw_textured_mesh_content(
		&self,
		as_mask: bool,
		components: &TexturedMeshComponents,
		render_ctx: &TexturedMeshRenderCtx,
		id: InoxNodeUuid,
	);

	/// Begin compositing. Get prepared for rendering children of a Composite.
	///
	/// Ref impl: Prepare composite buffers.
	fn begin_composite_content(
		&self,
		as_mask: bool,
		components: &CompositeComponents,
		render_ctx: &CompositeRenderCtx,
		id: InoxNodeUuid,
	);
	/// End compositing.
	///
	/// Ref impl: Transfer content from composite buffers to normal buffers.
	fn finish_composite_content(
		&self,
		as_mask: bool,
		components: &CompositeComponents,
		render_ctx: &CompositeRenderCtx,
		id: InoxNodeUuid,
	);

	/// Begin a nijilive DynamicComposite: what follows is drawn into its texture (until
	/// `finish_dynamic_composite`; the DynamicComposite's own masks come in between).
	fn begin_dynamic_composite(
		&self,
		_as_mask: bool,
		_components: &DynamicCompositeComponents,
		_render_ctx: &TexturedMeshRenderCtx,
		_id: InoxNodeUuid,
	) {
	}

	/// End a nijilive DynamicComposite: draw its mesh with the texture its children were drawn
	/// into.
	fn finish_dynamic_composite(
		&self,
		_as_mask: bool,
		_components: &DynamicCompositeComponents,
		_render_ctx: &TexturedMeshRenderCtx,
		_id: InoxNodeUuid,
	) {
	}
}

pub trait InoxRendererExt {
	/// Draw a Drawable, which is potentially masked.
	fn draw_drawable(&self, as_mask: bool, comps: &World, id: InoxNodeUuid);

	/// Draw one composite. `components` must be referencing `comps`.
	fn draw_composite(&self, as_mask: bool, comps: &World, components: &CompositeComponents, id: InoxNodeUuid);

	/// Draw one nijilive DynamicComposite (its children, then its masks, then its mesh).
	fn draw_dynamic_composite(
		&self,
		as_mask: bool,
		comps: &World,
		components: &DynamicCompositeComponents,
		id: InoxNodeUuid,
	);

	/// Iterate over top-level drawables (excluding masks) in zsort order,
	/// and make draw calls correspondingly.
	///
	/// This effectively draws the complete puppet.
	fn draw(&self, puppet: &Puppet);
}

impl<T: InoxRenderer> InoxRendererExt for T {
	fn draw_drawable(&self, as_mask: bool, comps: &World, id: InoxNodeUuid) {
		// (a mask whose source isn't drawn: nothing)
		let Some(drawable_kind) = DrawableKind::new(id, comps, false) else {
			return;
		};
		let masks = match drawable_kind {
			DrawableKind::TexturedMesh(ref components) => &components.drawable.masks,
			DrawableKind::Composite(ref components) => &components.drawable.masks,
			DrawableKind::DynamicComposite(ref components) => {
				return self.draw_dynamic_composite(as_mask, comps, components, id);
			}
		};

		let mut has_masks = false;
		if let Some(ref masks) = masks {
			has_masks = true;
			self.on_begin_masks(masks);
			for mask in &masks.masks {
				self.on_begin_mask(mask);

				self.draw_drawable(true, comps, mask.source);
			}
			self.on_begin_masked_content();
		}

		match drawable_kind {
			DrawableKind::TexturedMesh(ref components) => {
				self.draw_textured_mesh_content(as_mask, components, comps.get(id).unwrap(), id)
			}
			DrawableKind::Composite(ref components) => self.draw_composite(as_mask, comps, components, id),
			DrawableKind::DynamicComposite(_) => (),
		}

		if has_masks {
			self.on_end_mask();
		}
	}

	fn draw_dynamic_composite(
		&self,
		as_mask: bool,
		comps: &World,
		components: &DynamicCompositeComponents,
		id: InoxNodeUuid,
	) {
		let (Some(render_ctx), Some(mesh_ctx)) = (
			comps.get::<DynamicCompositeRenderCtx>(id),
			comps.get::<TexturedMeshRenderCtx>(id),
		) else {
			return;
		};
		if render_ctx.zsorted_children_list.is_empty() {
			return;
		}
		self.begin_dynamic_composite(as_mask, components, mesh_ctx, id);
		for uuid in &render_ctx.zsorted_children_list {
			self.draw_drawable(as_mask, comps, *uuid);
		}
		// (its masks apply to its mesh, once its texture is drawn)
		let masks = if as_mask {
			None
		} else {
			components.drawable.masks.as_ref()
		};
		if let Some(masks) = masks {
			self.on_begin_masks(masks);
			for mask in &masks.masks {
				self.on_begin_mask(mask);
				self.draw_drawable(true, comps, mask.source);
			}
			self.on_begin_masked_content();
		}
		self.finish_dynamic_composite(as_mask, components, mesh_ctx, id);
		if masks.is_some() {
			self.on_end_mask();
		}
	}

	fn draw_composite(&self, as_mask: bool, comps: &World, components: &CompositeComponents, id: InoxNodeUuid) {
		let render_ctx = comps.get::<CompositeRenderCtx>(id).unwrap();
		if render_ctx.zsorted_children_list.is_empty() {
			// Optimization: Nothing to be drawn, skip context switching
			return;
		}

		self.begin_composite_content(as_mask, components, render_ctx, id);

		for uuid in &render_ctx.zsorted_children_list {
			let drawable_kind = DrawableKind::new(*uuid, comps, false)
				.expect("All children in zsorted_children_list should be a Drawable.");
			match drawable_kind {
				DrawableKind::TexturedMesh(components) => {
					self.draw_textured_mesh_content(as_mask, &components, comps.get(*uuid).unwrap(), *uuid)
				}
				DrawableKind::Composite { .. } => panic!("Composite inside Composite not allowed."),
				DrawableKind::DynamicComposite(ref components) => {
					self.draw_dynamic_composite(as_mask, comps, components, *uuid)
				}
			}
		}

		self.finish_composite_content(as_mask, components, render_ctx, id);
	}

	/// Dispatches draw calls for all nodes of `puppet`
	/// - with provided renderer implementation,
	/// - in Inochi2D standard defined order.
	///
	/// This does not guarantee the display of a puppet on screen due to these possible reasons:
	/// - Only provided `InoxRenderer` method implementations are called.
	///
	/// For example, maybe the caller still need to transfer content from a texture buffer to the screen surface buffer.
	/// - The provided `InoxRender` implementation is wrong.
	/// - `puppet` here does not belong to the `model` this `renderer` is initialized with. This will likely result in panics for non-existent node uuids.
	fn draw(&self, puppet: &Puppet) {
		for uuid in &puppet
			.render_ctx
			.as_ref()
			.expect("RenderCtx of puppet must be initialized before calling draw().")
			.root_drawables_zsorted
		{
			self.draw_drawable(false, &puppet.node_comps, *uuid);
		}
	}
}
