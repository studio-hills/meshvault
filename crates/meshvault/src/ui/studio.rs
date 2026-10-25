//! The "Export from Roblox Studio" dialog: the code that fetches the
//! exporter from Meshvault and runs it in Studio's Command Bar, and the
//! state of the Studio link.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Vec2};

use super::theme::{BODY_LINE_HEIGHT, CARD_RADIUS, ROW, palette};
use super::{code, icons, widgets};
use crate::app::App;
use crate::studio::link::{self, CodeLayout, CodeMode, Status};

pub struct StudioDialog {
	schema: u32,
	mode: CodeMode,
	layout: CodeLayout,
	copied: Option<Instant>,
	/// The paths to leave out as typed, one a line.
	paths: String,
}

impl Default for StudioDialog {
	fn default() -> Self {
		Self {
			schema: link::LATEST_SCHEMA,
			mode: CodeMode::Send,
			layout: CodeLayout::OneLine,
			copied: None,
			paths: String::new(),
		}
	}
}

/// Why the link is off, in words for the user.
pub fn unavailable_text(why: &str) -> String {
	format!("Exporting from Roblox Studio is unavailable: {why}.")
}

/// Width of the dialog's content.
const DIALOG_WIDTH: f32 = 560.0;
/// How long the Copy button says "Copied" after a click.
const COPIED_FOR: Duration = Duration::from_secs(2);
/// How often the dialog redraws while it says "Copied", to turn it back.
const COPIED_REPAINT: Duration = Duration::from_millis(500);

