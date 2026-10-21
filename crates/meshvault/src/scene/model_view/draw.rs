//! What the viewer draws each frame: the parts and the layers over them,
//! the parts' lights, and the skeleton overlay.

use super::decals::NormalId;
use super::export::empty_material;
use super::{ModelView, PartShape, PartView};
use crate::anim::math::{CFrame, V3};
use crate::anim::rig::BoneParent;
use crate::render::lighting::SceneLight;
use crate::render::maths::IDENTITY;
use crate::render::viewport::ScenePart;
use crate::scene::geometry::Geometry;
use crate::scene::surface::DecalDraw;
use crate::scene::texture::Material;

/// The cone angle, in degrees, this viewer gives a SpotLight or SurfaceLight
/// whose export has no Angle.
const DEFAULT_LIGHT_ANGLE: f32 = 90.0;

impl ModelView {
	/// The skeleton where the model is posed now: lines from joint to joint
	/// (and on to the middle of each limb's last part), and each joint.
	pub fn skeleton(&mut self) -> Skeleton {
		self.update_pose();
		let posed = &self.posed;
		let rig = &self.rig;
		let links = rig.part_links();
		let pivot = |m: usize| {
			let m = &rig.motors[m];
			posed.parts[m.part0].mul(&m.c0).pos
		};
		// where a part hangs from its parent, or its middle for a root
		let point = |p: usize| links[p].map_or(posed.parts[p].pos, |(_, m)| pivot(m));
		let mut has_child = vec![false; rig.parts.len()];
		for &(parent, _) in links.iter().flatten() {
			has_child[parent] = true;
		}
		let mut skeleton = Skeleton::default();
		for (p, link) in links.iter().enumerate() {
			let Some((parent, _)) = link else { continue };
			skeleton.bones.push((point(*parent), point(p)));
			if !has_child[p] {
				skeleton.bones.push((point(p), posed.parts[p].pos));
			}
		}
		for (i, m) in rig.motors.iter().enumerate() {
			skeleton.joints.push(Joint {
				at: pivot(i),
				name: m.name.clone(),
				animated: m.animated,
			});
		}
		for (i, b) in rig.bones.iter().enumerate() {
			let at = posed.bones[i].pos;
			let from = match b.parent {
				BoneParent::Part(p) => Some(posed.parts[p].pos),
				BoneParent::Bone(q) => Some(posed.bones[q].pos),
				BoneParent::None => None,
			};
			if let Some(from) = from {
				skeleton.bones.push((from, at));
			}
			skeleton.joints.push(Joint {
				at,
				name: b.name.clone(),
				animated: true,
			});
		}
		skeleton
	}

	/// The parts to draw this frame. `plain` replaces mesh materials when
	/// maps are switched off.
	pub fn scene_parts<'a>(&'a mut self, plain: Option<&'a Material>) -> Vec<ScenePart<'a>> {
		self.update_pose();
		let posed = &self.posed;
		let rig = &self.rig;
		let show_blocks = self.show_blocks;
		let highlight = self.highlight;
		self.parts
			.iter_mut()
			.filter(|p| p.visible)
			.flat_map(|part| {
				let lit = highlight == Some(part.index);
				let part_world = posed.parts[part.index];
				let size = rig.parts[part.index].size;
				let PartView {
					shape,
					skin,
					gpu,
					centre,
					scale,
					offset,
					colour,
					surface,
					decals,
					..
				} = part;
				// SpecialMesh.Offset moves the mesh in the part's own space
				let in_part = CFrame::from_position(*offset);
				let world = part_world.mul(&in_part);
				let drawn = match shape {
					PartShape::Mesh(asset) => {
						let material = plain.unwrap_or(&asset.material);
						Some(match skin {
							// skinned vertices are already in world space
							Some(skin) => Drawn {
								geometry: &skin.geometry,
								groups: asset.groups.as_slice(),
								transform: IDENTITY,
								part_from_mesh: part_world.inverse().to_matrix([1.0; 3]),
								moved: std::mem::take(&mut skin.moved),
								material,
							},
							None => Drawn {
								geometry: &asset.geometry,
								groups: asset.groups.as_slice(),
								transform: placement(&world, *scale, *centre),
								part_from_mesh: placement(&in_part, *scale, *centre),
								moved: false,
								material,
							},
						})
					}
					PartShape::Primitive(kind) if show_blocks => Some(Drawn {
						geometry: kind.geometry(),
						groups: &[0][..],
						transform: placement(&world, *scale, [0.0; 3]),
						part_from_mesh: placement(&in_part, *scale, [0.0; 3]),
						moved: false,
						material: plain.unwrap_or_else(|| empty_material()),
					}),
					_ => None,
				};
				let Some(Drawn {
					geometry,
					groups,
					transform,
					part_from_mesh,
					moved,
					material,
				}) = drawn
				else {
					return Vec::new();
				};
				let mut drawn = vec![ScenePart {
					geometry,
					groups,
					transform,
					material,
					colour_groups: false,
					colour: *colour,
					scene: gpu,
					moved,
					surface: *surface,
					decal: None,
					highlight: lit,
				}];
				for d in decals.iter_mut() {
					drawn.push(ScenePart {
						geometry,
						groups,
						transform,
						material: &d.material,
						colour_groups: false,
						colour: *colour,
						scene: &mut d.gpu,
						moved,
						surface: *surface,
						highlight: lit,
						decal: Some(DecalDraw {
							// clothing places the mesh in its body section's box
							part_from_mesh: match &d.clothing {
								Some((_, in_section)) => {
									mat_mul(&in_section.to_matrix([1.0; 3]), &part_from_mesh)
								}
								None => part_from_mesh,
							},
							size,
							face: d.face,
							tiles: d.tiles,
							colour: d.colour,
							transparency: d.transparency,
							variant: d.variant,
							clothing: d.clothing.map(|(c, _)| c),
						}),
					});
				}
				drawn
			})
			.collect()
	}

