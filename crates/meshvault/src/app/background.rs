//! Work on background threads that ends with a message to show.

use eframe::egui;

use super::App;

impl App {
	/// Runs `work` on a background thread; its message is shown when it ends.
	pub(super) fn in_background(
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
	pub(super) fn poll_background(&mut self) {
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
}
