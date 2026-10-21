use super::roundtrip;
use crate::union_physics::{CSGK, CSGPHS3, CSGPHS5, CSGPHS7};
use std::fs::read;

#[test]
fn csgk() {
	let bytes = read("meshes/CSGK.data").unwrap();
	let mesh = roundtrip::<CSGK>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x6d0f6b15f32f7a5b);
}
#[test]
fn csgphs_3() {
	let bytes = read("meshes/CSGPHS_3.data").unwrap();
	let mesh = roundtrip::<CSGPHS3>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x5a48c4ffe6d8e5e5);
}
#[test]
fn csgphs_5() {
	let bytes = read("meshes/CSGPHS_5.data").unwrap();
	let mesh = roundtrip::<CSGPHS5>(bytes).unwrap();
	assert_debug_digest!(mesh, 0xcbb6aff31b5da707);
}
#[test]
fn csgphs_7() {
	let bytes = read("meshes/CSGPHS_7.data").unwrap();
	let mesh = roundtrip::<CSGPHS7>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x4900036ad9eeed77);
}
#[cfg(any(feature = "csgphs-v8-zstd", feature = "csgphs-v8-ruzstd"))]
#[test]
fn csgphs_8() {
	use crate::union_physics::CSGPHS8;
	let bytes = read("meshes/CSGPHS_8_00.data").unwrap();
	let mesh = super::readonly::<CSGPHS8>(bytes).unwrap();
	assert_debug_digest!(mesh, 0x829c246a9fe38c06);
}
#[cfg(any(feature = "csgphs-v8-zstd", feature = "csgphs-v8-ruzstd"))]
#[test]
fn csgphs_8_raw_hull_1() {
	use crate::union_physics::CSGPHS8;
	let bytes = read("meshes/CSGPHS_8_raw_hulls_206.data").unwrap();
	let mesh = super::readonly::<CSGPHS8>(bytes).unwrap();
	assert_debug_digest!(mesh, 0xfb0dd2e3d59ea81a);
}
#[cfg(any(feature = "csgphs-v8-zstd", feature = "csgphs-v8-ruzstd"))]
#[test]
fn csgphs_8_raw_hull_2() {
	use crate::union_physics::CSGPHS8;
	let bytes = read("meshes/CSGPHS_8_raw_hulls_972.data").unwrap();
	let mesh = super::readonly::<CSGPHS8>(bytes).unwrap();
	assert_debug_digest!(mesh, 0xd24ab97e19eb4355);
}
