//! The animated model view: the posed model, a transport bar to play its
//! clips, and its inspector (clips, parts and warnings).

use eframe::egui::{
	self, Align, Color32, CornerRadius, Frame, Id, Layout, Margin, Order, RichText, Stroke, Vec2,
};

use super::theme::{BODY_LINE_HEIGHT, CONTROL_RADIUS, ROW, palette, semibold};
use super::{icons, widgets};
use crate::anim::schema::ClipStatus;
use crate::app::App;
use crate::render::viewport;
use crate::scene::geometry::Primitive;
use crate::scene::model_view::{ModelView, PartShape};

/// Corner radius of the transport bar; the controls in it are rounded to
/// this less its padding and border.
const TRANSPORT_RADIUS: u8 = CONTROL_RADIUS + 4;

const SPEEDS: [f32; 6] = [0.1, 0.25, 0.5, 1.0, 1.5, 2.0];

fn speed_label(speed: f32) -> String {
	format!("{}×", (speed * 100.0).round() / 100.0)
}

fn status_colour(p: &super::theme::Palette, status: ClipStatus) -> Color32 {
	if status.is_playable() {
		p.accent
	} else {
		p.warning
	}
}

impl App {
	pub(super) fn model_center(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		let mut settings = self.view_settings(&p);
		let renderer = self.gpu.as_deref();
		let plain = (!self.show_maps).then_some(&self.no_material);
		let Some(model) = &mut self.model else { return };
		settings.lights = model.scene_lights();
		let dt = ui.input(|i| i.stable_dt).min(0.1);
		if model.tick(dt) {
			ui.ctx().request_repaint();
		}
		let home = model.home();
		let mut camera = std::mem::replace(&mut model.camera, home.clone());
		let rect = {
			let mut parts = model.scene_parts(plain);
			viewport::show(ui, &mut parts, &mut camera, &settings, renderer, &|| {
				home.clone()
			})
		};
		if self.show_skeleton || self.show_joints {
			let skeleton = model.skeleton();
			draw_skeleton(
				ui,
				&p,
				rect,
				&camera,
				&skeleton,
				self.show_skeleton,
				self.show_joints,
			);
		}
		model.camera = camera;
		self.viewport = rect;
		self.viewport_toolbar(ui.ctx(), rect);
		self.transport_bar(ui.ctx(), rect);
	}

	/// Play controls over the bottom of the viewport.
	fn transport_bar(&mut self, ctx: &egui::Context, rect: egui::Rect) {
		let p = palette(ctx);
		let Some(model) = &mut self.model else { return };
		let width = (rect.width() - 24.0).min(760.0);
		egui::Area::new(Id::unique("transport_bar"))
			.fixed_pos(egui::pos2(
				rect.center().x - width / 2.0,
				rect.bottom() - 60.0,
			))
			.order(Order::Middle)
			.show(ctx, |ui| {
				Frame::NONE
					.fill(p.panel.gamma_multiply(0.94))
					.stroke(Stroke::new(1.0, p.border))
					// the controls inside are nested to this radius less padding
					// and border (see `TRANSPORT_RADIUS`)
					.corner_radius(CornerRadius::same(TRANSPORT_RADIUS))
					.inner_margin(Margin::same(4))
					.shadow(ctx.global_style().visuals.popup_shadow)
					.show(ui, |ui| {
						ui.set_width(width - 8.0);
						ui.spacing_mut().interact_size.y = 30.0;
						// the controls' corners are concentric with the bar's
						widgets::nest(ui, TRANSPORT_RADIUS, 4.0 + 1.0);
						ui.horizontal(|ui| transport(ui, &p, model));
					});
			});
	}

