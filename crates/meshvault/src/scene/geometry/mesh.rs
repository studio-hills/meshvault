//! Mesh files, versions 1.00 to 7.00.

use std::ops::Range;

use rbx_mesh::mesh::{Chunk6, Chunks6, CoreMeshChunk, Mesh, Vertices2};

use super::{Geometry, GroupKind, Loaded, detail};

fn faces2(faces: &[rbx_mesh::mesh::Face2]) -> Vec<[u32; 3]> {
	faces
		.iter()
		.map(|f| [f.0[0].0, f.0[1].0, f.0[2].0])
		.collect()
}

type Attributes = (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>);

fn vertices2(vertices: &Vertices2) -> (Attributes, &'static str) {
	match vertices {
		Vertices2::Full(v) => (
			(
				v.iter().map(|v| v.pos).collect(),
				v.iter().map(|v| v.tex).collect(),
				v.iter().map(|v| v.norm).collect(),
			),
			"40 bytes (with colour)",
		),
		Vertices2::Truncated(v) => (
			(
				v.iter().map(|v| v.pos).collect(),
				v.iter().map(|v| v.tex).collect(),
				v.iter().map(|v| v.norm).collect(),
			),
			"36 bytes (no colour)",
		),
	}
}

/// Splits faces into LOD groups. LOD values are face indices marking where
/// each level starts, with the next value marking where it ends.
fn lod_groups(lods: &[rbx_mesh::mesh::Lod3], face_count: usize) -> Vec<Range<usize>> {
	let groups: Vec<_> = lods
		.windows(2)
		.map(|w| (w[0].0 as usize).min(face_count)..(w[1].0 as usize).min(face_count))
		.filter(|r| r.start < r.end)
		.collect();
	if groups.is_empty() {
		vec![0..face_count]
	} else {
		groups
	}
}

fn bone_names(bytes: &[u8]) -> String {
	bytes
		.split(|&b| b == 0)
		.filter(|s| !s.is_empty())
		.map(String::from_utf8_lossy)
		.collect::<Vec<_>>()
		.join(", ")
}

