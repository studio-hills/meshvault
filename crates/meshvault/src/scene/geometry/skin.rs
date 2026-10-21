//! Bone weights from mesh files.

use std::io::Cursor;

use rbx_mesh::mesh::{Chunks6, CoreMeshChunk, Mesh};

use super::clean;
use super::facs::{Facs, facs_of};

/// Where a Draco bitstream says how it encoded the mesh: after "DRACO",
/// the major and minor version and the encoder type.
const DRACO_METHOD_BYTE: usize = 8;
/// The Draco method that keeps vertices in order (edgebreaker, 1, does not).
const DRACO_SEQUENTIAL: u8 = 0;

/// Per-vertex bone weights from a mesh file (version 4 and later).
#[derive(Clone, Debug, PartialEq)]
pub struct Skin {
	/// Names of the bones the mesh was bound to; matched to the model's Bones.
	pub bones: Vec<String>,
	/// Each bone's parent bone.
	pub parents: Vec<Option<u16>>,
	/// Each bone's bind pose in mesh space, as CFrame components
	/// `[x, y, z, r00 .. r22]`.
	pub binds: Vec<[f32; 12]>,
	/// Up to four (index into `bones`, weight) per vertex, in the same
	/// order as [`Geometry::positions`](super::Geometry::positions). Weights
	/// sum to 1, or are all 0 for a vertex no bone moves.
	pub influences: Vec<[(u16, f32); 4]>,
	/// A FACS head's face poses, which move its face bones.
	pub facs: Option<Facs>,
}

/// Reads a mesh's skinning. `Ok(None)` when it has none; an error when it
/// has some that cannot be lined up with the decoded vertices.
/// [`load_with_skin`](super::load_with_skin) does this without parsing the
/// file a second time.
pub fn skin(bytes: &[u8], vertex_count: usize) -> Result<Option<Skin>, String> {
	if !bytes.starts_with(b"version ") {
		return Ok(None);
	}
	let mesh = rbx_mesh::read_mesh_versioned(Cursor::new(bytes)).map_err(clean)?;
	skin_of(&mesh, vertex_count)
}

/// The skinning of a parsed mesh with `vertex_count` decoded vertices.
pub(super) fn skin_of(mesh: &Mesh, vertex_count: usize) -> Result<Option<Skin>, String> {
	match mesh {
		Mesh::V4(m) => build_skin(
			&m.envelopes,
			&m.bones,
			&m.bone_names,
			&m.subsets,
			vertex_count,
		),
		Mesh::V5(m) => build_skin(
			&m.envelopes,
			&m.bones,
			&m.bone_names,
			&m.subsets,
			vertex_count,
		)
		.map(|s| {
			s.map(|s| Skin {
				facs: facs_of(&m.facs),
				..s
			})
		}),
		Mesh::V6(m) => chunked_skin(m, vertex_count),
		Mesh::V7(m) => chunked_skin(m, vertex_count),
		_ => Ok(None),
	}
}

fn chunked_skin(m: &impl Chunks6, vertex_count: usize) -> Result<Option<Skin>, String> {
	let Some(s) = m.skinning() else {
		return Ok(None);
	};
	if s.bones.is_empty() {
		return Ok(None);
	}
	// edgebreaker renumbers vertices, so the weights no longer line up
	if let Some(CoreMeshChunk::V2(draco)) = m.core_mesh()
		&& draco.bitstream.get(DRACO_METHOD_BYTE) != Some(&DRACO_SEQUENTIAL)
	{
		return Err(
			"its vertices are Draco compressed with edgebreaker, which reorders them away from their bone weights"
				.into(),
		);
	}
	let facs = m.facs().and_then(facs_of);
	build_skin(
		&s.envelopes,
		&s.bones,
		&s.bone_names,
		&s.subsets,
		vertex_count,
	)
	.map(|skin| skin.map(|skin| Skin { facs, ..skin }))
}

