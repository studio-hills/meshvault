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
//!   .cache/format                  the cache's format (see `store::upgrade_cache`)
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
use crate::studio::export_file::{Entry, Origin};

pub use self::manifest::{AssetState, Item, MapRef, ModelItem, ModelVersion};
use self::manifest::{MANIFEST_VERSION, Manifest};
pub(crate) use self::paths::check_no_links;
#[cfg(test)]
pub(crate) use self::paths::junction;
pub use self::paths::{CACHE_DIR, MANIFEST, cache_path, sanitize};
use self::paths::{MODELS_DIR, RIGS_DIR, fit_folder, image_id, image_stem, rig_stem, stem_name};
use self::rig_file::write_rig;
pub use self::store::read_asset;

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
	/// The place to send with the download: that of the first item or
	/// model wanting it that has one.
	pub place_id: Option<u64>,
}

impl Job {
	pub fn wants_image(&self) -> bool {
		self.targets.iter().any(|(_, k)| k.is_image())
	}
}

/// Where a model from an export shows in the library: its game, then the
/// folder holding its instance.
pub fn model_folder(origin: &Origin, path: &str) -> Vec<String> {
	let mut folder: Vec<String> = origin
		.game
		.iter()
		.cloned()
		.chain(path.split('/').filter(|s| !s.is_empty()).map(String::from))
		.collect();
	folder.pop();
	folder
}

