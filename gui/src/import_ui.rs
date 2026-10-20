//! The import dialog: choose what to add to the workspace from an export.
//!
//! Everything is shown in one folder tree. An animated model sits in its
//! folder with the meshes it uses listed under it; ticking the model brings
//! them along. Meshes no model uses are listed on their own. Which kinds of
//! map to download is a single choice for the whole import.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use eframe::egui::{self, Align, Layout, RichText};

use crate::anim::schema::AnimatedModel;
use crate::export::{Entry, Export, FolderTree};
use crate::maps::MapKind;
use crate::ui::icons;
use crate::ui::theme::{ROW, dialog_frame, display, palette};
use crate::ui::tree::{self, FolderCommand};
use crate::ui::widgets;
use crate::workspace::Workspace;

/// One line of the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
	/// An animated model, by index into `models`.
	Model(usize),
	/// A mesh no animated model uses, by index into `entries`.
	Mesh(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
	All,
	Animated,
	Meshes,
}

pub struct ImportDialog {
	pub source: PathBuf,
	entries: Vec<Entry>,
	/// Entries already in the workspace.
	existing: Vec<bool>,
	models: Vec<AnimatedModel>,
	/// Models already in the workspace (same instance path).
	model_existing: Vec<bool>,
	/// Entry indices of the meshes each model uses.
	model_meshes: Vec<Vec<usize>>,
	rows: Vec<Row>,
	selected: Vec<bool>,
	/// The tree over `rows`.
	tree: FolderTree,
	/// Which kinds of map to download.
	kinds: BTreeMap<MapKind, bool>,
	show: Show,
	skipped: Vec<String>,
	warnings: Vec<String>,
	filter: String,
	download_now: bool,
	/// Expand/collapse requested while drawing, applied on the next frame.
	pending: Option<FolderCommand>,
	/// Set until the first draw, which opens lone top folders.
	first_frame: bool,
}

/// Folder state namespace for the import tree.
const SCOPE: &str = "import";

pub enum Outcome {
	Cancel,
	Import {
		/// Selected entries, with `maps` narrowed to what will be downloaded.
		entries: Vec<Entry>,
		/// Selected animated models; their meshes are among `entries`.
		models: Vec<AnimatedModel>,
		download_now: bool,
	},
}

