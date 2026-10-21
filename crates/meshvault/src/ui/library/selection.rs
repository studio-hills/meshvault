//! Selecting several rows: clicks with Shift and Ctrl, the selection bar,
//! and Ctrl+A/Esc.

use std::collections::BTreeSet;

use eframe::egui::{self, CornerRadius, RichText};

use super::menus::export_items;
use super::{Action, Browser, Key, Selection};
use crate::ui::theme::{CARD_RADIUS, CONTROL_XXS, palette};
use crate::ui::{icons, widgets};
use crate::workspace::Workspace;

impl Selection {
	/// A click on `key` with these modifiers; returns the row to open.
	/// Shift selects the rows in `order` from the anchor (added to the
	/// selection with Ctrl too) and leaves the anchor where it is; without
	/// an anchor in `order`, it counts as a click without Shift.
	pub(super) fn click(&mut self, key: Key, shift: bool, command: bool) -> Option<Key> {
		if shift
			&& let Some(anchor) = self.anchor
			&& let (Some(a), Some(b)) = (
				self.order.iter().position(|k| *k == anchor),
				self.order.iter().position(|k| *k == key),
			) {
			if !command {
				self.keys.clear();
			}
			let range: Vec<Key> = self.order[a.min(b)..=a.max(b)].to_vec();
			self.keys.extend(range);
			return None;
		}
		self.anchor = Some(key);
		if command {
			if !self.keys.remove(&key) {
				self.keys.insert(key);
			}
			return None;
		}
		self.keys = BTreeSet::from([key]);
		Some(key)
	}
}

impl Browser {
	/// A click on a row: a plain click opens it and selects only it, Ctrl
	/// adds or takes it away, and Shift selects the rows from the last one
	/// clicked.
	pub(super) fn click(&self, ui: &egui::Ui, key: Key, actions: &mut Vec<Action>) {
		let m = ui.input(|i| i.modifiers);
		let open = self.selection.borrow_mut().click(key, m.shift, m.command);
		if let Some(key) = open {
			actions.push(match key {
				Key::Model(i) => Action::OpenModel(i),
				Key::Item(i) => Action::Open(i),
			});
		}
	}

	pub(super) fn is_selected(&self, key: Key) -> bool {
		self.selection.borrow().keys.contains(&key)
	}

	/// Notes a row as drawn, for Shift-click ranges and Ctrl+A.
	pub(super) fn drawn(&self, key: Key) {
		self.selection.borrow_mut().drawn.push(key);
	}

	/// The selected meshes and models.
	pub(super) fn selected(&self) -> (Vec<usize>, Vec<usize>) {
		let s = self.selection.borrow();
		let items = s
			.keys
			.iter()
			.filter_map(|k| match k {
				Key::Item(i) => Some(*i),
				Key::Model(_) => None,
			})
			.collect();
		let models = s
			.keys
			.iter()
			.filter_map(|k| match k {
				Key::Model(i) => Some(*i),
				Key::Item(_) => None,
			})
			.collect();
		(items, models)
	}

	/// The failed downloads among the selection: its meshes, and the
	/// meshes its models use.
	pub(super) fn selected_failures(&self, ws: &Workspace) -> Vec<usize> {
		let (items, models) = self.selected();
		let mut failed: BTreeSet<usize> = items
			.into_iter()
			.filter(|&i| ws.items()[i].has_failure())
			.collect();
		for m in models {
			failed.extend(
				self.model_items(ws, &ws.models()[m])
					.filter(|(_, item)| item.has_failure())
					.map(|(i, _)| i),
			);
		}
		failed.into_iter().collect()
	}

	/// A right-click on a row in a selection of several acts on all of them.
	pub(super) fn in_multi_selection(&self, key: Key) -> bool {
		let s = self.selection.borrow();
		s.keys.len() > 1 && s.keys.contains(&key)
	}

	/// The bar over the library while several models are selected.
	pub(super) fn selection_bar(
		&self,
		ui: &mut egui::Ui,
		ws: &Workspace,
		actions: &mut Vec<Action>,
	) {
		let n = self.selection.borrow().keys.len();
		if n < 2 {
			return;
		}
		let p = palette(ui.ctx());
		egui::Frame::NONE
			.fill(p.accent_soft)
			.corner_radius(CornerRadius::same(CARD_RADIUS))
			.inner_margin(egui::Margin::symmetric(10, 2))
			.show(ui, |ui| {
				// its buttons sit 2 px from its top and bottom edges
				widgets::nest(ui, CARD_RADIUS, 2.0);
				ui.horizontal(|ui| {
					ui.spacing_mut().interact_size.y = CONTROL_XXS;
					ui.label(RichText::new(format!("{n} selected")).color(p.accent));
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						ui.spacing_mut().item_spacing.x = 0.0;
						if widgets::icon_button(ui, icons::X, "Clear selection (Esc)").clicked() {
							self.selection.borrow_mut().keys.clear();
						}
						if widgets::danger_icon_button(ui, icons::TRASH, "Remove from workspace")
							.clicked()
						{
							let (items, models) = self.selected();
							actions.push(Action::RemoveSelection { items, models });
						}
						let export = widgets::icon_button(ui, icons::EXPORT, "Export");
						egui::Popup::menu(&export).show(|ui| {
							widgets::menu_style(ui);
							let (items, models) = self.selected();
							export_items(ui, &items, &models, actions);
						});
						let failed = self.selected_failures(ws);
						let retry = ui
							.add_enabled_ui(!failed.is_empty(), |ui| {
								widgets::icon_button(
									ui,
									icons::ARROWS_CLOCKWISE,
									&format!("Retry {} failed downloads", failed.len()),
								)
							})
							.inner;
						if retry.clicked() {
							actions.push(Action::Retry(failed));
						}
					});
				});
			});
		ui.add_space(4.0);
	}

	/// Ctrl+A selects every row shown, Esc clears the selection.
	pub(super) fn selection_keys(&self, ui: &egui::Ui) {
		if !ui.ui_contains_pointer() || ui.ctx().egui_wants_keyboard_input() {
			return;
		}
		let (all, clear) = ui.input_mut(|i| {
			(
				i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
				i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
			)
		});
		let mut s = self.selection.borrow_mut();
		if all {
			s.keys = s.order.iter().copied().collect();
		}
		if clear {
			s.keys.clear();
		}
	}
}
