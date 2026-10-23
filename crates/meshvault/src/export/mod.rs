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
/// other or with files already there (another export's included, even one
/// running at the same time), get a number. A job that fails leaves none of
/// its files behind.
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
		let mark = names.mark();
		let name = names.unique(dir, &job.name, format);
		let result = match format {
			Format::Obj => export_mesh(dir, &name, &job, &mut names),
			Format::Glb => export_mesh_glb(ctx, &dir.join(format!("{name}.glb")), &name, &job),
		};
		match result {
			Ok(()) => report.written += 1,
			Err(e) => {
				names.discard_since(mark);
				report.failed.push(format!("{}: {e}", job.name));
			}
		}
	}
	for job in models {
		let mark = names.mark();
		let name = names.unique(dir, &job.name, format);
		let result = (job.load)(ctx).and_then(|mut model| match format {
			Format::Obj => export_model(dir, &name, &mut model, &mut names),
			Format::Glb => {
				crate::export::gltf::export_model(&model, &dir.join(format!("{name}.glb")))
			}
		});
		match result {
			Ok(()) => report.written += 1,
			Err(e) => {
				names.discard_since(mark);
				report.failed.push(format!("{}: {e}", job.name));
			}
		}
	}
	report
}

/// File names in the export folder handed out so far (lower case, as
/// Windows ignores case), so no file an export writes, the OBJ's MTL and
/// maps included, replaces another of the same batch or one already there.
///
/// A name is handed out by creating its file, empty, only if there is no
/// such file yet, so another export writing into the same folder at the
/// same time (or another program) cannot be handed it too. The export then
/// writes over its own empty file.
#[derive(Default)]
struct Names {
	handed_out: std::collections::HashSet<String>,
	/// The files made by handing names out, in order.
	made: Vec<PathBuf>,
}

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
		loop {
			let mark = self.mark();
			if extensions
				.iter()
				.all(|ext| self.claim(dir, &format!("{name}.{ext}")))
			{
				return name;
			}
			// all or none: the OBJ's name is no use without its MTL's
			self.discard_since(mark);
			name = format!("{base} ({n})");
			n += 1;
		}
	}

	/// The first free file name `file(n)` for `n` counting up from `from`,
	/// handed out.
	fn free(&mut self, dir: &Path, from: usize, file: impl Fn(usize) -> String) -> String {
		let mut n = from;
		while !self.claim(dir, &file(n)) {
			n += 1;
		}
		file(n)
	}

	/// Hands out `file` in `dir` if it is free, making it.
	fn claim(&mut self, dir: &Path, file: &str) -> bool {
		if self.handed_out.contains(&file.to_lowercase()) {
			return false;
		}
		let path = dir.join(file);
		match std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(&path)
		{
			Ok(_) => self.made.push(path),
			Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return false,
			// it cannot be made (no such folder, say): writing it will fail
			// and say why, unless it is something already there
			Err(_) if path.exists() => return false,
			Err(_) => {}
		}
		self.reserve(file);
		true
	}

	/// Hands out `file` without making it: the OBJ the user chose, which
	/// may replace a file.
	fn reserve(&mut self, file: &str) {
		self.handed_out.insert(file.to_lowercase());
	}

	/// How many files have been made, for [`Self::discard_since`].
	fn mark(&self) -> usize {
		self.made.len()
	}

	/// Deletes the files made since `mark` (they are this export's own),
	/// and takes their names back.
	fn discard_since(&mut self, mark: usize) {
		for path in self.made.drain(mark..) {
			let _ = std::fs::remove_file(&path);
			if let Some(file) = path.file_name() {
				self.handed_out
					.remove(&file.to_string_lossy().to_lowercase());
			}
		}
	}
}

