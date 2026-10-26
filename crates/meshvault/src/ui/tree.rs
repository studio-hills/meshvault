//! Open/closed state for folder trees, keyed by each folder's full path so
//! folders can be expanded or collapsed from anywhere (e.g. "expand all"),
//! not just from the widget that draws them. Also the folder hierarchy
//! itself ([`FolderTree`]), its rows in display order ([`FlatTree`]), that
//! order ([`natural_cmp`]) and the expand/collapse controls the library and
//! the import dialog share.

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

	/// Every entry index in this folder and below, each folder's own before
	/// its subfolders'. Walked with a list rather than recursion, like
	/// everything else here, so a tree of any depth fits on the stack.
	#[cfg(test)]
	pub fn all_items(&self) -> Vec<usize> {
		let mut out = Vec::new();
		let mut stack = vec![self];
		while let Some(node) = stack.pop() {
			out.extend_from_slice(&node.items);
			stack.extend(node.folders.values().rev());
		}
		out
	}
}

impl Drop for FolderTree {
	/// Takes the subfolders apart one level at a time; dropping them
	/// recursively would overflow the stack on a deep tree.
	fn drop(&mut self) {
		let mut stack: Vec<FolderTree> = std::mem::take(&mut self.folders).into_values().collect();
		while let Some(mut node) = stack.pop() {
			stack.extend(std::mem::take(&mut node.folders).into_values());
		}
	}
}

/// A tree in display order, built once when its entries change. Folder
/// ranges refer to `items`, so counts and selections need no subtree walks.
#[derive(Default)]
pub struct FlatTree {
	pub rows: Vec<FlatRow>,
	pub items: Vec<usize>,
}

pub struct FlatRow {
	pub depth: usize,
	pub parent: Option<usize>,
	pub kind: FlatKind,
}

pub enum FlatKind {
	Folder {
		name: String,
		id: Id,
		/// First row after this folder's descendants.
		end: usize,
		items: std::ops::Range<usize>,
	},
	Item(usize),
}

impl FlatTree {
	pub fn build(
		root: &FolderTree,
		scope: &str,
		compare: impl Fn(usize, usize) -> std::cmp::Ordering,
	) -> Self {
		enum Visit<'a> {
			Node(&'a FolderTree, usize, Option<usize>),
			Folder(&'a str, &'a FolderTree, usize, Option<usize>),
			Items(&'a FolderTree, usize, Option<usize>),
			End(usize, usize),
		}
		let mut flat = Self::default();
		let mut path = String::new();
		let mut stack = vec![Visit::Node(root, 0, None)];
		while let Some(visit) = stack.pop() {
			match visit {
				Visit::Node(node, depth, parent) => {
					stack.push(Visit::Items(node, depth, parent));
					for (name, child) in node.sorted_folders().into_iter().rev() {
						stack.push(Visit::Folder(name, child, depth, parent));
					}
				}
				Visit::Folder(name, child, depth, parent) => {
					let prefix = path.len();
					if !path.is_empty() {
						path.push('/');
					}
					path.push_str(name);
					let index = flat.rows.len();
					flat.rows.push(FlatRow {
						depth,
						parent,
						kind: FlatKind::Folder {
							name: name.to_owned(),
							id: folder_id(scope, &path),
							end: 0,
							items: flat.items.len()..flat.items.len(),
						},
					});
					stack.push(Visit::End(index, prefix));
					stack.push(Visit::Node(child, depth + 1, Some(index)));
				}
				Visit::Items(node, depth, parent) => {
					let mut items = node.items.clone();
					items.sort_by(|&a, &b| compare(a, b));
					for index in items {
						flat.items.push(index);
						flat.rows.push(FlatRow {
							depth,
							parent,
							kind: FlatKind::Item(index),
						});
					}
				}
				Visit::End(index, prefix) => {
					let end = flat.rows.len();
					if let FlatKind::Folder {
						end: row_end,
						items,
						..
					} = &mut flat.rows[index].kind
					{
						*row_end = end;
						items.end = flat.items.len();
					}
					path.truncate(prefix);
				}
			}
		}
		flat
	}

	/// Prefix totals over the sorted entries, for constant-time folder counts.
	pub fn totals(&self, value: impl Fn(usize) -> usize) -> Vec<usize> {
		let mut totals = Vec::with_capacity(self.items.len() + 1);
		totals.push(0);
		for &i in &self.items {
			totals.push(totals.last().copied().unwrap_or(0) + value(i));
		}
		totals
	}

	pub fn count(&self, row: usize, totals: &[usize]) -> usize {
		match &self.rows[row].kind {
			FlatKind::Folder { items, .. } => totals[items.end] - totals[items.start],
			FlatKind::Item(_) => 0,
		}
	}

	pub fn folder_items(&self, row: usize) -> &[usize] {
		match &self.rows[row].kind {
			FlatKind::Folder { items, .. } => &self.items[items.clone()],
			FlatKind::Item(_) => &[],
		}
	}

	/// Reconstruct a path only for a folder whose menu is actually open.
	pub fn path(&self, mut row: usize) -> String {
		let mut parts = Vec::new();
		loop {
			if let FlatKind::Folder { name, .. } = &self.rows[row].kind {
				parts.push(name.as_str());
			}
			let Some(parent) = self.rows[row].parent else {
				break;
			};
			row = parent;
		}
		parts.reverse();
		parts.join("/")
	}

	/// Matching rows whose ancestors are open. Closed or empty subtrees
	/// are skipped in one step, without recursion or per-frame sorting.
	pub fn visible(
		&self,
		ctx: &egui::Context,
		matches: &[bool],
		totals: &[usize],
		force_open: bool,
	) -> Vec<usize> {
		let mut out = Vec::new();
		let mut i = 0;
		while i < self.rows.len() {
			match &self.rows[i].kind {
				FlatKind::Folder { id, end, .. } => {
					if self.count(i, totals) == 0 {
						i = *end;
						continue;
					}
					out.push(i);
					if force_open {
						set_open(ctx, *id, true);
					}
					if !is_open(ctx, *id, false) {
						i = *end;
						continue;
					}
				}
				FlatKind::Item(index) => {
					if matches[*index] {
						out.push(i);
					}
				}
			}
			i += 1;
		}
		out
	}
}

/// Changes to stored folder state invalidate visible rows, including changes
/// made by expand/collapse controls and reveal commands outside the tree.
pub fn revision(ctx: &egui::Context) -> u64 {
	ctx.data_mut(|d| d.get_temp::<u64>(Id::unique("tree_revision")).unwrap_or(0))
}

fn changed(ctx: &egui::Context) {
	let next = revision(ctx).wrapping_add(1);
	ctx.data_mut(|d| d.insert_temp(Id::unique("tree_revision"), next));
	ctx.request_repaint();
}

pub fn is_open(ctx: &egui::Context, id: Id, default: bool) -> bool {
	egui::collapsing_header::CollapsingState::load(ctx, id).map_or(default, |s| s.is_open())
}

/// A normal egui disclosure button, storing the same folder id as a
/// CollapsingHeader. Its body lives in the virtual list instead of a nested UI.
pub fn header<R>(
	ui: &mut egui::Ui,
	id: Id,
	default: bool,
	contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
	let mut state =
		egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default);
	let was_open = state.is_open();
	let inner = ui
		.horizontal(|ui| {
			let spacing = ui.spacing().item_spacing.x;
			ui.spacing_mut().item_spacing.x = 0.0;
			widgets::pointer(
				state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon),
			);
			ui.spacing_mut().item_spacing.x = spacing;
			contents(ui)
		})
		.inner;
	state.store(ui.ctx());
	if state.is_open() != was_open {
		changed(ui.ctx());
	}
	inner
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

