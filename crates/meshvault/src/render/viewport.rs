//! The 3D viewport: camera input, ground grid and axes, and drawing a
//! list of placed meshes on the GPU, or with the CPU rasteriser in
//! [`crate::render::cpu`] as a fallback.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, Vec2};

use crate::render::camera::{HALF_FOV_DEGREES, View};
use crate::render::cpu::draw_cpu;
use crate::render::gpu::{GpuRenderer, GpuScene, MeshCallback, Uniforms};
use crate::render::lighting::{Lighting, SceneLight, light_uniforms};
use crate::render::maths::{V3, apply, dot, mul, normal_matrix, sub, transpose};
use crate::scene::colour::{linear_rgb, srgb_to_linear};
use crate::scene::effects::EffectFrame;
use crate::scene::geometry::Geometry;
use crate::scene::surface::{DEFAULT_ROUGHNESS, DecalDraw};
use crate::scene::texture::Material;

use crate::render::camera::Camera;
use crate::render::maths::IDENTITY;
use crate::scene::surface::Surface;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shading {
	Solid,
	Wireframe,
	SolidWireframe,
}

pub struct ViewSettings {
	pub shading: Shading,
	pub cull_backfaces: bool,
	pub show_axes: bool,
	pub show_grid: bool,
	/// Top and bottom of the background gradient.
	pub background: [Color32; 2],
	pub grid_colour: Color32,
	/// The model's own lights, in world space.
	pub lights: Vec<SceneLight>,
	pub lighting: Lighting,
	/// The colour a highlighted part is tinted.
	pub highlight: Color32,
	/// The model's particles, beams and trails this frame, in world space.
	/// Drawn by the GPU renderer only.
	pub effects: EffectFrame,
}

/// One mesh placed in the scene.
pub struct ScenePart<'a> {
	pub geometry: &'a Geometry,
	/// Indices into `geometry.groups` to draw.
	pub groups: &'a [usize],
	/// Row-major mesh-to-world transform.
	pub transform: [[f32; 4]; 4],
	/// Maps to apply with the geometry's UVs.
	pub material: &'a Material,
	/// Colour each group differently (used for convex hulls).
	pub colour_groups: bool,
	/// Shown where there is no colour map, or it is transparent.
	pub colour: Color32,
	/// The part's mesh on the GPU, uploaded on first draw. Unused without
	/// a GPU renderer.
	pub scene: &'a mut Option<GpuScene>,
	/// The vertices moved since the last draw (skinning), so they are uploaded again.
	pub moved: bool,
	pub surface: Surface,
	/// Set when this draws a decal over its part's geometry; `material`
	/// then holds the decal's image.
	pub decal: Option<DecalDraw>,
	/// Tinted to stand out (hovered in the parts list).
	pub highlight: bool,
}

/// Radians the camera turns per point dragged.
const ORBIT_SPEED: f32 = 0.01;
/// How far the camera may look up or down, in radians: just short of
/// straight (π/2), where the camera basis has no right vector.
const MAX_PITCH: f32 = 1.55;
/// How fast scrolling zooms: the distance scales by `exp(-scroll * ZOOM_SPEED)`.
const ZOOM_SPEED: f32 = 0.002;
/// The closest and farthest the camera may be, in bounding radii.
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 50.0;

/// The wireframe modes, as `params.x` in mesh.wgsl (which compares it with
/// 0.5 and 1.5): none, over the surface, or the wireframe alone.
const WIRE_NONE: f32 = 0.0;
const WIRE_OVERLAY: f32 = 1.0;
const WIRE_ONLY_MODE: f32 = 2.0;

pub(super) const WIRE_ONLY: Color32 = Color32::from_rgb(140, 200, 255);

pub(super) fn group_colour(i: usize) -> Color32 {
	// golden-ratio hue walk gives well separated colours for neighbouring hulls
	let hue = (i as f32 * 0.618_034).fract();
	egui::ecolor::Hsva::new(hue, 0.55, 0.9, 1.0).into()
}

