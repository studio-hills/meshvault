//! The floating toolbar over the viewport: shading, display toggles,
//! lighting, framing and export.

use eframe::egui::{self, Id, Order, Vec2};

use super::theme::CONTROL_RADIUS;
use super::{icons, widgets};
use crate::app::App;
use crate::render::lighting::Lighting;
use crate::render::viewport::Shading;

impl App {
	/// Floating controls over the top-left of the viewport (see
	/// [`TOOLBAR_RADIUS`]).
	pub(super) fn viewport_toolbar(&mut self, ctx: &egui::Context, rect: egui::Rect) {
		egui::Area::new(Id::unique("viewport_toolbar"))
			.fixed_pos(rect.left_top() + Vec2::new(12.0, 12.0))
			.order(Order::Middle)
			.show(ctx, |ui| {
				// the controls' corners are concentric with the toolbar's
				widgets::floating_bar(ui, TOOLBAR_RADIUS, |ui| {
					ui.horizontal(|ui| {
						ui.spacing_mut().item_spacing.x = 4.0;
						widgets::icon_segmented(
							ui,
							&mut self.shading,
							&[
								(Shading::Solid, icons::CUBE, "Solid  (1)"),
								(
									Shading::SolidWireframe,
									icons::BOUNDING_BOX,
									"Solid + wireframe  (2)",
								),
								(Shading::Wireframe, icons::POLYGON, "Wireframe  (3)"),
							],
						);
						ui.separator();
						let toggles: [(&mut bool, &str, &str); 4] = [
							(&mut self.show_maps, icons::IMAGE, "Maps  (M)"),
							(&mut self.show_grid, icons::GRID_FOUR, "Ground grid  (G)"),
							(&mut self.show_axes, icons::CROSSHAIR, "Axes"),
							(
								&mut self.cull_backfaces,
								icons::EYE_SLASH,
								"Hide back faces",
							),
						];
						for (value, icon, tip) in toggles {
							if widgets::icon_toggle(ui, icon, tip, *value).clicked() {
								*value = !*value;
							}
						}
						if self.model.is_some() {
							for (value, icon, tip) in [
								(&mut self.show_skeleton, icons::BONE, "Skeleton  (K)"),
								(&mut self.show_joints, icons::SHARE_NETWORK, "Joints  (J)"),
							] {
								if widgets::icon_toggle(ui, icon, tip, *value).clicked() {
									*value = !*value;
								}
							}
						}
						let lighting = widgets::icon_button(ui, icons::LIGHTBULB, "Lighting  (L)");
						egui::Popup::menu(&lighting).show(|ui| {
							widgets::menu_style(ui);
							for preset in Lighting::ALL {
								let on = self.config.lighting == preset;
								if ui.selectable_label(on, preset.label()).clicked() {
									self.set_lighting(preset);
								}
							}
						});
						ui.separator();
						if widgets::icon_button(ui, icons::CORNERS_OUT, "Frame model  (F)")
							.clicked()
						{
							self.frame_model();
						}
						if widgets::icon_button(ui, icons::CAMERA, "Save image  (F12)").clicked() {
							self.save_image();
						}
						let export =
							widgets::icon_button(ui, icons::EXPORT, "Export and save images");
						egui::Popup::menu(&export).show(|ui| {
							widgets::menu_style(ui);
							ui.set_min_width(230.0);
							self.export_items(ui);
						});
						widgets::icon_button(
							ui,
							icons::INFO,
							"Left-drag: orbit\nRight or middle-drag: pan\nScroll: zoom\nDouble-click: frame model",
						);
					});
				});
			});
	}
}

/// Corner radius of the viewport toolbar; the controls in it are rounded
/// to this less its padding and border.
const TOOLBAR_RADIUS: u8 = CONTROL_RADIUS + 6;
