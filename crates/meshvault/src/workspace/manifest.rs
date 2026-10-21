//! The manifest's serialised types: items, models and their download
//! states, and the upgrade of older manifests on open.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::scene::maps::MapKind;
use crate::workspace::AssetKind;

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
pub(super) struct Manifest {
	pub version: u32,
	pub name: String,
	pub items: Vec<Item>,
	#[serde(default)]
	pub models: Vec<ModelItem>,
}

impl Manifest {
	/// Brings a manifest read from `root` up to [`MANIFEST_VERSION`], and
	/// resets downloads whose files are missing, items' and models' images
	/// alike. Returns whether anything changed.
	pub(super) fn upgrade(&mut self, root: &Path) -> bool {
		let mut dirty = false;
		// downloads interrupted by closing the app, or files deleted outside
		// the app, are downloaded again
		let reset = |state: &mut AssetState| {
			let missing = matches!(&*state, AssetState::Done { path } if !root.join(path).exists());
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
