//! One-shot work on a background thread whose result the app polls for
//! once a frame.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use eframe::egui;

/// Work running on its own thread that ends with one result.
///
/// Dropping a task does not stop its thread: the work runs to the end and
/// its result is thrown away.
pub struct Task<T> {
	rx: Receiver<Result<T, String>>,
	/// When the work started, for progress dialogs.
	pub started: Instant,
}

impl<T: Send + 'static> Task<T> {
	/// Runs `work` on a new thread, then repaints so the result is seen.
	pub fn spawn(
		ctx: &egui::Context,
		work: impl FnOnce(&egui::Context) -> Result<T, String> + Send + 'static,
	) -> Self {
		let (tx, rx) = channel();
		let ctx = ctx.clone();
		std::thread::spawn(move || {
			let _ = tx.send(work(&ctx));
			ctx.request_repaint();
		});
		Self {
			rx,
			started: Instant::now(),
		}
	}

	/// The result once the work has ended, or `stopped` as the error if its
	/// thread ended without one. Drop the task after its first result: a
	/// thread that has ended reports `stopped` on every later poll.
	pub fn poll(&self, stopped: &str) -> Option<Result<T, String>> {
		match self.rx.try_recv() {
			Ok(result) => Some(result),
			Err(TryRecvError::Empty) => None,
			Err(TryRecvError::Disconnected) => Some(Err(stopped.into())),
		}
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;

	/// Polls until the task has a result, for at most a few seconds.
	fn wait<T: Send + 'static>(task: &Task<T>, stopped: &str) -> Result<T, String> {
		for _ in 0..500 {
			if let Some(result) = task.poll(stopped) {
				return result;
			}
			std::thread::sleep(Duration::from_millis(10));
		}
		panic!("the task did not finish");
	}

	#[test]
	fn a_task_gives_its_result() {
		let ctx = egui::Context::default();
		let task = Task::spawn(&ctx, |_| Ok(7));
		assert_eq!(wait(&task, "stopped"), Ok(7));
		let task: Task<u8> = Task::spawn(&ctx, |_| Err("failed".into()));
		assert_eq!(wait(&task, "stopped"), Err("failed".into()));
	}

	#[test]
	fn a_task_whose_thread_panics_has_stopped() {
		let ctx = egui::Context::default();
		let task: Task<u8> = Task::spawn(&ctx, |_| panic!("the work failed"));
		assert_eq!(wait(&task, "It stopped."), Err("It stopped.".into()));
	}
}
