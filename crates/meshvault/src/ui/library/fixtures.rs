//! Workspace items for the library's tests.

use crate::workspace::{AssetState, Item, ModelItem};

pub(super) fn done(path: &str) -> AssetState {
	AssetState::Done { path: path.into() }
}

/// A mesh named `name` in `folder`, its stem made from both.
pub(super) fn item(name: &str, folder: &[&str], mesh_id: u64, mesh: AssetState) -> Item {
	let mut stem: Vec<&str> = folder.to_vec();
	stem.push(name);
	serde_json::from_value(serde_json::json!({
		"name": name,
		"folder": folder,
		"mesh_id": mesh_id,
		"stem": stem.join("/"),
		"mesh": mesh,
	}))
	.unwrap()
}

/// An animated model named `name`, imported from `game`.
pub(super) fn model(name: &str, game: Option<&str>) -> ModelItem {
	serde_json::from_value(serde_json::json!({
		"name": name,
		"path": format!("Workspace/{name}"),
		"folder": [game.unwrap_or("Workspace")],
		"game": game,
		"file": format!("Rigs/{name}.rig.json"),
		"meshes": {},
		"parts": 1,
		"clips": 0,
		"playable_clips": 0,
	}))
	.unwrap()
}
