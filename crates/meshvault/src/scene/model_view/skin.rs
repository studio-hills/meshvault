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
	pub(super) geometry: Geometry,
	pub(super) moved: bool,
	/// Scale for clip translations of mesh-only bones: the part's mesh
	/// scale, or the model's scale when the export has one.
	pub(super) track_scale: f32,
	/// The mesh's FACS poses, which move its face bones by face control.
	facs: Option<geometry::Facs>,
	/// Mesh units to studs for FACS offsets: always the part's mesh scale.
	facs_scale: f32,
	/// How many of the mesh's bones matched a Bone or part in the model.
	pub matched: usize,
	pub bones: usize,
}

/// A mesh point fitted to its part (moved off `centre`, then scaled) and
/// placed by `cf`.
pub(super) fn fit_point(cf: &CFrame, scale: V3, centre: V3, v: V3) -> V3 {
	cf.point([
		(v[0] - centre[0]) * scale[0],
		(v[1] - centre[1]) * scale[1],
		(v[2] - centre[2]) * scale[2],
	])
}

impl PartSkin {
	/// Binds the mesh of part `owner`, fitted to it as `fit` (the part's
	/// mesh centre, scale and SpecialMesh offset) says.
	pub(super) fn build(
		skin: &Skin,
		asset: &MeshAsset,
		rig: &Rig,
		rest: &Posed,
		owner: usize,
		(centre, scale, offset): (V3, V3, V3),
	) -> Self {
		let near = rig.parts[owner].path.as_str();
		// a bind pose moved into part space, like the vertices
		let fit = |c: &[f32; 12]| {
			let b = CFrame::from_components(c);
			CFrame {
				pos: [
					(b.pos[0] - centre[0]) * scale[0] + offset[0],
					(b.pos[1] - centre[1]) * scale[1] + offset[1],
					(b.pos[2] - centre[2]) * scale[2] + offset[2],
				],
				rot: b.rot,
			}
		};
		let mut matched = 0;
		let mut drivers: Vec<Driver> = skin
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
		let order = solve_order(&mut drivers, |k| fit(&skin.binds[k]));
		let owner_rest = rest.parts[owner];
		let g = &asset.geometry;
		// SpecialMesh.Offset moves the mesh in the part's own space
		let placed = owner_rest.mul(&CFrame::from_position(offset));
		let rest_positions = g
			.positions
			.iter()
			.map(|&v| fit_point(&placed, scale, centre, v))
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
			order,
			influences: skin.influences.clone(),
			rest_positions,
			rest_normals,
			geometry: g.clone(),
			moved: true,
			track_scale: mean(scale),
			facs: skin.facs.clone(),
			facs_scale: mean(scale),
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
					let pos = [o[0], o[1], o[2]].map(|v| v * self.facs_scale);
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
						None => by_name.get(name.as_str()).map_or(CFrame::IDENTITY, |&t| {
							scale_translation(t, self.track_scale)
						}),
					};
					(pn.mul(local).mul(&t), pr.mul(local))
				}
			};
		}
		(now, then)
	}
}

