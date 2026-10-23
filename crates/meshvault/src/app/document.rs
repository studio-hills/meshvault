//! The mesh document shown in the viewer: opening files, decoding meshes
//! and loading their maps.

use std::path::{Path, PathBuf};

use super::App;
use crate::render::camera::Camera;
use crate::render::gpu::GpuScene;
use crate::scene::geometry::{self, GroupKind, Loaded};
use crate::scene::maps::Slot;
use crate::scene::surface::AlphaMode;
use crate::scene::texture::{self, Material};
use crate::studio::export_file::{self, read_limited};

pub struct Document {
	pub path: PathBuf,
	pub size: usize,
	pub loaded: Loaded,
	pub camera: Camera,
	/// For LOD files: the single LOD shown. Unused otherwise.
	pub lod: usize,
	/// For hull files: which hulls are visible.
	pub hull_visible: Vec<bool>,
	pub material: Material,
	/// The mesh uploaded to the GPU, created on first draw.
	pub gpu: Option<GpuScene>,
	/// What the colour map's alpha does.
	pub alpha: AlphaMode,
}

impl Document {
	pub fn visible_groups(&self) -> Vec<usize> {
		let groups = &self.loaded.geometry.groups;
		match self.loaded.geometry.group_kind {
			GroupKind::Lod => vec![self.lod.min(groups.len().saturating_sub(1))],
			GroupKind::Hull => (0..groups.len())
				.filter(|&i| self.hull_visible[i])
				.collect(),
			GroupKind::Whole => (0..groups.len()).collect(),
		}
	}

	pub fn name(&self) -> String {
		file_name(&self.path)
	}
}

/// The last part of `path`, lossily as text; empty if it has none.
fn file_name(path: &Path) -> String {
	path.file_name()
		.map(|n| n.to_string_lossy().into_owned())
		.unwrap_or_default()
}

/// Extensions the open dialog offers for Roblox meshes and union data.
const MESH_EXTENSIONS: [&str; 5] = ["mesh", "meshdata", "meshdata5", "data", "bin"];

/// The largest file opened or imported: exports are the largest files
/// Meshvault reads, and this is their limit.
const MAX_FILE_BYTES: usize = export_file::MAX_EXPORT_BYTES;

impl App {
	/// Reads the file at `path`, or reports why it could not be read.
	pub(super) fn read_or_report(&mut self, path: &Path) -> Option<Vec<u8>> {
		match read_limited(path, MAX_FILE_BYTES) {
			Ok(bytes) => Some(bytes),
			Err(e) => {
				self.toasts
					.error(format!("Could not read {}: {e}", path.display()));
				None
			}
		}
	}

	pub fn open(&mut self, path: &Path) {
		if path.is_dir() {
			self.open_workspace(path);
			return;
		}
		if let Some(bytes) = self.read_or_report(path) {
			self.open_bytes(path.to_path_buf(), &bytes);
		}
	}

	pub fn open_bytes(&mut self, path: PathBuf, bytes: &[u8]) {
		if path
			.extension()
			.is_some_and(|e| e.eq_ignore_ascii_case("json"))
		{
			// a rig file of the open workspace opens its model; any other
			// export is imported
			if let Some(index) = self.workspace_model_at(&path) {
				self.open_model(index);
			} else {
				self.start_import(path, bytes);
			}
			return;
		}
		// an image dropped or opened while a mesh is shown becomes one of its
		// maps, chosen by name: Name.normal.png is a normal map, and so on
		if texture::is_image(&path) {
			if self.doc.is_some() {
				self.set_map(slot_for_image(&path), &path, bytes);
			} else {
				self.toasts.error(format!(
					"{} is an image. Open a mesh first, then drop images on it to use them as maps.",
					path.display()
				));
			}
			return;
		}
		// a mesh of the open workspace opens as its item, with its maps
		if let Some(index) = self.workspace_item_at(&path) {
			self.open_item(index);
			return;
		}
		if self.load_mesh(path, bytes) {
			self.open_stem = None;
			let found = self
				.doc
				.as_ref()
				.filter(|d| d.loaded.geometry.uvs.is_some())
				.map(|d| texture::find_siblings(&d.path))
				.unwrap_or_default();
			for (slot, image) in &found {
				self.load_map(*slot, image);
			}
			if !found.is_empty() {
				let names: Vec<_> = found
					.values()
					.filter_map(|p| p.file_name())
					.map(|n| n.to_string_lossy())
					.collect();
				self.toasts
					.info(format!("Loaded maps: {}", names.join(", ")));
			}
		}
	}

