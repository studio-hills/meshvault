//! The workspace browser: imported models with their download status, as
//! a folder tree or a grid of thumbnails, with search, filters and
//! actions on several models at once.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, Vec2};

use crate::export::FolderTree;
use crate::facts::Facts;
use crate::gpu::GpuRenderer;
use crate::maps::MapKind;
use crate::obj_export::Format;
use crate::thumbnails::{Subject, Thumbnails};
use crate::ui::theme::{Palette, ROW, display, palette};
use crate::ui::tree::{self, FolderCommand};
use crate::ui::{icons, widgets};
use crate::workspace::{AssetState, Item, ModelItem, Workspace};

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

/// The two export items of a menu, for `items` and `models`.
fn export_items(
	ui: &mut egui::Ui,
	items: &[usize],
	models: &[usize],
	n: usize,
	actions: &mut Vec<Action>,
) {
	let what = if n == 1 {
		String::new()
	} else {
		format!("{n} ")
	};
	let blender = widgets::blender_item(ui, format!("Export {what}for Blender (.glb)…"))
		.on_hover_text("Binary glTF with the skeleton, skin weights, clips and maps");
	let obj = ui.button(format!("{}  Export {what}as OBJ…", icons::EXPORT));
	for (response, format) in [(blender, Format::Glb), (obj, Format::Obj)] {
		if response.clicked() {
			actions.push(Action::Export {
				items: items.to_vec(),
				models: models.to_vec(),
				format,
			});
			ui.close();
		}
	}
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

#[derive(Default)]
struct Selection {
	keys: BTreeSet<Key>,
	/// The row a Shift-click selects from.
	anchor: Option<Key>,
	/// Rows in the order drawn this frame, and the last.
	drawn: Vec<Key>,
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
	/// Open the folders leading to the open model and scroll to it.
	reveal: bool,
	scroll_to_open: std::cell::Cell<bool>,
	selection: RefCell<Selection>,
	thumbnails: Option<Thumbnails>,
	facts: Option<Facts>,
}

/// Folder state namespace for the library tree.
const SCOPE: &str = "library";
/// Smallest grid tile, and the gap between tiles.
const TILE: f32 = 92.0;
const GAP: f32 = 6.0;

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

/// Shown in place of the list or grid when the search and filters leave
/// nothing.
fn nothing_matches(ui: &mut egui::Ui) {
	ui.label(RichText::new("Nothing matches.").color(palette(ui.ctx()).text_weak));
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

fn describe(state: &AssetState) -> String {
	match state {
		AssetState::Pending => "not downloaded yet".into(),
		AssetState::Done { path } => path.clone(),
		AssetState::Failed { error } => format!("failed: {error}"),
	}
}

fn item_hover(item: &Item) -> String {
	let mut hover = format!("Mesh {}: {}", item.mesh_id, describe(&item.mesh));
	for (kind, map) in &item.maps {
		hover.push_str(&format!(
			"\n{} {}: {}",
			kind.label(),
			map.id,
			describe(&map.state)
		));
	}
	hover
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

	/// The items a model's meshes are, where they are still in the workspace.
	fn model_items<'a>(
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

	/// A model is done when every mesh it uses is downloaded.
	fn model_status(&self, ws: &Workspace, model: &ModelItem) -> Status {
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
			.and_then(|f| f.get(path))
			.is_some_and(|f| f.skinned)
	}

	fn has_map(&self, item: &Item) -> bool {
		self.filters.maps.iter().any(|k| item.maps.contains_key(k))
	}

	fn matches(&self, item: &Item) -> bool {
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
		if self.filter.is_empty() {
			return true;
		}
		let needle = self.filter.to_lowercase();
		item.name.to_lowercase().contains(&needle)
			|| item
				.folder
				.iter()
				.any(|f| f.to_lowercase().contains(&needle))
			|| item.mesh_id.to_string() == needle
	}

	fn model_matches(&self, ws: &Workspace, model: &ModelItem) -> bool {
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
		let needle = self.filter.to_lowercase();
		needle.is_empty()
			|| model.name.to_lowercase().contains(&needle)
			|| model.path.to_lowercase().contains(&needle)
	}

	fn narrowed(&self) -> bool {
		!self.filter.is_empty() || self.status != StatusFilter::All || self.filters.count() > 0
	}

	// ------------------------------------------------------------ selection

	/// A click on a row: a plain click opens it and selects only it, Ctrl
	/// adds or takes it away, and Shift selects the rows from the last one
	/// clicked.
	fn click(&self, ui: &egui::Ui, key: Key, actions: &mut Vec<Action>) {
		let m = ui.input(|i| i.modifiers);
		let mut s = self.selection.borrow_mut();
		if m.shift
			&& let Some(anchor) = s.anchor
			&& let (Some(a), Some(b)) = (
				s.order.iter().position(|k| *k == anchor),
				s.order.iter().position(|k| *k == key),
			) {
			if !m.command {
				s.keys.clear();
			}
			let range: Vec<Key> = s.order[a.min(b)..=a.max(b)].to_vec();
			s.keys.extend(range);
			return;
		}
		s.anchor = Some(key);
		if m.command {
			if !s.keys.remove(&key) {
				s.keys.insert(key);
			}
			return;
		}
		s.keys = BTreeSet::from([key]);
		actions.push(match key {
			Key::Model(i) => Action::OpenModel(i),
			Key::Item(i) => Action::Open(i),
		});
	}

	fn is_selected(&self, key: Key) -> bool {
		self.selection.borrow().keys.contains(&key)
	}

	/// Notes a row as drawn, for Shift-click ranges and Ctrl+A.
	fn drawn(&self, key: Key) {
		self.selection.borrow_mut().drawn.push(key);
	}

	/// The selected meshes and models.
	fn selected(&self) -> (Vec<usize>, Vec<usize>) {
		let s = self.selection.borrow();
		let items = s
			.keys
			.iter()
			.filter_map(|k| match k {
				Key::Item(i) => Some(*i),
				Key::Model(_) => None,
			})
			.collect();
		let models = s
			.keys
			.iter()
			.filter_map(|k| match k {
				Key::Model(i) => Some(*i),
				Key::Item(_) => None,
			})
			.collect();
		(items, models)
	}

	/// The failed downloads among the selection: its meshes, and the
	/// meshes its models use.
	fn selected_failures(&self, ws: &Workspace) -> Vec<usize> {
		let (items, models) = self.selected();
		let mut failed: BTreeSet<usize> = items
			.into_iter()
			.filter(|&i| ws.items()[i].has_failure())
			.collect();
		for m in models {
			failed.extend(
				self.model_items(ws, &ws.models()[m])
					.filter(|(_, item)| item.has_failure())
					.map(|(i, _)| i),
			);
		}
		failed.into_iter().collect()
	}

	/// The actions on the whole selection, for its context menu.
	fn selection_menu(&self, ui: &mut egui::Ui, ws: &Workspace, actions: &mut Vec<Action>) {
		widgets::menu_style(ui);
		let n = self.selection.borrow().keys.len();
		let failed = self.selected_failures(ws);
		if ui
			.add_enabled(
				!failed.is_empty(),
				egui::Button::new(format!(
					"{}  Retry {} failed",
					icons::ARROWS_CLOCKWISE,
					failed.len()
				)),
			)
			.clicked()
		{
			actions.push(Action::Retry(failed));
			ui.close();
		}
		let (items, models) = self.selected();
		export_items(ui, &items, &models, n, actions);
		if ui
			.button(format!("{}  Clear selection", icons::X))
			.clicked()
		{
			self.selection.borrow_mut().keys.clear();
			ui.close();
		}
		ui.separator();
		if widgets::danger_item(ui, format!("{}  Remove {n} from workspace", icons::TRASH))
			.clicked()
		{
			let (items, models) = self.selected();
			actions.push(Action::RemoveSelection { items, models });
			ui.close();
		}
	}

	/// A right-click on a row in a selection of several acts on all of them.
	fn in_multi_selection(&self, key: Key) -> bool {
		let s = self.selection.borrow();
		s.keys.len() > 1 && s.keys.contains(&key)
	}

	/// The bar over the library while several models are selected.
	fn selection_bar(&self, ui: &mut egui::Ui, ws: &Workspace, actions: &mut Vec<Action>) {
		let n = self.selection.borrow().keys.len();
		if n < 2 {
			return;
		}
		let p = palette(ui.ctx());
		egui::Frame::NONE
			.fill(p.accent_soft)
			.corner_radius(CornerRadius::same(8))
			.inner_margin(egui::Margin::symmetric(10, 2))
			.show(ui, |ui| {
				ui.horizontal(|ui| {
					ui.spacing_mut().interact_size.y = 28.0;
					ui.label(RichText::new(format!("{n} selected")).color(p.accent));
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						ui.spacing_mut().item_spacing.x = 0.0;
						if widgets::icon_button(ui, icons::X, "Clear selection (Esc)").clicked() {
							self.selection.borrow_mut().keys.clear();
						}
						if widgets::danger_icon_button(ui, icons::TRASH, "Remove from workspace")
							.clicked()
						{
							let (items, models) = self.selected();
							actions.push(Action::RemoveSelection { items, models });
						}
						let export = widgets::icon_button(ui, icons::EXPORT, "Export");
						egui::Popup::menu(&export).show(|ui| {
							widgets::menu_style(ui);
							let (items, models) = self.selected();
							export_items(ui, &items, &models, n, actions);
						});
						let failed = self.selected_failures(ws);
						let retry = ui
							.add_enabled_ui(!failed.is_empty(), |ui| {
								widgets::icon_button(
									ui,
									icons::ARROWS_CLOCKWISE,
									&format!("Retry {} failed downloads", failed.len()),
								)
							})
							.inner;
						if retry.clicked() {
							actions.push(Action::Retry(failed));
						}
					});
				});
			});
		ui.add_space(4.0);
	}

	/// Ctrl+A selects every row shown, Esc clears the selection.
	fn selection_keys(&self, ui: &egui::Ui) {
		if !ui.ui_contains_pointer() || ui.ctx().egui_wants_keyboard_input() {
			return;
		}
		let (all, clear) = ui.input_mut(|i| {
			(
				i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
				i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
			)
		});
		let mut s = self.selection.borrow_mut();
		if all {
			s.keys = s.order.iter().copied().collect();
		}
		if clear {
			s.keys.clear();
		}
	}

	// ---------------------------------------------------------------- menus

	fn model_menu(&self, ui: &mut egui::Ui, ws: &Workspace, i: usize, actions: &mut Vec<Action>) {
		if self.in_multi_selection(Key::Model(i)) {
			self.selection_menu(ui, ws, actions);
			return;
		}
		let model = &ws.models()[i];
		widgets::menu_style(ui);
		if ui.button("Open").clicked() {
			actions.push(Action::OpenModel(i));
			ui.close();
		}
		if ui.button("Show rig file in file browser").clicked() {
			actions.push(Action::Reveal(ws.root.join(&model.file)));
			ui.close();
		}
		if ui.button("Copy instance path").clicked() {
			ui.ctx().copy_text(model.path.clone());
			ui.close();
		}
		export_items(ui, &[], &[i], 1, actions);
		ui.separator();
		if widgets::danger_item(ui, format!("{}  Remove from workspace", icons::TRASH)).clicked() {
			actions.push(Action::RemoveModels(vec![i]));
			ui.close();
		}
	}

	fn item_menu(&self, ui: &mut egui::Ui, ws: &Workspace, i: usize, actions: &mut Vec<Action>) {
		if self.in_multi_selection(Key::Item(i)) {
			self.selection_menu(ui, ws, actions);
			return;
		}
		let item = &ws.items()[i];
		widgets::menu_style(ui);
		if ui.button("Open").clicked() {
			actions.push(Action::Open(i));
			ui.close();
		}
		if item.has_failure() && ui.button("Retry").clicked() {
			actions.push(Action::Retry(vec![i]));
			ui.close();
		}
		if let AssetState::Done { path } = &item.mesh
			&& ui.button("Show in file browser").clicked()
		{
			actions.push(Action::Reveal(ws.root.join(path)));
			ui.close();
		}
		if ui.button("Copy mesh id").clicked() {
			ui.ctx().copy_text(item.mesh_id.to_string());
			ui.close();
		}
		if item.is_done() {
			export_items(ui, &[i], &[], 1, actions);
		}
		ui.separator();
		if widgets::danger_item(ui, format!("{}  Remove from workspace", icons::TRASH)).clicked() {
			actions.push(Action::Remove(vec![i]));
			ui.close();
		}
	}

	fn model_hover(&self, ws: &Workspace, model: &ModelItem) -> String {
		let missing = model.meshes.len()
			- self
				.model_items(ws, model)
				.filter(|(_, it)| matches!(it.mesh, AssetState::Done { .. }))
				.count();
		let mut hover = format!(
			"{}\n{} parts, {} meshes\n{} of {} clips playable",
			model.path,
			model.parts,
			model.meshes.len(),
			model.playable_clips,
			model.clips
		);
		if missing > 0 {
			hover.push_str(&format!("\n{missing} meshes not downloaded yet"));
		}
		hover
	}

	// ----------------------------------------------------------------- list

	/// Animated models, as a folder above the mesh folders.
	fn models(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		open_model: Option<usize>,
		actions: &mut Vec<Action>,
	) {
		let visible: Vec<usize> = (0..ws.models().len())
			.filter(|&i| self.model_matches(ws, &ws.models()[i]))
			.collect();
		if visible.is_empty() {
			return;
		}
		let p = palette(ui.ctx());
		// not a valid folder path, so it cannot clash with an export folder
		let id = tree::folder_id(SCOPE, "\0animated");
		let mut state =
			egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
		if self.narrowed() {
			state.set_open(true);
		}
		let mut clicked = false;
		let (arrow, _, _) = state
			.show_header(ui, |ui| {
				let response = widgets::pointer(
					ui.add(
						egui::Label::new(RichText::new(format!(
							"{}  Animated models",
							icons::PERSON_SIMPLE_RUN
						)))
						.sense(egui::Sense::click())
						.truncate(),
					),
				);
				clicked = response.clicked();
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					ui.label(
						RichText::new(ws.models().len().to_string())
							.size(11.5)
							.color(p.text_weak),
					);
				});
				response.context_menu(|ui| {
					widgets::menu_style(ui);
					if widgets::danger_item(
						ui,
						format!("{}  Remove all animated models", icons::TRASH),
					)
					.clicked()
					{
						actions.push(Action::RemoveModels((0..ws.models().len()).collect()));
						ui.close();
					}
				});
			})
			.body(|ui| {
				for &i in &visible {
					let model = &ws.models()[i];
					let key = Key::Model(i);
					let response = model_row(
						ui,
						&p,
						model,
						self.model_status(ws, model),
						open_model == Some(i),
						self.is_selected(key),
					);
					self.drawn(key);
					let response = response.on_hover_text(self.model_hover(ws, model));
					if response.clicked() {
						self.click(ui, key, actions);
					}
					response.context_menu(|ui| self.model_menu(ui, ws, i, actions));
				}
			});
		widgets::pointer(arrow);
		if clicked
			&& let Some(mut state) = egui::collapsing_header::CollapsingState::load(ui.ctx(), id)
		{
			state.toggle(ui);
			state.store(ui.ctx());
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
			ui.label(RichText::new("Library").size(16.0).family(display()));
			ui.label(RichText::new(items.len().to_string()).color(p.text_weak));
			if items.is_empty() && ws.models().is_empty() {
				return;
			}
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.spacing_mut().item_spacing.x = 0.0;
				ui.spacing_mut().interact_size.y = 28.0;
				if self.view == View::List {
					if widgets::icon_button(
						ui,
						icons::ARROWS_IN_LINE_VERTICAL,
						"Collapse all folders",
					)
					.clicked()
					{
						self.pending.replace(Some(FolderCommand::All(false)));
					}
					if widgets::icon_button(
						ui,
						icons::ARROWS_OUT_LINE_VERTICAL,
						"Expand all folders",
					)
					.clicked()
					{
						self.pending.replace(Some(FolderCommand::All(true)));
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
			ui.spacing_mut().interact_size.y = 28.0;
			ui.spacing_mut().button_padding = egui::vec2(10.0, 4.0);
			// room before the label for the chip's status marker
			let font = FontId::proportional(12.0);
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
						.size(12.0)
						.color(status_colour(&p, s)),
					None => RichText::new(format!("{label} {n}")).size(12.0),
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

	/// The filter button beside the search, with a popup of filters.
	fn filter_button(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		let n = self.filters.count();
		let tip = if n == 0 {
			"Filters".to_string()
		} else {
			format!("Filters ({n} on)")
		};
		let response = widgets::icon_toggle(ui, icons::FUNNEL, &tip, n > 0);
		if n > 0 {
			// a dot on the button while a filter is on
			let at = response.rect.right_top() + Vec2::new(-7.0, 7.0);
			ui.painter().circle_filled(at, 3.0, p.accent);
		}
		let reading = self.facts.as_ref().map_or(0, Facts::reading);
		egui::Popup::menu(&response)
			.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
			.show(|ui| {
				widgets::menu_style(ui);
				ui.set_min_width(200.0);
				let f = &mut self.filters;
				ui.label(RichText::new("Show only").size(11.5).color(p.text_weak));
				ui.checkbox(&mut f.animated, "Has animations");
				ui.checkbox(&mut f.skinned, "Skinned (bends with bones)");
				if f.skinned && reading > 0 {
					ui.label(
						RichText::new(format!("Reading meshes… {reading} left"))
							.size(11.5)
							.color(p.text_weak),
					);
				}
				ui.checkbox(&mut f.missing, "Missing meshes");
				ui.add_space(4.0);
				ui.label(RichText::new("Has a map").size(11.5).color(p.text_weak));
				for kind in MapKind::ALL {
					let mut on = f.maps.contains(&kind);
					if ui.checkbox(&mut on, kind.label()).changed() {
						if on {
							f.maps.insert(kind);
						} else {
							f.maps.remove(&kind);
						}
					}
				}
				ui.add_space(4.0);
				if ui
					.add_enabled(f.count() > 0, egui::Button::new("Clear filters"))
					.clicked()
				{
					*f = Filters::default();
				}
			});
	}

	fn folder(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		node: &FolderTree,
		id_path: &str,
		open_stem: Option<&str>,
		actions: &mut Vec<Action>,
	) {
		let items = ws.items();
		for (name, child) in node.sorted_folders() {
			let all = child.all_items();
			let visible: Vec<usize> = all
				.iter()
				.copied()
				.filter(|&i| self.matches(&items[i]))
				.collect();
			if visible.is_empty() {
				continue;
			}
			let failed: Vec<usize> = all
				.iter()
				.copied()
				.filter(|&i| items[i].has_failure())
				.collect();
			let done = all.iter().filter(|&&i| items[i].is_done()).count();
			let path = tree::child_path(id_path, name);
			let id = tree::folder_id(SCOPE, &path);
			let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
				ui.ctx(),
				id,
				false,
			);
			if self.narrowed() {
				state.set_open(true);
			}
			let p = palette(ui.ctx());
			let mut clicked = false;
			let (arrow, _, _) = state
				.show_header(ui, |ui| {
					let colour = if failed.is_empty() { p.text } else { p.danger };
					let text =
						RichText::new(format!("{}  {name}", icons::FOLDER_SIMPLE)).color(colour);
					let response = widgets::pointer(
						ui.add(
							egui::Label::new(text)
								.sense(egui::Sense::click())
								.truncate(),
						),
					);
					// the whole name toggles the folder, not just the arrow
					clicked = response.clicked();
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						ui.label(
							RichText::new(format!("{done}/{}", all.len()))
								.size(11.5)
								.color(p.text_weak),
						);
					});
					response.context_menu(|ui| {
						widgets::menu_style(ui);
						if ui
							.add_enabled(
								!failed.is_empty(),
								egui::Button::new(format!("Retry {} failed", failed.len())),
							)
							.clicked()
						{
							actions.push(Action::Retry(failed.clone()));
							ui.close();
						}
						if ui.button("Select all inside").clicked() {
							let mut s = self.selection.borrow_mut();
							s.keys = visible.iter().map(|&i| Key::Item(i)).collect();
							ui.close();
						}
						if let Some(first) = all.first() {
							let dir = ws.root.join(&items[*first].stem);
							if ui.button("Show in file browser").clicked() {
								if let Some(parent) = dir.parent() {
									actions.push(Action::Reveal(parent.to_path_buf()));
								}
								ui.close();
							}
						}
						ui.separator();
						if ui
							.button(format!(
								"{}  Expand all inside",
								icons::ARROWS_OUT_LINE_VERTICAL
							))
							.clicked()
						{
							self.pending
								.replace(Some(FolderCommand::Subtree(path.clone(), true)));
							ui.close();
						}
						if ui
							.button(format!(
								"{}  Collapse all inside",
								icons::ARROWS_IN_LINE_VERTICAL
							))
							.clicked()
						{
							self.pending
								.replace(Some(FolderCommand::Subtree(path.clone(), false)));
							ui.close();
						}
						ui.separator();
						if widgets::danger_item(
							ui,
							format!("{}  Remove folder ({} models)", icons::TRASH, all.len()),
						)
						.clicked()
						{
							actions.push(Action::Remove(all.clone()));
							ui.close();
						}
					});
				})
				.body(|ui| self.folder(ui, ws, child, &path, open_stem, actions));
			widgets::pointer(arrow);
			if clicked
				&& let Some(mut state) =
					egui::collapsing_header::CollapsingState::load(ui.ctx(), id)
			{
				state.toggle(ui);
				state.store(ui.ctx());
			}
		}
		for &i in &node.items {
			let item = &items[i];
			if !self.matches(item) {
				continue;
			}
			let p = palette(ui.ctx());
			let is_open = open_stem == Some(item.stem.as_str());
			let key = Key::Item(i);
			let response = item_row(ui, &p, item, is_open, self.is_selected(key));
			self.drawn(key);
			if is_open && self.scroll_to_open.replace(false) {
				response.scroll_to_me(Some(egui::Align::Center));
			}
			let response = response.on_hover_text(item_hover(item));
			if response.clicked() {
				self.click(ui, key, actions);
			}
			response.context_menu(|ui| self.item_menu(ui, ws, i, actions));
		}
	}

	// ----------------------------------------------------------------- grid

	/// Thumbnails: animated models first, then meshes by folder.
	fn grid(
		&mut self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		open_stem: Option<&str>,
		open_model: Option<usize>,
		actions: &mut Vec<Action>,
	) {
		let items = ws.items();
		let mut sections: Vec<(String, Vec<Key>)> = Vec::new();
		let models: Vec<Key> = (0..ws.models().len())
			.filter(|&i| self.model_matches(ws, &ws.models()[i]))
			.map(Key::Model)
			.collect();
		if !models.is_empty() {
			sections.push((
				format!("{}  Animated models", icons::PERSON_SIMPLE_RUN),
				models,
			));
		}
		let mut meshes: Vec<usize> = (0..items.len())
			.filter(|&i| self.matches(&items[i]))
			.collect();
		meshes.sort_by(|&a, &b| {
			items[a].folder.cmp(&items[b].folder).then_with(|| {
				items[a]
					.name
					.to_lowercase()
					.cmp(&items[b].name.to_lowercase())
			})
		});
		// a folder's section is titled with its own name; its path is on hover
		let mut paths: Vec<String> = vec![String::new(); sections.len()];
		for i in meshes {
			let path = items[i].folder.join(" / ");
			match sections.last_mut() {
				Some((_, keys)) if paths.last() == Some(&path) => keys.push(Key::Item(i)),
				_ => {
					let name = items[i].folder.last().map_or("", String::as_str);
					sections.push((
						format!("{}  {name}", icons::FOLDER_SIMPLE),
						vec![Key::Item(i)],
					));
					paths.push(path);
				}
			}
		}
		if sections.is_empty() {
			nothing_matches(ui);
			return;
		}
		let p = palette(ui.ctx());
		let width = ui.available_width();
		let columns = ((width + GAP) / (TILE + GAP)).floor().max(1.0) as usize;
		let side = (width - GAP * (columns - 1) as f32) / columns as f32;
		for ((title, keys), path) in sections.into_iter().zip(paths) {
			ui.add_space(4.0);
			let label = ui.add(
				egui::Label::new(RichText::new(title).size(12.0).color(p.text_weak)).truncate(),
			);
			if !path.is_empty() {
				label.on_hover_text(path);
			}
			ui.add_space(2.0);
			for row in keys.chunks(columns) {
				ui.horizontal(|ui| {
					ui.spacing_mut().item_spacing.x = GAP;
					for &key in row {
						self.tile(ui, ws, key, side, open_stem, open_model, actions);
					}
				});
				ui.add_space(GAP - ui.spacing().item_spacing.y);
			}
		}
	}

	#[allow(clippy::too_many_arguments)]
	fn tile(
		&mut self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		key: Key,
		side: f32,
		open_stem: Option<&str>,
		open_model: Option<usize>,
		actions: &mut Vec<Action>,
	) {
		let p = palette(ui.ctx());
		let label_height = 20.0;
		let (rect, response) =
			ui.allocate_exact_size(Vec2::new(side, side + label_height), Sense::click());
		self.drawn(key);
		let (name, open, s, subject) = match key {
			Key::Model(i) => {
				let m = &ws.models()[i];
				(
					m.name.as_str(),
					open_model == Some(i),
					self.model_status(ws, m),
					Subject::Model(m.path.clone()),
				)
			}
			Key::Item(i) => {
				let item = &ws.items()[i];
				(
					item.name.as_str(),
					open_stem == Some(item.stem.as_str()),
					status(item),
					Subject::Item(item.stem.clone()),
				)
			}
		};
		if ui.is_rect_visible(rect) {
			let selected = self.is_selected(key);
			let thumbnail = self.thumbnails.as_mut().and_then(|t| t.get(&subject));
			let painter = ui.painter_at(rect);
			let picture = egui::Rect::from_min_size(rect.min, Vec2::splat(side));
			let fill = if selected {
				p.accent_soft
			} else if response.hovered() {
				p.raised_hover
			} else {
				p.raised
			};
			painter.rect_filled(picture, CornerRadius::same(8), fill);
			if open {
				painter.rect_stroke(
					picture,
					CornerRadius::same(8),
					Stroke::new(1.5, p.accent),
					egui::StrokeKind::Inside,
				);
			}
			match thumbnail {
				Some(texture) => {
					painter.image(
						texture.id(),
						picture.shrink(4.0),
						egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
						Color32::WHITE,
					);
				}
				None => {
					let icon = match key {
						Key::Model(_) => icons::PERSON_SIMPLE_RUN,
						Key::Item(_) => icons::CUBE,
					};
					painter.text(
						picture.center(),
						Align2::CENTER_CENTER,
						icon,
						FontId::proportional(26.0),
						p.text_weak.gamma_multiply(0.6),
					);
				}
			}
			// downloads still to come, or failed, show in the corner
			if s != Status::Done {
				status_dot(&painter, &p, picture.left_top() + Vec2::new(10.0, 10.0), s);
			}
			if let Key::Model(i) = key {
				let clips = format!("{} {}", icons::FILM_STRIP, ws.models()[i].playable_clips);
				painter.text(
					picture.right_top() + Vec2::new(-8.0, 8.0),
					Align2::RIGHT_TOP,
					clips,
					FontId::proportional(11.0),
					p.text_weak,
				);
			}
			let label = egui::Rect::from_min_max(
				egui::pos2(rect.left() + 2.0, picture.bottom()),
				rect.right_bottom() - Vec2::new(2.0, 0.0),
			);
			let galley = painter.layout(
				name.to_string(),
				FontId::proportional(12.0),
				if open { p.accent } else { p.text },
				f32::INFINITY,
			);
			// centred when it fits, else cut at the right edge
			let x = if galley.size().x <= label.width() {
				label.center().x - galley.size().x / 2.0
			} else {
				label.left()
			};
			painter.with_clip_rect(label).galley(
				egui::pos2(x, label.center().y - galley.size().y / 2.0),
				galley,
				p.text,
			);
		}
		let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
		let response = match key {
			Key::Model(i) => response.on_hover_text(self.model_hover(ws, &ws.models()[i])),
			Key::Item(i) => response.on_hover_text(format!(
				"{}\n{}",
				ws.items()[i].folder.join(" / "),
				item_hover(&ws.items()[i])
			)),
		};
		if response.clicked() {
			self.click(ui, key, actions);
		}
		response.context_menu(|ui| match key {
			Key::Model(i) => self.model_menu(ui, ws, i, actions),
			Key::Item(i) => self.item_menu(ui, ws, i, actions),
		});
	}
}