fn plural(n: usize, one: &str, many: &str) -> String {
	format!("{n} {}", if n == 1 { one } else { many })
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
		let model_paths: HashSet<&str> =
			workspace.models().iter().map(|m| m.path.as_str()).collect();
		let model_existing = export
			.models
			.iter()
			.map(|m| model_paths.contains(m.path.as_str()))
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
			let mut folder: Vec<String> = m
				.path
				.split('/')
				.filter(|s| !s.is_empty())
				.map(String::from)
				.collect();
			folder.pop();
			rows.push(Row::Model(i));
			folders.push(folder);
		}
		for (i, e) in export.entries.iter().enumerate() {
			if !used.contains(&i) {
				rows.push(Row::Mesh(i));
				folders.push(e.folder.clone());
			}
		}
		let tree = FolderTree::build(folders.iter().map(Vec::as_slice));
		Self {
			source,
			selected: vec![false; rows.len()],
			rows,
			tree,
			kinds: MapKind::ALL.iter().map(|&k| (k, true)).collect(),
			show: Show::All,
			existing,
			model_existing,
			model_meshes,
			models: export.models,
			entries: export.entries,
			skipped: export.skipped,
			warnings: export.warnings,
			filter: String::new(),
			download_now: true,
			pending: None,
			first_frame: true,
		}
	}

	fn row_name(&self, row: Row) -> &str {
		match row {
			Row::Model(i) => &self.models[i].name,
			Row::Mesh(i) => &self.entries[i].name,
		}
	}

	fn matches(&self, r: usize) -> bool {
		let row = self.rows[r];
		let kind_ok = matches!(
			(self.show, row),
			(Show::All, _) | (Show::Animated, Row::Model(_)) | (Show::Meshes, Row::Mesh(_))
		);
		if !kind_ok {
			return false;
		}
		if self.filter.is_empty() {
			return true;
		}
		let needle = self.filter.to_lowercase();
		let mesh_matches =
			|e: &Entry| e.name.to_lowercase().contains(&needle) || e.mesh_id.to_string() == needle;
		match row {
			Row::Model(i) => {
				let m = &self.models[i];
				m.path.to_lowercase().contains(&needle)
					|| self.model_meshes[i]
						.iter()
						.any(|&e| mesh_matches(&self.entries[e]))
			}
			Row::Mesh(i) => {
				let e = &self.entries[i];
				mesh_matches(e) || e.folder.iter().any(|f| f.to_lowercase().contains(&needle))
			}
		}
	}

	fn visible(&self, rows: Vec<usize>) -> Vec<usize> {
		rows.into_iter().filter(|&r| self.matches(r)).collect()
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
		let modal = egui::Modal::new(egui::Id::unique("import_dialog"))
			.frame(dialog_frame(ctx))
			.backdrop_color(egui::Color32::from_black_alpha(140))
			.show(ctx, |ui| {
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
							.size(18.0)
							.family(display()),
					);
				});
				outcome = self.contents(ui);
			});
		if outcome.is_none() && modal.should_close() {
			outcome = Some(Outcome::Cancel);
		}
		outcome
	}

	fn summary(&self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		let standalone = self
			.rows
			.iter()
			.filter(|r| matches!(r, Row::Mesh(_)))
			.count();
		let text = if self.models.is_empty() {
			format!(
				"{} in {}",
				plural(self.entries.len(), "mesh", "meshes"),
				plural(self.tree.folders.len(), "folder", "folders")
			)
		} else {
			format!(
				"{} and {}. Ticking a model brings the meshes it uses.",
				plural(self.models.len(), "animated model", "animated models"),
				plural(standalone, "other mesh", "other meshes")
			)
		};
		ui.horizontal_wrapped(|ui| {
			ui.label(RichText::new(text).color(p.text_weak));
			let problems: Vec<&String> = self.skipped.iter().chain(&self.warnings).collect();
			if !problems.is_empty() {
				let mut list: Vec<String> =
					problems.iter().take(20).map(|s| s.to_string()).collect();
				if problems.len() > 20 {
					list.push(format!("… and {} more", problems.len() - 20));
				}
				ui.label(
					RichText::new(format!(
						"{}  {} ignored",
						icons::WARNING,
						plural(problems.len(), "entry", "entries")
					))
					.color(p.warning),
				)
				.on_hover_text(list.join("\n"));
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
		ui.horizontal(|ui| {
			let both =
				!self.models.is_empty() && self.rows.iter().any(|r| matches!(r, Row::Mesh(_)));
			let reserved = if both { 300.0 } else { 80.0 };
			ui.add(
				widgets::input(&mut self.filter)
					.hint_text(format!("{}  Search", icons::MAGNIFYING_GLASS))
					.desired_width((ui.available_width() - reserved).max(160.0)),
			);
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				ui.spacing_mut().item_spacing.x = 0.0;
				if widgets::icon_button(ui, icons::ARROWS_IN_LINE_VERTICAL, "Collapse all folders")
					.clicked()
				{
					self.pending = Some(FolderCommand::All(false));
				}
				if widgets::icon_button(ui, icons::ARROWS_OUT_LINE_VERTICAL, "Expand all folders")
					.clicked()
				{
					self.pending = Some(FolderCommand::All(true));
				}
				if both {
					ui.add_space(8.0);
					ui.spacing_mut().item_spacing.x = 4.0;
					for (value, label) in [
						(Show::Meshes, "Meshes"),
						(Show::Animated, "Animated"),
						(Show::All, "All"),
					] {
						widgets::pointer(ui.selectable_value(&mut self.show, value, label));
					}
				}
			});
		});
		ui.add_space(4.0);

		// summary and buttons below the list
		let footer_height = 92.0;
		widgets::scroll_in_margin(
			ui,
			egui::ScrollArea::vertical()
				.auto_shrink([false, false])
				.max_height(ui.available_height() - footer_height),
			|ui| {
				ui.spacing_mut().interact_size.y = ROW;
				self.select_all_row(ui);
				let tree = std::mem::take(&mut self.tree);
				self.folder_contents(ui, &tree, "");
				self.tree = tree;
			},
		);
		ui.separator();
		self.footer(ui)
	}

	/// A tri-state checkbox over everything shown.
	fn select_all_row(&mut self, ui: &mut egui::Ui) {
		let visible = self.visible((0..self.rows.len()).collect());
		if visible.is_empty() {
			ui.label(
				RichText::new("Nothing matches the search.").color(palette(ui.ctx()).text_weak),
			);
			return;
		}
		let selected = visible.iter().filter(|&&r| self.selected[r]).count();
		let mut all = selected == visible.len();
		let label = if self.filter.is_empty() && self.show == Show::All {
			"Select all".to_string()
		} else {
			format!("Select all {} shown", visible.len())
		};
		let response = widgets::pointer(
			ui.add(
				egui::Checkbox::new(&mut all, label)
					.indeterminate(selected > 0 && selected < visible.len()),
			),
		);
		if response.changed() {
			for &r in &visible {
				self.selected[r] = all;
			}
		}
	}

	fn footer(&mut self, ui: &mut egui::Ui) -> Option<Outcome> {
		let p = palette(ui.ctx());
		let (entries, models) = self.selection();
		let chosen: Vec<BTreeMap<MapKind, u64>> =
			entries.iter().map(|&e| self.chosen_maps(e)).collect();
		let downloads: HashSet<u64> = entries
			.iter()
			.zip(&chosen)
			.filter(|(e, _)| !self.existing[**e])
			.flat_map(|(&e, maps)| {
				std::iter::once(self.entries[e].mesh_id).chain(maps.values().copied())
			})
			.collect();
		let mut parts = Vec::new();
		if !models.is_empty() {
			parts.push(plural(models.len(), "animated model", "animated models"));
		}
		parts.push(plural(entries.len(), "mesh", "meshes"));
		let summary = format!(
			"{} selected · {} to download",
			parts.join(" and "),
			plural(downloads.len(), "file", "files")
		);
		ui.horizontal(|ui| {
			ui.label(summary);
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				self.maps_menu(ui)
			});
		});
		let already = entries.iter().filter(|&&e| self.existing[e]).count()
			+ models.iter().filter(|&&m| self.model_existing[m]).count();
		if already > 0 {
			ui.label(
				RichText::new(format!(
					"{already} already in this workspace; importing again only adds what is new."
				))
				.size(11.5)
				.color(p.text_weak),
			);
		} else {
			ui.add_space(15.0);
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
						entries: entries
							.iter()
							.zip(chosen)
							.map(|(&e, maps)| Entry {
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
		outcome
	}

	/// One dropdown choosing which kinds of map to download.
	fn maps_menu(&mut self, ui: &mut egui::Ui) {
		let counts: BTreeMap<MapKind, usize> = MapKind::ALL
			.iter()
			.map(|&k| {
				(
					k,
					self.entries
						.iter()
						.filter(|e| e.maps.contains_key(&k))
						.count(),
				)
			})
			.collect();
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
				ui.label(
					RichText::new("Images to download with each mesh")
						.size(11.5)
						.color(palette(ui.ctx()).text_weak),
				);
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

	fn folder_contents(&mut self, ui: &mut egui::Ui, node: &FolderTree, id_path: &str) {
		for (name, child) in node.sorted_folders() {
			let rows = self.visible(child.all_items());
			if rows.is_empty() {
				continue;
			}
			let selected = rows.iter().filter(|&&r| self.selected[r]).count();
			let path = tree::child_path(id_path, name);
			let id = tree::folder_id(SCOPE, &path);
			let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
				ui.ctx(),
				id,
				false,
			);
			// searching expands everything so matches are visible
			if !self.filter.is_empty() {
				state.set_open(true);
			}
			let (arrow, _, _) = state
				.show_header(ui, |ui| {
					let mut all = selected == rows.len();
					let label = format!("{}  {name}", icons::FOLDER_SIMPLE);
					let response = widgets::pointer(
						ui.add(
							egui::Checkbox::new(&mut all, label)
								.indeterminate(selected > 0 && selected < rows.len()),
						),
					);
					if response.changed() {
						for &r in &rows {
							self.selected[r] = all;
						}
					}
					ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
						let p = palette(ui.ctx());
						ui.label(
							RichText::new(format!("{selected}/{}", rows.len()))
								.size(11.5)
								.color(p.text_weak),
						);
					});
					response.context_menu(|ui| {
						widgets::menu_style(ui);
						if ui
							.button(format!(
								"{}  Expand all inside",
								icons::ARROWS_OUT_LINE_VERTICAL
							))
							.clicked()
						{
							self.pending = Some(FolderCommand::Subtree(path.clone(), true));
							ui.close();
						}
						if ui
							.button(format!(
								"{}  Collapse all inside",
								icons::ARROWS_IN_LINE_VERTICAL
							))
							.clicked()
						{
							self.pending = Some(FolderCommand::Subtree(path.clone(), false));
							ui.close();
						}
					});
				})
				.body(|ui| self.folder_contents(ui, child, &path));
			widgets::pointer(arrow);
		}
		// models first, then meshes, each by name
		let mut rows: Vec<usize> = node
			.items
			.iter()
			.copied()
			.filter(|&r| self.matches(r))
			.collect();
		rows.sort_by(|&a, &b| {
			let kind = |r: usize| matches!(self.rows[r], Row::Mesh(_));
			kind(a).cmp(&kind(b)).then_with(|| {
				crate::export::natural_cmp(self.row_name(self.rows[a]), self.row_name(self.rows[b]))
			})
		});
		for r in rows {
			match self.rows[r] {
				Row::Model(i) => self.model_row(ui, r, i),
				Row::Mesh(i) => {
					ui.horizontal(|ui| {
						let label = format!("{}  {}", icons::CUBE, self.entries[i].name);
						self.checkbox_row(ui, r, label, self.existing[i]);
						self.mesh_hint(ui, i);
					});
				}
			}
		}
	}

	fn checkbox_row(&mut self, ui: &mut egui::Ui, r: usize, label: String, existing: bool) {
		let mut text = RichText::new(label);
		if existing {
			text = text.color(ui.visuals().weak_text_color());
		}
		let response = widgets::pointer(ui.checkbox(&mut self.selected[r], text));
		if existing {
			response.on_hover_text("Already in this workspace");
		}
	}

	/// Map count after a mesh's name, with the ids on hover.
	fn mesh_hint(&self, ui: &mut egui::Ui, e: usize) {
		let entry = &self.entries[e];
		let p = palette(ui.ctx());
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
		ui.label(RichText::new(text).size(11.5).color(p.text_weak))
			.on_hover_text(hover);
		if self.existing[e] {
			ui.label(RichText::new("in workspace").size(11.5).color(p.text_weak));
		}
	}

	/// An animated model: a checkbox, with the meshes it brings along
	/// listed underneath when expanded.
	fn model_row(&mut self, ui: &mut egui::Ui, r: usize, i: usize) {
		let p = palette(ui.ctx());
		let id = egui::Id::unique(("import_model", i));
		let state =
			egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
		let meshes = self.model_meshes[i].clone();
		let m = &self.models[i];
		let missing = m
			.parts
			.iter()
			.filter(|part| part.mesh_ref != 0)
			.map(|part| part.mesh_ref)
			.collect::<HashSet<_>>()
			.len()
			.saturating_sub(meshes.len());
		let playable = m
			.animations
			.iter()
			.filter(|a| a.status.is_playable())
			.count();
		let clips = m.animations.len();
		let hover = format!(
			"{}\n{} · {} · {}",
			m.path,
			plural(m.parts.len(), "part", "parts"),
			plural(meshes.len(), "mesh", "meshes"),
			if playable == clips {
				plural(clips, "clip", "clips")
			} else {
				format!("{playable} of {clips} clips playable")
			}
		);
		let name = m.name.clone();
		let existing = self.model_existing[i];
		let (arrow, _, _) = state
			.show_header(ui, |ui| {
				let label = format!("{}  {name}", icons::PERSON_SIMPLE_RUN);
				let mut text = RichText::new(label);
				if existing {
					text = text.color(ui.visuals().weak_text_color());
				}
				widgets::pointer(ui.checkbox(&mut self.selected[r], text)).on_hover_text(hover);
				ui.label(
					RichText::new(format!(
						"{}  {}   {}  {playable}",
						icons::CUBE,
						meshes.len(),
						icons::FILM_STRIP
					))
					.size(11.5)
					.color(p.text_weak),
				)
				.on_hover_text("Meshes it uses · playable clips");
				if existing {
					ui.label(RichText::new("in workspace").size(11.5).color(p.text_weak));
				}
			})
			.body(|ui| {
				for &e in &meshes {
					ui.horizontal(|ui| {
						ui.add_space(22.0);
						ui.label(format!("{}  {}", icons::CUBE, self.entries[e].name));
						self.mesh_hint(ui, e);
					});
				}
				if missing > 0 {
					ui.horizontal(|ui| {
						ui.add_space(22.0);
						ui.label(
							RichText::new(format!(
								"{} not in this export",
								plural(missing, "mesh", "meshes")
							))
							.color(p.warning),
						);
					});
				}
				if meshes.is_empty() && missing == 0 {
					ui.horizontal(|ui| {
						ui.add_space(22.0);
						ui.label(
							RichText::new("No meshes: its parts are drawn as boxes")
								.color(p.text_weak),
						);
					});
				}
			});
		widgets::pointer(arrow);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
		let export = crate::export::parse(json).unwrap();
		let dir = std::env::temp_dir().join(format!("rbx_import_ui_{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
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

		dialog.show = Show::Meshes;
		assert!(!dialog.matches(0) && dialog.matches(1));
		dialog.show = Show::All;
		// searching a mesh name finds the model using it
		dialog.filter = "body".into();
		assert!(dialog.matches(0) && !dialog.matches(1));
		std::fs::remove_dir_all(&dir).unwrap();
	}
}
