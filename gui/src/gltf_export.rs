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
use crate::geometry::{Geometry, GroupKind};
use crate::maps::Slot;
use crate::model_view::{ExportMesh, ModelView};
use crate::texture::Material;
use crate::viewport::{AlphaMode, Surface};

/// Keyframes per second of baked clips.
pub const FPS: f32 = 30.0;

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;

/// Writes an animated model, with its clips, to `path`.
pub fn export_model(model: &ModelView, path: &Path) -> Result<(), String> {
	let mut g = Gltf::default();
	let nodes = model.export_nodes();
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
	let ibm = g.floats(&inverse_binds, "MAT4", None, None);
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
		let times = key_times(clip.duration);
		let frames = times.len();
		let locals: Vec<Vec<CFrame>> = times
			.iter()
			.map(|&t| {
				let worlds = model.export_frame(Some(i), t);
				(0..nodes.len()).map(|n| local(&worlds, n)).collect()
			})
			.collect();
		let input = g.floats(&times, "SCALAR", None, Some(min_max(&times, 1)));
		// every node is keyed, so Blender does not leave a bone this clip
		// does not move where another action put it; one that holds still
		// gets two keys
		let ends = [times[0], times[frames - 1]];
		let hold = g.floats(&ends, "SCALAR", None, Some(min_max(&ends, 1)));
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
				(translations, "VEC3", "translation"),
				(rotations, "VEC4", "rotation"),
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
	let positions = geometry.positions.clone();
	let index = g.mesh(
		name,
		geometry,
		groups,
		&positions,
		geometry.normals.as_deref(),
		None,
		material,
		colour,
		&Surface::OPAQUE,
	);
	let Some(index) = index else {
		return Err("the mesh has no triangles".into());
	};
	g.nodes.push(json!({ "name": name, "mesh": index }));
	g.scene = vec![0];
	g.write(path)
}

/// The groups a mesh opens with in the viewer: the finest LOD, or all.
pub fn default_groups(geometry: &Geometry) -> Vec<usize> {
	match geometry.group_kind {
		GroupKind::Lod => vec![0],
		_ => (0..geometry.groups.len()).collect(),
	}
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
		AlphaMode::Overlay | AlphaMode::TintMask => see_through || !white,
		AlphaMode::Transparency => !white,
		AlphaMode::Opaque => see_through || !white,
	};
	if !changes {
		return None;
	}
	let decode: Vec<f32> = (0..=255).map(|c| linear(c as f32 / 255.0)).collect();
	let encode = |v: f32| {
		let v = v.clamp(0.0, 1.0);
		let s = if v <= 0.003_130_8 {
			v * 12.92
		} else {
			1.055 * v.powf(1.0 / 2.4) - 0.055
		};
		(s * 255.0).round() as u8
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
				AlphaMode::Transparency | AlphaMode::Opaque => c[k] * tint[k],
			});
			let alpha = if mode == AlphaMode::Transparency {
				p[3]
			} else {
				255
			};
			image::Rgba([encode(rgb[0]), encode(rgb[1]), encode(rgb[2]), alpha])
		},
	))
}