/// Draws `parts` and handles camera input. Without a GPU renderer the
/// parts are drawn on the CPU through egui's painter. `home` is the camera
/// a double-click returns to.
pub fn show(
	ui: &mut egui::Ui,
	parts: &mut [ScenePart],
	camera: &mut Camera,
	settings: &ViewSettings,
	renderer: Option<&GpuRenderer>,
	home: &dyn Fn() -> Camera,
) -> Rect {
	let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
	let painter = ui.painter_at(rect);
	crate::ui::vertical_gradient(
		ui.painter(),
		rect,
		settings.background[0],
		settings.background[1],
	);

	// --- input ---
	if response.dragged_by(egui::PointerButton::Primary) {
		let d = response.drag_delta();
		camera.yaw -= d.x * ORBIT_SPEED;
		camera.pitch = (camera.pitch + d.y * ORBIT_SPEED).clamp(-MAX_PITCH, MAX_PITCH);
	}
	if response.dragged_by(egui::PointerButton::Secondary)
		|| response.dragged_by(egui::PointerButton::Middle)
	{
		let d = response.drag_delta();
		let (r, u, _) = camera.basis();
		let s = camera.distance / rect.height().max(1.0);
		for k in 0..3 {
			camera.target[k] += (-r[k] * d.x + u[k] * d.y) * s;
		}
	}
	if response.hovered() {
		let scroll = ui.input(|i| i.smooth_scroll_delta.y);
		if scroll != 0.0 {
			camera.distance = (camera.distance * (-scroll * ZOOM_SPEED).exp())
				.clamp(camera.radius * MIN_ZOOM, camera.radius * MAX_ZOOM);
		}
	}
	if response.double_clicked() {
		*camera = home();
	}
	// an open hand over the model, a closed one while orbiting, a move cursor while panning
	if response.dragged_by(egui::PointerButton::Primary) {
		ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
	} else if response.dragged() {
		ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
	} else if response.hovered() {
		ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
	}

	let (right, up, _) = camera.basis();
	if settings.show_grid {
		draw_grid(&painter, rect, camera, settings.grid_colour);
	}
	let nothing_drawn = match renderer {
		Some(renderer) => {
			draw_gpu(&painter, rect, parts, camera, settings, renderer);
			false
		}
		None => draw_cpu(&painter, rect, parts, camera, settings),
	};

	if settings.show_axes {
		draw_axes(&painter, rect, right, up);
	}
	if nothing_drawn && parts.iter().any(|p| !p.geometry.triangles.is_empty()) {
		painter.text(
			rect.center(),
			egui::Align2::CENTER_CENTER,
			"Nothing in view — double-click to reset the camera",
			egui::FontId::proportional(14.0),
			Color32::GRAY,
		);
	}
	rect
}

/// A ground grid under the model, drawn before the mesh so the mesh covers it.
/// Spacing adapts to the model's size (1, 2 or 5 times a power of ten).
fn draw_grid(painter: &egui::Painter, rect: Rect, camera: &Camera, colour: Color32) {
	let view = View::new(camera, rect);
	let near = camera.near();

	let raw = camera.radius / 4.0;
	let magnitude = 10f32.powf(raw.log10().floor());
	let step = [1.0, 2.0, 5.0, 10.0]
		.into_iter()
		.map(|m| m * magnitude)
		.find(|s| *s >= raw)
		.unwrap_or(raw);
	const LINES: i32 = 12;
	let half = step * LINES as f32;
	let cx = (camera.target[0] / step).round() * step;
	let cz = (camera.target[2] / step).round() * step;
	let y = camera.floor;
	for i in -LINES..=LINES {
		let o = i as f32 * step;
		// fade out towards the edge of the grid; every fifth line is stronger
		let fade = 1.0 - (i.abs() as f32 / (LINES as f32 + 1.0));
		let strength = if i.rem_euclid(5) == 0 { 1.0 } else { 0.55 };
		let stroke = Stroke::new(1.0, colour.gamma_multiply(fade * strength));
		for (a, b) in [
			([cx + o, y, cz - half], [cx + o, y, cz + half]),
			([cx - half, y, cz + o], [cx + half, y, cz + o]),
		] {
			let (mut va, mut vb) = (view.to_view(a), view.to_view(b));
			// clip the segment to in front of the camera
			if va[2] < near && vb[2] < near {
				continue;
			}
			if va[2] < near || vb[2] < near {
				let t = (near - va[2]) / (vb[2] - va[2]);
				let hit = [
					va[0] + (vb[0] - va[0]) * t,
					va[1] + (vb[1] - va[1]) * t,
					near,
				];
				if va[2] < near {
					va = hit;
				} else {
					vb = hit;
				}
			}
			painter.line_segment([view.project(va), view.project(vb)], stroke);
		}
	}
}

