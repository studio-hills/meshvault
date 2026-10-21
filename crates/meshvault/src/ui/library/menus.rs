//! The rows' context menus and hover text.

use eframe::egui;

use super::{Action, Browser, Row};
use crate::export::Format;
use crate::ui::{icons, widgets};
use crate::workspace::{AssetState, Item, ModelItem, Workspace};

/// The two export items of a menu, for `items` and `models`. The labels
/// say how many things they export when it is not 1.
pub(super) fn export_items(
	ui: &mut egui::Ui,
	items: &[usize],
	models: &[usize],
	actions: &mut Vec<Action>,
) {
	export_items_labelled(ui, items, models, items.len() + models.len(), actions);
}

/// [`export_items`], with the count in the labels given: the selection bar
/// labels them with the count it showed at the start of the frame, which
/// stays put for the frame in which Clear empties the selection.
pub(super) fn export_items_labelled(
	ui: &mut egui::Ui,
	items: &[usize],
	models: &[usize],
	n: usize,
	actions: &mut Vec<Action>,
) {
	let what = if n == 1 {
		String::new()
	} else {
		format!("{n} ")
	};
	let blender = widgets::blender_item(ui, format!("Export {what}for Blender (.glb)…"))
		.on_hover_text("Binary glTF with the skeleton, skin weights, clips and maps");
	let obj = ui.button(format!("{}  Export {what}as OBJ…", icons::EXPORT));
	for (response, format) in [(blender, Format::Glb), (obj, Format::Obj)] {
		if response.clicked() {
			actions.push(Action::Export {
				items: items.to_vec(),
				models: models.to_vec(),
				format,
			});
			ui.close();
		}
	}
}

fn describe(state: &AssetState) -> String {
	match state {
		AssetState::Pending => "not downloaded yet".into(),
		AssetState::Done { path } => path.clone(),
		AssetState::Failed { error } => format!("failed: {error}"),
	}
}

pub(super) fn item_hover(item: &Item) -> String {
	let mut hover = format!("Mesh {}: {}", item.mesh_id, describe(&item.mesh));
	for (kind, map) in &item.maps {
		hover.push_str(&format!(
			"\n{} {}: {}",
			kind.label(),
			map.id,
			describe(&map.state)
		));
	}
	hover
}

impl Browser {
	/// The actions on the whole selection, for its context menu.
	fn selection_menu(&self, ui: &mut egui::Ui, ws: &Workspace, actions: &mut Vec<Action>) {
		widgets::menu_style(ui);
		let n = self.selection.borrow().keys.len();
		let failed = self.selected_failures(ws);
		if ui
			.add_enabled(
				!failed.is_empty(),
				egui::Button::new(format!(
					"{}  Retry {} failed",
					icons::ARROWS_CLOCKWISE,
					failed.len()
				)),
			)
			.clicked()
		{
			actions.push(Action::Retry(failed));
			ui.close();
		}
		let (items, models) = self.selected();
		export_items(ui, &items, &models, actions);
		if ui
			.button(format!("{}  Clear selection", icons::X))
			.clicked()
		{
			self.selection.borrow_mut().keys.clear();
			ui.close();
		}
		ui.separator();
		if widgets::danger_item(ui, format!("{}  Remove {n} from workspace", icons::TRASH))
			.clicked()
		{
			let (items, models) = self.selected();
			actions.push(Action::RemoveSelection { items, models });
			ui.close();
		}
	}

	pub(super) fn model_menu(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		i: usize,
		actions: &mut Vec<Action>,
	) {
		if self.in_multi_selection(Row::Model(i)) {
			self.selection_menu(ui, ws, actions);
			return;
		}
		let model = &ws.models()[i];
		widgets::menu_style(ui);
		if ui.button("Open").clicked() {
			actions.push(Action::OpenModel(i));
			ui.close();
		}
		if ui.button("Show rig file in file browser").clicked() {
			actions.push(Action::Reveal(ws.root.join(&model.file)));
			ui.close();
		}
		if ui.button("Copy instance path").clicked() {
			ui.ctx().copy_text(model.path.clone());
			ui.close();
		}
		export_items(ui, &[], &[i], actions);
		ui.separator();
		if widgets::danger_item(ui, format!("{}  Remove from workspace", icons::TRASH)).clicked() {
			actions.push(Action::RemoveModels(vec![i]));
			ui.close();
		}
	}

	pub(super) fn item_menu(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		i: usize,
		actions: &mut Vec<Action>,
	) {
		if self.in_multi_selection(Row::Mesh(i)) {
			self.selection_menu(ui, ws, actions);
			return;
		}
		let item = &ws.items()[i];
		widgets::menu_style(ui);
		if ui.button("Open").clicked() {
			actions.push(Action::OpenItem(i));
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
		if item.is_done() {
			export_items(ui, &[i], &[], actions);
		}
		ui.separator();
		if widgets::danger_item(ui, format!("{}  Remove from workspace", icons::TRASH)).clicked() {
			actions.push(Action::RemoveItems(vec![i]));
			ui.close();
		}
	}

	pub(super) fn model_hover(&self, ws: &Workspace, model: &ModelItem) -> String {
		let missing = model.meshes.len()
			- self
				.model_items(ws, model)
				.filter(|(_, it)| matches!(it.mesh, AssetState::Done { .. }))
				.count();
		let mut hover = format!(
			"{}\n{} parts, {} meshes\n{} of {} clips playable",
			model.path,
			model.parts,
			model.meshes.len(),
			model.playable_clips,
			model.clips
		);
		if missing > 0 {
			hover.push_str(&format!("\n{missing} meshes not downloaded yet"));
		}
		hover
	}
}
