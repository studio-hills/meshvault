//! An animated model open in the viewer: its parts placed by the rig, the
//! clips it can play, and skinned meshes deformed by their bones.
//!
//! Parts take their meshes from the workspace. A mesh is centred and scaled
//! to the part's `Size`, as Roblox does for MeshParts. A skinned mesh is
//! deformed on the CPU: every bone it was bound to is matched by name to a
//! Bone in the model, else to a part (R15 bodies bind to their parts), else
//! rebuilt from the bind pose stored in the mesh file. Each vertex then
//! follows `world now * world at rest⁻¹` of its bones.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use eframe::egui::{self, Color32};

use crate::anim::clip::{Clip, ClipMarker};
use crate::anim::math::{CFrame, V3};
use crate::anim::rig::{Binding, BoneParent, Posed, Rig, Target};
use crate::anim::schema::{AnimatedModel, ClipStatus, ModelPart};
use crate::render::gpu::GpuScene;
use crate::render::viewport::{
	self, AlphaMode, Camera, ClothingDraw, DecalDraw, ScenePart, Surface,
};
use crate::scene::geometry::{self, Geometry, GroupKind, Primitive, Skin};
use crate::scene::maps::Slot;
use crate::scene::texture::{self, Material};

/// The files of one mesh a model uses, found in the workspace.
pub struct MeshSource {
	/// Item name, for messages.
	pub name: String,
	/// The downloaded mesh, or why there is none.
	pub mesh: Result<PathBuf, String>,
	pub maps: Vec<(Slot, PathBuf)>,
}

/// One mesh, shared by every part that uses it.
pub struct MeshAsset {
	pub name: String,
	pub geometry: Geometry,
	/// Groups drawn: the most detailed LOD.
	pub groups: Vec<usize>,
	/// `Ok(None)` when the mesh is not skinned.
	pub skin: Result<Option<Skin>, String>,
	pub material: Material,
}

pub enum PartShape {
	Mesh(Arc<MeshAsset>),
	/// A part without a file mesh: a block, ball, cylinder or wedge.
	Primitive(Primitive),
	/// Not drawn, and why.
	Missing(String),
}

/// How a part is drawn, decided from its class, shape and SpecialMesh.
#[derive(Clone, Debug, PartialEq)]
enum Look {
	/// Its file mesh. A MeshPart fits it to its size; a SpecialMesh
	/// instead scales the mesh's own units and moves it (scale, offset).
	Mesh {
		special: Option<(V3, V3)>,
	},
	Primitive {
		kind: Primitive,
		size: V3,
		offset: V3,
		/// Set when Meshvault has no geometry for the real shape.
		approximate: Option<String>,
	},
	Missing(String),
}

/// A SpecialMesh head's size relative to its part's size times Scale.
const HEAD_SIZE: f32 = 0.94;

fn look(part: &ModelPart) -> Look {
	let size = part.size;
	let special = part.special_mesh.as_ref();
	if part.mesh_ref != 0 {
		return Look::Mesh {
			special: special.map(|s| (s.scale, s.offset)),
		};
	}
	let primitive = |kind, size, offset, approximate: Option<&str>| Look::Primitive {
		kind,
		size,
		offset,
		approximate: approximate.map(String::from),
	};
	if let Some(s) = special {
		// procedural SpecialMesh types stretch to the part's size times Scale
		let scaled = [
			size[0] * s.scale[0],
			size[1] * s.scale[1],
			size[2] * s.scale[2],
		];
		return match s.mesh_type.as_str() {
			"FileMesh" => Look::Missing("its SpecialMesh's file mesh was not exported".into()),
			// a torso is a plain box too (checked in Studio)
			"Brick" | "Torso" => primitive(Primitive::Block, scaled, s.offset, None),
			"Sphere" => primitive(Primitive::Ball, scaled, s.offset, None),
			// like a cylinder part (checked in Studio): along X, as wide as the
			// smaller of the other two sides
			"Cylinder" => {
				let d = scaled[1].min(scaled[2]);
				primitive(Primitive::CylinderX, [scaled[0], d, d], s.offset, None)
			}
			"Wedge" => primitive(Primitive::Wedge, scaled, s.offset, None),
			"CornerWedge" => primitive(Primitive::CornerWedge, scaled, s.offset, None),
			// measured in Studio: 94% of the scaled size, upright, and as wide
			// as the narrower of the scaled X and Z
			"Head" => {
				let d = scaled[0].min(scaled[2]) * HEAD_SIZE;
				primitive(
					Primitive::Head,
					[d, scaled[1] * HEAD_SIZE, d],
					s.offset,
					None,
				)
			}
			other => primitive(
				Primitive::Block,
				scaled,
				s.offset,
				Some(&format!("a {other} SpecialMesh (drawn as a box)")),
			),
		};
	}
	let shape = part.shape.as_deref().unwrap_or("");
	match (part.class_name.as_deref(), shape) {
		// schema 1 records neither: a box is the best guess
		(None, _) | (_, "Block") => primitive(Primitive::Block, size, [0.0; 3], None),
		(_, "Ball") => {
			let d = size[0].min(size[1]).min(size[2]);
			primitive(Primitive::Ball, [d; 3], [0.0; 3], None)
		}
		(_, "Cylinder") => {
			let d = size[1].min(size[2]);
			primitive(Primitive::CylinderX, [size[0], d, d], [0.0; 3], None)
		}
		(_, "Wedge") | (Some("WedgePart"), _) => primitive(Primitive::Wedge, size, [0.0; 3], None),
		(_, "CornerWedge") | (Some("CornerWedgePart"), _) => {
			primitive(Primitive::CornerWedge, size, [0.0; 3], None)
		}
		(Some("MeshPart"), _) => Look::Missing("its mesh was not exported".into()),
		(_, "Truss") | (Some("TrussPart"), _) => {
			// 2-stud segments along its longest side
			let axis = (0..3)
				.max_by(|&a, &b| size[a].total_cmp(&size[b]))
				.unwrap_or(1);
			let segments = (size[axis] / 2.0).round().clamp(1.0, 4096.0) as u16;
			primitive(
				Primitive::Truss {
					segments,
					axis: axis as u8,
				},
				size,
				[0.0; 3],
				None,
			)
		}
		(Some(class), _) => primitive(
			Primitive::Block,
			size,
			[0.0; 3],
			Some(&format!("a {class} (drawn as a box)")),
		),
	}
}

/// e.g. "MeshPart · Plastic · Bright violet #6B32A8".
fn appearance(part: &ModelPart) -> Option<String> {
	let mut bits = Vec::new();
	bits.extend(part.class_name.clone());
	bits.extend(part.material.clone());
	if part.color.is_some() {
		let hex = colour_of(part, Color32::BLACK);
		let name = part.brick_color.clone().unwrap_or_default();
		bits.push(
			format!("{name} #{:02X}{:02X}{:02X}", hex.r(), hex.g(), hex.b())
				.trim()
				.to_string(),
		);
	}
	if let Some(r) = part.reflectance.filter(|r| *r > 0.0) {
		bits.push(format!("reflectance {r:.2}"));
	}
	(!bits.is_empty()).then(|| bits.join(" · "))
}

fn surface_of(part: &ModelPart) -> Surface {
	Surface {
		opacity: 1.0 - part.transparency.clamp(0.0, 1.0),
		neon: part.material.as_deref() == Some("Neon"),
		reflectance: part.reflectance.unwrap_or(0.0).clamp(0.0, 1.0),
		metal: matches!(
			part.material.as_deref(),
			Some("Metal" | "DiamondPlate" | "CorrodedMetal" | "Foil")
		),
		tint: part
			.surface_appearance
			.as_ref()
			.map_or([1.0; 3], |s| s.color),
		alpha_mode: part
			.surface_appearance
			.as_ref()
			.map_or(AlphaMode::Overlay, |s| AlphaMode::from_name(&s.alpha_mode)),
	}
}

