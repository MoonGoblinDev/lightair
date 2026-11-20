//! A model posed each frame: its bones turned by a [`Face`] (head and neck, a little of the
//! chest), arms down from the T-pose, breathing; expressions (blinks, vowels, an emotion);
//! the eyes' look-at; spring bones (hair, clothes) simulated; then morphed and skinned on the
//! CPU into world-space vertices ready to draw.
//!
//! Every model faces +Z (toward the camera), its left at +X.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};

use crate::body::{self, BodyMode, BodyState, J};
use crate::rig::Face;
use crate::vrm::{LookAtKind, Model, Override, Shape, Tail};

/// What the avatar should do this frame.
#[derive(Clone, Debug, Default)]
pub struct Controls {
    /// The face, as the avatar should make it (mirror it first for a mirror image): the head's
    /// rotation in the avatar's space, eyes [its right, its left], gaze x toward its left.
    pub face: Face,
    /// An emotion or other expression ("happy", "angry", …) and how much (0..1).
    pub expression: Option<(String, f32)>,
    /// Humanoid bones turned from their rest pose (in the model's space), from body tracking:
    /// these replace the idle pose's (arms down) for the bones they name.
    pub bones: Vec<(String, Quat)>,
    /// The torso's turn from body tracking (the head's is then measured from it).
    pub torso: Option<Quat>,
}

/// How much of the avatar the camera takes in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Framing {
    Head,
    #[default]
    Bust,
    Half,
    Full,
}

impl Framing {
    pub fn from_name(s: &str) -> Framing {
        match s {
            "head" => Framing::Head,
            "half" => Framing::Half,
            "full" => Framing::Full,
            _ => Framing::Bust,
        }
    }
}

/// Where to look from.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    /// Vertical field of view, degrees.
    pub fovy: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye, self.target, Vec3::Y)
    }
}

struct SpringState {
    prev: Vec3,
    current: Vec3,
    /// The tail's direction in the joint's space, at rest.
    axis: Vec3,
    length: f32,
}

pub struct Avatar {
    pub model: Arc<Model>,
    rest: Vec<(Vec3, Quat, Vec3)>,
    local: Vec<(Vec3, Quat, Vec3)>,
    /// World matrices, this frame.
    pub world: Vec<Mat4>,
    rest_world_rot: Vec<Quat>,
    rest_pos: Vec<Vec3>,
    springs: Vec<SpringState>,
    springs_ready: bool,
    /// Morph weights by glTF mesh.
    morph: HashMap<usize, Vec<f32>>,
    /// Each primitive's vertices this frame, world space.
    pub positions: Vec<Vec<Vec3>>,
    pub normals: Vec<Vec<Vec3>>,
    time: f32,
    /// The top of the model (its highest vertex at rest), metres.
    pub top: f32,
}

fn bone_rot_in(rest_world: Quat, delta: Quat) -> Quat {
    // a turn in the model's space, as a turn in the bone's own rest frame
    rest_world.inverse() * delta * rest_world
}

impl Avatar {
    pub fn new(model: Arc<Model>) -> Avatar {
        let rest: Vec<(Vec3, Quat, Vec3)> = model.nodes.iter().map(|n| (n.translation, n.rotation, n.scale)).collect();
        let world = model.rest_world();
        let rest_world_rot = world.iter().map(|m| m.to_scale_rotation_translation().1).collect();
        let rest_pos = world.iter().map(|m| m.w_axis.truncate()).collect();
        let positions = model.primitives.iter().map(|p| vec![Vec3::ZERO; p.positions.len()]).collect();
        let normals = model.primitives.iter().map(|p| vec![Vec3::Z; p.normals.len()]).collect();
        let mut a = Avatar {
            local: rest.clone(),
            rest,
            world,
            rest_world_rot,
            rest_pos,
            springs: Vec::new(),
            springs_ready: false,
            morph: HashMap::new(),
            positions,
            normals,
            time: 0.0,
            top: 1.6,
            model,
        };
        a.skin();
        a.top = a.positions.iter().flatten().map(|p| p.y).fold(f32::MIN, f32::max).max(0.3);
        a
    }

