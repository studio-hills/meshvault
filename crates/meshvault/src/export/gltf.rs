//! Exports models as binary glTF (`.glb`), the format Blender imports
//! best, with their maps embedded.
//!
//! An animated model becomes one armature: every part and Bone is a joint
//! (under a root named after the model), every mesh is skinned to it (a
//! rigid part's mesh fully to its part), and each clip is baked into
//! keyframes at [`FPS`], so Blender plays it as Meshvault does: easing,
//! poses for bones only meshes have, and face controls included. Lengths
//! are in studs (Blender reads them as metres).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui::Color32;
use serde_json::{Value, json};

use crate::anim::math::CFrame;
use crate::scene::colour::{linear_rgb, linear_to_srgb, srgb_to_linear, srgb8_to_linear};
use crate::scene::geometry::Geometry;
use crate::scene::maps::Slot;
use crate::scene::model_view::{ExportMesh, ModelView};
use crate::scene::surface::{AlphaMode, DEFAULT_ROUGHNESS, Surface};
use crate::scene::texture::Material;

/// Keyframes per second of baked clips.
const FPS: f32 = 30.0;

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
/// Sampler wrap and filter modes.
const REPEAT: u32 = 10497;
const LINEAR: u32 = 9729;
const LINEAR_MIPMAP_LINEAR: u32 = 9987;
/// A GLB file starts with a 12-byte header (magic, version, length), and
/// each chunk with an 8-byte one (length, type).
const GLB_HEADER_LEN: usize = 12;
const CHUNK_HEADER_LEN: usize = 8;

/// What each element of an accessor is.
#[derive(Clone, Copy)]
enum Accessor {
	Scalar,
	Vec2,
	Vec3,
	Vec4,
	Mat4,
}

impl Accessor {
	/// The accessor's `type` in the document.
	fn name(self) -> &'static str {
		match self {
			Accessor::Scalar => "SCALAR",
			Accessor::Vec2 => "VEC2",
			Accessor::Vec3 => "VEC3",
			Accessor::Vec4 => "VEC4",
			Accessor::Mat4 => "MAT4",
		}
	}

	/// Components per element.
	fn width(self) -> usize {
		match self {
			Accessor::Scalar => 1,
			Accessor::Vec2 => 2,
			Accessor::Vec3 => 3,
			Accessor::Vec4 => 4,
			Accessor::Mat4 => 16,
		}
	}
}

/// Pads `buf` with `byte` to a multiple of 4 bytes, as GLB chunks and
/// buffer views must be aligned.
fn pad_to_4(buf: &mut Vec<u8>, byte: u8) {
	while !buf.len().is_multiple_of(4) {
		buf.push(byte);
	}
}

/// Writes an animated model, with its clips, to `path`.
pub fn export_model(model: &ModelView, path: &Path) -> Result<(), String> {
	let mut g = Gltf::default();
	let nodes = model.export_nodes();
	// every clip's frames, counted and checked against the budget before
	// any is made
	let frames: Vec<usize> = model
		.clips
		.iter()
		.filter_map(|slot| slot.clip.as_ref().ok())
		.map(|clip| frame_count(clip.duration))
		.collect::<Result<_, _>>()?;
	// every node is keyed in every frame, the root not
	frames_fit(&frames, nodes.len())?;
	let rest = model.export_frame(None, 0.0);
	// a root above everything, so the armature has one
	let root = nodes.len();
	let parent_of = |n: usize| nodes.get(n).map(|node| node.parent.unwrap_or(root));
	let local = |worlds: &[CFrame], n: usize| match parent_of(n) {
		Some(p) if p != root => worlds[p].inverse().mul(&worlds[n]),
		_ => worlds[n],
	};
	let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len() + 1];
	for n in 0..nodes.len() {
		children[parent_of(n).unwrap_or(root)].push(n);
	}
	for (n, node) in nodes.iter().enumerate() {
		let l = local(&rest, n);
		let mut value = json!({
			"name": node.name,
			"translation": l.pos,
			"rotation": l.quat(),
		});
		if !children[n].is_empty() {
			value["children"] = json!(children[n]);
		}
		g.nodes.push(value);
	}
	g.nodes
		.push(json!({ "name": model.name, "children": children[root] }));

	// one skin over every node, so Blender builds one armature
	let joints: Vec<usize> = (0..=root).collect();
	let mut inverse_binds = Vec::with_capacity(joints.len() * 16);
	for world in &rest[..nodes.len()] {
		inverse_binds.extend(column_major(&world.inverse()));
	}
	inverse_binds.extend(column_major(&CFrame::IDENTITY));
	let ibm = g.floats(&inverse_binds, Accessor::Mat4, None, None);
	g.skins.push(json!({
		"name": model.name,
		"joints": joints,
		"skeleton": root,
		"inverseBindMatrices": ibm,
	}));

	let mut scene = vec![root];
	for mesh in model.export_meshes() {
		if let Some(index) = g.skinned_mesh(&mesh) {
			scene.push(g.nodes.len());
			g.nodes
				.push(json!({ "name": mesh.name, "mesh": index, "skin": 0 }));
		}
	}
	g.scene = scene;

	// each clip, baked
	for (i, slot) in model.clips.iter().enumerate() {
		let Ok(clip) = &slot.clip else { continue };
		// one clip's times at a time
		let times = key_times(clip.duration)?;
		let frames = times.len();
		let locals: Vec<Vec<CFrame>> = times
			.iter()
			.map(|&t| {
				let worlds = model.export_frame(Some(i), t);
				(0..nodes.len()).map(|n| local(&worlds, n)).collect()
			})
			.collect();
		let input = g.floats(&times, Accessor::Scalar, None, Some(min_max(&times, 1)));
		// every node is keyed, so Blender does not leave a bone this clip
		// does not move where another action put it; one that holds still
		// gets two keys
		let ends = [times[0], times[frames - 1]];
		let hold = g.floats(&ends, Accessor::Scalar, None, Some(min_max(&ends, 1)));
		let mut samplers = Vec::new();
		let mut channels = Vec::new();
		for n in 0..nodes.len() {
			let first = locals[0][n];
			let moves = locals.iter().any(|f| !same(&f[n], &first));
			let (keys, input): (Vec<CFrame>, usize) = if moves {
				(locals.iter().map(|f| f[n]).collect(), input)
			} else {
				(vec![first; 2], hold)
			};
			let translations: Vec<f32> = keys.iter().flat_map(|k| k.pos).collect();
			let mut rotations: Vec<f32> = Vec::with_capacity(keys.len() * 4);
			let mut last: Option<[f32; 4]> = None;
			for k in &keys {
				let mut q = k.quat();
				// the shorter way round from the last key
				if let Some(p) = last
					&& p.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>() < 0.0
				{
					q = q.map(|v| -v);
				}
				rotations.extend(q);
				last = Some(q);
			}
			for (values, ty, path) in [
				(translations, Accessor::Vec3, "translation"),
				(rotations, Accessor::Vec4, "rotation"),
			] {
				let output = g.floats(&values, ty, None, None);
				channels.push(json!({
					"sampler": samplers.len(),
					"target": { "node": n, "path": path },
				}));
				samplers
					.push(json!({ "input": input, "output": output, "interpolation": "LINEAR" }));
			}
		}
		g.animations
			.push(json!({ "name": slot.name, "channels": channels, "samplers": samplers }));
	}
	g.write(path)
}

