//! Switching between an animated model and the meshes it is built from,
//! shown at the top of the inspector.

use eframe::egui::{self, FontId, Sense, Vec2};

use super::theme::{CONTROL_SM, TITLE, medium, palette};
use super::{icons, widgets};
use crate::app::App;

/// The other side of what is open: (index, name) pairs of workspace
/// models and items.
#[derive(Default)]
pub(super) struct Counterparts {
	pub models: Vec<(usize, String)>,
	pub meshes: Vec<(usize, String)>,
}

pub(super) enum Switch {
	Model(usize),
	Mesh(usize),
}

impl App {
	/// The animated model open and its meshes, or the mesh open and the
	/// animated models that use it.
	pub(super) fn counterparts(&self) -> Counterparts {
		let Some(ws) = &self.workspace else {
			return Counterparts::default();
		};
		// an open model comes first, even with a mesh open as well
		if let Some(model) = &self.model {
			let Some(item) = ws.models().get(model.index) else {
				return Counterparts::default();
			};
			return Counterparts {
				models: vec![(model.index, item.name.clone())],
				meshes: ws.meshes_of_model(model.index),
			};
		}
		let Some(stem) = &self.open_stem else {
			return Counterparts::default();
		};
		Counterparts {
			models: ws.models_using(stem),
			meshes: ws.item_with_stem(stem).into_iter().collect(),
		}
	}

	pub(super) fn apply_switch(&mut self, switch: Switch) {
		match switch {
			Switch::Model(i) => self.open_model(i),
			Switch::Mesh(i) => self.open_item(i),
		}
	}
}

/// The top of the inspector, laid out the same for a model and a mesh so
/// nothing moves when switching between them: the name and a one-line
/// subtitle (both cut short, with the full text on hover), then the switch
/// when there is something to switch to.
pub(super) fn header(
	ui: &mut egui::Ui,
	on_model: bool,
	c: &Counterparts,
	title: &str,
	subtitle: &str,
	subtitle_hover: &str,
) -> Option<Switch> {
	ui.add(
		egui::Label::new(
			egui::RichText::new(title)
				.size(TITLE)
				.family(super::theme::display()),
		)
		.truncate(),
	)
	.on_hover_text(title);
	ui.add_space(2.0);
	ui.add(egui::Label::new(widgets::caption(ui, subtitle)).truncate())
		.on_hover_text(subtitle_hover);
	ui.add_space(12.0);
	let switch = view_switch(ui, on_model, c);
	if !c.models.is_empty() && !c.meshes.is_empty() {
		ui.add_space(8.0);
	}
	switch
}

/// A two-part control: "Animated" and "Meshes", with the open one
/// highlighted and how many of each there are beside its name. When the
/// other side has several choices (a model built from several meshes, or a
/// mesh several models use) it opens a list. Shown only when both sides
/// exist.
pub(super) fn view_switch(ui: &mut egui::Ui, on_model: bool, c: &Counterparts) -> Option<Switch> {
	if c.models.is_empty() || c.meshes.is_empty() {
		return None;
	}
	let p = palette(ui.ctx());
	let mut out = None;
	// as tall as a small control, like the other segmented controls; the
	// halves' corners are concentric with the track's
	widgets::segment_track(ui, CONTROL_SM, |ui, height| {
		let half = ((ui.available_width() - 2.0) / 2.0).floor();
		ui.horizontal(|ui| {
			let sides = [
				(true, icons::PERSON_SIMPLE_RUN, "Animated", &c.models),
				(false, icons::CUBE, "Meshes", &c.meshes),
			];
			for (is_model, icon, label, targets) in sides {
				let current = is_model == on_model;
				let (rect, response) =
					ui.allocate_exact_size(Vec2::new(half, height), Sense::click());
				let response = if current {
					response
				} else {
					widgets::pointer(response)
				};
				let hovered = !current && response.hovered();
				let (fill, colour) = widgets::option_colours(&p, current, hovered);
				ui.painter()
					.rect_filled(rect, widgets::control_radius(ui), fill);
				// icon and name, then the count and (when it opens a
				// list) a caret, quieter, centred together
				let many = !current && targets.len() > 1;
				let painter = ui.painter();
				// 12.5 is off the type scale on purpose; kept as it was
				let name = painter.layout_no_wrap(
					format!("{icon}  {label}"),
					FontId::new(12.5, medium()),
					colour,
				);
				let count = painter.layout_no_wrap(
					if many {
						format!("{}  {}", targets.len(), icons::CARET_DOWN)
					} else {
						targets.len().to_string()
					},
					FontId::proportional(crate::ui::theme::CAPTION),
					if current {
						p.accent.gamma_multiply(0.7)
					} else {
						p.text_weak.gamma_multiply(0.8)
					},
				);
				let gap = 6.0;
				let width = name.size().x + gap + count.size().x;
				let left = rect.center().x - width / 2.0;
				let name_pos = egui::pos2(left, rect.center().y - name.size().y / 2.0);
				let count_pos = egui::pos2(
					left + name.size().x + gap,
					rect.center().y - count.size().y / 2.0,
				);
				painter.galley(name_pos, name, colour);
				painter.galley(count_pos, count, colour);
				if current {
					continue;
				}
				let target = |i: usize| {
					if is_model {
						Switch::Model(i)
					} else {
						Switch::Mesh(i)
					}
				};
				if let [(i, name)] = targets.as_slice() {
					if response.on_hover_text(format!("Open {name}")).clicked() {
						out = Some(target(*i));
					}
				} else {
					egui::Popup::menu(&response).show(|ui| {
						widgets::menu_style(ui);
						ui.set_min_width(half);
						for (i, name) in targets.iter() {
							if ui.button(name).clicked() {
								out = Some(target(*i));
								ui.close();
							}
						}
					});
				}
			}
		});
	});
	out
}
