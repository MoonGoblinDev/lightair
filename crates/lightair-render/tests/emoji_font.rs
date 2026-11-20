//! The bundled emoji font draws in colour (docs/plans/fonts.md §3: Noto Color Emoji's format must
//! be one the renderer draws). Needs the bundle: `cargo xtask fonts-bundle`; skipped without it.

use std::path::Path;

use lightair_fonts::FontSet;
use lightair_render::generate;
use lightair_render::sequence::TextStyle;

#[test]
fn noto_color_emoji_draws_in_colour() {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fonts/bundled");
    let Ok(manifest) = lightair_fonts::bundled::Manifest::read(&bundle) else {
        eprintln!("no bundle (run `cargo xtask fonts-bundle`); skipped");
        return;
    };
    let mut set = FontSet::new();
    for family in manifest.families.iter().filter(|f| f.name == "Inter" || f.name == "Noto Color Emoji") {
        for file in &family.files {
            set.add_file(&bundle.join(file)).unwrap();
        }
    }
    assert!(set.has_family("Noto Color Emoji") && set.has_family("Inter"));
    generate::set_fonts(set);
    let r = generate::text(&TextStyle { color: 0xFFFFFF, ..TextStyle::title("GG 🔥🎉 👍🏽") });
    // white text would give grey pixels only; a colour emoji gives saturated ones
    let coloured = r
        .rgba
        .chunks(4)
        .filter(|p| p[3] > 200)
        .filter(|p| {
            let (mx, mn) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
            mx as i32 - mn as i32 > 60
        })
        .count();
    assert!(coloured > 200, "emoji drew {coloured} coloured pixels");
}
