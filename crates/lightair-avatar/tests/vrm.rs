//! A VRM model loaded, posed and drawn in software (flat, textured, with depth) to check the
//! pose by eye. Needs a model: LIGHTAIR_VRM (not checked in); skipped without one. Pictures go
//! to LIGHTAIR_VRM_OUT (a folder) when it's set.

use std::path::PathBuf;
use std::sync::Arc;

use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use lightair_avatar::vrm::Model;
use lightair_avatar::{Avatar, Controls, Face, Framing};

fn model() -> Option<Arc<Model>> {
    let path = PathBuf::from(std::env::var_os("LIGHTAIR_VRM")?);
    let bytes = std::fs::read(&path).unwrap();
    Some(Arc::new(Model::load(&bytes).unwrap()))
}

/// A software picture of the avatar: textured, unlit, depth-tested.
fn draw(a: &Avatar, w: usize, h: usize, framing: Framing) -> Vec<u8> {
    let cam = a.camera(framing, w as f32 / h as f32, 1.0);
    let vp = Mat4::perspective_rh(cam.fovy.to_radians(), w as f32 / h as f32, cam.near, cam.far) * cam.view();
    let mut color = vec![0u8; w * h * 4];
    let mut depth = vec![f32::INFINITY; w * h];
    let m = &a.model;
    for (pi, p) in m.primitives.iter().enumerate() {
        let mat = &m.materials[p.material];
        let tex = mat.base_texture.map(|t| &m.images[t]);
        let screen: Vec<(Vec2, f32)> = a.positions[pi]
            .iter()
            .map(|v| {
                let c = vp * Vec4::new(v.x, v.y, v.z, 1.0);
                let n = c.truncate() / c.w;
                (Vec2::new((n.x * 0.5 + 0.5) * w as f32, (0.5 - n.y * 0.5) * h as f32), n.z)
            })
            .collect();
        for t in p.indices.as_chunks::<3>().0 {
            let [i0, i1, i2] = [t[0] as usize, t[1] as usize, t[2] as usize];
            let (a0, a1, a2) = (screen[i0], screen[i1], screen[i2]);
            let area = (a1.0 - a0.0).perp_dot(a2.0 - a0.0);
            if area.abs() < 1e-6 {
                continue;
            }
            let min = a0.0.min(a1.0).min(a2.0).max(Vec2::ZERO);
            let max = a0.0.max(a1.0).max(a2.0).min(Vec2::new(w as f32 - 1.0, h as f32 - 1.0));
            for y in min.y as usize..=max.y as usize {
                for x in min.x as usize..=max.x as usize {
                    let pt = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let b0 = (a1.0 - pt).perp_dot(a2.0 - pt) / area;
                    let b1 = (a2.0 - pt).perp_dot(a0.0 - pt) / area;
                    let b2 = 1.0 - b0 - b1;
                    if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                        continue;
                    }
                    let z = a0.1 * b0 + a1.1 * b1 + a2.1 * b2;
                    let k = y * w + x;
                    if z >= depth[k] {
                        continue;
                    }
                    let uv = Vec2::from(p.uvs[i0]) * b0 + Vec2::from(p.uvs[i1]) * b1 + Vec2::from(p.uvs[i2]) * b2;
                    let mut c = [mat.base_color[0] * 255.0, mat.base_color[1] * 255.0, mat.base_color[2] * 255.0, mat.base_color[3] * 255.0];
                    if let Some(img) = tex {
                        let tx = ((uv.x.rem_euclid(1.0)) * img.width as f32) as usize % img.width as usize;
                        let ty = ((uv.y.rem_euclid(1.0)) * img.height as f32) as usize % img.height as usize;
                        let o = (ty * img.width as usize + tx) * 4;
                        for (i, ch) in c.iter_mut().enumerate() {
                            *ch *= img.rgba[o + i] as f32 / 255.0;
                        }
                    }
                    if c[3] < 128.0 {
                        continue;
                    }
                    depth[k] = z;
                    let n = (a.normals[pi][i0] * b0 + a.normals[pi][i1] * b1 + a.normals[pi][i2] * b2).normalize_or(Vec3::Z);
                    let light = 0.7 + 0.3 * n.dot(Vec3::new(0.3, 0.5, 0.8).normalize()).max(0.0);
                    color[k * 4..k * 4 + 4].copy_from_slice(&[(c[0] * light) as u8, (c[1] * light) as u8, (c[2] * light) as u8, 255]);
                }
            }
        }
    }
    color
}