	pub(super) fn model_inspector(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		let counterparts = self.counterparts();
		let mut switch = None;
		let Some(model) = &mut self.model else { return };
		widgets::scroll_in_margin(
			ui,
			egui::ScrollArea::vertical().auto_shrink([false, false]),
			|ui| {
				// the folder holding the model; the whole path is on hover
				let folder = model.path.rsplit('/').nth(1).unwrap_or_default();
				let subtitle = if folder.is_empty() {
					"Animated model".to_string()
				} else {
					format!("Animated model · {folder}")
				};
				switch = super::switch::header(
					ui,
					true,
					&counterparts,
					&model.name,
					&subtitle,
					&model.path,
				);
				widgets::stat_strip(
					ui,
					&[
						(model.parts.len().to_string(), "parts"),
						(model.rig.motors.len().to_string(), "joints"),
						(model.rig.bones.len().to_string(), "bones"),
						(
							super::inspector::format_count(model.triangle_count()),
							"triangles",
						),
					],
				);
				ui.add_space(10.0);

				if !model.warnings.is_empty() {
					let title = format!("Warnings · {}", model.warnings.len());
					widgets::section(ui, "model_warnings", &title, true, |ui| {
						for w in &model.warnings {
							ui.horizontal_top(|ui| {
								ui.label(
									RichText::new(icons::WARNING)
										.color(p.warning)
										.line_height(Some(BODY_LINE_HEIGHT)),
								);
								ui.add(
									egui::Label::new(
										RichText::new(w)
											.color(p.text_weak)
											.line_height(Some(BODY_LINE_HEIGHT)),
									)
									.wrap(),
								);
							});
						}
					});
				}

				let playable = model.clips.iter().filter(|c| c.clip.is_ok()).count();
				let title = format!("Animations · {playable}/{}", model.clips.len());
				widgets::section(ui, "model_clips", &title, true, |ui| {
					clip_list(ui, &p, model)
				});
				if let Some(slot) = model.current.and_then(|i| model.clips.get(i)) {
					widgets::section(ui, "model_clip_info", "Clip", true, |ui| {
						let clip = slot.clip.as_ref().ok();
						let mut rows = vec![
							("Type".to_string(), slot.status.label().to_string()),
							(
								"Duration".to_string(),
								format!("{:.2} s", clip.map_or(0.0, |c| c.duration)),
							),
							(
								"Looped".to_string(),
								if clip.is_some_and(|c| c.looped) {
									"Yes"
								} else {
									"No"
								}
								.to_string(),
							),
						];
						if !slot.priority.is_empty() {
							rows.push(("Priority".into(), slot.priority.clone()));
						}
						let tracks = slot.binding.targets.len();
						rows.push((
							"Tracks bound".into(),
							format!(
								"{} of {tracks}",
								slot.binding.matched() + slot.mesh_bone_tracks
							),
						));
						if let Some(c) = clip {
							if !c.markers.is_empty() {
								rows.push(("Markers".into(), c.markers.len().to_string()));
							}
							if c.skipped_curves > 0 {
								rows.push(("Curves skipped".into(), c.skipped_curves.to_string()));
							}
						}
						if !slot.animation_id.is_empty() {
							rows.push(("Animation".into(), slot.animation_id.clone()));
						}
						widgets::properties(ui, "model_clip_grid", &rows);
						let unmatched = slot.binding.unmatched.len() - slot.mesh_bone_tracks;
						if unmatched > 0 {
							ui.add_space(4.0);
							ui.collapsing(
								RichText::new(format!(
									"{unmatched} tracks match nothing in this model"
								))
								.color(p.warning),
								|ui| {
									ui.label(
										RichText::new(
											"Usually attachments, face bones or parts the export did not include.",
										)
										.small()
										.color(p.text_weak),
									);
									let mut names = slot.binding.unmatched.clone();
									names.sort();
									names.dedup();
									ui.label(names.join(", "));
								},
							);
						}
					});
				}

				widgets::section(ui, "model_properties", "Properties", true, |ui| {
					let mut rows = vec![(
						"Scale".to_string(),
						match model.scale {
							Some(s) => format!("{s}"),
							None => "Not in this export".into(),
						},
					)];
					rows.push((
						"Rest pose".to_string(),
						if model.captured_pose {
							"As exported (captured joints)".into()
						} else {
							"Joints at rest".into()
						},
					));
					widgets::properties(ui, "model_properties_grid", &rows);
				});

				widgets::section(ui, "model_display", "Display", true, |ui| {
					let blocks = model
						.parts
						.iter()
						.filter(|p| p.visible && matches!(p.shape, PartShape::Primitive(_)))
						.count();
					if blocks > 0 {
						widgets::pointer(ui.checkbox(
							&mut model.show_blocks,
							format!("Show parts without a mesh file ({blocks})"),
						));
					}
					if ui
						.add(egui::Button::new(format!(
							"{}  Reset to rest pose",
							icons::ARROW_COUNTER_CLOCKWISE
						)))
						.clicked()
					{
						model.reset();
					}
				});

				let title = format!("Parts · {}", model.parts.len());
				widgets::section(ui, "model_parts", &title, false, |ui| {
					part_list(ui, &p, model)
				});
			},
		);
		if let Some(s) = switch {
			self.apply_switch(s);
		}
	}
}

