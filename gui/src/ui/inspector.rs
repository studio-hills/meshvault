//! The right-hand panel describing the open model and its material.

use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, Sense, Stroke, Vec2};

use super::theme::{ROW, palette, semibold};
use super::{icons, widgets};
use crate::app::{App, format_bytes};
use crate::geometry::GroupKind;
use crate::maps::Slot;
use crate::ui::ConfirmAction;

impl App {
	pub(super) fn inspector(&mut self, ui: &mut egui::Ui) {
		if self.model.is_some() {
			self.model_inspector(ui);
			return;
		}
		let p = palette(ui.ctx());
		let counterparts = self.counterparts();
		let mut switch = None;
		let mut pick = None;
		let mut remove_map = None;
		widgets::scroll_in_margin(
			ui,
			egui::ScrollArea::vertical().auto_shrink([false, false]),
			|ui| {
				let Some(doc) = &mut self.doc else { return };
				let geometry = &doc.loaded.geometry;

				let mut subtitle = doc.loaded.format.clone();
				if self.open_stem.is_some() {
					subtitle.push_str(" · in workspace");
				}
				let path = doc.path.display().to_string();
				switch =
					super::switch::header(ui, false, &counterparts, &doc.name(), &subtitle, &path);
				widgets::card(ui, |ui| {
					ui.columns(3, |cols| {
						widgets::stat(
							&mut cols[0],
							&format_count(geometry.positions.len()),
							"vertices",
						);
						widgets::stat(
							&mut cols[1],
							&format_count(geometry.triangles.len()),
							"triangles",
						);
						let (n, what) = match geometry.group_kind {
							GroupKind::Lod => (geometry.groups.len(), "LODs"),
							GroupKind::Hull => (geometry.groups.len(), "hulls"),
							GroupKind::Whole => (doc.material.maps.len(), "maps"),
						};
						widgets::stat(&mut cols[2], &n.to_string(), what);
					});
				});
				if let Some(note) = &doc.loaded.note {
					ui.add_space(6.0);
					ui.horizontal_wrapped(|ui| {
						ui.label(RichText::new(icons::INFO).color(p.accent));
						ui.label(RichText::new(note).color(p.text_weak));
					});
				}
				ui.add_space(10.0);

				// material
				if !geometry.triangles.is_empty() {
					widgets::section(ui, "inspector_material", "Material", true, |ui| {
						if geometry.uvs.is_none() {
							ui.label(
								RichText::new(
									"This format has no texture coordinates, so maps cannot be applied.",
								)
								.color(p.text_weak),
							);
							return;
						}
						let mut toggle = None;
						let mut remove = None;
						for slot in Slot::ALL {
							map_row(ui, slot, &doc.material, &mut pick, &mut toggle, &mut remove);
						}
						if self.gpu.is_none()
							&& [Slot::Normal, Slot::Roughness, Slot::Metalness]
								.iter()
								.any(|s| doc.material.maps.contains_key(s))
						{
							ui.label(
								RichText::new(
									"GPU rendering is unavailable, so only the base color is shown.",
								)
								.small()
								.color(p.warning),
							);
						}
						if let Some((slot, on)) = toggle {
							doc.material.toggle(slot, on);
						}
						if remove.is_some() {
							remove_map = remove;
						}
					});
				}

				// levels of detail / hulls
				let groups = &doc.loaded.geometry.groups;
				match doc.loaded.geometry.group_kind {
					GroupKind::Lod if groups.len() > 1 => {
						widgets::section(ui, "inspector_lods", "Level of detail", true, |ui| {
							widgets::compact(ui);
							for (i, range) in groups.iter().enumerate() {
								let label = format!("LOD {i}");
								ui.horizontal(|ui| {
									ui.selectable_value(&mut doc.lod, i, label);
									ui.with_layout(
										egui::Layout::right_to_left(egui::Align::Center),
										|ui| {
											ui.label(
												RichText::new(format!("{} tris", range.len()))
													.color(p.text_weak),
											);
										},
									);
								});
							}
						});
					}
					GroupKind::Hull if !groups.is_empty() => {
						let title = format!("Convex hulls · {}", groups.len());
						widgets::section(ui, "inspector_hulls", &title, true, |ui| {
							ui.horizontal(|ui| {
								widgets::compact(ui);
								if widgets::text_button(ui, "Show all").clicked() {
									doc.hull_visible.fill(true);
								}
								if widgets::text_button(ui, "Hide all").clicked() {
									doc.hull_visible.fill(false);
								}
							});
							egui::ScrollArea::vertical()
								.id_salt("hull_list")
								.content_margin(egui::Margin {
									right: 10,
									..egui::Margin::ZERO
								})
								.max_height(260.0)
								.show(ui, |ui| {
									ui.spacing_mut().interact_size.y = ROW;
									for (i, range) in groups.iter().enumerate() {
										widgets::pointer(ui.checkbox(
											&mut doc.hull_visible[i],
											format!("Hull {i}  ·  {} tris", range.len()),
										));
									}
								});
						});
					}
					_ => {}
				}

				// properties
				widgets::section(ui, "inspector_properties", "Properties", true, |ui| {
					let mut rows = vec![("File size".to_string(), format_bytes(doc.size))];
					if let Some((lo, hi)) = doc.loaded.geometry.bounds() {
						rows.push((
							"Dimensions".into(),
							format!(
								"{:.2} × {:.2} × {:.2}",
								hi[0] - lo[0],
								hi[1] - lo[1],
								hi[2] - lo[2]
							),
						));
					}
					rows.extend(doc.loaded.details.iter().cloned());
					widgets::properties(ui, "inspector_properties_grid", &rows);
				});
			},
		);
		if let Some(s) = switch {
			self.apply_switch(s);
		}
		if let Some(slot) = pick {
			self.pick_map(slot);
		}
		if let Some(slot) = remove_map {
			self.ask(ConfirmAction::RemoveMap(slot));
		}
	}
}