    fn bone(&self, name: &str) -> Option<usize> {
        self.model.humanoid.get(name).copied()
    }

    /// Turns a humanoid bone by `delta` (in the model's space), from its rest pose.
    fn turn(&mut self, name: &str, delta: Quat) {
        if let Some(n) = self.bone(name) {
            self.local[n].1 = (self.rest[n].1 * bone_rot_in(self.rest_world_rot[n], delta)).normalize();
        }
    }

    /// A humanoid bone's world position this frame.
    pub fn bone_position(&self, name: &str) -> Option<Vec3> {
        self.bone(name).map(|n| self.world[n].w_axis.truncate())
    }

    /// Poses the model for this frame, `dt` seconds after the last.
    pub fn update(&mut self, c: &Controls, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        self.local.clone_from(&self.rest);
        let face = &c.face;

        // arms down from the T-pose, elbows a little bent
        for left in [true, false] {
            let (upper, lower) = body::idle_arm(left);
            let side = if left { "left" } else { "right" };
            self.turn(&format!("{side}UpperArm"), upper);
            self.turn(&format!("{side}LowerArm"), lower);
        }
        self.turn("leftShoulder", Quat::from_rotation_z(-4f32.to_radians()));
        self.turn("rightShoulder", Quat::from_rotation_z(4f32.to_radians()));

        // breathing: a slow rise of the chest and shoulders
        let breath = (self.time * std::f32::consts::TAU / 4.2).sin();
        self.turn("spine", Quat::from_rotation_x(-0.6f32.to_radians() * breath));
        let shrug = 0.8f32.to_radians() * breath;
        self.turn("leftShoulder", Quat::from_rotation_z(-4f32.to_radians() + shrug));
        self.turn("rightShoulder", Quat::from_rotation_z(4f32.to_radians() - shrug));

        // what body tracking says, over the idle pose
        for (name, q) in &c.bones {
            self.turn(name, *q);
        }

        // the head: shared by the chest, neck and head; with a tracked torso, from the torso
        match c.torso {
            Some(torso) => {
                let rel = (torso.inverse() * face.head).normalize();
                self.turn("neck", Quat::IDENTITY.slerp(rel, 0.4));
                self.turn("head", Quat::IDENTITY.slerp(rel, 0.6));
            }
            None => {
                let head = face.head;
                self.turn("upperChest", Quat::IDENTITY.slerp(head, 0.08));
                self.turn("chest", Quat::IDENTITY.slerp(head, 0.08));
                self.turn("neck", Quat::IDENTITY.slerp(head, 0.34));
                self.turn("head", Quat::IDENTITY.slerp(head, 0.5));
            }
        }

        // expressions
        let mut weights: HashMap<&str, f32> = HashMap::new();
        let (mut blink_k, mut mouth_k, mut look_k) = (1.0f32, 1.0f32, 1.0f32);
        let model = self.model.clone();
        if let Some((name, w)) = &c.expression
            && let Some(e) = model.expressions.get(name.as_str())
        {
            let w = if e.binary { if *w > 0.5 { 1.0 } else { 0.0 } } else { w.clamp(0.0, 1.0) };
            weights.insert(name.as_str(), w);
            let k = |o: Override| match o {
                Override::None => 1.0,
                Override::Block => if w > 0.0 { 0.0 } else { 1.0 },
                Override::Blend => 1.0 - w,
            };
            blink_k = k(e.override_blink);
            mouth_k = k(e.override_mouth);
            look_k = k(e.override_look_at);
        }
        let (r, l) = ((1.0 - face.eye_open[0]) * blink_k, (1.0 - face.eye_open[1]) * blink_k);
        let separate = model.expressions.contains_key("blinkLeft") && model.expressions.contains_key("blinkRight");
        if separate {
            let both = if model.expressions.contains_key("blink") { r.min(l) } else { 0.0 };
            weights.insert("blink", both);
            weights.insert("blinkLeft", l - both);
            weights.insert("blinkRight", r - both);
        } else {
            weights.insert("blink", (r + l) * 0.5);
        }
        for (name, v) in ["aa", "ih", "ou", "ee", "oh"].iter().zip(face.vowels()) {
            weights.insert(name, v * mouth_k);
        }

        // where the eyes look
        let yaw = face.gaze.x * 30.0 * look_k;
        let pitch = face.gaze.y * 25.0 * look_k;
        let la = model.look_at;
        match la.kind {
            LookAtKind::Bone => {
                let v = if pitch >= 0.0 { la.vertical_up.map(pitch) } else { -la.vertical_down.map(pitch) };
                // (looking toward its left is outward for its left eye, inward for its right)
                let (left, right) = if yaw >= 0.0 {
                    (la.horizontal_outer.map(yaw), la.horizontal_inner.map(yaw))
                } else {
                    (-la.horizontal_inner.map(yaw), -la.horizontal_outer.map(yaw))
                };
                self.turn("leftEye", Quat::from_rotation_y(left.to_radians()) * Quat::from_rotation_x(-v.to_radians()));
                self.turn("rightEye", Quat::from_rotation_y(right.to_radians()) * Quat::from_rotation_x(-v.to_radians()));
            }
            LookAtKind::Expression => {
                let h = la.horizontal_outer.map(yaw);
                weights.insert(if yaw >= 0.0 { "lookLeft" } else { "lookRight" }, h.min(1.0));
                let v = if pitch >= 0.0 { la.vertical_up.map(pitch) } else { la.vertical_down.map(pitch) };
                weights.insert(if pitch >= 0.0 { "lookUp" } else { "lookDown" }, v.min(1.0));
            }
        }

        self.morph.clear();
        for (name, w) in weights {
            if w <= 1e-4 {
                continue;
            }
            let Some(e) = model.expressions.get(name) else { continue };
            for &(mesh, index, bw) in &e.binds {
                let list = self.morph.entry(mesh).or_default();
                if list.len() <= index {
                    list.resize(index + 1, 0.0);
                }
                list[index] = (list[index] + w * bw).min(1.0);
            }
        }

        self.world = self.world_from_local();
        self.simulate(dt);
        self.skin();
    }

