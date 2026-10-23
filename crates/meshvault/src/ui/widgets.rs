//! Small styled building blocks shared by the panels.

use eframe::egui::{
	self, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, InnerResponse, Margin,
	Response, RichText, Sense, Stroke, Ui, Vec2, WidgetText,
};

use super::icons;
use super::theme::{
	BODY_LINE_HEIGHT, CAPTION, CARD_RADIUS, CONTROL_RADIUS, CONTROL_SM, CONTROL_XS, LIST_TEXT,
	MODAL_BACKDROP, PILL_RADIUS, Palette, TITLE, dialog_frame, display, medium, palette, semibold,
};

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
	let look = IconLook {
		on,
		..IconLook::default()
	};
	icon_widget(ui, icon, tooltip, look)
}

/// [`icon_toggle`] with the icon turned half a turn, e.g. to point a
/// left-sidebar icon at a panel on the right.
pub fn icon_toggle_flipped(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> Response {
	let look = IconLook {
		on,
		flipped: true,
		..IconLook::default()
	};
	icon_widget(ui, icon, tooltip, look)
}

/// An icon button for a destructive action: it turns red on hover.
pub fn danger_icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
	let look = IconLook {
		danger: true,
		..IconLook::default()
	};
	icon_widget(ui, icon, tooltip, look)
}

/// How an icon button looks.
#[derive(Clone, Copy, Default)]
struct IconLook {
	/// Shows the "on" state with the accent colour.
	on: bool,
	/// Turns red on hover (unless on).
	danger: bool,
	/// Turns the icon half a turn.
	flipped: bool,
}

fn icon_widget(ui: &mut Ui, icon: &str, tooltip: &str, look: IconLook) -> Response {
	let IconLook {
		on,
		danger,
		flipped,
	} = look;
	let p = palette(ui.ctx());
	let size = Vec2::splat(ui.spacing().interact_size.y);
	let (rect, response) = ui.allocate_exact_size(size, Sense::click());
	let response = pointer(response);
	let enabled = ui.is_enabled();
	let hovered = enabled && response.hovered();
	// "on" wins over danger
	let (fill, colour) = if hovered && danger && !on {
		(p.danger.gamma_multiply(0.16), p.danger)
	} else {
		option_colours(&p, on, hovered)
	};
	// disabled dims the icon, but an "on" button keeps its fill
	let colour = if enabled {
		colour
	} else {
		p.text_weak.gamma_multiply(0.45)
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
	// no tooltip when there is nothing to say (an empty one still shows)
	if tooltip.is_empty() {
		response
	} else {
		response.on_hover_text(tooltip)
	}
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
pub fn button(ui: &mut Ui, icon: &str, text: impl Into<String>) -> Response {
	let text = text.into();
	text_button(ui, format!("{icon}  {text}"))
}

/// A bordered button with a text label.
pub fn text_button(ui: &mut Ui, text: impl Into<String>) -> Response {
	ui.add(egui::Button::new(RichText::new(text).family(medium())))
}

/// Caption-sized text in the weak colour: notes, counts and hints. Callers
/// choose how it wraps or truncates.
pub fn caption(ui: &Ui, text: impl Into<String>) -> RichText {
	RichText::new(text)
		.size(CAPTION)
		.color(palette(ui.ctx()).text_weak)
}

/// A modal dialog with the app's dialog frame and backdrop. `id` names it
/// as `Id::unique(id)`; show it with `.show(ctx, ..)`.
pub fn modal(ctx: &egui::Context, id: &str) -> egui::Modal {
	egui::Modal::new(egui::Id::unique(id))
		.frame(dialog_frame(ctx))
		.backdrop_color(MODAL_BACKDROP)
}

/// A dialog's title in the heading style, after an icon when one is given
/// as (glyph, size, colour). With an icon the two share a row.
pub fn dialog_title(
	ui: &mut Ui,
	icon: Option<(&str, f32, Color32)>,
	text: impl Into<String>,
) -> Response {
	let title = RichText::new(text).heading();
	match icon {
		Some((icon, size, colour)) => {
			ui.horizontal(|ui| {
				ui.label(RichText::new(icon).size(size).color(colour));
				ui.label(title);
			})
			.response
		}
		None => ui.label(title),
	}
}

/// A single-line text input preset (shadcn's `px-3`, text vertically
/// centred). egui gives it the same height as a button, so the two line up.
/// Chain `.hint_text`, `.desired_width`, `.password` as needed.
pub fn input(text: &mut String) -> egui::TextEdit<'_> {
	egui::TextEdit::singleline(text)
		.margin(Margin::symmetric(12, 0))
		.align(Align2::LEFT_CENTER)
}

/// A row of mutually exclusive icon options, like a radio group: plain
/// icon buttons with the chosen one highlighted. No track of its own, so in
/// a toolbar it sits level with the buttons around it rather than as a
/// panel inside the panel.
pub fn icon_segmented<T: PartialEq + Copy>(
	ui: &mut Ui,
	value: &mut T,
	options: &[(T, &str, &str)],
) -> bool {
	let mut changed = false;
	ui.horizontal(|ui| {
		for &(option, icon, tooltip) in options {
			if icon_toggle(ui, icon, tooltip, *value == option).clicked() && *value != option {
				*value = option;
				changed = true;
			}
		}
	});
	changed
}

/// The (fill, text) colours of one option in a group: tinted when
/// selected, lightened when hovered, otherwise clear with weak text.
pub(crate) fn option_colours(p: &Palette, selected: bool, hovered: bool) -> (Color32, Color32) {
	if selected {
		(p.accent_soft, p.accent)
	} else if hovered {
		(p.raised_hover, p.text)
	} else {
		(Color32::TRANSPARENT, p.text_weak)
	}
}

/// Margin between a floating bar's border and its controls.
pub(super) const FLOATING_PAD: i8 = 4;

/// The frame of a bar floating over the viewport (the transport bar, the
/// viewport toolbar): the panel colour a touch see-through, a border, the
/// popup shadow and corner `radius`. Its controls are `CONTROL_XS` tall,
/// with corners concentric with the bar's (see [`nest`]).
pub fn floating_bar<R>(
	ui: &mut Ui,
	radius: u8,
	add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
	let p = palette(ui.ctx());
	Frame::NONE
		.fill(p.panel.gamma_multiply(0.94))
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(radius))
		.inner_margin(Margin::same(FLOATING_PAD))
		.shadow(ui.ctx().global_style().visuals.popup_shadow)
		.show(ui, |ui| {
			ui.spacing_mut().interact_size.y = CONTROL_XS;
			nest(ui, radius, FLOATING_PAD as f32 + 1.0);
			add(ui)
		})
}

