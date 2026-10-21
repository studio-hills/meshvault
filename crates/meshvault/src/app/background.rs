//! Work on background threads that ends with a message to show.

use eframe::egui;

use super::App;
use super::task::Task;

impl App {
	/// Runs `work` on a background thread; its message is shown when it ends.
	pub(super) fn in_background(
		&mut self,
		work: impl FnOnce(&egui::Context) -> Result<String, String> + Send + 'static,
	) {
		self.background.push(Task::spawn(&self.ctx, work));
	}

	/// Shows the messages of background work that ended.
	pub(super) fn poll_background(&mut self) {
		let mut done = Vec::new();
		self.background.retain(
			|task| match task.poll("Background work stopped unexpectedly.") {
				Some(result) => {
					done.push(result);
					false
				}
				None => true,
			},
		);
		for result in done {
			match result {
				Ok(message) => self.toasts.success(message),
				Err(message) => self.toasts.error(message),
			}
		}
	}
}
