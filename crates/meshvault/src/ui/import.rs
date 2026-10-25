//! The import dialog: choose what to add to the workspace from an export.
//!
//! Everything is shown in one folder tree. A model, animated (with clips) or
//! static, sits in its folder with the meshes it uses listed under it;
//! ticking the model brings them along. Meshes no model uses are listed on
//! their own. Which kinds of map to download is a single choice for the
//! whole import.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use eframe::egui::{self, Align, Layout, RichText};

use crate::anim::schema::AnimatedModel;
use crate::scene::maps::MapKind;
use crate::studio::export_file::{Entry, Export, Origin};
use crate::ui::format::plural;
use crate::ui::icons;
use crate::ui::library::{folder_matches, mesh_name_or_id_matches};
use crate::ui::theme::{ROW, palette};
use crate::ui::tree::{self, FlatKind, FlatTree, FolderCommand, FolderTree};
use crate::ui::widgets;
use crate::workspace::{Workspace, model_folder};

/// One line of the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
	/// A model, by index into `models`.
	Model(usize),
	/// A mesh no model uses, by index into `entries`.
	Mesh(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KindFilter {
	All,
	/// Models with clips.
	Animated,
	/// Models without.
	Static,
	/// Meshes no model uses.
	Meshes,
}

/// `parts` as a list: "a", "a and b", "a, b and c".
fn and_list(parts: &[String]) -> String {
	match parts {
		[] => String::new(),
		[one] => one.clone(),
		[rest @ .., last] => format!("{} and {last}", rest.join(", ")),
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum TreeLine {
	Row(usize),
	Mesh {
		entry: usize,
		depth: usize,
		model: usize,
	},
	Note {
		model: usize,
		depth: usize,
	},
}

#[derive(Default)]
struct TreeCache {
	filter: Option<(String, KindFilter)>,
	revision: Option<u64>,
	matches: Vec<bool>,
	totals: Vec<usize>,
	selected: Vec<usize>,
	visible: Vec<TreeLine>,
}

struct ModelInfo {
	missing: usize,
	playable: usize,
	hover: String,
}

struct ImportSelection {
	entries: Vec<usize>,
	models: Vec<usize>,
	summary: String,
	already: usize,
}

pub struct ImportDialog {
	source: PathBuf,
	entries: Vec<Entry>,
	/// Entries already in the workspace.
	existing: Vec<bool>,
	/// The place the export came from.
	origin: Origin,
	models: Vec<AnimatedModel>,
	/// Models already in the workspace (see `Workspace::find_model`).
	model_existing: Vec<bool>,
	/// Entry indices of the meshes each model uses.
	model_meshes: Vec<Vec<usize>>,
	rows: Vec<Row>,
	selected: Vec<bool>,
	/// The tree over `rows`.
	tree: FolderTree,
	flat: FlatTree,
	cache: TreeCache,
	counts: (usize, usize, usize),
	map_counts: BTreeMap<MapKind, usize>,
	model_info: Vec<ModelInfo>,
	selection_cache: Option<ImportSelection>,
	/// Which kinds of map to download.
	kinds: BTreeMap<MapKind, bool>,
	show: KindFilter,
	skipped: Vec<String>,
	warnings: Vec<String>,
	search: String,
	/// `search` lowercased, for matching this frame's rows.
	needle: String,
	download_now: bool,
	/// Expand/collapse requested while drawing, applied on the next frame.
	pending: Option<FolderCommand>,
	/// Set until the first draw, which opens lone top folders.
	first_frame: bool,
}

/// Folder state namespace for the import tree.
const SCOPE: &str = "import";

fn model_id(i: usize) -> egui::Id {
	egui::Id::unique(("import_model", i))
}

/// Height kept below the list for the summary, note and buttons.
const FOOTER_HEIGHT: f32 = 92.0;
/// Space kept for the footer's "already in this workspace" note when it is
/// not shown, so the footer does not jump when it comes and goes.
const NOTE_RESERVE: f32 = 15.0;
/// Indent that lines a row up with the text of a checkbox above it.
const CHECKBOX_INDENT: f32 = 22.0;

pub enum Outcome {
	Cancel,
	Import {
		/// The place the export came from.
		origin: Origin,
		/// Selected entries, with `maps` narrowed to what will be downloaded.
		entries: Vec<Entry>,
		/// Selected models; their meshes are among `entries`.
		models: Vec<AnimatedModel>,
		download_now: bool,
	},
}

impl ImportDialog {
	pub fn new(source: PathBuf, export: Export, workspace: &Workspace) -> Self {
		// what the workspace already has, as importing will find it
		let origin = &export.origin;
		let existing: Vec<bool> = export
			.entries
			.iter()
			.map(|e| workspace.find_entry(e, origin).is_some())
			.collect();
		let model_existing = export
			.models
			.iter()
			.map(|m| workspace.find_model(&m.path, origin).is_some())
			.collect();

		let by_source: HashMap<u32, usize> = export
			.entries
			.iter()
			.enumerate()
			.filter_map(|(i, e)| Some((e.source_id?, i)))
			.collect();
		let model_meshes: Vec<Vec<usize>> = export
			.models
			.iter()
			.map(|m| {
				let mut used: Vec<usize> = m
					.parts
					.iter()
					.filter_map(|p| by_source.get(&p.mesh_ref).copied())
					.collect();
				used.sort_unstable();
				used.dedup();
				used
			})
			.collect();
		let used: HashSet<usize> = model_meshes.iter().flatten().copied().collect();

		// models live in the folder holding them; meshes in their export folder
		let mut rows = Vec::new();
		let mut folders: Vec<Vec<String>> = Vec::new();
		for (i, m) in export.models.iter().enumerate() {
			rows.push(Row::Model(i));
			folders.push(model_folder(origin, &m.path));
		}
		for (i, e) in export.entries.iter().enumerate() {
			if !used.contains(&i) {
				rows.push(Row::Mesh(i));
				folders.push(e.folder.clone());
			}
		}
		let tree = FolderTree::build(folders.iter().map(Vec::as_slice));
		let mut dialog = Self {
			source,
			selected: vec![false; rows.len()],
			rows,
			tree,
			flat: FlatTree::default(),
			cache: TreeCache::default(),
			counts: (0, 0, 0),
			map_counts: BTreeMap::new(),
			model_info: Vec::new(),
			selection_cache: None,
			kinds: MapKind::ALL.iter().map(|&k| (k, true)).collect(),
			show: KindFilter::All,
			existing,
			model_existing,
			model_meshes,
			origin: export.origin,
			models: export.models,
			entries: export.entries,
			skipped: export.skipped,
			warnings: export.warnings,
			search: String::new(),
			needle: String::new(),
			download_now: true,
			pending: None,
			first_frame: true,
		};
		dialog.flat = FlatTree::build(&dialog.tree, SCOPE, |a, b| {
			let mesh = |r: usize| matches!(dialog.rows[r], Row::Mesh(_));
			mesh(a).cmp(&mesh(b)).then_with(|| {
				tree::natural_cmp(
					dialog.row_name(dialog.rows[a]),
					dialog.row_name(dialog.rows[b]),
				)
			})
		});
		for &row in &dialog.rows {
			match dialog.kind(row) {
				KindFilter::Animated => dialog.counts.0 += 1,
				KindFilter::Static => dialog.counts.1 += 1,
				KindFilter::Meshes => dialog.counts.2 += 1,
				KindFilter::All => unreachable!(),
			}
		}
		dialog.map_counts = MapKind::ALL
			.into_iter()
			.map(|k| {
				(
					k,
					dialog
						.entries
						.iter()
						.filter(|e| e.maps.contains_key(&k))
						.count(),
				)
			})
			.collect();
		dialog.model_info = dialog
			.models
			.iter()
			.enumerate()
			.map(|(i, m)| {
				let meshes = dialog.model_meshes[i].len();
				let missing = m
					.parts
					.iter()
					.filter(|p| p.mesh_ref != 0)
					.map(|p| p.mesh_ref)
					.collect::<HashSet<_>>()
					.len()
					.saturating_sub(meshes);
				let playable = m
					.animations
					.iter()
					.filter(|a| a.status.is_playable())
					.count();
				let clips = m.animations.len();
				let mut hover = format!(
					"{}\n{} ? {}",
					m.path,
					plural(m.parts.len(), "part", "parts"),
					plural(meshes, "mesh", "meshes")
				);
				if clips > 0 {
					hover.push_str(" ? ");
					hover.push_str(&if playable == clips {
						plural(clips, "clip", "clips")
					} else {
						format!("{playable} of {clips} clips playable")
					});
				}
				ModelInfo {
					missing,
					playable,
					hover,
				}
			})
			.collect();
		dialog
	}

	/// Whether model `i` has clips; the rest are static.
	fn is_animated(&self, i: usize) -> bool {
		!self.models[i].animations.is_empty()
	}

	/// The kind of a row, as the filter sorts them.
	fn kind(&self, row: Row) -> KindFilter {
		match row {
			Row::Model(i) if self.is_animated(i) => KindFilter::Animated,
			Row::Model(_) => KindFilter::Static,
			Row::Mesh(_) => KindFilter::Meshes,
		}
	}

	/// How many rows there are of each kind: animated models, static
	/// models, meshes no model uses.
	fn counts(&self) -> (usize, usize, usize) {
		self.counts
	}

	/// The line above the list: what the export holds.
	fn summary_text(&self) -> String {
		let (animated, still, meshes) = self.counts();
		if animated + still == 0 {
			return format!(
				"{} in {}",
				plural(self.entries.len(), "mesh", "meshes"),
				plural(self.tree.folders.len(), "folder", "folders")
			);
		}
		let mut parts = Vec::new();
		if animated > 0 {
			parts.push(plural(animated, "animated model", "animated models"));
		}
		if still > 0 {
			parts.push(plural(still, "static model", "static models"));
		}
		parts.push(plural(meshes, "other mesh", "other meshes"));
		format!(
			"{}. Ticking a model brings the meshes it uses.",
			and_list(&parts)
		)
	}

	fn row_name(&self, row: Row) -> &str {
		match row {
			Row::Model(i) => &self.models[i].name,
			Row::Mesh(i) => &self.entries[i].name,
		}
	}

	fn matches(&self, r: usize) -> bool {
		let row = self.rows[r];
		if self.show != KindFilter::All && self.show != self.kind(row) {
			return false;
		}
		if self.search.is_empty() {
			return true;
		}
		let needle = &self.needle;
		let mesh_matches = |e: &Entry| mesh_name_or_id_matches(needle, &e.name, e.mesh_id);
		match row {
			// a model's own meshes by name or id only, not by their folders
			Row::Model(i) => {
				let m = &self.models[i];
				m.path.to_lowercase().contains(needle)
					|| self.model_meshes[i]
						.iter()
						.any(|&e| mesh_matches(&self.entries[e]))
			}
			Row::Mesh(i) => {
				let e = &self.entries[i];
				mesh_matches(e) || folder_matches(needle, &e.folder)
			}
		}
	}

	/// Rebuild matching and visible rows only after a filter or stored open
	/// state changes. Selection totals are separate: checking a row does
	/// not change the shape of the tree.
	fn refresh_tree(&mut self, ctx: &egui::Context) {
		let filter = (self.needle.clone(), self.show);
		if self.cache.filter.as_ref() != Some(&filter) {
			self.cache.matches = (0..self.rows.len()).map(|r| self.matches(r)).collect();
			self.cache.totals = self.flat.totals(|r| self.cache.matches[r] as usize);
			self.cache.selected.clear();
			self.cache.filter = Some(filter);
			self.cache.revision = None;
		}
		self.refresh_selection();
		if self.cache.revision != Some(tree::revision(ctx)) {
			let visible = self.flat.visible(
				ctx,
				&self.cache.matches,
				&self.cache.totals,
				!self.search.is_empty(),
			);
			self.cache.visible.clear();
			for row in visible {
				self.cache.visible.push(TreeLine::Row(row));
				let flat = &self.flat.rows[row];
				if let FlatKind::Item(r) = flat.kind
					&& let Row::Model(i) = self.rows[r]
					&& tree::is_open(ctx, model_id(i), false)
				{
					let depth = flat.depth + 1;
					self.cache
						.visible
						.extend(self.model_meshes[i].iter().map(|&entry| TreeLine::Mesh {
							entry,
							depth,
							model: i,
						}));
					if self.model_info[i].missing > 0 || self.model_meshes[i].is_empty() {
						self.cache.visible.push(TreeLine::Note { model: i, depth });
					}
				}
			}
			self.cache.revision = Some(tree::revision(ctx));
		}
	}

	fn refresh_selection(&mut self) {
		if self.cache.selected.is_empty() {
			self.cache.selected = self
				.flat
				.totals(|r| (self.cache.matches[r] && self.selected[r]) as usize);
		}
	}

	fn selection_changed(&mut self) {
		self.cache.selected.clear();
		self.selection_cache = None;
	}

	/// The maps that will be downloaded for an entry.
	fn chosen_maps(&self, e: usize) -> BTreeMap<MapKind, u64> {
		self.entries[e]
			.maps
			.iter()
			.filter(|(k, _)| self.kinds[k])
			.map(|(&k, &id)| (k, id))
			.collect()
	}

	/// Entry indices to import (with the meshes of selected models) and the
	/// selected model indices.
	fn selection(&self) -> (Vec<usize>, Vec<usize>) {
		let mut entries = Vec::new();
		let mut models = Vec::new();
		for (r, row) in self.rows.iter().enumerate() {
			if !self.selected[r] {
				continue;
			}
			match *row {
				Row::Model(i) => {
					models.push(i);
					entries.extend(&self.model_meshes[i]);
				}
				Row::Mesh(i) => entries.push(i),
			}
		}
		entries.sort_unstable();
		entries.dedup();
		(entries, models)
	}

	/// Returns `Some` once the user imports or cancels.
	pub fn show(&mut self, ctx: &egui::Context) -> Option<Outcome> {
		let mut outcome = None;
		let screen = ctx.content_rect().size();
		let size = egui::vec2(
			(screen.x - 80.0).clamp(420.0, 760.0),
			(screen.y - 100.0).clamp(360.0, 680.0),
		);
		let file = self
			.source
			.file_name()
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_default();
		let modal = widgets::modal(ctx, "import_dialog").show(ctx, |ui| {
			ui.set_width(size.x);
			ui.set_height(size.y);
			let accent = palette(ui.ctx()).accent;
			widgets::dialog_title(
				ui,
				Some((icons::FILE_ARROW_DOWN, 20.0, accent)),
				format!("Import {file}"),
			);
			outcome = self.contents(ui);
		});
		if outcome.is_none() && modal.should_close() {
			outcome = Some(Outcome::Cancel);
		}
		outcome
	}

	fn summary(&self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		let text = self.summary_text();
		ui.horizontal_wrapped(|ui| {
			ui.label(RichText::new(text).color(p.text_weak));
			let problems = self.skipped.len() + self.warnings.len();
			if problems > 0 {
				ui.label(
					RichText::new(format!(
						"{}  {} ignored",
						icons::WARNING,
						plural(problems, "entry", "entries")
					))
					.color(p.warning),
				)
				.on_hover_ui(|ui| {
					let mut list: Vec<_> = self
						.skipped
						.iter()
						.chain(&self.warnings)
						.take(20)
						.cloned()
						.collect();
					if problems > 20 {
						list.push(format!("? and {} more", problems - 20));
					}
					ui.label(list.join("\n"));
				});
			}
		});
	}

	fn contents(&mut self, ui: &mut egui::Ui) -> Option<Outcome> {
		if std::mem::take(&mut self.first_frame) {
			// open folders that are the only thing at their level
			let mut path = Vec::new();
			let mut node = &self.tree;
			while node.items.is_empty() && node.folders.len() == 1 {
				let (name, child) = node.folders.iter().next().expect("one folder");
				path.push(name.clone());
				node = child;
			}
			tree::open_path(ui.ctx(), SCOPE, &path);
		}
		if let Some(command) = self.pending.take() {
			command.apply(ui.ctx(), SCOPE, &self.tree);
		}
		self.summary(ui);
		ui.add_space(4.0);

		// search, what to show, and folder controls
		// laid out from the right, so the search box takes what is left
		// and can never run into the controls
		ui.horizontal(|ui| {
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				// a choice for each kind the export has, when it has more than one
				let (animated, still, meshes) = self.counts();
				let mut choices = vec![(KindFilter::All, "All")];
				for (n, choice) in [
					(animated, (KindFilter::Animated, "Animated")),
					(still, (KindFilter::Static, "Static")),
					(meshes, (KindFilter::Meshes, "Meshes")),
				] {
					if n > 0 {
						choices.push(choice);
					}
				}
				ui.spacing_mut().item_spacing.x = 0.0;
				if let Some(command) = tree::expand_collapse_buttons(ui) {
					self.pending = Some(command);
				}
				if choices.len() > 2 {
					ui.add_space(8.0);
					widgets::text_segmented(ui, &mut self.show, &choices);
				}
				ui.add_space(8.0);
				ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
					ui.add(
						widgets::input(&mut self.search)
							.hint_text(format!("{}  Search", icons::MAGNIFYING_GLASS))
							.desired_width(ui.available_width()),
					);
				});
			});
		});
		self.needle = self.search.to_lowercase();
		ui.add_space(4.0);

		// summary and buttons below the list
		self.refresh_tree(ui.ctx());
		ui.spacing_mut().interact_size.y = ROW;
		widgets::scroll_rows_in_margin(
			ui,
			egui::ScrollArea::vertical()
				.auto_shrink([false, false])
				.max_height(ui.available_height() - FOOTER_HEIGHT),
			ROW,
			self.cache.visible.len() + 1,
			|ui, range| {
				ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
				for index in range {
					if index == 0 {
						self.select_all_row(ui);
						continue;
					}
					let line = self.cache.visible[index - 1];
					let depth = match line {
						TreeLine::Row(row) => self.flat.rows[row].depth,
						TreeLine::Mesh { depth, .. } | TreeLine::Note { depth, .. } => depth,
					};
					ui.scope_builder(
						egui::UiBuilder::new().scope_id(ui.make_persistent_id(line)),
						|ui| {
							ui.horizontal(|ui| {
								ui.set_min_height(ROW);
								if depth > 0 {
									ui.add_space(depth as f32 * ui.spacing().indent);
								}
								self.tree_line(ui, line);
							});
						},
					);
				}
			},
		);
		ui.separator();
		self.footer(ui)
	}

	/// A tri-state checkbox over everything shown.
	fn select_all_row(&mut self, ui: &mut egui::Ui) {
		let total = *self.cache.totals.last().unwrap_or(&0);
		if total == 0 {
			ui.label(
				RichText::new("Nothing matches the search.").color(palette(ui.ctx()).text_weak),
			);
			return;
		}
		self.refresh_selection();
		let selected = *self.cache.selected.last().unwrap_or(&0);
		let mut all = selected == total;
		let label = if self.search.is_empty() && self.show == KindFilter::All {
			"Select all".to_string()
		} else {
			format!("Select all {total} shown")
		};
		let response = widgets::pointer(ui.add(
			egui::Checkbox::new(&mut all, label).indeterminate(selected > 0 && selected < total),
		));
		if response.changed() {
			for (r, &visible) in self.cache.matches.iter().enumerate() {
				if visible {
					self.selected[r] = all;
				}
			}
			self.selection_changed();
		}
	}

	fn build_selection(&self) -> ImportSelection {
		let (entries, models) = self.selection();
		let downloads: HashSet<u64> = entries
			.iter()
			.copied()
			.filter(|&e| !self.existing[e])
			.flat_map(|e| {
				std::iter::once(self.entries[e].mesh_id).chain(
					self.entries[e]
						.maps
						.iter()
						.filter(|(k, _)| self.kinds[k])
						.map(|(_, &id)| id),
				)
			})
			.collect();
		let animated = models.iter().filter(|&&m| self.is_animated(m)).count();
		let still = models.len() - animated;
		let mut parts = Vec::new();
		if animated > 0 {
			parts.push(plural(animated, "animated model", "animated models"));
		}
		if still > 0 {
			parts.push(plural(still, "static model", "static models"));
		}
		parts.push(plural(entries.len(), "mesh", "meshes"));
		let summary = format!(
			"{} selected · {} to download",
			and_list(&parts),
			plural(downloads.len(), "file", "files")
		);
		let already = entries.iter().filter(|&&e| self.existing[e]).count()
			+ models.iter().filter(|&&m| self.model_existing[m]).count();
		ImportSelection {
			entries,
			models,
			summary,
			already,
		}
	}

	fn footer(&mut self, ui: &mut egui::Ui) -> Option<Outcome> {
		let kinds = self.kinds.clone();
		let cache = self
			.selection_cache
			.take()
			.unwrap_or_else(|| self.build_selection());
		let ImportSelection {
			entries,
			models,
			summary,
			already,
		} = &cache;
		ui.horizontal(|ui| {
			ui.label(summary.as_str());
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				self.maps_menu(ui)
			});
		});

		if *already > 0 {
			ui.label(widgets::caption(
				ui,
				format!(
					"{already} already in this workspace; importing again only adds what is new."
				),
			));
		} else {
			ui.add_space(NOTE_RESERVE);
		}

		let mut outcome = None;
		ui.horizontal(|ui| {
			widgets::pointer(ui.checkbox(&mut self.download_now, "Start downloading now"));
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				let count = entries.len() + models.len();
				let import = ui
					.add_enabled_ui(count > 0, |ui| {
						widgets::primary_button(
							ui,
							if count == 0 {
								"Import".into()
							} else {
								format!("Import {count}")
							},
						)
					})
					.inner;
				if import.clicked() {
					let selected: HashSet<usize> = models.iter().copied().collect();
					let models = std::mem::take(&mut self.models)
						.into_iter()
						.enumerate()
						.filter(|(i, _)| selected.contains(i))
						.map(|(_, m)| m)
						.collect();
					outcome = Some(Outcome::Import {
						origin: self.origin.clone(),
						entries: entries
							.iter()
							.map(|&e| (e, self.chosen_maps(e)))
							.map(|(e, maps)| Entry {
								maps,
								..self.entries[e].clone()
							})
							.collect(),
						models,
						download_now: self.download_now,
					});
				}
				if widgets::text_button(ui, "Cancel").clicked() {
					outcome = Some(Outcome::Cancel);
				}
			});
		});
		if self.kinds == kinds {
			self.selection_cache = Some(cache);
		}
		outcome
	}

	/// One dropdown choosing which kinds of map to download.
	fn maps_menu(&mut self, ui: &mut egui::Ui) {
		let counts = &self.map_counts;
		let offered: Vec<MapKind> = MapKind::ALL.into_iter().filter(|k| counts[k] > 0).collect();
		if offered.is_empty() {
			return;
		}
		let on = offered.iter().filter(|k| self.kinds[k]).count();
		let state = match on {
			0 => "none".to_string(),
			n if n == offered.len() => "all".to_string(),
			n => format!("{n} of {}", offered.len()),
		};
		ui.menu_button(
			format!("{}  Maps: {state}  {}", icons::IMAGE, icons::CARET_DOWN),
			|ui| {
				widgets::menu_style(ui);
				ui.set_min_width(240.0);
				ui.label(widgets::caption(ui, "Images to download with each mesh"));
				for kind in offered {
					let enabled = self.kinds.get_mut(&kind).expect("all kinds present");
					widgets::pointer(
						ui.checkbox(enabled, format!("{}  ({})", kind.label(), counts[&kind])),
					)
					.on_hover_text(format!("The {} field", kind.export_field()));
				}
			},
		)
		.response
		.on_hover_text("Choose which maps to download");
	}

	fn tree_line(&mut self, ui: &mut egui::Ui, line: TreeLine) {
		match line {
			TreeLine::Row(row) => match &self.flat.rows[row].kind {
				FlatKind::Folder { name, id, .. } => {
					let id = *id;
					let label = format!("{}  {name}", icons::FOLDER_SIMPLE);
					self.refresh_selection();
					let selected = self.flat.count(row, &self.cache.selected);
					let total = self.flat.count(row, &self.cache.totals);
					tree::header(ui, id, false, |ui| {
						let mut all = selected == total;
						let response = widgets::pointer(
							ui.add(
								egui::Checkbox::new(&mut all, label)
									.indeterminate(selected > 0 && selected < total),
							),
						);
						if response.changed() {
							for &r in self.flat.folder_items(row) {
								if self.cache.matches[r] {
									self.selected[r] = all;
								}
							}
							self.selection_changed();
						}
						ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
							ui.label(widgets::caption(ui, format!("{selected}/{total}")));
						});
						response
							.on_hover_ui(|ui| {
								ui.label(self.flat.path(row));
							})
							.context_menu(|ui| {
								widgets::menu_style(ui);
								if let Some(command) =
									tree::subtree_menu_items(ui, &self.flat.path(row))
								{
									self.pending = Some(command);
								}
							});
					});
				}
				FlatKind::Item(r) => {
					let r = *r;
					match self.rows[r] {
						Row::Model(i) => self.model_row(ui, r, i),
						Row::Mesh(i) => {
							self.checkbox_row(
								ui,
								r,
								format!("{}  {}", icons::CUBE, self.entries[i].name),
								self.existing[i],
							);
							self.mesh_hint(ui, i);
						}
					}
				}
			},
			TreeLine::Mesh { entry, .. } => {
				ui.add_space(CHECKBOX_INDENT);
				ui.label(format!("{}  {}", icons::CUBE, self.entries[entry].name));
				self.mesh_hint(ui, entry);
			}
			TreeLine::Note { model, .. } => {
				ui.add_space(CHECKBOX_INDENT);
				let missing = self.model_info[model].missing;
				let p = palette(ui.ctx());
				let text = if missing > 0 {
					RichText::new(format!(
						"{} not in this export",
						plural(missing, "mesh", "meshes")
					))
					.color(p.warning)
				} else {
					RichText::new("No meshes: its parts are drawn as boxes").color(p.text_weak)
				};
				ui.label(text);
			}
		}
	}

	fn checkbox_row(&mut self, ui: &mut egui::Ui, r: usize, label: String, existing: bool) {
		let mut text = RichText::new(label.clone());
		if existing {
			text = text.color(ui.visuals().weak_text_color());
		}
		let response = widgets::pointer(ui.checkbox(&mut self.selected[r], text));
		if response.changed() {
			self.selection_changed();
		}
		response.on_hover_ui(|ui| {
			ui.label(label);
			if existing {
				ui.label("Already in this workspace");
			}
		});
	}

	/// Map count after a mesh's name, with the ids on hover.
	fn mesh_hint(&self, ui: &mut egui::Ui, e: usize) {
		let entry = &self.entries[e];
		let mut hover = format!("Mesh {}", entry.mesh_id);
		for (k, id) in &entry.maps {
			let skipped = if self.kinds[k] {
				""
			} else {
				" (not downloaded)"
			};
			hover.push_str(&format!("\n{}: {id}{skipped}", k.export_field()));
		}
		let text = if entry.maps.is_empty() {
			String::new()
		} else {
			format!("{}  {}", icons::IMAGE, entry.maps.len())
		};
		ui.label(widgets::caption(ui, text)).on_hover_text(hover);
		if self.existing[e] {
			ui.label(widgets::caption(ui, "in workspace"));
		}
	}

	/// A model: a checkbox, with the meshes it brings along listed
	/// underneath when expanded. A static model has no clips to count.
	fn model_row(&mut self, ui: &mut egui::Ui, r: usize, i: usize) {
		tree::header(ui, model_id(i), false, |ui| {
			let animated = self.is_animated(i);
			let icon = if animated {
				icons::PERSON_SIMPLE_RUN
			} else {
				icons::PACKAGE
			};
			let text = RichText::new(format!("{icon}  {}", self.models[i].name));
			let text = if self.model_existing[i] {
				text.color(ui.visuals().weak_text_color())
			} else {
				text
			};
			let response = widgets::pointer(ui.checkbox(&mut self.selected[r], text));
			if response.changed() {
				self.selection_changed();
			}
			response.on_hover_ui(|ui| {
				ui.label(&self.model_info[i].hover);
			});
			let meshes = self.model_meshes[i].len();
			let caption = if animated {
				format!(
					"{}  {meshes}   {}  {}",
					icons::CUBE,
					icons::FILM_STRIP,
					self.model_info[i].playable
				)
			} else {
				format!("{}  {meshes}", icons::CUBE)
			};
			ui.label(widgets::caption(ui, caption))
				.on_hover_text(if animated {
					"Meshes it uses ? playable clips"
				} else {
					"Meshes it uses"
				});
			if self.model_existing[i] {
				ui.label(widgets::caption(ui, "in workspace"));
			}
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::TempDir;

	#[test]
	fn models_bring_their_meshes_and_others_stand_alone() {
		let json = r#"{"SchemaVersion":1,
			"Meshes":[
				{"Id":1,"Name":"Body","MeshId":"rbxassetid://10","Folder":"RS/Animals/Shark","TextureId":"rbxassetid://11"},
				{"Id":2,"Name":"Rock","MeshId":"rbxassetid://20","Folder":"RS/Props"}
			],
			"Models":[{"Name":"Shark","Path":"RS/Animals/Shark",
				"Parts":[{"Path":"RS/Animals/Shark/Body","Name":"Body","MeshRef":1,
					"CFrame":[0,0,0,1,0,0,0,1,0,0,0,1],"Size":[1,1,1],"Transparency":0}],
				"Joints":[],"Bones":[],"Animations":[]}]}"#;
		let export = crate::studio::export_file::parse(json).unwrap();
		let dir = TempDir::new("import_ui");
		let ws = Workspace::create(&dir, "Test").unwrap();
		let mut dialog = ImportDialog::new("x.json".into(), export, &ws);
		// the shark's body is listed under the shark, not again on its own
		assert_eq!(dialog.rows, [Row::Model(0), Row::Mesh(1)]);
		assert_eq!(dialog.tree.folders["RS"].folders["Animals"].items, [0]);

		dialog.selected[0] = true;
		assert_eq!(dialog.selection(), (vec![0], vec![0]));
		dialog.selected[1] = true;
		assert_eq!(dialog.selection(), (vec![0, 1], vec![0]));

		dialog.kinds.insert(MapKind::Texture, false);
		assert!(dialog.chosen_maps(0).is_empty());

		dialog.show = KindFilter::Meshes;
		assert!(!dialog.matches(0) && dialog.matches(1));
		dialog.show = KindFilter::All;
		// searching a mesh name finds the model using it
		dialog.search = "body".into();
		dialog.needle = "body".into();
		assert!(dialog.matches(0) && !dialog.matches(1));
	}

	#[test]
	fn models_with_clips_are_animated_and_the_rest_static() {
		let model = |name: &str, animations: &str| {
			format!(
				r#"{{"Name":"{name}","Path":"RS/{name}","Parts":[],"Joints":[],"Bones":[],
				"Animations":[{animations}]}}"#
			)
		};
		let clip = r#"{"Name":"Walk","AnimationId":"rbxassetid://5","Status":"KeyframeSequence",
			"Duration":1,"Loop":true,"Priority":"Core","Keyframes":[],"Curves":[]}"#;
		let json = format!(
			r#"{{"SchemaVersion":2,"Meshes":[
				{{"Id":1,"Name":"Rock","MeshId":"rbxassetid://20","Folder":"RS/Props"}}],
			"Models":[{},{},{}]}}"#,
			model("Crate", ""),
			model("Shark", clip),
			model("Barrel", "")
		);
		let export = crate::studio::export_file::parse(&json).unwrap();
		let dir = TempDir::new("import_kinds");
		let ws = Workspace::create(&dir, "Test").unwrap();
		let mut dialog = ImportDialog::new("x.json".into(), export, &ws);
		assert_eq!(dialog.counts(), (1, 2, 1));
		assert_eq!(
			dialog.summary_text(),
			"1 animated model, 2 static models and 1 other mesh. \
			 Ticking a model brings the meshes it uses."
		);
		let shown = |dialog: &ImportDialog| -> Vec<usize> {
			(0..dialog.rows.len())
				.filter(|&r| dialog.matches(r))
				.collect()
		};
		dialog.show = KindFilter::Animated;
		assert_eq!(shown(&dialog), [1]);
		dialog.show = KindFilter::Static;
		assert_eq!(shown(&dialog), [0, 2]);
		dialog.show = KindFilter::Meshes;
		assert_eq!(shown(&dialog), [3]);
		dialog.show = KindFilter::All;
		assert_eq!(shown(&dialog), [0, 1, 2, 3]);
	}

	#[test]
	fn cached_import_tracks_filters_selection_and_model_children() {
		let export = crate::studio::export_file::parse(r#"{"SchemaVersion":1,"Meshes":[
			{"Id":1,"Name":"Body","MeshId":"rbxassetid://10","TextureId":"rbxassetid://11","Folder":"RS/Animal"},
			{"Id":2,"Name":"Rock","MeshId":"rbxassetid://20","Folder":"RS/Props"}],
			"Models":[{"Name":"Animal","Path":"RS/Animal","Parts":[
				{"MeshRef":1},{"MeshRef":999}],"Animations":[]}] }"#).unwrap();
		let dir = TempDir::new("virtual_import");
		let ws = Workspace::create(&dir, "Test").unwrap();
		let mut dialog = ImportDialog::new("test.json".into(), export, &ws);
		let ctx = egui::Context::default();
		FolderCommand::All(true).apply(&ctx, SCOPE, &dialog.tree);
		dialog.refresh_tree(&ctx);
		assert_eq!(dialog.cache.matches, [true, true]);
		let rows = dialog.cache.visible.as_ptr();
		dialog.refresh_tree(&ctx);
		assert_eq!(dialog.cache.visible.as_ptr(), rows);
		let before = dialog.cache.visible.len();
		tree::set_open(&ctx, model_id(0), true);
		dialog.refresh_tree(&ctx);
		assert_eq!(dialog.cache.visible.len(), before + 2);
		assert!(dialog.cache.visible.contains(&TreeLine::Mesh {
			entry: 0,
			depth: 2,
			model: 0
		}));
		assert!(
			dialog
				.cache
				.visible
				.contains(&TreeLine::Note { model: 0, depth: 2 })
		);
		assert_eq!(dialog.model_info[0].missing, 1);
		dialog.search = "body".into();
		dialog.needle = dialog.search.clone();
		dialog.refresh_tree(&ctx);
		assert_eq!(dialog.cache.matches, [true, false]);
		dialog.selected[0] = true;
		dialog.selection_changed();
		dialog.refresh_tree(&ctx);
		assert_eq!(dialog.cache.selected.last(), Some(&1));
		let selected = dialog.build_selection();
		assert_eq!(selected.entries, [0]);
		assert_eq!(selected.models, [0]);
		assert!(selected.summary.contains("2 files to download"));
		dialog.kinds.insert(MapKind::Texture, false);
		assert!(
			dialog
				.build_selection()
				.summary
				.contains("1 file to download")
		);
		dialog.show = KindFilter::Meshes;
		dialog.refresh_tree(&ctx);
		assert!(dialog.cache.visible.is_empty());
		assert_eq!(dialog.cache.selected.last(), Some(&0));
	}

	#[test]
	fn lists_read_as_english() {
		let list =
			|parts: &[&str]| and_list(&parts.iter().map(|s| s.to_string()).collect::<Vec<_>>());
		assert_eq!(list(&[]), "");
		assert_eq!(list(&["a"]), "a");
		assert_eq!(list(&["a", "b"]), "a and b");
		assert_eq!(list(&["a", "b", "c"]), "a, b and c");
	}
}

#[cfg(test)]
mod replay;
