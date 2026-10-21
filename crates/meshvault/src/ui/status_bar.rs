//! The status bar: the renderer, the Studio link, the open model's path
//! and download progress.

use eframe::egui::{self, Align, Align2, Color32, Layout, RichText, Sense};

use super::theme::{CAPTION, palette};
use super::{icons, unavailable_text, widgets};
use crate::app::App;
use crate::studio::link::{self, Status};

impl App {
	pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		ui.horizontal(|ui| {
			ui.spacing_mut().item_spacing.x = 10.0;
			ui.spacing_mut().interact_size.y = 22.0;
			let small =
				|text: String, colour: Color32| RichText::new(text).size(CAPTION).color(colour);
			match &self.adapter {
				Some(name) if self.gpu.is_some() => {
					ui.label(small(format!("{}  GPU", icons::CIRCLE), p.success))
						.on_hover_text(format!("Rendering on {name}"));
				}
				_ => {
					ui.label(small(format!("{}  CPU renderer", icons::CIRCLE), p.warning))
						.on_hover_text(
							"No GPU renderer: normal, roughness and metalness maps are not shown",
						);
				}
			}
			// an export that stops arriving is forgotten after a while
			let receiving = self
				.studio_progress
				.filter(|(_, _, at)| at.elapsed().as_secs() < 60);
			let (text, colour, hover) = match (&self.studio_link.status, receiving) {
				(Status::Listening, Some((done, count, _))) => (
					format!("{}  Receiving from Studio {done}/{count}", icons::CIRCLE),
					p.accent,
					"An export is arriving from Roblox Studio".to_owned(),
				),
				(Status::Listening, None) => (
					format!("{}  Studio link", icons::CIRCLE),
					p.success,
					format!(
						"Listening on localhost:{} for exports from Roblox Studio. Click for the Command Bar line.",
						link::PORT
					),
				),
				(Status::Unavailable(why), _) => (
					format!("{}  Studio link unavailable", icons::CIRCLE),
					p.warning,
					unavailable_text(why),
				),
			};
			let studio = ui
				.add(egui::Label::new(small(text, colour)).sense(Sense::click()))
				.on_hover_text(hover);
			if widgets::pointer(studio).clicked() {
				self.show_studio_dialog();
			}
			let path = match (&self.doc, &self.model) {
				(Some(doc), _) => doc.path.display().to_string(),
				(None, Some(model)) => model.path.clone(),
				(None, None) => "No model open".into(),
			};
			ui.add(egui::Label::new(small(path, p.text_weak)).truncate());

			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				if self.downloader.is_running() {
					if widgets::icon_button(ui, icons::STOP, "Cancel downloads").clicked() {
						self.downloader.cancel();
					}
					let d = &self.downloader;
					let progress = d.completed as f32 / d.total.max(1) as f32;
					let bar = ui.add(
						egui::ProgressBar::new(progress)
							.desired_width(160.0)
							.desired_height(14.0)
							.fill(p.accent),
					);
					// the count's digits keep their width as it climbs
					widgets::paint_tabular(
						ui.painter(),
						bar.rect.center(),
						Align2::CENTER_CENTER,
						&format!("{}/{}", d.completed, d.total),
						&egui::FontId::proportional(CAPTION),
						p.text,
					);
					let mut text = format!("{}  Downloading", icons::CLOUD_ARROW_DOWN);
					if d.failed > 0 {
						text.push_str(&format!(" · {} failed", d.failed));
					}
					ui.label(small(text, p.text_weak));
				} else if let Some(ws) = &self.workspace {
					let items = ws.items();
					let done = items.iter().filter(|i| i.is_done()).count();
					let mut text = format!("{} meshes · {done} downloaded", items.len());
					let models = ws.models().len();
					if models > 0 {
						text = format!("{models} animated · {text}");
					}
					ui.label(small(text, p.text_weak));
				}
			});
		});
	}
}
