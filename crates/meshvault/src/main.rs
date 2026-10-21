//! Meshvault: a desktop viewer for Roblox meshes (including union graphics
//! and union physics files) and animated models, with workspaces for bulk
//! downloading a game's assets, a localhost link that receives exports from
//! Roblox Studio, and OBJ/glTF export.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim;
mod app;
mod config;
mod export;
mod render;
mod scene;
mod studio;
mod ui;
mod workspace;

#[cfg(test)]
mod test_support;

use std::path::PathBuf;

use eframe::egui;

use app::App;
use render::gpu::GpuRenderer;

fn main() -> eframe::Result {
	// meshvault [workspace folder] [mesh, image or export .json ...]
	let initial: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_title("Meshvault")
			// Meshvault draws its own title bar (see ui::window)
			.with_decorations(false)
			.with_inner_size([1280.0, 800.0])
			.with_min_inner_size([760.0, 480.0])
			.with_drag_and_drop(true),
		// the 3D view needs a depth buffer; MSAA smooths its edges
		depth_buffer: 32,
		multisampling: render::gpu::MSAA_SAMPLES as u16,
		..Default::default()
	};
	eframe::run_native(
		"Meshvault",
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
