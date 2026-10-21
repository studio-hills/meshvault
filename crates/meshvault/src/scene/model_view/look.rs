//! How a part looks: the shape it is drawn as, decided from its class,
//! shape and SpecialMesh, and its colour and surface.

use eframe::egui::Color32;

use crate::anim::math::V3;
use crate::anim::schema::ModelPart;
use crate::scene::geometry::Primitive;
use crate::scene::surface::{AlphaMode, Surface};

/// How a part is drawn, decided from its class, shape and SpecialMesh.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Look {
	/// Its file mesh. A MeshPart fits it to its size; a SpecialMesh
	/// instead scales the mesh's own units and moves it (scale, offset).
	Mesh {
		special: Option<(V3, V3)>,
	},
	Primitive {
		kind: Primitive,
		size: V3,
		offset: V3,
		/// Set when Meshvault has no geometry for the real shape.
		approximate: Option<String>,
	},
	Missing(String),
}

/// A SpecialMesh head's size relative to its part's size times Scale.
const HEAD_SIZE: f32 = 0.94;

/// The most segments one truss is built with, which bounds the size of
/// each truss shape (not of the cache that keeps them; see
/// `Primitive::geometry`).
const MAX_TRUSS_SEGMENTS: f32 = 4096.0;

pub(super) fn look(part: &ModelPart) -> Look {
	let size = part.size;
	let special = part.special_mesh.as_ref();
	if part.mesh_ref != 0 {
		return Look::Mesh {
			special: special.map(|s| (s.scale, s.offset)),
		};
	}
	let primitive = |kind, size, offset, approximate: Option<&str>| Look::Primitive {
		kind,
		size,
		offset,
		approximate: approximate.map(String::from),
	};
	if let Some(s) = special {
		// procedural SpecialMesh types stretch to the part's size times Scale
		let scaled = [
			size[0] * s.scale[0],
			size[1] * s.scale[1],
			size[2] * s.scale[2],
		];
		return match s.mesh_type.as_str() {
			"FileMesh" => Look::Missing("its SpecialMesh's file mesh was not exported".into()),
			// a torso is a plain box too (checked in Studio)
			"Brick" | "Torso" => primitive(Primitive::Block, scaled, s.offset, None),
			"Sphere" => primitive(Primitive::Ball, scaled, s.offset, None),
			// like a cylinder part (checked in Studio): along X, as wide as the
			// smaller of the other two sides
			"Cylinder" => {
				let d = scaled[1].min(scaled[2]);
				primitive(Primitive::CylinderX, [scaled[0], d, d], s.offset, None)
			}
			"Wedge" => primitive(Primitive::Wedge, scaled, s.offset, None),
			"CornerWedge" => primitive(Primitive::CornerWedge, scaled, s.offset, None),
			// measured in Studio: 94% of the scaled size, upright, and as wide
			// as the narrower of the scaled X and Z
			"Head" => {
				let d = scaled[0].min(scaled[2]) * HEAD_SIZE;
				primitive(
					Primitive::Head,
					[d, scaled[1] * HEAD_SIZE, d],
					s.offset,
					None,
				)
			}
			other => primitive(
				Primitive::Block,
				scaled,
				s.offset,
				Some(&format!("a {other} SpecialMesh (drawn as a box)")),
			),
		};
	}
	let shape = part.shape.as_deref().unwrap_or("");
	match (part.class_name.as_deref(), shape) {
		// schema 1 records neither: a box is the best guess
		(None, _) | (_, "Block") => primitive(Primitive::Block, size, [0.0; 3], None),
		(_, "Ball") => {
			let d = size[0].min(size[1]).min(size[2]);
			primitive(Primitive::Ball, [d; 3], [0.0; 3], None)
		}
		(_, "Cylinder") => {
			let d = size[1].min(size[2]);
			primitive(Primitive::CylinderX, [size[0], d, d], [0.0; 3], None)
		}
		(_, "Wedge") | (Some("WedgePart"), _) => primitive(Primitive::Wedge, size, [0.0; 3], None),
		(_, "CornerWedge") | (Some("CornerWedgePart"), _) => {
			primitive(Primitive::CornerWedge, size, [0.0; 3], None)
		}
		(Some("MeshPart"), _) => Look::Missing("its mesh was not exported".into()),
		(_, "Truss") | (Some("TrussPart"), _) => {
			// 2-stud segments along its longest side
			let axis = (0..3)
				.max_by(|&a, &b| size[a].total_cmp(&size[b]))
				.unwrap_or(1);
			let segments = (size[axis] / 2.0).round().clamp(1.0, MAX_TRUSS_SEGMENTS) as u16;
			primitive(
				Primitive::Truss {
					segments,
					axis: axis as u8,
				},
				size,
				[0.0; 3],
				None,
			)
		}
		(Some(class), _) => primitive(
			Primitive::Block,
			size,
			[0.0; 3],
			Some(&format!("a {class} (drawn as a box)")),
		),
	}
}

