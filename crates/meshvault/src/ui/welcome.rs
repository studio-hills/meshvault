//! What the centre of the window shows when no model is open.

use eframe::egui::{self, Align2, FontId, RichText, Sense, Vec2};

use super::theme::{CONTROL_XXS, display, palette, semibold};
use super::{icons, widgets};
use crate::app::App;

impl App {
	pub(super) fn welcome(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		let area = ui.max_rect();
		paint_gradient(ui, area, p.viewport_top, p.viewport_bottom);

		// centred both ways: on the height the content took last frame
		let width = 460.0_f32.min(area.width() - 32.0);
		let height_id = egui::Id::unique("welcome_height");
		let height: f32 = ui.ctx().data(|d| d.get_temp(height_id)).unwrap_or(0.0);
		let top = (area.center().y - height / 2.0).max(area.top() + 24.0);
		let column = egui::Rect::from_min_size(
			egui::pos2(area.center().x - width / 2.0, top),
			Vec2::new(width, area.bottom() - top),
		);
		let shown = ui.scope_builder(
			egui::UiBuilder::new()
				.max_rect(column)
				.layout(egui::Layout::top_down(egui::Align::Center)),
			|ui| {
				// hero icon
				let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
				ui.painter()
					.circle_filled(icon_rect.center(), 32.0, p.accent_soft);
				let icon = match &self.workspace {
					Some(ws) if ws.items().is_empty() => icons::FILE_ARROW_DOWN,
					Some(_) => icons::TREE_STRUCTURE,
					None => icons::CUBE,
				};
				ui.painter().text(
					icon_rect.center(),
					Align2::CENTER_CENTER,
					icon,
					FontId::proportional(30.0),
					p.accent,
				);
				ui.add_space(12.0);

				match &self.workspace {
					None => self.welcome_start(ui),
					Some(ws) if ws.items().is_empty() => {
						title(ui, "Your workspace is empty");
						subtitle(
							ui,
							"Import an export .json to choose which models, folders and maps to download.",
						);
						ui.add_space(14.0);
						centred_row(ui, "welcome_actions", |ui| {
							if widgets::primary_button(
								ui,
								format!("{}  Import export JSON…", icons::FILE_ARROW_DOWN),
							)
							.clicked()
							{
								self.pick_import();
							}
							if widgets::button(
								ui,
								icons::TRAY_ARROW_DOWN,
								"Export from Roblox Studio…",
							)
							.clicked()
							{
								self.show_studio_dialog();
							}
						});
						ui.add_space(14.0);
						if drop_zone(ui, "or drop the .json file anywhere in the window").clicked()
						{
							self.pick_import();
						}
					}
					Some(ws) => {
						let total = ws.items().len();
						let done = ws.items().iter().filter(|i| i.is_done()).count();
						title(ui, "Select a model");
						subtitle(
							ui,
							&format!(
								"Pick a model in the library to view it. {done} of {total} models are downloaded."
							),
						);
						let (pending, _) = self.workspace_counts();
						if pending > 0 && !self.downloader.is_running() {
							ui.add_space(14.0);
							if widgets::primary_button(
								ui,
								format!("{}  Download {pending} pending", icons::CLOUD_ARROW_DOWN),
							)
							.clicked()
							{
								self.start_downloads();
							}
						}
						if !self.show_library {
							ui.add_space(8.0);
							if widgets::button(ui, icons::TREE_STRUCTURE, "Show library").clicked()
							{
								self.show_library = true;
							}
						}
					}
				}
			},
		);
		let used = shown.response.rect.height();
		if (used - height).abs() > 0.5 {
			ui.ctx().data_mut(|d| d.insert_temp(height_id, used));
			ui.ctx().request_repaint();
		}
	}

