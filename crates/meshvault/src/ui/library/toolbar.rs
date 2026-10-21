//! The filter button beside the library's search.

use eframe::egui::{self, Vec2};

use super::{Browser, Filters};
use crate::scene::maps::MapKind;
use crate::ui::theme::palette;
use crate::ui::{icons, widgets};
use crate::workspace::facts::Facts;

impl Browser {
	/// The filter button beside the search, with a popup of filters.
	pub(super) fn filter_button(&mut self, ui: &mut egui::Ui) {
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
}
