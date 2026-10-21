//! Roblox part shapes as unit solids.

use std::sync::{Mutex, OnceLock};

use crate::scene::geometry::Geometry;

/// Roblox part shapes, as unit-sized solids centred on the origin; a part
/// scales one to its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primitive {
	Block,
	/// A sphere (Ball parts, and SpecialMesh spheres, which stretch).
	Ball,
	/// A cylinder along X, as Part.Shape = Cylinder.
	CylinderX,
	/// A SpecialMesh head: an upright cylinder whose top and bottom edges
	/// are rounded by a quarter of its height, as measured in Roblox Studio.
	Head,
	/// Full height at the back (+Z), sloping down to the front (-Z).
	Wedge,
	/// A pyramid with its peak over the front right corner (+X, -Z): the
	/// top is at height `min(x, -z)`, measured in Roblox Studio.
	CornerWedge,
	/// A TrussPart (AlternatingSupports): 2-stud cubes stacked `segments`
	/// times along `axis` (0 = X, 1 = Y, 2 = Z), each framed by corner
	/// posts and rings and crossed by an X brace on every side.
	Truss {
		segments: u16,
		axis: u8,
	},
}

impl Primitive {
	pub fn geometry(self) -> &'static Geometry {
		static SHAPES: [OnceLock<Geometry>; 6] = [const { OnceLock::new() }; 6];
		let (slot, make): (usize, fn() -> Geometry) = match self {
			Primitive::Block => (0, unit_cube),
			Primitive::Ball => (1, unit_sphere),
			Primitive::CylinderX => (2, unit_cylinder),
			Primitive::Head => (3, unit_head),
			Primitive::Wedge => (4, unit_wedge),
			Primitive::CornerWedge => (5, unit_corner_wedge),
			Primitive::Truss { segments, axis } => return truss_geometry(segments, axis),
		};
		SHAPES[slot].get_or_init(make)
	}

	/// The shape's name in lower case, as the part list tags it.
	pub fn label(self) -> &'static str {
		match self {
			Primitive::Block => "block",
			Primitive::Ball => "ball",
			Primitive::CylinderX => "cylinder",
			Primitive::Head => "head",
			Primitive::Truss { .. } => "truss",
			Primitive::Wedge => "wedge",
			Primitive::CornerWedge => "corner wedge",
		}
	}
}

/// The truss of `segments` along `axis`, built the first time it is asked for.
fn truss_geometry(segments: u16, axis: u8) -> &'static Geometry {
	// one per segment count and axis ever drawn, leaked and kept for the
	// life of the process: closing a model frees none of them. Each grows
	// with its segment count, and the 4096 clamp in model_view bounds one
	// shape, not the sum. Deliberate: trusses are rare and few lengths
	// recur, so this trades memory for not rebuilding them; any budget or
	// eviction is separate work.
	static TRUSSES: Mutex<Vec<((u16, u8), &'static Geometry)>> = Mutex::new(Vec::new());
	let mut built = TRUSSES.lock().unwrap_or_else(|e| e.into_inner());
	if let Some((_, g)) = built.iter().find(|(k, _)| *k == (segments, axis)) {
		return g;
	}
	let g: &'static Geometry = Box::leak(Box::new(unit_truss(segments, axis)));
	built.push(((segments, axis), g));
	g
}

/// Turns every triangle of a convex solid centred on the origin to face
/// outwards (counter-clockwise from outside).
fn face_outwards(positions: &[[f32; 3]], triangles: &mut [[u32; 3]]) {
	for t in triangles {
		let [a, b, c] = t.map(|i| positions[i as usize]);
		let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
		let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
		let n = [
			ab[1] * ac[2] - ab[2] * ac[1],
			ab[2] * ac[0] - ab[0] * ac[2],
			ab[0] * ac[1] - ab[1] * ac[0],
		];
		let centre = [
			(a[0] + b[0] + c[0]) / 3.0,
			(a[1] + b[1] + c[1]) / 3.0,
			(a[2] + b[2] + c[2]) / 3.0,
		];
		if n[0] * centre[0] + n[1] * centre[1] + n[2] * centre[2] < 0.0 {
			t.swap(1, 2);
		}
	}
}

