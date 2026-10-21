//! Small vector and row-major 4x4 matrix helpers for drawing.

pub(super) type V3 = [f32; 3];

pub(super) fn sub(a: V3, b: V3) -> V3 {
	[a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(super) fn cross(a: V3, b: V3) -> V3 {
	[
		a[1] * b[2] - a[2] * b[1],
		a[2] * b[0] - a[0] * b[2],
		a[0] * b[1] - a[1] * b[0],
	]
}
pub(super) fn dot(a: V3, b: V3) -> f32 {
	a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(super) fn normalize(a: V3) -> V3 {
	let len = dot(a, a).sqrt();
	if len > 0.0 {
		[a[0] / len, a[1] / len, a[2] / len]
	} else {
		[0.0, 0.0, 0.0]
	}
}

/// Row-major identity, for a mesh shown on its own.
pub const IDENTITY: [[f32; 4]; 4] = [
	[1.0, 0.0, 0.0, 0.0],
	[0.0, 1.0, 0.0, 0.0],
	[0.0, 0.0, 1.0, 0.0],
	[0.0, 0.0, 0.0, 1.0],
];

/// Row-major 4x4 matrix product.
pub(super) fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let mut out = [[0.0; 4]; 4];
	for r in 0..4 {
		for c in 0..4 {
			out[r][c] = (0..4).map(|k| a[r][k] * b[k][c]).sum();
		}
	}
	out
}

pub(super) fn transpose(m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let mut out = [[0.0; 4]; 4];
	for r in 0..4 {
		for c in 0..4 {
			out[c][r] = m[r][c];
		}
	}
	out
}

/// Inverse transpose of the upper 3x3 of a row-major transform, which
/// keeps normals perpendicular under non-uniform scale. A flat (singular)
/// 3x3 gives zeros there.
///
/// The 3x3 is first divided by its largest entry, so how singular it is
/// does not depend on its overall size: a tiny but invertible scale
/// (whose determinant is far below 1e-12) still has normals.
pub(super) fn normal_matrix(m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let size = (0..3)
		.flat_map(|r| (0..3).map(move |c| (r, c)))
		.map(|(r, c)| m[r][c].abs())
		.fold(0.0f32, f32::max);
	let mut out = IDENTITY;
	if !(size > 0.0 && size.is_finite()) {
		for row in out.iter_mut().take(3) {
			row[..3].fill(0.0);
		}
		return out;
	}
	let a = |r: usize, c: usize| m[r][c] / size;
	let cof = |r: usize, c: usize| {
		let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
		let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
		a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)
	};
	let det = a(0, 0) * cof(0, 0) + a(0, 1) * cof(0, 1) + a(0, 2) * cof(0, 2);
	// (M^-1)^T is the cofactor matrix over the determinant; dividing M by
	// `size` multiplies its inverse by `size`, which is undone here
	let inv_det = if det.abs() > 1e-12 {
		1.0 / (det * size)
	} else {
		0.0
	};
	for (r, row) in out.iter_mut().enumerate().take(3) {
		for (c, v) in row.iter_mut().enumerate().take(3) {
			*v = cof(r, c) * inv_det;
		}
	}
	out
}

/// The point `p` placed by the row-major transform `m`.
pub(crate) fn apply(m: &[[f32; 4]; 4], p: V3) -> V3 {
	[
		m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
		m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
		m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
	]
}

/// The normal `n` of a surface placed by the row-major transform `m`
/// (the translation does not reach it), scaled to unit length. A length
/// below 1e-12 divides by 1e-12 instead, so a zero vector stays zero.
///
/// Normals turn by the [`normal_matrix`] so they stay perpendicular under
/// a non-uniform scale. A turn with a uniform scale (no stretch) keeps
/// them perpendicular by itself, so it is used as it is, which gives the
/// same digits as before for every part that is not stretched.
pub(crate) fn apply_normal(m: &[[f32; 4]; 4], n: V3) -> V3 {
	let m = if is_uniform(m) { *m } else { normal_matrix(*m) };
	let n = [0, 1, 2].map(|r| m[r][0] * n[0] + m[r][1] * n[1] + m[r][2] * n[2]);
	let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
	n.map(|c| c / len)
}

/// Whether the upper 3x3 of `m` is a turn with a uniform scale: its
/// columns are square to each other and of the same length.
fn is_uniform(m: &[[f32; 4]; 4]) -> bool {
	let column = |c: usize| [m[0][c], m[1][c], m[2][c]];
	let [x, y, z] = [column(0), column(1), column(2)];
	let scale = (dot(x, x) + dot(y, y) + dot(z, z)) / 3.0;
	let near = |a: f32, b: f32| (a - b).abs() <= 1e-5 * scale;
	[dot(x, x), dot(y, y), dot(z, z)]
		.iter()
		.all(|&d| near(d, scale))
		&& [dot(x, y), dot(y, z), dot(z, x)]
			.iter()
			.all(|&d| near(d, 0.0))
}

/// Whether two numbers agree to 1e-5, for the render tests.
#[cfg(test)]
pub(super) fn close(a: f32, b: f32) -> bool {
	(a - b).abs() < 1e-5
}

#[cfg(test)]
mod tests {
	use super::*;

	fn assert_matrix(actual: [[f32; 4]; 4], expected: [[f32; 4]; 4]) {
		for r in 0..4 {
			for c in 0..4 {
				assert!(
					close(actual[r][c], expected[r][c]),
					"[{r}][{c}]: {actual:?} is not {expected:?}"
				);
			}
		}
	}

	#[test]
	fn normal_matrix_of_a_scale_is_the_inverse_scale() {
		let m = [
			[2.0, 0.0, 0.0, 5.0],
			[0.0, 4.0, 0.0, 6.0],
			[0.0, 0.0, 0.5, 7.0],
			[0.0, 0.0, 0.0, 1.0],
		];
		// the translation does not reach the normals
		assert_matrix(
			normal_matrix(m),
			[
				[0.5, 0.0, 0.0, 0.0],
				[0.0, 0.25, 0.0, 0.0],
				[0.0, 0.0, 2.0, 0.0],
				[0.0, 0.0, 0.0, 1.0],
			],
		);
	}

	#[test]
	fn normal_matrix_of_a_rotation_is_the_rotation() {
		let (s, c) = 0.7f32.sin_cos();
		let m = [
			[c, 0.0, s, 0.0],
			[0.0, 1.0, 0.0, 0.0],
			[-s, 0.0, c, 0.0],
			[0.0, 0.0, 0.0, 1.0],
		];
		assert_matrix(normal_matrix(m), m);
	}

	#[test]
	fn normal_matrix_of_a_flat_transform_is_zero() {
		let mut m = IDENTITY;
		m[1][1] = 0.0;
		let mut expected = [[0.0; 4]; 4];
		expected[3][3] = 1.0;
		assert_matrix(normal_matrix(m), expected);
	}

	/// Normals are turned by the inverse transpose, so under a non-uniform
	/// scale they stay perpendicular to the surface: stretching X by 2
	/// makes a 45° slope shallower, so its normal leans towards Y, not X
	/// (as the plain 3x3 made it, bug B28).
	#[test]
	fn apply_normal_ignores_the_translation_and_keeps_normals_perpendicular() {
		let m = [
			[2.0, 0.0, 0.0, 5.0],
			[0.0, 4.0, 0.0, 6.0],
			[0.0, 0.0, 0.5, 7.0],
			[0.0, 0.0, 0.0, 1.0],
		];
		assert_eq!(apply(&m, [1.0, 1.0, 1.0]), [7.0, 10.0, 7.5]);
		assert_eq!(apply_normal(&m, [3.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
		let n = apply_normal(&m, [1.0, 1.0, 0.0]);
		let len = 5f32.sqrt();
		assert!(
			close(n[0], 2.0 / len) && close(n[1], 1.0 / len) && n[2] == 0.0,
			"{n:?}"
		);
		// still perpendicular to the moved edge from (0, 0) to (1, -1)
		let edge = sub(apply(&m, [1.0, -1.0, 0.0]), apply(&m, [0.0; 3]));
		assert!(close(dot(n, edge), 0.0), "{n:?} {edge:?}");
		// a zero (or vanishing) normal is not blown up
		assert_eq!(apply_normal(&m, [0.0; 3]), [0.0; 3]);
		assert_eq!(apply_normal(&IDENTITY, [1e-14, 0.0, 0.0]), [0.01, 0.0, 0.0]);
	}

	/// A tiny but invertible non-uniform scale (its determinant, 5e-13, is
	/// below the singular cutoff) still turns normals: the cutoff is
	/// relative to the matrix's size, not absolute.
	#[test]
	fn apply_normal_of_a_tiny_stretch_is_not_zero() {
		let mut m = IDENTITY;
		m[0][0] = 0.0001;
		m[1][1] = 0.0001;
		m[2][2] = 0.00005;
		let n = apply_normal(&m, [1.0, 1.0, 0.0]);
		let half = 0.5f32.sqrt();
		assert!(
			close(n[0], half) && close(n[1], half) && n[2] == 0.0,
			"{n:?}"
		);
		let n = apply_normal(&m, [0.0, 1.0, 1.0]);
		let len = 5f32.sqrt();
		assert!(
			n[0] == 0.0 && close(n[1], 1.0 / len) && close(n[2], 2.0 / len),
			"{n:?}"
		);
		// the true inverse, not just its direction
		let inverse = normal_matrix(m);
		assert!(
			close(inverse[0][0] / 1e4, 1.0) && close(inverse[2][2] / 2e4, 1.0),
			"{inverse:?}"
		);
		// a flat transform at any size still has no normals
		m[1][1] = 0.0;
		assert_eq!(apply_normal(&m, [1.0, 1.0, 0.0]), [0.0; 3]);
	}

	/// A turn with a uniform scale keeps normals perpendicular as it is, so
	/// it gives exactly what the plain 3x3 gave, and exports stay the same
	/// to the last digit.
	#[test]
	fn apply_normal_of_a_scaled_turn_is_the_plain_turn() {
		let (s, c) = 0.7f32.sin_cos();
		let m = [
			[3.0 * c, 0.0, 3.0 * s, 1.0],
			[0.0, 3.0, 0.0, 2.0],
			[-3.0 * s, 0.0, 3.0 * c, 3.0],
			[0.0, 0.0, 0.0, 1.0],
		];
		let d = [0.3, -0.5, 0.8];
		let plain = [0, 1, 2].map(|r| m[r][0] * d[0] + m[r][1] * d[1] + m[r][2] * d[2]);
		let len = dot(plain, plain).sqrt();
		assert_eq!(apply_normal(&m, d), plain.map(|v| v / len));
	}

	/// The viewport's normalise scales any vector longer than 0, however
	/// short, and turns zero and NaN into zero. Skinning's normalise (in
	/// scene/model_view/skin.rs) stops at 1e-12 and keeps the vector instead.
	#[test]
	fn normalize_scales_any_nonzero_vector_and_zeroes_the_rest() {
		assert_eq!(normalize([3.0, 0.0, 4.0]), [0.6, 0.0, 0.8]);
		// far below skinning's 1e-12 it still normalises
		let n = normalize([1e-13, 0.0, 0.0]);
		assert!(close(n[0], 1.0) && n[1] == 0.0 && n[2] == 0.0, "{n:?}");
		assert_eq!(normalize([0.0; 3]), [0.0; 3]);
		// a NaN length is not > 0, so every component falls back to zero
		assert_eq!(normalize([f32::NAN, 1.0, 0.0]), [0.0; 3]);
	}
}
