//! The list view: the export's folder tree, one row per model.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Vec2};

use super::menus::item_hover;
use super::{
	Action, Browser, Filters, ModelKind, Opened, Row, SCOPE, Status, StatusFilter, game_label,
	models_by_game, status, status_dot,
};
use crate::ui::theme::{CAPTION, CONTROL_RADIUS, LIST_TEXT, Palette, ROW, palette};
use crate::ui::tree::{self, FlatKind, FlatTree, FolderTree};
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
	let mut stack = vec![(node, 0)];
	let (i, depth) = loop {
		let (node, depth) = stack.pop()?;
		if let Some(&i) = node.items.first() {
			break (i, depth);
		}
		stack.extend(node.folders.values().rev().map(|child| (child, depth + 1)));
	};
	let mut dir = root.join(&items.get(i)?.stem);
	for _ in 0..=depth {
		dir = dir.parent()?.to_path_buf();
	}
	Some(dir)
}

/// A models section's folder, which is not in the folder tree. Its path is
/// not a valid folder path, so it cannot clash with an export folder.
pub(super) fn models_folder_id(kind: ModelKind) -> egui::Id {
	tree::folder_id(
		SCOPE,
		match kind {
			ModelKind::Animated => "\0animated",
			ModelKind::Static => "\0static",
		},
	)
}

#[derive(Default)]
pub(super) struct ListCache {
	pub flat: FlatTree,
	key: Option<(String, StatusFilter, Filters, u64, u64)>,
	revision: Option<u64>,
	matches: Vec<bool>,
	totals: Vec<usize>,
	done: Vec<usize>,
	failed: Vec<usize>,
	model_status: Vec<Status>,
	all_models: [Vec<usize>; 2],
	groups: [Vec<(Option<String>, Vec<usize>)>; 2],
	visible: Vec<ListLine>,
	mesh_positions: Vec<Option<usize>>,
	model_positions: Vec<Option<usize>>,
}

impl ListCache {
	pub(super) fn invalidate_visibility(&mut self) {
		self.revision = None;
	}
}

enum ListLine {
	Section(ModelKind),
	Game(String),
	Model(usize),
	Tree(usize),
}

impl Browser {
	/// Filtering and folder counts are paid for when the data or filters
	/// change. Opening a folder changes only the list of visible rows.
	pub(super) fn refresh_list(&mut self, ctx: &egui::Context, ws: &Workspace) {
		let facts = self.facts.as_ref().map_or(0, |f| f.generation());
		let key = (
			self.needle.clone(),
			self.status,
			self.filters.clone(),
			ws.asset_generation(),
			facts,
		);
		if self.list.key.as_ref() != Some(&key) {
			self.list.matches = ws.items().iter().map(|item| self.matches(item)).collect();
			self.list.totals = self.list.flat.totals(|i| self.list.matches[i] as usize);
			self.list.done = self.list.flat.totals(|i| ws.items()[i].is_done() as usize);
			self.list.failed = self
				.list
				.flat
				.totals(|i| ws.items()[i].has_failure() as usize);
			self.list.model_status = ws
				.models()
				.iter()
				.map(|m| self.model_status(ws, m))
				.collect();
			let shown: Vec<_> = (0..ws.models().len())
				.filter(|&i| self.model_matches(ws, &ws.models()[i]))
				.collect();
			let all: Vec<_> = (0..ws.models().len()).collect();
			for (n, kind) in ModelKind::ALL.into_iter().enumerate() {
				self.list.all_models[n] = kind.among(ws.models(), &all);
				self.list.groups[n] = models_by_game(ws.models(), &kind.among(ws.models(), &shown))
					.into_iter()
					.map(|(game, group)| (game.map(str::to_owned), group))
					.collect();
			}
			self.list.key = Some(key);
			self.list.revision = None;
		}
		if self.list.revision == Some(tree::revision(ctx)) {
			return;
		}
		self.list.visible.clear();
		for (n, kind) in ModelKind::ALL.into_iter().enumerate() {
			if self.list.groups[n].is_empty() {
				continue;
			}
			let id = models_folder_id(kind);
			if self.narrowed() {
				tree::set_open(ctx, id, true);
			}
			self.list.visible.push(ListLine::Section(kind));
			if tree::is_open(ctx, id, true) {
				let by_game = self.list.groups[n].len() > 1;
				for (game, group) in &self.list.groups[n] {
					if by_game {
						self.list
							.visible
							.push(ListLine::Game(game_label(game.as_deref()).to_owned()));
					}
					self.list
						.visible
						.extend(group.iter().copied().map(ListLine::Model));
				}
			}
		}
		let visible =
			self.list
				.flat
				.visible(ctx, &self.list.matches, &self.list.totals, self.narrowed());
		self.list
			.visible
			.extend(visible.into_iter().map(ListLine::Tree));
		self.list.mesh_positions = vec![None; ws.items().len()];
		self.list.model_positions = vec![None; ws.models().len()];
		let mut selection = self.selection.borrow_mut();
		selection.order.clear();
		for (position, line) in self.list.visible.iter().enumerate() {
			match *line {
				ListLine::Model(i) => {
					self.list.model_positions[i] = Some(position);
					selection.order.push(Row::Model(i));
				}
				ListLine::Tree(row) => {
					if let FlatKind::Item(i) = self.list.flat.rows[row].kind {
						self.list.mesh_positions[i] = Some(position);
						selection.order.push(Row::Mesh(i));
					}
				}
				_ => {}
			}
		}
		self.list.revision = Some(tree::revision(ctx));
	}

