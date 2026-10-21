//! Small styled building blocks shared by the panels.

use eframe::egui::{
	self, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Margin, Response, RichText,
	Sense, Stroke, Ui, Vec2, WidgetText,
};

use super::theme::{CONTROL_RADIUS, CONTROL_SM, display, medium, palette, semibold};

/// Shows a pointing hand while hovering something clickable. egui does this
/// for buttons by itself (see `interact_cursor` in the theme); checkboxes,
/// radios, collapsing arrows, clickable labels and custom widgets need this.
pub fn pointer(response: Response) -> Response {
	if response.hovered() && response.enabled() {
		response.ctx.set_cursor_icon(CursorIcon::PointingHand);
	}
	response
}

/// Shrinks controls in this `ui` to the small size (toolbars, cards).
pub fn compact(ui: &mut Ui) {
	ui.spacing_mut().interact_size.y = CONTROL_SM;
	ui.spacing_mut().button_padding = Vec2::new(12.0, 6.0);
}

/// Rounds the controls inside a container whose corners have radius
/// `outer`, `padding` in from its edge (its margin plus its border), so
/// their corners are concentric with the container's: inner radius = outer
/// radius − padding. Where the gap is as large as the radius, they keep
/// their own radius.
pub fn nest(ui: &mut Ui, outer: u8, padding: f32) {
	let inner = outer as f32 - padding;
	if inner <= 0.0 {
		return;
	}
	let radius = CornerRadius::same(inner.round() as u8);
	let w = &mut ui.visuals_mut().widgets;
	for state in [
		&mut w.noninteractive,
		&mut w.inactive,
		&mut w.hovered,
		&mut w.active,
		&mut w.open,
	] {
		state.corner_radius = radius;
	}
}

/// The corner radius of controls in `ui`: `CONTROL_RADIUS`, or what a
/// container nesting them gives them (see [`nest`]).
pub fn control_radius(ui: &Ui) -> CornerRadius {
	ui.visuals().widgets.inactive.corner_radius
}

/// Sizes dropdown and context menu items: full-width rows of `CONTROL_SM`.
/// Their corners are concentric with the menu's.
pub fn menu_style(ui: &mut Ui) {
	ui.spacing_mut().interact_size.y = CONTROL_SM;
	ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
	let outer = ui.visuals().menu_corner_radius.nw;
	let padding = ui.spacing().menu_margin.top as f32 + ui.visuals().window_stroke.width;
	nest(ui, outer, padding);
}

/// A borderless square button showing one icon, with a tooltip. It is as
/// tall as the surrounding controls (`interact_size.y`).
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
	icon_toggle(ui, icon, tooltip, false)
}

/// An icon button that shows an "on" state with the accent colour.
pub fn icon_toggle(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> Response {
	icon_widget(ui, icon, tooltip, on, false, false)
}

/// [`icon_toggle`] with the icon turned half a turn, e.g. to point a
/// left-sidebar icon at a panel on the right.
pub fn icon_toggle_flipped(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> Response {
	icon_widget(ui, icon, tooltip, on, false, true)
}

/// An icon button for a destructive action: it turns red on hover.
pub fn danger_icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
	icon_widget(ui, icon, tooltip, false, true, false)
}

fn icon_widget(
	ui: &mut Ui,
	icon: &str,
	tooltip: &str,
	on: bool,
	danger: bool,
	flipped: bool,
) -> Response {
	let p = palette(ui.ctx());
	let size = Vec2::splat(ui.spacing().interact_size.y);
	let (rect, response) = ui.allocate_exact_size(size, Sense::click());
	let response = pointer(response);
	let enabled = ui.is_enabled();
	let hovered = enabled && response.hovered();
	let fill = if on {
		p.accent_soft
	} else if hovered && danger {
		p.danger.gamma_multiply(0.16)
	} else if hovered {
		p.raised_hover
	} else {
		Color32::TRANSPARENT
	};
	let colour = if !enabled {
		p.text_weak.gamma_multiply(0.45)
	} else if on {
		p.accent
	} else if hovered && danger {
		p.danger
	} else if hovered {
		p.text
	} else {
		p.text_weak
	};
	ui.painter().rect_filled(rect, control_radius(ui), fill);
	let galley = ui.painter().layout_no_wrap(
		icon.to_string(),
		FontId::proportional((size.y * 0.45).round()),
		colour,
	);
	let pos = rect.center() - galley.size() / 2.0;
	let mut shape = egui::epaint::TextShape::new(pos, galley, colour);
	if flipped {
		shape = shape.with_angle_and_anchor(std::f32::consts::PI, Align2::CENTER_CENTER);
	}
	ui.painter().add(shape);
	response.on_hover_text(tooltip)
}

/// A filled accent button for the main action of a screen or dialog
/// (shadcn's default variant).
pub fn primary_button(ui: &mut Ui, text: impl Into<String>) -> Response {
	let p = palette(ui.ctx());
	ui.add(
		egui::Button::new(RichText::new(text).color(p.on_accent).family(medium()))
			.fill(p.accent)
			.stroke(Stroke::NONE),
	)
}

/// A filled red button confirming a destructive action (shadcn's destructive variant).
pub fn danger_button(ui: &mut Ui, text: impl Into<String>) -> Response {
	let p = palette(ui.ctx());
	ui.add(
		egui::Button::new(RichText::new(text).color(Color32::WHITE).family(medium()))
			.fill(p.danger)
			.stroke(Stroke::NONE),
	)
}

/// A menu item for a destructive action, in red.
pub fn danger_item(ui: &mut Ui, text: impl Into<String>) -> Response {
	let p = palette(ui.ctx());
	ui.add(egui::Button::new(RichText::new(text).color(p.danger)))
}

/// A bordered button with an icon in front of the label (shadcn's outline variant).
pub fn button(ui: &mut Ui, icon: &str, text: &str) -> Response {
	text_button(ui, format!("{icon}  {text}"))
}

/// A bordered button with a text label.
pub fn text_button(ui: &mut Ui, text: impl Into<String>) -> Response {
	ui.add(egui::Button::new(RichText::new(text).family(medium())))
}

/// A single-line text input preset (shadcn's `px-3`, text vertically
/// centred). egui gives it the same height as a button, so the two line up.
/// Chain `.hint_text`, `.desired_width`, `.password` as needed.
pub fn input(text: &mut String) -> egui::TextEdit<'_> {
	egui::TextEdit::singleline(text)
		.margin(Margin::symmetric(12, 0))
		.align(Align2::LEFT_CENTER)
}

