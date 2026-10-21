//! Turning workspace entries into what the viewer opens: each mesh's
//! downloaded maps, and a loader for an animated model that snapshots the
//! workspace so it can run on a background thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui;

use super::{AssetKind, AssetState, Item, Workspace};
use crate::scene::maps::Slot;
use crate::scene::model_view::{MeshSource, ModelView};
use crate::studio::export_file;

/// Reads the workspace's model `index` with its downloaded meshes, maps
/// and decal images. Slow, so the returned closure runs on a background
/// thread.
pub fn model_loader(
	ws: &Workspace,
	index: usize,
) -> impl FnOnce(&egui::Context) -> Result<ModelView, String> + Send + 'static {
	let item = &ws.models()[index];
	let sources: HashMap<u32, MeshSource> = item
		.meshes
		.iter()
		.map(|(&mesh_ref, stem)| {
			let found = ws.items().iter().find(|i| &i.stem == stem);
			let source = match found {
				Some(i) => MeshSource {
					name: i.name.clone(),
					mesh: match &i.mesh {
						AssetState::Done { path } => Ok(ws.root.join(path)),
						AssetState::Pending => Err("not downloaded yet".into()),
						AssetState::Failed { error } => Err(format!("download failed: {error}")),
					},
					maps: item_maps(&ws.root, i),
				},
				None => MeshSource {
					name: stem.clone(),
					mesh: Err("removed from the workspace".into()),
					maps: Vec::new(),
				},
			};
			(mesh_ref, source)
		})
		.collect();
	// decal images that are downloaded
	let images: HashMap<u64, PathBuf> = item
		.images
		.iter()
		.filter_map(|(&id, state)| match state {
			AssetState::Done { path } => Some((id, ws.root.join(path))),
			_ => None,
		})
		.collect();
	let file = ws.root.join(&item.file);
	move |ctx: &egui::Context| {
		std::fs::read_to_string(&file)
			.map_err(|e| format!("Could not read {}: {e}", file.display()))
			.and_then(|text| export_file::parse(&text))
			.and_then(|export| {
				export
					.models
					.into_iter()
					.next()
					.ok_or_else(|| format!("{} holds no model", file.display()))
			})
			.map(|model| ModelView::load(ctx, index, &model, &sources, &images))
	}
}

/// Each material slot's downloaded map: the first that feeds it (ColorMap
/// over TextureID).
pub fn item_maps(root: &Path, item: &Item) -> Vec<(Slot, PathBuf)> {
	Slot::ALL
		.iter()
		.filter_map(|&slot| {
			let path = slot
				.sources()
				.iter()
				.find_map(|&k| item.done_path(AssetKind::Map(k)))?;
			Some((slot, root.join(path)))
		})
		.collect()
}
