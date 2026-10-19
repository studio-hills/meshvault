//! The workspace browser: a folder tree of imported models with their
//! download status.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};

use crate::export::FolderTree;
use crate::workspace::{AssetState, Item, Workspace};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
	#[default]
	All,
	Downloaded,
	Pending,
	Failed,
}

pub enum Action {
	Open(usize),
	Retry(Vec<usize>),
	Remove(Vec<usize>),
	Reveal(PathBuf),
}

#[derive(Default)]
pub struct Browser {
	filter: String,
	status: StatusFilter,
	tree: FolderTree,
	/// Workspace and generation the tree was built from.
	built_for: Option<(PathBuf, u64)>,
}

const DONE: Color32 = Color32::from_rgb(110, 200, 120);
const FAILED: Color32 = Color32::from_rgb(230, 100, 90);
const PENDING: Color32 = Color32::from_rgb(200, 180, 90);

/// A coloured dot; plain text so it can be tinted (emoji glyphs cannot).
fn status_icon(item: &Item) -> (&'static str, Color32) {
	if item.has_failure() {
		("●", FAILED)
	} else if item.is_done() {
		("●", DONE)
	} else {
		("○", PENDING)
	}
}

fn describe(state: &AssetState) -> String {
	match state {
		AssetState::Pending => "not downloaded yet".into(),
		AssetState::Done { path } => path.clone(),
		AssetState::Failed { error } => format!("failed: {error}"),
	}
}

impl Browser {
	fn rebuild_if_needed(&mut self, ws: &Workspace) {
		let key = (ws.root.clone(), ws.generation());
		if self.built_for.as_ref() != Some(&key) {
			self.tree = FolderTree::build(ws.items().iter().map(|i| i.folder.as_slice()));
			self.built_for = Some(key);
		}
	}

	fn matches(&self, item: &Item) -> bool {
		let status_ok = match self.status {
			StatusFilter::All => true,
			StatusFilter::Downloaded => item.is_done(),
			StatusFilter::Pending => item.has_pending() && !item.has_failure(),
			StatusFilter::Failed => item.has_failure(),
		};
		if !status_ok {
			return false;
		}
		if self.filter.is_empty() {
			return true;
		}
		let needle = self.filter.to_lowercase();
		item.name.to_lowercase().contains(&needle)
			|| item.folder.iter().any(|f| f.to_lowercase().contains(&needle))
			|| item.mesh_id.to_string() == needle
	}

	pub fn show(&mut self, ui: &mut egui::Ui, ws: &Workspace, open_stem: Option<&str>) -> Vec<Action> {
		self.rebuild_if_needed(ws);
		let mut actions = Vec::new();
		let items = ws.items();

		let done = items.iter().filter(|i| i.is_done()).count();
		let failed = items.iter().filter(|i| i.has_failure()).count();
		ui.horizontal(|ui| {
			ui.heading(ws.name());
			if ui
				.small_button("📂")
				.on_hover_text("Show workspace folder")
				.clicked()
			{
				actions.push(Action::Reveal(ws.root.clone()));
			}
		});
		ui.label(
			RichText::new(format!("{} models · {done} downloaded · {failed} failed", items.len()))
				.small(),
		);
		if items.is_empty() {
			ui.add_space(8.0);
			ui.label("This workspace is empty. Use Workspace → Import export JSON…, or drop an export .json onto the window.");
			return actions;
		}
		ui.horizontal(|ui| {
			ui.add(
				egui::TextEdit::singleline(&mut self.filter)
					.hint_text("Search")
					.desired_width(ui.available_width() - 28.0),
			);
			if !self.filter.is_empty() && ui.small_button("✕").clicked() {
				self.filter.clear();
			}
		});
		ui.horizontal(|ui| {
			for (value, label) in [
				(StatusFilter::All, RichText::new("All")),
				(StatusFilter::Downloaded, RichText::new("● Done").color(DONE)),
				(StatusFilter::Pending, RichText::new("○ Pending").color(PENDING)),
				(StatusFilter::Failed, RichText::new("● Failed").color(FAILED)),
			] {
				ui.selectable_value(&mut self.status, value, label);
			}
		});
		ui.separator();

		egui::ScrollArea::vertical()
			.auto_shrink([false, false])
			.show(ui, |ui| {
				let tree = std::mem::take(&mut self.tree);
				self.folder(ui, ws, &tree, "ws", open_stem, &mut actions);
				self.tree = tree;
			});
		actions
	}

