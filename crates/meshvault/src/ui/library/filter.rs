//! Which rows the search, status chips and filters let through, and a
//! model's download status rolled up from its meshes.

use super::{Browser, Status, StatusFilter, status};
use crate::workspace::{AssetState, Item, ModelItem, Workspace};

/// The search rule both the library and the import dialog use for a mesh:
/// its name contains `needle`, or its id is exactly `needle`. `needle` is
/// already lowercased. Folders are matched by the callers where they count.
pub(in crate::ui) fn mesh_name_or_id_matches(needle: &str, name: &str, mesh_id: u64) -> bool {
	name.to_lowercase().contains(needle) || mesh_id.to_string() == needle
}

/// Whether any of `folder`'s names contains `needle` (already lowercased).
pub(in crate::ui) fn folder_matches(needle: &str, folder: &[String]) -> bool {
	folder.iter().any(|f| f.to_lowercase().contains(needle))
}

impl Browser {
	/// Lowercases the search once for this frame's rows. Called after the
	/// search field is drawn, so an edit counts in the same frame.
	pub(super) fn update_needle(&mut self) {
		self.needle = self.search.to_lowercase();
	}

	/// The items a model's meshes are, where they are still in the workspace.
	pub(super) fn model_items<'a>(
		&'a self,
		ws: &'a Workspace,
		model: &'a ModelItem,
	) -> impl Iterator<Item = (usize, &'a Item)> + 'a {
		model
			.meshes
			.values()
			.filter_map(|stem| self.stems.get(stem))
			.map(|&i| (i, &ws.items()[i]))
	}

	/// A model is done when every mesh it uses is in the workspace and
	/// downloaded, with its maps (so a model with no meshes is done); pending
	/// while any mesh is missing or any mesh or map is still to download;
	/// failed when any mesh or map failed, even if others are pending.
	pub(super) fn model_status(&self, ws: &Workspace, model: &ModelItem) -> Status {
		let mut s = if self.model_items(ws, model).count() < model.meshes.len() {
			Status::Pending
		} else {
			Status::Done
		};
		for (_, item) in self.model_items(ws, model) {
			match status(item) {
				Status::Failed => return Status::Failed,
				Status::Pending => s = Status::Pending,
				Status::Done => {}
			}
		}
		s
	}

	fn skinned(&self, item: &Item) -> bool {
		let AssetState::Done { path } = &item.mesh else {
			return false;
		};
		self.facts
			.as_ref()
			.and_then(|f| f.get(path, item.mesh_id))
			.is_some_and(|f| f.skinned)
	}

	fn has_map(&self, item: &Item) -> bool {
		self.filters.maps.iter().any(|k| item.maps.contains_key(k))
	}

	pub(super) fn matches(&self, item: &Item) -> bool {
		let status_ok = match self.status {
			StatusFilter::All => true,
			StatusFilter::Downloaded => item.is_done(),
			StatusFilter::Pending => item.has_pending() && !item.has_failure(),
			StatusFilter::Failed => item.has_failure(),
		};
		let f = &self.filters;
		if !status_ok
			|| f.animated
			|| (f.skinned && !self.skinned(item))
			|| (f.missing && matches!(item.mesh, AssetState::Done { .. }))
			|| (!f.maps.is_empty() && !self.has_map(item))
		{
			return false;
		}
		if self.search.is_empty() {
			return true;
		}
		mesh_name_or_id_matches(&self.needle, &item.name, item.mesh_id)
			|| folder_matches(&self.needle, &item.folder)
	}

	pub(super) fn model_matches(&self, ws: &Workspace, model: &ModelItem) -> bool {
		let s = self.model_status(ws, model);
		let status_ok = match self.status {
			StatusFilter::All => true,
			StatusFilter::Downloaded => s == Status::Done,
			StatusFilter::Pending => s == Status::Pending,
			StatusFilter::Failed => s == Status::Failed,
		};
		let f = &self.filters;
		let mut items = self.model_items(ws, model);
		if !status_ok
			|| (f.animated && model.playable_clips == 0)
			|| (f.skinned && !items.any(|(_, i)| self.skinned(i)))
			|| (f.missing && s == Status::Done)
			|| (!f.maps.is_empty() && !self.model_items(ws, model).any(|(_, i)| self.has_map(i)))
		{
			return false;
		}
		let needle = &self.needle;
		needle.is_empty()
			|| model.name.to_lowercase().contains(needle)
			|| model.path.to_lowercase().contains(needle)
	}

	pub(super) fn narrowed(&self) -> bool {
		!self.search.is_empty() || self.status != StatusFilter::All || self.filters.count() > 0
	}
}

