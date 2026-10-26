//! The app's look: Cal Sans for text and titles, Phosphor for icons, and
//! colour themes (each a dark and a light palette) with consistent spacing,
//! rounding and elevation.

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
	/// egui's `faint_bg_color`: the faint background of striped grids.
	pub faint: Color32,
	pub success: Color32,
	pub warning: Color32,
	pub danger: Color32,
	/// The 3D viewport's background, top and bottom of the gradient.
	pub viewport_top: Color32,
	pub viewport_bottom: Color32,
	pub grid: Color32,
}

/// A colour theme: a dark and a light palette sharing one accent and tint.
/// [`ThemeMode`] picks which of the two is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColourTheme {
	/// Indigo on neutral greys: the original look.
	#[default]
	Meshvault,
	Ocean,
	Forest,
	Ember,
	Rose,
	Grape,
}

impl ColourTheme {
	pub const ALL: [ColourTheme; 6] = [
		ColourTheme::Meshvault,
		ColourTheme::Ocean,
		ColourTheme::Forest,
		ColourTheme::Ember,
		ColourTheme::Rose,
		ColourTheme::Grape,
	];

	pub fn name(self) -> &'static str {
		match self {
			ColourTheme::Meshvault => "Meshvault",
			ColourTheme::Ocean => "Ocean",
			ColourTheme::Forest => "Forest",
			ColourTheme::Ember => "Ember",
			ColourTheme::Rose => "Rose",
			ColourTheme::Grape => "Grape",
		}
	}

	/// The theme's palette in `mode`.
	pub fn palette(self, mode: ThemeMode) -> Palette {
		let (dark, light) = match self {
			ColourTheme::Meshvault => (MESHVAULT_DARK, MESHVAULT_LIGHT),
			ColourTheme::Ocean => (OCEAN_DARK, OCEAN_LIGHT),
			ColourTheme::Forest => (FOREST_DARK, FOREST_LIGHT),
			ColourTheme::Ember => (EMBER_DARK, EMBER_LIGHT),
			ColourTheme::Rose => (ROSE_DARK, ROSE_LIGHT),
			ColourTheme::Grape => (GRAPE_DARK, GRAPE_LIGHT),
		};
		match mode {
			ThemeMode::Dark => dark,
			ThemeMode::Light => light,
		}
	}
}

// Status colours shared by every theme, so green, amber and red keep
// meaning done, pending and failed whatever the accent.
const SUCCESS_DARK: Color32 = hex(0x4CC38A);
const WARNING_DARK: Color32 = hex(0xE5B454);
const DANGER_DARK: Color32 = hex(0xF0716A);
const SUCCESS_LIGHT: Color32 = hex(0x1F9D63);
const WARNING_LIGHT: Color32 = hex(0xB7811A);
const DANGER_LIGHT: Color32 = hex(0xD64541);

const MESHVAULT_DARK: Palette = Palette {
	bg: hex(0x111214),
	panel: hex(0x17181B),
	raised: hex(0x202227),
	raised_hover: hex(0x2A2D33),
	border: hex(0x2C2F36),
	text: hex(0xE7E8EC),
	text_weak: hex(0x8E929C),
	accent: hex(0x7C8CFF),
	on_accent: hex(0x0E1020),
	accent_soft: hex(0x23284A),
	faint: hex(0x1B1C20),
	success: SUCCESS_DARK,
	warning: WARNING_DARK,
	danger: DANGER_DARK,
	viewport_top: hex(0x24272E),
	viewport_bottom: hex(0x15161A),
	grid: Color32::from_rgba_premultiplied(0x30, 0x34, 0x3C, 0xA0),
};

const MESHVAULT_LIGHT: Palette = Palette {
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
	faint: hex(0xF2F3F6),
	success: SUCCESS_LIGHT,
	warning: WARNING_LIGHT,
	danger: DANGER_LIGHT,
	viewport_top: hex(0xE9ECF1),
	viewport_bottom: hex(0xC9CED7),
	grid: Color32::from_rgba_premultiplied(0x9A, 0xA1, 0xAE, 0x90),
};

