//! CPU skinning: each bone a mesh was bound to is matched to a Bone or
//! part of the model, or rebuilt from the mesh's bind pose, and the
//! mesh's vertices follow their bones.

use std::collections::HashMap;

use super::MeshAsset;
use crate::anim::math::{CFrame, V3};
use crate::anim::rig::{Posed, Rig};
use crate::scene::geometry::{self, Geometry, Skin};

/// What moves a bone a mesh was bound to.
#[derive(Clone, Debug)]
pub(super) enum Driver {
	Bone(usize),
	Part(usize),
	/// Only in the mesh file: placed from its bind pose relative to its
	/// parent (another driver), or to the part when it has none. Clip
	/// tracks with its name still move it.
	Mesh {
		parent: Option<usize>,
		local: CFrame,
		name: String,
	},
}

pub struct PartSkin {
	pub(super) drivers: Vec<Driver>,
	/// Drivers with their parents first.
	order: Vec<usize>,
	pub(super) influences: Vec<[(u16, f32); 4]>,
	pub(super) rest_positions: Vec<V3>,
	pub(super) rest_normals: Option<Vec<V3>>,
	/// World-space vertices in the current pose.
	pub geometry: Geometry,
	pub(super) moved: bool,
	/// The part's mesh scale, applied to clip translations of mesh-only bones.
	pub(super) unit_scale: f32,
	/// The mesh's FACS poses, which move its face bones by face control.
	facs: Option<geometry::Facs>,
	/// Mesh units to studs, for FACS offsets.
	mesh_scale: f32,
	/// How many of the mesh's bones matched a Bone or part in the model.
	pub matched: usize,
	pub bones: usize,
}

pub(super) fn fitted(cf: &CFrame, scale: V3, centre: V3, v: V3) -> V3 {
	cf.point([
		(v[0] - centre[0]) * scale[0],
		(v[1] - centre[1]) * scale[1],
		(v[2] - centre[2]) * scale[2],
	])
}

impl PartSkin {
	pub(super) fn build(
		skin: &Skin,
		asset: &MeshAsset,
		rig: &Rig,
		rest: &Posed,
		owner: usize,
		centre: V3,
		scale: V3,
	) -> Self {
		let near = rig.parts[owner].path.as_str();
		// a bind pose moved into part space, like the vertices
		let fit = |c: &[f32; 12]| {
			let b = CFrame::from_components(c);
			CFrame {
				pos: [
					(b.pos[0] - centre[0]) * scale[0],
					(b.pos[1] - centre[1]) * scale[1],
					(b.pos[2] - centre[2]) * scale[2],
				],
				rot: b.rot,
			}
		};
		let mut matched = 0;
		let drivers: Vec<Driver> = skin
			.bones
			.iter()
			.enumerate()
			.map(|(k, name)| {
				if let Some(b) = rig.bone_near(name, near) {
					matched += 1;
					Driver::Bone(b)
				} else if let Some(p) = rig.part_near(name, near) {
					matched += 1;
					Driver::Part(p)
				} else {
					let parent = skin.parents[k].map(usize::from);
					let own = fit(&skin.binds[k]);
					let local = match parent {
						Some(p) => fit(&skin.binds[p]).inverse().mul(&own),
						None => own,
					};
					Driver::Mesh {
						parent,
						local,
						name: name.clone(),
					}
				}
			})
			.collect();
		let depth = |mut k: usize| {
			let mut d = 0;
			while let Driver::Mesh {
				parent: Some(p), ..
			} = &drivers[k]
			{
				k = *p;
				d += 1;
				if d > drivers.len() {
					break;
				}
			}
			d
		};
		let mut order: Vec<(usize, usize)> = (0..drivers.len()).map(|k| (depth(k), k)).collect();
		order.sort();
		let owner_rest = rest.parts[owner];
		let g = &asset.geometry;
		let rest_positions = g
			.positions
			.iter()
			.map(|&v| fitted(&owner_rest, scale, centre, v))
			.collect();
		// normals transform by the inverse scale before rotating
		let rest_normals = g.normals.as_ref().map(|normals| {
			normals
				.iter()
				.map(|n| {
					let s = [n[0] / scale[0], n[1] / scale[1], n[2] / scale[2]];
					normalize(owner_rest.rotate(s))
				})
				.collect()
		});
		let mut out = Self {
			drivers,
			order: order.into_iter().map(|(_, k)| k).collect(),
			influences: skin.influences.clone(),
			rest_positions,
			rest_normals,
			geometry: g.clone(),
			moved: true,
			unit_scale: mean(scale),
			facs: skin.facs.clone(),
			mesh_scale: mean(scale),
			matched,
			bones: skin.bones.len(),
		};
		out.update(rest, rest, owner, &HashMap::new(), &[]);
		out
	}