/// Writes one mesh (no skeleton) to `path`, coloured `colour` where it has
/// no colour map.
pub fn export_mesh(
	geometry: &Geometry,
	groups: &[usize],
	material: &Material,
	colour: Color32,
	name: &str,
	path: &Path,
) -> Result<(), String> {
	let mut g = Gltf::default();
	let index = g.mesh(MeshInput {
		name,
		geometry,
		groups,
		positions: &geometry.positions,
		normals: geometry.normals.as_deref(),
		skin: None,
		material,
		colour,
		surface: &Surface::OPAQUE,
	});
	let Some(index) = index else {
		return Err("the mesh has no triangles".into());
	};
	g.nodes.push(json!({ "name": name, "mesh": index }));
	g.scene = vec![0];
	g.write(path)
}

/// A colour map as Meshvault draws it on a part (see `mesh.wgsl`): laid
/// over the part's colour by its alpha (Overlay), tinted where it is opaque
/// (TintMask), or tinted (Transparency keeps its alpha for a mask; Opaque
/// drops it). `None` when the map is drawn as it is.
fn bake(
	map: &image::RgbaImage,
	mode: AlphaMode,
	tint: [f32; 3],
	part: [f32; 3],
) -> Option<image::RgbaImage> {
	let white = tint.iter().all(|&t| (t - 1.0).abs() < 1e-4);
	let see_through = map.pixels().any(|p| p[3] < 255);
	let changes = match mode {
		AlphaMode::Overlay | AlphaMode::TintMask | AlphaMode::Opaque => see_through || !white,
		AlphaMode::Transparency | AlphaMode::Blend => !white,
	};
	if !changes {
		return None;
	}
	let decode: Vec<f32> = (0..=255).map(srgb8_to_linear).collect();
	let encode = |v: f32| {
		let v = v.clamp(0.0, 1.0);
		(linear_to_srgb(v) * 255.0).round() as u8
	};
	Some(image::RgbaImage::from_fn(
		map.width(),
		map.height(),
		|x, y| {
			let p = map.get_pixel(x, y);
			let a = p[3] as f32 / 255.0;
			let c = [0, 1, 2].map(|k| decode[p[k] as usize]);
			let rgb = [0, 1, 2].map(|k| match mode {
				AlphaMode::Overlay => part[k] + (c[k] * tint[k] - part[k]) * a,
				AlphaMode::TintMask => c[k] * (1.0 + (tint[k] - 1.0) * a),
				AlphaMode::Transparency | AlphaMode::Opaque | AlphaMode::Blend => c[k] * tint[k],
			});
			let alpha = if matches!(mode, AlphaMode::Transparency | AlphaMode::Blend) {
				p[3]
			} else {
				255
			};
			image::Rgba([encode(rgb[0]), encode(rgb[1]), encode(rgb[2]), alpha])
		},
	))
}

/// The most keys baked for one export: frames over all its clips times its
/// nodes. Each is 28 bytes in the file, so this keeps a file under 300 MB;
/// an hour-long clip of a 90-bone rig fits.
const MAX_BAKED_KEYS: usize = 10_000_000;

/// Whether clips of `frames` keys each, for `nodes` nodes, are within
/// [`MAX_BAKED_KEYS`]. A model with no nodes still has its clips sampled,
/// so each frame counts as at least one key.
fn frames_fit(frames: &[usize], nodes: usize) -> Result<(), String> {
	let keys = frames.iter().try_fold(0usize, |sum, &f| {
		sum.checked_add(f.checked_mul(nodes.max(1))?)
	});
	match keys {
		Some(keys) if keys <= MAX_BAKED_KEYS => Ok(()),
		_ => Err(format!(
			"The model's clips are too long to export: {} frames for {nodes} bones is more than {MAX_BAKED_KEYS} keys",
			frames.iter().fold(0usize, |a, &b| a.saturating_add(b))
		)),
	}
}

/// Frames at [`FPS`] before the end of a clip of `duration` seconds (the
/// end itself not counted), worked out without listing them. A clip
/// longer than [`MAX_CLIP_SECONDS`](crate::anim::clip::MAX_CLIP_SECONDS)
/// is an error.
fn frames_before_end(duration: f32) -> Result<usize, String> {
	if !(duration.is_finite() && duration <= crate::anim::clip::MAX_CLIP_SECONDS) {
		return Err(format!(
			"A clip is too long to export: it lasts {duration} seconds, and the most is an hour"
		));
	}
	let end = duration.max(0.0) - 1e-4;
	// the first frame `k` with `k / FPS >= end`, as f32 rounds it
	let mut k = if end > 0.0 {
		(end * FPS).ceil() as usize
	} else {
		0
	};
	while k > 0 && (k - 1) as f32 / FPS >= end {
		k -= 1;
	}
	while (k as f32) / FPS < end {
		k += 1;
	}
	Ok(k)
}

/// How many key times [`key_times`] gives a clip of `duration` seconds.
fn frame_count(duration: f32) -> Result<usize, String> {
	Ok((frames_before_end(duration)? + 1).max(2))
}

/// Key times of a baked clip: every frame at [`FPS`], and its end. A clip
/// that is a single pose holds it for one frame. A clip longer than
/// [`MAX_CLIP_SECONDS`](crate::anim::clip::MAX_CLIP_SECONDS) is an error.
fn key_times(duration: f32) -> Result<Vec<f32>, String> {
	let before = frames_before_end(duration)?;
	let times: Vec<f32> = if before == 0 {
		vec![0.0, 1.0 / FPS]
	} else {
		(0..before)
			.map(|k| k as f32 / FPS)
			.chain([duration.max(0.0)])
			.collect()
	};
	#[cfg(test)]
	TIMES_BUILT.with(|n| n.set(n.get() + times.len()));
	Ok(times)
}

#[cfg(test)]
thread_local! {
	/// Sample times built by [`key_times`], for tests.
	static TIMES_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A CFrame as a column-major 4 × 4 matrix.
fn column_major(cf: &CFrame) -> [f32; 16] {
	let m = cf.to_matrix([1.0; 3]);
	let mut out = [0.0; 16];
	for c in 0..4 {
		for r in 0..4 {
			out[c * 4 + r] = m[r][c];
		}
	}
	out
}

/// Per-component minimum and maximum of `values` taken `width` at a time.
fn min_max(values: &[f32], width: usize) -> (Vec<f32>, Vec<f32>) {
	let mut lo = vec![f32::MAX; width];
	let mut hi = vec![f32::MIN; width];
	for chunk in values.chunks(width) {
		for k in 0..width {
			lo[k] = lo[k].min(chunk[k]);
			hi[k] = hi[k].max(chunk[k]);
		}
	}
	(lo, hi)
}

/// One mesh for [`Gltf::mesh`]: `groups` of `geometry`, at `positions`
/// (which need not be the geometry's own) with `normals`, weighted to
/// joints by `skin`, and drawn with `material` over `colour` as `surface`.
struct MeshInput<'a> {
	name: &'a str,
	geometry: &'a Geometry,
	groups: &'a [usize],
	positions: &'a [[f32; 3]],
	normals: Option<&'a [[f32; 3]]>,
	/// Each vertex's four joints and weights.
	skin: Option<&'a [([u16; 4], [f32; 4])]>,
	material: &'a Material,
	colour: Color32,
	surface: &'a Surface,
}

/// A glTF document being built, with its one binary buffer.
#[derive(Default)]
struct Gltf {
	bin: Vec<u8>,
	views: Vec<Value>,
	accessors: Vec<Value>,
	meshes: Vec<Value>,
	materials: Vec<Value>,
	textures: Vec<Value>,
	images: Vec<Value>,
	nodes: Vec<Value>,
	skins: Vec<Value>,
	animations: Vec<Value>,
	scene: Vec<usize>,
	/// Texture index by the image file it shows.
	by_file: HashMap<PathBuf, usize>,
	/// Texture index of a colour map baked by [`bake`], by its file, alpha
	/// mode, tint and part colour.
	by_bake: HashMap<String, usize>,
	/// Material index by what it shows.
	by_look: HashMap<String, usize>,
}

