//! The window layout: top bar, library, viewport, inspector and status bar.

use eframe::egui::{self, Frame, Margin};

use super::inspector::header_backdrop;
use super::theme::{Palette, palette};
use crate::app::App;
use crate::render::camera::Camera;
use crate::render::maths::IDENTITY;
use crate::render::viewport::{self, ScenePart, ViewSettings};
use crate::scene::geometry::GroupKind;
use crate::scene::surface::{Surface, mesh_colour};

impl eframe::App for App {
	fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
		let ctx = ui.ctx().clone();
		self.poll();
		self.handle_dropped_files(&ctx);
		self.shortcuts(&ctx);
		let p = palette(&ctx);
		super::window::resize_grips(&ctx);
		// the inspector's part list lights the part under the pointer. It is
		// cleared here, before the inspector draws, so it also goes when the
		// inspector is hidden or the list collapsed
		if let Some(model) = &mut self.model {
			model.highlight = None;
		}

		egui::Panel::top("top_bar")
			.frame(
				Frame::NONE
					.fill(p.panel)
					.inner_margin(Margin::symmetric(TOP_BAR_MARGIN.0, TOP_BAR_MARGIN.1)),
			)
			.show(ui, |ui| self.top_bar(ui));
		egui::Panel::bottom("status_bar")
			.frame(
				Frame::NONE
					.fill(p.panel)
					.inner_margin(Margin::symmetric(12, 4)),
			)
			.show(ui, |ui| self.status_bar(ui));
		// without a workspace, or with an empty one, there is no library: the
		// welcome screen takes its place, centred in the window
		let library_shown = self.show_library && self.has_library();
		// in a narrow window the inspector gives way to the 3D view; the
		// panels' widths are last frame's
		let window = ctx.content_rect().width();
		let library = if library_shown {
			self.panel_widths.0
		} else {
			0.0
		};
		self.inspector_squeezed = !inspector_fits(window, library, self.panel_widths.1);
		if library_shown {
			self.panel_widths.0 = egui::Panel::left("library")
				.default_size(PANEL_WIDTHS.0)
				.size_range(220.0..=520.0)
				.frame(
					Frame::NONE
						.fill(p.panel)
						.inner_margin(Margin::same(PANEL_MARGIN)),
				)
				.show(ui, |ui| self.library(ui))
				.response
				.rect
				.width();
		}
		if self.show_inspector
			&& !self.inspector_squeezed
			&& (self.doc.is_some() || self.model.is_some())
		{
			self.panel_widths.1 = egui::Panel::right("inspector")
				.default_size(PANEL_WIDTHS.1)
				.size_range(240.0..=520.0)
				.frame(
					Frame::NONE
						.fill(p.panel)
						.inner_margin(Margin::same(PANEL_MARGIN)),
				)
				.show(ui, |ui| {
					header_backdrop(ui);
					self.inspector(ui)
				})
				.response
				.rect
				.width();
		}
		egui::CentralPanel::default()
			.frame(Frame::NONE.fill(p.bg))
			.show(ui, |ui| self.center(ui));

		self.dialogs(&ctx);
		self.toasts.show(&ctx);
		self.drop_overlay(&ctx);
	}
}

impl App {
	/// Whether there is a library to show: a workspace with something in it.
	pub(crate) fn has_library(&self) -> bool {
		self.workspace.as_ref().is_some_and(|ws| !ws.is_empty())
	}

	fn library(&mut self, ui: &mut egui::Ui) {
		if let Some(ws) = &self.workspace {
			let open_model = self.model.as_ref().map(|m| m.index);
			let actions = self.browser.show(
				ui,
				ws,
				self.open_stem.as_deref(),
				open_model,
				self.gpu.as_ref(),
			);
			self.handle_browser(actions);
		}
	}

	fn center(&mut self, ui: &mut egui::Ui) {
		if self.model.is_some() {
			self.model_center(ui);
			return;
		}
		if self.doc.is_none() {
			self.welcome(ui);
			return;
		}
		let p = palette(ui.ctx());
		let settings = self.view_settings(&p);
		let Some(doc) = &mut self.doc else { return };
		let groups = doc.visible_groups();
		let geometry = &doc.loaded.geometry;
		let mut parts = [ScenePart {
			geometry,
			groups: &groups,
			transform: IDENTITY,
			material: if self.show_maps {
				&doc.material
			} else {
				&self.no_material
			},
			colour_groups: geometry.group_kind == GroupKind::Hull,
			colour: mesh_colour(geometry),
			scene: &mut doc.gpu,
			moved: false,
			surface: Surface {
				alpha_mode: doc.alpha,
				..Surface::OPAQUE
			},
			decal: None,
			highlight: false,
		}];
		let home = || Camera::framing(geometry);
		let rect = viewport::show(
			ui,
			&mut parts,
			&mut doc.camera,
			&settings,
			self.gpu.as_deref(),
			&home,
		);
		self.viewport = rect;
		self.viewport_toolbar(ui.ctx(), rect);
	}

	pub(crate) fn view_settings(&self, p: &Palette) -> ViewSettings {
		ViewSettings {
			shading: self.shading,
			cull_backfaces: self.cull_backfaces,
			show_axes: self.show_axes,
			show_grid: self.show_grid,
			background: [p.viewport_top, p.viewport_bottom],
			grid_colour: p.grid,
			lights: Vec::new(),
			lighting: self.config.lighting,
			highlight: p.accent,
			effects: Default::default(),
		}
	}
}

/// The least width the 3D view keeps: below it the inspector is hidden
/// until the window is wider again (or the library is hidden).
const MIN_VIEW_WIDTH: f32 = 420.0;

/// Whether the inspector fits beside the library (0 when hidden) and still
/// leaves the 3D view [`MIN_VIEW_WIDTH`].
pub(crate) fn inspector_fits(window: f32, library: f32, inspector: f32) -> bool {
	window - library - inspector >= MIN_VIEW_WIDTH
}

/// The side panels' widths when they open: the library's, the inspector's.
pub(crate) const PANEL_WIDTHS: (f32, f32) = (290.0, 300.0);

/// Margin of the side panels (library and inspector). The inspector's
/// header backdrop reaches back over it.
pub(super) const PANEL_MARGIN: i8 = 12;
/// Margin of the top bar, (horizontal, vertical). The title-bar drag area
/// reaches back over it.
pub(super) const TOP_BAR_MARGIN: (i8, i8) = (12, 8);

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_inspector_gives_way_in_a_narrow_window() {
		let (library, inspector) = PANEL_WIDTHS;
		// the window's first size, and the smallest it can be
		assert!(inspector_fits(1280.0, library, inspector));
		assert!(!inspector_fits(760.0, library, inspector));
		// a 1920 by 1080 screen at 200%
		assert!(!inspector_fits(960.0, library, inspector));
		// hiding the library makes room for it
		assert!(inspector_fits(760.0, 0.0, inspector));
		// exactly the 3D view's least width still fits
		assert!(inspector_fits(
			library + inspector + MIN_VIEW_WIDTH,
			library,
			inspector
		));
	}
}
