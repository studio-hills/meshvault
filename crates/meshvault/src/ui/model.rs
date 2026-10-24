//! The animated model view: the posed model, a transport bar to play its
//! clips, and its inspector (clips, parts and warnings).

use eframe::egui::{self, Align, Color32, CornerRadius, Id, Layout, Order, RichText, Stroke, Vec2};

use super::theme::{CAPTION, CONTROL_RADIUS, Palette, ROW, palette, semibold};
use super::{icons, widgets};
use crate::anim::schema::ClipStatus;
use crate::app::App;
use crate::render::camera::{Camera, project, project_axis};
use crate::render::viewport;
use crate::scene::model_view::{ClipSlot, Joint, ModelView, PartShape, PartView, Points, Skeleton};

/// Corner radius of the transport bar; the controls in it are rounded to
/// this less its padding and border.
const TRANSPORT_RADIUS: u8 = CONTROL_RADIUS + 4;
/// The transport bar's widest, however wide the viewport.
const TRANSPORT_MAX_WIDTH: f32 = 760.0;
/// The least room the transport bar leaves across the viewport, both sides
/// together.
const TRANSPORT_INSET: f32 = 24.0;
/// How far the transport bar's top sits above the viewport's bottom edge.
const TRANSPORT_LIFT: f32 = 60.0;

const SPEEDS: [f32; 6] = [0.1, 0.25, 0.5, 1.0, 1.5, 2.0];
/// Narrower than this, the transport bar drops its loop and speed controls.
const TRANSPORT_FULL: f32 = 520.0;
/// Narrower than this, it drops the time as well, so the scrubber keeps room.
const TRANSPORT_TIMED: f32 = 380.0;

/// What the transport bar has room for, besides play, back, the clip
/// picker and the scrubber.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransportFit {
	time: bool,
	loop_and_speed: bool,
}

impl TransportFit {
	fn for_width(width: f32) -> Self {
		Self {
			time: width >= TRANSPORT_TIMED,
			loop_and_speed: width >= TRANSPORT_FULL,
		}
	}
}

fn speed_label(speed: f32) -> String {
	format!("{}×", (speed * 100.0).round() / 100.0)
}

