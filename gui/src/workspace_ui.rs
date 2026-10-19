//! The workspace browser: a folder tree of imported models with their
//! download status.

use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, Vec2};

use crate::export::FolderTree;
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
}

#[derive(Default)]
pub struct Browser {
	filter: String,
	status: StatusFilter,
	tree: FolderTree,
	/// Workspace and generation the tree was built from.
	built_for: Option<(PathBuf, u64)>,
	/// Expand/collapse requested while drawing, applied on the next frame.
	pending: std::cell::RefCell<Option<FolderCommand>>,
	/// Open the folders leading to the open model and scroll to it.
	reveal: bool,
	scroll_to_open: std::cell::Cell<bool>,
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

/// A model is done when every mesh it uses is downloaded.
fn model_status(ws: &Workspace, model: &ModelItem) -> Status {
	let mut status = Status::Done;
	for stem in model.meshes.values() {
		match ws.items().iter().find(|i| &i.stem == stem).map(status_of) {
			Some(Status::Failed) => return Status::Failed,
			Some(Status::Pending) | None => status = Status::Pending,
			Some(Status::Done) => {}
		}
	}
	status
}

fn status_of(item: &Item) -> Status {
	status(item)
}

fn describe(state: &AssetState) -> String {
	match state {
		AssetState::Pending => "not downloaded yet".into(),
		AssetState::Done { path } => path.clone(),
		AssetState::Failed { error } => format!("failed: {error}"),
	}
}

impl Browser {
	fn rebuild_if_needed(&mut self, ws: &Workspace) {
		let key = (ws.root.clone(), ws.generation());
		if self.built_for.as_ref() != Some(&key) {
			self.tree = FolderTree::build(ws.items().iter().map(|i| i.folder.as_slice()));
			self.built_for = Some(key);
		}
	}

