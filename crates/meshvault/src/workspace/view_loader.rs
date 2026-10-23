//! Turning workspace entries into what the viewer opens: each mesh's
//! downloaded maps, and a loader for an animated model that snapshots the
//! workspace so it can run on a background thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui;

use super::{AssetKind, AssetState, Item, ModelItem, Workspace, saved_file};
use crate::scene::maps::Slot;
use crate::scene::model_view::{MeshSource, ModelView};
use crate::studio::export_file;

/// Reads the workspace's model `index` with its downloaded meshes, maps
/// and decal images. Slow, so the returned closure runs on a background
/// thread. `None` if the workspace has no model `index`. Nothing behind a
/// link in the workspace is read (see [`Workspace::saved_file`]): a rig
/// there fails to load with the reason, a mesh there is left out with it,
/// and maps and images there are left out.
pub fn model_loader(
	ws: &Workspace,
	index: usize,
) -> Option<impl FnOnce(&egui::Context) -> Result<ModelView, String> + Send + 'static> {
	let item = ws.models().get(index)?;
	let sources = mesh_sources(ws, item);
	let images = decal_images(ws, item);
	let file = ws.saved_file(&item.file);
	Some(move |ctx: &egui::Context| {
		let file = file?;
		export_file::read_text(&file)
			.and_then(|text| export_file::parse(&text))
			.and_then(|export| {
				export
					.models
					.into_iter()
					.next()
					.ok_or_else(|| format!("{} holds no model", file.display()))
			})
			.map(|model| ModelView::load(ctx, index, &model, &sources, &images))
	})
}

/// `model`'s decal images that are downloaded, but not those behind a link.
fn decal_images(ws: &Workspace, model: &ModelItem) -> HashMap<u64, PathBuf> {
	model
		.images
		.iter()
		.filter_map(|(&id, state)| Some((id, ws.saved_file(state.done_path()?).ok()?)))
		.collect()
}

