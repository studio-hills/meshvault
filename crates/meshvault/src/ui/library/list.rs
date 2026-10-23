//! The list view: the export's folder tree, one row per model.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Vec2};

use super::menus::item_hover;
use super::{Action, Browser, Row, SCOPE, Status, game_label, models_by_game, status, status_dot};
use crate::ui::theme::{CAPTION, CONTROL_RADIUS, LIST_TEXT, Palette, ROW, palette};
use crate::ui::tree::{self, FolderTree};
use crate::ui::{icons, widgets};
use crate::workspace::{Item, Workspace};

/// Shown in place of the list or grid when the search and filters leave
/// nothing.
pub(super) fn nothing_matches(ui: &mut egui::Ui) {
	ui.label(RichText::new("Nothing matches.").color(palette(ui.ctx()).text_weak));
}

/// The folder on disk that holds `node`'s meshes. It is found from the
/// first mesh at or below `node`, going up from that mesh's own folder by
/// how far below `node` it is, so the stored (sanitised) names are used.
fn folder_dir(root: &Path, items: &[Item], node: &FolderTree) -> Option<PathBuf> {
	/// The first item in the tree, as (index, how many folders down).
	fn first(node: &FolderTree) -> Option<(usize, usize)> {
		if let Some(&i) = node.items.first() {
			return Some((i, 0));
		}
		node.folders
			.values()
			.find_map(first)
			.map(|(i, depth)| (i, depth + 1))
	}
	let (i, depth) = first(node)?;
	let mut dir = root.join(&items.get(i)?.stem);
	for _ in 0..=depth {
		dir = dir.parent()?.to_path_buf();
	}
	Some(dir)
}

/// The animated models' folder, which is not in the folder tree. Its path is
/// not a valid folder path, so it cannot clash with an export folder.
pub(super) fn models_folder_id() -> egui::Id {
	tree::folder_id(SCOPE, "\0animated")
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
		let id = models_folder_id();
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
				// a heading per game, when they come from more than one
				let groups = models_by_game(ws.models(), &visible);
				let by_game = groups.len() > 1;
				for (game, group) in groups {
					if by_game {
						ui.label(widgets::caption(ui, game_label(game)));
					}
					for i in group {
						let model = &ws.models()[i];
						let key = Row::Model(i);
						let response = library_row(
							ui,
							&p,
							self.model_status(ws, model),
							&model.name,
							open_model == Some(i),
							self.is_selected(key),
							Trailing::Clips(model.playable_clips),
						);
						self.drawn(key);
						if open_model == Some(i) && self.scroll_to_open.replace(false) {
							response.scroll_to_me(Some(egui::Align::Center));
						}
						let response = response.on_hover_text(self.model_hover(ws, model));
						if response.clicked() {
							self.click(ui, key, actions);
						}
						response.context_menu(|ui| self.model_menu(ui, ws, i, actions));
					}
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
						if let Some(dir) = folder_dir(&ws.root, items, child)
							&& ui.button("Show in file browser").clicked()
						{
							actions.push(Action::Reveal(dir));
							ui.close();
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
			let trailing = if item.maps.is_empty() {
				Trailing::None
			} else {
				Trailing::Map
			};
			let response = library_row(
				ui,
				&p,
				status(item),
				&item.name,
				is_open,
				self.is_selected(key),
				trailing,
			);
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

/// What a library row shows at its right end.
#[derive(Clone, Copy)]
enum Trailing {
	/// An animated model's playable clips, counted beside a film strip.
	Clips(usize),
	/// An image icon: the mesh has maps.
	Map,
	/// Nothing: a mesh without maps.
	None,
}

/// A full-width, hoverable library row: status dot, name, and what
/// `trailing` puts at the right end.
fn library_row(
	ui: &mut egui::Ui,
	p: &Palette,
	s: Status,
	name: &str,
	open: bool,
	selected: bool,
	trailing: Trailing,
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
	// the name stops short of what is at the right end
	let (clips, right_pad) = match trailing {
		Trailing::Clips(n) => {
			let clips = format!("{}  {n}", icons::FILM_STRIP);
			let galley = painter.layout_no_wrap(clips, FontId::proportional(CAPTION), p.text_weak);
			let pad = galley.size().x + 16.0;
			(Some(galley), pad)
		}
		Trailing::Map => (None, 26.0),
		Trailing::None => (None, 8.0),
	};
	let text_rect = egui::Rect::from_min_max(
		rect.left_top() + Vec2::new(24.0, 0.0),
		rect.right_bottom() - Vec2::new(right_pad, 0.0),
	);
	let colour = if open { p.accent } else { p.text };
	let name = widgets::truncated(
		&painter,
		name,
		FontId::proportional(LIST_TEXT),
		colour,
		text_rect.width(),
	);
	painter.galley(
		text_rect.left_center() - Vec2::new(0.0, name.size().y / 2.0),
		name,
		colour,
	);
	if let Some(clips) = clips {
		let pos = rect.right_center() - Vec2::new(10.0 + clips.size().x, clips.size().y / 2.0);
		painter.galley(pos, clips, p.text_weak);
	}
	if let Trailing::Map = trailing {
		painter.text(
			rect.right_center() - Vec2::new(10.0, 0.0),
			Align2::RIGHT_CENTER,
			icons::IMAGE,
			FontId::proportional(LIST_TEXT),
			p.text_weak,
		);
	}
	widgets::pointer(response)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ui::library::fixtures::{done, item};

	#[test]
	fn a_folder_is_shown_at_its_own_place_on_disk() {
		let items = [
			item("deep", &["A", "B"], 1, done("A/B/deep.mesh")),
			item("top", &["C"], 2, done("C/top.mesh")),
		];
		let tree = FolderTree::build(items.iter().map(|i| i.folder.as_slice()));
		let root = Path::new("ws");
		let a = &tree.folders["A"];
		// A holds only a subfolder; it once opened A/B (bug B33)
		assert_eq!(folder_dir(root, &items, a), Some(root.join("A")));
		assert_eq!(
			folder_dir(root, &items, &a.folders["B"]),
			Some(root.join("A").join("B"))
		);
		assert_eq!(
			folder_dir(root, &items, &tree.folders["C"]),
			Some(root.join("C"))
		);
	}
}