	fn welcome_start(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		title(ui, "Open a Roblox model");
		subtitle(
			ui,
			"View meshes, unions and physics data with their materials, or download a whole game's models into a workspace.",
		);
		ui.add_space(6.0);
		if ui
			.add(
				egui::Label::new(
					RichText::new(format!(
						"{}  Export a game from Roblox Studio…",
						icons::TRAY_ARROW_DOWN
					))
					.color(p.accent),
				)
				.sense(Sense::click()),
			)
			.on_hover_cursor(egui::CursorIcon::PointingHand)
			.clicked()
		{
			self.show_studio_dialog();
		}
		ui.add_space(16.0);
		centred_row(ui, "welcome_actions", |ui| {
			if widgets::primary_button(ui, format!("{}  Open file…", icons::FILE)).clicked() {
				self.pick_file();
			}
			if widgets::button(ui, icons::FOLDER_PLUS, "New workspace").clicked() {
				self.show_new_workspace(None);
			}
			if widgets::button(ui, icons::FOLDER_OPEN, "Open workspace").clicked() {
				self.pick_open_workspace();
			}
		});
		ui.add_space(16.0);
		if drop_zone(
			ui,
			"Drop a mesh, union data, export .json or images here, or click to browse",
		)
		.clicked()
		{
			self.pick_file();
		}

		let recent = self.recent_workspaces();
		if !recent.is_empty() {
			ui.add_space(18.0);
			ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
				ui.label(widgets::overline("Recent workspaces").color(p.text_weak));
				ui.add_space(2.0);
				for root in recent.into_iter().take(5) {
					match recent_row(ui, &root) {
						RowAction::Open => self.open_workspace(&root),
						RowAction::Remove => {
							self.config.remove_recent(&root);
							self.toasts.info(format!(
								"Removed {} from recent workspaces. Its folder was not touched.",
								root.file_name()
									.map(|n| n.to_string_lossy())
									.unwrap_or_default()
							));
						}
						RowAction::None => {}
					}
				}
			});
		}

		ui.add_space(18.0);
		ui.label(
			RichText::new("Mesh v1–v7 · union graphics CSGMDL v2–v5 · union physics CSGPHS v3–v8")
				.size(crate::ui::theme::CAPTION)
				.color(p.text_weak),
		);
	}
}

enum RowAction {
	None,
	Open,
	Remove,
}

/// One recent workspace: the whole row opens it, with an accent background
/// on hover and a button to take it off the list.
fn recent_row(ui: &mut egui::Ui, root: &std::path::Path) -> RowAction {
	let p = palette(ui.ctx());
	let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover());
	// the row is registered before the remove button, so the button sits on
	// top and a click on it never opens the workspace
	let row = widgets::pointer(ui.interact(
		rect,
		ui.scope_id().with(("recent_row", root)),
		Sense::click(),
	));
	let hovered = ui.rect_contains_pointer(rect);
	let painter = ui.painter_at(rect);
	if hovered {
		painter.rect_filled(rect, 8.0, p.accent_soft);
	}
	let name = root
		.file_name()
		.map(|n| n.to_string_lossy().into_owned())
		.unwrap_or_default();
	let left = rect.left() + 12.0;
	painter.text(
		egui::pos2(left, rect.center().y),
		Align2::LEFT_CENTER,
		icons::FOLDER_SIMPLE,
		FontId::proportional(16.0),
		p.accent,
	);
	let name_rect = painter.text(
		egui::pos2(left + 26.0, rect.center().y),
		Align2::LEFT_CENTER,
		&name,
		FontId::new(14.0, semibold()),
		if hovered { p.accent } else { p.text },
	);
	// the path fills what is left, cut short of the remove button
	let path_clip = egui::Rect::from_min_max(
		egui::pos2(name_rect.right() + 10.0, rect.top()),
		egui::pos2(rect.right() - 44.0, rect.bottom()),
	);
	let path = widgets::truncated(
		&painter,
		&root.display().to_string(),
		FontId::proportional(crate::ui::theme::CAPTION),
		p.text_weak,
		path_clip.width(),
	);
	painter.galley(
		egui::pos2(path_clip.left(), rect.center().y - path.size().y / 2.0),
		path,
		p.text_weak,
	);

	let mut action = if row.clicked() {
		RowAction::Open
	} else {
		RowAction::None
	};
	if hovered {
		let button = egui::Rect::from_center_size(
			rect.right_center() - Vec2::new(22.0, 0.0),
			Vec2::splat(CONTROL_XXS),
		);
		// a detached child: scope_builder would move the parent's cursor to the
		// button's bottom edge and shift everything below the row on hover
		let mut child = ui.new_child(egui::UiBuilder::new().max_rect(button));
		child.spacing_mut().interact_size.y = CONTROL_XXS;
		if widgets::danger_icon_button(&mut child, icons::X, "Remove from recent workspaces")
			.clicked()
		{
			action = RowAction::Remove;
		}
	}
	row.on_hover_text(root.display().to_string());
	action
}