	/// Deforms the mesh into `posed`. `by_name` holds clip transforms for
	/// bones that exist only in meshes.
	pub(super) fn update(
		&mut self,
		rest: &Posed,
		posed: &Posed,
		owner: usize,
		by_name: &HashMap<String, CFrame>,
		face: &[(String, f32)],
	) {
		let (now, then) = self.drivers_at(rest, posed, owner, by_name, face);
		let deltas: Vec<CFrame> = now
			.iter()
			.zip(&then)
			.map(|(a, b)| a.mul(&b.inverse()))
			.collect();
		let owner_delta = posed.parts[owner].mul(&rest.parts[owner].inverse());
		for (i, inf) in self.influences.iter().enumerate() {
			let p = self.rest_positions[i];
			let n = self.rest_normals.as_ref().map(|ns| ns[i]);
			let (mut pos, mut nrm) = ([0.0; 3], [0.0; 3]);
			let mut total = 0.0;
			for &(bone, w) in inf {
				if w <= 0.0 {
					continue;
				}
				let d = &deltas[bone as usize];
				add_scaled(&mut pos, d.point(p), w);
				if let Some(n) = n {
					add_scaled(&mut nrm, d.rotate(n), w);
				}
				total += w;
			}
			// vertices no bone moves follow their part
			if total <= 0.0 {
				pos = owner_delta.point(p);
				nrm = n.map_or([0.0; 3], |n| owner_delta.rotate(n));
			}
			self.geometry.positions[i] = pos;
			if let Some(normals) = &mut self.geometry.normals {
				normals[i] = normalize(nrm);
			}
		}
		self.moved = true;
	}

	/// Where each bone the mesh was bound to is in `posed`, and in `rest`.
	pub(super) fn drivers_at(
		&self,
		rest: &Posed,
		posed: &Posed,
		owner: usize,
		by_name: &HashMap<String, CFrame>,
		face: &[(String, f32)],
	) -> (Vec<CFrame>, Vec<CFrame>) {
		// face bones posed by the face controls (FACS), in this part's studs
		let face_bones: HashMap<&str, CFrame> = match &self.facs {
			Some(facs) if !face.is_empty() => facs
				.bone_offsets(|c| face.iter().find(|(n, _)| n == c).map_or(0.0, |f| f.1))
				.into_iter()
				// only the bones' movements: applying the poses' rotations as
				// read (degrees about X, Y, Z) curls lips far more than Studio
				// does, and how Roblox applies them is not yet known
				.map(|(b, o)| {
					let pos = [o[0], o[1], o[2]].map(|v| v * self.mesh_scale);
					(facs.bones[b].as_str(), CFrame::from_position(pos))
				})
				.collect(),
			_ => HashMap::new(),
		};
		let n = self.drivers.len();
		let mut now = vec![CFrame::IDENTITY; n];
		let mut then = vec![CFrame::IDENTITY; n];
		for &k in &self.order {
			(now[k], then[k]) = match &self.drivers[k] {
				Driver::Bone(i) => (posed.bones[*i], rest.bones[*i]),
				Driver::Part(i) => (posed.parts[*i], rest.parts[*i]),
				Driver::Mesh {
					parent,
					local,
					name,
				} => {
					let (pn, pr) = match parent {
						Some(p) => (now[*p], then[*p]),
						None => (posed.parts[owner], rest.parts[owner]),
					};
					let t = match face_bones.get(name.as_str()) {
						Some(&t) => t,
						None => by_name
							.get(name.as_str())
							.map_or(CFrame::IDENTITY, |&t| scale_translation(t, self.unit_scale)),
					};
					(pn.mul(local).mul(&t), pr.mul(local))
				}
			};
		}
		(now, then)
	}
}

