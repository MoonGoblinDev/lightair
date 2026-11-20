//! From face landmarks (MediaPipe's 468-point mesh and the irises, in image coordinates) to what
//! an avatar needs: the head's rotation, each eye's openness, the gaze, the brows, the mouth and
//! its vowel. Measured as the camera sees the person (the person's left is the image's right);
//! [`FaceRig::mirrored`] makes it a mirror image.
//!
//! The head's rotation is fitted to MediaPipe's canonical face (scaled orthographic, least
//! squares over points that don't move with expressions). Eyes, brows and mouth are ratios of
//! distances, so they don't change with the distance to the camera, read against a calibrated
//! neutral face.

use glam::{Mat3, Quat, Vec2, Vec3};

use crate::canonical::FACE;

/// The person's right (the image's left) and left eye, brow and so on.
const RIGHT: usize = 0;
const LEFT: usize = 1;

/// Mesh points (MediaPipe face mesh indices).
mod p {
    pub const EYE_OUTER: [usize; 2] = [33, 263];
    pub const EYE_INNER: [usize; 2] = [133, 362];
    pub const EYE_UPPER: [[usize; 3]; 2] = [[159, 158, 160], [386, 385, 387]];
    pub const EYE_LOWER: [[usize; 3]; 2] = [[145, 153, 144], [374, 380, 373]];
    pub const BROW: [[usize; 3]; 2] = [[105, 66, 107], [334, 296, 336]];
    pub const MOUTH_CORNER: [usize; 2] = [61, 291];
    pub const LIP_INNER_UPPER: [usize; 3] = [13, 82, 312];
    pub const LIP_INNER_LOWER: [usize; 3] = [14, 87, 317];
    /// Points that hold still whatever the face does: forehead, nose, cheekbones, eye corners.
    pub const RIGID: [usize; 31] = [
        1, 4, 5, 6, 8, 9, 10, 151, 168, 195, 197, 33, 133, 263, 362, 234, 454, 127, 356, 93, 323, 116, 345, 50, 280, 98, 327, 70, 300, 107, 336,
    ];
}

/// Landmarks of one frame.
pub struct Landmarks<'a> {
    /// The 468 mesh points, 0..1 across and down the image.
    pub mesh: &'a [[f32; 2]],
    /// Iris centres (any order; matched to the nearer eye), 0..1.
    pub irises: &'a [[f32; 2]],
    /// The image's size, in pixels (its aspect matters).
    pub width: f32,
    pub height: f32,
}

/// What the face does, as the camera sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    /// The head's rotation in the camera's space (x right, y up, z toward the camera): identity
    /// facing the camera.
    pub head: Quat,
    /// Openness, 0 shut to 1 open: the person's right and left eye.
    pub eye_open: [f32; 2],
    /// Where the eyes look, from straight ahead: x toward the image's right, y up, -1..1.
    pub gaze: Vec2,
    /// Brows, -1 lowered to 1 raised: right and left.
    pub brow: [f32; 2],
    /// 0 shut to 1 wide open.
    pub mouth_open: f32,
    /// -1 pursed to 1 wide (a smile, "ee").
    pub mouth_wide: f32,
}

impl Default for Face {
    fn default() -> Self {
        Face { head: Quat::IDENTITY, eye_open: [1.0; 2], gaze: Vec2::ZERO, brow: [0.0; 2], mouth_open: 0.0, mouth_wide: 0.0 }
    }
}

impl Face {
    /// As a mirror shows it: left and right swapped, turns the other way.
    pub fn mirrored(&self) -> Face {
        let q = self.head;
        Face {
            // reflected across the x = 0 plane: yaw and roll change sign, pitch stays
            head: Quat::from_xyzw(q.x, -q.y, -q.z, q.w),
            eye_open: [self.eye_open[1], self.eye_open[0]],
            gaze: Vec2::new(-self.gaze.x, self.gaze.y),
            brow: [self.brow[1], self.brow[0]],
            ..*self
        }
    }