/// Padding between a segmented track's border and its options.
const TRACK_PAD: f32 = 2.0;

/// A framed track for a row of options, `outer_height` tall with its
/// border and padding. `add` gets the height left for the options; their
/// corners are concentric with the track's (option = track − padding).
pub fn segment_track<R>(ui: &mut Ui, outer_height: f32, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
	let p = palette(ui.ctx());
	let height = outer_height - 2.0 * (TRACK_PAD + 1.0);
	let track = CONTROL_RADIUS + TRACK_PAD as u8;
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(track))
		.inner_margin(Margin::same(TRACK_PAD as i8))
		.show(ui, |ui| {
			ui.spacing_mut().item_spacing.x = 2.0;
			ui.spacing_mut().interact_size.y = height;
			nest(ui, track, TRACK_PAD + 1.0);
			add(ui, height)
		})
		.inner
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
	let outer_height = ui.spacing().interact_size.y;
	segment_track(ui, outer_height, |ui, height| {
		ui.horizontal(|ui| {
			// rows inside a right-to-left layout fill from the right; keep the
			// options in reading order either way
			let mut ordered: Vec<&(T, &str)> = options.iter().collect();
			if ui.layout().main_dir() == egui::Direction::RightToLeft {
				ordered.reverse();
			}
			for &&(option, label) in &ordered {
				let selected = *value == option;
				let font = FontId::new(LIST_TEXT, medium());
				let width = ui
					.painter()
					.layout_no_wrap(label.to_string(), font.clone(), p.text)
					.size()
					.x + 24.0;
				let (rect, response) =
					ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
				let response = pointer(response);
				let (fill, colour) = option_colours(&p, selected, response.hovered());
				ui.painter().rect_filled(rect, control_radius(ui), fill);
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
		.size(CAPTION)
		.family(semibold())
		.extra_letter_spacing(0.05 * CAPTION)
}

/// A collapsible panel section with a small uppercase title.
pub fn section(ui: &mut Ui, id: &str, title: &str, default_open: bool, body: impl FnOnce(&mut Ui)) {
	let p = palette(ui.ctx());
	let id = ui.make_persistent_id(id);
	let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
		ui.ctx(),
		id,
		default_open,
	);
	let header = ui.horizontal(|ui| {
		let icon = if state.is_open() {
			icons::CARET_DOWN
		} else {
			icons::CARET_RIGHT
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
		.corner_radius(CornerRadius::same(PILL_RADIUS))
		.inner_margin(Margin::symmetric(8, 2))
		.show(ui, |ui| {
			let text: WidgetText = text.into();
			ui.label(text.color(colour))
		})
		.inner
}

/// An icon and a note beside it that wraps, both at the body line height
/// so the icon sits on the first line. Callers style both parts.
pub fn note(ui: &mut Ui, icon: RichText, text: RichText) -> Response {
	ui.horizontal_top(|ui| {
		ui.label(icon.line_height(Some(BODY_LINE_HEIGHT)));
		ui.add(egui::Label::new(text.line_height(Some(BODY_LINE_HEIGHT))).wrap());
	})
	.response
}

/// Two-column label/value rows with weak labels.
pub fn properties(ui: &mut Ui, id: &str, rows: &[(String, String)]) {
	let p = palette(ui.ctx());
	// in a scope, so what is added after the grid keeps the caller's height
	ui.scope(|ui| {
		// grids use the control height as their minimum row height; these
		// rows hold text only
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
		.corner_radius(CornerRadius::same(CARD_RADIUS))
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
					FontId::new(TITLE, display()),
					p.text,
				);
				painter.text(
					egui::pos2(left, rect.bottom()),
					Align2::LEFT_BOTTOM,
					*caption,
					FontId::proportional(CAPTION),
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

/// `text` on one line at most `width` wide, ending in "…" when it is cut
/// short. Whoever draws it keeps the full text on hover.
pub fn truncated(
	painter: &egui::Painter,
	text: &str,
	font: FontId,
	colour: Color32,
	width: f32,
) -> std::sync::Arc<egui::Galley> {
	let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, colour);
	job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
	painter.layout_job(job)
}

/// A dashed outline along exactly `rect`, with square corners, used as a
/// drop target. `dash` is the length of a dash and of the gap after it.
pub fn dashed_rect(
	painter: &egui::Painter,
	rect: egui::Rect,
	width: f32,
	colour: Color32,
	dash: (f32, f32),
) {
	let corners = [
		rect.left_top(),
		rect.right_top(),
		rect.right_bottom(),
		rect.left_bottom(),
		rect.left_top(),
	];
	for pair in corners.windows(2) {
		painter.extend(egui::Shape::dashed_line(
			pair,
			Stroke::new(width, colour),
			dash.0,
			dash.1,
		));
	}
}

/// Fills `rect` with `top` fading down into `bottom`, as the 3D viewport's
/// background does. Clipped to whatever `painter` clips to.
pub fn vertical_gradient(painter: &egui::Painter, rect: egui::Rect, top: Color32, bottom: Color32) {
	let mut mesh = egui::Mesh::default();
	mesh.colored_vertex(rect.left_top(), top);
	mesh.colored_vertex(rect.right_top(), top);
	mesh.colored_vertex(rect.right_bottom(), bottom);
	mesh.colored_vertex(rect.left_bottom(), bottom);
	mesh.add_triangle(0, 1, 2);
	mesh.add_triangle(0, 2, 3);
	painter.add(egui::Shape::mesh(mesh));
}

/// How far a panel's scroll bar sits into the panel's own right margin.
const SCROLL_GUTTER: f32 = 12.0;

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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn properties_leave_the_callers_control_height_alone() {
		let ctx = egui::Context::default();
		let mut heights = None;
		let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
			ui.spacing_mut().interact_size.y = 36.0;
			properties(ui, "test_grid", &[("Key".into(), "Value".into())]);
			// it was left at 18 for whatever came next (bug B44)
			heights = Some(ui.spacing().interact_size.y);
		});
		// the font atlas upload is never applied here
		output.textures_delta.clear();
		assert_eq!(heights, Some(36.0));
	}
}
