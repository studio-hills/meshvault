//! Facts about each downloaded mesh that the library filters on (whether
//! it is skinned, how many triangles), read once on a background thread
//! and kept in `.cache/facts.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::scene::geometry;
use crate::studio::export_file::read_limited;
use crate::workspace::paths::replace_file;
use crate::workspace::{AssetState, CACHE_DIR, Workspace, saved_file};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshFacts {
	/// Bound to bones, so it bends with them.
	pub skinned: bool,
	pub triangles: usize,
}

/// The facts read from a path, and the mesh asset that was there. A
/// removed item's path can be reused by a different mesh, whose facts
/// are then read again.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Known {
	mesh_id: u64,
	#[serde(flatten)]
	facts: MeshFacts,
}

/// What the scan sends for each mesh: its path, its asset, and its facts,
/// or `None` when the file could not be read.
type Scanned = (String, u64, Option<MeshFacts>);

pub struct Facts {
	root: PathBuf,
	file: PathBuf,
	/// By the mesh's path in the workspace.
	known: HashMap<String, Known>,
	/// Meshes whose files could not be read this session: path to asset.
	/// They are not read again until the workspace is opened again, and
	/// not saved, so a passing failure is not kept for good.
	unreadable: HashMap<String, u64>,
	scan: Option<Receiver<Scanned>>,
	/// Meshes the running scan still has to read.
	left: usize,
	dirty: bool,
	/// Bumped when scan results arrive, so filtered lists can cache matches.
	generation: u64,
	/// An unchanged workspace needs no new scan for unread meshes.
	scanned_for: Option<(u64, u64)>,
}

impl Facts {
	pub fn open(root: &Path) -> Self {
		let file = root.join(facts_rel());
		// a file from before facts recorded the asset does not parse, so
		// every mesh is read again once; nor is one read through a link
		let known = saved_file(root, facts_rel())
			.map(|file| read_known(&file, MAX_FACTS_BYTES))
			.unwrap_or_default();
		Self {
			root: root.to_path_buf(),
			file,
			known,
			unreadable: HashMap::new(),
			scan: None,
			left: 0,
			dirty: false,
			generation: 0,
			scanned_for: None,
		}
	}

	/// What is known about mesh asset `mesh_id` downloaded to `path`.
	pub fn get(&self, path: &str, mesh_id: u64) -> Option<MeshFacts> {
		self.known
			.get(path)
			.filter(|k| k.mesh_id == mesh_id)
			.map(|k| k.facts)
	}

	/// How many meshes are still being read.
	pub fn reading(&self) -> usize {
		self.left
	}

	/// Changes to the known mesh facts while this workspace is open.
	pub fn generation(&self) -> u64 {
		self.generation
	}

	fn take(&mut self, (path, mesh_id, facts): Scanned) {
		self.generation = self.generation.wrapping_add(1);
		match facts {
			Some(facts) => {
				self.known.insert(path, Known { mesh_id, facts });
				self.dirty = true;
			}
			None => {
				self.unreadable.insert(path, mesh_id);
			}
		}
		self.left = self.left.saturating_sub(1);
	}

	/// Takes in what the scan read, and starts a scan of downloaded meshes
	/// not read yet. Call once a frame.
	pub fn update(&mut self, ctx: &egui::Context, ws: &Workspace) {
		if let Some(rx) = self.scan.take() {
			loop {
				match rx.try_recv() {
					Ok(scanned) => self.take(scanned),
					Err(TryRecvError::Empty) => {
						self.scan = Some(rx);
						break;
					}
					Err(TryRecvError::Disconnected) => {
						self.left = 0;
						self.save();
						break;
					}
				}
			}
			return;
		}
		let generation = (ws.generation(), ws.asset_generation());
		if self.scanned_for == Some(generation) {
			return;
		}
		self.scanned_for = Some(generation);
		let unread: Vec<(String, u64)> = ws
			.items()
			.iter()
			.filter_map(|i| match &i.mesh {
				AssetState::Done { path }
					if self.get(path, i.mesh_id).is_none()
						&& self.unreadable.get(path) != Some(&i.mesh_id) =>
				{
					Some((path.clone(), i.mesh_id))
				}
				_ => None,
			})
			.collect();
		if unread.is_empty() {
			return;
		}
		let (tx, rx) = std::sync::mpsc::channel();
		let root = ws.root.clone();
		let ctx = ctx.clone();
		self.left = unread.len();
		std::thread::spawn(move || {
			for (path, mesh_id) in unread {
				// one behind a link is not read, and counts as unreadable
				let facts = saved_file(&root, &path).ok().and_then(|file| read(&file));
				if tx.send((path, mesh_id, facts)).is_err() {
					return;
				}
				ctx.request_repaint();
			}
		});
		self.scan = Some(rx);
	}

	/// Test helper: starts a scan like [`Self::update`], then waits for it
	/// to finish and takes in all it read.
	#[cfg(test)]
	fn scan_all(&mut self, ws: &Workspace) {
		self.update(&egui::Context::default(), ws);
		if let Some(rx) = self.scan.take() {
			for scanned in rx {
				self.take(scanned);
			}
			self.left = 0;
		}
	}