fn draw_gpu(
	painter: &egui::Painter,
	rect: Rect,
	parts: &mut [ScenePart],
	camera: &Camera,
	settings: &ViewSettings,
	renderer: &GpuRenderer,
) {
	let aspect = rect.width() / rect.height().max(1.0);
	let callback = mesh_callback(parts, camera, settings, renderer, aspect);
	painter.add(eframe::egui_wgpu::Callback::new_paint_callback(
		rect, callback,
	));
}

/// Draws `parts` off screen into a `size` image with a transparent
/// background (no grid or axes). Fails when the GPU cannot hand it back.
pub fn render_image(
	parts: &mut [ScenePart],
	camera: &Camera,
	settings: &ViewSettings,
	renderer: &GpuRenderer,
	size: [u32; 2],
) -> Result<image::RgbaImage, String> {
	let aspect = size[0] as f32 / size[1].max(1) as f32;
	let callback = mesh_callback(parts, camera, settings, renderer, aspect);
	renderer.render_image(&callback, size)
}

/// Uploads what `parts` need to the GPU and builds the draw for a view
/// `aspect` times wider than it is tall.
fn mesh_callback(
	parts: &mut [ScenePart],
	camera: &Camera,
	settings: &ViewSettings,
	renderer: &GpuRenderer,
	aspect: f32,
) -> MeshCallback {
	for part in parts.iter_mut() {
		let geometry = part.geometry;
		let colours = || -> Vec<[f32; 3]> {
			(0..geometry.groups.len())
				.map(|g| {
					linear_rgb(if part.colour_groups {
						group_colour(g)
					} else {
						part.colour
					})
				})
				.collect()
		};
		match part.scene {
			Some(scene) if part.moved => renderer.update_vertices(scene, geometry, &colours()),
			Some(_) => {}
			None => *part.scene = Some(renderer.create_scene(geometry, &colours())),
		}
		if let Some(scene) = part.scene {
			renderer.sync_material(scene, part.material);
		}
	}

	let (right, up, forward) = camera.basis();
	let eye = camera.eye();
	// view: camera looks down -z
	let back = [-forward[0], -forward[1], -forward[2]];
	let view = [
		[right[0], right[1], right[2], -dot(right, eye)],
		[up[0], up[1], up[2], -dot(up, eye)],
		[back[0], back[1], back[2], -dot(back, eye)],
		[0.0, 0.0, 0.0, 1.0],
	];
	// perspective with a 70° vertical field of view, depth mapped to 0..1
	let f = 1.0 / HALF_FOV_DEGREES.to_radians().tan();
	let near = camera.near();
	let far = camera.distance + camera.radius * 4.0;
	let proj = [
		[f / aspect, 0.0, 0.0, 0.0],
		[0.0, f, 0.0, 0.0],
		[0.0, 0.0, far / (near - far), near * far / (near - far)],
		[0.0, 0.0, -1.0, 0.0],
	];
	let (to_light, mut env) = settings.lighting.rig(right, up, forward);
	let h = linear_rgb(settings.highlight);
	env[3] = [h[0], h[1], h[2], 0.0];
	let (wire_mode, wire_colour, wire_strength) = match settings.shading {
		Shading::Solid => (WIRE_NONE, [0.0; 3], 0.0),
		Shading::SolidWireframe => (WIRE_OVERLAY, [0.01, 0.012, 0.015], 0.7),
		Shading::Wireframe => (WIRE_ONLY_MODE, linear_rgb(WIRE_ONLY), 1.0),
	};
	let uniforms = Uniforms {
		// WGSL matrices are column-major
		view_proj: transpose(mul(proj, view)),
		model: IDENTITY,
		normal_model: IDENTITY,
		eye: [eye[0], eye[1], eye[2], 1.0],
		light_dir: [to_light[0], to_light[1], to_light[2], 0.0],
		wire_color: [
			wire_colour[0],
			wire_colour[1],
			wire_colour[2],
			wire_strength,
		],
		params: [wire_mode, 0.0, DEFAULT_ROUGHNESS as f32, 0.0],
		flags: [0; 4],
		surface: [1.0, 0.0, 0.0, 0.0],
		tint: [1.0, 1.0, 1.0, 0.0],
		part_from_mesh: IDENTITY,
		// mode 0: no decal
		decal: [0.0; 4],
		decal_size: [1.0, 1.0, 1.0, 0.0],
		decal_tile: [1.0, 1.0, 0.0, 0.0],
		decal_colour: [1.0; 4],
		env,
		..light_uniforms(&settings.lights, camera.target)
	};
	// see-through parts are drawn after the rest, the farthest first
	let distance = |part: &ScenePart| {
		let at = apply(&part.transform, [0.0; 3]);
		dot(sub(at, eye), sub(at, eye))
	};
	let mut order: Vec<&ScenePart> = parts.iter().collect();
	order.sort_by(|a, b| distance(b).total_cmp(&distance(a)));
	// decals are part of the surface; the wireframe alone shows none
	let wire_only = settings.shading == Shading::Wireframe;
	let draws = order.into_iter().filter_map(|part| {
		if wire_only && part.decal.is_some() {
			return None;
		}
		let scene = part.scene.as_ref()?;
		let s = part.surface;
		let decal = part.decal.map(|d| {
			let colour = d.colour.map(|c| srgb_to_linear(c.clamp(0.0, 1.0)));
			(
				transpose(d.part_from_mesh),
				[
					d.shader_mode(),
					d.face as f32,
					d.transparency.clamp(0.0, 1.0),
					0.0,
				],
				[d.size[0], d.size[1], d.size[2], 0.0],
				d.tiles.unwrap_or([1.0, 1.0, 0.0, 0.0]),
				[colour[0], colour[1], colour[2], 1.0],
				d.clothing.map_or([[0.0; 4]; 8], |c| {
					let mut packed = [[0.0; 4]; 8];
					packed[..6].copy_from_slice(&c.regions);
					packed[6] = [c.centre[0], c.centre[1], c.centre[2], 0.0];
					packed[7] = [c.half[0], c.half[1], c.half[2], 0.0];
					packed
				}),
			)
		});
		let uniforms = Uniforms {
			model: transpose(part.transform),
			normal_model: transpose(normal_matrix(part.transform)),
			surface: [
				s.opacity.clamp(0.0, 1.0),
				s.neon as u8 as f32,
				s.reflectance,
				s.metal as u8 as f32,
			],
			tint: {
				let linear = s.tint.map(|c| srgb_to_linear(c.clamp(0.0, 1.0)));
				[linear[0], linear[1], linear[2], s.alpha_mode.shader_value()]
			},
			env: {
				let mut env = uniforms.env;
				env[2][3] = if part.highlight { 0.75 } else { 0.0 };
				env
			},
			..match decal {
				Some((part_from_mesh, decal, decal_size, decal_tile, decal_colour, clothing)) => {
					Uniforms {
						part_from_mesh,
						decal,
						decal_size,
						decal_tile,
						decal_colour,
						clothing,
						..uniforms
					}
				}
				None => uniforms,
			}
		};
		Some((scene, part.groups, uniforms))
	});
	let effects = renderer.effect_draw(
		&settings.effects,
		super::effects::View { eye, right, up },
		uniforms.view_proj,
	);
	renderer.callback(draws, settings.cull_backfaces, effects)
}

