//! Loading image files as mesh textures and material maps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Pos2};

use crate::scene::maps::{MapKind, Slot};

pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "bmp", "tga", "webp"];

/// Rows of opaque white appended below the image. The CPU renderer's
/// wireframe overlay samples from here so it can share one draw call (and
/// therefore the back-to-front ordering) with the textured triangles.
const WHITE_ROWS: usize = 4;
/// Largest side kept in memory; bigger images are scaled down.
const MAX_SIDE: u32 = 4096;
/// The GPU's texture size limit is taken as at least this, so subtracting
/// the white rows always leaves room for the image.
const MIN_TEXTURE_SIDE: usize = 64;

#[derive(Clone)]
pub struct Texture {
	pub handle: egui::TextureHandle,
	pub path: PathBuf,
	/// Size of the image as shown to the user (before any downscaling).
	pub size: [usize; 2],
	pub info: TextureInfo,
	/// Decoded pixels, for uploading to the GPU renderer.
	pub rgba: Arc<image::RgbaImage>,
}

/// What the CPU viewport needs to map mesh UVs onto the uploaded texture.
#[derive(Clone, Copy)]
pub struct TextureInfo {
	pub id: egui::TextureId,
	/// Mesh V coordinates are scaled by this so they stay out of the white rows.
	pub v_scale: f32,
	/// A UV that samples solid white, for untextured overlay geometry.
	pub white_uv: Pos2,
}

impl TextureInfo {
	/// Maps a mesh UV into the uploaded texture.
	pub fn map(&self, [u, v]: [f32; 2]) -> Pos2 {
		Pos2::new(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0) * self.v_scale)
	}
}