	pub(super) fn list(
		&mut self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		opened: Opened<'_>,
		actions: &mut Vec<Action>,
	) {
		self.refresh_list(ui.ctx(), ws);
		if self.list.visible.is_empty() {
			nothing_matches(ui);
			return;
		}
		let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
		if self.scroll_to_open.get() {
			let position = opened
				.model
				.and_then(|i| self.list.model_positions.get(i).copied().flatten())
				.or_else(|| {
					opened
						.stem
						.and_then(|s| self.stems.get(s))
						.and_then(|&i| self.list.mesh_positions[i])
				});
			if let Some(position) = position {
				let stride = ROW + ui.spacing().item_spacing.y;
				scroll = scroll.vertical_scroll_offset(
					(position as f32 * stride - (ui.available_height() - ROW) / 2.0).max(0.0),
				);
				self.scroll_to_open.set(false);
			}
		}
		widgets::scroll_rows_in_margin(ui, scroll, ROW, self.list.visible.len(), |ui, range| {
			ui.spacing_mut().interact_size.y = ROW;
			ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
			for position in range {
				let line = &self.list.visible[position];
				let (depth, key) = match *line {
					ListLine::Section(kind) => (0, models_folder_id(kind)),
					ListLine::Game(_) => (1, egui::Id::unique(("library_game", position))),
					ListLine::Model(i) => (1, egui::Id::unique(("library_model", i))),
					ListLine::Tree(row) => (
						self.list.flat.rows[row].depth,
						egui::Id::unique(("library_tree", row)),
					),
				};
				ui.scope_builder(egui::UiBuilder::new().scope_id(key.with("row")), |ui| {
					ui.horizontal(|ui| {
						ui.set_min_height(ROW);
						if depth > 0 {
							ui.add_space(depth as f32 * ui.spacing().indent);
						}
						match line {
							ListLine::Section(kind) => self.model_section(ui, *kind, actions),
							ListLine::Game(title) => {
								ui.label(widgets::caption(ui, title));
							}
							ListLine::Model(i) => {
								self.list_model(ui, ws, *i, opened.model, actions)
							}
							ListLine::Tree(row) => match self.list.flat.rows[*row].kind {
								FlatKind::Folder { .. } => self.list_folder(ui, ws, *row, actions),
								FlatKind::Item(i) => {
									self.list_mesh(ui, ws, i, opened.stem, actions)
								}
							},
						}
					});
				});
			}
		});
	}