fn status_colour(p: &Palette, status: ClipStatus) -> Color32 {
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
		// effects run whether a clip plays or not, as in Studio; drawn by
		// the GPU renderer only
		if self.show_effects && renderer.is_some() && model.step_effects(dt) {
			settings.effects = model.effect_frame();
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
		// a part hovered in the parts list shows its pivot, overlay on or off
		if self.show_attachments || model.highlight.is_some() {
			let points = model.points();
			draw_points(ui, &p, rect, &camera, &points, self.show_attachments);
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
		let width = (rect.width() - TRANSPORT_INSET).min(TRANSPORT_MAX_WIDTH);
		egui::Area::new(Id::unique("transport_bar"))
			.fixed_pos(egui::pos2(
				rect.center().x - width / 2.0,
				rect.bottom() - TRANSPORT_LIFT,
			))
			.order(Order::Middle)
			.show(ctx, |ui| {
				// the controls inside are nested to this radius less padding and
				// border (see `TRANSPORT_RADIUS`)
				widgets::floating_bar(ui, TRANSPORT_RADIUS, |ui| {
					// the content, inside the bar's 4px margin on either side
					ui.set_width(width - 8.0);
					let fit = TransportFit::for_width(width);
					ui.horizontal(|ui| transport(ui, &p, model, fit));
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
				// as the library sorts it: animated when it has clips
				let kind = super::switch::model_kind(
					counterparts.animated.unwrap_or(!model.clips.is_empty()),
				);
				let subtitle = if folder.is_empty() {
					format!("{kind} model")
				} else {
					format!("{kind} model · {folder}")
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
						(model.rig.joints.len().to_string(), "joints"),
						(model.rig.bones.len().to_string(), "bones"),
						(
							super::format::format_count(model.triangle_count()),
							"triangles",
						),
					],
				);
				ui.add_space(10.0);

				if !model.warnings.is_empty() {
					let title = format!("Warnings · {}", model.warnings.len());
					widgets::section(ui, "model_warnings", &title, true, |ui| {
						warning_list(ui, &p, &model.warnings)
					});
				}

				let playable = model.clips.iter().filter(|c| c.clip.is_ok()).count();
				let title = format!("Animations · {playable}/{}", model.clips.len());
				widgets::section(ui, "model_clips", &title, true, |ui| {
					clip_list(ui, &p, model)
				});
				if let Some(slot) = model.current.and_then(|i| model.clips.get(i)) {
					widgets::section(ui, "model_clip_info", "Clip", true, |ui| {
						clip_details(ui, &p, slot)
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
					rows.push((
						"PrimaryPart".to_string(),
						match (model.has_pivot(), model.primary_part()) {
							(_, Some(name)) => name.to_string(),
							(true, None) => "None".into(),
							(false, None) => "Not in this export".into(),
						},
					));
					rows.push((
						"Attachments".to_string(),
						model.attachment_count().to_string(),
					));
					rows.push(("Effects".to_string(), model.effect_count().to_string()));
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

/// The model's load warnings, one note each.
fn warning_list(ui: &mut egui::Ui, p: &Palette, warnings: &[String]) {
	for w in warnings {
		widgets::note(
			ui,
			RichText::new(icons::WARNING).color(p.warning),
			RichText::new(w).color(p.text_weak),
		);
	}
}

/// What the chosen clip is and how much of it binds to this model.
fn clip_details(ui: &mut egui::Ui, p: &Palette, slot: &ClipSlot) {
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
	if let Some((count, names)) = unmatched_tracks(slot) {
		ui.add_space(4.0);
		ui.collapsing(
			RichText::new(format!("{count} tracks match nothing in this model")).color(p.warning),
			|ui| {
				ui.label(
					RichText::new(
						"Usually attachments, face bones or parts the export did not include.",
					)
					.small()
					.color(p.text_weak),
				);
				ui.label(names);
			},
		);
	}
}

/// How many of the clip's tracks move nothing, with their names, each
/// once in order; `None` when every track moves something. Tracks for
/// bones inside skinned meshes are bound, so neither part counts them.
fn unmatched_tracks(slot: &ClipSlot) -> Option<(usize, String)> {
	if slot.unmatched.is_empty() {
		return None;
	}
	let mut names = slot.unmatched.clone();
	names.sort();
	names.dedup();
	Some((slot.unmatched.len(), names.join(", ")))
}

fn transport(ui: &mut egui::Ui, p: &Palette, model: &mut ModelView, fit: TransportFit) {
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
	clip_picker(ui, model);

	let duration = model.duration();
	let time_text = format!("{:.2} / {:.2} s", model.time, duration);
	ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
		if fit.loop_and_speed {
			speed_menu(ui, model);
			if widgets::icon_toggle(ui, icons::REPEAT, "Loop", model.looping).clicked() {
				model.looping = !model.looping;
			}
			// room between the controls and the seeker with its time
			ui.add_space(12.0);
		}
		if fit.time {
			// a slot as wide as the clip's own length, so the time (and the
			// scrubber beside it) does not shift as it counts
			let font = egui::FontId::new(CAPTION, semibold());
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
		}
		// the scrubber takes the remaining width
		ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
			scrubber(ui, p, model, duration, has_clip)
		});
	});
}

/// The clip menu: the rest pose, then every clip (those that cannot play
/// are greyed, with why).
fn clip_picker(ui: &mut egui::Ui, model: &mut ModelView) {
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
}

/// Playback speed, as a menu of the `SPEEDS`.
fn speed_menu(ui: &mut egui::Ui, model: &mut ModelView) {
	// 12.5 is off the type scale on purpose; kept as it was
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
}

/// The seek slider across the remaining width, with a tick for each marker
/// (named on hover).
fn scrubber(ui: &mut egui::Ui, p: &Palette, model: &mut ModelView, duration: f32, has_clip: bool) {
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
}

fn clip_list(ui: &mut egui::Ui, p: &Palette, model: &mut ModelView) {
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
						.size(CAPTION)
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
	p: &Palette,
	rect: egui::Rect,
	camera: &Camera,
	skeleton: &Skeleton,
	bones: bool,
	joints: bool,
) {
	let painter = ui.painter_at(rect);
	let project = |at| project(camera, rect, at);
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
	let mut nearest: Option<(f32, egui::Pos2, &Joint)> = None;
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
		hover_label(&painter, p, at, text);
	}
}

/// A small label beside the overlay mark at `at`, naming it.
fn hover_label(painter: &egui::Painter, p: &Palette, at: egui::Pos2, text: String) {
	let galley = painter.layout_no_wrap(text, egui::FontId::proportional(CAPTION), p.text);
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

/// Studio's colours for an attachment: green, with an orange line along its
/// Axis (its X) and a yellow one along its SecondaryAxis (its Y).
const ATTACHMENT: Color32 = Color32::from_rgb(74, 222, 128);
const AXIS: Color32 = Color32::from_rgb(255, 170, 0);
const SECONDARY_AXIS: Color32 = Color32::from_rgb(255, 230, 80);
/// The pivot's X, Y and Z axes, as Studio colours them.
const PIVOT_AXES: [Color32; 3] = [
	Color32::from_rgb(232, 72, 77),
	Color32::from_rgb(70, 190, 90),
	Color32::from_rgb(62, 120, 230),
];

/// The attachments overlay, when `overlay` is on: each attachment as a dot
/// with short lines along its axes, and the model's pivot as three longer
/// axes. Attachments Studio hides until selected (Visible off) are drawn
/// fainter. The pivot of a part highlighted in the parts list is drawn
/// either way, a little smaller. Hovering a mark names it.
fn draw_points(
	ui: &egui::Ui,
	p: &Palette,
	rect: egui::Rect,
	camera: &Camera,
	points: &Points,
	overlay: bool,
) {
	let painter = ui.painter_at(rect);
	let shadow = Color32::from_black_alpha(140);
	let axis = |at: &crate::anim::math::CFrame, k: usize, length: f32| {
		let mut dir = [0.0; 3];
		dir[k] = 1.0;
		project_axis(camera, rect, at.pos, at.rotate(dir), length)
	};
	let line = |(a, b): (egui::Pos2, egui::Pos2), colour: Color32, width: f32| {
		painter.line_segment([a, b], Stroke::new(width + 2.0, shadow));
		painter.line_segment([a, b], Stroke::new(width, colour));
	};
	let pointer = ui
		.input(|i| i.pointer.hover_pos())
		.filter(|at| rect.contains(*at));
	let mut nearest: Option<(f32, egui::Pos2, &str)> = None;
	// each mark, with the length of its axes when it is a pivot
	let overlay_marks = overlay.then(|| {
		points
			.attachments
			.iter()
			.map(|a| (a, None))
			.chain(points.pivot.iter().map(|a| (a, Some(28.0))))
	});
	let marks = overlay_marks
		.into_iter()
		.flatten()
		.chain(points.part_pivot.iter().map(|a| (a, Some(22.0))));
	for (point, pivot) in marks {
		let Some(at) = project(camera, rect, point.at.pos) else {
			continue;
		};
		if let Some(length) = pivot {
			for (k, colour) in PIVOT_AXES.into_iter().enumerate() {
				if let Some(segment) = axis(&point.at, k, length) {
					line(segment, colour, 2.0);
				}
			}
			painter.circle(at, 4.0, p.text, Stroke::new(1.5, shadow));
		} else {
			let fade = if point.visible { 1.0 } else { 0.55 };
			for (k, colour) in [(0, AXIS), (1, SECONDARY_AXIS)] {
				if let Some(segment) = axis(&point.at, k, 14.0) {
					line(segment, colour.gamma_multiply(fade), 1.5);
				}
			}
			painter.circle(
				at,
				3.5,
				ATTACHMENT.gamma_multiply(fade),
				Stroke::new(1.5, shadow),
			);
		}
		if let Some(pointer) = pointer {
			let d = at.distance(pointer);
			if d < 9.0 && nearest.is_none_or(|(best, _, _)| d < best) {
				nearest = Some((d, at, &point.label));
			}
		}
	}
	if let Some((_, at, label)) = nearest {
		hover_label(&painter, p, at, label.to_string());
	}
}

fn part_list(ui: &mut egui::Ui, p: &Palette, model: &mut ModelView) {
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
			// the part under the pointer stands out in the view; cleared each
			// frame in the layout
			for part in parts.iter_mut() {
				let rp = &rig.parts[part.index];
				let (tag, colour, hover) = part_tag(part, p);
				let joint = rig
					.joints
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
						ui.label(RichText::new(tag).size(CAPTION).color(colour));
					});
				});
				if ui.rect_contains_pointer(row.response.rect) {
					*highlight = Some(part.index);
				}
			}
		},
	);
}