fn save(name: &str, rgba: &[u8], w: usize, h: usize) {
    if let Some(dir) = std::env::var_os("LIGHTAIR_VRM_OUT") {
        let path = PathBuf::from(dir).join(name);
        image::save_buffer(&path, rgba, w as u32, h as u32, image::ColorType::Rgba8).unwrap();
    }
}

#[test]
fn a_vrm_is_posed_by_a_face() {
    let Some(model) = model() else {
        eprintln!("LIGHTAIR_VRM isn't set; skipping");
        return;
    };
    assert!(!model.primitives.is_empty() && model.humanoid.contains_key("head"));
    assert!(model.expressions.contains_key("blink") || model.expressions.contains_key("blinkLeft"));
    eprintln!("{} (VRM {}): {} primitives, {} spring joints, {} colliders", model.meta.name, model.version, model.primitives.len(), model.joints.len(), model.colliders.len());
    let mut a = Avatar::new(model.clone());
    save("initial.png", &draw(&a, 480, 640, Framing::Bust), 480, 640);
    // facing the camera, at rest: the head above the hips, the arms down
    let mut c = Controls::default();
    for _ in 0..60 {
        a.update(&c, 1.0 / 60.0);
    }
    let head = a.bone_position("head").unwrap();
    let hips = a.bone_position("hips").unwrap();
    let hand = a.bone_position("leftHand").unwrap();
    let shoulder = a.bone_position("leftUpperArm").unwrap();
    assert!(head.y > hips.y + 0.3, "{head} {hips}");
    assert!(hand.y < shoulder.y - 0.2 && hand.x > 0.0, "the left hand hangs at the model's left (+x): {hand} {shoulder}");
    assert!(a.positions.iter().flatten().all(|p| p.is_finite()));
    let (w, h) = (480, 640);
    save("rest.png", &draw(&a, w, h, Framing::Bust), w, h);
    save("full.png", &draw(&a, w, h, Framing::Full), w, h);

    // the head turned to its left (+x), eyes shut, mouth open
    c.face = Face { head: Quat::from_rotation_y(0.5), eye_open: [0.0, 0.0], mouth_open: 1.0, ..Face::default() };
    for _ in 0..30 {
        a.update(&c, 1.0 / 60.0);
    }
    let nose_dir = a.world[model.humanoid["head"]].transform_vector3(Vec3::Z);
    assert!(nose_dir.x > 0.1, "the head faces its left: {nose_dir}");
    save("turned.png", &draw(&a, w, h, Framing::Bust), w, h);

    // an emotion
    c.face = Face::default();
    c.expression = Some(("happy".into(), 1.0));
    for _ in 0..10 {
        a.update(&c, 1.0 / 60.0);
    }
    save("happy.png", &draw(&a, w, h, Framing::Head), w, h);
    assert!(a.positions.iter().flatten().all(|p| p.is_finite()));
}

