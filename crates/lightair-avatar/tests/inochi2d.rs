//! An Inochi2D puppet posed by a face, drawn in software (masks, composites, blend modes) the way
//! the libobs renderer draws it. Needs a puppet: LIGHTAIR_INOCHI2D (an .inp or .inx, not checked
//! in); skipped otherwise. LIGHTAIR_INOCHI2D_OUT (a folder) keeps the pictures.

use std::path::PathBuf;

use glam::{Quat, Vec2};
use lightair_avatar::inochi2d::{Blend, Look, Op, Puppet};
use lightair_avatar::{Face, Framing};

fn assert_send<T: Send>() {}

/// Premultiplied RGBA, f32.
type Px = [f32; 4];

struct Canvas {
    w: usize,
    h: usize,
    layers: Vec<Vec<Px>>,
    mask: Vec<f32>,
}

fn blend(b: Blend, s: Px, d: Px) -> Px {
    let mut o = [0.0; 4];
    for k in 0..4 {
        let (sk, dk) = (s[k], d[k]);
        o[k] = match b {
            Blend::Normal => sk + dk * (1.0 - s[3]),
            Blend::Multiply => sk * dk + dk * (1.0 - s[3]),
            Blend::ColorDodge => sk * dk + dk,
            Blend::LinearDodge => sk + dk,
            Blend::Screen => sk + dk * (1.0 - sk),
            Blend::ClipToLower => sk * d[3] + dk * (1.0 - s[3]),
            Blend::SliceFromLower => sk * (1.0 - d[3]) - dk * (1.0 - s[3]),
        }
        .clamp(0.0, 1.0);
    }
    o
}

fn shade(t: Px, look: &Look) -> Px {
    let a = t[3];
    let mut o = [0.0; 4];
    for k in 0..3 {
        let screened = 1.0 - (1.0 - t[k]) * (1.0 - look.screen[k] * a);
        o[k] = screened * look.tint[k] * look.opacity;
    }
    o[3] = a * look.opacity;
    o
}

/// LIGHTAIR_INOCHI2D_PROBE=x,y: which parts draw that pixel of the pictures.
fn probe() -> Option<(usize, usize)> {
    let v = std::env::var("LIGHTAIR_INOCHI2D_PROBE").ok()?;
    let (x, y) = v.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

fn draw(p: &Puppet, w: usize, h: usize, framing: Framing) -> Vec<u8> {
    let (l, r, t, b) = p.frame(framing, w as f32 / h as f32, 1.0);
    let to_px = |v: [f32; 2]| Vec2::new((v[0] - l) / (r - l) * w as f32, (t - v[1]) / (t - b) * h as f32);
    let mut c = Canvas { w, h, layers: vec![vec![[0.0; 4]; w * h]], mask: vec![1.0; w * h] };
    let mut masking = false; // drawing into the mask
    let mut masked = false; // content limited by it
    let mut dodge = false;
    let mut threshold = 0.0;
    let mut in_composite = false;
    for op in p.ops() {
        match op {
            Op::BeginMasks { threshold: th, clear } => {
                c.mask.iter_mut().for_each(|m| *m = *clear);
                threshold = *th;
                masking = true;
                masked = false;
            }
            Op::Mask { dodge: d } => dodge = *d,
            Op::MaskedContent => {
                masking = false;
                masked = true;
            }
            Op::EndMasks => {
                masked = false;
                masking = false;
            }
            Op::BeginComposite { as_mask } => {
                if !as_mask {
                    c.layers.push(vec![[0.0; 4]; w * h]);
                    in_composite = true;
                }
            }
            Op::EndComposite { as_mask, look } => {
                if *as_mask {
                    continue;
                }
                in_composite = false;
                let layer = c.layers.pop().unwrap();
                let below = c.layers.last_mut().unwrap();
                for i in 0..w * h {
                    let m = if masked { c.mask[i] } else { 1.0 };
                    let mut s = shade(layer[i], look);
                    s.iter_mut().for_each(|x| *x *= m);
                    below[i] = blend(look.blend, s, below[i]);
                }
            }
            Op::Part { part, as_mask, look } => {
                let part_data = &p.parts()[*part];
                let Some(tex) = p.textures().get(part_data.texture) else { continue };
                let pos = &p.positions()[*part];
                let as_mask = *as_mask || masking;
                // LIGHTAIR_INOCHI2D_ONLY: draws only the parts with this in their names
                if !as_mask
                    && std::env::var("LIGHTAIR_INOCHI2D_ONLY").is_ok_and(|only| !part_data.name.contains(only.as_str()))
                {
                    continue;
                }
                let use_mask = masked && !in_composite;
                for tri in part_data.indices.chunks_exact(3) {
                    let [a, b2, c2] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                    let (pa, pb, pc) = (to_px(pos[a]), to_px(pos[b2]), to_px(pos[c2]));
                    let (ua, ub, uc) =
                        (Vec2::from(part_data.uvs[a]), Vec2::from(part_data.uvs[b2]), Vec2::from(part_data.uvs[c2]));
                    let area = (pb - pa).perp_dot(pc - pa);
                    if area.abs() < 1e-9 {
                        continue;
                    }
                    let x0 = pa.x.min(pb.x).min(pc.x).floor().max(0.0) as usize;
                    let x1 = (pa.x.max(pb.x).max(pc.x).ceil() as isize).clamp(0, w as isize) as usize;
                    let y0 = pa.y.min(pb.y).min(pc.y).floor().max(0.0) as usize;
                    let y1 = (pa.y.max(pb.y).max(pc.y).ceil() as isize).clamp(0, h as isize) as usize;
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                            let wa = (pb - q).perp_dot(pc - q) / area;
                            let wb = (pc - q).perp_dot(pa - q) / area;
                            let wc = 1.0 - wa - wb;
                            if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                                continue;
                            }
                            let uv = ua * wa + ub * wb + uc * wc;
                            let tx = ((uv.x * tex.width as f32) as i64).clamp(0, tex.width as i64 - 1) as usize;
                            let ty = ((uv.y * tex.height as f32) as i64).clamp(0, tex.height as i64 - 1) as usize;
                            let ti = (ty * tex.width as usize + tx) * 4;
                            let t: Px = std::array::from_fn(|k| tex.rgba[ti + k] as f32 / 255.0);
                            let i = y * w + x;
                            if as_mask {
                                if t[3] > threshold {
                                    c.mask[i] = if dodge { 0.0 } else { 1.0 };
                                }
                                continue;
                            }
                            if let Some((px, py)) = probe()
                                && (x, y) == (px, py)
                                && t[3] > 0.05
                            {
                                eprintln!(
                                    "probe: {} ({:?}, mask {}, a {:.2})",
                                    part_data.name, look.blend, use_mask, t[3]
                                );
                            }
                            let mut s = shade(t, look);
                            if use_mask {
                                s.iter_mut().for_each(|v| *v *= c.mask[i]);
                            }
                            let dst = c.layers.last_mut().unwrap();
                            dst[i] = blend(look.blend, s, dst[i]);
                        }
                    }
                }
            }
        }
    }
    // over a grey backdrop
    let mut out = vec![0u8; w * h * 4];
    for (i, px) in c.layers[0].iter().enumerate() {
        for k in 0..3 {
            out[i * 4 + k] = ((px[k] + 0.31 * (1.0 - px[3])).clamp(0.0, 1.0) * 255.0) as u8;
        }
        out[i * 4 + 3] = 255;
    }
    out
}

