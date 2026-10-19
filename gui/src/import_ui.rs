//! The import dialog: choose which folders, meshes and maps from an
//! export file to add to the workspace.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use eframe::egui::{self, RichText};

use crate::export::{Entry, Export, FolderTree};
use crate::maps::MapKind;
use crate::ui::icons;
use crate::ui::theme::{palette, semibold};
use crate::ui::widgets;
use crate::workspace::Workspace;

pub struct ImportDialog {
	pub source: PathBuf,
	entries: Vec<Entry>,
	skipped: Vec<String>,
	warnings: Vec<String>,
	tree: FolderTree,
	mesh: Vec<bool>,
	/// Whether to import each entry's maps.
	maps: Vec<bool>,
	/// Which kinds of map to import at all.
	kinds: BTreeMap<MapKind, bool>,
	/// Entries already in the workspace.
	existing: Vec<bool>,
	filter: String,
	download_now: bool,
}

pub enum Outcome {
	Cancel,
	Import {
		/// Selected entries, with `maps` narrowed to what will be downloaded.
		entries: Vec<Entry>,
		download_now: bool,
	},
}

impl ImportDialog {
	pub fn new(source: PathBuf, export: Export, workspace: &Workspace) -> Self {
		let in_ws: HashSet<(&[String], &str, u64)> = workspace
			.items()
			.iter()
			.map(|i| (i.folder.as_slice(), i.name.as_str(), i.mesh_id))
			.collect();
		let existing: Vec<bool> = export
			.entries
			.iter()
			.map(|e| in_ws.contains(&(e.folder.as_slice(), e.name.as_str(), e.mesh_id)))
			.collect();
		let tree = FolderTree::build(export.entries.iter().map(|e| e.folder.as_slice()));
		let n = export.entries.len();
		Self {
			source,
			tree,
			mesh: vec![false; n],
			maps: vec![true; n],
			kinds: MapKind::ALL.iter().map(|&k| (k, true)).collect(),
			existing,
			entries: export.entries,
			skipped: export.skipped,
			warnings: export.warnings,
			filter: String::new(),
			download_now: true,
		}
	}

	fn matches(&self, i: usize) -> bool {
		if self.filter.is_empty() {
			return true;
		}
		let needle = self.filter.to_lowercase();
		let e = &self.entries[i];
		e.name.to_lowercase().contains(&needle)
			|| e.folder.iter().any(|f| f.to_lowercase().contains(&needle))
			|| e.mesh_id.to_string() == needle
	}

	fn visible(&self, items: Vec<usize>) -> Vec<usize> {
		items.into_iter().filter(|&i| self.matches(i)).collect()
	}

	/// Returns `Some` once the user imports or cancels.
	pub fn show(&mut self, ctx: &egui::Context) -> Option<Outcome> {
		let mut outcome = None;
		let screen = ctx.content_rect().size();
		let size = egui::vec2(
			(screen.x - 80.0).clamp(420.0, 820.0),
			(screen.y - 100.0).clamp(360.0, 680.0),
		);
		let file = self
			.source
			.file_name()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_default();
		let modal = egui::Modal::new(egui::Id::unique("import_dialog")).show(ctx, |ui| {
			ui.set_width(size.x);
			ui.set_height(size.y);
			ui.horizontal(|ui| {
				ui.label(
					RichText::new(icons::FILE_ARROW_DOWN)
						.size(20.0)
						.color(palette(ui.ctx()).accent),
				);
				ui.label(
					RichText::new(format!("Import {file}"))
						.size(17.0)
						.family(semibold()),
				);
			});
			ui.add_space(4.0);
			outcome = self.contents(ui);
		});
		if outcome.is_none() && modal.should_close() {
			outcome = Some(Outcome::Cancel);
		}
		outcome
	}

	/// The maps that will be imported for entry `i`.
	fn chosen_maps(&self, i: usize) -> BTreeMap<MapKind, u64> {
		if !self.maps[i] {
			return BTreeMap::new();
		}
		self.entries[i]
			.maps
			.iter()
			.filter(|(k, _)| self.kinds[k])
			.map(|(&k, &id)| (k, id))
			.collect()
	}

	fn contents(&mut self, ui: &mut egui::Ui) -> Option<Outcome> {
		let count = |kind| {
			self.entries
				.iter()
				.filter(|e| e.maps.contains_key(&kind))
				.count()
		};
		let counts: BTreeMap<MapKind, usize> =
			MapKind::ALL.iter().map(|&k| (k, count(k))).collect();
		let with_maps = self.entries.iter().filter(|e| !e.maps.is_empty()).count();
		ui.label(format!(
			"{} models ({} with maps) in {} top-level folders.",
			self.entries.len(),
			with_maps,
			self.tree.folders.len()
		));
		for (title, list) in [
			("entries skipped", &self.skipped),
			("map ids ignored", &self.warnings),
		] {
			if !list.is_empty() {
				ui.collapsing(format!("{} {title}", list.len()), |ui| {
					for s in list {
						ui.label(s);
					}
				});
			}
		}
		ui.horizontal(|ui| {
			ui.label("Search:");
			ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("name, folder or id"));
			if !self.filter.is_empty() && ui.small_button("✕").clicked() {
				self.filter.clear();
			}
		});
		ui.horizontal(|ui| {
			let visible = self.visible((0..self.entries.len()).collect());
			let scope = if self.filter.is_empty() {
				"all"
			} else {
				"matching"
			};
			if ui.button(format!("Select {scope}")).clicked() {
				for &i in &visible {
					self.mesh[i] = true;
				}
			}
			if ui.button(format!("Deselect {scope}")).clicked() {
				for &i in &visible {
					self.mesh[i] = false;
				}
			}
			ui.separator();
			ui.label("Maps:");
			if ui.small_button("all").clicked() {
				for &i in &visible {
					self.maps[i] = true;
				}
			}
			if ui.small_button("none").clicked() {
				for &i in &visible {
					self.maps[i] = false;
				}
			}
		});
		if with_maps > 0 {
			ui.horizontal_wrapped(|ui| {
				ui.label("Download map types:");
				for kind in MapKind::ALL {
					let n = counts[&kind];
					if n == 0 {
						continue;
					}
					let enabled = self.kinds.get_mut(&kind).expect("all kinds present");
					ui.checkbox(enabled, format!("{} ({n})", kind.label()))
						.on_hover_text(format!("The {} field", kind.export_field()));
				}
			});
		}
		ui.separator();

