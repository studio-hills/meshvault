//! The grid view: thumbnails, grouped by folder.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Sense, Stroke, Vec2};
use eframe::emath;

use super::list::nothing_matches;
use super::menus::item_hover;
use super::{
	Action, Browser, ModelKind, Opened, Row, Status, game_label, models_by_game, status, status_dot,
};
use crate::render::thumbnails::Subject;
use crate::ui::theme::{CAPTION, CARD_RADIUS, palette};
use crate::ui::{icons, widgets};
use crate::workspace::{Item, Workspace};

/// Smallest grid tile.
const TILE: f32 = 92.0;
/// Space between grid tiles.
const GAP: f32 = 6.0;
/// Space below a picture for its name.
const LABEL_HEIGHT: f32 = 20.0;
/// How long a thumbnail takes to fade in over its icon when it arrives, in
/// seconds.
const THUMBNAIL_FADE: f32 = 0.15;

/// A run of tiles under one heading.
#[derive(Debug, PartialEq)]
struct Section {
	title: String,
	/// The folder's whole path, shown on hover; empty for none.
	path: String,
	rows: Vec<Row>,
}

/// Models grouped for the grid: per kind, its groups of rows by game.
type ModelGroups<'a> = Vec<(ModelKind, Vec<(Option<&'a str>, Vec<Row>)>)>;

/// The grid's sections: the models first, by kind (animated, then static)
/// with a section per game when a kind's come from more than one, then the
/// meshes in `meshes` sorted by folder and name, one section per folder.
fn grid_sections(items: &[Item], models: ModelGroups<'_>, mut meshes: Vec<usize>) -> Vec<Section> {
	let mut sections: Vec<Section> = Vec::new();
	for (kind, groups) in models {
		let by_game = groups.len() > 1;
		for (game, rows) in groups {
			let title = if by_game {
				format!("{} · {}", kind.title(), game_label(game))
			} else {
				kind.title()
			};
			sections.push(Section {
				title,
				path: String::new(),
				rows,
			});
		}
	}
	meshes.sort_by(|&a, &b| {
		items[a].folder.cmp(&items[b].folder).then_with(|| {
			items[a]
				.name
				.to_lowercase()
				.cmp(&items[b].name.to_lowercase())
		})
	});
	// a folder's section is titled with its own name; its path is on hover.
	// Only folder sections are joined: root meshes share the models' empty
	// path but not their section.
	let folders_from = sections.len();
	for i in meshes {
		let path = items[i].folder.join(" / ");
		match sections[folders_from..].last_mut() {
			Some(section) if section.path == path => section.rows.push(Row::Mesh(i)),
			_ => {
				let name = items[i].folder.last().map_or("", String::as_str);
				sections.push(Section {
					title: format!("{}  {name}", icons::FOLDER_SIMPLE),
					path,
					rows: vec![Row::Mesh(i)],
				});
			}
		}
	}
	sections
}

impl Browser {
	/// Thumbnails: models first, then meshes by folder.
	pub(super) fn grid(
		&mut self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		opened: Opened<'_>,
		actions: &mut Vec<Action>,
	) {
		let items = ws.items();
		let shown: Vec<usize> = (0..ws.models().len())
			.filter(|&i| self.model_matches(ws, &ws.models()[i]))
			.collect();
		let models = ModelKind::ALL
			.into_iter()
			.map(|kind| {
				let groups = models_by_game(ws.models(), &kind.among(ws.models(), &shown))
					.into_iter()
					.map(|(game, group)| (game, group.into_iter().map(Row::Model).collect()))
					.collect();
				(kind, groups)
			})
			.collect();
		let meshes: Vec<usize> = (0..items.len())
			.filter(|&i| self.matches(&items[i]))
			.collect();
		let sections = grid_sections(items, models, meshes);
		if sections.is_empty() {
			nothing_matches(ui);
			return;
		}
		let width = ui.available_width();
		let columns = ((width + GAP) / (TILE + GAP)).floor().max(1.0) as usize;
		let side = (width - GAP * (columns - 1) as f32) / columns as f32;
		for (section_index, section) in sections.into_iter().enumerate() {
			ui.add_space(4.0);
			let label = ui.add(egui::Label::new(widgets::caption(ui, section.title)).truncate());
			if !section.path.is_empty() {
				label.on_hover_text(section.path);
			}
			ui.add_space(2.0);
			// Selection ranges include the whole grid, even its clipped rows.
			self.selection
				.borrow_mut()
				.drawn
				.extend_from_slice(&section.rows);
			for (row_index, row) in section.rows.chunks(columns).enumerate() {
				let size = Vec2::new(width, side + LABEL_HEIGHT);
				let rect = egui::Rect::from_min_size(ui.next_widget_position(), size);
				if ui.is_rect_visible(rect) {
					ui.push_id((section_index, row_index), |ui| {
						ui.horizontal(|ui| {
							ui.spacing_mut().item_spacing.x = GAP;
							for &key in row {
								self.tile(ui, ws, key, side, opened, actions);
							}
						});
					});
				} else {
					ui.allocate_space(size);
				}
				ui.add_space(GAP - ui.spacing().item_spacing.y);
			}
		}
	}