/// Poses for checking a body tracker by eye (LIGHTAIR_VRM_OUT): the left arm forward, the body
/// turned to its left.
#[test]
fn body_poses() {
    let Some(model) = model() else { return };
    if std::env::var_os("LIGHTAIR_VRM_OUT").is_none() {
        return;
    }
    let mut a = Avatar::new(model);
    let (w, h) = (480, 640);
    let poses: [(&str, Vec<(String, Quat)>); 4] = [
        ("arm_forward.png", vec![("leftUpperArm".into(), Quat::from_rotation_y(-90f32.to_radians()))]),
        (
            "pose.png",
            vec![
                ("leftUpperArm".into(), Quat::from_rotation_y(-60f32.to_radians()) * Quat::from_rotation_z(-30f32.to_radians())),
                ("rightUpperArm".into(), Quat::from_rotation_z(50f32.to_radians())),
                ("rightLowerArm".into(), Quat::from_rotation_z(80f32.to_radians())),
                ("leftUpperLeg".into(), Quat::from_rotation_x(-40f32.to_radians())),
                ("leftLowerLeg".into(), Quat::from_rotation_x(60f32.to_radians())),
            ],
        ),
        ("arm_up.png", vec![("leftUpperArm".into(), Quat::from_rotation_z(70f32.to_radians()))]),
        ("turned_body.png", vec![("hips".into(), Quat::from_rotation_y(20f32.to_radians()))]),
    ];
    for (name, bones) in poses {
        let c = Controls { bones, ..Controls::default() };
        for _ in 0..30 {
            a.update(&c, 1.0 / 60.0);
        }
        save(name, &draw(&a, w, h, Framing::Full), w, h);
        if name == "pose.png" {
            let hips = a.bone_position("hips").unwrap();
            for b in ["leftUpperArm", "leftLowerArm", "leftHand", "rightUpperArm", "rightLowerArm", "rightHand", "leftUpperLeg", "leftLowerLeg", "leftFoot"] {
                eprintln!("true {b:14} {:.3}", a.bone_position(b).unwrap() - hips);
            }
        }
    }
}

/// A body tracker's joints (LIGHTAIR_BODY_JSON: {"j3": [[x,y,z]|null ×17] | null, "j2":
/// [[x,y,conf]|null ×17], "aspect"}) put on the avatar and drawn (LIGHTAIR_VRM_OUT), to check
/// the round trip: a pose drawn, tracked, and drawn again from what was tracked.
#[test]
fn body_from_tracking() {
    use lightair_avatar::body::{BodyFrame, BodyMode, BodyState, JOINTS};
    let (Some(model), Some(json)) = (model(), std::env::var_os("LIGHTAIR_BODY_JSON")) else { return };
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(json).unwrap()).unwrap();
    let f3 = |x: &serde_json::Value| x.as_f64().unwrap() as f32;
    let mut j2 = [None; JOINTS];
    for (i, p) in v["j2"].as_array().unwrap().iter().enumerate() {
        if let Some(a) = p.as_array() {
            j2[i] = Some((glam::Vec2::new(f3(&a[0]), f3(&a[1])), f3(&a[2])));
        }
    }
    let joints3d = v["j3"].as_array().map(|a| {
        let mut j = [None; JOINTS];
        for (i, p) in a.iter().enumerate() {
            if let Some(p) = p.as_array() {
                j[i] = Some(Vec3::new(f3(&p[0]), f3(&p[1]), f3(&p[2])));
            }
        }
        j
    });
    let frame = |id| BodyFrame { joints3d, joints2d: j2, hands: [None, None], hands3d: [None, None], aspect: f3(&v["aspect"]), frame_id: id };
    let mut a = Avatar::new(model);
    let mut state = BodyState::default();
    for i in 0..60 {
        state.step(Some(&frame(i)), false, 1.0 / 30.0);
        let (bones, torso) = a.body_bones(&state, BodyMode::Full);
        a.update(&Controls { bones, torso, ..Controls::default() }, 1.0 / 30.0);
    }
    eprintln!("seen {:?}", state.seen);
    save("retarget.png", &draw(&a, 480, 640, Framing::Half), 480, 640);
    for (x, y, jx, jy) in [("leftUpperArm", "leftLowerArm", 11, 12), ("leftLowerArm", "leftHand", 12, 13), ("rightUpperArm", "rightLowerArm", 14, 15), ("rightLowerArm", "rightHand", 15, 16), ("leftUpperLeg", "leftLowerLeg", 1, 2), ("leftLowerLeg", "leftFoot", 2, 3)] {
        let got = (a.bone_position(y).unwrap() - a.bone_position(x).unwrap()).normalize();
        let (Some(b), Some(a0)) = (state.joints[jy], state.joints[jx]) else { continue };
        let want = (b - a0).normalize();
        eprintln!("{x:14} got {got:.2} want {want:.2} off {:.1}°", got.angle_between(want).to_degrees());
    }
}

