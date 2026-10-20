//! The 3D viewport: orbit camera, ground grid and axes, drawing a list of
//! placed meshes on the GPU, or with a small CPU rasteriser on top of
//! egui's painter (flat shading, back-to-front sorting) as a fallback.

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

#[derive(Clone)]
pub struct Camera {
	pub yaw: f32,
	pub pitch: f32,
	pub distance: f32,
	pub target: V3,
	/// Radius of the mesh's bounding sphere; scales pan and zoom speed.
	radius: f32,
	/// Lowest point of the model, where the ground grid sits.
	floor: f32,
}

impl Camera {
	pub fn framing(geometry: &Geometry) -> Self {
		Self::framing_bounds(geometry.bounds())
	}

	/// Frames an axis-aligned box `(min, max)`.
	pub fn framing_bounds(bounds: Option<(V3, V3)>) -> Self {
		let floor = bounds.map_or(0.0, |(lo, _)| lo[1]);
		let (target, radius) = match bounds {
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
			floor,
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

pub struct ViewSettings {
	pub shading: Shading,
	pub cull_backfaces: bool,
	pub show_axes: bool,
	pub show_grid: bool,
	/// Top and bottom of the background gradient.
	pub background: [Color32; 2],
	pub grid_colour: Color32,
}

/// Row-major identity, for a mesh shown on its own.
pub const IDENTITY: [[f32; 4]; 4] = [
	[1.0, 0.0, 0.0, 0.0],
	[0.0, 1.0, 0.0, 0.0],
	[0.0, 0.0, 1.0, 0.0],
	[0.0, 0.0, 0.0, 1.0],
];

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
}

/// A Decal or Texture drawn on one face of its part: the part's geometry
/// again, with the image projected onto the face as Roblox does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalDraw {
	/// Row-major mesh-to-part transform: positions in the part's own studs.
	pub part_from_mesh: [[f32; 4]; 4],
	/// The part's size.
	pub size: V3,
	/// `Enum.NormalId` value: Right, Top, Back, Left, Bottom, Front.
	pub face: u8,
	/// A Texture: studs per tile (U, V) and offset in studs (U, V). A Decal
	/// stretches over the whole face.
	pub tiles: Option<[f32; 4]>,
	/// `Color3` (sRGB 0..1) and `Transparency`.
	pub colour: [f32; 3],
	pub transparency: f32,
}

/// How a Roblox part's surface is drawn, beyond its colour and maps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
	/// 1 − `Transparency`.
	pub opacity: f32,
	/// Neon parts glow in their colour, unlit.
	pub neon: bool,
	/// `Reflectance`: how much of the sky the part mirrors.
	pub reflectance: f32,
	/// A metal material: reflections take the part's colour.
	pub metal: bool,
	/// `SurfaceAppearance.Color`, which tints the colour map (sRGB 0..1).
	pub tint: [f32; 3],
	pub alpha_mode: AlphaMode,
}

impl Surface {
	pub const OPAQUE: Surface = Surface {
		opacity: 1.0,
		neon: false,
		reflectance: 0.0,
		metal: false,
		tint: [1.0; 3],
		alpha_mode: AlphaMode::Overlay,
	};
}

/// What a colour map's alpha means (`SurfaceAppearance.AlphaMode`), as
/// measured in Studio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
	/// The map is laid over the part's colour by its alpha.
	Overlay,
	/// Alpha below one half cuts the surface away; the rest is opaque.
	Transparency,
	/// Alpha says where the tint applies; the part's colour is not used.
	TintMask,
	/// Alpha is ignored.
	Opaque,
}

impl AlphaMode {
	pub fn from_name(name: &str) -> AlphaMode {
		match name {
			"Transparency" => AlphaMode::Transparency,
			"TintMask" => AlphaMode::TintMask,
			"Opaque" => AlphaMode::Opaque,
			_ => AlphaMode::Overlay,
		}
	}
}

const BASE: Color32 = Color32::from_rgb(170, 185, 205);
/// Roblox's default part colour ("Medium stone grey"), shown where a
/// colour map is transparent.
pub const PART_COLOUR: Color32 = Color32::from_rgb(163, 162, 165);
const WIRE: Color32 = Color32::from_rgba_premultiplied(20, 24, 30, 200);
const WIRE_ONLY: Color32 = Color32::from_rgb(140, 200, 255);

fn srgb_channel_to_linear(c: f32) -> f32 {
	if c <= 0.04045 {
		c / 12.92
	} else {
		((c + 0.055) / 1.055).powf(2.4)
	}
}

fn group_colour(i: usize) -> Color32 {
	// golden-ratio hue walk gives well separated colours for neighbouring hulls
	let hue = (i as f32 * 0.618_034).fract();
	egui::ecolor::Hsva::new(hue, 0.55, 0.9, 1.0).into()
}

fn shade(c: Color32, light: f32) -> Color32 {
	let f = |v: u8| (v as f32 * light).clamp(0.0, 255.0) as u8;
	Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
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
	crate::ui::paint_gradient(ui, rect, settings.background[0], settings.background[1]);

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
	let (right, up, forward) = camera.basis();
	let eye = camera.eye();
	let focal = rect.height() * 0.5 / 35f32.to_radians().tan();
	let near = camera.radius * 0.01;
	let centre = rect.center();
	let to_view = |p: V3| {
		let d = sub(p, eye);
		[dot(d, right), dot(d, up), dot(d, forward)]
	};
	let project = |v: V3| {
		Pos2::new(
			centre.x + v[0] / v[2] * focal,
			centre.y - v[1] / v[2] * focal,
		)
	};

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
			let (mut va, mut vb) = (to_view(a), to_view(b));
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
			painter.line_segment([project(va), project(vb)], stroke);
		}
	}
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

