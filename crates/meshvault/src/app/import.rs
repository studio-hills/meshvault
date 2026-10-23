//! Importing exports: picked files, and exports sent from Roblox Studio.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use super::App;
use super::task::Task;
use crate::studio::{export_file, link};
use crate::ui::format::format_bytes;
use crate::ui::import::{self, ImportDialog};

/// An export being read on a background thread.
pub struct Parsing {
	pub path: PathBuf,
	pub task: Task<export_file::Export>,
}

/// Most exports waiting to be read. Each is a file on disk (exports from
/// Studio are saved first), so waiting costs no memory.
const MAX_WAITING: usize = 8;

/// Exports are read one at a time: one that arrives while another is read
/// or shown waits here, and is read from its file when its turn comes.
/// Each waits for the workspace that was open when it arrived, and is
/// dropped if another is open by its turn.
#[derive(Default)]
pub struct ImportQueue {
	/// Each export's file, and the root of the workspace it is for.
	waiting: VecDeque<(PathBuf, PathBuf)>,
	/// Held by the thread reading an export, until it ends. A reader that
	/// was cancelled still holds it, so the next one waits for it.
	reader: Arc<()>,
}

impl ImportQueue {
	/// Whether an export reader thread is still running.
	fn reading(&self) -> bool {
		Arc::strong_count(&self.reader) > 1
	}

	/// Puts `path`, for the workspace at `workspace`, at the back; false if
	/// the queue is full.
	fn push(&mut self, path: PathBuf, workspace: &Path) -> bool {
		if self.waiting.len() >= MAX_WAITING {
			return false;
		}
		self.waiting.push_back((path, workspace.to_path_buf()));
		true
	}

	/// Drops every waiting export, returning how many there were. A reader
	/// still running keeps the queue busy until it ends.
	pub(super) fn discard_waiting(&mut self) -> usize {
		let dropped = self.waiting.len();
		self.waiting.clear();
		dropped
	}

	/// Drops the exports waiting for another workspace than `current`, then
	/// takes the next one unless `busy`. Returns how many were dropped, and
	/// the export to read.
	fn next(&mut self, current: Option<&Path>, busy: bool) -> (usize, Option<PathBuf>) {
		let before = self.waiting.len();
		self.waiting.retain(|(_, ws)| Some(ws.as_path()) == current);
		let dropped = before - self.waiting.len();
		let next = if busy {
			None
		} else {
			self.waiting.pop_front().map(|(path, _)| path)
		};
		(dropped, next)
	}
}

impl App {
	pub fn pick_import(&mut self) {
		if let Some(path) = rfd::FileDialog::new()
			.set_title("Import export JSON")
			.add_filter("Export JSON", &["json"])
			.pick_file()
			&& let Some(bytes) = self.read_or_report(&path)
		{
			self.start_import(path, &bytes);
		}
	}

	pub fn start_import(&mut self, path: PathBuf, bytes: &[u8]) {
		if self.workspace.is_none() {
			// make a workspace first, then come back to this export
			self.show_new_workspace(Some(path));
			return;
		}
		if bytes.len() > export_file::MAX_EXPORT_BYTES {
			let why = export_file::too_big(bytes.len());
			self.toasts.error(format!("{}: {why}", path.display()));
			return;
		}
		if self.import_busy() {
			self.queue_import(path);
			return;
		}
		// schema 1 exports with animations run to hundreds of megabytes
		let bytes = bytes.to_vec();
		let reader = Arc::clone(&self.import_queue.reader);
		let task = Task::spawn(&self.ctx, move |_| {
			let _reader = reader;
			let result = export_file::parse(&String::from_utf8_lossy(&bytes));
			drop(bytes);
			result
		});
		self.parsing = Some(Parsing { path, task });
	}

	/// Whether an export is being read or shown for import.
	fn import_busy(&self) -> bool {
		self.parsing.is_some() || self.import.is_some() || self.import_queue.reading()
	}

	/// Keeps `path` to read once the export being read or shown is done.
	fn queue_import(&mut self, path: PathBuf) {
		let Some(ws) = &self.workspace else { return };
		let name = path.display();
		if self.import_queue.push(path.clone(), &ws.root) {
			self.toasts.info(format!(
				"{name} opens for import when the current one is done ({} waiting).",
				self.import_queue.waiting.len()
			));
		} else {
			self.toasts.error(format!(
				"Not importing {name}: {MAX_WAITING} exports are already waiting. Import it again later."
			));
		}
	}

	/// Starts the next waiting export once nothing else is being read or
	/// shown.
	fn next_import(&mut self) {
		if self.import_queue.waiting.is_empty() {
			return;
		}
		let busy = self.import_busy();
		let current = self.workspace.as_ref().map(|ws| ws.root.as_path());
		let (dropped, next) = self.import_queue.next(current, busy);
		self.report_dropped(dropped);
		let Some(path) = next else { return };
		if let Some(bytes) = self.read_or_report(&path) {
			self.start_import(path, &bytes);
		}
	}

