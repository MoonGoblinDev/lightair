//! From a tracked body and hands (3D: MediaPipe's world points) to an avatar's humanoid bones:
//! the torso's lean and twist, arms, legs, the hands' way and finger curls. The picture's points
//! only say what's in view; limbs out of view fade back to the idle pose (arms down).
//!
//! Joints are the person's own left and right; 3D ones in metres from the hips' centre, x toward
//! the person's left, y up, z toward the camera: the avatar's own space (it faces the camera,
//! its left at +X), so the camera's view needs no turning. A mirror swaps left and right.

use glam::{Quat, Vec2, Vec3};

/// The tracker's joints (body-tracker.h's order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum J {
    Root,
    LeftHip,
    LeftKnee,
    LeftAnkle,
    RightHip,
    RightKnee,
    RightAnkle,
    Spine,
    CenterShoulder,
    CenterHead,
    TopHead,
    LeftShoulder,
    LeftElbow,
    LeftWrist,
    RightShoulder,
    RightElbow,
    RightWrist,
}

pub const JOINTS: usize = 17;
pub const HAND_POINTS: usize = 21;

/// One frame of body tracking.
#[derive(Clone, Debug)]
pub struct BodyFrame {
    /// 3D joints, when the tracker found a 3D body.
    pub joints3d: Option<[Option<Vec3>; JOINTS]>,
    /// The joints in the picture (0..1, y down) and how sure the tracker is (0..1).
    pub joints2d: [Option<(Vec2, f32)>; JOINTS],
    /// The person's left and right hand: 21 points in the picture (MediaPipe's order), sureness.
    pub hands: [Option<[(Vec2, f32); HAND_POINTS]>; 2],
    /// The same hands in 3D (metres, our axes), when known.
    pub hands3d: [Option<[Vec3; HAND_POINTS]>; 2],
    /// The picture's width / height.
    pub aspect: f32,
    /// Which analysed frame this is (a new one each time the tracker answers).
    pub frame_id: u64,
}

/// How much of the body moves the avatar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BodyMode {
    /// The face only (the idle pose below the neck).
    Off,
    /// Torso, arms and hands (sitting at a desk).
    #[default]
    Upper,
    /// And the legs.
    Full,
}

impl BodyMode {
    pub fn from_name(s: &str) -> BodyMode {
        match s {
            "off" => BodyMode::Off,
            "full" => BodyMode::Full,
            _ => BodyMode::Upper,
        }
    }
}

/// The body as the avatar should take it: joints in its space (mirrored if asked), smoothed,
/// with how visible each part is (0..1, faded in and out).
#[derive(Clone, Debug, Default)]
pub struct BodyState {
    pub joints: [Option<Vec3>; JOINTS],
    /// Torso, the avatar's left arm, right arm, legs.
    pub seen: [f32; 4],
    /// Finger curls (0 straight to 1 a fist): [the avatar's left, right][thumb, index, middle,
    /// ring, little].
    pub curls: [[f32; 5]; 2],
    /// Each hand's way (the avatar's left and right): where it points (wrist toward the middle
    /// knuckle) and, from 3D, across its knuckles (index toward little finger).
    pub hand_dir: [Option<(Vec3, Option<Vec3>)>; 2],
    /// Seconds since each joint (and hand) was last seen surely: an old one isn't used.
    pub age: [f32; JOINTS],
    pub hand_age: [f32; 2],
    /// Answers in a row each joint has been surely seen (a blip of one isn't believed).
    streak: [u8; JOINTS],
    /// Each arm (the avatar's left, right) as last followed: shoulder, elbow (if seen), wrist.
    /// An arm fading back to rest fades from this, held still.
    pub arms: [Option<(Vec3, Option<Vec3>, Vec3)>; 2],
    /// How much each hand's way counts (0..1, faded in and out like [`BodyState::seen`]).
    pub hand_seen: [f32; 2],
    /// The tracker's answers, filtered as they come (One Euro: steady when still, quick when
    /// moving); the state eases toward them each frame.
    filtered: [Euro3; JOINTS],
    hand_filters: [(Euro3, Euro3); 2],
    curl_filters: [[Euro1; 5]; 2],
    curl_targets: [[f32; 5]; 2],
    last_frame: Option<u64>,
    since_frame: f32,
    /// Metres a picture height (from the shoulders' width), eased.
    scale: Option<f32>,
}

