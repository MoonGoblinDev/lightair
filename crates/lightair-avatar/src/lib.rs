//! Lightair's avatars (docs/plans/avatars.md): character models that move with a face.
//!
//! - [`vrm`]: VRM 0.x and 1.0 models.
//! - [`live2d`]: Live2D models (on lightair-moc3, the built-in .moc3 runtime).
//! - [`inochi2d`]: Inochi2D puppets (on lightair-inochi2d, the built-in puppet runtime).
//! - [`uma`]: Umamo puppets (`.uma`), evaluated the way Umamo's runtime does.
//! - [`rig`]: from face landmarks to the head's turn, eyes, gaze, brows and mouth.
//! - [`pose`]: a model posed by a face: bones, expressions, look-at, spring bones, then
//!   skinned and morphed on the CPU, ready to draw.

pub mod body;
mod canonical;
mod glb;
pub mod inochi2d;
pub mod live2d;
pub mod pose;
pub mod rig;
pub mod uma;
pub mod vrm;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("couldn't read the model: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub use body::{BodyFrame, BodyMode, BodyState};
/// The built-in .moc3 runtime, for drawing its meshes.
pub use lightair_moc3;
pub use pose::{Avatar, Controls, Framing};
pub use rig::{Calibration, Face, Landmarks, Measures, Smoother};
pub use vrm::Model;
