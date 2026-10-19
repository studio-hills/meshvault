//! A model's rig: parts connected by Motor6D joints, plus bones, solved
//! into world transforms for a set of animated local transforms.
//!
//! Motor6D: `Part1.world = Part0.world * C0 * Transform * C1⁻¹`.
//! Bone:    `Bone.world  = Parent.world * Bone.CFrame * Transform`.
//! Parts that are not any joint's `Part1` stay at their exported CFrame.

use std::collections::HashMap;

use super::clip::{Clip, TargetKey};
use super::math::{CFrame, V3};
use super::schema::AnimatedModel;

#[derive(Clone, Debug)]
pub struct RigPart {
	pub name: String,
	pub path: String,
	pub rest: CFrame,
	pub size: V3,
	pub mesh_ref: u32,
	pub transparency: f32,
}

#[derive(Clone, Debug)]
pub struct Motor {
	pub name: String,
	pub part0: usize,
	pub part1: usize,
	pub c0: CFrame,
	pub c1: CFrame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoneParent {
	Part(usize),
	Bone(usize),
	/// The parent is outside the exported parts (treated as the origin).
	None,
}

#[derive(Clone, Debug)]
pub struct RigBone {
	pub name: String,
	pub path: String,
	pub parent: BoneParent,
	pub local: CFrame,
}

/// What a clip track drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
	Motor(usize),
	Bone(usize),
}

#[derive(Clone, Debug, Default)]
pub struct Binding {
	/// For each track of the clip, its target if one was found.
	pub targets: Vec<Option<Target>>,
	pub unmatched: Vec<String>,
}

impl Binding {
	pub fn matched(&self) -> usize {
		self.targets.iter().filter(|t| t.is_some()).count()
	}
}

/// World transforms of every part and bone.
#[derive(Clone, Debug)]
pub struct Posed {
	pub parts: Vec<CFrame>,
	pub bones: Vec<CFrame>,
}

#[derive(Clone, Debug, Default)]
pub struct Rig {
	pub parts: Vec<RigPart>,
	pub motors: Vec<Motor>,
	pub bones: Vec<RigBone>,
	/// Motors in an order where every Part0 is solved before its Part1.
	motor_order: Vec<usize>,
	/// Bones in an order where parents come first.
	bone_order: Vec<usize>,
	/// Problems found while building, for the inspector.
	pub warnings: Vec<String>,
}

impl Rig {
	pub fn build(model: &AnimatedModel) -> Rig {
		let mut rig = Rig::default();
		// names may contain '/', so references are resolved by exact path
		let mut part_index = HashMap::new();
		for p in &model.parts {
			part_index.insert(p.path.as_str(), rig.parts.len());
			rig.parts.push(RigPart {
				name: p.name.clone(),
				path: p.path.clone(),
				rest: CFrame::from_components(&p.cframe),
				size: p.size,
				mesh_ref: p.mesh_ref,
				transparency: p.transparency,
			});
		}
		for j in &model.joints {
			match (part_index.get(j.part0.as_str()), part_index.get(j.part1.as_str())) {
				(Some(&part0), Some(&part1)) => rig.motors.push(Motor {
					name: j.name.clone(),
					part0,
					part1,
					c0: CFrame::from_components(&j.c0),
					c1: CFrame::from_components(&j.c1),
				}),
				_ => rig
					.warnings
					.push(format!("Joint {} links a part that was not exported", j.path)),
			}
		}
		let bone_index: HashMap<&str, usize> =
			model.bones.iter().enumerate().map(|(i, b)| (b.path.as_str(), i)).collect();
		for b in &model.bones {
			let parent = if let Some(&i) = part_index.get(b.parent_path.as_str()) {
				BoneParent::Part(i)
			} else if let Some(&i) = bone_index.get(b.parent_path.as_str()) {
				BoneParent::Bone(i)
			} else {
				BoneParent::None
			};
			rig.bones.push(RigBone {
				name: b.name.clone(),
				path: b.path.clone(),
				parent,
				local: CFrame::from_components(&b.cframe),
			});
		}
		rig.motor_order = rig.solve_order();
		rig.bone_order = rig.bone_solve_order();
		rig
	}

	/// Orders motors by the depth of their Part1 in the joint tree.
	fn solve_order(&mut self) -> Vec<usize> {
		let mut driver: HashMap<usize, usize> = HashMap::new();
		for (i, m) in self.motors.iter().enumerate() {
			if driver.insert(m.part1, i).is_some() {
				self.warnings.push(format!(
					"Part {} is driven by more than one joint; using the last",
					self.parts[m.part1].path
				));
			}
		}
		let depth = |mut part: usize| -> Option<usize> {
			let mut d = 0;
			while let Some(&m) = driver.get(&part) {
				part = self.motors[m].part0;
				d += 1;
				if d > self.motors.len() {
					return None; // a cycle
				}
			}
			Some(d)
		};
		let mut order: Vec<(usize, usize)> = Vec::new();
		let mut cyclic = false;
		for (i, m) in self.motors.iter().enumerate() {
			if driver.get(&m.part1) != Some(&i) {
				continue;
			}
			match depth(m.part1) {
				Some(d) => order.push((d, i)),
				None => cyclic = true,
			}
		}
		if cyclic {
			self.warnings.push("Some joints form a loop and are ignored".into());
		}
		order.sort();
		order.into_iter().map(|(_, i)| i).collect()
	}