/// A part's tag in the list, its colour, and the line its hover text gives
/// about how it is drawn.
fn part_tag(part: &PartView, p: &Palette) -> (&'static str, Color32, String) {
	match &part.shape {
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
				kind.label(),
				p.text_weak,
				"No file mesh: a Roblox primitive shape".to_string(),
			),
		},
		PartShape::Missing(why) => ("missing", p.danger, why.clone()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::anim::rig::Binding;

	/// A narrow viewport drops the transport bar's extras so the scrubber
	/// keeps room: loop and speed first, then the time.
	#[test]
	fn the_transport_bar_drops_extras_as_it_narrows() {
		let fit = |time, loop_and_speed| TransportFit {
			time,
			loop_and_speed,
		};
		assert_eq!(TransportFit::for_width(736.0), fit(true, true));
		assert_eq!(TransportFit::for_width(TRANSPORT_FULL), fit(true, true));
		assert_eq!(
			TransportFit::for_width(TRANSPORT_FULL - 1.0),
			fit(true, false)
		);
		assert_eq!(TransportFit::for_width(TRANSPORT_TIMED), fit(true, false));
		assert_eq!(
			TransportFit::for_width(TRANSPORT_TIMED - 1.0),
			fit(false, false)
		);
		assert_eq!(TransportFit::for_width(146.0), fit(false, false));
	}

	#[test]
	fn unmatched_tracks_count_every_slot_track_and_name_each_once() {
		// the slot's own list is what is shown, not the binding's: "Jaw" is a
		// bone inside a skinned mesh, which clip_slots has already left out
		let slot = ClipSlot {
			name: "Talk".into(),
			status: ClipStatus::KeyframeSequence,
			priority: String::new(),
			animation_id: String::new(),
			clip: Err("not needed".into()),
			binding: Binding {
				targets: vec![None; 4],
				unmatched: vec!["Tail".into(), "Jaw".into(), "Hat".into(), "Tail".into()],
			},
			mesh_bone_tracks: 1,
			unmatched: vec!["Tail".into(), "Hat".into(), "Tail".into()],
		};
		// three tracks, two distinct names in order; the names once listed Jaw
		// too (bug B46)
		assert_eq!(unmatched_tracks(&slot), Some((3, "Hat, Tail".into())));
		let bound = ClipSlot {
			unmatched: Vec::new(),
			..slot
		};
		assert_eq!(unmatched_tracks(&bound), None);
	}
}
