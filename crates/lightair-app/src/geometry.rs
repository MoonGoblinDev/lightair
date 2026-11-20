//! Canvas geometry for the editor (docs/07-scenes-objects-canvas.md, "Canvas"): rotated boxes,
//! hit-testing, and resize/crop computed in the object's own frame. libobs turns a object around
//! its box centre, clockwise for positive degrees (y points down).

use lightair_model::{Crop, Layout, ShapeKind};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

pub const CORNERS: [Corner; 4] = [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight];

/// Turns (x, y) by `deg` (clockwise on screen).
pub fn rotate((x, y): (f32, f32), deg: f32) -> (f32, f32) {
    let (s, c) = deg.to_radians().sin_cos();
    (x * c - y * s, x * s + y * c)
}

pub fn centre(l: &Layout) -> (f32, f32) {
    (l.x + l.width / 2.0, l.y + l.height / 2.0)
}

/// A point given relative to the box centre in the object's frame, in canvas coordinates.
fn to_canvas(l: &Layout, local: (f32, f32)) -> (f32, f32) {
    let (cx, cy) = centre(l);
    let (x, y) = rotate(local, l.rotation);
    (cx + x, cy + y)
}

/// A canvas point relative to the box centre in the object's frame.
fn to_local(l: &Layout, (x, y): (f32, f32)) -> (f32, f32) {
    let (cx, cy) = centre(l);
    rotate((x - cx, y - cy), -l.rotation)
}

pub fn corner(l: &Layout, c: Corner) -> (f32, f32) {
    let (hw, hh) = (l.width / 2.0, l.height / 2.0);
    let local = match c {
        Corner::TopLeft => (-hw, -hh),
        Corner::TopRight => (hw, -hh),
        Corner::BottomLeft => (-hw, hh),
        Corner::BottomRight => (hw, hh),
    };
    to_canvas(l, local)
}

/// The box outline, clockwise from the top-left corner.
pub fn outline(l: &Layout) -> [(f32, f32); 4] {
    [
        corner(l, Corner::TopLeft),
        corner(l, Corner::TopRight),
        corner(l, Corner::BottomRight),
        corner(l, Corner::BottomLeft),
    ]
}

/// The rotation handle: `offset` canvas units above the middle of the top edge.
pub fn rotate_handle(l: &Layout, offset: f32) -> (f32, f32) {
    to_canvas(l, (0.0, -l.height / 2.0 - offset))
}

pub fn contains(l: &Layout, p: (f32, f32)) -> bool {
    let (x, y) = to_local(l, p);
    x.abs() <= l.width / 2.0 && y.abs() <= l.height / 2.0
}

/// Places `edited` (a change to `orig` computed as if it weren't rotated) back under the
/// rotation: its centre moves by the same offset, turned with the object.
fn reanchor(orig: &Layout, edited: Layout) -> Layout {
    let (ox, oy) = centre(orig);
    let (nx, ny) = centre(&edited);
    let (wx, wy) = rotate((nx - ox, ny - oy), orig.rotation);
    Layout { x: ox + wx - edited.width / 2.0, y: oy + wy - edited.height / 2.0, ..edited }
}

/// Dragging `c` by a canvas delta resizes the box, the opposite corner staying put.
/// `keep_aspect` keeps the width:height ratio, driven by the width.
pub fn resize(l: &Layout, c: Corner, delta: (f32, f32), keep_aspect: bool) -> Layout {
    let (dx, dy) = rotate(delta, -l.rotation);
    let min = 8.0;
    let (mut left, mut top, mut right, mut bottom) = (l.x, l.y, l.x + l.width, l.y + l.height);
    match c {
        Corner::TopLeft => (left, top) = (left + dx, top + dy),
        Corner::TopRight => (right, top) = (right + dx, top + dy),
        Corner::BottomLeft => (left, bottom) = (left + dx, bottom + dy),
        Corner::BottomRight => (right, bottom) = (right + dx, bottom + dy),
    }
    if keep_aspect {
        let h = (right - left).abs() * l.height / l.width.max(1.0);
        match c {
            Corner::TopLeft | Corner::TopRight => top = bottom - h,
            _ => bottom = top + h,
        }
    }
    let edited = Layout {
        x: left.min(right - min),
        y: top.min(bottom - min),
        width: (right - left).max(min),
        height: (bottom - top).max(min),
        ..*l
    };
    reanchor(l, edited)
}