	fn save(&mut self) {
		if !self.dirty {
			return;
		}
		self.dirty = false;
		// facts.json is a cache that can be rebuilt, so failing to write it
		// is not worth reporting; nor is one written through a link (see
		// [`replace_file`])
		if let Some(dir) = self.file.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		if let Ok(text) = serde_json::to_string(&self.known) {
			let _ = replace_file(&self.root, &facts_rel(), text.as_bytes());
		}
	}
}

impl Drop for Facts {
	fn drop(&mut self) {
		self.save();
	}
}

/// The facts in `file`, or none if it does not parse or is longer than
/// `limit` bytes: the facts are then read again from the meshes.
fn read_known(file: &Path, limit: usize) -> HashMap<String, Known> {
	read_limited(file, limit)
		.ok()
		.and_then(|bytes| serde_json::from_slice(&bytes).ok())
		.unwrap_or_default()
}

/// The largest facts.json read. Each mesh takes some hundred bytes, so
/// this is room for several hundred thousand.
const MAX_FACTS_BYTES: usize = 64 * 1024 * 1024;

/// Where the facts are kept, under the workspace's root.
fn facts_rel() -> PathBuf {
	Path::new(CACHE_DIR).join("facts.json")
}

/// Reads a mesh's facts, or `None` if the file cannot be read. A file that
/// does not decode counts as an empty, unskinned mesh, so it is not read
/// again.
fn read(path: &Path) -> Option<MeshFacts> {
	let bytes = crate::workspace::read_asset(path).ok()?;
	let Ok((loaded, skin)) = geometry::load_with_skin(&bytes) else {
		return Some(MeshFacts::default());
	};
	let geometry = loaded.geometry;
	let skinned = skin.ok().flatten().is_some_and(|s| !s.bones.is_empty());
	Some(MeshFacts {
		skinned,
		triangles: geometry.triangles.len(),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::studio::export_file::{Entry, Origin};
	use crate::test_support::{SAMPLE_MESHES, TempDir};
	use crate::workspace::AssetKind;

	/// A mesh's triangles are counted, and it is skinned only if it is bound
	/// to bones: the known skinned head is, the version 2 torso is not.
	#[test]
	fn reads_a_mesh() {
		let torso = read(&Path::new(SAMPLE_MESHES).join("torso.mesh")).unwrap();
		assert!(torso.triangles > 0);
		assert!(!torso.skinned);
		let head = read(&Path::new(SAMPLE_MESHES).join("13674780763")).unwrap();
		assert!(head.triangles > 0);
		assert!(head.skinned);
	}

	/// A file that does not decode reads as empty and unskinned (and is
	/// then cached as that); one that cannot be read has no facts.
	#[test]
	fn a_bad_file_reads_as_empty_and_a_missing_one_as_nothing() {
		let dir = TempDir::new("facts_bad");
		assert_eq!(read(&dir.join("missing.mesh")), None);
		std::fs::write(dir.join("bad.mesh"), b"version 9.99\nnot a mesh").unwrap();
		assert_eq!(read(&dir.join("bad.mesh")), Some(MeshFacts::default()));
	}

	/// Adds a new item A to `ws` whose download is a copy of the sample mesh
	/// `sample` at `Models/A.mesh`, as mesh asset `mesh_id`; with A removed
	/// first, this is a removed item's stem used again.
	fn download_a(ws: &mut Workspace, sample: &str, mesh_id: u64) {
		let entry = Entry {
			name: "A".into(),
			folder: Vec::new(),
			mesh_id,
			maps: Default::default(),
			source_id: None,
		};
		let (stem, _) = ws.add_entry(&entry, &Origin::default());
		std::fs::create_dir_all(ws.root.join("Models")).unwrap();
		std::fs::copy(
			Path::new(SAMPLE_MESHES).join(sample),
			ws.root.join("Models/A.mesh"),
		)
		.unwrap();
		let path = "Models/A.mesh".to_string();
		ws.set_state(&stem, AssetKind::Mesh, AssetState::Done { path });
	}

	/// A different mesh downloaded to a path whose mesh was read before is
	/// read again, in the same session and after reopening.
	#[test]
	fn a_reused_path_with_another_mesh_is_read_again() {
		let dir = TempDir::new("facts_reuse");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		download_a(&mut ws, "torso.mesh", 1);
		let mut facts = Facts::open(&dir);
		facts.scan_all(&ws);
		let torso = facts.get("Models/A.mesh", 1).unwrap();

		ws.remove(&[0]).unwrap();
		download_a(&mut ws, "sphere.mesh", 2);
		assert_eq!(ws.items()[0].stem, "Models/A");
		facts.scan_all(&ws);
		let sphere = facts.get("Models/A.mesh", 2).unwrap();
		assert_ne!(sphere, torso);
		// what was read about the old mesh no longer counts
		assert_eq!(facts.get("Models/A.mesh", 1), None);

		drop(facts);
		ws.remove(&[0]).unwrap();
		download_a(&mut ws, "torso.mesh", 3);
		let mut facts = Facts::open(&dir);
		assert_eq!(facts.get("Models/A.mesh", 2), Some(sphere));
		facts.scan_all(&ws);
		assert_eq!(facts.get("Models/A.mesh", 3), Some(torso));
	}

	/// A mesh whose file cannot be read is not read again this session,
	/// but is not saved as empty either, so it is read once the workspace
	/// opens again.
	#[test]
	fn a_file_that_cannot_be_read_is_tried_again_next_time() {
		let dir = TempDir::new("facts_unreadable");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		download_a(&mut ws, "torso.mesh", 1);
		let mesh = dir.join("Models/A.mesh");
		let bytes = std::fs::read(&mesh).unwrap();
		std::fs::remove_file(&mesh).unwrap();
		let mut facts = Facts::open(&dir);
		facts.scan_all(&ws);
		assert_eq!(facts.get("Models/A.mesh", 1), None);
		// no second scan this session
		facts.update(&egui::Context::default(), &ws);
		assert!(facts.scan.is_none());
		drop(facts);

		std::fs::write(&mesh, bytes).unwrap();
		let mut facts = Facts::open(&dir);
		facts.scan_all(&ws);
		assert!(facts.get("Models/A.mesh", 1).unwrap().triangles > 0);
	}
	/// Facts are neither read nor saved through a `.cache` that links
	/// elsewhere (N01).
	#[test]
	fn facts_are_not_kept_through_a_link() {
		let parent = TempDir::new("facts_linked");
		let (dir, outside) = (parent.join("ws"), parent.join("outside"));
		std::fs::create_dir_all(&outside).unwrap();
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		download_a(&mut ws, "torso.mesh", 1);
		std::fs::remove_dir_all(dir.join(CACHE_DIR)).unwrap();
		if !crate::workspace::junction(&dir.join(CACHE_DIR), &outside) {
			return;
		}
		let mut facts = Facts::open(&dir);
		facts.scan_all(&ws);
		assert!(facts.get("Models/A.mesh", 1).is_some());
		drop(facts);
		assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
	}
	/// R3-01: facts are saved under a fresh temporary name and renamed into
	/// place, so a link at either name is never written through.
	#[test]
	fn facts_are_never_saved_through_a_file_link() {
		use crate::workspace::paths::{file_link, set_temp_tags};
		let parent = TempDir::new("facts_file_link");
		let dir = parent.join("ws");
		let outside = parent.join("outside.txt");
		std::fs::write(&outside, b"keep me").unwrap();
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		download_a(&mut ws, "torso.mesh", 1);
		let kind = file_link(&dir.join(".cache/facts.json"), &outside);
		file_link(&dir.join(".cache/facts.json.a.tmp"), &outside);
		let mut facts = Facts::open(&dir);
		facts.scan_all(&ws);
		set_temp_tags(Some(&["a", "b"]));
		drop(facts);
		set_temp_tags(None);
		assert_eq!(std::fs::read(&outside).unwrap(), b"keep me", "{kind}");
		if kind == "hard link" {
			// saved in place of the link
			assert!(Facts::open(&dir).get("Models/A.mesh", 1).is_some());
		}
	}

	/// N02: a facts file longer than its limit is not read, so every mesh
	/// is read again.
	#[test]
	fn an_oversized_facts_file_is_not_read() {
		let dir = TempDir::new("facts_limit");
		let file = dir.join("facts.json");
		let text = r#"{"Models/A.mesh":{"mesh_id":1,"skinned":false,"triangles":3}}"#;
		std::fs::write(&file, text).unwrap();
		assert_eq!(read_known(&file, text.len()).len(), 1);
		assert!(read_known(&file, text.len() - 1).is_empty());
	}

	/// R4-03: a downloaded mesh behind a link below `Models` was read for
	/// its facts. Now it is not read, and counts as unreadable.
	#[test]
	fn facts_are_not_read_behind_a_link() {
		let parent = TempDir::new("facts_behind_link");
		let (dir, outside) = (parent.join("ws"), parent.join("outside"));
		std::fs::create_dir_all(&outside).unwrap();
		std::fs::copy(
			Path::new(SAMPLE_MESHES).join("torso.mesh"),
			outside.join("A.mesh"),
		)
		.unwrap();
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		download_a(&mut ws, "torso.mesh", 1);
		if !crate::workspace::junction(&dir.join("Models/External"), &outside) {
			return;
		}
		let path = "Models/External/A.mesh".to_string();
		ws.set_state("Models/A", AssetKind::Mesh, AssetState::Done { path });
		let mut facts = Facts::open(&dir);
		facts.scan_all(&ws);
		assert_eq!(
			facts.get("Models/External/A.mesh", 1),
			None,
			"the mesh behind the link was read"
		);
		assert_eq!(facts.unreadable.get("Models/External/A.mesh"), Some(&1));
	}
}
