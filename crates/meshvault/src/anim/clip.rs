//! Sampling animation clips into per-target local transforms.
//!
//! A clip becomes a list of tracks, each driving one named target (a
//! Motor6D's `Part1` or a Bone) with a local transform over time.

use super::math::CFrame;
use super::schema::{
	AnimationClip, Channels, ClipStatus, CurveData, EasingDirection, EasingStyle, Pose,
	RotationKey, ScalarKey,
};

/// One face control (JawDrop, ..) over time: NumberPoses of a keyframe
/// clip, or a FloatCurve of a curve clip.
#[derive(Clone, Debug)]
pub struct FaceTrack {
	pub control: String,
	keys: FaceKeys,
}

#[derive(Clone, Debug)]
enum FaceKeys {
	Poses(Vec<(f32, f32, EasingStyle, EasingDirection)>),
	Curve(Vec<ScalarKey>),
}

impl FaceTrack {
	fn sample(&self, time: f32) -> f32 {
		match &self.keys {
			FaceKeys::Curve(keys) => sample_scalar(keys, time).unwrap_or(0.0),
			FaceKeys::Poses(keys) => match segment(keys, time, |k| k.0) {
				Err(i) => keys[i].1,
				Ok(i) => {
					let (a, b) = (&keys[i], &keys[i + 1]);
					let span = b.0 - a.0;
					let t = if span > 0.0 { (time - a.0) / span } else { 1.0 };
					a.1 + (b.1 - a.1) * pose_ease(a.2, a.3, t)
				}
			},
		}
	}
}

/// What a track animates: a pose name plus its parent pose's name, which
/// tells apart same-named targets in different places of the rig.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TargetKey {
	pub name: String,
	pub parent: Option<String>,
}

#[derive(Clone, Debug)]
struct PoseKey {
	time: f32,
	transform: CFrame,
	style: EasingStyle,
	direction: EasingDirection,
}

#[derive(Clone, Debug)]
enum Rotation {
	None,
	Euler {
		order: String,
		channels: [Vec<ScalarKey>; 3],
	},
	Keys(Vec<RotationKey>),
}

#[derive(Clone, Debug)]
enum Sampler {
	Poses(Vec<PoseKey>),
	Curves {
		position: Option<[Vec<ScalarKey>; 3]>,
		rotation: Rotation,
	},
}

#[derive(Clone, Debug)]
pub struct Track {
	pub target: TargetKey,
	sampler: Sampler,
}

#[derive(Clone, Debug)]
pub struct ClipMarker {
	pub time: f32,
	pub name: String,
	pub value: String,
}

#[derive(Clone, Debug)]
pub struct Clip {
	pub duration: f32,
	pub looped: bool,
	pub tracks: Vec<Track>,
	pub markers: Vec<ClipMarker>,
	/// Curves the player cannot apply (e.g. FloatCurves, unknown types).
	pub skipped_curves: usize,
	/// Face controls the clip animates, for FACS heads.
	pub face: Vec<FaceTrack>,
}

impl Clip {
	/// Builds a playable clip, or explains why the record has no motion.
	pub fn from_record(record: &AnimationClip) -> Result<Clip, String> {
		let mut clip = Clip {
			duration: record.duration,
			looped: record.looped,
			tracks: Vec::new(),
			markers: Vec::new(),
			skipped_curves: 0,
			face: Vec::new(),
		};
		match record.status {
			ClipStatus::KeyframeSequence => clip.add_keyframes(record),
			ClipStatus::CurveAnimation => clip.add_curves(record),
			other => {
				return Err(match other {
					ClipStatus::Unavailable => {
						"The clip could not be loaded when it was exported.".into()
					}
					ClipStatus::CurveAnimationExportFailed => {
						"The curve animation failed to export.".into()
					}
					_ => "This clip type is not supported.".into(),
				});
			}
		}
		if clip.tracks.is_empty() && clip.face.is_empty() {
			return Err("The clip has no motion data.".into());
		}
		// the exported duration is the last key time; recompute if missing
		if clip.duration <= 0.0 {
			clip.duration = clip.tracks.iter().map(Track::end_time).fold(0.0, f32::max);
		}
		Ok(clip)
	}

