//! Files dropped onto the window, and the overlay while they hover over it.

use std::path::Path;

use eframe::egui::{self, Align2, Color32, FontId, Id, Order, Vec2};
use eframe::emath;

use super::theme::{palette, semibold};
use super::{icons, widgets};
use crate::app::App;
use crate::scene::texture::is_image;

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

	/// Dims the window and explains what dropping will do, fading in as a
	/// file comes over the window and out as it leaves or is dropped.
	pub(super) fn drop_overlay(&self, ctx: &egui::Context) {
		let id = Id::unique("drop_overlay_fade");
		let hovered = ctx.input(|i| {
			i.raw
				.hovered_files
				.first()
				.map(|f| drop_label(&f.path.clone().unwrap_or_default()))
		});
		// kept for the fade-out, when nothing is hovering any more
		if let Some(label) = hovered {
			ctx.data_mut(|d| d.insert_temp(id, label));
		}
		let t = ctx.animate_bool_with_time_and_easing(
			id,
			hovered.is_some(),
			DROP_FADE,
			emath::easing::cubic_out,
		);
		if t <= 0.0 {
			return;
		}
		let Some((icon, title)) = ctx.data(|d| d.get_temp::<(&'static str, &'static str)>(id))
		else {
			return;
		};
		let p = palette(ctx);
		let screen = ctx.content_rect();
		let painter = ctx.layer_painter(egui::LayerId::new(
			Order::Foreground,
			Id::unique("drop_overlay"),
		));
		painter.rect_filled(
			screen,
			0.0,
			Color32::from_black_alpha(150).gamma_multiply(t),
		);
		widgets::dashed_rect(
			&painter,
			screen.shrink(24.0),
			2.0,
			p.accent.gamma_multiply(t),
			(10.0, 8.0),
		);
		painter.text(
			screen.center() - Vec2::new(0.0, 24.0),
			Align2::CENTER_CENTER,
			icon,
			FontId::proportional(48.0),
			p.accent.gamma_multiply(t),
		);
		painter.text(
			screen.center() + Vec2::new(0.0, 24.0),
			Align2::CENTER_CENTER,
			title,
			FontId::new(18.0, semibold()),
			Color32::WHITE.gamma_multiply(t),
		);
	}
}

/// How long the drop overlay takes to fade in or out, in seconds.
const DROP_FADE: f32 = 0.15;

/// The overlay's icon and title for a file hovering over the window: what
/// dropping it will do.
fn drop_label(path: &Path) -> (&'static str, &'static str) {
	let ext = path
		.extension()
		.map(|e| e.to_string_lossy().to_lowercase())
		.unwrap_or_default();
	let is_rig = path
		.file_name()
		.is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(".rig.json"));
	if is_rig {
		(icons::PACKAGE, "Open model")
	} else if ext == "json" {
		(icons::FILE_ARROW_DOWN, "Import export file")
	} else if is_image(path) {
		(icons::IMAGE, "Use as material map")
	} else {
		(icons::CUBE, "Open model")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_overlay_says_what_dropping_does() {
		let label = |p: &str| drop_label(Path::new(p)).1;
		assert_eq!(label("Rigs/Wolf.rig.json"), "Open model");
		assert_eq!(label("export.JSON"), "Import export file");
		assert_eq!(label("albedo.png"), "Use as material map");
		assert_eq!(label("Body.mesh"), "Open model");
	}
}
