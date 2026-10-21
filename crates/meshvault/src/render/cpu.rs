//! The software fallback renderer: flat shading on top of egui's painter,
//! triangles sorted back to front.

use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

use crate::render::camera::Camera;
use crate::render::maths::{V3, apply, cross, dot, normalize, sub};
use crate::render::viewport::{ScenePart, Shading, ViewSettings, WIRE_ONLY, group_colour};
use crate::scene::maps::Slot;

const BASE: Color32 = Color32::from_rgb(170, 185, 205);
const WIRE: Color32 = Color32::from_rgba_premultiplied(20, 24, 30, 200);

fn shade(c: Color32, light: f32) -> Color32 {
	let f = |v: u8| (v as f32 * light).clamp(0.0, 255.0) as u8;
	Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// Software fallback: flat shading, triangles sorted back to front.
/// Returns true when nothing was in view.
pub(super) fn draw_cpu(
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
