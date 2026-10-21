//! The bars above the library's rows: the heading with the view buttons,
//! the search with its filter button, and the status chips.

use eframe::egui::{self, FontId, RichText, Vec2};

use super::{Browser, Filters, Status, StatusFilter, View, status_colour, status_dot};
use crate::scene::maps::MapKind;
use crate::ui::theme::{CAPTION, CONTROL_XXS, TITLE, display, palette};
use crate::ui::tree;
use crate::ui::{icons, widgets};
use crate::workspace::facts::Facts;
use crate::workspace::{Item, Workspace};

impl Browser {
	/// The filter button beside the search, with a popup of filters.
	fn filter_button(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		let n = self.filters.count();
		let tip = if n == 0 {
			"Filters".to_string()
		} else {
			format!("Filters ({n} on)")
		};
		let response = widgets::icon_toggle(ui, icons::FUNNEL, &tip, n > 0);
		if n > 0 {
			// a dot on the button while a filter is on
			let at = response.rect.right_top() + Vec2::new(-7.0, 7.0);
			ui.painter().circle_filled(at, 3.0, p.accent);
		}
		let reading = self.facts.as_ref().map_or(0, Facts::reading);
		egui::Popup::menu(&response)
			.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
			.show(|ui| {
				widgets::menu_style(ui);
				ui.set_min_width(200.0);
				let f = &mut self.filters;
				ui.label(widgets::caption(ui, "Show only"));
				ui.checkbox(&mut f.animated, "Has animations");
				ui.checkbox(&mut f.skinned, "Skinned (bends with bones)");
				if f.skinned && reading > 0 {
					ui.label(widgets::caption(
						ui,
						format!("Reading meshes… {reading} left"),
					));
				}
				ui.checkbox(&mut f.missing, "Missing meshes");
				ui.add_space(4.0);
				ui.label(widgets::caption(ui, "Has a map"));
				for kind in MapKind::ALL {
					let mut on = f.maps.contains(&kind);
					if ui.checkbox(&mut on, kind.label()).changed() {
						if on {
							f.maps.insert(kind);
						} else {
							f.maps.remove(&kind);
						}
					}
				}
				ui.add_space(4.0);
				if ui
					.add_enabled(f.count() > 0, egui::Button::new("Clear filters"))
					.clicked()
				{
					*f = Filters::default();
				}
			});
	}

	/// The heading with the model count, and the view buttons beside it.
	pub(super) fn header(&mut self, ui: &mut egui::Ui, ws: &Workspace, open_stem: Option<&str>) {
		let p = palette(ui.ctx());
		let items = ws.items();
		ui.horizontal(|ui| {
			ui.label(RichText::new("Library").size(TITLE).family(display()));
			ui.label(RichText::new(items.len().to_string()).color(p.text_weak));
			if items.is_empty() && ws.models().is_empty() {
				return;
			}
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.spacing_mut().item_spacing.x = 0.0;
				ui.spacing_mut().interact_size.y = CONTROL_XXS;
				if self.view == View::List {
					if let Some(command) = tree::expand_collapse_buttons(ui) {
						self.pending.replace(Some(command));
					}
					let reveal = ui
						.add_enabled_ui(open_stem.is_some(), |ui| {
							widgets::icon_button(
								ui,
								icons::CROSSHAIR_SIMPLE,
								"Reveal the open model",
							)
						})
						.inner;
					if reveal.clicked() {
						self.reveal = true;
					}
				}
				let (icon, tip, other) = match self.view {
					View::List => (icons::SQUARES_FOUR, "Show thumbnails", View::Grid),
					View::Grid => (icons::LIST, "Show the folder tree", View::List),
				};
				if widgets::icon_button(ui, icon, tip).clicked() {
					self.view = other;
				}
			});
		});
	}

	/// The search, with a filter button (and a clear button) beside it, the
	/// same height.
	pub(super) fn search_row(&mut self, ui: &mut egui::Ui) {
		ui.horizontal(|ui| {
			let button = ui.spacing().interact_size.y + ui.spacing().item_spacing.x;
			let buttons = if self.filter.is_empty() { 1.0 } else { 2.0 };
			ui.add(
				widgets::input(&mut self.filter)
					.hint_text(format!(
						"{}  Search name, folder or id",
						icons::MAGNIFYING_GLASS
					))
					.desired_width(ui.available_width() - button * buttons),
			);
			if !self.filter.is_empty()
				&& widgets::icon_button(ui, icons::X, "Clear search").clicked()
			{
				self.filter.clear();
			}
			self.filter_button(ui);
		});
	}

	/// The status chips: every mesh, then the done, pending and failed ones
	/// (failed only while there are some, or while it is the filter).
	pub(super) fn status_chips(&mut self, ui: &mut egui::Ui, items: &[Item]) {
		let p = palette(ui.ctx());
		let count = |f: fn(&Item) -> bool| items.iter().filter(|i| f(i)).count();
		let done = count(Item::is_done);
		let failed = count(Item::has_failure);
		let pending = count(|i| i.has_pending() && !i.has_failure());
		ui.horizontal_wrapped(|ui| {
			ui.spacing_mut().item_spacing.x = 4.0;
			ui.spacing_mut().interact_size.y = CONTROL_XXS;
			ui.spacing_mut().button_padding = egui::vec2(10.0, 4.0);
			// room before the label for the chip's status marker
			let font = FontId::proportional(CAPTION);
			let space = ui
				.painter()
				.layout_no_wrap(" ".into(), font, p.text)
				.size()
				.x;
			let gap = " ".repeat((13.0 / space.max(1.0)).ceil() as usize);
			for (value, label, n, marker) in [
				(StatusFilter::All, "All", items.len(), None),
				(StatusFilter::Downloaded, "Done", done, Some(Status::Done)),
				(
					StatusFilter::Pending,
					"Pending",
					pending,
					Some(Status::Pending),
				),
				(StatusFilter::Failed, "Failed", failed, Some(Status::Failed)),
			] {
				if value == StatusFilter::Failed && n == 0 && self.status != StatusFilter::Failed {
					continue;
				}
				let text = match marker {
					Some(s) => RichText::new(format!("{gap}{label} {n}"))
						.size(CAPTION)
						.color(status_colour(&p, s)),
					None => RichText::new(format!("{label} {n}")).size(CAPTION),
				};
				let response = ui.selectable_value(&mut self.status, value, text);
				// the same marker as the rows and tiles this chip filters
				if let Some(s) = marker {
					let padding = ui.spacing().button_padding.x;
					let centre = response.rect.left_center() + Vec2::new(padding + 3.5, 0.0);
					status_dot(ui.painter(), &p, centre, s);
				}
			}
		});
	}
}