	fn add_keyframes(&mut self, record: &AnimationClip) {
		let mut tracks: Vec<(TargetKey, Vec<PoseKey>)> = Vec::new();
		let mut frames: Vec<_> = record.keyframes.iter().collect();
		frames.sort_by(|a, b| a.time.total_cmp(&b.time));
		for frame in frames {
			for marker in &frame.markers {
				self.markers.push(ClipMarker {
					time: frame.time,
					name: marker.name.clone(),
					value: marker.value.clone(),
				});
			}
			walk_poses(&frame.poses, None, frame.time, &mut tracks);
			for face in frame.poses.iter().flat_map(face_controls_of) {
				let key = (
					frame.time,
					face.value,
					face.easing_style,
					face.easing_direction,
				);
				match self.face.iter_mut().find(|t| t.control == face.name) {
					Some(FaceTrack {
						keys: FaceKeys::Poses(keys),
						..
					}) => keys.push(key),
					_ => self.face.push(FaceTrack {
						control: face.name.clone(),
						keys: FaceKeys::Poses(vec![key]),
					}),
				}
			}
		}
		self.tracks = tracks
			.into_iter()
			.map(|(target, keys)| Track {
				target,
				sampler: Sampler::Poses(keys),
			})
			.collect();
	}

	fn add_curves(&mut self, record: &AnimationClip) {
		// target, its X/Y/Z position channels, and its rotation
		type Gathered = (TargetKey, Option<[Vec<ScalarKey>; 3]>, Rotation);
		let mut by_target: Vec<Gathered> = Vec::new();
		for track in &record.curves {
			let name = track
				.path
				.rsplit('/')
				.next()
				.unwrap_or_default()
				.to_string();
			let target = TargetKey {
				name: if name.is_empty() {
					track.name.clone()
				} else {
					name
				},
				// the folder above the target names its parent pose
				parent: track.path.rsplit('/').nth(1).map(String::from),
			};
			let index = match by_target.iter().position(|(t, ..)| *t == target) {
				Some(i) => i,
				None => {
					by_target.push((target, None, Rotation::None));
					by_target.len() - 1
				}
			};
			let entry = &mut by_target[index];
			match &track.curve {
				CurveData::Vector3Curve { channels } => entry.1 = Some(channels_of(channels)),
				CurveData::EulerRotationCurve {
					rotation_order,
					channels,
				} => {
					entry.2 = Rotation::Euler {
						order: rotation_order.clone(),
						channels: channels_of(channels),
					}
				}
				CurveData::RotationCurve { keys } => {
					let mut keys = keys.clone();
					keys.sort_by(|a, b| a.time.total_cmp(&b.time));
					entry.2 = Rotation::Keys(keys);
				}
				// a face control: its folder is FaceControls
				CurveData::FloatCurve { keys }
					if track.path.rsplit('/').next() == Some("FaceControls") =>
				{
					let mut keys = keys.clone();
					keys.sort_by(|a, b| a.time.total_cmp(&b.time));
					self.face.push(FaceTrack {
						control: track.name.clone(),
						keys: FaceKeys::Curve(keys),
					});
				}
				CurveData::FloatCurve { .. } | CurveData::Unknown => self.skipped_curves += 1,
			}
		}
		self.tracks = by_target
			.into_iter()
			.filter(|(_, position, rotation)| {
				position.is_some() || !matches!(rotation, Rotation::None)
			})
			.map(|(target, position, rotation)| Track {
				target,
				sampler: Sampler::Curves { position, rotation },
			})
			.collect();
	}

	/// Every animated face control's value at `time` seconds.
	pub fn face_weights(&self, time: f32) -> Vec<(&str, f32)> {
		self.face
			.iter()
			.map(|t| (t.control.as_str(), t.sample(time)))
			.collect()
	}

	/// Local transforms of every track at `time` seconds.
	pub fn sample(&self, time: f32) -> impl Iterator<Item = (&TargetKey, CFrame)> + '_ {
		self.tracks.iter().map(move |t| (&t.target, t.sample(time)))
	}
}