impl Gltf {
	/// Appends `bytes` as a buffer view, 4-byte aligned.
	fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
		pad_to_4(&mut self.bin, 0);
		let mut view =
			json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() });
		if let Some(t) = target {
			view["target"] = json!(t);
		}
		self.bin.extend_from_slice(bytes);
		self.views.push(view);
		self.views.len() - 1
	}

	fn floats(
		&mut self,
		values: &[f32],
		ty: Accessor,
		target: Option<u32>,
		bounds: Option<(Vec<f32>, Vec<f32>)>,
	) -> usize {
		let view = self.view(bytemuck::cast_slice(values), target);
		let mut accessor = json!({
			"bufferView": view,
			"componentType": FLOAT,
			"count": values.len() / ty.width(),
			"type": ty.name(),
		});
		if let Some((lo, hi)) = bounds {
			accessor["min"] = json!(lo);
			accessor["max"] = json!(hi);
		}
		self.accessors.push(accessor);
		self.accessors.len() - 1
	}

	fn integers<T: bytemuck::Pod>(
		&mut self,
		values: &[T],
		component: u32,
		ty: Accessor,
		count: usize,
		target: u32,
	) -> usize {
		let view = self.view(bytemuck::cast_slice(values), Some(target));
		self.accessors.push(json!({
			"bufferView": view,
			"componentType": component,
			"count": count,
			"type": ty.name(),
		}));
		self.accessors.len() - 1
	}

	fn skinned_mesh(&mut self, m: &ExportMesh) -> Option<usize> {
		let skin: Vec<([u16; 4], [f32; 4])> = m
			.influences
			.iter()
			.map(|inf| {
				let total: f32 = inf.iter().map(|&(_, w)| w.max(0.0)).sum();
				let joints = inf.map(|(j, w)| {
					if w > 0.0 {
						m.joints[j as usize] as u16
					} else {
						0
					}
				});
				let weights = inf.map(|(_, w)| if total > 0.0 { w.max(0.0) / total } else { 0.0 });
				(joints, weights)
			})
			.collect();
		self.mesh(MeshInput {
			name: &m.name,
			geometry: m.geometry,
			groups: &m.groups,
			positions: &m.positions,
			normals: m.normals.as_deref(),
			skin: Some(&skin),
			material: m.material,
			colour: m.colour,
			surface: &m.surface,
		})
	}

	/// Adds `input` as a mesh, or nothing when its groups have no triangles.
	fn mesh(&mut self, input: MeshInput) -> Option<usize> {
		let MeshInput {
			name,
			geometry,
			groups,
			positions,
			normals,
			skin,
			material,
			colour,
			surface,
		} = input;
		let indices: Vec<u32> = groups
			.iter()
			.filter_map(|&g| geometry.groups.get(g))
			.flat_map(|r| geometry.triangles[r.clone()].iter().flatten().copied())
			.collect();
		if indices.is_empty() {
			return None;
		}
		let flat: Vec<f32> = positions.iter().flatten().copied().collect();
		let bounds = min_max(&flat, 3);
		let mut attributes = json!({ "POSITION": self.floats(&flat, Accessor::Vec3, Some(ARRAY_BUFFER), Some(bounds)) });
		if let Some(normals) = normals.filter(|n| n.len() == positions.len()) {
			let flat: Vec<f32> = normals
				.iter()
				.flat_map(|n| {
					let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
					if len > 1e-6 {
						n.map(|v| v / len)
					} else {
						[0.0, 1.0, 0.0]
					}
				})
				.collect();
			attributes["NORMAL"] =
				json!(self.floats(&flat, Accessor::Vec3, Some(ARRAY_BUFFER), None));
		}
		// Roblox and glTF both put the texture origin at the top left
		if let Some(uvs) = geometry.uvs.as_ref().filter(|u| u.len() == positions.len()) {
			let flat: Vec<f32> = uvs.iter().flatten().copied().collect();
			attributes["TEXCOORD_0"] =
				json!(self.floats(&flat, Accessor::Vec2, Some(ARRAY_BUFFER), None));
		}
		if let Some(skin) = skin {
			let joints: Vec<u16> = skin.iter().flat_map(|s| s.0).collect();
			let weights: Vec<f32> = skin.iter().flat_map(|s| s.1).collect();
			attributes["JOINTS_0"] = json!(self.integers(
				&joints,
				UNSIGNED_SHORT,
				Accessor::Vec4,
				skin.len(),
				ARRAY_BUFFER
			));
			attributes["WEIGHTS_0"] =
				json!(self.floats(&weights, Accessor::Vec4, Some(ARRAY_BUFFER), None));
		}
		let index_accessor = self.integers(
			&indices,
			UNSIGNED_INT,
			Accessor::Scalar,
			indices.len(),
			ELEMENT_ARRAY_BUFFER,
		);
		let mut primitive = json!({ "attributes": attributes, "indices": index_accessor });
		if let Some(m) = self.material(material, colour, surface, geometry.uvs.is_some()) {
			primitive["material"] = json!(m);
		}
		self.meshes
			.push(json!({ "name": name, "primitives": [primitive] }));
		Some(self.meshes.len() - 1)
	}

	/// The material showing `material`'s maps over `colour`, as Meshvault
	/// draws the surface.
	fn material(
		&mut self,
		material: &Material,
		colour: Color32,
		surface: &Surface,
		has_uvs: bool,
	) -> Option<usize> {
		let map = |slot| material.active(slot).filter(|_| has_uvs);
		let base = map(Slot::Base);
		let normal = map(Slot::Normal);
		let rough = map(Slot::Roughness);
		let metal = map(Slot::Metalness);
		let key = format!(
			"{:?}|{:?}|{:?}|{:?}|{colour:?}|{surface:?}",
			base.map(|t| &t.path),
			normal.map(|t| &t.path),
			rough.map(|t| &t.path),
			metal.map(|t| &t.path)
		);
		if let Some(&i) = self.by_look.get(&key) {
			return Some(i);
		}
		let part = linear_rgb(colour);
		let tint = surface.tint.map(|c| srgb_to_linear(c.clamp(0.0, 1.0)));
		// a colour map is baked as Meshvault draws it (over the part colour,
		// tinted), since glTF cannot mix them the way Roblox does
		let rgb = if base.is_some() { [1.0; 3] } else { part };
		let opacity = surface.opacity.clamp(0.0, 1.0);
		let mut pbr = json!({
			"baseColorFactor": [rgb[0], rgb[1], rgb[2], opacity],
			"metallicFactor": if metal.is_some() || surface.metal { 1.0 } else { 0.0 },
			"roughnessFactor": if rough.is_some() { 1.0 } else { DEFAULT_ROUGHNESS },
		});
		let base_texture = base.and_then(|t| match bake(&t.rgba, surface.alpha_mode, tint, part) {
			Some(baked) => {
				let key = format!(
					"{}|{:?}|{tint:?}|{part:?}",
					t.path.display(),
					surface.alpha_mode
				);
				if let Some(&i) = self.by_bake.get(&key) {
					return Some(i);
				}
				let i = self.texture_image(&baked)?;
				self.by_bake.insert(key, i);
				Some(i)
			}
			None => self.texture_file(&t.path, &t.rgba),
		});
		if let Some(t) = base_texture {
			pbr["baseColorTexture"] = json!({ "index": t });
		}
		// glTF packs roughness (green) and metalness (blue) in one image
		if rough.is_some() || metal.is_some() {
			let (w, h) = [rough, metal]
				.into_iter()
				.flatten()
				.map(|t| t.rgba.dimensions())
				.fold((1, 1), |(w, h), (a, b)| (w.max(a), h.max(b)));
			let fit = |t: Option<&crate::scene::texture::Texture>| {
				t.map(|t| {
					image::imageops::resize(
						t.rgba.as_ref(),
						w,
						h,
						image::imageops::FilterType::Triangle,
					)
				})
			};
			let (r, m) = (fit(rough), fit(metal));
			// without a metalness map the part's material decides, as it
			// does without any map: full metalness (times the factor of 1)
			// for metal, none for the rest
			let no_metal_map = if surface.metal { 255 } else { 0 };
			let packed = image::RgbaImage::from_fn(w, h, |x, y| {
				let g = r.as_ref().map_or(255, |r| r.get_pixel(x, y)[0]);
				let b = m.as_ref().map_or(no_metal_map, |m| m.get_pixel(x, y)[0]);
				image::Rgba([255, g, b, 255])
			});
			if let Some(t) = self.texture_image(&packed) {
				pbr["metallicRoughnessTexture"] = json!({ "index": t });
			}
		}
		let mut value = json!({ "pbrMetallicRoughness": pbr });
		if let Some(t) = normal.and_then(|t| self.texture_file(&t.path, &t.rgba)) {
			value["normalTexture"] = json!({ "index": t });
		}
		if surface.neon {
			value["emissiveFactor"] = json!(part);
		}
		if opacity < 0.999 || (surface.alpha_mode == AlphaMode::Blend && base.is_some()) {
			value["alphaMode"] = json!("BLEND");
		} else if surface.alpha_mode == AlphaMode::Transparency && base.is_some() {
			value["alphaMode"] = json!("MASK");
			value["alphaCutoff"] = json!(0.5);
		}
		self.materials.push(value);
		let i = self.materials.len() - 1;
		self.by_look.insert(key, i);
		Some(i)
	}

	/// A texture of an image file, embedded as it is when PNG or JPEG.
	fn texture_file(&mut self, path: &Path, decoded: &image::RgbaImage) -> Option<usize> {
		if let Some(&t) = self.by_file.get(path) {
			return Some(t);
		}
		let bytes = std::fs::read(path).ok();
		let t = match bytes.as_deref() {
			Some(b) if b.starts_with(b"\x89PNG") => self.texture_bytes(b, "image/png"),
			Some(b) if b.starts_with(&[0xFF, 0xD8]) => self.texture_bytes(b, "image/jpeg"),
			_ => self.texture_image(decoded)?,
		};
		self.by_file.insert(path.to_path_buf(), t);
		Some(t)
	}

	fn texture_image(&mut self, image: &image::RgbaImage) -> Option<usize> {
		let mut png = Vec::new();
		image
			.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
			.ok()?;
		Some(self.texture_bytes(&png, "image/png"))
	}

	fn texture_bytes(&mut self, bytes: &[u8], mime: &str) -> usize {
		let view = self.view(bytes, None);
		self.images
			.push(json!({ "bufferView": view, "mimeType": mime }));
		self.textures
			.push(json!({ "source": self.images.len() - 1, "sampler": 0 }));
		self.textures.len() - 1
	}

	fn write(mut self, path: &Path) -> Result<(), String> {
		pad_to_4(&mut self.bin, 0);
		let mut doc = json!({
			"asset": { "version": "2.0", "generator": "Meshvault" },
			"scene": 0,
			"scenes": [{ "nodes": self.scene }],
			"nodes": self.nodes,
			"buffers": [{ "byteLength": self.bin.len() }],
			"bufferViews": self.views,
			"accessors": self.accessors,
		});
		for (key, list) in [
			("meshes", self.meshes),
			("materials", self.materials),
			("textures", self.textures),
			("images", self.images),
			("skins", self.skins),
			("animations", self.animations),
		] {
			if !list.is_empty() {
				doc[key] = Value::Array(list);
			}
		}
		if doc.get("textures").is_some() {
			doc["samplers"] = json!([{
				"wrapS": REPEAT,
				"wrapT": REPEAT,
				"magFilter": LINEAR,
				"minFilter": LINEAR_MIPMAP_LINEAR,
			}]);
		}
		let mut text = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
		// the JSON chunk is padded with spaces, the binary one with zeros
		pad_to_4(&mut text, b' ');
		let total =
			GLB_HEADER_LEN + CHUNK_HEADER_LEN + text.len() + CHUNK_HEADER_LEN + self.bin.len();
		let mut out = Vec::with_capacity(total);
		out.extend_from_slice(b"glTF");
		out.extend_from_slice(&2u32.to_le_bytes());
		out.extend_from_slice(&(total as u32).to_le_bytes());
		out.extend_from_slice(&(text.len() as u32).to_le_bytes());
		out.extend_from_slice(b"JSON");
		out.extend_from_slice(&text);
		out.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
		out.extend_from_slice(b"BIN\0");
		out.extend_from_slice(&self.bin);
		std::fs::write(path, out).map_err(|e| format!("could not write {}: {e}", path.display()))
	}
}