/// Opens or closes the folder `id`, drawn or not.
pub fn set_open(ctx: &egui::Context, id: Id, open: bool) {
	let stored = egui::collapsing_header::CollapsingState::load(ctx, id);
	if stored.as_ref().is_none_or(|state| state.is_open() != open) {
		changed(ctx);
	}
	let mut state = stored.unwrap_or_else(|| {
		egui::collapsing_header::CollapsingState::load_with_default_open(ctx, id, false)
	});
	state.set_open(open);
	state.store(ctx);
}

/// Opens or closes every folder below `node`.
fn set_all(ctx: &egui::Context, scope: &str, node: &FolderTree, prefix: &str, open: bool) {
	let mut stack = vec![(node, prefix.to_owned())];
	while let Some((node, prefix)) = stack.pop() {
		for (name, child) in &node.folders {
			let path = child_path(&prefix, name);
			set_open(ctx, folder_id(scope, &path), open);
			stack.push((child, path));
		}
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
		changed(ui.ctx());
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn fixture() -> (FolderTree, FlatTree) {
		let paths: Vec<Vec<String>> = ["Zone10", "zone2/Deep", "zone2", "", "zone2"]
			.into_iter()
			.map(|path| {
				path.split('/')
					.filter(|s| !s.is_empty())
					.map(str::to_owned)
					.collect()
			})
			.collect();
		let tree = FolderTree::build(paths.iter().map(Vec::as_slice));
		let flat = FlatTree::build(&tree, "flat", |a, b| b.cmp(&a));
		(tree, flat)
	}

	#[test]
	fn flat_tree_sorts_once_and_keeps_subtree_ranges() {
		let (_, flat) = fixture();
		assert_eq!(flat.items, [1, 4, 2, 0, 3]);
		assert_eq!(flat.folder_items(0), [1, 4, 2]);
		assert_eq!(flat.folder_items(1), [1]);
		assert_eq!(flat.path(1), "zone2/Deep");
		assert_eq!(flat.rows[2].depth, 2);
		assert_eq!(flat.rows[2].parent, Some(1));
		let totals = flat.totals(|i| (i == 1 || i == 4) as usize);
		assert_eq!(flat.count(0, &totals), 2);
		assert_eq!(flat.count(1, &totals), 1);
	}

	#[test]
	fn flat_visibility_skips_closed_ancestors_and_empty_matches() {
		let (tree, flat) = fixture();
		let ctx = egui::Context::default();
		let matches = [true; 5];
		let totals = flat.totals(|_| 1);
		assert_eq!(flat.visible(&ctx, &matches, &totals, false), [0, 5, 7]);
		open_path(&ctx, "flat", &["zone2".into(), "Deep".into()]);
		assert_eq!(
			flat.visible(&ctx, &matches, &totals, false),
			[0, 1, 2, 3, 4, 5, 7]
		);
		FolderCommand::Subtree("zone2".into(), false).apply(&ctx, "flat", &tree);
		assert_eq!(flat.visible(&ctx, &matches, &totals, false), [0, 5, 7]);
		let matches = [false, true, false, false, false];
		let totals = flat.totals(|i| matches[i] as usize);
		assert_eq!(flat.visible(&ctx, &matches, &totals, true), [0, 1, 2]);
		assert!(is_open(&ctx, folder_id("flat", "zone2/Deep"), false));
		assert!(
			flat.visible(&ctx, &[false; 5], &flat.totals(|_| 0), true)
				.is_empty()
		);
	}

	#[test]
	fn revision_tracks_external_commands_and_default_open_sections() {
		let ctx = egui::Context::default();
		let id = folder_id("flat", "section");
		assert!(is_open(&ctx, id, true));
		let before = revision(&ctx);
		set_open(&ctx, id, false);
		assert!(revision(&ctx) > before);
		assert!(!is_open(&ctx, id, true));
		let before = revision(&ctx);
		set_open(&ctx, id, false);
		assert_eq!(
			revision(&ctx),
			before,
			"writing unchanged state does not invalidate caches"
		);
	}

	#[test]
	fn disclosure_buttons_keep_keyboard_activation_and_invalidate_visibility() {
		let ctx = egui::Context::default();
		let id = folder_id("keyboard", "A");
		let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
			header(ui, id, false, |ui| {
				ui.label("A");
			});
		});
		output.textures_delta.clear();
		ctx.memory_mut(|m| m.request_focus(id));
		let before = revision(&ctx);
		let mut output = ctx.run_ui(
			egui::RawInput {
				events: vec![egui::Event::Key {
					key: egui::Key::Space,
					physical_key: None,
					pressed: true,
					repeat: false,
					modifiers: Default::default(),
				}],
				..Default::default()
			},
			|ui| {
				header(ui, id, false, |ui| {
					ui.label("A");
				});
			},
		);
		output.textures_delta.clear();
		assert!(is_open(&ctx, id, false));
		assert!(revision(&ctx) > before);
	}

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

	/// A folder path of tens of thousands of parts is listed, expanded,
	/// collapsed and dropped without running out of stack.
	#[test]
	fn deep_trees_do_not_overflow_the_stack() {
		let deep: Vec<String> = (0..10_000).map(|i| format!("f{i}")).collect();
		let worker = std::thread::Builder::new()
			.stack_size(256 * 1024)
			.spawn(move || {
				let tree = FolderTree::build([deep.as_slice(), &deep[..2]]);
				assert_eq!(tree.all_items(), [1, 0]);
				let flat = FlatTree::build(&tree, "t", |a, b| a.cmp(&b));
				assert_eq!(flat.rows.len(), 10_002);
				assert_eq!(flat.rows.iter().map(|row| row.depth).max(), Some(10_000));
				assert_eq!(flat.folder_items(0), [0, 1]);
				let ctx = egui::Context::default();
				FolderCommand::All(true).apply(&ctx, "t", &tree);
				assert_eq!(
					flat.visible(&ctx, &[true, true], &flat.totals(|_| 1), false)
						.len(),
					10_002
				);
				FolderCommand::Subtree("f0/f1".into(), false).apply(&ctx, "t", &tree);
				drop(tree);
			})
			.unwrap();
		worker.join().unwrap();
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

		// unknown paths, wholly or partly, are ignored: no state for them and
		// no change to the known folders
		let known = [
			"Animals",
			"Animals/Diamond",
			"Animals/Diamond/Chicken",
			"Animals/Gold",
			"Eggs",
		];
		let before = known.map(is_open);
		for unknown in ["Nope/Missing", "Animals/Nope"] {
			FolderCommand::Subtree(unknown.into(), true).apply(&ctx, "t", &tree);
			assert!(
				egui::collapsing_header::CollapsingState::load(&ctx, folder_id("t", unknown))
					.is_none(),
				"{unknown} should get no state"
			);
			assert_eq!(known.map(is_open), before, "after {unknown}");
		}
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
