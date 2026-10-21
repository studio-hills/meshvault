//! Importing exports: picked files, and exports sent from Roblox Studio.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use super::App;
use super::task::Task;
use crate::studio::export_file;
use crate::ui::format::format_bytes;
use crate::ui::import::{self, ImportDialog};

/// An export being read on a background thread.
pub struct Parsing {
	pub path: PathBuf,
	pub task: Task<export_file::Export>,
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
		// schema 1 exports with animations run to hundreds of megabytes
		let bytes = bytes.to_vec();
		let task = Task::spawn(&self.ctx, move |_| {
			let result = export_file::parse(&String::from_utf8_lossy(&bytes));
			drop(bytes);
			result
		});
		self.parsing = Some(Parsing { path, task });
	}

	pub(super) fn poll_parsing(&mut self) {
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

	/// Saves exports that arrived from Studio and opens them for import.
	pub(super) fn poll_studio_link(&mut self) {
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
}
