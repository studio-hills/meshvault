//! An animated model open in the viewer: its parts placed by the rig, the
//! clips it can play, and skinned meshes deformed by their bones.
//!
//! Parts take their meshes from the workspace. A mesh is centred and scaled
//! to the part's `Size`, as Roblox does for MeshParts. A skinned mesh is
//! deformed on the CPU: every bone it was bound to is matched by name to a
//! Bone in the model, else to a part (R15 bodies bind to their parts), else
//! rebuilt from the bind pose stored in the mesh file. Each vertex then
//! follows `world now * world at rest⁻¹` of its bones.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Color32};

use crate::anim::clip::{Clip, ClipMarker};
use crate::anim::math::{CFrame, V3, box_corners, grow_bounds};
use crate::anim::rig::{Binding, BoneParent, Posed, Rig, Target};
use crate::anim::schema::{AnimatedModel, ClipStatus};
use crate::render::camera::Camera;
use crate::render::gpu::GpuScene;
use crate::scene::geometry::{self, Geometry, Primitive, Skin};
use crate::scene::maps::Slot;
use crate::scene::surface::{PART_COLOUR, Surface};
use crate::scene::texture::{self, Material};

mod decals;
mod draw;
mod export;
mod look;
mod skin;

pub use self::draw::{Joint, Skeleton};
pub use self::export::ExportMesh;
pub use self::skin::PartSkin;

use self::decals::{
	ImageCache, PartDecal, body_sections, clothing_layers, insert_clothing, part_decals,
	variant_layer,
};
use self::look::{BLOCK_COLOUR, INVISIBLE, Look, appearance, colour_of, look, surface_of};
use self::skin::{Driver, mean, scale_translation};

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
	geometry: Geometry,
	/// Groups drawn: the finest LOD, or every hull or group.
	groups: Vec<usize>,
	/// `Ok(None)` when the mesh is not skinned.
	skin: Result<Option<Skin>, String>,
	material: Material,
}

pub enum PartShape {
	Mesh(Arc<MeshAsset>),
	/// A part without a file mesh: a block, ball, cylinder, head, wedge,
	/// corner wedge or truss.
	Primitive(Primitive),
	/// Not drawn, and why.
	Missing(String),
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
	joint_scale: Vec<f32>,
	bone_scale: Vec<f32>,
	/// The pose needs solving again (time, clip or rest changed).
	dirty: bool,
	/// The part tinted to stand out (hovered in the parts list).
	pub highlight: Option<usize>,
}

fn load_asset(ctx: &egui::Context, source: &MeshSource) -> Result<MeshAsset, String> {
	let path = source.mesh.as_ref().map_err(Clone::clone)?;
	let bytes =
		std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
	let (loaded, skin) = geometry::load_with_skin(&bytes)?;
	let geometry = loaded.geometry;
	if geometry.triangles.is_empty() {
		return Err(loaded.note.unwrap_or_else(|| "the mesh is empty".into()));
	}
	let groups = geometry.default_groups();
	let material = if geometry.uvs.is_some() {
		texture::load_maps(ctx, &source.maps)
	} else {
		Material::default()
	};
	Ok(MeshAsset {
		name: source.name.clone(),
		geometry,
		groups,
		skin,
		material,
	})
}

/// What `load` counted while building the parts, for its summary warnings.
#[derive(Default)]
struct LoadCounts {
	/// Parts not drawn: no mesh, or one that did not load.
	missing_parts: usize,
	/// Decals whose images did not load.
	missing_decals: usize,
	/// MaterialVariants drawn without some of their maps.
	partial_variants: usize,
	/// MaterialVariants drawn over mesh parts.
	variants_on_meshes: usize,
}

/// How far, in studs, the joints may place a part from where it was saved
/// before the rest pose is called out.
const REST_TOLERANCE: f32 = 0.01;

/// A mesh no thicker than this along an axis (a flat one) is not stretched
/// along it to the part's size.
const MIN_FIT_EXTENT: f32 = 1e-6;