	/// The parts' lights where the parts are now. Spot and surface lights
	/// shine out of their face; a surface light from the face itself.
	pub fn scene_lights(&mut self) -> Vec<SceneLight> {
		self.update_pose();
		let mut lights = Vec::new();
		for part in &self.parts {
			if part.lights.is_empty() {
				continue;
			}
			let world = self.posed.parts[part.index];
			let size = self.rig.parts[part.index].size;
			for l in &part.lights {
				let at = l
					.cframe
					.as_ref()
					.map_or(world, |c| world.mul(&CFrame::from_components(c)));
				let normal = NormalId::from_name(l.face.as_deref().unwrap_or("Front")).normal();
				let mut position = at.pos;
				if l.class_name == "SurfaceLight" && l.cframe.is_none() {
					// from the middle of the face
					let half = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
					position = at.point([
						normal[0] * half[0],
						normal[1] * half[1],
						normal[2] * half[2],
					]);
				}
				lights.push(SceneLight {
					position,
					direction: at.rotate(normal),
					colour: l.color,
					brightness: l.brightness,
					range: l.range,
					half_angle: match l.class_name.as_str() {
						"SpotLight" | "SurfaceLight" => {
							Some(l.angle.unwrap_or(DEFAULT_LIGHT_ANGLE) * 0.5)
						}
						_ => None,
					},
				});
			}
		}
		lights
	}
}

/// What one part draws this frame.
struct Drawn<'a> {
	geometry: &'a Geometry,
	groups: &'a [usize],
	/// Mesh to world.
	transform: [[f32; 4]; 4],
	/// Mesh to the part's own space, which decals are laid out in.
	part_from_mesh: [[f32; 4]; 4],
	/// Whether the vertices moved since they were last drawn.
	moved: bool,
	material: &'a Material,
}

/// What the skeleton overlay draws, in world space.
#[derive(Default)]
pub struct Skeleton {
	pub bones: Vec<(V3, V3)>,
	pub joints: Vec<Joint>,
}

/// A joint of the skeleton overlay: a Motor6D, weld or Bone.
pub struct Joint {
	pub at: V3,
	pub name: String,
	/// Clips drive it (a weld only holds its parts together).
	pub animated: bool,
}

/// `a * b` for row-major 4 x 4 matrices.
fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
	let mut out = [[0.0; 4]; 4];
	for (r, row) in out.iter_mut().enumerate() {
		for (c, cell) in row.iter_mut().enumerate() {
			*cell = (0..4).map(|k| a[r][k] * b[k][c]).sum();
		}
	}
	out
}

