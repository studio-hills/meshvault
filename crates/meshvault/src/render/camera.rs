//! The orbit camera, and projecting world points into the view.

use eframe::egui::{Pos2, Rect};

use crate::render::maths::{V3, cross, dot, normalize, sub};
use crate::scene::geometry::Geometry;

/// Half the vertical field of view, in degrees (70° top to bottom).
pub(super) const HALF_FOV_DEGREES: f32 = 35.0;
/// How far from its target a framed camera starts, in bounding radii.
const FRAMING_DISTANCE: f32 = 1.9;
/// The near plane, in bounding radii: points, the grid and the GPU's
/// depth range start here.
const NEAR_FACTOR: f32 = 0.01;

#[derive(Clone)]
pub struct Camera {
	pub yaw: f32,
	pub pitch: f32,
	pub distance: f32,
	pub(super) target: V3,
	/// Radius of the mesh's bounding sphere; scales pan and zoom speed.
	pub(super) radius: f32,
	/// Lowest point of the model, where the ground grid sits.
	pub(super) floor: f32,
}

impl Camera {
	pub fn framing(geometry: &Geometry) -> Self {
		Self::framing_bounds(geometry.bounds())
	}

	/// Frames an axis-aligned box `(min, max)`.
	pub fn framing_bounds(bounds: Option<(V3, V3)>) -> Self {
		let floor = bounds.map_or(0.0, |(lo, _)| lo[1]);
		let (target, radius) = match bounds {
			Some((lo, hi)) => {
				let centre = [
					(lo[0] + hi[0]) * 0.5,
					(lo[1] + hi[1]) * 0.5,
					(lo[2] + hi[2]) * 0.5,
				];
				let r = dot(sub(hi, centre), sub(hi, centre)).sqrt();
				(centre, if r > 1e-6 { r } else { 1.0 })
			}
			None => ([0.0; 3], 1.0),
		};
		Self {
			// Roblox models face -Z, so start from a front three-quarter view
			yaw: std::f32::consts::PI - 0.6,
			pitch: 0.4,
			distance: radius * FRAMING_DISTANCE,
			target,
			radius,
			floor,
		}
	}

	/// Orthonormal camera basis: right, up, forward (pointing into the scene).
	pub(super) fn basis(&self) -> (V3, V3, V3) {
		let (sy, cy) = self.yaw.sin_cos();
		let (sp, cp) = self.pitch.sin_cos();
		let forward = [-sy * cp, -sp, -cy * cp];
		let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
		let up = cross(right, forward);
		(right, up, forward)
	}

	pub(super) fn eye(&self) -> V3 {
		let (_, _, f) = self.basis();
		[
			self.target[0] - f[0] * self.distance,
			self.target[1] - f[1] * self.distance,
			self.target[2] - f[2] * self.distance,
		]
	}

	/// The distance to the near plane.
	pub(super) fn near(&self) -> f32 {
		self.radius * NEAR_FACTOR
	}
}

/// A camera looking into a view `rect`, worked out once per draw so each
/// point costs a few products rather than the camera's trigonometry.
pub(super) struct View {
	right: V3,
	up: V3,
	forward: V3,
	eye: V3,
	/// Screen points per unit of view-space height at unit depth.
	focal: f32,
	centre: Pos2,
}

impl View {
	pub(super) fn new(camera: &Camera, rect: Rect) -> Self {
		let (right, up, forward) = camera.basis();
		Self {
			right,
			up,
			forward,
			eye: camera.eye(),
			focal: rect.height() * 0.5 / HALF_FOV_DEGREES.to_radians().tan(),
			centre: rect.center(),
		}
	}

	/// The world point `p` in view space: along right, up and forward from
	/// the eye. In front of the camera z is positive.
	pub(super) fn to_view(&self, p: V3) -> V3 {
		let d = sub(p, self.eye);
		[dot(d, self.right), dot(d, self.up), dot(d, self.forward)]
	}

	/// Where the view-space point `v` lands on screen. It does not check
	/// that `v` is in front of the camera; each caller rejects or clips
	/// points nearer than its own limit first.
	pub(super) fn project(&self, v: V3) -> Pos2 {
		Pos2::new(
			self.centre.x + v[0] / v[2] * self.focal,
			self.centre.y - v[1] / v[2] * self.focal,
		)
	}
}