/// A desk camera's view (from a real one): shoulders, neck and nose, one wrist raised by the
/// head, the elbow below the picture. Mirrored, the avatar raises its other arm, the elbow
/// worked out.
#[test]
fn a_hand_raised_with_the_elbow_out_of_view() {
    use lightair_avatar::body::{BodyFrame, BodyMode, BodyState, J, JOINTS};
    let Some(model) = model() else { return };
    // (3D as the tracker gives it: x the person's left, y up; the picture's visibility beside)
    let mut j3 = [None; JOINTS];
    let mut j2 = [None; JOINTS];
    let mut put = |j: J, x: f32, y: f32, seen: f32| {
        j3[j as usize] = Some(Vec3::new(x, y, 0.0));
        j2[j as usize] = Some((glam::Vec2::new(0.5 + x * 0.5, 0.5 - y * 0.5), seen));
    };
    put(J::Root, 0.0, 0.0, 0.1);
    put(J::CenterShoulder, 0.0, 0.5, 0.8);
    put(J::CenterHead, 0.02, 0.72, 0.8);
    put(J::LeftShoulder, 0.17, 0.52, 0.8);
    put(J::RightShoulder, -0.17, 0.48, 0.8);
    put(J::LeftWrist, 0.3, 0.62, 0.8);
    let frame = |id| BodyFrame { joints3d: Some(j3), joints2d: j2, hands: [None, None], hands3d: [None, None], aspect: 16.0 / 9.0, frame_id: id };
    let mut a = Avatar::new(model);
    let mut state = BodyState::default();
    for i in 0..60 {
        state.step(Some(&frame(i)), true, 1.0 / 30.0);
        let (bones, torso) = a.body_bones(&state, BodyMode::Upper);
        a.update(&Controls { bones, torso, ..Controls::default() }, 1.0 / 30.0);
    }
    let (hand, shoulder) = (a.bone_position("rightHand").unwrap(), a.bone_position("rightUpperArm").unwrap());
    let elbow = a.bone_position("rightLowerArm").unwrap();
    let (lhand, lshoulder) = (a.bone_position("leftHand").unwrap(), a.bone_position("leftUpperArm").unwrap());
    eprintln!("seen {:?}; right hand {hand:.2} elbow {elbow:.2} shoulder {shoulder:.2}", state.seen);
    let (w, h) = (480, 640);
    save("raised.png", &draw(&a, w, h, Framing::Half), w, h);
    assert!(hand.y > shoulder.y, "the right hand is up: {hand} vs {shoulder}");
    assert!(elbow.y < shoulder.y && elbow.x < shoulder.x, "the elbow is down and out: {elbow} vs {shoulder}");
    assert!(lhand.y < lshoulder.y - 0.2, "the other arm stays down");
}

