//! The app's look: Inter for text, Phosphor for icons, and a dark and a
//! light palette with consistent spacing, rounding and elevation.

use std::sync::Arc;

use eframe::egui::{
	self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
	Stroke, TextStyle, Theme, Vec2,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
	#[default]
	Dark,
	Light,
}

impl ThemeMode {
	pub fn egui(self) -> Theme {
		match self {
			ThemeMode::Dark => Theme::Dark,
			ThemeMode::Light => Theme::Light,
		}
	}
}

/// `0xRRGGBB` to a colour.
const fn hex(v: u32) -> Color32 {
	Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Named colours used by custom widgets. Read with [`palette`].
#[derive(Clone, Copy)]
pub struct Palette {
	/// Window background behind panels.
	pub bg: Color32,
	/// Side panels and bars.
	pub panel: Color32,
	/// Cards, inputs and other raised surfaces.
	pub raised: Color32,
	/// Hovered raised surfaces.
	pub raised_hover: Color32,
	pub border: Color32,
	pub text: Color32,
	pub text_weak: Color32,
	pub accent: Color32,
	/// Text drawn on top of `accent`.
	pub on_accent: Color32,
	pub accent_soft: Color32,
	pub success: Color32,
	pub warning: Color32,
	pub danger: Color32,
	/// The 3D viewport's background, top and bottom of the gradient.
	pub viewport_top: Color32,
	pub viewport_bottom: Color32,
	pub grid: Color32,
}

const DARK: Palette = Palette {
	bg: hex(0x111214),
	panel: hex(0x17181B),
	raised: hex(0x202227),
	raised_hover: hex(0x2A2D33),
	border: hex(0x2C2F36),
	text: hex(0xE7E8EC),
	text_weak: hex(0x8E929C),
	accent: hex(0x7C8CFF),
	on_accent: hex(0x0E1020),
	accent_soft: Color32::from_rgba_premultiplied(0x23, 0x28, 0x4A, 0xFF),
	success: hex(0x4CC38A),
	warning: hex(0xE5B454),
	danger: hex(0xF0716A),
	viewport_top: hex(0x24272E),
	viewport_bottom: hex(0x15161A),
	grid: Color32::from_rgba_premultiplied(0x30, 0x34, 0x3C, 0xA0),
};

const LIGHT: Palette = Palette {
	bg: hex(0xEEF0F3),
	panel: hex(0xF8F9FB),
	raised: hex(0xFFFFFF),
	raised_hover: hex(0xF0F2F6),
	border: hex(0xDDE0E6),
	text: hex(0x1D2026),
	text_weak: hex(0x6A707C),
	accent: hex(0x4F5FE8),
	on_accent: hex(0xFFFFFF),
	accent_soft: hex(0xE3E6FD),
	success: hex(0x1F9D63),
	warning: hex(0xB7811A),
	danger: hex(0xD64541),
	viewport_top: hex(0xE9ECF1),
	viewport_bottom: hex(0xC9CED7),
	grid: Color32::from_rgba_premultiplied(0x9A, 0xA1, 0xAE, 0x90),
};

/// The palette matching the context's current theme.
pub fn palette(ctx: &egui::Context) -> Palette {
	if ctx.global_style().visuals.dark_mode {
		DARK
	} else {
		LIGHT
	}
}

/// Font family for semibold text (headings, labels that need weight).
pub fn semibold() -> FontFamily {
	FontFamily::Name("semibold".into())
}

pub fn install_fonts(ctx: &egui::Context) {
	let mut fonts = FontDefinitions::default();
	let mut add = |name: &str, bytes: &'static [u8]| {
		fonts
			.font_data
			.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
	};
	add("inter", include_bytes!("../../assets/Inter-Regular.ttf"));
	add(
		"inter-semibold",
		include_bytes!("../../assets/Inter-SemiBold.ttf"),
	);
	add("phosphor", include_bytes!("../../assets/Phosphor.ttf"));

	// Phosphor first: it only has private-use codepoints, and Inter puts
	// alternate letter shapes in that range, so Inter would shadow the icons.
	// Then Inter for text, then egui's defaults (emoji, symbols).
	let defaults = fonts
		.families
		.get(&FontFamily::Proportional)
		.cloned()
		.unwrap_or_default();
	let chain = |text_font: &str| {
		let mut v = vec!["phosphor".to_owned(), text_font.to_owned()];
		v.extend(defaults.iter().cloned());
		v
	};
	fonts
		.families
		.insert(FontFamily::Proportional, chain("inter"));
	fonts.families.insert(semibold(), chain("inter-semibold"));
	fonts
		.families
		.entry(FontFamily::Monospace)
		.or_default()
		.insert(0, "phosphor".to_owned());
	ctx.set_fonts(fonts);
}