#[cfg(test)]
mod tests {
	use std::collections::{BTreeSet, HashMap};

	use super::*;
	use crate::scene::maps::MapKind;
	use crate::test_support::TempDir;
	use crate::ui::library::Filters;
	use crate::ui::library::fixtures::{done, item};
	use crate::workspace::MapRef;

	fn failed() -> AssetState {
		AssetState::Failed {
			error: "nope".into(),
		}
	}

	fn with_map(mut item: Item, kind: MapKind, state: AssetState) -> Item {
		item.maps.insert(kind, MapRef { id: 99, state });
		item
	}

	fn model(name: &str, path: &str, stems: &[&str], playable_clips: usize) -> ModelItem {
		ModelItem {
			name: name.into(),
			path: path.into(),
			folder: Vec::new(),
			game: None,
			place_id: None,
			file: format!("Rigs/{name}.rig.json"),
			meshes: stems
				.iter()
				.enumerate()
				.map(|(i, s)| (i as u32 + 1, s.to_string()))
				.collect(),
			parts: stems.len(),
			clips: playable_clips,
			playable_clips,
			images: Default::default(),
			version: Default::default(),
		}
	}

	/// A workspace in a temp dir holding `items` and `models`, and a browser
	/// built for it. `facts` are (mesh path, mesh asset id, skinned),
	/// written to the facts cache first; with `None` the browser has no
	/// facts at all.
	fn fixture(
		items: Vec<Item>,
		models: Vec<ModelItem>,
		facts: Option<&[(&str, u64, bool)]>,
	) -> (TempDir, Workspace, Browser) {
		let dir = TempDir::new("library_ui");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.set_contents(items, models);
		if let Some(facts) = facts {
			let known: HashMap<&str, serde_json::Value> = facts
				.iter()
				.map(|&(path, mesh_id, skinned)| {
					let facts = serde_json::json!({
						"mesh_id": mesh_id,
						"skinned": skinned,
						"triangles": 1,
					});
					(path, facts)
				})
				.collect();
			let cache = dir.join(crate::workspace::CACHE_DIR);
			std::fs::create_dir_all(&cache).unwrap();
			std::fs::write(
				cache.join("facts.json"),
				serde_json::to_string(&known).unwrap(),
			)
			.unwrap();
		}
		let mut browser = Browser::default();
		browser.rebuild_if_needed(&ws);
		if facts.is_none() {
			browser.facts = None;
		}
		(dir, ws, browser)
	}

	/// Types `text` into the search, as the search field does.
	fn search(b: &mut Browser, text: &str) {
		b.search = text.into();
		b.update_needle();
	}

