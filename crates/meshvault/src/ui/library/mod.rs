//! The library: imported models with their download status, as
//! a folder tree or a grid of thumbnails, with search, filters and
//! actions on several models at once.

mod filter;
#[cfg(test)]
mod fixtures;
mod grid;
mod list;
mod menus;
mod selection;
mod toolbar;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText, Stroke};

pub(super) use self::filter::{folder_matches, mesh_name_or_id_matches};
use self::list::nothing_matches;
use crate::export::Format;
use crate::render::gpu::GpuRenderer;
use crate::render::thumbnails::Thumbnails;
use crate::scene::maps::MapKind;
use crate::ui::theme::{Palette, ROW, palette};
use crate::ui::tree::{self, FolderCommand, FolderTree};
use crate::ui::widgets;
use crate::workspace::facts::Facts;
use crate::workspace::{Item, Workspace};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum StatusFilter {
	#[default]
	All,
	Downloaded,
	Pending,
	Failed,
}

pub enum Action {
	OpenItem(usize),
	OpenModel(usize),
	RemoveModels(Vec<usize>),
	Retry(Vec<usize>),
	RemoveItems(Vec<usize>),
	Reveal(PathBuf),
	/// Removes meshes and animated models together (a selection).
	RemoveSelection {
		items: Vec<usize>,
		models: Vec<usize>,
	},
	/// Exports meshes and animated models as files.
	Export {
		items: Vec<usize>,
		models: Vec<usize>,
		format: Format,
	},
}

/// How the library lays out its models.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum View {
	/// The export's folder tree, one row per model.
	#[default]
	List,
	/// Thumbnails, grouped by folder.
	Grid,
}

/// Narrowing beyond the search text and download status. Each one that is
/// on must hold.
#[derive(Clone, Default, PartialEq)]
struct Filters {
	/// Animated models with a clip that plays (meshes have none).
	animated: bool,
	/// Meshes bound to bones, and models using one.
	skinned: bool,
	/// Meshes not downloaded, and models with one.
	missing: bool,
	/// Meshes with any of these maps, and models using one.
	maps: BTreeSet<MapKind>,
}

impl Filters {
	fn count(&self) -> usize {
		self.animated as usize + self.skinned as usize + self.missing as usize + self.maps.len()
	}
}

/// A row (or tile) of the library.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Row {
	/// An animated model, by index into the workspace's models.
	Model(usize),
	/// A mesh, by index into the workspace's items.
	Mesh(usize),
}

/// The library's selected rows, with what Shift-click needs to select a
/// range of them.
#[derive(Default)]
struct Selection {
	keys: BTreeSet<Row>,
	/// The row a Shift-click selects from.
	anchor: Option<Row>,
	/// Rows in the order they are drawn, gathered during this frame.
	drawn: Vec<Row>,
	/// Rows in the order the previous frame drew them; clicks, handled
	/// while this frame draws, select ranges from this complete list.
	order: Vec<Row>,
}

#[derive(Default)]
pub struct Browser {
	search: String,
	/// `search` lowercased, for matching this frame's rows.
	needle: String,
	status: StatusFilter,
	filters: Filters,
	view: View,
	tree: FolderTree,
	/// Item index by stem.
	stems: HashMap<String, usize>,
	/// Workspace and generation the tree was built from.
	built_for: Option<(PathBuf, u64)>,
	/// Expand/collapse requested while drawing, applied on the next frame.
	pending: RefCell<Option<FolderCommand>>,
	/// Open the folders leading to the open mesh or animated model and
	/// scroll to it.
	reveal: bool,
	scroll_to_open: std::cell::Cell<bool>,
	selection: RefCell<Selection>,
	thumbnails: Option<Thumbnails>,
	facts: Option<Facts>,
}

/// What the viewer has open, so its row or tile can be marked: a mesh by
/// its stem, an animated model by its index. Each is checked on its own.
#[derive(Clone, Copy)]
struct Opened<'a> {
	stem: Option<&'a str>,
	model: Option<usize>,
}