fn build_skin(
	envelopes: &[rbx_mesh::mesh::Envelope4],
	bones: &[rbx_mesh::mesh::Bone4],
	names: &[u8],
	subsets: &[rbx_mesh::mesh::Subset4],
	vertex_count: usize,
) -> Result<Option<Skin>, String> {
	if bones.is_empty() {
		return Ok(None);
	}
	if envelopes.len() != vertex_count {
		return Err(format!(
			"it has {} bone weights for {vertex_count} vertices",
			envelopes.len()
		));
	}
	let name_at = |pos: u32| {
		let rest = names.get(pos as usize..).unwrap_or_default();
		let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
		String::from_utf8_lossy(&rest[..end]).into_owned()
	};
	let bone_names = bones.iter().map(|b| name_at(b.bone_name_pos)).collect();
	let parents = bones
		.iter()
		.map(|b| b.parent.get().filter(|&p| (p as usize) < bones.len()))
		.collect();
	let binds = bones
		.iter()
		.map(|b| {
			let c = &b.cframe;
			[
				c.x, c.y, c.z, c.r00, c.r01, c.r02, c.r10, c.r11, c.r12, c.r20, c.r21, c.r22,
			]
		})
		.collect();
	let mut influences = vec![[(0u16, 0.0f32); 4]; vertex_count];
	// envelope bone numbers index the bone table of the subset holding the vertex
	for subset in subsets {
		let start = subset.vertices_offset as usize;
		let end = (start + subset.vertices_len as usize).min(vertex_count);
		for v in start.min(end)..end {
			let e = &envelopes[v];
			let mut out = [(0u16, 0.0f32); 4];
			let mut total = 0.0;
			for ((slot, &local), &weight) in out.iter_mut().zip(&e.bones).zip(&e.weights) {
				let weight = weight as f32;
				let bone = subset
					.bones
					.get(local as usize)
					.and_then(|b| b.get())
					.filter(|&b| (b as usize) < bones.len());
				if let (true, Some(bone)) = (weight > 0.0, bone) {
					*slot = (bone, weight);
					total += weight;
				}
			}
			if total > 0.0 {
				for o in &mut out {
					o.1 /= total;
				}
			}
			influences[v] = out;
		}
	}
	Ok(Some(Skin {
		bones: bone_names,
		parents,
		binds,
		influences,
		facs: None,
	}))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::scene::geometry::load;

	#[test]
	fn sample_skins_line_up_with_vertices() {
		let dir = crate::test_support::SAMPLE_MESHES;
		let mut skinned = Vec::new();
		for entry in std::fs::read_dir(dir).unwrap() {
			let path = entry.unwrap().path();
			if !path.is_file() {
				continue;
			}
			let bytes = std::fs::read(&path).unwrap();
			let Ok(loaded) = load(&bytes) else { continue };
			let n = loaded.geometry.positions.len();
			match skin(&bytes, n) {
				Ok(Some(s)) => {
					skinned.push(path.file_name().unwrap().to_string_lossy().into_owned());
					assert_eq!(s.influences.len(), n);
					for inf in &s.influences {
						let total: f32 = inf.iter().map(|i| i.1).sum();
						assert!(total == 0.0 || (total - 1.0).abs() < 1e-4);
						assert!(inf.iter().all(|i| (i.0 as usize) < s.bones.len()));
					}
					println!(
						"{}: {} bones {:?}",
						path.display(),
						s.bones.len(),
						&s.bones[..s.bones.len().min(4)]
					);
				}
				Ok(None) => {}
				Err(e) => println!("{}: skin unavailable: {e}", path.display()),
			}
		}
		println!("{} skinned samples", skinned.len());
		// skin errors are printed, not counted: a known skinned sample must
		// really skin
		assert!(!skinned.is_empty());
		assert!(
			skinned.iter().any(|n| n == "13674780763"),
			"the skinned head should skin: {skinned:?}"
		);
	}

	/// One parse gives what `load` then `skin` give: the same geometry,
	/// details and notes, the same skin or skin error, and the same error
	/// for a file that does not load.
	#[test]
	fn load_with_skin_matches_load_then_skin() {
		use crate::scene::geometry::load_with_skin;
		let separately = |bytes: &[u8]| {
			load(bytes).map(|l| {
				let n = l.geometry.positions.len();
				(l, skin(bytes, n))
			})
		};
		let same = |name: &str, bytes: &[u8]| {
			let (one, two) = (load_with_skin(bytes), separately(bytes));
			assert_eq!(format!("{one:?}"), format!("{two:?}"), "{name}");
			one
		};
		let mut skinned = 0;
		let mut skin_errors = 0;
		for entry in std::fs::read_dir(crate::test_support::SAMPLE_MESHES).unwrap() {
			let path = entry.unwrap().path();
			if !path.is_file() {
				continue;
			}
			let bytes = std::fs::read(&path).unwrap();
			let name = path.display().to_string();
			match same(&name, &bytes).unwrap().1 {
				Ok(Some(_)) => skinned += 1,
				Ok(None) => {}
				Err(_) => skin_errors += 1,
			}
			// cut short, a file fails to load, or loads with less
			same(&name, &bytes[..bytes.len() / 2]).ok();
		}
		assert!(skinned > 0);
		println!("{skinned} skinned, {skin_errors} skin errors");
		for bytes in [
			&b""[..],
			b"version 9.00\n",
			b"version 4.00\nnot a mesh",
			b"CSGPHS not physics",
			b"something else",
		] {
			assert!(same("bad input", bytes).is_err());
		}
	}

	#[test]
	fn skin_uses_subset_bone_tables() {
		use rbx_mesh::mesh::{Bone4, BoneId4, CFrame4, Envelope4, Subset4};
		let cf = CFrame4 {
			r00: 1.0,
			r01: 0.0,
			r02: 0.0,
			r10: 0.0,
			r11: 1.0,
			r12: 0.0,
			r20: 0.0,
			r21: 0.0,
			r22: 1.0,
			x: 0.0,
			y: 0.0,
			z: 0.0,
		};
		let bone = |pos| Bone4 {
			bone_name_pos: pos,
			parent: BoneId4::new(None),
			lod_parent: BoneId4::new(None),
			cull_distance: 0.0,
			cframe: cf.clone(),
		};
		let mut table = std::array::from_fn(|_| BoneId4::new(None));
		table[0] = BoneId4::new(Some(1));
		table[1] = BoneId4::new(Some(0));
		let subsets = [Subset4 {
			faces_offset: 0,
			faces_len: 0,
			vertices_offset: 0,
			vertices_len: 2,
			bone_count: 2,
			bones: table,
		}];
		let envelopes = [
			Envelope4 {
				bones: [0, 1, 0, 0],
				weights: [51, 204, 0, 0],
			},
			Envelope4 {
				bones: [0; 4],
				weights: [0; 4],
			},
		];
		let skin = build_skin(
			&envelopes,
			&[bone(0), bone(5)],
			b"Root\0Neck\0",
			&subsets,
			2,
		)
		.unwrap()
		.unwrap();
		assert_eq!(skin.bones, ["Root", "Neck"]);
		// subset slot 0 is mesh bone 1 (Neck), slot 1 is bone 0 (Root)
		assert_eq!(skin.influences[0][0].0, 1);
		assert!((skin.influences[0][0].1 - 0.2).abs() < 1e-6);
		assert_eq!(skin.influences[0][1].0, 0);
		assert!((skin.influences[0][1].1 - 0.8).abs() < 1e-6);
		assert_eq!(skin.influences[1], [(0, 0.0); 4]);
		assert!(build_skin(&envelopes, &[bone(0)], b"Root\0", &subsets, 3).is_err());
	}
}
