//! The manifest's serialised types: items, models and their download
//! states, and the upgrade of older manifests on open.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::anim::budget;
use crate::scene::maps::MapKind;
use crate::workspace::AssetKind;
use crate::workspace::paths::{
	IMAGES_DIR, MODELS_DIR, RIGS_DIR, check_folder, check_path, saved_file,
};

/// Manifest format version: 1 had a single texture per item, 2 added
/// `maps`, 3 added animated models. tools/make_test_workspace.py writes
/// this version too; update it with this.
pub(super) const MANIFEST_VERSION: u32 = 3;

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

impl AssetState {
	/// Path of the downloaded file, relative to the workspace root.
	pub fn done_path(&self) -> Option<&str> {
		match self {
			AssetState::Done { path } => Some(path),
			_ => None,
		}
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
	#[serde(deserialize_with = "budget::folders")]
	pub folder: Vec<String>,
	pub mesh_id: u64,
	/// Relative path without extension, unique within the workspace.
	pub stem: String,
	pub mesh: AssetState,
	/// Maps selected for import. Maps that were not selected are absent.
	#[serde(default)]
	pub maps: BTreeMap<MapKind, MapRef>,
	/// The game it was imported from (its `folder` starts with it), or
	/// `None` for exports that do not name one and items imported before
	/// exports did.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub game: Option<String>,
	/// The place it was exported from, sent with its downloads.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub place_id: Option<u64>,
	// Version 1 manifests stored only a texture; migrated into `maps` on open.
	#[serde(default, skip_serializing)]
	texture_id: Option<u64>,
	#[serde(default, skip_serializing)]
	texture: Option<AssetState>,
}

impl Item {
	/// A new item with no download state yet but `Pending`.
	pub(super) fn new(
		name: String,
		folder: Vec<String>,
		mesh_id: u64,
		stem: String,
		maps: BTreeMap<MapKind, MapRef>,
	) -> Self {
		Self {
			name,
			folder,
			mesh_id,
			stem,
			mesh: AssetState::Pending,
			maps,
			game: None,
			place_id: None,
			texture_id: None,
			texture: None,
		}
	}
	pub(super) fn states(&self) -> impl Iterator<Item = &AssetState> {
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
		state.done_path()
	}
}

/// An animated model (rig and clips) from an export. Its parts use meshes
/// stored as ordinary items.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelItem {
	pub name: String,
	/// Roblox instance path; with `game`, identifies the model when an export
	/// is re-imported.
	pub path: String,
	/// Where it shows in the library: its game, then its instance's folder.
	#[serde(deserialize_with = "budget::folders")]
	pub folder: Vec<String>,
	/// The game it was imported from, as for [`Item::game`].
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub game: Option<String>,
	/// The place it was exported from, sent with its images' downloads.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub place_id: Option<u64>,
	/// The stored model record, relative to the workspace root.
	pub file: String,
	/// `Parts[].MeshRef` to the stem of the item holding that mesh. No more
	/// are read than a model may have parts.
	#[serde(deserialize_with = "mesh_refs")]
	pub meshes: BTreeMap<u32, String>,
	pub parts: usize,
	pub clips: usize,
	pub playable_clips: usize,
	/// Images the model's parts use directly (decals), by asset id, stored
	/// once each under `Images/`. No more are read than an export may have
	/// meshes.
	#[serde(
		default,
		skip_serializing_if = "BTreeMap::is_empty",
		deserialize_with = "model_images"
	)]
	pub images: BTreeMap<u64, AssetState>,
	/// Which model this is and which import of it, while the app runs.
	#[serde(skip)]
	pub version: ModelVersion,
}

/// Tells a model apart from one stored later under the same file name, and
/// one import of a model from the next, so work started on a model can tell
/// whether it is still the model it was started on. Not saved: every model
/// read from a manifest gets a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelVersion {
	/// The same for every import of one model; new when a model is added,
	/// including one added again after it was removed.
	model: u64,
	/// New on every import, including one that replaces the model.
	import: u64,
}

impl ModelVersion {
	fn next() -> u64 {
		static NEXT: AtomicU64 = AtomicU64::new(1);
		NEXT.fetch_add(1, Ordering::Relaxed)
	}

	/// A version for a model new to the workspace.
	pub fn new() -> Self {
		let n = Self::next();
		Self {
			model: n,
			import: n,
		}
	}

	/// The version of this model imported again.
	pub fn reimported(self) -> Self {
		Self {
			model: self.model,
			import: Self::next(),
		}
	}

	/// Whether both are imports of the same model.
	pub fn same_model(self, other: Self) -> bool {
		self.model == other.model
	}
}

