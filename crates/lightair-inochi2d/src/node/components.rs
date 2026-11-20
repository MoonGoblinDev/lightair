/*!
Inochi2D node types to Inox2D components:
- Node -> (Nothing)
- Part -> Drawable + TexturedMesh + Mesh (+ Welding, nijilive)
- Composite -> Drawable + Composite
- MeshGroup -> MeshGroup + Mesh
- SimplePhysics -> SimplePhysics
- nijilive's Mask -> Drawable + TexturedMesh + Mesh + MaskSource
- nijilive's DynamicComposite -> Drawable + DynamicComposite + Mesh (Drawable + Composite without a mesh)
- nijilive's PathDeformer -> PathDeformer + Mesh (its control points)
- Custom nodes by inheritance -> Custom nodes by composition
*/

use glam::{Mat4, Vec2, Vec3};

use crate::math::deform::Deform;
use crate::node::{InoxNodeUuid, TransformOffset};
use crate::params::ParamUuid;
use crate::physics::{
	pendulum::{rigid::RigidPendulum, spring::SpringPendulum},
	runge_kutta::PhysicsState,
};
use crate::texture::TextureId;

/* --- COMPOSITE --- */

/// If has this as a component, the node should composite all children
///
/// Zsorted children list constructed later on demand
pub struct Composite {
	/// Whether a MeshGroup above deforms this composite's children too (Inochi2D 0.8's
	/// `propagate_meshgroup`; false for puppets saved before it).
	pub propagate_meshgroup: bool,
}

/* --- MESH GROUP --- */

/// If has this as a component (with a Mesh), the node deforms its descendants with its mesh
/// (Inochi2D 0.8's MeshGroup).
pub struct MeshGroup {
	/// Children's vertices follow the group's mesh as deformed (true), or move by how much the
	/// group's mesh moved where they rest (false).
	pub dynamic: bool,
	/// Whether nodes that aren't drawn (plain nodes, physics) move with the mesh too.
	pub translate_children: bool,
}

/* --- NIJILIVE --- */

/// If has this as a component (with a Mesh holding its control points), the node is a nijilive
/// PathDeformer: a curve that bends the drawn nodes under it.
pub struct PathDeformer {
	pub curve: CurveType,
	/// A physics driver swinging the curve.
	pub physics: Option<PathPhysics>,
	/// Whether it bends its nodes after their own deformation is final (nijilive's "dynamic").
	pub dynamic: bool,
}

/// How a PathDeformer's curve runs through its control points.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CurveType {
	/// One Bezier curve with all control points.
	Bezier,
	/// A Catmull-Rom spline through them.
	Spline,
}

/// A PathDeformer's physics driver.
#[derive(Clone, Debug)]
pub enum PathPhysics {
	/// A chain of pendulums, one per segment.
	Pendulum {
		damping: f32,
		restore_constant: f32,
		gravity: f32,
		input_scale: f32,
		propagate_scale: f32,
	},
	/// Masses on springs.
	SpringPendulum,
}

/// If has this as a component (with a Drawable and a Mesh), the node is a nijilive
/// DynamicComposite: its children are drawn into a texture of their own, which is then drawn
/// on its mesh.
pub struct DynamicComposite {
	/// Its texture's centre, in its own space.
	pub texture_offset: Vec2,
	/// Its texture's size (model units).
	pub texture_size: Vec2,
}

/// A nijilive Mask: a mesh only drawn into masks (wholly, it has no texture), never shown.
pub struct MaskSource;

/// nijilive's welded vertices: a drawn node's vertices held together with another's.
pub struct Welding {
	pub links: Vec<WeldLink>,
}

pub struct WeldLink {
	pub target: InoxNodeUuid,
	/// For each of this node's vertices, the target's vertex it's welded to (or negative).
	pub indices: Vec<i64>,
	/// How much the shared place is this node's (1) or the target's (0).
	pub weight: f32,
}

/* --- DRAWABLE --- */

/// If has this as a component, the node should render something
pub struct Drawable {
	pub blending: Blending,
	/// If Some, the node should consider masking when rendering
	pub masks: Option<Masks>,
}

pub struct Blending {
	pub mode: BlendMode,
	pub tint: Vec3,
	pub screen_tint: Vec3,
	pub opacity: f32,
}

#[derive(Default, PartialEq, Clone, Copy)]
pub enum BlendMode {
	/// Normal blending mode.
	#[default]
	Normal,
	/// Multiply blending mode.
	Multiply,
	/// Color Dodge.
	ColorDodge,
	/// Linear Dodge.
	LinearDodge,
	/// Screen.
	Screen,
	/// Clip to Lower.
	/// Special blending mode that clips the drawable
	/// to a lower rendered area.
	ClipToLower,
	/// Slice from Lower.
	/// Special blending mode that slices the drawable
	/// via a lower rendered area.
	/// (Basically inverse ClipToLower.)
	SliceFromLower,
}

impl BlendMode {
	pub const VALUES: [BlendMode; 7] = [
		BlendMode::Normal,
		BlendMode::Multiply,
		BlendMode::ColorDodge,
		BlendMode::LinearDodge,
		BlendMode::Screen,
		BlendMode::ClipToLower,
		BlendMode::SliceFromLower,
	];
}

