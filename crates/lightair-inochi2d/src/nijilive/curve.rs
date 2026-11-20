/*!
nijilive's curves for PathDeformers: a Bezier through all control points, or a Catmull-Rom
spline, each with its closest-point search. Ported from nijilive (`core/nodes/deformer/curve.d`)
and its C++ port nicxlive (`core/nodes/curve.cpp`); both BSD-2-Clause, © 2020 Inochi2D Project,
© 2024 nijigenerate Project.
*/

use glam::Vec2;

use crate::node::components::CurveType;

/// A curve through control points (a PathDeformer's space).
#[derive(Clone, Default)]
pub(crate) struct Curve {
	kind: Kind,
	points: Vec<Vec2>,
	/// A Bezier's derivative's control points.
	derivatives: Vec<Vec2>,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Kind {
	Bezier,
	#[default]
	Spline,
}

fn binomial(n: usize, k: usize) -> f32 {
	if k > n {
		return 0.0;
	}
	if k == 0 || k == n {
		return 1.0;
	}
	let k = k.min(n - k);
	let mut res = 1.0f32;
	for i in 0..k {
		res *= (n - i) as f32 / (i + 1) as f32;
	}
	res
}

/// Bernstein sum of `points` at `t` (clamped to 0..1).
fn bernstein(points: &[Vec2], t: f32) -> Vec2 {
	if points.is_empty() {
		return Vec2::ZERO;
	}
	let order = points.len() - 1;
	let t = t.clamp(0.0, 1.0);
	let one_minus = 1.0 - t;
	let mut t_pow = vec![1.0f32; order + 1];
	let mut one_pow = vec![1.0f32; order + 1];
	for i in 1..=order {
		t_pow[i] = t_pow[i - 1] * t;
		one_pow[i] = one_pow[i - 1] * one_minus;
	}
	let mut res = Vec2::ZERO;
	for (i, p) in points.iter().enumerate() {
		let coeff = binomial(order, i) * one_pow[order - i] * t_pow[i];
		res.x += coeff * p.x;
		res.y += coeff * p.y;
	}
	res
}

/// D's `std.numeric.findLocalMin` (Brent's method) on 0..1, in f32 as nijilive uses it.
fn find_local_min01(f: impl Fn(f32) -> f32) -> f32 {
	// (3 - sqrt(5)) / 2 and its complement
	const C: f32 = 0.381_966_02;
	const CM1: f32 = 0.618_034;
	let rel_tolerance = f32::EPSILON.sqrt();
	let abs_tolerance = f32::EPSILON.sqrt();

	let (mut a, mut b) = (0.0f32, 1.0f32);
	let mut v = a * CM1 + b * C;
	let mut fv = f(v);
	if !fv.is_finite() {
		return v.clamp(0.0, 1.0);
	}
	let (mut w, mut fw) = (v, fv);
	let (mut x, mut fx) = (v, fv);
	let (mut d, mut e) = (0.0f32, 0.0f32);

	loop {
		let mut m = (a + b) * 0.5;
		if !m.is_finite() {
			m = a * 0.5 + b * 0.5;
			if !m.is_finite() {
				return x.clamp(0.0, 1.0);
			}
		}
		let tolerance = abs_tolerance * x.abs() + rel_tolerance;
		let t2 = tolerance * 2.0;
		if !((x - m).abs() > t2 - (b - a) * 0.5) {
			break;
		}

		let (mut p, mut q, mut r) = (0.0f32, 0.0f32, 0.0f32);
		if e.abs() > tolerance {
			let xw = x - w;
			let fxw = fx - fw;
			let xv = x - v;
			let fxv = fx - fv;
			let xwfxv = xw * fxv;
			let xvfxw = xv * fxw;
			p = xv * xvfxw - xw * xwfxv;
			q = (xvfxw - xwfxv) * 2.0;
			if q > 0.0 {
				p = -p;
			} else {
				q = -q;
			}
			r = e;
			e = d;
		}

		let mut u;
		if p.abs() < (q * r * 0.5).abs() && p > q * (a - x) && p < q * (b - x) {
			d = p / q;
			u = x + d;
			if u - a < t2 || b - u < t2 {
				d = if x < m { tolerance } else { -tolerance };
			}
		} else {
			e = if x < m { b } else { a } - x;
			d = C * e;
		}

		u = x + if d.abs() >= tolerance {
			d
		} else if d > 0.0 {
			tolerance
		} else {
			-tolerance
		};
		let fu = f(u);
		if !fu.is_finite() {
			return u.clamp(0.0, 1.0);
		}

		if fu <= fx {
			if u < x {
				b = x;
			} else {
				a = x;
			}
			v = w;
			fv = fw;
			w = x;
			fw = fx;
			x = u;
			fx = fu;
		} else {
			if u < x {
				a = u;
			} else {
				b = u;
			}
			if fu <= fw || w == x {
				v = w;
				fv = fw;
				w = u;
				fw = fu;
			} else if fu <= fv || v == x || v == w {
				v = u;
				fv = fu;
			}
		}
	}
	x.clamp(0.0, 1.0)
}

impl Curve {
	pub fn new(kind: CurveType, points: Vec<Vec2>) -> Self {
		let kind = match kind {
			CurveType::Bezier => Kind::Bezier,
			CurveType::Spline => Kind::Spline,
		};
		let derivatives = if kind == Kind::Bezier && points.len() > 1 {
			let order = (points.len() - 1) as f32;
			points.windows(2).map(|w| (w[1] - w[0]) * order).collect()
		} else {
			Vec::new()
		};
		Self {
			kind,
			points,
			derivatives,
		}
	}