pub(super) fn from_mesh(mesh: Mesh) -> Loaded {
	match mesh {
		Mesh::V1(m) => {
			let revision = match m.revision {
				rbx_mesh::mesh::Revision1::Version100 => "1.00",
				rbx_mesh::mesh::Revision1::Version101 => "1.01",
			};
			let positions: Vec<_> = m.vertices.iter().map(|v| v.pos).collect();
			let uvs = m.vertices.iter().map(|v| [v.tex[0], v.tex[1]]).collect();
			let normals = m.vertices.iter().map(|v| v.norm).collect();
			let triangles = (0..positions.len() as u32 / 3)
				.map(|i| [i * 3, i * 3 + 1, i * 3 + 2])
				.collect();
			Loaded {
				format: format!("Mesh v{revision} (ASCII)"),
				details: vec![detail("Vertices", positions.len())],
				geometry: Geometry::whole(positions, triangles)
					.with_uvs(uvs)
					.with_normals(normals),
				note: None,
			}
		}
		Mesh::V2(m) => {
			let ((positions, uvs, normals), vertex_size) = vertices2(&m.vertices);
			let triangles = faces2(&m.faces);
			Loaded {
				format: "Mesh v2.00".into(),
				details: vec![
					detail("Vertices", positions.len()),
					detail("Faces", triangles.len()),
					detail("Vertex size", vertex_size),
				],
				geometry: Geometry::whole(positions, triangles)
					.with_uvs(uvs)
					.with_normals(normals),
				note: None,
			}
		}
		Mesh::V3(m) => {
			let revision = match m.revision {
				rbx_mesh::mesh::Revision3::Version300 => "3.00",
				rbx_mesh::mesh::Revision3::Version301 => "3.01",
			};
			let ((positions, uvs, normals), vertex_size) = vertices2(&m.vertices);
			let triangles = faces2(&m.faces);
			let groups = lod_groups(&m.lods, triangles.len());
			Loaded {
				format: format!("Mesh v{revision}"),
				details: vec![
					detail("Vertices", positions.len()),
					detail("Faces", triangles.len()),
					detail("Vertex size", vertex_size),
					detail("LODs", groups.len()),
				],
				geometry: Geometry {
					positions,
					uvs: None,
					normals: None,
					triangles,
					groups,
					group_kind: GroupKind::Lod,
				}
				.with_uvs(uvs)
				.with_normals(normals),
				note: None,
			}
		}
		Mesh::V4(m) => {
			let revision = match m.revision {
				rbx_mesh::mesh::Revision4::Version400 => "4.00",
				rbx_mesh::mesh::Revision4::Version401 => "4.01",
			};
			let positions: Vec<_> = m.vertices.iter().map(|v| v.pos).collect();
			let uvs = m.vertices.iter().map(|v| v.tex).collect();
			let normals = m.vertices.iter().map(|v| v.norm).collect();
			let triangles = faces2(&m.faces);
			let groups = lod_groups(&m.lods, triangles.len());
			let mut details = vec![
				detail("Vertices", positions.len()),
				detail("Faces", triangles.len()),
				detail("LODs", groups.len()),
				detail("LOD type", format!("{:?}", m.lod_type)),
				detail("Bones", m.bones.len()),
				detail("Subsets", m.subsets.len()),
			];
			if !m.bones.is_empty() {
				details.push(detail("Bone names", bone_names(&m.bone_names)));
			}
			Loaded {
				format: format!("Mesh v{revision}"),
				details,
				geometry: Geometry {
					positions,
					uvs: None,
					normals: None,
					triangles,
					groups,
					group_kind: GroupKind::Lod,
				}
				.with_uvs(uvs)
				.with_normals(normals),
				note: None,
			}
		}
		Mesh::V5(m) => {
			let positions: Vec<_> = m.vertices.iter().map(|v| v.pos).collect();
			let uvs = m.vertices.iter().map(|v| v.tex).collect();
			let normals = m.vertices.iter().map(|v| v.norm).collect();
			let triangles = faces2(&m.faces);
			let groups = lod_groups(&m.lods, triangles.len());
			let mut details = vec![
				detail("Vertices", positions.len()),
				detail("Faces", triangles.len()),
				detail("LODs", groups.len()),
				detail("LOD type", format!("{:?}", m.lod_type)),
				detail("Bones", m.bones.len()),
				detail("Subsets", m.subsets.len()),
				detail("FACS data", format!("{} bytes", m.sizeof_facs)),
			];
			if !m.bones.is_empty() {
				details.push(detail("Bone names", bone_names(&m.bone_names)));
			}
			let controls = bone_names(&m.facs.face_control_names);
			if !controls.is_empty() {
				details.push(detail("FACS controls", controls));
			}
			Loaded {
				format: "Mesh v5.00".into(),
				details,
				geometry: Geometry {
					positions,
					uvs: None,
					normals: None,
					triangles,
					groups,
					group_kind: GroupKind::Lod,
				}
				.with_uvs(uvs)
				.with_normals(normals),
				note: None,
			}
		}
		Mesh::V6(m) => from_chunks(&m, "6.00"),
		Mesh::V7(m) => from_chunks(&m, "7.00"),
	}
}

fn chunk_name(name: &[u8; 8]) -> String {
	String::from_utf8_lossy(name)
		.trim_end_matches('\0')
		.to_string()
}

