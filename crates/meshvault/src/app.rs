//! Application state and the actions the UI triggers. Drawing lives in `ui`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::config::Config;
use crate::export::obj::Format;
use crate::render::gpu::{GpuRenderer, GpuScene};
use crate::render::viewport::{Camera, Shading};
use crate::scene::geometry::{self, GroupKind, Loaded};
use crate::scene::maps::Slot;
use crate::scene::model_view::ModelView;
use crate::scene::texture::{self, Material};
use crate::studio::export_file;
use crate::ui::format::{format_bytes, plural};
use crate::ui::import::{self, ImportDialog};
use crate::ui::settings::{self, SettingsDialog};
use crate::ui::theme;
use crate::ui::{self, Confirm, ConfirmAction, StudioDialog, Toasts};
use crate::workspace::download::{Downloader, Event};
use crate::workspace::view_loader::{item_maps, model_loader};
use crate::workspace::{self, AssetKind, AssetState, Workspace};

pub struct Document {
	pub path: PathBuf,
	pub size: usize,
	pub loaded: Loaded,
	pub camera: Camera,
	/// For LOD files: the single LOD shown. Unused otherwise.
	pub lod: usize,
	/// For hull files: which hulls are visible.
	pub hull_visible: Vec<bool>,
	pub material: Material,
	/// The mesh uploaded to the GPU, created on first draw.
	pub gpu: Option<GpuScene>,
	/// What the colour map's alpha does.
	pub alpha: crate::render::viewport::AlphaMode,
}

impl Document {
	pub fn visible_groups(&self) -> Vec<usize> {
		let groups = &self.loaded.geometry.groups;
		match self.loaded.geometry.group_kind {
			GroupKind::Lod => vec![self.lod.min(groups.len().saturating_sub(1))],
			GroupKind::Hull => (0..groups.len())
				.filter(|&i| self.hull_visible[i])
				.collect(),
			GroupKind::Whole => (0..groups.len()).collect(),
		}
	}

	pub fn name(&self) -> String {
		self.path
			.file_name()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_default()
	}
}

/// An export being read on a background thread.
pub struct Parsing {
	pub path: PathBuf,
	rx: std::sync::mpsc::Receiver<Result<export_file::Export, String>>,
	pub started: Instant,
}

/// An animated model being read on a background thread.
pub struct LoadingModel {
	pub name: String,
	rx: std::sync::mpsc::Receiver<Result<ModelView, String>>,
	pub started: Instant,
}

pub struct NewWorkspaceDialog {
	pub name: String,
	pub parent: PathBuf,
	/// Export to import once the workspace exists.
	pub then_import: Option<PathBuf>,
}

pub struct App {
	pub ctx: egui::Context,
	pub config: Config,
	pub doc: Option<Document>,
	pub toasts: Toasts,
	pub shading: Shading,
	pub cull_backfaces: bool,
	pub show_axes: bool,
	pub show_grid: bool,
	pub show_maps: bool,
	/// Overlays on an animated model: lines between its joints and bones,
	/// and a mark at each joint.
	pub show_skeleton: bool,
	pub show_joints: bool,
	/// Shown instead of the document's maps when they are switched off.
	pub no_material: Material,
	/// `None` when the GPU renderer is unavailable; drawing falls back to the CPU.
	pub gpu: Option<Arc<GpuRenderer>>,
	/// Name of the graphics adapter, for the status bar.
	pub adapter: Option<String>,
	pub workspace: Option<Workspace>,
	pub browser: ui::workspace::Browser,
	pub downloader: Downloader,
	pub import: Option<ImportDialog>,
	/// An export being read on a background thread.
	pub parsing: Option<Parsing>,
	pub settings: Option<SettingsDialog>,
	pub new_workspace: Option<NewWorkspaceDialog>,
	pub show_shortcuts: bool,
	/// A destructive action awaiting confirmation.
	pub confirm: Option<Confirm>,
	pub show_library: bool,
	pub show_inspector: bool,
	/// Stem of the workspace item shown in the viewer.
	pub open_stem: Option<String>,
	/// The animated model shown instead of `doc`.
	pub model: Option<ModelView>,
	pub loading_model: Option<LoadingModel>,
	/// Receives exports sent from Roblox Studio.
	pub studio_link: crate::studio::link::StudioLink,
	pub studio_dialog: Option<StudioDialog>,
	/// Pieces of an export from Studio received so far, out of how many, and when.
	pub studio_progress: Option<(usize, usize, Instant)>,
	/// Work on background threads (exports, saved images), each ending
	/// with a message to show.
	background: Vec<std::sync::mpsc::Receiver<Result<String, String>>>,
	/// Where the 3D view was last drawn, for the shape of saved images.
	pub viewport: egui::Rect,
	last_save: Instant,
}

impl Drop for App {
	fn drop(&mut self) {
		self.save_workspace();
	}
}

