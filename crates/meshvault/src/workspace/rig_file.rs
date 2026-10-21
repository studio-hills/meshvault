//! Rig files: an animated model written as an export of its own.

use std::path::{Path, PathBuf};

use crate::anim::schema::AnimatedModel;
use crate::scene::maps::MapKind;
use crate::studio::export_file::{self, Entry};

/// Writes a model as an export in the current schema
/// ([`export_file::SCHEMA_VERSION`]) holding just that model and the meshes
/// it uses, so the file can also be imported on its own. Workspaces made
/// before schema 2 hold schema 1 rig files; the export reader still accepts
/// those, so they load as they are.
pub(super) fn write_rig(
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
	// write then rename, so a failed write never damages the rig file a
	// model being imported again already has
	let mut tmp = path.as_os_str().to_owned();
	tmp.push(".tmp");
	let tmp = PathBuf::from(tmp);
	let result = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, path));
	if let Err(e) = result {
		let _ = std::fs::remove_file(&tmp);
		return Err(format!("Could not write {}: {e}", path.display()));
	}
	Ok(())
}
