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
//!   Rigs/<Path>.rig.json           animated models: an export holding one model (see `write_rig`)
//!   .cache/<asset id>              one raw copy per asset id, so duplicates download once
//!   .cache/thumbnails/<hash>.png   the library's pictures of models (see `thumbnails`)
//!   .cache/facts.json              what the library filters on, per mesh (see `facts`)
//! ```

pub mod download;
pub mod facts;
pub mod view_loader;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::anim::schema::AnimatedModel;
use crate::content_id::parse_asset_id;
use crate::scene::maps::MapKind;
use crate::studio::export_file::{self, Entry};

pub const MANIFEST: &str = "workspace.json";
const MODELS_DIR: &str = "Models";
const RIGS_DIR: &str = "Rigs";
pub const CACHE_DIR: &str = ".cache";
/// Images models use directly (decals), one file per asset.
const IMAGES_DIR: &str = "Images";
/// Manifest format version: 1 had a single texture per item, 2 added
/// `maps`, 3 added animated models. tools/make_test_workspace.py writes
/// this version too; update it with this.
const MANIFEST_VERSION: u32 = 3;

/// Where an image a model uses is stored, without its extension.
fn image_stem(asset_id: u64) -> String {
	format!("{IMAGES_DIR}/{asset_id}")
}

/// Where the raw download of an asset is kept, shared by every item using it.
pub fn cache_path(root: &Path, asset_id: u64) -> PathBuf {
	root.join(CACHE_DIR).join(asset_id.to_string())
}

fn rig_stem(file: &str) -> &str {
	file.strip_suffix(".rig.json").unwrap_or(file)
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
		"CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
		"COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
	];
	let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
	if RESERVED.contains(&base.as_str()) {
		format!("{s}_")
	} else {
		s
	}
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AssetState {
	Pending,
	/// `path` is relative to the workspace root.
	Done {
		path: String,
	},
	Failed {
		error: String,
	},
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
	Mesh,
	Map(MapKind),
	/// An image a model uses directly (a decal); its stem is [`image_stem`].
	Image,
}

impl AssetKind {
	pub fn is_image(self) -> bool {
		matches!(self, AssetKind::Map(_) | AssetKind::Image)
	}
	pub fn file_suffix(self) -> &'static str {
		match self {
			AssetKind::Mesh | AssetKind::Image => "",
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
		self.states()
			.any(|s| matches!(s, AssetState::Failed { .. }))
	}
	pub fn has_pending(&self) -> bool {
		self.states().any(|s| *s == AssetState::Pending)
	}
	/// Path of a downloaded file, relative to the workspace root.
	pub fn done_path(&self, kind: AssetKind) -> Option<&str> {
		let state = match kind {
			AssetKind::Mesh => &self.mesh,
			AssetKind::Map(k) => &self.maps.get(&k)?.state,
			// images belong to models, not items
			AssetKind::Image => return None,
		};
		match state {
			AssetState::Done { path } => Some(path.as_str()),
			_ => None,
		}
	}
}

/// An animated model (rig and clips) from an export. Its parts use meshes
/// stored as ordinary items.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelItem {
	pub name: String,
	/// Roblox instance path; identifies the model when an export is re-imported.
	pub path: String,
	pub folder: Vec<String>,
	/// The stored model record, relative to the workspace root.
	pub file: String,
	/// `Parts[].MeshRef` to the stem of the item holding that mesh.
	pub meshes: BTreeMap<u32, String>,
	pub parts: usize,
	pub clips: usize,
	pub playable_clips: usize,
	/// Images the model's parts use directly (decals), by asset id, stored
	/// once each under `Images/`.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub images: BTreeMap<u64, AssetState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
	pub version: u32,
	pub name: String,
	pub items: Vec<Item>,
	#[serde(default)]
	pub models: Vec<ModelItem>,
}

pub struct Workspace {
	pub root: PathBuf,
	manifest: Manifest,
	/// Lower-cased stems in use, since Windows paths are case-insensitive.
	stems: HashSet<String>,
	dirty: bool,
	/// Bumped whenever items are added or removed, so views can cache.
	generation: u64,
}