impl App {
	pub(super) fn studio_dialog(&mut self, ctx: &egui::Context) {
		let Some(dialog) = &mut self.studio_dialog else {
			return;
		};
		let p = palette(ctx);
		let listening = self.studio_link.is_listening();
		let mut leave_out_changed = false;
		let modal = widgets::modal(ctx, "studio_export").show(ctx, |ui| {
			ui.set_width(DIALOG_WIDTH);
			widgets::dialog_title(ui, None, "Export from Roblox Studio");
			ui.add_space(2.0);
			ui.label(
				RichText::new("Paste this into Studio's Command Bar, in Edit mode.")
					.color(p.text_weak),
			);
			ui.add_space(16.0);

			if let Status::Unavailable(why) = &self.studio_link.status {
				Frame::NONE
					.fill(p.warning.gamma_multiply(0.12))
					.stroke(Stroke::new(1.0, p.warning.gamma_multiply(0.5)))
					.corner_radius(CornerRadius::same(CARD_RADIUS))
					.inner_margin(Margin::symmetric(12, 10))
					.show(ui, |ui| {
						widgets::note(
							ui,
							RichText::new(icons::WARNING).size(16.0).color(p.warning),
							RichText::new(unavailable_text(why)),
						);
					});
				ui.add_space(14.0);
			}

			// what the code does, and in a sentence what that means
			let modes = [
				(CodeMode::Send, "Send to Meshvault"),
				(CodeMode::Edit, "Edit first"),
			];
			if widgets::text_segmented(ui, &mut dialog.mode, &modes) {
				dialog.copied = None;
			}
			ui.add_space(8.0);
			let reads_leave_out = link::reads_leave_out(dialog.schema);
			let about = match dialog.mode {
				CodeMode::Send => {
					let services = if reads_leave_out {
						self.config.leave_out.services.clone()
					} else {
						link::LeaveOut::default().services
					};
					format!(
						"Exports the selected model (with nothing selected, the whole game{}) \
						 and opens it here for import.",
						except(&services)
					)
				}
				CodeMode::Edit => "Gives you the export to change first. send(export) then sends \
					it here, or save it your own way."
					.to_string(),
			};
			ui.add(
				egui::Label::new(
					RichText::new(about)
						.color(p.text_weak)
						.line_height(Some(BODY_LINE_HEIGHT)),
				)
				.wrap(),
			);
			ui.add_space(10.0);

			widgets::section(ui, "studio_leave_out", "Leave out", false, |ui| {
				ui.add_enabled_ui(reads_leave_out, |ui| {
					leave_out_changed =
						leave_out(ui, &mut self.config.leave_out, &mut dialog.paths);
				});
				if !reads_leave_out {
					ui.add_space(4.0);
					ui.label(
						RichText::new(
							"Schema 1's exporter always leaves out Workspace, and nothing else.",
						)
						.color(p.text_weak),
					);
				}
			});
			if leave_out_changed {
				dialog.copied = None;
			}
			ui.add_space(6.0);

			let source = link::studio_code(
				dialog.schema,
				dialog.mode,
				dialog.layout,
				&self.config.leave_out,
			);
			if code_card(ui, &mut dialog.layout, &source, dialog.mode) {
				dialog.copied = None;
			}
			ui.add_space(16.0);

			// the schema on the left (rarely changed), the actions on the right
			ui.horizontal(|ui| {
				ui.label(RichText::new("Schema").color(p.text_weak));
				let options: Vec<(u32, String)> = link::SCHEMAS
					.iter()
					.rev()
					.map(|&v| {
						let label = if v == link::LATEST_SCHEMA {
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
				ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
					let copied = dialog.copied.is_some_and(|t| t.elapsed() < COPIED_FOR);
					let label = if copied {
						format!("{}  Copied", icons::CHECK)
					} else {
						format!("{}  Copy", icons::COPY)
					};
					let copy = ui
						.add_enabled_ui(listening, |ui| widgets::primary_button(ui, label))
						.inner;
					if copy.clicked() {
						ui.ctx().copy_text(source.clone());
						dialog.copied = Some(Instant::now());
					}
					if copied {
						ui.ctx().request_repaint_after(COPIED_REPAINT);
					}
					widgets::text_button(ui, "Close").clicked()
				})
				.inner
			})
			.inner
		});
		if leave_out_changed && let Err(e) = self.config.save() {
			self.toasts.error(e);
		}
		if modal.inner || modal.should_close() {
			self.studio_dialog = None;
		}
	}

	pub fn show_studio_dialog(&mut self) {
		self.studio_dialog = Some(StudioDialog {
			paths: self.config.leave_out.paths.join("\n"),
			..Default::default()
		});
	}
}

/// " except A, B and C" for the services a whole-game export skips, or
/// nothing when it skips none.
fn except(services: &[String]) -> String {
	match services {
		[] => String::new(),
		[one] => format!(" except {one}"),
		[rest @ .., last] => format!(" except {} and {last}", rest.join(", ")),
	}
}

/// The paths typed, one a line: each trimmed, without blank lines.
fn paths_from(text: &str) -> Vec<String> {
	text.lines()
		.map(str::trim)
		.filter(|line| !line.is_empty())
		.map(String::from)
		.collect()
}

/// The choice of what exports leave out: the services a whole-game export
/// skips, and paths left out of every export, typed into `paths`. Returns
/// whether `leave_out` changed.
fn leave_out(ui: &mut egui::Ui, leave_out: &mut link::LeaveOut, paths: &mut String) -> bool {
	let p = palette(ui.ctx());
	let mut changed = false;
	ui.label(RichText::new("From the whole game, when nothing is selected:").color(p.text_weak));
	ui.add_space(2.0);
	ui.horizontal_wrapped(|ui| {
		for service in link::SERVICES {
			let mut on = leave_out.services.iter().any(|s| s == service);
			if widgets::pointer(ui.checkbox(&mut on, service)).changed() {
				// in the Explorer's order, then any not offered here
				let kept = |name: &str| {
					if name == service {
						on
					} else {
						leave_out.services.iter().any(|s| s == name)
					}
				};
				let others = leave_out
					.services
					.iter()
					.filter(|s| !link::SERVICES.contains(&s.as_str()))
					.cloned();
				leave_out.services = link::SERVICES
					.iter()
					.filter(|s| kept(s))
					.map(|s| s.to_string())
					.chain(others)
					.collect();
				changed = true;
			}
		}
	});
	ui.add_space(8.0);
	ui.label(
		RichText::new("From every export, these and everything in them, one full name a line:")
			.color(p.text_weak),
	);
	ui.add_space(2.0);
	let edit = ui.add(
		egui::TextEdit::multiline(paths)
			.hint_text("Workspace.Map.Trees")
			.font(egui::FontId::monospace(12.0))
			.desired_rows(3)
			.desired_width(f32::INFINITY),
	);
	if edit.changed() {
		let typed = paths_from(paths);
		if typed != leave_out.paths {
			leave_out.paths = typed;
			changed = true;
		}
	}
	changed
}

/// The code, in a card with a bar along its top: the language, a choice of
/// one line or formatted, and a note on what it does with HttpEnabled.
/// Returns whether the layout changed.
fn code_card(ui: &mut egui::Ui, layout: &mut CodeLayout, source: &str, mode: CodeMode) -> bool {
	let p = palette(ui.ctx());
	let mut changed = false;
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(CARD_RADIUS))
		.show(ui, |ui| {
			ui.set_width(ui.available_width());
			// the bar
			Frame::NONE
				.inner_margin(Margin {
					left: 12,
					right: 6,
					top: 6,
					bottom: 6,
				})
				.show(ui, |ui| {
					ui.horizontal(|ui| {
						ui.spacing_mut().interact_size.y = ROW;
						ui.label(widgets::overline("Luau").color(p.text_weak));
						ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
							let unreachable = match mode {
								CodeMode::Send => {
									"If Meshvault cannot be reached, the export is printed in the \
									 Output window instead."
								}
								CodeMode::Edit => {
									"If send cannot reach Meshvault it says why in the Output \
									 window and returns false; the export is still yours."
								}
							};
							widgets::icon_button(
								ui,
								icons::INFO,
								&format!(
									"HttpService.HttpEnabled is on only while the exporter talks \
									 to Meshvault, and put back after. {unreachable}"
								),
							);
							let layouts = [
								(CodeLayout::OneLine, "One line"),
								(CodeLayout::Formatted, "Formatted"),
							];
							changed = widgets::text_segmented(ui, layout, &layouts);
						});
					});
				});
			let line = ui.cursor().min.y;
			ui.painter()
				.hline(ui.max_rect().x_range(), line, Stroke::new(1.0, p.border));
			// the code
			Frame::NONE.inner_margin(Margin::same(12)).show(ui, |ui| {
				ui.set_width(ui.available_width());
				let font = egui::FontId::monospace(12.0);
				match layout {
					// one line wraps to fit
					CodeLayout::OneLine => {
						let job = code::luau(source, font, &p, ui.available_width());
						ui.add(egui::Label::new(job).selectable(true));
					}
					// formatted lines keep their shape, scrolling sideways if wide
					CodeLayout::Formatted => {
						egui::ScrollArea::horizontal()
							.id_salt("studio_code")
							.show(ui, |ui| {
								let job = code::luau(source, font, &p, f32::INFINITY);
								ui.add(egui::Label::new(job).selectable(true).extend());
							});
					}
				}
				ui.allocate_space(Vec2::new(ui.available_width(), 0.0));
			});
		});
	changed
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn except_lists_the_services_skipped() {
		let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
		assert_eq!(except(&names(&[])), "");
		assert_eq!(except(&names(&["Workspace"])), " except Workspace");
		assert_eq!(
			except(&names(&["Workspace", "Lighting", "ServerStorage"])),
			" except Workspace, Lighting and ServerStorage"
		);
	}

	#[test]
	fn paths_are_one_a_line_trimmed() {
		assert_eq!(
			paths_from("  Workspace.Map \n\n\tServerStorage.Old\r\n"),
			["Workspace.Map", "ServerStorage.Old"]
		);
		assert!(paths_from(" \n").is_empty());
	}
}
