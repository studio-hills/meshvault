//! The model as glTF export sees it: a node per part, Bone and mesh-only
//! bone, where each node is at a time in a clip, and the meshes drawn.

use std::sync::OnceLock;

use eframe::egui::Color32;

use super::look::shown;
use super::skin::{Driver, PartSkin, fit_point, normalize};
use super::{ModelView, PartShape, PartView};
use crate::anim::math::{CFrame, V3};
use crate::anim::rig::BoneParent;
use crate::scene::geometry::{Geometry, Primitive};
use crate::scene::surface::Surface;
use crate::scene::texture::Material;

impl ModelView {
	/// The model as a skeleton for export: every part, then every Bone,
	/// then each skinned mesh's bones that only the mesh has.
	pub fn export_nodes(&self) -> Vec<ExportNode> {
		let parents = self.rig.part_parents();
		let parts = self.rig.parts.len();
		let mut nodes: Vec<ExportNode> = self
			.rig
			.parts
			.iter()
			.zip(parents)
			.map(|(p, parent)| ExportNode {
				name: p.name.clone(),
				parent,
			})
			.collect();
		nodes.extend(self.rig.bones.iter().map(|b| ExportNode {
			name: b.name.clone(),
			parent: match b.parent {
				BoneParent::Part(p) => Some(p),
				BoneParent::Bone(b) => Some(parts + b),
				BoneParent::None => None,
			},
		}));
		for (part, skin) in self.skins() {
			let first = nodes.len();
			let driver_node = self.driver_nodes(skin, first);
			for (k, d) in skin.drivers.iter().enumerate() {
				if let Driver::Mesh { parent, name, .. } = d {
					nodes.push(ExportNode {
						name: name.clone(),
						parent: Some(match parent {
							Some(p) => driver_node[*p],
							None => part.index,
						}),
					});
					debug_assert_eq!(nodes.len() - 1, driver_node[k]);
				}
			}
		}
		nodes
	}

	/// Every part with a skin, visible or not, in node order (hidden parts
	/// still own nodes).
	fn skins(&self) -> impl Iterator<Item = (&PartView, &PartSkin)> {
		self.parts
			.iter()
			.filter_map(|p| Some((p, p.skin.as_ref()?)))
	}

	/// The node of each of a skin's bones, mesh-only ones numbered from `first`.
	fn driver_nodes(&self, skin: &PartSkin, first: usize) -> Vec<usize> {
		let parts = self.rig.parts.len();
		let mut next = first;
		skin.drivers
			.iter()
			.map(|d| match d {
				Driver::Part(p) => *p,
				Driver::Bone(b) => parts + b,
				Driver::Mesh { .. } => {
					next += 1;
					next - 1
				}
			})
			.collect()
	}

	/// Where every node of [`Self::export_nodes`] is, in world space, when
	/// `clip` is at `time` (the rest pose without a clip).
	pub fn export_frame(&self, clip: Option<usize>, time: f32) -> Vec<CFrame> {
		let (posed, by_name, face) = self.solve_at(clip, time);
		let mut worlds = posed.parts.clone();
		worlds.extend(posed.bones.iter().copied());
		for (part, skin) in self.skins() {
			let (now, _) = skin.drivers_at(&self.rest, &posed, part.index, &by_name, &face);
			for (k, d) in skin.drivers.iter().enumerate() {
				if matches!(d, Driver::Mesh { .. }) {
					worlds.push(now[k]);
				}
			}
		}
		worlds
	}

	/// What is drawn, in the rest pose, with the nodes that move it.
	/// Primitive parts are included when blocks are shown.
	pub fn export_meshes(&self) -> Vec<ExportMesh<'_>> {
		let mut first = self.rig.parts.len() + self.rig.bones.len();
		let mut out = Vec::new();
		for part in &self.parts {
			let skinned = part.skin.as_ref().map(|skin| {
				let nodes = self.driver_nodes(skin, first);
				first += skin
					.drivers
					.iter()
					.filter(|d| matches!(d, Driver::Mesh { .. }))
					.count();
				(skin, nodes)
			});
			if !part.visible {
				continue;
			}
			let name = self.rig.parts[part.index].name.clone();
			let base = ExportMesh {
				name,
				// every arm below sets its own
				geometry: Primitive::Block.geometry(),
				groups: Vec::new(),
				positions: Vec::new(),
				normals: None,
				joints: vec![part.index],
				influences: Vec::new(),
				material: empty_material(),
				colour: part.colour,
				surface: shown(part.surface),
			};
			let rest = self.rest.parts[part.index].mul(&CFrame::from_position(part.offset));
			match (&part.shape, skinned) {
				(PartShape::Mesh(asset), Some((skin, nodes))) => {
					// joint k of the skin is its bone k; one more for the part,
					// which carries the vertices no bone moves
					let mut joints = nodes;
					joints.push(part.index);
					let owner = (joints.len() - 1) as u16;
					let influences = skin
						.influences
						.iter()
						.map(|inf| {
							if inf.iter().any(|&(_, w)| w > 0.0) {
								*inf
							} else {
								[(owner, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]
							}
						})
						.collect();
					out.push(ExportMesh {
						geometry: &asset.geometry,
						groups: asset.groups.clone(),
						positions: skin.rest_positions.clone(),
						normals: skin.rest_normals.clone(),
						joints,
						influences,
						material: &asset.material,
						..base
					});
				}
				(PartShape::Mesh(asset), None) => {
					let (positions, normals) =
						fit_vertices(&asset.geometry, &rest, part.scale, part.centre);
					out.push(ExportMesh {
						geometry: &asset.geometry,
						groups: asset.groups.clone(),
						influences: vec![[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]; positions.len()],
						positions,
						normals,
						material: &asset.material,
						..base
					});
				}
				(PartShape::Primitive(kind), _) if self.show_blocks => {
					let geometry = kind.geometry();
					let (positions, normals) = fit_vertices(geometry, &rest, part.scale, [0.0; 3]);
					out.push(ExportMesh {
						geometry,
						groups: vec![0],
						influences: vec![[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]; positions.len()],
						positions,
						normals,
						..base
					});
				}
				_ => {}
			}
		}
		out
	}
}