pub(super) fn mean(v: V3) -> f32 {
	(v[0] + v[1] + v[2]) / 3.0
}

/// A bone's animated offset in the units of the mesh it deforms. Roblox
/// keeps skinning in mesh space and scales the result to the part's size,
/// so when a mesh made at another scale is resized (a pet modelled in
/// centimetres shrunk to 3 studs) its clips' translations shrink with it.
/// Without this, such clips throw the model metres off its spot.
pub(super) fn scale_translation(mut t: CFrame, scale: f32) -> CFrame {
	for v in &mut t.pos {
		*v *= scale;
	}
	t
}

fn add_scaled(acc: &mut V3, v: V3, w: f32) {
	for k in 0..3 {
		acc[k] += v[k] * w;
	}
}

pub(super) fn normalize(v: V3) -> V3 {
	let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
	if len > 1e-12 {
		[v[0] / len, v[1] / len, v[2] / len]
	} else {
		v
	}
}

#[cfg(test)]
mod tests {
	use super::super::fixtures::{asset, cf};
	use super::*;
	use crate::anim::rig::Binding;
	use crate::anim::schema::{AnimatedModel, BoneInfo, ModelPart, MotorJoint};

	/// Two parts on a joint; a skinned mesh on the root with one vertex on
	/// each bone, named after the parts as R15 meshes are.
	fn two_part_rig() -> (Rig, AnimatedModel) {
		let model = AnimatedModel {
			name: "Arm".into(),
			path: "Workspace/Arm".into(),
			scale: None,
			clothing: None,
			parts: vec![
				ModelPart {
					path: "Workspace/Arm/Upper".into(),
					name: "Upper".into(),
					cframe: cf([0.0, 0.0, 0.0]),
					size: [1.0, 1.0, 1.0],
					..Default::default()
				},
				ModelPart {
					path: "Workspace/Arm/Lower".into(),
					name: "Lower".into(),
					cframe: cf([0.0, -1.0, 0.0]),
					size: [1.0, 1.0, 1.0],
					..Default::default()
				},
			],
			joints: vec![MotorJoint {
				name: "Elbow".into(),
				path: "Workspace/Arm/Lower/Elbow".into(),
				part0: "Workspace/Arm/Upper".into(),
				part1: "Workspace/Arm/Lower".into(),
				c0: cf([0.0, -0.5, 0.0]),
				c1: cf([0.0, 0.5, 0.0]),
				transform: None,
				kind: None,
			}],
			bones: vec![BoneInfo {
				name: "Tip".into(),
				path: "Workspace/Arm/Lower/Tip".into(),
				parent_path: "Workspace/Arm/Lower".into(),
				cframe: cf([0.0, -0.5, 0.0]),
			}],
			animations: vec![],
		};
		(Rig::build(&model), model)
	}

