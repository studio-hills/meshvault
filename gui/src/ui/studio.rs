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
	mode: Mode,
	copied: Option<Instant>,
}

/// What the exporter does with the export.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
	/// Sends it straight to Meshvault: one Command Bar line.
	Send,
	/// Hands it back to change first, then send or save it: a short script.
	Edit,
}

impl Default for StudioDialog {
	fn default() -> Self {
		Self {
			schema: studio_link::LATEST_SCHEMA,
			mode: Mode::Send,
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
						"Run the exporter from Studio's Command Bar. Meshvault serves it, \
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
								ui.label(
									RichText::new(icons::WARNING)
										.size(16.0)
										.color(p.warning)
										.line_height(Some(super::theme::BODY_LINE_HEIGHT)),
								);
								ui.add(
									egui::Label::new(
										egui::RichText::new(unavailable_text(why))
											.line_height(Some(super::theme::BODY_LINE_HEIGHT)),
									)
									.wrap(),
								);
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
				ui.add_space(6.0);
				ui.horizontal(|ui| {
					ui.label("Export");
					let modes = [
						(Mode::Send, "Send to Meshvault"),
						(Mode::Edit, "Edit first"),
					];
					if widgets::text_segmented(ui, &mut dialog.mode, &modes) {
						dialog.copied = None;
					}
				});
				ui.add_space(12.0);

				step(
					ui,
					1,
					"In Studio, open View → Command Bar (in Edit mode, not Play).",
				);
				let command = match dialog.mode {
					Mode::Send => {
						step(ui, 2, "Paste this line and press Enter:");
						studio_link::studio_command(dialog.schema)
					}
					Mode::Edit => {
						step(
							ui,
							2,
							"Paste this script, change the export where it says, and press Enter:",
						);
						studio_link::studio_script(dialog.schema)
					}
				};
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
					let label = match (copied, dialog.mode) {
						(true, _) => "Copied",
						(false, Mode::Send) => "Copy command",
						(false, Mode::Edit) => "Copy script",
					};
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
				let arrives = match dialog.mode {
					Mode::Send => "The export arrives here.",
					Mode::Edit => {
						"Nothing is sent until send(export) runs; it then arrives here. To keep the \
						 export yourself instead, save h:JSONEncode(export) your own way."
					}
				};
				step(
					ui,
					3,
					&format!(
						"{arrives} Selecting a model, or anything inside one, exports that model; \
						 with nothing selected, it exports the whole game except Workspace."
					),
				);
				ui.add_space(4.0);
				let unreachable = match dialog.mode {
					Mode::Send => {
						"If Meshvault cannot be reached while sending, the export is printed in the \
						 Output window instead."
					}
					Mode::Edit => {
						"If send cannot reach Meshvault it says why in the Output window and returns \
						 false; the export is still yours to save."
					}
				};
				ui.label(
					RichText::new(format!(
						"HttpService.HttpEnabled is on only while the exporter talks to Meshvault. \
						 {unreachable}"
					))
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
