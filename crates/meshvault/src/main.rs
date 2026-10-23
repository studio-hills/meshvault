//! Meshvault: a desktop viewer for Roblox meshes (including union graphics
//! and union physics files) and animated models, with workspaces for bulk
//! downloading a game's assets, a localhost link that receives exports from
//! Roblox Studio, and OBJ/glTF export.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim;
mod app;
mod config;
mod content_id;
mod export;
mod platform;
mod render;
mod scene;
mod studio;
mod ui;
mod update;
mod workspace;

#[cfg(test)]
mod test_support;

use std::path::PathBuf;

use eframe::egui;

use app::App;
use render::gpu::GpuRenderer;

/// The window title, and the app name given to eframe.
const APP_NAME: &str = "Meshvault";
/// The window's size when it first opens, and the smallest it can be.
const WINDOW_SIZE: [f32; 2] = [1280.0, 800.0];
const MIN_WINDOW_SIZE: [f32; 2] = [760.0, 480.0];

/// The window's icon, in the title bar's place and the taskbar. The
/// executable's own icon is embedded by build.rs.
fn window_icon() -> egui::IconData {
	let png = include_bytes!("../assets/icons/meshvault.png");
	let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
		.expect("the bundled icon is a valid PNG")
		.into_rgba8();
	egui::IconData {
		width: image.width(),
		height: image.height(),
		rgba: image.into_raw(),
	}
}

fn main() -> eframe::Result {
	// meshvault [workspace folder] [mesh, image or export .json ...]
	let initial: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_title(APP_NAME)
			.with_icon(window_icon())
			// Meshvault draws its own title bar (see ui::window)
			.with_decorations(false)
			.with_inner_size(WINDOW_SIZE)
			.with_min_inner_size(MIN_WINDOW_SIZE)
			.with_drag_and_drop(true),
		// the 3D view needs a depth buffer; MSAA smooths its edges
		depth_buffer: 32,
		multisampling: render::gpu::MSAA_SAMPLES as u16,
		..Default::default()
	};
	eframe::run_native(
		APP_NAME,
		options,
		Box::new(move |cc| {
			ui::window::round_corners(cc);
			let renderer = cc.wgpu_render_state.as_ref().map(GpuRenderer::new);
			let adapter = cc
				.wgpu_render_state
				.as_ref()
				.map(|rs| rs.adapter.get_info().name);
			let mut app = App::new(cc.egui_ctx.clone(), renderer, adapter);
			for path in initial {
				app.open(&path);
			}
			Ok(Box::new(app))
		}),
	)
}

#[cfg(test)]
mod tests {
	/// The app panics at startup if its icon can't be read.
	#[test]
	fn window_icon_decodes() {
		let icon = super::window_icon();
		assert_eq!((icon.width, icon.height), (256, 256));
		assert_eq!(icon.rgba.len(), 256 * 256 * 4);
	}
}