fn channels_of(channels: &Channels) -> [Vec<ScalarKey>; 3] {
	let sorted = |keys: &Option<Vec<ScalarKey>>| {
		let mut keys = keys.clone().unwrap_or_default();
		keys.sort_by(|a, b| a.time.total_cmp(&b.time));
		keys
	};
	[
		sorted(&channels.x),
		sorted(&channels.y),
		sorted(&channels.z),
	]
}

/// Every face control pose in a pose tree.
fn face_controls_of(pose: &Pose) -> Vec<&super::schema::FaceControlPose> {
	let mut found: Vec<&super::schema::FaceControlPose> = pose.face_controls.iter().collect();
	for child in &pose.children {
		found.extend(face_controls_of(child));
	}
	found
}

fn walk_poses(
	poses: &[Pose],
	parent: Option<&str>,
	time: f32,
	tracks: &mut Vec<(TargetKey, Vec<PoseKey>)>,
) {
	for pose in poses {
		// weight 0 marks placeholder poses that only group their children;
		// treating them as keys would snap the target back to rest
		if pose.weight > 0.0 {
			let target = TargetKey {
				name: pose.name.clone(),
				parent: parent.map(String::from),
			};
			let key = PoseKey {
				time,
				transform: CFrame::from_components(&pose.transform),
				style: pose.easing_style,
				direction: pose.easing_direction,
			};
			match tracks.iter_mut().find(|(t, _)| *t == target) {
				Some((_, keys)) => keys.push(key),
				None => tracks.push((target, vec![key])),
			}
		}
		walk_poses(&pose.children, Some(&pose.name), time, tracks);
	}
}

impl Track {
	fn end_time(&self) -> f32 {
		match &self.sampler {
			Sampler::Poses(keys) => keys.last().map_or(0.0, |k| k.time),
			Sampler::Curves { position, rotation } => {
				let mut end = 0.0f32;
				for channel in position.iter().flatten() {
					end = end.max(channel.last().map_or(0.0, |k| k.time));
				}
				match rotation {
					Rotation::Euler { channels, .. } => {
						for channel in channels {
							end = end.max(channel.last().map_or(0.0, |k| k.time));
						}
					}
					Rotation::Keys(keys) => end = end.max(keys.last().map_or(0.0, |k| k.time)),
					Rotation::None => {}
				}
				end
			}
		}
	}

	pub fn sample(&self, time: f32) -> CFrame {
		match &self.sampler {
			Sampler::Poses(keys) => sample_poses(keys, time),
			Sampler::Curves { position, rotation } => {
				let pos = match position {
					Some(channels) => [
						sample_scalar(&channels[0], time).unwrap_or(0.0),
						sample_scalar(&channels[1], time).unwrap_or(0.0),
						sample_scalar(&channels[2], time).unwrap_or(0.0),
					],
					None => [0.0; 3],
				};
				let rot = match rotation {
					Rotation::None => CFrame::IDENTITY,
					Rotation::Euler { order, channels } => CFrame::from_euler(
						[
							sample_scalar(&channels[0], time).unwrap_or(0.0),
							sample_scalar(&channels[1], time).unwrap_or(0.0),
							sample_scalar(&channels[2], time).unwrap_or(0.0),
						],
						order,
					),
					Rotation::Keys(keys) => sample_rotation(keys, time),
				};
				CFrame::from_position(pos).mul(&CFrame {
					pos: [0.0; 3],
					..rot
				})
			}
		}
	}
}

/// Index of the key segment containing `time`: `Err(i)` when `time` is
/// before the first (`0`) or after the last key (`len - 1`). `keys` must
/// not be empty and must be sorted by time.
fn segment<T>(keys: &[T], time: f32, key_time: impl Fn(&T) -> f32) -> Result<usize, usize> {
	if time <= key_time(&keys[0]) {
		return Err(0);
	}
	if time >= key_time(&keys[keys.len() - 1]) {
		return Err(keys.len() - 1);
	}
	// keys are sorted; find the last key at or before `time`
	let i = keys.partition_point(|k| key_time(k) <= time);
	Ok(i - 1)
}