/// ⌥-dragging `c` crops the source (`size` in source pixels) instead of scaling it: the box
/// edge follows the pointer and the picture keeps its scale.
pub fn crop(l: &Layout, c: Corner, size: (f32, f32), delta: (f32, f32)) -> Layout {
    let (dx, dy) = rotate(delta, -l.rotation);
    let (sw, sh) = size;
    let k = l.crop;
    // Source pixels per canvas pixel in the visible (already cropped) region.
    let sx = (sw - (k.left + k.right) as f32).max(1.0) / l.width.max(1.0);
    let sy = (sh - (k.top + k.bottom) as f32).max(1.0) / l.height.max(1.0);
    let min = 8.0;
    let edge = |orig: u32, other: u32, extent: f32, d: f32| -> u32 {
        (orig as f32 + d).clamp(0.0, (extent - other as f32 - min).max(0.0)).round() as u32
    };
    let left_side = matches!(c, Corner::TopLeft | Corner::BottomLeft);
    let top_side = matches!(c, Corner::TopLeft | Corner::TopRight);
    let left = if left_side { edge(k.left, k.right, sw, dx * sx) } else { k.left };
    let right = if left_side { k.right } else { edge(k.right, k.left, sw, -dx * sx) };
    let top = if top_side { edge(k.top, k.bottom, sh, dy * sy) } else { k.top };
    let bottom = if top_side { k.bottom } else { edge(k.bottom, k.top, sh, -dy * sy) };
    let (dl, dr) = ((left as f32 - k.left as f32) / sx, (right as f32 - k.right as f32) / sx);
    let (dt, db) = ((top as f32 - k.top as f32) / sy, (bottom as f32 - k.bottom as f32) / sy);
    let edited = Layout {
        x: l.x + dl,
        y: l.y + dt,
        width: l.width - dl - dr,
        height: l.height - dt - db,
        crop: Crop { left, top, right, bottom },
        ..*l
    };
    reanchor(l, edited)
}