pub struct Masks {
	pub threshold: f32,
	pub masks: Vec<Mask>,
}

impl Masks {
	/// Checks whether has masks of mode `MaskMode::Mask`.
	pub fn has_masks(&self) -> bool {
		self.masks.iter().any(|mask| mask.mode == MaskMode::Mask)
	}

	/// Checks whether has masks of mode `MaskMode::Dodge`.
	pub fn has_dodge_masks(&self) -> bool {
		self.masks.iter().any(|mask| mask.mode == MaskMode::Dodge)
	}
}

pub struct Mask {
	pub source: InoxNodeUuid,
	pub mode: MaskMode,
}

#[derive(PartialEq)]
pub enum MaskMode {
	/// The part should be masked by the drawables specified.
	Mask,
	/// The path should be dodge-masked by the drawables specified.
	Dodge,
}

/* --- SIMPLE PHYSICS --- */

/// If has this as a component, the node is capable of doing Inochi2D SimplePhysics simulations
#[derive(Clone)]
pub struct SimplePhysics {
	pub param: ParamUuid,
	pub model_type: PhysicsModel,
	pub map_mode: PhysicsParamMapMode,
	pub props: PhysicsProps,
	/// Whether physics system listens to local transform only.
	pub local_only: bool,
}

#[derive(Clone)]
pub enum PhysicsModel {
	RigidPendulum,
	SpringPendulum,
}

#[derive(Clone)]
pub enum PhysicsParamMapMode {
	AngleLength,
	LengthAngle,
	XY,
	YX,
}

#[derive(Clone)]
pub struct PhysicsProps {
	/// Gravity scale (1.0 = puppet gravity)
	pub gravity: f32,
	/// Pendulum/spring rest length (pixels)
	pub length: f32,
	/// Resonant frequency (Hz)
	pub frequency: f32,
	/// Angular damping ratio
	pub angle_damping: f32,
	/// Length damping ratio
	pub length_damping: f32,
	pub output_scale: Vec2,
}

impl Default for PhysicsProps {
	fn default() -> Self {
		Self {
			gravity: 1.,
			length: 1.,
			frequency: 1.,
			angle_damping: 0.5,
			length_damping: 0.5,
			output_scale: Vec2::ONE,
		}
	}
}

/// Physical states for simulating a rigid pendulum.
#[derive(Default)]
pub(crate) struct RigidPendulumCtx {
	pub bob: Vec2,
	pub state: PhysicsState<2, RigidPendulum>,
}

/// Physical states for simulating a spring pendulum.
#[derive(Default)]
pub(crate) struct SpringPendulumCtx {
	pub state: PhysicsState<4, SpringPendulum>,
}

/* --- TEXTURED MESH --- */

/// If has this as a component, the node should render a deformed texture
pub struct TexturedMesh {
	pub tex_albedo: TextureId,
	pub tex_emissive: TextureId,
	pub tex_bumpmap: TextureId,
}

/* --- MESH --- */

/// A deformable mesh, deforming either textures (TexturedMesh nodes), or children (MeshGroup nodes)
pub struct Mesh {
	/// Vertices in the mesh.
	pub vertices: Vec<Vec2>,
	/// Base UVs.
	pub uvs: Vec<Vec2>,
	/// Indices in the mesh.
	pub indices: Vec<u32>,
	/// Origin of the mesh.
	pub origin: Vec2,
}

/* --- DEFORM STACK --- */

/// Source of a deform.
#[derive(Hash, PartialEq, Eq, Copy, Clone)]
#[allow(unused)]
pub(crate) enum DeformSource {
	Param(ParamUuid),
	Node(InoxNodeUuid),
}

/// Internal component solving for deforms of a node.
/// Storing deforms specified by multiple sources to apply on one node for one frame.
///
/// Despite the name (this is respecting the ref impl), this is not in any way a stack.
/// The order of deforms being applied, or more generally speaking, the way multiple deforms adds up to be a single one, needs to be defined according to the spec.
pub(crate) struct DeformStack {
	/// this is a component so cannot use generics for the length.
	pub(crate) deform_len: usize,
	/// map of (src, (enabled, Deform)).
	/// On reset, only set enabled to false instead of clearing the map, as deforms from same sources tend to come in every frame.
	pub(crate) stack: std::collections::HashMap<DeformSource, (bool, Deform)>,
}

/* --- TRANSFORM STORE --- */

/// Internal component storing:
/// - Relative transform being determined in between frames.
/// - Absolute transform prepared from all relative transforms just before rendering.
#[derive(Default, Clone)]
pub struct TransformStore {
	pub absolute: Mat4,
	pub relative: TransformOffset,
}

/* --- ZSORT --- */

/// Component holding zsort values that may be modified across frames.
// only one value instead of absolute + relative as in TransformStore, cause inheritance of zsort (+) is commutative
#[derive(Default)]
pub(crate) struct ZSort(pub f32);

// so ZSort automatically gets the `.total_cmp()` of `f32`
impl std::ops::Deref for ZSort {
	type Target = f32;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}