fn sample_poses(keys: &[PoseKey], time: f32) -> CFrame {
	match segment(keys, time, |k| k.time) {
		Err(i) => keys[i].transform,
		Ok(i) => {
			let (a, b) = (&keys[i], &keys[i + 1]);
			let span = b.time - a.time;
			let t = if span > 0.0 {
				(time - a.time) / span
			} else {
				1.0
			};
			// a pose's easing shapes the transition from it to the next pose
			let eased = pose_ease(a.style, a.direction, t);
			a.transform.lerp(&b.transform, eased)
		}
	}
}

/// Roblox's `PoseEasingStyle`, as measured in Studio. The legacy Cubic style
/// has its In/Out directions swapped relative to TweenService (CubicV2 was
/// added with the conventional meaning); Elastic and Bounce are not swapped.
/// Constant jumps: "In" to the next pose at once, "Out" only when it is
/// reached, "InOut" halfway.
fn pose_ease(style: EasingStyle, direction: EasingDirection, t: f32) -> f32 {
	let t = t.clamp(0.0, 1.0);
	match style {
		EasingStyle::Linear | EasingStyle::Other => t,
		EasingStyle::Constant => {
			let jumped = match direction {
				EasingDirection::In => t > 0.0,
				EasingDirection::InOut => t >= 0.5,
				_ => false,
			};
			if jumped { 1.0 } else { 0.0 }
		}
		EasingStyle::Cubic => {
			let flipped = match direction {
				EasingDirection::In => EasingDirection::Out,
				EasingDirection::Out => EasingDirection::In,
				other => other,
			};
			ease(EasingStyle::CubicV2, flipped, t)
		}
		_ => ease(style, direction, t),
	}
}

/// Easing curve for `style`, shaped by `direction`; Roblox-specific cases
/// are noted below.
fn ease(style: EasingStyle, direction: EasingDirection, t: f32) -> f32 {
	let f = |x: f32| -> f32 {
		use std::f32::consts::PI;
		match style {
			EasingStyle::Cubic | EasingStyle::CubicV2 => x * x * x,
			EasingStyle::Quad => x * x,
			EasingStyle::Quart => x.powi(4),
			EasingStyle::Quint => x.powi(5),
			EasingStyle::Sine => 1.0 - (x * PI / 2.0).cos(),
			EasingStyle::Exponential => {
				if x == 0.0 {
					0.0
				} else {
					2f32.powf(10.0 * x - 10.0)
				}
			}
			EasingStyle::Circular => 1.0 - (1.0 - x * x).max(0.0).sqrt(),
			EasingStyle::Back => {
				let c = 1.70158;
				(c + 1.0) * x * x * x - c * x * x
			}
			EasingStyle::Elastic => {
				if x == 0.0 || x == 1.0 {
					x
				} else {
					-(2f32.powf(10.0 * x - 10.0)) * ((x * 10.0 - 10.75) * (2.0 * PI / 3.0)).sin()
				}
			}
			EasingStyle::Bounce => 1.0 - bounce_out(1.0 - x),
			EasingStyle::Linear | EasingStyle::Constant | EasingStyle::Other => x,
		}
	};
	match direction {
		EasingDirection::In | EasingDirection::Other => f(t),
		EasingDirection::Out => 1.0 - f(1.0 - t),
		// Elastic's InOut is not made of its In curve: it has a longer period
		EasingDirection::InOut if style == EasingStyle::Elastic => {
			use std::f32::consts::PI;
			if t == 0.0 || t == 1.0 {
				return t;
			}
			let wave = ((20.0 * t - 11.125) * (2.0 * PI / 4.5)).sin();
			if t < 0.5 {
				-(2f32.powf(20.0 * t - 10.0) * wave) / 2.0
			} else {
				2f32.powf(-20.0 * t + 10.0) * wave / 2.0 + 1.0
			}
		}
		// Roblox's Bounce InOut plays the In curve twice, the second time from halfway
		EasingDirection::InOut if style == EasingStyle::Bounce => {
			if t < 0.5 {
				f(2.0 * t) / 2.0
			} else {
				0.5 + f(2.0 * t - 1.0) / 2.0
			}
		}
		EasingDirection::InOut => {
			if t < 0.5 {
				f(2.0 * t) / 2.0
			} else {
				1.0 - f(2.0 * (1.0 - t)) / 2.0
			}
		}
	}
}