fn style_for(p: &Palette, dark: bool) -> egui::Style {
	let mut style = egui::Style::default();
	style.text_styles = [
		(
			TextStyle::Small,
			FontId::new(11.5, FontFamily::Proportional),
		),
		(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
		(
			TextStyle::Button,
			FontId::new(13.5, FontFamily::Proportional),
		),
		(TextStyle::Heading, FontId::new(17.0, semibold())),
		(
			TextStyle::Monospace,
			FontId::new(12.5, FontFamily::Monospace),
		),
	]
	.into();

	let s = &mut style.spacing;
	s.item_spacing = Vec2::new(8.0, 6.0);
	s.button_padding = Vec2::new(10.0, 5.0);
	s.interact_size = Vec2::new(28.0, 26.0);
	s.window_margin = Margin::same(16);
	s.menu_margin = Margin::same(6);
	s.indent = 16.0;
	s.icon_spacing = 6.0;

	let mut v = if dark {
		egui::Visuals::dark()
	} else {
		egui::Visuals::light()
	};
	v.panel_fill = p.panel;
	v.window_fill = p.panel;
	v.extreme_bg_color = p.raised;
	v.text_edit_bg_color = Some(p.raised);
	v.faint_bg_color = if dark { hex(0x1B1C20) } else { hex(0xF2F3F6) };
	v.code_bg_color = p.raised;
	v.window_stroke = Stroke::new(1.0, p.border);
	v.window_corner_radius = CornerRadius::same(12);
	v.menu_corner_radius = CornerRadius::same(10);
	v.window_shadow = Shadow {
		offset: [0, 12],
		blur: 36,
		spread: 0,
		color: Color32::from_black_alpha(if dark { 110 } else { 40 }),
	};
	v.popup_shadow = Shadow {
		offset: [0, 6],
		blur: 18,
		spread: 0,
		color: Color32::from_black_alpha(if dark { 90 } else { 30 }),
	};
	v.hyperlink_color = p.accent;
	v.warn_fg_color = p.warning;
	v.error_fg_color = p.danger;
	v.selection.bg_fill = p.accent_soft;
	v.selection.stroke = Stroke::new(1.0, p.accent);
	v.collapsing_header_frame = false;
	v.indent_has_left_vline = false;
	v.slider_trailing_fill = true;

	let radius = CornerRadius::same(7);
	let w = &mut v.widgets;
	w.noninteractive.bg_fill = p.panel;
	w.noninteractive.weak_bg_fill = p.panel;
	w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
	w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
	w.noninteractive.corner_radius = radius;
	for (state, fill) in [
		(&mut w.inactive, p.raised),
		(&mut w.hovered, p.raised_hover),
		(&mut w.active, p.raised_hover),
		(&mut w.open, p.raised_hover),
	] {
		state.bg_fill = fill;
		state.weak_bg_fill = fill;
		state.corner_radius = radius;
		state.expansion = 0.0;
	}
	w.inactive.bg_stroke = Stroke::new(1.0, p.border);
	w.inactive.fg_stroke = Stroke::new(1.0, p.text);
	w.hovered.bg_stroke = Stroke::new(1.0, p.border.gamma_multiply(1.6));
	w.hovered.fg_stroke = Stroke::new(1.5, p.text);
	w.active.bg_stroke = Stroke::new(1.0, p.accent);
	w.active.fg_stroke = Stroke::new(1.5, p.text);
	w.open.bg_stroke = Stroke::new(1.0, p.border);
	w.open.fg_stroke = Stroke::new(1.0, p.text);
	style.visuals = v;
	style
}

pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
	ctx.set_style_of(Theme::Dark, style_for(&DARK, true));
	ctx.set_style_of(Theme::Light, style_for(&LIGHT, false));
	ctx.set_theme(mode.egui());
}
