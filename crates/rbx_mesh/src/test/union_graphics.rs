use super::{readonly, roundtrip, with_alloc_cap};
use crate::union_graphics::{CSGMDL2, CSGMDL4, CSGMDL5};
use binrw::BinReaderExt;
use std::fs::read;
use std::io::Cursor;

// far above anything the crafted inputs need, far below what their counts ask for
const CAP: usize = 16 << 20;

#[test]
fn meshdata_385416572_2() {
	let bytes = read("meshes/385416572.meshdata").unwrap();
	let mesh = roundtrip::<CSGMDL2>(bytes).unwrap();
	assert_debug_digest!(mesh, 0xd2ad6742b3bdf3c3);
}
#[test]
fn meshdata_394453730_2() {
	let bytes = read("meshes/394453730.meshdata").unwrap();
	let mesh = roundtrip::<CSGMDL2>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x849f54c6df8e40da);
}
#[test]
fn meshdata_5692112940_2() {
	let bytes = read("meshes/5692112940_2.meshdata").unwrap();
	let mesh = roundtrip::<CSGMDL2>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x273f06e074243ee6);
}
#[test]
fn meshdata_4500696697_4() {
	let bytes = read("meshes/4500696697_4.meshdata").unwrap();
	let mesh = roundtrip::<CSGMDL4>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x4753eddf31d04ec2);
}
#[test]
fn meshdata_15124417947_5() {
	let bytes = read("meshes/15124417947_5.meshdata").unwrap();
	let mesh = readonly::<CSGMDL5>(bytes).unwrap();
	assert_debug_digest!(mesh, 0xafab69b0d5361969);
}
#[test]
fn meshdata_14846974687_5() {
	let bytes = read("meshes/14846974687_5.meshdata").unwrap();
	let mesh = readonly::<CSGMDL5>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x14d0ca78bd1aee15);
}
#[test]
fn meshdata_13626979828_5() {
	let bytes = read("meshes/13626979828.meshdata5").unwrap();
	let mesh = readonly::<CSGMDL5>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x6f1709cd66ddafc1);
}

// crafted CSGMDL5 inputs whose declared counts are far larger than their data

fn csgmdl5(vertex_count: u32, vertex_data: &[u8]) -> Vec<u8> {
	let mut b = b"\x15\x7d\x29\x15\x75\x6c\x35\x04\x34\x69".to_vec();
	b.extend(0u16.to_le_bytes()); // positions
	b.extend(0u16.to_le_bytes()); // normals
	b.extend(0u32.to_le_bytes()); // normals len
	b.extend(0u16.to_le_bytes()); // colors
	b.extend(0u16.to_le_bytes()); // normal ids
	b.extend(0u16.to_le_bytes()); // tex
	b.extend(0u16.to_le_bytes()); // tangents
	b.extend(0u32.to_le_bytes()); // tangents len
	b.extend(vertex_count.to_le_bytes());
	b.extend((vertex_data.len() as u32).to_le_bytes());
	b.extend(vertex_data);
	b
}

#[test]
fn csgmdl5_vertex_count_beyond_the_data_is_an_error() {
	let mut b = csgmdl5(u32::MAX, &[]);
	b.push(0); // range markers
	let read = with_alloc_cap(CAP, || Cursor::new(b).read_le::<CSGMDL5>());
	assert!(read.is_err());
}

#[test]
fn csgmdl5_vertex_data_len_beyond_the_file_is_an_error() {
	let mut b = csgmdl5(0, &[]);
	let len = b.len();
	b[len - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
	let read = with_alloc_cap(CAP, || Cursor::new(b).read_le::<CSGMDL5>());
	assert!(read.is_err());
}