	fn shown_items<'a>(b: &Browser, ws: &'a Workspace) -> Vec<&'a str> {
		ws.items()
			.iter()
			.filter(|i| b.matches(i))
			.map(|i| i.name.as_str())
			.collect()
	}

	fn shown_models<'a>(b: &Browser, ws: &'a Workspace) -> Vec<&'a str> {
		ws.models()
			.iter()
			.filter(|m| b.model_matches(ws, m))
			.map(|m| m.name.as_str())
			.collect()
	}

	// -------------------------------------------------------------- filters

	#[test]
	fn filters_count_each_one_on_and_each_map() {
		let mut f = Filters::default();
		assert_eq!(f.count(), 0);
		f.animated = true;
		f.skinned = true;
		f.missing = true;
		f.maps = BTreeSet::from([MapKind::Normal, MapKind::Roughness]);
		assert_eq!(f.count(), 5);
	}

	#[test]
	fn an_item_is_failed_before_pending_and_its_maps_count() {
		let mesh_done = item("A", &[], 1, done("a.mesh"));
		assert!(status(&mesh_done) == Status::Done);
		let map_pending = with_map(mesh_done, MapKind::Normal, AssetState::Pending);
		assert!(status(&map_pending) == Status::Pending);
		let map_failed = with_map(
			item("B", &[], 2, AssetState::Pending),
			MapKind::Normal,
			failed(),
		);
		assert!(status(&map_failed) == Status::Failed);
	}

	#[test]
	fn the_status_filter_uses_the_item_and_its_maps() {
		let items = vec![
			item("Done", &[], 1, done("1.mesh")),
			with_map(
				item("MapPending", &[], 2, done("2.mesh")),
				MapKind::Color,
				AssetState::Pending,
			),
			item("Pending", &[], 3, AssetState::Pending),
			with_map(
				item("Both", &[], 4, AssetState::Pending),
				MapKind::Color,
				failed(),
			),
		];
		let (_dir, ws, mut b) = fixture(items, Vec::new(), None);
		assert!(!b.narrowed());
		assert_eq!(
			shown_items(&b, &ws),
			["Done", "MapPending", "Pending", "Both"]
		);
		b.status = StatusFilter::Downloaded;
		assert!(b.narrowed());
		assert_eq!(shown_items(&b, &ws), ["Done"]);
		b.status = StatusFilter::Pending;
		assert_eq!(shown_items(&b, &ws), ["MapPending", "Pending"]);
		b.status = StatusFilter::Failed;
		assert_eq!(shown_items(&b, &ws), ["Both"]);
	}

	#[test]
	fn search_finds_names_folders_and_exact_mesh_ids() {
		let items = vec![
			item("Shark Body", &["RS", "Animals"], 12345, done("1.mesh")),
			item("Rock", &["RS", "Props"], 678, done("2.mesh")),
		];
		let (_dir, ws, mut b) = fixture(items, Vec::new(), None);
		search(&mut b, "BODY");
		assert!(b.narrowed());
		assert_eq!(shown_items(&b, &ws), ["Shark Body"]);
		search(&mut b, "prop");
		assert_eq!(shown_items(&b, &ws), ["Rock"]);
		search(&mut b, "rs");
		assert_eq!(shown_items(&b, &ws), ["Shark Body", "Rock"]);
		search(&mut b, "12345");
		assert_eq!(shown_items(&b, &ws), ["Shark Body"]);
		// part of an id is not enough
		search(&mut b, "1234");
		assert!(shown_items(&b, &ws).is_empty());
	}

	#[test]
	fn missing_means_the_mesh_for_items_but_anything_for_models() {
		let items = vec![
			with_map(
				item("MapPending", &[], 1, done("1.mesh")),
				MapKind::Color,
				AssetState::Pending,
			),
			item("Pending", &[], 2, AssetState::Pending),
			item("Done", &[], 3, done("3.mesh")),
		];
		let models = vec![
			model("UsesMapPending", "RS/A", &["MapPending"], 1),
			model("UsesDone", "RS/B", &["Done"], 1),
		];
		let (_dir, ws, mut b) = fixture(items, models, None);
		b.filters.missing = true;
		assert!(b.narrowed());
		// a mesh whose map is still pending is not missing...
		assert_eq!(shown_items(&b, &ws), ["Pending"]);
		// ...but a model using it is
		assert_eq!(shown_models(&b, &ws), ["UsesMapPending"]);
	}

	#[test]
	fn the_animated_filter_hides_meshes_and_still_models() {
		let items = vec![item("A", &[], 1, done("1.mesh"))];
		let models = vec![
			model("Moves", "RS/M", &["A"], 2),
			model("Still", "RS/S", &["A"], 0),
		];
		let (_dir, ws, mut b) = fixture(items, models, None);
		b.filters.animated = true;
		assert!(shown_items(&b, &ws).is_empty());
		assert_eq!(shown_models(&b, &ws), ["Moves"]);
	}

	#[test]
	fn map_filters_match_any_of_the_kinds_chosen() {
		let items = vec![
			with_map(
				item("Rough", &[], 1, done("1.mesh")),
				MapKind::Roughness,
				AssetState::Pending,
			),
			with_map(
				item("Textured", &[], 2, done("2.mesh")),
				MapKind::Texture,
				done("2.png"),
			),
			item("Plain", &[], 3, done("3.mesh")),
		];
		let models = vec![
			model("UsesRough", "RS/R", &["Plain", "Rough"], 1),
			model("UsesTextured", "RS/T", &["Textured"], 1),
		];
		let (_dir, ws, mut b) = fixture(items, models, None);
		b.filters.maps = BTreeSet::from([MapKind::Normal, MapKind::Roughness]);
		// a map not downloaded yet still counts
		assert_eq!(shown_items(&b, &ws), ["Rough"]);
		assert_eq!(shown_models(&b, &ws), ["UsesRough"]);
	}

	#[test]
	fn models_are_searched_by_name_and_path_only() {
		let items = vec![item("Fin", &["Sea"], 42, done("1.mesh"))];
		let models = vec![model("Shark", "Workspace/Animals/Shark", &["Sea/Fin"], 1)];
		let (_dir, ws, mut b) = fixture(items, models, None);
		search(&mut b, "SHARK");
		assert_eq!(shown_models(&b, &ws), ["Shark"]);
		search(&mut b, "animals");
		assert_eq!(shown_models(&b, &ws), ["Shark"]);
		// not by the meshes it uses
		search(&mut b, "fin");
		assert!(shown_models(&b, &ws).is_empty());
		search(&mut b, "42");
		assert!(shown_models(&b, &ws).is_empty());
	}

	#[test]
	fn the_status_filter_uses_the_model_status() {
		let items = vec![
			item("Bad", &[], 1, failed()),
			item("Good", &[], 2, done("2.mesh")),
		];
		let models = vec![
			model("Broken", "RS/B", &["Bad", "Good"], 1),
			model("Fine", "RS/F", &["Good"], 1),
		];
		let (_dir, ws, mut b) = fixture(items, models, None);
		assert_eq!(shown_models(&b, &ws), ["Broken", "Fine"]);
		b.status = StatusFilter::Failed;
		assert_eq!(shown_models(&b, &ws), ["Broken"]);
		b.status = StatusFilter::Downloaded;
		assert_eq!(shown_models(&b, &ws), ["Fine"]);
		b.status = StatusFilter::Pending;
		assert!(shown_models(&b, &ws).is_empty());
	}

	// --------------------------------------------------------- model status

	#[test]
	fn model_status_rolls_up_its_meshes_and_their_maps() {
		let items = vec![
			item("Done", &[], 1, done("1.mesh")),
			with_map(
				item("MapPending", &[], 2, done("2.mesh")),
				MapKind::Normal,
				AssetState::Pending,
			),
			with_map(
				item("MapFailed", &[], 3, done("3.mesh")),
				MapKind::Normal,
				failed(),
			),
			item("Pending", &[], 4, AssetState::Pending),
		];
		let models = vec![
			model("NoMeshes", "RS/0", &[], 0),
			model("AllDone", "RS/1", &["Done"], 0),
			model("Twice", "RS/2", &["Done", "Done"], 0),
			model("Gone", "RS/3", &["Done", "NotThere"], 0),
			model("MapPending", "RS/4", &["Done", "MapPending"], 0),
			model("MapFailed", "RS/5", &["Done", "MapFailed"], 0),
			model("FailedFirst", "RS/6", &["MapFailed", "Pending"], 0),
			model("FailedLast", "RS/7", &["Pending", "MapFailed"], 0),
			model("GoneAndFailed", "RS/8", &["NotThere", "MapFailed"], 0),
		];
		let (_dir, ws, b) = fixture(items, models, None);
		let expected = [
			("NoMeshes", Status::Done),
			("AllDone", Status::Done),
			("Twice", Status::Done),
			("Gone", Status::Pending),
			("MapPending", Status::Pending),
			("MapFailed", Status::Failed),
			("FailedFirst", Status::Failed),
			("FailedLast", Status::Failed),
			("GoneAndFailed", Status::Failed),
		];
		assert_eq!(ws.models().len(), expected.len());
		for (m, (name, want)) in ws.models().iter().zip(expected) {
			assert_eq!(m.name, name);
			assert!(b.model_status(&ws, m) == want, "{name}");
		}
	}

	// -------------------------------------------------------------- skinned

	#[test]
	fn the_skinned_filter_reads_the_facts() {
		let items = vec![
			item("Skinned", &[], 1, done("Models/Skinned.mesh")),
			item("Rigid", &[], 2, done("Models/Rigid.mesh")),
			item("Unread", &[], 3, done("Models/Unread.mesh")),
			// there are facts for its path, but it is not downloaded
			item("Pending", &[], 4, AssetState::Pending),
		];
		let models = vec![
			model("Mixed", "RS/M", &["Rigid", "Skinned"], 1),
			model("Stiff", "RS/S", &["Rigid", "Unread", "Pending"], 1),
		];
		let facts = [
			("Models/Skinned.mesh", 1, true),
			("Models/Rigid.mesh", 2, false),
			("Models/Pending.mesh", 4, true),
		];
		let (_dir, ws, mut b) = fixture(items, models, Some(&facts));
		let skinned: Vec<&str> = ws
			.items()
			.iter()
			.filter(|i| b.skinned(i))
			.map(|i| i.name.as_str())
			.collect();
		assert_eq!(skinned, ["Skinned"]);
		b.filters.skinned = true;
		assert_eq!(shown_items(&b, &ws), ["Skinned"]);
		assert_eq!(shown_models(&b, &ws), ["Mixed"]);
	}

	#[test]
	fn nothing_is_skinned_without_facts() {
		let items = vec![item("A", &[], 1, done("Models/A.mesh"))];
		let models = vec![model("M", "RS/M", &["A"], 1)];
		let (_dir, ws, mut b) = fixture(items, models, None);
		assert!(b.facts.is_none());
		assert!(!b.skinned(&ws.items()[0]));
		b.filters.skinned = true;
		assert!(shown_items(&b, &ws).is_empty());
		assert!(shown_models(&b, &ws).is_empty());
	}
}