	fn folder(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		node: &FolderTree,
		id_path: &str,
		open_stem: Option<&str>,
		actions: &mut Vec<Action>,
	) {
		let items = ws.items();
		for (name, child) in node.sorted_folders() {
			let all = child.all_items();
			let visible: Vec<usize> = all.iter().copied().filter(|&i| self.matches(&items[i])).collect();
			if visible.is_empty() {
				continue;
			}
			let failed: Vec<usize> = all.iter().copied().filter(|&i| items[i].has_failure()).collect();
			let done = all.iter().filter(|&&i| items[i].is_done()).count();
			let path = format!("{id_path}/{name}");
			let id = ui.make_persistent_id(&path);
			let mut state =
				egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
			if !self.filter.is_empty() || self.status != StatusFilter::All {
				state.set_open(true);
			}
			state
				.show_header(ui, |ui| {
					let mut text = RichText::new(format!("📁 {name}  {done}/{}", all.len()));
					if !failed.is_empty() {
						text = text.color(FAILED);
					}
					let response = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
					response.context_menu(|ui| {
						if ui
							.add_enabled(!failed.is_empty(), egui::Button::new(format!("Retry {} failed", failed.len())))
							.clicked()
						{
							actions.push(Action::Retry(failed.clone()));
							ui.close();
						}
						if let Some(first) = all.first() {
							let dir = ws.root.join(&items[*first].stem);
							if ui.button("Show in file browser").clicked() {
								if let Some(parent) = dir.parent() {
									actions.push(Action::Reveal(parent.to_path_buf()));
								}
								ui.close();
							}
						}
						ui.separator();
						if ui.button(format!("Remove folder ({} models)", all.len())).clicked() {
							actions.push(Action::Remove(all.clone()));
							ui.close();
						}
					});
				})
				.body(|ui| self.folder(ui, ws, child, &path, open_stem, actions));
		}
		for &i in &node.items {
			let item = &items[i];
			if !self.matches(item) {
				continue;
			}
			let (icon, colour) = status_icon(item);
			let is_open = open_stem == Some(item.stem.as_str());
			ui.horizontal(|ui| {
				ui.label(RichText::new(icon).color(colour));
				let mut label = item.name.clone();
				if !item.maps.is_empty() {
					label.push_str("  🖼");
				}
				let response = ui.selectable_label(is_open, label);
				let mut hover = format!("Mesh {}: {}", item.mesh_id, describe(&item.mesh));
				for (kind, map) in &item.maps {
					hover.push_str(&format!("\n{} {}: {}", kind.label(), map.id, describe(&map.state)));
				}
				let response = response.on_hover_text(hover);
				if response.clicked() {
					actions.push(Action::Open(i));
				}
				response.context_menu(|ui| {
					if ui.button("Open").clicked() {
						actions.push(Action::Open(i));
						ui.close();
					}
					if item.has_failure() && ui.button("Retry").clicked() {
						actions.push(Action::Retry(vec![i]));
						ui.close();
					}
					if let AssetState::Done { path } = &item.mesh
						&& ui.button("Show in file browser").clicked()
					{
						actions.push(Action::Reveal(ws.root.join(path)));
						ui.close();
					}
					if ui.button("Copy mesh id").clicked() {
						ui.ctx().copy_text(item.mesh_id.to_string());
						ui.close();
					}
					ui.separator();
					if ui.button("Remove from workspace").clicked() {
						actions.push(Action::Remove(vec![i]));
						ui.close();
					}
				});
			});
		}
	}
}

/// Opens a file browser at `path` (selecting it if it is a file).
pub fn reveal(path: &std::path::Path) {
	#[cfg(target_os = "windows")]
	{
		let mut cmd = std::process::Command::new("explorer");
		if path.is_file() {
			cmd.arg(format!("/select,{}", path.display()));
		} else {
			cmd.arg(path);
		}
		let _ = cmd.spawn();
	}
	#[cfg(target_os = "macos")]
	{
		let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
	}
	#[cfg(not(any(target_os = "windows", target_os = "macos")))]
	{
		let dir = if path.is_file() { path.parent().unwrap_or(path) } else { path };
		let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
	}
}
