//! Open/closed state for folder trees, keyed by each folder's full path so
//! folders can be expanded or collapsed from anywhere (e.g. "expand all"),
//! not just from the widget that draws them.

use eframe::egui::{self, Id};

use crate::studio::export_file::FolderTree;

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

pub fn set_open(ctx: &egui::Context, id: Id, open: bool) {
	let mut state =
		egui::collapsing_header::CollapsingState::load_with_default_open(ctx, id, false);
	state.set_open(open);
	state.store(ctx);
}

/// Opens or closes every folder below `node`.
pub fn set_all(ctx: &egui::Context, scope: &str, node: &FolderTree, prefix: &str, open: bool) {
	for (name, child) in &node.folders {
		let path = child_path(prefix, name);
		set_open(ctx, folder_id(scope, &path), open);
		set_all(ctx, scope, child, &path, open);
	}
}

/// Opens or closes the folder at `path` and everything inside it.
pub fn set_subtree(ctx: &egui::Context, scope: &str, root: &FolderTree, path: &str, open: bool) {
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

#[cfg(test)]
mod tests {
	use super::*;

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
}
