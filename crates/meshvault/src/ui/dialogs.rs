//! Modal dialogs owned by the app: new workspace, keyboard shortcuts,
//! confirmation of destructive actions, and the progress dialogs shown
//! while an export is parsed and while an animated model opens. The
//! `dialogs` dispatcher also shows the import, Roblox access and Studio
//! dialogs, which live in their own modules.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, RichText, Vec2};

use super::shortcuts::SHORTCUTS;
use super::theme::{BODY_LINE_HEIGHT, TITLE, display, palette};
use super::{icons, widgets};
use crate::app::App;
use crate::scene::maps::Slot;
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

pub struct NewWorkspaceDialog {
	pub name: String,
	pub parent: PathBuf,
	/// Export to import once the workspace exists.
	pub then_import: Option<PathBuf>,
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
		let file = parsing
			.path
			.file_name()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_default();
		if progress_dialog(
			ctx,
			"parsing_export",
			parsing.task.started,
			&format!("Reading {file}"),
			"Large exports with animations can take a few seconds.",
		) {
			// the reader thread finishes on its own; its result is dropped
			self.parsing = None;
		}
	}

	/// Shown while an animated model's meshes and maps are read.
	fn loading_model_dialog(&mut self, ctx: &egui::Context) {
		let Some(loading) = &self.loading_model else {
			return;
		};
		if progress_dialog(
			ctx,
			"loading_model",
			loading.task.started,
			&format!("Opening {}", loading.name),
			"Reading its meshes, maps and animations.",
		) {
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
		let modal = widgets::modal(ctx, "confirm").show(ctx, |ui| {
			ui.set_width(400.0);
			widgets::dialog_title(
				ui,
				Some((icons::WARNING_CIRCLE, 22.0, p.danger)),
				&confirm.title,
			);
			ui.add_space(4.0);
			ui.add(
				egui::Label::new(
					RichText::new(&confirm.body)
						.color(p.text_weak)
						.line_height(Some(BODY_LINE_HEIGHT)),
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
		let modal = widgets::modal(ctx, "new_workspace").show(ctx, |ui| {
			ui.set_width(440.0);
			widgets::dialog_title(ui, None, "New workspace");
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
				ui.label(widgets::caption(
					ui,
					format!(
						"{}  {}",
						icons::FOLDER_SIMPLE,
						dialog
							.parent
							.join(workspace::sanitize(dialog.name.trim()))
							.display()
					),
				));
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
		if self.show_shortcuts && show_shortcuts(ctx) {
			self.show_shortcuts = false;
		}
	}
}

/// Draws the keyboard shortcuts dialog. Returns whether it was closed.
fn show_shortcuts(ctx: &egui::Context) -> bool {
	let p = palette(ctx);
	let window = ctx.content_rect().size();
	let columns = shortcut_columns(window.x);
	let modal = widgets::modal(ctx, "shortcuts").show(ctx, |ui| {
		ui.set_width(columns as f32 * SHORTCUT_COLUMN_WIDTH);
		widgets::dialog_title(ui, None, "Keyboard shortcuts");
		ui.add_space(8.0);
		let section = |ui: &mut egui::Ui, title: &str, rows: &[(&str, &str)]| {
			ui.vertical(|ui| {
				ui.set_width(SHORTCUT_COLUMN_WIDTH - 16.0);
				ui.label(widgets::overline(title).weak());
				ui.add_space(4.0);
				egui::Grid::new(("shortcut_grid", title))
					.num_columns(2)
					.min_row_height(SHORTCUT_ROW)
					.spacing(Vec2::new(14.0, 6.0))
					.show(ui, |ui| {
						for (keys, action) in rows {
							widgets::badge(ui, RichText::new(*keys).monospace(), p.text);
							ui.label(*action);
							ui.end_row();
						}
					});
			});
		};
		// never taller than the window: what does not fit scrolls
		egui::ScrollArea::vertical()
			.max_height((window.y - SHORTCUT_DIALOG_CHROME).max(120.0))
			.show(ui, |ui| {
				if columns == 2 {
					ui.horizontal_top(|ui| {
						for (title, rows) in SHORTCUTS {
							section(ui, title, rows);
						}
					});
				} else {
					for (i, (title, rows)) in SHORTCUTS.iter().enumerate() {
						if i > 0 {
							ui.add_space(12.0);
						}
						section(ui, title, rows);
					}
				}
			});
		ui.add_space(10.0);
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
			widgets::text_button(ui, "Close").clicked()
		})
		.inner
	});
	modal.inner || modal.should_close()
}

/// The width of one column of the shortcuts dialog.
const SHORTCUT_COLUMN_WIDTH: f32 = 360.0;
/// The height of a row of the shortcuts dialog, keycap included.
const SHORTCUT_ROW: f32 = 24.0;
/// What the shortcuts dialog needs besides its list: title, button, margins
/// and room around the dialog.
const SHORTCUT_DIALOG_CHROME: f32 = 190.0;

/// How many columns of shortcuts fit a window `width` wide: the sections
/// side by side when there is room, one under the other when not.
fn shortcut_columns(width: f32) -> usize {
	if width >= 2.0 * SHORTCUT_COLUMN_WIDTH + 120.0 {
		2
	} else {
		1
	}
}

/// How long a background job runs before its progress dialog shows: small
/// files finish before a dialog would be noticed.
const SHOW_AFTER: Duration = Duration::from_millis(150);

/// A progress dialog for a job started at `started`: a spinner and title,
/// a line of `body` and a Cancel button. Shows nothing until the job has run
/// for [`SHOW_AFTER`]. Escape and the backdrop do not close it. Returns
/// whether Cancel was clicked.
fn progress_dialog(
	ctx: &egui::Context,
	id: &str,
	started: Instant,
	title: &str,
	body: &str,
) -> bool {
	if started.elapsed() < SHOW_AFTER {
		ctx.request_repaint_after(SHOW_AFTER);
		return false;
	}
	let mut cancel = false;
	widgets::modal(ctx, id).show(ctx, |ui| {
		ui.set_width(340.0);
		ui.horizontal(|ui| {
			ui.add(egui::Spinner::new().size(18.0));
			ui.label(RichText::new(title).size(TITLE).family(display()));
		});
		ui.add_space(6.0);
		ui.label(RichText::new(body).color(palette(ui.ctx()).text_weak));
		ui.add_space(10.0);
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
			cancel = widgets::text_button(ui, "Cancel").clicked();
		});
	});
	cancel
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_shortcut_sections_sit_side_by_side_only_when_they_fit() {
		assert_eq!(shortcut_columns(1280.0), 2);
		assert_eq!(shortcut_columns(960.0), 2);
		assert_eq!(shortcut_columns(760.0), 1);
	}

	/// The dialog as laid out in a window of `size`, fonts and theme as in
	/// the app.
	fn shortcuts_rect(size: Vec2) -> egui::Rect {
		let ctx = egui::Context::default();
		super::super::theme::install_fonts(&ctx);
		super::super::theme::apply(&ctx, super::super::theme::ThemeMode::Dark);
		let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
		// a few frames, for the layout to settle
		for _ in 0..3 {
			let input = egui::RawInput {
				screen_rect: Some(screen),
				..Default::default()
			};
			let mut output = ctx.run_ui(input, |ui| {
				show_shortcuts(ui.ctx());
			});
			output.textures_delta.clear();
		}
		ctx.memory(|m| m.area_rect(egui::Id::unique("shortcuts")))
			.expect("the dialog was drawn")
	}

	/// It was taller than the window and ran off its bottom.
	#[test]
	fn the_shortcuts_dialog_fits_the_window() {
		for size in [
			Vec2::new(760.0, 480.0),
			Vec2::new(1280.0, 800.0),
			Vec2::new(1920.0, 1080.0),
		] {
			let rect = shortcuts_rect(size);
			let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
			assert!(
				screen.contains_rect(rect),
				"{rect:?} is outside the {size:?} window"
			);
		}
	}

	/// The dialog and the user docs list the same keys, in the same sections.
	#[test]
	fn the_shortcuts_dialog_matches_the_docs() {
		let docs = include_str!("../../../../docs/shortcuts.md");
		for (title, rows) in SHORTCUTS {
			let heading = format!("## {title}\n");
			let start = docs
				.find(&heading)
				.unwrap_or_else(|| panic!("no section {title:?}"));
			let section = &docs[start + heading.len()..];
			let section = &section[..section.find("\n## ").unwrap_or(section.len())];
			for (keys, _) in *rows {
				assert!(
					section.contains(&format!("| {keys} |")),
					"{keys} is not under {title:?} in docs/shortcuts.md"
				);
			}
		}
	}
}