/// Orders the drivers parents first, visiting each once. A mesh file can
/// loop its bones' parents back to themselves: as the rig does for Bones,
/// walking up from the first bone in the loop, the last bone before the
/// walk comes back drops its parent and is placed from the part, at its
/// bind pose (`own(k)`, in part space).
fn solve_order(drivers: &mut [Driver], own: impl Fn(usize) -> CFrame) -> Vec<usize> {
	#[derive(Clone, Copy, PartialEq)]
	enum Seen {
		No,
		OnWalk,
		Ordered,
	}
	let mut seen = vec![Seen::No; drivers.len()];
	let mut order = Vec::with_capacity(drivers.len());
	let mut walk = Vec::new();
	for start in 0..drivers.len() {
		let mut k = start;
		while seen[k] == Seen::No {
			seen[k] = Seen::OnWalk;
			walk.push(k);
			match &drivers[k] {
				Driver::Mesh {
					parent: Some(p), ..
				} => k = *p,
				_ => break,
			}
		}
		if seen[k] == Seen::OnWalk
			&& let Some(&last) = walk.last()
			&& let Driver::Mesh { parent, local, .. } = &mut drivers[last]
			&& *parent == Some(k)
		{
			*parent = None;
			*local = own(last);
		}
		// the walk went up from `start`, so its top is solved first
		for &k in walk.iter().rev() {
			seen[k] = Seen::Ordered;
			order.push(k);
		}
		walk.clear();
	}
	order
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
	use crate::anim::schema::{AnimatedModel, BoneInfo, JointInfo, ModelPart};

	/// Two parts on a joint; a skinned mesh on the root with one vertex on
	/// each bone, named after the parts as R15 meshes are.
	fn two_part_rig() -> (Rig, AnimatedModel) {
		let model = AnimatedModel {
			name: "Arm".into(),
			path: "Workspace/Arm".into(),
			scale: None,
			clothing: None,
			pivot: None,
			primary_part_path: None,
			attachments: Vec::new(),
			effects: Vec::new(),
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
			joints: vec![JointInfo {
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
		let mut ps = PartSkin::build(&skin, &a, &rig, &rest, 0, ([0.0; 3], [1.0; 3], [0.0; 3]));
		assert_eq!(ps.matched, 3);
		// at rest nothing moves
		assert_eq!(ps.geometry.positions[1], [0.0, -1.0, 0.0]);
		// bend the elbow 90° about X: the lower part swings towards +Z or -Z
		let joint_t = vec![CFrame::axis_rotation(0, std::f32::consts::FRAC_PI_2)];
		let binding = Binding {
			targets: vec![Some(crate::anim::rig::Target::Joint(0))],
			unmatched: vec![],
		};
		let posed = rig.pose_from(&binding, &joint_t);
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

	/// A posed skinned mesh moves its vertices and keeps their colours.
	#[test]
	fn skinned_meshes_keep_their_vertex_colours() {
		let (rig, _) = two_part_rig();
		let rest = rig.rest();
		let skin = Skin {
			bones: vec!["Upper".into(), "Lower".into()],
			parents: vec![None, Some(0)],
			binds: vec![cf([0.0; 3]); 2],
			influences: vec![[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]; 3]
				.into_iter()
				.enumerate()
				.map(|(i, mut w)| {
					w[0].0 = (i > 0) as u16;
					w
				})
				.collect(),
			facs: None,
		};
		let mut a = asset(
			vec![[0.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, -1.5, 0.0]],
			skin.clone(),
		);
		let colors = vec![[102, 115, 72, 255], [16, 21, 29, 255], [194, 205, 156, 255]];
		a.geometry.colors = Some(colors.clone());
		let mut ps = PartSkin::build(&skin, &a, &rig, &rest, 0, ([0.0; 3], [1.0; 3], [0.0; 3]));
		let binding = Binding {
			targets: vec![Some(crate::anim::rig::Target::Joint(0))],
			unmatched: vec![],
		};
		let joint_t = vec![CFrame::axis_rotation(0, std::f32::consts::FRAC_PI_2)];
		let posed = rig.pose_from(&binding, &joint_t);
		ps.update(&rest, &posed, 0, &HashMap::new(), &[]);
		assert_ne!(
			ps.geometry.positions[1],
			[0.0, -1.0, 0.0],
			"the lower vertex moved"
		);
		assert_eq!(ps.geometry.colors, Some(colors));
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
		let mut ps = PartSkin::build(&skin, &a, &rig, &rest, 0, ([0.0; 3], [1.0; 3], [0.0; 3]));
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

	/// A SpecialMesh's Offset moves a skinned mesh in its part's space, as
	/// it does an unskinned one: the vertices, and the bones only the mesh
	/// has, so a named track still moves the vertices from there.
	#[test]
	fn skinned_meshes_keep_their_special_mesh_offset() {
		let (rig, _) = two_part_rig();
		let rest = rig.rest();
		let skin = Skin {
			bones: vec!["Upper".into(), "Jaw".into()],
			parents: vec![None, None],
			binds: vec![cf([0.0; 3]), cf([0.0, 0.0, 1.0])],
			influences: vec![
				[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
				[(1, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
				[(1, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
			],
			facs: None,
		};
		let a = asset(
			vec![[0.0, 0.0, 0.0], [0.0, 0.0, 2.0], [1.0, 0.0, 2.0]],
			skin.clone(),
		);
		let offset = [0.5, 0.25, -1.0];
		let mut ps = PartSkin::build(&skin, &a, &rig, &rest, 0, ([0.0; 3], [2.0; 3], offset));
		// placed as an unskinned mesh with the same fit is
		let in_part = rest.parts[0].mul(&CFrame::from_position(offset));
		let unskinned: Vec<V3> = (a.geometry.positions.iter())
			.map(|&v| fit_point(&in_part, [2.0; 3], [0.0; 3], v))
			.collect();
		assert_eq!(unskinned[1], [0.5, 0.25, 3.0]);
		assert_eq!(ps.rest_positions, unskinned);
		assert_eq!(ps.geometry.positions, unskinned);
		// the mesh-only Jaw is placed from its bind pose in the offset part
		let Driver::Mesh { local, .. } = &ps.drivers[1] else {
			panic!("Jaw is only in the mesh");
		};
		assert_eq!(local.pos, [0.5, 0.25, 1.0]);
		let by_name = HashMap::from([("Jaw".to_string(), CFrame::from_position([0.0, 1.0, 0.0]))]);
		ps.update(&rest, &rest, 0, &by_name, &[]);
		let p = ps.geometry.positions[1];
		// a track's movement is in mesh units, scaled by the mesh (2)
		assert!(
			(p[0] - 0.5).abs() < 1e-5 && (p[1] - 2.25).abs() < 1e-5 && (p[2] - 3.0).abs() < 1e-5,
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
		// the threshold is strict: a length of exactly 1e-12 is kept
		let v = [1e-12_f32, 0.0, 0.0];
		let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
		assert_eq!(len, 1e-12, "the length must be exactly the threshold");
		assert_eq!(normalize(v), v);
		// just over the threshold it normalises
		let n = normalize([1e-11, 0.0, 0.0]);
		assert!(
			(n[0] - 1.0).abs() < 1e-6 && n[1] == 0.0 && n[2] == 0.0,
			"{n:?}"
		);
		let nan = normalize([f32::NAN, 1.0, 0.0]);
		assert!(nan[0].is_nan() && nan[1] == 1.0 && nan[2] == 0.0);
	}

	/// A skin of `parents.len()` bones only the mesh has, each a step of 1
	/// above the last in its bind pose, bound to part 0 of the two-part rig.
	fn mesh_only(parents: Vec<Option<u16>>) -> PartSkin {
		let (rig, _) = two_part_rig();
		let rest = rig.rest();
		let n = parents.len();
		let skin = Skin {
			bones: vec!["Loose".into(); n],
			parents,
			binds: (0..n).map(|k| cf([0.0, k as f32, 0.0])).collect(),
			influences: vec![[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]; 3],
			facs: None,
		};
		let a = asset(vec![[0.0; 3]; 3], skin.clone());
		let start = std::time::Instant::now();
		let ps = PartSkin::build(&skin, &a, &rig, &rest, 0, ([0.0; 3], [1.0; 3], [0.0; 3]));
		let took = start.elapsed();
		assert!(took.as_secs_f32() < 1.0, "binding {n} bones took {took:?}");
		ps
	}

	/// Where each driver is placed at rest.
	fn placed(ps: &PartSkin) -> Vec<CFrame> {
		let (rig, _) = two_part_rig();
		let rest = rig.rest();
		ps.drivers_at(&rest, &rest, 0, &HashMap::new(), &[]).0
	}

	fn parent_of(ps: &PartSkin, k: usize) -> Option<usize> {
		match &ps.drivers[k] {
			Driver::Mesh { parent, .. } => *parent,
			_ => panic!("bone {k} is only in the mesh"),
		}
	}

	/// Mesh bone parents that loop are broken once, as the rig breaks
	/// looping Bones: walking up from the first bone in the loop, the bone
	/// that closes it is placed from the part. A bone its own parent, with
	/// every other bone under it, binds in one pass.
	#[test]
	fn mesh_bone_parent_loops_are_broken_quickly() {
		const N: usize = 65_535;
		let ps = mesh_only(vec![Some(0); N]);
		assert_eq!(parent_of(&ps, 0), None);
		assert!((1..N).all(|k| parent_of(&ps, k) == Some(0)));
		let at = placed(&ps);
		assert_eq!(at[0].pos, [0.0, 0.0, 0.0]);
		assert!((at[N - 1].pos[1] - (N - 1) as f32).abs() < 1e-2);
		// B0 -> B1 -> B2 -> B0, and B3 under B2: B2 closes the loop
		let ps = mesh_only(vec![Some(1), Some(2), Some(0), Some(2)]);
		let parents: Vec<_> = (0..4).map(|k| parent_of(&ps, k)).collect();
		assert_eq!(parents, [Some(1), Some(2), None, Some(2)]);
		let order = &ps.order;
		let at = |b: usize| order.iter().position(|&o| o == b).unwrap();
		assert!(at(2) < at(1) && at(1) < at(0) && at(2) < at(3), "{order:?}");
		// each bone stays at its bind pose, the loop's closer from the part
		let p = placed(&ps);
		for (k, cf) in p.iter().enumerate() {
			assert!((cf.pos[1] - k as f32).abs() < 1e-5, "{k}: {:?}", cf.pos);
		}
	}

	/// A long chain of mesh bones is ordered parents first in one pass.
	#[test]
	fn long_mesh_bone_chains_bind_quickly() {
		const N: usize = 65_535;
		// bone k under k + 1, so the root is last
		let parents = (0..N)
			.map(|k| (k + 1 < N).then(|| (k + 1) as u16))
			.collect();
		let ps = mesh_only(parents);
		assert_eq!(ps.order.len(), N);
		assert_eq!(ps.order[0], N - 1);
		let p = placed(&ps);
		assert!((p[0].pos[1]).abs() < 1e-2 && (p[N - 1].pos[1] - (N - 1) as f32).abs() < 1e-2);
	}
}
