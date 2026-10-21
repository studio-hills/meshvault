//! The window layout: top bar, library, viewport, inspector and status bar.

use eframe::egui::{self, Frame, Margin};

use super::inspector::header_backdrop;
use super::theme::{Palette, palette};
use crate::app::App;
use crate::render::camera::Camera;
use crate::render::maths::IDENTITY;
use crate::render::viewport::{self, ScenePart, ViewSettings};
use crate::scene::geometry::GroupKind;
use crate::scene::surface::{PART_COLOUR, Surface};

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
		// without a workspace there is no library: the welcome screen takes its place
		if self.show_library && self.workspace.is_some() {
			egui::Panel::left("library")
				.default_size(290.0)
				.size_range(220.0..=520.0)
				.frame(
					Frame::NONE
						.fill(p.panel)
						.inner_margin(Margin::same(PANEL_MARGIN)),
				)
				.show(ui, |ui| self.library(ui));
		}
		if self.show_inspector && (self.doc.is_some() || self.model.is_some()) {
			egui::Panel::right("inspector")
				.default_size(300.0)
				.size_range(240.0..=520.0)
				.frame(
					Frame::NONE
						.fill(p.panel)
						.inner_margin(Margin::same(PANEL_MARGIN)),
				)
				.show(ui, |ui| {
					header_backdrop(ui);
					self.inspector(ui)
				});
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
	fn library(&mut self, ui: &mut egui::Ui) {
		if let Some(ws) = &self.workspace {
			let open_model = self.model.as_ref().map(|m| m.index);
			let actions = self.browser.show(
				ui,
				ws,
				self.open_stem.as_deref(),
				open_model,
				self.gpu.as_deref(),
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
			colour: PART_COLOUR,
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
		}
	}
}

/// Margin of the side panels (library and inspector). The inspector's
/// header backdrop reaches back over it.
pub(super) const PANEL_MARGIN: i8 = 12;
/// Margin of the top bar, (horizontal, vertical). The title-bar drag area
/// reaches back over it.
pub(super) const TOP_BAR_MARGIN: (i8, i8) = (12, 8);