/// Where each of `model`'s meshes is read from, by mesh ref.
fn mesh_sources(ws: &Workspace, model: &ModelItem) -> HashMap<u32, MeshSource> {
	model
		.meshes
		.iter()
		.map(|(&mesh_ref, stem)| {
			let source = match ws.item_by_stem(stem) {
				Some(i) => MeshSource {
					name: i.name.clone(),
					mesh: match &i.mesh {
						AssetState::Done { path } => ws.saved_file(path),
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
		.collect()
}

/// Each material slot's downloaded map: the first that feeds it (ColorMap
/// over TextureID). One behind a link (see [`saved_file`]) is left out.
pub fn item_maps(root: &Path, item: &Item) -> Vec<(Slot, PathBuf)> {
	Slot::ALL
		.iter()
		.filter_map(|&slot| {
			let path = slot
				.sources()
				.iter()
				.find_map(|&k| item.done_path(AssetKind::Map(k)))?;
			Some((slot, saved_file(root, path).ok()?))
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::TempDir;
	use crate::workspace::{ITEMS_VISITED, MANIFEST};

	/// R3-04: a model's mesh refs were each found by looking through every
	/// item, on the UI thread: 100,000 items and a model with 100,000 refs
	/// took ten billion comparisons before the model started loading. Now
	/// each is found through an index of the stems, so the work is linear;
	/// and refs to the same stem, or to one no longer there, resolve as
	/// they did.
	#[test]
	fn a_models_meshes_are_found_with_linear_work() {
		let dir = TempDir::new("loader_linear");
		Workspace::create(&dir, "Test").unwrap();
		const N: usize = 20_000;
		let mut items: Vec<_> = (0..N)
			.map(|i| {
				serde_json::json!({
					"name": format!("I{i}"), "folder": [], "mesh_id": 1,
					"stem": format!("Models/I{i}"), "mesh": AssetState::Pending,
				})
			})
			.collect();
		items.push(serde_json::json!({
			"name": "A", "folder": [], "mesh_id": 1, "stem": "Models/A",
			"mesh": {"state": "done", "path": "Models/A.mesh"},
		}));
		let mut meshes = serde_json::Map::new();
		for i in 0..N {
			meshes.insert(i.to_string(), "Models/Missing".into());
		}
		meshes.insert((N).to_string(), "Models/A".into());
		meshes.insert((N + 1).to_string(), "Models/A".into());
		meshes.insert((N + 2).to_string(), "Models/I7".into());
		let manifest = serde_json::json!({
			"version": 3, "name": "Shared", "items": items,
			"models": [{
				"name": "M", "path": "Workspace/M", "folder": [], "file": "Rigs/M.rig.json",
				"meshes": meshes, "parts": N + 3, "clips": 0, "playable_clips": 0,
			}],
		});
		std::fs::create_dir_all(dir.join("Models")).unwrap();
		std::fs::write(
			dir.join("Models/A.mesh"),
			b"version 4.00
",
		)
		.unwrap();
		std::fs::write(dir.join(MANIFEST), manifest.to_string()).unwrap();
		let ws = Workspace::open(&dir).unwrap();

		ITEMS_VISITED.with(|n| n.set(0));
		let sources = mesh_sources(&ws, &ws.models()[0]);
		let visited = ITEMS_VISITED.with(|n| n.get());
		assert!(visited <= 2 * (N + 1), "{visited} items looked at");
		assert_eq!(sources.len(), N + 3);
		for r in [N, N + 1] {
			assert_eq!(sources[&(r as u32)].name, "A");
			assert_eq!(sources[&(r as u32)].mesh, Ok(dir.join("Models/A.mesh")));
		}
		assert_eq!(sources[&(N as u32 + 2)].name, "I7");
		assert_eq!(
			sources[&(N as u32 + 2)].mesh,
			Err("not downloaded yet".into())
		);
		assert_eq!(sources[&0].name, "Models/Missing");
		assert_eq!(sources[&0].mesh, Err("removed from the workspace".into()));
		assert!(model_loader(&ws, 0).is_some());
	}

	/// R4-03: a model's rig, meshes, maps and decal images were read
	/// through any link below the workspace's own folders. Now each is
	/// refused there, with the reason, and nothing behind the link is read.
	#[test]
	fn nothing_is_read_behind_a_link() {
		let parent = TempDir::new("loader_linked");
		let (dir, outside) = (parent.join("ws"), parent.join("outside"));
		std::fs::create_dir_all(&outside).unwrap();
		for file in ["A.mesh", "A.png", "7.png", "M.rig.json"] {
			std::fs::write(outside.join(file), b"keep me").unwrap();
		}
		Workspace::create(&dir, "Test").unwrap();
		let done = |path: &str| serde_json::json!({"state": "done", "path": path});
		let manifest = serde_json::json!({
			"version": 3, "name": "Shared",
			"items": [{
				"name": "A", "folder": [], "mesh_id": 1, "stem": "Models/External/A",
				"mesh": done("Models/External/A.mesh"),
				"maps": {"color": {"id": 2, "state": done("Models/External/A.png")}},
			}],
			"models": [{
				"name": "M", "path": "Workspace/M", "folder": [],
				"file": "Rigs/External/M.rig.json", "meshes": {"1": "Models/External/A"},
				"parts": 1, "clips": 0, "playable_clips": 0,
				"images": {"7": done("Images/External/7.png")},
			}],
		});
		std::fs::write(dir.join(MANIFEST), manifest.to_string()).unwrap();
		for folder in ["Models", "Rigs", "Images"] {
			std::fs::create_dir_all(dir.join(folder)).unwrap();
			if !crate::workspace::junction(&dir.join(folder).join("External"), &outside) {
				return;
			}
		}
		let ws = Workspace::open(&dir).unwrap();
		let model = &ws.models()[0];
		let sources = mesh_sources(&ws, model);
		let error = sources[&1].mesh.clone().unwrap_err();
		assert!(
			error.starts_with("Models/External in the workspace is a link"),
			"{error}"
		);
		assert!(sources[&1].maps.is_empty());
		assert!(item_maps(&ws.root, &ws.items()[0]).is_empty());
		assert!(decal_images(&ws, model).is_empty());
		let load = model_loader(&ws, 0).unwrap();
		let Err(error) = load(&egui::Context::default()) else {
			panic!("the model loaded");
		};
		assert!(
			error.starts_with("Rigs/External in the workspace is a link"),
			"{error}"
		);
	}
}
