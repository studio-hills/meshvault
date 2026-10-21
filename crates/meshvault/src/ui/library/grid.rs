//! The grid view: thumbnails, grouped by folder.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Sense, Stroke, Vec2};

use super::list::nothing_matches;
use super::menus::item_hover;
use super::{Action, Browser, Key, Status, status, status_dot};
use crate::render::thumbnails::Subject;
use crate::ui::theme::{CAPTION, CARD_RADIUS, palette};
use crate::ui::{icons, widgets};
use crate::workspace::{Item, Workspace};

/// Smallest grid tile, and the gap between tiles.
const TILE: f32 = 92.0;
const GAP: f32 = 6.0;

/// The grid's sections, as (title, keys), and each one's hover path: the
/// animated models in `models` first, then the meshes in `meshes` sorted by
/// folder and name, one section per folder.
pub(super) fn grid_sections(
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

impl Browser {
	/// Thumbnails: animated models first, then meshes by folder.
	pub(super) fn grid(
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
		let width = ui.available_width();
		let columns = ((width + GAP) / (TILE + GAP)).floor().max(1.0) as usize;
		let side = (width - GAP * (columns - 1) as f32) / columns as f32;
		for ((title, keys), path) in sections.into_iter().zip(paths) {
			ui.add_space(4.0);
			let label = ui.add(egui::Label::new(widgets::caption(ui, title)).truncate());
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
			painter.rect_filled(picture, CornerRadius::same(CARD_RADIUS), fill);
			if open {
				painter.rect_stroke(
					picture,
					CornerRadius::same(CARD_RADIUS),
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
					FontId::proportional(CAPTION),
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
				FontId::proportional(CAPTION),
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
