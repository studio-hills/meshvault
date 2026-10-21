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
mod manifest;
mod paths;
mod rig_file;
mod store;
pub mod view_loader;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::anim::schema::AnimatedModel;
use crate::content_id::parse_asset_id;
use crate::scene::maps::MapKind;
use crate::studio::export_file::Entry;

pub use self::manifest::{AssetState, Item, MapRef, ModelItem, ModelVersion};
use self::manifest::{MANIFEST_VERSION, Manifest};
pub use self::paths::{CACHE_DIR, MANIFEST, cache_path, sanitize};
use self::paths::{MODELS_DIR, RIGS_DIR, image_id, image_stem, rig_stem};
use self::rig_file::write_rig;

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

pub struct Workspace {
	pub root: PathBuf,
	manifest: Manifest,
	/// Lower-cased stems in use, since Windows paths are case-insensitive.
	stems: HashSet<String>,
	dirty: bool,
	/// Bumped whenever items are added or removed, so views can cache.
	generation: u64,
}

/// Every image a model uses directly, by asset id: its parts' decals and
/// MaterialVariant maps, then its clothing. An id can appear more than once.
fn image_ids(model: &AnimatedModel) -> impl Iterator<Item = u64> + '_ {
	model
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
		let dirty = manifest.upgrade(root);
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
		// an item's maps are written as stem + suffix, so `A`'s normal map and
		// the texture of `A.normal` would be the same file: a stem is only
		// free if none of its file names is another stem's
		let suffixes: Vec<&str> = if top == MODELS_DIR {
			MapKind::ALL.iter().map(|k| k.file_suffix()).collect()
		} else {
			vec![""]
		};
		let taken = |stem: &str| {
			let stem = stem.to_lowercase();
			suffixes.iter().any(|mine| {
				let file = format!("{stem}{mine}");
				suffixes.iter().any(|theirs| {
					file.strip_suffix(theirs)
						.is_some_and(|other| self.stems.contains(other))
				})
			})
		};
		let mut stem = format!("{dir}/{base}");
		let mut n = 2;
		while taken(&stem) {
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
		self.manifest.items.push(Item::new(
			entry.name.clone(),
			entry.folder.clone(),
			entry.mesh_id,
			stem.clone(),
			maps,
		));
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
		if let Err(e) = write_rig(&self.root.join(&file), model, meshes) {
			// a new model that could not be written frees the stem it took
			if existing.is_none() {
				self.stems.remove(&rig_stem(&file).to_lowercase());
			}
			return Err(e);
		}
		// decal images: kept as they were when re-imported, otherwise to download
		let previous = existing
			.map(|i| self.manifest.models[i].images.clone())
			.unwrap_or_default();
		let version = existing.map_or_else(ModelVersion::new, |i| {
			self.manifest.models[i].version.reimported()
		});
		let images = image_ids(model)
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
			version,
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
	/// A model whose rig file cannot be deleted stays, and the error says so.
	pub fn remove_models(&mut self, models: &[usize]) -> Result<(), String> {
		let mut models = models.to_vec();
		models.sort_unstable();
		models.dedup();
		let mut errors = Vec::new();
		for &i in models.iter().rev() {
			let Some(model) = self.manifest.models.get(i) else {
				continue;
			};
			if let Err(e) = delete_file(&self.root, &model.file) {
				errors.push(e);
				continue;
			}
			let model = self.manifest.models.remove(i);
			self.stems.remove(&rig_stem(&model.file).to_lowercase());
		}
		self.dirty = true;
		self.generation += 1;
		deletion_result(errors)
	}

	/// The first item with `stem`, as (index, name).
	pub fn item_with_stem(&self, stem: &str) -> Option<(usize, String)> {
		let items = &self.manifest.items;
		items
			.iter()
			.position(|i| i.stem == stem)
			.map(|i| (i, items[i].name.clone()))
	}

	/// The items holding an animated model's meshes, as (index, name): each
	/// stem once, in stem order, skipping stems no item has. Empty when
	/// there is no model `model`.
	pub fn meshes_of_model(&self, model: usize) -> Vec<(usize, String)> {
		let Some(model) = self.manifest.models.get(model) else {
			return Vec::new();
		};
		let mut stems: Vec<&String> = model.meshes.values().collect();
		stems.sort();
		stems.dedup();
		stems
			.into_iter()
			.filter_map(|stem| self.item_with_stem(stem))
			.collect()
	}

	/// The animated models using the mesh with `stem`, as (index, name), in
	/// workspace order.
	pub fn models_using(&self, stem: &str) -> Vec<(usize, String)> {
		self.manifest
			.models
			.iter()
			.enumerate()
			.filter(|(_, m)| m.meshes.values().any(|s| s == stem))
			.map(|(i, m)| (i, m.name.clone()))
			.collect()
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
			let id = image_id(stem);
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

	/// Records a finished download: the files it wrote are moved into place
	/// and become `Done`, or every target of the job becomes `Failed` with
	/// the error. Targets whose item no longer uses the job's asset (it was
	/// removed while the download ran, and its stem perhaps reused) are
	/// left alone, and their files deleted.
	pub fn record_download(&mut self, job: &Job, result: Result<Vec<store::Stored>, String>) {
		match result {
			Ok(written) => {
				for w in written {
					if !self.uses(&w.stem, w.kind, job.asset_id) {
						w.discard();
						continue;
					}
					let state = match w.publish(&self.root) {
						Ok(()) => AssetState::Done { path: w.path },
						Err(error) => AssetState::Failed { error },
					};
					self.set_state(&w.stem, w.kind, state);
				}
			}
			Err(error) => {
				for (stem, kind) in &job.targets {
					if !self.uses(stem, *kind, job.asset_id) {
						continue;
					}
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

	/// Whether the item with `stem` (for an image, any model) still uses
	/// asset `asset_id` as `kind`.
	fn uses(&self, stem: &str, kind: AssetKind, asset_id: u64) -> bool {
		let items = &self.manifest.items;
		match kind {
			AssetKind::Mesh => items
				.iter()
				.any(|i| i.stem == stem && i.mesh_id == asset_id),
			AssetKind::Map(k) => items
				.iter()
				.any(|i| i.stem == stem && i.maps.get(&k).is_some_and(|m| m.id == asset_id)),
			AssetKind::Image => {
				image_id(stem) == Some(asset_id)
					&& self
						.manifest
						.models
						.iter()
						.any(|m| m.images.contains_key(&asset_id))
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
	/// Cached downloads are kept, so re-importing is instant. An item with
	/// a file that cannot be deleted stays (and keeps its stem), with the
	/// files that were deleted to download again, and the error says so.
	pub fn remove(&mut self, items: &[usize]) -> Result<(), String> {
		let mut items = items.to_vec();
		items.sort_unstable();
		items.dedup();
		let mut errors = Vec::new();
		for &i in items.iter().rev() {
			let Some(item) = self.manifest.items.get_mut(i) else {
				continue;
			};
			let mut kept = false;
			let states =
				std::iter::once(&mut item.mesh).chain(item.maps.values_mut().map(|m| &mut m.state));
			for state in states {
				let AssetState::Done { path } = state else {
					continue;
				};
				match delete_file(&self.root, path) {
					Ok(()) => *state = AssetState::Pending,
					Err(e) => {
						errors.push(e);
						kept = true;
					}
				}
			}
			if !kept {
				let item = self.manifest.items.remove(i);
				self.stems.remove(&item.stem.to_lowercase());
			}
		}
		self.dirty = true;
		self.generation += 1;
		deletion_result(errors)
	}
}

/// Deletes a file in the workspace. One that is already gone counts as
/// deleted.
fn delete_file(root: &Path, path: &str) -> Result<(), String> {
	match std::fs::remove_file(root.join(path)) {
		Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
			Err(format!("Could not delete {path}: {e}"))
		}
		_ => Ok(()),
	}
}

/// The first error of a removal, saying how many more there were.
fn deletion_result(errors: Vec<String>) -> Result<(), String> {
	match errors.as_slice() {
		[] => Ok(()),
		[one] => Err(one.clone()),
		[first, rest @ ..] => Err(format!("{first} (and {} more)", rest.len())),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::studio::export_file;
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

	/// An item named `name` with `stem`.
	fn item_at(stem: &str, name: &str) -> Item {
		serde_json::from_value(serde_json::json!({
			"name": name,
			"folder": [],
			"mesh_id": 1,
			"stem": stem,
			"mesh": AssetState::Pending,
		}))
		.unwrap()
	}

	/// A model named `name` whose parts use `stems`, by mesh ref in order.
	fn model_using(name: &str, stems: &[&str]) -> ModelItem {
		ModelItem {
			name: name.into(),
			path: format!("Workspace/{name}"),
			folder: Vec::new(),
			file: format!("Rigs/{name}.rig.json"),
			meshes: (0..).zip(stems.iter().map(|s| s.to_string())).collect(),
			parts: stems.len(),
			clips: 0,
			playable_clips: 0,
			images: BTreeMap::new(),
			version: Default::default(),
		}
	}

	#[test]
	fn a_models_meshes_are_its_stems_once_each_in_stem_order() {
		let dir = TempDir::new("ws_meshes_of_model");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.set_contents(
			vec![
				item_at("Models/Tail", "Tail"),
				item_at("Models/Body", "Body"),
				item_at("Models/Body", "Body again"),
			],
			vec![model_using(
				"Shark",
				&["Models/Tail", "Models/Body", "Models/Tail", "Models/Gone"],
			)],
		);
		// sorted by stem, the repeated tail once, the first item of a stem,
		// and a stem no item has left out
		assert_eq!(
			ws.meshes_of_model(0),
			[(1, "Body".to_string()), (0, "Tail".to_string())]
		);
		assert!(ws.meshes_of_model(1).is_empty());
	}

	#[test]
	fn the_models_using_a_stem_are_in_workspace_order() {
		let dir = TempDir::new("ws_models_using");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.set_contents(
			vec![item_at("Models/Fin", "Fin")],
			vec![
				model_using("Wolf", &["Models/Leg"]),
				model_using("Shark", &["Models/Fin", "Models/Fin"]),
				model_using("Fish", &["Models/Fin"]),
			],
		);
		assert_eq!(
			ws.models_using("Models/Fin"),
			[(1, "Shark".to_string()), (2, "Fish".to_string())]
		);
		// a stem no item has still finds the models that use it
		assert_eq!(ws.models_using("Models/Leg"), [(0, "Wolf".to_string())]);
		assert!(ws.models_using("Models/Nothing").is_empty());
		assert_eq!(
			ws.item_with_stem("Models/Fin"),
			Some((0, "Fin".to_string()))
		);
		assert_eq!(ws.item_with_stem("Models/Leg"), None);
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
		ws.remove_models(&[0]).unwrap();
		assert!(ws.models().is_empty());
		assert!(!dir.join("Rigs/Workspace/Animals/Wolf.rig.json").exists());
	}

	/// A model named `name` at `Workspace/<name>` with `parts` parts.
	fn rig_model(name: &str, parts: usize) -> AnimatedModel {
		AnimatedModel {
			name: name.into(),
			path: format!("Workspace/{name}"),
			parts: vec![Default::default(); parts],
			..Default::default()
		}
	}

	/// Importing a model again replaces its rig file with a new one rather
	/// than writing over the old one in place, so a failed write leaves the
	/// old file whole. A hard link to the old file still sees the old rig.
	#[test]
	fn importing_a_model_again_replaces_its_rig_file_whole() {
		let dir = TempDir::new("ws_rig_replace");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add_model(&rig_model("Wolf", 1), &[]).unwrap();
		let rig = dir.join(&ws.models()[0].file);
		let old = std::fs::read_to_string(&rig).unwrap();
		let link = dir.join("old.rig.json");
		std::fs::hard_link(&rig, &link).unwrap();

		ws.add_model(&rig_model("Wolf", 2), &[]).unwrap();
		assert_eq!(std::fs::read_to_string(&link).unwrap(), old);
		let new = export_file::parse(&std::fs::read_to_string(&rig).unwrap()).unwrap();
		assert_eq!(new.models[0].parts.len(), 2);

		// when the new file cannot be written, the old one and its record stay
		std::fs::create_dir(dir.join("Rigs/Workspace/Wolf.rig.json.tmp")).unwrap();
		let written = std::fs::read_to_string(&rig).unwrap();
		assert!(ws.add_model(&rig_model("Wolf", 3), &[]).is_err());
		assert_eq!(std::fs::read_to_string(&rig).unwrap(), written);
		assert_eq!(ws.models()[0].parts, 2);
	}

	/// A new model whose rig file cannot be written is not added and does
	/// not keep its stem, so the next try gets the same file name.
	#[test]
	fn a_new_rig_that_fails_to_write_frees_its_stem() {
		let dir = TempDir::new("ws_rig_fails");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		// a file where the Rigs folder should be
		std::fs::write(dir.join("Rigs"), b"x").unwrap();
		assert!(ws.add_model(&rig_model("Wolf", 1), &[]).is_err());
		assert!(ws.models().is_empty());
		std::fs::remove_file(dir.join("Rigs")).unwrap();
		assert!(ws.add_model(&rig_model("Wolf", 1), &[]).unwrap());
		assert_eq!(ws.models()[0].file, "Rigs/Workspace/Wolf.rig.json");
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

	/// `A`'s normal map is `A.normal.png`, which is also where the texture
	/// of an item `A.normal` would go, so the second one gets a numbered stem
	/// whichever comes first, ignoring case.
	#[test]
	fn stems_whose_map_files_would_collide_are_numbered() {
		let dir = TempDir::new("ws_map_names");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add([
			entry("F", "A", 1, &[(MapKind::Normal, 2)]),
			entry("F", "a.NORMAL", 3, &[(MapKind::Texture, 4)]),
			entry("G", "B.color", 5, &[]),
			entry("G", "B", 6, &[(MapKind::Color, 7)]),
			// a dot that is not a map suffix is fine
			entry("G", "B.other", 8, &[]),
		]);
		let stems: Vec<_> = ws.items().iter().map(|i| i.stem.as_str()).collect();
		assert_eq!(
			stems,
			[
				"Models/F/A",
				"Models/F/a.NORMAL (2)",
				"Models/G/B.color",
				"Models/G/B (2)",
				"Models/G/B.other",
			]
		);
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

	/// Opening a workspace queues again a map whose file has gone, like a
	/// mesh; files still there stay done.
	#[test]
	fn open_queues_missing_maps_again() {
		let dir = TempDir::new("ws_missing_maps");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add([entry(
			"",
			"A",
			1,
			&[(MapKind::Color, 2), (MapKind::Normal, 3)],
		)]);
		std::fs::create_dir_all(dir.join("Models")).unwrap();
		for file in ["Models/A.mesh", "Models/A.png"] {
			std::fs::write(dir.join(file), b"x").unwrap();
		}
		let done = |path: &str| AssetState::Done { path: path.into() };
		ws.set_state("Models/A", AssetKind::Mesh, done("Models/A.mesh"));
		ws.set_state(
			"Models/A",
			AssetKind::Map(MapKind::Color),
			done("Models/A.png"),
		);
		ws.set_state(
			"Models/A",
			AssetKind::Map(MapKind::Normal),
			done("Models/A_normal.png"),
		);
		ws.save_if_dirty().unwrap();

		let reopened = Workspace::open(&dir).unwrap();
		let a = &reopened.items()[0];
		assert_eq!(a.mesh, done("Models/A.mesh"));
		assert_eq!(a.maps[&MapKind::Color].state, done("Models/A.png"));
		assert_eq!(a.maps[&MapKind::Normal].state, AssetState::Pending);
		// so it is saved again
		assert!(reopened.dirty);
	}

	/// Opening a workspace also queues again the images models use (a
	/// decal, a MaterialVariant map, clothing) whose files have gone, in
	/// every model sharing them; images still there stay done.
	#[test]
	fn open_queues_missing_model_images_again() {
		use crate::anim::schema::{ClothingInfo, DecalInfo, MaterialVariantInfo, ModelPart};
		let dir = TempDir::new("ws_missing_images");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let model = |name: &str| AnimatedModel {
			name: name.into(),
			path: format!("Workspace/{name}"),
			parts: vec![ModelPart {
				decals: vec![DecalInfo {
					texture: "rbxassetid://77".into(),
					..Default::default()
				}],
				material_variant: Some(MaterialVariantInfo {
					color_map: "rbxassetid://88".into(),
					..Default::default()
				}),
				..Default::default()
			}],
			clothing: Some(ClothingInfo {
				shirt_template: Some("rbxassetid://99".into()),
				pants_template: Some("rbxassetid://66".into()),
				..Default::default()
			}),
			..Default::default()
		};
		ws.add_model(&model("A"), &[]).unwrap();
		ws.add_model(&model("B"), &[]).unwrap();
		std::fs::create_dir_all(dir.join("Images")).unwrap();
		std::fs::write(dir.join("Images/66.png"), b"x").unwrap();
		let done = |id: u64| AssetState::Done {
			path: format!("Images/{id}.png"),
		};
		for id in [66, 77, 88, 99] {
			ws.set_state(&image_stem(id), AssetKind::Image, done(id));
		}
		ws.save_if_dirty().unwrap();

		let reopened = Workspace::open(&dir).unwrap();
		for m in reopened.models() {
			assert_eq!(m.images[&66], done(66));
			for id in [77, 88, 99] {
				assert_eq!(m.images[&id], AssetState::Pending, "image {id}");
			}
		}
		assert!(reopened.dirty);
		assert_eq!(reopened.image_counts(), (3, 0));
	}

	/// Removing an item deletes its organised files, keeps the cached
	/// download and frees its stem, so importing it again gets the same one.
	#[test]
	fn remove_deletes_files_and_frees_the_stem() {
		let dir = TempDir::new("ws_remove");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add([entry("F", "A", 1, &[]), entry("F", "B", 2, &[])]);
		std::fs::create_dir_all(dir.join("Models/F")).unwrap();
		std::fs::write(dir.join("Models/F/A.mesh"), b"x").unwrap();
		let cache = cache_path(&dir, 1);
		std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
		std::fs::write(&cache, b"x").unwrap();
		ws.set_state(
			"Models/F/A",
			AssetKind::Mesh,
			AssetState::Done {
				path: "Models/F/A.mesh".into(),
			},
		);
		let generation = ws.generation();

		// an index past the end is skipped
		ws.remove(&[0, 0, 9]).unwrap();
		assert_eq!(ws.items().len(), 1);
		assert_eq!(ws.items()[0].stem, "Models/F/B");
		assert!(!dir.join("Models/F/A.mesh").exists());
		assert!(cache.exists());
		assert_eq!(ws.generation(), generation + 1);

		let (stem, new) = ws.add_entry(&entry("F", "a", 1, &[]));
		assert!(new);
		assert_eq!(stem, "Models/F/a");
	}

	/// An item with a file that cannot be deleted (here a folder in its
	/// place) is not removed and keeps its stem; its other files are deleted
	/// and download again. A file that is already gone does not count.
	#[test]
	fn remove_keeps_items_whose_files_cannot_be_deleted() {
		let dir = TempDir::new("ws_remove_fails");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add([
			entry("F", "A", 1, &[(MapKind::Color, 2)]),
			entry("F", "B", 3, &[]),
		]);
		std::fs::create_dir_all(dir.join("Models/F/A.mesh")).unwrap();
		std::fs::write(dir.join("Models/F/A.color.png"), b"x").unwrap();
		let done = |path: &str| AssetState::Done { path: path.into() };
		ws.set_state("Models/F/A", AssetKind::Mesh, done("Models/F/A.mesh"));
		let color = AssetKind::Map(MapKind::Color);
		ws.set_state("Models/F/A", color, done("Models/F/A.color.png"));
		// B's file is already gone
		ws.set_state("Models/F/B", AssetKind::Mesh, done("Models/F/B.mesh"));

		let error = ws.remove(&[0, 1]).unwrap_err();
		assert!(
			error.starts_with("Could not delete Models/F/A.mesh"),
			"{error}"
		);
		assert_eq!(ws.items().len(), 1);
		let a = &ws.items()[0];
		assert_eq!(a.stem, "Models/F/A");
		assert_eq!(a.mesh, done("Models/F/A.mesh"));
		assert_eq!(a.maps[&MapKind::Color].state, AssetState::Pending);
		assert!(!dir.join("Models/F/A.color.png").exists());
		let (stem, _) = ws.add_entry(&entry("F", "A", 4, &[]));
		assert_eq!(stem, "Models/F/A (2)");

		// the same for a model's rig file
		ws.add_model(&rig_model("Wolf", 1), &[]).unwrap();
		let rig = dir.join(&ws.models()[0].file);
		std::fs::remove_file(&rig).unwrap();
		std::fs::create_dir(&rig).unwrap();
		let error = ws.remove_models(&[0]).unwrap_err();
		assert!(error.starts_with("Could not delete Rigs/"), "{error}");
		assert_eq!(ws.models().len(), 1);
	}

	/// A download that finishes after its item was removed changes nothing.
	#[test]
	fn set_state_ignores_removed_items() {
		let dir = TempDir::new("ws_set_state");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add([entry("", "A", 1, &[(MapKind::Color, 2)])]);
		ws.remove(&[0]).unwrap();
		ws.save_if_dirty().unwrap();
		let done = AssetState::Done {
			path: "Models/A.mesh".into(),
		};
		ws.set_state("Models/A", AssetKind::Mesh, done.clone());
		ws.set_state("Models/A", AssetKind::Map(MapKind::Color), done.clone());
		assert!(ws.items().is_empty());
		assert!(!ws.dirty);

		// nor does a map the item does not have, or an image no model uses
		ws.add([entry("", "B", 3, &[])]);
		ws.save_if_dirty().unwrap();
		ws.set_state("Models/B", AssetKind::Map(MapKind::Normal), done.clone());
		ws.set_state("Images/5", AssetKind::Image, done);
		assert!(ws.items()[0].maps.is_empty());
		assert!(!ws.dirty);
	}
}