/// A raised left hand, palm to the camera, then its back: the avatar's hand turns over (its
/// knuckles' line flips from one side to the other).
#[test]
fn palm_and_back_of_the_hand() {
    use lightair_avatar::body::{BodyFrame, BodyMode, BodyState, HAND_POINTS, J, JOINTS};
    let Some(model) = model() else { return };
    let mut across_x = Vec::new();
    for (name, across) in [("palm", 1.0f32), ("back", -1.0f32)] {
        let mut j3 = [None; JOINTS];
        let mut j2 = [None; JOINTS];
        let mut put = |j: J, p: Vec3| {
            j3[j as usize] = Some(p);
            j2[j as usize] = Some((glam::Vec2::new(0.5 + p.x * 0.5, 0.5 - p.y * 0.5), 0.9));
        };
        put(J::Root, Vec3::ZERO);
        put(J::CenterShoulder, Vec3::new(0.0, 0.5, 0.0));
        put(J::LeftShoulder, Vec3::new(0.18, 0.5, 0.0));
        put(J::RightShoulder, Vec3::new(-0.18, 0.5, 0.0));
        put(J::LeftElbow, Vec3::new(0.35, 0.4, 0.0));
        put(J::LeftWrist, Vec3::new(0.35, 0.65, 0.0));
        put(J::RightElbow, Vec3::new(-0.2, 0.25, 0.0));
        put(J::RightWrist, Vec3::new(-0.2, 0.0, 0.0));
        // fingers up; index on one side, little on the other
        let mut h3 = [Vec3::ZERO; HAND_POINTS];
        for f in 0..5 {
            for i in 0..4 {
                h3[1 + f * 4 + i] = Vec3::new(across * (f as f32 - 1.0) * 0.018, 0.04 + i as f32 * 0.025, 0.0);
            }
        }
        let h2 = std::array::from_fn(|k| (glam::Vec2::new(0.7 + h3[k].x, 0.2 - h3[k].y), 0.9));
        let frame = |id| BodyFrame { joints3d: Some(j3), joints2d: j2, hands: [Some(h2), None], hands3d: [Some(h3), None], aspect: 1.0, frame_id: id };
        let mut a = Avatar::new(model.clone());
        let mut state = BodyState::default();
        for i in 0..60 {
            state.step(Some(&frame(i)), false, 1.0 / 30.0);
            let (bones, torso) = a.body_bones(&state, BodyMode::Upper);
            a.update(&Controls { bones, torso, ..Controls::default() }, 1.0 / 30.0);
        }
        let line = a.bone_position("leftLittleProximal").unwrap() - a.bone_position("leftIndexProximal").unwrap();
        let up = a.bone_position("leftMiddleProximal").unwrap() - a.bone_position("leftHand").unwrap();
        eprintln!("{name}: knuckles {line:.3}, hand {up:.3}");
        assert!(up.y > 0.0, "{name}: the hand points up");
        across_x.push(line.x);
        let (w, h) = (480, 640);
        save(&format!("hand_{name}.png"), &draw(&a, w, h, Framing::Half), w, h);
    }
    assert!(across_x[0] > 0.01 && across_x[1] < -0.01, "the knuckles' line flips: {across_x:?}");
}

