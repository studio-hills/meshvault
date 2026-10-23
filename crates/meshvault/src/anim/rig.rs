//! A model's rig: parts connected by joints (Motor6Ds, AnimationConstraints
//! and welds), plus bones, solved into world transforms for a set of
//! animated local transforms.
//!
//! Motor6D: `Part1.world = Part0.world * C0 * Transform * C1⁻¹`.
//! Bone:    `Bone.world  = Parent.world * Bone.CFrame * Transform`.
//! Welds follow the Motor6D formula with no Transform. Joints are solved
//! outward from each assembly's root, from whichever end is already
//! placed; parts no joint reaches stay at their exported CFrame.

use std::collections::{HashMap, VecDeque};

use super::clip::{Clip, TargetKey};
use super::math::{CFrame, V3, distance};
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
pub struct Joint {
	pub name: String,
	pub part0: usize,
	pub part1: usize,
	pub c0: CFrame,
	pub c1: CFrame,
	/// `Motor6D.Transform` when the model was exported (schema 2), or
	/// identity. The rest pose shows it; clips replace it.
	pub captured: CFrame,
	/// A Motor6D or AnimationConstraint, which clips drive; a weld only
	/// holds its parts together.
	pub animated: bool,
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
	Joint(usize),
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
	pub joints: Vec<Joint>,
	pub bones: Vec<RigBone>,
	/// Joints in solving order, each with whether it places its Part1 from
	/// its Part0 (or, for a joint pointing back at the root, the reverse).
	joint_order: Vec<(usize, bool)>,
	/// Bones in an order where parents come first.
	bone_order: Vec<usize>,
	/// Problems found while building, for the inspector.
	pub warnings: Vec<String>,
}