/// Whether two transforms are the same, to well under what shows.
fn same(a: &CFrame, b: &CFrame) -> bool {
	let close = |x: f32, y: f32| (x - y).abs() < 1e-5;
	(0..3).all(|i| close(a.pos[i], b.pos[i]) && (0..3).all(|j| close(a.rot[i][j], b.rot[i][j])))
}

#[cfg(test)]
// geometry groups are lists of ranges, so one-element lists are intended
#[allow(clippy::single_range_in_vec_init)]
mod tests {
	use super::*;
	use crate::anim::schema::AnimatedModel;
	use crate::export::fixtures::parse_glb;
	use crate::scene::geometry::GroupKind;
	use crate::test_support::TempDir;

	#[test]
	fn key_times_increase_and_end_on_the_clip() {
		for duration in [0.0, 0.01, 1.0 / FPS, 0.5, 1.0, 2.345] {
			let t = key_times(duration).unwrap();
			assert_eq!(frame_count(duration).unwrap(), t.len(), "{duration}");
			assert!(t.len() >= 2, "{duration}: {t:?}");
			assert!(t.windows(2).all(|w| w[1] > w[0]), "{duration}: {t:?}");
			assert_eq!(*t.first().unwrap(), 0.0);
			if duration > 0.0 {
				assert!(
					(t.last().unwrap() - duration).abs() < 1e-6,
					"{duration}: {t:?}"
				);
			}
		}
	}

	/// A clip too long to bake, or more frames than the budget over all the
	/// nodes, is refused before any frame is baked.
	#[test]
	fn export_model_refuses_clips_too_long_to_bake() {
		for duration in [1e9, f32::INFINITY, f32::NAN] {
			let err = key_times(duration).unwrap_err();
			assert!(err.contains("too long"), "{duration}: {err}");
		}
		let longest = key_times(crate::anim::clip::MAX_CLIP_SECONDS).unwrap();
		assert_eq!(longest.len(), 108_001);
		// counting the frames agrees with listing them one by one, as they
		// were before, at every length
		let listed = |d: f32| {
			let n = (0..).take_while(|&k| (k as f32 / FPS) < d - 1e-4).count();
			(n + 1).max(2)
		};
		for frames in (0..108_000).step_by(997).chain(107_990..108_000) {
			for d in [
				frames as f32 / FPS,
				frames as f32 / FPS + 5e-5,
				frames as f32 / FPS + 0.01,
			] {
				let d = d.min(crate::anim::clip::MAX_CLIP_SECONDS);
				assert_eq!(frame_count(d).unwrap(), listed(d), "{d}");
				assert_eq!(key_times(d).unwrap().len(), listed(d), "{d}");
			}
		}
		assert!(frames_fit(&[longest.len(); 2], 2).is_ok());
		let err = frames_fit(&[longest.len(); 2], MAX_BAKED_KEYS / 100_000).unwrap_err();
		assert!(err.contains("too long"), "{err}");
		assert!(frames_fit(&[usize::MAX, 1], usize::MAX).is_err());

		// a clip made some other way than loading still fails, quickly
		let dir = TempDir::new("glb_long_clip");
		let ctx = eframe::egui::Context::default();
		let (sources, images) = (HashMap::new(), HashMap::new());
		let mut view = ModelView::load(&ctx, 0, &swinging_arm(true), &sources, &images);
		if let Ok(clip) = &mut view.clips[0].clip {
			clip.duration = 1e9;
		}
		let path = dir.join("long.glb");
		assert!(export_model(&view, &path).unwrap_err().contains("too long"));
		assert!(!path.exists());
	}