/// Inverse transpose of the upper 3x3 of a row-major transform, which
/// keeps normals perpendicular under non-uniform scale.
fn normal_matrix(m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let a = |r: usize, c: usize| m[r][c];
	let cof = |r: usize, c: usize| {
		let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
		let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
		a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)
	};
	let det = a(0, 0) * cof(0, 0) + a(0, 1) * cof(0, 1) + a(0, 2) * cof(0, 2);
	let inv_det = if det.abs() > 1e-12 { 1.0 / det } else { 0.0 };
	// (M^-1)^T is the cofactor matrix over the determinant
	let mut out = IDENTITY;
	for (r, row) in out.iter_mut().enumerate().take(3) {
		for (c, v) in row.iter_mut().enumerate().take(3) {
			*v = cof(r, c) * inv_det;
		}
	}
	out
}

fn apply(m: &[[f32; 4]; 4], p: V3) -> V3 {
	[
		m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
		m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
		m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
	]
}

fn draw_gpu(
	painter: &egui::Painter,
	rect: Rect,
	parts: &mut [ScenePart],
	camera: &Camera,
	settings: &ViewSettings,
	renderer: &GpuRenderer,
) {
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
		params: [wire_mode, 0.0, 0.6, 0.0],
		flags: [0; 4],
		surface: [1.0, 0.0, 0.0, 0.0],
		tint: [1.0, 1.0, 1.0, 0.0],
		part_from_mesh: IDENTITY,
		decal: [0.0; 4],
		decal_size: [1.0, 1.0, 1.0, 0.0],
		decal_tile: [1.0, 1.0, 0.0, 0.0],
		decal_colour: [1.0; 4],
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
			let colour = d.colour.map(|c| srgb_channel_to_linear(c.clamp(0.0, 1.0)));
			(
				transpose(d.part_from_mesh),
				[
					if d.tiles.is_some() { 2.0 } else { 1.0 },
					d.face as f32,
					d.transparency.clamp(0.0, 1.0),
					0.0,
				],
				[d.size[0], d.size[1], d.size[2], 0.0],
				d.tiles.unwrap_or([1.0, 1.0, 0.0, 0.0]),
				[colour[0], colour[1], colour[2], 1.0],
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
				let linear = s.tint.map(|c| srgb_channel_to_linear(c.clamp(0.0, 1.0)));
				[linear[0], linear[1], linear[2], s.alpha_mode as u8 as f32]
			},
			..match decal {
				Some((part_from_mesh, decal, decal_size, decal_tile, decal_colour)) => Uniforms {
					part_from_mesh,
					decal,
					decal_size,
					decal_tile,
					decal_colour,
					..uniforms
				},
				None => uniforms,
			}
		};
		Some((scene, part.groups, uniforms))
	});
	let callback = renderer.callback(draws, settings.cull_backfaces);
	painter.add(eframe::egui_wgpu::Callback::new_paint_callback(
		rect, callback,
	));
}

/// Software fallback: flat shading, triangles sorted back to front.
/// Returns true when nothing was in view.
fn draw_cpu(
	painter: &egui::Painter,
	rect: Rect,
	parts: &[ScenePart],
	camera: &Camera,
	settings: &ViewSettings,
) -> bool {
	// --- projection ---
	let (right, up, forward) = camera.basis();
	let eye = camera.eye();
	let focal = rect.height() * 0.5 / (35f32.to_radians()).tan();
	let near = camera.radius * 1e-3;
	let centre = rect.center();

	let project = |v: V3| {
		Pos2::new(
			centre.x + v[0] / v[2] * focal,
			centre.y - v[1] / v[2] * focal,
		)
	};

	// headlight slightly above and to the left of the camera
	let light = normalize([-0.4, 0.5, -1.0]);

	struct Tri {
		depth: f32,
		pts: [Pos2; 3],
		uvs: [Pos2; 3],
		colour: Color32,
	}
	// egui meshes take one texture, so maps are only drawn for a single mesh
	let texture = match parts {
		[part] => match (part.material.active(Slot::Base), &part.geometry.uvs) {
			(Some(tex), Some(uvs)) => Some((tex.info, uvs)),
			_ => None,
		},
		_ => None,
	};
	let mut tris = Vec::new();
	// the software renderer draws no decals
	for part in parts.iter().filter(|p| p.decal.is_none()) {
		let geometry = part.geometry;
		let view: Vec<V3> = geometry
			.positions
			.iter()
			.map(|&p| {
				let d = sub(apply(&part.transform, p), eye);
				[dot(d, right), dot(d, up), dot(d, forward)]
			})
			.collect();
		for &g in part.groups {
			let Some(range) = geometry.groups.get(g) else {
				continue;
			};
			let base = if texture.is_some() {
				Color32::WHITE
			} else if part.colour_groups {
				group_colour(g)
			} else if parts.len() == 1 {
				BASE
			} else {
				part.colour
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
	}
	tris.sort_unstable_by(|a, b| b.depth.total_cmp(&a.depth));

	let clip = rect.expand(rect.width().max(rect.height()));
	let on_screen =
		|p: &[Pos2; 3]| p.iter().any(|p| rect.contains(*p)) || p.iter().all(|p| clip.contains(*p));

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
		mesh.vertices.push(egui::epaint::Vertex {
			pos,
			uv,
			color: colour,
		});
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