/// Why the rest pose is not quite the saved one, when the joints place
/// parts more than [`REST_TOLERANCE`] from where they were.
fn rest_pose_warning(model: &AnimatedModel, rest_error: f32) -> Option<String> {
	// schema 2 records each joint's pose, so a difference left over is in
	// the saved scene itself
	(rest_error > REST_TOLERANCE).then(|| {
		if model.joints.iter().any(|j| j.transform.is_some()) {
			format!(
				"The joints place parts up to {rest_error:.2} studs from where they were saved: the parts and their joints do not quite meet in the place (common with AnimationConstraint avatars, which Roblox pulls together once the game runs). Meshvault places them by the joints, as the running game does."
			)
		} else {
			format!(
				"The joints place parts up to {rest_error:.2} studs from where they were exported, so the model was probably captured mid-animation. Clips play from the joints' rest pose."
			)
		}
	})
}

/// What a part is drawn with. Each mesh is loaded once for every part that
/// uses it, and one that fails stays failed; `missing` counts each part not
/// drawn.
fn part_shape(
	ctx: &egui::Context,
	look: &Look,
	mesh_ref: u32,
	sources: &HashMap<u32, MeshSource>,
	assets: &mut HashMap<u32, Result<Arc<MeshAsset>, String>>,
	missing: &mut usize,
) -> PartShape {
	match look {
		Look::Primitive { kind, .. } => PartShape::Primitive(*kind),
		Look::Missing(why) => {
			*missing += 1;
			PartShape::Missing(why.clone())
		}
		Look::Mesh { .. } => {
			let asset = assets
				.entry(mesh_ref)
				.or_insert_with(|| match sources.get(&mesh_ref) {
					Some(source) => load_asset(ctx, source)
						.map(Arc::new)
						.map_err(|e| format!("{}: {e}", source.name)),
					None => Err("its mesh is not in this workspace".into()),
				});
			match asset {
				Ok(asset) => PartShape::Mesh(asset.clone()),
				Err(e) => {
					*missing += 1;
					PartShape::Missing(e.clone())
				}
			}
		}
	}
}

/// The centre and per-axis scale fitting a part's shape to its `size`,
/// then an offset in the part's space.
fn fit(shape: &PartShape, look: &Look, size: V3) -> (V3, V3, V3) {
	match (shape, look) {
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
					if extent > MIN_FIT_EXTENT {
						scale[k] = size[k] / extent;
					}
				}
				(centre, scale, [0.0; 3])
			}
			None => ([0.0; 3], [1.0; 3], [0.0; 3]),
		},
		_ => ([0.0; 3], size, [0.0; 3]),
	}
}

/// The summary warnings about the parts, in a fixed order.
fn load_warnings(warnings: &mut Vec<String>, counts: &LoadCounts, parts: &[PartView]) {
	let variants_on_meshes = counts.variants_on_meshes;
	if variants_on_meshes > 0 {
		warnings.push(format!(
			"{variants_on_meshes} mesh parts have a MaterialVariant, drawn tiled over the part's box faces as on ordinary parts; Studio lays it out differently on meshes."
		));
	}
	let variants_missing = counts.partial_variants;
	if variants_missing > 0 {
		warnings.push(format!(
			"{variants_missing} parts show their MaterialVariant without some of its maps: they are not downloaded yet."
		));
	}
	let decals_missing = counts.missing_decals;
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
	let missing = counts.missing_parts;
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
		warnings.push(format!(
			"{unbound} bones inside skinned meshes (such as face bones) are not in the model. They hold their bind pose unless a clip animates them by name; facial animation (FACS) is not exported."
		));
	}
}

/// The model's clips, each bound to the rig or with why it cannot play.
fn clip_slots(model: &AnimatedModel, rig: &Rig, parts: &[PartView]) -> Vec<ClipSlot> {
	model
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
			let mesh_names: HashSet<&str> = parts
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
		.collect()
}

