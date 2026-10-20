//! The "Export from Roblox Studio" dialog: the Command Bar line that fetches
//! the exporter from Meshvault and runs it, and the state of the Studio link.

use std::time::Instant;

use eframe::egui::{self, Align, CornerRadius, Frame, Id, Layout, Margin, RichText, Stroke};

use super::theme::{dialog_frame, display, palette};
use super::{icons, widgets};
use crate::app::App;
use crate::studio_link::{self, Status};

pub struct StudioDialog {
	schema: u32,
	copied: Option<Instant>,
}

impl Default for StudioDialog {
	fn default() -> Self {
		Self {
			schema: studio_link::LATEST_SCHEMA,
			copied: None,
		}
	}
}

/// Why the link is off, in words for the user.
pub fn unavailable_text(why: &str) -> String {
	format!("Exporting from Roblox Studio is unavailable: {why}.")
}

impl App {
	pub(super) fn studio_dialog(&mut self, ctx: &egui::Context) {
		let Some(dialog) = &mut self.studio_dialog else {
			return;
		};
		let p = palette(ctx);
		let modal = egui::Modal::new(Id::unique("studio_export"))
			.frame(dialog_frame(ctx))
			.backdrop_color(egui::Color32::from_black_alpha(140))
			.show(ctx, |ui| {
				ui.set_width(480.0);
				ui.label(
					RichText::new("Export from Roblox Studio")
						.size(18.0)
						.family(display()),
				);
				ui.add_space(4.0);
				ui.label(
					RichText::new(
						"Run one line in Studio's Command Bar. Meshvault serves the exporter, \
						 receives the export, saves it to Documents/Meshvault/Exports and opens it for import.",
					)
					.color(p.text_weak),
				);
				ui.add_space(12.0);

				if let Status::Unavailable(why) = &self.studio_link.status {
					Frame::NONE
						.fill(p.warning.gamma_multiply(0.12))
						.stroke(Stroke::new(1.0, p.warning.gamma_multiply(0.5)))
						.corner_radius(CornerRadius::same(8))
						.inner_margin(Margin::symmetric(12, 10))
						.show(ui, |ui| {
							ui.horizontal_top(|ui| {
								ui.label(RichText::new(icons::WARNING).size(16.0).color(p.warning));
								ui.add(egui::Label::new(unavailable_text(why)).wrap());
							});
						});
					ui.add_space(12.0);
				}

				ui.horizontal(|ui| {
					ui.label("Schema");
					let options: Vec<(u32, String)> = studio_link::SCHEMAS
						.iter()
						.rev()
						.map(|&v| {
							let label = if v == studio_link::LATEST_SCHEMA {
								format!("{v} (latest)")
							} else {
								v.to_string()
							};
							(v, label)
						})
						.collect();
					let options: Vec<(u32, &str)> =
						options.iter().map(|(v, l)| (*v, l.as_str())).collect();
					if widgets::text_segmented(ui, &mut dialog.schema, &options) {
						dialog.copied = None;
					}
				});
				ui.add_space(12.0);

				step(ui, 1, "In Studio, open View → Command Bar (in Edit mode, not Play).");
				step(ui, 2, "Paste this line and press Enter:");
				let command = studio_link::studio_command(dialog.schema);
				Frame::NONE
					.fill(p.raised)
					.stroke(Stroke::new(1.0, p.border))
					.corner_radius(CornerRadius::same(8))
					.inner_margin(Margin::same(10))
					.show(ui, |ui| {
						ui.set_width(ui.available_width());
						ui.add(
							egui::Label::new(RichText::new(&command).monospace().size(11.5))
								.wrap()
								.selectable(true),
						);
					});
				ui.add_space(6.0);
				ui.horizontal(|ui| {
					let copied = dialog.copied.is_some_and(|t| t.elapsed().as_secs() < 2);
					let label = if copied { "Copied" } else { "Copy command" };
					let icon = if copied { icons::CHECK } else { icons::COPY };
					let enabled = self.studio_link.is_listening();
					if ui
						.add_enabled_ui(enabled, |ui| widgets::button(ui, icon, label))
						.inner
						.clicked()
					{
						ui.ctx().copy_text(command.clone());
						dialog.copied = Some(Instant::now());
					}
					if copied {
						ui.ctx()
							.request_repaint_after(std::time::Duration::from_millis(500));
					}
				});
				ui.add_space(10.0);
				step(
					ui,
					3,
					"The export arrives here. Selecting a model, or anything inside one, exports \
					 that model; with nothing selected, it exports the whole game except Workspace.",
				);
				ui.add_space(4.0);
				ui.label(
					RichText::new(
						"The line turns HttpService.HttpEnabled on only while it talks to Meshvault. \
						 If Meshvault cannot be reached while sending, the export is printed in the Output window instead.",
					)
					.size(12.0)
					.color(p.text_weak),
				);
				ui.add_space(14.0);
				ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
					widgets::text_button(ui, "Close").clicked()
				})
				.inner
			});
		if modal.inner || modal.should_close() {
			self.studio_dialog = None;
		}
	}

	pub fn show_studio_dialog(&mut self) {
		self.studio_dialog = Some(StudioDialog::default());
	}
}

fn step(ui: &mut egui::Ui, n: u32, text: &str) {
	let p = palette(ui.ctx());
	ui.horizontal_top(|ui| {
		ui.label(RichText::new(format!("{n}.")).color(p.text_weak));
		ui.add(egui::Label::new(text).wrap());
	});
	ui.add_space(4.0);
}