/// A One Euro filter (Casiez et al.): smooths hard while a value holds still, follows quickly
/// when it moves.
#[derive(Clone, Debug, Default)]
struct Euro3 {
    x: Option<Vec3>,
    dx: Vec3,
}

fn euro_alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (std::f32::consts::TAU * cutoff);
    1.0 / (1.0 + tau / dt.max(1e-4))
}

impl Euro3 {
    fn filter(&mut self, v: Vec3, dt: f32, min_cutoff: f32, beta: f32) -> Vec3 {
        let Some(prev) = self.x else {
            self.x = Some(v);
            self.dx = Vec3::ZERO;
            return v;
        };
        self.dx = self.dx.lerp((v - prev) / dt.max(1e-4), euro_alpha(1.0, dt));
        let x = prev.lerp(v, euro_alpha(min_cutoff + beta * self.dx.length(), dt));
        self.x = Some(x);
        x
    }
}

#[derive(Clone, Debug, Default)]
struct Euro1 {
    x: Option<f32>,
    dx: f32,
}

impl Euro1 {
    fn filter(&mut self, v: f32, dt: f32, min_cutoff: f32, beta: f32) -> f32 {
        let Some(prev) = self.x else {
            self.x = Some(v);
            return v;
        };
        self.dx += ((v - prev) / dt.max(1e-4) - self.dx) * euro_alpha(1.0, dt);
        let x = prev + (v - prev) * euro_alpha(min_cutoff + beta * self.dx.abs(), dt);
        self.x = Some(x);
        x
    }
}

/// A joint (or hand) unseen for longer is out of view.
const STALE: f32 = 0.35;

impl BodyState {
    /// A joint seen lately (where it is now, smoothed).
    pub fn joint(&self, j: J) -> Option<Vec3> {
        if self.age[j as usize] < STALE { self.joints[j as usize] } else { None }
    }

    /// The avatar's hand on `side` (0 left, 1 right): where it points, across its knuckles when
    /// known, and how much that counts (fading out after it was last seen).
    pub fn hand(&self, side: usize) -> Option<(Vec3, Option<Vec3>, f32)> {
        let (along, across) = self.hand_dir[side]?;
        (self.hand_seen[side] > 0.01).then_some((along, across, self.hand_seen[side]))
    }
}

pub const TORSO: usize = 0;
pub const LEFT_ARM: usize = 1;
pub const RIGHT_ARM: usize = 2;
pub const LEGS: usize = 3;

/// The finger curl at rest (a relaxed hand).
const RELAXED: f32 = 0.18;
/// Sure enough of a joint in the picture.
const SURE: f32 = 0.3;

impl BodyState {
    /// The next state from `frame` (or none: fades back to the idle pose), `dt` seconds on.
    /// A frame already seen (the tracker answers less often than frames are drawn) only eases
    /// the state on toward what it said.
    pub fn step(&mut self, frame: Option<&BodyFrame>, mirror: bool, dt: f32) -> &BodyState {
        let dt = dt.clamp(0.0, 0.25);
        let k = |tau: f32| 1.0 - (-dt / tau).exp();
        self.since_frame += dt;
        for a in self.age.iter_mut().chain(self.hand_age.iter_mut()) {
            *a += dt;
        }
        let new = frame.filter(|f| self.last_frame != Some(f.frame_id));
        if let Some(f) = new {
            self.last_frame = Some(f.frame_id);
            let fdt = self.since_frame.clamp(1.0 / 120.0, 0.25);
            self.since_frame = 0.0;
            self.take(f, mirror, fdt);
        }
        if frame.is_none() {
            self.curl_targets = [[RELAXED; 5]; 2];
        }
        // eased toward the filtered answers
        for (j, joint) in self.joints.iter_mut().enumerate() {
            if let Some(t) = self.filtered[j].x {
                *joint = Some(joint.map_or(t, |p| p.lerp(t, k(0.035))));
            }
        }
        for (side, curls) in self.curls.iter_mut().enumerate() {
            for (c, t) in curls.iter_mut().zip(self.curl_targets[side]) {
                *c += (t - *c) * k(if t == RELAXED { 0.4 } else { 0.04 });
            }
        }
        let ok = |j: J| self.age[j as usize] < STALE && self.joints[j as usize].is_some();
        let parts = [
            ok(J::LeftShoulder) && ok(J::RightShoulder),
            // an arm: while its hand is in the picture (out of it, on a desk or a lap, the arm
            // rests; an elbow just out of the picture is worked out)
            ok(J::LeftShoulder) && ok(J::LeftWrist) && self.streak[J::LeftWrist as usize] >= 3,
            ok(J::RightShoulder) && ok(J::RightWrist) && self.streak[J::RightWrist as usize] >= 3,
            [J::LeftHip, J::LeftKnee, J::LeftAnkle, J::RightHip, J::RightKnee, J::RightAnkle].into_iter().all(ok),
        ];
        for (i, (sj, ej, wj)) in [(J::LeftShoulder, J::LeftElbow, J::LeftWrist), (J::RightShoulder, J::RightElbow, J::RightWrist)].into_iter().enumerate() {
            if parts[1 + i]
                && let (Some(s), Some(w)) = (self.joint(sj), self.joint(wj))
            {
                self.arms[i] = Some((s, self.joint(ej), w));
            }
        }
        for (s, on) in self.seen.iter_mut().zip(parts) {
            // in quickly, out slowly (a hand leaving the picture for a moment)
            *s += (if on { 1.0 } else { 0.0 } - *s) * k(if on { 0.15 } else { 0.5 });
        }
        for side in 0..2 {
            let on = self.hand_age[side] < STALE && self.hand_dir[side].is_some();
            let s = &mut self.hand_seen[side];
            *s += (if on { 1.0 } else { 0.0 } - *s) * k(if on { 0.15 } else { 0.4 });
        }
        self
    }

