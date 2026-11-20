//! The families every Lightair app ships (docs/plans/fonts.md §3): interfaces, overlays and
//! captions work offline and look right on first launch. `cargo xtask fonts-bundle` downloads
//! them, checked against the catalogue, into a folder the apps package with a [`Manifest`].

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Licence, Result};

/// The default sans-serif: what text falls back to and what interfaces use.
pub const SANS: &str = "Inter";
/// The monospace interface font.
pub const MONO: &str = "JetBrains Mono";
/// Emoji, drawn the same everywhere.
pub const EMOJI: &str = "Noto Color Emoji";

/// The bundled families, by kind.
pub const FAMILIES: &[&str] = &[
    // interface
    "Inter",
    "JetBrains Mono",
    // sans
    "Roboto",
    "Open Sans",
    "Montserrat",
    "Poppins",
    "Lato",
    "Nunito",
    "Raleway",
    "Work Sans",
    "DM Sans",
    "Manrope",
    "Space Grotesk",
    "Rubik",
    // bold and condensed display
    "Oswald",
    "Bebas Neue",
    "Anton",
    "Barlow Condensed",
    "Archivo Black",
    "Bungee",
    "Righteous",
    "Bangers",
    "Alfa Slab One",
    "Russo One",
    // rounded
    "Fredoka",
    "Baloo 2",
    "Quicksand",
    "Comfortaa",
    // serif
    "Playfair Display",
    "Lora",
    "DM Serif Display",
    "Abril Fatface",
    // handwriting and script
    "Pacifico",
    "Caveat",
    "Permanent Marker",
    "Dancing Script",
    "Lobster",
    "Satisfy",
    // pixel and retro
    "Press Start 2P",
    "VT323",
    "Silkscreen",
    // mono
    "Space Mono",
    "Roboto Mono",
    // emoji
    "Noto Color Emoji",
];

/// What a bundle folder holds: written by `cargo xtask fonts-bundle` as `bundled.json` next to
/// the files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// The catalogue commit the files were read at.
    pub commit: String,
    pub families: Vec<BundledFamily>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledFamily {
    pub name: String,
    pub licence: Licence,
    /// Paths relative to the bundle folder.
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_file: Option<String>,
}

impl Manifest {
    pub fn read(folder: &Path) -> Result<Manifest> {
        Ok(serde_json::from_str(&std::fs::read_to_string(folder.join("bundled.json"))?)?)
    }

    /// The font files, as paths.
    pub fn font_paths(&self, folder: &Path) -> Vec<std::path::PathBuf> {
        self.families.iter().flat_map(|f| f.files.iter().map(|p| folder.join(p))).collect()
    }
}

/// The files of a family an app ships: a variable family's variable files (upright and italic);
/// a static family's regular and bold, upright and italic (other weights download when used).
/// Keeps the bundle within its budget (docs/plans/fonts.md §3).
pub fn files_to_bundle(family: &crate::Family) -> Vec<&crate::FontFile> {
    if family.files.iter().any(|f| !f.axes.is_empty()) {
        return family.files.iter().filter(|f| !f.axes.is_empty()).collect();
    }
    let mut out: Vec<&crate::FontFile> = Vec::new();
    for style in [crate::Style::Normal, crate::Style::Italic] {
        for weight in [400, 700] {
            let want = crate::FontRef { style, ..crate::FontRef::new(family.name.clone(), weight) };
            if let Some(f) = crate::resolve::file(family, &want)
                && f.style == style
                && !out.contains(&f)
            {
                out.push(f);
            }
        }
    }
    if out.is_empty() {
        out.extend(family.files.first());
    }
    out
}

/// Whether a family is bundled.
pub fn is_bundled(family: &str) -> bool {
    FAMILIES.iter().any(|f| f.eq_ignore_ascii_case(family))
}