	fn tile(
		&mut self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		key: Row,
		side: f32,
		opened: Opened<'_>,
		actions: &mut Vec<Action>,
	) {
		let (rect, response) =
			ui.allocate_exact_size(Vec2::new(side, side + LABEL_HEIGHT), Sense::click());
		if !ui.is_rect_visible(rect) {
			return;
		}
		let p = palette(ui.ctx());
		let (name, open, s, subject) = match key {
			Row::Model(i) => {
				let m = &ws.models()[i];
				(
					m.name.as_str(),
					opened.is_model(i),
					self.model_status(ws, m),
					Subject::Model(m.path.clone()),
				)
			}
			Row::Mesh(i) => {
				let item = &ws.items()[i];
				(
					item.name.as_str(),
					opened.is_item(item),
					status(item),
					Subject::Item(item.stem.clone()),
				)
			}
		};
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
		// a thumbnail that arrives while the tile is shown fades in over
		// its icon; one already there when the tile first shows does not
		let shown = ui.ctx().animate_bool_with_time_and_easing(
			egui::Id::unique("thumbnail_fade").with(&subject),
			thumbnail.is_some(),
			THUMBNAIL_FADE,
			emath::easing::cubic_out,
		);
		if let Some(texture) = &thumbnail {
			painter.image(
				texture.id(),
				picture.shrink(4.0),
				egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
				Color32::WHITE.gamma_multiply(shown),
			);
		}
		if shown < 1.0 {
			let icon = match key {
				Row::Model(i) if ws.models()[i].is_animated() => icons::PERSON_SIMPLE_RUN,
				Row::Model(_) => icons::PACKAGE,
				Row::Mesh(_) => icons::CUBE,
			};
			painter.text(
				picture.center(),
				Align2::CENTER_CENTER,
				icon,
				FontId::proportional(26.0),
				p.text_weak.gamma_multiply(0.6 * (1.0 - shown)),
			);
		}
		// downloads still to come, or failed, show in the corner
		if s != Status::Done {
			status_dot(&painter, &p, picture.left_top() + Vec2::new(10.0, 10.0), s);
		}
		if let Row::Model(i) = key
			&& ws.models()[i].is_animated()
		{
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
		let response = widgets::pointer(response);
		let response = match key {
			Row::Model(i) => response.on_hover_ui(|ui| {
				ui.label(self.model_hover(ws, &ws.models()[i]));
			}),
			Row::Mesh(i) => response.on_hover_ui(|ui| {
				ui.label(format!(
					"{}\n{}",
					ws.items()[i].folder.join(" / "),
					item_hover(&ws.items()[i])
				));
			}),
		};
		if response.clicked() {
			self.click(ui, key, actions);
		}
		response.context_menu(|ui| match key {
			Row::Model(i) => self.model_menu(ui, ws, i, actions),
			Row::Mesh(i) => self.item_menu(ui, ws, i, actions),
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ui::library::fixtures::{done, item};
	use crate::workspace::AssetState;

	#[test]
	fn clipped_rows_keep_the_grids_height_and_selection_order() {
		let dir = crate::test_support::TempDir::new("grid_rows");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let items = (0..120)
			.map(|i| item(&format!("Mesh {i:03}"), &["Props"], i, AssetState::Pending))
			.collect();
		ws.set_contents(items, Vec::new());
		let ctx = egui::Context::default();
		crate::ui::theme::install_fonts(&ctx);
		let mut browser = Browser::default();
		for width in [140.0, 460.0] {
			let mut height = None;
			for offset in [0.0, 270.0, 900.0, 0.0] {
				let mut output = ctx.run_ui(
					egui::RawInput {
						screen_rect: Some(egui::Rect::from_min_size(
							egui::Pos2::ZERO,
							Vec2::new(width, 240.0),
						)),
						..Default::default()
					},
					|ui| {
						browser.selection.borrow_mut().drawn.clear();
						let scroll = egui::ScrollArea::vertical()
							.vertical_scroll_offset(offset)
							.show(ui, |ui| {
								browser.grid(
									ui,
									&ws,
									Opened {
										stem: None,
										model: None,
									},
									&mut Vec::new(),
								);
							});
						let previous = *height.get_or_insert(scroll.content_size.y);
						assert!((previous - scroll.content_size.y).abs() < 1.0);
					},
				);
				output.textures_delta.clear();
				let expected: Vec<_> = (0..120).map(Row::Mesh).collect();
				let selection = browser.selection.borrow();
				assert_eq!(selection.drawn, expected);
			}
		}
	}

	// ----------------------------------------------------------------- grid

	fn section(title: String, path: &str, rows: &[Row]) -> Section {
		Section {
			title,
			path: path.into(),
			rows: rows.to_vec(),
		}
	}

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
		let sections = grid_sections(&items, Vec::new(), (0..items.len()).collect());
		assert_eq!(
			sections,
			[
				section(folder_title(""), "", &[Row::Mesh(1)]),
				// names sort without regard to case
				section(
					folder_title("Props"),
					"Props",
					&[Row::Mesh(3), Row::Mesh(0), Row::Mesh(5)]
				),
				section(folder_title("Indoor"), "Props / Indoor", &[Row::Mesh(4)]),
				section(folder_title("Sea"), "Sea", &[Row::Mesh(2)]),
			]
		);
		// with nothing to show there are no sections, so the grid says nothing matches
		assert!(grid_sections(&items, Vec::new(), Vec::new()).is_empty());
	}

	#[test]
	fn models_from_several_games_get_a_section_each() {
		let items = grid_items();
		let sections = grid_sections(
			&items,
			vec![(
				ModelKind::Animated,
				vec![
					(Some("Pets"), vec![Row::Model(0)]),
					(None, vec![Row::Model(1)]),
				],
			)],
			Vec::new(),
		);
		let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
		let run = icons::PERSON_SIMPLE_RUN;
		assert_eq!(
			titles,
			[
				format!("{run}  Animated models · Pets"),
				format!("{run}  Animated models · Other"),
			]
		);
	}

	#[test]
	fn grid_puts_models_first_and_root_meshes_in_their_own_section() {
		// root meshes once joined the "Animated models" section (bug B32)
		let items = grid_items();
		let sections = grid_sections(
			&items,
			vec![
				(
					ModelKind::Animated,
					vec![(None, vec![Row::Model(1), Row::Model(0)])],
				),
				// static models next, under their own heading
				(ModelKind::Static, vec![(Some("Town"), vec![Row::Model(2)])]),
			],
			vec![5, 1, 2],
		);
		assert_eq!(
			sections,
			[
				section(
					format!("{}  Animated models", icons::PERSON_SIMPLE_RUN),
					"",
					&[Row::Model(1), Row::Model(0)],
				),
				section(
					format!("{}  Static models", icons::PACKAGE),
					"",
					&[Row::Model(2)],
				),
				section(folder_title(""), "", &[Row::Mesh(1)]),
				section(folder_title("Props"), "Props", &[Row::Mesh(5)]),
				section(folder_title("Sea"), "Sea", &[Row::Mesh(2)]),
			]
		);
	}
}
