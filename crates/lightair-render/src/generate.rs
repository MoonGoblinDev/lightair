//! Pictures made rather than read: text (shaped with cosmic-text, in the fonts the caller gave
//! [`set_fonts`]; the system's own on desktops when none were given) and shapes (tiny-skia), as
//! premultiplied RGBA just big enough for what they hold.

use std::collections::HashMap;
use std::sync::Mutex;

use cosmic_text::{Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style as Slant, SwashCache, Weight};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::sequence::{Content, ShapeKind, ShapeStyle, TextStyle};

/// A made picture: premultiplied RGBA.
#[derive(Clone)]
pub struct Raster {
    pub rgba: Vec<u8>,
    pub size: (u32, u32),
}

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
    /// The family plain text uses, when the caller chose the fonts ([`set_fonts`]); then emoji
    /// and other scripts get their fallback families explicitly ([`spans`]).
    sans: Option<String>,
}

/// The fonts text draws with: the caller's ([`set_fonts`]), else, on desktops, the system's,
/// loaded once (it takes a moment).
static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);

/// Draws text with these fonts from now on (docs/plans/fonts.md §5): the bundled set, cached
/// downloads and imported fonts, with no system fonts, so the same project draws the same pixels
/// on every machine. Phones must call it before drawing text; desktops fall back to the system's
/// fonts until it's called.
pub fn set_fonts(set: lightair_fonts::FontSet) {
    let sans = set.sans().to_string();
    let db = set.into_db();
    let system = FontSystem::new_with_locale_and_db("en-US".into(), db);
    *FONTS.lock().unwrap() = Some(Fonts { system, cache: SwashCache::new(), sans: Some(sans) });
}

/// The fonts to draw with, loading the system's on a desktop when none were given. `None` (a
/// phone that hasn't called [`set_fonts`], or no faces at all) draws no text instead of panicking.
fn fonts(guard: &mut Option<Fonts>) -> Option<&mut Fonts> {
    if guard.is_none() && cfg!(any(target_os = "macos", target_os = "windows", target_os = "linux")) {
        *guard = Some(Fonts { system: FontSystem::new(), cache: SwashCache::new(), sans: None });
    }
    match guard {
        Some(f) if !f.system.db().is_empty() => Some(f),
        _ => {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| tracing::warn!("no fonts to draw text with: call generate::set_fonts"));
            None
        }
    }
}