/// A row of mutually exclusive icon options, like a radio group, in a
/// framed track as tall as the other controls in its row (see
/// [`text_segmented`]).
pub fn segmented<T: PartialEq + Copy>(
	ui: &mut Ui,
	value: &mut T,
	options: &[(T, &str, &str)],
) -> bool {
	let p = palette(ui.ctx());
	let mut changed = false;
	// the track (border + padding) is as tall as a control beside it
	let pad = 2.0;
	let height = ui.spacing().interact_size.y - 2.0 * (pad + 1.0);
	// the track is a control of its row, rounded like the others; the
	// options inside it nest in it
	let track = control_radius(ui).nw;
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(track))
		.inner_margin(Margin::same(pad as i8))
		.show(ui, |ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			ui.spacing_mut().interact_size.y = height;
			nest(ui, track, pad + 1.0);
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

/// A row of mutually exclusive text options in a framed track, as tall as
/// the other controls. Radii are concentric: option = track − padding.
pub fn text_segmented<T: PartialEq + Copy>(
	ui: &mut Ui,
	value: &mut T,
	options: &[(T, &str)],
) -> bool {
	let p = palette(ui.ctx());
	let mut changed = false;
	// the track (border + padding) is as tall as an input beside it
	let pad = 2.0;
	let height = ui.spacing().interact_size.y - 2.0 * (pad + 1.0);
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(CONTROL_RADIUS + pad as u8))
		.inner_margin(Margin::same(pad as i8))
		.show(ui, |ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			ui.spacing_mut().interact_size.y = height;
			nest(ui, CONTROL_RADIUS + pad as u8, pad + 1.0);
			ui.horizontal(|ui| {
				// rows inside a right-to-left layout fill from the right; keep the
				// options in reading order either way
				let mut ordered: Vec<&(T, &str)> = options.iter().collect();
				if ui.layout().main_dir() == egui::Direction::RightToLeft {
					ordered.reverse();
				}
				for &&(option, label) in &ordered {
					let selected = *value == option;
					let font = FontId::new(13.0, medium());
					let width = ui
						.painter()
						.layout_no_wrap(label.to_string(), font.clone(), p.text)
						.size()
						.x + 24.0;
					let (rect, response) =
						ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
					let response = pointer(response);
					let fill = if selected {
						p.accent_soft
					} else if response.hovered() {
						p.raised_hover
					} else {
						Color32::TRANSPARENT
					};
					ui.painter().rect_filled(rect, control_radius(ui), fill);
					let colour = if selected {
						p.accent
					} else if response.hovered() {
						p.text
					} else {
						p.text_weak
					};
					ui.painter()
						.text(rect.center(), Align2::CENTER_CENTER, label, font, colour);
					if response.clicked() && !selected {
						*value = option;
						changed = true;
					}
				}
			});
		});
	changed
}

/// A small uppercase label over a group (a section title, "Recent"):
/// `text` in natural case, set in capitals with 0.05em of tracking.
pub fn overline(text: &str) -> RichText {
	RichText::new(text.to_uppercase())
		.size(crate::ui::theme::CAPTION)
		.family(semibold())
		.extra_letter_spacing(0.05 * crate::ui::theme::CAPTION)
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
		let text = overline(&format!("{icon}  {title}")).color(p.text_weak);
		ui.add(
			egui::Label::new(text)
				.sense(Sense::click())
				.selectable(false),
		)
	});
	if pointer(header.inner).clicked() {
		state.toggle(ui);
	}
	state.show_body_unindented(ui, |ui| {
		ui.add_space(2.0);
		body(ui);
		ui.add_space(6.0);
	});
	state.store(ui.ctx());
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
	// grids use the control height as their minimum row height; these rows
	// hold text only
	ui.spacing_mut().interact_size.y = 18.0;
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