	/// Many clips, each a permitted hour long, are refused for their total
	/// before a single sample time is made, also for a model with no nodes.
	#[test]
	fn export_model_refuses_many_long_clips_before_sampling() {
		let dir = TempDir::new("glb_many_clips");
		let ctx = eframe::egui::Context::default();
		let (sources, images) = (HashMap::new(), HashMap::new());
		let hours = |mut model: AnimatedModel, clips: usize| {
			let mut clip = model.animations[0].clone();
			clip.duration = crate::anim::clip::MAX_CLIP_SECONDS;
			model.animations = vec![clip; clips];
			model
		};
		let mut empty = swinging_arm(true);
		empty.parts.clear();
		empty.joints.clear();
		// 2 nodes x 50 clips x 108,001 frames, and 0 nodes x 100 clips
		for (model, clips) in [(swinging_arm(true), 50), (empty, 100)] {
			let view = ModelView::load(&ctx, 0, &hours(model, clips), &sources, &images);
			assert!(view.clips.iter().all(|c| c.clip.is_ok()));
			TIMES_BUILT.with(|n| n.set(0));
			let path = dir.join("many.glb");
			let err = export_model(&view, &path).unwrap_err();
			assert!(err.contains("too long"), "{err}");
			assert_eq!(
				TIMES_BUILT.with(std::cell::Cell::get),
				0,
				"no times were made"
			);
			assert!(!path.exists());
		}
	}

	fn map(pixels: &[[u8; 4]]) -> image::RgbaImage {
		let mut image = image::RgbaImage::new(pixels.len() as u32, 1);
		for (x, p) in pixels.iter().enumerate() {
			image.put_pixel(x as u32, 0, image::Rgba(*p));
		}
		image
	}

	fn pixels(image: &image::RgbaImage) -> Vec<[u8; 4]> {
		image.pixels().map(|p| p.0).collect()
	}

	const ALL_MODES: [AlphaMode; 5] = [
		AlphaMode::Overlay,
		AlphaMode::Transparency,
		AlphaMode::TintMask,
		AlphaMode::Opaque,
		AlphaMode::Blend,
	];

	/// `bake` takes linear colours; the maps hold sRGB.
	const BLACK: [f32; 3] = [0.0; 3];
	const WHITE: [f32; 3] = [1.0; 3];

	#[test]
	fn bake_leaves_an_opaque_untinted_map_alone() {
		let opaque = map(&[[200, 100, 50, 255], [0, 0, 0, 255]]);
		for mode in ALL_MODES {
			assert!(bake(&opaque, mode, WHITE, BLACK).is_none(), "{mode:?}");
		}
	}

	/// Transparency and Blend keep the map's alpha, so untinted they show
	/// the map as it is; the other modes have to flatten it.
	#[test]
	fn bake_of_an_untinted_see_through_map_depends_on_the_mode() {
		let see_through = map(&[[200, 100, 50, 128]]);
		for mode in ALL_MODES {
			let keeps_alpha = matches!(mode, AlphaMode::Transparency | AlphaMode::Blend);
			assert_eq!(
				bake(&see_through, mode, WHITE, BLACK).is_none(),
				keeps_alpha,
				"{mode:?}"
			);
		}
	}

	/// Overlay lays the map over the part colour in linear light and
	/// encodes the result as sRGB: white at half alpha over black is 188,
	/// not the byte average 128.
	#[test]
	fn bake_overlay_blends_in_linear_light() {
		let half = map(&[[255, 255, 255, 128], [10, 20, 30, 0], [10, 20, 30, 255]]);
		let baked = bake(&half, AlphaMode::Overlay, WHITE, BLACK).unwrap();
		assert_eq!(
			pixels(&baked),
			vec![[188, 188, 188, 255], [0, 0, 0, 255], [10, 20, 30, 255]]
		);
		// fully transparent shows the part's colour
		let part = [srgb_to_linear(1.0), srgb_to_linear(128.0 / 255.0), 0.0];
		let baked = bake(&half, AlphaMode::Overlay, WHITE, part).unwrap();
		assert_eq!(pixels(&baked)[1], [255, 128, 0, 255]);
	}

	/// TintMask tints where the map is opaque. Where alpha is 0 the colour
	/// is kept, but the pixel becomes opaque.
	#[test]
	fn bake_tint_mask_tints_by_alpha_and_drops_it() {
		let tint = [srgb_to_linear(128.0 / 255.0), 1.0, 1.0];
		let mask = map(&[[255, 255, 255, 0], [255, 255, 255, 255], [200, 100, 50, 0]]);
		let baked = bake(&mask, AlphaMode::TintMask, tint, BLACK).unwrap();
		assert_eq!(
			pixels(&baked),
			vec![
				[255, 255, 255, 255],
				[128, 255, 255, 255],
				[200, 100, 50, 255]
			]
		);
	}

	/// Transparency (and Blend) tint the colour and keep the alpha; it takes
	/// a tint to change anything (untinted, the map is used as it is).
	#[test]
	fn bake_transparency_tints_and_keeps_alpha() {
		let tint = [srgb_to_linear(128.0 / 255.0), 1.0, srgb_to_linear(0.0)];
		let see_through = map(&[[255, 255, 255, 77], [255, 255, 255, 255]]);
		for mode in [AlphaMode::Transparency, AlphaMode::Blend] {
			let baked = bake(&see_through, mode, tint, WHITE).unwrap();
			assert_eq!(
				pixels(&baked),
				vec![[128, 255, 0, 77], [128, 255, 0, 255]],
				"{mode:?}"
			);
		}
	}

	#[test]
	fn bake_opaque_drops_alpha() {
		let see_through = map(&[[200, 100, 50, 0]]);
		let baked = bake(&see_through, AlphaMode::Opaque, WHITE, BLACK).unwrap();
		assert_eq!(pixels(&baked), vec![[200, 100, 50, 255]]);
	}

	#[test]
	fn min_max_is_per_component() {
		let (lo, hi) = min_max(&[1.0, 5.0, -2.0, 3.0, -1.0, 4.0], 3);
		assert_eq!((lo, hi), (vec![1.0, -1.0, -2.0], vec![3.0, 5.0, 4.0]));
		let (lo, hi) = min_max(&[2.0, 7.0], 1);
		assert_eq!((lo, hi), (vec![2.0], vec![7.0]));
	}

	#[test]
	fn column_major_puts_translation_last() {
		// the root's inverse bind
		assert_eq!(
			column_major(&CFrame::IDENTITY),
			[
				1.0, 0.0, 0.0, 0.0, //
				0.0, 1.0, 0.0, 0.0, //
				0.0, 0.0, 1.0, 0.0, //
				0.0, 0.0, 0.0, 1.0,
			]
		);
		let cf = CFrame::from_components(&[
			1.0, 2.0, 3.0, //
			0.0, -1.0, 0.0, //
			1.0, 0.0, 0.0, //
			0.0, 0.0, 1.0,
		]);
		assert_eq!(
			column_major(&cf),
			[
				0.0, 1.0, 0.0, 0.0, //
				-1.0, 0.0, 0.0, 0.0, //
				0.0, 0.0, 1.0, 0.0, //
				1.0, 2.0, 3.0, 1.0,
			]
		);
	}

