//! A small CPU rasteriser on top of egui's painter: orbit camera, flat
//! shading with back-to-front sorting, and an optional wireframe overlay.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

use crate::geometry::Geometry;
use crate::gpu::{GpuRenderer, GpuScene, Uniforms, linear_rgb};
use crate::maps::Slot;
use crate::texture::Material;

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
	[a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V3, b: V3) -> V3 {
	[
		a[1] * b[2] - a[2] * b[1],
		a[2] * b[0] - a[0] * b[2],
		a[0] * b[1] - a[1] * b[0],
	]
}
fn dot(a: V3, b: V3) -> f32 {
	a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalize(a: V3) -> V3 {
	let len = dot(a, a).sqrt();
	if len > 0.0 {
		[a[0] / len, a[1] / len, a[2] / len]
	} else {
		[0.0, 0.0, 0.0]
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shading {
	Solid,
	Wireframe,
	SolidWireframe,
}

pub struct Camera {
	pub yaw: f32,
	pub pitch: f32,
	pub distance: f32,
	pub target: V3,
	/// Radius of the mesh's bounding sphere; scales pan and zoom speed.
	radius: f32,
}

impl Camera {
	pub fn framing(geometry: &Geometry) -> Self {
		let (target, radius) = match geometry.bounds() {
			Some((lo, hi)) => {
				let centre = [
					(lo[0] + hi[0]) * 0.5,
					(lo[1] + hi[1]) * 0.5,
					(lo[2] + hi[2]) * 0.5,
				];
				let r = dot(sub(hi, centre), sub(hi, centre)).sqrt();
				(centre, if r > 1e-6 { r } else { 1.0 })
			}
			None => ([0.0; 3], 1.0),
		};
		Self {
			// Roblox models face -Z, so start from a front three-quarter view
			yaw: std::f32::consts::PI - 0.6,
			pitch: 0.4,
			distance: radius * 1.9,
			target,
			radius,
		}
	}

	/// Orthonormal camera basis: right, up, forward (pointing into the scene).
	fn basis(&self) -> (V3, V3, V3) {
		let (sy, cy) = self.yaw.sin_cos();
		let (sp, cp) = self.pitch.sin_cos();
		let forward = [-sy * cp, -sp, -cy * cp];
		let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
		let up = cross(right, forward);
		(right, up, forward)
	}

	fn eye(&self) -> V3 {
		let (_, _, f) = self.basis();
		[
			self.target[0] - f[0] * self.distance,
			self.target[1] - f[1] * self.distance,
			self.target[2] - f[2] * self.distance,
		]
	}
}

pub struct ViewSettings<'a> {
	pub shading: Shading,
	pub cull_backfaces: bool,
	pub show_axes: bool,
	/// Indices into `geometry.groups` to draw.
	pub groups: &'a [usize],
	/// Colour each group differently (used for convex hulls).
	pub colour_groups: bool,
	/// Maps to apply with the geometry's UVs.
	pub material: &'a Material,
}

/// The GPU renderer and the open document's uploaded scene. Without it the
/// viewer falls back to drawing on the CPU through egui's painter.
pub struct Gpu<'a> {
	pub renderer: &'a GpuRenderer,
	pub scene: &'a mut Option<GpuScene>,
}

const BASE: Color32 = Color32::from_rgb(170, 185, 205);
/// Roblox's default part colour ("Medium stone grey"), shown where a
/// colour map is transparent.
const PART_COLOUR: Color32 = Color32::from_rgb(163, 162, 165);
const WIRE: Color32 = Color32::from_rgba_premultiplied(20, 24, 30, 200);
const WIRE_ONLY: Color32 = Color32::from_rgb(140, 200, 255);

fn group_colour(i: usize) -> Color32 {
	// golden-ratio hue walk gives well separated colours for neighbouring hulls
	let hue = (i as f32 * 0.618_034).fract();
	egui::ecolor::Hsva::new(hue, 0.55, 0.9, 1.0).into()
}

fn shade(c: Color32, light: f32) -> Color32 {
	let f = |v: u8| (v as f32 * light).clamp(0.0, 255.0) as u8;
	Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

pub fn show(
	ui: &mut egui::Ui,
	geometry: &Geometry,
	camera: &mut Camera,
	settings: &ViewSettings,
	gpu: Option<Gpu>,
) {
	let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
	let painter = ui.painter_at(rect);
	painter.rect_filled(rect, 0.0, Color32::from_rgb(28, 30, 36));

	// --- input ---
	if response.dragged_by(egui::PointerButton::Primary) {
		let d = response.drag_delta();
		camera.yaw -= d.x * 0.01;
		camera.pitch = (camera.pitch + d.y * 0.01).clamp(-1.55, 1.55);
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
			camera.distance = (camera.distance * (-scroll * 0.002).exp())
				.clamp(camera.radius * 0.05, camera.radius * 50.0);
		}
	}
	if response.double_clicked() {
		*camera = Camera::framing(geometry);
	}

	let (right, up, _) = camera.basis();
	let nothing_drawn = match gpu {
		Some(gpu) => {
			draw_gpu(&painter, rect, geometry, camera, settings, gpu);
			false
		}
		None => draw_cpu(&painter, rect, geometry, camera, settings),
	};

	if settings.show_axes {
		draw_axes(&painter, rect, right, up);
	}
	if nothing_drawn && !geometry.triangles.is_empty() {
		painter.text(
			rect.center(),
			egui::Align2::CENTER_CENTER,
			"Nothing in view — double-click to reset the camera",
			egui::FontId::proportional(14.0),
			Color32::GRAY,
		);
	}
	painter.text(
		rect.left_bottom() + Vec2::new(8.0, -8.0),
		egui::Align2::LEFT_BOTTOM,
		"Left-drag: orbit   Right/middle-drag: pan   Scroll: zoom   Double-click: reset",
		egui::FontId::proportional(11.0),
		Color32::from_gray(130),
	);
}

/// Row-major 4x4 matrix product.
fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let mut out = [[0.0; 4]; 4];
	for r in 0..4 {
		for c in 0..4 {
			out[r][c] = (0..4).map(|k| a[r][k] * b[k][c]).sum();
		}
	}
	out
}

