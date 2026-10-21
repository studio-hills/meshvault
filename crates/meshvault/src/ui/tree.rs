//! Open/closed state for folder trees, keyed by each folder's full path so
//! folders can be expanded or collapsed from anywhere (e.g. "expand all"),
//! not just from the widget that draws them. Also the folder hierarchy
//! itself ([`FolderTree`]), its display order ([`natural_cmp`]) and the
//! expand/collapse controls the library and the import dialog share.

use std::collections::BTreeMap;

use eframe::egui::{self, Id};

use crate::ui::{icons, widgets};

/// Folder hierarchy over a list of entries, for tree views.
#[derive(Default)]
pub struct FolderTree {
	pub folders: BTreeMap<String, FolderTree>,
	/// Indices into the entry list.
	pub items: Vec<usize>,
}

impl FolderTree {
	pub fn build<'a>(folders: impl IntoIterator<Item = &'a [String]>) -> Self {
		let mut root = Self::default();
		for (i, path) in folders.into_iter().enumerate() {
			let mut node = &mut root;
			for part in path {
				node = node.folders.entry(part.clone()).or_default();
			}
			node.items.push(i);
		}
		root
	}

	/// Subfolders in display order: case-insensitive, with numbers compared
	/// by value so "2: Lizard" comes before "10: Wolf".
	pub fn sorted_folders(&self) -> Vec<(&String, &FolderTree)> {
		let mut folders: Vec<_> = self.folders.iter().collect();
		folders.sort_by(|a, b| natural_cmp(a.0, b.0));
		folders
	}

	/// Every entry index in this folder and below.
	pub fn all_items(&self) -> Vec<usize> {
		let mut out = self.items.clone();
		for child in self.folders.values() {
			out.extend(child.all_items());
		}
		out
	}
}

/// Case-insensitive comparison that orders runs of digits numerically.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
	use std::cmp::Ordering;
	let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
	loop {
		match (a.peek().copied(), b.peek().copied()) {
			(None, None) => return Ordering::Equal,
			(None, Some(_)) => return Ordering::Less,
			(Some(_), None) => return Ordering::Greater,
			(Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
				let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
					let mut digits = String::new();
					while let Some(c) = it.peek().filter(|c| c.is_ascii_digit()) {
						digits.push(*c);
						it.next();
					}
					digits
				};
				let (da, db) = (take(&mut a), take(&mut b));
				let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
				let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
				if ord != Ordering::Equal {
					return ord;
				}
			}
			(Some(x), Some(y)) => {
				let ord = x.to_lowercase().cmp(y.to_lowercase());
				if ord != Ordering::Equal {
					return ord;
				}
				a.next();
				b.next();
			}
		}
	}
}

/// The id of the folder at `path` (components joined with `/`) in a tree.
/// `scope` keeps different trees (library, import dialog) apart.
pub fn folder_id(scope: &str, path: &str) -> Id {
	Id::unique(("folder", scope, path))
}

/// `prefix/name`, or just `name` at the root.
pub fn child_path(prefix: &str, name: &str) -> String {
	if prefix.is_empty() {
		name.to_owned()
	} else {
		format!("{prefix}/{name}")
	}
}

fn set_open(ctx: &egui::Context, id: Id, open: bool) {
	let mut state =
		egui::collapsing_header::CollapsingState::load_with_default_open(ctx, id, false);
	state.set_open(open);
	state.store(ctx);
}

/// Opens or closes every folder below `node`.
fn set_all(ctx: &egui::Context, scope: &str, node: &FolderTree, prefix: &str, open: bool) {
	for (name, child) in &node.folders {
		let path = child_path(prefix, name);
		set_open(ctx, folder_id(scope, &path), open);
		set_all(ctx, scope, child, &path, open);
	}
}

/// Opens or closes the folder at `path` and everything inside it.
fn set_subtree(ctx: &egui::Context, scope: &str, root: &FolderTree, path: &str, open: bool) {
	let mut node = root;
	for part in path.split('/') {
		match node.folders.get(part) {
			Some(child) => node = child,
			None => return,
		}
	}
	set_open(ctx, folder_id(scope, path), open);
	set_all(ctx, scope, node, path, open);
}

/// Opens each folder on the way to `folder` so its contents are visible.
pub fn open_path(ctx: &egui::Context, scope: &str, folder: &[String]) {
	let mut path = String::new();
	for part in folder {
		path = child_path(&path, part);
		set_open(ctx, folder_id(scope, &path), true);
	}
}

/// A folder control requested this frame, applied before the next draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderCommand {
	All(bool),
	Subtree(String, bool),
}

impl FolderCommand {
	pub fn apply(&self, ctx: &egui::Context, scope: &str, root: &FolderTree) {
		match self {
			FolderCommand::All(open) => set_all(ctx, scope, root, "", *open),
			FolderCommand::Subtree(path, open) => set_subtree(ctx, scope, root, path, *open),
		}
	}
}

