//! Switching between an animated model and the meshes it is built from,
//! shown at the top of the inspector.

use eframe::egui::{self, Align2, CornerRadius, FontId, Frame, Margin, Sense, Stroke, Vec2};

use super::theme::{CONTROL_RADIUS, CONTROL_SM, medium, palette};
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
		let item_named = |stem: &String| {
			ws.items()
				.iter()
				.position(|i| &i.stem == stem)
				.map(|i| (i, ws.items()[i].name.clone()))
		};
		if let Some(model) = &self.model {
			let Some(item) = ws.models().get(model.index) else {
				return Counterparts::default();
			};
			let mut stems: Vec<&String> = item.meshes.values().collect();
			stems.sort();
			stems.dedup();
			return Counterparts {
				models: vec![(model.index, item.name.clone())],
				meshes: stems.into_iter().filter_map(item_named).collect(),
			};
		}
		let Some(stem) = &self.open_stem else {
			return Counterparts::default();
		};
		Counterparts {
			models: ws
				.models()
				.iter()
				.enumerate()
				.filter(|(_, m)| m.meshes.values().any(|s| s == stem))
				.map(|(i, m)| (i, m.name.clone()))
				.collect(),
			meshes: item_named(stem).into_iter().collect(),
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
/// nothing moves when switching between them: the switch (when there is
/// something to switch to), then the name and a one-line subtitle, both
/// cut short with the full text on hover.
pub(super) fn header(
	ui: &mut egui::Ui,
	on_model: bool,
	c: &Counterparts,
	title: &str,
	subtitle: &str,
	subtitle_hover: &str,
) -> Option<Switch> {
	let p = palette(ui.ctx());
	let switch = view_switch(ui, on_model, c);
	if !c.models.is_empty() && !c.meshes.is_empty() {
		ui.add_space(10.0);
	}
	ui.add(
		egui::Label::new(
			egui::RichText::new(title)
				.size(16.0)
				.family(super::theme::display()),
		)
		.truncate(),
	)
	.on_hover_text(title);
	ui.add(
		egui::Label::new(egui::RichText::new(subtitle).size(12.0).color(p.text_weak)).truncate(),
	)
	.on_hover_text(subtitle_hover);
	ui.add_space(8.0);
	switch
}

/// A two-part control: "Animated" and "Mesh", with the open one
/// highlighted. When the other side has several choices (a model built
/// from several meshes, or a mesh several models use) it opens a list.
/// Shown only when both sides exist.
pub(super) fn view_switch(ui: &mut egui::Ui, on_model: bool, c: &Counterparts) -> Option<Switch> {
	if c.models.is_empty() || c.meshes.is_empty() {
		return None;
	}
	let p = palette(ui.ctx());
	let mut out = None;
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(CONTROL_RADIUS + 2))
		.inner_margin(Margin::same(2))
		.show(ui, |ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			// the row is exactly as tall as the halves, so the 2px padding is
			// even on every side, and the halves' corners are concentric with
			// the track's (inner = outer − padding and border)
			ui.spacing_mut().interact_size.y = CONTROL_SM - 4.0;
			widgets::nest(ui, CONTROL_RADIUS + 2, 3.0);
			let half = ((ui.available_width() - 2.0) / 2.0).floor();
			ui.horizontal(|ui| {
				let sides = [
					(
						true,
						icons::PERSON_SIMPLE_RUN,
						"Animated",
						"Animated",
						&c.models,
					),
					(false, icons::CUBE, "Mesh", "Meshes", &c.meshes),
				];
				for (is_model, icon, label, plural, targets) in sides {
					let current = is_model == on_model;
					let label = match (current, targets.len()) {
						(false, n) if n > 1 => {
							format!("{icon}  {plural} ({n})  {}", icons::CARET_DOWN)
						}
						_ => format!("{icon}  {label}"),
					};
					let (rect, response) =
						ui.allocate_exact_size(Vec2::new(half, CONTROL_SM - 4.0), Sense::click());
					let response = if current {
						response
					} else {
						widgets::pointer(response)
					};
					let hovered = !current && response.hovered();
					let fill = if current {
						p.accent_soft
					} else if hovered {
						p.raised_hover
					} else {
						egui::Color32::TRANSPARENT
					};
					ui.painter()
						.rect_filled(rect, widgets::control_radius(ui), fill);
					let colour = if current {
						p.accent
					} else if hovered {
						p.text
					} else {
						p.text_weak
					};
					ui.painter().text(
						rect.center(),
						Align2::CENTER_CENTER,
						label,
						FontId::new(12.5, medium()),
						colour,
					);
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