/// Where the world point `p` lands in the view `rect`, if it is in front
/// of the camera.
pub fn project(camera: &Camera, rect: Rect, p: V3) -> Option<Pos2> {
	let view = View::new(camera, rect);
	let v = view.to_view(p);
	if v[2] <= camera.near() {
		return None;
	}
	Some(view.project(v))
}

#[cfg(test)]
mod tests {
	use eframe::egui::Vec2;

	use super::*;
	use crate::render::maths::close;

	#[test]
	fn framing_nothing_gives_a_unit_radius() {
		let camera = Camera::framing_bounds(None);
		assert_eq!(camera.target, [0.0; 3]);
		assert_eq!(camera.radius, 1.0);
		assert_eq!(camera.floor, 0.0);
		assert!(close(camera.distance, 1.9));
	}

	#[test]
	fn framing_a_point_does_not_give_a_zero_radius() {
		let camera = Camera::framing_bounds(Some(([1.0, 2.0, 3.0], [1.0, 2.0, 3.0])));
		assert_eq!(camera.target, [1.0, 2.0, 3.0]);
		assert_eq!(camera.radius, 1.0);
		assert_eq!(camera.floor, 2.0);
	}

	#[test]
	fn framing_a_box_centres_on_it() {
		let camera = Camera::framing_bounds(Some(([-1.0, -2.0, 0.0], [1.0, 2.0, 4.0])));
		assert_eq!(camera.target, [0.0, 0.0, 2.0]);
		assert!(close(camera.radius, 3.0));
		assert!(close(camera.distance, 3.0 * 1.9));
		assert_eq!(camera.floor, -2.0);
		assert!(close(camera.yaw, std::f32::consts::PI - 0.6));
		assert_eq!(camera.pitch, 0.4);
	}

	#[test]
	fn project_follows_the_camera_basis() {
		let camera = Camera::framing_bounds(Some(([-1.0; 3], [1.0; 3])));
		// a rect away from the origin, so its centre is not its half size
		let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(300.0, 200.0));
		let (right, up, _) = camera.basis();
		let focal = 100.0 / HALF_FOV_DEGREES.to_radians().tan();
		let shift = focal / camera.distance;
		let centre = project(&camera, rect, camera.target).unwrap();
		assert!((centre.x - 160.0).abs() < 1e-3 && (centre.y - 120.0).abs() < 1e-3);
		let to_right = project(&camera, rect, add(camera.target, right)).unwrap();
		assert!((to_right.x - (160.0 + shift)).abs() < 1e-3);
		assert!((to_right.y - 120.0).abs() < 1e-3);
		// screen y grows downwards
		let above = project(&camera, rect, add(camera.target, up)).unwrap();
		assert!((above.x - 160.0).abs() < 1e-3);
		assert!((above.y - (120.0 - shift)).abs() < 1e-3);
	}

	#[test]
	fn project_rejects_points_behind_or_at_the_eye() {
		let camera = Camera::framing_bounds(Some(([-1.0; 3], [1.0; 3])));
		let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 200.0));
		let eye = camera.eye();
		let (_, _, forward) = camera.basis();
		assert_eq!(project(&camera, rect, eye), None);
		assert_eq!(project(&camera, rect, sub(eye, forward)), None);
		// the near limit is a hundredth of the radius
		let near = camera.radius * 0.01;
		let just_inside = add(eye, forward.map(|f| f * near * 0.5));
		assert_eq!(project(&camera, rect, just_inside), None);
		let just_beyond = add(eye, forward.map(|f| f * near * 2.0));
		assert!(project(&camera, rect, just_beyond).is_some());
		// exactly on the near plane is rejected, the next f32 beyond it is not:
		// looking down -Z from the origin, a radius of 100 puts it at depth 1
		let camera = Camera {
			yaw: 0.0,
			pitch: 0.0,
			distance: 0.0,
			target: [0.0; 3],
			radius: 100.0,
			floor: 0.0,
		};
		assert_eq!(camera.near(), 1.0);
		assert_eq!(project(&camera, rect, [0.0, 0.0, -1.0]), None);
		let beyond = project(&camera, rect, [0.0, 0.0, -(1.0 + f32::EPSILON)]);
		assert!(beyond.is_some());
	}

	fn add(a: V3, b: V3) -> V3 {
		[a[0] + b[0], a[1] + b[1], a[2] + b[2]]
	}
}