fn bounce_out(x: f32) -> f32 {
	let (n, d) = (7.5625, 2.75);
	if x < 1.0 / d {
		n * x * x
	} else if x < 2.0 / d {
		let x = x - 1.5 / d;
		n * x * x + 0.75
	} else if x < 2.5 / d {
		let x = x - 2.25 / d;
		n * x * x + 0.9375
	} else {
		let x = x - 2.625 / d;
		n * x * x + 0.984375
	}
}

/// Samples a float curve. Each key's interpolation shapes the segment that
/// starts at it; cubic segments use the keys' tangents (value per second).
fn sample_scalar(keys: &[ScalarKey], time: f32) -> Option<f32> {
	if keys.is_empty() {
		return None;
	}
	Some(match segment(keys, time, |k| k.time) {
		Err(i) => keys[i].value,
		Ok(i) => {
			let (a, b) = (&keys[i], &keys[i + 1]);
			let span = b.time - a.time;
			let s = if span > 0.0 {
				(time - a.time) / span
			} else {
				1.0
			};
			match a.interpolation.as_str() {
				"Constant" => a.value,
				"Cubic" => {
					// cubic Hermite with tangents scaled to the segment length
					let (s2, s3) = (s * s, s * s * s);
					let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
					let h10 = s3 - 2.0 * s2 + s;
					let h01 = -2.0 * s3 + 3.0 * s2;
					let h11 = s3 - s2;
					h00 * a.value
						+ h10 * span * a.right_tangent
						+ h01 * b.value + h11 * span * b.left_tangent
				}
				_ => a.value + (b.value - a.value) * s,
			}
		}
	})
}