    /// The vowel the mouth makes, [A, I, U, E, O], 0..1 each.
    pub fn vowels(&self) -> [f32; 5] {
        let o = self.mouth_open.clamp(0.0, 1.0);
        let wide = self.mouth_wide.clamp(0.0, 1.0);
        let round = (-self.mouth_wide).clamp(0.0, 1.0);
        let a = o * (1.0 - wide.max(round));
        let i = wide * (1.0 - o) * o.min(0.25) * 4.0;
        let e = wide * o;
        let u = round * (1.0 - o) * o.min(0.25) * 4.0;
        let oo = round * o;
        let v = [a, i, u, e, oo];
        let sum: f32 = v.iter().sum();
        if sum > 1.0 { v.map(|x| x / sum) } else { v }
    }

    /// Head yaw, pitch and roll in degrees (yaw > 0 toward the image's right, pitch > 0 up, roll
    /// > 0 counter-clockwise as the camera sees it).
    ///
    /// The head is yaw, then pitch, then roll: `Ry(yaw)·Rx(-pitch)·Rz(roll)`.
    pub fn angles(&self) -> (f32, f32, f32) {
        let (y, x, z) = self.head.to_euler(glam::EulerRot::YXZ);
        (y.to_degrees(), -x.to_degrees(), z.to_degrees())
    }
}

/// The measurements a neutral face gives (looking at the camera, eyes open, mouth shut).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calibration {
    pub head: Quat,
    pub eye: [f32; 2],
    pub brow: [f32; 2],
    pub mouth_width: f32,
}

impl Default for Calibration {
    fn default() -> Self {
        Calibration { head: Quat::IDENTITY, eye: [0.30; 2], brow: [0.62; 2], mouth_width: 0.95 }
    }
}

/// Raw measurements of a frame, before calibration.
#[derive(Clone, Copy, Debug)]
pub struct Measures {
    pub head: Quat,
    pub eye: [f32; 2],
    pub gaze: [Vec2; 2],
    pub brow: [f32; 2],
    pub mouth_gap: f32,
    pub mouth_width: f32,
}

/// Measures a frame. `None` without a whole mesh.
pub fn measure(l: &Landmarks) -> Option<Measures> {
    if l.mesh.len() < 468 {
        return None;
    }
    // pixels, y up
    let pt = |i: usize| Vec2::new(l.mesh[i][0] * l.width, -l.mesh[i][1] * l.height);
    let iod = pt(p::EYE_OUTER[RIGHT]).distance(pt(p::EYE_OUTER[LEFT])).max(1.0);

    let head = fit_head(&pt)?;

    let mut eye = [0.0; 2];
    let mut gaze = [Vec2::ZERO; 2];
    let mut brow = [0.0; 2];
    let irises: Vec<Vec2> = l.irises.iter().map(|i| Vec2::new(i[0] * l.width, -i[1] * l.height)).collect();
    for s in [RIGHT, LEFT] {
        let outer = pt(p::EYE_OUTER[s]);
        let inner = pt(p::EYE_INNER[s]);
        let width = outer.distance(inner).max(1e-3);
        let upper = p::EYE_UPPER[s].iter().map(|&i| pt(i)).sum::<Vec2>() / 3.0;
        let lower = p::EYE_LOWER[s].iter().map(|&i| pt(i)).sum::<Vec2>() / 3.0;
        eye[s] = upper.distance(lower) / width;
        let centre = (outer + inner) * 0.5;
        // the iris nearest this eye, against the eye's own axes
        if let Some(iris) = irises.iter().min_by(|a, b| a.distance(centre).total_cmp(&b.distance(centre))).filter(|i| i.distance(centre) < width) {
            let across = (inner - outer).normalize_or_zero();
            let across = if across.x < 0.0 { -across } else { across };
            let up = Vec2::new(-across.y, across.x);
            let mid = (upper + lower) * 0.5;
            let d = *iris - Vec2::new(centre.x, mid.y);
            gaze[s] = Vec2::new(d.dot(across) / (width * 0.22), d.dot(up) / (width * 0.18)).clamp(Vec2::splat(-1.0), Vec2::splat(1.0));
        }
        let b = p::BROW[s].iter().map(|&i| pt(i)).sum::<Vec2>() / 3.0;
        brow[s] = b.distance(centre) / iod;
    }
    let lip_up = p::LIP_INNER_UPPER.iter().map(|&i| pt(i)).sum::<Vec2>() / 3.0;
    let lip_down = p::LIP_INNER_LOWER.iter().map(|&i| pt(i)).sum::<Vec2>() / 3.0;
    let mouth_gap = lip_up.distance(lip_down) / iod;
    let mouth_width = pt(p::MOUTH_CORNER[RIGHT]).distance(pt(p::MOUTH_CORNER[LEFT])) / iod;
    Some(Measures { head, eye, gaze, brow, mouth_gap, mouth_width })
}

