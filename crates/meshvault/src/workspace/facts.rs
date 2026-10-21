//! Facts about each downloaded mesh that the library filters on (whether
//! it is skinned, how many triangles), read once on a background thread
//! and kept in `.cache/facts.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::scene::geometry;
use crate::workspace::{AssetState, CACHE_DIR, Workspace};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshFacts {
	/// Bound to bones, so it bends with them.
	pub skinned: bool,
	pub triangles: usize,
}

pub struct Facts {
	file: PathBuf,
	/// By the mesh's path in the workspace.
	known: HashMap<String, MeshFacts>,
	scan: Option<Receiver<(String, MeshFacts)>>,
	/// Meshes the running scan still has to read.
	left: usize,
	dirty: bool,
}

impl Facts {
	pub fn open(root: &Path) -> Self {
		let file = root.join(CACHE_DIR).join("facts.json");
		let known = std::fs::read_to_string(&file)
			.ok()
			.and_then(|text| serde_json::from_str(&text).ok())
			.unwrap_or_default();
		Self {
			file,
			known,
			scan: None,
			left: 0,
			dirty: false,
		}
	}

	/// What is known about the mesh downloaded to `path`.
	pub fn get(&self, path: &str) -> Option<MeshFacts> {
		self.known.get(path).copied()
	}

	/// How many meshes are still being read.
	pub fn reading(&self) -> usize {
		self.left
	}

	/// Takes in what the scan read, and starts a scan of downloaded meshes
	/// not read yet. Call once a frame.
	pub fn update(&mut self, ctx: &egui::Context, ws: &Workspace) {
		if let Some(rx) = &self.scan {
			loop {
				match rx.try_recv() {
					Ok((path, facts)) => {
						self.known.insert(path, facts);
						self.left = self.left.saturating_sub(1);
						self.dirty = true;
					}
					Err(TryRecvError::Empty) => break,
					Err(TryRecvError::Disconnected) => {
						self.scan = None;
						self.left = 0;
						self.save();
						break;
					}
				}
			}
			return;
		}
		let unread: Vec<String> = ws
			.items()
			.iter()
			.filter_map(|i| match &i.mesh {
				AssetState::Done { path } if !self.known.contains_key(path) => Some(path.clone()),
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
			for path in unread {
				let facts = read(&root.join(&path));
				if tx.send((path, facts)).is_err() {
					return;
				}
				ctx.request_repaint();
			}
		});
		self.scan = Some(rx);
	}

	fn save(&mut self) {
		if !self.dirty {
			return;
		}
		self.dirty = false;
		// facts.json is a cache that can be rebuilt, so failing to write it
		// is not worth reporting
		if let Some(dir) = self.file.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		if let Ok(text) = serde_json::to_string(&self.known) {
			let _ = std::fs::write(&self.file, text);
		}
	}
}

impl Drop for Facts {
	fn drop(&mut self) {
		self.save();
	}
}

/// Reads a mesh's facts. A file that cannot be read or decoded counts as an
/// empty, unskinned mesh, so it is not read again.
fn read(path: &Path) -> MeshFacts {
	let Ok(bytes) = std::fs::read(path) else {
		return MeshFacts::default();
	};
	let Ok(loaded) = geometry::load(&bytes) else {
		return MeshFacts::default();
	};
	let geometry = loaded.geometry;
	let skinned = geometry::skin(&bytes, geometry.positions.len())
		.ok()
		.flatten()
		.is_some_and(|s| !s.bones.is_empty());
	MeshFacts {
		skinned,
		triangles: geometry.triangles.len(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::{SAMPLE_MESHES, TempDir};

	#[test]
	fn reads_a_mesh() {
		let facts = read(&Path::new(SAMPLE_MESHES).join("torso.mesh"));
		assert!(facts.triangles > 0);
	}

	/// A file that is missing or does not decode reads as empty and
	/// unskinned (and is then cached as that).
	#[test]
	fn a_bad_file_reads_as_empty() {
		let dir = TempDir::new("facts_bad");
		assert_eq!(read(&dir.join("missing.mesh")), MeshFacts::default());
		std::fs::write(dir.join("bad.mesh"), b"version 9.99\nnot a mesh").unwrap();
		assert_eq!(read(&dir.join("bad.mesh")), MeshFacts::default());
	}
}