/// A sphere of diameter 1 with smooth normals.
fn unit_sphere() -> Geometry {
	const RINGS: u32 = 16;
	const SEGMENTS: u32 = 32;
	let mut positions = Vec::new();
	let mut normals = Vec::new();
	for r in 0..=RINGS {
		let phi = std::f32::consts::PI * r as f32 / RINGS as f32;
		for s in 0..=SEGMENTS {
			let theta = std::f32::consts::TAU * s as f32 / SEGMENTS as f32;
			let n = [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()];
			positions.push(n.map(|v| v * 0.5));
			normals.push(n);
		}
	}
	let row = SEGMENTS + 1;
	let mut triangles = Vec::new();
	for r in 0..RINGS {
		for s in 0..SEGMENTS {
			let (a, b) = (r * row + s, (r + 1) * row + s);
			if r > 0 {
				triangles.push([a, b, a + 1]);
			}
			if r + 1 < RINGS {
				triangles.push([a + 1, b, b + 1]);
			}
		}
	}
	face_outwards(&positions, &mut triangles);
	Geometry::whole(positions, triangles).with_normals(normals)
}

/// The SpecialMesh head, height and diameter 1 along Y: a surface of
/// revolution whose profile is straight in the middle half and a quarter
/// circle of radius 0.25 at the top and bottom, closed by flat caps.
fn unit_head() -> Geometry {
	use std::f32::consts::{FRAC_PI_2, TAU};
	const SEGMENTS: u32 = 32;
	const ARC: u32 = 6;
	const R: f32 = 0.25;
	// the profile from the bottom cap's edge to the top cap's edge:
	// (radius, height, normal radius, normal height)
	let mut profile = Vec::new();
	for (centre, start) in [(-R, -FRAC_PI_2), (R, 0.0)] {
		for k in 0..=ARC {
			let a = start + FRAC_PI_2 * k as f32 / ARC as f32;
			profile.push((R + R * a.cos(), centre + R * a.sin(), a.cos(), a.sin()));
		}
	}
	let mut positions = Vec::new();
	let mut normals = Vec::new();
	let mut triangles = Vec::new();
	let row = profile.len() as u32;
	for s in 0..=SEGMENTS {
		let (sin, cos) = (TAU * s as f32 / SEGMENTS as f32).sin_cos();
		for &(r, y, nr, ny) in &profile {
			positions.push([r * cos, y, r * sin]);
			normals.push([nr * cos, ny, nr * sin]);
		}
	}
	for s in 0..SEGMENTS {
		for k in 0..row - 1 {
			let (a, b) = (s * row + k, (s + 1) * row + k);
			triangles.push([a, b, a + 1]);
			triangles.push([a + 1, b, b + 1]);
		}
	}
	// flat caps inside the rounded edges
	for y in [-0.5f32, 0.5] {
		let centre = positions.len() as u32;
		positions.push([0.0, y, 0.0]);
		normals.push([0.0, y.signum(), 0.0]);
		for s in 0..=SEGMENTS {
			let (sin, cos) = (TAU * s as f32 / SEGMENTS as f32).sin_cos();
			positions.push([R * cos, y, R * sin]);
			normals.push([0.0, y.signum(), 0.0]);
		}
		for s in 0..SEGMENTS {
			triangles.push([centre, centre + 1 + s, centre + 2 + s]);
		}
	}
	face_outwards(&positions, &mut triangles);
	Geometry::whole(positions, triangles).with_normals(normals)
}

