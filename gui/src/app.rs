//! Application state and the actions the UI triggers. Drawing lives in `ui`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::config::Config;
use crate::download::{Downloader, Event};
use crate::geometry::{self, GroupKind, Loaded};
use crate::gpu::{GpuRenderer, GpuScene};
use crate::import_ui::{self, ImportDialog};
use crate::maps::Slot;
use crate::model_view::{MeshSource, ModelView};
use crate::settings_ui::{self, SettingsDialog};
use crate::texture::{self, Material};
use crate::ui::theme;
use crate::ui::{Confirm, ConfirmAction, StudioDialog, Toasts};
use crate::viewport::{Camera, Shading};
use crate::workspace::{self, AssetKind, AssetState, Workspace};
use crate::{export, workspace_ui};

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

pub struct Parsing {
	pub path: PathBuf,
	rx: std::sync::mpsc::Receiver<Result<export::Export, String>>,
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
	/// Shown instead of the document's maps when they are switched off.
	pub no_material: Material,
	/// `None` when the GPU renderer is unavailable; drawing falls back to the CPU.
	pub gpu: Option<Arc<GpuRenderer>>,
	/// Name of the graphics adapter, for the status bar.
	pub adapter: Option<String>,
	pub workspace: Option<Workspace>,
	pub browser: workspace_ui::Browser,
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
	pub studio_link: crate::studio_link::StudioLink,
	pub studio_dialog: Option<StudioDialog>,
	/// Pieces of an export from Studio received so far, out of how many, and when.
	pub studio_progress: Option<(usize, usize, Instant)>,
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
		let studio_link = crate::studio_link::StudioLink::start(&ctx);
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
			no_material: Material::default(),
			gpu,
			adapter,
			workspace: None,
			browser: workspace_ui::Browser::default(),
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
			last_save: Instant::now(),
		};
		if let crate::studio_link::Status::Unavailable(why) = &app.studio_link.status {
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
		self.config.save();
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
			let result = export::parse(&String::from_utf8_lossy(&bytes));
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

	pub fn finish_import(&mut self, outcome: import_ui::Outcome) {
		let import_ui::Outcome::Import {
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
		let by_id: HashMap<u32, &export::Entry> = entries
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
			let meshes: Vec<(&export::Entry, String)> = refs
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
		let Some(client) = settings_ui::client_from_config(&self.config) else {
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
		}
		self.start_downloads();
	}

	/// Counts of (pending, failed) items in the open workspace.
	pub fn workspace_counts(&self) -> (usize, usize) {
		self.workspace
			.as_ref()
			.map(|ws| {
				let items = ws.items();
				(
					items.iter().filter(|i| i.has_pending()).count(),
					items.iter().filter(|i| i.has_failure()).count(),
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
					match result {
						Ok(written) => {
							for w in written {
								ws.set_state(&w.stem, w.kind, AssetState::Done { path: w.path });
							}
						}
						Err(error) => {
							for (stem, kind) in &job.targets {
								ws.set_state(
									stem,
									*kind,
									AssetState::Failed {
										error: error.clone(),
									},
								);
							}
						}
					}
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
		let sources: HashMap<u32, MeshSource> = item
			.meshes
			.iter()
			.map(|(&mesh_ref, stem)| {
				let found = ws.items().iter().find(|i| &i.stem == stem);
				let source = match found {
					Some(i) => MeshSource {
						name: i.name.clone(),
						mesh: match &i.mesh {
							AssetState::Done { path } => Ok(ws.root.join(path)),
							AssetState::Pending => Err("not downloaded yet".into()),
							AssetState::Failed { error } => {
								Err(format!("download failed: {error}"))
							}
						},
						maps: item_maps(&ws.root, i),
					},
					None => MeshSource {
						name: stem.clone(),
						mesh: Err("removed from the workspace".into()),
						maps: Vec::new(),
					},
				};
				(mesh_ref, source)
			})
			.collect();
		let file = ws.root.join(&item.file);
		let name = item.name.clone();
		let (tx, rx) = std::sync::mpsc::channel();
		let ctx = self.ctx.clone();
		std::thread::spawn(move || {
			let result = std::fs::read_to_string(&file)
				.map_err(|e| format!("Could not read {}: {e}", file.display()))
				.and_then(|text| export::parse(&text))
				.and_then(|export| {
					export
						.models
						.into_iter()
						.next()
						.ok_or_else(|| format!("{} holds no model", file.display()))
				})
				.map(|model| ModelView::load(&ctx, index, &model, &sources));
			let _ = tx.send(result);
			ctx.request_repaint();
		});
		self.loading_model = Some(LoadingModel {
			name,
			rx,
			started: Instant::now(),
		});
	}

	/// The open workspace's model stored at `path`, if it is one.
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
				crate::studio_link::Event::Progress { done, count } => {
					self.studio_progress = Some((done, count, Instant::now()));
					continue;
				}
				crate::studio_link::Event::Received(received) => received,
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

	pub fn handle_browser(&mut self, actions: Vec<workspace_ui::Action>) {
		for action in actions {
			match action {
				workspace_ui::Action::Open(i) => self.open_item(i),
				workspace_ui::Action::OpenModel(i) => self.open_model(i),
				workspace_ui::Action::RemoveModels(models) => {
					self.ask(ConfirmAction::RemoveModels(models))
				}
				workspace_ui::Action::Retry(items) => {
					if let Some(ws) = &mut self.workspace {
						ws.retry(items);
					}
					self.start_downloads();
				}
				// deletes files, so it is confirmed first
				workspace_ui::Action::Remove(items) => self.ask(ConfirmAction::RemoveItems(items)),
				workspace_ui::Action::Reveal(path) => workspace_ui::reveal(&path),
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

	pub fn export_obj(&mut self) {
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
		match std::fs::write(&out, obj) {
			Ok(()) => self.toasts.success(format!("Exported {}", out.display())),
			Err(e) => self.toasts.error(format!("Export failed: {e}")),
		}
	}
}

/// Each material slot's downloaded map: the first that feeds it (ColorMap
/// over TextureID).
fn item_maps(root: &Path, item: &workspace::Item) -> Vec<(Slot, PathBuf)> {
	Slot::ALL
		.iter()
		.filter_map(|&slot| {
			let path = slot
				.sources()
				.iter()
				.find_map(|&k| item.done_path(AssetKind::Map(k)))?;
			Some((slot, root.join(path)))
		})
		.collect()
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

pub fn format_bytes(n: usize) -> String {
	match n {
		0..1024 => format!("{n} B"),
		1024..1_048_576 => format!("{:.1} KiB", n as f64 / 1024.0),
		_ => format!("{:.2} MiB", n as f64 / 1_048_576.0),
	}
}
