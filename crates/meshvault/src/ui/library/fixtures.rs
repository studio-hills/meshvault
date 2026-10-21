//! Workspace items for the library's tests.

use crate::workspace::{AssetState, Item};

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
