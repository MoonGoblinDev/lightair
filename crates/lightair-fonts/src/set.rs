//! The faces a renderer draws with (docs/plans/fonts.md §5): bundled files, cached downloads and
//! imported fonts, loaded into the `fontdb` database cosmic-text (and so `lightair-render`) uses,
//! with Inter as the sans-serif text falls back to. No system fonts: the same project draws the
//! same pixels on every machine.

use std::path::Path;

use crate::{Result, bundled};

pub struct FontSet {
    db: fontdb::Database,
    sans: String,
}

impl Default for FontSet {
    fn default() -> FontSet {
        FontSet::new()
    }
}

impl FontSet {
    pub fn new() -> FontSet {
        FontSet { db: fontdb::Database::new(), sans: bundled::SANS.to_string() }
    }

    /// Adds a font file (a `.ttc` adds every face in it).
    pub fn add_file(&mut self, path: &Path) -> Result<()> {
        self.db.load_font_file(path)?;
        Ok(())
    }

    /// Adds a font from memory (an imported font read from a project).
    pub fn add_bytes(&mut self, bytes: Vec<u8>) {
        self.db.load_font_data(bytes);
    }

    /// Adds every font file in a folder, recursively (a bundle folder).
    pub fn add_folder(&mut self, folder: &Path) {
        self.db.load_fonts_dir(folder);
    }

    /// The family plain sans-serif text uses (Inter unless changed).
    pub fn set_sans(&mut self, family: impl Into<String>) {
        self.sans = family.into();
    }

    pub fn sans(&self) -> &str {
        &self.sans
    }

    pub fn is_empty(&self) -> bool {
        self.db.is_empty()
    }

    /// Faces loaded.
    pub fn len(&self) -> usize {
        self.db.len()
    }

    /// Whether a family is loaded (any face), ignoring case.
    pub fn has_family(&self, family: &str) -> bool {
        self.db.faces().any(|f| f.families.iter().any(|(name, _)| name.eq_ignore_ascii_case(family)))
    }

    pub fn db(&self) -> &fontdb::Database {
        &self.db
    }

    /// The database, with its sans-serif (and the generic families text may ask for) pointed at
    /// the set's sans.
    pub fn into_db(self) -> fontdb::Database {
        let mut db = self.db;
        db.set_sans_serif_family(self.sans.clone());
        db.set_serif_family(self.sans.clone());
        db.set_monospace_family(bundled::MONO);
        db
    }
}

/// Whether a family is in a database (any face), ignoring case.
pub fn has_family(db: &fontdb::Database, family: &str) -> bool {
    db.faces().any(|f| f.families.iter().any(|(name, _)| name.eq_ignore_ascii_case(family)))
}

/// Whether a family's regular face has a glyph for `c` (what decides whether text in another
/// script, or an emoji, needs a fallback family; docs/plans/fonts.md §5).
pub fn covers(db: &fontdb::Database, family: &str, c: char) -> bool {
    let query = fontdb::Query { families: &[fontdb::Family::Name(family)], ..fontdb::Query::default() };
    let Some(id) = db.query(&query) else { return false };
    db.with_face_data(id, |data, index| {
        ttf_parser::Face::parse(data, index).ok().and_then(|f| f.glyph_index(c)).is_some()
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn coverage_comes_from_the_face_itself() {
        let inter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../branding/source/fonts/Inter-Variable.ttf");
        let mut set = FontSet::new();
        set.add_file(&inter).unwrap();
        let db = set.into_db();
        assert!(has_family(&db, "inter"));
        assert!(covers(&db, "Inter", 'A'));
        assert!(covers(&db, "Inter", 'Ж'));
        assert!(!covers(&db, "Inter", '渋'));
        assert!(!covers(&db, "Nobody", 'A'));
    }
}