/// A node of the exported skeleton: a part, a Bone, or a bone only a
/// skinned mesh has.
pub struct ExportNode {
	pub name: String,
	/// Index of the parent node.
	pub parent: Option<usize>,
}

/// One drawn mesh for export, in the rest pose (world space), skinned to
/// nodes of [`ModelView::export_nodes`].
pub struct ExportMesh<'a> {
	/// The part's name.
	pub name: String,
	/// Triangles, UVs and groups; its positions are replaced by `positions`.
	pub geometry: &'a Geometry,
	pub groups: Vec<usize>,
	pub positions: Vec<V3>,
	pub normals: Option<Vec<V3>>,
	/// The nodes that move it; `influences` index into these.
	pub joints: Vec<usize>,
	pub influences: Vec<[(u16, f32); 4]>,
	pub material: &'a Material,
	pub colour: Color32,
	pub surface: Surface,
}

/// A mesh's vertices and normals fitted by `scale` and `centre`, then placed
/// in the world by `cf` (each vertex as [`fit_point`] places it).
fn fit_vertices(
	geometry: &Geometry,
	cf: &CFrame,
	scale: V3,
	centre: V3,
) -> (Vec<V3>, Option<Vec<V3>>) {
	let positions = geometry
		.positions
		.iter()
		.map(|&v| fit_point(cf, scale, centre, v))
		.collect();
	let normals = geometry.normals.as_ref().map(|normals| {
		normals
			.iter()
			.map(|n| normalize(cf.rotate([n[0] / scale[0], n[1] / scale[1], n[2] / scale[2]])))
			.collect()
	});
	(positions, normals)
}

pub(super) fn empty_material() -> &'static Material {
	static EMPTY: OnceLock<Material> = OnceLock::new();
	EMPTY.get_or_init(Material::default)
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;
	use std::sync::Arc;

	use eframe::egui;

	use super::super::fixtures::{asset, cf, placed_part};
	use super::*;
	use crate::anim::schema::AnimatedModel;
	use crate::scene::geometry::Skin;

	/// Skinned parts add their mesh-only bones to the export in part order,
	/// hidden parts included, so a visible part's joints count past them.
	#[test]
	fn hidden_skins_still_number_export_nodes() {
		let mut hidden = placed_part(None, None, "A", [0.0; 3]);
		hidden.transparency = 1.0;
		let shown = placed_part(None, None, "B", [3.0, 0.0, 0.0]);
		let model = AnimatedModel {
			name: "Dummy".into(),
			path: "Workspace/Dummy".into(),
			parts: vec![hidden, shown],
			..Default::default()
		};
		let ctx = egui::Context::default();
		let mut view = ModelView::load(&ctx, 0, &model, &HashMap::new(), &HashMap::new());
		let one = [(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)];
		let skins = [
			Skin {
				bones: vec!["Jaw".into()],
				parents: vec![None],
				binds: vec![cf([0.0; 3])],
				influences: vec![one; 3],
				facs: None,
			},
			Skin {
				bones: vec!["Root".into(), "Tip".into()],
				parents: vec![None, Some(0)],
				binds: vec![cf([0.0; 3]), cf([0.0, 1.0, 0.0])],
				influences: vec![one; 3],
				facs: None,
			},
		];
		for (i, skin) in skins.into_iter().enumerate() {
			let a = asset(vec![[0.0; 3]; 3], skin.clone());
			let ps = PartSkin::build(
				&skin,
				&a,
				&view.rig,
				&view.rest,
				i,
				([0.0; 3], [1.0; 3], [0.0; 3]),
			);
			view.parts[i].shape = PartShape::Mesh(Arc::new(a));
			view.parts[i].skin = Some(ps);
		}
		assert!(!view.parts[0].visible && view.parts[1].visible);

		let nodes: Vec<(String, Option<usize>)> = view
			.export_nodes()
			.into_iter()
			.map(|n| (n.name, n.parent))
			.collect();
		let node = |name: &str, parent| (name.to_string(), parent);
		assert_eq!(
			nodes,
			[
				node("A", None),
				node("B", None),
				// the hidden part's bone is exported too
				node("Jaw", Some(0)),
				node("Root", Some(1)),
				node("Tip", Some(3)),
			]
		);
		assert_eq!(view.export_frame(None, 0.0).len(), nodes.len());
		let meshes = view.export_meshes();
		assert_eq!(meshes.len(), 1);
		assert_eq!(meshes[0].name, "B");
		// B's bones, then B itself for the vertices no bone moves
		assert_eq!(meshes[0].joints, [3, 4, 1]);
	}
}