	#[test]
	fn same_allows_differences_under_1e5() {
		let a = CFrame::from_position([1.0, 2.0, 3.0]);
		assert!(same(&a, &a));
		let mut b = a;
		b.pos[2] += 5e-6;
		b.rot[1][0] = -9e-6;
		assert!(same(&a, &b));
		let mut b = a;
		b.pos[0] += 2e-5;
		assert!(!same(&a, &b));
		let mut b = a;
		b.rot[2][1] = 2e-5;
		assert!(!same(&a, &b));
	}

	/// The cutoff is strict: a difference of exactly 1e-5 counts as a change.
	/// Starting from zero components keeps the difference exact.
	#[test]
	fn same_treats_exactly_1e5_as_different() {
		let zero = CFrame::from_position([0.0; 3]);
		for diff in [1e-5f32, 1e-5f32.next_down()] {
			let at_cutoff = diff == 1e-5;
			let mut b = zero;
			b.pos[1] = diff;
			assert_eq!(same(&zero, &b), !at_cutoff, "position {diff:e}");
			let mut b = zero;
			b.rot[0][2] = diff;
			assert_eq!(same(&zero, &b), !at_cutoff, "rotation {diff:e}");
		}
	}

	fn triangle(uvs: bool) -> Geometry {
		Geometry {
			positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, -1.0], [0.0, 3.0, 0.0]],
			uvs: uvs.then(|| vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
			normals: Some(vec![[0.0, 0.0, 2.0]; 3]),
			triangles: vec![[0, 1, 2]],
			groups: vec![0..1],
			group_kind: GroupKind::Whole,
		}
	}

	fn floats(doc: &Value, bin: &[u8], accessor: &Value) -> Vec<f32> {
		let view = &doc["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
		let at = view["byteOffset"].as_u64().unwrap() as usize;
		let len = view["byteLength"].as_u64().unwrap() as usize;
		bin[at..at + len]
			.chunks(4)
			.map(|b| f32::from_le_bytes(b.try_into().unwrap()))
			.collect()
	}

	#[test]
	fn export_mesh_writes_one_coloured_mesh() {
		let dir = TempDir::new("glb_mesh");
		let path = dir.join("tri.glb");
		let colour = Color32::from_rgb(163, 162, 165);
		export_mesh(
			&triangle(false),
			&[0],
			&Material::default(),
			colour,
			"Tri",
			&path,
		)
		.unwrap();
		let glb = std::fs::read(&path).unwrap();
		let (doc, bin) = parse_glb(&glb);
		assert_eq!(doc["scene"], 0);
		assert_eq!(doc["scenes"], json!([{ "nodes": [0] }]));
		assert_eq!(doc["nodes"], json!([{ "name": "Tri", "mesh": 0 }]));
		assert_eq!(doc["buffers"][0]["byteLength"], bin.len());
		let primitive = &doc["meshes"][0]["primitives"][0];
		let attributes = &primitive["attributes"];
		assert!(attributes.get("TEXCOORD_0").is_none());
		let position = &doc["accessors"][attributes["POSITION"].as_u64().unwrap() as usize];
		assert_eq!(position["count"], 3);
		assert_eq!(position["min"], json!([0.0, 0.0, -1.0]));
		assert_eq!(position["max"], json!([2.0, 3.0, 0.0]));
		// normals are written normalised
		let normal = &doc["accessors"][attributes["NORMAL"].as_u64().unwrap() as usize];
		assert_eq!(floats(&doc, bin, normal), [0.0, 0.0, 1.0].repeat(3));
		let indices = &doc["accessors"][primitive["indices"].as_u64().unwrap() as usize];
		assert_eq!(
			(indices["count"].as_u64(), indices["componentType"].as_u64()),
			(Some(3), Some(5125))
		);
		// no map: the part colour, in linear light, opaque
		let pbr = &doc["materials"][0]["pbrMetallicRoughness"];
		let factor: Vec<f32> = serde_json::from_value(pbr["baseColorFactor"].clone()).unwrap();
		let expected = [163, 162, 165].map(srgb8_to_linear);
		assert_eq!(factor, [expected[0], expected[1], expected[2], 1.0]);
		assert_eq!(
			(
				pbr["metallicFactor"].as_f64(),
				pbr["roughnessFactor"].as_f64()
			),
			(Some(0.0), Some(0.6))
		);
		assert!(doc["materials"][0].get("alphaMode").is_none());
		assert!(doc.get("textures").is_none() && doc.get("samplers").is_none());
	}

	#[test]
	fn export_mesh_without_triangles_fails() {
		let dir = TempDir::new("glb_empty");
		let path = dir.join("none.glb");
		let mut g = triangle(false);
		g.groups = vec![0..0];
		let result = export_mesh(
			&g,
			&[0],
			&Material::default(),
			Color32::WHITE,
			"None",
			&path,
		);
		assert_eq!(result, Err("the mesh has no triangles".into()));
		// a group index past the geometry's groups draws nothing either
		let result = export_mesh(
			&triangle(false),
			&[3],
			&Material::default(),
			Color32::WHITE,
			"None",
			&path,
		);
		assert_eq!(result, Err("the mesh has no triangles".into()));
		assert!(!path.exists());
	}

	fn material_with_map(dir: &Path, pixels: &[[u8; 4]]) -> (Material, PathBuf) {
		let file = dir.join("map.png");
		map(pixels).save(&file).unwrap();
		let bytes = std::fs::read(&file).unwrap();
		let texture =
			crate::scene::texture::load(&eframe::egui::Context::default(), &file, &bytes).unwrap();
		let mut material = Material::default();
		material.set(Slot::Base, texture);
		(material, file)
	}

	fn image_bytes<'a>(doc: &Value, bin: &'a [u8], texture: usize) -> &'a [u8] {
		let image = doc["textures"][texture]["source"].as_u64().unwrap() as usize;
		let view =
			&doc["bufferViews"][doc["images"][image]["bufferView"].as_u64().unwrap() as usize];
		let at = view["byteOffset"].as_u64().unwrap() as usize;
		&bin[at..at + view["byteLength"].as_u64().unwrap() as usize]
	}

	/// An opaque PNG map is embedded byte for byte, over a white factor.
	#[test]
	fn export_mesh_embeds_an_opaque_png_as_it_is() {
		let dir = TempDir::new("glb_map");
		let (material, file) = material_with_map(&dir, &[[200, 100, 50, 255], [1, 2, 3, 255]]);
		let path = dir.join("tri.glb");
		export_mesh(&triangle(true), &[0], &material, Color32::RED, "Tri", &path).unwrap();
		let glb = std::fs::read(&path).unwrap();
		let (doc, bin) = parse_glb(&glb);
		assert!(
			doc["meshes"][0]["primitives"][0]["attributes"]
				.get("TEXCOORD_0")
				.is_some()
		);
		let pbr = &doc["materials"][0]["pbrMetallicRoughness"];
		assert_eq!(pbr["baseColorFactor"], json!([1.0, 1.0, 1.0, 1.0]));
		assert_eq!(pbr["baseColorTexture"], json!({ "index": 0 }));
		assert_eq!(doc["images"][0]["mimeType"], "image/png");
		assert_eq!(image_bytes(&doc, bin, 0), std::fs::read(&file).unwrap());
		assert_eq!(doc["samplers"].as_array().unwrap().len(), 1);
	}

	/// A see-through map is baked over the part colour (Overlay) and
	/// embedded as a new PNG.
	#[test]
	fn export_mesh_bakes_a_see_through_map() {
		let dir = TempDir::new("glb_baked");
		let (material, _) = material_with_map(&dir, &[[255, 255, 255, 128], [9, 9, 9, 0]]);
		let path = dir.join("tri.glb");
		export_mesh(
			&triangle(true),
			&[0],
			&material,
			Color32::BLACK,
			"Tri",
			&path,
		)
		.unwrap();
		let glb = std::fs::read(&path).unwrap();
		let (doc, bin) = parse_glb(&glb);
		let png = image::load_from_memory(image_bytes(&doc, bin, 0))
			.unwrap()
			.to_rgba8();
		assert_eq!(pixels(&png), vec![[188, 188, 188, 255], [0, 0, 0, 255]]);
		assert!(doc["materials"][0].get("alphaMode").is_none());
	}

	/// Without texture coordinates the map is left out.
	#[test]
	fn export_mesh_without_uvs_ignores_the_map() {
		let dir = TempDir::new("glb_no_uvs");
		let (material, _) = material_with_map(&dir, &[[200, 100, 50, 255]]);
		let path = dir.join("tri.glb");
		export_mesh(
			&triangle(false),
			&[0],
			&material,
			Color32::WHITE,
			"Tri",
			&path,
		)
		.unwrap();
		let (doc, _) = parse_glb(&std::fs::read(&path).unwrap());
		assert!(doc.get("textures").is_none());
		assert_eq!(
			doc["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"],
			json!([1.0, 1.0, 1.0, 1.0])
		);
	}

	/// A metal part stays metal with a roughness map but no metalness map
	/// (D16): the packed texture's metalness (blue) is full and the factor
	/// is 1, as without the roughness map. A plain part stays at zero.
	#[test]
	fn metalness_does_not_depend_on_a_roughness_map() {
		let dir = TempDir::new("glb_metal");
		let (mut rough, _) = material_with_map(&dir, &[[90, 90, 90, 255]]);
		let texture = rough.maps.remove(&Slot::Base).unwrap();
		rough.set(Slot::Roughness, texture);
		let metallic = |material: &Material, metal: bool| {
			let mut g = Gltf::default();
			let surface = Surface {
				metal,
				..Surface::OPAQUE
			};
			let i = g
				.material(material, Color32::WHITE, &surface, true)
				.unwrap();
			let pbr = g.materials[i]["pbrMetallicRoughness"].clone();
			let blue = pbr.get("metallicRoughnessTexture").map(|t| {
				let image = &g.images[g.textures[t["index"].as_u64().unwrap() as usize]["source"]
					.as_u64()
					.unwrap() as usize];
				let view = &g.views[image["bufferView"].as_u64().unwrap() as usize];
				let at = view["byteOffset"].as_u64().unwrap() as usize;
				let len = view["byteLength"].as_u64().unwrap() as usize;
				let png = image::load_from_memory(&g.bin[at..at + len])
					.unwrap()
					.to_rgba8();
				png.get_pixel(0, 0)[2]
			});
			(pbr["metallicFactor"].as_f64().unwrap(), blue)
		};
		let plain = Material::default();
		assert_eq!(metallic(&plain, true), (1.0, None));
		assert_eq!(metallic(&rough, true), (1.0, Some(255)));
		assert_eq!(metallic(&plain, false), (0.0, None));
		assert_eq!(metallic(&rough, false).0, 0.0);
	}

	/// A block on a block: `Root` at (1, 2, 3), and `Arm` at (4, 2, 3) on a
	/// Motor6D hinged 2 studs along X from Root's centre, 1 stud short of
	/// Arm's. Its clip `Swing` turns the hinge about Z from 230° to 250°
	/// over one second, and `Broken` could not be exported.
	fn swinging_arm(clips: bool) -> AnimatedModel {
		use crate::anim::schema::{
			AnimationClip, ClipStatus, JointInfo, Keyframe, ModelPart, Pose,
		};

		let block = |name: &str, at: [f32; 3]| ModelPart {
			name: name.into(),
			path: format!("Workspace/Rig/{name}"),
			class_name: Some("Part".into()),
			shape: Some("Block".into()),
			size: [1.0; 3],
			cframe: CFrame::from_position(at).components(),
			..Default::default()
		};
		let key = |time: f32, degrees: f32| Keyframe {
			time,
			poses: vec![Pose {
				name: "Root".into(),
				transform: CFrame::IDENTITY.components(),
				children: vec![Pose {
					name: "Arm".into(),
					transform: CFrame::axis_rotation(2, degrees.to_radians()).components(),
					weight: 1.0,
					..Default::default()
				}],
				..Default::default()
			}],
			markers: vec![],
		};
		let animations = if clips {
			vec![
				AnimationClip {
					name: "Swing".into(),
					status: ClipStatus::KeyframeSequence,
					duration: 1.0,
					keyframes: vec![key(0.0, 230.0), key(1.0, 250.0)],
					..Default::default()
				},
				AnimationClip {
					name: "Broken".into(),
					status: ClipStatus::Unavailable,
					..Default::default()
				},
			]
		} else {
			Vec::new()
		};
		AnimatedModel {
			name: "Rig".into(),
			path: "Workspace/Rig".into(),
			parts: vec![
				block("Root", [1.0, 2.0, 3.0]),
				block("Arm", [4.0, 2.0, 3.0]),
			],
			joints: vec![JointInfo {
				name: "Shoulder".into(),
				path: "Workspace/Rig/Root/Shoulder".into(),
				part0: "Workspace/Rig/Root".into(),
				part1: "Workspace/Rig/Arm".into(),
				c0: CFrame::from_position([2.0, 0.0, 0.0]).components(),
				c1: CFrame::from_position([-1.0, 0.0, 0.0]).components(),
				..Default::default()
			}],
			animations,
			..Default::default()
		}
	}

	/// An animated model is one armature: a joint per part under a root
	/// named after the model, each block skinned rigidly to its part, and
	/// each clip that loaded baked, the parts that hold still on two keys.
	#[test]
	fn export_model_writes_a_bound_rig_and_its_clip() {
		let dir = TempDir::new("glb_model");
		let ctx = eframe::egui::Context::default();
		for clips in [true, false] {
			let (sources, images) = (HashMap::new(), HashMap::new());
			let mut view = ModelView::load(&ctx, 0, &swinging_arm(clips), &sources, &images);
			view.show_blocks = true;
			let path = dir.join(format!("rig_{clips}.glb"));
			export_model(&view, &path).unwrap();
			let glb = std::fs::read(&path).unwrap();
			let (doc, bin) = parse_glb(&glb);
			let accessor = |i: &Value| &doc["accessors"][i.as_u64().unwrap() as usize];

			// nodes 0 and 1 are the parts, 2 the root, then a mesh per block
			let nodes = doc["nodes"].as_array().unwrap();
			assert_eq!(nodes.len(), 5, "{nodes:?}");
			assert_eq!(nodes[0]["name"], "Root");
			assert_eq!(nodes[0]["children"], json!([1]));
			assert_eq!(nodes[0]["translation"], json!([1.0, 2.0, 3.0]));
			assert_eq!(nodes[1]["name"], "Arm");
			assert!(nodes[1].get("children").is_none());
			// Arm sits 3 studs along X from Root, unturned
			assert_eq!(nodes[1]["translation"], json!([3.0, 0.0, 0.0]));
			assert_eq!(nodes[1]["rotation"], json!([0.0, 0.0, 0.0, 1.0]));
			assert_eq!(nodes[2], json!({ "name": "Rig", "children": [0] }));
			assert_eq!(doc["scenes"], json!([{ "nodes": [2, 3, 4] }]));
			let skins = doc["skins"].as_array().unwrap();
			assert_eq!(skins.len(), 1);
			assert_eq!(skins[0]["joints"], json!([0, 1, 2]));
			assert_eq!(skins[0]["skeleton"], 2);

			// inverse binds, column-major: the parts' rest positions undone,
			// and nothing for the root
			let ibm = accessor(&skins[0]["inverseBindMatrices"]);
			assert_eq!(
				(ibm["type"].as_str(), ibm["count"].as_u64()),
				(Some("MAT4"), Some(3))
			);
			assert_eq!(ibm["componentType"], FLOAT);
			let moved = |x: f32, y: f32, z: f32| {
				let mut m = [0.0; 16];
				for k in 0..4 {
					m[k * 5] = 1.0;
				}
				m[12..15].copy_from_slice(&[x, y, z]);
				m
			};
			let expected = [
				moved(-1.0, -2.0, -3.0),
				moved(-4.0, -2.0, -3.0),
				moved(0.0, 0.0, 0.0),
			];
			assert_eq!(floats(&doc, bin, ibm), expected.concat());

			// each block is skinned wholly to its own part, where it stands
			let mut meshes = Vec::new();
			for (node, (part, centre)) in nodes[3..]
				.iter()
				.zip([(0, [1.0, 2.0, 3.0]), (1, [4.0, 2.0, 3.0])])
			{
				assert_eq!(node["skin"], 0, "{node}");
				let mesh = node["mesh"].as_u64().unwrap() as usize;
				meshes.push(mesh);
				let attributes = &doc["meshes"][mesh]["primitives"][0]["attributes"];
				let position = accessor(&attributes["POSITION"]);
				let lo: Vec<f32> = centre.iter().map(|c| c - 0.5).collect();
				let hi: Vec<f32> = centre.iter().map(|c| c + 0.5).collect();
				assert_eq!(
					(&position["min"], &position["max"]),
					(&json!(lo), &json!(hi)),
					"{node}"
				);
				let count = position["count"].as_u64().unwrap() as usize;
				assert!(count > 0);
				let joints = accessor(&attributes["JOINTS_0"]);
				assert_eq!(joints["componentType"], UNSIGNED_SHORT);
				let view = &doc["bufferViews"][joints["bufferView"].as_u64().unwrap() as usize];
				let at = view["byteOffset"].as_u64().unwrap() as usize;
				let joints: Vec<u16> = bin[at..at + count * 8]
					.chunks(2)
					.map(|b| u16::from_le_bytes([b[0], b[1]]))
					.collect();
				assert_eq!(joints, [part, 0, 0, 0].repeat(count), "{node}");
				let weights = floats(&doc, bin, accessor(&attributes["WEIGHTS_0"]));
				assert_eq!(weights, [1.0, 0.0, 0.0, 0.0].repeat(count), "{node}");
			}
			assert!(
				meshes[0] != meshes[1]
					&& meshes
						.iter()
						.all(|&m| m < doc["meshes"].as_array().unwrap().len())
			);

			if !clips {
				assert!(doc.get("animations").is_none());
				continue;
			}
			// only the clip that loaded
			let animations = doc["animations"].as_array().unwrap();
			assert_eq!(animations.len(), 1);
			assert_eq!(animations[0]["name"], "Swing");
			let channels = animations[0]["channels"].as_array().unwrap();
			let targets: Vec<(u64, &str)> = channels
				.iter()
				.map(|c| {
					(
						c["target"]["node"].as_u64().unwrap(),
						c["target"]["path"].as_str().unwrap(),
					)
				})
				.collect();
			assert_eq!(
				targets,
				[
					(0, "translation"),
					(0, "rotation"),
					(1, "translation"),
					(1, "rotation")
				]
			);
			let sampled = |channel: usize| {
				let sampler = &animations[0]["samplers"]
					[channels[channel]["sampler"].as_u64().unwrap() as usize];
				let (input, output) = (accessor(&sampler["input"]), accessor(&sampler["output"]));
				assert_eq!(input["count"], output["count"], "channel {channel}");
				(floats(&doc, bin, input), floats(&doc, bin, output))
			};
			// Root holds still: two keys, at the start and end
			let (times, translations) = sampled(0);
			assert_eq!(times, [0.0, 1.0]);
			assert_eq!(translations, [1.0, 2.0, 3.0].repeat(2));
			let (_, rotations) = sampled(1);
			assert_eq!(rotations, [0.0, 0.0, 0.0, 1.0].repeat(2));
			// Arm moves: a key every 1/30 s from 0 to 1 inclusive
			let (times, translations) = sampled(2);
			assert_eq!(times.len(), 31);
			assert!(times.windows(2).all(|w| w[1] > w[0]), "{times:?}");
			assert_eq!((times[0], times[30]), (0.0, 1.0));
			// the hinge 2 along X, Arm's centre 1 further along the turned X
			let swung = |degrees: f32| {
				let (s, c) = degrees.to_radians().sin_cos();
				[2.0 + c, s, 0.0]
			};
			let close = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
			assert!(close(&translations[..3], &swung(230.0)), "{translations:?}");
			assert!(
				close(&translations[90..], &swung(250.0)),
				"{translations:?}"
			);
			// half-angles 115° and 125°, on the same side all the way: the turn
			// passes 240°, where the matrix-to-quaternion conversion switches sign
			let (_, rotations) = sampled(3);
			assert_eq!(rotations.len(), 31 * 4);
			for (k, pair) in rotations
				.chunks(4)
				.collect::<Vec<_>>()
				.windows(2)
				.enumerate()
			{
				let dot: f32 = pair[0].iter().zip(pair[1]).map(|(a, b)| a * b).sum();
				assert!(dot > 0.0, "keys {k} and {}: {pair:?}", k + 1);
			}
			let half = |degrees: f32| {
				let (s, c) = (degrees / 2.0).to_radians().sin_cos();
				[0.0, 0.0, s, c]
			};
			assert!(
				close(&rotations[..4], &half(230.0)),
				"{:?}",
				&rotations[..4]
			);
			assert!(
				close(&rotations[120..], &half(250.0)),
				"{:?}",
				&rotations[120..]
			);
		}
	}

	/// Exports every animated model of the workspace at `MESHVAULT_WS`,
	/// calling the glTF writer directly, to the folder `MESHVAULT_GLB_OUT`
	/// (default: `meshvault-glb-test` in the temp folder, not emptied first;
	/// files of the same name are overwritten and left there to inspect).
	/// Ignored as it needs a real workspace; run it with
	/// `MESHVAULT_WS=<workspace folder> cargo test -p meshvault -- --ignored gltf::tests`.
	/// The OBJ counterpart is `export::tests::exports_workspace_models`.
	#[test]
	#[ignore]
	fn exports_workspace_models() {
		let root = std::env::var("MESHVAULT_WS").expect("MESHVAULT_WS");
		let out = std::env::var("MESHVAULT_GLB_OUT")
			.map(PathBuf::from)
			.unwrap_or_else(|_| std::env::temp_dir().join("meshvault-glb-test"));
		std::fs::create_dir_all(&out).unwrap();
		let ws = crate::workspace::Workspace::open(Path::new(&root)).unwrap();
		let ctx = eframe::egui::Context::default();
		let mut exported = 0;
		for i in 0..ws.models().len() {
			let model = crate::workspace::view_loader::model_loader(&ws, i).unwrap()(&ctx).unwrap();
			let path = out.join(format!("{}.glb", ws.models()[i].name));
			export_model(&model, &path).unwrap();
			let glb = std::fs::read(&path).unwrap();
			let (doc, _) = parse_glb(&glb);
			let joints = doc["skins"][0]["joints"].as_array().unwrap();
			assert_eq!(
				joints.len(),
				model.export_nodes().len() + 1,
				"{}",
				path.display()
			);
			let clips = model.clips.iter().filter(|c| c.clip.is_ok()).count();
			let animations = doc
				.get("animations")
				.map_or(0, |a| a.as_array().unwrap().len());
			assert_eq!(animations, clips, "{}", path.display());
			exported += 1;
			println!("{} ({} bytes)", path.display(), glb.len());
		}
		assert!(exported > 0, "no models in {root}");
	}
}