fn transport(ui: &mut egui::Ui, p: &super::theme::Palette, model: &mut ModelView) {
	ui.spacing_mut().item_spacing.x = 4.0;
	let has_clip = model.current.is_some();
	let (icon, tip) = if model.playing {
		(icons::PAUSE, "Pause  (Space)")
	} else {
		(icons::PLAY, "Play  (Space)")
	};
	let play = ui
		.add_enabled_ui(has_clip, |ui| widgets::icon_button(ui, icon, tip))
		.inner;
	if play.clicked() {
		model.toggle_play();
	}
	if ui
		.add_enabled_ui(has_clip, |ui| {
			widgets::icon_button(ui, icons::SKIP_BACK, "Back to the start")
		})
		.inner
		.clicked()
	{
		model.set_time(0.0);
	}
	// clip picker
	let current = model
		.current
		.and_then(|i| model.clips.get(i))
		.map_or("Rest pose".to_string(), |c| c.name.clone());
	ui.scope(|ui| {
		ui.set_max_width(160.0);
		let menu = ui.menu_button(
			RichText::new(format!("{current}  {}", icons::CARET_DOWN)).family(semibold()),
			|ui| {
				widgets::menu_style(ui);
				ui.set_min_width(220.0);
				if ui
					.selectable_label(model.current.is_none(), "Rest pose")
					.clicked()
				{
					model.reset();
					ui.close();
				}
				let mut pick = None;
				for (i, slot) in model.clips.iter().enumerate() {
					let enabled = slot.clip.is_ok();
					let label = if enabled {
						slot.name.clone()
					} else {
						format!("{}  ({})", slot.name, slot.status.label())
					};
					let response = ui.add_enabled(
						enabled,
						egui::Button::selectable(model.current == Some(i), label),
					);
					if response.clicked() {
						pick = Some(i);
						ui.close();
					}
				}
				if let Some(i) = pick {
					model.select(Some(i));
					model.playing = true;
				}
			},
		);
		menu.response.on_hover_text("Choose a clip");
	});

	let duration = model.duration();
	let time_text = format!("{:.2} / {:.2} s", model.time, duration);
	ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
		ui.menu_button(RichText::new(speed_label(model.speed)).size(12.5), |ui| {
			widgets::menu_style(ui);
			for s in SPEEDS {
				if ui
					.selectable_label((model.speed - s).abs() < 1e-3, speed_label(s))
					.clicked()
				{
					model.speed = s;
					ui.close();
				}
			}
		})
		.response
		.on_hover_text("Playback speed");
		if widgets::icon_toggle(ui, icons::REPEAT, "Loop", model.looping).clicked() {
			model.looping = !model.looping;
		}
		// room between the controls and the seeker with its time
		ui.add_space(12.0);
		// a slot as wide as the clip's own length, so the time (and the
		// scrubber beside it) does not shift as it counts
		let font = egui::FontId::new(12.0, semibold());
		let widest = format!("{duration:.2} / {duration:.2} s");
		let size = widgets::tabular_size(ui, &widest, &font);
		let (slot, _) = ui.allocate_exact_size(size, egui::Sense::hover());
		widgets::paint_tabular(
			ui.painter(),
			slot.right_center(),
			egui::Align2::RIGHT_CENTER,
			&time_text,
			&font,
			p.text_weak,
		);
		ui.add_space(4.0);
		// the scrubber takes the remaining width
		ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
			let width = ui.available_width().max(60.0);
			ui.spacing_mut().slider_width = width;
			let mut t = model.time;
			let slider = ui.add_enabled(
				has_clip && duration > 0.0,
				egui::Slider::new(&mut t, 0.0..=duration.max(1e-3)).show_value(false),
			);
			let slider = widgets::pointer(slider);
			if slider.changed() {
				model.set_time(t);
			}
			if slider.drag_started() {
				model.playing = false;
			}
			// marker ticks under the track
			if duration > 0.0 {
				let r = slider.rect;
				let painter = ui.painter();
				let mut hovered = None;
				let pointer = ui.ctx().pointer_hover_pos();
				for m in model.markers() {
					let x = r.left() + (m.time / duration).clamp(0.0, 1.0) * r.width();
					let top = egui::pos2(x, r.bottom() - 4.0);
					painter.line_segment(
						[top, top + Vec2::new(0.0, 5.0)],
						Stroke::new(2.0, p.warning),
					);
					if pointer.is_some_and(|q| (q.x - x).abs() < 4.0 && r.expand(4.0).contains(q)) {
						hovered = Some(m);
					}
				}
				if let Some(m) = hovered {
					let text = if m.value.is_empty() {
						format!("{}  {:.2} s", m.name, m.time)
					} else {
						format!("{} = {}  {:.2} s", m.name, m.value, m.time)
					};
					slider.on_hover_text(format!("{}  {text}", icons::FLAG));
				}
			}
		});
	});
}