const OCEAN_DARK: Palette = Palette {
	bg: hex(0x0E1316),
	panel: hex(0x131A1E),
	raised: hex(0x1B2429),
	raised_hover: hex(0x232F35),
	border: hex(0x26333A),
	text: hex(0xE4EEF1),
	text_weak: hex(0x86989F),
	accent: hex(0x3CC6D4),
	on_accent: hex(0x04181B),
	accent_soft: hex(0x123A40),
	faint: hex(0x161E22),
	success: SUCCESS_DARK,
	warning: WARNING_DARK,
	danger: DANGER_DARK,
	viewport_top: hex(0x1E2A30),
	viewport_bottom: hex(0x10171A),
	grid: Color32::from_rgba_premultiplied(0x2A, 0x3A, 0x40, 0xA0),
};

const OCEAN_LIGHT: Palette = Palette {
	bg: hex(0xE9F0F2),
	panel: hex(0xF6FAFB),
	raised: hex(0xFFFFFF),
	raised_hover: hex(0xEDF4F6),
	border: hex(0xD3E0E4),
	text: hex(0x15252A),
	text_weak: hex(0x5E737A),
	accent: hex(0x0B7A86),
	on_accent: hex(0xFFFFFF),
	accent_soft: hex(0xD4F0F3),
	faint: hex(0xEFF5F7),
	success: SUCCESS_LIGHT,
	warning: WARNING_LIGHT,
	danger: DANGER_LIGHT,
	viewport_top: hex(0xE4EEF1),
	viewport_bottom: hex(0xC1D2D8),
	grid: Color32::from_rgba_premultiplied(0x8F, 0xA8, 0xB0, 0x90),
};

const FOREST_DARK: Palette = Palette {
	bg: hex(0x0F1311),
	panel: hex(0x151A17),
	raised: hex(0x1D2420),
	raised_hover: hex(0x262E29),
	border: hex(0x29322D),
	text: hex(0xE5ECE7),
	text_weak: hex(0x8A978F),
	accent: hex(0x5FCB86),
	on_accent: hex(0x08170E),
	accent_soft: hex(0x173826),
	faint: hex(0x181E1A),
	success: SUCCESS_DARK,
	warning: WARNING_DARK,
	danger: DANGER_DARK,
	viewport_top: hex(0x212A25),
	viewport_bottom: hex(0x111613),
	grid: Color32::from_rgba_premultiplied(0x2E, 0x3A, 0x33, 0xA0),
};

const FOREST_LIGHT: Palette = Palette {
	bg: hex(0xECF1EC),
	panel: hex(0xF7FAF7),
	raised: hex(0xFFFFFF),
	raised_hover: hex(0xEEF3EF),
	border: hex(0xD7E1D9),
	text: hex(0x18231C),
	text_weak: hex(0x627268),
	accent: hex(0x23824B),
	on_accent: hex(0xFFFFFF),
	accent_soft: hex(0xD8EEDF),
	faint: hex(0xF0F5F1),
	success: SUCCESS_LIGHT,
	warning: WARNING_LIGHT,
	danger: DANGER_LIGHT,
	viewport_top: hex(0xE6EEE7),
	viewport_bottom: hex(0xC5D3C8),
	grid: Color32::from_rgba_premultiplied(0x95, 0xA8, 0x9A, 0x90),
};

const EMBER_DARK: Palette = Palette {
	bg: hex(0x141110),
	panel: hex(0x1A1615),
	raised: hex(0x241F1D),
	raised_hover: hex(0x2E2826),
	border: hex(0x342C29),
	text: hex(0xEFE8E4),
	text_weak: hex(0x9C918B),
	accent: hex(0xFF8A4C),
	on_accent: hex(0x1E0D04),
	accent_soft: hex(0x4A2716),
	faint: hex(0x1E1918),
	success: SUCCESS_DARK,
	warning: WARNING_DARK,
	danger: DANGER_DARK,
	viewport_top: hex(0x2B2421),
	viewport_bottom: hex(0x171312),
	grid: Color32::from_rgba_premultiplied(0x3C, 0x33, 0x2F, 0xA0),
};