impl Opened<'_> {
	fn is_item(&self, item: &Item) -> bool {
		self.stem == Some(item.stem.as_str())
	}

	fn is_model(&self, i: usize) -> bool {
		self.model == Some(i)
	}
}

/// Folder state namespace for the library tree.
const SCOPE: &str = "library";

/// Applies an expand or collapse to the library's folders. "All" includes
/// the animated models' folder, which is not in `tree`.
fn apply_folder_command(ctx: &egui::Context, command: &FolderCommand, tree: &FolderTree) {
	command.apply(ctx, SCOPE, tree);
	if let FolderCommand::All(open) = *command {
		tree::set_open(ctx, list::models_folder_id(), open);
	}
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
	Done,
	Pending,
	Failed,
}

fn status(item: &Item) -> Status {
	if item.has_failure() {
		Status::Failed
	} else if item.is_done() {
		Status::Done
	} else {
		Status::Pending
	}
}

fn status_colour(p: &Palette, s: Status) -> Color32 {
	match s {
		Status::Done => p.success,
		Status::Pending => p.warning,
		Status::Failed => p.danger,
	}
}

/// A status marker: a filled dot when done or failed, a ring while
/// pending. Rows, tiles and the status chips share it.
fn status_dot(painter: &egui::Painter, p: &Palette, centre: egui::Pos2, s: Status) {
	if s == Status::Pending {
		painter.circle_stroke(centre, 3.5, Stroke::new(1.3, status_colour(p, s)));
	} else {
		painter.circle_filled(centre, 3.5, status_colour(p, s));
	}
}

impl Browser {
	fn rebuild_if_needed(&mut self, ws: &Workspace) {
		if self.built_for.as_ref().map(|(root, _)| root) != Some(&ws.root) {
			self.thumbnails = Some(Thumbnails::new(&ws.root));
			self.facts = Some(Facts::open(&ws.root));
		}
		let key = (ws.root.clone(), ws.generation());
		if self.built_for.as_ref() != Some(&key) {
			self.tree = FolderTree::build(ws.items().iter().map(|i| i.folder.as_slice()));
			self.stems = ws
				.items()
				.iter()
				.enumerate()
				.map(|(i, item)| (item.stem.clone(), i))
				.collect();
			// indices move when models come and go
			*self.selection.borrow_mut() = Selection::default();
			self.built_for = Some(key);
		}
	}