    /// A new answer from the tracker, `fdt` seconds after the last.
    fn take(&mut self, f: &BodyFrame, mirror: bool, fdt: f32) {
        let swap = |j: usize| -> usize {
            if !mirror {
                return j;
            }
            match j {
                1..=3 => j + 3,
                4..=6 => j - 3,
                11..=13 => j + 3,
                14..=16 => j - 3,
                _ => j,
            }
        };
        let flip = |v: Vec3| if mirror { Vec3::new(-v.x, v.y, v.z) } else { v };
        let mut target: [Option<Vec3>; JOINTS] = [None; JOINTS];
        let mut sure = [0.0f32; JOINTS];
        let Some(j3) = &f.joints3d else { return };
        // Each joint in 3D: across and up from the picture (its points are far steadier than
        // the model's 3D guess), put in metres by the shoulders' width; depth from the 3D.
        let (ls, rs) = (J::LeftShoulder as usize, J::RightShoulder as usize);
        let anchor = match (f.joints2d[ls], f.joints2d[rs], j3[ls], j3[rs]) {
            (Some((l2, _)), Some((r2, _)), Some(l3), Some(r3)) => {
                let px = Vec2::new((l2.x - r2.x) * f.aspect, l2.y - r2.y).length();
                let m = Vec2::new(l3.x - r3.x, l3.y - r3.y).length();
                if px > 1e-3 && m > 0.05 {
                    let scale = m / px;
                    self.scale = Some(self.scale.map_or(scale, |s| s + (scale - s) * 0.05));
                }
                Some(((l2 + r2) * 0.5, (l3 + r3) * 0.5))
            }
            _ => None,
        };
        let picture = |p2: Vec2, p3: Vec3| -> Vec3 {
            match (anchor, self.scale) {
                (Some((c2, c3)), Some(scale)) => Vec3::new(c3.x + (p2.x - c2.x) * f.aspect * scale, c3.y - (p2.y - c2.y) * scale, p3.z),
                _ => p3,
            }
        };
        for j in 0..JOINTS {
            // (the joint the avatar's side takes: the other side's, mirrored)
            let from = swap(j);
            let Some(p) = j3[from] else { continue };
            let p = match f.joints2d[from] {
                Some((p2, c)) if c > 0.05 && !matches!(j, 0 | 1 | 4 | 7) => picture(p2, p),
                _ => p,
            };
            target[j] = Some(flip(p));
            // how sure: in view (the picture's visibility); the body's centre always; an
            // elbow just out of the picture is a good guess (the arm's shape follows from the
            // shoulder and wrist); any other joint out of view, unsure
            // (a point the tracker puts outside the picture is a guess, however sure it says)
            let seen = f.joints2d[from].filter(|(p, _)| (-0.02..=1.02).contains(&p.x) && (-0.02..=1.02).contains(&p.y)).map_or(0.0, |(_, c)| c);
            sure[j] = if matches!(j, 0 | 7 | 8 | 9 | 10) {
                seen.max(0.6)
            } else if matches!(j, 12 | 15) {
                seen.max(SURE)
            } else {
                seen
            };
        }
        for j in 0..JOINTS {
            if let Some(t) = target[j].filter(|_| sure[j] >= SURE) {
                // (back after a while out of view: from where it is now, not where it was)
                if self.age[j] >= STALE {
                    self.filtered[j] = Euro3::default();
                    self.joints[j] = None;
                }
                // (tuned on live tracking replayed: a low speed term, as the tracker's own
                // shake reads as speed and would open the filter up)
                self.filtered[j].filter(t, fdt, 0.8, 0.5);
                self.age[j] = 0.0;
                self.streak[j] = self.streak[j].saturating_add(1);
            } else {
                self.streak[j] = 0;
            }
        }
        // hands: their way and their fingers
        for side in 0..2 {
            let from = if mirror { 1 - side } else { side };
            let Some(h) = &f.hands[from] else {
                self.curl_targets[side] = [RELAXED; 5];
                continue;
            };
            if self.hand_age[side] >= STALE {
                self.hand_filters[side] = Default::default();
                self.curl_filters[side] = Default::default();
            }
            let Some(p) = &f.hands3d[from] else { continue };
            let p = p.map(flip);
            if let (Some(along), Some(across)) = ((p[9] - p[0]).try_normalize(), (p[17] - p[5]).try_normalize())
                && h[0].1 >= SURE
            {
                let (fa, fc) = &mut self.hand_filters[side];
                let along = fa.filter(along, fdt, 1.0, 0.6).normalize_or(along);
                let across = fc.filter(across, fdt, 1.0, 0.6).normalize_or(across);
                self.hand_dir[side] = Some((along, Some(across)));
                self.hand_age[side] = 0.0;
            }
            for (k, c) in finger_curls_3d(&p).into_iter().enumerate() {
                self.curl_targets[side][k] = self.curl_filters[side][k].filter(c, fdt, 1.5, 1.5);
            }
        }
    }
}