	fn bone_solve_order(&self) -> Vec<usize> {
		let depth = |mut i: usize| {
			let mut d = 0;
			while let BoneParent::Bone(p) = self.bones[i].parent {
				i = p;
				d += 1;
				if d > self.bones.len() {
					break;
				}
			}
			d
		};
		let mut order: Vec<(usize, usize)> = (0..self.bones.len()).map(|i| (depth(i), i)).collect();
		order.sort();
		order.into_iter().map(|(_, i)| i).collect()
	}

	/// Matches each clip track to a motor (by its Part1 name) or a bone.
	/// When several targets share a name, the track's parent name picks
	/// the one whose Part0 or parent has that name.
	pub fn bind(&self, clip: &Clip) -> Binding {
		let mut binding = Binding::default();
		for track in &clip.tracks {
			let target = self.find_target(&track.target);
			// a pose on the assembly root (a part no joint drives) has nothing to
			// drive; it only groups the poses below it
			let is_root_part = || self.parts.iter().any(|p| p.name == track.target.name);
			if target.is_none() && !is_root_part() {
				binding.unmatched.push(track.target.name.clone());
			}
			binding.targets.push(target);
		}
		binding
	}

	fn find_target(&self, key: &TargetKey) -> Option<Target> {
		let pick = |candidates: Vec<(Target, &str)>| -> Option<Target> {
			match candidates.len() {
				0 => None,
				1 => Some(candidates[0].0),
				_ => key
					.parent
					.as_deref()
					.and_then(|parent| candidates.iter().find(|(_, p)| *p == parent))
					.or(candidates.first())
					.map(|(t, _)| *t),
			}
		};
		let motors = self
			.motors
			.iter()
			.enumerate()
			.filter(|(_, m)| self.parts[m.part1].name == key.name)
			.map(|(i, m)| (Target::Motor(i), self.parts[m.part0].name.as_str()))
			.collect();
		pick(motors).or_else(|| {
			let bones = self
				.bones
				.iter()
				.enumerate()
				.filter(|(_, b)| b.name == key.name)
				.map(|(i, b)| {
					let parent = match b.parent {
						BoneParent::Part(p) => self.parts[p].name.as_str(),
						BoneParent::Bone(p) => self.bones[p].name.as_str(),
						BoneParent::None => "",
					};
					(Target::Bone(i), parent)
				})
				.collect();
			pick(bones)
		})
	}

	pub fn rest(&self) -> Posed {
		self.solve(&[], &[])
	}

	/// Solves world transforms from a clip sample at `time`.
	pub fn pose(&self, clip: &Clip, binding: &Binding, time: f32) -> Posed {
		let mut motor_t = vec![CFrame::IDENTITY; self.motors.len()];
		let mut bone_t = vec![CFrame::IDENTITY; self.bones.len()];
		for ((_, transform), target) in clip.sample(time).zip(&binding.targets) {
			match target {
				Some(Target::Motor(i)) => motor_t[*i] = transform,
				Some(Target::Bone(i)) => bone_t[*i] = transform,
				None => {}
			}
		}
		self.solve(&motor_t, &bone_t)
	}

	/// `motor_t` / `bone_t` are local animated transforms (identity when empty).
	fn solve(&self, motor_t: &[CFrame], bone_t: &[CFrame]) -> Posed {
		let mut parts: Vec<CFrame> = self.parts.iter().map(|p| p.rest).collect();
		for &i in &self.motor_order {
			let m = &self.motors[i];
			let t = motor_t.get(i).copied().unwrap_or(CFrame::IDENTITY);
			parts[m.part1] = parts[m.part0].mul(&m.c0).mul(&t).mul(&m.c1.inverse());
		}
		let mut bones = vec![CFrame::IDENTITY; self.bones.len()];
		for &i in &self.bone_order {
			let b = &self.bones[i];
			let parent = match b.parent {
				BoneParent::Part(p) => parts[p],
				BoneParent::Bone(p) => bones[p],
				BoneParent::None => CFrame::IDENTITY,
			};
			let t = bone_t.get(i).copied().unwrap_or(CFrame::IDENTITY);
			bones[i] = parent.mul(&b.local).mul(&t);
		}
		Posed { parts, bones }
	}