	/// Shows what the viewer has open in the list: clears the search and
	/// filters, opens the folders leading to it and scrolls to it.
	fn reveal_opened(&mut self, ctx: &egui::Context, ws: &Workspace, opened: Opened<'_>) {
		let item = opened
			.stem
			.and_then(|stem| ws.items().iter().find(|i| i.stem == stem));
		let model = opened.model.filter(|&i| i < ws.models().len());
		if item.is_none() && model.is_none() {
			return;
		}
		self.search.clear();
		self.status = StatusFilter::All;
		self.filters = Filters::default();
		self.view = View::List;
		if let Some(item) = item {
			tree::open_path(ctx, SCOPE, &item.folder);
		}
		if model.is_some() {
			tree::set_open(ctx, list::models_folder_id(), true);
		}
		self.scroll_to_open.set(true);
	}

	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		open_stem: Option<&str>,
		open_model: Option<usize>,
		renderer: Option<&GpuRenderer>,
	) -> Vec<Action> {
		self.rebuild_if_needed(ws);
		let opened = Opened {
			stem: open_stem,
			model: open_model,
		};
		let p = palette(ui.ctx());
		let mut actions = Vec::new();
		let items = ws.items();
		{
			let mut s = self.selection.borrow_mut();
			s.order = std::mem::take(&mut s.drawn);
		}

		let ctx = ui.ctx().clone();
		if let Some(command) = self.pending.borrow_mut().take() {
			apply_folder_command(&ctx, &command, &self.tree);
		}
		if std::mem::take(&mut self.reveal) {
			self.reveal_opened(&ctx, ws, opened);
		}

		self.header(ui, ws, open_stem.is_some() || open_model.is_some());
		if items.is_empty() && ws.models().is_empty() {
			ui.add_space(8.0);
			ui.label(
				RichText::new(
					"No models yet. Import an export .json with the Import button, or drop it onto the window.",
				)
				.color(p.text_weak),
			);
			return actions;
		}
		ui.add_space(4.0);
		self.search_row(ui);
		self.update_needle();
		ui.add_space(2.0);
		self.status_chips(ui, items);
		ui.add_space(4.0);
		self.selection_bar(ui, ws, &mut actions);
		self.selection_keys(ui);

		let view = self.view;
		widgets::scroll_in_margin(
			ui,
			egui::ScrollArea::vertical().auto_shrink([false, false]),
			|ui| match view {
				View::List => {
					ui.spacing_mut().interact_size.y = ROW;
					let any = (0..ws.models().len())
						.any(|i| self.model_matches(ws, &ws.models()[i]))
						|| ws.items().iter().any(|item| self.matches(item));
					if !any {
						nothing_matches(ui);
					}
					self.models(ui, ws, opened.model, &mut actions);
					let tree = std::mem::take(&mut self.tree);
					self.folder(ui, ws, &tree, "", opened.stem, &mut actions);
					self.tree = tree;
				}
				View::Grid => self.grid(ui, ws, opened, &mut actions),
			},
		);

		// meshes are read for the skinned filter once it is first used
		if let Some(facts) = &mut self.facts
			&& (self.filters.skinned || facts.reading() > 0)
		{
			facts.update(&ctx, ws);
		}
		if let Some(thumbnails) = &mut self.thumbnails {
			thumbnails.update(&ctx, ws, renderer);
		}
		actions
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn is_open(ctx: &egui::Context, id: egui::Id) -> Option<bool> {
		egui::collapsing_header::CollapsingState::load(ctx, id).map(|s| s.is_open())
	}

	#[test]
	fn collapse_all_includes_the_animated_models_folder() {
		let ctx = egui::Context::default();
		let folder: Vec<String> = vec!["Props".into()];
		let tree = FolderTree::build([folder.as_slice()]);
		let props = tree::folder_id(SCOPE, "Props");
		for open in [false, true] {
			apply_folder_command(&ctx, &FolderCommand::All(open), &tree);
			assert_eq!(is_open(&ctx, props), Some(open));
			// it was once left as it was (bug B42)
			assert_eq!(is_open(&ctx, list::models_folder_id()), Some(open));
		}
		// a folder's own "collapse all inside" leaves the models alone
		apply_folder_command(&ctx, &FolderCommand::Subtree("Props".into(), false), &tree);
		assert_eq!(is_open(&ctx, list::models_folder_id()), Some(true));
	}

	#[test]
	fn reveal_shows_an_open_animated_model() {
		let dir = crate::test_support::TempDir::new("reveal");
		let mut ws = Workspace::create(&dir.join("ws"), "ws").unwrap();
		let model: crate::workspace::ModelItem = serde_json::from_value(serde_json::json!({
			"name": "Rig", "path": "Workspace.Rig", "folder": [], "file": "Rigs/Rig.rig.json",
			"meshes": {}, "parts": 1, "clips": 0, "playable_clips": 0,
		}))
		.unwrap();
		ws.set_contents(Vec::new(), vec![model]);
		let ctx = egui::Context::default();
		let mut browser = Browser {
			search: "nothing like it".into(),
			view: View::Grid,
			..Default::default()
		};
		tree::set_open(&ctx, list::models_folder_id(), false);
		// it once did nothing unless a mesh was open (bug B43)
		let opened = Opened {
			stem: None,
			model: Some(0),
		};
		browser.reveal_opened(&ctx, &ws, opened);
		assert!(browser.search.is_empty());
		assert!(browser.view == View::List);
		assert_eq!(is_open(&ctx, list::models_folder_id()), Some(true));
		assert!(browser.scroll_to_open.get());

		// nothing open, or a model that has gone: nothing changes
		let mut browser = Browser {
			search: "kept".into(),
			..Default::default()
		};
		for model in [None, Some(1)] {
			browser.reveal_opened(&ctx, &ws, Opened { stem: None, model });
		}
		assert_eq!(browser.search, "kept");
		assert!(!browser.scroll_to_open.get());
	}
}
