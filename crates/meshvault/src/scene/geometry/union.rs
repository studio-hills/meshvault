//! Union graphics and union physics files.

use rbx_mesh::union_graphics::UnionGraphics;
use rbx_mesh::union_physics::UnionPhysics;

use super::{Geometry, GroupKind, Loaded, attributes, detail};
use crate::scene::primitive::unit_cube;

/// What a CSGK file opens as: no geometry, since it only holds the UUID of
/// the real union data.
fn csgk_placeholder(uuid: &[u8; 32], kind: &str) -> Loaded {
	Loaded {
		format: format!("{kind} CSGK"),
		details: vec![detail("UUID", String::from_utf8_lossy(uuid))],
		geometry: Geometry::empty(),
		note: Some("CSGK files only hold a reference UUID and contain no mesh data.".into()),
	}
}

fn union_mesh2(m: &rbx_mesh::union_graphics::Mesh2) -> Geometry {
	let (positions, uvs, normals) = attributes(&m.vertices, |v| v.pos, |v| v.tex, |v| v.norm);
	let triangles = m.faces.iter().map(|f| [f[0].0, f[1].0, f[2].0]).collect();
	Geometry::whole(positions, triangles)
		.with_uvs(uvs)
		.with_normals(normals)
}

/// A CSGMDL v2 or v4 file, which differ only in their header.
fn csgmdl(m: &rbx_mesh::union_graphics::Mesh2, format: &str) -> Loaded {
	let geometry = union_mesh2(m);
	Loaded {
		format: format.into(),
		details: vec![
			detail("Vertices", geometry.positions.len()),
			detail("Faces", geometry.triangles.len()),
		],
		geometry,
		note: None,
	}
}

pub(super) fn from_union_graphics(mesh: UnionGraphics) -> Loaded {
	match mesh {
		UnionGraphics::CSGK(c) => csgk_placeholder(&c.uuid_ascii_hex, "Union graphics"),
		UnionGraphics::V2(m) => csgmdl(&m.mesh, "Union graphics CSGMDL v2"),
		UnionGraphics::V4(m) => csgmdl(&m.mesh, "Union graphics CSGMDL v4"),
		UnionGraphics::V5(m) => {
			let triangles: Vec<_> = m
				.faces
				.indices
				.chunks_exact(3)
				.map(|c| [c[0], c[1], c[2]])
				.collect();
			Loaded {
				format: "Union graphics CSGMDL v5".into(),
				details: vec![
					detail("Positions", m.positions.len()),
					detail("Normals", m.normals.len()),
					detail("Faces", triangles.len()),
					detail("Extra index lists", m.faces._unknown.len()),
				],
				geometry: Geometry::whole(m.positions, triangles)
					.with_uvs(m.tex)
					.with_normals(m.normals.iter().map(|n| n.0).collect()),
				note: None,
			}
		}
	}
}

fn physics_info(details: &mut Vec<(String, String)>, info: &rbx_mesh::union_physics::PhysicsInfo) {
	let [x, y, z] = info.center_of_gravity;
	details.push(detail("Volume", format!("{:.4}", info.volume)));
	details.push(detail(
		"Centre of gravity",
		format!("{x:.3}, {y:.3}, {z:.3}"),
	));
}

/// Convex hulls, each its own group, from their positions and triangles.
fn hulls<'a, T>(pieces: impl IntoIterator<Item = (&'a [[f32; 3]], T)>) -> Geometry
where
	T: IntoIterator<Item = [u32; 3]>,
{
	let mut geometry = Geometry::empty();
	geometry.group_kind = GroupKind::Hull;
	for (positions, triangles) in pieces {
		geometry.push_part(positions, triangles);
	}
	geometry
}

fn hulls_from_meshes(meshes: &[rbx_mesh::union_physics::Mesh]) -> Geometry {
	hulls(meshes.iter().map(|m| {
		(
			m.positions.as_slice(),
			m.faces.iter().map(|f| [f[0].0, f[1].0, f[2].0]),
		)
	}))
}

fn hull_details(geometry: &Geometry) -> Vec<(String, String)> {
	vec![
		detail("Hulls", geometry.groups.len()),
		detail("Vertices", geometry.positions.len()),
		detail("Faces", geometry.triangles.len()),
	]
}

/// A CSGPHS v3 or v5 file, which differ only in their header.
fn csgphs(meshes: &[rbx_mesh::union_physics::Mesh], format: &str) -> Loaded {
	let geometry = hulls_from_meshes(meshes);
	Loaded {
		format: format.into(),
		details: hull_details(&geometry),
		geometry,
		note: None,
	}
}

pub(super) fn from_union_physics(mesh: UnionPhysics) -> Loaded {
	match mesh {
		UnionPhysics::CSGK(c) => csgk_placeholder(&c.uuid_ascii_hex, "Union physics"),
		UnionPhysics::Block(_) => Loaded {
			format: "Union physics BLOCK".into(),
			details: Vec::new(),
			geometry: unit_cube(),
			note: Some("The physics shape is the part's own box. Shown as a unit cube.".into()),
		},
		UnionPhysics::V3(m) => csgphs(&m.meshes, "Union physics CSGPHS v3"),
		UnionPhysics::V5(m) => csgphs(&m.meshes, "Union physics CSGPHS v5"),
		UnionPhysics::V6(m) => {
			let geometry = hulls_from_meshes(std::slice::from_ref(&m.mesh));
			let mut details = hull_details(&geometry);
			physics_info(&mut details, &m.physics_info);
			Loaded {
				format: "Union physics CSGPHS v6".into(),
				details,
				geometry,
				note: None,
			}
		}
		UnionPhysics::V7(m) => {
			let geometry = hulls_from_meshes(&m.meshes);
			let mut details = hull_details(&geometry);
			details.push(detail("Geometry type", format!("{:?}", m.geom_type)));
			physics_info(&mut details, &m.physics_info);
			Loaded {
				format: "Union physics CSGPHS v7".into(),
				details,
				geometry,
				note: None,
			}
		}
		UnionPhysics::V8(m) => {
			let geometry = hulls(
				m.mesh
					.hulls
					.iter_hulls()
					.map(|hull| (hull.positions, hull.faces.iter().copied())),
			);
			let mut details = hull_details(&geometry);
			details.push(detail("Raw hulls", m.mesh.raw_hull_count));
			details.push(detail("Geometry type", format!("{:?}", m.geom_type)));
			Loaded {
				format: "Union physics CSGPHS v8".into(),
				details,
				geometry,
				note: None,
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use crate::test_support::load_sample;

	#[test]
	fn physics_has_no_uvs() {
		assert!(load_sample("CSGPHS_7.data").geometry.uvs.is_none());
	}
}