/// Row background: the selection is tinted, a hovered row lightened.
fn row_fill(p: &Palette, response: &egui::Response, selected: bool) -> Color32 {
	if selected {
		p.accent_soft
	} else if response.hovered() {
		p.raised_hover
	} else {
		Color32::TRANSPARENT
	}
}

/// A library row for an animated model: status dot, name, clip count.
fn model_row(
	ui: &mut egui::Ui,
	p: &Palette,
	model: &ModelItem,
	s: Status,
	open: bool,
	selected: bool,
) -> egui::Response {
	let (rect, response) =
		ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
	let painter = ui.painter_at(rect);
	painter.rect_filled(
		rect,
		CornerRadius::same(6),
		row_fill(p, &response, selected),
	);
	status_dot(&painter, p, rect.left_center() + Vec2::new(11.0, 0.0), s);
	let clips = format!("{}  {}", icons::FILM_STRIP, model.playable_clips);
	let clips_galley = painter.layout_no_wrap(clips, FontId::proportional(11.5), p.text_weak);
	let text_rect = egui::Rect::from_min_max(
		rect.left_top() + Vec2::new(24.0, 0.0),
		rect.right_bottom() - Vec2::new(clips_galley.size().x + 16.0, 0.0),
	);
	painter.with_clip_rect(text_rect).text(
		text_rect.left_center(),
		Align2::LEFT_CENTER,
		&model.name,
		FontId::proportional(13.0),
		if open { p.accent } else { p.text },
	);
	let pos =
		rect.right_center() - Vec2::new(10.0 + clips_galley.size().x, clips_galley.size().y / 2.0);
	painter.galley(pos, clips_galley, p.text_weak);
	response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A full-width, hoverable row: status dot, name, and a map indicator.
fn item_row(
	ui: &mut egui::Ui,
	p: &Palette,
	item: &Item,
	open: bool,
	selected: bool,
) -> egui::Response {
	let (rect, response) =
		ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
	let painter = ui.painter_at(rect);
	painter.rect_filled(
		rect,
		CornerRadius::same(6),
		row_fill(p, &response, selected),
	);
	let s = status(item);
	status_dot(&painter, p, rect.left_center() + Vec2::new(11.0, 0.0), s);
	let text_colour = if open { p.accent } else { p.text };
	let right_pad = if item.maps.is_empty() { 8.0 } else { 26.0 };
	let text_rect = egui::Rect::from_min_max(
		rect.left_top() + Vec2::new(24.0, 0.0),
		rect.right_bottom() - Vec2::new(right_pad, 0.0),
	);
	painter.with_clip_rect(text_rect).text(
		text_rect.left_center(),
		Align2::LEFT_CENTER,
		&item.name,
		FontId::proportional(13.0),
		text_colour,
	);
	if !item.maps.is_empty() {
		painter.text(
			rect.right_center() - Vec2::new(10.0, 0.0),
			Align2::RIGHT_CENTER,
			icons::IMAGE,
			FontId::proportional(13.0),
			p.text_weak,
		);
	}
	response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Opens a file browser at `path` (selecting it if it is a file).
pub fn reveal(path: &std::path::Path) {
	#[cfg(target_os = "windows")]
	{
		let mut cmd = std::process::Command::new("explorer");
		if path.is_file() {
			cmd.arg(format!("/select,{}", path.display()));
		} else {
			cmd.arg(path);
		}
		let _ = cmd.spawn();
	}
	#[cfg(target_os = "macos")]
	{
		let _ = std::process::Command::new("open")
			.arg("-R")
			.arg(path)
			.spawn();
	}
	#[cfg(not(any(target_os = "windows", target_os = "macos")))]
	{
		let dir = if path.is_file() {
			path.parent().unwrap_or(path)
		} else {
			path
		};
		let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
	}
}