/// Key times of a baked clip: every frame at [`FPS`], and its end. A clip
/// that is a single pose holds it for one frame.
fn key_times(duration: f32) -> Vec<f32> {
	let duration = duration.max(0.0);
	let mut times: Vec<f32> = (0..)
		.map(|k| k as f32 / FPS)
		.take_while(|&t| t < duration - 1e-4)
		.collect();
	times.push(duration);
	if times.len() < 2 {
		times = vec![0.0, 1.0 / FPS];
	}
	times
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

fn linear(c: f32) -> f32 {
	if c <= 0.04045 {
		c / 12.92
	} else {
		((c + 0.055) / 1.055).powf(2.4)
	}
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
	/// Material index by what it shows.
	by_look: HashMap<String, usize>,
}

impl Gltf {
	/// Appends `bytes` as a buffer view, 4-byte aligned.
	fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
		while !self.bin.len().is_multiple_of(4) {
			self.bin.push(0);
		}
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
		ty: &str,
		target: Option<u32>,
		bounds: Option<(Vec<f32>, Vec<f32>)>,
	) -> usize {
		let width = match ty {
			"SCALAR" => 1,
			"VEC2" => 2,
			"VEC3" => 3,
			"VEC4" => 4,
			_ => 16,
		};
		let view = self.view(bytemuck::cast_slice(values), target);
		let mut accessor = json!({
			"bufferView": view,
			"componentType": FLOAT,
			"count": values.len() / width,
			"type": ty,
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
		ty: &str,
		count: usize,
		target: u32,
	) -> usize {
		let view = self.view(bytemuck::cast_slice(values), Some(target));
		self.accessors.push(json!({
			"bufferView": view,
			"componentType": component,
			"count": count,
			"type": ty,
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
		self.mesh(
			&m.name,
			m.geometry,
			&m.groups,
			&m.positions,
			m.normals.as_deref(),
			Some(&skin),
			m.material,
			m.colour,
			&m.surface,
		)
	}

	#[allow(clippy::too_many_arguments)]
	fn mesh(
		&mut self,
		name: &str,
		geometry: &Geometry,
		groups: &[usize],
		positions: &[[f32; 3]],
		normals: Option<&[[f32; 3]]>,
		skin: Option<&[([u16; 4], [f32; 4])]>,
		material: &Material,
		colour: Color32,
		surface: &Surface,
	) -> Option<usize> {
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
		let mut attributes =
			json!({ "POSITION": self.floats(&flat, "VEC3", Some(ARRAY_BUFFER), Some(bounds)) });
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
			attributes["NORMAL"] = json!(self.floats(&flat, "VEC3", Some(ARRAY_BUFFER), None));
		}
		// Roblox and glTF both put the texture origin at the top left
		if let Some(uvs) = geometry.uvs.as_ref().filter(|u| u.len() == positions.len()) {
			let flat: Vec<f32> = uvs.iter().flatten().copied().collect();
			attributes["TEXCOORD_0"] = json!(self.floats(&flat, "VEC2", Some(ARRAY_BUFFER), None));
		}
		if let Some(skin) = skin {
			let joints: Vec<u16> = skin.iter().flat_map(|s| s.0).collect();
			let weights: Vec<f32> = skin.iter().flat_map(|s| s.1).collect();
			attributes["JOINTS_0"] =
				json!(self.integers(&joints, UNSIGNED_SHORT, "VEC4", skin.len(), ARRAY_BUFFER));
			attributes["WEIGHTS_0"] =
				json!(self.floats(&weights, "VEC4", Some(ARRAY_BUFFER), None));
		}
		let index_accessor = self.integers(
			&indices,
			UNSIGNED_INT,
			"SCALAR",
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
		let part = [colour.r(), colour.g(), colour.b()].map(|c| linear(c as f32 / 255.0));
		let tint = surface.tint.map(|c| linear(c.clamp(0.0, 1.0)));
		// a colour map is baked as Meshvault draws it (over the part colour,
		// tinted), since glTF cannot mix them the way Roblox does
		let rgb = if base.is_some() { [1.0; 3] } else { part };
		let opacity = surface.opacity.clamp(0.0, 1.0);
		let mut pbr = json!({
			"baseColorFactor": [rgb[0], rgb[1], rgb[2], opacity],
			"metallicFactor": if metal.is_some() || surface.metal { 1.0 } else { 0.0 },
			"roughnessFactor": if rough.is_some() { 1.0 } else { 0.6 },
		});
		let base_texture = base.and_then(|t| match bake(&t.rgba, surface.alpha_mode, tint, part) {
			Some(baked) => {
				let key = PathBuf::from(format!(
					"{}|{:?}|{tint:?}|{part:?}",
					t.path.display(),
					surface.alpha_mode
				));
				if let Some(&i) = self.by_file.get(&key) {
					return Some(i);
				}
				let i = self.texture_image(&baked)?;
				self.by_file.insert(key, i);
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
			let fit = |t: Option<&crate::texture::Texture>| {
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
			let packed = image::RgbaImage::from_fn(w, h, |x, y| {
				let g = r.as_ref().map_or(255, |r| r.get_pixel(x, y)[0]);
				let b = m.as_ref().map_or(0, |m| m.get_pixel(x, y)[0]);
				image::Rgba([255, g, b, 255])
			});
			if let Some(t) = self.texture_image(&packed) {
				pbr["metallicRoughnessTexture"] = json!({ "index": t });
				if metal.is_none() {
					pbr["metallicFactor"] = json!(0.0);
				}
			}
		}
		let mut value = json!({ "pbrMetallicRoughness": pbr });
		if let Some(t) = normal.and_then(|t| self.texture_file(&t.path, &t.rgba)) {
			value["normalTexture"] = json!({ "index": t });
		}
		if surface.neon {
			value["emissiveFactor"] = json!(part);
		}
		if opacity < 0.999 {
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
		while !self.bin.len().is_multiple_of(4) {
			self.bin.push(0);
		}
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
			// 10497 repeat, 9729 linear, 9987 linear mipmap linear
			doc["samplers"] =
				json!([{ "wrapS": 10497, "wrapT": 10497, "magFilter": 9729, "minFilter": 9987 }]);
		}
		let mut text = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
		while !text.len().is_multiple_of(4) {
			text.push(b' ');
		}
		let total = 12 + 8 + text.len() + 8 + self.bin.len();
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
mod tests {
	use super::*;

	#[test]
	fn key_times_increase_and_end_on_the_clip() {
		for duration in [0.0, 0.01, 1.0 / FPS, 0.5, 1.0, 2.345] {
			let t = key_times(duration);
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

	/// Exports every animated model of the workspace at `MESHVAULT_WS` to
	/// `MESHVAULT_GLB_OUT` (or the temp folder).
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
		for i in 0..ws.models().len() {
			let model = crate::app::model_loader(&ws, i)(&ctx).unwrap();
			let path = out.join(format!("{}.glb", ws.models()[i].name));
			export_model(&model, &path).unwrap();
			println!(
				"{} ({} bytes)",
				path.display(),
				std::fs::metadata(&path).unwrap().len()
			);
		}
	}
}
