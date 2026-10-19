//! Modal dialogs owned by the app: new workspace and keyboard shortcuts.
//! Import and Roblox access live in their own modules.

use eframe::egui::{self, Id, Key, RichText, Vec2};

use super::layout::SHORTCUTS;
use super::theme::{dialog_frame, display, palette};
use super::{icons, widgets};
use crate::app::App;
use crate::workspace;

impl App {
	pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
		if let Some(dialog) = &mut self.import
			&& let Some(outcome) = dialog.show(ctx)
		{
			self.import = None;
			self.finish_import(outcome);
		}
		if let Some(dialog) = &mut self.settings
			&& !dialog.show(ctx, &mut self.config)
		{
			self.settings = None;
		}
		self.new_workspace_dialog(ctx);
		self.shortcuts_dialog(ctx);
	}

	fn new_workspace_dialog(&mut self, ctx: &egui::Context) {
		let Some(dialog) = &mut self.new_workspace else {
			return;
		};
		let p = palette(ctx);
		let mut create = false;
		let mut cancel = false;
		let modal = egui::Modal::new(Id::unique("new_workspace"))
			.frame(dialog_frame(ctx))
			.backdrop_color(egui::Color32::from_black_alpha(140))
			.show(ctx, |ui| {
				ui.set_width(440.0);
				ui.label(RichText::new("New workspace").size(18.0).family(display()));
				if let Some(export) = &dialog.then_import {
					ui.label(
						RichText::new(format!(
							"Importing {} needs a workspace to download into.",
							export
								.file_name()
								.map(|s| s.to_string_lossy())
								.unwrap_or_default()
						))
						.color(p.text_weak),
					);
				} else {
					ui.label(
						RichText::new(
							"A folder that holds downloaded models, organized like your export.",
						)
						.color(p.text_weak),
					);
				}
				ui.add_space(10.0);
				ui.label("Name");
				let response = ui.add(
					egui::TextEdit::singleline(&mut dialog.name)
						.hint_text("e.g. My game assets")
						.desired_width(f32::INFINITY),
				);
				response.request_focus();
				if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
					create = true;
				}
				ui.add_space(6.0);
				ui.label("Location");
				ui.horizontal(|ui| {
					ui.add(
						egui::Label::new(
							RichText::new(dialog.parent.display().to_string()).color(p.text_weak),
						)
						.truncate(),
					);
					if ui.small_button("Change…").clicked()
						&& let Some(dir) = rfd::FileDialog::new()
							.set_directory(&dialog.parent)
							.pick_folder()
					{
						dialog.parent = dir;
					}
				});
				if !dialog.name.trim().is_empty() {
					ui.label(
						RichText::new(format!(
							"{}  {}",
							icons::FOLDER_SIMPLE,
							dialog
								.parent
								.join(workspace::sanitize(dialog.name.trim()))
								.display()
						))
						.size(11.5)
						.color(p.text_weak),
					);
				}
				ui.add_space(14.0);
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					create |= widgets::primary_button(ui, "Create").clicked();
					cancel = ui.button("Cancel").clicked();
				});
			});
		if cancel || modal.should_close() {
			self.new_workspace = None;
			return;
		}
		if create {
			let dialog = self.new_workspace.take().expect("dialog is open");
			match self.create_workspace(&dialog.name, &dialog.parent) {
				Ok(()) => {
					if let Some(export) = dialog.then_import {
						match std::fs::read(&export) {
							Ok(bytes) => self.start_import(export, &bytes),
							Err(e) => self
								.toasts
								.error(format!("Could not read {}: {e}", export.display())),
						}
					}
				}
				Err(e) => {
					self.toasts.error(e);
					self.new_workspace = Some(dialog);
				}
			}
		}
	}

	fn shortcuts_dialog(&mut self, ctx: &egui::Context) {
		if !self.show_shortcuts {
			return;
		}
		let p = palette(ctx);
		let modal = egui::Modal::new(Id::unique("shortcuts"))
			.frame(dialog_frame(ctx))
			.backdrop_color(egui::Color32::from_black_alpha(140))
			.show(ctx, |ui| {
				ui.set_width(380.0);
				ui.label(
					RichText::new("Keyboard shortcuts")
						.size(18.0)
						.family(display()),
				);
				ui.add_space(8.0);
				egui::Grid::new("shortcut_grid")
					.num_columns(2)
					.spacing(Vec2::new(16.0, 8.0))
					.show(ui, |ui| {
						for (keys, action) in SHORTCUTS {
							widgets::badge(ui, RichText::new(*keys).monospace(), p.text);
							ui.label(*action);
							ui.end_row();
						}
					});
				ui.add_space(10.0);
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					ui.button("Close").clicked()
				})
				.inner
			});
		if modal.inner || modal.should_close() {
			self.show_shortcuts = false;
		}
	}
}
