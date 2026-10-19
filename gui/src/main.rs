//! Desktop viewer for Roblox mesh, union graphics and union physics files.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod download;
mod export;
mod geometry;
mod gpu;
mod import_ui;
mod maps;
mod settings_ui;
mod texture;
mod ui;
mod viewport;
mod workspace;
mod workspace_ui;

use std::path::PathBuf;

use eframe::egui;

use app::App;
use gpu::GpuRenderer;

fn main() -> eframe::Result {
	// rbx_mesh_gui [workspace folder] [mesh, image or export .json ...]
	let initial: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_title("rbx_mesh")
			.with_inner_size([1280.0, 800.0])
			.with_min_inner_size([760.0, 480.0])
			.with_drag_and_drop(true),
		// the 3D view needs a depth buffer; MSAA smooths its edges
		depth_buffer: 32,
		multisampling: gpu::MSAA_SAMPLES as u16,
		..Default::default()
	};
	eframe::run_native(
		"rbx_mesh",
		options,
		Box::new(move |cc| {
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