fn export_mesh_glb(
	ctx: &egui::Context,
	path: &Path,
	name: &str,
	job: &MeshJob,
) -> Result<(), String> {
	let bytes = crate::workspace::read_asset(&job.mesh).map_err(|e| e.to_string())?;
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

	/// Names handed out are made on disk, so a second export into the same
	/// folder, with names of its own, does not get them too.
	#[test]
	fn names_are_free_across_exports() {
		let dir = TempDir::new("obj_names_shared");
		let (mut first, mut second) = (Names::default(), Names::default());
		assert_eq!(first.unique(&dir, "Tree", Format::Obj), "Tree");
		assert_eq!(second.unique(&dir, "Tree", Format::Obj), "Tree (2)");
		let sidecar = |n: usize| obj::numbered("Tree", n, ".png");
		assert_eq!(first.free(&dir, 1, sidecar), "Tree.png");
		assert_eq!(second.free(&dir, 1, sidecar), "Tree_2.png");
		// a name taken by another export only for its MTL is passed over
		// whole, and the OBJ tried with it is not left behind
		std::fs::write(dir.join("Bush.mtl"), "").unwrap();
		assert_eq!(first.unique(&dir, "Bush", Format::Obj), "Bush (2)");
		assert!(!dir.join("Bush.obj").exists());
		// a failed job's files go, and their names are free again
		let mark = second.mark();
		assert_eq!(second.unique(&dir, "Rock", Format::Obj), "Rock");
		second.discard_since(mark);
		assert!(!dir.join("Rock.obj").exists() && !dir.join("Rock.mtl").exists());
		assert_eq!(first.unique(&dir, "Rock", Format::Obj), "Rock");
	}

	/// A model job of one block in `colour`, read only once `ready` lets it.
	fn block_job(ready: std::sync::Arc<std::sync::Barrier>, colour: [f32; 3]) -> ModelJob {
		use std::collections::HashMap;

		use crate::anim::math::CFrame;
		use crate::anim::schema::{AnimatedModel, ModelPart};

		let model = AnimatedModel {
			name: "Tree".into(),
			path: "Workspace/Tree".into(),
			parts: vec![ModelPart {
				name: "Trunk".into(),
				path: "Workspace/Tree/Trunk".into(),
				class_name: Some("Part".into()),
				shape: Some("Block".into()),
				size: [1.0; 3],
				color: Some(colour),
				cframe: CFrame::from_position([0.0; 3]).components(),
				..Default::default()
			}],
			..Default::default()
		};
		ModelJob {
			name: "Tree".into(),
			load: Box::new(move |ctx| {
				ready.wait();
				Ok(ModelView::load(
					ctx,
					0,
					&model,
					&HashMap::new(),
					&HashMap::new(),
				))
			}),
		}
	}

	/// Two exports into one folder at once each get their own files. Bug:
	/// each export only knew its own names, so both picked `Tree.obj` and
	/// `Tree.mtl` before reading their models, and the second written
	/// replaced the first.
	#[test]
	fn exports_at_the_same_time_never_share_a_file() {
		let dir = TempDir::new("obj_concurrent");
		// both have picked their names before either reads its model
		let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
		let exports: Vec<_> = [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]
			.map(|colour| {
				let (dir, job) = (dir.to_path_buf(), block_job(ready.clone(), colour));
				std::thread::spawn(move || {
					let ctx = egui::Context::default();
					export(&ctx, &dir, Vec::new(), vec![job], Format::Obj)
				})
			})
			.into_iter()
			.map(|t| t.join().unwrap())
			.collect();
		for report in exports {
			assert_eq!(report.written, 1, "{:?}", report.failed);
		}
		let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
		assert!(read("Tree.obj").contains("mtllib Tree.mtl\n"));
		assert!(read("Tree (2).obj").contains("mtllib Tree (2).mtl\n"));
		let mut kd: Vec<String> = ["Tree.mtl", "Tree (2).mtl"]
			.map(|mtl| {
				read(mtl)
					.lines()
					.find(|l| l.starts_with("Kd "))
					.unwrap()
					.to_string()
			})
			.into();
		kd.sort();
		assert_eq!(kd, ["Kd 0 0 1", "Kd 1 0 0"]);
	}

	/// A job that fails leaves no files, not even the names it was given.
	#[test]
	fn a_failed_job_leaves_nothing() {
		let dir = TempDir::new("obj_failed");
		for format in [Format::Obj, Format::Glb] {
			let model = ModelJob {
				name: "Broken".into(),
				load: Box::new(|_| Err("unreadable".into())),
			};
			let mesh = MeshJob {
				name: "Gone".into(),
				mesh: dir.join("missing.mesh"),
				maps: Vec::new(),
			};
			let report = export(
				&egui::Context::default(),
				&dir,
				vec![mesh],
				vec![model],
				format,
			);
			assert_eq!(report.written, 0);
			assert_eq!(report.failed.len(), 2, "{:?}", report.failed);
		}
		assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
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