    fn local_matrix(&self, n: usize) -> Mat4 {
        let (t, r, s) = self.local[n];
        Mat4::from_scale_rotation_translation(s, r, t)
    }

    fn world_from_local(&self) -> Vec<Mat4> {
        let locals: Vec<Mat4> = (0..self.local.len()).map(|n| self.local_matrix(n)).collect();
        self.model.world(&locals)
    }

    /// A node's world matrix from the current local poses (walking up to the root).
    fn world_of(&self, mut n: usize) -> Mat4 {
        let mut m = self.local_matrix(n);
        while let Some(p) = self.model.nodes[n].parent {
            m = self.local_matrix(p) * m;
            n = p;
        }
        Mat4::from_quat(self.model.root) * m
    }

    fn parent_world(&self, n: usize) -> Mat4 {
        match self.model.nodes[n].parent {
            Some(p) => self.world_of(p),
            None => Mat4::from_quat(self.model.root),
        }
    }

    /// Spring bones (VRM 1.0's algorithm): each joint's tail keeps its momentum, is pulled
    /// back toward its rest direction and down by gravity, kept at its length and out of the
    /// colliders.
    fn simulate(&mut self, dt: f32) {
        let model = self.model.clone();
        if model.joints.is_empty() {
            return;
        }
        if !self.springs_ready {
            self.springs = model
                .joints
                .iter()
                .map(|j| {
                    let w = self.world[j.node];
                    let tail = match j.tail {
                        Tail::Node(c) => self.world[c].w_axis.truncate(),
                        Tail::Local(v) => w.transform_point3(v),
                    };
                    let head = w.w_axis.truncate();
                    let local = w.inverse().transform_point3(tail);
                    SpringState { prev: tail, current: tail, axis: local.normalize_or(Vec3::Y), length: head.distance(tail).max(1e-4) }
                })
                .collect();
            self.springs_ready = true;
        }
        let steps = (dt / (1.0 / 60.0)).ceil().clamp(1.0, 4.0) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            for (k, j) in model.joints.iter().enumerate() {
                let parent = self.parent_world(j.node);
                let (_, pr, _) = parent.to_scale_rotation_translation();
                let head = (parent * Mat4::from_translation(self.rest[j.node].0)).w_axis.truncate();
                let rest_rot = self.rest[j.node].1;
                let s = &self.springs[k];
                let axis = (pr * rest_rot * s.axis).normalize_or(Vec3::Y);
                let inertia = (s.current - s.prev) * (1.0 - j.drag);
                let mut next = s.current + inertia + axis * j.stiffness * h + j.gravity_dir * j.gravity_power * h;
                next = head + (next - head).normalize_or(axis) * s.length;
                for &ci in &j.colliders {
                    let col = model.colliders[ci];
                    let cw = self.world_of(col.node);
                    let scale = cw.to_scale_rotation_translation().0.max_element();
                    let (a, b, radius) = match col.shape {
                        Shape::Sphere { offset, radius } => {
                            let p = cw.transform_point3(offset);
                            (p, p, radius * scale)
                        }
                        Shape::Capsule { offset, radius, tail } => (cw.transform_point3(offset), cw.transform_point3(tail), radius * scale),
                    };
                    let ab = b - a;
                    let t = if ab.length_squared() > 1e-10 { ((next - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                    let closest = a + ab * t;
                    let reach = radius + j.hit_radius;
                    let d = next - closest;
                    if d.length_squared() < reach * reach {
                        next = closest + d.normalize_or(axis) * reach;
                        next = head + (next - head).normalize_or(axis) * s.length;
                    }
                }
                let to = (next - head).normalize_or(axis);
                let turned = Quat::from_rotation_arc(axis, to) * pr * rest_rot;
                self.local[j.node].1 = (pr.inverse() * turned).normalize();
                let s = &mut self.springs[k];
                s.prev = s.current;
                s.current = next;
            }
        }
        self.world = self.world_from_local();
    }

    /// Morphs and skins every primitive into [`Avatar::positions`] and [`Avatar::normals`].
    fn skin(&mut self) {
        let model = self.model.clone();
        let mut pos = Vec::new();
        let mut nor = Vec::new();
        for (pi, p) in model.primitives.iter().enumerate() {
            pos.clear();
            pos.extend_from_slice(&p.positions);
            nor.clear();
            nor.extend_from_slice(&p.normals);
            if let Some(w) = self.morph.get(&p.mesh) {
                for (t, &wt) in w.iter().enumerate() {
                    if wt <= 1e-4 {
                        continue;
                    }
                    let Some(m) = p.morphs.get(t) else { continue };
                    for &(i, d) in &m.positions {
                        pos[i as usize] += d * wt;
                    }
                    for &(i, d) in &m.normals {
                        nor[i as usize] += d * wt;
                    }
                }
            }
            let out_p = &mut self.positions[pi];
            let out_n = &mut self.normals[pi];
            match p.skin.and_then(|s| model.skins.get(s)) {
                Some(skin) => {
                    let mats: Vec<Mat4> = skin
                        .joints
                        .iter()
                        .zip(&skin.inverse_bind)
                        .map(|(&j, ib)| self.world.get(j).copied().unwrap_or(Mat4::IDENTITY) * *ib)
                        .collect();
                    for v in 0..pos.len() {
                        let (js, ws) = (p.joints[v], p.weights[v]);
                        let mut m = Mat4::ZERO;
                        for k in 0..4 {
                            if ws[k] > 0.0
                                && let Some(jm) = mats.get(js[k] as usize)
                            {
                                m += *jm * ws[k];
                            }
                        }
                        out_p[v] = m.transform_point3(pos[v]);
                        out_n[v] = m.transform_vector3(nor[v]).normalize_or(Vec3::Z);
                    }
                }
                None => {
                    let m = self.world[p.node];
                    for v in 0..pos.len() {
                        out_p[v] = m.transform_point3(pos[v]);
                        out_n[v] = m.transform_vector3(nor[v]).normalize_or(Vec3::Z);
                    }
                }
            }
        }
    }

    fn rest_dir(&self, from: &str, to: &str) -> Option<Vec3> {
        let (a, b) = (self.bone(from)?, self.bone(to)?);
        (self.rest_pos[b] - self.rest_pos[a]).try_normalize()
    }

    /// The bones a tracked body turns (for [`Controls::bones`]) and the torso's turn (for
    /// [`Controls::torso`]).
    pub fn body_bones(&self, b: &BodyState, mode: BodyMode) -> (Vec<(String, Quat)>, Option<Quat>) {
        let mut out = Vec::new();
        if mode == BodyMode::Off {
            return (out, None);
        }
        let j = |x: J| b.joint(x);
        let has = |n: &str| self.model.humanoid.contains_key(n);

        // the torso: leaning and twisting, shared down the spine
        let mut torso = Quat::IDENTITY;
        if b.seen[body::TORSO] > 0.01
            && let (Some(ls), Some(rs)) = (j(J::LeftShoulder), j(J::RightShoulder))
            && let (Some(rest_left), Some(rest_up)) = (self.rest_dir("rightUpperArm", "leftUpperArm"), self.rest_dir("hips", "neck").or_else(|| self.rest_dir("hips", "head")))
        {
            // (up from the hips: their lean toward or away from the camera only partly, as a
            // seated person's shoulders sit well ahead of their hips without leaning)
            let up = match (j(J::CenterShoulder), j(J::Root)) {
                (Some(c), Some(r)) if (c - r).length() > 0.1 => {
                    let d = c - r;
                    Vec3::new(d.x, d.y, d.z * if mode == BodyMode::Full { 1.0 } else { 0.25 })
                }
                _ => Vec3::Y,
            };
            let t = clamp_angle(body::basis_turn(rest_left, rest_up, ls - rs, up), 40f32.to_radians());
            torso = Quat::IDENTITY.slerp(t, b.seen[body::TORSO]);
            let spine: Vec<&str> = ["spine", "chest", "upperChest"].into_iter().filter(|n| has(n)).collect();
            let share = Quat::IDENTITY.slerp(torso, 1.0 / spine.len().max(1) as f32);
            for n in spine {
                out.push((n.to_string(), share));
            }
        }

        // arms: aimed shoulder → elbow → wrist, from the idle pose as much as they're seen. An
        // elbow out of view (below a desk camera's picture) is worked out from the shoulder and
        // wrist, bent down and out; a wrist out of view hangs from the elbow.
        let shoulder_width = match (j(J::LeftShoulder), j(J::RightShoulder)) {
            (Some(l), Some(r)) => Some(l.distance(r)),
            _ => None,
        };
        let rest_width = match (self.bone("leftUpperArm"), self.bone("rightUpperArm")) {
            (Some(l), Some(r)) => Some(self.rest_pos[l].distance(self.rest_pos[r])),
            _ => None,
        };
        for (left, part, (sj, ej, wj)) in [(true, body::LEFT_ARM, (J::LeftShoulder, J::LeftElbow, J::LeftWrist)), (false, body::RIGHT_ARM, (J::RightShoulder, J::RightElbow, J::RightWrist))] {
            let side = if left { "left" } else { "right" };
            let (upper, lower, hand) = (format!("{side}UpperArm"), format!("{side}LowerArm"), format!("{side}Hand"));
            let (idle_u, idle_l) = body::idle_arm(left);
            let seen = b.seen[part];
            let (mut qu, mut ql) = (idle_u, idle_l);
            let mut forearm_turn = None;
            let lengths = match (self.bone(&upper), self.bone(&lower), self.bone(&hand), shoulder_width, rest_width) {
                (Some(u), Some(l), Some(h), Some(sw), Some(rw)) if rw > 1e-4 => {
                    let scale = sw / rw;
                    Some((self.rest_pos[u].distance(self.rest_pos[l]) * scale, self.rest_pos[l].distance(self.rest_pos[h]) * scale))
                }
                _ => None,
            };
            let out_dir = if left { 1.0 } else { -1.0 };
            // (the arm as last followed: held still while it fades back to rest)
            let _ = (sj, ej, wj);
            let elbow_wrist = match (b.arms[if left { 0 } else { 1 }], lengths) {
                (Some((s, Some(e), w)), _) => Some((s, e, w)),
                (Some((s, None, w)), Some((lu, ll))) => Some((s, ik_elbow(s, w, lu, ll, Vec3::new(out_dir * 0.6, -1.0, 0.0)), w)),
                _ => None,
            };
            if seen > 0.01
                && let Some((s, e, w)) = elbow_wrist
                && let (Some(ru), Some(rl)) = (self.rest_dir(&upper, &lower), self.rest_dir(&lower, &hand))
                && let Some((lu, ll)) = lengths
            {
                // the arm at rest, in the tracked body's terms; the tracked joints blended
                // toward it by place, not by turn (a turn of near half a circle could flip the
                // way it goes from one frame to the next)
                let e0 = s + torso * idle_u * ru * lu;
                let w0 = e0 + torso * idle_u * idle_l * rl * ll;
                let e = e0.lerp(e, seen);
                let w = w0.lerp(w, seen);
                let (Some(du), Some(dl)) = ((e - s).try_normalize(), (w - e).try_normalize()) else { continue };
                let tu = Quat::from_rotation_arc(ru, torso.inverse() * du);
                let tl = Quat::from_rotation_arc(rl, (torso * tu).inverse() * dl);
                qu = tu;
                ql = tl;
                forearm_turn = Some(torso * qu * ql);
            }
            // the hand: turned the way the hand tracker sees it (where it points and, from 3D,
            // which way its palm faces); most of a roll is the forearm's, so the wrist doesn't wring
            let k = if left { 0 } else { 1 };
            let rest_along = self.rest_dir(&hand, &format!("{side}MiddleProximal"));
            let rest_across = self.rest_dir(&format!("{side}IndexProximal"), &format!("{side}LittleProximal"));
            if let (Some(acc), Some((along, across, hand_seen)), Some(ra), Some(rl)) = (forearm_turn, b.hand(k), rest_along, self.rest_dir(&lower, &hand)) {
                let weight = seen * hand_seen;
                let world = match (across, rest_across) {
                    (Some(c), Some(rc)) => body::basis_turn(ra, rc, along, c),
                    // (pointing only: the least turn from the forearm's way)
                    _ => Quat::from_rotation_arc(acc * ra, along) * acc,
                };
                // the hand's turn from the forearm, split into a roll about the forearm and the rest
                let rel = (acc.inverse() * world).normalize();
                let axis = rl;
                let proj = axis * Vec3::new(rel.x, rel.y, rel.z).dot(axis);
                let len = (proj.length_squared() + rel.w * rel.w).sqrt();
                let roll = if len > 1e-6 { Quat::from_xyzw(proj.x / len, proj.y / len, proj.z / len, rel.w / len) } else { Quat::IDENTITY };
                let forearm_roll = Quat::IDENTITY.slerp(roll, 0.65 * weight);
                ql = (ql * forearm_roll).normalize();
                let th = ((torso * qu * ql).inverse() * world).normalize();
                // (a wrist bends so far)
                out.push((hand.clone(), Quat::IDENTITY.slerp(clamp_angle(th, 95f32.to_radians()), weight)));
            }
            out.push((upper, qu));
            out.push((lower, ql));
            // fingers: each curls toward the palm (palm down in the T-pose)
            for &(bone, finger, amount) in body::FINGER_BONES {
                let name = format!("{side}{bone}");
                if !has(&name) {
                    continue;
                }
                let a = b.curls[k][finger] * amount * 70f32.to_radians();
                let s = if left { 1.0 } else { -1.0 };
                let q = if finger == 0 { Quat::from_rotation_y(s * a * 0.6) } else { Quat::from_rotation_z(-s * a) };
                out.push((name, q));
            }
        }

        // legs: hip → knee → ankle
        if mode == BodyMode::Full && b.seen[body::LEGS] > 0.01 {
            for (side, (hj, kj, aj)) in [("left", (J::LeftHip, J::LeftKnee, J::LeftAnkle)), ("right", (J::RightHip, J::RightKnee, J::RightAnkle))] {
                let (upper, lower, foot) = (format!("{side}UpperLeg"), format!("{side}LowerLeg"), format!("{side}Foot"));
                if let (Some(h), Some(k), Some(a)) = (j(hj), j(kj), j(aj))
                    && let (Some(ru), Some(rl)) = (self.rest_dir(&upper, &lower), self.rest_dir(&lower, &foot))
                    && let (Some(du), Some(dl)) = ((k - h).try_normalize(), (a - k).try_normalize())
                {
                    let tu = Quat::from_rotation_arc(ru, du);
                    let tl = Quat::from_rotation_arc(rl, tu.inverse() * dl);
                    let seen = b.seen[body::LEGS];
                    out.push((upper, Quat::IDENTITY.slerp(tu, seen)));
                    out.push((lower, Quat::IDENTITY.slerp(tl, seen)));
                }
            }
        }
        let torso = (b.seen[body::TORSO] > 0.01).then_some(torso);
        (out, torso)
    }

    /// Where to look from for a framing, at an aspect (width / height).
    pub fn camera(&self, framing: Framing, aspect: f32, zoom: f32) -> Camera {
        let head = self.bone_position("head").unwrap_or(Vec3::new(0.0, 1.4, 0.0));
        let top = self.top.max(head.y + 0.05);
        let chest = self.bone_position("upperChest").or_else(|| self.bone_position("chest")).unwrap_or(head - Vec3::Y * 0.3);
        let hips = self.bone_position("hips").unwrap_or(head * 0.55);
        let (bottom, width) = match framing {
            Framing::Head => (head.y - 0.1, 0.32),
            Framing::Bust => (chest.y - 0.16, 0.56),
            Framing::Half => (hips.y - 0.08, 0.75),
            Framing::Full => (-0.04, 0.9),
        };
        let top = top + 0.04;
        let height = (top - bottom) / zoom.clamp(0.25, 4.0);
        let fovy = 22.0f32;
        let tan = (fovy.to_radians() * 0.5).tan();
        let dist = (height * 0.5 / tan).max(width / zoom.clamp(0.25, 4.0) * 0.5 / (tan * aspect.max(0.1)));
        let centre = Vec3::new(head.x, (top + bottom) * 0.5 + (top - bottom - height) * 0.5, head.z);
        Camera { eye: centre + Vec3::Z * dist, target: centre, fovy, near: (dist * 0.1).max(0.01), far: dist + 10.0 }
    }
}

/// `q`, turned at most `max` radians.
fn clamp_angle(q: Quat, max: f32) -> Quat {
    // (the short way round: q and -q are the same turn)
    let q = if q.w < 0.0 { -q } else { q };
    let (axis, angle) = q.to_axis_angle();
    if angle.abs() <= max { q } else { Quat::from_axis_angle(axis, max * angle.signum()) }
}

/// Where an elbow is, from the shoulder and wrist and the arm's two lengths: bent toward `pole`.
fn ik_elbow(s: Vec3, w: Vec3, upper: f32, lower: f32, pole: Vec3) -> Vec3 {
    let to = w - s;
    let d = to.length().clamp(1e-4, (upper + lower) * 0.999);
    let dir = to.normalize_or(Vec3::NEG_Y);
    // the angle at the shoulder, between the wrist and the elbow
    let cos = ((upper * upper + d * d - lower * lower) / (2.0 * upper * d)).clamp(-1.0, 1.0);
    let bend = (pole - dir * pole.dot(dir)).normalize_or(Vec3::NEG_Y);
    s + (dir * cos + bend * (1.0 - cos * cos).sqrt()) * upper
}
