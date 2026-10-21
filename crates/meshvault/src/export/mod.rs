//! Exporting models out of Meshvault: OBJ with MTL, and binary glTF for
//! Blender, one at a time from the viewer or several at once from the
//! library.

use std::path::{Path, PathBuf};

use eframe::egui;

use self::obj::{export_mesh, export_model};
use crate::scene::geometry;
use crate::scene::maps::Slot;
use crate::scene::model_view::ModelView;
use crate::scene::surface::PART_COLOUR;
use crate::scene::texture::{self, Material};
use crate::workspace::sanitize;

pub mod gltf;
pub mod obj;

/// What to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
	/// Binary glTF: skeleton, skin weights, clips and embedded maps.
	Glb,
	/// Wavefront OBJ with an MTL file and the maps beside it.
	Obj,
}

impl Format {
	fn extension(self) -> &'static str {
		match self {
			Format::Glb => "glb",
			Format::Obj => "obj",
		}
	}
}

/// A downloaded mesh to export: its name, file and maps.
pub struct MeshJob {
	pub name: String,
	pub mesh: PathBuf,
	pub maps: Vec<(Slot, PathBuf)>,
}

/// Reads an animated model (see [`crate::workspace::view_loader::model_loader`]).
pub type ModelLoader = Box<dyn FnOnce(&egui::Context) -> Result<ModelView, String> + Send>;

/// A model to export: its name, and what reads it.
pub struct ModelJob {
	pub name: String,
	pub load: ModelLoader,
}

pub struct Report {
	pub dir: PathBuf,
	pub written: usize,
	/// What could not be exported, and why.
	pub failed: Vec<String>,
}

/// Writes each mesh and model into `dir`. Names that clash, with each
/// other or with files already there, get a number.
pub fn export(
	ctx: &egui::Context,
	dir: &Path,
	meshes: Vec<MeshJob>,
	models: Vec<ModelJob>,
	format: Format,
) -> Report {
	let mut report = Report {
		dir: dir.to_path_buf(),
		written: 0,
		failed: Vec::new(),
	};
	let mut names = Names::default();
	for job in meshes {
		let name = names.unique(dir, &job.name, format);
		let result = match format {
			Format::Obj => export_mesh(dir, &name, &job, &mut names),
			Format::Glb => export_mesh_glb(ctx, &dir.join(format!("{name}.glb")), &name, &job),
		};
		match result {
			Ok(()) => report.written += 1,
			Err(e) => report.failed.push(format!("{}: {e}", job.name)),
		}
	}
	for job in models {
		let name = names.unique(dir, &job.name, format);
		let result = (job.load)(ctx).and_then(|mut model| match format {
			Format::Obj => export_model(dir, &name, &mut model, &mut names),
			Format::Glb => {
				crate::export::gltf::export_model(&model, &dir.join(format!("{name}.glb")))
			}
		});
		match result {
			Ok(()) => report.written += 1,
			Err(e) => report.failed.push(format!("{}: {e}", job.name)),
		}
	}
	report
}

/// File names in the export folder handed out so far (lower case, as
/// Windows ignores case), so no file an export writes, the OBJ's MTL and
/// maps included, replaces another of the same batch or one already there.
#[derive(Default)]
struct Names(std::collections::HashSet<String>);

impl Names {
	/// A name for the files of `name` that are all free: `name.obj` and
	/// `name.mtl`, or `name.glb`.
	fn unique(&mut self, dir: &Path, name: &str, format: Format) -> String {
		let mut extensions = vec![format.extension()];
		if format == Format::Obj {
			extensions.push("mtl");
		}
		// never empty: a name with nothing usable becomes "_"
		let base = sanitize(name);
		let mut name = base.clone();
		let mut n = 2;
		while extensions
			.iter()
			.any(|ext| self.taken(dir, &format!("{name}.{ext}")))
		{
			name = format!("{base} ({n})");
			n += 1;
		}
		for ext in extensions {
			self.reserve(&format!("{name}.{ext}"));
		}
		name
	}

	/// The first free file name `file(n)` for `n` counting up from `from`,
	/// handed out.
	fn free(&mut self, dir: &Path, from: usize, file: impl Fn(usize) -> String) -> String {
		let mut n = from;
		while self.taken(dir, &file(n)) {
			n += 1;
		}
		let file = file(n);
		self.reserve(&file);
		file
	}

	fn taken(&self, dir: &Path, file: &str) -> bool {
		self.0.contains(&file.to_lowercase()) || dir.join(file).exists()
	}

	fn reserve(&mut self, file: &str) {
		self.0.insert(file.to_lowercase());
	}
}

fn export_mesh_glb(
	ctx: &egui::Context,
	path: &Path,
	name: &str,
	job: &MeshJob,
) -> Result<(), String> {
	let bytes = std::fs::read(&job.mesh).map_err(|e| e.to_string())?;
	let geometry = geometry::load(&bytes)?.geometry;
	let material = if geometry.uvs.is_some() {
		texture::load_maps(ctx, &job.maps)
	} else {
		Material::default()
	};
	let groups = geometry.default_groups();
	crate::export::gltf::export_mesh(&geometry, &groups, &material, PART_COLOUR, name, path)
}