const EMBER_LIGHT: Palette = Palette {
	bg: hex(0xF4EFEC),
	panel: hex(0xFBF8F6),
	raised: hex(0xFFFFFF),
	raised_hover: hex(0xF5EFEB),
	border: hex(0xE6DCD6),
	text: hex(0x2A1F1A),
	text_weak: hex(0x7A6A62),
	accent: hex(0xC4521F),
	on_accent: hex(0xFFFFFF),
	accent_soft: hex(0xFBE3D6),
	faint: hex(0xF6F1EE),
	success: SUCCESS_LIGHT,
	warning: WARNING_LIGHT,
	danger: DANGER_LIGHT,
	viewport_top: hex(0xF1E9E4),
	viewport_bottom: hex(0xD8CAC2),
	grid: Color32::from_rgba_premultiplied(0xAE, 0xA0, 0x98, 0x90),
};

const ROSE_DARK: Palette = Palette {
	bg: hex(0x141012),
	panel: hex(0x1A1518),
	raised: hex(0x241D21),
	raised_hover: hex(0x2E252A),
	border: hex(0x352B31),
	text: hex(0xF0E6EB),
	text_weak: hex(0x9D8F96),
	accent: hex(0xF472B6),
	on_accent: hex(0x22071A),
	accent_soft: hex(0x4A1D36),
	faint: hex(0x1E181B),
	success: SUCCESS_DARK,
	warning: WARNING_DARK,
	danger: DANGER_DARK,
	viewport_top: hex(0x2B2228),
	viewport_bottom: hex(0x171215),
	grid: Color32::from_rgba_premultiplied(0x3D, 0x31, 0x38, 0xA0),
};

const ROSE_LIGHT: Palette = Palette {
	bg: hex(0xF5EEF1),
	panel: hex(0xFCF8FA),
	raised: hex(0xFFFFFF),
	raised_hover: hex(0xF7EEF2),
	border: hex(0xE8D9E0),
	text: hex(0x2A1A22),
	text_weak: hex(0x7D6872),
	accent: hex(0xC2256F),
	on_accent: hex(0xFFFFFF),
	accent_soft: hex(0xFADCEB),
	faint: hex(0xF8F1F4),
	success: SUCCESS_LIGHT,
	warning: WARNING_LIGHT,
	danger: DANGER_LIGHT,
	viewport_top: hex(0xF3E8ED),
	viewport_bottom: hex(0xDBC8D1),
	grid: Color32::from_rgba_premultiplied(0xB0, 0x9C, 0xA6, 0x90),
};

const GRAPE_DARK: Palette = Palette {
	bg: hex(0x120F17),
	panel: hex(0x18141E),
	raised: hex(0x211C29),
	raised_hover: hex(0x2A2434),
	border: hex(0x302940),
	text: hex(0xEBE6F2),
	text_weak: hex(0x958DA3),
	accent: hex(0xB18CFF),
	on_accent: hex(0x150A2A),
	accent_soft: hex(0x352555),
	faint: hex(0x1B1722),
	success: SUCCESS_DARK,
	warning: WARNING_DARK,
	danger: DANGER_DARK,
	viewport_top: hex(0x272033),
	viewport_bottom: hex(0x14111A),
	grid: Color32::from_rgba_premultiplied(0x37, 0x30, 0x45, 0xA0),
};

