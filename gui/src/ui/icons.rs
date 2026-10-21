//! Phosphor icon codepoints (see assets/PHOSPHOR-LICENSE.txt). Only the
//! icons this app uses are listed; add more from https://phosphoricons.com.

pub const ARROWS_CLOCKWISE: &str = "\u{E094}";
pub const BOUNDING_BOX: &str = "\u{E6CE}";
pub const CARET_DOWN: &str = "\u{E136}";
pub const CARET_RIGHT: &str = "\u{E13A}";
pub const CHECK: &str = "\u{E182}";
pub const CHECK_CIRCLE: &str = "\u{E184}";
pub const CIRCLE: &str = "\u{E18A}";
pub const CLOUD_ARROW_DOWN: &str = "\u{E1AC}";
pub const CORNERS_OUT: &str = "\u{E1D0}";
pub const CROSSHAIR: &str = "\u{E1D6}";
pub const COPY: &str = "\u{E1CA}";
pub const CUBE: &str = "\u{E1DA}";
pub const DOTS_THREE_VERTICAL: &str = "\u{E208}";
pub const EXPORT: &str = "\u{EAF0}";
pub const EYE: &str = "\u{E220}";
pub const EYE_SLASH: &str = "\u{E224}";
pub const FILE: &str = "\u{E230}";
pub const FILE_ARROW_DOWN: &str = "\u{E232}";
pub const FUNNEL: &str = "\u{E266}";
pub const LIST: &str = "\u{E2F0}";
pub const SQUARES_FOUR: &str = "\u{E464}";
pub const FOLDER: &str = "\u{E24A}";
pub const FOLDER_OPEN: &str = "\u{E256}";
pub const FOLDER_PLUS: &str = "\u{E258}";
pub const FOLDER_SIMPLE: &str = "\u{E25A}";
pub const GRID_FOUR: &str = "\u{E296}";
pub const IMAGE: &str = "\u{E2CA}";
pub const INFO: &str = "\u{E2CE}";
pub const KEY: &str = "\u{E2D6}";
pub const MOON: &str = "\u{E330}";
pub const PLUS: &str = "\u{E3D4}";
pub const POLYGON: &str = "\u{E6D0}";
pub const SIDEBAR_SIMPLE: &str = "\u{EC24}";
pub const STOP: &str = "\u{E46C}";
pub const SUN: &str = "\u{E472}";
pub const TRASH: &str = "\u{E4A6}";
pub const TRAY_ARROW_DOWN: &str = "\u{E010}";
pub const TREE_STRUCTURE: &str = "\u{E67C}";
pub const WARNING_CIRCLE: &str = "\u{E4E2}";
pub const X: &str = "\u{E4F6}";
pub const MAGNIFYING_GLASS: &str = "\u{E30C}";
pub const ARROWS_IN_LINE_VERTICAL: &str = "\u{E532}";
pub const ARROWS_OUT_LINE_VERTICAL: &str = "\u{E536}";
pub const CROSSHAIR_SIMPLE: &str = "\u{E1D8}";
pub const SIGN_OUT: &str = "\u{E42A}";
pub const PLAY: &str = "\u{E3D0}";
pub const PAUSE: &str = "\u{E39E}";
pub const REPEAT: &str = "\u{E3F6}";
pub const PERSON_SIMPLE_RUN: &str = "\u{E730}";
pub const SKIP_BACK: &str = "\u{E5A4}";
pub const FLAG: &str = "\u{E244}";
pub const FILM_STRIP: &str = "\u{E792}";
pub const WARNING: &str = "\u{E4E0}";
pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{E038}";

/// Blender's logo (assets/blender.svg), drawn once at 64 pixels and kept.
pub fn blender(ctx: &eframe::egui::Context) -> Option<eframe::egui::TextureHandle> {
	use eframe::egui;
	use resvg::{tiny_skia, usvg};
	let id = egui::Id::unique("blender_logo");
	if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
		return Some(texture);
	}
	const SIDE: u32 = 64;
	let svg = include_bytes!("../../assets/blender.svg");
	let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
	let mut pixmap = tiny_skia::Pixmap::new(SIDE, SIDE)?;
	let size = tree.size();
	let scale = SIDE as f32 / size.width().max(size.height());
	// centred in the square
	let dx = (SIDE as f32 - size.width() * scale) / 2.0;
	let dy = (SIDE as f32 - size.height() * scale) / 2.0;
	let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
	resvg::render(&tree, transform, &mut pixmap.as_mut());
	let image = egui::ColorImage::from_rgba_premultiplied([SIDE as usize; 2], pixmap.data());
	let texture = ctx.load_texture("blender_logo", image, egui::TextureOptions::LINEAR);
	ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
	Some(texture)
}
