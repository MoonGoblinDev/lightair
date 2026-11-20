//! The Editor's model and Lightair's offline renderer (docs/plans/screen-studio.md, 4.2 and 4.3).
//!
//! libobs composites live; it can't seek or render faster than real time, deterministically.
//! This crate turns a take or a recording, plus an edit, into frames and a finished file:
//!
//! - [`edit`]: the edit document (`edit.json` beside the take): cuts, speed, zooms, the look.
//! - [`timeline`]: output time ↔ source time through cuts and speed.
//! - [`compose`]: one frame on the GPU; [`media`]: FFmpeg, as a helper program;
//!   [`export`]: both, for the preview and the finished file.
//! - [`zoom`]: automatic zooms from the take's clicks and typing, and the camera that follows
//!   them (springs, evaluated deterministically).

pub mod compose;
pub mod edit;
pub mod export;
pub mod exporter;
pub mod fx;
pub mod generate;
pub mod keys;
pub mod sound;
pub mod media;
pub mod motion;
pub mod pointer;
pub mod seqrender;
pub mod sequence;
pub mod shapes;
#[cfg(target_os = "macos")]
pub mod surface;
pub mod thumbs;
pub mod speech;
pub mod timeline;
pub mod zoom;