fn colour_of(part: &ModelPart, fallback: Color32) -> Color32 {
	part.color.map_or(fallback, |c| {
		let u = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
		Color32::from_rgb(u(c[0]), u(c[1]), u(c[2]))
	})
}

pub struct PartView {
	/// Index into the rig's parts.
	pub index: usize,
	pub shape: PartShape,
	pub visible: bool,
	/// Mesh centre and per-axis scale fitting the mesh to the part's size,
	/// then an offset in the part's space (SpecialMesh.Offset).
	centre: V3,
	scale: V3,
	offset: V3,
	/// `BasePart.Color`, shown where there is no colour map.
	pub colour: Color32,
	pub surface: Surface,
	/// Why the part is drawn as an approximation, if it is.
	pub approximate: Option<String>,
	/// Class, material and colour, when the export has them (schema 2).
	pub appearance: Option<String>,
	pub skin: Option<PartSkin>,
	gpu: Option<GpuScene>,
	/// Decals and Textures drawn over the part, lowest ZIndex first.
	decals: Vec<PartDecal>,
	/// The part's enabled lights.
	lights: Vec<crate::anim::schema::LightInfo>,
}

/// A Decal or Texture of a part, with its image loaded.
struct PartDecal {
	face: u8,
	tiles: Option<[f32; 4]>,
	colour: [f32; 3],
	transparency: f32,
	/// The image, as the colour map (a MaterialVariant's maps fill their slots).
	material: Material,
	gpu: Option<GpuScene>,
	/// A MaterialVariant over the whole part, drawn before its decals.
	variant: bool,
	/// Clothing: how the part's body section wears the template, and where
	/// the part sits in that section.
	clothing: Option<(ClothingDraw, CFrame)>,
}

/// The classic clothing layout: which body sections there are, the parts
/// of each (R15 pieces, or the R6 part), and whether the shirt or the
/// pants covers it.
const SECTIONS: [(&[&str], Section); 5] = [
	(&["UpperTorso", "LowerTorso", "Torso"], Section::Torso),
	(
		&["RightUpperArm", "RightLowerArm", "RightHand", "Right Arm"],
		Section::RightArm,
	),
	(
		&["LeftUpperArm", "LeftLowerArm", "LeftHand", "Left Arm"],
		Section::LeftArm,
	),
	(
		&["RightUpperLeg", "RightLowerLeg", "RightFoot", "Right Leg"],
		Section::RightLeg,
	),
	(
		&["LeftUpperLeg", "LeftLowerLeg", "LeftFoot", "Left Leg"],
		Section::LeftLeg,
	),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
	Torso,
	RightArm,
	LeftArm,
	RightLeg,
	LeftLeg,
}

impl Section {
	/// Each box face's region of the 585 x 559 template (pixels), in
	/// `Enum.NormalId` order: Right (+X), Top, Back, Left (-X), Bottom,
	/// Front. The template unfolds each section's box around its front.
	fn regions(self) -> [[f32; 4]; 6] {
		let torso = [
			[165.0, 74.0, 64.0, 128.0],
			[231.0, 8.0, 128.0, 64.0],
			[427.0, 74.0, 128.0, 128.0],
			[361.0, 74.0, 64.0, 128.0],
			[231.0, 204.0, 128.0, 64.0],
			[231.0, 74.0, 128.0, 128.0],
		];
		let right = [
			[151.0, 355.0, 64.0, 128.0],
			[217.0, 289.0, 64.0, 64.0],
			[85.0, 355.0, 64.0, 128.0],
			[19.0, 355.0, 64.0, 128.0],
			[217.0, 485.0, 64.0, 64.0],
			[217.0, 355.0, 64.0, 128.0],
		];
		let left = [
			[506.0, 355.0, 64.0, 128.0],
			[308.0, 289.0, 64.0, 64.0],
			[440.0, 355.0, 64.0, 128.0],
			[374.0, 355.0, 64.0, 128.0],
			[308.0, 485.0, 64.0, 64.0],
			[308.0, 355.0, 64.0, 128.0],
		];
		let pixels = match self {
			Section::Torso => torso,
			Section::RightArm | Section::RightLeg => right,
			Section::LeftArm | Section::LeftLeg => left,
		};
		pixels.map(|[x, y, w, h]| [x / 585.0, y / 559.0, w / 585.0, h / 559.0])
	}
}

/// `a * b` for row-major 4 x 4 matrices.
fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let mut out = [[0.0; 4]; 4];
	for (r, row) in out.iter_mut().enumerate() {
		for (c, cell) in row.iter_mut().enumerate() {
			*cell = (0..4).map(|k| a[r][k] * b[k][c]).sum();
		}
	}
	out
}

/// `Enum.NormalId` value of a face name.
fn face_index(name: &str) -> u8 {
	match name {
		"Right" => 0,
		"Top" => 1,
		"Back" => 2,
		"Left" => 3,
		"Bottom" => 4,
		_ => 5,
	}
}

/// What moves a bone a mesh was bound to.
#[derive(Clone, Debug)]
enum Driver {
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
	drivers: Vec<Driver>,
	/// Drivers with their parents first.
	order: Vec<usize>,
	influences: Vec<[(u16, f32); 4]>,
	rest_positions: Vec<V3>,
	rest_normals: Option<Vec<V3>>,
	/// World-space vertices in the current pose.
	pub geometry: Geometry,
	moved: bool,
	/// The part's mesh scale, applied to clip translations of mesh-only bones.
	unit_scale: f32,
	/// The mesh's FACS poses, which move its face bones by face control.
	facs: Option<geometry::Facs>,
	/// Mesh units to studs, for FACS offsets.
	mesh_scale: f32,
	/// How many of the mesh's bones matched a Bone or part in the model.
	pub matched: usize,
	pub bones: usize,
}

pub struct ClipSlot {
	pub name: String,
	pub status: ClipStatus,
	pub priority: String,
	pub animation_id: String,
	/// The playable clip, or why it is not playable.
	pub clip: Result<Clip, String>,
	pub binding: Binding,
	/// Tracks for bones that exist only inside skinned meshes.
	pub mesh_bone_tracks: usize,
}

pub struct ModelView {
	/// Index of the model in the workspace.
	pub index: usize,
	pub name: String,
	pub path: String,
	pub rig: Rig,
	rest: Posed,
	posed: Posed,
	pub clips: Vec<ClipSlot>,
	pub current: Option<usize>,
	pub time: f32,
	pub playing: bool,
	pub looping: bool,
	pub speed: f32,
	pub parts: Vec<PartView>,
	pub camera: Camera,
	bounds: Option<(V3, V3)>,
	pub warnings: Vec<String>,
	pub show_blocks: bool,
	/// `Model:GetScale()` (schema 2).
	pub scale: Option<f32>,
	/// Whether joints recorded the pose they were exported in (schema 2).
	pub captured_pose: bool,
	/// Scale applied to clip translations of each joint and Bone; see
	/// [`scale_translation`].
	motor_scale: Vec<f32>,
	bone_scale: Vec<f32>,
	/// The pose needs solving again (time, clip or rest changed).
	dirty: bool,
	/// The part tinted to stand out (hovered in the parts list).
	pub highlight: Option<usize>,
}

/// Parts this transparent are invisible in Roblox (HumanoidRootPart).
const INVISIBLE: f32 = 0.99;
const BLOCK_COLOUR: Color32 = Color32::from_rgb(140, 146, 156);

fn load_asset(ctx: &egui::Context, source: &MeshSource) -> Result<MeshAsset, String> {
	let path = source.mesh.as_ref().map_err(Clone::clone)?;
	let bytes =
		std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
	let loaded = geometry::load(&bytes)?;
	let geometry = loaded.geometry;
	if geometry.triangles.is_empty() {
		return Err(loaded.note.unwrap_or_else(|| "the mesh is empty".into()));
	}
	let groups = match geometry.group_kind {
		GroupKind::Lod => vec![0],
		_ => (0..geometry.groups.len()).collect(),
	};
	let skin = geometry::skin(&bytes, geometry.positions.len());
	let mut material = Material::default();
	if geometry.uvs.is_some() {
		for (slot, map) in &source.maps {
			// a missing map leaves the mesh plain rather than failing the part
			if let Ok(texture) = std::fs::read(map)
				.map_err(|e| e.to_string())
				.and_then(|b| texture::load(ctx, map, &b))
			{
				material.set(*slot, texture);
			}
		}
	}
	Ok(MeshAsset {
		name: source.name.clone(),
		geometry,
		groups,
		skin,
		material,
	})
}