/// The head's rotation: the canonical face's rigid points fitted to the image's (scaled
/// orthographic camera, least squares), its rows made orthonormal.
fn fit_head(pt: &dyn Fn(usize) -> Vec2) -> Option<Quat> {
    let n = p::RIGID.len() as f32;
    let xs: Vec<Vec2> = p::RIGID.iter().map(|&i| pt(i)).collect();
    let ys: Vec<Vec3> = p::RIGID.iter().map(|&i| Vec3::from(FACE[i])).collect();
    let xm = xs.iter().sum::<Vec2>() / n;
    let ym = ys.iter().sum::<Vec3>() / n;
    // M (2×3) = (Σ x Yᵀ)(Σ Y Yᵀ)⁻¹
    let mut a = [Vec3::ZERO; 2];
    let mut yy = Mat3::ZERO;
    for (x, y) in xs.iter().zip(&ys) {
        let (x, y) = (*x - xm, *y - ym);
        a[0] += y * x.x;
        a[1] += y * x.y;
        yy += Mat3::from_cols(y * y.x, y * y.y, y * y.z);
    }
    if yy.determinant().abs() < 1e-9 {
        return None;
    }
    let inv = yy.inverse();
    let r1 = inv * a[0];
    let r2 = inv * a[1];
    if r1.length() < 1e-6 || r2.length() < 1e-6 {
        return None;
    }
    // orthonormal rows, splitting the difference between the two
    let (mut r1, mut r2) = (r1.normalize(), r2.normalize());
    let c = r1.dot(r2) * 0.5;
    (r1, r2) = ((r1 - r2 * c).normalize(), (r2 - r1 * c).normalize());
    let r3 = r1.cross(r2).normalize();
    let r2 = r3.cross(r1).normalize();
    let m = Mat3::from_cols(r1, r2, r3).transpose();
    let q = Quat::from_mat3(&m).normalize();
    q.is_finite().then_some(q)
}

impl Calibration {
    /// A neutral face's measurements.
    pub fn from(m: &Measures) -> Calibration {
        Calibration { head: m.head, eye: m.eye, brow: m.brow, mouth_width: m.mouth_width }
    }

    pub fn face(&self, m: &Measures) -> Face {
        let open = |s: usize| {
            let r = m.eye[s] / self.eye[s].max(0.05);
            ((r - 0.45) / (0.85 - 0.45)).clamp(0.0, 1.0)
        };
        let brow = |s: usize| ((m.brow[s] / self.brow[s].max(0.05) - 1.0) / 0.12).clamp(-1.0, 1.0);
        // gaze: both eyes together (closing ones don't count)
        let w = [open(RIGHT), open(LEFT)];
        let gaze = if w[0] + w[1] > 0.1 { (m.gaze[0] * w[0] + m.gaze[1] * w[1]) / (w[0] + w[1]) } else { Vec2::ZERO };
        Face {
            head: (m.head * self.head.inverse()).normalize(),
            eye_open: w,
            gaze,
            brow: [brow(RIGHT), brow(LEFT)],
            mouth_open: ((m.mouth_gap - 0.03) / 0.45).clamp(0.0, 1.0),
            mouth_wide: ((m.mouth_width / self.mouth_width.max(0.1) - 1.0) / 0.18).clamp(-1.0, 1.0),
        }
    }
}