/// Splits text into runs drawn in one family each: `None` is the chosen family; emoji go to the
/// emoji font, and characters the chosen family lacks to their script's Noto family, when those
/// are loaded (cosmic-text's own fallback only knows the system's font names).
fn spans<'t>(text: &'t str, chosen: &str, db: &lightair_fonts::fontdb::Database) -> Vec<(&'t str, Option<&'static str>)> {
    use lightair_fonts::{bundled, fallback, set};
    let emoji_loaded = set::has_family(db, bundled::EMOJI);
    let mut out: Vec<(usize, usize, Option<&'static str>)> = Vec::new();
    let mut prev: Option<&'static str> = None;
    for (i, c) in text.char_indices() {
        let u = c as u32;
        // joiners, variation selectors, skin tones and tags stay with what they follow
        let joins = matches!(u, 0x200D | 0xFE0E | 0xFE0F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0x20E3);
        let family = if joins {
            prev
        } else if fallback::is_emoji(c) {
            emoji_loaded.then_some(bundled::EMOJI)
        } else {
            match fallback::noto_for(c) {
                Some(f) if f != bundled::EMOJI && !set::covers(db, chosen, c) && set::has_family(db, f) => Some(f),
                _ => None,
            }
        };
        match out.last_mut() {
            Some(last) if last.2 == family => last.1 = i + c.len_utf8(),
            _ => out.push((i, i + c.len_utf8(), family)),
        }
        prev = family;
    }
    out.into_iter().map(|(a, b, f)| (&text[a..b], f)).collect()
}

fn paint(rgb: u32, alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, (alpha.clamp(0.0, 1.0) * 255.0) as u8);
    p.anti_alias = true;
    p
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r < 0.5 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    let mut p = PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.quad_to(x + w, y, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.quad_to(x + w, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.quad_to(x, y + h, x, y + h - r);
    p.line_to(x, y + r);
    p.quad_to(x, y, x + r, y);
    p.close();
    p.finish()
}

/// The colour plain glyphs are drawn in (a mark: colour glyphs, emoji, come in their own).
const PLAIN: (u8, u8, u8) = (1, 2, 3);

/// Lays text out; draws its glyph coverage through `each(x, y, alpha, colour)` (pixels from the
/// text's own top-left; the colour of colour glyphs, emoji) and returns its size.
fn layout(style: &TextStyle, mut each: impl FnMut(i32, i32, u8, Option<u32>)) -> (f32, f32) {
    let mut guard = FONTS.lock().unwrap();
    let Some(fonts) = fonts(&mut guard) else { return (0.0, 0.0) };
    let metrics = Metrics::new(style.size.max(4.0), style.size.max(4.0) * 1.2);
    let mut buf = Buffer::new(&mut fonts.system, metrics);
    buf.set_size(Some(style.width.max(style.size)), None);
    let family = style.font.as_ref().map(|f| Family::Name(f.family.as_str())).unwrap_or(Family::SansSerif);
    let weight = match &style.font {
        Some(f) if style.bold => f.weight.max(600),
        Some(f) => f.weight,
        None if style.bold => 700,
        None => 400,
    };
    let italic = style.font.as_ref().is_some_and(|f| f.style == lightair_fonts::Style::Italic);
    let attrs = Attrs::new().family(family).weight(Weight(weight)).style(if italic { Slant::Italic } else { Slant::Normal });
    let align = match style.align {
        0 => Align::Left,
        2 => Align::Right,
        _ => Align::Center,
    };
    match fonts.sans.clone() {
        // the caller's fonts: emoji and other scripts in their families explicitly
        Some(sans) => {
            let chosen = style.font.as_ref().map(|f| f.family.clone()).unwrap_or(sans);
            let runs = spans(&style.text, &chosen, fonts.system.db());
            let rich = runs.iter().map(|(t, f)| match f {
                Some(name) => (*t, attrs.clone().family(Family::Name(name))),
                None => (*t, attrs.clone()),
            });
            buf.set_rich_text(rich, &attrs, Shaping::Advanced, Some(align));
        }
        None => buf.set_text(&style.text, &attrs, Shaping::Advanced, Some(align)),
    }
    buf.shape_until_scroll(&mut fonts.system, false);
    // what the lines cover (alignment moves them inside the wrapping width)
    let (mut left, mut right, mut bottom) = (f32::MAX, 0.0f32, 0.0f32);
    for run in buf.layout_runs() {
        for g in run.glyphs {
            left = left.min(g.x);
            right = right.max(g.x + g.w);
        }
        bottom = bottom.max(run.line_top + run.line_height);
    }
    if left == f32::MAX {
        return (0.0, 0.0);
    }
    let dx = left.floor() as i32;
    buf.draw(&mut fonts.system, &mut fonts.cache, Color::rgb(PLAIN.0, PLAIN.1, PLAIN.2), |x, y, w, h, c| {
        let own = ((c.r(), c.g(), c.b()) != PLAIN).then(|| ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32);
        for yy in y..y + h as i32 {
            for xx in x..x + w as i32 {
                each(xx - dx, yy, c.a(), own);
            }
        }
    });
    (right - left, bottom)
}

/// Text as a picture: its words, an outline and a box behind them if the style asks.
pub fn text(style: &TextStyle) -> Raster {
    let mut cover: Vec<(i32, i32, u8, Option<u32>)> = Vec::new();
    let (w, h) = layout(style, |x, y, a, own| {
        if a > 0 {
            cover.push((x, y, a, own));
        }
    });
    let pad = if style.background.is_some() { style.size * 0.35 } else { style.size * 0.12 };
    let (cw, ch) = ((w + 2.0 * pad).ceil().max(2.0) as u32, (h + 2.0 * pad).ceil().max(2.0) as u32);
    let Some(mut pm) = Pixmap::new(cw, ch) else { return Raster { rgba: vec![0; 16], size: (2, 2) } };
    if let Some((bg, alpha)) = style.background
        && let Some(path) = rounded(0.0, 0.0, cw as f32, ch as f32, style.size * 0.25)
    {
        pm.fill_path(&path, &paint(bg, alpha), FillRule::Winding, Transform::identity(), None);
    }
    let (pw, ph) = (cw as i32, ch as i32);
    let off = pad.round() as i32;
    let data = pm.data_mut();
    let mut put = |x: i32, y: i32, rgb: u32, a: u8| {
        if x < 0 || y < 0 || x >= pw || y >= ph || a == 0 {
            return;
        }
        let i = ((y * pw + x) * 4) as usize;
        let a32 = a as u32;
        let src = [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF];
        for k in 0..3 {
            data[i + k] = ((src[k] * a32 + data[i + k] as u32 * (255 - a32)) / 255) as u8;
        }
        data[i + 3] = (a32 + data[i + 3] as u32 * (255 - a32) / 255) as u8;
    };
    if let Some(o) = style.outline {
        let r = (style.size * 0.06).max(1.5) as i32;
        for (x, y, a, _) in &cover {
            for (dx, dy) in [(-r, 0), (r, 0), (0, -r), (0, r), (-r, -r), (r, r), (-r, r), (r, -r)] {
                put(x + off + dx, y + off + dy, o, *a);
            }
        }
    }
    for (x, y, a, own) in &cover {
        put(x + off, y + off, own.unwrap_or(style.color), *a);
    }
    Raster { rgba: pm.data().to_vec(), size: (cw, ch) }
}

/// A shape as a picture.
pub fn shape(style: &ShapeStyle) -> Raster {
    let line = style.stroke.map_or(0.0, |(_, w)| w);
    let (w, h) = (style.width.max(2.0), style.height.max(2.0));
    let (cw, ch) = ((w + 2.0 * line).ceil() as u32, (h + 2.0 * line).ceil() as u32);
    let Some(mut pm) = Pixmap::new(cw, ch) else { return Raster { rgba: vec![0; 16], size: (2, 2) } };
    let (x, y) = (line, line);
    let path = match style.shape {
        ShapeKind::Rectangle => rounded(x, y, w, h, style.radius),
        ShapeKind::Ellipse => Rect::from_xywh(x, y, w, h).and_then(PathBuilder::from_oval),
        ShapeKind::Arrow => {
            // pointing right: a shaft and a head
            let (shaft, head) = (h * 0.3, w.min(h * 1.2));
            let mut p = PathBuilder::new();
            p.move_to(x, y + h / 2.0 - shaft / 2.0);
            p.line_to(x + w - head, y + h / 2.0 - shaft / 2.0);
            p.line_to(x + w - head, y);
            p.line_to(x + w, y + h / 2.0);
            p.line_to(x + w - head, y + h);
            p.line_to(x + w - head, y + h / 2.0 + shaft / 2.0);
            p.line_to(x, y + h / 2.0 + shaft / 2.0);
            p.close();
            p.finish()
        }
    };
    if let Some(path) = path {
        if let Some((rgb, a)) = style.fill {
            pm.fill_path(&path, &paint(rgb, a), FillRule::Winding, Transform::identity(), None);
        }
        if let Some((rgb, width)) = style.stroke {
            let stroke = Stroke { width, ..Stroke::default() };
            pm.stroke_path(&path, &paint(rgb, 1.0), &stroke, Transform::identity(), None);
        }
    }
    Raster { rgba: pm.data().to_vec(), size: (cw, ch) }
}

/// Made pictures, kept while their content is the same.
#[derive(Default)]
pub struct Cache {
    made: HashMap<String, Raster>,
}

impl Cache {
    pub fn get(&mut self, content: &Content) -> Raster {
        let key = serde_json::to_string(content).unwrap_or_default();
        if let Some(r) = self.made.get(&key) {
            return r.clone();
        }
        let r = match content {
            Content::Text(t) => text(t),
            Content::Shape(s) => shape(s),
        };
        // a few hundred at most: drop the lot when it grows (cheap to make again)
        if self.made.len() > 400 {
            self.made.clear();
        }
        self.made.insert(key, r.clone());
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_drawn_tight_around_its_words() {
        let r = text(&TextStyle { text: "Hello".into(), size: 48.0, ..TextStyle::title("") });
        assert!(r.size.0 > 60 && r.size.0 < 400 && r.size.1 > 40 && r.size.1 < 120, "{:?}", r.size);
        assert!(r.rgba.chunks(4).any(|p| p[3] > 200 && p[0] > 200), "white letters");
        let boxed = text(&TextStyle { background: Some((0x000000, 0.6)), ..TextStyle::title("Hi") });
        let top_middle = ((2 * boxed.size.0 + boxed.size.0 / 2) * 4 + 3) as usize;
        assert!(boxed.rgba[top_middle] > 100, "the box reaches the edge");
    }

    #[test]
    fn shapes_fill_what_they_cover() {
        let r = shape(&ShapeStyle { shape: ShapeKind::Ellipse, width: 100.0, height: 50.0, fill: Some((0xFF0000, 1.0)), stroke: None, radius: 0.0 });
        let at = |x: u32, y: u32| r.rgba[((y * r.size.0 + x) * 4) as usize..][..4].to_vec();
        assert_eq!(at(50, 25), vec![255, 0, 0, 255], "the middle is filled");
        assert_eq!(at(1, 1)[3], 0, "the corner isn't");
    }
}

/// Emoji come in their own colours (the system's colour emoji font).
#[cfg(all(test, target_os = "macos"))]
mod emoji {
    #[test]
    fn emoji_keep_their_colours() {
        let mut s = crate::sequence::TextStyle::title("😀🎉");
        s.size = 120.0;
        let r = super::text(&s);
        let coloured = r.rgba.chunks(4).filter(|p| p[3] > 200 && (p[0] as i32 - p[2] as i32).abs() > 60).count();
        assert!(coloured > 2_000, "{coloured} coloured pixels");
    }
}