		let footer_height = 64.0;
		egui::ScrollArea::vertical()
			.auto_shrink([false, false])
			.max_height(ui.available_height() - footer_height)
			.show(ui, |ui| {
				let tree = std::mem::take(&mut self.tree);
				self.folder_contents(ui, &tree, "root");
				self.tree = tree;
			});

		ui.separator();
		let selected: Vec<usize> = (0..self.entries.len()).filter(|&i| self.mesh[i]).collect();
		let chosen: Vec<BTreeMap<MapKind, u64>> =
			selected.iter().map(|&i| self.chosen_maps(i)).collect();
		let map_count: usize = chosen.iter().map(BTreeMap::len).sum();
		let unique: HashSet<u64> = selected
			.iter()
			.zip(&chosen)
			.flat_map(|(&i, maps)| {
				std::iter::once(self.entries[i].mesh_id).chain(maps.values().copied())
			})
			.collect();
		let already = selected.iter().filter(|&&i| self.existing[i]).count();
		ui.label(format!(
			"Selected {} meshes and {} maps: {} unique assets to download.{}",
			selected.len(),
			map_count,
			unique.len(),
			if already > 0 {
				format!(" {already} are already in this workspace.")
			} else {
				String::new()
			}
		));
		let mut outcome = None;
		ui.horizontal(|ui| {
			ui.checkbox(&mut self.download_now, "Start downloading now");
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				let import = ui
					.add_enabled_ui(!selected.is_empty(), |ui| {
						widgets::primary_button(ui, format!("Import {}", selected.len()))
					})
					.inner;
				if import.clicked() {
					outcome = Some(Outcome::Import {
						entries: selected
							.iter()
							.zip(chosen)
							.map(|(&i, maps)| Entry {
								maps,
								..self.entries[i].clone()
							})
							.collect(),
						download_now: self.download_now,
					});
				}
				if ui.button("Cancel").clicked() {
					outcome = Some(Outcome::Cancel);
				}
			});
		});
		outcome
	}

	fn folder_contents(&mut self, ui: &mut egui::Ui, node: &FolderTree, id_path: &str) {
		for (name, child) in node.sorted_folders() {
			let items = self.visible(child.all_items());
			if items.is_empty() {
				continue;
			}
			let selected = items.iter().filter(|&&i| self.mesh[i]).count();
			let id = ui.make_persistent_id(format!("{id_path}/{name}"));
			let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
				ui.ctx(),
				id,
				false,
			);
			// searching expands everything so matches are visible
			if !self.filter.is_empty() {
				state.set_open(true);
			}
			let path = format!("{id_path}/{name}");
			state
				.show_header(ui, |ui| {
					let mut all = selected == items.len();
					let label = format!(
						"{}  {name}  ({selected}/{})",
						icons::FOLDER_SIMPLE,
						items.len()
					);
					let response = ui.add(
						egui::Checkbox::new(&mut all, label)
							.indeterminate(selected > 0 && selected < items.len()),
					);
					if response.changed() {
						for &i in &items {
							self.mesh[i] = all;
						}
					}
				})
				.body(|ui| self.folder_contents(ui, child, &path));
		}
		for &i in &node.items {
			if !self.matches(i) {
				continue;
			}
			ui.horizontal(|ui| {
				let e = &self.entries[i];
				let mut label = RichText::new(&e.name);
				if self.existing[i] {
					label = label.color(ui.visuals().weak_text_color());
				}
				ui.checkbox(&mut self.mesh[i], label)
					.on_hover_text(format!("Mesh id {}", e.mesh_id));
				if !e.maps.is_empty() {
					let hover = e
						.maps
						.iter()
						.map(|(k, id)| {
							let skipped = if self.kinds[k] {
								""
							} else {
								" (type not selected)"
							};
							format!("{}: {id}{skipped}", k.export_field())
						})
						.collect::<Vec<_>>()
						.join("\n");
					let label = format!(
						"{}  {} map{}",
						icons::IMAGE,
						e.maps.len(),
						if e.maps.len() == 1 { "" } else { "s" }
					);
					ui.add_enabled(self.mesh[i], egui::Checkbox::new(&mut self.maps[i], label))
						.on_hover_text(hover);
				}
				if self.existing[i] {
					ui.label(RichText::new("in workspace").small().weak());
				}
			});
		}
	}
}