impl App {
	pub fn new(ctx: egui::Context, gpu: Option<Arc<GpuRenderer>>, adapter: Option<String>) -> Self {
		let config = Config::load();
		theme::install_fonts(&ctx);
		theme::apply(&ctx, config.theme);
		let studio_link = crate::studio::link::StudioLink::start(&ctx);
		let mut app = Self {
			ctx,
			config,
			doc: None,
			toasts: Toasts::default(),
			shading: Shading::Solid,
			cull_backfaces: false,
			show_axes: true,
			show_grid: true,
			show_maps: true,
			show_skeleton: false,
			show_joints: false,
			no_material: Material::default(),
			gpu,
			adapter,
			workspace: None,
			browser: ui::workspace::Browser::default(),
			downloader: Downloader::default(),
			import: None,
			parsing: None,
			settings: None,
			new_workspace: None,
			show_shortcuts: false,
			confirm: None,
			show_library: true,
			show_inspector: true,
			open_stem: None,
			model: None,
			loading_model: None,
			studio_link,
			studio_dialog: None,
			studio_progress: None,
			background: Vec::new(),
			viewport: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(16.0, 9.0)),
			last_save: Instant::now(),
		};
		if let crate::studio::link::Status::Unavailable(why) = &app.studio_link.status {
			app.toasts.warning(crate::ui::unavailable_text(why));
		}
		// reopen the last workspace
		if let Some(root) = app.config.recent_workspaces.first().cloned()
			&& root.join(workspace::MANIFEST).exists()
		{
			app.open_workspace(&root);
		}
		app
	}

	pub fn set_theme(&mut self, mode: theme::ThemeMode) {
		self.config.theme = mode;
		// a failed save is not reported yet (bug B08)
		let _ = self.config.save();
		theme::apply(&self.ctx, mode);
	}

	pub fn open(&mut self, path: &Path) {
		if path.is_dir() {
			self.open_workspace(path);
			return;
		}
		match std::fs::read(path) {
			Ok(bytes) => self.open_bytes(path.to_path_buf(), &bytes),
			Err(e) => self
				.toasts
				.error(format!("Could not read {}: {e}", path.display())),
		}
	}

	pub fn open_bytes(&mut self, path: PathBuf, bytes: &[u8]) {
		if path
			.extension()
			.is_some_and(|e| e.eq_ignore_ascii_case("json"))
		{
			// a rig file of the open workspace opens its model; any other
			// export is imported
			if let Some(index) = self.workspace_model_at(&path) {
				self.open_model(index);
			} else {
				self.start_import(path, bytes);
			}
			return;
		}
		// an image dropped or opened while a mesh is shown becomes one of its
		// maps, chosen by name: Name.normal.png is a normal map, and so on
		if texture::is_image(&path) {
			if self.doc.is_some() {
				self.set_map(slot_for_image(&path), &path, bytes);
			} else {
				self.toasts.error(format!(
					"{} is an image. Open a mesh first, then drop images on it to use them as maps.",
					path.display()
				));
			}
			return;
		}
		// a mesh of the open workspace opens as its item, with its maps
		if let Some(index) = self.workspace_item_at(&path) {
			self.open_item(index);
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
				let names: Vec<_> = found
					.values()
					.filter_map(|p| p.file_name())
					.map(|n| n.to_string_lossy())
					.collect();
				self.toasts
					.info(format!("Loaded maps: {}", names.join(", ")));
			}
		}
	}

	/// Decodes a mesh into the viewer. Returns false (and reports the error) on failure.
	pub fn load_mesh(&mut self, path: PathBuf, bytes: &[u8]) -> bool {
		match geometry::load(bytes) {
			Ok(loaded) => {
				let camera = Camera::framing(&loaded.geometry);
				let hull_visible = vec![true; loaded.geometry.groups.len()];
				self.model = None;
				self.loading_model = None;
				self.doc = Some(Document {
					path,
					size: bytes.len(),
					loaded,
					camera,
					lod: 0,
					hull_visible,
					material: Material::default(),
					gpu: None,
					alpha: crate::render::viewport::AlphaMode::Overlay,
				});
				true
			}
			Err(e) => {
				let name = path
					.file_name()
					.map(|n| n.to_string_lossy().into_owned())
					.unwrap_or_default();
				self.toasts.error(format!("Could not open {name}: {e}"));
				false
			}
		}
	}

	pub fn frame_model(&mut self) {
		if let Some(doc) = &mut self.doc {
			doc.camera = Camera::framing(&doc.loaded.geometry);
		}
		if let Some(model) = &mut self.model {
			model.camera = model.home();
		}
	}

	// ------------------------------------------------------------ workspaces

	pub fn open_workspace(&mut self, root: &Path) {
		match Workspace::open(root) {
			Ok(ws) => {
				self.close_workspace();
				self.workspace = Some(ws);
				self.show_library = true;
				self.config.add_recent(root.to_path_buf());
			}
			Err(e) => self.toasts.error(e),
		}
	}

	pub fn close_workspace(&mut self) {
		if self.downloader.is_running() {
			self.downloader.cancel();
		}
		self.save_workspace();
		// a model from the workspace closes with it; one opened from a file
		// elsewhere stays open
		if let (Some(ws), Some(doc)) = (&self.workspace, &self.doc)
			&& (self.open_stem.is_some() || doc.path.starts_with(&ws.root))
		{
			self.doc = None;
		}
		self.model = None;
		self.loading_model = None;
		self.workspace = None;
		self.import = None;
		self.parsing = None;
		self.open_stem = None;
	}

	pub fn create_workspace(&mut self, name: &str, parent: &Path) -> Result<(), String> {
		let name = name.trim();
		if name.is_empty() {
			return Err("Enter a workspace name.".into());
		}
		let root = parent.join(workspace::sanitize(name));
		let ws = Workspace::create(&root, name)?;
		self.close_workspace();
		self.workspace = Some(ws);
		self.show_library = true;
		self.config.add_recent(root);
		self.toasts.success(format!("Created workspace {name}"));
		Ok(())
	}

	pub fn pick_open_workspace(&mut self) {
		if let Some(dir) = rfd::FileDialog::new()
			.set_title("Open workspace folder")
			.set_directory(Config::default_workspaces_dir())
			.pick_folder()
		{
			self.open_workspace(&dir);
		}
	}

	pub fn show_new_workspace(&mut self, then_import: Option<PathBuf>) {
		self.new_workspace = Some(NewWorkspaceDialog {
			name: String::new(),
			parent: Config::default_workspaces_dir(),
			then_import,
		});
	}

	/// Workspaces from the recent list that still exist.
	pub fn recent_workspaces(&self) -> Vec<PathBuf> {
		self.config
			.recent_workspaces
			.iter()
			.filter(|p| p.join(workspace::MANIFEST).exists())
			.cloned()
			.collect()
	}

	pub fn save_workspace(&mut self) {
		if let Some(ws) = &mut self.workspace
			&& let Err(e) = ws.save_if_dirty()
		{
			self.toasts.error(e);
		}
		self.last_save = Instant::now();
	}

	pub fn pick_import(&mut self) {
		if let Some(path) = rfd::FileDialog::new()
			.set_title("Import export JSON")
			.add_filter("Export JSON", &["json"])
			.pick_file()
		{
			match std::fs::read(&path) {
				Ok(bytes) => self.start_import(path, &bytes),
				Err(e) => self
					.toasts
					.error(format!("Could not read {}: {e}", path.display())),
			}
		}
	}

	pub fn start_import(&mut self, path: PathBuf, bytes: &[u8]) {
		if self.workspace.is_none() {
			// make a workspace first, then come back to this export
			self.show_new_workspace(Some(path));
			return;
		}
		// schema 1 exports with animations run to hundreds of megabytes
		let (tx, rx) = std::sync::mpsc::channel();
		let bytes = bytes.to_vec();
		let ctx = self.ctx.clone();
		std::thread::spawn(move || {
			let result = export_file::parse(&String::from_utf8_lossy(&bytes));
			drop(bytes);
			let _ = tx.send(result);
			ctx.request_repaint();
		});
		self.parsing = Some(Parsing {
			path,
			rx,
			started: Instant::now(),
		});
	}

	pub fn poll_parsing(&mut self) {
		let Some(parsing) = &self.parsing else { return };
		let result = match parsing.rx.try_recv() {
			Ok(result) => result,
			Err(std::sync::mpsc::TryRecvError::Empty) => return,
			Err(std::sync::mpsc::TryRecvError::Disconnected) => {
				Err("The export reader stopped unexpectedly".into())
			}
		};
		let path = self.parsing.take().expect("checked above").path;
		let Some(ws) = &self.workspace else { return };
		match result {
			Ok(export) => self.import = Some(ImportDialog::new(path, export, ws)),
			Err(e) => self.toasts.error(format!("{}: {e}", path.display())),
		}
	}

	pub fn finish_import(&mut self, outcome: import::Outcome) {
		let import::Outcome::Import {
			entries,
			models,
			download_now,
		} = outcome
		else {
			return;
		};
		let Some(ws) = &mut self.workspace else {
			return;
		};
		let count = entries.len();
		let mut added = 0;
		let mut stems = HashMap::new();
		for entry in &entries {
			let (stem, new) = ws.add_entry(entry);
			added += usize::from(new);
			if let Some(id) = entry.source_id {
				stems.insert(id, stem);
			}
		}
		let by_id: HashMap<u32, &export_file::Entry> = entries
			.iter()
			.filter_map(|e| Some((e.source_id?, e)))
			.collect();
		let mut rigs = 0;
		for model in &models {
			let refs: BTreeSet<u32> = model
				.parts
				.iter()
				.map(|p| p.mesh_ref)
				.filter(|&r| r != 0)
				.collect();
			let meshes: Vec<(&export_file::Entry, String)> = refs
				.iter()
				.filter_map(|r| Some((*by_id.get(r)?, stems.get(r)?.clone())))
				.collect();
			match ws.add_model(model, &meshes) {
				Ok(_) => rigs += 1,
				Err(e) => self.toasts.error(format!("{}: {e}", model.name)),
			}
		}
		let rigs = match rigs {
			0 => String::new(),
			1 => " and 1 animated model".into(),
			n => format!(" and {n} animated models"),
		};
		self.toasts.success(format!(
			"Imported {count} meshes{rigs} ({added} new, {} already in the workspace)",
			count - added
		));
		self.show_library = true;
		self.save_workspace();
		if download_now {
			self.start_downloads();
		}
	}

	pub fn open_settings(&mut self, notice: Option<String>) {
		let test_id = self
			.workspace
			.as_ref()
			.and_then(|ws| ws.items().first().map(|i| i.mesh_id));
		let mut dialog = SettingsDialog::new(&self.config, test_id);
		dialog.notice = notice;
		self.settings = Some(dialog);
	}

	pub fn start_downloads(&mut self) {
		let Some(ws) = &self.workspace else { return };
		let jobs = ws.pending_jobs();
		if jobs.is_empty() {
			self.toasts.info("Nothing to download.");
			return;
		}
		let Some(client) = settings::client_from_config(&self.config) else {
			self.open_settings(Some("Add a Roblox credential to start downloading.".into()));
			return;
		};
		let root = ws.root.clone();
		self.downloader
			.start(&self.ctx, root, client, jobs, self.config.concurrency);
	}

	pub fn retry_failed(&mut self) {
		if let Some(ws) = &mut self.workspace {
			ws.retry(0..ws.items().len());
			ws.retry_images();
		}
		self.start_downloads();
	}

	/// Counts of (pending, failed) items in the open workspace.
	pub fn workspace_counts(&self) -> (usize, usize) {
		self.workspace
			.as_ref()
			.map(|ws| {
				let items = ws.items();
				let (images_pending, images_failed) = ws.image_counts();
				(
					items.iter().filter(|i| i.has_pending()).count() + images_pending,
					items.iter().filter(|i| i.has_failure()).count() + images_failed,
				)
			})
			.unwrap_or((0, 0))
	}

	pub fn poll_downloads(&mut self) {
		let events = self.downloader.poll();
		let had_events = !events.is_empty();
		for event in events {
			match event {
				Event::Finished { job, result } => {
					let Some(ws) = &mut self.workspace else {
						continue;
					};
					ws.record_download(&job, result);
				}
				Event::AuthFailed(message) => {
					self.toasts.error(format!("Downloads stopped: {message}"));
					self.open_settings(Some(message));
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
			let ok = d.completed - d.failed;
			if d.failed > 0 {
				self.toasts.error(format!(
					"Downloads finished: {ok} succeeded, {} failed. Hover a red item in the library for the reason.",
					d.failed
				));
			} else {
				self.toasts.success(format!("Downloaded {ok} assets"));
			}
		}
	}

	pub fn open_item(&mut self, index: usize) {
		let Some(ws) = &self.workspace else { return };
		let Some(item) = ws.items().get(index) else {
			return;
		};
		let mesh_path = match &item.mesh {
			AssetState::Done { path } => ws.root.join(path),
			AssetState::Pending => {
				self.toasts
					.info(format!("{} has not been downloaded yet.", item.name));
				return;
			}
			AssetState::Failed { error } => {
				self.toasts
					.error(format!("{} failed to download: {error}", item.name));
				return;
			}
		};
		let maps = item_maps(&ws.root, item);
		let stem = item.stem.clone();
		match std::fs::read(&mesh_path) {
			Ok(bytes) => {
				if self.load_mesh(mesh_path, &bytes) {
					self.open_stem = Some(stem);
					for (slot, path) in maps {
						self.load_map(slot, &path);
					}
				}
			}
			Err(e) => self
				.toasts
				.error(format!("Could not read {}: {e}", mesh_path.display())),
		}
	}

	/// Opens an animated model from the workspace. Its meshes and maps are
	/// read on a background thread.
	pub fn open_model(&mut self, index: usize) {
		let Some(ws) = &self.workspace else { return };
		let Some(item) = ws.models().get(index) else {
			return;
		};
		let Some(load) = model_loader(ws, index) else {
			return;
		};
		let name = item.name.clone();
		let (tx, rx) = std::sync::mpsc::channel();
		let ctx = self.ctx.clone();
		std::thread::spawn(move || {
			let _ = tx.send(load(&ctx));
			ctx.request_repaint();
		});
		self.loading_model = Some(LoadingModel {
			name,
			rx,
			started: Instant::now(),
		});
	}

	/// The open workspace's item whose mesh is the file at `path`.
	fn workspace_item_at(&self, path: &Path) -> Option<usize> {
		let ws = self.workspace.as_ref()?;
		let path = path.canonicalize().ok()?;
		ws.items().iter().position(|i| {
			i.done_path(AssetKind::Mesh)
				.and_then(|p| ws.root.join(p).canonicalize().ok())
				.is_some_and(|p| p == path)
		})
	}

	/// The open workspace's model stored at `path`, if it is one.
	fn workspace_model_at(&self, path: &Path) -> Option<usize> {
		let ws = self.workspace.as_ref()?;
		let path = path.canonicalize().ok()?;
		ws.models().iter().position(|m| {
			ws.root
				.join(&m.file)
				.canonicalize()
				.is_ok_and(|p| p == path)
		})
	}

	/// Saves exports that arrived from Studio and opens them for import.
	pub fn poll_studio_link(&mut self) {
		for event in self.studio_link.poll() {
			let received = match event {
				crate::studio::link::Event::Progress { done, count } => {
					self.studio_progress = Some((done, count, Instant::now()));
					continue;
				}
				crate::studio::link::Event::Received(received) => received,
			};
			self.studio_progress = None;
			// the export is here, so the instructions are no longer needed
			self.studio_dialog = None;
			let path = received.path.clone();
			let saved = path
				.parent()
				.map_or(Ok(()), std::fs::create_dir_all)
				.and_then(|_| std::fs::write(&path, &received.json));
			match saved {
				Ok(()) => {
					self.toasts.success(format!(
						"Received {} from Roblox Studio ({}), saved to {}",
						if received.name.is_empty() {
							"an export"
						} else {
							&received.name
						},
						format_bytes(received.json.len()),
						path.display()
					));
					self.start_import(path, &received.json);
				}
				Err(e) => self.toasts.error(format!(
					"Could not save the export from Studio to {}: {e}",
					path.display()
				)),
			}
		}
	}

	pub fn poll_model(&mut self) {
		let Some(loading) = &self.loading_model else {
			return;
		};
		let result = match loading.rx.try_recv() {
			Ok(result) => result,
			Err(std::sync::mpsc::TryRecvError::Empty) => return,
			Err(std::sync::mpsc::TryRecvError::Disconnected) => {
				Err("Reading the model stopped unexpectedly".into())
			}
		};
		self.loading_model = None;
		match result {
			Ok(model) => {
				self.doc = None;
				self.open_stem = None;
				self.model = Some(model);
			}
			Err(e) => self.toasts.error(e),
		}
	}

	/// Removes animated models from the workspace; their meshes stay.
	pub fn remove_models(&mut self, models: &[usize]) {
		if let Some(ws) = &mut self.workspace {
			if let Some(open) = &mut self.model {
				if models.contains(&open.index) {
					self.model = None;
				} else {
					// later models move up
					open.index -= models.iter().filter(|&&i| i < open.index).count();
				}
			}
			ws.remove_models(models);
		}
		self.save_workspace();
	}

	/// Removes workspace items and deletes their organized files.
	pub fn remove_items(&mut self, items: &[usize]) {
		if let Some(ws) = &mut self.workspace {
			let removes_open = self.open_stem.as_ref().is_some_and(|open| {
				items
					.iter()
					.any(|&i| ws.items().get(i).is_some_and(|it| &it.stem == open))
			});
			if removes_open {
				self.open_stem = None;
			}
			ws.remove(items);
		}
		self.save_workspace();
	}

	pub fn handle_browser(&mut self, actions: Vec<ui::workspace::Action>) {
		for action in actions {
			match action {
				ui::workspace::Action::Open(i) => self.open_item(i),
				ui::workspace::Action::OpenModel(i) => self.open_model(i),
				ui::workspace::Action::RemoveModels(models) => {
					self.ask(ConfirmAction::RemoveModels(models))
				}
				ui::workspace::Action::Retry(items) => {
					if let Some(ws) = &mut self.workspace {
						ws.retry(items);
					}
					self.start_downloads();
				}
				// deletes files, so it is confirmed first
				ui::workspace::Action::Remove(items) => self.ask(ConfirmAction::RemoveItems(items)),
				ui::workspace::Action::Reveal(path) => {
					// a failure to start the file browser is not shown yet
					let _ = crate::platform::reveal(&path);
				}
				ui::workspace::Action::RemoveSelection { items, models } => {
					self.ask(ConfirmAction::RemoveSelection { items, models })
				}
				ui::workspace::Action::Export {
					items,
					models,
					format,
				} => self.export_library(&items, &models, format),
			}
		}
	}

	// ----------------------------------------------------------------- maps

	pub fn load_map(&mut self, slot: Slot, path: &Path) {
		match std::fs::read(path) {
			Ok(bytes) => self.set_map(slot, path, &bytes),
			Err(e) => self
				.toasts
				.error(format!("Could not read {}: {e}", path.display())),
		}
	}

	pub fn set_map(&mut self, slot: Slot, path: &Path, bytes: &[u8]) {
		let Some(doc) = &mut self.doc else { return };
		match texture::load(&self.ctx, path, bytes) {
			Ok(texture) => {
				doc.material.set(slot, texture);
				self.show_maps = true;
			}
			Err(e) => self.toasts.error(e),
		}
	}

	pub fn pick_map(&mut self, slot: Slot) {
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

	pub fn remove_all_maps(&mut self) {
		if let Some(doc) = &mut self.doc {
			for slot in Slot::ALL {
				doc.material.remove(slot);
			}
		}
	}

	pub fn pick_file(&mut self) {
		if let Some(path) = rfd::FileDialog::new()
			.set_title("Open Roblox mesh")
			.add_filter(
				"Roblox mesh / union data",
				&["mesh", "meshdata", "meshdata5", "data", "bin"],
			)
			.add_filter("All files", &["*"])
			.pick_file()
		{
			self.open(&path);
		}
	}

	/// Exports library meshes and animated models into a folder the user
	/// picks, on a background thread.
	pub fn export_library(&mut self, items: &[usize], models: &[usize], format: Format) {
		use crate::export::obj::{MeshJob, ModelJob};
		let Some(ws) = &self.workspace else { return };
		let meshes: Vec<MeshJob> = items
			.iter()
			.filter_map(|&i| {
				let item = ws.items().get(i)?;
				let AssetState::Done { path } = &item.mesh else {
					return None;
				};
				Some(MeshJob {
					name: item.name.clone(),
					mesh: ws.root.join(path),
					maps: item_maps(&ws.root, item),
				})
			})
			.collect();
		let skipped = items.len() - meshes.len();
		let models: Vec<ModelJob> = models
			.iter()
			.filter_map(|&i| {
				Some(ModelJob {
					name: ws.models().get(i)?.name.clone(),
					load: Box::new(model_loader(ws, i)?),
				})
			})
			.collect();
		if meshes.is_empty() && models.is_empty() {
			self.toasts
				.info("Nothing to export: the selected meshes are not downloaded yet.");
			return;
		}
		let what = match format {
			Format::Glb => "for Blender (.glb)",
			Format::Obj => "as OBJ",
		};
		let Some(dir) = rfd::FileDialog::new()
			.set_title(format!("Export {what} into"))
			.pick_folder()
		else {
			return;
		};
		if skipped > 0 {
			self.toasts.info(format!(
				"{skipped} selected meshes are not downloaded yet and are skipped."
			));
		}
		self.in_background(move |ctx| {
			let report = crate::export::obj::export(ctx, &dir, meshes, models, format);
			let written = plural(report.written, "model", "models");
			if report.failed.is_empty() {
				Ok(format!(
					"Exported {written} {what} to {}",
					report.dir.display()
				))
			} else {
				Err(format!(
					"Exported {written}; {} failed:\n{}",
					report.failed.len(),
					report.failed.join("\n")
				))
			}
		});
	}

	/// Runs `work` on a background thread; its message is shown when it ends.
	fn in_background(
		&mut self,
		work: impl FnOnce(&egui::Context) -> Result<String, String> + Send + 'static,
	) {
		let (tx, rx) = std::sync::mpsc::channel();
		let ctx = self.ctx.clone();
		std::thread::spawn(move || {
			let _ = tx.send(work(&ctx));
			ctx.request_repaint();
		});
		self.background.push(rx);
	}

	/// Shows the messages of background work that ended.
	pub fn poll_background(&mut self) {
		let mut done = Vec::new();
		self.background.retain(|rx| match rx.try_recv() {
			Ok(result) => {
				done.push(result);
				false
			}
			Err(std::sync::mpsc::TryRecvError::Empty) => true,
			Err(std::sync::mpsc::TryRecvError::Disconnected) => {
				done.push(Err("Background work stopped unexpectedly.".into()));
				false
			}
		});
		for result in done {
			match result {
				Ok(message) => self.toasts.success(message),
				Err(message) => self.toasts.error(message),
			}
		}
	}

	/// The open mesh's or model's name, for file names.
	fn shown_name(&self) -> String {
		if let Some(model) = &self.model {
			return workspace::sanitize(&model.name);
		}
		self.doc
			.as_ref()
			.and_then(|d| d.path.file_stem())
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_else(|| "mesh".into())
	}

	fn save_dialog(&self, title: &str, extension: &str, filter: &str) -> Option<PathBuf> {
		rfd::FileDialog::new()
			.set_title(title)
			.set_file_name(format!("{}.{extension}", self.shown_name()))
			.add_filter(filter, &[extension])
			.save_file()
	}

	/// Exports what is shown as binary glTF for Blender: an animated model
	/// with its skeleton, skin weights and every clip; a mesh with its maps.
	pub fn export_glb(&mut self) {
		if let Some(model) = &self.model {
			let Some(ws) = &self.workspace else { return };
			// a stale index still panics here, as before (bug B13)
			let load = model_loader(ws, model.index).expect("the open model is in the workspace");
			let name = model.name.clone();
			let Some(path) = self.save_dialog("Export for Blender", "glb", "glTF binary") else {
				return;
			};
			self.in_background(move |ctx| {
				let model = load(ctx)?;
				crate::export::gltf::export_model(&model, &path)?;
				Ok(format!(
					"Exported {name} with {} for Blender to {}. In Blender: File › Import › glTF 2.0.",
					plural(
						model.clips.iter().filter(|c| c.clip.is_ok()).count(),
						"clip",
						"clips"
					),
					path.display()
				))
			});
		} else if self.doc.is_some() {
			let Some(path) = self.save_dialog("Export for Blender", "glb", "glTF binary") else {
				return;
			};
			let Some(doc) = &self.doc else { return };
			let name = self.shown_name();
			let result = crate::export::gltf::export_mesh(
				&doc.loaded.geometry,
				&doc.visible_groups(),
				&doc.material,
				crate::render::viewport::PART_COLOUR,
				&name,
				&path,
			);
			match result {
				Ok(()) => self.toasts.success(format!(
					"Exported {} for Blender. In Blender: File › Import › glTF 2.0.",
					path.display()
				)),
				Err(e) => self.toasts.error(format!("Export failed: {e}")),
			}
		}
	}

	/// Exports what is shown as OBJ, with an MTL file and its maps beside
	/// it: the visible LODs or hulls of a mesh, or a model as posed now.
	pub fn export_obj(&mut self) {
		if self.doc.is_none() && self.model.is_none() {
			return;
		}
		let Some(path) = self.save_dialog("Export OBJ", "obj", "Wavefront OBJ") else {
			return;
		};
		let result = if let Some(model) = &mut self.model {
			crate::export::obj::write_model(&path, model)
		} else if let Some(doc) = &self.doc {
			crate::export::obj::write_mesh(
				&path,
				&doc.loaded.geometry,
				&doc.visible_groups(),
				&doc.material,
			)
		} else {
			return;
		};
		match result {
			Ok(()) => self.toasts.success(format!("Exported {}", path.display())),
			Err(e) => self.toasts.error(format!("Export failed: {e}")),
		}
	}

	/// Draws the view as it is now into `frames` images of `size`; with more
	/// than one, the camera turns once around the model while a selected
	/// clip plays on, `step` seconds a frame.
	fn render_view(
		&mut self,
		size: [u32; 2],
		frames: usize,
		step: f32,
	) -> Option<Vec<image::RgbaImage>> {
		let renderer = self.gpu.clone()?;
		let p = theme::palette(&self.ctx);
		let mut settings = self.view_settings(&p);
		settings.show_grid = false;
		settings.show_axes = false;
		let plain_material = self.no_material.clone();
		let plain = (!self.show_maps).then_some(&plain_material);
		let turn = |camera: &Camera, k: usize| {
			let mut camera = camera.clone();
			camera.yaw += std::f32::consts::TAU * k as f32 / frames as f32;
			camera
		};
		let mut out = Vec::with_capacity(frames);
		if let Some(model) = &mut self.model {
			let (camera, start) = (model.camera.clone(), model.time);
			let duration = model.duration();
			for k in 0..frames {
				if frames > 1 && model.current.is_some() && duration > 0.0 {
					model.set_time((start + k as f32 * step).rem_euclid(duration));
				}
				settings.lights = model.scene_lights();
				let mut parts = model.scene_parts(plain);
				out.push(crate::render::viewport::render_image(
					&mut parts,
					&turn(&camera, k),
					&settings,
					&renderer,
					size,
				));
			}
			model.set_time(start);
		} else if let Some(doc) = &mut self.doc {
			let groups = doc.visible_groups();
			let geometry = &doc.loaded.geometry;
			for k in 0..frames {
				let mut parts = [crate::render::viewport::ScenePart {
					geometry,
					groups: &groups,
					transform: crate::render::viewport::IDENTITY,
					material: plain.unwrap_or(&doc.material),
					colour_groups: geometry.group_kind == GroupKind::Hull,
					colour: crate::render::viewport::PART_COLOUR,
					scene: &mut doc.gpu,
					moved: false,
					surface: crate::render::viewport::Surface {
						alpha_mode: doc.alpha,
						..crate::render::viewport::Surface::OPAQUE
					},
					decal: None,
					highlight: false,
				}];
				out.push(crate::render::viewport::render_image(
					&mut parts,
					&turn(&doc.camera, k),
					&settings,
					&renderer,
					size,
				));
			}
		} else {
			return None;
		}
		Some(out)
	}

	pub fn set_lighting(&mut self, lighting: crate::render::viewport::Lighting) {
		self.config.lighting = lighting;
		// a failed save is not reported yet (bug B08)
		let _ = self.config.save();
	}

	/// Copies the view as it is now, over the viewport's background.
	pub fn copy_image(&mut self) {
		let size = frame_size(self.viewport, STILL_HEIGHT);
		let Some(image) = self.render_view(size, 1, 0.0).and_then(|mut f| f.pop()) else {
			self.toasts.error("Copying images needs the GPU renderer.");
			return;
		};
		let p = theme::palette(&self.ctx);
		let image = over_gradient(&image, p.viewport_top, p.viewport_bottom);
		let pixels = egui::ColorImage::from_rgba_unmultiplied(
			[image.width() as usize, image.height() as usize],
			image.as_raw(),
		);
		self.ctx.copy_image(pixels);
		self.toasts.success("Copied the view to the clipboard");
	}

	/// Saves the view as it is now as a PNG with a transparent background.
	pub fn save_image(&mut self) {
		if self.gpu.is_none() {
			self.toasts.error("Saving images needs the GPU renderer.");
			return;
		}
		let Some(path) = self.save_dialog("Save image", "png", "PNG image") else {
			return;
		};
		let size = frame_size(self.viewport, STILL_HEIGHT);
		let Some(image) = self.render_view(size, 1, 0.0).and_then(|mut f| f.pop()) else {
			return;
		};
		self.in_background(move |_| {
			image
				.save(&path)
				.map_err(|e| format!("Could not save {}: {e}", path.display()))?;
			Ok(format!("Saved {}", path.display()))
		});
	}

	/// Saves a GIF of the camera turning once around the model, playing
	/// the selected clip, over the viewport's background.
	pub fn save_turntable(&mut self) {
		if self.gpu.is_none() {
			self.toasts
				.error("Saving turntables needs the GPU renderer.");
			return;
		}
		let Some(path) = self.save_dialog("Save turntable", "gif", "GIF animation") else {
			return;
		};
		let size = frame_size(self.viewport, TURNTABLE_HEIGHT);
		let delay = 1.0 / TURNTABLE_FPS;
		let Some(frames) = self.render_view(size, TURNTABLE_FRAMES, delay) else {
			return;
		};
		let p = theme::palette(&self.ctx);
		let (top, bottom) = (p.viewport_top, p.viewport_bottom);
		self.in_background(move |_| {
			use image::codecs::gif::{GifEncoder, Repeat};
			let file = std::fs::File::create(&path)
				.map_err(|e| format!("Could not create {}: {e}", path.display()))?;
			let mut encoder = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 10);
			encoder
				.set_repeat(Repeat::Infinite)
				.map_err(|e| e.to_string())?;
			for frame in frames {
				let frame = over_gradient(&frame, top, bottom);
				let delay = image::Delay::from_numer_denom_ms((delay * 1000.0).round() as u32, 1);
				encoder
					.encode_frame(image::Frame::from_parts(frame, 0, 0, delay))
					.map_err(|e| format!("Could not write {}: {e}", path.display()))?;
			}
			Ok(format!("Saved {}", path.display()))
		});
	}
}

/// Height in pixels of saved images.
const STILL_HEIGHT: f32 = 1080.0;
/// A turntable: frame count, rate and height in pixels.
const TURNTABLE_FRAMES: usize = 72;
const TURNTABLE_FPS: f32 = 25.0;
const TURNTABLE_HEIGHT: f32 = 480.0;

/// The viewport's shape, scaled to `height` pixels (even sides).
fn frame_size(viewport: egui::Rect, height: f32) -> [u32; 2] {
	let aspect = viewport.width() / viewport.height().max(1.0);
	let aspect = if aspect.is_finite() && aspect > 0.0 {
		aspect
	} else {
		16.0 / 9.0
	};
	let even = |v: f32| ((v / 2.0).round() as u32).max(1) * 2;
	[even(height * aspect), even(height)]
}

/// `image` laid over a vertical gradient from `top` to `bottom`.
fn over_gradient(
	image: &image::RgbaImage,
	top: egui::Color32,
	bottom: egui::Color32,
) -> image::RgbaImage {
	let h = image.height().max(2) - 1;
	image::RgbaImage::from_fn(image.width(), image.height(), |x, y| {
		let t = y as f32 / h as f32;
		let bg = [top.r(), top.g(), top.b()]
			.iter()
			.zip([bottom.r(), bottom.g(), bottom.b()])
			.map(|(&a, b)| a as f32 + (b as f32 - a as f32) * t)
			.collect::<Vec<f32>>();
		let px = image.get_pixel(x, y);
		let a = px[3] as f32 / 255.0;
		let mix = |k: usize| (px[k] as f32 * a + bg[k] * (1.0 - a)).round() as u8;
		image::Rgba([mix(0), mix(1), mix(2), 255])
	})
}

/// Picks the material slot for a dropped image from its name, following the
/// workspace naming: `Name.normal.png`, `Name.roughness.png`, `Name.metalness.png`.
pub fn slot_for_image(path: &Path) -> Slot {
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn dropped_images_pick_their_slot_from_the_name() {
		let slot = |name: &str| slot_for_image(Path::new(name));
		assert_eq!(slot("Models/Rock.normal.png"), Slot::Normal);
		assert_eq!(slot("Rock.roughness.png"), Slot::Roughness);
		assert_eq!(slot("Rock.metalness.jpg"), Slot::Metalness);
		assert_eq!(slot("Rock.NORMAL.PNG"), Slot::Normal);
		assert_eq!(slot("Rock.png"), Slot::Base);
		assert_eq!(slot("Rock.color.png"), Slot::Base);
		// the suffix needs its dot
		assert_eq!(slot("normal.png"), Slot::Base);
		assert_eq!(slot(""), Slot::Base);
	}

	fn rect(w: f32, h: f32) -> egui::Rect {
		egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(w, h))
	}

	#[test]
	fn frames_keep_the_viewport_shape_with_even_sides() {
		assert_eq!(frame_size(rect(1600.0, 900.0), 1080.0), [1920, 1080]);
		assert_eq!(frame_size(rect(900.0, 900.0), 480.0), [480, 480]);
		// 721.5 x 481 rounds to the nearest even sides
		assert_eq!(frame_size(rect(300.0, 200.0), 481.0), [722, 482]);
		// never below 2 pixels
		assert_eq!(frame_size(rect(300.0, 200.0), 0.0), [2, 2]);
	}

	#[test]
	fn frames_of_a_degenerate_viewport() {
		// no width, or not a number: 16:9
		assert_eq!(frame_size(egui::Rect::ZERO, 1080.0), [1920, 1080]);
		assert_eq!(frame_size(rect(0.0, 500.0), 1080.0), [1920, 1080]);
		assert_eq!(frame_size(egui::Rect::NAN, 1080.0), [1920, 1080]);
		// no height counts as 1 pixel high
		assert_eq!(frame_size(rect(100.0, 0.0), 10.0), [1000, 10]);
	}

	#[test]
	fn images_are_laid_over_the_gradient() {
		let image = image::RgbaImage::from_raw(
			2,
			3,
			[
				[9, 9, 9, 0],
				[10, 20, 30, 255],
				[255, 255, 255, 64],
				[9, 9, 9, 0],
				[255, 255, 255, 128],
				[40, 50, 60, 255],
			]
			.concat(),
		)
		.unwrap();
		let top = egui::Color32::from_rgb(0, 0, 0);
		let bottom = egui::Color32::from_rgb(200, 100, 50);
		let out = over_gradient(&image, top, bottom);
		let pixels: Vec<[u8; 4]> = out.pixels().map(|p| p.0).collect();
		assert_eq!(
			pixels,
			[
				// top row: the top colour where transparent
				[0, 0, 0, 255],
				[10, 20, 30, 255],
				// half way down, a quarter opaque white and the bare gradient
				[139, 101, 83, 255],
				[100, 50, 25, 255],
				// bottom row: half opaque white
				[228, 178, 153, 255],
				[40, 50, 60, 255],
			]
		);
	}

	#[test]
	fn a_single_row_takes_the_top_colour() {
		let image = image::RgbaImage::from_raw(1, 1, vec![0, 0, 0, 0]).unwrap();
		let top = egui::Color32::from_rgb(12, 34, 56);
		let bottom = egui::Color32::from_rgb(200, 100, 50);
		assert_eq!(
			over_gradient(&image, top, bottom).get_pixel(0, 0).0,
			[12, 34, 56, 255]
		);
	}
}
