//! Auto-reframe (docs/07-scenes-objects-canvas.md, "Formats"): a first layout for a object in a
//! newly added canvas. Full-canvas objects (backgrounds) fill the new canvas; everything else
//! keeps its arrangement, scaled to the new width and centred vertically. The user refines it.

use lightair_model::{Fit, Canvas, Layout};

pub fn reframe(l: &Layout, from: &Canvas, to: &Canvas) -> Layout {
    let (fw, fh, tw, th) = (from.width as f32, from.height as f32, to.width as f32, to.height as f32);
    let full = l.x.abs() < 1.0 && l.y.abs() < 1.0 && (l.width - fw).abs() < 2.0 && (l.height - fh).abs() < 2.0;
    if full {
        return Layout { x: 0.0, y: 0.0, width: tw, height: th, fit: Fit::Fill, ..*l };
    }
    let s = (tw / fw).min(th / fh);
    let (ox, oy) = ((tw - fw * s) / 2.0, (th - fh * s) / 2.0);
    Layout { x: ox + l.x * s, y: oy + l.y * s, width: l.width * s, height: l.height * s, ..*l }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backgrounds_fill_and_others_scale_into_a_centred_band() {
        let (land, vert) = (Canvas::landscape(), Canvas::vertical());
        let bg = reframe(&Layout::boxed(0.0, 0.0, 1920.0, 1080.0), &land, &vert);
        assert_eq!((bg.x, bg.y, bg.width, bg.height, bg.fit), (0.0, 0.0, 1080.0, 1920.0, Fit::Fill));
        let logo = reframe(&Layout::boxed(960.0, 540.0, 192.0, 108.0), &land, &vert);
        let s = 1080.0 / 1920.0;
        assert!((logo.width - 192.0 * s).abs() < 0.01);
        assert!((logo.y - ((1920.0 - 1080.0 * s) / 2.0 + 540.0 * s)).abs() < 0.01);
    }
}
