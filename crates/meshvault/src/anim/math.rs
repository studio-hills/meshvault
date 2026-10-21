//! Rigid transforms in Roblox's convention.
//!
//! A `CFrame` is a position plus a 3x3 rotation matrix. Exports store them
//! as `CFrame:GetComponents()`: `[x, y, z, r00, r01, r02, r10, r11, r12,
//! r20, r21, r22]`, with the rotation in row-major order, so a point maps
//! as `world = R * local + position`.

pub type V3 = [f32; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CFrame {
	pub pos: V3,
	/// Row-major: `rot[row][col]`.
	pub rot: [[f32; 3]; 3],
}

impl Default for CFrame {
	fn default() -> Self {
		Self::IDENTITY
	}
}

fn add(a: V3, b: V3) -> V3 {
	[a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
	[a[0] * s, a[1] * s, a[2] * s]
}
pub fn sub(a: V3, b: V3) -> V3 {
	[a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
/// The straight-line distance between two points.
pub fn distance(a: V3, b: V3) -> f32 {
	let d = sub(a, b);
	(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}
pub fn dot(a: V3, b: V3) -> f32 {
	a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
	[
		a[1] * b[2] - a[2] * b[1],
		a[2] * b[0] - a[0] * b[2],
		a[0] * b[1] - a[1] * b[0],
	]
}
/// The corners of a box centred on the origin with half-size `half`.
/// Corner `i` is on the positive side of X when bit 1 of `i` is set, of Y
/// for bit 2 and of Z for bit 4.
pub fn box_corners(half: V3) -> [V3; 8] {
	std::array::from_fn(|i| {
		std::array::from_fn(|k| if i & (1 << k) == 0 { -half[k] } else { half[k] })
	})
}
/// Widens `(lo, hi)` to take in `p`. Callers choose the starting bounds.
pub fn grow_bounds((lo, hi): &mut (V3, V3), p: V3) {
	for k in 0..3 {
		lo[k] = lo[k].min(p[k]);
		hi[k] = hi[k].max(p[k]);
	}
}
fn lerp3(a: V3, b: V3, t: f32) -> V3 {
	add(a, scale(sub(b, a), t))
}

impl CFrame {
	pub const IDENTITY: CFrame = CFrame {
		pos: [0.0; 3],
		rot: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
	};

	pub fn from_position(pos: V3) -> Self {
		Self {
			pos,
			..Self::IDENTITY
		}
	}

	/// From `CFrame:GetComponents()` order.
	pub fn from_components(c: &[f32; 12]) -> Self {
		Self {
			pos: [c[0], c[1], c[2]],
			rot: [[c[3], c[4], c[5]], [c[6], c[7], c[8]], [c[9], c[10], c[11]]],
		}
	}

	#[cfg(test)]
	pub fn components(&self) -> [f32; 12] {
		let r = &self.rot;
		[
			self.pos[0],
			self.pos[1],
			self.pos[2],
			r[0][0],
			r[0][1],
			r[0][2],
			r[1][0],
			r[1][1],
			r[1][2],
			r[2][0],
			r[2][1],
			r[2][2],
		]
	}

	pub fn rotate(&self, v: V3) -> V3 {
		let r = &self.rot;
		[
			r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
			r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
			r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
		]
	}

	pub fn point(&self, v: V3) -> V3 {
		add(self.rotate(v), self.pos)
	}

	/// `self * other`: apply `other` first, then `self`.
	pub fn mul(&self, other: &CFrame) -> CFrame {
		let (a, b) = (&self.rot, &other.rot);
		let mut rot = [[0.0; 3]; 3];
		for (r, row) in rot.iter_mut().enumerate() {
			for (c, cell) in row.iter_mut().enumerate() {
				*cell = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
			}
		}
		CFrame {
			pos: self.point(other.pos),
			rot,
		}
	}

	/// Inverse of a rigid transform (the rotation is orthonormal).
	pub fn inverse(&self) -> CFrame {
		let r = &self.rot;
		let rot = [
			[r[0][0], r[1][0], r[2][0]],
			[r[0][1], r[1][1], r[2][1]],
			[r[0][2], r[1][2], r[2][2]],
		];
		let inv = CFrame { pos: [0.0; 3], rot };
		CFrame {
			pos: scale(inv.rotate(self.pos), -1.0),
			rot,
		}
	}

	/// Unit quaternion `[x, y, z, w]` of the rotation.
	pub fn quat(&self) -> [f32; 4] {
		let m = &self.rot;
		let trace = m[0][0] + m[1][1] + m[2][2];
		let q = if trace > 0.0 {
			let s = (trace + 1.0).sqrt() * 2.0;
			[
				(m[2][1] - m[1][2]) / s,
				(m[0][2] - m[2][0]) / s,
				(m[1][0] - m[0][1]) / s,
				0.25 * s,
			]
		} else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
			let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
			[
				0.25 * s,
				(m[0][1] + m[1][0]) / s,
				(m[0][2] + m[2][0]) / s,
				(m[2][1] - m[1][2]) / s,
			]
		} else if m[1][1] > m[2][2] {
			let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
			[
				(m[0][1] + m[1][0]) / s,
				0.25 * s,
				(m[1][2] + m[2][1]) / s,
				(m[0][2] - m[2][0]) / s,
			]
		} else {
			let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
			[
				(m[0][2] + m[2][0]) / s,
				(m[1][2] + m[2][1]) / s,
				0.25 * s,
				(m[1][0] - m[0][1]) / s,
			]
		};
		normalize4(q)
	}

	fn from_quat(pos: V3, q: [f32; 4]) -> CFrame {
		let [x, y, z, w] = normalize4(q);
		CFrame {
			pos,
			rot: [
				[
					1.0 - 2.0 * (y * y + z * z),
					2.0 * (x * y - z * w),
					2.0 * (x * z + y * w),
				],
				[
					2.0 * (x * y + z * w),
					1.0 - 2.0 * (x * x + z * z),
					2.0 * (y * z - x * w),
				],
				[
					2.0 * (x * z - y * w),
					2.0 * (y * z + x * w),
					1.0 - 2.0 * (x * x + y * y),
				],
			],
		}
	}

	/// Interpolates position linearly and rotation spherically, like
	/// Roblox's `CFrame:Lerp`.
	pub fn lerp(&self, other: &CFrame, t: f32) -> CFrame {
		CFrame::from_quat(
			lerp3(self.pos, other.pos, t),
			slerp(self.quat(), other.quat(), t),
		)
	}

	/// Rotation about one axis (0 = X, 1 = Y, 2 = Z) by `angle` radians.
	pub fn axis_rotation(axis: usize, angle: f32) -> CFrame {
		let (s, c) = angle.sin_cos();
		let rot = match axis {
			0 => [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
			1 => [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
			_ => [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
		};
		CFrame { pos: [0.0; 3], rot }
	}

	/// `CFrame.fromEulerAngles(x, y, z, order)`: the rotations are multiplied
	/// in the order the letters appear, so `"XYZ"` is `Rx * Ry * Rz`.
	#[cfg(test)]
	pub fn from_euler(angles: V3, order: &str) -> CFrame {
		CFrame::from_euler_axes(angles, euler_axes(order))
	}

	/// The rotations about `axes` (from [`euler_axes`]), multiplied in that
	/// order, each by its own angle in `angles`.
	pub fn from_euler_axes(angles: V3, axes: [usize; 3]) -> CFrame {
		let mut out = CFrame::IDENTITY;
		for axis in axes {
			out = out.mul(&CFrame::axis_rotation(axis, angles[axis]));
		}
		out
	}

	/// Row-major 4x4 matrix with a non-uniform scale applied before the
	/// transform (`self * diag(scale)`), for placing meshes.
	pub fn to_matrix(self, scale: V3) -> [[f32; 4]; 4] {
		let r = &self.rot;
		let mut m = [[0.0; 4]; 4];
		for row in 0..3 {
			for col in 0..3 {
				m[row][col] = r[row][col] * scale[col];
			}
			m[row][3] = self.pos[row];
		}
		m[3][3] = 1.0;
		m
	}

	#[cfg(test)]
	pub fn approx_eq(&self, other: &CFrame, eps: f32) -> bool {
		self.components()
			.iter()
			.zip(other.components())
			.all(|(a, b)| (a - b).abs() <= eps)
	}
}

fn normalize4(q: [f32; 4]) -> [f32; 4] {
	let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
	if len > 0.0 {
		[q[0] / len, q[1] / len, q[2] / len, q[3] / len]
	} else {
		[0.0, 0.0, 0.0, 1.0]
	}
}

/// The axes (0 = X, 1 = Y, 2 = Z) of a curve's `RotationOrder`, in order:
/// its uppercase X, Y and Z letters when there are exactly three of them
/// (other characters are ignored and repeats count), otherwise XYZ.
pub fn euler_axes(order: &str) -> [usize; 3] {
	let mut axes = [0; 3];
	let mut found = 0;
	for letter in order.chars() {
		let axis = match letter {
			'X' => 0,
			'Y' => 1,
			'Z' => 2,
			_ => continue,
		};
		if found == 3 {
			return [0, 1, 2];
		}
		axes[found] = axis;
		found += 1;
	}
	if found == 3 { axes } else { [0, 1, 2] }
}

/// Shortest-path spherical interpolation.
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
	let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
	let b = if dot < 0.0 {
		dot = -dot;
		[-b[0], -b[1], -b[2], -b[3]]
	} else {
		b
	};
	if dot > 0.9995 {
		// nearly identical: plain lerp avoids dividing by ~0
		return normalize4([
			a[0] + (b[0] - a[0]) * t,
			a[1] + (b[1] - a[1]) * t,
			a[2] + (b[2] - a[2]) * t,
			a[3] + (b[3] - a[3]) * t,
		]);
	}
	let theta = dot.clamp(-1.0, 1.0).acos();
	let s = theta.sin();
	let wa = ((1.0 - t) * theta).sin() / s;
	let wb = (t * theta).sin() / s;
	[
		a[0] * wa + b[0] * wb,
		a[1] * wa + b[1] * wb,
		a[2] * wa + b[2] * wb,
		a[3] * wa + b[3] * wb,
	]
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::f32::consts::FRAC_PI_2;

	/// Box corners count in binary over X, Y and Z, X the lowest bit.
	#[test]
	fn box_corners_follow_their_bits() {
		let c = box_corners([1.0, 2.0, 3.0]);
		assert_eq!(c[0], [-1.0, -2.0, -3.0]);
		assert_eq!(c[1], [1.0, -2.0, -3.0]);
		assert_eq!(c[2], [-1.0, 2.0, -3.0]);
		assert_eq!(c[4], [-1.0, -2.0, 3.0]);
		assert_eq!(c[7], [1.0, 2.0, 3.0]);
		assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
		assert_eq!(dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), 32.0);
		// bounds grow from whatever they start as
		let mut b = ([0.0; 3], [0.0; 3]);
		grow_bounds(&mut b, [1.0, -2.0, 0.5]);
		assert_eq!(b, ([0.0, -2.0, 0.0], [1.0, 0.0, 0.5]));
	}

	#[test]
	fn components_round_trip() {
		let c = [1.0, 2.0, 3.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
		assert_eq!(CFrame::from_components(&c).components(), c);
	}

	#[test]
	fn mul_and_inverse() {
		let a = CFrame {
			pos: [1.0, 2.0, 3.0],
			..CFrame::axis_rotation(1, 0.7)
		};
		let b = CFrame {
			pos: [-4.0, 0.5, 2.0],
			..CFrame::axis_rotation(0, -1.1)
		};
		assert!(a.mul(&a.inverse()).approx_eq(&CFrame::IDENTITY, 1e-5));
		// (a*b) applied to a point equals a(b(p))
		let p = [0.3, -2.0, 5.0];
		let lhs = a.mul(&b).point(p);
		let rhs = a.point(b.point(p));
		for k in 0..3 {
			assert!((lhs[k] - rhs[k]).abs() < 1e-5);
		}
	}

	#[test]
	fn rotations_follow_roblox_convention() {
		// CFrame.Angles(0, pi/2, 0) turns the look vector (-Z) towards -X
		let r = CFrame::axis_rotation(1, FRAC_PI_2);
		let look = r.rotate([0.0, 0.0, -1.0]);
		assert!((look[0] + 1.0).abs() < 1e-6 && look[2].abs() < 1e-6);
		// XYZ order multiplies Rx * Ry * Rz
		let e = CFrame::from_euler([0.3, 0.4, 0.5], "XYZ");
		let manual = CFrame::axis_rotation(0, 0.3)
			.mul(&CFrame::axis_rotation(1, 0.4))
			.mul(&CFrame::axis_rotation(2, 0.5));
		assert!(e.approx_eq(&manual, 1e-6));
		let yxz = CFrame::from_euler([0.3, 0.4, 0.5], "YXZ");
		assert!(!yxz.approx_eq(&e, 1e-3), "order must matter");
	}

	/// The rotation order parser as it was before the axes were worked out
	/// once per curve, to check `euler_axes` against.
	fn from_euler_by_letters(angles: V3, order: &str) -> CFrame {
		let mut out = CFrame::IDENTITY;
		let letters: Vec<char> = order.chars().filter(|c| "XYZ".contains(*c)).collect();
		let letters = if letters.len() == 3 {
			letters
		} else {
			vec!['X', 'Y', 'Z']
		};
		for letter in letters {
			let axis = match letter {
				'X' => 0,
				'Y' => 1,
				_ => 2,
			};
			out = out.mul(&CFrame::axis_rotation(axis, angles[axis]));
		}
		out
	}

	#[test]
	fn rotation_orders_are_read_as_before() {
		let cases: [(&str, [usize; 3]); 15] = [
			("XYZ", [0, 1, 2]),
			("XZY", [0, 2, 1]),
			("YXZ", [1, 0, 2]),
			("YZX", [1, 2, 0]),
			("ZXY", [2, 0, 1]),
			("ZYX", [2, 1, 0]),
			// repeats are kept and other characters skipped
			("XXZ", [0, 0, 2]),
			("ZZZ", [2, 2, 2]),
			("Z-Y-X", [2, 1, 0]),
			("Enum.RotationOrder.ZYX", [2, 1, 0]),
			// anything without exactly three uppercase letters is XYZ
			("", [0, 1, 2]),
			("ZY", [0, 1, 2]),
			("zyx", [0, 1, 2]),
			("ZYXZ", [0, 1, 2]),
			("Enum.RotationOrder.ZXYX", [0, 1, 2]),
		];
		let angles = [0.3, -0.9, 1.7];
		for (order, axes) in cases {
			assert_eq!(euler_axes(order), axes, "{order}");
			assert_eq!(
				CFrame::from_euler(angles, order),
				from_euler_by_letters(angles, order),
				"{order}"
			);
		}
	}

	#[test]
	fn quaternion_round_trip_and_slerp() {
		for (axis, angle) in [(0, 0.4), (1, 2.9), (2, -3.0), (1, std::f32::consts::PI)] {
			let r = CFrame::axis_rotation(axis, angle);
			assert!(
				CFrame::from_quat([0.0; 3], r.quat()).approx_eq(&r, 1e-5),
				"axis {axis} angle {angle}"
			);
		}
		let a = CFrame::IDENTITY;
		let b = CFrame {
			pos: [2.0, 0.0, 0.0],
			..CFrame::axis_rotation(2, FRAC_PI_2)
		};
		let mid = a.lerp(&b, 0.5);
		assert!(mid.approx_eq(
			&CFrame {
				pos: [1.0, 0.0, 0.0],
				..CFrame::axis_rotation(2, FRAC_PI_2 / 2.0)
			},
			1e-5
		));
	}
}
