//! Keyboard shortcuts, and the table of them the help dialog shows. They
//! live together so that the two stay in step.

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};

use crate::app::App;
use crate::render::lighting::Lighting;
use crate::render::viewport::Shading;

impl App {
	pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
		let shortcut = |modifiers, key| KeyboardShortcut::new(modifiers, key);
		let cmd = Modifiers::COMMAND;
		let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
		let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
		// shifted variants first so they are not taken by the plain ones
		if pressed(shortcut(cmd_shift, Key::O)) {
			self.pick_open_workspace();
		}
		if pressed(shortcut(cmd_shift, Key::E)) {
			self.export_obj();
		}
		if pressed(shortcut(cmd_shift, Key::B)) {
			self.show_inspector = !self.show_inspector;
		}
		if pressed(shortcut(cmd, Key::O)) {
			self.pick_file();
		}
		if pressed(shortcut(cmd, Key::N)) {
			self.show_new_workspace(None);
		}
		if pressed(shortcut(cmd, Key::I)) && self.workspace.is_some() {
			self.pick_import();
		}
		if pressed(shortcut(cmd, Key::D)) {
			self.start_downloads();
		}
		if pressed(shortcut(cmd, Key::E)) {
			self.export_glb();
		}
		if pressed(shortcut(cmd, Key::B)) && self.has_library() {
			self.show_library = !self.show_library;
		}
		if pressed(shortcut(cmd, Key::Comma)) {
			self.open_settings(None);
		}
		// single keys only while no text field has focus
		if ctx.egui_wants_keyboard_input() {
			return;
		}
		let key = |k| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
		if key(Key::F) {
			self.frame_model();
		}
		if let Some(model) = &mut self.model
			&& key(Key::Space)
		{
			model.toggle_play();
		}
		if key(Key::Num1) {
			self.shading = Shading::Solid;
		}
		if key(Key::Num2) {
			self.shading = Shading::SolidWireframe;
		}
		if key(Key::Num3) {
			self.shading = Shading::Wireframe;
		}
		if key(Key::M) {
			self.show_maps = !self.show_maps;
		}
		if key(Key::G) {
			self.show_grid = !self.show_grid;
		}
		if key(Key::K) {
			self.show_skeleton = !self.show_skeleton;
		}
		if key(Key::J) {
			self.show_joints = !self.show_joints;
		}
		if key(Key::A) {
			self.show_attachments = !self.show_attachments;
		}
		if key(Key::P) {
			self.show_effects = !self.show_effects;
		}
		if key(Key::L) {
			let all = Lighting::ALL;
			let next = all
				.iter()
				.position(|l| *l == self.config.lighting)
				.map_or(0, |i| (i + 1) % all.len());
			self.set_lighting(all[next]);
		}
		if key(Key::F12) {
			self.save_image();
		}
		if ctx.input_mut(|i| {
			i.consume_key(Modifiers::SHIFT, Key::Questionmark)
				|| i.consume_key(Modifiers::NONE, Key::Questionmark)
		}) {
			self.show_shortcuts = true;
		}
	}
}

/// Keyboard shortcuts, for the help dialog, in the sections of
/// docs/shortcuts.md.
pub(super) const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
	(
		"Files and workspaces",
		&[
			("Ctrl+O", "Open a file"),
			("Ctrl+Shift+O", "Open a workspace"),
			("Ctrl+N", "New workspace"),
			("Ctrl+I", "Import an export .json"),
			("Ctrl+D", "Download pending"),
			("Ctrl+E", "Export for Blender (.glb)"),
			("Ctrl+Shift+E", "Export as OBJ"),
			("Ctrl+,", "Roblox access"),
		],
	),
	(
		"Panels and view",
		&[
			("Ctrl+B", "Toggle library"),
			("Ctrl+Shift+B", "Toggle inspector"),
			("F", "Frame model"),
			("Space", "Play or pause the animation"),
			("1 / 2 / 3", "Solid / solid + wireframe / wireframe"),
			("M", "Toggle maps"),
			("G", "Toggle ground grid"),
			("K / J", "Toggle the skeleton / joints"),
			("L", "Next lighting preset"),
			("F12", "Save the view as an image"),
			("?", "Show this list"),
		],
	),
];
