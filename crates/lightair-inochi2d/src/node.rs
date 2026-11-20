pub mod components;
pub mod drawables;

use crate::math::transform::TransformOffset;

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
#[repr(transparent)]
pub struct InoxNodeUuid(pub(crate) u32);

impl InoxNodeUuid {
	/// The id as a number.
	pub fn raw(self) -> u32 {
		self.0
	}
}

pub struct InoxNode {
	pub uuid: InoxNodeUuid,
	pub name: String,
	pub enabled: bool,
	pub zsort: f32,
	pub trans_offset: TransformOffset,
	pub lock_to_root: bool,
}
