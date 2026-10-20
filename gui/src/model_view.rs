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
use crate::anim::schema::{AnimatedModel, ClipStatus};
use crate::geometry::{self, Geometry, GroupKind, Skin};
use crate::gpu::GpuScene;
use crate::maps::Slot;
use crate::texture::{self, Material};
use crate::viewport::{self, Camera, ScenePart};

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
	/// A part without a file mesh, drawn as a box of its size.
	Block,
	/// Not drawn, and why.
	Missing(String),
}

pub struct PartView {
	/// Index into the rig's parts.
	pub index: usize,
	pub shape: PartShape,
	pub visible: bool,
	/// Mesh centre and per-axis scale fitting the mesh to the part's size.
	centre: V3,
	scale: V3,
	pub skin: Option<PartSkin>,
	gpu: Option<GpuScene>,
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
	/// Scale of the mesh each Bone moves (1 for none); see [`scale_translation`].
	bone_scale: Vec<f32>,
	/// The pose needs solving again (time, clip or rest changed).
	dirty: bool,
}

/// Parts this transparent are invisible in Roblox (HumanoidRootPart).
const INVISIBLE: f32 = 0.99;
const BLOCK_COLOUR: Color32 = Color32::from_rgb(140, 146, 156);

fn cube() -> &'static Geometry {
	static CUBE: OnceLock<Geometry> = OnceLock::new();
	CUBE.get_or_init(geometry::unit_cube)
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
			matched,
			bones: skin.bones.len(),
		};
		out.update(rest, rest, owner, &HashMap::new());
		out
	}

	/// Deforms the mesh into `posed`. `by_name` holds clip transforms for
	/// bones that exist only in meshes.
	fn update(
		&mut self,
		rest: &Posed,
		posed: &Posed,
		owner: usize,
		by_name: &HashMap<&str, CFrame>,
	) {
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
					let t = by_name
						.get(name.as_str())
						.map_or(CFrame::IDENTITY, |&t| scale_translation(t, self.unit_scale));
					(pn.mul(local).mul(&t), pr.mul(local))
				}
			};
		}
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
	) -> Self {
		let mut warnings = Vec::new();
		let rig = Rig::build(model);
		warnings.extend(rig.warnings.iter().cloned());
		let rest = rig.rest();
		let rest_error = rig.rest_error();
		if rest_error > 0.01 {
			warnings.push(format!(
				"The joints place parts up to {rest_error:.2} studs from where they were exported, so the model was probably captured mid-animation. Clips play from the joints' rest pose."
			));
		}

		let mut assets: HashMap<u32, Result<Arc<MeshAsset>, String>> = HashMap::new();
		let mut parts = Vec::with_capacity(rig.parts.len());
		let mut missing = 0;
		for (i, part) in rig.parts.iter().enumerate() {
			let shape = if part.mesh_ref == 0 {
				PartShape::Block
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
			let (centre, scale) = match &shape {
				PartShape::Mesh(asset) => match asset.geometry.bounds() {
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
						(centre, scale)
					}
					None => ([0.0; 3], [1.0; 3]),
				},
				_ => ([0.0; 3], part.size),
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
			parts.push(PartView {
				index: i,
				shape,
				visible: part.transparency < INVISIBLE,
				centre,
				scale,
				skin,
				gpu: None,
			});
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
		let bone_scale = (0..rig.bones.len())
			.map(|mut b| {
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

		let mut view = Self {
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
			dirty: true,
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
		let slot = self.current.and_then(|i| self.clips.get(i));
		let (posed, by_name) = match slot.and_then(|s| Some((s.clip.as_ref().ok()?, &s.binding))) {
			Some((clip, binding)) => {
				let samples: Vec<(&str, CFrame)> = clip
					.sample(self.time)
					.map(|(key, t)| (key.name.as_str(), t))
					.collect();
				let transforms: Vec<CFrame> = samples
					.iter()
					.zip(&binding.targets)
					.map(|(s, target)| match target {
						Some(Target::Bone(b)) => scale_translation(s.1, self.bone_scale[*b]),
						_ => s.1,
					})
					.collect();
				let posed = self.rig.pose_from(binding, &transforms);
				let by_name: HashMap<&str, CFrame> = samples
					.iter()
					.zip(&binding.targets)
					.filter(|(_, t)| t.is_none())
					.map(|(s, _)| *s)
					.collect();
				(posed, by_name)
			}
			None => (self.rest.clone(), HashMap::new()),
		};
		for part in &mut self.parts {
			if let Some(skin) = &mut part.skin {
				skin.update(&self.rest, &posed, part.index, &by_name);
			}
		}
		self.posed = posed;
	}

	/// The parts to draw this frame. `plain` replaces mesh materials when
	/// maps are switched off.
	pub fn scene_parts<'a>(&'a mut self, plain: Option<&'a Material>) -> Vec<ScenePart<'a>> {
		self.update_pose();
		let posed = &self.posed;
		let show_blocks = self.show_blocks;
		self.parts
			.iter_mut()
			.filter(|p| p.visible)
			.filter_map(|part| {
				let world = posed.parts[part.index];
				let PartView {
					shape,
					skin,
					gpu,
					centre,
					scale,
					..
				} = part;
				match shape {
					PartShape::Mesh(asset) => {
						let material = plain.unwrap_or(&asset.material);
						Some(match skin {
							Some(skin) => {
								let moved = std::mem::take(&mut skin.moved);
								ScenePart {
									geometry: &skin.geometry,
									groups: &asset.groups,
									transform: viewport::IDENTITY,
									material,
									colour_groups: false,
									colour: viewport::PART_COLOUR,
									scene: gpu,
									moved,
								}
							}
							None => ScenePart {
								geometry: &asset.geometry,
								groups: &asset.groups,
								transform: placement(&world, *scale, *centre),
								material,
								colour_groups: false,
								colour: viewport::PART_COLOUR,
								scene: gpu,
								moved: false,
							},
						})
					}
					PartShape::Block if show_blocks => Some(ScenePart {
						geometry: cube(),
						groups: &[0],
						transform: placement(&world, *scale, [0.0; 3]),
						material: plain.unwrap_or_else(|| empty_material()),
						colour_groups: false,
						colour: BLOCK_COLOUR,
						scene: gpu,
						moved: false,
					}),
					_ => None,
				}
			})
			.collect()
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
				PartShape::Block if self.show_blocks => 12,
				_ => 0,
			})
			.sum()
	}
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
		ps.update(&rest, &posed, 0, &HashMap::new());
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
		};
		let a = asset(vec![[0.0, 0.0, 2.0]; 3], skin.clone());
		let mut ps = PartSkin::build(&skin, &a, &rig, &rest, 0, [0.0; 3], [1.0; 3]);
		assert_eq!(ps.matched, 1);
		assert_eq!(ps.geometry.positions[0], [0.0, 0.0, 2.0]);
		// a track named Jaw moving it up by 1
		let by_name = HashMap::from([("Jaw", CFrame::from_position([0.0, 1.0, 0.0]))]);
		ps.update(&rest, &rest, 0, &by_name);
		let p = ps.geometry.positions[0];
		assert!(
			(p[1] - 1.0).abs() < 1e-5 && (p[2] - 2.0).abs() < 1e-5,
			"{p:?}"
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
		let export = crate::export::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
		let ctx = egui::Context::default();
		let mut frames = 0;
		for (i, model) in export.models.iter().enumerate() {
			let mut view = ModelView::load(&ctx, i, model, &HashMap::new());
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