impl Default for ModelVersion {
	fn default() -> Self {
		Self::new()
	}
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Manifest {
	pub version: u32,
	pub name: String,
	#[serde(deserialize_with = "budget::meshes")]
	pub items: Vec<Item>,
	#[serde(default, deserialize_with = "budget::models")]
	pub models: Vec<ModelItem>,
}

impl Manifest {
	/// Checks every path in a manifest read from disk before anything uses
	/// one (see [`check_path`]): item stems and their downloads are under
	/// `Models/`, model files under `Rigs/`, models' images under `Images/`,
	/// and folders are not too deep ([`check_folder`]). A manifest from
	/// someone else could otherwise name any file on the computer or the
	/// network. The error names the first bad entry.
	pub(super) fn validate(&self) -> Result<(), String> {
		let done = |state: &AssetState, top: &str| match state {
			AssetState::Done { path } => check_path(path, top),
			_ => Ok(()),
		};
		for (i, item) in self.items.iter().enumerate() {
			check_folder(&item.folder).map_err(|e| format!("items[{i}].folder {e}"))?;
			check_path(&item.stem, MODELS_DIR).map_err(|e| format!("items[{i}].stem {e}"))?;
			done(&item.mesh, MODELS_DIR).map_err(|e| format!("items[{i}].mesh {e}"))?;
			for (kind, map) in &item.maps {
				done(&map.state, MODELS_DIR)
					.map_err(|e| format!("items[{i}].maps.{kind:?} {e}"))?;
			}
			// version 1's texture, moved into `maps` by `upgrade`
			if let Some(state) = &item.texture {
				done(state, MODELS_DIR).map_err(|e| format!("items[{i}].texture {e}"))?;
			}
		}
		for (i, model) in self.models.iter().enumerate() {
			check_folder(&model.folder).map_err(|e| format!("models[{i}].folder {e}"))?;
			check_path(&model.file, RIGS_DIR).map_err(|e| format!("models[{i}].file {e}"))?;
			for stem in model.meshes.values() {
				check_path(stem, MODELS_DIR).map_err(|e| format!("models[{i}].meshes {e}"))?;
			}
			for state in model.images.values() {
				done(state, IMAGES_DIR).map_err(|e| format!("models[{i}].images {e}"))?;
			}
		}
		Ok(())
	}

	/// Brings a manifest read from `root` up to [`MANIFEST_VERSION`], and
	/// resets downloads whose files are missing, items' and models' images
	/// alike. One behind a link is not looked for (see [`saved_file`]) and
	/// is left as it is: whatever would read it refuses it, and it is not
	/// downloaded again, which could only be written through the link.
	/// Returns whether anything changed.
	pub(super) fn upgrade(&mut self, root: &Path) -> bool {
		let mut dirty = false;
		// downloads interrupted by closing the app, or files deleted outside
		// the app, are downloaded again
		let reset = |state: &mut AssetState| {
			let missing = matches!(&*state, AssetState::Done { path }
				if saved_file(root, path).is_ok_and(|file| !file.exists()));
			if missing {
				*state = AssetState::Pending;
			}
			missing
		};
		for item in &mut self.items {
			// version 1: a single optional texture
			if let (Some(id), Some(state)) = (item.texture_id.take(), item.texture.take()) {
				item.maps.insert(MapKind::Texture, MapRef { id, state });
				dirty = true;
			}
			dirty |= reset(&mut item.mesh);
			for map in item.maps.values_mut() {
				dirty |= reset(&mut map.state);
			}
		}
		// and so are the images models use (decals, MaterialVariant maps and
		// clothing)
		for model in &mut self.models {
			for state in model.images.values_mut() {
				dirty |= reset(state);
			}
		}
		if self.version != MANIFEST_VERSION {
			self.version = MANIFEST_VERSION;
			dirty = true;
		}
		dirty
	}
}

/// Reads a map of at most `cap` entries, stopping at the next one rather
/// than reading the rest: a model's mesh refs are each looked up when it is
/// shown, so a manifest from someone else could otherwise make that take
/// as long as it likes. `what` names the entries, as in "more than 10
/// {what}".
fn bounded_map<'de, D, K, V>(
	d: D,
	cap: usize,
	what: &'static str,
) -> Result<BTreeMap<K, V>, D::Error>
where
	D: serde::Deserializer<'de>,
	K: Deserialize<'de> + Ord,
	V: Deserialize<'de>,
{
	struct Bounded<K, V> {
		cap: usize,
		what: &'static str,
		kv: std::marker::PhantomData<(K, V)>,
	}
	impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> serde::de::Visitor<'de>
		for Bounded<K, V>
	{
		type Value = BTreeMap<K, V>;

		fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
			f.write_str("a map")
		}

		fn visit_map<A: serde::de::MapAccess<'de>>(
			self,
			mut map: A,
		) -> Result<Self::Value, A::Error> {
			let mut out = BTreeMap::new();
			// counted as read, so a key given again counts too
			let mut read = 0;
			while let Some(key) = map.next_key()? {
				if read == self.cap {
					return Err(serde::de::Error::custom(format!(
						"more than {} {}",
						self.cap, self.what
					)));
				}
				read += 1;
				out.insert(key, map.next_value()?);
			}
			Ok(out)
		}
	}
	d.deserialize_map(Bounded {
		cap,
		what,
		kv: std::marker::PhantomData,
	})
}

/// The most mesh refs one model may have: one for each part it may have.
pub(super) const MAX_MESH_REFS: usize = budget::LIMITS.parts;
/// The most images one model may use directly: as many as an export may
/// have meshes.
pub(super) const MAX_MODEL_IMAGES: usize = budget::LIMITS.meshes;

fn mesh_refs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<u32, String>, D::Error> {
	bounded_map(d, MAX_MESH_REFS, "mesh refs in one model")
}

fn model_images<'de, D: serde::Deserializer<'de>>(
	d: D,
) -> Result<BTreeMap<u64, AssetState>, D::Error> {
	bounded_map(d, MAX_MODEL_IMAGES, "images in one model")
}

#[cfg(test)]
mod tests {
	use super::*;

	/// R3-04: a map is read up to its cap, counting a key given twice, and
	/// refused at the next entry.
	#[test]
	fn bounded_maps_stop_at_their_cap() {
		let read = |json: &str| {
			let mut d = serde_json::Deserializer::from_str(json);
			bounded_map::<_, u32, String>(&mut d, 2, "refs")
		};
		assert_eq!(read(r#"{"1": "a", "2": "b"}"#).unwrap().len(), 2);
		let over = read(r#"{"1": "a", "2": "b", "3": "c"}"#).unwrap_err();
		assert!(over.to_string().contains("more than 2 refs"), "{over}");
		assert!(read(r#"{"1": "a", "1": "b", "1": "c"}"#).is_err());
	}
}
