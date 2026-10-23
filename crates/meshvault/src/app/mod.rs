//! Application state and the actions the UI triggers. Drawing lives in `ui`.

mod background;
mod capture;
mod document;
mod downloads;
mod export;
mod import;
mod task;
mod workspace;

use std::sync::Arc;
use std::time::Instant;

use eframe::egui;

use crate::config::Config;
use crate::render::camera::Camera;
use crate::render::gpu::GpuRenderer;
use crate::render::lighting::Lighting;
use crate::render::viewport::Shading;
use crate::scene::model_view::ModelView;
use crate::scene::texture::Material;
use crate::studio::link::{self, StudioLink};
use crate::ui::import::ImportDialog;
use crate::ui::settings::SettingsDialog;
use crate::ui::theme;
use crate::ui::{self, Confirm, NewWorkspaceDialog, StudioDialog, Toasts};
use crate::update::{self, Release};
use crate::workspace::download::Downloader;
use crate::workspace::{MANIFEST, Workspace};

pub use document::Document;
pub use import::Parsing;
use task::Task;
pub use workspace::LoadingModel;

/// The 3D view's size before it is first drawn. Its shape is also the
/// fallback for saved images when the view has no usable size.
const DEFAULT_VIEW_SIZE: egui::Vec2 = egui::vec2(16.0, 9.0);

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
	pub browser: ui::library::Browser,
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
	pub studio_link: StudioLink,
	pub studio_dialog: Option<StudioDialog>,
	/// The export from Studio that last had a piece arrive (its id), its
	/// pieces received so far, out of how many, and when.
	pub studio_progress: Option<(String, usize, usize, Instant)>,
	/// Work on background threads (exports, saved images), each ending
	/// with a message to show.
	background: Vec<Task<String>>,
	/// The check for a newer release, while it runs.
	update_check: Option<Task<Option<Release>>>,
	/// A newer release than this build, once the check has found one.
	pub update: Option<Release>,
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
		let (config, config_problems) = Config::load();
		theme::install_fonts(&ctx);
		theme::apply(&ctx, config.theme);
		let studio_link = StudioLink::start(&ctx);
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
			browser: ui::library::Browser::default(),
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
			update_check: None,
			update: None,
			viewport: egui::Rect::from_min_size(egui::Pos2::ZERO, DEFAULT_VIEW_SIZE),
			last_save: Instant::now(),
		};
		for problem in config_problems {
			app.toasts.error(problem);
		}
		if let link::Status::Unavailable(why) = &app.studio_link.status {
			app.toasts.warning(ui::unavailable_text(why));
		}
		if app.config.check_for_updates {
			app.update_check = Some(Task::spawn(&app.ctx, |_| {
				update::check(update::LATEST_RELEASE_URL)
			}));
		}
		// reopen the last workspace
		if let Some(root) = app.config.recent_workspaces.first().cloned()
			&& root.join(MANIFEST).exists()
		{
			app.open_workspace(&root);
		}
		app
	}

	pub fn set_theme(&mut self, mode: theme::ThemeMode) {
		self.config.theme = mode;
		if let Err(e) = self.config.save() {
			self.toasts.error(e);
		}
		theme::apply(&self.ctx, mode);
	}

	/// Takes in what background work finished since the last frame. Runs
	/// once a frame, before anything is drawn.
	pub fn poll(&mut self) {
		self.poll_downloads();
		self.poll_parsing();
		self.poll_model();
		self.poll_background();
		self.poll_studio_link();
		self.poll_update();
	}

	/// A failed check is not worth interrupting anyone for; the next start
	/// tries again.
	fn poll_update(&mut self) {
		let Some(result) = self.update_check.as_ref().and_then(|t| t.poll("")) else {
			return;
		};
		self.update_check = None;
		if let Ok(Some(release)) = result {
			self.toasts.info(format!(
				"Meshvault {} is available. Click Update in the status bar to see what's new.",
				release.version
			));
			self.update = Some(release);
		}
	}

	pub fn set_check_for_updates(&mut self, check: bool) {
		self.config.check_for_updates = check;
		if let Err(e) = self.config.save() {
			self.toasts.error(e);
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

	pub fn set_lighting(&mut self, lighting: Lighting) {
		self.config.lighting = lighting;
		if let Err(e) = self.config.save() {
			self.toasts.error(e);
		}
	}
}