	/// Decodes a mesh into the viewer. Returns false (and reports the error) on failure.
	pub fn load_mesh(&mut self, path: PathBuf, bytes: &[u8]) -> bool {
		match geometry::load(bytes) {
			Ok(loaded) => {
				let camera = Camera::framing(&loaded.geometry);
				let hull_visible = vec![true; loaded.geometry.groups.len()];
				self.model = None;
				self.loading_model = None;
				self.doc = Some(Document {
					path,
					size: bytes.len(),
					loaded,
					camera,
					lod: 0,
					hull_visible,
					material: Material::default(),
					gpu: None,
					alpha: AlphaMode::Overlay,
				});
				true
			}
			Err(e) => {
				let name = file_name(&path);
				self.toasts.error(format!("Could not open {name}: {e}"));
				false
			}
		}
	}

	pub fn load_map(&mut self, slot: Slot, path: &Path) {
		if let Some(bytes) = self.read_or_report(path) {
			self.set_map(slot, path, &bytes);
		}
	}

	pub fn set_map(&mut self, slot: Slot, path: &Path, bytes: &[u8]) {
		let Some(doc) = &mut self.doc else { return };
		match texture::load(&self.ctx, path, bytes) {
			Ok(texture) => {
				doc.material.set(slot, texture);
				self.show_maps = true;
			}
			Err(e) => self.toasts.error(e),
		}
	}

	pub fn pick_map(&mut self, slot: Slot) {
		let mut dialog = rfd::FileDialog::new()
			.set_title(format!("Load {} map", slot.label().to_lowercase()))
			.add_filter("Images", &texture::IMAGE_EXTENSIONS);
		if let Some(dir) = self.doc.as_ref().and_then(|d| d.path.parent()) {
			dialog = dialog.set_directory(dir);
		}
		if let Some(path) = dialog.pick_file() {
			self.load_map(slot, &path);
		}
	}

	pub fn remove_all_maps(&mut self) {
		if let Some(doc) = &mut self.doc {
			for slot in Slot::ALL {
				doc.material.remove(slot);
			}
		}
	}

	pub fn pick_file(&mut self) {
		if let Some(path) = rfd::FileDialog::new()
			.set_title("Open Roblox mesh")
			.add_filter("Roblox mesh / union data", &MESH_EXTENSIONS)
			.add_filter("All files", &["*"])
			.pick_file()
		{
			self.open(&path);
		}
	}
}

/// Picks the material slot for a dropped image from its name, following the
/// workspace naming: `Name.normal.png`, `Name.roughness.png`, `Name.metalness.png`.
pub fn slot_for_image(path: &Path) -> Slot {
	let stem = path
		.file_stem()
		.map(|s| s.to_string_lossy().to_lowercase())
		.unwrap_or_default();
	for (suffix, slot) in [
		(".normal", Slot::Normal),
		(".roughness", Slot::Roughness),
		(".metalness", Slot::Metalness),
	] {
		if stem.ends_with(suffix) {
			return slot;
		}
	}
	Slot::Base
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A file over the limit is not read; one at the limit is.
	#[test]
	fn reads_files_up_to_the_limit() {
		let dir = crate::test_support::TempDir::new("read_limited");
		let path = dir.join("export.json");
		std::fs::write(&path, b"12345678").unwrap();
		assert_eq!(read_limited(&path, 8).unwrap(), b"12345678");
		let err = read_limited(&path, 7).unwrap_err();
		assert!(err.to_string().contains("larger than"), "{err}");
	}

	#[test]
	fn dropped_images_pick_their_slot_from_the_name() {
		let slot = |name: &str| slot_for_image(Path::new(name));
		assert_eq!(slot("Models/Rock.normal.png"), Slot::Normal);
		assert_eq!(slot("Rock.roughness.png"), Slot::Roughness);
		assert_eq!(slot("Rock.metalness.jpg"), Slot::Metalness);
		assert_eq!(slot("Rock.NORMAL.PNG"), Slot::Normal);
		assert_eq!(slot("Rock.png"), Slot::Base);
		assert_eq!(slot("Rock.color.png"), Slot::Base);
		// the suffix needs its dot
		assert_eq!(slot("normal.png"), Slot::Base);
		assert_eq!(slot(""), Slot::Base);
	}
}
