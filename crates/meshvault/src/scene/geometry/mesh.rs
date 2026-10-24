//! Mesh files, versions 1.00 to 7.00.

use std::ops::Range;

use rbx_mesh::mesh::{Bone4, Chunk6, Chunks6, CoreMeshChunk, Mesh, Subset4, Vertex2, Vertices2};

use super::{Attributes, Geometry, Loaded, attributes, detail, nul_separated, single_group};

fn faces2(faces: &[rbx_mesh::mesh::Face2]) -> Vec<[u32; 3]> {
	faces
		.iter()
		.map(|f| [f.0[0].0, f.0[1].0, f.0[2].0])
		.collect()
}

/// The vertices' colours, or `None` when every one is white (most meshes),
/// which draws the same as having none and is not stored.
fn vertex_colors(vertices: &[Vertex2]) -> Option<Vec<[u8; 4]>> {
	vertices
		.iter()
		.any(|v| v.color[..3] != [255; 3])
		.then(|| vertices.iter().map(|v| v.color).collect())
}

fn vertices2(vertices: &Vertices2) -> (Attributes, Option<Vec<[u8; 4]>>, &'static str) {
	match vertices {
		Vertices2::Full(v) => (
			attributes(v, |v| v.pos, |v| v.tex, |v| v.norm),
			vertex_colors(v),
			"40 bytes (with colour)",
		),
		Vertices2::Truncated(v) => (
			attributes(v, |v| v.pos, |v| v.tex, |v| v.norm),
			None,
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

pub(super) fn from_mesh(mesh: &Mesh) -> Loaded {
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
			let ((positions, uvs, normals), colors, vertex_size) = vertices2(&m.vertices);
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
					.with_normals(normals)
					.with_colors(colors),
				note: None,
			}
		}
		Mesh::V3(m) => {
			let revision = match m.revision {
				rbx_mesh::mesh::Revision3::Version300 => "3.00",
				rbx_mesh::mesh::Revision3::Version301 => "3.01",
			};
			let ((positions, uvs, normals), colors, vertex_size) = vertices2(&m.vertices);
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
					.with_normals(normals)
					.with_colors(colors),
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
			let colors = vertex_colors(&m.vertices);
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
					.with_normals(normals)
					.with_colors(colors),
				note: None,
			}
		}
		Mesh::V5(m) => {
			let (positions, uvs, normals) =
				attributes(&m.vertices, |v| v.pos, |v| v.tex, |v| v.norm);
			let colors = vertex_colors(&m.vertices);
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
					.with_normals(normals)
					.with_colors(colors),
				note: None,
			}
		}
		Mesh::V6(m) => from_chunks(m, "6.00"),
		Mesh::V7(m) => from_chunks(m, "7.00"),
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

	let ((positions, uvs, normals), triangles, colors) = match &core {
		Some(core) => (
			attributes(&core.vertices, |v| v.pos, |v| v.tex, |v| v.norm),
			faces2(&core.faces),
			vertex_colors(&core.vertices),
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
			.with_normals(normals)
			.with_colors(colors),
		note,
	}
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeSet;

	use crate::scene::geometry::load;
	use crate::test_support::load_sample;

	/// A v2.00 mesh of one triangle per three `colors`, with 40-byte
	/// vertices (with colour) or 36-byte ones (without).
	fn mesh_v2(colors: &[[u8; 4]], with_colour: bool) -> Vec<u8> {
		let mut b = b"version 2.00\n".to_vec();
		b.extend(12u16.to_le_bytes());
		b.push(if with_colour { 40 } else { 36 });
		b.push(12);
		b.extend((colors.len() as u32).to_le_bytes());
		b.extend((colors.len() as u32 / 3).to_le_bytes());
		for (i, c) in colors.iter().enumerate() {
			let f = i as f32;
			for v in [f, 0.0, 0.0, 0.0, 0.0, 1.0, f / 10.0, 0.5] {
				b.extend(v.to_le_bytes());
			}
			b.extend([0, 0, 127, 127]);
			if with_colour {
				b.extend(c);
			}
		}
		for t in 0..colors.len() as u32 / 3 {
			for i in 0..3 {
				b.extend((t * 3 + i).to_le_bytes());
			}
		}
		b
	}

	/// Each vertex keeps its own colour, beside its position and UV.
	#[test]
	fn vertex_colours_are_kept_with_their_vertex() {
		let colors = [[102, 115, 72, 255], [16, 21, 29, 255], [255, 255, 255, 128]];
		let g = load(&mesh_v2(&colors, true)).unwrap().geometry;
		assert_eq!(g.colors.as_deref(), Some(&colors[..]));
		let uvs = g.uvs.unwrap();
		for (i, (p, uv)) in g.positions.iter().zip(&uvs).enumerate() {
			assert_eq!(p[0], i as f32);
			assert_eq!(*uv, [i as f32 / 10.0, 0.5]);
		}
	}

	/// All-white vertex colours draw as none, so none are kept; 36-byte
	/// vertices have none.
	#[test]
	fn white_or_missing_vertex_colours_are_none() {
		let white = [
			[255, 255, 255, 255],
			[255, 255, 255, 0],
			[255, 255, 255, 255],
		];
		assert!(
			load(&mesh_v2(&white, true))
				.unwrap()
				.geometry
				.colors
				.is_none()
		);
		let coloured = [[1, 2, 3, 255]; 3];
		assert!(
			load(&mesh_v2(&coloured, false))
				.unwrap()
				.geometry
				.colors
				.is_none()
		);
		// the repo's samples have no vertex colours
		for name in ["sphere.mesh", "torso.mesh"] {
			assert!(load_sample(name).geometry.colors.is_none(), "{name}");
		}
	}

	/// The Cobra (a game's tank, so kept out of the repo: copy its two v7
	/// Draco meshes to `rbx_mesh/meshes/personal/` as `cobra_body.mesh` and
	/// `cobra_neon.mesh`). Their colours are the ones Roblox's EditableMesh
	/// reports.
	#[test]
	#[ignore = "needs the Cobra meshes in rbx_mesh/meshes/personal"]
	fn the_cobra_keeps_its_vertex_colours() {
		let colours = |name: &str, lod0: usize, vertices: usize| {
			let g = load_sample(&format!("personal/{name}")).geometry;
			assert_eq!(g.positions.len(), vertices, "{name}");
			assert_eq!(g.groups[0].len(), lod0, "{name}");
			let colors = g.colors.expect("vertex colours");
			assert_eq!(colors.len(), vertices);
			// every LOD-0 corner, as EditableMesh samples them
			g.triangles[g.groups[0].clone()]
				.iter()
				.flatten()
				.map(|&i| {
					let c = colors[i as usize];
					format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
				})
				.collect::<BTreeSet<_>>()
		};
		assert_eq!(
			colours("cobra_body.mesh", 3194, 5832),
			BTreeSet::from(
				["#10151D", "#252B32", "#343E43", "#667348", "#A4A37F"].map(String::from)
			)
		);
		assert_eq!(
			colours("cobra_neon.mesh", 156, 336),
			BTreeSet::from(["#C2CD9C".to_string()])
		);
	}

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

	/// LOD values mark where each level starts; values past the last face
	/// are clamped, empty levels dropped, and a table with no usable level
	/// gives one group of every face.
	#[test]
	fn lod_tables_become_face_ranges() {
		use super::lod_groups;
		use crate::scene::geometry::single_group;
		use rbx_mesh::mesh::Lod3;
		let all = single_group(6);
		assert_eq!((all.len(), all[0].clone()), (1, 0..6));
		let lods = |v: &[u32]| v.iter().map(|&x| Lod3(x)).collect::<Vec<_>>();
		assert_eq!(lod_groups(&lods(&[0, 4, 6]), 6), [0..4, 4..6]);
		assert_eq!(lod_groups(&lods(&[0, 4, 4, 9]), 6), [0..4, 4..6]);
		assert_eq!(lod_groups(&[], 6), single_group(6));
		assert_eq!(lod_groups(&lods(&[0]), 6), single_group(6));
		assert_eq!(lod_groups(&lods(&[7, 9]), 6), single_group(6));
		assert_eq!(lod_groups(&[], 0), single_group(0));
	}

	/// The info panel lists names on one line: split at NULs, in order,
	/// without empty ones; bytes that are not UTF-8 are replaced, not dropped.
	#[test]
	fn name_lists_are_comma_separated() {
		assert_eq!(super::name_list(b"Root\0Neck\0\0Head"), "Root, Neck, Head");
		assert_eq!(
			super::name_list(b"\0Root\0\0Neck\xff\0"),
			"Root, Neck\u{fffd}"
		);
		assert_eq!(super::name_list(b""), "");
		assert_eq!(super::name_list(b"\0\0"), "");
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