/// Smooths a face over time, and lets it settle to neutral when the face is lost.
#[derive(Clone, Debug, Default)]
pub struct Smoother {
    face: Option<Face>,
}

impl Smoother {
    /// The next face: `seen` this frame (or not), `dt` seconds after the last.
    pub fn step(&mut self, seen: Option<Face>, dt: f32) -> Face {
        let dt = dt.clamp(0.0, 0.25);
        let k = |tau: f32| 1.0 - (-dt / tau).exp();
        let prev = self.face.unwrap_or_default();
        let (target, slow) = match seen {
            Some(f) => (f, 1.0),
            // lost: back to neutral over a second
            None => (Face::default(), 12.0),
        };
        let lerp = |a: f32, b: f32, tau: f32| a + (b - a) * k(tau * slow);
        let next = Face {
            head: prev.head.slerp(target.head, k(0.06 * slow)),
            eye_open: [lerp(prev.eye_open[0], target.eye_open[0], 0.03), lerp(prev.eye_open[1], target.eye_open[1], 0.03)],
            gaze: prev.gaze.lerp(target.gaze, k(0.08 * slow)),
            brow: [lerp(prev.brow[0], target.brow[0], 0.08), lerp(prev.brow[1], target.brow[1], 0.08)],
            mouth_open: lerp(prev.mouth_open, target.mouth_open, 0.04),
            mouth_wide: lerp(prev.mouth_wide, target.mouth_wide, 0.08),
        };
        self.face = Some(next);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical face, turned by `q`, as a camera sees it.
    fn project(q: Quat) -> Vec<[f32; 2]> {
        FACE.iter()
            .map(|p| {
                let v = q * Vec3::from(*p);
                // 640×480, 20 px a centimetre, centred
                [(320.0 + v.x * 20.0) / 640.0, (240.0 - v.y * 20.0) / 480.0]
            })
            .collect()
    }

    #[test]
    fn the_head_turn_comes_back() {
        for (yaw, pitch, roll) in [(0.0f32, 0.0f32, 0.0f32), (25.0, 0.0, 0.0), (-30.0, 10.0, 0.0), (0.0, -15.0, 12.0), (15.0, 8.0, -10.0)] {
            let q = Quat::from_rotation_y(yaw.to_radians()) * Quat::from_rotation_x(-pitch.to_radians()) * Quat::from_rotation_z(roll.to_radians());
            let mesh = project(q);
            let m = measure(&Landmarks { mesh: &mesh, irises: &[], width: 640.0, height: 480.0 }).unwrap();
            let face = Calibration::default().face(&m);
            let (y, p, r) = face.angles();
            assert!((y - yaw).abs() < 2.0 && (p - pitch).abs() < 2.0 && (r - roll).abs() < 2.0, "{yaw},{pitch},{roll} → {y},{p},{r}");
        }
    }

    #[test]
    fn the_mirror_turns_the_other_way() {
        let face = Face { head: Quat::from_rotation_y(0.4) * Quat::from_rotation_z(0.2), eye_open: [0.2, 1.0], ..Face::default() };
        let (y, _, r) = face.angles();
        let (my, _, mr) = face.mirrored().angles();
        assert!((y + my).abs() < 0.01 && (r + mr).abs() < 0.01);
        assert_eq!(face.mirrored().eye_open, [1.0, 0.2]);
    }

    #[test]
    fn vowels() {
        let shut = Face::default().vowels();
        assert!(shut.iter().all(|v| *v < 0.01));
        let a = Face { mouth_open: 1.0, ..Face::default() }.vowels();
        assert!(a[0] > 0.9);
        let o = Face { mouth_open: 0.8, mouth_wide: -1.0, ..Face::default() }.vowels();
        assert!(o[4] > o[0] && o[4] > 0.5);
        let e = Face { mouth_open: 0.6, mouth_wide: 1.0, ..Face::default() }.vowels();
        assert!(e[3] > 0.4);
    }
}