fn clip_list(ui: &mut egui::Ui, p: &super::theme::Palette, model: &mut ModelView) {
	if model.clips.is_empty() {
		ui.label(RichText::new("This model has no animations.").color(p.text_weak));
		return;
	}
	ui.spacing_mut().interact_size.y = ROW;
	let mut pick = None;
	for (i, slot) in model.clips.iter().enumerate() {
		let selected = model.current == Some(i);
		let enabled = slot.clip.is_ok();
		ui.horizontal(|ui| {
			let mut text = RichText::new(&slot.name);
			if !enabled {
				text = text.color(p.text_weak);
			}
			let response =
				ui.add_enabled(enabled, egui::Button::selectable(selected, text).truncate());
			let response = match &slot.clip {
				Ok(c) => widgets::pointer(response).on_hover_text(format!(
					"{:.2} s, {} tracks",
					c.duration,
					c.tracks.len()
				)),
				Err(why) => response.on_disabled_hover_text(why.as_str()),
			};
			if response.clicked() {
				pick = Some(i);
			}
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				ui.label(
					RichText::new(slot.status.label())
						.size(crate::ui::theme::CAPTION)
						.color(status_colour(p, slot.status)),
				);
			});
		});
	}
	if let Some(i) = pick {
		model.select(Some(i));
		model.playing = true;
	}
}

