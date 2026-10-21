//! The list view: the export's folder tree, one row per model.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Vec2};

use super::menus::item_hover;
use super::{Action, Browser, Row, SCOPE, Status, status, status_dot};
use crate::ui::theme::{CAPTION, CONTROL_RADIUS, LIST_TEXT, Palette, ROW, palette};
use crate::ui::tree::{self, FolderTree};
use crate::ui::{icons, widgets};
use crate::workspace::{Item, ModelItem, Workspace};

/// Shown in place of the list or grid when the search and filters leave
/// nothing.
pub(super) fn nothing_matches(ui: &mut egui::Ui) {
	ui.label(RichText::new("Nothing matches.").color(palette(ui.ctx()).text_weak));
}

impl Browser {
	/// Animated models, as a folder above the mesh folders.
	pub(super) fn models(
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
					ui.label(widgets::caption(ui, ws.models().len().to_string()));
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
					let key = Row::Model(i);
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
		if clicked {
			tree::toggle(ui, id);
		}
	}

	pub(super) fn folder(
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
						ui.label(widgets::caption(ui, format!("{done}/{}", all.len())));
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
							s.keys = visible.iter().map(|&i| Row::Mesh(i)).collect();
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
						if let Some(command) = tree::subtree_menu_items(ui, &path) {
							self.pending.replace(Some(command));
						}
						ui.separator();
						if widgets::danger_item(
							ui,
							format!("{}  Remove folder ({} models)", icons::TRASH, all.len()),
						)
						.clicked()
						{
							actions.push(Action::RemoveItems(all.clone()));
							ui.close();
						}
					});
				})
				.body(|ui| self.folder(ui, ws, child, &path, open_stem, actions));
			widgets::pointer(arrow);
			if clicked {
				tree::toggle(ui, id);
			}
		}
		for &i in &node.items {
			let item = &items[i];
			if !self.matches(item) {
				continue;
			}
			let p = palette(ui.ctx());
			let is_open = open_stem == Some(item.stem.as_str());
			let key = Row::Mesh(i);
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
		ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
	let painter = ui.painter_at(rect);
	painter.rect_filled(
		rect,
		CornerRadius::same(CONTROL_RADIUS),
		row_fill(p, &response, selected),
	);
	status_dot(&painter, p, rect.left_center() + Vec2::new(11.0, 0.0), s);
	let clips = format!("{}  {}", icons::FILM_STRIP, model.playable_clips);
	let clips_galley = painter.layout_no_wrap(clips, FontId::proportional(CAPTION), p.text_weak);
	let text_rect = egui::Rect::from_min_max(
		rect.left_top() + Vec2::new(24.0, 0.0),
		rect.right_bottom() - Vec2::new(clips_galley.size().x + 16.0, 0.0),
	);
	let name = widgets::truncated(
		&painter,
		&model.name,
		FontId::proportional(LIST_TEXT),
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
		ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
	let painter = ui.painter_at(rect);
	painter.rect_filled(
		rect,
		CornerRadius::same(CONTROL_RADIUS),
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
		FontId::proportional(LIST_TEXT),
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
			FontId::proportional(LIST_TEXT),
			p.text_weak,
		);
	}
	response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