const GRAPE_LIGHT: Palette = Palette {
	bg: hex(0xF1EEF6),
	panel: hex(0xFAF8FC),
	raised: hex(0xFFFFFF),
	raised_hover: hex(0xF2EEF8),
	border: hex(0xE0D9EA),
	text: hex(0x221B2E),
	text_weak: hex(0x6F6680),
	accent: hex(0x7C4DDB),
	on_accent: hex(0xFFFFFF),
	accent_soft: hex(0xEAE0FC),
	faint: hex(0xF5F2F9),
	success: SUCCESS_LIGHT,
	warning: WARNING_LIGHT,
	danger: DANGER_LIGHT,
	viewport_top: hex(0xEEE9F5),
	viewport_bottom: hex(0xD0C7DF),
	grid: Color32::from_rgba_premultiplied(0xA4, 0x9B, 0xB4, 0x90),
};

/// Where [`apply`] keeps the chosen colour theme for [`palette`].
fn theme_id() -> egui::Id {
	egui::Id::unique("colour_theme")
}

/// The colour theme [`apply`] last chose for this context.
pub fn colour_theme(ctx: &egui::Context) -> ColourTheme {
	ctx.data(|d| d.get_temp(theme_id())).unwrap_or_default()
}

/// The palette matching the context's current theme.
pub fn palette(ctx: &egui::Context) -> Palette {
	let mode = if ctx.global_style().visuals.dark_mode {
		ThemeMode::Dark
	} else {
		ThemeMode::Light
	};
	colour_theme(ctx).palette(mode)
}

/// Font family for semibold interface text (labels that need weight).
pub fn semibold() -> FontFamily {
	FontFamily::Name("semibold".into())
}

/// Font family for medium-weight text: button labels, like shadcn's `font-medium`.
pub fn medium() -> FontFamily {
	FontFamily::Name("medium".into())
}

// Control sizes, following shadcn/ui: default controls are 36px tall
// (`h-9`), small ones 32px (`h-8`). Buttons and text inputs share a height
// so they line up when placed side by side.
/// Height of default buttons, inputs and icon buttons.
pub const CONTROL: f32 = 36.0;
/// Height of small controls: toolbars, menus and buttons inside cards.
pub const CONTROL_SM: f32 = 32.0;
/// Height of the controls in floating bars and in cards.
pub const CONTROL_XS: f32 = 30.0;
/// Height of the compact controls in the library and on the welcome screen.
pub const CONTROL_XXS: f32 = 28.0;
/// Height of rows in trees and lists.
pub const ROW: f32 = 26.0;

// The type scale.
/// Size of captions, counts and small labels: `TextStyle::Small`, and the
/// smallest interface text.
pub const CAPTION: f32 = 12.0;
/// Size of the names in library lists, and of segmented-control labels.
pub const LIST_TEXT: f32 = 13.0;
/// Size of body text (`TextStyle::Body` and `Button`; shadcn's `text-sm`).
pub const BODY: f32 = 14.0;
/// Size of panel titles (the inspector's and the library's) and stat values.
pub const TITLE: f32 = 16.0;
/// Size of dialog titles: `TextStyle::Heading`.
pub const HEADING: f32 = 18.0;

/// Line height of body text that can wrap to several lines: 20px at 14px
/// (about 1.43). Cal Sans's own is 1.145, too tight for paragraphs.
pub const BODY_LINE_HEIGHT: f32 = 20.0;
/// Corner radius of buttons and inputs (`rounded-md`).
pub const CONTROL_RADIUS: u8 = 6;
/// Corner radius of cards: the stat strip, map cards, tiles, callouts.
pub const CARD_RADIUS: u8 = 8;
/// Corner radius of popup menus.
pub const MENU_RADIUS: u8 = 10;
/// Corner radius of windows (egui's `window_corner_radius`).
pub const WINDOW_RADIUS: u8 = 12;
/// Corner radius of modal dialogs.
pub const DIALOG_RADIUS: u8 = 14;
/// Corner radius of pills: fully round ends at any height.
pub const PILL_RADIUS: u8 = u8::MAX;

/// What a modal dialog dims the window behind it with.
pub const MODAL_BACKDROP: Color32 = Color32::from_black_alpha(140);