	/// Says that `dropped` waiting exports were not imported, if any were.
	pub(super) fn report_dropped(&mut self, dropped: usize) {
		match dropped {
			0 => {}
			1 => self
				.toasts
				.info("A waiting export was not imported: its workspace was closed."),
			n => self.toasts.info(format!(
				"{n} waiting exports were not imported: their workspace was closed."
			)),
		}
	}

	pub(super) fn poll_parsing(&mut self) {
		self.next_import();
		let Some(parsing) = &self.parsing else { return };
		let Some(result) = parsing.task.poll("The export reader stopped unexpectedly") else {
			return;
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
			origin,
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
			let (stem, new) = ws.add_entry(entry, &origin);
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
			match ws.add_model(model, &meshes, &origin) {
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

	/// Saves exports that arrived from Studio and opens them for import.
	pub(super) fn poll_studio_link(&mut self) {
		for event in self.studio_link.poll() {
			let received = match event {
				link::Event::Progress {
					export,
					done,
					count,
				} => {
					self.studio_progress = Some((export, done, count, Instant::now()));
					continue;
				}
				link::Event::Received(received) => received,
			};
			// another export may still be arriving
			if self
				.studio_progress
				.as_ref()
				.is_some_and(|(export, ..)| *export == received.export)
			{
				self.studio_progress = None;
			}
			// the export is here, so the instructions are no longer needed
			self.studio_dialog = None;
			let path = received.path.clone();
			match received.save() {
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
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The queue holds a few exports, and a reader counts as running until
	/// its thread ends, even after its task was dropped (cancelled).
	#[test]
	fn the_queue_is_bounded_and_knows_its_reader() {
		let mut queue = ImportQueue::default();
		for i in 0..MAX_WAITING {
			assert!(queue.push(PathBuf::from(format!("{i}.json")), Path::new("A")));
		}
		assert!(!queue.push(PathBuf::from("one more.json"), Path::new("A")));
		assert_eq!(
			queue.next(Some(Path::new("A")), false).1,
			Some(PathBuf::from("0.json"))
		);

		assert!(!queue.reading());
		let ctx = eframe::egui::Context::default();
		let (go, wait) = std::sync::mpsc::channel::<()>();
		let reader = Arc::clone(&queue.reader);
		let task = Task::spawn(&ctx, move |_| {
			let _reader = reader;
			wait.recv().map_err(|e| e.to_string())
		});
		drop(task);
		assert!(queue.reading(), "a cancelled reader still runs");
		go.send(()).unwrap();
		for _ in 0..500 {
			if !queue.reading() {
				return;
			}
			std::thread::sleep(std::time::Duration::from_millis(10));
		}
		panic!("the reader did not end");
	}
	/// A reader of workspace A's export, cancelled, that runs until `go`.
	fn running_reader(queue: &ImportQueue) -> std::sync::mpsc::Sender<()> {
		let ctx = eframe::egui::Context::default();
		let (go, wait) = std::sync::mpsc::channel::<()>();
		let reader = Arc::clone(&queue.reader);
		drop(Task::spawn(&ctx, move |_| {
			let _reader = reader;
			wait.recv().map_err(|e| e.to_string())
		}));
		go
	}

	fn wait_for_reader(queue: &ImportQueue) {
		for _ in 0..500 {
			if !queue.reading() {
				return;
			}
			std::thread::sleep(std::time::Duration::from_millis(10));
		}
		panic!("the reader did not end");
	}

	/// Exports waiting while workspace A's export is read are not offered
	/// to workspace B, whether B was opened straight from A or A was closed
	/// first; A's reader still keeps the queue busy until it ends.
	#[test]
	fn switching_workspaces_drops_the_waiting_exports() {
		let (a, b) = (Path::new("A"), Path::new("B"));
		// opened straight from A: B's turn comes with A's exports still there
		let mut queue = ImportQueue::default();
		let go = running_reader(&queue);
		assert!(queue.push("a1.json".into(), a));
		assert!(queue.push("a2.json".into(), a));
		assert_eq!(queue.next(Some(b), queue.reading()), (2, None));
		assert!(queue.waiting.is_empty());
		assert!(queue.reading(), "A's reader still runs");
		// B's own export waits for that reader, then is read
		assert!(queue.push("b1.json".into(), b));
		assert_eq!(queue.next(Some(b), queue.reading()), (0, None));
		go.send(()).unwrap();
		wait_for_reader(&queue);
		assert_eq!(
			queue.next(Some(b), queue.reading()),
			(0, Some(PathBuf::from("b1.json")))
		);

		// closing A (as opening or creating B does first) drops them at once
		let go = running_reader(&queue);
		assert!(queue.push("a3.json".into(), a));
		assert_eq!(queue.discard_waiting(), 1);
		assert!(queue.reading());
		assert_eq!(queue.next(Some(a), queue.reading()), (0, None));
		go.send(()).unwrap();
		wait_for_reader(&queue);
		// with no workspace open, nothing waits
		assert!(queue.push("a4.json".into(), a));
		assert_eq!(queue.next(None, false), (1, None));
	}
}
