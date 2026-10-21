//! The workspace's life cycle, opening its items and models, and removing them.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::App;
use super::task::Task;
use crate::config::Config;
use crate::scene::model_view::ModelView;
use crate::ui::{self, ConfirmAction, NewWorkspaceDialog};
use crate::workspace::view_loader::{item_maps, model_loader};
use crate::workspace::{self, AssetKind, AssetState, Workspace};

/// An animated model being read on a background thread.
pub struct LoadingModel {
	pub name: String,
	pub task: Task<ModelView>,
}

impl App {
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
		let task = Task::spawn(&self.ctx, load);
		self.loading_model = Some(LoadingModel { name, task });
	}

	/// The open workspace's item whose mesh is the file at `path`.
	pub(super) fn workspace_item_at(&self, path: &Path) -> Option<usize> {
		let ws = self.workspace.as_ref()?;
		let path = path.canonicalize().ok()?;
		ws.items().iter().position(|i| {
			i.done_path(AssetKind::Mesh)
				.and_then(|p| ws.root.join(p).canonicalize().ok())
				.is_some_and(|p| p == path)
		})
	}

	/// The open workspace's model stored at `path`, if it is one.
	pub(super) fn workspace_model_at(&self, path: &Path) -> Option<usize> {
		let ws = self.workspace.as_ref()?;
		let path = path.canonicalize().ok()?;
		ws.models().iter().position(|m| {
			ws.root
				.join(&m.file)
				.canonicalize()
				.is_ok_and(|p| p == path)
		})
	}

	pub(super) fn poll_model(&mut self) {
		let Some(loading) = &self.loading_model else {
			return;
		};
		let Some(result) = loading.task.poll("Reading the model stopped unexpectedly") else {
			return;
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

	pub fn handle_browser(&mut self, actions: Vec<ui::library::Action>) {
		for action in actions {
			match action {
				ui::library::Action::OpenItem(i) => self.open_item(i),
				ui::library::Action::OpenModel(i) => self.open_model(i),
				ui::library::Action::RemoveModels(models) => {
					self.ask(ConfirmAction::RemoveModels(models))
				}
				ui::library::Action::Retry(items) => {
					if let Some(ws) = &mut self.workspace {
						ws.retry(items);
					}
					self.start_downloads();
				}
				// deletes files, so it is confirmed first
				ui::library::Action::RemoveItems(items) => {
					self.ask(ConfirmAction::RemoveItems(items))
				}
				ui::library::Action::Reveal(path) => {
					// a failure to start the file browser is not shown yet
					let _ = crate::platform::reveal(&path);
				}
				ui::library::Action::RemoveSelection { items, models } => {
					self.ask(ConfirmAction::RemoveSelection { items, models })
				}
				ui::library::Action::Export {
					items,
					models,
					format,
				} => self.export_library(&items, &models, format),
			}
		}
	}
}
