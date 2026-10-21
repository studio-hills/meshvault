//! The library: imported models with their download status, as
//! a folder tree or a grid of thumbnails, with search, filters and
//! actions on several models at once.

mod filter;
mod grid;
mod list;
mod menus;
mod selection;
#[cfg(test)]
mod tests;
mod toolbar;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2};

use self::list::nothing_matches;
use crate::export::Format;
use crate::render::gpu::GpuRenderer;
use crate::render::thumbnails::Thumbnails;
use crate::scene::maps::MapKind;
use crate::ui::theme::{CAPTION, CONTROL_XXS, Palette, ROW, TITLE, display, palette};
use crate::ui::tree::{self, FolderCommand, FolderTree};
use crate::ui::{icons, widgets};
use crate::workspace::facts::Facts;
use crate::workspace::{Item, Workspace};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
	#[default]
	All,
	Downloaded,
	Pending,
	Failed,
}

pub enum Action {
	Open(usize),
	OpenModel(usize),
	RemoveModels(Vec<usize>),
	Retry(Vec<usize>),
	Remove(Vec<usize>),
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
enum Key {
	Model(usize),
	Item(usize),
}

/// The library's selected rows, with what Shift-click needs to select a
/// range of them.
#[derive(Default)]
struct Selection {
	keys: BTreeSet<Key>,
	/// The row a Shift-click selects from.
	anchor: Option<Key>,
	/// Rows in the order they are drawn, gathered during this frame.
	drawn: Vec<Key>,
	/// Rows in the order the previous frame drew them; clicks, handled
	/// while this frame draws, select ranges from this complete list.
	order: Vec<Key>,
}

#[derive(Default)]
pub struct Browser {
	filter: String,
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
	/// Open the folders leading to the open model and scroll to it. Only an
	/// open mesh is revealed today; an open animated model is not, though
	/// the button's label says "model".
	reveal: bool,
	scroll_to_open: std::cell::Cell<bool>,
	selection: RefCell<Selection>,
	thumbnails: Option<Thumbnails>,
	facts: Option<Facts>,
}

/// Folder state namespace for the library tree.
const SCOPE: &str = "library";

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

	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		open_stem: Option<&str>,
		open_model: Option<usize>,
		renderer: Option<&GpuRenderer>,
	) -> Vec<Action> {
		self.rebuild_if_needed(ws);
		let p = palette(ui.ctx());
		let mut actions = Vec::new();
		let items = ws.items();
		{
			let mut s = self.selection.borrow_mut();
			s.order = std::mem::take(&mut s.drawn);
		}

		let ctx = ui.ctx().clone();
		if let Some(command) = self.pending.borrow_mut().take() {
			command.apply(&ctx, SCOPE, &self.tree);
		}
		if std::mem::take(&mut self.reveal)
			&& let Some(item) = open_stem.and_then(|stem| items.iter().find(|i| i.stem == stem))
		{
			self.filter.clear();
			self.status = StatusFilter::All;
			self.filters = Filters::default();
			self.view = View::List;
			tree::open_path(&ctx, SCOPE, &item.folder);
			self.scroll_to_open.set(true);
		}

		ui.horizontal(|ui| {
			ui.label(RichText::new("Library").size(TITLE).family(display()));
			ui.label(RichText::new(items.len().to_string()).color(p.text_weak));
			if items.is_empty() && ws.models().is_empty() {
				return;
			}
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.spacing_mut().item_spacing.x = 0.0;
				ui.spacing_mut().interact_size.y = CONTROL_XXS;
				if self.view == View::List {
					if let Some(command) = tree::expand_collapse_buttons(ui) {
						self.pending.replace(Some(command));
					}
					let reveal = ui
						.add_enabled_ui(open_stem.is_some(), |ui| {
							widgets::icon_button(
								ui,
								icons::CROSSHAIR_SIMPLE,
								"Reveal the open model",
							)
						})
						.inner;
					if reveal.clicked() {
						self.reveal = true;
					}
				}
				let (icon, tip, other) = match self.view {
					View::List => (icons::SQUARES_FOUR, "Show thumbnails", View::Grid),
					View::Grid => (icons::LIST, "Show the folder tree", View::List),
				};
				if widgets::icon_button(ui, icon, tip).clicked() {
					self.view = other;
				}
			});
		});
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
		// search with a filter button (and a clear button) beside it, the same height
		ui.horizontal(|ui| {
			let button = ui.spacing().interact_size.y + ui.spacing().item_spacing.x;
			let buttons = if self.filter.is_empty() { 1.0 } else { 2.0 };
			ui.add(
				widgets::input(&mut self.filter)
					.hint_text(format!(
						"{}  Search name, folder or id",
						icons::MAGNIFYING_GLASS
					))
					.desired_width(ui.available_width() - button * buttons),
			);
			if !self.filter.is_empty()
				&& widgets::icon_button(ui, icons::X, "Clear search").clicked()
			{
				self.filter.clear();
			}
			self.filter_button(ui);
		});
		ui.add_space(2.0);
		let count = |f: fn(&Item) -> bool| items.iter().filter(|i| f(i)).count();
		let done = count(Item::is_done);
		let failed = count(Item::has_failure);
		let pending = count(|i| i.has_pending() && !i.has_failure());
		ui.horizontal_wrapped(|ui| {
			ui.spacing_mut().item_spacing.x = 4.0;
			ui.spacing_mut().interact_size.y = CONTROL_XXS;
			ui.spacing_mut().button_padding = egui::vec2(10.0, 4.0);
			// room before the label for the chip's status marker
			let font = FontId::proportional(CAPTION);
			let space = ui
				.painter()
				.layout_no_wrap(" ".into(), font, p.text)
				.size()
				.x;
			let gap = " ".repeat((13.0 / space.max(1.0)).ceil() as usize);
			for (value, label, n, marker) in [
				(StatusFilter::All, "All", items.len(), None),
				(StatusFilter::Downloaded, "Done", done, Some(Status::Done)),
				(
					StatusFilter::Pending,
					"Pending",
					pending,
					Some(Status::Pending),
				),
				(StatusFilter::Failed, "Failed", failed, Some(Status::Failed)),
			] {
				if value == StatusFilter::Failed && n == 0 && self.status != StatusFilter::Failed {
					continue;
				}
				let text = match marker {
					Some(s) => RichText::new(format!("{gap}{label} {n}"))
						.size(CAPTION)
						.color(status_colour(&p, s)),
					None => RichText::new(format!("{label} {n}")).size(CAPTION),
				};
				let response = ui.selectable_value(&mut self.status, value, text);
				// the same marker as the rows and tiles this chip filters
				if let Some(s) = marker {
					let padding = ui.spacing().button_padding.x;
					let centre = response.rect.left_center() + Vec2::new(padding + 3.5, 0.0);
					status_dot(ui.painter(), &p, centre, s);
				}
			}
		});
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
					self.models(ui, ws, open_model, &mut actions);
					let tree = std::mem::take(&mut self.tree);
					self.folder(ui, ws, &tree, "", open_stem, &mut actions);
					self.tree = tree;
				}
				View::Grid => self.grid(ui, ws, open_stem, open_model, &mut actions),
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