/// A cylinder of length and diameter 1 along X, with smooth sides and flat caps.
fn unit_cylinder() -> Geometry {
	const SEGMENTS: u32 = 32;
	let mut positions = Vec::new();
	let mut normals = Vec::new();
	let mut triangles = Vec::new();
	let ring = |x: f32, s: u32| {
		let a = std::f32::consts::TAU * s as f32 / SEGMENTS as f32;
		([x, 0.5 * a.cos(), 0.5 * a.sin()], [0.0, a.cos(), a.sin()])
	};
	// sides
	for s in 0..=SEGMENTS {
		for x in [-0.5, 0.5] {
			let (p, n) = ring(x, s);
			positions.push(p);
			normals.push(n);
		}
	}
	for s in 0..SEGMENTS {
		let i = s * 2;
		triangles.push([i, i + 1, i + 2]);
		triangles.push([i + 1, i + 3, i + 2]);
	}
	// caps, each a fan around its centre
	for x in [-0.5f32, 0.5] {
		let centre = positions.len() as u32;
		positions.push([x, 0.0, 0.0]);
		normals.push([x.signum(), 0.0, 0.0]);
		for s in 0..=SEGMENTS {
			positions.push(ring(x, s).0);
			normals.push([x.signum(), 0.0, 0.0]);
		}
		for s in 0..SEGMENTS {
			triangles.push([centre, centre + 1 + s, centre + 2 + s]);
		}
	}
	face_outwards(&positions, &mut triangles);
	Geometry::whole(positions, triangles).with_normals(normals)
}