/// Writes a model as an export in the current schema
/// ([`export_file::SCHEMA_VERSION`]) holding just that model and the meshes
/// it uses, so the file can also be imported on its own. Workspaces made
/// before schema 2 hold schema 1 rig files; the export reader still accepts
/// those, so they load as they are.
fn write_rig(
	path: &Path,
	model: &AnimatedModel,
	meshes: &[(&Entry, String)],
) -> Result<(), String> {
	let meshes: Vec<serde_json::Value> = meshes
		.iter()
		.filter_map(|(e, _)| {
			let mut asset = serde_json::Map::new();
			asset.insert("Id".into(), e.source_id?.into());
			asset.insert("Name".into(), e.name.clone().into());
			asset.insert(
				"MeshId".into(),
				format!("rbxassetid://{}", e.mesh_id).into(),
			);
			asset.insert("Folder".into(), e.folder.join("/").into());
			for kind in MapKind::ALL {
				let value = e
					.maps
					.get(&kind)
					.map(|id| format!("rbxassetid://{id}"))
					.unwrap_or_default();
				asset.insert(kind.export_field().into(), value.into());
			}
			Some(asset.into())
		})
		.collect();
	let doc = serde_json::json!({
		"SchemaVersion": export_file::SCHEMA_VERSION,
		"Meshes": meshes,
		"Models": [model],
	});
	if let Some(dir) = path.parent() {
		std::fs::create_dir_all(dir)
			.map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
	}
	let json = serde_json::to_string(&doc).map_err(|e| e.to_string())?;
	std::fs::write(path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))
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
				models: Vec::new(),
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
			let missing = |state: &AssetState| matches!(state, AssetState::Done { path } if !root.join(path).exists());
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
		let stems = manifest
			.items
			.iter()
			.map(|i| i.stem.to_lowercase())
			.chain(
				manifest
					.models
					.iter()
					.map(|m| rig_stem(&m.file).to_lowercase()),
			)
			.collect();
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

	pub fn models(&self) -> &[ModelItem] {
		&self.manifest.models
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

	fn unique_stem(&mut self, top: &str, folder: &[String], name: &str) -> String {
		let mut dir = String::from(top);
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

	/// Test helper: [`Self::add_entry`] for each entry, then one extra
	/// generation bump even if nothing was added. Returns how many new items
	/// were added.
	#[cfg(test)]
	pub(crate) fn add(&mut self, entries: impl IntoIterator<Item = Entry>) -> usize {
		let added = entries.into_iter().filter(|e| self.add_entry(e).1).count();
		self.generation += 1;
		added
	}

	/// Test helper: replaces the items and models outright, without the
	/// stem or generation bookkeeping, for fixtures that need exact contents.
	#[cfg(test)]
	pub(crate) fn set_contents(&mut self, items: Vec<Item>, models: Vec<ModelItem>) {
		self.manifest.items = items;
		self.manifest.models = models;
	}

	/// Adds one export entry; its `maps` should already be filtered to the
	/// maps selected for import. An entry matching an item already in the
	/// workspace (same folder, name and mesh, and no map ID that differs
	/// from the item's) gives that item any newly selected maps instead of
	/// being duplicated, so re-importing an export is safe. An entry whose
	/// maps conflict with the item's is a separate item. Returns the stem of
	/// its item and whether the item is new.
	pub fn add_entry(&mut self, entry: &Entry) -> (String, bool) {
		let maps = entry.maps.iter().map(|(&kind, &id)| {
			(
				kind,
				MapRef {
					id,
					state: AssetState::Pending,
				},
			)
		});
		// the same mesh with a different image is a different item
		let existing = self.manifest.items.iter_mut().find(|i| {
			i.folder == entry.folder
				&& i.name == entry.name
				&& i.mesh_id == entry.mesh_id
				&& i.maps
					.iter()
					.all(|(k, m)| entry.maps.get(k).is_none_or(|&id| id == m.id))
		});
		if let Some(item) = existing {
			for (kind, map) in maps {
				if let std::collections::btree_map::Entry::Vacant(slot) = item.maps.entry(kind) {
					slot.insert(map);
					self.dirty = true;
				}
			}
			return (item.stem.clone(), false);
		}
		let stem = self.unique_stem(MODELS_DIR, &entry.folder, &entry.name);
		let maps = maps.collect();
		self.manifest.items.push(Item {
			name: entry.name.clone(),
			folder: entry.folder.clone(),
			mesh_id: entry.mesh_id,
			stem: stem.clone(),
			mesh: AssetState::Pending,
			maps,
			texture_id: None,
			texture: None,
		});
		self.dirty = true;
		self.generation += 1;
		(stem, true)
	}

	/// Stores an animated model. `meshes` are the entries its parts use, each
	/// with the stem of the item holding it (from [`Self::add_entry`]). A model
	/// with the same instance path is replaced. Returns whether it is new.
	pub fn add_model(
		&mut self,
		model: &AnimatedModel,
		meshes: &[(&Entry, String)],
	) -> Result<bool, String> {
		let existing = self
			.manifest
			.models
			.iter()
			.position(|m| m.path == model.path);
		let mut folder: Vec<String> = model
			.path
			.split('/')
			.filter(|s| !s.is_empty())
			.map(String::from)
			.collect();
		folder.pop();
		let file = match existing {
			Some(i) => self.manifest.models[i].file.clone(),
			None => format!(
				"{}.rig.json",
				self.unique_stem(RIGS_DIR, &folder, &model.name)
			),
		};
		write_rig(&self.root.join(&file), model, meshes)?;
		// decal images: kept as they were when re-imported, otherwise to download
		let previous = existing
			.map(|i| self.manifest.models[i].images.clone())
			.unwrap_or_default();
		let images = model
			.parts
			.iter()
			.flat_map(|p| {
				let variant = p
					.material_variant
					.iter()
					.flat_map(|v| v.maps().map(|(_, id)| id));
				p.decals.iter().map(|d| d.texture.as_str()).chain(variant)
			})
			.chain(model.clothing.iter().flat_map(|c| c.images()))
			.filter_map(parse_asset_id)
			.map(|id| {
				let state = previous.get(&id).cloned().unwrap_or(AssetState::Pending);
				(id, state)
			})
			.collect();
		let item = ModelItem {
			name: model.name.clone(),
			path: model.path.clone(),
			folder,
			file,
			meshes: meshes
				.iter()
				.filter_map(|(e, stem)| Some((e.source_id?, stem.clone())))
				.collect(),
			parts: model.parts.len(),
			clips: model.animations.len(),
			playable_clips: model
				.animations
				.iter()
				.filter(|a| a.status.is_playable())
				.count(),
			images,
		};
		self.dirty = true;
		self.generation += 1;
		match existing {
			Some(i) => {
				self.manifest.models[i] = item;
				Ok(false)
			}
			None => {
				self.manifest.models.push(item);
				Ok(true)
			}
		}
	}

	/// Removes animated models and their rig files. The meshes they use stay.
	pub fn remove_models(&mut self, models: &[usize]) {
		let mut models = models.to_vec();
		models.sort_unstable();
		models.dedup();
		for &i in models.iter().rev() {
			if i >= self.manifest.models.len() {
				continue;
			}
			let model = self.manifest.models.remove(i);
			let _ = std::fs::remove_file(self.root.join(&model.file));
			self.stems.remove(&rig_stem(&model.file).to_lowercase());
		}
		self.dirty = true;
		self.generation += 1;
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
		let mut images: Vec<u64> = self
			.manifest
			.models
			.iter()
			.flat_map(|m| &m.images)
			.filter(|(_, state)| **state == AssetState::Pending)
			.map(|(&id, _)| id)
			.collect();
		images.sort_unstable();
		images.dedup();
		for id in images {
			push(id, &image_stem(id), AssetKind::Image);
		}
		jobs
	}

	/// Updates an item's download state. Items removed while their
	/// download was running are ignored.
	pub fn set_state(&mut self, stem: &str, kind: AssetKind, state: AssetState) {
		if kind == AssetKind::Image {
			// every model using the image
			let id = stem.rsplit('/').next().and_then(|s| s.parse::<u64>().ok());
			for model in &mut self.manifest.models {
				if let Some(slot) = id.and_then(|id| model.images.get_mut(&id)) {
					*slot = state.clone();
					self.dirty = true;
				}
			}
			return;
		}
		let Some(item) = self.manifest.items.iter_mut().find(|i| i.stem == stem) else {
			return;
		};
		match kind {
			AssetKind::Mesh => item.mesh = state,
			AssetKind::Map(k) => match item.maps.get_mut(&k) {
				Some(map) => map.state = state,
				None => return,
			},
			AssetKind::Image => return,
		}
		self.dirty = true;
	}

	/// Records a finished download: the files it wrote become `Done`, or
	/// every target of the job becomes `Failed` with the error.
	pub fn record_download(&mut self, job: &Job, result: Result<Vec<download::Stored>, String>) {
		match result {
			Ok(written) => {
				for w in written {
					self.set_state(&w.stem, w.kind, AssetState::Done { path: w.path });
				}
			}
			Err(error) => {
				for (stem, kind) in &job.targets {
					self.set_state(
						stem,
						*kind,
						AssetState::Failed {
							error: error.clone(),
						},
					);
				}
			}
		}
	}

	/// Marks every failed decal image as pending so it is retried.
	pub fn retry_images(&mut self) {
		for model in &mut self.manifest.models {
			for state in model.images.values_mut() {
				if matches!(state, AssetState::Failed { .. }) {
					*state = AssetState::Pending;
					self.dirty = true;
				}
			}
		}
	}

	/// (pending, failed) decal images, each asset counted once.
	pub fn image_counts(&self) -> (usize, usize) {
		let mut states: BTreeMap<u64, &AssetState> = BTreeMap::new();
		for (id, state) in self.manifest.models.iter().flat_map(|m| &m.images) {
			states.insert(*id, state);
		}
		(
			states
				.values()
				.filter(|s| ***s == AssetState::Pending)
				.count(),
			states
				.values()
				.filter(|s| matches!(***s, AssetState::Failed { .. }))
				.count(),
		)
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
	use crate::test_support::TempDir;

	fn entry(folder: &str, name: &str, mesh: u64, maps: &[(MapKind, u64)]) -> Entry {
		Entry {
			name: name.into(),
			folder: folder
				.split('/')
				.filter(|s| !s.is_empty())
				.map(String::from)
				.collect(),
			mesh_id: mesh,
			maps: maps.iter().copied().collect(),
			source_id: None,
		}
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
		let dir = TempDir::new("ws_organize");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let added = ws.add([
			entry("Zone_10/8: Scorpion", "Body", 1, &[]),
			// TextureID and ColorMap are the same image, as exporters often do
			entry(
				"Zone_10/8: Scorpion",
				"Body",
				2,
				&[(Texture, 9), (Color, 9), (Normal, 10)],
			),
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
				(
					"Models/Zone_10/8_ Scorpion/Body (2)".to_string(),
					AssetKind::Map(Texture)
				),
				(
					"Models/Zone_10/8_ Scorpion/Body (2)".to_string(),
					AssetKind::Map(Color)
				),
			]
		);

		// re-importing adds newly selected maps only
		assert_eq!(
			ws.add([entry("Zone_10/8: Scorpion", "body", 3, &[(Roughness, 11)])]),
			0
		);
		assert!(ws.items()[2].maps.contains_key(&Roughness));
		assert!(ws.items()[2].has_pending());

		ws.save_if_dirty().unwrap();
		let reopened = Workspace::open(&dir).unwrap();
		assert_eq!(reopened.items().len(), 4);
		assert_eq!(reopened.items()[1].maps.len(), 3);
	}

	#[test]
	fn stores_animated_models() {
		use crate::anim::schema::{AnimationClip, ClipStatus, ModelPart};
		let dir = TempDir::new("ws_models");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let mut torso = entry("Workspace/Wolf", "Torso", 10, &[(MapKind::Color, 11)]);
		torso.source_id = Some(1);
		let (stem, new) = ws.add_entry(&torso);
		assert!(new);
		let model = AnimatedModel {
			name: "Wolf".into(),
			path: "Workspace/Animals/Wolf".into(),
			parts: vec![ModelPart {
				name: "Torso".into(),
				mesh_ref: 1,
				..Default::default()
			}],
			animations: vec![
				AnimationClip {
					status: ClipStatus::KeyframeSequence,
					..Default::default()
				},
				AnimationClip::default(),
			],
			..Default::default()
		};
		assert!(ws.add_model(&model, &[(&torso, stem.clone())]).unwrap());
		// re-importing replaces it in place
		assert!(!ws.add_model(&model, &[(&torso, stem.clone())]).unwrap());
		let m = ws.models()[0].clone();
		assert_eq!(m.file, "Rigs/Workspace/Animals/Wolf.rig.json");
		assert_eq!(m.folder, ["Workspace", "Animals"]);
		assert_eq!(m.meshes[&1], stem);
		assert_eq!((m.parts, m.clips, m.playable_clips), (1, 2, 1));

		ws.save_if_dirty().unwrap();
		let reopened = Workspace::open(&dir).unwrap();
		assert_eq!(reopened.models().len(), 1);
		// the rig file is an export of its own
		let file = std::fs::read_to_string(dir.join(&m.file)).unwrap();
		let export = export_file::parse(&file).unwrap();
		assert_eq!(export.models[0].parts[0].mesh_ref, 1);
		assert_eq!(export.entries[0].maps, torso.maps);

		let mut ws = reopened;
		ws.remove_models(&[0]);
		assert!(ws.models().is_empty());
		assert!(!dir.join("Rigs/Workspace/Animals/Wolf.rig.json").exists());
	}

	/// Decal images are downloaded once per asset into `Images/`, for every
	/// model using them, and kept when the model is imported again.
	#[test]
	fn decal_images_are_downloaded_for_models() {
		use crate::anim::schema::{DecalInfo, ModelPart};
		let dir = TempDir::new("ws_decals");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let decal = |texture: &str| DecalInfo {
			class_name: "Decal".into(),
			texture: texture.into(),
			face: "Front".into(),
			..Default::default()
		};
		let model = |name: &str| AnimatedModel {
			name: name.into(),
			path: format!("Workspace/{name}"),
			parts: vec![ModelPart {
				name: "Sign".into(),
				decals: vec![decal("rbxassetid://77"), decal("rbxassetid://88")],
				..Default::default()
			}],
			..Default::default()
		};
		ws.add_model(&model("A"), &[]).unwrap();
		ws.add_model(&model("B"), &[]).unwrap();
		// one job per image, for both models
		let jobs = ws.pending_jobs();
		assert_eq!(jobs.len(), 2);
		assert!(jobs.iter().all(|j| j.wants_image()));
		assert_eq!(
			jobs[0].targets,
			[("Images/77".to_string(), AssetKind::Image)]
		);
		assert_eq!(ws.image_counts(), (2, 0));
		let done = AssetState::Done {
			path: "Images/77.png".into(),
		};
		ws.set_state("Images/77", AssetKind::Image, done.clone());
		ws.set_state(
			"Images/88",
			AssetKind::Image,
			AssetState::Failed {
				error: "gone".into(),
			},
		);
		assert!(ws.models().iter().all(|m| m.images[&77] == done));
		assert_eq!(ws.image_counts(), (0, 1));
		ws.retry_images();
		assert_eq!(ws.image_counts(), (1, 0));
		// importing again keeps what was downloaded
		ws.add_model(&model("A"), &[]).unwrap();
		assert_eq!(ws.models()[0].images[&77], done);
		// a MaterialVariant's maps are downloaded the same way
		let mut tiled = model("C");
		tiled.parts[0].material_variant = Some(crate::anim::schema::MaterialVariantInfo {
			name: "Bricks".into(),
			studs_per_tile: 4.0,
			color_map: "rbxassetid://99".into(),
			normal_map: "rbxassetid://100".into(),
			..Default::default()
		});
		ws.add_model(&tiled, &[]).unwrap();
		let images = &ws.models()[2].images;
		assert!(images.contains_key(&99) && images.contains_key(&100) && images.contains_key(&77));
	}

	#[test]
	fn same_mesh_with_other_images_is_separate() {
		let dir = TempDir::new("ws_variants");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let a = entry("F", "Handle", 1, &[(MapKind::Texture, 2)]);
		let b = entry("F", "Handle", 1, &[(MapKind::Texture, 3)]);
		assert_eq!(ws.add([a.clone(), b, a]), 2);
	}

	#[test]
	fn migrates_version_1() {
		let dir = TempDir::new("ws_migrate");
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
		assert!(saved.contains("\"version\": 3"));
		assert!(!saved.contains("texture_id"));
	}
}