/// The skeleton overlay: lines between joints and bones, and a mark at each
/// joint (accent for joints clips drive, grey for welds), drawn over the
/// model. Hovering a mark names it.
fn draw_skeleton(
	ui: &egui::Ui,
	p: &super::theme::Palette,
	rect: egui::Rect,
	camera: &viewport::Camera,
	skeleton: &crate::scene::model_view::Skeleton,
	bones: bool,
	joints: bool,
) {
	let painter = ui.painter_at(rect);
	let project = |at| viewport::project(camera, rect, at);
	let shadow = Color32::from_black_alpha(140);
	if bones {
		for &(a, b) in &skeleton.bones {
			if let (Some(a), Some(b)) = (project(a), project(b)) {
				painter.line_segment([a, b], Stroke::new(4.0, shadow));
				painter.line_segment([a, b], Stroke::new(2.0, p.warning));
			}
		}
	}
	let pointer = ui
		.input(|i| i.pointer.hover_pos())
		.filter(|at| rect.contains(*at));
	let mut nearest: Option<(f32, egui::Pos2, &crate::scene::model_view::Joint)> = None;
	for joint in &skeleton.joints {
		let Some(at) = project(joint.at) else {
			continue;
		};
		let radius = if joints { 4.5 } else { 2.5 };
		let fill = if !joints {
			p.warning
		} else if joint.animated {
			p.accent
		} else {
			p.text_weak
		};
		painter.circle(at, radius, fill, Stroke::new(1.5, shadow));
		if let Some(pointer) = pointer {
			let d = at.distance(pointer);
			if d < 9.0 && nearest.is_none_or(|(best, _, _)| d < best) {
				nearest = Some((d, at, joint));
			}
		}
	}
	if let Some((_, at, joint)) = nearest {
		let text = if joint.animated {
			joint.name.clone()
		} else {
			format!("{} (weld)", joint.name)
		};
		let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), p.text);
		let label = egui::Rect::from_min_size(
			at + Vec2::new(10.0, -10.0 - galley.size().y),
			galley.size() + Vec2::new(12.0, 6.0),
		);
		painter.rect_filled(label, CornerRadius::same(5), p.panel);
		painter.rect_stroke(
			label,
			CornerRadius::same(5),
			Stroke::new(1.0, p.border),
			egui::StrokeKind::Inside,
		);
		painter.galley(label.min + Vec2::new(6.0, 3.0), galley, p.text);
	}
}

fn part_list(ui: &mut egui::Ui, p: &super::theme::Palette, model: &mut ModelView) {
	ui.horizontal(|ui| {
		widgets::compact(ui);
		if widgets::text_button(ui, "Show all").clicked() {
			model.parts.iter_mut().for_each(|p| p.visible = true);
		}
		if widgets::text_button(ui, "Hide all").clicked() {
			model.parts.iter_mut().for_each(|p| p.visible = false);
		}
	});
	widgets::scroll_in_margin(
		ui,
		egui::ScrollArea::vertical()
			.id_salt("model_part_list")
			.max_height(320.0),
		|ui| {
			ui.spacing_mut().interact_size.y = ROW;
			let ModelView {
				parts,
				rig,
				highlight,
				..
			} = model;
			// the part under the pointer stands out in the view
			*highlight = None;
			for part in parts.iter_mut() {
				let rp = &rig.parts[part.index];
				let (tag, colour, hover) = match &part.shape {
					PartShape::Mesh(asset) => match &part.skin {
						Some(s) => (
							"skinned",
							p.accent,
							format!(
								"{}\n{} of {} bones matched in the model",
								asset.name, s.matched, s.bones
							),
						),
						None => ("mesh", p.text_weak, asset.name.clone()),
					},
					PartShape::Primitive(kind) => match &part.approximate {
						Some(why) => ("approx.", p.warning, format!("Drawn approximately: {why}")),
						None => (
							match kind {
								Primitive::Block => "block",
								Primitive::Ball => "ball",
								Primitive::CylinderX => "cylinder",
								Primitive::Head => "head",
								Primitive::Truss { .. } => "truss",
								Primitive::Wedge => "wedge",
								Primitive::CornerWedge => "corner wedge",
							},
							p.text_weak,
							"No file mesh: a Roblox primitive shape".to_string(),
						),
					},
					PartShape::Missing(why) => ("missing", p.danger, why.clone()),
				};
				let joint = rig
					.motors
					.iter()
					.find(|m| m.part1 == part.index)
					.map_or(String::new(), |m| {
						format!("\nMoved by {} from {}", m.name, rig.parts[m.part0].name)
					});
				let appearance = part
					.appearance
					.as_ref()
					.map_or(String::new(), |a| format!("\n{a}"));
				let row = ui.horizontal(|ui| {
					let response = widgets::pointer(ui.checkbox(&mut part.visible, &rp.name));
					response.on_hover_text(format!(
						"{}\n{hover}{joint}{appearance}\nTransparency {:.2}",
						rp.path, rp.transparency
					));
					ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
						ui.label(
							RichText::new(tag)
								.size(crate::ui::theme::CAPTION)
								.color(colour),
						);
					});
				});
				if ui.rect_contains_pointer(row.response.rect) {
					*highlight = Some(part.index);
				}
			}
		},
	);
}