/// Where the axes gizmo's centre sits in from the viewport's top-right
/// corner, and the length of each axis.
const AXES_INSET: f32 = 40.0;
const AXES_LENGTH: f32 = 26.0;
/// How far the gizmo's letters sit beyond the axes' tips.
const AXES_LABEL_GAP: f32 = 8.0;
/// How far in from the viewport's right edge the gizmo can reach, letters
/// included, so controls can keep clear of it.
pub const AXES_REACH: f32 = AXES_INSET + AXES_LENGTH + AXES_LABEL_GAP + 8.0;

/// Draws a small XYZ gizmo in the top-right corner.
fn draw_axes(painter: &egui::Painter, rect: Rect, right: V3, up: V3) {
	let origin = rect.right_top() + Vec2::new(-AXES_INSET, AXES_INSET);
	let len = AXES_LENGTH;
	let axes = [
		([1.0, 0.0, 0.0], Color32::from_rgb(230, 80, 80), "X"),
		([0.0, 1.0, 0.0], Color32::from_rgb(110, 210, 90), "Y"),
		([0.0, 0.0, 1.0], Color32::from_rgb(90, 140, 240), "Z"),
	];
	for (axis, colour, label) in axes {
		let tip = origin + Vec2::new(dot(axis, right), -dot(axis, up)) * len;
		painter.line_segment([origin, tip], Stroke::new(2.0, colour));
		painter.text(
			tip + (tip - origin).normalized() * AXES_LABEL_GAP,
			egui::Align2::CENTER_CENTER,
			label,
			egui::FontId::proportional(crate::ui::theme::CAPTION),
			colour,
		);
	}
}