	#[test]
	fn skinned_vertices_follow_matched_parts_and_bones() {
		let (rig, _) = two_part_rig();
		let rest = rig.rest();
		let skin = Skin {
			bones: vec!["Upper".into(), "Lower".into(), "Tip".into()],
			parents: vec![None, Some(0), Some(1)],
			binds: vec![cf([0.0; 3]); 3],
			influences: vec![
				[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
				[(1, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
				[(2, 0.5), (1, 0.5), (0, 0.0), (0, 0.0)],
			],
			facs: None,
		};
		let a = asset(
			vec![[0.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, -1.5, 0.0]],
			skin.clone(),
		);
		let mut ps = PartSkin::build(&skin, &a, &rig, &rest, 0, [0.0; 3], [1.0; 3]);
		assert_eq!(ps.matched, 3);
		// at rest nothing moves
		assert_eq!(ps.geometry.positions[1], [0.0, -1.0, 0.0]);
		// bend the elbow 90° about X: the lower part swings towards +Z or -Z
		let motor_t = vec![CFrame::axis_rotation(0, std::f32::consts::FRAC_PI_2)];
		let binding = Binding {
			targets: vec![Some(crate::anim::rig::Target::Motor(0))],
			unmatched: vec![],
		};
		let posed = rig.pose_from(&binding, &motor_t);
		ps.update(&rest, &posed, 0, &HashMap::new(), &[]);
		let p = ps.geometry.positions;
		assert!(
			(p[0][1] - 0.0).abs() < 1e-5,
			"upper vertex stays: {:?}",
			p[0]
		);
		// the elbow is at y = -0.5; the lower vertex was 0.5 below it
		assert!(
			(p[1][1] + 0.5).abs() < 1e-5 && (p[1][2].abs() - 0.5).abs() < 1e-5,
			"{:?}",
			p[1]
		);
	}

	#[test]
	fn mesh_only_bones_use_bind_pose_and_named_tracks() {
		let (rig, _) = two_part_rig();
		let rest = rig.rest();
		// "Jaw" is not in the model: rebuilt under "Upper" from bind poses
		let skin = Skin {
			bones: vec!["Upper".into(), "Jaw".into()],
			parents: vec![None, Some(0)],
			binds: vec![cf([0.0; 3]), cf([0.0, 0.0, 1.0])],
			influences: vec![[(1, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]; 3],
			facs: None,
		};
		let a = asset(vec![[0.0, 0.0, 2.0]; 3], skin.clone());
		let mut ps = PartSkin::build(&skin, &a, &rig, &rest, 0, [0.0; 3], [1.0; 3]);
		assert_eq!(ps.matched, 1);
		assert_eq!(ps.geometry.positions[0], [0.0, 0.0, 2.0]);
		// a track named Jaw moving it up by 1
		let by_name = HashMap::from([("Jaw".to_string(), CFrame::from_position([0.0, 1.0, 0.0]))]);
		ps.update(&rest, &rest, 0, &by_name, &[]);
		let p = ps.geometry.positions[0];
		assert!(
			(p[1] - 1.0).abs() < 1e-5 && (p[2] - 2.0).abs() < 1e-5,
			"{p:?}"
		);
	}

	#[test]
	fn bone_translations_scale_with_their_mesh() {
		// a pet modelled in centimetres: -16 mesh units is 8 cm at scale 0.005
		let t = CFrame {
			pos: [0.0, -16.0, 0.0],
			rot: CFrame::axis_rotation(0, 0.3).rot,
		};
		let scaled = scale_translation(t, 0.005);
		assert!((scaled.pos[1] + 0.08).abs() < 1e-6);
		assert_eq!(scaled.rot, t.rot, "rotation is unchanged");
	}

	/// Skinning's normalise leaves vectors of length 1e-12 or less (and
	/// NaN) as they are. The viewport's own normalise uses `> 0.0` instead;
	/// the two differ only below this threshold.
	#[test]
	fn normalize_keeps_tiny_vectors() {
		assert_eq!(normalize([3.0, 0.0, 4.0]), [0.6, 0.0, 0.8]);
		assert_eq!(normalize([0.0; 3]), [0.0; 3]);
		assert_eq!(normalize([1e-13, 0.0, 0.0]), [1e-13, 0.0, 0.0]);
		// just over the threshold it normalises
		let n = normalize([1e-11, 0.0, 0.0]);
		assert!(
			(n[0] - 1.0).abs() < 1e-6 && n[1] == 0.0 && n[2] == 0.0,
			"{n:?}"
		);
		let nan = normalize([f32::NAN, 1.0, 0.0]);
		assert!(nan[0].is_nan() && nan[1] == 1.0 && nan[2] == 0.0);
	}

	/// The skinning threshold is strict: a length of exactly 1e-12 is kept.
	#[test]
	fn normalize_keeps_a_vector_of_exactly_the_threshold() {
		let v = [1e-12_f32, 0.0, 0.0];
		let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
		assert_eq!(len, 1e-12, "the length must be exactly the threshold");
		assert_eq!(normalize(v), v);
	}
}