/// Fixtures shared by the OBJ and glTF tests.
#[cfg(test)]
mod fixtures {
	use std::path::PathBuf;

	use serde_json::Value;

	use crate::scene::geometry;
	use crate::test_support::SAMPLE_MESHES;

	/// The first sample mesh, by file name, that has triangles (and texture
	/// coordinates when `need_uvs`), so every run picks the same one.
	pub(super) fn sample_mesh(need_uvs: bool) -> PathBuf {
		let mut paths: Vec<PathBuf> = std::fs::read_dir(SAMPLE_MESHES)
			.unwrap()
			.map(|e| e.unwrap().path())
			.collect();
		paths.sort();
		paths
			.into_iter()
			.find(|p| {
				std::fs::read(p)
					.ok()
					.and_then(|b| geometry::load(&b).ok())
					.is_some_and(|l| {
						!l.geometry.triangles.is_empty() && (!need_uvs || l.geometry.uvs.is_some())
					})
			})
			.expect("a sample mesh")
	}

	/// Splits a binary glTF into its JSON document and binary chunk,
	/// checking the header and chunk lengths on the way.
	pub(super) fn parse_glb(glb: &[u8]) -> (Value, &[u8]) {
		let word = |at: usize| u32::from_le_bytes(glb[at..at + 4].try_into().unwrap()) as usize;
		assert_eq!(&glb[..4], b"glTF");
		assert_eq!(word(4), 2, "version");
		assert_eq!(word(8), glb.len(), "total length");
		let json_len = word(12);
		assert_eq!(&glb[16..20], b"JSON");
		let json = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
		let at = 20 + json_len;
		let bin_len = word(at);
		assert_eq!(&glb[at + 4..at + 8], b"BIN\0");
		assert_eq!(
			at + 8 + bin_len,
			glb.len(),
			"the binary chunk ends the file"
		);
		(json, &glb[at + 8..])
	}

