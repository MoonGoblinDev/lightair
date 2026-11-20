//! An Umamo puppet posed by a face, drawn in software (clip masks, multiply and screen colours,
//! blend modes) the way a libobs renderer would draw it. Needs a puppet: LIGHTAIR_UMA (a .uma, not
//! checked in); skipped otherwise. LIGHTAIR_UMA_OUT (a folder) keeps the pictures.

use std::path::PathBuf;

use glam::{Quat, Vec2};
use lightair_avatar::uma::{Blend, Drawable, Puppet, Texture};
use lightair_avatar::{Face, Framing};

fn assert_send<T: Send>() {}

/// Premultiplied RGBA, f32.
type Px = [f32; 4];

fn blend(b: Blend, s: Px, d: Px) -> Px {
    std::array::from_fn(|k| {
        match b {
            Blend::Normal => s[k] + d[k] * (1.0 - s[3]),
            Blend::Additive => s[k] + d[k],
            Blend::Multiply => s[k] * d[k] + d[k] * (1.0 - s[3]),
        }
        .clamp(0.0, 1.0)
    })
}

/// Calls `f(pixel index, texel)` for each pixel a drawable covers.
fn raster(
    d: &Drawable,
    tex: &Texture,
    w: usize,
    h: usize,
    to_px: &impl Fn([f32; 2]) -> Vec2,
    mut f: impl FnMut(usize, Px),
) {
    for tri in d.indices.chunks_exact(3) {
        let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (pa, pb, pc) = (to_px(d.positions[a]), to_px(d.positions[b]), to_px(d.positions[c]));
        let (ua, ub, uc) = (Vec2::from(d.uvs[a]), Vec2::from(d.uvs[b]), Vec2::from(d.uvs[c]));
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
                f(y * w + x, std::array::from_fn(|k| tex.rgba[ti + k] as f32 / 255.0));
            }
        }
    }
}

fn draw(p: &Puppet, w: usize, h: usize, framing: Framing) -> Vec<u8> {
    let frame = p.frame(framing, w as f32 / h as f32, 1.0);
    draw_meshes(p.drawables(), &p.draw_order(), p.textures(), frame, w, h)
}

fn draw_meshes(
    drawables: &[Drawable],
    order: &[usize],
    textures: &[Texture],
    (l, r, t, b): (f32, f32, f32, f32),
    w: usize,
    h: usize,
) -> Vec<u8> {
    let to_px = |v: [f32; 2]| Vec2::new((v[0] - l) / (r - l) * w as f32, (t - v[1]) / (t - b) * h as f32);
    let mut canvas: Vec<Px> = vec![[0.0; 4]; w * h];
    let mut mask = vec![0.0f32; w * h];
    let only = std::env::var("LIGHTAIR_UMA_ONLY").ok();
    for &i in order {
        let d = &drawables[i];
        if only.as_deref().is_some_and(|o| !d.name.contains(o)) {
            continue;
        }
        let Some(tex) = textures.get(d.texture) else { continue };
        let masked = !d.masks.is_empty();
        if masked {
            // coverage: the masks' alpha, unmasked, at full opacity
            mask.iter_mut().for_each(|m| *m = 0.0);
            for &m in &d.masks {
                let Some(md) = drawables.get(m).filter(|md| md.posed) else { continue };
                let Some(mt) = textures.get(md.texture) else { continue };
                raster(md, mt, w, h, &to_px, |i, t| mask[i] = t[3] + mask[i] * (1.0 - t[3]));
            }
        }
        raster(d, tex, w, h, &to_px, |i, t| {
            let mut alpha = t[3] * d.opacity;
            if masked {
                alpha *= if d.inverted_mask { 1.0 - mask[i] } else { mask[i] };
            }
            if alpha <= 0.0 {
                return;
            }
            let mut s = [0.0; 4];
            for k in 0..3 {
                let c = if t[3] > 0.0 { (t[k] / t[3]).min(1.0) } else { 0.0 };
                let tinted = c * d.multiply[k];
                s[k] = (tinted + d.screen[k] - tinted * d.screen[k]) * alpha;
            }
            s[3] = alpha;
            canvas[i] = blend(d.blend, s, canvas[i]);
        });
    }
    // over a grey backdrop
    let mut out = vec![0u8; w * h * 4];
    for (i, px) in canvas.iter().enumerate() {
        for k in 0..3 {
            out[i * 4 + k] = ((px[k] + 0.31 * (1.0 - px[3])).clamp(0.0, 1.0) * 255.0) as u8;
        }
        out[i * 4 + 3] = 255;
    }
    out
}

