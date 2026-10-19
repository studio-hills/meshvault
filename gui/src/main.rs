//! Desktop viewer for Roblox mesh, union graphics and union physics files.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod download;
mod export;
mod geometry;
mod gpu;
mod import_ui;
mod maps;
mod settings_ui;
mod texture;
mod viewport;
mod workspace;
mod workspace_ui;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use config::Config;
use download::{Downloader, Event};
use geometry::{GroupKind, Loaded};
use import_ui::ImportDialog;
use settings_ui::SettingsDialog;
use gpu::{GpuRenderer, GpuScene};
use maps::Slot;
use texture::Material;
use viewport::{Camera, Shading, ViewSettings};
use workspace::{AssetKind, AssetState, Workspace};

fn main() -> eframe::Result {
	// rbx_mesh_gui [workspace folder] [mesh, image or export .json ...]
	let initial: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_title("rbx_mesh viewer")
			.with_inner_size([1100.0, 720.0])
			.with_drag_and_drop(true),
		// the 3D view needs a depth buffer; MSAA smooths its edges
		depth_buffer: 32,
		multisampling: gpu::MSAA_SAMPLES as u16,
		..Default::default()
	};
	eframe::run_native(
		"rbx_mesh viewer",
		options,
		Box::new(move |cc| {
			let renderer = cc.wgpu_render_state.as_ref().map(GpuRenderer::new);
			let mut app = App::new(cc.egui_ctx.clone(), renderer);
			for path in initial {
				app.open(&path);
			}
			Ok(Box::new(app))
		}),
	)
}

struct Document {
	path: PathBuf,
	size: usize,
	loaded: Loaded,
	camera: Camera,
	/// For LOD files: the single LOD shown. Unused otherwise.
	lod: usize,
	/// For hull files: which hulls are visible.
	hull_visible: Vec<bool>,
	material: Material,
	/// The mesh uploaded to the GPU, created on first draw.
	gpu: Option<GpuScene>,
}

impl Document {
	fn visible_groups(&self) -> Vec<usize> {
		let groups = &self.loaded.geometry.groups;
		match self.loaded.geometry.group_kind {
			GroupKind::Lod => vec![self.lod.min(groups.len().saturating_sub(1))],
			GroupKind::Hull => (0..groups.len()).filter(|&i| self.hull_visible[i]).collect(),
			GroupKind::Whole => (0..groups.len()).collect(),
		}
	}
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SideTab {
	Workspace,
	Details,
}

struct NewWorkspaceDialog {
	name: String,
	parent: PathBuf,
	/// Export to import once the workspace exists.
	then_import: Option<PathBuf>,
}

struct App {
	ctx: egui::Context,
	config: Config,
	doc: Option<Document>,
	error: Option<String>,
	status: Option<String>,
	shading: Shading,
	cull_backfaces: bool,
	show_axes: bool,
	show_maps: bool,
	/// Shown instead of the document's maps when they are switched off.
	no_material: Material,
	/// `None` when the GPU renderer is unavailable; drawing falls back to the CPU.
	gpu: Option<Arc<GpuRenderer>>,
	workspace: Option<Workspace>,
	browser: workspace_ui::Browser,
	downloader: Downloader,
	import: Option<ImportDialog>,
	settings: Option<SettingsDialog>,
	new_workspace: Option<NewWorkspaceDialog>,
	side_tab: SideTab,
	/// Stem of the workspace item shown in the viewer.
	open_stem: Option<String>,
	last_save: Instant,
}

impl Drop for App {
	fn drop(&mut self) {
		self.save_workspace();
	}
}

impl App {
	fn new(ctx: egui::Context, gpu: Option<Arc<GpuRenderer>>) -> Self {
		let config = Config::load();
		let mut app = Self {
			ctx,
			config,
			doc: None,
			error: None,
			status: None,
			shading: Shading::SolidWireframe,
			cull_backfaces: false,
			show_axes: true,
			show_maps: true,
			no_material: Material::default(),
			gpu,
			workspace: None,
			browser: workspace_ui::Browser::default(),
			downloader: Downloader::default(),
			import: None,
			settings: None,
			new_workspace: None,
			side_tab: SideTab::Details,
			open_stem: None,
			last_save: Instant::now(),
		};
		// reopen the last workspace
		if let Some(root) = app.config.recent_workspaces.first().cloned()
			&& root.join(workspace::MANIFEST).exists()
		{
			app.open_workspace(&root);
		}
		app
	}