/// The scales applied to clip translations of each joint and each Bone
/// (see [`scale_translation`]). With a model scale, also sets every skin's.
fn translation_scales(
	model: &AnimatedModel,
	rig: &Rig,
	parts: &mut [PartView],
) -> (Vec<f32>, Vec<f32>) {
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
	let joint_scale = vec![model_scale.unwrap_or(1.0); rig.joints.len()];
	if let Some(s) = model_scale {
		for skin in parts.iter_mut().filter_map(|p| p.skin.as_mut()) {
			skin.track_scale = s;
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
	(joint_scale, bone_scale)
}

/// A warning when clips move parts no exported joint connects (the
/// exporters do read AnimationConstraints, so these are other or broken
/// joints).
fn stranded_warning(rig: &Rig, clips: &[ClipSlot]) -> Option<String> {
	let joined: HashSet<usize> = rig.joints.iter().flat_map(|m| [m.part0, m.part1]).collect();
	let loose: HashSet<&str> = rig
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
		.collect::<BTreeSet<_>>();
	(!stranded.is_empty()).then(|| {
		format!(
			"Its clips animate {} parts that no exported joint connects, so they stay still. Rigs built with AnimationConstraints (newer avatars) or other joint types are not in the export.",
			stranded.len()
		)
	})
}

/// Bounds of the visible parts' boxes at rest, for framing.
fn rest_bounds(parts: &[PartView], rig: &Rig, rest: &Posed) -> Option<(V3, V3)> {
	let mut bounds: Option<(V3, V3)> = None;
	for part in parts {
		if !part.visible || matches!(part.shape, PartShape::Missing(_)) {
			continue;
		}
		let rp = &rig.parts[part.index];
		let rest = rest.parts[part.index];
		let h = [rp.size[0] * 0.5, rp.size[1] * 0.5, rp.size[2] * 0.5];
		for local in box_corners(h) {
			let p = rest.point(local);
			grow_bounds(bounds.get_or_insert((p, p)), p);
		}
	}
	bounds
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
		warnings.extend(rest_pose_warning(model, rig.rest_error()));

		let mut assets = HashMap::new();
		let mut images = ImageCache::new(ctx, images);
		let mut counts = LoadCounts::default();
		// classic clothing: the images over each body section, and each
		// section's box in the joints' rest pose
		let (clothing, sections) = match &model.clothing {
			Some(c) => (
				clothing_layers(c, &mut images, &mut warnings),
				body_sections(&rig, &rig.joint_rest()),
			),
			None => (Vec::new(), HashMap::new()),
		};
		let mut parts = Vec::with_capacity(rig.parts.len());
		for (i, part) in rig.parts.iter().enumerate() {
			// the rig keeps the parts in the record's order
			let record = &model.parts[i];
			let look = look(record);
			let shape = part_shape(
				ctx,
				&look,
				part.mesh_ref,
				sources,
				&mut assets,
				&mut counts.missing_parts,
			);
			let (centre, scale, offset) = fit(&shape, &look, part.size);
			let approximate = match &look {
				Look::Primitive { approximate, .. } => approximate.clone(),
				_ => None,
			};
			let colour = match &shape {
				PartShape::Mesh(_) => colour_of(record, PART_COLOUR),
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
			let mut decals = part_decals(record, &mut images, &mut counts.missing_decals);
			// a MaterialVariant covers the part, under its decals
			if let Some(v) = &record.material_variant
				&& let Some(layer) = variant_layer(v, &mut images, &mut counts.partial_variants)
			{
				if matches!(shape, PartShape::Mesh(_)) {
					counts.variants_on_meshes += 1;
				}
				decals.insert(0, (i32::MIN, layer));
			}
			// clothing over the body part, under its decals
			if let Some(section) = sections.get(&i) {
				insert_clothing(&mut decals, section, &clothing);
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
		load_warnings(&mut warnings, &counts, &parts);
		let clips = clip_slots(model, &rig, &parts);
		let (joint_scale, bone_scale) = translation_scales(model, &rig, &mut parts);
		warnings.extend(stranded_warning(&rig, &clips));

		let bounds = rest_bounds(&parts, &rig, &rest);
		let mut view = Self {
			joint_scale,
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
			camera: Camera::framing_bounds(bounds),
			bounds,
			warnings,
			show_blocks: true,
			scale: model.scale,
			captured_pose: model.joints.iter().any(|j| j.transform.is_some()),
			dirty: true,
			highlight: None,
			clips,
			parts,
		};
		// start playing the first clip that can play
		if let Some(first) = view.clips.iter().position(|c| c.clip.is_ok()) {
			view.select(Some(first));
			view.playing = true;
		}
		view
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
						Some(Target::Joint(m)) => scale_translation(s.1, self.joint_scale[*m]),
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

/// Test fixtures shared by the model view's test modules.
#[cfg(test)]
mod fixtures {
	use super::*;
	use crate::anim::schema::ModelPart;
	use crate::scene::geometry::GroupKind;

	pub(super) fn cf(pos: V3) -> [f32; 12] {
		CFrame::from_position(pos).components()
	}

	pub(super) fn asset(positions: Vec<V3>, skin: Skin) -> MeshAsset {
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

	pub(super) fn part_with(class: Option<&str>, shape: Option<&str>, size: V3) -> ModelPart {
		ModelPart {
			class_name: class.map(String::from),
			shape: shape.map(String::from),
			size,
			..Default::default()
		}
	}

	/// A unit part of the model at `Workspace/Dummy`, placed at `at`.
	pub(super) fn placed_part(
		class: Option<&str>,
		shape: Option<&str>,
		name: &str,
		at: V3,
	) -> ModelPart {
		ModelPart {
			name: name.into(),
			path: format!("Workspace/Dummy/{name}"),
			cframe: cf(at),
			..part_with(class, shape, [1.0; 3])
		}
	}
}

#[cfg(test)]
mod tests {
	use super::decals::Section;
	use super::fixtures::placed_part;
	use super::*;

	/// Poses every playable clip of `view` at 21 times, checking that each
	/// pose is finite and the draw count adds up. Returns the poses solved.
	fn play_every_clip(view: &mut ModelView) -> usize {
		let mut frames = 0;
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
						view.path,
						view.clips[c].name
					);
				}
				// a visible part draws once, plus once for each decal,
				// clothing layer and MaterialVariant over it
				let most: usize = view
					.parts
					.iter()
					.filter(|p| p.visible)
					.map(|p| 1 + p.decals.len())
					.sum();
				assert!(n <= most, "{n} draws for at most {most}");
			}
		}
		frames
	}

	/// Plays every clip of every model in a real export (without meshes):
	/// `MESHVAULT_EXPORT=path cargo test -p meshvault --release -- --ignored plays_real_export --nocapture`
	/// (`RBX_V1_EXPORT` is still read when `MESHVAULT_EXPORT` is not set).
	#[test]
	#[ignore]
	fn plays_real_export() {
		let path = crate::test_support::real_export();
		let export =
			crate::studio::export_file::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
		let ctx = egui::Context::default();
		let mut frames = 0;
		for (i, model) in export.models.iter().enumerate() {
			let mut view = ModelView::load(&ctx, i, model, &HashMap::new(), &HashMap::new());
			frames += play_every_clip(&mut view);
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

	/// Opens every model of a real workspace as the app does, with its
	/// downloaded meshes, maps and images, so clothing, decals and
	/// MaterialVariants draw; plays every clip and prints each model's
	/// warnings and layer counts. Reads the workspace, never writes it:
	/// `MESHVAULT_WS=<workspace folder> cargo test -p meshvault --release -- --ignored plays_workspace_models --nocapture`
	#[test]
	#[ignore]
	fn plays_workspace_models() {
		let root = std::env::var("MESHVAULT_WS").expect("set MESHVAULT_WS to a workspace folder");
		let ws = crate::workspace::Workspace::open(std::path::Path::new(&root)).unwrap();
		let ctx = egui::Context::default();
		let mut frames = 0;
		for i in 0..ws.models().len() {
			let load = crate::workspace::view_loader::model_loader(&ws, i).unwrap();
			let mut view = match load(&ctx) {
				Ok(view) => view,
				Err(e) => {
					println!("{}: {e}", ws.models()[i].name);
					continue;
				}
			};
			frames += play_every_clip(&mut view);
			let layers: usize = view.parts.iter().map(|p| p.decals.len()).sum();
			println!(
				"{}: {} parts, {} layers, {} clips",
				view.path,
				view.parts.len(),
				layers,
				view.clips.len()
			);
			for w in &view.warnings {
				println!("  {w}");
			}
		}
		println!("{frames} poses solved");
	}

	/// A part and one clip of `duration` seconds, playing from the start.
	fn view_with_clip(duration: f32, looped: bool) -> ModelView {
		use crate::anim::rig::Binding;
		let model = AnimatedModel {
			name: "Dummy".into(),
			path: "Workspace/Dummy".into(),
			parts: vec![placed_part(Some("Part"), Some("Block"), "Torso", [0.0; 3])],
			..Default::default()
		};
		let ctx = egui::Context::default();
		let mut view = ModelView::load(&ctx, 0, &model, &HashMap::new(), &HashMap::new());
		assert!(!view.playing && view.current.is_none(), "no clips yet");
		view.clips.push(ClipSlot {
			name: "Clip".into(),
			status: ClipStatus::KeyframeSequence,
			priority: String::new(),
			animation_id: String::new(),
			clip: Ok(Clip {
				duration,
				looped,
				tracks: Vec::new(),
				markers: Vec::new(),
				skipped_curves: 0,
				face: Vec::new(),
			}),
			binding: Binding::default(),
			mesh_bone_tracks: 0,
		});
		view.select(Some(0));
		view.playing = true;
		view
	}

	/// A looping clip wraps round at its end, at any speed.
	#[test]
	fn looping_clips_wrap_round() {
		let mut view = view_with_clip(2.0, true);
		assert!(view.looping, "select takes the clip's looping");
		assert!(view.tick(0.5));
		assert_eq!(view.time, 0.5);
		assert!(view.tick(2.0));
		assert_eq!(view.time, 0.5);
		view.speed = 2.0;
		assert!(view.tick(1.0));
		assert_eq!(view.time, 0.5);
		// paused, time stands still
		view.toggle_play();
		assert!(!view.tick(1.0));
		assert_eq!(view.time, 0.5);
		view.toggle_play();
		assert!(view.playing && view.time == 0.5, "resumes where it was");
	}

	/// A one-shot clip stops on its last frame; playing it again from
	/// there starts it over, but playing it after a pause does not.
	#[test]
	fn one_shot_clips_stop_at_the_end_and_restart() {
		let mut view = view_with_clip(2.0, false);
		assert!(view.tick(1.5));
		assert_eq!(view.time, 1.5);
		assert!(!view.tick(1.0));
		assert_eq!((view.time, view.playing), (2.0, false));
		assert!(!view.tick(1.0), "a stopped clip does not move");
		assert_eq!(view.time, 2.0);
		view.toggle_play();
		assert_eq!((view.time, view.playing), (0.0, true));
		view.tick(0.5);
		view.toggle_play();
		view.toggle_play();
		assert_eq!((view.time, view.playing), (0.5, true));
		// the rest pose plays nothing
		view.reset();
		assert!(!view.playing && view.current.is_none());
		view.toggle_play();
		assert!(!view.playing && !view.tick(1.0));
	}

	/// A clip of no length stays at time 0. Pinned as it is today: even a
	/// one-shot one keeps playing, so tick keeps asking for frames (bug
	/// B24). The fix flips the one-shot half of this test.
	#[test]
	fn zero_length_clips_stay_at_the_start_and_keep_playing() {
		for looped in [true, false] {
			let mut view = view_with_clip(0.0, looped);
			assert!(view.tick(0.1), "looped {looped}");
			assert_eq!((view.time, view.playing), (0.0, true), "looped {looped}");
		}
	}

	// ---------------------------------------------- characterisation (SCN-11a)
	//
	// These pin what `load` and the export do today, before the scene code
	// is split up. Where today's behaviour looks wrong it is pinned anyway
	// and said so: fixing it is separate work.

	/// Writes a small PNG, so loading it gives a real texture.
	fn png(dir: &std::path::Path, name: &str) -> PathBuf {
		let path = dir.join(name);
		image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 100, 50, 255]))
			.save(&path)
			.unwrap();
		path
	}

	fn decal(class: &str, id: &str, face: &str, z_index: i32) -> crate::anim::schema::DecalInfo {
		crate::anim::schema::DecalInfo {
			class_name: class.into(),
			texture: id.into(),
			face: face.into(),
			color: [1.0; 3],
			z_index,
			studs_per_tile_u: Some(2.0),
			..Default::default()
		}
	}

	fn file_name(t: &texture::Texture) -> String {
		t.path.file_name().unwrap().to_string_lossy().into_owned()
	}

	/// Each layer over a part, bottom first, as text.
	fn layers(part: &PartView) -> Vec<String> {
		part.decals
			.iter()
			.map(|d| {
				let maps: Vec<String> = d.material.maps.values().map(file_name).collect();
				let maps = maps.join("+");
				if d.variant {
					format!("variant {maps}")
				} else if d.clothing.is_some() {
					format!("clothing {maps}")
				} else {
					format!("decal face {} {maps}", d.face)
				}
			})
			.collect()
	}

	/// A MaterialVariant goes under the clothing, the clothing (pants, shirt,
	/// then T-shirt) under the decals, and the decals by ZIndex with ties in
	/// the order they were exported. The warnings come in a fixed order.
	#[test]
	fn layers_and_warnings_keep_their_order() {
		use crate::anim::schema::{ClothingInfo, MaterialVariantInfo};
		let dir = crate::test_support::TempDir::new("layers");
		let mut images = HashMap::new();
		for (id, name) in [
			(11, "front.png"),
			(12, "top.png"),
			(13, "back.png"),
			(20, "variant.png"),
			(30, "shirt.png"),
			(31, "pants.png"),
			(32, "tshirt.png"),
		] {
			images.insert(id, png(&dir, name));
		}
		// downloaded but not an image: still counted as not downloaded
		let bad = dir.join("bad.png");
		std::fs::write(&bad, b"not an image").unwrap();
		images.insert(99, bad);

		let mut torso = placed_part(Some("MeshPart"), None, "Torso", [0.0; 3]);
		torso.size = [2.0, 2.0, 1.0];
		torso.mesh_ref = 1;
		torso.decals = vec![
			decal("Decal", "rbxassetid://11", "Front", 2),
			decal("Texture", "12", "Top", -1),
			decal("Decal", "rbxassetid://13", "Back", 2),
			// not in the images at all
			decal("Decal", "rbxassetid://14", "Front", 0),
			decal("Decal", "rbxassetid://99", "Front", 0),
		];
		torso.material_variant = Some(MaterialVariantInfo {
			name: "Fabric".into(),
			studs_per_tile: 4.0,
			color_map: "rbxassetid://20".into(),
			// not downloaded
			normal_map: "rbxassetid://21".into(),
			..Default::default()
		});
		let leg = placed_part(Some("Part"), Some("Block"), "Left Leg", [0.0, -2.0, 0.0]);
		let union = placed_part(Some("UnionOperation"), None, "Union", [3.0, 0.0, 0.0]);
		let mut gone = placed_part(Some("MeshPart"), None, "Gone", [-3.0, 0.0, 0.0]);
		gone.mesh_ref = 2;
		let model = AnimatedModel {
			name: "Dummy".into(),
			path: "Workspace/Dummy".into(),
			parts: vec![torso, leg, union, gone],
			clothing: Some(ClothingInfo {
				shirt_template: Some("rbxassetid://30".into()),
				shirt_color: Some([1.0, 0.0, 0.0]),
				pants_template: Some("rbxassetid://31".into()),
				pants_color: Some([0.0, 1.0, 0.0]),
				graphic: Some("rbxassetid://32".into()),
				graphic_color: None,
			}),
			..Default::default()
		};
		let sources = HashMap::from([(
			1,
			MeshSource {
				name: "torso".into(),
				mesh: Ok(
					std::path::Path::new(crate::test_support::SAMPLE_MESHES).join("torso.mesh")
				),
				maps: Vec::new(),
			},
		)]);
		let ctx = egui::Context::default();
		let view = ModelView::load(&ctx, 0, &model, &sources, &images);

		assert!(matches!(view.parts[0].shape, PartShape::Mesh(_)));
		assert_eq!(
			layers(&view.parts[0]),
			[
				"variant variant.png",
				"clothing pants.png",
				"clothing shirt.png",
				"clothing tshirt.png",
				"decal face 1 top.png",
				"decal face 5 front.png",
				"decal face 2 back.png",
			]
		);
		let torso = &view.parts[0].decals;
		// the clothing takes its colours; a T-shirt covers only the front
		assert_eq!(torso[1].colour, [0.0, 1.0, 0.0]);
		assert_eq!(torso[2].colour, [1.0, 0.0, 0.0]);
		assert_eq!(torso[3].colour, [1.0; 3]);
		let regions = |d: &PartDecal| d.clothing.as_ref().unwrap().0.regions;
		assert_eq!(regions(&torso[1]), Section::Torso.regions());
		assert_eq!(regions(&torso[3])[5], [0.0, 0.0, 1.0, 1.0]);
		assert!(regions(&torso[3])[..5].iter().all(|r| *r == [0.0; 4]));
		// only a Texture tiles; a variant tiles by its StudsPerTile
		assert_eq!(torso[4].tiles, Some([2.0, 2.0, 0.0, 0.0]));
		assert_eq!(torso[5].tiles, None);
		assert_eq!(torso[0].tiles, Some([4.0, 4.0, 0.0, 0.0]));
		// a leg wears the pants only
		assert_eq!(layers(&view.parts[1]), ["clothing pants.png"]);
		assert!(view.parts[2].decals.is_empty());

		assert_eq!(
			view.warnings,
			[
				"1 mesh parts have a MaterialVariant, drawn tiled over the part's box faces as on ordinary parts; Studio lays it out differently on meshes.",
				"1 parts show their MaterialVariant without some of its maps: they are not downloaded yet.",
				// one of these two is downloaded but cannot be decoded: the
				// message is misleading for it (a known bug, kept for now)
				"2 decals are not drawn: their images are not downloaded yet.",
				"1 parts are drawn approximately because Meshvault has no geometry for their shape: a UnionOperation (drawn as a box).",
				"1 parts are not shown because their meshes are not downloaded or could not be read. Hover them in the part list for details.",
			]
		);
	}

	/// Clothing, decals and MaterialVariants share one image cache: an image
	/// is loaded once for all of them, and an image that fails fails for
	/// all of them.
	#[test]
	fn clothing_decals_and_variants_share_their_images() {
		use crate::anim::schema::{ClothingInfo, MaterialVariantInfo};
		let dir = crate::test_support::TempDir::new("image_cache");
		let bad = dir.join("bad.png");
		std::fs::write(&bad, b"not an image").unwrap();
		let images = HashMap::from([(41, png(&dir, "good.png")), (40, bad)]);
		let model = |id: &str| {
			let mut torso = placed_part(Some("Part"), Some("Block"), "Torso", [0.0; 3]);
			torso.decals = vec![decal("Decal", id, "Front", 0)];
			torso.material_variant = Some(MaterialVariantInfo {
				studs_per_tile: 4.0,
				color_map: id.into(),
				// not an asset id: skipped, so not counted as missing
				normal_map: "not an id".into(),
				..Default::default()
			});
			AnimatedModel {
				name: "Dummy".into(),
				path: "Workspace/Dummy".into(),
				parts: vec![torso],
				clothing: Some(ClothingInfo {
					shirt_template: Some(id.into()),
					..Default::default()
				}),
				..Default::default()
			}
		};
		let ctx = egui::Context::default();

		let view = ModelView::load(&ctx, 0, &model("rbxassetid://41"), &HashMap::new(), &images);
		assert!(view.warnings.is_empty(), "{:?}", view.warnings);
		let decals = &view.parts[0].decals;
		assert_eq!(
			layers(&view.parts[0]),
			[
				"variant good.png",
				"clothing good.png",
				"decal face 5 good.png"
			]
		);
		// one texture, uploaded once
		let ids: Vec<_> = decals
			.iter()
			.map(|d| d.material.maps[&Slot::Base].handle.id())
			.collect();
		assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");

		let view = ModelView::load(&ctx, 0, &model("rbxassetid://40"), &HashMap::new(), &images);
		assert!(view.parts[0].decals.is_empty());
		assert_eq!(
			view.warnings,
			[
				"Some of the model's clothing is not drawn: its images are not downloaded yet.",
				"1 parts show their MaterialVariant without some of its maps: they are not downloaded yet.",
				"1 decals are not drawn: their images are not downloaded yet.",
			]
		);
	}
}