/// Live tracking replayed (LIGHTAIR_BODY_JSONL: the engine's LIGHTAIR_AVATAR_DUMP file, at
/// about 35 frames a second): how much the avatar's arms shake from one drawn frame to the next.
#[test]
fn replayed_tracking_is_steady() {
    use lightair_avatar::body::{BodyFrame, BodyMode, BodyState, HAND_POINTS, JOINTS};
    let (Some(model), Some(path)) = (model(), std::env::var_os("LIGHTAIR_BODY_JSONL")) else { return };
    let f3 = |x: &serde_json::Value| x.as_f64().unwrap_or(0.0) as f32;
    let v3 = |p: &serde_json::Value| p.as_array().map(|a| Vec3::new(f3(&a[0]), f3(&a[1]), f3(&a[2])));
    let frames: Vec<BodyFrame> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            let joints3d = v["j3"].as_array().map(|a| std::array::from_fn(|j| v3(&a[j])));
            let joints2d = std::array::from_fn(|j| v["j2"][j].as_array().map(|a| (glam::Vec2::new(f3(&a[0]), f3(&a[1])), f3(&a[2]))));
            let hands = std::array::from_fn(|s| v["hands"][s].as_array().map(|h| std::array::from_fn::<_, HAND_POINTS, _>(|k| (glam::Vec2::new(f3(&h[k][0]), f3(&h[k][1])), f3(&h[k][2])))));
            let hands3d = std::array::from_fn(|s| v["hands3d"][s].as_array().map(|h| std::array::from_fn::<_, HAND_POINTS, _>(|k| v3(&h[k]).unwrap())));
            let _ = JOINTS;
            BodyFrame { joints3d, joints2d, hands, hands3d, aspect: f3(&v["aspect"]), frame_id: v["frame_id"].as_u64().unwrap() }
        })
        .collect();
    let mut a = Avatar::new(model);
    let mut state = BodyState::default();
    let mut steps: Vec<Vec<f32>> = vec![Vec::new(); 4];
    let mut last: Option<[Vec3; 4]> = None;
    let mut dirs: Vec<[Vec3; 4]> = Vec::new();
    let mut hidden: Vec<(&str, f32, usize, f32, f32)> = Vec::new();
    // drawn at 60 frames a second, the tracker answering at 35
    let mut t = 0.0f32;
    let mut k = 0;
    while k < frames.len() {
        t += 1.0 / 60.0;
        while k + 1 < frames.len() && (k as f32) / 35.0 < t {
            k += 1;
        }
        if k + 1 >= frames.len() {
            break;
        }
        state.step(Some(&frames[k]), true, 1.0 / 60.0);
        let (bones, torso) = a.body_bones(&state, BodyMode::Upper);
        a.update(&Controls { bones, torso, ..Controls::default() }, 1.0 / 60.0);
        let dir = |x: &str, y: &str| (a.bone_position(y).unwrap() - a.bone_position(x).unwrap()).normalize();
        let now = [dir("leftUpperArm", "leftLowerArm"), dir("leftLowerArm", "leftHand"), dir("rightUpperArm", "rightLowerArm"), dir("rightLowerArm", "rightHand")];
        if let Some(prev) = last {
            for i in 0..4 {
                let step = prev[i].angle_between(now[i]).to_degrees();
                if i == 1 && step > 8.0 && std::env::var_os("SHOW_JUMPS").is_some() {
                    eprintln!("jump {step:.0}° at frame {k}: seen {:.2?} hand_seen {:.2?}", state.seen, state.hand_seen);
                }
                steps[i].push(step);
            }
        }
        last = Some(now);
        dirs.push(now);
        // an arm out of view (mirrored: the person's left is the avatar's right)
        for (side, wj, ej, upper) in [("avatar's right (person's left)", 13usize, 12usize, 2usize), ("avatar's left (person's right)", 16, 15, 0)] {
            let f = &frames[k];
            let (w, e) = (f.joints2d[wj].map_or(0.0, |p| p.1), f.joints2d[ej].map_or(0.0, |p| p.1));
            if w < 0.3 && e < 0.5 {
                let down = now[upper].angle_between(Vec3::NEG_Y).to_degrees();
                hidden.push((side, down, k, w, e));
            }
        }
    }
    let mut by: std::collections::BTreeMap<&str, Vec<f32>> = Default::default();
    for (side, d, _, _, _) in &hidden {
        by.entry(side).or_default().push(*d);
    }
    for (side, mut d) in by {
        d.sort_by(f32::total_cmp);
        eprintln!("hidden {side}: {} frames, upper arm from straight down: median {:.0}° p90 {:.0}° max {:.0}°", d.len(), d[d.len() / 2], d[d.len() * 9 / 10], d[d.len() - 1]);
    }
    if let Some(worst) = hidden.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
        eprintln!("worst: {} {:.0}° at frame {} (wrist seen {:.2}, elbow {:.2}); its joints: {:?}", worst.0, worst.1, worst.2, worst.3, worst.4, frames[worst.2].joints2d);
    }
    // wobble: how far each frame strays from its half-second's average
    let mut wobble: Vec<Vec<f32>> = vec![Vec::new(); 4];
    for c in 15..dirs.len().saturating_sub(15) {
        for i in 0..4 {
            let mean = dirs[c - 15..c + 15].iter().map(|d| d[i]).sum::<Vec3>().normalize();
            wobble[i].push(mean.angle_between(dirs[c][i]).to_degrees());
        }
    }
    for ((name, mut s), mut w) in ["left upper arm", "left forearm", "right upper arm", "right forearm"].into_iter().zip(steps).zip(wobble) {
        s.sort_by(f32::total_cmp);
        w.sort_by(f32::total_cmp);
        eprintln!("{name:16} step median {:.2}°  p99 {:.2}°   wobble median {:.2}°  p90 {:.2}°", s[s.len() / 2], s[s.len() * 99 / 100], w[w.len() / 2], w[w.len() * 9 / 10]);
    }
}
