// v6.00 / v7.00 chunks (added in Meshvault), built byte by byte: there is no
// v6.00 sample file, and these cover the paths the v7.00 sample does not.

use crate::mesh::{
	Chunk6, Chunks6, CoreMeshChunk, Facs6, FacsChunk, LodsChunk, Mesh, Mesh6, SkinningChunk,
};
use binrw::{BinRead, BinReaderExt};
use std::io::Cursor;

/// A chunk: `[u8; 8]` name, `u32` version, `u32` size, then the payload.
/// `size` may differ from the payload's length, for padding and overruns.
fn sized_chunk(name: &[u8; 8], version: u32, size: u32, payload: &[u8]) -> Vec<u8> {
	let mut bytes = name.to_vec();
	bytes.extend(version.to_le_bytes());
	bytes.extend(size.to_le_bytes());
	bytes.extend(payload);
	bytes
}

fn chunk(name: &[u8; 8], version: u32, payload: &[u8]) -> Vec<u8> {
	sized_chunk(name, version, payload.len() as u32, payload)
}

fn mesh6(chunks: &[Vec<u8>]) -> Vec<u8> {
	let mut bytes = b"version 6.00\n".to_vec();
	bytes.extend(chunks.concat());
	bytes
}

fn read6(bytes: Vec<u8>) -> binrw::BinResult<Mesh6> {
	Cursor::new(bytes).read_le()
}

