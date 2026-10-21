//! Starting workspace downloads and applying their results.

use std::time::Duration;

use super::App;
use crate::ui::settings::{self, SettingsDialog};
use crate::workspace::download::Event;

/// How often the workspace is saved while downloads are running.
const DOWNLOAD_SAVE_INTERVAL: Duration = Duration::from_secs(2);

impl App {
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
		let client = match settings::client_from_config(&self.config) {
			Ok(Some(client)) => client,
			Ok(None) => {
				self.open_settings(Some("Add a Roblox credential to start downloading.".into()));
				return;
			}
			Err(e) => {
				self.toasts.error(e);
				return;
			}
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

	pub(super) fn poll_downloads(&mut self) {
		let root = self.workspace.as_ref().map(|ws| ws.root.as_path());
		let events = self.downloader.poll(root);
		let had_events = !events.is_empty();
		let mut finished = false;
		// a rejected credential has had its own toast
		let mut summary = true;
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
				Event::Stopped { auth_failed } => {
					finished = true;
					summary = !auth_failed;
				}
			}
		}
		// save progress regularly, and once everything has finished
		if finished || (had_events && self.last_save.elapsed() > DOWNLOAD_SAVE_INTERVAL) {
			self.save_workspace();
		}
		if finished && summary {
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
}