/// Each finger's curl from the hand in 3D: how far it bends at its three joints (the thumb's
/// two), 0 straight to 1 a fist. The knuckle's bend is measured from the palm's own way (wrist
/// to middle knuckle), its sideways spread left out.
fn finger_curls_3d(p: &[Vec3; HAND_POINTS]) -> [f32; 5] {
    let palm = (p[9] - p[0]).normalize_or(Vec3::Y);
    let across = (p[17] - p[5]).normalize_or(Vec3::X);
    let bend = |a: usize, b: usize, c: usize| (p[b] - p[a]).angle_between(p[c] - p[b]);
    let mut out = [0.0; 5];
    out[0] = ((bend(1, 2, 3) + bend(2, 3, 4) - 0.2) / 1.4).clamp(0.0, 1.0);
    for (f, o) in out.iter_mut().enumerate().skip(1) {
        let b = 1 + f * 4;
        let first = p[b + 1] - p[b];
        let first = first - across * first.dot(across);
        let knuckle = palm.angle_between(first);
        *o = ((knuckle + bend(b, b + 1, b + 2) + bend(b + 1, b + 2, b + 3) - 0.25) / 3.6).clamp(0.0, 1.0);
    }
    out
}

/// The idle pose's arm turns (the model's space): T-pose to arms down.
pub fn idle_arm(side_left: bool) -> (Quat, Quat) {
    let s = if side_left { -1.0 } else { 1.0 };
    (Quat::from_rotation_z(s * 72f32.to_radians()), Quat::from_rotation_y(s * 12f32.to_radians()))
}

/// The turn that takes the rest basis (`left`, `up`) to the tracked one.
pub fn basis_turn(rest_left: Vec3, rest_up: Vec3, left: Vec3, up: Vec3) -> Quat {
    let frame = |l: Vec3, u: Vec3| {
        let l = l.normalize_or(Vec3::X);
        let f = l.cross(u).normalize_or(Vec3::Z);
        let u = f.cross(l).normalize_or(Vec3::Y);
        Quat::from_mat3(&glam::Mat3::from_cols(l, u, f))
    };
    (frame(left, up) * frame(rest_left, rest_up).inverse()).normalize()
}

