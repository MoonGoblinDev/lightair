//! Lightair's avatars (docs/plans/avatars.md): character models that move with a face.
//!
//! - [`vrm`]: VRM 0.x and 1.0 models.
//! - [`live2d`]: Live2D models (on mocari, Lightair's fork of a pure Rust runtime).
//! - [`rig`]: from face landmarks to the head's turn, eyes, gaze, brows and mouth.
//! - [`pose`]: a model posed by a face: bones, expressions, look-at, spring bones, then
//!   skinned and morphed on the CPU, ready to draw.

pub mod body;
mod canonical;
mod glb;
pub mod live2d;
pub mod pose;
pub mod rig;
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
pub use pose::{Avatar, Controls, Framing};
pub use rig::{Calibration, Face, Landmarks, Measures, Smoother};
pub use vrm::Model;
/// The Live2D runtime (Lightair's fork of mocari), for drawing its meshes.
pub use mocari;