	fn open(&mut self, path: &Path) {
		if path.is_dir() {
			self.open_workspace(path);
			return;
		}
		match std::fs::read(path) {
			Ok(bytes) => self.open_bytes(path.to_path_buf(), &bytes),
			Err(e) => self.error = Some(format!("Could not read {}: {e}", path.display())),
		}
	}

	fn open_bytes(&mut self, path: PathBuf, bytes: &[u8]) {
		self.status = None;
		if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
			self.start_import(path, bytes);
			return;
		}
		// an image dropped or opened while a mesh is shown becomes one of its
		// maps, chosen by name: Name.normal.png is a normal map, and so on
		if texture::is_image(&path) {
			if self.doc.is_some() {
				self.set_map(slot_for_image(&path), &path, bytes);
			} else {
				self.error = Some(format!(
					"{} is an image. Open a mesh first, then load the image as its texture.",
					path.display()
				));
			}
			return;
		}
		if self.load_mesh(path, bytes) {
			self.open_stem = None;
			let found = self
				.doc
				.as_ref()
				.filter(|d| d.loaded.geometry.uvs.is_some())
				.map(|d| texture::find_siblings(&d.path))
				.unwrap_or_default();
			for (slot, image) in &found {
				self.load_map(*slot, image);
			}
			if !found.is_empty() {
				let names: Vec<_> = found.values().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy()).collect();
				self.status = Some(format!("Auto-loaded {}", names.join(", ")));
			}
		}
	}

	/// Decodes a mesh into the viewer. Returns false (and shows the error) on failure.
	fn load_mesh(&mut self, path: PathBuf, bytes: &[u8]) -> bool {
		match geometry::load(bytes) {
			Ok(loaded) => {
				let camera = Camera::framing(&loaded.geometry);
				let hull_visible = vec![true; loaded.geometry.groups.len()];
				self.doc = Some(Document {
					path,
					size: bytes.len(),
					loaded,
					camera,
					lod: 0,
					hull_visible,
					material: Material::default(),
					gpu: None,
				});
				self.error = None;
				self.side_tab = SideTab::Details;
				true
			}
			Err(e) => {
				self.error = Some(format!("{}\n\n{e}", path.display()));
				false
			}
		}
	}

	// ------------------------------------------------------------ workspaces

	fn open_workspace(&mut self, root: &Path) {
		match Workspace::open(root) {
			Ok(ws) => {
				self.close_workspace();
				self.status = Some(format!("Opened workspace {}", ws.name()));
				self.workspace = Some(ws);
				self.side_tab = SideTab::Workspace;
				self.config.add_recent(root.to_path_buf());
			}
			Err(e) => self.error = Some(e),
		}
	}

	fn close_workspace(&mut self) {
		if self.downloader.is_running() {
			self.downloader.cancel();
		}
		self.save_workspace();
		self.workspace = None;
		self.import = None;
		self.open_stem = None;
		self.side_tab = SideTab::Details;
	}

	fn create_workspace(&mut self, name: &str, parent: &Path) -> Result<(), String> {
		let name = name.trim();
		if name.is_empty() {
			return Err("Enter a workspace name.".into());
		}
		let root = parent.join(workspace::sanitize(name));
		let ws = Workspace::create(&root, name)?;
		self.close_workspace();
		self.workspace = Some(ws);
		self.side_tab = SideTab::Workspace;
		self.config.add_recent(root);
		self.status = Some(format!("Created workspace {name}"));
		Ok(())
	}

	fn pick_open_workspace(&mut self) {
		if let Some(dir) = rfd::FileDialog::new()
			.set_title("Open workspace folder")
			.set_directory(Config::default_workspaces_dir())
			.pick_folder()
		{
			self.open_workspace(&dir);
		}
	}

	fn show_new_workspace(&mut self, then_import: Option<PathBuf>) {
		self.new_workspace = Some(NewWorkspaceDialog {
			name: String::new(),
			parent: Config::default_workspaces_dir(),
			then_import,
		});
	}

	fn save_workspace(&mut self) {
		if let Some(ws) = &mut self.workspace
			&& let Err(e) = ws.save_if_dirty()
		{
			self.error = Some(e);
		}
		self.last_save = Instant::now();
	}

	fn pick_import(&mut self) {
		if let Some(path) = rfd::FileDialog::new()
			.set_title("Import export JSON")
			.add_filter("Export JSON", &["json"])
			.pick_file()
		{
			match std::fs::read(&path) {
				Ok(bytes) => self.start_import(path, &bytes),
				Err(e) => self.error = Some(format!("Could not read {}: {e}", path.display())),
			}
		}
	}

	fn start_import(&mut self, path: PathBuf, bytes: &[u8]) {
		let Some(ws) = &self.workspace else {
			// make a workspace first, then come back to this export
			self.show_new_workspace(Some(path));
			self.status = Some("Create a workspace to import into.".into());
			return;
		};
		let json = String::from_utf8_lossy(bytes);
		match export::parse(&json) {
			Ok(export) => self.import = Some(ImportDialog::new(path, export, ws)),
			Err(e) => self.error = Some(format!("{}\n\n{e}", path.display())),
		}
	}

	fn finish_import(&mut self, outcome: import_ui::Outcome) {
		let import_ui::Outcome::Import {
			entries,
			download_now,
		} = outcome
		else {
			return;
		};
		let Some(ws) = &mut self.workspace else { return };
		let count = entries.len();
		let added = ws.add(entries);
		self.status = Some(format!(
			"Imported {count} models ({added} new, {} already in the workspace)",
			count - added
		));
		self.side_tab = SideTab::Workspace;
		self.save_workspace();
		if download_now {
			self.start_downloads();
		}
	}

	fn start_downloads(&mut self) {
		let Some(ws) = &self.workspace else { return };
		let jobs = ws.pending_jobs();
		if jobs.is_empty() {
			self.status = Some("Nothing to download.".into());
			return;
		}
		let Some(client) = settings_ui::client_from_config(&self.config) else {
			let mut dialog = SettingsDialog::new(&self.config, jobs.first().map(|j| j.asset_id));
			dialog.notice = Some(
				"Add a Roblox credential, then use Workspace → Download pending.".into(),
			);
			self.settings = Some(dialog);
			return;
		};
		let root = ws.root.clone();
		self.downloader
			.start(&self.ctx, root, client, jobs, self.config.concurrency);
		self.error = None;
	}

	fn poll_downloads(&mut self) {
		let events = self.downloader.poll();
		let had_events = !events.is_empty();
		for event in events {
			match event {
				Event::Finished { job, result } => {
					let Some(ws) = &mut self.workspace else { continue };
					match result {
						Ok(written) => {
							for w in written {
								ws.set_state(&w.stem, w.kind, AssetState::Done { path: w.path });
							}
						}
						Err(error) => {
							for (stem, kind) in &job.targets {
								ws.set_state(stem, *kind, AssetState::Failed { error: error.clone() });
							}
						}
					}
				}
				Event::AuthFailed(message) => {
					self.error = Some(format!("Downloads stopped: {message}"));
					let mut dialog = SettingsDialog::new(&self.config, None);
					dialog.notice = Some(message);
					self.settings = Some(dialog);
				}
			}
		}
		// save progress regularly, and once everything has finished
		let finished = had_events && !self.downloader.is_running();
		if finished || (had_events && self.last_save.elapsed() > Duration::from_secs(2)) {
			self.save_workspace();
		}
		if finished {
			let d = &self.downloader;
			self.status = Some(format!(
				"Downloads finished: {} succeeded, {} failed",
				d.completed - d.failed,
				d.failed
			));
		}
	}

	fn open_item(&mut self, index: usize) {
		let Some(ws) = &self.workspace else { return };
		let Some(item) = ws.items().get(index) else { return };
		let mesh_path = match &item.mesh {
			AssetState::Done { path } => ws.root.join(path),
			AssetState::Pending => {
				self.status = Some(format!("{} has not been downloaded yet.", item.name));
				return;
			}
			AssetState::Failed { error } => {
				self.error = Some(format!("{} failed to download:\n{error}", item.name));
				return;
			}
		};
		// each slot uses the first downloaded map that feeds it (ColorMap over TextureID)
		let maps: Vec<(Slot, PathBuf)> = Slot::ALL
			.iter()
			.filter_map(|&slot| {
				let path = slot.sources().iter().find_map(|&k| item.done_path(AssetKind::Map(k)))?;
				Some((slot, ws.root.join(path)))
			})
			.collect();
		let stem = item.stem.clone();
		let name = item.name.clone();
		match std::fs::read(&mesh_path) {
			Ok(bytes) => {
				if self.load_mesh(mesh_path, &bytes) {
					self.open_stem = Some(stem);
					self.side_tab = SideTab::Workspace;
					for (slot, path) in maps {
						self.load_map(slot, &path);
					}
					self.status = Some(format!("Opened {name}"));
				}
			}
			Err(e) => self.error = Some(format!("Could not read {}: {e}", mesh_path.display())),
		}
	}

	fn handle_browser(&mut self, actions: Vec<workspace_ui::Action>) {
		for action in actions {
			match action {
				workspace_ui::Action::Open(i) => self.open_item(i),
				workspace_ui::Action::Retry(items) => {
					if let Some(ws) = &mut self.workspace {
						ws.retry(items);
					}
					self.start_downloads();
				}
				workspace_ui::Action::Remove(items) => {
					if let Some(ws) = &mut self.workspace {
						let removes_open = self.open_stem.as_ref().is_some_and(|open| {
							items.iter().any(|&i| ws.items().get(i).is_some_and(|it| &it.stem == open))
						});
						if removes_open {
							self.open_stem = None;
						}
						ws.remove(&items);
					}
					self.save_workspace();
				}
				workspace_ui::Action::Reveal(path) => workspace_ui::reveal(&path),
			}
		}
	}

	fn workspace_menu(&mut self, ui: &mut egui::Ui) {
		if ui.button("New workspace…").clicked() {
			ui.close();
			self.show_new_workspace(None);
		}
		if ui.button("Open workspace…").clicked() {
			ui.close();
			self.pick_open_workspace();
		}
		let recent: Vec<PathBuf> = self
			.config
			.recent_workspaces
			.iter()
			.filter(|p| p.join(workspace::MANIFEST).exists())
			.cloned()
			.collect();
		ui.add_enabled_ui(!recent.is_empty(), |ui| {
			ui.menu_button("Open recent", |ui| {
				for root in recent {
					if ui.button(root.display().to_string()).clicked() {
						ui.close();
						self.open_workspace(&root);
					}
				}
			});
		});
		let has_ws = self.workspace.is_some();
		if ui.add_enabled(has_ws, egui::Button::new("Close workspace")).clicked() {
			ui.close();
			self.close_workspace();
		}
		ui.separator();
		if ui
			.add_enabled(has_ws, egui::Button::new("Import export JSON…"))
			.clicked()
		{
			ui.close();
			self.pick_import();
		}
		let (pending, failed) = self
			.workspace
			.as_ref()
			.map(|ws| {
				let items = ws.items();
				(
					items.iter().filter(|i| i.has_pending()).count(),
					items.iter().filter(|i| i.has_failure()).count(),
				)
			})
			.unwrap_or((0, 0));
		if ui
			.add_enabled(pending > 0, egui::Button::new(format!("Download pending ({pending})")))
			.clicked()
		{
			ui.close();
			self.start_downloads();
		}
		if ui
			.add_enabled(failed > 0, egui::Button::new(format!("Retry failed ({failed})")))
			.clicked()
		{
			ui.close();
			if let Some(ws) = &mut self.workspace {
				ws.retry(0..ws.items().len());
			}
			self.start_downloads();
		}
		if ui
			.add_enabled(self.downloader.is_running(), egui::Button::new("Cancel downloads"))
			.clicked()
		{
			ui.close();
			self.downloader.cancel();
		}
		ui.separator();
		if ui
			.add_enabled(has_ws, egui::Button::new("Show folder in file browser"))
			.clicked()
		{
			ui.close();
			if let Some(ws) = &self.workspace {
				workspace_ui::reveal(&ws.root);
			}
		}
		if ui.button("Roblox access…").clicked() {
			ui.close();
			let test_id = self
				.workspace
				.as_ref()
				.and_then(|ws| ws.items().first().map(|i| i.mesh_id));
			self.settings = Some(SettingsDialog::new(&self.config, test_id));
		}
	}

	fn dialogs(&mut self, ctx: &egui::Context) {
		if let Some(dialog) = &mut self.import
			&& let Some(outcome) = dialog.show(ctx)
		{
			self.import = None;
			self.finish_import(outcome);
		}
		if let Some(dialog) = &mut self.settings
			&& !dialog.show(ctx, &mut self.config)
		{
			self.settings = None;
		}
		self.new_workspace_dialog(ctx);
	}

	fn new_workspace_dialog(&mut self, ctx: &egui::Context) {
		let Some(dialog) = &mut self.new_workspace else { return };
		let mut open = true;
		let mut create = false;
		let mut cancel = false;
		egui::Window::new("New workspace")
			.open(&mut open)
			.collapsible(false)
			.resizable(false)
			.show(ctx, |ui| {
				if let Some(export) = &dialog.then_import {
					ui.label(format!(
						"Importing {} needs a workspace to download into.",
						export.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
					));
					ui.add_space(4.0);
				}
				egui::Grid::new("new_ws").num_columns(2).show(ui, |ui| {
					ui.label("Name");
					let response = ui.add(
						egui::TextEdit::singleline(&mut dialog.name)
							.hint_text("e.g. My game assets")
							.desired_width(280.0),
					);
					response.request_focus();
					if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
						create = true;
					}
					ui.end_row();
					ui.label("Location");
					ui.horizontal(|ui| {
						ui.label(dialog.parent.display().to_string());
						if ui.small_button("Change…").clicked()
							&& let Some(dir) = rfd::FileDialog::new()
								.set_directory(&dialog.parent)
								.pick_folder()
						{
							dialog.parent = dir;
						}
					});
					ui.end_row();
				});
				if !dialog.name.trim().is_empty() {
					ui.label(
						egui::RichText::new(format!(
							"Will create {}",
							dialog.parent.join(workspace::sanitize(dialog.name.trim())).display()
						))
						.small()
						.weak(),
					);
				}
				ui.horizontal(|ui| {
					create |= ui.button("Create").clicked();
					cancel = ui.button("Cancel").clicked();
				});
			});
		if !open || cancel {
			self.new_workspace = None;
			return;
		}
		if create {
			let dialog = self.new_workspace.take().expect("dialog is open");
			match self.create_workspace(&dialog.name, &dialog.parent) {
				Ok(()) => {
					if let Some(export) = dialog.then_import {
						match std::fs::read(&export) {
							Ok(bytes) => self.start_import(export, &bytes),
							Err(e) => {
								self.error = Some(format!("Could not read {}: {e}", export.display()))
							}
						}
					}
				}
				Err(e) => {
					self.error = Some(e);
					self.new_workspace = Some(dialog);
				}
			}
		}
	}

	fn load_map(&mut self, slot: Slot, path: &Path) {
		match std::fs::read(path) {
			Ok(bytes) => self.set_map(slot, path, &bytes),
			Err(e) => self.error = Some(format!("Could not read {}: {e}", path.display())),
		}
	}

	fn set_map(&mut self, slot: Slot, path: &Path, bytes: &[u8]) {
		let Some(doc) = &mut self.doc else { return };
		match texture::load(&self.ctx, path, bytes) {
			Ok(texture) => {
				doc.material.set(slot, texture);
				self.show_maps = true;
				self.error = None;
			}
			Err(e) => self.error = Some(e),
		}
	}

	fn pick_map(&mut self, slot: Slot) {
		let mut dialog = rfd::FileDialog::new()
			.set_title(format!("Load {} map", slot.label().to_lowercase()))
			.add_filter("Images", &texture::IMAGE_EXTENSIONS);
		if let Some(dir) = self.doc.as_ref().and_then(|d| d.path.parent()) {
			dialog = dialog.set_directory(dir);
		}
		if let Some(path) = dialog.pick_file() {
			self.load_map(slot, &path);
		}
	}

	fn pick_file(&mut self) {
		if let Some(path) = rfd::FileDialog::new()
			.set_title("Open Roblox mesh")
			.add_filter("Roblox mesh / union data", &["mesh", "meshdata", "meshdata5", "data", "bin"])
			.add_filter("All files", &["*"])
			.pick_file()
		{
			self.open(&path);
		}
	}

	fn export_obj(&mut self) {
		let Some(doc) = &self.doc else { return };
		let stem = doc
			.path
			.file_stem()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_else(|| "mesh".into());
		let Some(out) = rfd::FileDialog::new()
			.set_title("Export OBJ")
			.set_file_name(format!("{stem}.obj"))
			.add_filter("Wavefront OBJ", &["obj"])
			.save_file()
		else {
			return;
		};
		let obj = geometry::to_obj(&doc.loaded.geometry, &doc.visible_groups());
		self.status = Some(match std::fs::write(&out, obj) {
			Ok(()) => format!("Exported {}", out.display()),
			Err(e) => format!("Export failed: {e}"),
		});
	}

	fn menu_bar(&mut self, ui: &mut egui::Ui) {
		egui::MenuBar::new().ui(ui, |ui| {
			ui.menu_button("File", |ui| {
				if ui.button("Open…").clicked() {
					ui.close();
					self.pick_file();
				}
				let can_export = self
					.doc
					.as_ref()
					.is_some_and(|d| !d.loaded.geometry.triangles.is_empty());
				if ui
					.add_enabled(can_export, egui::Button::new("Export visible as OBJ…"))
					.clicked()
				{
					ui.close();
					self.export_obj();
				}
				ui.separator();
				let has_doc = self.doc.is_some();
				ui.add_enabled_ui(has_doc, |ui| {
					ui.menu_button("Load map", |ui| {
						for slot in Slot::ALL {
							if ui.button(format!("{}…", slot.label())).clicked() {
								ui.close();
								self.pick_map(slot);
							}
						}
					});
				});
				let has_maps = self.doc.as_ref().is_some_and(|d| !d.material.maps.is_empty());
				if ui
					.add_enabled(has_maps, egui::Button::new("Remove all maps"))
					.clicked()
				{
					ui.close();
					if let Some(doc) = &mut self.doc {
						for slot in Slot::ALL {
							doc.material.remove(slot);
						}
					}
				}
				ui.separator();
				if ui.button("Quit").clicked() {
					ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
				}
			});
			ui.menu_button("Workspace", |ui| self.workspace_menu(ui));
			ui.menu_button("View", |ui| {
				ui.radio_value(&mut self.shading, Shading::Solid, "Solid");
				ui.radio_value(&mut self.shading, Shading::SolidWireframe, "Solid + wireframe");
				ui.radio_value(&mut self.shading, Shading::Wireframe, "Wireframe");
				ui.separator();
				ui.checkbox(&mut self.show_maps, "Show maps");
				ui.checkbox(&mut self.cull_backfaces, "Cull back faces");
				ui.checkbox(&mut self.show_axes, "Show axes");
				ui.separator();
				if ui.button("Reset camera").clicked() {
					if let Some(doc) = &mut self.doc {
						doc.camera = Camera::framing(&doc.loaded.geometry);
					}
					ui.close();
				}
			});
		});
	}

	fn side_panel(&mut self, ui: &mut egui::Ui) {
		let Some(doc) = &mut self.doc else {
			ui.heading("No file loaded");
			ui.label("Use File → Open, or drop a file onto the window.");
			ui.add_space(8.0);
			ui.label("Supported formats:");
			for line in [
				"Mesh v1.00 – v7.00 (v7 Draco compressed)",
				"Union graphics (CSGMDL v2, v4, v5)",
				"Union physics (CSGPHS v3, v5–v8, BLOCK)",
				"CSGK references",
			] {
				ui.label(format!("• {line}"));
			}
			return;
		};

		let name = doc
			.path
			.file_name()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_default();
		ui.heading(&name);
		ui.label(egui::RichText::new(&doc.loaded.format).strong());
		ui.add_space(4.0);

		egui::Grid::new("details").num_columns(2).striped(true).show(ui, |ui| {
			ui.label("File size");
			ui.label(format_bytes(doc.size));
			ui.end_row();
			for (k, v) in &doc.loaded.details {
				ui.label(k);
				ui.add(egui::Label::new(v).wrap());
				ui.end_row();
			}
			if let Some((lo, hi)) = doc.loaded.geometry.bounds() {
				ui.label("Size");
				ui.label(format!(
					"{:.3} × {:.3} × {:.3}",
					hi[0] - lo[0],
					hi[1] - lo[1],
					hi[2] - lo[2]
				));
				ui.end_row();
			}
		});

		if let Some(note) = &doc.loaded.note {
			ui.add_space(6.0);
			ui.label(egui::RichText::new(note).italics());
		}

		if !doc.loaded.geometry.triangles.is_empty() {
			ui.separator();
			ui.horizontal(|ui| {
				ui.label(egui::RichText::new("Material").strong());
				ui.checkbox(&mut self.show_maps, "Show maps");
			});
			if doc.loaded.geometry.uvs.is_none() {
				ui.label(egui::RichText::new("This format has no texture coordinates, so maps cannot be applied.").italics());
			} else {
				let mut pick = None;
				let mut toggle = None;
				let mut remove = None;
				for slot in Slot::ALL {
					ui.horizontal(|ui| {
						let thumb = egui::vec2(44.0, 44.0);
						match doc.material.maps.get(&slot) {
							Some(texture) => {
								// the uploaded image has white rows below it; crop them off
								let uv = egui::Rect::from_min_max(
									egui::Pos2::ZERO,
									egui::Pos2::new(1.0, texture.info.v_scale),
								);
								let image = |size| {
									egui::Image::new((texture.handle.id(), size))
										.uv(uv)
										.maintain_aspect_ratio(false)
								};
								ui.add(image(thumb)).on_hover_ui(|ui| {
									ui.add(image(egui::vec2(256.0, 256.0)));
								});
								ui.vertical(|ui| {
									ui.horizontal(|ui| {
										let mut on = !doc.material.disabled.contains(&slot);
										if ui.checkbox(&mut on, egui::RichText::new(slot.label()).strong()).changed() {
											toggle = Some((slot, on));
										}
										if ui.small_button("…").on_hover_text("Replace").clicked() {
											pick = Some(slot);
										}
										if ui.small_button("✕").on_hover_text("Remove").clicked() {
											remove = Some(slot);
										}
									});
									let name = texture
										.path
										.file_name()
										.map(|s| s.to_string_lossy().into_owned())
										.unwrap_or_default();
									ui.add(
										egui::Label::new(
											egui::RichText::new(format!("{}×{} · {name}", texture.size[0], texture.size[1]))
												.small()
												.weak(),
										)
										.truncate(),
									)
									.on_hover_text(texture.path.display().to_string());
								});
							}
							None => {
								let (rect, _) = ui.allocate_exact_size(thumb, egui::Sense::hover());
								ui.painter().rect_stroke(
									rect,
									3.0,
									egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
									egui::StrokeKind::Inside,
								);
								ui.vertical(|ui| {
									ui.label(slot.label());
									if ui.small_button("Load…").clicked() {
										pick = Some(slot);
									}
								});
							}
						}
					});
				}
				let pbr = [Slot::Normal, Slot::Roughness, Slot::Metalness];
				if self.gpu.is_none() && pbr.iter().any(|s| doc.material.maps.contains_key(s)) {
					ui.label(
						egui::RichText::new("GPU rendering is unavailable, so only the base color is shown.")
							.small()
							.color(ui.visuals().warn_fg_color),
					);
				}
				if let Some((slot, on)) = toggle {
					doc.material.toggle(slot, on);
				}
				if let Some(slot) = remove {
					doc.material.remove(slot);
				}
				if let Some(slot) = pick {
					self.pick_map(slot);
					return;
				}
			}
		}

		let Some(doc) = &mut self.doc else { return };
		let groups = &doc.loaded.geometry.groups;
		match doc.loaded.geometry.group_kind {
			GroupKind::Lod if groups.len() > 1 => {
				ui.separator();
				ui.label("Level of detail");
				for (i, range) in groups.iter().enumerate() {
					ui.radio_value(&mut doc.lod, i, format!("LOD {i}  ({} faces)", range.len()));
				}
			}
			GroupKind::Hull if !groups.is_empty() => {
				ui.separator();
				ui.horizontal(|ui| {
					ui.label(format!("Convex hulls ({})", groups.len()));
					if ui.small_button("All").clicked() {
						doc.hull_visible.fill(true);
					}
					if ui.small_button("None").clicked() {
						doc.hull_visible.fill(false);
					}
				});
				egui::ScrollArea::vertical().show(ui, |ui| {
					for (i, range) in groups.iter().enumerate() {
						ui.checkbox(
							&mut doc.hull_visible[i],
							format!("Hull {i}  ({} faces)", range.len()),
						);
					}
				});
			}
			_ => {}
		}
	}
}