fn save(name: &str, rgba: &[u8], w: usize, h: usize) {
    if let Some(dir) = std::env::var_os("LIGHTAIR_UMA_OUT") {
        image::save_buffer(PathBuf::from(dir).join(name), rgba, w as u32, h as u32, image::ColorType::Rgba8).unwrap();
    }
}

/// LIGHTAIR_UMA_LIVE2D (a .model3.json, the model the .uma came from): the same poses drawn from
/// the Live2D model by mocari, to compare with.
fn draw_live2d(faces: &[(&str, Face)], w: usize, h: usize) {
    use lightair_avatar::live2d;
    use lightair_avatar::mocari::moc3::Moc3DrawableBlendMode;
    let Some(path) = std::env::var_os("LIGHTAIR_UMA_LIVE2D").map(PathBuf::from) else { return };
    let mut p = live2d::Puppet::load(&path).unwrap();
    let textures: Vec<Texture> = p
        .textures()
        .iter()
        .map(|t| Texture {
            width: t.width(),
            height: t.height(),
            rgba: t
                .rgba()
                .chunks_exact(4)
                .flat_map(|c| {
                    let pm = |v: u8| ((v as u32 * c[3] as u32 + 127) / 255) as u8;
                    [pm(c[0]), pm(c[1]), pm(c[2]), c[3]]
                })
                .collect(),
        })
        .collect();
    for (name, face) in faces {
        for _ in 0..30 {
            p.update(face, None, 1.0 / 60.0);
        }
        let drawables: Vec<Drawable> = p
            .meshes()
            .iter()
            .enumerate()
            .map(|(i, m)| Drawable {
                name: format!("{i}"),
                uvs: m.vertices().iter().map(|v| v.uv()).collect(),
                indices: m.indices().iter().map(|&i| i as u32).collect(),
                texture: m.texture_index().max(0) as usize,
                positions: m.vertices().iter().map(|v| v.position()).collect(),
                opacity: m.opacity(),
                multiply: m.multiply_color(),
                screen: m.screen_color(),
                blend: match m.blend_mode() {
                    Moc3DrawableBlendMode::Normal => Blend::Normal,
                    Moc3DrawableBlendMode::Additive => Blend::Additive,
                    Moc3DrawableBlendMode::Multiplicative => Blend::Multiply,
                },
                masks: m.masks().iter().filter(|&&k| k >= 0).map(|&k| k as usize).collect(),
                inverted_mask: m.is_inverted_mask(),
                posed: true,
            })
            .collect();
        let order: Vec<usize> = p.draw_order().into_iter().filter(|&i| drawables[i].opacity > 0.0).collect();
        let frame = p.frame(Framing::Bust, w as f32 / h as f32, 1.0);
        save(&format!("live2d_{name}"), &draw_meshes(&drawables, &order, &textures, frame, w, h), w, h);
    }
}

fn positions(p: &Puppet) -> Vec<Vec<[f32; 2]>> {
    p.drawables().iter().map(|d| d.positions.clone()).collect()
}