	fn matches(&self, item: &Item) -> bool {
		let status_ok = match self.status {
			StatusFilter::All => true,
			StatusFilter::Downloaded => item.is_done(),
			StatusFilter::Pending => item.has_pending() && !item.has_failure(),
			StatusFilter::Failed => item.has_failure(),
		};
		if !status_ok {
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
		let status_ok = match self.status {
			StatusFilter::All => true,
			StatusFilter::Downloaded => model_status(ws, model) == Status::Done,
			StatusFilter::Pending => model_status(ws, model) == Status::Pending,
			StatusFilter::Failed => model_status(ws, model) == Status::Failed,
		};
		let needle = self.filter.to_lowercase();
		status_ok
			&& (needle.is_empty()
				|| model.name.to_lowercase().contains(&needle)
				|| model.path.to_lowercase().contains(&needle))
	}

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
		if !self.filter.is_empty() || self.status != StatusFilter::All {
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
					let is_open = open_model == Some(i);
					let response = model_row(ui, &p, model, model_status(ws, model), is_open);
					let missing = model
						.meshes
						.values()
						.filter(|stem| {
							ws.items()
								.iter()
								.find(|it| &it.stem == *stem)
								.is_none_or(|it| !matches!(it.mesh, AssetState::Done { .. }))
						})
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
					let response = response.on_hover_text(hover);
					if response.clicked() {
						actions.push(Action::OpenModel(i));
					}
					response.context_menu(|ui| {
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
						ui.separator();
						if widgets::danger_item(
							ui,
							format!("{}  Remove from workspace", icons::TRASH),
						)
						.clicked()
						{
							actions.push(Action::RemoveModels(vec![i]));
							ui.close();
						}
					});
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
	) -> Vec<Action> {
		self.rebuild_if_needed(ws);
		let p = palette(ui.ctx());
		let mut actions = Vec::new();
		let items = ws.items();

		let ctx = ui.ctx().clone();
		if let Some(command) = self.pending.borrow_mut().take() {
			command.apply(&ctx, SCOPE, &self.tree);
		}
		if std::mem::take(&mut self.reveal)
			&& let Some(item) = open_stem.and_then(|stem| items.iter().find(|i| i.stem == stem))
		{
			self.filter.clear();
			self.status = StatusFilter::All;
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
				if widgets::icon_button(ui, icons::ARROWS_IN_LINE_VERTICAL, "Collapse all folders")
					.clicked()
				{
					self.pending.replace(Some(FolderCommand::All(false)));
				}
				if widgets::icon_button(ui, icons::ARROWS_OUT_LINE_VERTICAL, "Expand all folders")
					.clicked()
				{
					self.pending.replace(Some(FolderCommand::All(true)));
				}
				let reveal = ui
					.add_enabled_ui(open_stem.is_some(), |ui| {
						widgets::icon_button(ui, icons::CROSSHAIR_SIMPLE, "Reveal the open model")
					})
					.inner;
				if reveal.clicked() {
					self.reveal = true;
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
		// search with a clear button beside it, the same height
		ui.horizontal(|ui| {
			let clear_width = if self.filter.is_empty() {
				0.0
			} else {
				ui.spacing().interact_size.y + ui.spacing().item_spacing.x
			};
			ui.add(
				widgets::input(&mut self.filter)
					.hint_text(format!(
						"{}  Search name, folder or id",
						icons::MAGNIFYING_GLASS
					))
					.desired_width(ui.available_width() - clear_width),
			);
			if !self.filter.is_empty()
				&& widgets::icon_button(ui, icons::X, "Clear search").clicked()
			{
				self.filter.clear();
			}
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
			for (value, label, n, colour) in [
				(StatusFilter::All, "All", items.len(), None),
				(StatusFilter::Downloaded, "Done", done, Some(p.success)),
				(StatusFilter::Pending, "Pending", pending, Some(p.warning)),
				(StatusFilter::Failed, "Failed", failed, Some(p.danger)),
			] {
				if value == StatusFilter::Failed && n == 0 && self.status != StatusFilter::Failed {
					continue;
				}
				let mut text = RichText::new(format!("{label} {n}")).size(12.0);
				if let Some(c) = colour {
					text = RichText::new(format!("{}  {label} {n}", icons::CIRCLE))
						.size(12.0)
						.color(c);
				}
				ui.selectable_value(&mut self.status, value, text);
			}
		});
		ui.add_space(4.0);

		egui::ScrollArea::vertical()
			.auto_shrink([false, false])
			.show(ui, |ui| {
				ui.spacing_mut().interact_size.y = ROW;
				self.models(ui, ws, open_model, &mut actions);
				let tree = std::mem::take(&mut self.tree);
				self.folder(ui, ws, &tree, "", open_stem, &mut actions);
				self.tree = tree;
			});
		actions
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
			if !self.filter.is_empty() || self.status != StatusFilter::All {
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
			{
				let response = item_row(ui, &p, item, is_open);
				if is_open && self.scroll_to_open.replace(false) {
					response.scroll_to_me(Some(egui::Align::Center));
				}
				let mut hover = format!("Mesh {}: {}", item.mesh_id, describe(&item.mesh));
				for (kind, map) in &item.maps {
					hover.push_str(&format!(
						"\n{} {}: {}",
						kind.label(),
						map.id,
						describe(&map.state)
					));
				}
				let response = response.on_hover_text(hover);
				if response.clicked() {
					actions.push(Action::Open(i));
				}
				response.context_menu(|ui| {
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
					ui.separator();
					if widgets::danger_item(ui, format!("{}  Remove from workspace", icons::TRASH))
						.clicked()
					{
						actions.push(Action::Remove(vec![i]));
						ui.close();
					}
				});
			}
		}
	}
}

/// A library row for an animated model: status dot, name, clip count.
fn model_row(
	ui: &mut egui::Ui,
	p: &Palette,
	model: &ModelItem,
	s: Status,
	selected: bool,
) -> egui::Response {
	let (rect, response) =
		ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
	let painter = ui.painter_at(rect);
	let fill = if selected {
		p.accent_soft
	} else if response.hovered() {
		p.raised_hover
	} else {
		Color32::TRANSPARENT
	};
	painter.rect_filled(rect, CornerRadius::same(6), fill);
	let dot = rect.left_center() + Vec2::new(11.0, 0.0);
	if s == Status::Pending {
		painter.circle_stroke(dot, 3.5, Stroke::new(1.3, status_colour(p, s)));
	} else {
		painter.circle_filled(dot, 3.5, status_colour(p, s));
	}
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
		if selected { p.accent } else { p.text },
	);
	let pos =
		rect.right_center() - Vec2::new(10.0 + clips_galley.size().x, clips_galley.size().y / 2.0);
	painter.galley(pos, clips_galley, p.text_weak);
	response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A full-width, hoverable row: status dot, name, and a map indicator.
fn item_row(ui: &mut egui::Ui, p: &Palette, item: &Item, selected: bool) -> egui::Response {
	let (rect, response) =
		ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
	let painter = ui.painter_at(rect);
	let fill = if selected {
		p.accent_soft
	} else if response.hovered() {
		p.raised_hover
	} else {
		Color32::TRANSPARENT
	};
	painter.rect_filled(rect, CornerRadius::same(6), fill);
	let s = status(item);
	let dot = rect.left_center() + Vec2::new(11.0, 0.0);
	if s == Status::Pending {
		painter.circle_stroke(dot, 3.5, Stroke::new(1.3, status_colour(p, s)));
	} else {
		painter.circle_filled(dot, 3.5, status_colour(p, s));
	}
	let text_colour = if selected { p.accent } else { p.text };
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