	/// How far the joint-solved rest pose is from the exported part CFrames.
	/// A consistent rig gives ~0; a large value means the joint data does
	/// not match the snapshot and playback may look offset.
	pub fn rest_error(&self) -> f32 {
		let posed = self.rest();
		posed
			.parts
			.iter()
			.zip(&self.parts)
			.map(|(a, b)| {
				let d = [a.pos[0] - b.rest.pos[0], a.pos[1] - b.rest.pos[1], a.pos[2] - b.rest.pos[2]];
				(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
			})
			.fold(0.0, f32::max)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::anim::schema::{
		AnimationClip, BoneInfo, ClipStatus, EasingDirection, EasingStyle, Keyframe, ModelPart, MotorJoint, Pose,
	};

	const IDENT: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

	fn at(x: f32, y: f32, z: f32) -> [f32; 12] {
		let mut c = IDENT;
		c[0] = x;
		c[1] = y;
		c[2] = z;
		c
	}

	fn part(path: &str, name: &str, cframe: [f32; 12]) -> ModelPart {
		ModelPart {
			path: path.into(),
			name: name.into(),
			mesh_ref: 0,
			cframe,
			size: [1.0; 3],
			transparency: 0.0,
		}
	}

	/// Root at y=2; an arm hinged 1 stud above the root, hanging 1 below the hinge.
	/// The arm's name contains '/', as real exports do.
	fn model() -> AnimatedModel {
		AnimatedModel {
			name: "M".into(),
			path: "W/M".into(),
			parts: vec![part("W/M/Root", "Root", at(0.0, 2.0, 0.0)), part("W/M/Meshes/Arm", "Meshes/Arm", at(0.0, 2.0, 0.0))],
			joints: vec![MotorJoint {
				name: "Shoulder".into(),
				path: "W/M/Root/Shoulder".into(),
				part0: "W/M/Root".into(),
				part1: "W/M/Meshes/Arm".into(),
				c0: at(0.0, 1.0, 0.0),
				c1: at(0.0, 1.0, 0.0),
			}],
			bones: vec![
				BoneInfo {
					name: "Tail".into(),
					path: "W/M/Root/Tail".into(),
					parent_path: "W/M/Root".into(),
					cframe: at(0.0, 0.0, 1.0),
				},
				BoneInfo {
					name: "Tip".into(),
					path: "W/M/Root/Tail/Tip".into(),
					parent_path: "W/M/Root/Tail".into(),
					cframe: at(0.0, 0.0, 1.0),
				},
			],
			animations: vec![],
		}
	}

	fn rotate_pose(name: &str, angle: f32) -> Pose {
		let r = CFrame::axis_rotation(2, angle).components();
		Pose {
			name: name.into(),
			transform: r,
			weight: 1.0,
			easing_style: EasingStyle::Linear,
			easing_direction: EasingDirection::In,
			children: vec![],
		}
	}

	#[test]
	fn rest_pose_matches_exported_cframes() {
		let rig = Rig::build(&model());
		assert!(rig.warnings.is_empty(), "{:?}", rig.warnings);
		assert!(rig.rest_error() < 1e-6);
		let rest = rig.rest();
		assert!(rest.bones[1].approx_eq(&CFrame::from_components(&at(0.0, 2.0, 2.0)), 1e-6));
	}

	#[test]
	fn motor_and_bone_animation() {
		let rig = Rig::build(&model());
		let half_turn = std::f32::consts::PI;
		let record = AnimationClip {
			status: ClipStatus::KeyframeSequence,
			duration: 1.0,
			keyframes: vec![
				Keyframe {
					time: 0.0,
					poses: vec![rotate_pose("Meshes/Arm", 0.0), rotate_pose("Tail", 0.0)],
					markers: vec![],
				},
				Keyframe {
					time: 1.0,
					poses: vec![rotate_pose("Meshes/Arm", half_turn), rotate_pose("Tail", half_turn / 2.0)],
					markers: vec![],
				},
			],
			..Default::default()
		};
		let clip = Clip::from_record(&record).unwrap();
		let binding = rig.bind(&clip);
		assert_eq!(binding.matched(), 2, "unmatched: {:?}", binding.unmatched);
		let posed = rig.pose(&clip, &binding, 1.0);
		// the arm swings up around the hinge at y=3: its centre goes from 2 to 4
		assert!((posed.parts[1].pos[1] - 4.0).abs() < 1e-5, "{:?}", posed.parts[1].pos);
		// the root does not move
		assert!(posed.parts[0].approx_eq(&rig.parts[0].rest, 1e-6));
		// the tail bends 90° about Z: the tip, 1 stud along Z from the tail, stays put along Z
		assert!(posed.bones[1].pos[2] > 1.9);
	}

	#[test]
	fn missing_parts_warn_instead_of_failing() {
		let mut m = model();
		m.joints[0].part1 = "W/M/Gone".into();
		let rig = Rig::build(&m);
		assert_eq!(rig.motors.len(), 0);
		assert_eq!(rig.warnings.len(), 1);
	}
}
