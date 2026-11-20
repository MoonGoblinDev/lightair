//! A Live2D model posed by a face. Needs a model: LIGHTAIR_LIVE2D (a .model3.json, not checked
//! in); skipped otherwise.

use std::path::PathBuf;

use glam::Quat;
use lightair_avatar::Face;
use lightair_avatar::live2d::Puppet;

fn assert_send<T: Send>() {}

#[test]
fn a_live2d_model_follows_a_face() {
    assert_send::<Puppet>();
    let Some(path) = std::env::var_os("LIGHTAIR_LIVE2D").map(PathBuf::from) else {
        eprintln!("LIGHTAIR_LIVE2D isn't set; skipping");
        return;
    };
    let t = std::time::Instant::now();
    let mut p = Puppet::load(&path).unwrap();
    eprintln!("loaded in {:?}: {} meshes, {} textures, expressions {:?}, bounds {:?}", t.elapsed(), p.meshes().len(), p.textures().len(), p.expression_names(), p.bounds);
    let centre = |p: &Puppet, name_has: &str| -> Option<f32> {
        let _ = name_has;
        let m = p.meshes().iter().filter(|m| m.opacity() > 0.5).max_by_key(|m| m.vertices().len())?;
        Some(m.vertices().iter().map(|v| v.position()[0]).sum::<f32>() / m.vertices().len() as f32)
    };
    p.update(&Face::default(), None, 1.0 / 60.0);
    let rest = centre(&p, "").unwrap();
    // the head turned toward the viewer's right, eyes shut, mouth open
    let face = Face { head: Quat::from_rotation_y(0.45), eye_open: [0.0, 0.0], mouth_open: 1.0, ..Face::default() };
    let t = std::time::Instant::now();
    for _ in 0..30 {
        p.update(&face, None, 1.0 / 60.0);
    }
    eprintln!("30 frames in {:?}", t.elapsed());
    assert!(p.meshes().iter().all(|m| m.vertices().iter().all(|v| v.position()[0].is_finite() && v.position()[1].is_finite())));
    let turned = centre(&p, "").unwrap();
    eprintln!("the largest mesh's centre: {rest:.4} → {turned:.4}");
    assert!((turned - rest).abs() > 1e-4, "the model moved");
    if let Some(e) = p.expression_names().first().cloned() {
        for _ in 0..30 {
            p.update(&face, Some(&e), 1.0 / 60.0);
        }
    }
}
