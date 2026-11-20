//! What a project stores for a font: a name, never a file (docs/plans/fonts.md §1, §5). Opening
//! a project on another device fetches whatever it names that isn't there yet.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    #[default]
    Normal,
    Italic,
}

impl Style {
    /// From `METADATA.pb`'s `style:` (`normal`, `italic`).
    pub fn from_google(name: &str) -> Style {
        if name.eq_ignore_ascii_case("italic") { Style::Italic } else { Style::Normal }
    }
}

/// Where the font comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FontSource {
    /// Shipped in every app ([`crate::bundled`]); also in the catalogue.
    Bundled,
    /// From the catalogue, downloaded on first use.
    #[default]
    Catalog,
    /// The person's own file, copied into the project (named by its SHA-256).
    Imported { sha256: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontRef {
    pub family: String,
    #[serde(default = "regular")]
    pub weight: u16,
    #[serde(default, skip_serializing_if = "is_normal")]
    pub style: Style,
    /// Variable axes besides weight (`wdth`, `slnt`…), by tag.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub axes: BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "is_catalog")]
    pub source: FontSource,
}

fn regular() -> u16 {
    400
}

fn is_normal(s: &Style) -> bool {
    *s == Style::Normal
}

fn is_catalog(s: &FontSource) -> bool {
    *s == FontSource::Catalog
}

impl FontRef {
    /// A catalogue family at a weight, upright.
    pub fn new(family: impl Into<String>, weight: u16) -> FontRef {
        FontRef {
            family: family.into(),
            weight,
            style: Style::Normal,
            axes: BTreeMap::new(),
            source: FontSource::Catalog,
        }
    }

    pub fn italic(mut self) -> FontRef {
        self.style = Style::Italic;
        self
    }

    /// Whether the same family and face, whatever its axes: what a project needs downloaded.
    pub fn same_face(&self, other: &FontRef) -> bool {
        self.family.eq_ignore_ascii_case(&other.family) && self.style == other.style && self.source == other.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_only_what_differs_from_the_usual() {
        let r = FontRef::new("Bebas Neue", 400);
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"family":"Bebas Neue","weight":400}"#);
        let back: FontRef = serde_json::from_str(r#"{"family":"Inter"}"#).unwrap();
        assert_eq!(back, FontRef::new("Inter", 400));
        let imported = FontRef {
            source: FontSource::Imported { sha256: "ab".into(), name: "Mine.otf".into() },
            ..FontRef::new("Mine", 700).italic()
        };
        let text = serde_json::to_string(&imported).unwrap();
        assert!(text.contains(r#""style":"italic""#) && text.contains(r#""kind":"imported""#));
        assert_eq!(serde_json::from_str::<FontRef>(&text).unwrap(), imported);
    }
}