/// The axis-aligned bounds of a (possibly rotated) box: (left, top, right, bottom).
pub fn bounds(l: &Layout) -> (f32, f32, f32, f32) {
    let o = outline(l);
    let xs = o.map(|p| p.0);
    let ys = o.map(|p| p.1);
    let min = |v: [f32; 4]| v.into_iter().fold(f32::INFINITY, f32::min);
    let max = |v: [f32; 4]| v.into_iter().fold(f32::NEG_INFINITY, f32::max);
    (min(xs), min(ys), max(xs), max(ys))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Align {
    Left,
    Centre,
    Right,
    Top,
    Middle,
    Bottom,
}

/// Lines objects up by their visible bounds, against the bounds of them all.
pub fn align(layouts: &[Layout], how: Align) -> Vec<Layout> {
    let all: Vec<_> = layouts.iter().map(bounds).collect();
    let left = all.iter().map(|b| b.0).fold(f32::INFINITY, f32::min);
    let top = all.iter().map(|b| b.1).fold(f32::INFINITY, f32::min);
    let right = all.iter().map(|b| b.2).fold(f32::NEG_INFINITY, f32::max);
    let bottom = all.iter().map(|b| b.3).fold(f32::NEG_INFINITY, f32::max);
    layouts
        .iter()
        .zip(all)
        .map(|(l, b)| {
            let (dx, dy) = match how {
                Align::Left => (left - b.0, 0.0),
                Align::Right => (right - b.2, 0.0),
                Align::Centre => ((left + right) / 2.0 - (b.0 + b.2) / 2.0, 0.0),
                Align::Top => (0.0, top - b.1),
                Align::Bottom => (0.0, bottom - b.3),
                Align::Middle => (0.0, (top + bottom) / 2.0 - (b.1 + b.3) / 2.0),
            };
            Layout { x: l.x + dx, y: l.y + dy, ..*l }
        })
        .collect()
}

/// Spaces objects evenly between the outermost two (by visible bounds): equal gaps, along x
/// when `horizontal`, else y. Needs three or more; fewer are returned unchanged.
pub fn distribute(layouts: &[Layout], horizontal: bool) -> Vec<Layout> {
    if layouts.len() < 3 {
        return layouts.to_vec();
    }
    let span = |l: &Layout| {
        let b = bounds(l);
        if horizontal { (b.0, b.2) } else { (b.1, b.3) }
    };
    let mut order: Vec<usize> = (0..layouts.len()).collect();
    order.sort_by(|a, b| span(&layouts[*a]).0.total_cmp(&span(&layouts[*b]).0));
    let first = span(&layouts[order[0]]).0;
    let last = span(&layouts[*order.last().unwrap()]).1;
    let total: f32 = layouts.iter().map(|l| span(l).1 - span(l).0).sum();
    let gap = (last - first - total) / (layouts.len() - 1) as f32;
    let mut out = layouts.to_vec();
    let mut at = first;
    for i in order {
        let (start, end) = span(&layouts[i]);
        let d = at - start;
        if horizontal {
            out[i].x += d;
        } else {
            out[i].y += d;
        }
        at += end - start + gap;
    }
    out
}

/// Whether a object's visible bounds overlap the rectangle between two points.
pub fn touches(l: &Layout, a: (f32, f32), b: (f32, f32)) -> bool {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    let (l0, t0, r0, b0) = bounds(l);
    l0 <= x1 && r0 >= x0 && t0 <= y1 && b0 >= y0
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

/// The middle of an edge, in canvas coordinates.
#[cfg(test)]
pub fn edge_middle(l: &Layout, e: Edge) -> (f32, f32) {
    let (hw, hh) = (l.width / 2.0, l.height / 2.0);
    let local = match e {
        Edge::Left => (-hw, 0.0),
        Edge::Right => (hw, 0.0),
        Edge::Top => (0.0, -hh),
        Edge::Bottom => (0.0, hh),
    };
    to_canvas(l, local)
}

/// Dragging one edge crops that side only (the pointer's movement along the edge is ignored).
pub fn crop_edge(l: &Layout, e: Edge, size: (f32, f32), delta: (f32, f32)) -> Layout {
    let (dx, dy) = rotate(delta, -l.rotation);
    let (corner, local) = match e {
        Edge::Left => (Corner::TopLeft, (dx, 0.0)),
        Edge::Right => (Corner::TopRight, (dx, 0.0)),
        Edge::Top => (Corner::TopLeft, (0.0, dy)),
        Edge::Bottom => (Corner::BottomLeft, (0.0, dy)),
    };
    crop(l, corner, size, rotate(local, l.rotation))
}

/// The rectangle the picture actually fills inside a object's box: in Fit mode the (cropped)
/// source keeps its shape, so it's letterboxed; Original shows it at its own size. The frame and
/// handles sit on this, so they always hug the picture. `source` is the source's pixel size.
pub fn content_rect(l: &Layout, source: Option<(f32, f32)>) -> Layout {
    let Some((sw, sh)) = source.filter(|(w, h)| *w > 0.0 && *h > 0.0) else { return *l };
    let k = l.crop;
    let vw = (sw - (k.left + k.right) as f32).max(1.0);
    let vh = (sh - (k.top + k.bottom) as f32).max(1.0);
    let (w, h) = match l.fit {
        lightair_model::Fit::Fit => {
            let s = (l.width / vw).min(l.height / vh);
            (vw * s, vh * s)
        }
        lightair_model::Fit::Original => (vw, vh),
        lightair_model::Fit::Fill | lightair_model::Fit::Stretch => return *l,
    };
    // Same centre, same rotation.
    let (cx, cy) = centre(l);
    Layout { x: cx - w / 2.0, y: cy - h / 2.0, width: w, height: h, ..*l }
}

/// Crop mode: dragging inside the crop slides the picture under it (the box stays; the crop
/// window moves the other way), within the source's edges.
pub fn crop_pan(l: &Layout, size: (f32, f32), delta: (f32, f32)) -> Layout {
    let (dx, dy) = rotate(delta, -l.rotation);
    let k = l.crop;
    let sx = (size.0 - (k.left + k.right) as f32).max(1.0) / l.width.max(1.0);
    let sy = (size.1 - (k.top + k.bottom) as f32).max(1.0) / l.height.max(1.0);
    let (span_x, span_y) = ((k.left + k.right) as f32, (k.top + k.bottom) as f32);
    let left = (k.left as f32 - dx * sx).clamp(0.0, span_x).round();
    let top = (k.top as f32 - dy * sy).clamp(0.0, span_y).round();
    Layout {
        crop: Crop { left: left as u32, right: (span_x - left) as u32, top: top as u32, bottom: (span_y - top) as u32 },
        ..*l
    }
}

/// The corner radius and half extents of a object's shape, in its own frame (canvas units).
fn shape_extent(l: &Layout) -> ((f32, f32), f32) {
    let (hw, hh) = (l.width / 2.0, l.height / 2.0);
    let short = hw.min(hh);
    match l.shape.map(|s| (s.kind, s.radius)) {
        Some((ShapeKind::Circle, _)) => ((short, short), short),
        Some((ShapeKind::Pill, _)) => ((hw, hh), short),
        Some((ShapeKind::Rounded, r)) => ((hw, hh), r.clamp(0.0, 1.0) * short),
        None => ((hw, hh), 0.0),
    }
}

/// The object's shape as a closed polyline in canvas coordinates (its box when it has none).
pub fn shape_outline(l: &Layout) -> Vec<(f32, f32)> {
    let ((bx, by), r) = shape_extent(l);
    if r <= 0.5 {
        return outline(l).to_vec();
    }
    let mut pts = Vec::new();
    // Four arcs, clockwise from the top-right corner (y points down).
    for (cx, cy, start) in
        [(bx - r, -by + r, -90.0f32), (bx - r, by - r, 0.0), (-bx + r, by - r, 90.0), (-bx + r, -by + r, 180.0)]
    {
        for k in 0..=12 {
            let a = (start + 90.0 * k as f32 / 12.0).to_radians();
            pts.push(to_canvas(l, (cx + r * a.cos(), cy + r * a.sin())));
        }
    }
    pts
}

/// Crop handle positions: corners and edges, each with where it sits on the canvas.
pub type CropHandles = (Vec<(Corner, (f32, f32))>, Vec<(Edge, (f32, f32))>);

/// Where crop handles sit for a object's shape: on the shape itself (a circle's handles are on
/// the circle), corners at its 45° points, edges at its sides' middles.
pub fn crop_handles(l: &Layout) -> CropHandles {
    let ((bx, by), r) = shape_extent(l);
    let d = r * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
    let corners = [
        (Corner::TopLeft, (-bx + d, -by + d)),
        (Corner::TopRight, (bx - d, -by + d)),
        (Corner::BottomLeft, (-bx + d, by - d)),
        (Corner::BottomRight, (bx - d, by - d)),
    ]
    .map(|(c, p)| (c, to_canvas(l, p)))
    .to_vec();
    let edges =
        [(Edge::Left, (-bx, 0.0)), (Edge::Right, (bx, 0.0)), (Edge::Top, (0.0, -by)), (Edge::Bottom, (0.0, by))]
            .map(|(e, p)| (e, to_canvas(l, p)))
            .to_vec();
    (corners, edges)
}

/// A point in the object's own frame (from its centre), as the 3D tilt shows it: the same
/// rotation and perspective as Lightair's transform filter (positive X tilt: the bottom
/// recedes; positive Y tilt: the left side recedes; the eye two box-sizes away).
fn tilted(l: &Layout, (x, y): (f32, f32)) -> (f32, f32) {
    let (sa, ca) = l.tilt_x.to_radians().sin_cos();
    let (sb, cb) = l.tilt_y.to_radians().sin_cos();
    // Around X, then around Y.
    let (y1, z1) = (y * ca, y * sa);
    let (x2, z2) = (x * cb + z1 * sb, -x * sb + z1 * cb);
    let d = l.width.max(l.height) * 2.0;
    let w = (1.0 + z2 / d).max(0.05);
    (x2 / w, y1 / w)
}

/// The object's outline as the viewer sees it, tilt included (clockwise from top-left).
pub fn projected_outline(l: &Layout) -> [(f32, f32); 4] {
    let (hw, hh) = (l.width / 2.0, l.height / 2.0);
    [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|p| to_canvas(l, tilted(l, p)))
}

/// The 3D handle: `offset` canvas units below the middle of the bottom edge.
pub fn tilt_handle(l: &Layout, offset: f32) -> (f32, f32) {
    to_canvas(l, (0.0, l.height / 2.0 + offset))
}

/// A rotate-gizmo ring: which axis it turns around.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    /// Tilt around the horizontal axis (red).
    X,
    /// Tilt around the vertical axis (green).
    Y,
    /// Spin in the picture's plane: the 2D rotation (blue).
    Z,
}

/// The rotate gizmo's rings, centred on the object, radius `r` canvas units, as closed
/// polylines in canvas coordinates. X and Y are the 3D circles they turn around, seen from the
/// front: edge-on rings would be lines, so each keeps at least 30 % of its width to stay
/// grabbable, and widens as the object tilts.
pub fn gizmo_rings(l: &Layout, r: f32) -> [(Axis, Vec<(f32, f32)>); 3] {
    let (cx, cy) = centre(l);
    let ring = |rx: f32, ry: f32| -> Vec<(f32, f32)> {
        (0..48)
            .map(|k| {
                let a = (k as f32 / 48.0) * std::f32::consts::TAU;
                let (x, y) = rotate((rx * a.cos(), ry * a.sin()), l.rotation);
                (cx + x, cy + y)
            })
            .collect()
    };
    let open_x = l.tilt_y.to_radians().sin().abs().max(0.3);
    let open_y = l.tilt_x.to_radians().sin().abs().max(0.3);
    [(Axis::Z, ring(r, r)), (Axis::X, ring(r * open_x, r)), (Axis::Y, ring(r, r * open_y))]
}

/// Distance from `p` to the segment `a`–`b`.
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx * dx + dy * dy;
    let t = if len > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

/// The ring under `p`, if any (within `tol`), nearest first.
pub fn gizmo_hit(l: &Layout, r: f32, p: (f32, f32), tol: f32) -> Option<Axis> {
    gizmo_rings(l, r)
        .into_iter()
        .filter_map(|(axis, pts)| {
            let d = (0..pts.len())
                .map(|i| segment_distance(p, pts[i], pts[(i + 1) % pts.len()]))
                .fold(f32::INFINITY, f32::min);
            (d <= tol).then_some((axis, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(axis, _)| axis)
}

/// Snaps the edges a corner drag moves to the canvas's edges and centre lines (within `thr`
/// canvas px): returns the box and the guides to show ((vertical, position), as for moving).
/// Keeping the aspect, only the nearer edge snaps and the other follows the shape, from the
/// opposite corner. Turned or tilted boxes don't snap (their edges aren't along the canvas).
pub fn snap_resize(
    l: &Layout,
    c: Corner,
    extent: (f32, f32),
    thr: f32,
    keep_aspect: bool,
) -> (Layout, Vec<(bool, f32)>) {
    if l.rotation.abs() > 0.01 || l.tilt_x.abs() > 0.01 || l.tilt_y.abs() > 0.01 {
        return (*l, vec![]);
    }
    let min = 8.0;
    let left_side = matches!(c, Corner::TopLeft | Corner::BottomLeft);
    let top_side = matches!(c, Corner::TopLeft | Corner::TopRight);
    let (mut left, mut top, mut right, mut bottom) = (l.x, l.y, l.x + l.width, l.y + l.height);
    let nearest = |pos: f32, extent: f32| -> Option<f32> {
        [0.0, extent / 2.0, extent]
            .into_iter()
            .filter(|t| (pos - t).abs() < thr)
            .min_by(|a, b| (pos - a).abs().total_cmp(&(pos - b).abs()))
    };
    let mx = if left_side { left } else { right };
    let my = if top_side { top } else { bottom };
    let mut sx = nearest(mx, extent.0);
    let mut sy = nearest(my, extent.1);
    if keep_aspect && sx.is_some() && sy.is_some() {
        // one edge leads; the nearer one
        if (sx.unwrap() - mx).abs() <= (sy.unwrap() - my).abs() {
            sy = None;
        } else {
            sx = None;
        }
    }
    let ratio = l.height / l.width.max(1.0);
    if let Some(t) = sx {
        if left_side { left = t } else { right = t }
        if keep_aspect {
            let h = (right - left).abs() * ratio;
            if top_side { top = bottom - h } else { bottom = top + h }
        }
    }
    if let Some(t) = sy {
        if top_side { top = t } else { bottom = t }
        if keep_aspect {
            let w = (bottom - top).abs() / ratio.max(1e-6);
            if left_side { left = right - w } else { right = left + w }
        }
    }
    if right - left < min || bottom - top < min {
        return (*l, vec![]);
    }
    let mut guides = Vec::new();
    if let Some(t) = sx {
        guides.push((true, t));
    }
    if let Some(t) = sy {
        guides.push((false, t));
    }
    (Layout { x: left, y: top, width: right - left, height: bottom - top, ..*l }, guides)
}

/// How far the canvas may be panned on one axis: `stage` is the view's length, `canvas` the
/// canvas's length on screen. Zoomed out, the canvas's centre stays in the view; zoomed in, a
/// canvas edge can come at most to the middle of the view - never out into empty space.
pub fn clamp_pan(pan: f32, stage: f32, canvas: f32) -> f32 {
    let keep = stage.min(canvas) / 2.0;
    let base = (stage - canvas) / 2.0; // where the canvas starts when centred
    let (lo, hi) = (keep - canvas - base, stage - keep - base);
    pan.clamp(lo.min(hi), hi.max(lo))
}

/// An angle in (−180, 180].
pub fn normalise_degrees(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_snaps_the_dragged_corner_to_the_canvas() {
        let l = Layout::boxed(100.0, 100.0, 1815.0, 600.0);
        // The right edge 5 px from the canvas's: snaps; the bottom (700) isn't near a line.
        let (s, guides) = snap_resize(&l, Corner::BottomRight, (1920.0, 1080.0), 8.0, false);
        assert_eq!((s.x, s.width, s.height), (100.0, 1820.0, 600.0));
        assert_eq!(guides, vec![(true, 1920.0)]);
        // The top-left corner to the centre lines.
        let l = Layout::boxed(957.0, 543.0, 400.0, 300.0);
        let (s, guides) = snap_resize(&l, Corner::TopLeft, (1920.0, 1080.0), 8.0, false);
        assert_eq!((s.x, s.y, s.x + s.width, s.y + s.height), (960.0, 540.0, 1357.0, 843.0));
        assert_eq!(guides.len(), 2);
        // Keeping the aspect: the nearer edge leads, the other follows the shape.
        let l = Layout::boxed(0.0, 0.0, 1914.0, 957.0);
        let (s, guides) = snap_resize(&l, Corner::BottomRight, (1920.0, 1080.0), 8.0, true);
        assert_eq!((s.width, s.height), (1920.0, 960.0));
        assert_eq!(guides, vec![(true, 1920.0)]);
        // Far from every line, or turned: unchanged.
        let far = Layout::boxed(300.0, 300.0, 500.0, 300.0);
        assert_eq!(snap_resize(&far, Corner::BottomRight, (1920.0, 1080.0), 8.0, false).0, far);
        let turned = Layout { rotation: 10.0, ..Layout::boxed(0.0, 0.0, 1915.0, 1075.0) };
        assert!(snap_resize(&turned, Corner::BottomRight, (1920.0, 1080.0), 8.0, false).1.is_empty());
    }

    #[test]
    fn panning_stops_before_the_canvas_leaves_the_view() {
        // Zoomed out (canvas 400 in an 800 view): its centre stays in the view.
        assert_eq!(clamp_pan(10_000.0, 800.0, 400.0), 400.0);
        let (lo, hi) = (clamp_pan(-1e6, 800.0, 400.0), clamp_pan(1e6, 800.0, 400.0));
        let centre = |pan: f32| (800.0 - 400.0) / 2.0 + pan + 200.0;
        assert_eq!((centre(lo), centre(hi)), (0.0, 800.0));
        // Zoomed in (canvas 3000 in an 800 view): an edge reaches the middle at most.
        let (lo, hi) = (clamp_pan(-1e6, 800.0, 3000.0), clamp_pan(1e6, 800.0, 3000.0));
        let start = |pan: f32| (800.0 - 3000.0) / 2.0 + pan;
        assert_eq!((start(hi), start(lo) + 3000.0), (400.0, 400.0));
        assert_eq!(clamp_pan(25.0, 800.0, 3000.0), 25.0, "inside the range: untouched");
    }

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn rotated_corners_and_hits() {
        let l = Layout { rotation: 90.0, ..Layout::boxed(0.0, 0.0, 200.0, 100.0) };
        // Centre (100, 50); turned a quarter clockwise, the top-left corner goes to the top-right.
        assert!(close(corner(&l, Corner::TopLeft), (150.0, -50.0)), "{:?}", corner(&l, Corner::TopLeft));
        assert!(close(rotate_handle(&l, 20.0), (170.0, 50.0)));
        assert!(contains(&l, (100.0, -40.0)), "inside the turned box");
        assert!(!contains(&l, (10.0, 50.0)), "outside it, though inside the unturned one");
    }

    #[test]
    fn resize_keeps_the_opposite_corner_when_rotated() {
        for rotation in [0.0, 30.0, -120.0] {
            let l = Layout { rotation, ..Layout::boxed(100.0, 100.0, 400.0, 200.0) };
            let anchor = corner(&l, Corner::TopLeft);
            let r = resize(&l, Corner::BottomRight, rotate((50.0, 20.0), rotation), false);
            assert!((r.width - 450.0).abs() < 0.01 && (r.height - 220.0).abs() < 0.01, "{rotation}: {r:?}");
            assert!(close(corner(&r, Corner::TopLeft), anchor), "{rotation}: anchor moved");
        }
        let unrotated = resize(&Layout::boxed(0.0, 0.0, 100.0, 50.0), Corner::TopLeft, (10.0, 0.0), true);
        assert_eq!((unrotated.x, unrotated.width, unrotated.height), (10.0, 90.0, 45.0));
    }

    #[test]
    fn crop_keeps_the_picture_scale() {
        let l = Layout::boxed(100.0, 100.0, 960.0, 540.0);
        let c = crop(&l, Corner::TopLeft, (1920.0, 1080.0), (50.0, 25.0));
        assert_eq!((c.crop.left, c.crop.top, c.crop.right, c.crop.bottom), (100, 50, 0, 0));
        assert_eq!((c.x, c.y, c.width, c.height), (150.0, 125.0, 910.0, 515.0));
        let c2 = crop(&c, Corner::BottomRight, (1920.0, 1080.0), (-10.0, -10.0));
        assert_eq!((c2.crop.right, c2.crop.bottom), (20, 20));
        let back = crop(&c, Corner::TopLeft, (1920.0, 1080.0), (-500.0, -500.0));
        assert_eq!((back.crop.left, back.crop.top, back.x, back.y), (0, 0, 100.0, 100.0));
        // Rotated: the uncropped corner stays put.
        let r = Layout { rotation: 45.0, ..l };
        let anchor = corner(&r, Corner::BottomRight);
        let rc = crop(&r, Corner::TopLeft, (1920.0, 1080.0), rotate((50.0, 25.0), 45.0));
        assert_eq!((rc.crop.left, rc.crop.top), (100, 50));
        assert!(close(corner(&rc, Corner::BottomRight), anchor));
    }

    #[test]
    fn alignment_uses_visible_bounds() {
        let a = Layout::boxed(10.0, 10.0, 100.0, 50.0);
        let b = Layout::boxed(200.0, 100.0, 50.0, 50.0);
        let left = align(&[a, b], Align::Left);
        assert_eq!((left[0].x, left[1].x), (10.0, 10.0));
        let bottom = align(&[a, b], Align::Bottom);
        assert_eq!((bottom[0].y, bottom[1].y), (100.0, 100.0));
        let centre = align(&[a, b], Align::Centre);
        assert_eq!((centre[0].x + 50.0, centre[1].x + 25.0), (130.0, 130.0));
        // A object turned a quarter shows 50 wide: it aligns by that.
        let turned = Layout { rotation: 90.0, ..Layout::boxed(300.0, 0.0, 100.0, 50.0) };
        let r = align(&[a, turned], Align::Left);
        assert!((bounds(&r[1]).0 - 10.0).abs() < 0.01);
    }

    #[test]
    fn distribute_makes_equal_gaps() {
        let ls = [
            Layout::boxed(0.0, 0.0, 100.0, 10.0),
            Layout::boxed(500.0, 0.0, 100.0, 10.0),
            Layout::boxed(130.0, 0.0, 50.0, 10.0),
        ];
        let d = distribute(&ls, true);
        // Span 0..600, widths 250: two gaps of 175.
        assert_eq!((d[0].x, d[2].x, d[1].x), (0.0, 275.0, 500.0));
        assert_eq!(distribute(&ls[..2], true), ls[..2].to_vec(), "two layers stay put");
    }

    #[test]
    fn marquee_touches() {
        let l = Layout::boxed(100.0, 100.0, 50.0, 50.0);
        assert!(touches(&l, (0.0, 0.0), (120.0, 120.0)));
        assert!(touches(&l, (200.0, 200.0), (140.0, 140.0)), "either drag direction");
        assert!(!touches(&l, (0.0, 0.0), (90.0, 300.0)));
    }

    #[test]
    fn edges_crop_one_side() {
        let l = Layout::boxed(100.0, 100.0, 960.0, 540.0);
        let c = crop_edge(&l, Edge::Right, (1920.0, 1080.0), (-50.0, 30.0));
        assert_eq!((c.crop.left, c.crop.top, c.crop.right, c.crop.bottom), (0, 0, 100, 0));
        assert_eq!((c.x, c.y, c.width, c.height), (100.0, 100.0, 910.0, 540.0));
        let t = crop_edge(&l, Edge::Top, (1920.0, 1080.0), (80.0, 20.0));
        assert_eq!((t.crop.top, t.y, t.height), (40, 120.0, 520.0));
        // Turned a quarter clockwise, the bottom edge is on the left of the screen: dragging it
        // right (inwards) crops the bottom.
        let r = Layout { rotation: 90.0, ..l };
        let b = crop_edge(&r, Edge::Bottom, (1920.0, 1080.0), (20.0, 0.0));
        assert_eq!((b.crop.bottom, b.crop.left, b.crop.right), (40, 0, 0));
        assert!(close(edge_middle(&b, Edge::Top), edge_middle(&r, Edge::Top)), "the opposite edge stays");
    }

    #[test]
    fn the_frame_hugs_the_picture() {
        // A 16:9 box holding a 4:3 source in Fit: pillarboxed.
        let l = Layout::boxed(0.0, 0.0, 1600.0, 900.0);
        let c = content_rect(&l, Some((1440.0, 1080.0)));
        assert_eq!((c.x, c.y, c.width, c.height), (200.0, 0.0, 1200.0, 900.0));
        // Cropping changes the picture's shape: a square crop of it is a square.
        let sq = Layout { crop: Crop { left: 180, right: 180, top: 0, bottom: 0 }, ..l };
        let c = content_rect(&sq, Some((1440.0, 1080.0)));
        assert_eq!((c.x, c.width, c.height), (350.0, 900.0, 900.0));
        // Fill and Stretch fill the box; an unknown size leaves it alone.
        let fill = Layout { fit: lightair_model::Fit::Fill, ..l };
        assert_eq!(content_rect(&fill, Some((1440.0, 1080.0))), fill);
        assert_eq!(content_rect(&l, None), l);
    }

    #[test]
    fn panning_moves_the_crop_window() {
        let l =
            Layout { crop: Crop { left: 100, top: 0, right: 100, bottom: 0 }, ..Layout::boxed(0.0, 0.0, 860.0, 540.0) };
        // 1920 wide, 1720 visible in 860 px: 2 source px per canvas px. Dragging right 20 px
        // shows more of the left.
        let p = crop_pan(&l, (1920.0, 1080.0), (20.0, 0.0));
        assert_eq!((p.crop.left, p.crop.right), (60, 140));
        assert_eq!((p.x, p.width), (0.0, 860.0), "the box stays");
        let far = crop_pan(&l, (1920.0, 1080.0), (500.0, 0.0));
        assert_eq!((far.crop.left, far.crop.right), (0, 200), "stops at the source's edge");
    }

    #[test]
    fn handles_sit_on_the_shape() {
        use lightair_model::Shape;
        // A 200×100 box with a circle: radius 50, centred at (100, 50).
        let l = Layout { shape: Some(Shape::new(ShapeKind::Circle)), ..Layout::boxed(0.0, 0.0, 200.0, 100.0) };
        let (corners, edges) = crop_handles(&l);
        let left = edges.iter().find(|(e, _)| *e == Edge::Left).unwrap().1;
        assert!(close(left, (50.0, 50.0)), "on the circle, not the box: {left:?}");
        let tl = corners.iter().find(|(c, _)| *c == Corner::TopLeft).unwrap().1;
        let r = ((tl.0 - 100.0).powi(2) + (tl.1 - 50.0).powi(2)).sqrt();
        assert!((r - 50.0).abs() < 0.01, "corner handles on the circle too");
        let ring = shape_outline(&l);
        assert!(ring.iter().all(|p| (((p.0 - 100.0).powi(2) + (p.1 - 50.0).powi(2)).sqrt() - 50.0).abs() < 0.01));
        // No shape: the box's own corners.
        let plain = Layout::boxed(0.0, 0.0, 200.0, 100.0);
        assert!(close(crop_handles(&plain).0[0].1, (0.0, 0.0)));
        assert_eq!(shape_outline(&plain).len(), 4);
    }

    #[test]
    fn tilted_outline_matches_the_filter() {
        let flat = Layout::boxed(0.0, 0.0, 320.0, 180.0);
        assert!(projected_outline(&flat).iter().zip(outline(&flat)).all(|(a, b)| close(*a, b)));
        // Positive Y tilt: the left edge recedes, so it's shorter than the right.
        let y = Layout { tilt_y: 60.0, ..flat };
        let o = projected_outline(&y);
        let (left, right) = ((o[3].1 - o[0].1).abs(), (o[2].1 - o[1].1).abs());
        assert!(left < right, "{left} vs {right}");
        // Positive X tilt: the bottom recedes.
        let x = Layout { tilt_x: 60.0, ..flat };
        let o = projected_outline(&x);
        assert!((o[2].0 - o[3].0).abs() < (o[1].0 - o[0].0).abs());
    }

    #[test]
    fn gizmo_rings_and_hits() {
        // A flat 400×400 object centred at (200, 200), rings of radius 100.
        let l = Layout::boxed(0.0, 0.0, 400.0, 400.0);
        assert_eq!(gizmo_hit(&l, 100.0, (300.0, 200.0), 4.0), Some(Axis::Z), "the blue ring's right side");
        // Flat, the X ring is a tall narrow ellipse (30 % wide), the Y ring a wide flat one.
        assert_eq!(gizmo_hit(&l, 100.0, (230.0, 200.0), 4.0), Some(Axis::X));
        assert_eq!(gizmo_hit(&l, 100.0, (200.0, 230.0), 4.0), Some(Axis::Y));
        assert_eq!(gizmo_hit(&l, 100.0, (200.0, 200.0), 4.0), None, "the middle is for moving");
        // Tilting around Y opens the X ring.
        let t = Layout { tilt_y: 90.0, ..l };
        assert_eq!(gizmo_hit(&t, 100.0, (230.0, 200.0), 4.0), None);
        let z_or_x = gizmo_hit(&t, 100.0, (300.0, 200.0), 4.0);
        assert!(matches!(z_or_x, Some(Axis::X | Axis::Z)), "fully open, it lies on the blue ring");
    }

    #[test]
    fn angles() {
        assert_eq!(normalise_degrees(270.0), -90.0);
        assert_eq!(normalise_degrees(-190.0), 170.0);
        assert_eq!(normalise_degrees(180.0), 180.0);
    }
}