fn transpose(m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let mut out = [[0.0; 4]; 4];
	for r in 0..4 {
		for c in 0..4 {
			out[c][r] = m[r][c];
		}
	}
	out
}

fn draw_gpu(painter: &egui::Painter, rect: Rect, geometry: &Geometry, camera: &Camera, settings: &ViewSettings, gpu: Gpu) {
	let scene = gpu.scene.get_or_insert_with(|| {
		let colours: Vec<[f32; 3]> = (0..geometry.groups.len())
			.map(|g| linear_rgb(if settings.colour_groups { group_colour(g) } else { PART_COLOUR }))
			.collect();
		gpu.renderer.create_scene(geometry, &colours)
	});
	gpu.renderer.sync_material(scene, settings.material);

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
	let f = 1.0 / 35f32.to_radians().tan();
	let aspect = rect.width() / rect.height().max(1.0);
	let near = camera.radius * 0.01;
	let far = camera.distance + camera.radius * 4.0;
	let proj = [
		[f / aspect, 0.0, 0.0, 0.0],
		[0.0, f, 0.0, 0.0],
		[0.0, 0.0, far / (near - far), near * far / (near - far)],
		[0.0, 0.0, -1.0, 0.0],
	];
	// key light above and to the left of the camera
	let to_light = normalize([
		-forward[0] + 0.6 * up[0] - 0.4 * right[0],
		-forward[1] + 0.6 * up[1] - 0.4 * right[1],
		-forward[2] + 0.6 * up[2] - 0.4 * right[2],
	]);
	let (wire_mode, wire_colour, wire_strength) = match settings.shading {
		Shading::Solid => (0.0, [0.0; 3], 0.0),
		Shading::SolidWireframe => (1.0, [0.01, 0.012, 0.015], 0.7),
		Shading::Wireframe => (2.0, linear_rgb(WIRE_ONLY), 1.0),
	};
	let uniforms = Uniforms {
		// WGSL matrices are column-major
		view_proj: transpose(mul(proj, view)),
		eye: [eye[0], eye[1], eye[2], 1.0],
		light_dir: [to_light[0], to_light[1], to_light[2], 0.0],
		wire_color: [wire_colour[0], wire_colour[1], wire_colour[2], wire_strength],
		params: [wire_mode, 0.0, 0.6, 0.0],
		flags: [0; 4],
	};
	let callback = gpu.renderer.callback(scene, settings.groups, settings.cull_backfaces, uniforms);
	painter.add(eframe::egui_wgpu::Callback::new_paint_callback(rect, callback));
}

