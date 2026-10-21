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
/// keeps normals perpendicular under non-uniform scale.
pub(super) fn normal_matrix(m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let a = |r: usize, c: usize| m[r][c];
	let cof = |r: usize, c: usize| {
		let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
		let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
		a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)
	};
	let det = a(0, 0) * cof(0, 0) + a(0, 1) * cof(0, 1) + a(0, 2) * cof(0, 2);
	let inv_det = if det.abs() > 1e-12 { 1.0 / det } else { 0.0 };
	// (M^-1)^T is the cofactor matrix over the determinant
	let mut out = IDENTITY;
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

/// The direction `d` turned by the upper 3x3 of the row-major transform
/// `m` (no translation), then scaled to unit length. A length below 1e-12
/// divides by 1e-12 instead, so a zero vector stays zero. Unlike
/// [`normal_matrix`], this is the plain 3x3 the OBJ writer uses.
pub(crate) fn apply_dir(m: &[[f32; 4]; 4], d: V3) -> V3 {
	let d = [0, 1, 2].map(|r| m[r][0] * d[0] + m[r][1] * d[1] + m[r][2] * d[2]);
	let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-12);
	d.map(|c| c / len)
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

	#[test]
	fn apply_dir_ignores_the_translation_and_normalises() {
		let m = [
			[2.0, 0.0, 0.0, 5.0],
			[0.0, 4.0, 0.0, 6.0],
			[0.0, 0.0, 0.5, 7.0],
			[0.0, 0.0, 0.0, 1.0],
		];
		assert_eq!(apply(&m, [1.0, 1.0, 1.0]), [7.0, 10.0, 7.5]);
		assert_eq!(apply_dir(&m, [3.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
		let d = apply_dir(&m, [1.0, 1.0, 0.0]);
		let len = 20f32.sqrt();
		assert!(
			close(d[0], 2.0 / len) && close(d[1], 4.0 / len) && d[2] == 0.0,
			"{d:?}"
		);
		// a zero (or vanishing) direction is not blown up
		assert_eq!(apply_dir(&m, [0.0; 3]), [0.0; 3]);
		assert_eq!(apply_dir(&IDENTITY, [1e-14, 0.0, 0.0]), [0.01, 0.0, 0.0]);
	}

	/// The viewport's normalise scales any vector longer than 0, however
	/// short, and turns zero and NaN into zero. Skinning's normalise (in
	/// scene/model_view.rs) stops at 1e-12 and keeps the vector instead.
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