#[test]
fn an_umamo_puppet_follows_a_face() {
    assert_send::<Puppet>();
    let Some(path) = std::env::var_os("LIGHTAIR_UMA").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_UMA isn't set; skipping");
        return;
    };
    let t = std::time::Instant::now();
    let mut p = Puppet::load(&path).unwrap();
    eprintln!(
        "loaded in {:?}: {} drawables ({} drawn), {} textures, {} parameters, bounds {:?}, centre {}",
        t.elapsed(),
        p.drawables().len(),
        p.draw_order().len(),
        p.textures().len(),
        p.parameters().len(),
        p.bounds,
        p.centre_x
    );
    eprintln!("the face drives {:?}", p.driven());
    let (w, h) = (512, 512);
    p.update(&Face::default(), 1.0 / 60.0);
    save("rest.png", &draw(&p, w, h, Framing::Bust), w, h);
    save("rest_full.png", &draw(&p, w, h, Framing::Full), w, h);
    let rest = positions(&p);
    let poses = [
        // toward the viewer's right
        ("yaw.png", Face { head: Quat::from_rotation_y(0.45), ..Face::default() }),
        // looking up
        ("pitch.png", Face { head: Quat::from_rotation_x(-0.25), ..Face::default() }),
        // tilted counter-clockwise as seen
        ("roll.png", Face { head: Quat::from_rotation_z(0.35), ..Face::default() }),
        ("shut_open.png", Face { eye_open: [0.0, 0.0], mouth_open: 1.0, ..Face::default() }),
        ("wink.png", Face { eye_open: [0.0, 1.0], ..Face::default() }),
        ("smile.png", Face { mouth_wide: 1.0, mouth_open: 0.3, gaze: Vec2::new(1.0, 0.0), ..Face::default() }),
    ];
    draw_live2d(&poses, w, h);
    let t = std::time::Instant::now();
    for (name, face) in poses {
        for _ in 0..30 {
            p.update(&face, 1.0 / 60.0);
        }
        save(name, &draw(&p, w, h, Framing::Bust), w, h);
    }
    eprintln!("6 × 30 frames posed (and 6 drawn) in {:?}", t.elapsed());
    let t = std::time::Instant::now();
    for _ in 0..60 {
        p.update(&Face { head: Quat::from_rotation_y(0.3), ..Face::default() }, 1.0 / 60.0);
    }
    eprintln!("a frame posed in {:?}", t.elapsed() / 60);

    // the head turned toward the viewer's right moves the face that way
    let centre = |p: &Puppet| -> f32 {
        let (mut sum, mut n) = (0.0, 0);
        for &i in &p.draw_order() {
            for v in
                p.drawables()[i].positions.iter().filter(|v| v[1] > p.bounds.1.y - (p.bounds.1.y - p.bounds.0.y) * 0.2)
            {
                sum += v[0];
                n += 1;
            }
        }
        sum / n.max(1) as f32
    };
    p.update(&Face::default(), 1.0 / 60.0);
    let ahead = centre(&p);
    p.update(&Face { head: Quat::from_rotation_y(0.45), ..Face::default() }, 1.0 / 60.0);
    let turned = centre(&p);
    eprintln!("the head's middle: {ahead:.4} → {turned:.4} turned toward the viewer's right");
    assert!(turned > ahead, "the head turns the way the face does");

    // the eyes: looking toward the viewer's right moves what moves (the irises) right
    let mut looked = Vec::new();
    for x in [-1.0, 1.0] {
        p.update(&Face { gaze: Vec2::new(x, 0.0), ..Face::default() }, 1.0 / 60.0);
        looked.push(positions(&p));
    }
    let shift: Vec<f32> = looked[0]
        .iter()
        .zip(&looked[1])
        .flat_map(|(a, b)| a.iter().zip(b))
        .map(|(a, b)| b[0] - a[0])
        .filter(|d| d.abs() > 1e-4)
        .collect();
    let mean = shift.iter().sum::<f32>() / shift.len().max(1) as f32;
    eprintln!("looking right moves {} vertices by {mean:.4} on average", shift.len());
    assert!(shift.is_empty() || mean > 0.0, "the eyes look the way the face does");
    assert!(p.drawables().iter().flat_map(|d| &d.positions).all(|v| v[0].is_finite() && v[1].is_finite()));
    let moved = positions(&p)
        .iter()
        .zip(&rest)
        .flat_map(|(a, b)| a.iter().zip(b))
        .filter(|(a, b)| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1e-5)
        .count();
    assert!(moved > 0, "the puppet moved");
}

#[test]
fn a_bad_file_is_an_error() {
    let dir = std::env::temp_dir().join(format!("lightair-uma-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.uma");
    std::fs::write(&path, b"not a zip").unwrap();
    assert!(Puppet::load(&path).is_err());
    assert!(Puppet::load(&dir.join("missing.uma")).is_err());
    std::fs::remove_dir_all(&dir).ok();
}