/// `transform * diag(scale) * translate(-centre)` as a row-major matrix.
fn placement(cf: &CFrame, scale: V3, centre: V3) -> [[f32; 4]; 4] {
	let mut m = cf.to_matrix(scale);
	for row in m.iter_mut().take(3) {
		row[3] -= row[0] * centre[0] + row[1] * centre[1] + row[2] * centre[2];
	}
	m
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;

	use eframe::egui;

	use super::super::fixtures::{part_with, placed_part};
	use super::*;
	use crate::anim::schema::AnimatedModel;

	/// Lights follow their part: a spotlight shines out of its face, a
	/// surface light from the middle of its face, and disabled lights are off.
	#[test]
	fn lights_shine_from_their_part() {
		use crate::anim::schema::LightInfo;
		let light = |class: &str, face: Option<&str>, enabled: bool| LightInfo {
			class_name: class.into(),
			enabled,
			brightness: 1.0,
			color: [1.0; 3],
			range: 8.0,
			angle: face.map(|_| 60.0),
			face: face.map(String::from),
			..Default::default()
		};
		let mut lamp = part_with(Some("Part"), Some("Block"), [2.0, 2.0, 2.0]);
		lamp.path = "Workspace/Lamp/Body".into();
		lamp.name = "Body".into();
		// turned a quarter about Y: its Front (-Z) faces -X in the world
		lamp.cframe = CFrame::axis_rotation(1, std::f32::consts::FRAC_PI_2).components();
		lamp.cframe[1] = 5.0;
		lamp.lights = vec![
			light("SpotLight", Some("Front"), true),
			light("SurfaceLight", Some("Top"), true),
			light("PointLight", None, false),
		];
		let model = AnimatedModel {
			name: "Lamp".into(),
			path: "Workspace/Lamp".into(),
			parts: vec![lamp],
			..Default::default()
		};
		let ctx = egui::Context::default();
		let mut view = ModelView::load(&ctx, 0, &model, &HashMap::new(), &HashMap::new());
		let lights = view.scene_lights();
		assert_eq!(lights.len(), 2);
		let close = |a: V3, b: V3| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5);
		assert!(
			close(lights[0].direction, [-1.0, 0.0, 0.0]),
			"{:?}",
			lights[0].direction
		);
		assert_eq!(lights[0].half_angle, Some(30.0));
		assert!(
			close(lights[1].position, [0.0, 6.0, 0.0]),
			"{:?}",
			lights[1].position
		);
	}

	#[test]
	fn placement_centres_and_scales() {
		let m = placement(
			&CFrame::from_position([10.0, 0.0, 0.0]),
			[2.0, 2.0, 2.0],
			[1.0, 1.0, 1.0],
		);
		// the mesh centre lands on the part position
		let at = |v: V3| {
			[
				m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2] + m[0][3],
				m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2] + m[1][3],
				m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2] + m[2][3],
			]
		};
		assert_eq!(at([1.0, 1.0, 1.0]), [10.0, 0.0, 0.0]);
		assert_eq!(at([2.0, 1.0, 1.0]), [12.0, 0.0, 0.0]);
	}

	/// An unknown or differently cased face name is the Front face, for
	/// decals and lights alike.
	#[test]
	fn unknown_faces_are_the_front() {
		let face_index = |name| NormalId::from_name(name) as u8;
		assert_eq!(face_index("Right"), 0);
		assert_eq!(face_index("Top"), 1);
		assert_eq!(face_index("Back"), 2);
		assert_eq!(face_index("Left"), 3);
		assert_eq!(face_index("Bottom"), 4);
		assert_eq!(face_index("Front"), 5);
		assert_eq!(face_index("front"), 5);
		assert_eq!(face_index("Sideways"), 5);
		assert_eq!(face_index(""), 5);
		// each face's normal points out of it
		assert_eq!(NormalId::Left.normal(), [-1.0, 0.0, 0.0]);
		assert_eq!(NormalId::Front.normal(), [0.0, 0.0, -1.0]);

		use crate::anim::schema::LightInfo;
		let mut lamp = placed_part(Some("Part"), Some("Block"), "Lamp", [0.0; 3]);
		lamp.lights = ["Sideways", "top", "Top"]
			.map(|face| LightInfo {
				class_name: "SpotLight".into(),
				enabled: true,
				face: Some(face.into()),
				..Default::default()
			})
			.into();
		let model = AnimatedModel {
			name: "Dummy".into(),
			path: "Workspace/Dummy".into(),
			parts: vec![lamp],
			..Default::default()
		};
		let ctx = egui::Context::default();
		let mut view = ModelView::load(&ctx, 0, &model, &HashMap::new(), &HashMap::new());
		let directions: Vec<V3> = view.scene_lights().iter().map(|l| l.direction).collect();
		assert_eq!(
			directions,
			[[0.0, 0.0, -1.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]]
		);
	}
}