/// Display text: the brand mark, titles and headings.
pub fn display() -> FontFamily {
	FontFamily::Name("display".into())
}

/// The frame for modal dialogs: roomier than egui's popup frame.
pub fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
	let p = palette(ctx);
	egui::Frame::NONE
		.fill(p.panel)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(DIALOG_RADIUS))
		.inner_margin(Margin::same(24))
		.shadow(ctx.global_style().visuals.window_shadow)
}

pub fn install_fonts(ctx: &egui::Context) {
	let mut fonts = FontDefinitions::default();
	let mut add = |name: &str, bytes: &'static [u8]| {
		fonts
			.font_data
			.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
	};
	// Cal Sans Text UI for interface text, Cal Sans for display text
	// (see assets/fonts/cal-sans/OFL.txt)
	add(
		"ui",
		include_bytes!("../../assets/fonts/cal-sans/CalSansTextUI-Regular.ttf"),
	);
	add(
		"ui-medium",
		include_bytes!("../../assets/fonts/cal-sans/CalSansTextUI-Medium.ttf"),
	);
	add(
		"ui-semibold",
		include_bytes!("../../assets/fonts/cal-sans/CalSansTextUI-SemiBold.ttf"),
	);
	add(
		"display",
		include_bytes!("../../assets/fonts/cal-sans/CalSans-SemiBold.ttf"),
	);
	add(
		"phosphor",
		include_bytes!("../../assets/fonts/phosphor/Phosphor.ttf"),
	);

	// Phosphor first: it has only private-use icon codepoints, so it never
	// replaces letters, and no text font can shadow an icon. Then the text
	// font, then egui's defaults (emoji, symbols).
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
	fonts.families.insert(FontFamily::Proportional, chain("ui"));
	fonts.families.insert(medium(), chain("ui-medium"));
	fonts.families.insert(semibold(), chain("ui-semibold"));
	fonts.families.insert(display(), chain("display"));
	fonts
		.families
		.entry(FontFamily::Monospace)
		.or_default()
		.insert(0, "phosphor".to_owned());
	ctx.set_fonts(fonts);
}