/// A row of buttons centred in the column. egui lays rows out from the
/// left, so the row is moved over by half the space its width (measured
/// the frame before) leaves.
fn centred_row(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
	let id = egui::Id::unique(id);
	let width: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0.0);
	let indent = ((ui.available_width() - width) / 2.0).max(0.0);
	ui.horizontal(|ui| {
		ui.add_space(indent);
		let row = ui.horizontal(add).response.rect.width();
		if (row - width).abs() > 0.5 {
			ui.ctx().data_mut(|d| d.insert_temp(id, row));
			ui.ctx().request_repaint();
		}
	});
}

fn title(ui: &mut egui::Ui, text: &str) {
	ui.label(RichText::new(text).size(24.0).family(display()));
}

fn subtitle(ui: &mut egui::Ui, text: &str) {
	let p = palette(ui.ctx());
	// Cal Sans sets tightly; extra leading keeps wrapped lines readable
	ui.add(
		egui::Label::new(
			RichText::new(text)
				.color(p.text_weak)
				.size(14.0)
				.line_height(Some(super::theme::BODY_LINE_HEIGHT)),
		)
		.wrap(),
	);
}

/// A dashed drop target that can also be clicked to browse.
fn drop_zone(ui: &mut egui::Ui, text: &str) -> egui::Response {
	let p = palette(ui.ctx());
	let (rect, response) =
		ui.allocate_exact_size(Vec2::new(ui.available_width(), 72.0), Sense::click());
	let response = widgets::pointer(response);
	let fill = if response.hovered() {
		p.raised
	} else {
		p.panel.gamma_multiply(0.6)
	};
	ui.painter().rect_filled(rect, 10.0, fill);
	widgets::dashed_rect(ui, rect, p.text_weak.gamma_multiply(0.55));
	ui.painter().text(
		rect.center() - Vec2::new(0.0, 10.0),
		Align2::CENTER_CENTER,
		icons::TRAY_ARROW_DOWN,
		FontId::proportional(20.0),
		p.text_weak,
	);
	ui.painter().text(
		rect.center() + Vec2::new(0.0, 14.0),
		Align2::CENTER_CENTER,
		text,
		FontId::proportional(12.5),
		p.text_weak,
	);
	response
}

/// A vertical gradient matching the 3D viewport's background.
pub fn paint_gradient(ui: &egui::Ui, rect: egui::Rect, top: egui::Color32, bottom: egui::Color32) {
	let mut mesh = egui::Mesh::default();
	mesh.colored_vertex(rect.left_top(), top);
	mesh.colored_vertex(rect.right_top(), top);
	mesh.colored_vertex(rect.right_bottom(), bottom);
	mesh.colored_vertex(rect.left_bottom(), bottom);
	mesh.add_triangle(0, 1, 2);
	mesh.add_triangle(0, 2, 3);
	ui.painter().add(egui::Shape::mesh(mesh));
}
