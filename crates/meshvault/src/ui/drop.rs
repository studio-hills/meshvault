//! Files dropped onto the window, and the overlay while they hover over it.

use eframe::egui::{self, Align2, Color32, FontId, Id, Order, Stroke, Vec2};

use super::icons;
use super::theme::{palette, semibold};
use crate::app::App;

impl App {
	pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
		let dropped = ctx.input(|i| i.raw.dropped_files.clone());
		for file in dropped {
			let path = file.path().to_path_buf();
			match file.bytes() {
				Ok(bytes) => self.open_bytes(path, &bytes),
				Err(e) => self
					.toasts
					.error(format!("Could not read {}: {e}", path.display())),
			}
		}
	}

	/// Dims the window and explains what dropping will do.
	pub(super) fn drop_overlay(&self, ctx: &egui::Context) {
		let hovered = ctx.input(|i| i.raw.hovered_files.clone());
		let Some(first) = hovered.first() else { return };
		let p = palette(ctx);
		let path = first.path.clone().unwrap_or_default();
		let ext = path
			.extension()
			.map(|e| e.to_string_lossy().to_lowercase())
			.unwrap_or_default();
		let is_rig = path
			.file_name()
			.is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(".rig.json"));
		let (icon, title) = if is_rig {
			(icons::PERSON_SIMPLE_RUN, "Open animated model")
		} else if ext == "json" {
			(icons::FILE_ARROW_DOWN, "Import export file")
		} else if crate::scene::texture::is_image(&path) {
			(icons::IMAGE, "Use as material map")
		} else {
			(icons::CUBE, "Open model")
		};
		let screen = ctx.content_rect();
		let painter = ctx.layer_painter(egui::LayerId::new(
			Order::Foreground,
			Id::unique("drop_overlay"),
		));
		painter.rect_filled(screen, 0.0, Color32::from_black_alpha(150));
		let inner = screen.shrink(24.0);
		let corners = [
			inner.left_top(),
			inner.right_top(),
			inner.right_bottom(),
			inner.left_bottom(),
			inner.left_top(),
		];
		for pair in corners.windows(2) {
			painter.extend(egui::Shape::dashed_line(
				pair,
				Stroke::new(2.0, p.accent),
				10.0,
				8.0,
			));
		}
		painter.text(
			screen.center() - Vec2::new(0.0, 24.0),
			Align2::CENTER_CENTER,
			icon,
			FontId::proportional(48.0),
			p.accent,
		);
		painter.text(
			screen.center() + Vec2::new(0.0, 24.0),
			Align2::CENTER_CENTER,
			title,
			FontId::new(18.0, semibold()),
			Color32::WHITE,
		);
	}
}
