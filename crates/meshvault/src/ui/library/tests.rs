//! Tests for the library's selection, filters and grid sections.

use super::grid::grid_sections;
use super::*;
use crate::test_support::TempDir;
use crate::ui::icons;
use crate::workspace::{AssetState, MapRef, ModelItem};

fn done(path: &str) -> AssetState {
	AssetState::Done { path: path.into() }
}

fn failed() -> AssetState {
	AssetState::Failed {
		error: "nope".into(),
	}
}

/// A mesh named `name` in `folder`, its stem made from both.
fn item(name: &str, folder: &[&str], mesh_id: u64, mesh: AssetState) -> Item {
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

fn with_map(mut item: Item, kind: MapKind, state: AssetState) -> Item {
	item.maps.insert(kind, MapRef { id: 99, state });
	item
}

fn model(name: &str, path: &str, stems: &[&str], playable_clips: usize) -> ModelItem {
	ModelItem {
		name: name.into(),
		path: path.into(),
		folder: Vec::new(),
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
	}
}

/// A workspace in a temp dir holding `items` and `models`, and a browser
/// built for it. `facts` are (mesh path, skinned), written to the facts
/// cache first; with `None` the browser has no facts at all.
fn fixture(
	items: Vec<Item>,
	models: Vec<ModelItem>,
	facts: Option<&[(&str, bool)]>,
) -> (TempDir, Workspace, Browser) {
	let dir = TempDir::new("library_ui");
	let mut ws = Workspace::create(&dir, "Test").unwrap();
	ws.set_contents(items, models);
	if let Some(facts) = facts {
		let known: HashMap<&str, serde_json::Value> = facts
			.iter()
			.map(|&(path, skinned)| {
				(
					path,
					serde_json::json!({"skinned": skinned, "triangles": 1}),
				)
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

// ------------------------------------------------------------ selection

const M0: Key = Key::Model(0);
const I0: Key = Key::Item(0);
const I1: Key = Key::Item(1);
const I2: Key = Key::Item(2);
const I3: Key = Key::Item(3);

/// A selection of `keys` with rows M0, I0, I1, I2 drawn last frame.
fn selection(keys: &[Key], anchor: Option<Key>) -> Selection {
	Selection {
		keys: keys.iter().copied().collect(),
		anchor,
		drawn: Vec::new(),
		order: vec![M0, I0, I1, I2],
	}
}

fn keys(s: &Selection) -> Vec<Key> {
	s.keys.iter().copied().collect()
}

#[test]
fn a_plain_click_selects_only_that_row_and_opens_it() {
	let mut s = selection(&[M0, I1], Some(I1));
	assert_eq!(s.click(I0, false, false), Some(I0));
	assert_eq!(keys(&s), [I0]);
	assert_eq!(s.anchor, Some(I0));
	// clicking the only selected row opens it again
	assert_eq!(s.click(I0, false, false), Some(I0));
	assert_eq!(keys(&s), [I0]);
}

#[test]
fn ctrl_toggles_a_row_and_moves_the_anchor() {
	let mut s = selection(&[I0], Some(I0));
	assert_eq!(s.click(I2, false, true), None);
	assert_eq!(keys(&s), [I0, I2]);
	assert_eq!(s.anchor, Some(I2));
	assert_eq!(s.click(I0, false, true), None);
	assert_eq!(keys(&s), [I2]);
	assert_eq!(s.anchor, Some(I0));
	// taking the last one away leaves nothing selected
	assert_eq!(s.click(I2, false, true), None);
	assert!(s.keys.is_empty());
	assert_eq!(s.anchor, Some(I2));
}

#[test]
fn shift_selects_the_range_from_the_anchor_and_keeps_it() {
	let mut s = selection(&[M0], Some(I0));
	assert_eq!(s.click(I2, true, false), None);
	assert_eq!(keys(&s), [I0, I1, I2]);
	assert_eq!(s.anchor, Some(I0));
	// a second Shift-click replaces the range, from the same anchor
	assert_eq!(s.click(I1, true, false), None);
	assert_eq!(keys(&s), [I0, I1]);
	assert_eq!(s.anchor, Some(I0));
}

#[test]
fn shift_ranges_run_backwards_too() {
	let mut s = selection(&[], Some(I2));
	assert_eq!(s.click(M0, true, false), None);
	assert_eq!(keys(&s), [M0, I0, I1, I2]);
	assert_eq!(s.anchor, Some(I2));
	// the anchor alone
	assert_eq!(s.click(I2, true, false), None);
	assert_eq!(keys(&s), [I2]);
}

#[test]
fn shift_ctrl_adds_the_range_to_the_selection() {
	let mut s = selection(&[M0], Some(I2));
	assert_eq!(s.click(I1, true, true), None);
	assert_eq!(keys(&s), [M0, I1, I2]);
	assert_eq!(s.anchor, Some(I2));
}

#[test]
fn shift_without_an_anchor_is_a_plain_or_ctrl_click() {
	let mut s = selection(&[M0], None);
	assert_eq!(s.click(I1, true, false), Some(I1));
	assert_eq!(keys(&s), [I1]);
	assert_eq!(s.anchor, Some(I1));

	let mut s = selection(&[M0], None);
	assert_eq!(s.click(I1, true, true), None);
	assert_eq!(keys(&s), [M0, I1]);
	assert_eq!(s.anchor, Some(I1));
}

#[test]
fn shift_with_a_row_not_drawn_is_a_plain_or_ctrl_click() {
	// the clicked row is not in `order`
	let mut s = selection(&[M0], Some(I0));
	assert_eq!(s.click(I3, true, false), Some(I3));
	assert_eq!(keys(&s), [I3]);
	assert_eq!(s.anchor, Some(I3));
	// the anchor is not in `order` any more
	let mut s = selection(&[M0], Some(I3));
	assert_eq!(s.click(I1, true, true), None);
	assert_eq!(keys(&s), [M0, I1]);
	assert_eq!(s.anchor, Some(I1));
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
	b.filter = "BODY".into();
	assert!(b.narrowed());
	assert_eq!(shown_items(&b, &ws), ["Shark Body"]);
	b.filter = "prop".into();
	assert_eq!(shown_items(&b, &ws), ["Rock"]);
	b.filter = "rs".into();
	assert_eq!(shown_items(&b, &ws), ["Shark Body", "Rock"]);
	b.filter = "12345".into();
	assert_eq!(shown_items(&b, &ws), ["Shark Body"]);
	// part of an id is not enough
	b.filter = "1234".into();
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
	b.filter = "SHARK".into();
	assert_eq!(shown_models(&b, &ws), ["Shark"]);
	b.filter = "animals".into();
	assert_eq!(shown_models(&b, &ws), ["Shark"]);
	// not by the meshes it uses
	b.filter = "fin".into();
	assert!(shown_models(&b, &ws).is_empty());
	b.filter = "42".into();
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
		("Models/Skinned.mesh", true),
		("Models/Rigid.mesh", false),
		("Models/Pending.mesh", true),
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

// ----------------------------------------------------------------- grid

fn folder_title(name: &str) -> String {
	format!("{}  {name}", icons::FOLDER_SIMPLE)
}

fn grid_items() -> Vec<Item> {
	vec![
		item("Rock", &["Props"], 1, done("1.mesh")),
		item("Loose", &[], 2, done("2.mesh")),
		item("Fish", &["Sea"], 3, done("3.mesh")),
		item("apple", &["Props"], 4, done("4.mesh")),
		item("Lamp", &["Props", "Indoor"], 5, done("5.mesh")),
		item("Zed", &["Props"], 6, done("6.mesh")),
	]
}

#[test]
fn grid_meshes_group_by_folder_in_order() {
	let items = grid_items();
	let (sections, paths) = grid_sections(&items, Vec::new(), (0..items.len()).collect());
	assert_eq!(paths, ["", "Props", "Props / Indoor", "Sea"]);
	assert_eq!(
		sections,
		[
			(folder_title(""), vec![Key::Item(1)]),
			// names sort without regard to case
			(
				folder_title("Props"),
				vec![Key::Item(3), Key::Item(0), Key::Item(5)]
			),
			(folder_title("Indoor"), vec![Key::Item(4)]),
			(folder_title("Sea"), vec![Key::Item(2)]),
		]
	);
}

#[test]
fn grid_puts_models_first_and_root_meshes_with_them_today() {
	// Bug B32: root-level meshes share the models' empty hover path, so
	// they land in the "Animated models" section. Flip this when it is
	// fixed.
	let items = grid_items();
	let (sections, paths) =
		grid_sections(&items, vec![Key::Model(1), Key::Model(0)], vec![5, 1, 2]);
	assert_eq!(paths, ["", "Props", "Sea"]);
	assert_eq!(
		sections,
		[
			(
				format!("{}  Animated models", icons::PERSON_SIMPLE_RUN),
				vec![Key::Model(1), Key::Model(0), Key::Item(1)],
			),
			(folder_title("Props"), vec![Key::Item(5)]),
			(folder_title("Sea"), vec![Key::Item(2)]),
		]
	);
}

#[test]
fn an_empty_grid_has_no_sections() {
	let (sections, paths) = grid_sections(&grid_items(), Vec::new(), Vec::new());
	assert!(sections.is_empty() && paths.is_empty());
}
