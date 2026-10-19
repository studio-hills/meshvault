//! Small styled building blocks shared by the panels.

use eframe::egui::{
	self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Response, RichText, Sense, Stroke,
	Ui, Vec2, WidgetText,
};

use super::theme::{display, palette, semibold};

/// A borderless square button showing one icon, with a tooltip.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
	icon_toggle(ui, icon, tooltip, false)
}

/// An icon button that shows an "on" state with the accent colour.
pub fn icon_toggle(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> Response {
	let p = palette(ui.ctx());
	let size = Vec2::splat(28.0);
	let (rect, response) = ui.allocate_exact_size(size, Sense::click());
	let enabled = ui.is_enabled();
	let hovered = enabled && response.hovered();
	let fill = if on {
		p.accent_soft
	} else if hovered {
		p.raised_hover
	} else {
		Color32::TRANSPARENT
	};
	let colour = if !enabled {
		p.text_weak.gamma_multiply(0.45)
	} else if on {
		p.accent
	} else if hovered {
		p.text
	} else {
		p.text_weak
	};
	ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
	ui.painter().text(
		rect.center(),
		Align2::CENTER_CENTER,
		icon,
		FontId::proportional(16.0),
		colour,
	);
	response.on_hover_text(tooltip)
}

/// A filled accent button for the main action of a screen or dialog.
pub fn primary_button(ui: &mut Ui, text: impl Into<String>) -> Response {
	let p = palette(ui.ctx());
	ui.add(
		egui::Button::new(RichText::new(text).color(p.on_accent).family(semibold()))
			.fill(p.accent)
			.stroke(Stroke::NONE)
			.corner_radius(CornerRadius::same(7))
			.min_size(Vec2::new(0.0, 30.0)),
	)
}

/// A regular button with an icon in front of the label.
pub fn button(ui: &mut Ui, icon: &str, text: &str) -> Response {
	ui.add(egui::Button::new(format!("{icon}  {text}")).min_size(Vec2::new(0.0, 30.0)))
}

/// A row of mutually exclusive icon options, like a radio group.
pub fn segmented<T: PartialEq + Copy>(
	ui: &mut Ui,
	value: &mut T,
	options: &[(T, &str, &str)],
) -> bool {
	let p = palette(ui.ctx());
	let mut changed = false;
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(8))
		.inner_margin(Margin::same(2))
		.show(ui, |ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			ui.horizontal(|ui| {
				for &(option, icon, tooltip) in options {
					if icon_toggle(ui, icon, tooltip, *value == option).clicked()
						&& *value != option
					{
						*value = option;
						changed = true;
					}
				}
			});
		});
	changed
}

/// A collapsible panel section with a small uppercase title.
pub fn section<R>(
	ui: &mut Ui,
	id: &str,
	title: &str,
	default_open: bool,
	body: impl FnOnce(&mut Ui) -> R,
) {
	let p = palette(ui.ctx());
	let id = ui.make_persistent_id(id);
	let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
		ui.ctx(),
		id,
		default_open,
	);
	let header = ui.horizontal(|ui| {
		let icon = if state.is_open() {
			super::icons::CARET_DOWN
		} else {
			super::icons::CARET_RIGHT
		};
		let text = RichText::new(format!("{icon}  {}", title.to_uppercase()))
			.size(11.0)
			.family(semibold())
			.color(p.text_weak);
		ui.add(
			egui::Label::new(text)
				.sense(Sense::click())
				.selectable(false),
		)
	});
	if header.inner.clicked() {
		state.toggle(ui);
	}
	state.show_body_unindented(ui, |ui| {
		ui.add_space(2.0);
		body(ui);
		ui.add_space(6.0);
	});
	state.store(ui.ctx());
}

/// A raised, rounded surface for grouping content.
pub fn card<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
	let p = palette(ui.ctx());
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(10))
		.inner_margin(Margin::same(12))
		.show(ui, body)
		.inner
}

/// A small rounded label, e.g. a format or status badge.
pub fn badge(ui: &mut Ui, text: impl Into<WidgetText>, colour: Color32) -> Response {
	Frame::NONE
		.fill(colour.gamma_multiply(0.18))
		.corner_radius(CornerRadius::same(255))
		.inner_margin(Margin::symmetric(8, 2))
		.show(ui, |ui| {
			let text: WidgetText = text.into();
			ui.label(text.color(colour))
		})
		.inner
}

/// Two-column label/value rows with weak labels.
pub fn properties(ui: &mut Ui, id: &str, rows: &[(String, String)]) {
	let p = palette(ui.ctx());
	egui::Grid::new(id)
		.num_columns(2)
		.spacing(Vec2::new(12.0, 6.0))
		.show(ui, |ui| {
			for (key, value) in rows {
				ui.label(RichText::new(key).color(p.text_weak));
				ui.add(egui::Label::new(value).wrap());
				ui.end_row();
			}
		});
}

/// A big statistic with a caption, used in overview cards.
pub fn stat(ui: &mut Ui, value: &str, caption: &str) {
	let p = palette(ui.ctx());
	ui.vertical(|ui| {
		ui.spacing_mut().item_spacing.y = 0.0;
		ui.label(RichText::new(value).size(18.0).family(display()));
		ui.label(RichText::new(caption).size(11.0).color(p.text_weak));
	});
}

/// A dashed rounded rectangle, used as a drop target.
pub fn dashed_rect(ui: &Ui, rect: egui::Rect, colour: Color32) {
	let r = rect.shrink(1.0);
	let corners = [
		r.left_top(),
		r.right_top(),
		r.right_bottom(),
		r.left_bottom(),
		r.left_top(),
	];
	for pair in corners.windows(2) {
		ui.painter().extend(egui::Shape::dashed_line(
			pair,
			Stroke::new(1.5, colour),
			8.0,
			6.0,
		));
	}
}
