//! The workspace's life cycle, opening its items and models, and removing them.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::App;
use super::task::Task;
use crate::config::Config;
use crate::platform;
use crate::scene::model_view::ModelView;
use crate::ui::{self, ConfirmAction, NewWorkspaceDialog};
use crate::workspace::view_loader::{item_maps, model_loader};
use crate::workspace::{self, AssetKind, AssetState, ModelVersion, Workspace};

/// An animated model being read on a background thread.
pub struct LoadingModel {
	pub name: String,
	/// The version of the model being read. Unlike its index, which shifts
	/// when models before it are removed, or its file name, which a model
	/// imported again or added after it was removed keeps, this is its own.
	version: ModelVersion,
	pub task: Task<ModelView>,
}

/// What a finished model load is for now.
#[derive(Debug, PartialEq)]
enum Loaded {
	/// The model it read, now at this index.
	Current(usize),
	/// The model at this index, imported again since the load began, so
	/// what was read may be its old rig or old meshes.
	Reimported(usize),
	/// A model no longer in the workspace.
	Gone,
}

/// What the load of the model with `version` is for now.
fn loaded(ws: &Workspace, version: ModelVersion) -> Loaded {
	let models = ws.models();
	if let Some(i) = models.iter().position(|m| m.version == version) {
		Loaded::Current(i)
	} else if let Some(i) = models.iter().position(|m| m.version.same_model(version)) {
		Loaded::Reimported(i)
	} else {
		Loaded::Gone
	}
}