fn u32s(values: &[u32]) -> Vec<u8> {
	values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// COREMESH v1 with no vertices and no faces.
fn empty_core_mesh() -> Vec<u8> {
	u32s(&[0, 0])
}

/// HSRAVIS v1 with ten faces: bits 0, 7 and 9 set, either side of the
/// byte boundary.
fn hsr_avis() -> Vec<u8> {
	let mut payload = u32s(&[10]);
	payload.extend([0b1000_0001, 0b0000_0010]);
	payload
}

/// A valid empty `Facs5`: no names or correctives, and six raw 0×0 matrices.
fn empty_facs5() -> Vec<u8> {
	let raw_empty_matrix = [&1u16.to_le_bytes()[..], &u32s(&[0, 0])].concat();
	let mut bytes = u32s(&[0, 0]);
	// quantized_transforms_len: six matrices of 10 bytes each
	bytes.extend(60u64.to_le_bytes());
	bytes.extend(u32s(&[0, 0]));
	for _ in 0..6 {
		bytes.extend(&raw_empty_matrix);
	}
	bytes
}

#[test]
fn chunked_v6_reads_known_and_unknown_chunks() {
	let bytes = mesh6(&[
		chunk(b"COREMESH", 1, &empty_core_mesh()),
		chunk(b"FOOBAR\0\0", 3, b"xyz"),
		chunk(b"HSRAVIS\0", 1, &hsr_avis()),
	]);
	let Ok(Mesh::V6(mesh)) = crate::read_mesh_versioned(Cursor::new(bytes)) else {
		panic!("expected a v6.00 mesh");
	};
	assert_eq!(mesh.chunks.len(), 3);
	let Some(CoreMeshChunk::V1(core)) = mesh.core_mesh() else {
		panic!("expected an uncompressed COREMESH chunk");
	};
	assert!(core.vertices.is_empty());
	assert!(core.faces.is_empty());
	let Chunk6::Unknown(unknown) = &mesh.chunks[1] else {
		panic!("expected an unknown chunk");
	};
	assert_eq!(&unknown.name, b"FOOBAR\0\0");
	assert_eq!(unknown.version, 3);
	assert_eq!(unknown.data, b"xyz");
	let hsr = mesh.hsr_avis().unwrap();
	assert_eq!(hsr.bit_count, 10);
	let bits: Vec<_> = (0..11).map(|face| hsr.get(face)).collect();
	let (t, f) = (Some(true), Some(false));
	assert_eq!(bits, [t, f, f, f, f, f, f, t, f, t, None]);
	assert!(mesh.lods().is_none());
	assert!(mesh.skinning().is_none());
	assert!(mesh.facs().is_none());
}

#[test]
fn chunk_with_unknown_version_is_kept() {
	let mesh = read6(mesh6(&[
		chunk(b"LODS\0\0\0\0", 9, &[1, 2, 3]),
		chunk(b"COREMESH", 3, &[4]),
	]))
	.unwrap();
	let Chunk6::Lods(LodsChunk::Unsupported(lods)) = &mesh.chunks[0] else {
		panic!("expected an unsupported LODS chunk");
	};
	assert_eq!(lods.version, 9);
	assert_eq!(lods.data, [1, 2, 3]);
	assert!(mesh.lods().is_none());
	let Some(CoreMeshChunk::Unsupported(core)) = mesh.core_mesh() else {
		panic!("expected an unsupported COREMESH chunk");
	};
	assert_eq!(core.version, 3);
	assert_eq!(core.data, [4]);
}

#[test]
fn chunk_padding_is_skipped() {
	let mut padded = empty_core_mesh();
	padded.extend([0xee; 8]);
	let mesh = read6(mesh6(&[
		sized_chunk(b"COREMESH", 1, 16, &padded),
		chunk(b"HSRAVIS\0", 1, &hsr_avis()),
	]))
	.unwrap();
	assert_eq!(mesh.chunks.len(), 2);
	assert!(matches!(mesh.core_mesh(), Some(CoreMeshChunk::V1(_))));
	assert_eq!(mesh.hsr_avis().unwrap().bit_count, 10);
}

#[test]
fn chunk_overrun_is_an_error() {
	// the whole payload is there, but the size says it is only 4 bytes
	let bytes = mesh6(&[
		sized_chunk(b"COREMESH", 1, 4, &empty_core_mesh()),
		chunk(b"HSRAVIS\0", 1, &hsr_avis()),
	]);
	let error = read6(bytes).unwrap_err();
	assert!(
		error
			.to_string()
			.contains("chunk payload overran its size of 4 bytes by 4"),
		"{error}"
	);
}

#[test]
fn facs6_pads_to_its_own_length() {
	let facs5 = empty_facs5();
	assert_eq!(facs5.len(), 84);
	// facs_len counts 8 bytes of padding after the FACS data
	let mut bytes = u32s(&[84 + 8]);
	bytes.extend(&facs5);
	bytes.extend([0xee; 8]);
	bytes.extend(b"next");
	let mut cursor = Cursor::new(bytes);
	let facs = Facs6::read_le(&mut cursor).unwrap();
	assert_eq!(cursor.position(), 4 + 84 + 8);
	assert!(facs.facs.face_bone_names.is_empty());
	assert!(facs.facs.two_pose_correctives.is_empty());

	// and inside a mesh, through the accessor
	let mut payload = u32s(&[84]);
	payload.extend(&facs5);
	let mesh = read6(mesh6(&[chunk(b"FACS\0\0\0\0", 1, &payload)])).unwrap();
	assert!(matches!(mesh.chunks[0], Chunk6::Facs(FacsChunk::V1(_))));
	let facs = mesh.facs().unwrap();
	assert!(facs.face_control_names.is_empty());
	assert!(facs.three_pose_correctives.is_empty());
}

#[test]
fn skinning_chunk_is_read() {
	// one envelope, no bones, one bone name, no subsets
	let mut payload = u32s(&[1]);
	payload.extend([0, 0, 0, 0, 255, 0, 0, 0]);
	payload.extend(u32s(&[0, 5]));
	payload.extend(b"Root\0");
	payload.extend(u32s(&[0]));
	let mesh = read6(mesh6(&[chunk(b"SKINNING", 1, &payload)])).unwrap();
	assert!(matches!(
		mesh.chunks[0],
		Chunk6::Skinning(SkinningChunk::V1(_))
	));
	let skinning = mesh.skinning().unwrap();
	assert_eq!(skinning.envelopes.len(), 1);
	assert_eq!(skinning.envelopes[0].bones, [0; 4]);
	assert_eq!(skinning.envelopes[0].weights, [255, 0, 0, 0]);
	assert!(skinning.bones.is_empty());
	assert_eq!(skinning.bone_names, b"Root\0");
	assert!(skinning.subsets.is_empty());
}

// How malformed tails are treated today (B50). Stricter rejection is a
// separate format-policy decision (D20); these pin the current behaviour.
#[test]
fn malformed_tails_today() {
	// a final chunk whose size runs past the end of the file is accepted
	let mesh = read6(mesh6(&[sized_chunk(
		b"COREMESH",
		1,
		100,
		&empty_core_mesh(),
	)]))
	.unwrap();
	assert_eq!(mesh.chunks.len(), 1);

	// a chunk name cut short at the end is ignored
	let mut bytes = mesh6(&[chunk(b"COREMESH", 1, &empty_core_mesh())]);
	bytes.extend(b"LODS");
	assert_eq!(read6(bytes).unwrap().chunks.len(), 1);

	// a chunk cut short after its name, or inside its payload, is an error
	let mut bytes = mesh6(&[chunk(b"COREMESH", 1, &empty_core_mesh())]);
	bytes.extend(b"HSRAVIS\0");
	assert!(read6(bytes).is_err());
	let mut cut = chunk(b"FOOBAR\0\0", 1, b"xyz");
	cut.truncate(cut.len() - 1);
	assert!(read6(mesh6(&[cut])).is_err());
}
