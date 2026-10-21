//! Mesh files, versions 1.00 to 7.00.

use std::ops::Range;

use rbx_mesh::mesh::{Bone4, Chunk6, Chunks6, CoreMeshChunk, Mesh, Subset4, Vertices2};

use super::{Attributes, Geometry, Loaded, attributes, detail, nul_separated, single_group};

fn faces2(faces: &[rbx_mesh::mesh::Face2]) -> Vec<[u32; 3]> {
	faces
		.iter()
		.map(|f| [f.0[0].0, f.0[1].0, f.0[2].0])
		.collect()
}

fn vertices2(vertices: &Vertices2) -> (Attributes, &'static str) {
	match vertices {
		Vertices2::Full(v) => (
			attributes(v, |v| v.pos, |v| v.tex, |v| v.norm),
			"40 bytes (with colour)",
		),
		Vertices2::Truncated(v) => (
			attributes(v, |v| v.pos, |v| v.tex, |v| v.norm),
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
		single_group(face_count)
	} else {
		groups
	}
}

/// A NUL-separated name list as one line, for the info panel.
fn name_list(bytes: &[u8]) -> String {
	nul_separated(bytes).join(", ")
}

/// The skinning rows of the info panel, before any FACS row.
fn skin_counts(bones: &[Bone4], subsets: &[Subset4]) -> [(String, String); 2] {
	[
		detail("Bones", bones.len()),
		detail("Subsets", subsets.len()),
	]
}

/// The bone names row, for a mesh with bones.
fn bone_names_row(bones: &[Bone4], names: &[u8]) -> Option<(String, String)> {
	(!bones.is_empty()).then(|| detail("Bone names", name_list(names)))
}

/// The FACS controls row, when the mesh names any.
fn facs_controls_row(names: &[u8]) -> Option<(String, String)> {
	let controls = name_list(names);
	(!controls.is_empty()).then(|| detail("FACS controls", controls))
}

pub(super) fn from_mesh(mesh: Mesh) -> Loaded {
	match mesh {
		Mesh::V1(m) => {
			let revision = match m.revision {
				rbx_mesh::mesh::Revision1::Version100 => "1.00",
				rbx_mesh::mesh::Revision1::Version101 => "1.01",
			};
			// v1 stores a third texture coordinate, which is dropped
			let (positions, uvs, normals) =
				attributes(&m.vertices, |v| v.pos, |v| [v.tex[0], v.tex[1]], |v| v.norm);
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
				geometry: Geometry::lods(positions, triangles, groups)
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
			let (positions, uvs, normals) =
				attributes(&m.vertices, |v| v.pos, |v| v.tex, |v| v.norm);
			let triangles = faces2(&m.faces);
			let groups = lod_groups(&m.lods, triangles.len());
			let mut details = vec![
				detail("Vertices", positions.len()),
				detail("Faces", triangles.len()),
				detail("LODs", groups.len()),
				detail("LOD type", format!("{:?}", m.lod_type)),
			];
			details.extend(skin_counts(&m.bones, &m.subsets));
			details.extend(bone_names_row(&m.bones, &m.bone_names));
			Loaded {
				format: format!("Mesh v{revision}"),
				details,
				geometry: Geometry::lods(positions, triangles, groups)
					.with_uvs(uvs)
					.with_normals(normals),
				note: None,
			}
		}
		Mesh::V5(m) => {
			let (positions, uvs, normals) =
				attributes(&m.vertices, |v| v.pos, |v| v.tex, |v| v.norm);
			let triangles = faces2(&m.faces);
			let groups = lod_groups(&m.lods, triangles.len());
			let mut details = vec![
				detail("Vertices", positions.len()),
				detail("Faces", triangles.len()),
				detail("LODs", groups.len()),
				detail("LOD type", format!("{:?}", m.lod_type)),
			];
			details.extend(skin_counts(&m.bones, &m.subsets));
			// v5 shows its FACS size between the counts and the bone names
			details.push(detail("FACS data", format!("{} bytes", m.sizeof_facs)));
			details.extend(bone_names_row(&m.bones, &m.bone_names));
			details.extend(facs_controls_row(&m.facs.face_control_names));
			Loaded {
				format: "Mesh v5.00".into(),
				details,
				geometry: Geometry::lods(positions, triangles, groups)
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

	let ((positions, uvs, normals), triangles) = match &core {
		Some(core) => (
			attributes(&core.vertices, |v| v.pos, |v| v.tex, |v| v.norm),
			faces2(&core.faces),
		),
		None => Default::default(),
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
		None => single_group(triangles.len()),
	};
	if let Some(skinning) = m.skinning() {
		details.extend(skin_counts(&skinning.bones, &skinning.subsets));
		details.extend(bone_names_row(&skinning.bones, &skinning.bone_names));
	}
	if let Some(facs) = m.facs() {
		details.extend(facs_controls_row(&facs.face_control_names));
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
		geometry: Geometry::lods(positions, triangles, groups)
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
