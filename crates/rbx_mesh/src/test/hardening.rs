//! Crafted inputs whose declared counts are far larger than the data behind
//! them. Each must come back as an error, without reserving for the count.

use super::with_alloc_cap;
use crate::mesh::Mesh;
use binrw::BinReaderExt;
use std::io::Cursor;

// far above anything these tiny inputs need, far below what the counts ask for
const CAP: usize = 16 << 20;

fn read_mesh(bytes: Vec<u8>) -> binrw::BinResult<Mesh> {
	with_alloc_cap(CAP, || crate::read_mesh_versioned(Cursor::new(bytes)))
}

#[cfg(feature = "mesh-v1")]
#[test]
fn mesh_v1_face_count_overflow_is_an_error() {
	// 3 * face_count does not fit a u32
	let text = format!(
		"version 1.00\n{}\n[0,0,0][0,0,0][0,0,0]\n",
		u32::MAX / 3 + 1
	);
	assert!(read_mesh(text.into_bytes()).is_err());
}

#[cfg(feature = "mesh-v1")]
#[test]
fn mesh_v1_face_count_beyond_the_line_is_an_error() {
	let text = "version 1.00\n100000000\n[0,0,0][0,0,0][0,0,0]\n";
	assert!(read_mesh(text.as_bytes().to_vec()).is_err());
}

fn mesh4_header(bone_names_len: u32) -> Vec<u8> {
	let mut b = b"version 4.00\n".to_vec();
	b.extend(24u16.to_le_bytes());
	b.extend(0u16.to_le_bytes()); // lod type
	b.extend(0u32.to_le_bytes()); // vertices
	b.extend(0u32.to_le_bytes()); // faces
	b.extend(0u16.to_le_bytes()); // lods
	b.extend(0u16.to_le_bytes()); // bones
	b.extend(bone_names_len.to_le_bytes());
	b.extend(0u16.to_le_bytes()); // subsets
	b.extend([0, 0]); // lod_hq_count, padding
	b
}

#[test]
fn mesh_v4_bone_names_len_beyond_the_file_is_an_error() {
	assert!(read_mesh(mesh4_header(u32::MAX)).is_err());
}

#[test]
fn mesh_v4_bone_names_within_the_file_still_read() {
	let mut b = mesh4_header(3);
	b.extend(b"abc");
	let Mesh::V4(mesh) = read_mesh(b).unwrap() else {
		panic!("not v4")
	};
	assert_eq!(mesh.bone_names, b"abc");
}

#[test]
fn mesh_v6_chunk_size_beyond_the_file_is_an_error() {
	let mut b = b"version 6.00\n".to_vec();
	b.extend(b"UNKNOWN!");
	b.extend(1u32.to_le_bytes());
	b.extend(u32::MAX.to_le_bytes());
	assert!(read_mesh(b).is_err());

	let mut b = b"version 7.00\n".to_vec();
	b.extend(b"COREMESH");
	b.extend(2u32.to_le_bytes());
	b.extend(8u32.to_le_bytes()); // chunk size
	b.extend(u32::MAX.to_le_bytes()); // Draco bitstream length
	assert!(read_mesh(b).is_err());
}

#[test]
fn mesh_v5_matrix_size_overflow_is_an_error() {
	use crate::mesh::QuantizedMatrix5;
	// x * y does not fit a u32
	let mut b = 1u16.to_le_bytes().to_vec();
	b.extend(0x10000u32.to_le_bytes());
	b.extend(0x10000u32.to_le_bytes());
	let read = with_alloc_cap(CAP, || Cursor::new(b).read_le::<QuantizedMatrix5>());
	assert!(read.is_err());
}