pub fn is_image(path: &Path) -> bool {
	path.extension()
		.and_then(|e| e.to_str())
		.is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Reads and decodes an image file. A read error is the OS's message.
pub fn load_file(ctx: &egui::Context, path: &Path) -> Result<Texture, String> {
	std::fs::read(path)
		.map_err(|e| e.to_string())
		.and_then(|bytes| load(ctx, path, &bytes))
}

/// Loads each map in order into a new material; a later map for the same
/// slot replaces an earlier one. Callers only do this for meshes with UVs.
pub fn load_maps(ctx: &egui::Context, maps: &[(Slot, PathBuf)]) -> Material {
	let mut material = Material::default();
	for (slot, path) in maps {
		// a missing map leaves the mesh plain rather than failing the part
		if let Ok(texture) = load_file(ctx, path) {
			material.set(*slot, texture);
		}
	}
	material
}

pub fn load(ctx: &egui::Context, path: &Path, bytes: &[u8]) -> Result<Texture, String> {
	let image = image::load_from_memory(bytes)
		.map_err(|e| format!("Could not decode {}: {e}", path.display()))?;
	let size = [image.width() as usize, image.height() as usize];

	// leave room for the white rows within the GPU's size limit
	let max_side = (ctx.input(|i| i.max_texture_side).max(MIN_TEXTURE_SIDE) - WHITE_ROWS)
		.min(MAX_SIDE as usize);
	let image = if image.width() as usize > max_side || image.height() as usize > max_side {
		image.resize(
			max_side as u32,
			max_side as u32,
			image::imageops::FilterType::Triangle,
		)
	} else {
		image
	};

	let rgba = Arc::new(image.to_rgba8());
	let (w, h) = (rgba.width() as usize, rgba.height() as usize);
	let mut pixels = Vec::with_capacity((w * (h + WHITE_ROWS)) * 4);
	pixels.extend_from_slice(rgba.as_raw());
	pixels.resize(pixels.len() + w * WHITE_ROWS * 4, 255);
	let total_h = h + WHITE_ROWS;
	let colour_image = egui::ColorImage::from_rgba_unmultiplied([w, total_h], &pixels);

	let name = path
		.file_name()
		.map(|n| n.to_string_lossy().into_owned())
		.unwrap_or_else(|| "texture".into());
	// No mipmaps: they would blend the white rows into the bottom of the image.
	let handle = ctx.load_texture(name, colour_image, egui::TextureOptions::LINEAR);
	let info = TextureInfo {
		id: handle.id(),
		v_scale: h as f32 / total_h as f32,
		white_uv: Pos2::new(0.5, (h as f32 + WHITE_ROWS as f32 * 0.5) / total_h as f32),
	};
	Ok(Texture {
		handle,
		path: path.to_path_buf(),
		size,
		info,
		rgba,
	})
}

/// The maps applied to the mesh in the viewer.
#[derive(Clone, Default)]
pub struct Material {
	pub maps: BTreeMap<Slot, Texture>,
	/// Slots switched off in the UI (to compare with and without a map).
	pub disabled: Vec<Slot>,
	/// Bumped on every change so the GPU renderer knows to re-upload.
	pub generation: u64,
}

impl Material {
	/// Puts `texture` in `slot`, replacing any map there, and switches the
	/// slot on.
	pub fn set(&mut self, slot: Slot, texture: Texture) {
		self.maps.insert(slot, texture);
		self.disabled.retain(|s| *s != slot);
		self.generation += 1;
	}

	/// Takes the map out of `slot`, bumping the generation only when there
	/// was one. A slot switched off stays marked off, but a later `set`
	/// clears the mark, so a map put back is shown.
	pub fn remove(&mut self, slot: Slot) {
		if self.maps.remove(&slot).is_some() {
			self.generation += 1;
		}
	}

	/// The map in `slot`, unless there is none or the slot is switched off.
	pub fn active(&self, slot: Slot) -> Option<&Texture> {
		self.maps
			.get(&slot)
			.filter(|_| !self.disabled.contains(&slot))
	}

	/// Switches `slot` on or off, with or without a map in it.
	pub fn toggle(&mut self, slot: Slot, on: bool) {
		self.disabled.retain(|s| *s != slot);
		if !on {
			self.disabled.push(slot);
		}
		self.generation += 1;
	}
}

/// Finds the maps that most likely belong to a mesh file, following the
/// workspace naming (`Name.png`, `Name.color.png`, `Name.normal.png`, ...).
/// For the base colour, the only image in the folder is used as a fallback.
pub fn find_siblings(mesh_path: &Path) -> BTreeMap<Slot, PathBuf> {
	let mut found = BTreeMap::new();
	let (Some(dir), Some(stem)) = (mesh_path.parent(), mesh_path.file_stem()) else {
		return found;
	};
	let stem = stem.to_string_lossy();
	// a folder that cannot be read has no maps, and an entry that cannot be
	// read is skipped: either way the mesh opens without those maps
	let Ok(read) = std::fs::read_dir(dir) else {
		return found;
	};
	let images: Vec<PathBuf> = read
		.filter_map(|e| e.ok().map(|e| e.path()))
		.filter(|p| p.is_file() && is_image(p))
		.collect();
	let named = |kind: MapKind| {
		let want = format!("{stem}{}", kind.file_suffix());
		images
			.iter()
			.find(|p| {
				p.file_stem()
					.is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&want))
			})
			.cloned()
	};
	for slot in Slot::ALL {
		if let Some(path) = slot.sources().iter().find_map(|&k| named(k)) {
			found.insert(slot, path);
		}
	}
	// a lone image is the base colour, unless it is already another map
	if !found.contains_key(&Slot::Base)
		&& images.len() == 1
		&& !found.values().any(|p| *p == images[0])
	{
		found.insert(Slot::Base, images[0].clone());
	}
	found
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::TempDir;

	fn png(dir: &Path, name: &str) -> PathBuf {
		let path = dir.join(name);
		image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
			.save(&path)
			.unwrap();
		path
	}

	/// A read error is the OS's own message; a decode error names the file.
	#[test]
	fn load_file_reports_read_and_decode_errors() {
		let dir = TempDir::new("texture_load_file");
		let ctx = egui::Context::default();
		let missing = dir.join("missing.png");
		let err = load_file(&ctx, &missing).err().unwrap();
		assert_eq!(err, std::fs::read(&missing).unwrap_err().to_string());
		let bad = dir.join("bad.png");
		std::fs::write(&bad, b"not an image").unwrap();
		let err = load_file(&ctx, &bad).err().unwrap();
		assert!(err.starts_with(&format!("Could not decode {}: ", bad.display())));
		assert_eq!(
			load_file(&ctx, &png(&dir, "good.png")).unwrap().size,
			[2, 2]
		);
	}

	/// A mesh UV is clamped to the image and kept out of the white rows.
	#[test]
	fn uvs_map_into_the_image_above_the_white_rows() {
		let info = TextureInfo {
			id: egui::TextureId::default(),
			v_scale: 0.75,
			white_uv: Pos2::ZERO,
		};
		assert_eq!(info.map([0.5, 1.0]), Pos2::new(0.5, 0.75));
		assert_eq!(info.map([-1.0, 2.0]), Pos2::new(0.0, 0.75));
		assert_eq!(info.map([3.0, -0.5]), Pos2::new(1.0, 0.0));
	}

	/// Every change to a material bumps its generation, except removing a
	/// map that is not there. A slot switched off hides its map, stays off
	/// when the map is removed, and comes back on when a map is set.
	#[test]
	fn material_maps_switch_on_and_off() {
		let dir = TempDir::new("texture_material");
		let ctx = egui::Context::default();
		let texture = load_file(&ctx, &png(&dir, "base.png")).unwrap();
		let mut m = Material::default();
		m.set(Slot::Base, texture.clone());
		assert!(m.active(Slot::Base).is_some());
		assert_eq!(m.generation, 1);
		m.toggle(Slot::Base, false);
		m.toggle(Slot::Base, false);
		assert!(m.active(Slot::Base).is_none());
		assert!(m.maps.contains_key(&Slot::Base));
		assert_eq!(
			(m.disabled.as_slice(), m.generation),
			(&[Slot::Base][..], 3)
		);
		m.remove(Slot::Base);
		assert_eq!(
			(m.disabled.as_slice(), m.generation),
			(&[Slot::Base][..], 4)
		);
		m.remove(Slot::Base);
		assert_eq!(m.generation, 4, "nothing to remove");
		// a slot can be switched off with no map in it
		m.toggle(Slot::Normal, false);
		assert_eq!(m.disabled, [Slot::Base, Slot::Normal]);
		m.set(Slot::Base, texture);
		assert!(m.active(Slot::Base).is_some());
		assert_eq!(
			(m.disabled.as_slice(), m.generation),
			(&[Slot::Normal][..], 6)
		);
		m.toggle(Slot::Normal, true);
		assert!(m.disabled.is_empty());
	}

	/// Maps next to a mesh are found by name (any case), ColorMap before
	/// the texture; a lone image is the base colour unless it is already
	/// another map; an unreadable folder has none.
	#[test]
	fn sibling_maps_are_found_by_name_or_alone() {
		let found = |files: &[&str]| {
			let dir = TempDir::new("texture_siblings");
			for f in files {
				std::fs::write(dir.join(f), b"").unwrap();
			}
			let names = |m: BTreeMap<Slot, PathBuf>| -> Vec<(Slot, String)> {
				m.into_iter()
					.map(|(s, p)| (s, p.file_name().unwrap().to_string_lossy().into_owned()))
					.collect()
			};
			names(find_siblings(&dir.join("Hat.mesh")))
		};
		assert_eq!(
			found(&[
				"Hat.mesh",
				"Hat.png",
				"Hat.color.png",
				"hat.NORMAL.PNG",
				"Other.roughness.png"
			]),
			[
				(Slot::Base, "Hat.color.png".into()),
				(Slot::Normal, "hat.NORMAL.PNG".into())
			]
		);
		assert_eq!(found(&["Hat.png"]), [(Slot::Base, "Hat.png".into())]);
		assert_eq!(
			found(&["Hat.mesh", "anything.jpg"]),
			[(Slot::Base, "anything.jpg".into())]
		);
		assert_eq!(
			found(&["Hat.mesh", "Hat.metalness.png"]),
			[(Slot::Metalness, "Hat.metalness.png".into())]
		);
		assert!(found(&["Hat.mesh", "a.png", "b.png", "notes.txt"]).is_empty());
		assert!(find_siblings(Path::new("no such folder/Hat.mesh")).is_empty());
	}

	/// Maps load in order, a later one for a slot wins, and maps that are
	/// missing or do not decode are skipped.
	#[test]
	fn load_maps_skips_failures_and_lets_later_maps_win() {
		let dir = TempDir::new("texture_load_maps");
		let ctx = egui::Context::default();
		let bad = dir.join("bad.png");
		std::fs::write(&bad, b"not an image").unwrap();
		let (first, second) = (png(&dir, "first.png"), png(&dir, "second.png"));
		let material = load_maps(
			&ctx,
			&[
				(Slot::Base, first),
				(Slot::Normal, dir.join("missing.png")),
				(Slot::Base, second.clone()),
				(Slot::Roughness, bad),
			],
		);
		let slots: Vec<(Slot, PathBuf)> = material
			.maps
			.iter()
			.map(|(s, t)| (*s, t.path.clone()))
			.collect();
		assert_eq!(slots, [(Slot::Base, second)]);
		assert_eq!(material.generation, 2);
		assert!(material.disabled.is_empty());
	}
}