/// The finger bones: (name, which finger: 0 thumb … 4 little, how much of the curl it takes).
pub const FINGER_BONES: &[(&str, usize, f32)] = &[
    ("ThumbProximal", 0, 0.5),
    ("ThumbIntermediate", 0, 0.6),
    ("ThumbDistal", 0, 0.6),
    ("ThumbMetacarpal", 0, 0.3),
    ("IndexProximal", 1, 1.0),
    ("IndexIntermediate", 1, 1.2),
    ("IndexDistal", 1, 0.8),
    ("MiddleProximal", 2, 1.0),
    ("MiddleIntermediate", 2, 1.2),
    ("MiddleDistal", 2, 0.8),
    ("RingProximal", 3, 1.0),
    ("RingIntermediate", 3, 1.2),
    ("RingDistal", 3, 0.8),
    ("LittleProximal", 4, 1.0),
    ("LittleIntermediate", 4, 1.2),
    ("LittleDistal", 4, 0.8),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mirror_swaps_sides() {
        let mut j3 = [None; JOINTS];
        j3[J::LeftWrist as usize] = Some(Vec3::new(0.3, 0.1, 0.2));
        let mut j2 = [None; JOINTS];
        j2[J::LeftWrist as usize] = Some((Vec2::new(0.7, 0.5), 0.9));
        let f = BodyFrame { joints3d: Some(j3), joints2d: j2, hands: [None, None], hands3d: [None, None], aspect: 16.0 / 9.0, frame_id: 1 };
        let mut s = BodyState::default();
        s.step(Some(&f), true, 1.0 / 30.0);
        assert_eq!(s.joints[J::RightWrist as usize], Some(Vec3::new(-0.3, 0.1, 0.2)));
        // the same answer again: the state holds
        s.step(Some(&f), true, 1.0 / 30.0);
        assert_eq!(s.joints[J::RightWrist as usize], Some(Vec3::new(-0.3, 0.1, 0.2)));
        assert!(s.joints[J::LeftWrist as usize].is_none());
    }

    #[test]
    fn a_still_joint_steadies_a_moving_one_follows() {
        let mut e = Euro3::default();
        // jitter of ±1 cm around a still point: much less after filtering
        let mut worst = 0.0f32;
        for i in 0..60 {
            let v = Vec3::new(if i % 2 == 0 { 0.01 } else { -0.01 }, 0.0, 0.0);
            let x = e.filter(v, 1.0 / 30.0, 0.8, 0.5);
            if i > 10 {
                worst = worst.max(x.x.abs());
            }
        }
        assert!(worst < 0.004, "{worst}");
        // a quick move of 30 cm: most of the way there within 0.15 s
        let mut x = Vec3::ZERO;
        for _ in 0..5 {
            x = e.filter(Vec3::new(0.3, 0.0, 0.0), 1.0 / 30.0, 0.8, 0.5);
        }
        assert!(x.x > 0.2, "{x}");
    }

    #[test]
    fn curls_in_3d() {
        let mut p = [Vec3::ZERO; HAND_POINTS];
        // a flat hand, fingers up, a little spread; the wrist under the middle finger
        p[0] = Vec3::new(0.03, -0.06, 0.0);
        for f in 0..5 {
            for i in 0..4 {
                let spread = (f as f32 - 2.0) * 0.12;
                p[1 + f * 4 + i] = Vec3::new(f as f32 * 0.015 + spread * i as f32 * 0.025, i as f32 * 0.025, 0.0);
            }
        }
        assert!(finger_curls_3d(&p).iter().all(|c| *c < 0.05), "{:?}", finger_curls_3d(&p));
        // the index folded: each joint 80°
        let mut q = p;
        let (mut at, mut dir) = (p[5], Vec3::Y);
        for i in 1..4 {
            dir = Quat::from_rotation_x(80f32.to_radians()) * dir;
            at += dir * 0.025;
            q[5 + i] = at;
        }
        assert!(finger_curls_3d(&q)[1] > 0.8, "{:?}", finger_curls_3d(&q));
    }

    #[test]
    fn the_basis_turn() {
        let q = basis_turn(Vec3::X, Vec3::Y, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
        assert!((q * Vec3::X - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-4);
        assert!((q * Vec3::Y - Vec3::Y).length() < 1e-4);
    }
}