fn save(name: &str, rgba: &[u8], w: usize, h: usize) {
    if let Some(dir) = std::env::var_os("LIGHTAIR_INOCHI2D_OUT") {
        image::save_buffer(PathBuf::from(dir).join(name), rgba, w as u32, h as u32, image::ColorType::Rgba8).unwrap();
    }
}

#[test]
fn an_inochi2d_puppet_follows_a_face() {
    assert_send::<Puppet>();
    let Some(path) = std::env::var_os("LIGHTAIR_INOCHI2D").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_INOCHI2D isn't set; skipping");
        return;
    };
    let t = std::time::Instant::now();
    let mut p = Puppet::load(&path).unwrap();
    eprintln!(
        "loaded in {:?}: {} parts, {} textures, bounds {:?}, {} ops",
        t.elapsed(),
        p.parts().len(),
        p.textures().len(),
        p.bounds,
        p.ops().len()
    );
    for d in p.mapping.describe() {
        eprintln!("  {d}");
    }
    let (w, h) = (512, 512);
    for _ in 0..30 {
        p.update(&Face::default(), 1.0 / 60.0);
    }
    save("rest.png", &draw(&p, w, h, Framing::Bust), w, h);
    save("rest_full.png", &draw(&p, w, h, Framing::Full), w, h);
    let rest: Vec<Vec<[f32; 2]>> = p.positions().to_vec();
    let poses = [
        // toward the viewer's right
        ("yaw.png", Face { head: Quat::from_rotation_y(0.45), ..Face::default() }),
        // looking up
        ("pitch.png", Face { head: Quat::from_rotation_x(-0.25), ..Face::default() }),
        // tilted counter-clockwise as seen
        ("roll.png", Face { head: Quat::from_rotation_z(0.35), ..Face::default() }),
        ("shut_open.png", Face { eye_open: [0.0, 1.0], mouth_open: 1.0, ..Face::default() }),
        ("smile.png", Face { mouth_wide: 1.0, mouth_open: 0.3, gaze: Vec2::new(1.0, 0.0), ..Face::default() }),
    ];
    let t = std::time::Instant::now();
    for (name, face) in poses {
        for _ in 0..30 {
            p.update(&face, 1.0 / 60.0);
        }
        save(name, &draw(&p, w, h, Framing::Bust), w, h);
    }
    eprintln!("5 × 30 frames posed (and 5 drawn) in {:?}", t.elapsed());
    // the eyes: looking toward the viewer's right moves what moves (the irises) right
    let mut looked = Vec::new();
    for x in [-1.0, 1.0] {
        for _ in 0..10 {
            p.update(&Face { gaze: Vec2::new(x, 0.0), ..Face::default() }, 1.0 / 60.0);
        }
        looked.push(p.positions().to_vec());
    }
    let shift: Vec<f32> = looked[0]
        .iter()
        .zip(&looked[1])
        .flat_map(|(a, b)| a.iter().zip(b))
        .map(|(a, b)| b[0] - a[0])
        .filter(|d| d.abs() > 2.0)
        .collect();
    let mean = shift.iter().sum::<f32>() / shift.len().max(1) as f32;
    eprintln!("looking right moves {} vertices by {mean:.2} on average", shift.len());
    assert!(shift.is_empty() || mean > 0.0, "the eyes look the way the face does");
    assert!(p.positions().iter().flatten().all(|v| v[0].is_finite() && v[1].is_finite()));
    let moved = p
        .positions()
        .iter()
        .zip(&rest)
        .flat_map(|(a, b)| a.iter().zip(b))
        .filter(|(a, b)| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-3)
        .count();
    assert!(moved > 0, "the puppet moved");
}
