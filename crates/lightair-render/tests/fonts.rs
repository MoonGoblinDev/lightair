//! Text draws the same pixels whichever way its font arrived (docs/plans/fonts.md, phase 1): from
//! a bundle folder, downloaded into the cache, or imported from a project; and with only the
//! fonts it was given, never the system's.

use std::path::{Path, PathBuf};

use lightair_fonts::catalog::{Blob, Catalog, Category, Family, FontFile, Licence, Source};
use lightair_fonts::{Cache, FontRef, FontSet, Style, fetch, hash};
use lightair_render::generate::{self, Raster};
use lightair_render::sequence::TextStyle;

fn inter() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../branding/source/fonts/Inter-Variable.ttf")
}

fn draw(set: FontSet, font: Option<FontRef>) -> Raster {
    generate::set_fonts(set);
    generate::text(&TextStyle { font, ..TextStyle::title("Lightair 123") })
}

fn inked(r: &Raster) -> usize {
    r.rgba.chunks(4).filter(|p| p[3] > 0).count()
}

#[test]
fn the_same_font_draws_the_same_pixels_from_every_source() {
    let bytes = std::fs::read(inter()).expect("the brand's Inter");
    let want = Some(FontRef::new("Inter", 700));

    // bundled: a folder of files, as an app packages them
    let bundle = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(bundle.path().join("inter")).unwrap();
    std::fs::write(bundle.path().join("inter/Inter-Variable.ttf"), &bytes).unwrap();
    let mut set = FontSet::new();
    set.add_folder(bundle.path());
    assert!(set.has_family("Inter"));
    let bundled = draw(set, want.clone());

    // downloaded: through the catalogue, checked, into the cache
    let blob = Blob {
        path: "ofl/inter/Inter-Variable.ttf".into(),
        size: bytes.len() as u64,
        git_sha1: hash::git_blob_sha1(&bytes),
        sha256: None,
        urls: vec![],
    };
    let catalog = Catalog {
        version: lightair_fonts::catalog::VERSION,
        source: Source { repo: "google/fonts".into(), commit: "test".into() },
        generated: "2026-11-20T00:00:00Z".into(),
        families: vec![Family {
            name: "Inter".into(),
            category: Category::Sans,
            designer: String::new(),
            licence: Licence::Ofl,
            dir: "ofl/inter".into(),
            licence_file: None,
            scripts: vec![],
            popularity: None,
            added: None,
            files: vec![FontFile { blob, style: Style::Normal, weight: 400, axes: vec![] }],
        }],
    };
    let cache_dir = tempfile::tempdir().unwrap();
    let mut cache = Cache::open(cache_dir.path()).unwrap();
    let served = bytes.clone();
    let fetcher = move |_: &str| Ok::<_, String>(served.clone());
    let path = fetch::ensure(&catalog, want.as_ref().unwrap(), &mut cache, &fetcher).unwrap();
    let mut set = FontSet::new();
    set.add_file(&path).unwrap();
    let downloaded = draw(set, want.clone());

    // imported: the bytes a project carries
    let mut set = FontSet::new();
    set.add_bytes(bytes.clone());
    let imported = draw(set, want.clone());

    assert!(inked(&bundled) > 500, "text was drawn");
    assert_eq!(bundled.size, downloaded.size);
    assert_eq!(bundled.rgba, downloaded.rgba);
    assert_eq!(bundled.size, imported.size);
    assert_eq!(bundled.rgba, imported.rgba);

    // a family that isn't there falls back to the set's sans (Inter), not to a system font
    let mut set = FontSet::new();
    set.add_bytes(bytes.clone());
    let fallback = draw(set, Some(FontRef::new("Some Font Nobody Has", 700)));
    assert!(inked(&fallback) > 500);

    // no fonts at all: nothing drawn, and no panic
    let empty = draw(FontSet::new(), None);
    assert_eq!(inked(&empty), 0);
}