/// Whether something stored with `game` in `folder` is where an export
/// from `origin` puts `entry_folder` (which starts with the export's game,
/// when it names one). Something imported before exports named their game
/// is there when it was in the same folder below the game.
fn same_place(
	game: Option<&str>,
	folder: &[String],
	origin: &Origin,
	entry_folder: &[String],
) -> bool {
	match (game, origin.game.as_deref()) {
		(None, Some(g)) => {
			entry_folder.first().is_some_and(|f| f == g) && folder == &entry_folder[1..]
		}
		_ => game == origin.game.as_deref() && folder == entry_folder,
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
		store::upgrade_cache(root);
		Ok(ws)
	}

	pub fn open(root: &Path) -> Result<Self, String> {
		// before anything reads or writes in it: a shared workspace whose own
		// folders link elsewhere could have the cache upgrade delete there
		for entry in paths::OWN_ENTRIES {
			check_no_links(root, Path::new(entry))
				.map_err(|e| format!("{} is not a valid workspace: {e}", root.display()))?;
		}
		let path = root.join(MANIFEST);
		let json = crate::studio::export_file::read_text(&path)?;
		// a manifest's lists are read within the same bounds as an export's
		let limits = crate::anim::budget::LIMITS;
		let (parsed, over) = crate::anim::budget::with_budget(&limits, json.len(), || {
			serde_json::from_str::<Manifest>(&json)
		});
		let invalid = |why: String| format!("{} is not a valid workspace: {why}", path.display());
		let mut manifest = match (parsed, over) {
			(_, Some(why)) => return Err(invalid(why)),
			(Err(e), None) => return Err(invalid(e.to_string())),
			(Ok(manifest), None) => manifest,
		};
		if manifest.version > MANIFEST_VERSION {
			return Err(format!(
				"{} was made by a newer version of this app (workspace version {})",
				path.display(),
				manifest.version
			));
		}
		// before anything touches a path in it, even to see if it exists
		manifest
			.validate()
			.map_err(|e| format!("{} is not a valid workspace: {e}", path.display()))?;
		let dirty = manifest.upgrade(root);
		store::upgrade_cache(root);
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
		let tmp_name = format!("{MANIFEST}.tmp");
		for name in [MANIFEST, &tmp_name] {
			check_no_links(&self.root, Path::new(name))
				.map_err(|e| format!("Could not save workspace: {e}"))?;
		}
		let tmp = self.root.join(tmp_name);
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
		let base = stem_name(name);
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
		let added = entries
			.into_iter()
			.filter(|e| self.add_entry(e, &Origin::default()).1)
			.count();
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
	///
	/// An item imported before exports named their game is the same item,
	/// and moves into its game's folder (its files stay where they are). The
	/// item keeps the place it was last imported from, for its downloads.
	pub fn add_entry(&mut self, entry: &Entry, origin: &Origin) -> (String, bool) {
		let maps = entry.maps.iter().map(|(&kind, &id)| {
			(
				kind,
				MapRef {
					id,
					state: AssetState::Pending,
				},
			)
		});
		if let Some(i) = self.find_entry(entry, origin) {
			let item = &mut self.manifest.items[i];
			if item.game.is_none() && origin.game.is_some() {
				item.game = origin.game.clone();
				item.folder = fit_folder(&entry.folder);
				self.dirty = true;
				self.generation += 1;
			}
			if origin.place_id.is_some() && item.place_id != origin.place_id {
				item.place_id = origin.place_id;
				self.dirty = true;
			}
			for (kind, map) in maps {
				if let std::collections::btree_map::Entry::Vacant(slot) = item.maps.entry(kind) {
					slot.insert(map);
					self.dirty = true;
				}
			}
			return (item.stem.clone(), false);
		}
		let stem = self.unique_stem(MODELS_DIR, &fit_folder(&entry.folder), &entry.name);
		let maps = maps.collect();
		let mut item = Item::new(
			entry.name.clone(),
			fit_folder(&entry.folder),
			entry.mesh_id,
			stem.clone(),
			maps,
		);
		item.game = origin.game.clone();
		item.place_id = origin.place_id;
		self.manifest.items.push(item);
		self.dirty = true;
		self.generation += 1;
		(stem, true)
	}

	/// The item an export entry from `origin` is, if the workspace has it:
	/// in the same place (see `same_place`), with the same name and mesh,
	/// and no map id that differs from the item's (the same mesh with a
	/// different image is a different item).
	pub fn find_entry(&self, entry: &Entry, origin: &Origin) -> Option<usize> {
		// as stored, cut to the depth a manifest holds
		let folder = fit_folder(&entry.folder);
		self.manifest.items.iter().position(|i| {
			same_place(i.game.as_deref(), &i.folder, origin, &folder)
				&& i.name == entry.name
				&& i.mesh_id == entry.mesh_id
				&& i.maps
					.iter()
					.all(|(k, m)| entry.maps.get(k).is_none_or(|&id| id == m.id))
		})
	}

	/// The stored model that an export's model at instance `path` from
	/// `origin` is, if any: the same path from the same game, or one
	/// imported before exports named their game.
	pub fn find_model(&self, path: &str, origin: &Origin) -> Option<usize> {
		self.manifest.models.iter().position(|m| {
			m.path == path && (m.game == origin.game || (m.game.is_none() && origin.game.is_some()))
		})
	}

	/// Stores an animated model. `meshes` are the entries its parts use, each
	/// with the stem of the item holding it (from [`Self::add_entry`]). The
	/// same model already stored (see [`Self::find_model`]) is replaced,
	/// keeping its rig file. Returns whether it is new.
	pub fn add_model(
		&mut self,
		model: &AnimatedModel,
		meshes: &[(&Entry, String)],
		origin: &Origin,
	) -> Result<bool, String> {
		let existing = self.find_model(&model.path, origin);
		let folder = fit_folder(&model_folder(origin, &model.path));
		let file = match existing {
			Some(i) => self.manifest.models[i].file.clone(),
			None => format!(
				"{}.rig.json",
				self.unique_stem(RIGS_DIR, &folder, &model.name)
			),
		};
		if let Err(e) = write_rig(&self.root, &file, model, meshes) {
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
			game: origin.game.clone(),
			place_id: origin.place_id,
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
			if let Err(e) = delete_file(&self.root, RIGS_DIR, &model.file) {
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
		let mut push = |asset_id: u64, stem: &String, kind: AssetKind, place_id: Option<u64>| {
			let i = *index.entry(asset_id).or_insert_with(|| {
				jobs.push(Job {
					asset_id,
					targets: Vec::new(),
					place_id: None,
				});
				jobs.len() - 1
			});
			jobs[i].targets.push((stem.clone(), kind));
			jobs[i].place_id = jobs[i].place_id.or(place_id);
		};
		for item in &self.manifest.items {
			if item.mesh == AssetState::Pending {
				push(item.mesh_id, &item.stem, AssetKind::Mesh, item.place_id);
			}
			for (&kind, map) in &item.maps {
				if map.state == AssetState::Pending {
					push(map.id, &item.stem, AssetKind::Map(kind), item.place_id);
				}
			}
		}
		// each image once, with the first place of the models wanting it
		let mut images: BTreeMap<u64, Option<u64>> = BTreeMap::new();
		for model in &self.manifest.models {
			for (&id, state) in &model.images {
				if *state == AssetState::Pending {
					let place = images.entry(id).or_insert(None);
					*place = place.or(model.place_id);
				}
			}
		}
		for (id, place_id) in images {
			push(id, &image_stem(id), AssetKind::Image, place_id);
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
	/// left alone, and their files deleted. Returns false if the download
	/// succeeded but a file an item still wants could not be put in place
	/// (that item is then `Failed`); a failed download returns true, since
	/// the downloader counted its failure already.
	pub fn record_download(
		&mut self,
		job: &Job,
		result: Result<Vec<store::Stored>, String>,
	) -> bool {
		let mut placed = true;
		match result {
			Ok(written) => {
				for mut w in written {
					// one not published is deleted when dropped
					if !self.uses(&w.stem, w.kind, job.asset_id) {
						continue;
					}
					let state = match w.publish(&self.root) {
						Ok(()) => AssetState::Done {
							path: w.path.clone(),
						},
						Err(error) => {
							placed = false;
							AssetState::Failed { error }
						}
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
		placed
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
				match delete_file(&self.root, MODELS_DIR, path) {
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

/// Deletes a file in the workspace's folder `top`. One that is already gone
/// counts as deleted. A path that is not a plain one in `top` (see
/// [`paths::check_path`]) is refused, though `open` refuses manifests with
/// such paths already, and so is one through a link (see [`check_no_links`]).
fn delete_file(root: &Path, top: &str, path: &str) -> Result<(), String> {
	paths::check_path(path, top).map_err(|e| format!("Could not delete {path:?}: {e}"))?;
	check_no_links(root, Path::new(path)).map_err(|e| format!("Could not delete {path}: {e}"))?;
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
			game: None,
			place_id: None,
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

	fn from(game: &str, place_id: Option<u64>) -> Origin {
		Origin {
			game: Some(game.into()),
			place_id,
		}
	}

	/// An export naming its place: its items go under the game's folder,
	/// and remember the game and the place for their downloads.
	#[test]
	fn an_import_is_filed_under_its_game() {
		let dir = TempDir::new("ws_game");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let pets = from("Pet Sim", Some(11));
		let (stem, new) = ws.add_entry(&entry("Pet Sim/RS/Wolf", "Body", 1, &[]), &pets);
		assert!(new);
		assert_eq!(stem, "Models/Pet Sim/RS/Wolf/Body");
		let item = &ws.items()[0];
		assert_eq!(item.game.as_deref(), Some("Pet Sim"));
		assert_eq!(item.place_id, Some(11));
		// the same mesh in the same folder of another game is another item
		let (stem, new) = ws.add_entry(&entry("Zoo/RS/Wolf", "Body", 1, &[]), &from("Zoo", None));
		assert!(new);
		assert_eq!(stem, "Models/Zoo/RS/Wolf/Body");
		// and the same game again is the same item
		let (_, new) = ws.add_entry(&entry("Pet Sim/RS/Wolf", "Body", 1, &[]), &pets);
		assert!(!new);
		assert_eq!(ws.items().len(), 2);
	}

	/// Items imported before exports named their game are the same items
	/// when the export is imported again: they move under the game in the
	/// library, keep their files, and take its place for downloads.
	#[test]
	fn older_items_are_adopted_by_their_game() {
		let dir = TempDir::new("ws_adopt");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let (old_stem, _) = ws.add_entry(&entry("RS/Wolf", "Body", 1, &[]), &Origin::default());
		let pets = from("Pet Sim", Some(11));
		let again = entry("Pet Sim/RS/Wolf", "Body", 1, &[]);
		assert_eq!(ws.find_entry(&again, &pets), Some(0));
		let (stem, new) = ws.add_entry(&again, &pets);
		assert!(!new);
		assert_eq!(stem, old_stem);
		let item = &ws.items()[0];
		assert_eq!(item.folder, ["Pet Sim", "RS", "Wolf"]);
		assert_eq!(item.game.as_deref(), Some("Pet Sim"));
		assert_eq!(item.place_id, Some(11));
		// once adopted it belongs to that game: another game's copy is new
		let zoo = entry("Zoo/RS/Wolf", "Body", 1, &[]);
		assert_eq!(ws.find_entry(&zoo, &from("Zoo", None)), None);
		// and an export naming no game does not find it either
		assert_eq!(
			ws.find_entry(&entry("RS/Wolf", "Body", 1, &[]), &Origin::default()),
			None
		);
	}

	/// A model is the same model only from the same game (or, imported before
	/// exports named their game, from any); each game's rig file is its own.
	#[test]
	fn models_are_told_apart_by_game() {
		let dir = TempDir::new("ws_game_models");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let wolf = rig_model("Wolf", 1);
		assert!(ws.add_model(&wolf, &[], &Origin::default()).unwrap());
		let old_file = ws.models()[0].file.clone();
		// adopted by the first game that brings it
		let pets = from("Pet Sim", Some(11));
		assert!(!ws.add_model(&wolf, &[], &pets).unwrap());
		assert_eq!(ws.models().len(), 1);
		let model = &ws.models()[0];
		assert_eq!(model.file, old_file);
		assert_eq!(model.folder, ["Pet Sim", "Workspace"]);
		assert_eq!(
			(model.game.as_deref(), model.place_id),
			(Some("Pet Sim"), Some(11))
		);
		// another game's model at the same path is another model
		assert!(ws.add_model(&wolf, &[], &from("Zoo", None)).unwrap());
		assert_eq!(ws.models()[1].file, "Rigs/Zoo/Workspace/Wolf.rig.json");
		assert_eq!(ws.find_model("Workspace/Wolf", &pets), Some(0));
	}

	/// Each download is sent with the place its asset was exported from.
	#[test]
	fn downloads_carry_their_place() {
		let dir = TempDir::new("ws_place_jobs");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add_entry(
			&entry("A/X", "One", 1, &[(MapKind::Texture, 5)]),
			&from("A", Some(11)),
		);
		ws.add_entry(&entry("X", "Two", 2, &[]), &Origin::default());
		// the same mesh from two places: the first place that has one
		ws.add_entry(&entry("X", "Again", 1, &[]), &Origin::default());
		let mut model = rig_model("Wolf", 1);
		model.parts[0].decals = vec![crate::anim::schema::DecalInfo {
			texture: "rbxassetid://77".into(),
			..Default::default()
		}];
		ws.add_model(&model, &[], &from("B", Some(22))).unwrap();
		let place = |id| {
			ws.pending_jobs()
				.into_iter()
				.find(|j| j.asset_id == id)
				.unwrap()
				.place_id
		};
		assert_eq!(place(1), Some(11));
		assert_eq!(place(5), Some(11));
		assert_eq!(place(2), None);
		assert_eq!(place(77), Some(22));
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
		let (stem, new) = ws.add_entry(&torso, &Origin::default());
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
		assert!(
			ws.add_model(&model, &[(&torso, stem.clone())], &Origin::default())
				.unwrap()
		);
		// re-importing replaces it in place
		assert!(
			!ws.add_model(&model, &[(&torso, stem.clone())], &Origin::default())
				.unwrap()
		);
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
		ws.add_model(&rig_model("Wolf", 1), &[], &Origin::default())
			.unwrap();
		let rig = dir.join(&ws.models()[0].file);
		let old = std::fs::read_to_string(&rig).unwrap();
		let link = dir.join("old.rig.json");
		std::fs::hard_link(&rig, &link).unwrap();

		ws.add_model(&rig_model("Wolf", 2), &[], &Origin::default())
			.unwrap();
		assert_eq!(std::fs::read_to_string(&link).unwrap(), old);
		let new = export_file::parse(&std::fs::read_to_string(&rig).unwrap()).unwrap();
		assert_eq!(new.models[0].parts.len(), 2);

		// when the new file cannot be written, the old one and its record stay
		std::fs::create_dir(dir.join("Rigs/Workspace/Wolf.rig.json.tmp")).unwrap();
		let written = std::fs::read_to_string(&rig).unwrap();
		assert!(
			ws.add_model(&rig_model("Wolf", 3), &[], &Origin::default())
				.is_err()
		);
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
		assert!(
			ws.add_model(&rig_model("Wolf", 1), &[], &Origin::default())
				.is_err()
		);
		assert!(ws.models().is_empty());
		std::fs::remove_file(dir.join("Rigs")).unwrap();
		assert!(
			ws.add_model(&rig_model("Wolf", 1), &[], &Origin::default())
				.unwrap()
		);
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
		ws.add_model(&model("A"), &[], &Origin::default()).unwrap();
		ws.add_model(&model("B"), &[], &Origin::default()).unwrap();
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
		ws.add_model(&model("A"), &[], &Origin::default()).unwrap();
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
		ws.add_model(&tiled, &[], &Origin::default()).unwrap();
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
		ws.add_model(&model("A"), &[], &Origin::default()).unwrap();
		ws.add_model(&model("B"), &[], &Origin::default()).unwrap();
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

	/// A cache entry written by an older build may hold only the start of
	/// its asset (here a mesh's header line), which was then used as the
	/// asset on every later import (once: review of bug B14). Such entries
	/// cannot be told from whole ones, so opening a workspace whose cache
	/// has no format deletes them once; the rest of the cache stays, and
	/// entries written from then on are kept.
	#[test]
	fn cache_entries_from_before_whole_writes_are_dropped_once() {
		let dir = TempDir::new("ws_old_cache");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add([entry("F", "A", 1, &[])]);
		ws.save_if_dirty().unwrap();
		// the cache as an older build left it
		let cache = dir.join(CACHE_DIR);
		std::fs::remove_file(cache.join("format")).unwrap();
		std::fs::write(cache_path(&dir, 1), b"version 4.00\n").unwrap();
		std::fs::write(cache.join("facts.json"), b"{}").unwrap();
		std::fs::create_dir_all(cache.join("thumbnails")).unwrap();
		std::fs::write(cache.join("thumbnails/1.png"), b"png").unwrap();

		Workspace::open(&dir).unwrap();
		assert_eq!(store::read_cache(&dir, 1), None);
		assert!(cache.join("facts.json").exists());
		assert!(cache.join("thumbnails/1.png").exists());

		store::write_cache(&dir, 1, b"version 4.00\nWHOLE");
		Workspace::open(&dir).unwrap();
		assert_eq!(
			store::read_cache(&dir, 1).as_deref(),
			Some(&b"version 4.00\nWHOLE"[..])
		);
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

		let (stem, new) = ws.add_entry(&entry("F", "a", 1, &[]), &Origin::default());
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
		let (stem, _) = ws.add_entry(&entry("F", "A", 4, &[]), &Origin::default());
		assert_eq!(stem, "Models/F/A (2)");

		// the same for a model's rig file
		ws.add_model(&rig_model("Wolf", 1), &[], &Origin::default())
			.unwrap();
		let rig = dir.join(&ws.models()[0].file);
		std::fs::remove_file(&rig).unwrap();
		std::fs::create_dir(&rig).unwrap();
		let error = ws.remove_models(&[0]).unwrap_err();
		assert!(error.starts_with("Could not delete Rigs/"), "{error}");
		assert_eq!(ws.models().len(), 1);
	}

	/// Writes a version 3 manifest with `items` and `models` into `dir`.
	fn write_manifest(dir: &Path, items: serde_json::Value, models: serde_json::Value) {
		let manifest = serde_json::json!({
			"version": 3, "name": "Shared", "items": items, "models": models,
		});
		std::fs::write(dir.join(MANIFEST), manifest.to_string()).unwrap();
	}

	/// An item at `stem` whose mesh is downloaded to `mesh`.
	fn done_item(stem: &str, mesh: &str) -> serde_json::Value {
		serde_json::json!({
			"name": "A", "folder": [], "mesh_id": 1, "stem": stem,
			"mesh": {"state": "done", "path": mesh},
		})
	}

	/// A model stored in `file` using the item at `mesh_stem`, with image 7
	/// downloaded to `image`.
	fn done_model(file: &str, mesh_stem: &str, image: &str) -> serde_json::Value {
		serde_json::json!({
			"name": "M", "path": "Workspace/M", "folder": ["Workspace"], "file": file,
			"meshes": {"1": mesh_stem}, "parts": 1, "clips": 0, "playable_clips": 0,
			"images": {"7": {"state": "done", "path": image}},
		})
	}

	/// A manifest's lists are read within bounds, as an export's are: a
	/// folder list far over the cap stops being read at the cap, rather than
	/// being built whole and checked after.
	#[test]
	fn open_stops_reading_a_list_at_its_cap() {
		let dir = TempDir::new("ws_manifest_budget");
		let folder = vec!["F"; 10_000];
		let manifest = serde_json::json!({
			"version": 3,
			"name": "Big",
			"items": [{
				"name": "A",
				"folder": folder,
				"mesh_id": 1,
				"stem": "Models/A",
				"mesh": {"state": "pending"},
			}],
		});
		std::fs::write(dir.join(MANIFEST), manifest.to_string()).unwrap();
		let err = Workspace::open(&dir).err().unwrap();
		assert!(err.contains("folders in one path"), "{err}");
		let read = crate::anim::budget::records_read();
		assert!(read < 200, "read {read} records of a 10,000-folder list");
	}

	/// A workspace someone hands over is checked when it is opened: every
	/// path in it must be a plain relative path under the folder it belongs
	/// in, so none can reach outside the workspace (MV-01). Each bad path
	/// refuses the whole workspace, naming the entry.
	#[test]
	fn open_refuses_paths_outside_the_workspace() {
		let good_item = || done_item("Models/A", "Models/A.mesh");
		let good_model = || done_model("Rigs/M.rig.json", "Models/A", "Images/7.png");
		let deep: Vec<String> = (0..65).map(|i| format!("f{i}")).collect();
		let mut deep_item = good_item();
		deep_item["folder"] = serde_json::json!(deep);
		let mut deep_model = good_model();
		deep_model["folder"] = serde_json::json!(deep);
		let mut map_item = good_item();
		map_item["maps"] = serde_json::json!({
			"normal": {"id": 2, "state": {"state": "done", "path": "Models/a.mesh:evil"}},
		});
		let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
			(
				"items[0].stem",
				done_item("../outside", "Models/A.mesh"),
				good_model(),
			),
			(
				"items[0].stem",
				done_item("Models/../../outside", "Models/A.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", r"C:\Windows\x.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "C:/Windows/x.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", r"\\server\share\x"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "//server/share/x"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", r"\\?\C:\x.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "/etc/x.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "Models/a.mesh:evil"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", r"Models\A.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "Rigs/A.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "Models/./A.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "Models/A.mesh."),
				good_model(),
			),
			(
				"items[0].stem",
				done_item("Models/CON", "Models/A.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "Models/nul.mesh"),
				good_model(),
			),
			(
				"items[0].mesh",
				done_item("Models/A", "Models/com1.mesh"),
				good_model(),
			),
			("items[0].maps", map_item, good_model()),
			("items[0].folder", deep_item, good_model()),
			("models[0].folder", good_item(), deep_model),
			(
				"models[0].file",
				good_item(),
				done_model(r"\\server\share\x.rig.json", "Models/A", "Images/7.png"),
			),
			(
				"models[0].file",
				good_item(),
				done_model("../x.rig.json", "Models/A", "Images/7.png"),
			),
			(
				"models[0].file",
				good_item(),
				done_model("Models/M.rig.json", "Models/A", "Images/7.png"),
			),
			(
				"models[0].meshes",
				good_item(),
				done_model("Rigs/M.rig.json", "../A", "Images/7.png"),
			),
			(
				"models[0].images",
				good_item(),
				done_model("Rigs/M.rig.json", "Models/A", "Images/../../x.png"),
			),
			(
				"models[0].images",
				good_item(),
				done_model("Rigs/M.rig.json", "Models/A", "Models/7.png"),
			),
		];
		for (field, item, model) in cases {
			let dir = TempDir::new("ws_bad_path");
			write_manifest(&dir, serde_json::json!([item]), serde_json::json!([model]));
			match Workspace::open(&dir) {
				Ok(_) => panic!("opened a workspace with a bad {field}: {item} {model}"),
				Err(e) => assert!(e.contains(field), "{field}: {e}"),
			}
		}
		// and the good ones open
		let dir = TempDir::new("ws_good_path");
		write_manifest(
			&dir,
			serde_json::json!([good_item()]),
			serde_json::json!([good_model()]),
		);
		Workspace::open(&dir).unwrap();
	}

	/// Paths older versions wrote are still accepted: a reserved name
	/// before the first dot with the old underscore at the end (`con.txt_`),
	/// the superscript COM and LPT names that were not reserved then, and
	/// the old map suffix.
	#[test]
	fn open_accepts_paths_older_versions_wrote() {
		let dir = TempDir::new("ws_old_paths");
		write_manifest(
			&dir,
			serde_json::json!([
				done_item(
					"Models/con.txt_/com1.part_",
					"Models/con.txt_/com1.part_.mesh"
				),
				done_item("Models/COM¹", "Models/COM¹.mesh"),
				done_item("Models/.hidden/A (2)", "Models/.hidden/A (2)_normal.png"),
				done_item("Models/CON_", "Models/CON_.mesh"),
			]),
			serde_json::json!([]),
		);
		Workspace::open(&dir).unwrap();
	}

	/// The original problem (MV-01): a shared workspace naming a file beside
	/// it made Remove delete that file. Now open refuses it; and should a
	/// bad path get past open, Remove still refuses to delete it.
	#[test]
	fn remove_never_deletes_outside_the_workspace() {
		let parent = TempDir::new("ws_remove_outside");
		let dir = parent.join("ws");
		std::fs::create_dir_all(&dir).unwrap();
		let outside = parent.join("outside.mesh");
		std::fs::write(&outside, b"keep me").unwrap();
		write_manifest(
			&dir,
			serde_json::json!([done_item("Models/A", "../outside.mesh")]),
			serde_json::json!([]),
		);
		if let Ok(mut ws) = Workspace::open(&dir) {
			let _ = ws.remove(&[0]);
		}
		assert!(
			outside.exists(),
			"Remove deleted a file outside the workspace"
		);

		// past open
		let mut ws = Workspace::create(&dir.join("new"), "Test").unwrap();
		let mut bad = item_at("Models/A", "A");
		bad.mesh = AssetState::Done {
			path: "../../outside.mesh".into(),
		};
		ws.set_contents(vec![bad], vec![model_using("M", &[])]);
		ws.manifest.models[0].file = "../../outside.mesh".into();
		assert!(ws.remove(&[0]).is_err());
		assert!(ws.remove_models(&[0]).is_err());
		assert!(outside.exists());
		assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");
	}

	/// Downloads are only put in place under the workspace: a stem that
	/// climbs out is refused when the file is published.
	#[test]
	fn downloads_are_never_written_outside_the_workspace() {
		let parent = TempDir::new("ws_publish_outside");
		let dir = parent.join("ws");
		Workspace::create(&dir, "Test").unwrap();
		let job = Job {
			asset_id: 1,
			targets: vec![("../outside".into(), AssetKind::Mesh)],
			place_id: None,
		};
		let mut written = store::stage(&dir, &job, b"version 4.00\n").unwrap();
		assert!(written[0].publish(&dir).is_err());
		assert!(!parent.join("outside.mesh").exists());
		let job = Job {
			asset_id: 2,
			targets: vec![("Images/../../outside".into(), AssetKind::Image)],
			place_id: None,
		};
		let mut written = store::stage(&dir, &job, b"\x89PNG").unwrap();
		assert!(written[0].publish(&dir).is_err());
		assert!(!parent.join("outside.png").exists());
	}

	/// Importing a model again writes its rig to the file the manifest
	/// names; one outside the workspace is refused and left alone.
	#[test]
	fn importing_a_model_again_never_writes_outside_the_workspace() {
		let parent = TempDir::new("ws_rig_outside");
		let dir = parent.join("ws");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let outside = parent.join("outside.txt");
		std::fs::write(&outside, b"keep me").unwrap();
		let mut model = model_using("Wolf", &[]);
		model.file = "../outside.txt".into();
		ws.set_contents(Vec::new(), vec![model]);
		assert!(
			ws.add_model(&rig_model("Wolf", 1), &[], &Origin::default())
				.is_err()
		);
		assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");
	}

	/// A folder `outside` beside the workspace folder `ws` in a new
	/// temporary folder, holding `file` with "keep me".
	fn beside_outside(tag: &str, file: &str) -> (TempDir, PathBuf, PathBuf) {
		let parent = TempDir::new(tag);
		let dir = parent.join("ws");
		let outside = parent.join("outside");
		std::fs::create_dir_all(&outside).unwrap();
		std::fs::write(outside.join(file), b"keep me").unwrap();
		(parent, dir, outside)
	}

	/// N01: opening a shared workspace whose `.cache` links to a folder
	/// elsewhere upgraded that folder's cache: it deleted the numeric files
	/// there and wrote its marker. Now a workspace whose own entries are
	/// links is refused before anything touches them.
	#[test]
	fn open_refuses_linked_entries_and_leaves_their_targets_alone() {
		let (_parent, dir, outside) = beside_outside("ws_linked_cache", "123");
		Workspace::create(&dir, "Test").unwrap();
		std::fs::remove_dir_all(dir.join(CACHE_DIR)).unwrap();
		if !junction(&dir.join(CACHE_DIR), &outside) {
			return;
		}
		let opened = Workspace::open(&dir);
		assert_eq!(
			std::fs::read(outside.join("123")).unwrap(),
			b"keep me",
			"opening deleted a file outside the workspace"
		);
		assert!(!outside.join("format").exists());
		let error = opened
			.err()
			.expect("a workspace with a linked .cache opened");
		assert!(error.contains(".cache"), "{error}");

		// each other folder or file of the workspace's own
		for name in [
			"Models",
			"Rigs",
			"Images",
			".cache/thumbnails",
			".cache/facts.json",
		] {
			let (_parent, dir, outside) = beside_outside("ws_linked_entry", "123");
			Workspace::create(&dir, "Test").unwrap();
			if !junction(&dir.join(name), &outside) {
				return;
			}
			let error = Workspace::open(&dir).err().expect(name);
			assert!(error.contains(name), "{error}");
		}
	}

	/// Remove deletes nothing through a linked folder inside `Models/` or
	/// `Rigs/` (N01).
	#[test]
	fn remove_never_deletes_through_a_link() {
		let (_parent, dir, outside) = beside_outside("ws_remove_linked", "A.mesh");
		std::fs::write(outside.join("M.rig.json"), b"keep me").unwrap();
		std::fs::create_dir_all(dir.join("Models")).unwrap();
		std::fs::create_dir_all(dir.join("Rigs")).unwrap();
		write_manifest(
			&dir,
			serde_json::json!([done_item("Models/F/A", "Models/F/A.mesh")]),
			serde_json::json!([done_model(
				"Rigs/F/M.rig.json",
				"Models/F/A",
				"Images/7.png"
			)]),
		);
		if !junction(&dir.join("Models/F"), &outside)
			|| !paths::junction(&dir.join("Rigs/F"), &outside)
		{
			return;
		}
		let mut ws = Workspace::open(&dir).unwrap();
		let removed = (ws.remove(&[0]), ws.remove_models(&[0]));
		assert_eq!(std::fs::read(outside.join("A.mesh")).unwrap(), b"keep me");
		assert_eq!(
			std::fs::read(outside.join("M.rig.json")).unwrap(),
			b"keep me"
		);
		assert!(removed.0.is_err() && removed.1.is_err(), "{removed:?}");
	}

	/// Downloads are not put in place, staged or cached through a link
	/// (N01).
	#[test]
	fn downloads_are_never_written_through_a_link() {
		let (_parent, dir, outside) = beside_outside("ws_publish_linked", "A.mesh");
		Workspace::create(&dir, "Test").unwrap();
		std::fs::create_dir_all(dir.join("Models")).unwrap();
		if !junction(&dir.join("Models/F"), &outside) {
			return;
		}
		let job = Job {
			asset_id: 1,
			targets: vec![("Models/F/A".into(), AssetKind::Mesh)],
			place_id: None,
		};
		let mut written = store::stage(&dir, &job, b"version 4.00\n").unwrap();
		let published = written[0].publish(&dir);
		assert_eq!(std::fs::read(outside.join("A.mesh")).unwrap(), b"keep me");
		assert!(published.is_err());

		// the cache linked elsewhere after the workspace was opened
		let (_parent, dir, outside) = beside_outside("ws_stage_linked", "keep");
		Workspace::create(&dir, "Test").unwrap();
		std::fs::remove_dir_all(dir.join(CACHE_DIR)).unwrap();
		if !junction(&dir.join(CACHE_DIR), &outside) {
			return;
		}
		let staged = store::stage(&dir, &job, b"version 4.00\n");
		store::write_cache(&dir, 1, b"version 4.00\n");
		let names: Vec<_> = std::fs::read_dir(&outside)
			.unwrap()
			.map(|e| e.unwrap().file_name())
			.collect();
		assert_eq!(
			names,
			["keep"],
			"a download was written outside the workspace"
		);
		assert!(staged.is_err());
	}

	/// Importing a model again does not write its rig through a linked
	/// folder inside `Rigs/` (N01).
	#[test]
	fn importing_a_model_again_never_writes_through_a_link() {
		let (_parent, dir, outside) = beside_outside("ws_rig_linked", "Wolf.rig.json");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.add_model(&rig_model("Wolf", 1), &[], &Origin::default())
			.unwrap();
		assert_eq!(ws.models()[0].file, "Rigs/Workspace/Wolf.rig.json");
		std::fs::remove_dir_all(dir.join("Rigs/Workspace")).unwrap();
		if !junction(&dir.join("Rigs/Workspace"), &outside) {
			return;
		}
		let added = ws.add_model(&rig_model("Wolf", 2), &[], &Origin::default());
		assert_eq!(
			std::fs::read(outside.join("Wolf.rig.json")).unwrap(),
			b"keep me"
		);
		assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
		assert!(added.is_err());
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