/// One material slot: thumbnail, name, and show/replace/remove controls.
fn map_row(
	ui: &mut egui::Ui,
	slot: Slot,
	material: &crate::texture::Material,
	pick: &mut Option<Slot>,
	toggle: &mut Option<(Slot, bool)>,
	remove: &mut Option<Slot>,
) {
	let p = palette(ui.ctx());
	Frame::NONE
		.fill(p.raised)
		.stroke(Stroke::new(1.0, p.border))
		.corner_radius(CornerRadius::same(8))
		.inner_margin(Margin::same(6))
		.show(ui, |ui| {
			ui.set_width(ui.available_width());
			// small icon buttons inside the card row
			ui.spacing_mut().interact_size.y = 30.0;
			ui.horizontal(|ui| {
				let thumb = Vec2::splat(40.0);
				match material.maps.get(&slot) {
					Some(texture) => {
						let on = !material.disabled.contains(&slot);
						// the uploaded image has white rows below it; crop them off
						let uv = egui::Rect::from_min_max(
							egui::Pos2::ZERO,
							egui::Pos2::new(1.0, texture.info.v_scale),
						);
						let image = |size| {
							egui::Image::new((texture.handle.id(), size))
								.uv(uv)
								.maintain_aspect_ratio(false)
								.corner_radius(CornerRadius::same(5))
						};
						ui.add(image(thumb).tint(if on {
							egui::Color32::WHITE
						} else {
							egui::Color32::from_gray(90)
						}))
						.on_hover_ui(|ui| {
							ui.add(image(Vec2::splat(256.0)));
						});
						// buttons first (right to left) so the text gets only the space left over
						ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
							ui.spacing_mut().item_spacing.x = 0.0;
							if widgets::danger_icon_button(ui, icons::TRASH, "Remove map").clicked()
							{
								*remove = Some(slot);
							}
							if widgets::icon_button(ui, icons::ARROWS_CLOCKWISE, "Replace…")
								.clicked()
							{
								*pick = Some(slot);
							}
							let (icon, tip) = if on {
								(icons::EYE, "Hide this map")
							} else {
								(icons::EYE_SLASH, "Show this map")
							};
							if widgets::icon_button(ui, icon, tip).clicked() {
								*toggle = Some((slot, !on));
							}
							ui.add_space(6.0);
							ui.vertical(|ui| {
								ui.spacing_mut().item_spacing.y = 1.0;
								ui.label(RichText::new(slot.label()).family(semibold()));
								let name = texture
									.path
									.file_name()
									.map(|s| s.to_string_lossy().into_owned())
									.unwrap_or_default();
								ui.add(
									egui::Label::new(
										RichText::new(format!(
											"{}×{} · {name}",
											texture.size[0], texture.size[1]
										))
										.size(11.0)
										.color(p.text_weak),
									)
									.truncate(),
								)
								.on_hover_text(texture.path.display().to_string());
							});
						});
					}
					None => {
						let (rect, _) = ui.allocate_exact_size(thumb, Sense::hover());
						widgets::dashed_rect(ui, rect, p.text_weak.gamma_multiply(0.55));
						ui.vertical(|ui| {
							ui.spacing_mut().item_spacing.y = 1.0;
							ui.label(RichText::new(slot.label()).color(p.text_weak));
							ui.label(RichText::new("No map").size(11.0).color(p.text_weak));
						});
						ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
							if widgets::icon_button(ui, icons::PLUS, "Load…").clicked() {
								*pick = Some(slot);
							}
						});
					}
				}
			});
		});
	ui.add_space(2.0);
}

/// 12345 -> "12,345"
fn format_count(n: usize) -> String {
	let s = n.to_string();
	let mut out = String::new();
	for (i, c) in s.chars().enumerate() {
		if i > 0 && (s.len() - i) % 3 == 0 {
			out.push(',');
		}
		out.push(c);
	}
	out
}

#[cfg(test)]
mod tests {
	#[test]
	fn formats_counts() {
		assert_eq!(super::format_count(0), "0");
		assert_eq!(super::format_count(999), "999");
		assert_eq!(super::format_count(1000), "1,000");
		assert_eq!(super::format_count(1234567), "1,234,567");
	}
}