/// The "Collapse all folders" and "Expand all folders" buttons, in that
/// order. The caller owns the layout (direction, spacing, height).
pub fn expand_collapse_buttons(ui: &mut egui::Ui) -> Option<FolderCommand> {
	let mut command = None;
	if widgets::icon_button(ui, icons::ARROWS_IN_LINE_VERTICAL, "Collapse all folders").clicked() {
		command = Some(FolderCommand::All(false));
	}
	if widgets::icon_button(ui, icons::ARROWS_OUT_LINE_VERTICAL, "Expand all folders").clicked() {
		command = Some(FolderCommand::All(true));
	}
	command
}

/// The "Expand all inside" and "Collapse all inside" items of a folder's
/// context menu. Closes the menu when one is chosen.
pub fn subtree_menu_items(ui: &mut egui::Ui, path: &str) -> Option<FolderCommand> {
	let mut command = None;
	if ui
		.button(format!(
			"{}  Expand all inside",
			icons::ARROWS_OUT_LINE_VERTICAL
		))
		.clicked()
	{
		command = Some(FolderCommand::Subtree(path.to_owned(), true));
		ui.close();
	}
	if ui
		.button(format!(
			"{}  Collapse all inside",
			icons::ARROWS_IN_LINE_VERTICAL
		))
		.clicked()
	{
		command = Some(FolderCommand::Subtree(path.to_owned(), false));
		ui.close();
	}
	command
}

/// Flips the folder `id` open or closed. A folder that has never been
/// drawn has no state yet and is left alone.
pub fn toggle(ui: &egui::Ui, id: Id) {
	if let Some(mut state) = egui::collapsing_header::CollapsingState::load(ui.ctx(), id) {
		state.toggle(ui);
		state.store(ui.ctx());
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn natural_order() {
		let mut v = vec![
			"10: Wolf",
			"monkisrokBoss",
			"2: Lizard",
			"Zone_10",
			"1: Bug",
			"Alien",
		];
		v.sort_by(|a, b| natural_cmp(a, b));
		assert_eq!(
			v,
			[
				"1: Bug",
				"2: Lizard",
				"10: Wolf",
				"Alien",
				"monkisrokBoss",
				"Zone_10"
			]
		);
	}

	#[test]
	fn builds_folder_hierarchy() {
		let folders: Vec<Vec<String>> = ["Zone_10/8: CrystalScorpion", "Eggs/Angel Egg", ""]
			.iter()
			.map(|p| {
				p.split('/')
					.filter(|s| !s.is_empty())
					.map(String::from)
					.collect()
			})
			.collect();
		let tree = FolderTree::build(folders.iter().map(Vec::as_slice));
		assert_eq!(tree.items, [2]);
		assert_eq!(
			tree.folders["Zone_10"].folders["8: CrystalScorpion"].items,
			[0]
		);
		assert_eq!(tree.all_items().len(), 3);
	}

	#[test]
	fn expands_and_collapses_by_path() {
		let folders: Vec<Vec<String>> = ["Animals/Diamond/Chicken", "Animals/Gold", "Eggs"]
			.iter()
			.map(|p| p.split('/').map(String::from).collect())
			.collect();
		let tree = FolderTree::build(folders.iter().map(Vec::as_slice));
		let ctx = egui::Context::default();
		let is_open = |path: &str| {
			egui::collapsing_header::CollapsingState::load(&ctx, folder_id("t", path))
				.is_some_and(|s| s.is_open())
		};

		FolderCommand::All(true).apply(&ctx, "t", &tree);
		for path in [
			"Animals",
			"Animals/Diamond",
			"Animals/Diamond/Chicken",
			"Animals/Gold",
			"Eggs",
		] {
			assert!(is_open(path), "{path} should be open");
		}

		FolderCommand::Subtree("Animals/Diamond".into(), false).apply(&ctx, "t", &tree);
		assert!(!is_open("Animals/Diamond") && !is_open("Animals/Diamond/Chicken"));
		assert!(
			is_open("Animals") && is_open("Animals/Gold"),
			"siblings and parents stay open"
		);

		FolderCommand::All(false).apply(&ctx, "t", &tree);
		open_path(&ctx, "t", &["Animals".into(), "Diamond".into()]);
		assert!(is_open("Animals") && is_open("Animals/Diamond"));
		assert!(!is_open("Animals/Diamond/Chicken") && !is_open("Eggs"));

		// unknown paths are ignored
		FolderCommand::Subtree("Nope/Missing".into(), true).apply(&ctx, "t", &tree);
	}

	#[test]
	fn toggle_flips_stored_folders_and_ignores_unknown_ones() {
		let ctx = egui::Context::default();
		let (shut, open, unknown) = (
			folder_id("t", "a"),
			folder_id("t", "b"),
			folder_id("t", "c"),
		);
		set_open(&ctx, shut, false);
		set_open(&ctx, open, true);
		let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
			toggle(ui, shut);
			toggle(ui, open);
			toggle(ui, unknown);
		});
		// the font atlas upload is never applied here
		output.textures_delta.clear();
		let state = |id| egui::collapsing_header::CollapsingState::load(&ctx, id);
		assert!(state(shut).is_some_and(|s| s.is_open()));
		assert!(state(open).is_some_and(|s| !s.is_open()));
		assert!(state(unknown).is_none(), "an unknown folder gets no state");
	}
}
