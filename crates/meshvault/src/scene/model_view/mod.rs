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
use std::sync::Arc;

use eframe::egui::{self, Color32};

use crate::anim::clip::{Clip, ClipMarker};
use crate::anim::math::{CFrame, V3};
use crate::anim::rig::{Binding, BoneParent, Posed, Rig, Target};
use crate::anim::schema::{AnimatedModel, ClipStatus};
use crate::render::camera::Camera;
use crate::render::gpu::GpuScene;
use crate::scene::geometry::{self, Geometry, Primitive, Skin};
use crate::scene::maps::Slot;
use crate::scene::surface::{ClothingDraw, PART_COLOUR, Surface};
use crate::scene::texture::{self, Material};

mod decals;
mod draw;
mod export;
mod look;
mod skin;

pub use self::draw::{Joint, Skeleton};
pub use self::export::ExportMesh;
pub use self::skin::PartSkin;

use self::decals::{PartDecal, SECTIONS, Section, face_index};
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
	pub geometry: Geometry,
	/// Groups drawn: the finest LOD, or every hull or group.
	pub groups: Vec<usize>,
	/// `Ok(None)` when the mesh is not skinned.
	pub skin: Result<Option<Skin>, String>,
	pub material: Material,
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
	motor_scale: Vec<f32>,
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
	let loaded = geometry::load(&bytes)?;
	let geometry = loaded.geometry;
	if geometry.triangles.is_empty() {
		return Err(loaded.note.unwrap_or_else(|| "the mesh is empty".into()));
	}
	let groups = geometry.default_groups();
	let skin = geometry::skin(&bytes, geometry.positions.len());
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
				let id = crate::content_id::parse_asset_id(content.as_deref()?)?;
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
			let mut decals: Vec<(i32, PartDecal)> = Vec::new();
			for d in &record.decals {
				let image = crate::content_id::parse_asset_id(&d.texture).and_then(|id| {
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
					let Some(id) = crate::content_id::parse_asset_id(content) else {
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

		// clips that move parts no exported joint connects (the exporters do
		// read AnimationConstraints, so these are other or broken joints)
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

fn log_unbound(warnings: &mut Vec<String>, unbound: usize) {
	warnings.push(format!(
		"{unbound} bones inside skinned meshes (such as face bones) are not in the model. They hold their bind pose unless a clip animates them by name; facial animation (FACS) is not exported."
	));
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
	use super::fixtures::placed_part;
	use super::*;

	/// Plays every clip of every model in a real export (without meshes):
	/// `MESHVAULT_EXPORT=path cargo test -p meshvault --release -- --ignored plays_real_export --nocapture`
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