fn sample_rotation(keys: &[RotationKey], time: f32) -> CFrame {
	if keys.is_empty() {
		return CFrame::IDENTITY;
	}
	match segment(keys, time, |k| k.time) {
		Err(i) => CFrame::from_components(&keys[i].value),
		Ok(i) => {
			let (a, b) = (&keys[i], &keys[i + 1]);
			let (ca, cb) = (
				CFrame::from_components(&a.value),
				CFrame::from_components(&b.value),
			);
			if a.interpolation == "Constant" {
				return ca;
			}
			let span = b.time - a.time;
			let s = if span > 0.0 {
				(time - a.time) / span
			} else {
				1.0
			};
			// cubic rotation keys are approximated by a spherical lerp
			ca.lerp(&cb, s)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::anim::schema::{CurveTrack, Keyframe, Marker};

	fn key(time: f32, value: f32, interpolation: &str) -> ScalarKey {
		ScalarKey {
			time,
			value,
			interpolation: interpolation.into(),
			left_tangent: 0.0,
			right_tangent: 0.0,
		}
	}

	fn pose(name: &str, y: f32, weight: f32, children: Vec<Pose>) -> Pose {
		Pose {
			name: name.into(),
			transform: [0.0, y, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
			weight,
			easing_style: EasingStyle::Linear,
			easing_direction: EasingDirection::In,
			children,
			face_controls: vec![],
		}
	}

	#[test]
	fn scalar_interpolation_modes() {
		let keys = vec![
			key(0.0, 0.0, "Linear"),
			key(1.0, 10.0, "Constant"),
			key(2.0, 20.0, "Linear"),
		];
		assert_eq!(sample_scalar(&keys, -1.0), Some(0.0));
		assert_eq!(sample_scalar(&keys, 0.25), Some(2.5));
		assert_eq!(sample_scalar(&keys, 1.5), Some(10.0), "constant holds");
		assert_eq!(sample_scalar(&keys, 9.0), Some(20.0));
		assert_eq!(sample_scalar(&[], 1.0), None);
		// cubic with zero tangents eases in and out; endpoints are exact
		let cubic = vec![key(0.0, 0.0, "Cubic"), key(1.0, 1.0, "Linear")];
		assert!((sample_scalar(&cubic, 0.5).unwrap() - 0.5).abs() < 1e-6);
		assert!(sample_scalar(&cubic, 0.25).unwrap() < 0.25);
	}

	#[test]
	fn easing_shapes() {
		assert_eq!(
			pose_ease(EasingStyle::Linear, EasingDirection::In, 0.3),
			0.3
		);
		// Constant "Out" holds the pose until the next one is reached
		assert_eq!(
			pose_ease(EasingStyle::Constant, EasingDirection::Out, 0.9),
			0.0
		);
		for style in [
			EasingStyle::CubicV2,
			EasingStyle::Sine,
			EasingStyle::Bounce,
			EasingStyle::Elastic,
			EasingStyle::Back,
		] {
			for dir in [
				EasingDirection::In,
				EasingDirection::Out,
				EasingDirection::InOut,
			] {
				assert!(ease(style, dir, 0.0).abs() < 1e-5, "{style:?} {dir:?} at 0");
				assert!(
					(ease(style, dir, 1.0) - 1.0).abs() < 1e-5,
					"{style:?} {dir:?} at 1"
				);
			}
		}
		// legacy Cubic "In" behaves like CubicV2 "Out"
		assert_eq!(
			pose_ease(EasingStyle::Cubic, EasingDirection::In, 0.3),
			ease(EasingStyle::CubicV2, EasingDirection::Out, 0.3)
		);
	}

	/// Roblox's pose easing, measured in Studio: a joint moved by a two-key
	/// clip with each style and direction, at t = 0.05, 0.10 … 0.95.
	#[test]
	fn pose_easing_matches_studio() {
		#[rustfmt::skip]
		const STUDIO: &[(&str, &str, [f32; 19])] = &[
			("Linear", "In", [0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95]),
			("Linear", "Out", [0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95]),
			("Linear", "InOut", [0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95]),
			("Constant", "In", [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
			("Constant", "Out", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
			("Constant", "InOut", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
			("Cubic", "In", [0.1426, 0.271, 0.3859, 0.488, 0.5781, 0.657, 0.7254, 0.784, 0.8336, 0.875, 0.9089, 0.936, 0.9571, 0.973, 0.9844, 0.992, 0.9966, 0.999, 0.9999]),
			("Cubic", "Out", [0.0001, 0.001, 0.0034, 0.008, 0.0156, 0.027, 0.0429, 0.064, 0.0911, 0.125, 0.1664, 0.216, 0.2746, 0.343, 0.4219, 0.512, 0.6141, 0.729, 0.8574]),
			("Cubic", "InOut", [0.0005, 0.004, 0.0135, 0.032, 0.0625, 0.108, 0.1715, 0.256, 0.3645, 0.5, 0.6355, 0.744, 0.8285, 0.892, 0.9375, 0.968, 0.9865, 0.996, 0.9995]),
			("CubicV2", "In", [0.0001, 0.001, 0.0034, 0.008, 0.0156, 0.027, 0.0429, 0.064, 0.0911, 0.125, 0.1664, 0.216, 0.2746, 0.343, 0.4219, 0.512, 0.6141, 0.729, 0.8574]),
			("CubicV2", "Out", [0.1426, 0.271, 0.3859, 0.488, 0.5781, 0.657, 0.7254, 0.784, 0.8336, 0.875, 0.9089, 0.936, 0.9571, 0.973, 0.9844, 0.992, 0.9966, 0.999, 0.9999]),
			("CubicV2", "InOut", [0.0005, 0.004, 0.0135, 0.032, 0.0625, 0.108, 0.1715, 0.256, 0.3645, 0.5, 0.6355, 0.744, 0.8285, 0.892, 0.9375, 0.968, 0.9865, 0.996, 0.9995]),
			("Elastic", "In", [0.0007, 0.002, 0.0014, -0.002, -0.0055, -0.0039, 0.0055, 0.0156, 0.011, -0.0156, -0.0442, -0.0313, 0.0442, 0.125, 0.0884, -0.125, -0.3536, -0.25, 0.3536]),
			("Elastic", "Out", [0.6464, 1.25, 1.3536, 1.125, 0.9116, 0.875, 0.9558, 1.0313, 1.0442, 1.0156, 0.989, 0.9844, 0.9945, 1.0039, 1.0055, 1.002, 0.9986, 0.998, 0.9993]),
			("Elastic", "InOut", [0.001, 0.0003, -0.0037, -0.0039, 0.012, 0.0239, -0.0313, -0.1175, 0.0434, 0.5, 0.9566, 1.1175, 1.0313, 0.9761, 0.988, 1.0039, 1.0037, 0.9997, 0.999]),
			("Bounce", "In", [0.0155, 0.0119, 0.0548, 0.06, 0.0273, 0.0694, 0.1673, 0.2275, 0.2498, 0.2344, 0.1811, 0.09, 0.0736, 0.3194, 0.5273, 0.6975, 0.8298, 0.9244, 0.9811]),
			("Bounce", "Out", [0.0189, 0.0756, 0.1702, 0.3025, 0.4727, 0.6806, 0.9264, 0.91, 0.8189, 0.7656, 0.7502, 0.7725, 0.8327, 0.9306, 0.9727, 0.94, 0.9452, 0.9881, 0.9845]),
			("Bounce", "InOut", [0.0059, 0.03, 0.0347, 0.1137, 0.1172, 0.045, 0.1597, 0.3487, 0.4622, 0.5, 0.5059, 0.53, 0.5347, 0.6137, 0.6172, 0.545, 0.6597, 0.8487, 0.9622]),
		];
		fn parse<T: serde::de::DeserializeOwned>(name: &str) -> T {
			serde_json::from_value(serde_json::Value::String(name.into())).unwrap()
		}
		for (style, direction, expected) in STUDIO {
			let (style, direction): (EasingStyle, EasingDirection) =
				(parse(style), parse(direction));
			for (i, want) in expected.iter().enumerate() {
				let t = (i + 1) as f32 * 0.05;
				let got = pose_ease(style, direction, t);
				assert!(
					(got - want).abs() < 2e-3,
					"{style:?} {direction:?} at {t:.2}: Meshvault {got:.4}, Studio {want:.4}"
				);
			}
		}
	}

	#[test]
	fn keyframe_tracks_skip_placeholder_poses() {
		let record = AnimationClip {
			name: "Idle".into(),
			status: ClipStatus::KeyframeSequence,
			duration: 2.0,
			looped: true,
			keyframes: vec![
				Keyframe {
					time: 0.0,
					poses: vec![pose(
						"HumanoidRootPart",
						0.0,
						0.0,
						vec![pose("Torso", 0.0, 1.0, vec![])],
					)],
					markers: vec![],
				},
				Keyframe {
					time: 1.0,
					poses: vec![pose("HumanoidRootPart", 0.0, 0.0, vec![])],
					markers: vec![Marker {
						name: "Step".into(),
						value: "L".into(),
					}],
				},
				Keyframe {
					time: 2.0,
					poses: vec![pose(
						"HumanoidRootPart",
						0.0,
						0.0,
						vec![pose("Torso", 2.0, 1.0, vec![])],
					)],
					markers: vec![],
				},
			],
			..Default::default()
		};
		let clip = Clip::from_record(&record).unwrap();
		assert_eq!(clip.tracks.len(), 1, "the weight-0 root is not a track");
		let torso = &clip.tracks[0];
		assert_eq!(torso.target.name, "Torso");
		assert_eq!(torso.target.parent.as_deref(), Some("HumanoidRootPart"));
		// frame 1 has no Torso pose, so it interpolates straight from 0 to 2
		assert!((torso.sample(1.0).pos[1] - 1.0).abs() < 1e-6);
		assert_eq!(clip.markers.len(), 1);
		assert_eq!(clip.markers[0].time, 1.0);
	}

	#[test]
	fn curve_tracks_combine_position_and_rotation() {
		let channels = Channels {
			y: Some(vec![key(0.0, 0.0, "Linear"), key(1.0, 0.25, "Linear")]),
			..Default::default()
		};
		let euler = Channels {
			y: Some(vec![
				key(0.0, 0.0, "Linear"),
				key(1.0, std::f32::consts::PI, "Linear"),
			]),
			..Default::default()
		};
		let record = AnimationClip {
			status: ClipStatus::CurveAnimation,
			duration: 1.0,
			curves: vec![
				CurveTrack {
					path: "HumanoidRootPart/Body".into(),
					name: "Position".into(),
					curve: CurveData::Vector3Curve { channels },
				},
				CurveTrack {
					path: "HumanoidRootPart/Body".into(),
					name: "Rotation".into(),
					curve: CurveData::EulerRotationCurve {
						rotation_order: "XYZ".into(),
						channels: euler,
					},
				},
				CurveTrack {
					path: "Face".into(),
					name: "Smile".into(),
					curve: CurveData::FloatCurve { keys: vec![] },
				},
			],
			..Default::default()
		};
		let clip = Clip::from_record(&record).unwrap();
		assert_eq!(clip.tracks.len(), 1);
		assert_eq!(clip.skipped_curves, 1);
		let body = &clip.tracks[0];
		assert_eq!(body.target.name, "Body");
		assert_eq!(body.target.parent.as_deref(), Some("HumanoidRootPart"));
		let mid = body.sample(0.5);
		assert!((mid.pos[1] - 0.125).abs() < 1e-6);
		assert!(mid.approx_eq(
			&CFrame {
				pos: [0.0, 0.125, 0.0],
				..CFrame::axis_rotation(1, std::f32::consts::FRAC_PI_2)
			},
			1e-5
		));
	}

	/// Face controls come from NumberPoses in a keyframe clip's FaceControls
	/// and from FloatCurves in a curve clip's FaceControls folder; a clip
	/// with nothing else still plays.
	#[test]
	fn face_controls_are_tracks() {
		use crate::anim::schema::FaceControlPose;
		let jaw = |value: f32| FaceControlPose {
			name: "JawDrop".into(),
			value,
			easing_style: EasingStyle::Linear,
			easing_direction: EasingDirection::In,
		};
		let frame = |time: f32, value: f32| Keyframe {
			time,
			poses: vec![Pose {
				face_controls: vec![jaw(value)],
				..pose("Head", 0.0, 0.0, vec![])
			}],
			markers: vec![],
		};
		let record = AnimationClip {
			status: ClipStatus::KeyframeSequence,
			duration: 1.0,
			keyframes: vec![frame(0.0, 0.0), frame(1.0, 0.8)],
			..Default::default()
		};
		let clip = Clip::from_record(&record).unwrap();
		assert!(clip.tracks.is_empty());
		let w = clip.face_weights(0.5);
		assert_eq!(w.len(), 1);
		assert_eq!(w[0].0, "JawDrop");
		assert!((w[0].1 - 0.4).abs() < 1e-6);
		let curves = AnimationClip {
			status: ClipStatus::CurveAnimation,
			duration: 1.0,
			curves: vec![CurveTrack {
				path: "Head/FaceControls".into(),
				name: "LeftEyeClosed".into(),
				curve: CurveData::FloatCurve {
					keys: vec![key(0.0, 0.0, "Linear"), key(1.0, 1.0, "Linear")],
				},
			}],
			..Default::default()
		};
		let clip = Clip::from_record(&curves).unwrap();
		assert_eq!(clip.skipped_curves, 0);
		assert_eq!(clip.face_weights(0.25)[0], ("LeftEyeClosed", 0.25));
	}

	#[test]
	fn unplayable_clips_explain_why() {
		let record = AnimationClip {
			status: ClipStatus::Unavailable,
			..Default::default()
		};
		assert!(
			Clip::from_record(&record)
				.unwrap_err()
				.contains("could not be loaded")
		);
	}
}
