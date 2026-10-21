//! The workspace browser: imported models with their download status, as
//! a folder tree or a grid of thumbnails, with search, filters and
//! actions on several models at once.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, Vec2};

use crate::export::obj::Format;
use crate::render::gpu::GpuRenderer;
use crate::render::thumbnails::{Subject, Thumbnails};
use crate::scene::maps::MapKind;
use crate::studio::export_file::FolderTree;
use crate::ui::theme::{Palette, ROW, display, palette};
use crate::ui::tree::{self, FolderCommand};
use crate::ui::{icons, widgets};
use crate::workspace::facts::Facts;
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

impl Selection {
	/// A click on `key` with these modifiers; returns the row to open.
	/// Shift selects the rows in `order` from the anchor (added to the
	/// selection with Ctrl too) and leaves the anchor where it is; without
	/// an anchor in `order`, it counts as a click without Shift.
	fn click(&mut self, key: Key, shift: bool, command: bool) -> Option<Key> {
		if shift
			&& let Some(anchor) = self.anchor
			&& let (Some(a), Some(b)) = (
				self.order.iter().position(|k| *k == anchor),
				self.order.iter().position(|k| *k == key),
			) {
			if !command {
				self.keys.clear();
			}
			let range: Vec<Key> = self.order[a.min(b)..=a.max(b)].to_vec();
			self.keys.extend(range);
			return None;
		}
		self.anchor = Some(key);
		if command {
			if !self.keys.remove(&key) {
				self.keys.insert(key);
			}
			return None;
		}
		self.keys = BTreeSet::from([key]);
		Some(key)
	}
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

/// The grid's sections, as (title, keys), and each one's hover path: the
/// animated models in `models` first, then the meshes in `meshes` sorted by
/// folder and name, one section per folder.
fn grid_sections(
	items: &[Item],
	models: Vec<Key>,
	mut meshes: Vec<usize>,
) -> (Vec<(String, Vec<Key>)>, Vec<String>) {
	let mut sections: Vec<(String, Vec<Key>)> = Vec::new();
	if !models.is_empty() {
		sections.push((
			format!("{}  Animated models", icons::PERSON_SIMPLE_RUN),
			models,
		));
	}
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
	(sections, paths)
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

	/// A model is done when every mesh it uses is downloaded, with its maps;
	/// failed when any of them failed, even if others are still pending.
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
		let open = self.selection.borrow_mut().click(key, m.shift, m.command);
		if let Some(key) = open {
			actions.push(match key {
				Key::Model(i) => Action::OpenModel(i),
				Key::Item(i) => Action::Open(i),
			});
		}
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
				// its buttons sit 2 px from its top and bottom edges
				widgets::nest(ui, 8, 2.0);
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
							.size(crate::ui::theme::CAPTION)
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
				ui.label(
					RichText::new("Show only")
						.size(crate::ui::theme::CAPTION)
						.color(p.text_weak),
				);
				ui.checkbox(&mut f.animated, "Has animations");
				ui.checkbox(&mut f.skinned, "Skinned (bends with bones)");
				if f.skinned && reading > 0 {
					ui.label(
						RichText::new(format!("Reading meshes… {reading} left"))
							.size(crate::ui::theme::CAPTION)
							.color(p.text_weak),
					);
				}
				ui.checkbox(&mut f.missing, "Missing meshes");
				ui.add_space(4.0);
				ui.label(
					RichText::new("Has a map")
						.size(crate::ui::theme::CAPTION)
						.color(p.text_weak),
				);
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
								.size(crate::ui::theme::CAPTION)
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
		let models: Vec<Key> = (0..ws.models().len())
			.filter(|&i| self.model_matches(ws, &ws.models()[i]))
			.map(Key::Model)
			.collect();
		let meshes: Vec<usize> = (0..items.len())
			.filter(|&i| self.matches(&items[i]))
			.collect();
		let (sections, paths) = grid_sections(items, models, meshes);
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
					FontId::proportional(crate::ui::theme::CAPTION),
					p.text_weak,
				);
			}
			let label = egui::Rect::from_min_max(
				egui::pos2(rect.left() + 2.0, picture.bottom()),
				rect.right_bottom() - Vec2::new(2.0, 0.0),
			);
			// centred, and cut short with an ellipsis when it does not fit
			let galley = widgets::truncated(
				&painter,
				name,
				FontId::proportional(12.0),
				if open { p.accent } else { p.text },
				label.width(),
			);
			painter.galley(label.center() - galley.size() / 2.0, galley, p.text);
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
	let clips_galley = painter.layout_no_wrap(
		clips,
		FontId::proportional(crate::ui::theme::CAPTION),
		p.text_weak,
	);
	let text_rect = egui::Rect::from_min_max(
		rect.left_top() + Vec2::new(24.0, 0.0),
		rect.right_bottom() - Vec2::new(clips_galley.size().x + 16.0, 0.0),
	);
	let name = widgets::truncated(
		&painter,
		&model.name,
		FontId::proportional(13.0),
		if open { p.accent } else { p.text },
		text_rect.width(),
	);
	painter.galley(
		text_rect.left_center() - Vec2::new(0.0, name.size().y / 2.0),
		name,
		p.text,
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
	let name = widgets::truncated(
		&painter,
		&item.name,
		FontId::proportional(13.0),
		text_colour,
		text_rect.width(),
	);
	painter.galley(
		text_rect.left_center() - Vec2::new(0.0, name.size().y / 2.0),
		name,
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::TempDir;
	use crate::workspace::MapRef;

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
		ws.manifest.items = items;
		ws.manifest.models = models;
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
}