/// `transform * diag(scale) * translate(-centre)` as a row-major matrix.
fn placement(cf: &CFrame, scale: V3, centre: V3) -> [[f32; 4]; 4] {
	let mut m = cf.to_matrix(scale);
	for row in m.iter_mut().take(3) {
		row[3] -= row[0] * centre[0] + row[1] * centre[1] + row[2] * centre[2];
	}
	m
}

fn fitted(cf: &CFrame, scale: V3, centre: V3, v: V3) -> V3 {
	cf.point([
		(v[0] - centre[0]) * scale[0],
		(v[1] - centre[1]) * scale[1],
		(v[2] - centre[2]) * scale[2],
	])
}

impl PartSkin {
	fn build(
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
	fn update(
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
	fn drivers_at(
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

fn mean(v: V3) -> f32 {
	(v[0] + v[1] + v[2]) / 3.0
}

/// A bone's animated offset in the units of the mesh it deforms. Roblox
/// keeps skinning in mesh space and scales the result to the part's size,
/// so when a mesh made at another scale is resized (a pet modelled in
/// centimetres shrunk to 3 studs) its clips' translations shrink with it.
/// Without this, such clips throw the model metres off its spot.
fn scale_translation(mut t: CFrame, scale: f32) -> CFrame {
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

fn normalize(v: V3) -> V3 {
	let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
	if len > 1e-12 {
		[v[0] / len, v[1] / len, v[2] / len]
	} else {
		v
	}
}

impl ModelView {
	/// Builds the view, reading and decoding every mesh and map. Slow for
	/// big models, so it runs on a background thread.
	pub fn load(
		ctx: &egui::Context,
		index: usize,
		model: &AnimatedModel,
		sources: &HashMap<u32, MeshSource>,
		images: &HashMap<u64, PathBuf>,
	) -> Self {
		let mut warnings = Vec::new();
		let rig = Rig::build(model);
		warnings.extend(rig.warnings.iter().cloned());
		let rest = rig.rest();
		let rest_error = rig.rest_error();
		if rest_error > 0.01 {
			// schema 2 records each joint's pose, so a difference left over is in
			// the saved scene itself
			warnings.push(if model.joints.iter().any(|j| j.transform.is_some()) {
				format!(
					"The joints place parts up to {rest_error:.2} studs from where they were saved: the parts and their joints do not quite meet in the place (common with AnimationConstraint avatars, which Roblox pulls together once the game runs). Meshvault places them by the joints, as the running game does."
				)
			} else {
				format!(
					"The joints place parts up to {rest_error:.2} studs from where they were exported, so the model was probably captured mid-animation. Clips play from the joints' rest pose."
				)
			});
		}

		let mut assets: HashMap<u32, Result<Arc<MeshAsset>, String>> = HashMap::new();
		// decal images, each loaded once
		let mut decal_images: HashMap<u64, Option<texture::Texture>> = HashMap::new();
		let mut decals_missing = 0;
		// classic clothing: each body section's box in the joints' rest pose,
		// and where each of its parts sits in it
		let joint_rest = rig.joint_rest();
		let mut sections: HashMap<usize, (Section, CFrame, V3, V3)> = HashMap::new();
		let mut clothing_images: Vec<(Section, Material, [f32; 3], bool)> = Vec::new();
		if let Some(c) = &model.clothing {
			let mut load = |content: &Option<String>| -> Option<texture::Texture> {
				let id = crate::studio::export_file::parse_asset_id(content.as_deref()?)?;
				decal_images
					.entry(id)
					.or_insert_with(|| {
						let path = images.get(&id)?;
						let bytes = std::fs::read(path).ok()?;
						texture::load(ctx, path, &bytes).ok()
					})
					.clone()
			};
			let material = |t: texture::Texture| {
				let mut m = Material::default();
				m.set(Slot::Base, t);
				m
			};
			// drawn in this order: pants, then the shirt over them, then a T-shirt
			let pants = load(&c.pants_template);
			let shirt = load(&c.shirt_template);
			let graphic = load(&c.graphic);
			for (_, section) in SECTIONS {
				let legs = matches!(section, Section::RightLeg | Section::LeftLeg);
				let arms = matches!(section, Section::RightArm | Section::LeftArm);
				if let Some(t) = &pants
					&& !arms
				{
					clothing_images.push((
						section,
						material(t.clone()),
						c.pants_color.unwrap_or([1.0; 3]),
						false,
					));
				}
				if let Some(t) = &shirt
					&& !legs
				{
					clothing_images.push((
						section,
						material(t.clone()),
						c.shirt_color.unwrap_or([1.0; 3]),
						false,
					));
				}
			}
			if let Some(t) = graphic {
				clothing_images.push((
					Section::Torso,
					material(t),
					c.graphic_color.unwrap_or([1.0; 3]),
					true,
				));
			}
			let wanted = [&c.pants_template, &c.shirt_template, &c.graphic]
				.iter()
				.filter(|t| t.is_some())
				.count();
			let got = [
				pants.is_some(),
				shirt.is_some(),
				clothing_images.iter().any(|c| c.3),
			]
			.iter()
			.filter(|b| **b)
			.count();
			if got < wanted {
				warnings.push(
					"Some of the model's clothing is not drawn: its images are not downloaded yet."
						.into(),
				);
			}
			for (names, section) in SECTIONS {
				let members: Vec<usize> = names
					.iter()
					.filter_map(|n| rig.parts.iter().position(|p| p.name == *n))
					.collect();
				let Some(&first) = members.first() else {
					continue;
				};
				let frame = joint_rest.parts[first];
				let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
				for &m in &members {
					let in_frame = frame.inverse().mul(&joint_rest.parts[m]);
					let h = rig.parts[m].size.map(|s| s * 0.5);
					for corner in 0..8 {
						let p = in_frame.point([
							if corner & 1 == 0 { -h[0] } else { h[0] },
							if corner & 2 == 0 { -h[1] } else { h[1] },
							if corner & 4 == 0 { -h[2] } else { h[2] },
						]);
						for k in 0..3 {
							lo[k] = lo[k].min(p[k]);
							hi[k] = hi[k].max(p[k]);
						}
					}
				}
				let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5);
				let half = [0, 1, 2].map(|k| (hi[k] - lo[k]) * 0.5);
				for &m in &members {
					let in_frame = frame.inverse().mul(&joint_rest.parts[m]);
					sections.insert(m, (section, in_frame, centre, half));
				}
			}
		}
		let mut variants_missing = 0;
		let mut variants_on_meshes = 0;
		let mut parts = Vec::with_capacity(rig.parts.len());
		let mut missing = 0;
		for (i, part) in rig.parts.iter().enumerate() {
			// the rig keeps the parts in the record's order
			let record = &model.parts[i];
			let look = look(record);
			let shape = if let Look::Primitive { kind, .. } = look {
				PartShape::Primitive(kind)
			} else if let Look::Missing(why) = &look {
				missing += 1;
				PartShape::Missing(why.clone())
			} else {
				let asset = assets.entry(part.mesh_ref).or_insert_with(|| {
					match sources.get(&part.mesh_ref) {
						Some(source) => load_asset(ctx, source)
							.map(Arc::new)
							.map_err(|e| format!("{}: {e}", source.name)),
						None => Err("its mesh is not in this workspace".into()),
					}
				});
				match asset {
					Ok(asset) => PartShape::Mesh(asset.clone()),
					Err(e) => {
						missing += 1;
						PartShape::Missing(e.clone())
					}
				}
			};
			let (centre, scale, offset) = match (&shape, &look) {
				(_, Look::Primitive { size, offset, .. }) => ([0.0; 3], *size, *offset),
				// a SpecialMesh scales the mesh's own units instead of fitting it
				(
					PartShape::Mesh(_),
					Look::Mesh {
						special: Some((scale, offset)),
					},
				) => ([0.0; 3], *scale, *offset),
				(PartShape::Mesh(asset), _) => match asset.geometry.bounds() {
					Some((lo, hi)) => {
						let mut centre = [0.0; 3];
						let mut scale = [1.0; 3];
						for k in 0..3 {
							centre[k] = (lo[k] + hi[k]) * 0.5;
							let extent = hi[k] - lo[k];
							if extent > 1e-6 {
								scale[k] = part.size[k] / extent;
							}
						}
						(centre, scale, [0.0; 3])
					}
					None => ([0.0; 3], [1.0; 3], [0.0; 3]),
				},
				_ => ([0.0; 3], part.size, [0.0; 3]),
			};
			let approximate = match &look {
				Look::Primitive { approximate, .. } => approximate.clone(),
				_ => None,
			};
			let colour = match &shape {
				PartShape::Mesh(_) => colour_of(record, viewport::PART_COLOUR),
				_ => colour_of(record, BLOCK_COLOUR),
			};
			let skin = match &shape {
				PartShape::Mesh(asset) => match &asset.skin {
					Ok(Some(skin)) => {
						Some(PartSkin::build(skin, asset, &rig, &rest, i, centre, scale))
					}
					Ok(None) => None,
					Err(e) => {
						warnings.push(format!("{} is drawn without skinning: {e}", part.name));
						None
					}
				},
				_ => None,
			};
			let mut decals: Vec<(i32, PartDecal)> = Vec::new();
			for d in &record.decals {
				let image = crate::studio::export_file::parse_asset_id(&d.texture).and_then(|id| {
					decal_images
						.entry(id)
						.or_insert_with(|| {
							let path = images.get(&id)?;
							let bytes = std::fs::read(path).ok()?;
							texture::load(ctx, path, &bytes).ok()
						})
						.clone()
				});
				let Some(image) = image else {
					decals_missing += 1;
					continue;
				};
				let mut material = Material::default();
				material.set(Slot::Base, image);
				let tiles = d.studs_per_tile_u.map(|u| {
					[
						u,
						d.studs_per_tile_v.unwrap_or(u),
						d.offset_studs_u.unwrap_or(0.0),
						d.offset_studs_v.unwrap_or(0.0),
					]
				});
				decals.push((
					d.z_index,
					PartDecal {
						face: face_index(&d.face),
						tiles: if d.class_name == "Texture" {
							tiles
						} else {
							None
						},
						colour: d.color,
						transparency: d.transparency,
						material,
						gpu: None,
						variant: false,
						clothing: None,
					},
				));
			}
			decals.sort_by_key(|(z, _)| *z);
			// a MaterialVariant covers the part, under its decals
			if let Some(v) = &record.material_variant {
				let mut material = Material::default();
				let mut wanted = 0;
				for (slot, content) in v.maps() {
					let Some(id) = crate::studio::export_file::parse_asset_id(content) else {
						continue;
					};
					wanted += 1;
					let image = decal_images
						.entry(id)
						.or_insert_with(|| {
							let path = images.get(&id)?;
							let bytes = std::fs::read(path).ok()?;
							texture::load(ctx, path, &bytes).ok()
						})
						.clone();
					if let Some(image) = image {
						material.set(slot, image);
					}
				}
				if material.maps.len() < wanted {
					variants_missing += 1;
				}
				if !material.maps.is_empty() {
					if matches!(shape, PartShape::Mesh(_)) {
						variants_on_meshes += 1;
					}
					let tile = v.studs_per_tile.max(0.01);
					decals.insert(
						0,
						(
							i32::MIN,
							PartDecal {
								face: 0,
								tiles: Some([tile, tile, 0.0, 0.0]),
								colour: [1.0; 3],
								transparency: 0.0,
								material,
								gpu: None,
								variant: true,
								clothing: None,
							},
						),
					);
				}
			}
			// clothing over the body part, under its decals
			if let Some(&(section, in_section, centre, half)) = sections.get(&i) {
				let at = decals.iter().take_while(|(z, _)| *z == i32::MIN).count();
				let layers = clothing_images.iter().filter(|c| c.0 == section).rev();
				for (_, material, colour, graphic) in layers {
					let mut regions = section.regions();
					if *graphic {
						// a T-shirt covers the front of the torso
						regions = [[0.0; 4]; 6];
						regions[5] = [0.0, 0.0, 1.0, 1.0];
					}
					decals.insert(
						at,
						(
							i32::MIN,
							PartDecal {
								face: 0,
								tiles: None,
								colour: *colour,
								transparency: 0.0,
								material: material.clone(),
								gpu: None,
								variant: false,
								clothing: Some((
									ClothingDraw {
										regions,
										centre,
										half,
									},
									in_section,
								)),
							},
						),
					);
				}
			}
			parts.push(PartView {
				index: i,
				shape,
				visible: part.transparency < INVISIBLE,
				centre,
				scale,
				offset,
				colour,
				surface: surface_of(record),
				approximate,
				appearance: appearance(record),
				skin,
				gpu: None,
				decals: decals.into_iter().map(|(_, d)| d).collect(),
				lights: record
					.lights
					.iter()
					.filter(|l| l.enabled)
					.cloned()
					.collect(),
			});
		}
		if variants_on_meshes > 0 {
			warnings.push(format!(
				"{variants_on_meshes} mesh parts have a MaterialVariant, drawn tiled over the part's box faces as on ordinary parts; Studio lays it out differently on meshes."
			));
		}
		if variants_missing > 0 {
			warnings.push(format!(
				"{variants_missing} parts show their MaterialVariant without some of its maps: they are not downloaded yet."
			));
		}
		if decals_missing > 0 {
			warnings.push(format!(
				"{decals_missing} decals are not drawn: their images are not downloaded yet."
			));
		}
		let approximated: Vec<&str> = parts
			.iter()
			.filter(|p| p.visible)
			.filter_map(|p| p.approximate.as_deref())
			.collect();
		if !approximated.is_empty() {
			let mut kinds = approximated.clone();
			kinds.sort_unstable();
			kinds.dedup();
			warnings.push(format!(
				"{} parts are drawn approximately because Meshvault has no geometry for their shape: {}.",
				approximated.len(),
				kinds.join(", ")
			));
		}
		if missing > 0 {
			warnings.push(format!(
				"{missing} parts are not shown because their meshes are not downloaded or could not be read. Hover them in the part list for details."
			));
		}
		let unbound: usize = parts
			.iter()
			.filter_map(|p| p.skin.as_ref())
			.map(|s| s.bones - s.matched)
			.sum();
		if unbound > 0 {
			log_unbound(&mut warnings, unbound);
		}

		let clips: Vec<ClipSlot> = model
			.animations
			.iter()
			.map(|record| {
				let clip = if record.status.is_playable() {
					Clip::from_record(record)
				} else {
					Err(match record.status {
						ClipStatus::Unavailable => "The exporter could not read this animation (it may be private or not owned by the game).".into(),
						ClipStatus::CurveAnimationExportFailed => "The curve animation could not be exported.".into(),
						ClipStatus::UnsupportedClipType => "This clip type is not supported by the exporter.".into(),
						_ => format!("Status {:?} has no motion data.", record.status),
					})
				};
				let binding = clip.as_ref().map(|c| rig.bind(c)).unwrap_or_default();
				let mesh_names: std::collections::HashSet<&str> = parts
					.iter()
					.filter_map(|p| p.skin.as_ref())
					.flat_map(|s| s.drivers.iter())
					.filter_map(|d| match d {
						Driver::Mesh { name, .. } => Some(name.as_str()),
						_ => None,
					})
					.collect();
				let mesh_bone_tracks = binding
					.unmatched
					.iter()
					.filter(|n| mesh_names.contains(n.as_str()))
					.count();
				ClipSlot {
					name: record.name.clone(),
					status: record.status,
					priority: record.priority.clone(),
					animation_id: record.animation_id.clone(),
					clip,
					binding,
					mesh_bone_tracks,
				}
			})
			.collect();

		// bones under a resized skinned mesh move in the mesh's own units
		let part_scale: HashMap<usize, f32> = parts
			.iter()
			.filter(|p| matches!(p.shape, PartShape::Mesh(_)))
			.map(|p| (p.index, mean(p.scale)))
			.collect();
		// Schema 2 records Model:GetScale(), which Roblox applies to every
		// animated translation. Schema 1 did not, so for bones it is inferred
		// from the mesh they deform, and joints are left as they are.
		let model_scale = model.scale.filter(|s| s.is_finite() && *s > 0.0);
		let motor_scale = vec![model_scale.unwrap_or(1.0); rig.motors.len()];
		if let Some(s) = model_scale {
			for skin in parts.iter_mut().filter_map(|p| p.skin.as_mut()) {
				skin.unit_scale = s;
			}
		}
		let bone_scale = (0..rig.bones.len())
			.map(|mut b| {
				if let Some(s) = model_scale {
					return s;
				}
				for _ in 0..=rig.bones.len() {
					match rig.bones[b].parent {
						BoneParent::Part(p) => return part_scale.get(&p).copied().unwrap_or(1.0),
						BoneParent::Bone(q) => b = q,
						BoneParent::None => break,
					}
				}
				1.0
			})
			.collect();

		// clips that move parts no exported joint connects: newer avatars use
		// AnimationConstraints, which the exporter does not read
		let joined: std::collections::HashSet<usize> =
			rig.motors.iter().flat_map(|m| [m.part0, m.part1]).collect();
		let loose: std::collections::HashSet<&str> = rig
			.parts
			.iter()
			.enumerate()
			.filter(|(i, _)| !joined.contains(i))
			.map(|(_, p)| p.name.as_str())
			.collect();
		let stranded = clips
			.iter()
			.flat_map(|c| c.binding.unmatched.iter())
			.filter(|n| loose.contains(n.as_str()))
			.collect::<std::collections::BTreeSet<_>>();
		if !stranded.is_empty() {
			warnings.push(format!(
				"Its clips animate {} parts that no exported joint connects, so they stay still. Rigs built with AnimationConstraints (newer avatars) or other joint types are not in the export.",
				stranded.len()
			));
		}

		let mut view = Self {
			motor_scale,
			bone_scale,
			index,
			name: model.name.clone(),
			path: model.path.clone(),
			posed: rest.clone(),
			rest,
			rig,
			current: None,
			time: 0.0,
			playing: false,
			looping: true,
			speed: 1.0,
			camera: Camera::framing_bounds(None),
			bounds: None,
			warnings,
			show_blocks: true,
			scale: model.scale,
			captured_pose: model.joints.iter().any(|j| j.transform.is_some()),
			dirty: true,
			highlight: None,
			clips,
			parts,
		};
		view.bounds = view.rest_bounds();
		view.camera = Camera::framing_bounds(view.bounds);
		// start playing the first clip that can play
		if let Some(first) = view.clips.iter().position(|c| c.clip.is_ok()) {
			view.select(Some(first));
			view.playing = true;
		}
		view
	}

	/// Bounds of the visible parts at rest, for framing.
	fn rest_bounds(&self) -> Option<(V3, V3)> {
		let mut bounds: Option<(V3, V3)> = None;
		for part in &self.parts {
			if !part.visible || matches!(part.shape, PartShape::Missing(_)) {
				continue;
			}
			let rp = &self.rig.parts[part.index];
			let rest = self.rest.parts[part.index];
			let h = [rp.size[0] * 0.5, rp.size[1] * 0.5, rp.size[2] * 0.5];
			for corner in 0..8 {
				let local = [
					if corner & 1 == 0 { -h[0] } else { h[0] },
					if corner & 2 == 0 { -h[1] } else { h[1] },
					if corner & 4 == 0 { -h[2] } else { h[2] },
				];
				let p = rest.point(local);
				let (lo, hi) = bounds.get_or_insert((p, p));
				for k in 0..3 {
					lo[k] = lo[k].min(p[k]);
					hi[k] = hi[k].max(p[k]);
				}
			}
		}
		bounds
	}

	pub fn home(&self) -> Camera {
		Camera::framing_bounds(self.bounds)
	}

	pub fn clip(&self) -> Option<&Clip> {
		self.clips.get(self.current?)?.clip.as_ref().ok()
	}

	pub fn duration(&self) -> f32 {
		self.clip().map_or(0.0, |c| c.duration)
	}

	pub fn markers(&self) -> &[ClipMarker] {
		self.clip().map_or(&[], |c| c.markers.as_slice())
	}

	/// Chooses a clip (or the rest pose with `None`) and rewinds.
	pub fn select(&mut self, clip: Option<usize>) {
		let clip = clip.filter(|&i| self.clips.get(i).is_some_and(|c| c.clip.is_ok()));
		self.current = clip;
		self.time = 0.0;
		if let Some(c) = self.clip() {
			self.looping = c.looped;
		} else {
			self.playing = false;
		}
		self.dirty = true;
	}

	pub fn set_time(&mut self, time: f32) {
		self.time = time.clamp(0.0, self.duration());
		self.dirty = true;
	}

	/// Shows the rest pose: no clip.
	pub fn reset(&mut self) {
		self.select(None);
	}

	/// Advances playback by `dt` seconds. Returns whether it is still playing.
	pub fn tick(&mut self, dt: f32) -> bool {
		if !self.playing || self.current.is_none() {
			return false;
		}
		let duration = self.duration();
		let mut t = self.time + dt * self.speed;
		if duration <= 0.0 {
			t = 0.0;
		} else if self.looping {
			t = t.rem_euclid(duration);
		} else if t >= duration {
			t = duration;
			self.playing = false;
		}
		self.time = t;
		self.dirty = true;
		self.playing
	}

	pub fn toggle_play(&mut self) {
		if self.current.is_none() {
			return;
		}
		// playing again from the end of a one-shot clip starts it over
		if !self.playing && !self.looping && self.time >= self.duration() {
			self.time = 0.0;
		}
		self.playing = !self.playing;
	}

	/// Solves the rig at the current time and deforms skinned meshes.
	fn update_pose(&mut self) {
		if !std::mem::take(&mut self.dirty) {
			return;
		}
		let (posed, by_name, face) = self.solve_at(self.current, self.time);
		for part in &mut self.parts {
			if let Some(skin) = &mut part.skin {
				skin.update(&self.rest, &posed, part.index, &by_name, &face);
			}
		}
		self.posed = posed;
	}

	/// The rig posed by `clip` at `time` (the rest pose without a clip),
	/// with the clip's transforms for bones only meshes have, by name, and
	/// its face control weights.
	fn solve_at(
		&self,
		clip: Option<usize>,
		time: f32,
	) -> (Posed, HashMap<String, CFrame>, Vec<(String, f32)>) {
		let slot = clip.and_then(|i| self.clips.get(i));
		// face controls the clip animates, for FACS heads
		let face: Vec<(String, f32)> = slot
			.and_then(|s| s.clip.as_ref().ok())
			.map(|clip| {
				clip.face_weights(time)
					.into_iter()
					.map(|(n, w)| (n.to_string(), w))
					.collect()
			})
			.unwrap_or_default();
		let (posed, by_name) = match slot.and_then(|s| Some((s.clip.as_ref().ok()?, &s.binding))) {
			Some((clip, binding)) => {
				let samples: Vec<(&str, CFrame)> = clip
					.sample(time)
					.map(|(key, t)| (key.name.as_str(), t))
					.collect();
				let transforms: Vec<CFrame> = samples
					.iter()
					.zip(&binding.targets)
					.map(|(s, target)| match target {
						Some(Target::Bone(b)) => scale_translation(s.1, self.bone_scale[*b]),
						Some(Target::Motor(m)) => scale_translation(s.1, self.motor_scale[*m]),
						_ => s.1,
					})
					.collect();
				let posed = self.rig.pose_from(binding, &transforms);
				let by_name: HashMap<String, CFrame> = samples
					.iter()
					.zip(&binding.targets)
					.filter(|(_, t)| t.is_none())
					.map(|(s, _)| (s.0.to_string(), s.1))
					.collect();
				(posed, by_name)
			}
			None => (self.rest.clone(), HashMap::new()),
		};
		(posed, by_name, face)
	}

	/// The skeleton where the model is posed now: lines from joint to joint
	/// (and on to the middle of each limb's last part), and each joint.
	pub fn skeleton(&mut self) -> Skeleton {
		self.update_pose();
		let posed = &self.posed;
		let rig = &self.rig;
		let links = rig.part_links();
		let pivot = |m: usize| {
			let m = &rig.motors[m];
			posed.parts[m.part0].mul(&m.c0).pos
		};
		// where a part hangs from its parent, or its middle for a root
		let point = |p: usize| links[p].map_or(posed.parts[p].pos, |(_, m)| pivot(m));
		let mut has_child = vec![false; rig.parts.len()];
		for &(parent, _) in links.iter().flatten() {
			has_child[parent] = true;
		}
		let mut skeleton = Skeleton::default();
		for (p, link) in links.iter().enumerate() {
			let Some((parent, _)) = link else { continue };
			skeleton.bones.push((point(*parent), point(p)));
			if !has_child[p] {
				skeleton.bones.push((point(p), posed.parts[p].pos));
			}
		}
		for (i, m) in rig.motors.iter().enumerate() {
			skeleton.joints.push(Joint {
				at: pivot(i),
				name: m.name.clone(),
				animated: m.animated,
			});
		}
		for (i, b) in rig.bones.iter().enumerate() {
			let at = posed.bones[i].pos;
			let from = match b.parent {
				BoneParent::Part(p) => Some(posed.parts[p].pos),
				BoneParent::Bone(q) => Some(posed.bones[q].pos),
				BoneParent::None => None,
			};
			if let Some(from) = from {
				skeleton.bones.push((from, at));
			}
			skeleton.joints.push(Joint {
				at,
				name: b.name.clone(),
				animated: true,
			});
		}
		skeleton
	}

	// ---------------------------------------------------------------- export

	/// The model as a skeleton for export: every part, then every Bone,
	/// then each skinned mesh's bones that only the mesh has.
	pub fn export_nodes(&self) -> Vec<ExportNode> {
		let parents = self.rig.part_parents();
		let parts = self.rig.parts.len();
		let mut nodes: Vec<ExportNode> = self
			.rig
			.parts
			.iter()
			.zip(parents)
			.map(|(p, parent)| ExportNode {
				name: p.name.clone(),
				parent,
			})
			.collect();
		nodes.extend(self.rig.bones.iter().map(|b| ExportNode {
			name: b.name.clone(),
			parent: match b.parent {
				BoneParent::Part(p) => Some(p),
				BoneParent::Bone(b) => Some(parts + b),
				BoneParent::None => None,
			},
		}));
		for (part, skin) in self.skins() {
			let first = nodes.len();
			let driver_node = self.driver_nodes(skin, first);
			for (k, d) in skin.drivers.iter().enumerate() {
				if let Driver::Mesh { parent, name, .. } = d {
					nodes.push(ExportNode {
						name: name.clone(),
						parent: Some(match parent {
							Some(p) => driver_node[*p],
							None => part.index,
						}),
					});
					debug_assert_eq!(nodes.len() - 1, driver_node[k]);
				}
			}
		}
		nodes
	}

	/// The visible parts with a skin, in node order.
	fn skins(&self) -> impl Iterator<Item = (&PartView, &PartSkin)> {
		self.parts
			.iter()
			.filter_map(|p| Some((p, p.skin.as_ref()?)))
	}

	/// The node of each of a skin's bones, mesh-only ones numbered from `first`.
	fn driver_nodes(&self, skin: &PartSkin, first: usize) -> Vec<usize> {
		let parts = self.rig.parts.len();
		let mut next = first;
		skin.drivers
			.iter()
			.map(|d| match d {
				Driver::Part(p) => *p,
				Driver::Bone(b) => parts + b,
				Driver::Mesh { .. } => {
					next += 1;
					next - 1
				}
			})
			.collect()
	}

	/// Where every node of [`Self::export_nodes`] is, in world space, when
	/// `clip` is at `time` (the rest pose without a clip).
	pub fn export_frame(&self, clip: Option<usize>, time: f32) -> Vec<CFrame> {
		let (posed, by_name, face) = self.solve_at(clip, time);
		let mut worlds = posed.parts.clone();
		worlds.extend(posed.bones.iter().copied());
		for (part, skin) in self.skins() {
			let (now, _) = skin.drivers_at(&self.rest, &posed, part.index, &by_name, &face);
			for (k, d) in skin.drivers.iter().enumerate() {
				if matches!(d, Driver::Mesh { .. }) {
					worlds.push(now[k]);
				}
			}
		}
		worlds
	}

	/// What is drawn, in the rest pose, with the nodes that move it.
	/// Primitive parts are included when blocks are shown.
	pub fn export_meshes(&self) -> Vec<ExportMesh<'_>> {
		let mut first = self.rig.parts.len() + self.rig.bones.len();
		let mut out = Vec::new();
		for part in &self.parts {
			let skinned = part.skin.as_ref().map(|skin| {
				let nodes = self.driver_nodes(skin, first);
				first += skin
					.drivers
					.iter()
					.filter(|d| matches!(d, Driver::Mesh { .. }))
					.count();
				(skin, nodes)
			});
			if !part.visible {
				continue;
			}
			let name = self.rig.parts[part.index].name.clone();
			let base = ExportMesh {
				name,
				// every arm below sets its own
				geometry: Primitive::Block.geometry(),
				groups: Vec::new(),
				positions: Vec::new(),
				normals: None,
				joints: vec![part.index],
				influences: Vec::new(),
				material: empty_material(),
				colour: part.colour,
				surface: part.surface,
			};
			let rest = self.rest.parts[part.index].mul(&CFrame::from_position(part.offset));
			match (&part.shape, skinned) {
				(PartShape::Mesh(asset), Some((skin, nodes))) => {
					// joint k of the skin is its bone k; one more for the part,
					// which carries the vertices no bone moves
					let mut joints = nodes;
					joints.push(part.index);
					let owner = (joints.len() - 1) as u16;
					let influences = skin
						.influences
						.iter()
						.map(|inf| {
							if inf.iter().any(|&(_, w)| w > 0.0) {
								*inf
							} else {
								[(owner, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]
							}
						})
						.collect();
					out.push(ExportMesh {
						geometry: &asset.geometry,
						groups: asset.groups.clone(),
						positions: skin.rest_positions.clone(),
						normals: skin.rest_normals.clone(),
						joints,
						influences,
						material: &asset.material,
						..base
					});
				}
				(PartShape::Mesh(asset), None) => {
					let (positions, normals) =
						placed(&asset.geometry, &rest, part.scale, part.centre);
					out.push(ExportMesh {
						geometry: &asset.geometry,
						groups: asset.groups.clone(),
						influences: vec![[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]; positions.len()],
						positions,
						normals,
						material: &asset.material,
						..base
					});
				}
				(PartShape::Primitive(kind), _) if self.show_blocks => {
					let geometry = kind.geometry();
					let (positions, normals) = placed(geometry, &rest, part.scale, [0.0; 3]);
					out.push(ExportMesh {
						geometry,
						groups: vec![0],
						influences: vec![[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]; positions.len()],
						positions,
						normals,
						..base
					});
				}
				_ => {}
			}
		}
		out
	}

	/// The parts to draw this frame. `plain` replaces mesh materials when
	/// maps are switched off.
	pub fn scene_parts<'a>(&'a mut self, plain: Option<&'a Material>) -> Vec<ScenePart<'a>> {
		self.update_pose();
		let posed = &self.posed;
		let rig = &self.rig;
		let show_blocks = self.show_blocks;
		let highlight = self.highlight;
		self.parts
			.iter_mut()
			.filter(|p| p.visible)
			.flat_map(|part| {
				let lit = highlight == Some(part.index);
				let part_world = posed.parts[part.index];
				let size = rig.parts[part.index].size;
				let PartView {
					shape,
					skin,
					gpu,
					centre,
					scale,
					offset,
					colour,
					surface,
					decals,
					..
				} = part;
				// SpecialMesh.Offset moves the mesh in the part's own space
				let in_part = CFrame::from_position(*offset);
				let world = part_world.mul(&in_part);
				// what is drawn: geometry, groups, mesh to world, mesh to part,
				// whether the vertices moved, and the material
				let drawn: Option<(&Geometry, &[usize], _, _, bool, &Material)> = match shape {
					PartShape::Mesh(asset) => {
						let material = plain.unwrap_or(&asset.material);
						Some(match skin {
							// skinned vertices are already in world space
							Some(skin) => (
								&skin.geometry,
								asset.groups.as_slice(),
								viewport::IDENTITY,
								part_world.inverse().to_matrix([1.0; 3]),
								std::mem::take(&mut skin.moved),
								material,
							),
							None => (
								&asset.geometry,
								asset.groups.as_slice(),
								placement(&world, *scale, *centre),
								placement(&in_part, *scale, *centre),
								false,
								material,
							),
						})
					}
					PartShape::Primitive(kind) if show_blocks => Some((
						kind.geometry(),
						&[0][..],
						placement(&world, *scale, [0.0; 3]),
						placement(&in_part, *scale, [0.0; 3]),
						false,
						plain.unwrap_or_else(|| empty_material()),
					)),
					_ => None,
				};
				let Some((geometry, groups, transform, part_from_mesh, moved, material)) = drawn
				else {
					return Vec::new();
				};
				let mut drawn = vec![ScenePart {
					geometry,
					groups,
					transform,
					material,
					colour_groups: false,
					colour: *colour,
					scene: gpu,
					moved,
					surface: *surface,
					decal: None,
					highlight: lit,
				}];
				for d in decals.iter_mut() {
					drawn.push(ScenePart {
						geometry,
						groups,
						transform,
						material: &d.material,
						colour_groups: false,
						colour: *colour,
						scene: &mut d.gpu,
						moved,
						surface: *surface,
						highlight: lit,
						decal: Some(DecalDraw {
							// clothing places the mesh in its body section's box
							part_from_mesh: match &d.clothing {
								Some((_, in_section)) => {
									mat_mul(&in_section.to_matrix([1.0; 3]), &part_from_mesh)
								}
								None => part_from_mesh,
							},
							size,
							face: d.face,
							tiles: d.tiles,
							colour: d.colour,
							transparency: d.transparency,
							variant: d.variant,
							clothing: d.clothing.map(|(c, _)| c),
						}),
					});
				}
				drawn
			})
			.collect()
	}

	/// The parts' lights where the parts are now. Spot and surface lights
	/// shine out of their face; a surface light from the face itself.
	pub fn scene_lights(&mut self) -> Vec<viewport::SceneLight> {
		self.update_pose();
		let mut lights = Vec::new();
		for part in &self.parts {
			if part.lights.is_empty() {
				continue;
			}
			let world = self.posed.parts[part.index];
			let size = self.rig.parts[part.index].size;
			for l in &part.lights {
				let at = l
					.cframe
					.as_ref()
					.map_or(world, |c| world.mul(&CFrame::from_components(c)));
				let normal = match l.face.as_deref().unwrap_or("Front") {
					"Right" => [1.0, 0.0, 0.0],
					"Top" => [0.0, 1.0, 0.0],
					"Back" => [0.0, 0.0, 1.0],
					"Left" => [-1.0, 0.0, 0.0],
					"Bottom" => [0.0, -1.0, 0.0],
					_ => [0.0, 0.0, -1.0],
				};
				let mut position = at.pos;
				if l.class_name == "SurfaceLight" && l.cframe.is_none() {
					// from the middle of the face
					let half = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
					position = at.point([
						normal[0] * half[0],
						normal[1] * half[1],
						normal[2] * half[2],
					]);
				}
				lights.push(viewport::SceneLight {
					position,
					direction: at.rotate(normal),
					colour: l.color,
					brightness: l.brightness,
					range: l.range,
					half_angle: match l.class_name.as_str() {
						"SpotLight" | "SurfaceLight" => Some(l.angle.unwrap_or(90.0) * 0.5),
						_ => None,
					},
				});
			}
		}
		lights
	}

	pub fn triangle_count(&self) -> usize {
		self.parts
			.iter()
			.filter(|p| p.visible)
			.map(|p| match &p.shape {
				PartShape::Mesh(a) => a
					.groups
					.iter()
					.filter_map(|&g| a.geometry.groups.get(g))
					.map(|r| r.len())
					.sum(),
				PartShape::Primitive(kind) if self.show_blocks => kind.geometry().triangles.len(),
				_ => 0,
			})
			.sum()
	}
}

/// What the skeleton overlay draws, in world space.
#[derive(Default)]
pub struct Skeleton {
	pub bones: Vec<(V3, V3)>,
	pub joints: Vec<Joint>,
}

/// A joint of the skeleton overlay: a Motor6D, weld or Bone.
pub struct Joint {
	pub at: V3,
	pub name: String,
	/// Clips drive it (a weld only holds its parts together).
	pub animated: bool,
}

/// A node of the exported skeleton: a part, a Bone, or a bone only a
/// skinned mesh has.
pub struct ExportNode {
	pub name: String,
	/// Index of the parent node.
	pub parent: Option<usize>,
}

/// One drawn mesh for export, in the rest pose (world space), skinned to
/// nodes of [`ModelView::export_nodes`].
pub struct ExportMesh<'a> {
	/// The part's name.
	pub name: String,
	/// Triangles, UVs and groups; its positions are replaced by `positions`.
	pub geometry: &'a Geometry,
	pub groups: Vec<usize>,
	pub positions: Vec<V3>,
	pub normals: Option<Vec<V3>>,
	/// The nodes that move it; `influences` index into these.
	pub joints: Vec<usize>,
	pub influences: Vec<[(u16, f32); 4]>,
	pub material: &'a Material,
	pub colour: Color32,
	pub surface: Surface,
}

/// A mesh's vertices and normals placed in the world by `cf`, fitted by
/// `scale` and `centre`.
fn placed(geometry: &Geometry, cf: &CFrame, scale: V3, centre: V3) -> (Vec<V3>, Option<Vec<V3>>) {
	let positions = geometry
		.positions
		.iter()
		.map(|&v| fitted(cf, scale, centre, v))
		.collect();
	let normals = geometry.normals.as_ref().map(|normals| {
		normals
			.iter()
			.map(|n| normalize(cf.rotate([n[0] / scale[0], n[1] / scale[1], n[2] / scale[2]])))
			.collect()
	});
	(positions, normals)
}

fn empty_material() -> &'static Material {
	static EMPTY: OnceLock<Material> = OnceLock::new();
	EMPTY.get_or_init(Material::default)
}

fn log_unbound(warnings: &mut Vec<String>, unbound: usize) {
	warnings.push(format!(
		"{unbound} bones inside skinned meshes (such as face bones) are not in the model. They hold their bind pose unless a clip animates them by name; facial animation (FACS) is not exported."
	));
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::anim::schema::{BoneInfo, ModelPart, MotorJoint};

	fn cf(pos: V3) -> [f32; 12] {
		CFrame::from_position(pos).components()
	}

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

	fn asset(positions: Vec<V3>, skin: Skin) -> MeshAsset {
		let n = positions.len() as u32;
		MeshAsset {
			name: "mesh".into(),
			geometry: Geometry {
				triangles: vec![[0, 1 % n, 2 % n]],
				groups: std::iter::once(0..1).collect(),
				positions,
				uvs: None,
				normals: None,
				group_kind: GroupKind::Whole,
			},
			groups: vec![0],
			skin: Ok(Some(skin.clone())),
			material: Material::default(),
		}
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

	/// Lights follow their part: a spotlight shines out of its face, a
	/// surface light from the middle of its face, and disabled lights are off.
	#[test]
	fn lights_shine_from_their_part() {
		use crate::anim::schema::LightInfo;
		let light = |class: &str, face: Option<&str>, enabled: bool| LightInfo {
			class_name: class.into(),
			enabled,
			brightness: 1.0,
			color: [1.0; 3],
			range: 8.0,
			angle: face.map(|_| 60.0),
			face: face.map(String::from),
			..Default::default()
		};
		let mut lamp = part_with(Some("Part"), Some("Block"), [2.0, 2.0, 2.0]);
		lamp.path = "Workspace/Lamp/Body".into();
		lamp.name = "Body".into();
		// turned a quarter about Y: its Front (-Z) faces -X in the world
		lamp.cframe = CFrame::axis_rotation(1, std::f32::consts::FRAC_PI_2).components();
		lamp.cframe[1] = 5.0;
		lamp.lights = vec![
			light("SpotLight", Some("Front"), true),
			light("SurfaceLight", Some("Top"), true),
			light("PointLight", None, false),
		];
		let model = AnimatedModel {
			name: "Lamp".into(),
			path: "Workspace/Lamp".into(),
			parts: vec![lamp],
			..Default::default()
		};
		let ctx = egui::Context::default();
		let mut view = ModelView::load(&ctx, 0, &model, &HashMap::new(), &HashMap::new());
		let lights = view.scene_lights();
		assert_eq!(lights.len(), 2);
		let close = |a: V3, b: V3| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5);
		assert!(
			close(lights[0].direction, [-1.0, 0.0, 0.0]),
			"{:?}",
			lights[0].direction
		);
		assert_eq!(lights[0].half_angle, Some(30.0));
		assert!(
			close(lights[1].position, [0.0, 6.0, 0.0]),
			"{:?}",
			lights[1].position
		);
	}

	/// Plays every clip of every model in a real export (without meshes):
	/// `RBX_V1_EXPORT=path cargo test -p meshvault --release -- --ignored plays_real_export --nocapture`
	#[test]
	#[ignore]
	fn plays_real_export() {
		let Ok(path) = std::env::var("RBX_V1_EXPORT") else {
			return;
		};
		let export =
			crate::studio::export_file::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
		let ctx = egui::Context::default();
		let mut frames = 0;
		for (i, model) in export.models.iter().enumerate() {
			let mut view = ModelView::load(&ctx, i, model, &HashMap::new(), &HashMap::new());
			for c in 0..view.clips.len() {
				if view.clips[c].clip.is_err() {
					continue;
				}
				view.select(Some(c));
				let duration = view.duration();
				for step in 0..=20 {
					view.set_time(duration * step as f32 / 20.0);
					let n = view.scene_parts(None).len();
					frames += 1;
					for (k, cf) in view.posed.parts.iter().enumerate() {
						assert!(
							cf.pos
								.iter()
								.chain(cf.rot.iter().flatten())
								.all(|v| v.is_finite()),
							"{} clip {} part {k} is not finite",
							model.path,
							view.clips[c].name
						);
					}
					assert!(n <= view.parts.len());
				}
			}
			println!(
				"{}: {} parts, {} clips, {} warnings",
				model.path,
				view.parts.len(),
				view.clips.len(),
				view.warnings.len()
			);
		}
		println!("{frames} poses solved");
	}

	fn part_with(class: Option<&str>, shape: Option<&str>, size: V3) -> ModelPart {
		ModelPart {
			class_name: class.map(String::from),
			shape: shape.map(String::from),
			size,
			..Default::default()
		}
	}

	#[test]
	fn parts_choose_their_geometry() {
		use crate::anim::schema::SpecialMeshInfo;
		let prim = |l: Look| match l {
			Look::Primitive {
				kind,
				size,
				approximate,
				..
			} => (kind, size, approximate.is_some()),
			other => panic!("{other:?}"),
		};
		// schema 1 has no class: a box
		assert_eq!(
			prim(look(&part_with(None, None, [1.0, 2.0, 3.0]))),
			(Primitive::Block, [1.0, 2.0, 3.0], false)
		);
		// a ball is as wide as the part's smallest side; a cylinder runs along X
		assert_eq!(
			prim(look(&part_with(
				Some("Part"),
				Some("Ball"),
				[3.0, 2.0, 4.0]
			))),
			(Primitive::Ball, [2.0; 3], false)
		);
		assert_eq!(
			prim(look(&part_with(
				Some("Part"),
				Some("Cylinder"),
				[5.0, 2.0, 3.0]
			))),
			(Primitive::CylinderX, [5.0, 2.0, 2.0], false)
		);
		assert_eq!(
			prim(look(&part_with(Some("WedgePart"), Some("Wedge"), [1.0; 3]))).0,
			Primitive::Wedge
		);
		assert!(prim(look(&part_with(Some("UnionOperation"), Some(""), [1.0; 3]))).2);
		assert!(matches!(
			look(&part_with(Some("MeshPart"), Some(""), [1.0; 3])),
			Look::Missing(_)
		));
		// a SpecialMesh sphere stretches to size × scale and moves by its offset
		let mut p = part_with(Some("Part"), Some("Block"), [2.0, 2.0, 2.0]);
		p.special_mesh = Some(SpecialMeshInfo {
			mesh_type: "Sphere".into(),
			scale: [1.0, 0.5, 1.0],
			offset: [0.0, 1.0, 0.0],
		});
		assert_eq!(
			look(&p),
			Look::Primitive {
				kind: Primitive::Ball,
				size: [2.0, 1.0, 2.0],
				offset: [0.0, 1.0, 0.0],
				approximate: None
			}
		);
		// a SpecialMesh cylinder lies along X like a cylinder part
		p.special_mesh.as_mut().unwrap().mesh_type = "Cylinder".into();
		let Look::Primitive { kind, size, .. } = look(&p) else {
			panic!()
		};
		assert_eq!((kind, size), (Primitive::CylinderX, [2.0, 1.0, 1.0]));
		// a SpecialMesh head, measured in Studio: a classic 2×1×1 head part with
		// scale 1.25 is 1.18 studs wide and tall, round however X and Z differ
		let mut head = part_with(Some("Part"), Some("Block"), [2.0, 1.0, 1.0]);
		head.special_mesh = Some(SpecialMeshInfo {
			mesh_type: "Head".into(),
			scale: [1.25; 3],
			offset: [0.0; 3],
		});
		let (kind, size, approximate) = prim(look(&head));
		assert_eq!(kind, Primitive::Head);
		assert!(!approximate);
		assert!(size.iter().all(|s| (s - 1.175).abs() < 1e-3), "{size:?}");
		// a SpecialMesh file mesh keeps its own scale instead of fitting
		p.mesh_ref = 3;
		p.special_mesh.as_mut().unwrap().mesh_type = "FileMesh".into();
		assert_eq!(
			look(&p),
			Look::Mesh {
				special: Some(([1.0, 0.5, 1.0], [0.0, 1.0, 0.0]))
			}
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

	#[test]
	fn placement_centres_and_scales() {
		let m = placement(
			&CFrame::from_position([10.0, 0.0, 0.0]),
			[2.0, 2.0, 2.0],
			[1.0, 1.0, 1.0],
		);
		// the mesh centre lands on the part position
		let at = |v: V3| {
			[
				m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2] + m[0][3],
				m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2] + m[1][3],
				m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2] + m[2][3],
			]
		};
		assert_eq!(at([1.0, 1.0, 1.0]), [10.0, 0.0, 0.0]);
		assert_eq!(at([2.0, 1.0, 1.0]), [12.0, 0.0, 0.0]);
	}
}