	pub fn points(&self) -> &[Vec2] {
		&self.points
	}

	/// The spline's segment for `t`: its four control points and where in it `t` is. (At `t = 1`
	/// the reference lands on the start of the last segment, not its end; kept.)
	fn spline_segment(&self, t: f32) -> ([Vec2; 4], f32) {
		let len = self.points.len();
		let t = t.clamp(0.0, 1.0);
		let segment = t * (len - 1) as f32;
		let index = segment as i64;
		let p1 = index.clamp(0, len as i64 - 2) as usize;
		let p0 = p1.saturating_sub(1);
		let p2 = (p1 + 1).min(len - 1);
		let p3 = (p2 + 1).min(len - 1);
		let lt = segment - index as f32;
		let p = &self.points;
		([p[p0], p[p1], p[p2], p[p3]], lt)
	}

	pub fn point(&self, t: f32) -> Vec2 {
		match self.kind {
			Kind::Bezier => bernstein(&self.points, t),
			Kind::Spline => {
				let len = self.points.len();
				if len < 2 {
					return Vec2::ZERO;
				}
				if len == 2 {
					let t = t.clamp(0.0, 1.0);
					return self.points[0] * (1.0 - t) + self.points[1] * t;
				}
				let ([p0, p1, p2, p3], lt) = self.spline_segment(t);
				let (lt2, lt3) = (lt * lt, lt * lt * lt);
				let a = 2.0 * p1;
				let b = p2 - p0;
				let c = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
				let d = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
				0.5 * (a + b * lt + c * lt2 + d * lt3)
			}
		}
	}

	pub fn derivative(&self, t: f32) -> Vec2 {
		match self.kind {
			Kind::Bezier => bernstein(&self.derivatives, t),
			Kind::Spline => {
				let len = self.points.len();
				if len < 2 {
					return Vec2::ZERO;
				}
				if len == 2 {
					return self.points[1] - self.points[0];
				}
				let ([p0, p1, p2, p3], lt) = self.spline_segment(t);
				let lt2 = lt * lt;
				let b = p2 - p0;
				let c = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
				let d = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
				0.5 * (b + 2.0 * c * lt + 3.0 * d * lt2)
			}
		}
	}

	/// Where on the curve (0..1) is closest to `p`: a local minimum, as the reference finds it.
	pub fn closest_point(&self, p: Vec2) -> f32 {
		find_local_min01(|t| (self.point(t) - p).length_squared())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_straight_spline_finds_its_points() {
		let c = Curve::new(
			CurveType::Spline,
			vec![
				Vec2::new(0.0, 0.0),
				Vec2::new(0.0, 10.0),
				Vec2::new(0.0, 20.0),
				Vec2::new(0.0, 30.0),
			],
		);
		let t = c.closest_point(Vec2::new(3.0, 15.0));
		assert!(
			(c.point(t) - Vec2::new(0.0, 15.0)).length() < 0.01,
			"{t} {:?}",
			c.point(t)
		);
		assert!((c.derivative(t).normalize() - Vec2::Y).length() < 1e-4);
		// the reference's quirk: t = 1 is the last segment's start
		assert_eq!(c.point(1.0), Vec2::new(0.0, 20.0));
	}

	#[test]
	fn a_bezier_goes_through_its_ends() {
		let c = Curve::new(
			CurveType::Bezier,
			vec![Vec2::new(0.0, 0.0), Vec2::new(5.0, 10.0), Vec2::new(10.0, 0.0)],
		);
		assert_eq!(c.point(0.0), Vec2::ZERO);
		assert!((c.point(1.0) - Vec2::new(10.0, 0.0)).length() < 1e-5);
		assert!((c.point(0.5) - Vec2::new(5.0, 5.0)).length() < 1e-5);
		assert!((c.derivative(0.5) - Vec2::new(10.0, 0.0)).length() < 1e-4);
	}
}
