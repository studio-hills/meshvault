//! Bone weights from mesh files.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use rbx_mesh::mesh::{Chunks6, CoreMeshChunk, Mesh};

use super::facs::{Facs, facs_of};

/// Where a Draco bitstream says how it encoded the mesh: after "DRACO",
/// the major and minor version and the encoder type.
const DRACO_METHOD_BYTE: usize = 8;
/// The Draco method that keeps vertices in order (edgebreaker, 1, does not).
const DRACO_SEQUENTIAL: u8 = 0;

/// The longest bone name read, in bytes. Roblox keeps names to 100
/// characters, which UTF-8 holds in at most 400 bytes.
const MAX_BONE_NAME: usize = 400;
/// The most bytes of bone names one skin decodes, counting each bone's
/// copy of a name it shares.
const MAX_BONE_NAME_BYTES: usize = 4 * 1024 * 1024;

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
				// a head whose FACS cannot be used still skins, without face poses
				facs: facs_of(&m.facs).ok(),
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
	// a head whose FACS cannot be used still skins, without face poses
	let facs = m.facs().and_then(|f| facs_of(f).ok());
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
	let bone_names = bone_names(bones, names, MAX_BONE_NAME, MAX_BONE_NAME_BYTES)?;
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

/// Each bone's name, read from the NUL-ended `names` at its offset. A name
/// is read once per offset, however many bones share it, and only up to
/// `longest` bytes: a longer name, or names adding up to more than `total`
/// bytes over every bone, refuse the skin before they are copied.
fn bone_names(
	bones: &[rbx_mesh::mesh::Bone4],
	names: &[u8],
	longest: usize,
	total: usize,
) -> Result<Vec<String>, String> {
	let mut read: HashMap<u32, String> = HashMap::new();
	let mut used = 0usize;
	let mut out = Vec::with_capacity(bones.len());
	for bone in bones {
		let pos = bone.bone_name_pos;
		let name = match read.entry(pos) {
			Entry::Occupied(e) => e.into_mut(),
			Entry::Vacant(e) => {
				let rest = names.get(pos as usize..).unwrap_or_default();
				// one byte past the longest name is enough to see its end
				let rest = &rest[..rest.len().min(longest + 1)];
				let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
				if end > longest {
					return Err(format!("a bone name is longer than {longest} bytes"));
				}
				e.insert(String::from_utf8_lossy(&rest[..end]).into_owned())
			}
		};
		used += name.len();
		if used > total {
			return Err(format!("its bone names add up to more than {total} bytes"));
		}
		out.push(name.clone());
	}
	Ok(out)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::scene::geometry::load_with_skin;

	/// Every sample loads with its skin in one parse; a skin lines up with
	/// the loaded vertices; cut-short files do not panic; and files that are
	/// not meshes fail to load.
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
			let (loaded, skin) =
				load_with_skin(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
			let n = loaded.geometry.positions.len();
			match skin {
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
			// cut short, a file fails to load, or loads with less
			let _ = load_with_skin(&bytes[..bytes.len() / 2]);
		}
		println!("{} skinned samples", skinned.len());
		// skin errors are printed, not counted: a known skinned sample must
		// really skin
		assert!(!skinned.is_empty());
		assert!(
			skinned.iter().any(|n| n == "13674780763"),
			"the skinned head should skin: {skinned:?}"
		);
		for bytes in [
			&b""[..],
			b"version 9.00\n",
			b"version 4.00\nnot a mesh",
			b"CSGPHS not physics",
			b"something else",
		] {
			assert!(load_with_skin(bytes).is_err(), "{bytes:?}");
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

	/// A version 4 mesh of one triangle whose `bones` bones all name
	/// themselves from offset 0 of `names`, each weighting nothing.
	fn v4_mesh(bones: u16, names: &[u8]) -> Vec<u8> {
		let mut b = b"version 4.00\n".to_vec();
		let u16s = |b: &mut Vec<u8>, v: u16| b.extend(v.to_le_bytes());
		let u32s = |b: &mut Vec<u8>, v: u32| b.extend(v.to_le_bytes());
		u16s(&mut b, 24);
		u16s(&mut b, 0); // no LODs
		u32s(&mut b, 3); // vertices
		u32s(&mut b, 1); // faces
		u16s(&mut b, 0); // LOD offsets
		u16s(&mut b, bones);
		u32s(&mut b, names.len() as u32);
		u16s(&mut b, 0); // subsets
		b.extend([0, 0]);
		for v in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
			for c in v.into_iter().chain([0.0, 1.0, 0.0, 0.0, 0.0]) {
				b.extend(c.to_le_bytes());
			}
			b.extend([0u8; 8]); // tangent and colour
		}
		b.extend([0u8; 3 * 8]); // envelopes
		for i in 0..3u32 {
			u32s(&mut b, i);
		}
		for _ in 0..bones {
			u32s(&mut b, 0); // name offset
			u16s(&mut b, 0xFFFF);
			u16s(&mut b, 0xFFFF);
			b.extend(0.0f32.to_le_bytes());
			for c in [
				1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
			] {
				b.extend(c.to_le_bytes());
			}
		}
		b.extend(names);
		b
	}

	/// Bones share one name table entry. A name longer than any Roblox name,
	/// or names that add up past the budget when every bone that shares
	/// them is counted, refuse the skin before copying it, and the mesh
	/// still loads without it.
	#[test]
	fn long_or_shared_bone_names_are_bounded() {
		let mut long = vec![b'A'; 1024 * 1024];
		long.push(0);
		let start = std::time::Instant::now();
		let (loaded, skin) = load_with_skin(&v4_mesh(2, &long)).unwrap();
		assert_eq!(loaded.geometry.positions.len(), 3);
		let Err(e) = skin else {
			panic!("a 1 MiB bone name should refuse the skin");
		};
		assert!(e.contains("bone name"), "{e}");
		// the most a name may be, shared by enough bones to pass the total
		let mut name = vec![b'B'; MAX_BONE_NAME];
		name.push(0);
		let bones = (MAX_BONE_NAME_BYTES / MAX_BONE_NAME + 1) as u16;
		let (loaded, skin) = load_with_skin(&v4_mesh(bones, &name)).unwrap();
		assert_eq!(loaded.geometry.positions.len(), 3);
		let Err(e) = skin else {
			panic!("shared names past the total should refuse the skin");
		};
		assert!(e.contains("bone names"), "{e}");
		// a few bones sharing a short name still skin, with the name each
		let (_, skin) = load_with_skin(&v4_mesh(3, b"Root\0")).unwrap();
		assert_eq!(skin.unwrap().unwrap().bones, ["Root"; 3]);
		let took = start.elapsed();
		assert!(took.as_secs_f32() < 1.0, "reading names took {took:?}");
	}

	/// Under a small budget, many bones sharing one name, or overlapping
	/// suffixes of it, are refused once their copies pass the total.
	#[test]
	fn bone_names_stay_within_a_budget() {
		use rbx_mesh::mesh::{Bone4, BoneId4, CFrame4};
		let bone = |pos| Bone4 {
			bone_name_pos: pos,
			parent: BoneId4::new(None),
			lod_parent: BoneId4::new(None),
			cull_distance: 0.0,
			cframe: CFrame4 {
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
			},
		};
		let mut names = vec![b'N'; 60];
		names.push(0);
		let shared: Vec<_> = (0..10_000).map(|_| bone(0)).collect();
		let e = bone_names(&shared, &names, 64, 1024).unwrap_err();
		assert!(e.contains("more than 1024 bytes"), "{e}");
		let suffixes: Vec<_> = (0..10_000).map(|i| bone(i % 60)).collect();
		assert!(bone_names(&suffixes, &names, 64, 1024).is_err());
		assert!(bone_names(&shared[..1], &names, 59, 1024).is_err());
		// within it, every bone gets its name, a name past the table is empty
		let read = bone_names(&[bone(0), bone(58), bone(0), bone(99)], &names, 64, 1024).unwrap();
		assert_eq!(
			read,
			[&names[..60], b"NN", &names[..60], b""]
				.map(|n| String::from_utf8(n.to_vec()).unwrap())
		);
	}
}