/// e.g. "MeshPart · Plastic · Bright violet #6B32A8".
pub(super) fn appearance(part: &ModelPart) -> Option<String> {
	let mut bits = Vec::new();
	bits.extend(part.class_name.clone());
	bits.extend(part.material.clone());
	if part.color.is_some() {
		let hex = colour_of(part, Color32::BLACK);
		let name = part.brick_color.clone().unwrap_or_default();
		bits.push(
			format!("{name} #{:02X}{:02X}{:02X}", hex.r(), hex.g(), hex.b())
				.trim()
				.to_string(),
		);
	}
	if let Some(r) = part.reflectance.filter(|r| *r > 0.0) {
		bits.push(format!("reflectance {r:.2}"));
	}
	(!bits.is_empty()).then(|| bits.join(" · "))
}

pub(super) fn surface_of(part: &ModelPart) -> Surface {
	Surface {
		opacity: 1.0 - part.transparency.clamp(0.0, 1.0),
		neon: part.material.as_deref() == Some("Neon"),
		reflectance: part.reflectance.unwrap_or(0.0).clamp(0.0, 1.0),
		metal: matches!(
			part.material.as_deref(),
			Some("Metal" | "DiamondPlate" | "CorrodedMetal" | "Foil")
		),
		tint: part
			.surface_appearance
			.as_ref()
			.map_or([1.0; 3], |s| s.color),
		alpha_mode: part
			.surface_appearance
			.as_ref()
			.map_or(AlphaMode::Overlay, |s| AlphaMode::from_name(&s.alpha_mode)),
	}
}

pub(super) fn colour_of(part: &ModelPart, fallback: Color32) -> Color32 {
	part.color.map_or(fallback, |c| {
		let u = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
		Color32::from_rgb(u(c[0]), u(c[1]), u(c[2]))
	})
}

/// Parts this transparent are invisible in Roblox (HumanoidRootPart).
pub(super) const INVISIBLE: f32 = 0.99;
pub(super) const BLOCK_COLOUR: Color32 = Color32::from_rgb(140, 146, 156);

#[cfg(test)]
mod tests {
	use super::super::fixtures::part_with;
	use super::*;

	#[test]
	fn parts_choose_their_geometry() {
		use crate::anim::schema::SpecialMeshInfo;
		let prim = |l: Look| match l {
			Look::Primitive {
				kind,
				size,
				approximate,
				..
			} => (kind, size, approximate.is_some()),
			other => panic!("{other:?}"),
		};
		// schema 1 has no class: a box
		assert_eq!(
			prim(look(&part_with(None, None, [1.0, 2.0, 3.0]))),
			(Primitive::Block, [1.0, 2.0, 3.0], false)
		);
		// a ball is as wide as the part's smallest side; a cylinder runs along X
		assert_eq!(
			prim(look(&part_with(
				Some("Part"),
				Some("Ball"),
				[3.0, 2.0, 4.0]
			))),
			(Primitive::Ball, [2.0; 3], false)
		);
		assert_eq!(
			prim(look(&part_with(
				Some("Part"),
				Some("Cylinder"),
				[5.0, 2.0, 3.0]
			))),
			(Primitive::CylinderX, [5.0, 2.0, 2.0], false)
		);
		assert_eq!(
			prim(look(&part_with(Some("WedgePart"), Some("Wedge"), [1.0; 3]))).0,
			Primitive::Wedge
		);
		assert!(prim(look(&part_with(Some("UnionOperation"), Some(""), [1.0; 3]))).2);
		assert!(matches!(
			look(&part_with(Some("MeshPart"), Some(""), [1.0; 3])),
			Look::Missing(_)
		));
		// a SpecialMesh sphere stretches to size × scale and moves by its offset
		let mut p = part_with(Some("Part"), Some("Block"), [2.0, 2.0, 2.0]);
		p.special_mesh = Some(SpecialMeshInfo {
			mesh_type: "Sphere".into(),
			scale: [1.0, 0.5, 1.0],
			offset: [0.0, 1.0, 0.0],
		});
		assert_eq!(
			look(&p),
			Look::Primitive {
				kind: Primitive::Ball,
				size: [2.0, 1.0, 2.0],
				offset: [0.0, 1.0, 0.0],
				approximate: None
			}
		);
		// a SpecialMesh cylinder lies along X like a cylinder part
		p.special_mesh.as_mut().unwrap().mesh_type = "Cylinder".into();
		let Look::Primitive { kind, size, .. } = look(&p) else {
			panic!()
		};
		assert_eq!((kind, size), (Primitive::CylinderX, [2.0, 1.0, 1.0]));
		// a SpecialMesh head, measured in Studio: a classic 2×1×1 head part with
		// scale 1.25 is 1.18 studs wide and tall, round however X and Z differ
		let mut head = part_with(Some("Part"), Some("Block"), [2.0, 1.0, 1.0]);
		head.special_mesh = Some(SpecialMeshInfo {
			mesh_type: "Head".into(),
			scale: [1.25; 3],
			offset: [0.0; 3],
		});
		let (kind, size, approximate) = prim(look(&head));
		assert_eq!(kind, Primitive::Head);
		assert!(!approximate);
		assert!(size.iter().all(|s| (s - 1.175).abs() < 1e-3), "{size:?}");
		// a SpecialMesh file mesh keeps its own scale instead of fitting
		p.mesh_ref = 3;
		p.special_mesh.as_mut().unwrap().mesh_type = "FileMesh".into();
		assert_eq!(
			look(&p),
			Look::Mesh {
				special: Some(([1.0, 0.5, 1.0], [0.0, 1.0, 0.0]))
			}
		);
	}
}