/// A truss of `segments` 2-stud cubes along `axis`, built in studs and then
/// scaled to the unit box (a part scales it back to its size). Beam sizes
/// are from Roblox Studio: posts and rings 0.3 studs, braces 0.2.
fn unit_truss(segments: u16, axis: u8) -> Geometry {
	const POST: f32 = 0.3;
	const BRACE: f32 = 0.2;
	let n = segments.max(1) as f32;
	let length = 2.0 * n;
	let mut positions: Vec<[f32; 3]> = Vec::new();
	let mut triangles: Vec<[u32; 3]> = Vec::new();
	// a box from its centre and three half-axes (studs, truss along Y)
	let mut add_box = |c: [f32; 3], a: [f32; 3], b: [f32; 3], d: [f32; 3]| {
		let base = positions.len() as u32;
		for i in 0..8 {
			let s = |bit: u32| if i & bit == 0 { -1.0 } else { 1.0 };
			positions.push([0, 1, 2].map(|k| c[k] + s(1) * a[k] + s(2) * b[k] + s(4) * d[k]));
		}
		let mut faces: Vec<[u32; 3]> = [
			[0, 2, 6, 4],
			[1, 5, 7, 3],
			[0, 4, 5, 1],
			[2, 3, 7, 6],
			[0, 1, 3, 2],
			[4, 6, 7, 5],
		]
		.iter()
		.flat_map(|f| [[f[0], f[1], f[2]], [f[0], f[2], f[3]]])
		.map(|t| t.map(|i| base + i))
		.collect();
		let local: Vec<[f32; 3]> = positions[base as usize..]
			.iter()
			.map(|p| [p[0] - c[0], p[1] - c[1], p[2] - c[2]])
			.collect();
		let mut shifted = faces.clone();
		for t in &mut shifted {
			*t = t.map(|i| i - base);
		}
		face_outwards(&local, &mut shifted);
		for (t, s) in faces.iter_mut().zip(shifted) {
			*t = s.map(|i| i + base);
		}
		triangles.extend(faces);
	};
	let h = 1.0 - POST / 2.0;
	let half = length / 2.0;
	// corner posts, full length
	for (x, z) in [(-h, -h), (h, -h), (h, h), (-h, h)] {
		add_box(
			[x, 0.0, z],
			[POST / 2.0, 0.0, 0.0],
			[0.0, half, 0.0],
			[0.0, 0.0, POST / 2.0],
		);
	}
	for k in 0..=segments.max(1) {
		let y = (-half + 2.0 * k as f32).clamp(-half + POST / 2.0, half - POST / 2.0);
		// a ring of four bars at each segment boundary
		for (c, a, d) in [
			([0.0, y, -h], [1.0, 0.0, 0.0], [0.0, 0.0, POST / 2.0]),
			([0.0, y, h], [1.0, 0.0, 0.0], [0.0, 0.0, POST / 2.0]),
			([-h, y, 0.0], [POST / 2.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
			([h, y, 0.0], [POST / 2.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
		] {
			add_box(c, a, [0.0, POST / 2.0, 0.0], d);
		}
		if k == segments.max(1) {
			break;
		}
		// an X brace on each side of the segment above
		let mid = -half + 2.0 * k as f32 + 1.0;
		for (along, normal, at) in [
			([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], -h),
			([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], h),
			([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], -h),
			([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], h),
		] {
			for slope in [-1.0f32, 1.0] {
				// from corner to corner of the 2 x 2 face, inside the frame
				let dir = [along[0] * slope, 1.0, along[2] * slope];
				let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
				let u = dir.map(|v| v / len);
				// across the bar, in the face
				let p = [
					u[1] * normal[2] - u[2] * normal[1],
					u[2] * normal[0] - u[0] * normal[2],
					u[0] * normal[1] - u[1] * normal[0],
				];
				let c = [normal[0] * at, mid, normal[2] * at];
				add_box(
					c,
					u.map(|v| v * (h * std::f32::consts::SQRT_2 - POST / 2.0)),
					p.map(|v| v * BRACE / 2.0),
					normal.map(|v| v * BRACE / 2.0),
				);
			}
		}
	}
	// studs to the unit box, then the long side onto `axis`
	let unit = |p: [f32; 3]| {
		let q = [p[0] / 2.0, p[1] / length, p[2] / 2.0];
		match axis {
			0 => [q[1], q[0], q[2]],
			2 => [q[0], q[2], q[1]],
			_ => q,
		}
	};
	let positions = positions.into_iter().map(unit).collect();
	if axis != 1 {
		// swapping two axes mirrors the solid: turn the faces back outwards
		for t in &mut triangles {
			t.swap(1, 2);
		}
	}
	Geometry::whole(positions, triangles)
}

/// Flat-shaded (no normals) solid from corner points and faces.
fn solid(positions: Vec<[f32; 3]>, faces: &[&[u32]]) -> Geometry {
	let mut triangles: Vec<[u32; 3]> = faces
		.iter()
		.flat_map(|f| (1..f.len() - 1).map(|i| [f[0], f[i], f[i + 1]]))
		.collect();
	// the centre of a wedge is not inside all its faces' planes the same way
	// a box's is, so orient against the solid's centroid instead
	let n = positions.len() as f32;
	let c = positions.iter().fold([0.0f32; 3], |acc, p| {
		[acc[0] + p[0] / n, acc[1] + p[1] / n, acc[2] + p[2] / n]
	});
	let shifted: Vec<[f32; 3]> = positions
		.iter()
		.map(|p| [p[0] - c[0], p[1] - c[1], p[2] - c[2]])
		.collect();
	face_outwards(&shifted, &mut triangles);
	Geometry::whole(positions, triangles)
}

fn unit_wedge() -> Geometry {
	let h = 0.5;
	solid(
		vec![
			[-h, -h, -h],
			[h, -h, -h],
			[h, -h, h],
			[-h, -h, h],
			[h, h, h],
			[-h, h, h],
		],
		&[
			&[0, 1, 2, 3], // bottom
			&[2, 4, 5, 3], // back
			&[0, 5, 4, 1], // slope
			&[0, 3, 5],    // left
			&[1, 4, 2],    // right
		],
	)
}

fn unit_corner_wedge() -> Geometry {
	let h = 0.5;
	solid(
		vec![
			[-h, -h, -h],
			[h, -h, -h],
			[h, -h, h],
			[-h, -h, h],
			[h, h, -h],
		],
		&[
			&[0, 1, 2, 3], // bottom
			&[0, 4, 1],    // front, upright
			&[1, 4, 2],    // right, upright
			&[0, 3, 4],    // left slope
			&[3, 2, 4],    // back slope
		],
	)
}

/// A unit cube: the Block primitive, and the BLOCK physics shape.
pub(super) fn unit_cube() -> Geometry {
	let positions = (0..8)
		.map(|i| {
			[
				if i & 1 == 0 { -0.5 } else { 0.5 },
				if i & 2 == 0 { -0.5 } else { 0.5 },
				if i & 4 == 0 { -0.5 } else { 0.5 },
			]
		})
		.collect();
	let quads = [
		[0, 2, 3, 1],
		[4, 5, 7, 6],
		[0, 1, 5, 4],
		[2, 6, 7, 3],
		[0, 4, 6, 2],
		[1, 3, 7, 5],
	];
	let triangles = quads
		.iter()
		.flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]])
		.collect();
	Geometry::whole(positions, triangles)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A truss fills its unit box with beams: every box faces outwards (its
	/// signed volume is positive) and lengthening it adds segments.
	#[test]
	fn trusses_are_framed_segments() {
		for axis in 0..3u8 {
			let g = Primitive::Truss { segments: 3, axis }.geometry();
			let (lo, hi) = g.bounds().unwrap();
			for k in 0..3 {
				assert!(
					(lo[k] + 0.5).abs() < 1e-5 && (hi[k] - 0.5).abs() < 1e-5,
					"axis {axis}"
				);
			}
			let volume: f32 = g
				.triangles
				.iter()
				.map(|t| {
					let [a, b, c] = t.map(|i| g.positions[i as usize]);
					(a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
						+ a[2] * (b[0] * c[1] - b[1] * c[0]))
						/ 6.0
				})
				.sum();
			// open beams, far from a solid box
			assert!(volume > 0.05 && volume < 0.5, "axis {axis}: {volume}");
		}
		let one = Primitive::Truss {
			segments: 1,
			axis: 1,
		}
		.geometry();
		let three = Primitive::Truss {
			segments: 3,
			axis: 1,
		}
		.geometry();
		assert!(three.triangles.len() > 2 * one.triangles.len());
	}

	/// Signed volume by the divergence theorem: right only when the solid is
	/// closed and every triangle faces outwards.
	#[test]
	fn primitives_are_closed_outward_unit_solids() {
		use std::f32::consts::PI;
		for (shape, volume) in [
			(Primitive::Block, 1.0),
			(Primitive::Ball, PI / 6.0),
			(Primitive::CylinderX, PI / 4.0),
			// measured profile: see unit_head
			(Primitive::Head, 0.7105),
			(Primitive::Wedge, 0.5),
			(Primitive::CornerWedge, 1.0 / 3.0),
		] {
			let g = shape.geometry();
			let v: f32 = g
				.triangles
				.iter()
				.map(|t| {
					let [a, b, c] = t.map(|i| g.positions[i as usize]);
					(a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
						+ a[2] * (b[0] * c[1] - b[1] * c[0]))
						/ 6.0
				})
				.sum();
			// polygonal spheres and cylinders fall a little short
			assert!(
				(v - volume).abs() < volume * 0.03,
				"{shape:?}: volume {v}, want {volume}"
			);
			let (lo, hi) = g.bounds().unwrap();
			assert!(
				lo.iter().chain(&hi).all(|c| c.abs() <= 0.5 + 1e-6),
				"{shape:?} fits the unit box"
			);
		}
		// a cylinder part runs along X
		let (lo, hi) = Primitive::CylinderX.geometry().bounds().unwrap();
		assert!((hi[0] - lo[0] - 1.0).abs() < 1e-6 && (hi[1] - lo[1] - 1.0).abs() < 1e-3);
		// a corner wedge peaks over its front right corner, as measured in Studio
		let peak = Primitive::CornerWedge
			.geometry()
			.positions
			.iter()
			.copied()
			.max_by(|a, b| a[1].total_cmp(&b[1]))
			.unwrap();
		assert_eq!(peak, [0.5, 0.5, -0.5]);
	}
}
