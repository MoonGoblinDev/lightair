//! A font reference to the file that draws it: the family's file in the right style whose weight
//! is closest (a variable font whose weight axis covers it is exact), following CSS's font
//! matching closely enough that the same reference picks the same file everywhere.

use crate::catalog::{Catalog, Family, FontFile};
use crate::fontref::{FontRef, Style};

/// The file of `family` that draws `want`.
pub fn file<'a>(family: &'a Family, want: &FontRef) -> Option<&'a FontFile> {
    let in_style: Vec<&FontFile> = family.files.iter().filter(|f| f.style == want.style).collect();
    // no italic: the upright face (renderers slant it); no upright: whatever there is
    let candidates = if in_style.is_empty() {
        let other = if want.style == Style::Italic { Style::Normal } else { Style::Italic };
        family.files.iter().filter(|f| f.style == other).collect::<Vec<_>>()
    } else {
        in_style
    };
    candidates.into_iter().min_by_key(|f| distance(f, want.weight))
}

/// The catalogue file for a reference.
pub fn in_catalog<'a>(catalog: &'a Catalog, want: &FontRef) -> Option<(&'a Family, &'a FontFile)> {
    let family = catalog.family(&want.family)?;
    Some((family, file(family, want)?))
}

/// How far a file's weight is from the wanted one, as a sort key: 0 when a weight axis covers it;
/// otherwise the gap, with CSS's tie-break (up to 500 lighter faces win a tie, above it heavier
/// ones).
fn distance(file: &FontFile, want: u16) -> (u32, u8) {
    let want = want as f32;
    if let Some(axis) = file.axes.iter().find(|a| a.tag == "wght")
        && want >= axis.min
        && want <= axis.max
    {
        return (0, 0);
    }
    let (lo, hi) = match file.axes.iter().find(|a| a.tag == "wght") {
        Some(axis) => (axis.min, axis.max),
        None => (file.weight as f32, file.weight as f32),
    };
    let nearest = want.clamp(lo, hi);
    let gap = (nearest - want).abs() as u32;
    let lighter = nearest < want;
    let prefer_lighter = want <= 500.0;
    (gap, if lighter == prefer_lighter { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tests::sample;

    #[test]
    fn variable_fonts_cover_their_range() {
        let c = sample();
        let (_, f) = in_catalog(&c, &FontRef::new("Inter", 650)).unwrap();
        assert_eq!(f.blob.path, "ofl/inter/Inter[opsz,wght].ttf");
        let (_, f) = in_catalog(&c, &FontRef::new("inter", 300).italic()).unwrap();
        assert_eq!(f.blob.path, "ofl/inter/Inter-Italic[opsz,wght].ttf");
    }

    #[test]
    fn static_families_take_the_nearest_weight() {
        let c = sample();
        let pick = |w| in_catalog(&c, &FontRef::new("Lato", w)).unwrap().1.blob.path.clone();
        assert_eq!(pick(400), "ofl/lato/Lato-Regular.ttf");
        assert_eq!(pick(600), "ofl/lato/Lato-Bold.ttf");
        assert_eq!(pick(800), "ofl/lato/Lato-Black.ttf", "700 and 900 are equally far; above 500 heavier wins");
        assert_eq!(pick(200), "ofl/lato/Lato-Light.ttf");
        assert_eq!(pick(350), "ofl/lato/Lato-Light.ttf");
    }

    #[test]
    fn a_missing_style_falls_back_to_the_other() {
        let c = sample();
        let (_, f) = in_catalog(&c, &FontRef::new("Bebas Neue", 400).italic()).unwrap();
        assert_eq!(f.blob.path, "ofl/bebasneue/BebasNeue-Regular.ttf");
        assert!(in_catalog(&c, &FontRef::new("Comic Sans", 400)).is_none());
    }
}