/// Picks the material slot for a dropped image from its name, following the
/// workspace naming: `Name.normal.png`, `Name.roughness.png`, `Name.metalness.png`.
fn slot_for_image(path: &Path) -> Slot {
	let stem = path
		.file_stem()
		.map(|s| s.to_string_lossy().to_lowercase())
		.unwrap_or_default();
	for (suffix, slot) in [
		(".normal", Slot::Normal),
		(".roughness", Slot::Roughness),
		(".metalness", Slot::Metalness),
	] {
		if stem.ends_with(suffix) {
			return slot;
		}
	}
	Slot::Base
}

fn format_bytes(n: usize) -> String {
	match n {
		0..1024 => format!("{n} B"),
		1024..1_048_576 => format!("{:.1} KiB", n as f64 / 1024.0),
		_ => format!("{:.2} MiB", n as f64 / 1_048_576.0),
	}
}

impl eframe::App for App {
	fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
		self.poll_downloads();
		self.dialogs(ui.ctx());

		// drag and drop
		let dropped = ui.input(|i| i.raw.dropped_files.clone());
		if let Some(file) = dropped.into_iter().next() {
			let path = file.path().to_path_buf();
			match file.bytes() {
				Ok(bytes) => self.open_bytes(path, &bytes),
				Err(e) => self.error = Some(format!("Could not read {}: {e}", path.display())),
			}
		}
		if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
			self.pick_file();
		}

		egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));

		egui::Panel::bottom("status").show(ui, |ui| {
			ui.horizontal(|ui| {
				if self.downloader.is_running() {
					let d = &self.downloader;
					let progress = d.completed as f32 / d.total.max(1) as f32;
					ui.add(
						egui::ProgressBar::new(progress)
							.desired_width(220.0)
							.text(format!("Downloading {}/{}", d.completed, d.total)),
					);
					if d.failed > 0 {
						ui.label(
							egui::RichText::new(format!("{} failed", d.failed))
								.color(ui.visuals().error_fg_color),
						);
					}
					if ui.small_button("Cancel").clicked() {
						self.downloader.cancel();
					}
					ui.separator();
				}
				match (&self.doc, &self.status) {
					(_, Some(status)) => ui.label(status),
					(Some(doc), None) => ui.label(doc.path.display().to_string()),
					(None, None) => ui.label("Ready"),
				};
			});
		});

		egui::Panel::left("info")
			.default_size(300.0)
			.show(ui, |ui| {
				if self.workspace.is_some() {
					ui.horizontal(|ui| {
						ui.selectable_value(&mut self.side_tab, SideTab::Workspace, "Workspace");
						ui.selectable_value(&mut self.side_tab, SideTab::Details, "Details");
					});
					ui.separator();
				}
				match (&self.workspace, self.side_tab) {
					(Some(ws), SideTab::Workspace) => {
						let actions = self.browser.show(ui, ws, self.open_stem.as_deref());
						self.handle_browser(actions);
					}
					_ => {
						egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui));
					}
				}
			});

		egui::CentralPanel::default()
			.frame(egui::Frame::NONE)
			.show(ui, |ui| {
				if let Some(error) = &self.error {
					let mut close = false;
					egui::Frame::NONE
						.fill(egui::Color32::from_rgb(70, 30, 30))
						.inner_margin(8.0)
						.show(ui, |ui| {
							ui.horizontal(|ui| {
								ui.label(egui::RichText::new("Failed to open file").strong());
								close = ui.small_button("✕").clicked();
							});
							egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
								ui.label(egui::RichText::new(error).monospace());
							});
						});
					if close {
						self.error = None;
					}
				}
				match &mut self.doc {
					Some(doc) => {
						let groups = doc.visible_groups();
						let settings = ViewSettings {
							shading: self.shading,
							cull_backfaces: self.cull_backfaces,
							show_axes: self.show_axes,
							groups: &groups,
							colour_groups: doc.loaded.geometry.group_kind == GroupKind::Hull,
							material: if self.show_maps { &doc.material } else { &self.no_material },
						};
						let gpu = self.gpu.as_deref().map(|renderer| viewport::Gpu {
							renderer,
							scene: &mut doc.gpu,
						});
						viewport::show(ui, &doc.loaded.geometry, &mut doc.camera, &settings, gpu);
					}
					None => {
						let hint = match &self.workspace {
							Some(ws) if ws.items().is_empty() => {
								"Drop an export .json here to import models\nor use Workspace → Import export JSON…"
							}
							Some(_) => "Select a downloaded model in the workspace\n\nor drop a mesh, image or export .json here",
							None => {
								"Drop a Roblox mesh file here\nor press Ctrl+O\n\nDrop images afterwards to use them as maps\n(Name.normal.png, Name.roughness.png, … go to their slot)\n\nTo bulk download, use Workspace → New workspace…"
							}
						};
						ui.centered_and_justified(|ui| {
							ui.label(egui::RichText::new(hint).size(20.0).color(egui::Color32::GRAY));
						});
					}
				}
			});
	}
}