fn style_for(p: &Palette, dark: bool) -> egui::Style {
	let mut style = egui::Style {
		text_styles: [
			(
				TextStyle::Small,
				FontId::new(CAPTION, FontFamily::Proportional),
			),
			(TextStyle::Body, FontId::new(BODY, FontFamily::Proportional)),
			(TextStyle::Button, FontId::new(BODY, medium())),
			(TextStyle::Heading, FontId::new(HEADING, display())),
			(
				TextStyle::Monospace,
				FontId::new(12.5, FontFamily::Monospace),
			),
		]
		.into(),
		..Default::default()
	};

	let s = &mut style.spacing;
	s.item_spacing = Vec2::new(8.0, 6.0);
	// egui pads buttons and text inputs alike, and gives both the same
	// minimum height, so these two settings size every control
	s.button_padding = Vec2::new(16.0, 8.0);
	s.interact_size = Vec2::new(CONTROL, CONTROL);
	s.window_margin = Margin::same(16);
	s.menu_margin = Margin::symmetric(8, 8);
	// scroll bars stay out of the way: hidden until the pointer is over
	// their column, then a faint 2px line that widens to a full bar on hover
	// or while scrolling. They float and take no space; panels put them in
	// their right margin (see `widgets::scroll_in_margin`), so content never
	// shifts or sits under them. The edge that hides content fades out, so
	// more to scroll still shows without a bar.
	s.scroll = egui::style::ScrollStyle {
		floating: true,
		bar_width: 8.0,
		floating_width: 2.0,
		floating_allocated_width: 0.0,
		bar_inner_margin: 2.0,
		bar_outer_margin: 3.0,
		dormant_background_opacity: 0.0,
		dormant_handle_opacity: 0.0,
		active_background_opacity: 0.0,
		active_handle_opacity: 0.55,
		interact_background_opacity: 0.3,
		interact_handle_opacity: 0.9,
		..egui::style::ScrollStyle::solid()
	};
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
	v.faint_bg_color = p.faint;
	v.code_bg_color = p.raised;
	v.window_stroke = Stroke::new(1.0, p.border);
	v.window_corner_radius = CornerRadius::same(WINDOW_RADIUS);
	v.menu_corner_radius = CornerRadius::same(MENU_RADIUS);
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
	// a pointing hand over buttons; widgets.rs adds it to other clickable things
	v.interact_cursor = Some(egui::CursorIcon::PointingHand);

	let radius = CornerRadius::same(CONTROL_RADIUS);
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

/// Styles the context in `colours`, showing its `mode` palette.
pub fn apply(ctx: &egui::Context, mode: ThemeMode, colours: ColourTheme) {
	ctx.data_mut(|d| d.insert_temp(theme_id(), colours));
	let dark = colours.palette(ThemeMode::Dark);
	let light = colours.palette(ThemeMode::Light);
	ctx.set_style_of(Theme::Dark, style_for(&dark, true));
	ctx.set_style_of(Theme::Light, style_for(&light, false));
	ctx.set_theme(mode.egui());
}

#[cfg(test)]
mod tests {
	use super::*;

	/// WCAG contrast ratio between two opaque colours.
	fn contrast(a: Color32, b: Color32) -> f32 {
		let luminance = |c: Color32| {
			let channel = |v: u8| {
				let v = v as f32 / 255.0;
				if v <= 0.04045 {
					v / 12.92
				} else {
					((v + 0.055) / 1.055).powf(2.4)
				}
			};
			0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
		};
		let (a, b) = (luminance(a), luminance(b));
		(a.max(b) + 0.05) / (a.min(b) + 0.05)
	}

	#[test]
	fn every_theme_keeps_its_text_readable() {
		for colours in ColourTheme::ALL {
			for mode in [ThemeMode::Dark, ThemeMode::Light] {
				let p = colours.palette(mode);
				let name = format!("{} {mode:?}", colours.name());
				for (what, fg, bg, least) in [
					("text on panels", p.text, p.panel, 7.0),
					("text on raised", p.text, p.raised, 7.0),
					("weak text on panels", p.text_weak, p.panel, 4.5),
					("accent buttons", p.on_accent, p.accent, 4.5),
					("accent text", p.accent, p.panel, 3.0),
					("selected options", p.accent, p.accent_soft, 3.0),
				] {
					let ratio = contrast(fg, bg);
					assert!(ratio >= least, "{name}: {what} {ratio:.2} < {least}");
				}
			}
		}
	}

	#[test]
	fn themes_differ_and_the_first_is_the_original() {
		let accents: std::collections::HashSet<_> = ColourTheme::ALL
			.iter()
			.map(|c| c.palette(ThemeMode::Dark).accent)
			.collect();
		assert_eq!(accents.len(), ColourTheme::ALL.len());
		assert_eq!(ColourTheme::default(), ColourTheme::ALL[0]);
		assert_eq!(
			ColourTheme::Meshvault.palette(ThemeMode::Dark).accent,
			hex(0x7C8CFF)
		);
	}

	#[test]
	fn the_palette_follows_the_applied_theme() {
		let ctx = egui::Context::default();
		apply(&ctx, ThemeMode::Light, ColourTheme::Forest);
		ctx.run_ui(egui::RawInput::default(), |_| {})
			.textures_delta
			.clear();
		assert_eq!(colour_theme(&ctx), ColourTheme::Forest);
		assert_eq!(
			palette(&ctx).accent,
			ColourTheme::Forest.palette(ThemeMode::Light).accent
		);
		apply(&ctx, ThemeMode::Dark, ColourTheme::Ember);
		ctx.run_ui(egui::RawInput::default(), |_| {})
			.textures_delta
			.clear();
		assert_eq!(palette(&ctx).panel, EMBER_DARK.panel);
	}
}