impl App {
	pub fn open_workspace(&mut self, root: &Path) {
		match Workspace::open(root) {
			Ok(ws) => {
				self.close_workspace();
				self.workspace = Some(ws);
				self.show_library = true;
				if let Err(e) = self.config.add_recent(root.to_path_buf()) {
					self.toasts.error(e);
				}
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
		if let Err(e) = self.config.add_recent(root) {
			self.toasts.error(e);
		}
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
		// owned, so the workspace is no longer borrowed when reading
		let maps = item_maps(&ws.root, item);
		let stem = item.stem.clone();
		let Some(bytes) = self.read_or_report(&mesh_path) else {
			return;
		};
		if self.load_mesh(mesh_path, &bytes) {
			self.open_stem = Some(stem);
			for (slot, path) in maps {
				self.load_map(slot, &path);
			}
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
		let version = item.version;
		let task = Task::spawn(&self.ctx, load);
		self.loading_model = Some(LoadingModel {
			name,
			version,
			task,
		});
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
		let version = loading.version;
		self.loading_model = None;
		// a model removed while it loaded is not shown, one imported again is
		// read again, and one that moved up is shown at its new place
		let index = match self.workspace.as_ref().map(|ws| loaded(ws, version)) {
			Some(Loaded::Current(index)) => index,
			Some(Loaded::Reimported(index)) => return self.open_model(index),
			Some(Loaded::Gone) | None => return,
		};
		match result {
			Ok(mut model) => {
				model.index = index;
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
			// the open model is found again by its path afterwards, since a
			// model whose rig file could not be deleted stays
			let open_path = self
				.model
				.as_ref()
				.and_then(|open| ws.models().get(open.index))
				.map(|m| m.path.clone());
			let removed = ws.remove_models(models);
			if let Some(open) = &mut self.model {
				match open_path.and_then(|p| ws.models().iter().position(|m| m.path == p)) {
					Some(i) => open.index = i,
					None => self.model = None,
				}
			}
			if let Err(e) = removed {
				self.toasts.error(e);
			}
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
			if let Err(e) = ws.remove(items) {
				self.toasts.error(e);
			}
		}
		self.save_workspace();
	}

	/// Shows `path` in the system file browser, or says why it could not.
	pub fn reveal(&mut self, path: &Path) {
		if let Err(e) = platform::reveal(path) {
			self.toasts
				.error(format!("Could not open the file browser: {e}"));
		}
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
				ui::library::Action::Reveal(path) => self.reveal(&path),
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

#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;

	use super::*;
	use crate::anim::schema::AnimatedModel;
	use crate::test_support::TempDir;

	fn model(file: &str) -> workspace::ModelItem {
		workspace::ModelItem {
			name: file.into(),
			path: file.into(),
			folder: Vec::new(),
			file: file.into(),
			meshes: BTreeMap::new(),
			parts: 0,
			clips: 0,
			playable_clips: 0,
			images: BTreeMap::new(),
			version: Default::default(),
		}
	}

	#[test]
	fn a_loading_model_is_found_again_after_removals() {
		let dir = TempDir::new("loading-model");
		let mut ws = Workspace::create(&dir.join("ws"), "ws").unwrap();
		let files = ["Rigs/a.rig.json", "Rigs/b.rig.json", "Rigs/c.rig.json"];
		ws.set_contents(Vec::new(), files.iter().map(|f| model(f)).collect());
		let (b, c) = (ws.models()[1].version, ws.models()[2].version);
		// c was loading at index 2 when a was removed
		ws.remove_models(&[0]).unwrap();
		assert_eq!(loaded(&ws, c), Loaded::Current(1));
		// and once c itself is removed it is not shown at all
		ws.remove_models(&[1]).unwrap();
		assert_eq!(loaded(&ws, c), Loaded::Gone);
		assert_eq!(loaded(&ws, b), Loaded::Current(0));
	}

	fn rig_model(name: &str, parts: usize) -> AnimatedModel {
		AnimatedModel {
			name: name.into(),
			path: format!("Workspace/{name}"),
			parts: vec![Default::default(); parts],
			..Default::default()
		}
	}

	/// A model imported again while it loads keeps its file name, but what
	/// the load read may be the old rig (or the new rig with the old
	/// meshes), so it is read again rather than shown (once bug B13).
	#[test]
	fn a_model_imported_again_while_it_loads_is_read_again() {
		let dir = TempDir::new("loading-model-reimport");
		let mut ws = Workspace::create(&dir.join("ws"), "ws").unwrap();
		ws.add_model(&rig_model("Other", 1), &[]).unwrap();
		ws.add_model(&rig_model("Wolf", 1), &[]).unwrap();
		let loading = ws.models()[1].version;
		let file = ws.models()[1].file.clone();
		assert!(model_loader(&ws, 1).is_some());

		assert!(!ws.add_model(&rig_model("Wolf", 2), &[]).unwrap());
		assert_eq!(ws.models()[1].file, file);
		assert_eq!(loaded(&ws, loading), Loaded::Reimported(1));
		// the load started again is for the new import, and moves with it
		let reloading = ws.models()[1].version;
		ws.remove_models(&[0]).unwrap();
		assert_eq!(loaded(&ws, reloading), Loaded::Current(0));
		assert_eq!(loaded(&ws, loading), Loaded::Reimported(0));
	}

	/// A model removed while it loads, and another added under the same file
	/// name before the load ends, is a different model: nothing is shown.
	#[test]
	fn a_model_added_under_a_removed_models_file_name_is_not_the_one_loaded() {
		let dir = TempDir::new("loading-model-reuse");
		let mut ws = Workspace::create(&dir.join("ws"), "ws").unwrap();
		ws.add_model(&rig_model("Wolf", 1), &[]).unwrap();
		let loading = ws.models()[0].version;
		let file = ws.models()[0].file.clone();

		ws.remove_models(&[0]).unwrap();
		assert!(ws.add_model(&rig_model("Wolf", 3), &[]).unwrap());
		assert_eq!(ws.models()[0].file, file);
		assert_eq!(loaded(&ws, loading), Loaded::Gone);
	}

	/// A model read again from the manifest is a new version, so a load
	/// from before the workspace was reopened is never taken for it.
	#[test]
	fn models_read_from_the_manifest_get_new_versions() {
		let dir = TempDir::new("loading-model-reopen");
		let root = dir.join("ws");
		let mut ws = Workspace::create(&root, "ws").unwrap();
		ws.add_model(&rig_model("Wolf", 1), &[]).unwrap();
		ws.save_if_dirty().unwrap();
		let before = ws.models()[0].version;
		let reopened = Workspace::open(&root).unwrap();
		assert_eq!(loaded(&reopened, before), Loaded::Gone);
	}
}
