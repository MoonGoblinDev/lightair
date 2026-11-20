//! Fonts for every Lightair app (docs/plans/fonts.md): the desktop, Lightair Live and Lightair
//! Editor offer the same fonts, from the same catalogue, resolved by the same code.
//!
//! - [`catalog`]: the Google Fonts catalogue (`fonts-catalog.json`), generated from `google/fonts`
//!   at a pinned commit by `cargo xtask fonts-catalog`; only OFL, Apache and UFL families.
//! - [`fontref`]: what a project stores, a font by name (family, weight, style, axes), never a
//!   file.
//! - [`resolve`]: a reference to the catalogue file that draws it.
//! - [`fallback`]: the families to try for text in other scripts, and emoji.
//! - [`cache`]: downloaded files and their licences, kept by content hash.
//! - [`fetch`]: getting a file into the cache through the app's own HTTP ([`fetch::Fetcher`]),
//!   checked against the catalogue's hash.
//! - [`set`]: the faces the renderer is given to draw with.
//! - [`bundled`]: the families every app ships.

pub mod bundled;
pub mod cache;
pub mod catalog;
pub mod fallback;
pub mod fetch;
pub mod fontref;
pub mod hash;
pub mod resolve;
pub mod set;

pub use cache::Cache;
pub use catalog::{Blob, Catalog, Category, Family, FontFile, Licence};
pub use fetch::Fetcher;
pub use fontdb;
pub use fontref::{FontRef, FontSource, Style};
pub use set::FontSet;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0} isn't in the font catalogue")]
    NotInCatalog(String),
    #[error("{file}: the downloaded bytes don't match the catalogue ({why})")]
    Integrity { file: String, why: String },
    #[error("{file}: no source could deliver it ({tried})")]
    NoSource { file: String, tried: String },
}

pub type Result<T> = std::result::Result<T, Error>;
