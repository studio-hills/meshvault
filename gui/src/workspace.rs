//! Workspaces: a folder on disk holding downloaded assets, organized to
//! mirror the folders of the export they were imported from.
//!
//! Layout:
//! ```text
//! <workspace>/
//!   workspace.json                 manifest (items and their download state)
//!   Models/<Folder>/<Name>.mesh    organized copies, browsable in Explorer
//!   Models/<Folder>/<Name>.png     TextureID, next to its mesh (same file stem)
//!   Models/<Folder>/<Name>.normal.png   SurfaceAppearance maps (.color, .normal, ...)
//!   .cache/<asset id>              one raw copy per asset id, so duplicates download once
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::export::Entry;
use crate::maps::MapKind;

pub const MANIFEST: &str = "workspace.json";
pub const MODELS_DIR: &str = "Models";
pub const CACHE_DIR: &str = ".cache";
const MANIFEST_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AssetState {
	Pending,
	/// `path` is relative to the workspace root.
	Done { path: String },
	Failed { error: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapRef {
	pub id: u64,
	pub state: AssetState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
	pub name: String,
	pub folder: Vec<String>,
	pub mesh_id: u64,
	/// Relative path without extension, unique within the workspace.
	pub stem: String,
	pub mesh: AssetState,
	/// Maps selected for import. Maps that were not selected are absent.
	#[serde(default)]
	pub maps: BTreeMap<MapKind, MapRef>,
	// Version 1 manifests stored only a texture; migrated into `maps` on open.
	#[serde(default, skip_serializing)]
	texture_id: Option<u64>,
	#[serde(default, skip_serializing)]
	texture: Option<AssetState>,
}

impl Item {
	fn states(&self) -> impl Iterator<Item = &AssetState> {
		std::iter::once(&self.mesh).chain(self.maps.values().map(|m| &m.state))
	}
	pub fn is_done(&self) -> bool {
		self.states().all(|s| matches!(s, AssetState::Done { .. }))
	}
	pub fn has_failure(&self) -> bool {
		self.states().any(|s| matches!(s, AssetState::Failed { .. }))
	}
	pub fn has_pending(&self) -> bool {
		self.states().any(|s| *s == AssetState::Pending)
	}
	/// Path of a downloaded file, relative to the workspace root.
	pub fn done_path(&self, kind: AssetKind) -> Option<&str> {
		let state = match kind {
			AssetKind::Mesh => &self.mesh,
			AssetKind::Map(k) => &self.maps.get(&k)?.state,
		};
		match state {
			AssetState::Done { path } => Some(path.as_str()),
			_ => None,
		}
	}
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
	pub version: u32,
	pub name: String,
	pub items: Vec<Item>,
}

pub struct Workspace {
	pub root: PathBuf,
	pub manifest: Manifest,
	/// Lower-cased stems in use, since Windows paths are case-insensitive.
	stems: HashSet<String>,
	dirty: bool,
	/// Bumped whenever items are added or removed, so views can cache.
	generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
	Mesh,
	Map(MapKind),
}

impl AssetKind {
	pub fn is_image(self) -> bool {
		matches!(self, AssetKind::Map(_))
	}
	pub fn file_suffix(self) -> &'static str {
		match self {
			AssetKind::Mesh => "",
			AssetKind::Map(k) => k.file_suffix(),
		}
	}
}

/// One asset to download, and every (item stem, use) that needs it. The same
/// image is often both a model's TextureID and its ColorMap, or shared by
/// many models; it is downloaded once.
#[derive(Clone, Debug)]
pub struct Job {
	pub asset_id: u64,
	pub targets: Vec<(String, AssetKind)>,
}

impl Job {
	pub fn wants_image(&self) -> bool {
		self.targets.iter().any(|(_, k)| k.is_image())
	}
}

/// Where the raw download of an asset is kept, shared by every item using it.
pub fn cache_path(root: &Path, asset_id: u64) -> PathBuf {
	root.join(CACHE_DIR).join(asset_id.to_string())
}

/// Makes one path component safe on Windows, macOS and Linux.
pub fn sanitize(name: &str) -> String {
	let mut s: String = name
		.chars()
		.map(|c| match c {
			'<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
			c if c.is_control() => '_',
			c => c,
		})
		.collect();
	// Windows silently drops trailing dots and spaces
	while s.ends_with(['.', ' ']) {
		s.pop();
	}
	let s = s.trim_start().to_string();
	if s.is_empty() {
		return "_".into();
	}
	const RESERVED: [&str; 22] = [
		"CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
		"COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
	];
	let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
	if RESERVED.contains(&base.as_str()) {
		format!("{s}_")
	} else {
		s
	}
}

impl Workspace {
	pub fn create(root: &Path, name: &str) -> Result<Self, String> {
		if root.join(MANIFEST).exists() {
			return Err(format!("{} already contains a workspace", root.display()));
		}
		std::fs::create_dir_all(root)
			.map_err(|e| format!("Could not create {}: {e}", root.display()))?;
		let ws = Self {
			root: root.to_path_buf(),
			manifest: Manifest {
				version: MANIFEST_VERSION,
				name: name.to_string(),
				items: Vec::new(),
			},
			stems: HashSet::new(),
			dirty: true,
			generation: 0,
		};
		ws.write()?;
		Ok(ws)
	}

	pub fn open(root: &Path) -> Result<Self, String> {
		let path = root.join(MANIFEST);
		let json = std::fs::read_to_string(&path)
			.map_err(|e| format!("Could not read {}: {e}", path.display()))?;
		let mut manifest: Manifest = serde_json::from_str(&json)
			.map_err(|e| format!("{} is not a valid workspace: {e}", path.display()))?;
		if manifest.version > MANIFEST_VERSION {
			return Err(format!(
				"{} was made by a newer version of this app (workspace version {})",
				path.display(),
				manifest.version
			));
		}
		let mut dirty = false;
		for item in &mut manifest.items {
			// version 1: a single optional texture
			if let (Some(id), Some(state)) = (item.texture_id.take(), item.texture.take()) {
				item.maps.insert(MapKind::Texture, MapRef { id, state });
				dirty = true;
			}
			// downloads interrupted by closing the app, or files deleted
			// outside the app, are downloaded again
			let missing = |state: &AssetState| {
				matches!(state, AssetState::Done { path } if !root.join(path).exists())
			};
			if missing(&item.mesh) {
				item.mesh = AssetState::Pending;
				dirty = true;
			}
			for map in item.maps.values_mut() {
				if missing(&map.state) {
					map.state = AssetState::Pending;
					dirty = true;
				}
			}
		}
		if manifest.version != MANIFEST_VERSION {
			manifest.version = MANIFEST_VERSION;
			dirty = true;
		}
		let stems = manifest.items.iter().map(|i| i.stem.to_lowercase()).collect();
		Ok(Self {
			root: root.to_path_buf(),
			manifest,
			stems,
			dirty,
			generation: 0,
		})
	}

	pub fn name(&self) -> &str {
		&self.manifest.name
	}

	pub fn items(&self) -> &[Item] {
		&self.manifest.items
	}

	pub fn generation(&self) -> u64 {
		self.generation
	}

	fn write(&self) -> Result<(), String> {
		let json = serde_json::to_string_pretty(&self.manifest).map_err(|e| e.to_string())?;
		// write then rename so a crash never leaves a half-written manifest
		let tmp = self.root.join(format!("{MANIFEST}.tmp"));
		std::fs::write(&tmp, json).map_err(|e| format!("Could not save workspace: {e}"))?;
		std::fs::rename(&tmp, self.root.join(MANIFEST))
			.map_err(|e| format!("Could not save workspace: {e}"))
	}

	pub fn save_if_dirty(&mut self) -> Result<(), String> {
		if self.dirty {
			self.write()?;
			self.dirty = false;
		}
		Ok(())
	}

	fn unique_stem(&mut self, folder: &[String], name: &str) -> String {
		let mut dir = String::from(MODELS_DIR);
		for part in folder {
			dir.push('/');
			dir.push_str(&sanitize(part));
		}
		let base = sanitize(name);
		let mut stem = format!("{dir}/{base}");
		let mut n = 2;
		while self.stems.contains(&stem.to_lowercase()) {
			stem = format!("{dir}/{base} ({n})");
			n += 1;
		}
		self.stems.insert(stem.to_lowercase());
		stem
	}

	/// Adds export entries; each entry's `maps` should already be filtered
	/// to the maps selected for import. Entries already in the workspace
	/// (same folder, name and mesh) gain any newly selected maps instead of
	/// being duplicated, so re-importing an export is safe. Returns how
	/// many new items were added.
	pub fn add(&mut self, entries: impl IntoIterator<Item = Entry>) -> usize {
		let mut added = 0;
		for entry in entries {
			let maps = entry.maps.iter().map(|(&kind, &id)| {
				(
					kind,
					MapRef {
						id,
						state: AssetState::Pending,
					},
				)
			});
			let existing = self.manifest.items.iter_mut().find(|i| {
				i.folder == entry.folder && i.name == entry.name && i.mesh_id == entry.mesh_id
			});
			if let Some(item) = existing {
				for (kind, map) in maps {
					if !item.maps.contains_key(&kind) {
						item.maps.insert(kind, map);
						self.dirty = true;
					}
				}
				continue;
			}
			let stem = self.unique_stem(&entry.folder, &entry.name);
			let maps = maps.collect();
			self.manifest.items.push(Item {
				name: entry.name,
				folder: entry.folder,
				mesh_id: entry.mesh_id,
				stem,
				mesh: AssetState::Pending,
				maps,
				texture_id: None,
				texture: None,
			});
			added += 1;
			self.dirty = true;
		}
		self.generation += 1;
		added
	}

	/// Pending downloads grouped by asset id, so each asset is fetched once.
	pub fn pending_jobs(&self) -> Vec<Job> {
		let mut jobs: Vec<Job> = Vec::new();
		let mut index = HashMap::new();
		let mut push = |asset_id: u64, stem: &String, kind: AssetKind| {
			let i = *index.entry(asset_id).or_insert_with(|| {
				jobs.push(Job {
					asset_id,
					targets: Vec::new(),
				});
				jobs.len() - 1
			});
			jobs[i].targets.push((stem.clone(), kind));
		};
		for item in &self.manifest.items {
			if item.mesh == AssetState::Pending {
				push(item.mesh_id, &item.stem, AssetKind::Mesh);
			}
			for (&kind, map) in &item.maps {
				if map.state == AssetState::Pending {
					push(map.id, &item.stem, AssetKind::Map(kind));
				}
			}
		}
		jobs
	}

	/// Updates an item's download state. Items removed while their
	/// download was running are ignored.
	pub fn set_state(&mut self, stem: &str, kind: AssetKind, state: AssetState) {
		let Some(item) = self.manifest.items.iter_mut().find(|i| i.stem == stem) else {
			return;
		};
		match kind {
			AssetKind::Mesh => item.mesh = state,
			AssetKind::Map(k) => match item.maps.get_mut(&k) {
				Some(map) => map.state = state,
				None => return,
			},
		}
		self.dirty = true;
	}

	/// Marks failed downloads as pending so they are retried.
	pub fn retry(&mut self, items: impl IntoIterator<Item = usize>) {
		let retry = |state: &mut AssetState| {
			if matches!(state, AssetState::Failed { .. }) {
				*state = AssetState::Pending;
				true
			} else {
				false
			}
		};
		for i in items {
			let Some(item) = self.manifest.items.get_mut(i) else {
				continue;
			};
			let mut changed = retry(&mut item.mesh);
			for map in item.maps.values_mut() {
				changed |= retry(&mut map.state);
			}
			self.dirty |= changed;
		}
	}

	/// Removes items from the workspace and deletes their organized files.
	/// Cached downloads are kept, so re-importing is instant.
	pub fn remove(&mut self, items: &[usize]) {
		let mut items = items.to_vec();
		items.sort_unstable();
		items.dedup();
		for &i in items.iter().rev() {
			if i >= self.manifest.items.len() {
				continue;
			}
			let item = self.manifest.items.remove(i);
			for state in item.states() {
				if let AssetState::Done { path } = state {
					let _ = std::fs::remove_file(self.root.join(path));
				}
			}
			self.stems.remove(&item.stem.to_lowercase());
		}
		self.dirty = true;
		self.generation += 1;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn entry(folder: &str, name: &str, mesh: u64, maps: &[(MapKind, u64)]) -> Entry {
		Entry {
			name: name.into(),
			folder: folder.split('/').filter(|s| !s.is_empty()).map(String::from).collect(),
			mesh_id: mesh,
			maps: maps.iter().copied().collect(),
		}
	}

	fn temp_dir(tag: &str) -> PathBuf {
		let dir = std::env::temp_dir().join(format!("rbx_ws_{tag}_{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		dir
	}

	#[test]
	fn sanitizes_names() {
		assert_eq!(sanitize("8: CrystalScorpion"), "8_ CrystalScorpion");
		assert_eq!(sanitize("Demon Egg.013"), "Demon Egg.013");
		assert_eq!(sanitize("trailing. "), "trailing");
		assert_eq!(sanitize("CON"), "CON_");
		assert_eq!(sanitize("con.txt"), "con.txt_");
		assert_eq!(sanitize("a/b\\c"), "a_b_c");
		assert_eq!(sanitize("   "), "_");
	}

	#[test]
	fn organizes_and_dedupes() {
		use MapKind::*;
		let dir = temp_dir("organize");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let added = ws.add([
			entry("Zone_10/8: Scorpion", "Body", 1, &[]),
			// TextureID and ColorMap are the same image, as exporters often do
			entry("Zone_10/8: Scorpion", "Body", 2, &[(Texture, 9), (Color, 9), (Normal, 10)]),
			entry("Zone_10/8: Scorpion", "body", 3, &[]),
			entry("", "Root", 1, &[]),
		]);
		assert_eq!(added, 4);
		let stems: Vec<_> = ws.items().iter().map(|i| i.stem.as_str()).collect();
		assert_eq!(
			stems,
			[
				"Models/Zone_10/8_ Scorpion/Body",
				"Models/Zone_10/8_ Scorpion/Body (2)",
				"Models/Zone_10/8_ Scorpion/body (3)",
				"Models/Root",
			]
		);

		let jobs = ws.pending_jobs();
		// meshes 1, 2, 3 and images 9, 10: each fetched once
		assert_eq!(jobs.len(), 5);
		let mesh1 = jobs.iter().find(|j| j.asset_id == 1).unwrap();
		assert_eq!(mesh1.targets.len(), 2);
		let image9 = jobs.iter().find(|j| j.asset_id == 9).unwrap();
		assert_eq!(
			image9.targets,
			[
				("Models/Zone_10/8_ Scorpion/Body (2)".to_string(), AssetKind::Map(Texture)),
				("Models/Zone_10/8_ Scorpion/Body (2)".to_string(), AssetKind::Map(Color)),
			]
		);

		// re-importing adds newly selected maps only
		assert_eq!(ws.add([entry("Zone_10/8: Scorpion", "body", 3, &[(Roughness, 11)])]), 0);
		assert!(ws.items()[2].maps.contains_key(&Roughness));
		assert!(ws.items()[2].has_pending());

		ws.save_if_dirty().unwrap();
		let reopened = Workspace::open(&dir).unwrap();
		assert_eq!(reopened.items().len(), 4);
		assert_eq!(reopened.items()[1].maps.len(), 3);
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[test]
	fn migrates_version_1() {
		let dir = temp_dir("migrate");
		std::fs::create_dir_all(dir.join("Models")).unwrap();
		std::fs::write(dir.join("Models/A.mesh"), b"version 2.00\n").unwrap();
		std::fs::write(dir.join("Models/A.png"), b"png").unwrap();
		std::fs::write(
			dir.join(MANIFEST),
			r#"{"version":1,"name":"Old","items":[
				{"name":"A","folder":[],"mesh_id":1,"texture_id":5,"stem":"Models/A",
				 "mesh":{"state":"done","path":"Models/A.mesh"},
				 "texture":{"state":"done","path":"Models/A.png"}},
				{"name":"B","folder":[],"mesh_id":2,"texture_id":null,"stem":"Models/B",
				 "mesh":{"state":"done","path":"Models/B.mesh"},"texture":null}
			]}"#,
		)
		.unwrap();
		let mut ws = Workspace::open(&dir).unwrap();
		let a = &ws.items()[0];
		assert_eq!(
			a.maps[&MapKind::Texture],
			MapRef {
				id: 5,
				state: AssetState::Done {
					path: "Models/A.png".into()
				}
			}
		);
		assert!(a.is_done());
		// B's mesh file is missing, so it is queued again
		assert_eq!(ws.items()[1].mesh, AssetState::Pending);
		ws.save_if_dirty().unwrap();
		let saved = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
		assert!(saved.contains("\"version\": 2"));
		assert!(!saved.contains("texture_id"));
		std::fs::remove_dir_all(&dir).unwrap();
	}
}