	/// Every face corner points at a position, texture coordinate and
	/// normal that the file holds, though parts differ in what they have.
	pub(super) fn check_indices(obj: &str) {
		let count = |prefix: &str| obj.lines().filter(|l| l.starts_with(prefix)).count();
		let counts = [count("v "), count("vt "), count("vn ")];
		for face in obj.lines().filter(|l| l.starts_with("f ")) {
			for corner in face.split_whitespace().skip(1) {
				for (k, index) in corner.split('/').enumerate() {
					if index.is_empty() {
						continue;
					}
					let index: usize = index.parse().unwrap();
					assert!((1..=counts[k]).contains(&index), "{face}: {counts:?}");
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::export::fixtures::{check_indices, parse_glb, sample_mesh};
	use crate::test_support::TempDir;

	#[test]
	fn exports_a_sample_mesh() {
		let mesh = sample_mesh(true);
		let dir = TempDir::new("obj_sample");
		let job = MeshJob {
			name: "Sample".into(),
			mesh,
			maps: Vec::new(),
		};
		let report = export(
			&egui::Context::default(),
			&dir,
			vec![job],
			Vec::new(),
			Format::Obj,
		);
		assert_eq!(report.written, 1, "{:?}", report.failed);
		let obj = std::fs::read_to_string(dir.join("Sample.obj")).unwrap();
		assert!(obj.contains("mtllib Sample.mtl") && obj.contains("\nvt "));
		check_indices(&obj);
		assert!(dir.join("Sample.mtl").exists());
	}

	#[test]
	fn exports_a_sample_mesh_as_glb() {
		let mesh = sample_mesh(false);
		let dir = TempDir::new("glb_sample");
		let job = MeshJob {
			name: "Sample".into(),
			mesh,
			maps: Vec::new(),
		};
		let report = export(
			&egui::Context::default(),
			&dir,
			vec![job],
			Vec::new(),
			Format::Glb,
		);
		assert_eq!(report.written, 1, "{:?}", report.failed);
		let glb = std::fs::read(dir.join("Sample.glb")).unwrap();
		let (doc, bin) = parse_glb(&glb);
		assert_eq!(doc["asset"]["generator"], "Meshvault");
		assert_eq!(doc["buffers"][0]["byteLength"], bin.len());
		assert_eq!(doc["nodes"][0]["name"], "Sample");
		assert_eq!(doc["meshes"][0]["name"], "Sample");
		assert_eq!(doc["meshes"].as_array().unwrap().len(), 1);
	}

	#[test]
	fn names_are_sanitised_and_numbered() {
		let dir = TempDir::new("obj_names");
		let mut names = Names::default();
		assert_eq!(names.unique(&dir, "a", Format::Obj), "a");
		assert_eq!(names.unique(&dir, "a", Format::Obj), "a (2)");
		// clashes ignore case
		assert_eq!(names.unique(&dir, "A", Format::Obj), "A (3)");
		assert_eq!(names.unique(&dir, "a/b", Format::Obj), "a_b");
		// sanitize never returns an empty name, so this is `_`
		assert_eq!(names.unique(&dir, "", Format::Obj), "_");
		assert_eq!(names.unique(&dir, "  ", Format::Obj), "_ (2)");
	}

	#[test]
	fn names_avoid_files_already_there() {
		let dir = TempDir::new("obj_names_files");
		std::fs::write(dir.join("b.obj"), "").unwrap();
		std::fs::write(dir.join("b (2).obj"), "").unwrap();
		assert_eq!(Names::default().unique(&dir, "b", Format::Obj), "b (3)");
		// only files of the format being written count
		assert_eq!(Names::default().unique(&dir, "b", Format::Glb), "b");
		// an OBJ's MTL counts too
		std::fs::write(dir.join("c.mtl"), "").unwrap();
		assert_eq!(Names::default().unique(&dir, "c", Format::Obj), "c (2)");
		assert_eq!(Names::default().unique(&dir, "c", Format::Glb), "c");
	}

	fn map_file(dir: &Path, name: &str) -> PathBuf {
		let path = dir.join(name);
		std::fs::write(&path, name).unwrap();
		path
	}

	/// A later job's sidecar must not land on an earlier one's: `Tree`'s
	/// normal map is `Tree_2.normal.png`, which `Tree_2` would also pick.
	#[test]
	fn obj_batches_never_overwrite_a_file() {
		let dir = TempDir::new("obj_batch_sidecars");
		let maps = TempDir::new("obj_batch_maps");
		std::fs::write(dir.join("Old.mtl"), "keep").unwrap();
		std::fs::write(dir.join("Old.png"), "keep").unwrap();
		let job = |name: &str, maps: Vec<(Slot, PathBuf)>| MeshJob {
			name: name.into(),
			mesh: sample_mesh(true),
			maps,
		};
		let jobs = vec![
			job(
				"Tree",
				vec![
					(Slot::Base, map_file(&maps, "tree base.png")),
					(Slot::Normal, map_file(&maps, "tree normal.png")),
				],
			),
			job(
				"Tree_2",
				vec![(Slot::Normal, map_file(&maps, "tree 2 normal.png"))],
			),
			job("Old", vec![(Slot::Base, map_file(&maps, "old base.png"))]),
		];
		let report = export(
			&egui::Context::default(),
			&dir,
			jobs,
			Vec::new(),
			Format::Obj,
		);
		assert_eq!(report.written, 3, "{:?}", report.failed);
		let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
		assert_eq!(read("Tree.png"), "tree base.png");
		assert_eq!(read("Tree_2.normal.png"), "tree normal.png");
		assert!(read("Tree.mtl").contains("norm Tree_2.normal.png"));
		assert_eq!(read("Tree_2_2.normal.png"), "tree 2 normal.png");
		assert!(read("Tree_2.mtl").contains("norm Tree_2_2.normal.png"));
		// the files already there are kept, and the new ones named around them
		assert_eq!(read("Old.mtl"), "keep");
		assert_eq!(read("Old.png"), "keep");
		assert!(read("Old (2).obj").contains("mtllib Old (2).mtl"));
		assert!(read("Old (2).mtl").contains("map_Kd Old (2).png"));
	}

	/// Exports every animated model of the workspace at `MESHVAULT_WS`
	/// through the batch export, and checks each OBJ's face indices.
	/// Ignored as it needs a real workspace; run it with
	/// `MESHVAULT_WS=<workspace folder> cargo test -p meshvault -- --ignored export::tests`.
	/// It writes to `meshvault-obj-test-models` in the temp folder (emptied
	/// first) and leaves the files there to inspect. The glTF counterpart
	/// is `gltf::tests::exports_workspace_models`.
	#[test]
	#[ignore]
	fn exports_workspace_models() {
		let root = std::env::var("MESHVAULT_WS").expect("MESHVAULT_WS");
		let ws = crate::workspace::Workspace::open(Path::new(&root)).unwrap();
		let models = (0..ws.models().len())
			.map(|i| ModelJob {
				name: ws.models()[i].name.clone(),
				load: Box::new(crate::workspace::view_loader::model_loader(&ws, i).unwrap()),
			})
			.collect();
		let dir = std::env::temp_dir().join("meshvault-obj-test-models");
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		let report = export(
			&egui::Context::default(),
			&dir,
			Vec::new(),
			models,
			Format::Obj,
		);
		assert!(report.failed.is_empty(), "{:?}", report.failed);
		assert_eq!(report.written, ws.models().len());
		for entry in std::fs::read_dir(&dir).unwrap() {
			let path = entry.unwrap().path();
			if path.extension().is_some_and(|e| e == "obj") {
				check_indices(&std::fs::read_to_string(&path).unwrap());
			}
		}
		println!("exported to {}", dir.display());
	}
}