fn closest<'a>(
	items: impl Iterator<Item = (&'a str, &'a str)>,
	name: &str,
	near: &str,
) -> Option<usize> {
	let shared = |path: &str| {
		path.split('/')
			.zip(near.split('/'))
			.take_while(|(a, b)| a == b)
			.count()
	};
	items
		.enumerate()
		.filter(|(_, (n, _))| *n == name)
		// max_by_key keeps the last of equals, so reverse to prefer the first
		.map(|(i, (_, path))| (shared(path), std::cmp::Reverse(i)))
		.max()
		.map(|(_, std::cmp::Reverse(i))| i)
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
			match (
				part_index.get(j.part0.as_str()),
				part_index.get(j.part1.as_str()),
			) {
				(Some(&part0), Some(&part1)) => rig.joints.push(Joint {
					name: j.name.clone(),
					part0,
					part1,
					c0: CFrame::from_components(&j.c0),
					c1: CFrame::from_components(&j.c1),
					captured: j
						.transform
						.as_ref()
						.map_or(CFrame::IDENTITY, CFrame::from_components),
					animated: matches!(
						j.kind.as_deref(),
						None | Some("Motor6D" | "AnimationConstraint")
					),
				}),
				_ => rig.warnings.push(format!(
					"Joint {} links a part that was not exported",
					j.path
				)),
			}
		}
		let bone_index: HashMap<&str, usize> = model
			.bones
			.iter()
			.enumerate()
			.map(|(i, b)| (b.path.as_str(), i))
			.collect();
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
		rig.joint_order = rig.solve_order();
		rig.bone_order = rig.bone_solve_order();
		rig
	}

	/// Which parts are animation roots: a part that drives animated joints
	/// but that no animated joint drives, like HumanoidRootPart.
	fn animation_roots(&self) -> Vec<bool> {
		let n = self.parts.len();
		let (mut drives, mut driven) = (vec![false; n], vec![false; n]);
		for m in self.joints.iter().filter(|m| m.animated) {
			drives[m.part0] = true;
			driven[m.part1] = true;
		}
		drives.iter().zip(driven).map(|(&d, n)| d && !n).collect()
	}

	/// Orders the joints outward from each assembly's root, so each is
	/// solved from whichever of its parts is placed first. The animation
	/// root is the root where there is one. Welds may point back towards
	/// it: an accessory's weld runs from its Handle to the Head.
	fn solve_order(&mut self) -> Vec<(usize, bool)> {
		let n = self.parts.len();
		let animation_root = self.animation_roots();
		let mut joints_of: Vec<Vec<usize>> = vec![Vec::new(); n];
		let mut driven = vec![false; n];
		for (i, m) in self.joints.iter().enumerate() {
			joints_of[m.part0].push(i);
			if m.part1 != m.part0 {
				joints_of[m.part1].push(i);
			}
			driven[m.part1] = true;
		}
		let mut roots: Vec<usize> = (0..n).collect();
		// stable: among equals, the export's order
		roots.sort_by_key(|&p| {
			if animation_root[p] {
				0
			} else if !driven[p] {
				1
			} else {
				2
			}
		});
		let mut placed = vec![false; n];
		let mut used = vec![false; self.joints.len()];
		let mut order = Vec::new();
		let mut looped = false;
		for root in roots {
			if placed[root] || joints_of[root].is_empty() {
				continue;
			}
			placed[root] = true;
			let mut queue = VecDeque::from([root]);
			while let Some(part) = queue.pop_front() {
				for &j in &joints_of[part] {
					if std::mem::replace(&mut used[j], true) {
						continue;
					}
					let m = &self.joints[j];
					let (other, forward) = if m.part0 == part {
						(m.part1, true)
					} else {
						(m.part0, false)
					};
					if placed[other] {
						looped = true;
						continue;
					}
					placed[other] = true;
					order.push((j, forward));
					queue.push_back(other);
				}
			}
		}
		if looped {
			self.warnings.push(
				"Some joints connect parts that other joints already connect, and are ignored"
					.into(),
			);
		}
		order
	}

	/// Orders the bones parents first, visiting each once. Bad data can loop
	/// a bone's parents back to it: walking up from the first bone in the
	/// loop, the last bone before the walk comes back drops its parent and
	/// is placed from the origin.
	fn bone_solve_order(&mut self) -> Vec<usize> {
		#[derive(Clone, Copy, PartialEq)]
		enum Seen {
			No,
			OnWalk,
			Ordered,
		}
		let mut seen = vec![Seen::No; self.bones.len()];
		let mut order = Vec::with_capacity(self.bones.len());
		let mut walk = Vec::new();
		let mut broken = Vec::new();
		for start in 0..self.bones.len() {
			let mut b = start;
			while seen[b] == Seen::No {
				seen[b] = Seen::OnWalk;
				walk.push(b);
				match self.bones[b].parent {
					BoneParent::Bone(p) => b = p,
					_ => break,
				}
			}
			if seen[b] == Seen::OnWalk
				&& let Some(&last) = walk.last()
				&& self.bones[last].parent == BoneParent::Bone(b)
			{
				self.bones[last].parent = BoneParent::None;
				broken.push(last);
			}
			// the walk went up from `start`, so its top is solved first
			for &b in walk.iter().rev() {
				seen[b] = Seen::Ordered;
				order.push(b);
			}
			walk.clear();
		}
		if let Some(&first) = broken.first() {
			let others = match broken.len() {
				1 => String::new(),
				n => format!(" (and {} other bones)", n - 1),
			};
			self.warnings.push(format!(
				"Bone {}{others}: its parents loop back to it, so it is placed from the origin",
				self.bones[first].path
			));
		}
		order
	}

	/// Bones in an order where parents come first.
	pub fn bone_order(&self) -> &[usize] {
		&self.bone_order
	}

	/// Matches each clip track to a joint (by its Part1 name) or a bone.
	/// When several targets share a name, the track's parent name picks
	/// the one whose Part0 or parent has that name.
	pub fn bind(&self, clip: &Clip) -> Binding {
		let mut binding = Binding::default();
		let animation_root = self.animation_roots();
		for track in &clip.tracks {
			let target = self.find_target(&track.target);
			// a pose on an animation root has nothing to move; it only groups
			// the poses below it. Any root part of that name counts, not just
			// this track's own. A part no joint touches at all is a real miss: a
			// rig whose joints were not exported would otherwise look fully bound.
			let is_root_part = || {
				self.parts
					.iter()
					.zip(&animation_root)
					.any(|(p, &root)| root && p.name == track.target.name)
			};
			if target.is_none() && !is_root_part() {
				binding.unmatched.push(track.target.name.clone());
			}
			binding.targets.push(target);
		}
		binding
	}

	/// Roblox's Animator drives a joint only when both names match: the pose
	/// is named after the Motor6D's Part1 (or the Bone) and its parent pose
	/// after Part0 (or the Bone's parent). A pose under the wrong parent, or
	/// at the top of a keyframe, moves nothing, so it is unmatched here too.
	fn find_target(&self, key: &TargetKey) -> Option<Target> {
		let parent = key.parent.as_deref()?;
		let pick = |candidates: Vec<(Target, &str)>| -> Option<Target> {
			candidates
				.iter()
				.find(|(_, p)| *p == parent)
				.map(|(t, _)| *t)
		};
		let joints = self
			.joints
			.iter()
			.enumerate()
			// welds are not animated, even when named like a body part
			.filter(|(_, m)| m.animated && self.parts[m.part1].name == key.name)
			.map(|(i, m)| (Target::Joint(i), self.parts[m.part0].name.as_str()))
			.collect();
		pick(joints).or_else(|| {
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

	/// Each part's parent in the joint tree: the part a joint places it
	/// from. Roots (and parts no joint reaches) have none.
	pub fn part_parents(&self) -> Vec<Option<usize>> {
		self.part_links()
			.into_iter()
			.map(|l| l.map(|(parent, _)| parent))
			.collect()
	}

	/// Each part's parent in the joint tree and the joint between them.
	pub fn part_links(&self) -> Vec<Option<(usize, usize)>> {
		let mut links = vec![None; self.parts.len()];
		for &(i, forward) in &self.joint_order {
			let m = &self.joints[i];
			if forward {
				links[m.part1] = Some((m.part0, i));
			} else {
				links[m.part0] = Some((m.part1, i));
			}
		}
		links
	}

	/// The pose the model was exported in: joints at their captured
	/// transforms (identity for schema 1, which did not record them).
	pub fn rest(&self) -> Posed {
		let captured: Vec<CFrame> = self.joints.iter().map(|m| m.captured).collect();
		self.solve(&captured, &[])
	}

	/// The pose with every joint at rest (no Transform), as a character is
	/// built: what its clothing is laid out on.
	pub fn joint_rest(&self) -> Posed {
		self.solve(&[], &[])
	}

	/// Solves world transforms from a clip sample at `time`.
	#[cfg(test)]
	pub fn pose(&self, clip: &Clip, binding: &Binding, time: f32) -> Posed {
		let samples: Vec<CFrame> = clip.sample(time).map(|(_, t)| t).collect();
		self.pose_from(binding, &samples)
	}

	/// Solves world transforms from already sampled tracks, one per entry
	/// of `binding.targets`.
	pub fn pose_from(&self, binding: &Binding, samples: &[CFrame]) -> Posed {
		let mut joint_t = vec![CFrame::IDENTITY; self.joints.len()];
		let mut bone_t = vec![CFrame::IDENTITY; self.bones.len()];
		for (transform, target) in samples.iter().zip(&binding.targets) {
			match target {
				Some(Target::Joint(i)) => joint_t[*i] = *transform,
				Some(Target::Bone(i)) => bone_t[*i] = *transform,
				None => {}
			}
		}
		self.solve(&joint_t, &bone_t)
	}

	/// The bone named `name` closest to `near` in the instance tree (the
	/// longest shared path), since names repeat across accessories.
	pub fn bone_near(&self, name: &str, near: &str) -> Option<usize> {
		closest(
			self.bones
				.iter()
				.map(|b| (b.name.as_str(), b.path.as_str())),
			name,
			near,
		)
	}

	/// The part named `name` closest to `near`; see [`Self::bone_near`].
	pub fn part_near(&self, name: &str, near: &str) -> Option<usize> {
		closest(
			self.parts
				.iter()
				.map(|p| (p.name.as_str(), p.path.as_str())),
			name,
			near,
		)
	}

	/// `joint_t` / `bone_t` are local animated transforms (identity when empty).
	fn solve(&self, joint_t: &[CFrame], bone_t: &[CFrame]) -> Posed {
		let mut parts: Vec<CFrame> = self.parts.iter().map(|p| p.rest).collect();
		for &(i, forward) in &self.joint_order {
			let m = &self.joints[i];
			let t = joint_t.get(i).copied().unwrap_or(CFrame::IDENTITY);
			if forward {
				parts[m.part1] = parts[m.part0].mul(&m.c0).mul(&t).mul(&m.c1.inverse());
			} else {
				parts[m.part0] = parts[m.part1]
					.mul(&m.c1)
					.mul(&t.inverse())
					.mul(&m.c0.inverse());
			}
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
			.map(|(a, b)| distance(a.pos, b.rest.pos))
			.fold(0.0, f32::max)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::anim::schema::{
		AnimationClip, BoneInfo, ClipStatus, EasingDirection, EasingStyle, JointInfo, Keyframe,
		ModelPart, Pose,
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
			..Default::default()
		}
	}

	/// Root at y=2; an arm hinged 1 stud above the root, hanging 1 below the hinge.
	/// The arm's name contains '/', as real exports do.
	fn model() -> AnimatedModel {
		AnimatedModel {
			name: "M".into(),
			path: "W/M".into(),
			scale: None,
			parts: vec![
				part("W/M/Root", "Root", at(0.0, 2.0, 0.0)),
				part("W/M/Meshes/Arm", "Meshes/Arm", at(0.0, 2.0, 0.0)),
			],
			joints: vec![JointInfo {
				name: "Shoulder".into(),
				path: "W/M/Root/Shoulder".into(),
				part0: "W/M/Root".into(),
				part1: "W/M/Meshes/Arm".into(),
				c0: at(0.0, 1.0, 0.0),
				c1: at(0.0, 1.0, 0.0),
				transform: None,
				kind: None,
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
			clothing: None,
		}
	}

	fn rotate_pose(name: &str, angle: f32) -> Pose {
		turn_pose(name, 2, angle)
	}

	/// A pose turning `angle` radians about `axis` (0 = X, 1 = Y, 2 = Z).
	fn turn_pose(name: &str, axis: usize, angle: f32) -> Pose {
		let r = CFrame::axis_rotation(axis, angle).components();
		Pose {
			name: name.into(),
			transform: r,
			weight: 1.0,
			easing_style: EasingStyle::Linear,
			easing_direction: EasingDirection::In,
			children: vec![],
			face_controls: vec![],
		}
	}

	/// `children` under a placeholder pose for `name`, as Roblox nests them.
	fn under(name: &str, children: Vec<Pose>) -> Pose {
		Pose {
			weight: 0.0,
			children,
			..rotate_pose(name, 0.0)
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

	/// Schema 2 records Motor6D.Transform: the rest pose shows it, so a
	/// model captured mid-animation solves back to its exported CFrames.
	#[test]
	fn rest_pose_uses_captured_joint_transforms() {
		let mut m = model();
		// the arm was exported swung 90° about X at the shoulder
		let swing = CFrame::axis_rotation(0, std::f32::consts::FRAC_PI_2);
		m.joints[0].transform = Some(swing.components());
		let root = CFrame::from_components(&m.parts[0].cframe);
		let c0 = CFrame::from_components(&m.joints[0].c0);
		let c1 = CFrame::from_components(&m.joints[0].c1);
		m.parts[1].cframe = root.mul(&c0).mul(&swing).mul(&c1.inverse()).components();
		let rig = Rig::build(&m);
		assert!(rig.rest_error() < 1e-5, "captured pose matches the export");
		// without the capture the same export looks like a mid-animation snapshot
		m.joints[0].transform = None;
		assert!(Rig::build(&m).rest_error() > 0.5);
	}

	/// A pose on the assembly root is harmless, but a pose on a part that no
	/// exported joint connects (an AnimationConstraint rig) is a real miss.
	#[test]
	fn unjoined_parts_are_unmatched() {
		let record = AnimationClip {
			status: ClipStatus::KeyframeSequence,
			duration: 1.0,
			keyframes: vec![Keyframe {
				time: 0.0,
				poses: vec![Pose {
					children: vec![rotate_pose("Meshes/Arm", 0.0)],
					..rotate_pose("Root", 0.0)
				}],
				markers: vec![],
			}],
			..Default::default()
		};
		let clip = Clip::from_record(&record).unwrap();
		let rig = Rig::build(&model());
		assert!(rig.bind(&clip).unmatched.is_empty(), "Root drives the arm");
		let mut unjoined = model();
		unjoined.joints.clear();
		let binding = Rig::build(&unjoined).bind(&clip);
		assert_eq!(binding.unmatched, ["Root", "Meshes/Arm"]);
	}

	#[test]
	fn joint_and_bone_animation() {
		let rig = Rig::build(&model());
		let half_turn = std::f32::consts::PI;
		let record = AnimationClip {
			status: ClipStatus::KeyframeSequence,
			duration: 1.0,
			keyframes: vec![
				Keyframe {
					time: 0.0,
					poses: vec![under(
						"Root",
						vec![rotate_pose("Meshes/Arm", 0.0), rotate_pose("Tail", 0.0)],
					)],
					markers: vec![],
				},
				Keyframe {
					time: 1.0,
					poses: vec![under(
						"Root",
						vec![
							rotate_pose("Meshes/Arm", half_turn),
							turn_pose("Tail", 0, half_turn / 2.0),
						],
					)],
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
		assert!(
			(posed.parts[1].pos[1] - 4.0).abs() < 1e-5,
			"{:?}",
			posed.parts[1].pos
		);
		// the root does not move
		assert!(posed.parts[0].approx_eq(&rig.parts[0].rest, 1e-6));
		// the tail, at (0, 2, 1), bends 90° about X: the tip, 1 stud along its
		// Z, swings from (0, 2, 2) down to (0, 1, 1)
		let tip = posed.bones[1].pos;
		for (got, want) in tip.into_iter().zip([0.0, 1.0, 1.0]) {
			assert!((got - want).abs() < 1e-5, "tip {tip:?}");
		}
	}

	/// An accessory's weld runs from its Handle to the body part (Part0 is
	/// the Handle), the other way to the Motor6Ds: it is solved backwards
	/// from the arm, follows the arm's animation, and no clip drives it.
	#[test]
	fn welded_accessories_follow_their_part() {
		let mut m = model();
		m.parts
			.push(part("W/M/Hat/Handle", "Handle", at(0.0, 0.5, 0.0)));
		m.joints.push(JointInfo {
			name: "AccessoryWeld".into(),
			path: "W/M/Hat/Handle/AccessoryWeld".into(),
			part0: "W/M/Hat/Handle".into(),
			part1: "W/M/Meshes/Arm".into(),
			// the handle sits 1.5 studs below the arm's centre
			c0: at(0.0, 0.0, 0.0),
			c1: at(0.0, -1.5, 0.0),
			transform: None,
			kind: Some("Weld".into()),
		});
		let rig = Rig::build(&m);
		assert!(rig.warnings.is_empty(), "{:?}", rig.warnings);
		assert!(
			rig.rest_error() < 1e-5,
			"the weld holds the handle where it was exported"
		);
		// swing the arm half a turn about Z around its hinge: the handle goes with it
		let clip = Clip::from_record(&AnimationClip {
			status: ClipStatus::KeyframeSequence,
			duration: 1.0,
			keyframes: vec![Keyframe {
				time: 0.0,
				poses: vec![under(
					"Root",
					vec![
						rotate_pose("Meshes/Arm", std::f32::consts::PI),
						// a pose for the handle under the arm: welds are not animated
						rotate_pose("Handle", 1.0),
					],
				)],
				markers: vec![],
			}],
			..Default::default()
		})
		.unwrap();
		let binding = rig.bind(&clip);
		assert_eq!(binding.matched(), 1);
		assert_eq!(binding.unmatched, ["Handle"]);
		let posed = rig.pose(&clip, &binding, 0.0);
		let (arm, handle) = (posed.parts[1], posed.parts[2]);
		// the arm's centre moved from y = 2 to y = 4, and the handle from 0.5 to 5.5
		assert!((arm.pos[1] - 4.0).abs() < 1e-4, "{:?}", arm.pos);
		assert!((handle.pos[1] - 5.5).abs() < 1e-4, "{:?}", handle.pos);
	}

	/// Like Roblox, a pose only drives a joint under its parent's name: the
	/// right name under another parent, or at the top level, moves nothing.
	#[test]
	fn poses_match_by_parent_and_name() {
		let rig = Rig::build(&model());
		let clip = |poses: Vec<Pose>| {
			Clip::from_record(&AnimationClip {
				status: ClipStatus::KeyframeSequence,
				duration: 1.0,
				keyframes: vec![Keyframe {
					time: 0.0,
					poses,
					markers: vec![],
				}],
				..Default::default()
			})
			.unwrap()
		};
		let tracks = || vec![rotate_pose("Meshes/Arm", 0.0), rotate_pose("Tail", 0.0)];
		assert_eq!(rig.bind(&clip(vec![under("Root", tracks())])).matched(), 2);
		let elsewhere = rig.bind(&clip(vec![under("Other", tracks())]));
		assert_eq!(elsewhere.matched(), 0);
		assert_eq!(elsewhere.unmatched, ["Meshes/Arm", "Tail"]);
		assert_eq!(rig.bind(&clip(tracks())).matched(), 0);
		// a bone's parent may be another bone
		let tip = clip(vec![under(
			"Root",
			vec![under("Tail", vec![rotate_pose("Tip", 0.0)])],
		)]);
		assert_eq!(rig.bind(&tip).targets, [Some(Target::Bone(1))]);
	}

	#[test]
	fn missing_parts_warn_instead_of_failing() {
		let mut m = model();
		m.joints[0].part1 = "W/M/Gone".into();
		let rig = Rig::build(&m);
		assert_eq!(rig.joints.len(), 0);
		assert_eq!(rig.warnings.len(), 1);
	}

	fn bones(parents: impl Fn(usize) -> String, n: usize) -> AnimatedModel {
		AnimatedModel {
			bones: (0..n)
				.map(|i| BoneInfo {
					name: format!("B{i}"),
					path: format!("B{i}"),
					parent_path: parents(i),
					cframe: at(0.0, 1.0, 0.0),
				})
				.collect(),
			..Default::default()
		}
	}

	/// Bones are ordered in one pass over their parents: many bones each
	/// their own parent, or one long chain, build quickly.
	#[test]
	fn bone_order_is_linear() {
		const N: usize = 100_000;
		for (what, model) in [
			("self-parented", bones(|i| format!("B{i}"), N)),
			("chain", bones(|i| format!("B{}", i.wrapping_sub(1)), N)),
		] {
			let start = std::time::Instant::now();
			let rig = Rig::build(&model);
			let took = start.elapsed();
			assert!(took.as_secs_f32() < 1.0, "{what} took {took:?}");
			assert_eq!(rig.bone_order.len(), N);
		}
		// a chain is solved from its root, each bone a stud above its parent
		let rig = Rig::build(&bones(|i| format!("B{}", i.wrapping_sub(1)), 1000));
		let posed = rig.rest();
		assert!((posed.bones[999].pos[1] - 1000.0).abs() < 1e-2);
	}

	/// A loop of parents is broken where the walk from the first bone in it
	/// closes it: that bone's parent link is dropped, with a warning, so
	/// the same data always gives the same rig.
	#[test]
	fn bone_parent_loops_are_broken() {
		// B0 -> B1 -> B2 -> B0, and B3 under B2
		let parent = |i: usize| ["B1", "B2", "B0", "B2"][i].to_owned();
		let rig = Rig::build(&bones(parent, 4));
		let parents: Vec<_> = rig.bones.iter().map(|b| b.parent).collect();
		assert_eq!(
			parents,
			[
				BoneParent::Bone(1),
				BoneParent::Bone(2),
				BoneParent::None,
				BoneParent::Bone(2),
			]
		);
		assert!(
			rig.warnings.iter().any(|w| w.contains("B2")),
			"{:?}",
			rig.warnings
		);
		let order = &rig.bone_order;
		let at = |b: usize| order.iter().position(|&o| o == b).unwrap();
		assert!(at(2) < at(1) && at(1) < at(0) && at(2) < at(3), "{order:?}");
		// a bone its own parent is a root too
		let rig = Rig::build(&bones(|_| "B0".into(), 1));
		assert_eq!(rig.bones[0].parent, BoneParent::None);
	}
}