/// Key numbers about what is open, in a card: equal columns, each a value
/// over its caption, left-aligned on a shared edge.
pub fn stat_strip(ui: &mut Ui, stats: &[(String, &str)]) {
	let p = palette(ui.ctx());
	if stats.is_empty() {
		return;
	}
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(8))
		.inner_margin(Margin::symmetric(12, 10))
		.show(ui, |ui| {
			let (rect, _) =
				ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
			let cell = rect.width() / stats.len() as f32;
			for (i, (value, caption)) in stats.iter().enumerate() {
				let left = rect.left() + cell * i as f32;
				let clip = egui::Rect::from_min_max(
					egui::pos2(left, rect.top()),
					egui::pos2(left + cell - 4.0, rect.bottom()),
				);
				let painter = ui.painter().with_clip_rect(clip);
				painter.text(
					egui::pos2(left, rect.top()),
					Align2::LEFT_TOP,
					value,
					FontId::new(16.0, display()),
					p.text,
				);
				painter.text(
					egui::pos2(left, rect.bottom()),
					Align2::LEFT_BOTTOM,
					*caption,
					FontId::proportional(crate::ui::theme::CAPTION),
					p.text_weak,
				);
			}
		});
}

/// Lays `text` out with tabular figures: every digit takes the widest
/// digit's width, so a number that changes keeps its width. Cal Sans's own
/// digits are proportional, and egui cannot switch on a font's tabular
/// figures. Returns each glyph with the width it takes, and the total size.
fn tabular_glyphs(
	painter: &egui::Painter,
	text: &str,
	font: &FontId,
	colour: Color32,
) -> (Vec<(std::sync::Arc<egui::Galley>, f32)>, Vec2) {
	let digit = ('0'..='9')
		.map(|d| {
			painter
				.layout_no_wrap(d.to_string(), font.clone(), colour)
				.size()
				.x
		})
		.fold(0.0, f32::max);
	let mut size = Vec2::ZERO;
	let glyphs = text
		.chars()
		.map(|c| {
			let galley = painter.layout_no_wrap(c.to_string(), font.clone(), colour);
			let width = if c.is_ascii_digit() {
				digit
			} else {
				galley.size().x
			};
			size.x += width;
			size.y = size.y.max(galley.size().y);
			(galley, width)
		})
		.collect();
	(glyphs, size)
}

/// The size `text` takes with tabular figures (see [`paint_tabular`]).
pub fn tabular_size(ui: &Ui, text: &str, font: &FontId) -> Vec2 {
	tabular_glyphs(ui.painter(), text, font, Color32::WHITE).1
}

/// Paints `text` with tabular figures, placed by `align` at `at`, so a
/// changing number (a timer, a count) does not jitter.
pub fn paint_tabular(
	painter: &egui::Painter,
	at: egui::Pos2,
	align: Align2,
	text: &str,
	font: &FontId,
	colour: Color32,
) {
	let (glyphs, size) = tabular_glyphs(painter, text, font, colour);
	let rect = align.anchor_size(at, size);
	let mut x = rect.left();
	for (galley, width) in glyphs {
		// a digit is centred in its cell
		let left = x + (width - galley.size().x) / 2.0;
		painter.galley(egui::pos2(left, rect.top()), galley, colour);
		x += width;
	}
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

/// How far a panel's scroll bar sits into the panel's own right margin.
pub const SCROLL_GUTTER: f32 = 12.0;

/// Shows a scroll area whose thin floating bar sits in the margin to the
/// right of the column instead of inside it: the area reaches
/// [`SCROLL_GUTTER`] further right and keeps that strip for the bar alone.
/// The content stays exactly as wide as the column, lines up with what is
/// above and below it, and never moves when a bar appears.
pub fn scroll_in_margin<R>(
	ui: &mut Ui,
	scroll: egui::ScrollArea,
	add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
	let column = ui.available_rect_before_wrap();
	let mut rect = column;
	rect.max.x += SCROLL_GUTTER;
	// a child, so the wider rect is not claimed from the parent (panels
	// would grow to fit it)
	let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()));
	let inner = scroll
		.content_margin(Margin {
			right: SCROLL_GUTTER as i8,
			..Margin::ZERO
		})
		.show(&mut child, add_contents)
		.inner;
	let used = child.min_rect().height();
	ui.allocate_rect(
		egui::Rect::from_min_size(column.min, Vec2::new(column.width(), used)),
		Sense::hover(),
	);
	inner
}

/// A menu item with Blender's logo before its text.
pub fn blender_item(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
	match super::icons::blender(ui.ctx()) {
		Some(logo) => ui.add(egui::Button::image_and_text(
			egui::Image::new((logo.id(), Vec2::splat(16.0))),
			text,
		)),
		None => ui.button(text),
	}
}