/// v6.00 / v7.00 meshes: geometry comes from the COREMESH chunk, which may
/// be Draco compressed, with LODs, bones and FACS in their own chunks.
fn from_chunks(m: &impl Chunks6, revision: &str) -> Loaded {
	let chunk_names: Vec<String> = m
		.chunks()
		.iter()
		.map(|c| match c {
			Chunk6::CoreMesh(CoreMeshChunk::V1(_)) => "COREMESH v1".into(),
			Chunk6::CoreMesh(CoreMeshChunk::V2(_)) => "COREMESH v2 (Draco)".into(),
			Chunk6::CoreMesh(CoreMeshChunk::Unsupported(u)) => format!("COREMESH v{}?", u.version),
			Chunk6::Lods(_) => "LODS".into(),
			Chunk6::Skinning(_) => "SKINNING".into(),
			Chunk6::Facs(_) => "FACS".into(),
			Chunk6::HsrAvis(_) => "HSRAVIS".into(),
			Chunk6::Unknown(u) => format!("{}?", chunk_name(&u.name)),
		})
		.collect();
	let mut details = vec![detail("Chunks", chunk_names.join(", "))];
	let mut note = None;

	let core = match m.core_mesh() {
		Some(CoreMeshChunk::V1(core)) => Some(core.clone()),
		Some(CoreMeshChunk::V2(draco)) => {
			details.push(detail(
				"Draco bitstream",
				format!("{} bytes", draco.bitstream.len()),
			));
			match draco.decode() {
				Ok(core) => Some(core),
				Err(e) => {
					note = Some(format!("Could not decode the Draco compressed mesh: {e}"));
					None
				}
			}
		}
		Some(CoreMeshChunk::Unsupported(u)) => {
			note = Some(format!(
				"COREMESH chunk version {} is not supported.",
				u.version
			));
			None
		}
		None => {
			note = Some("This file has no COREMESH chunk.".into());
			None
		}
	};

	let (positions, uvs, normals, triangles) = match &core {
		Some(core) => (
			core.vertices.iter().map(|v| v.pos).collect(),
			core.vertices.iter().map(|v| v.tex).collect(),
			core.vertices.iter().map(|v| v.norm).collect(),
			faces2(&core.faces),
		),
		None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
	};
	details.push(detail("Vertices", positions.len()));
	details.push(detail("Faces", triangles.len()));

	let groups = match m.lods() {
		Some(lods) => {
			let groups = lod_groups(&lods.lods, triangles.len());
			details.push(detail("LODs", groups.len()));
			details.push(detail("LOD type", format!("{:?}", lods.lod_type)));
			groups
		}
		None => vec![0..triangles.len()],
	};
	if let Some(skinning) = m.skinning() {
		details.push(detail("Bones", skinning.bones.len()));
		details.push(detail("Subsets", skinning.subsets.len()));
		if !skinning.bones.is_empty() {
			details.push(detail("Bone names", bone_names(&skinning.bone_names)));
		}
	}
	if let Some(facs) = m.facs() {
		let controls = bone_names(&facs.face_control_names);
		if !controls.is_empty() {
			details.push(detail("FACS controls", controls));
		}
	}
	if let Some(hsr) = m.hsr_avis() {
		let hidden = (0..hsr.bit_count as usize)
			.filter(|&i| hsr.get(i) == Some(true))
			.count();
		details.push(detail("HSR culled faces", hidden));
	}

	Loaded {
		format: format!("Mesh v{revision}"),
		details,
		geometry: Geometry {
			positions,
			uvs: None,
			normals: None,
			triangles,
			groups,
			group_kind: GroupKind::Lod,
		}
		.with_uvs(uvs)
		.with_normals(normals),
		note,
	}
}

#[cfg(test)]
mod tests {
	use crate::scene::geometry::load;
	use crate::test_support::load_sample;

	/// Every sample file in the repo should either decode with geometry
	/// or be a CSGK placeholder.
	#[test]
	fn loads_all_sample_meshes() {
		let dir = crate::test_support::SAMPLE_MESHES;
		for entry in std::fs::read_dir(dir).unwrap() {
			let path = entry.unwrap().path();
			if !path.is_file() {
				continue;
			}
			let bytes = std::fs::read(&path).unwrap();
			let loaded = load(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
			assert!(
				!loaded.geometry.triangles.is_empty() || loaded.note.is_some(),
				"{} produced no triangles",
				path.display()
			);
			println!(
				"{}: {} ({} tris)",
				path.display(),
				loaded.format,
				loaded.geometry.triangles.len()
			);
		}
	}

	/// Formats that store texture coordinates should expose one per vertex.
	#[test]
	fn textured_formats_have_uvs() {
		for name in [
			"torso.mesh",
			"sphere.mesh",
			"13674780763",
			"mesh7_127279296594138.bin",
			"4500696697_4.meshdata",
		] {
			let geometry = load_sample(name).geometry;
			let uvs = geometry.uvs.unwrap_or_else(|| panic!("{name} has no uvs"));
			assert_eq!(uvs.len(), geometry.positions.len(), "{name}");
		}
	}
}