	fn model_section(&self, ui: &mut egui::Ui, kind: ModelKind, actions: &mut Vec<Action>) {
		let n = ModelKind::ALL
			.iter()
			.position(|&k| k == kind)
			.expect("known kind");
		let all = &self.list.all_models[n];
		let id = models_folder_id(kind);
		let clicked = tree::header(ui, id, true, |ui| {
			let response = widgets::pointer(
				ui.add(
					egui::Label::new(RichText::new(kind.title()))
						.sense(Sense::click())
						.truncate(),
				),
			);
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.label(widgets::caption(ui, all.len().to_string()));
			});
			let clicked = response.clicked();
			response.context_menu(|ui| {
				widgets::menu_style(ui);
				if widgets::danger_item(
					ui,
					format!("{}  Remove all {}", icons::TRASH, kind.plural()),
				)
				.clicked()
				{
					actions.push(Action::RemoveModels(all.clone()));
					ui.close();
				}
			});
			clicked
		});
		if clicked {
			tree::toggle(ui, id);
		}
	}

	fn list_model(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		i: usize,
		open: Option<usize>,
		actions: &mut Vec<Action>,
	) {
		let model = &ws.models()[i];
		let key = Row::Model(i);
		let trailing = if model.is_animated() {
			Trailing::Clips(model.playable_clips)
		} else {
			Trailing::None
		};
		let response = library_row(
			ui,
			&palette(ui.ctx()),
			self.list.model_status[i],
			&model.name,
			open == Some(i),
			self.is_selected(key),
			trailing,
		);
		if response.clicked() {
			self.click(ui, key, actions);
		}
		response
			.on_hover_ui(|ui| {
				ui.label(self.model_hover(ws, model));
			})
			.context_menu(|ui| self.model_menu(ui, ws, i, actions));
	}

	fn list_mesh(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		i: usize,
		open: Option<&str>,
		actions: &mut Vec<Action>,
	) {
		let item = &ws.items()[i];
		let key = Row::Mesh(i);
		let trailing = if item.maps.is_empty() {
			Trailing::None
		} else {
			Trailing::Map
		};
		let response = library_row(
			ui,
			&palette(ui.ctx()),
			status(item),
			&item.name,
			open == Some(item.stem.as_str()),
			self.is_selected(key),
			trailing,
		);
		if response.clicked() {
			self.click(ui, key, actions);
		}
		response
			.on_hover_ui(|ui| {
				ui.label(item_hover(item));
			})
			.context_menu(|ui| self.item_menu(ui, ws, i, actions));
	}

	fn list_folder(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		row: usize,
		actions: &mut Vec<Action>,
	) {
		let FlatKind::Folder { name, id, .. } = &self.list.flat.rows[row].kind else {
			return;
		};
		let all = self.list.flat.folder_items(row);
		let done = self.list.flat.count(row, &self.list.done);
		let failed_count = self.list.flat.count(row, &self.list.failed);
		let p = palette(ui.ctx());
		let clicked = tree::header(ui, *id, false, |ui| {
			let colour = if failed_count == 0 { p.text } else { p.danger };
			let text = RichText::new(format!("{}  {name}", icons::FOLDER_SIMPLE)).color(colour);
			let response =
				widgets::pointer(ui.add(egui::Label::new(text).sense(Sense::click()).truncate()));
			let clicked = response.clicked();
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.label(widgets::caption(ui, format!("{done}/{}", all.len())));
			});
			response.context_menu(|ui| {
				widgets::menu_style(ui);
				if ui
					.add_enabled(
						failed_count > 0,
						egui::Button::new(format!("Retry {failed_count} failed")),
					)
					.clicked()
				{
					actions.push(Action::Retry(
						all.iter()
							.copied()
							.filter(|&i| ws.items()[i].has_failure())
							.collect(),
					));
					ui.close();
				}
				if ui.button("Select all inside").clicked() {
					self.selection.borrow_mut().select(
						all.iter()
							.copied()
							.filter(|&i| self.list.matches[i])
							.map(Row::Mesh),
					);
					ui.close();
				}
				let path = self.list.flat.path(row);
				let mut node = &self.tree;
				for part in path.split('/') {
					if let Some(child) = node.folders.get(part) {
						node = child;
					}
				}
				if let Some(dir) = folder_dir(&ws.root, ws.items(), node)
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
					actions.push(Action::RemoveItems(all.to_vec()));
					ui.close();
				}
			});
			clicked
		});
		if clicked {
			tree::toggle(ui, *id);
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
	fn cached_list_keeps_offscreen_selection_order_and_reacts_to_changes() {
		let dir = crate::test_support::TempDir::new("virtual_list");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let items = (0..200)
			.map(|i| {
				item(
					&format!("Item{i}"),
					&["Folder"],
					i,
					crate::workspace::AssetState::Pending,
				)
			})
			.collect();
		ws.set_contents(items, vec![]);
		let ctx = egui::Context::default();
		let mut browser = Browser::default();
		browser.rebuild_if_needed(&ws);
		tree::open_path(&ctx, SCOPE, &["Folder".into()]);
		browser.refresh_list(&ctx, &ws);
		assert_eq!(browser.selection.borrow().order.len(), 200);
		assert_eq!(browser.list.mesh_positions[199], Some(200));
		let rows = browser.list.visible.as_ptr();
		browser.refresh_list(&ctx, &ws);
		assert_eq!(browser.list.visible.as_ptr(), rows);
		browser.selection.borrow_mut().anchor = Some(Row::Mesh(0));
		let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
			ui.input_mut(|input| input.modifiers.shift = true);
			browser.click(ui, Row::Mesh(199), &mut Vec::new());
		});
		output.textures_delta.clear();
		assert_eq!(browser.selection.borrow().keys.len(), 200);
		tree::set_open(&ctx, tree::folder_id(SCOPE, "Folder"), false);
		browser.refresh_list(&ctx, &ws);
		assert!(browser.selection.borrow().order.is_empty());
		browser.search = "Item199".into();
		browser.update_needle();
		browser.refresh_list(&ctx, &ws);
		assert_eq!(browser.selection.borrow().order, [Row::Mesh(199)]);
		assert_eq!(browser.list.flat.count(0, &browser.list.totals), 1);
		browser.search.clear();
		browser.update_needle();
		let stem = ws.items()[199].stem.clone();
		ws.set_state(
			&stem,
			crate::workspace::AssetKind::Mesh,
			done("Folder/Item199.mesh"),
		);
		browser.status = StatusFilter::Downloaded;
		browser.refresh_list(&ctx, &ws);
		assert_eq!(browser.selection.borrow().order, [Row::Mesh(199)]);
		assert_eq!(browser.list.flat.count(0, &browser.list.done), 1);
		let extra = crate::studio::export_file::parse(
			r#"[{"Name":"Replacement","MeshId":"rbxassetid://999"}]"#,
		)
		.unwrap();
		ws.add(extra.entries);
		browser.rebuild_if_needed(&ws);
		browser.status = StatusFilter::All;
		browser.refresh_list(&ctx, &ws);
		assert_eq!(browser.selection.borrow().order.len(), 201);
		assert!(browser.selection.borrow().keys.is_empty());
	}

	#[test]
	fn reveal_scrolls_to_an_offscreen_row() {
		let dir = crate::test_support::TempDir::new("virtual_reveal");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		ws.set_contents(
			(0..200)
				.map(|i| {
					item(
						&format!("Item{i}"),
						&["Folder"],
						i,
						crate::workspace::AssetState::Pending,
					)
				})
				.collect(),
			vec![],
		);
		let ctx = egui::Context::default();
		crate::ui::theme::install_fonts(&ctx);
		crate::ui::theme::apply(&ctx, Default::default());
		let mut browser = Browser {
			reveal: true,
			..Default::default()
		};
		let mut output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(600.0, 500.0),
				)),
				..Default::default()
			},
			|ui| {
				browser.show(ui, &ws, Some(&ws.items()[199].stem), None, None);
			},
		);
		output.textures_delta.clear();
		assert!(!browser.scroll_to_open.get());
		let (drawn, offset) = widgets::SCROLL_METRICS.get();
		assert!(drawn < 30);
		assert!(
			offset > 4000.0,
			"the target was below the viewport: {offset}"
		);
	}

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