/// Software fallback: flat shading, triangles sorted back to front.
/// Returns true when nothing was in view.
fn draw_cpu(painter: &egui::Painter, rect: Rect, geometry: &Geometry, camera: &Camera, settings: &ViewSettings) -> bool {
	// --- projection ---
	let (right, up, forward) = camera.basis();
	let eye = camera.eye();
	let focal = rect.height() * 0.5 / (35f32.to_radians()).tan();
	let near = camera.radius * 1e-3;
	let centre = rect.center();

	let view: Vec<V3> = geometry
		.positions
		.iter()
		.map(|&p| {
			let d = sub(p, eye);
			[dot(d, right), dot(d, up), dot(d, forward)]
		})
		.collect();
	let project = |v: V3| Pos2::new(centre.x + v[0] / v[2] * focal, centre.y - v[1] / v[2] * focal);

	// headlight slightly above and to the left of the camera
	let light = normalize([-0.4, 0.5, -1.0]);

	struct Tri {
		depth: f32,
		pts: [Pos2; 3],
		uvs: [Pos2; 3],
		colour: Color32,
	}
	let texture = match (settings.material.active(Slot::Base), &geometry.uvs) {
		(Some(tex), Some(uvs)) => Some((tex.info, uvs)),
		_ => None,
	};
	let mut tris = Vec::new();
	for &g in settings.groups {
		let Some(range) = geometry.groups.get(g) else {
			continue;
		};
		let base = if texture.is_some() {
			Color32::WHITE
		} else if settings.colour_groups {
			group_colour(g)
		} else {
			BASE
		};
		for t in &geometry.triangles[range.clone()] {
			let v = t.map(|i| view[i as usize]);
			if v.iter().any(|v| v[2] <= near) {
				continue;
			}
			let n = normalize(cross(sub(v[1], v[0]), sub(v[2], v[0])));
			// Roblox winds front faces counter-clockwise; in view space the
			// camera looks down +z, so front faces have a negative normal z.
			let facing = -dot(n, normalize(v[0]));
			if settings.cull_backfaces && facing < 0.0 {
				continue;
			}
			let diffuse = dot(n, light).abs();
			// textures are already lit by their artist, so darken them less
			let ambient = if texture.is_some() { 0.5 } else { 0.25 };
			let colour = shade(base, ambient + (1.0 - ambient) * diffuse);
			let uvs = match texture {
				Some((tex, uvs)) => t.map(|i| tex.map(uvs[i as usize])),
				None => [egui::epaint::WHITE_UV; 3],
			};
			tris.push(Tri {
				depth: v[0][2] + v[1][2] + v[2][2],
				pts: v.map(project),
				uvs,
				colour,
			});
		}
	}
	tris.sort_unstable_by(|a, b| b.depth.total_cmp(&a.depth));

	let clip = rect.expand(rect.width().max(rect.height()));
	let on_screen = |p: &[Pos2; 3]| p.iter().any(|p| rect.contains(*p)) || p.iter().all(|p| clip.contains(*p));

	let solid = settings.shading != Shading::Wireframe;
	let wire = settings.shading != Shading::Solid;
	let (mut mesh, white_uv) = match texture {
		Some((tex, _)) if solid => (egui::Mesh::with_texture(tex.id), tex.white_uv),
		_ => (egui::Mesh::default(), egui::epaint::WHITE_UV),
	};
	let wire_colour = if solid { WIRE } else { WIRE_ONLY };
	for t in tris.iter().filter(|t| on_screen(&t.pts)) {
		if solid {
			let i = mesh.vertices.len() as u32;
			for (pos, uv) in t.pts.into_iter().zip(t.uvs) {
				mesh.vertices.push(egui::epaint::Vertex {
					pos,
					uv,
					color: t.colour,
				});
			}
			mesh.add_triangle(i, i + 1, i + 2);
		}
		if wire {
			// thin quads per edge keep everything in a single draw call
			for (a, b) in [(0, 1), (1, 2), (2, 0)] {
				add_line(&mut mesh, t.pts[a], t.pts[b], 0.5, wire_colour, white_uv);
			}
		}
	}
	painter.add(egui::Shape::mesh(mesh));
	tris.is_empty()
}

fn add_line(mesh: &mut egui::Mesh, a: Pos2, b: Pos2, half_width: f32, colour: Color32, uv: Pos2) {
	let d = b - a;
	let len = d.length();
	if len < 0.01 {
		return;
	}
	let n = Vec2::new(-d.y, d.x) / len * half_width;
	let i = mesh.vertices.len() as u32;
	for pos in [a + n, a - n, b - n, b + n] {
		mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: colour });
	}
	mesh.add_triangle(i, i + 1, i + 2);
	mesh.add_triangle(i, i + 2, i + 3);
}

/// Draws a small XYZ gizmo in the top-right corner.
fn draw_axes(painter: &egui::Painter, rect: Rect, right: V3, up: V3) {
	let origin = rect.right_top() + Vec2::new(-40.0, 40.0);
	let len = 26.0;
	let axes = [
		([1.0, 0.0, 0.0], Color32::from_rgb(230, 80, 80), "X"),
		([0.0, 1.0, 0.0], Color32::from_rgb(110, 210, 90), "Y"),
		([0.0, 0.0, 1.0], Color32::from_rgb(90, 140, 240), "Z"),
	];
	for (axis, colour, label) in axes {
		let tip = origin + Vec2::new(dot(axis, right), -dot(axis, up)) * len;
		painter.line_segment([origin, tip], Stroke::new(2.0, colour));
		painter.text(
			tip + (tip - origin).normalized() * 8.0,
			egui::Align2::CENTER_CENTER,
			label,
			egui::FontId::proportional(11.0),
			colour,
		);
	}
}
