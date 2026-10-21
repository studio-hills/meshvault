//! Modal dialogs owned by the app: new workspace, keyboard shortcuts and
//! confirmation of destructive actions. Import and Roblox access live in
//! their own modules.

use eframe::egui::{self, Id, Key, RichText, Vec2};

use super::layout::SHORTCUTS;
use super::theme::{dialog_frame, display, palette};
use super::{icons, widgets};
use crate::app::App;
use crate::maps::Slot;
use crate::workspace;

/// A destructive action waiting for the user to confirm it.
#[derive(Clone, Debug)]
pub enum ConfirmAction {
	Quit,
	CloseWorkspace,
	RemoveMap(Slot),
	RemoveAllMaps,
	/// Workspace item indices; their downloaded files are deleted.
	RemoveItems(Vec<usize>),
	/// Animated model indices; their rig files are deleted.
	RemoveModels(Vec<usize>),
	/// Both at once, from a selection in the library.
	RemoveSelection {
		items: Vec<usize>,
		models: Vec<usize>,
	},
}

pub struct Confirm {
	title: String,
	body: String,
	button: String,
	action: ConfirmAction,
}

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
		self.parsing_dialog(ctx);
		self.loading_model_dialog(ctx);
		self.new_workspace_dialog(ctx);
		self.shortcuts_dialog(ctx);
		self.studio_dialog(ctx);
		self.confirm_dialog(ctx);
	}

	/// Shown while a large export is read in the background.
	fn parsing_dialog(&mut self, ctx: &egui::Context) {
		let Some(parsing) = &self.parsing else { return };
		// small files finish before a dialog would be noticed
		if parsing.started.elapsed().as_millis() < 150 {
			ctx.request_repaint_after(std::time::Duration::from_millis(150));
			return;
		}
		let file = parsing
			.path
			.file_name()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_default();
		let mut cancel = false;
		egui::Modal::new(Id::unique("parsing_export"))
			.frame(dialog_frame(ctx))
			.backdrop_color(egui::Color32::from_black_alpha(140))
			.show(ctx, |ui| {
				ui.set_width(340.0);
				ui.horizontal(|ui| {
					ui.add(egui::Spinner::new().size(18.0));
					ui.label(
						RichText::new(format!("Reading {file}"))
							.size(16.0)
							.family(display()),
					);
				});
				ui.add_space(6.0);
				ui.label(
					RichText::new("Large exports with animations can take a few seconds.")
						.color(palette(ui.ctx()).text_weak),
				);
				ui.add_space(10.0);
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					cancel = widgets::text_button(ui, "Cancel").clicked();
				});
			});
		if cancel {
			// the reader thread finishes on its own; its result is dropped
			self.parsing = None;
		}
	}

	/// Shown while an animated model's meshes and maps are read.
	fn loading_model_dialog(&mut self, ctx: &egui::Context) {
		let Some(loading) = &self.loading_model else {
			return;
		};
		if loading.started.elapsed().as_millis() < 150 {
			ctx.request_repaint_after(std::time::Duration::from_millis(150));
			return;
		}
		let name = loading.name.clone();
		let mut cancel = false;
		egui::Modal::new(Id::unique("loading_model"))
			.frame(dialog_frame(ctx))
			.backdrop_color(egui::Color32::from_black_alpha(140))
			.show(ctx, |ui| {
				ui.set_width(340.0);
				ui.horizontal(|ui| {
					ui.add(egui::Spinner::new().size(18.0));
					ui.label(
						RichText::new(format!("Opening {name}"))
							.size(16.0)
							.family(display()),
					);
				});
				ui.add_space(6.0);
				ui.label(
					RichText::new("Reading its meshes, maps and animations.")
						.color(palette(ui.ctx()).text_weak),
				);
				ui.add_space(10.0);
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					cancel = widgets::text_button(ui, "Cancel").clicked();
				});
			});
		if cancel {
			self.loading_model = None;
		}
	}

	/// Asks before running a destructive action, explaining what it does.
	pub fn ask(&mut self, action: ConfirmAction) {
		let downloading = self.downloader.is_running();
		let (title, body, button) = match &action {
			ConfirmAction::Quit => (
				"Quit Meshvault?".to_string(),
				if downloading {
					"Downloads in progress will stop. They pick up where they left off the next time you open the workspace."
				} else {
					"Your workspace is saved. The open model will close."
				}
				.to_string(),
				"Quit",
			),
			ConfirmAction::CloseWorkspace => {
				let name = self.workspace.as_ref().map(|ws| ws.name().to_string()).unwrap_or_default();
				let mut body = "A model opened from it closes too. Nothing is deleted.".to_string();
				if downloading {
					body.push_str(" Downloads in progress will stop and resume the next time you open it.");
				}
				(format!("Close workspace \u{201c}{name}\u{201d}?"), body, "Close workspace")
			}
			ConfirmAction::RemoveMap(slot) => (
				format!("Remove the {} map?", slot.label().to_lowercase()),
				"It is removed from this view only. The image file is not deleted.".to_string(),
				"Remove map",
			),
			ConfirmAction::RemoveAllMaps => (
				"Remove all maps?".to_string(),
				"They are removed from this view only. The image files are not deleted.".to_string(),
				"Remove all",
			),
			ConfirmAction::RemoveItems(items) => {
				let what = match (items.len(), items.first().and_then(|&i| self.workspace.as_ref()?.items().get(i))) {
					(1, Some(item)) => format!("\u{201c}{}\u{201d}", item.name),
					(n, _) => format!("{n} models"),
				};
				(
					format!("Remove {what} from the workspace?"),
					"Their downloaded files are deleted from the workspace folder. The download cache is kept, so importing them again is quick.".to_string(),
					"Remove",
				)
			}
			ConfirmAction::RemoveModels(models) => {
				let what = match (models.len(), models.first().and_then(|&i| self.workspace.as_ref()?.models().get(i))) {
					(1, Some(model)) => format!("\u{201c}{}\u{201d}", model.name),
					(n, _) => format!("{n} animated models"),
				};
				(
					format!("Remove {what} from the workspace?"),
					"The rig and its animations are deleted from the workspace folder. The meshes it uses stay in the library.".to_string(),
					"Remove",
				)
			}
			ConfirmAction::RemoveSelection { items, models } => (
				format!("Remove {} models from the workspace?", items.len() + models.len()),
				"Downloaded meshes and maps, and animated models' rigs, are deleted from the workspace folder. The download cache is kept, so importing them again is quick.".to_string(),
				"Remove",
			),
		};
		self.confirm = Some(Confirm {
			title,
			body,
			button: button.to_string(),
			action,
		});
	}

	fn confirm_dialog(&mut self, ctx: &egui::Context) {
		let Some(confirm) = &self.confirm else { return };
		let p = palette(ctx);
		let modal = egui::Modal::new(Id::unique("confirm"))
			.frame(dialog_frame(ctx))
			.backdrop_color(egui::Color32::from_black_alpha(140))
			.show(ctx, |ui| {
				ui.set_width(400.0);
				ui.horizontal(|ui| {
					ui.label(
						RichText::new(icons::WARNING_CIRCLE)
							.size(22.0)
							.color(p.danger),
					);
					ui.label(RichText::new(&confirm.title).size(18.0).family(display()));
				});
				ui.add_space(4.0);
				ui.add(
					egui::Label::new(
						RichText::new(&confirm.body)
							.color(p.text_weak)
							.line_height(Some(20.0)),
					)
					.wrap(),
				);
				ui.add_space(16.0);
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if widgets::danger_button(ui, &confirm.button).clicked() {
						return Some(true);
					}
					if widgets::text_button(ui, "Cancel").clicked() {
						return Some(false);
					}
					None
				})
				.inner
			});
		let decision = match modal.inner {
			Some(decision) => Some(decision),
			None if modal.should_close() => Some(false),
			None => None,
		};
		if let Some(confirmed) = decision {
			let action = self.confirm.take().expect("dialog is open").action;
			if confirmed {
				self.run_confirmed(action, ctx);
			}
		}
	}

	fn run_confirmed(&mut self, action: ConfirmAction, ctx: &egui::Context) {
		match action {
			ConfirmAction::Quit => {
				self.save_workspace();
				ctx.send_viewport_cmd(egui::ViewportCommand::Close);
			}
			ConfirmAction::CloseWorkspace => self.close_workspace(),
			ConfirmAction::RemoveMap(slot) => {
				if let Some(doc) = &mut self.doc {
					doc.material.remove(slot);
				}
			}
			ConfirmAction::RemoveAllMaps => self.remove_all_maps(),
			ConfirmAction::RemoveItems(items) => self.remove_items(&items),
			ConfirmAction::RemoveModels(models) => self.remove_models(&models),
			ConfirmAction::RemoveSelection { items, models } => {
				// models first: removing items does not move model indices
				self.remove_models(&models);
				self.remove_items(&items);
			}
		}
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
					widgets::input(&mut dialog.name)
						.hint_text("e.g. My game assets")
						.desired_width(f32::INFINITY),
				);
				response.request_focus();
				if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
					create = true;
				}
				ui.add_space(6.0);
				ui.label("Location");
				// a read-only field with a Browse button, the same height as the name input
				ui.allocate_ui_with_layout(
					egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
					egui::Layout::right_to_left(egui::Align::Center),
					|ui| {
						if widgets::text_button(ui, "Browse…").clicked()
							&& let Some(dir) = rfd::FileDialog::new()
								.set_directory(&dialog.parent)
								.pick_folder()
						{
							dialog.parent = dir;
						}
						let mut shown = dialog.parent.display().to_string();
						ui.add(
							widgets::input(&mut shown)
								.interactive(false)
								.desired_width(ui.available_width()),
						);
					},
				);
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
					cancel = widgets::text_button(ui, "Cancel").clicked();
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
					widgets::text_button(ui, "Close").clicked()
				})
				.inner
			});
		if modal.inner || modal.should_close() {
			self.show_shortcuts = false;
		}
	}
}
