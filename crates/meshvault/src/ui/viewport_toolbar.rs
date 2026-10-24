//! The floating toolbar over the viewport: shading, display toggles,
//! lighting, framing and export.

use eframe::egui::{self, Id, Order, Vec2};

use super::theme::CONTROL_RADIUS;
use super::{icons, widgets};
use crate::app::App;
use crate::render::lighting::Lighting;
use crate::render::viewport::{AXES_REACH, Shading};

impl App {
	/// Floating controls over the top-left of the viewport (see
	/// [`TOOLBAR_RADIUS`]). In a viewport too narrow for them in one row,
	/// the actions go on a second row under the view controls.
	pub(super) fn viewport_toolbar(&mut self, ctx: &egui::Context, rect: egui::Rect) {
		let room = toolbar_room(rect.width(), self.show_axes);
		// the controls' width in one row, as last measured; it differs with
		// and without an animated model's overlay toggles
		let id = Id::unique("viewport_toolbar").with(self.model.is_some());
		let one_row: Option<f32> = ctx.data(|d| d.get_temp(id));
		let two_rows = one_row.is_some_and(|w| w > room);
		egui::Area::new(Id::unique("viewport_toolbar"))
			.fixed_pos(rect.left_top() + Vec2::new(TOOLBAR_INSET, TOOLBAR_INSET))
			.order(Order::Middle)
			.show(ctx, |ui| {
				// the controls' corners are concentric with the toolbar's
				widgets::floating_bar(ui, TOOLBAR_RADIUS, |ui| {
					if two_rows {
						ui.vertical(|ui| {
							ui.spacing_mut().item_spacing.y = 4.0;
							ui.horizontal(|ui| self.view_controls(ui));
							ui.horizontal(|ui| self.action_controls(ui));
						});
					} else {
						let row = ui.horizontal(|ui| {
							self.view_controls(ui);
							ui.separator();
							self.action_controls(ui);
						});
						let width = row.response.rect.width();
						ctx.data_mut(|d| d.insert_temp(id, width));
					}
				});
			});
	}

	/// Shading, display toggles and lighting.
	fn view_controls(&mut self, ui: &mut egui::Ui) {
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
				(
					&mut self.show_attachments,
					icons::PUSH_PIN,
					"Attachments and pivot  (A)",
				),
				(&mut self.show_effects, icons::SPARKLE, "Effects  (P)"),
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
	}

	/// Framing, saving and exporting, and the mouse controls on hover.
	fn action_controls(&mut self, ui: &mut egui::Ui) {
		ui.spacing_mut().item_spacing.x = 4.0;
		if widgets::icon_button(ui, icons::CORNERS_OUT, "Frame model  (F)").clicked() {
			self.frame_model();
		}
		if widgets::icon_button(ui, icons::CAMERA, "Save image  (F12)").clicked() {
			self.save_image();
		}
		let export = widgets::icon_button(ui, icons::EXPORT, "Export and save images");
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
	}
}

/// Corner radius of the viewport toolbar; the controls in it are rounded
/// to this less its padding and border.
const TOOLBAR_RADIUS: u8 = CONTROL_RADIUS + 6;

/// How far the toolbar sits in from the viewport's top-left corner, and the
/// least room it leaves at the right.
const TOOLBAR_INSET: f32 = 12.0;
/// The bar's padding and border on either side of its controls.
const TOOLBAR_PADDING: f32 = widgets::FLOATING_PAD as f32 + 1.0;

/// The room for the toolbar's controls in a viewport `width` wide: inside
/// its insets and padding, and clear of the axes gizmo when it is shown.
fn toolbar_room(width: f32, axes: bool) -> f32 {
	let right = if axes { AXES_REACH } else { TOOLBAR_INSET };
	(width - TOOLBAR_INSET - right - 2.0 * TOOLBAR_PADDING).max(0.0)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_toolbar_stays_inside_the_viewport_and_clear_of_the_axes() {
		for width in [420.0, 670.0, 1310.0] {
			let bar = toolbar_room(width, false) + 2.0 * TOOLBAR_PADDING;
			assert_eq!(TOOLBAR_INSET + bar + TOOLBAR_INSET, width, "{width}");
			let bar = toolbar_room(width, true) + 2.0 * TOOLBAR_PADDING;
			assert_eq!(TOOLBAR_INSET + bar + AXES_REACH, width, "{width}");
		}
		assert_eq!(toolbar_room(10.0, true), 0.0);
	}
}
