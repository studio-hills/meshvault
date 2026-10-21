//! Exporting models out of Meshvault: OBJ with MTL, and binary glTF for
//! Blender.

pub mod gltf;
pub mod obj;

/// Fixtures shared by the OBJ and glTF tests.
#[cfg(test)]
mod fixtures {
	use std::path::PathBuf;

	use serde_json::Value;

	use crate::scene::geometry;
	use crate::test_support::SAMPLE_MESHES;

	/// The first sample mesh, by file name, that has triangles (and texture
	/// coordinates when `need_uvs`), so every run picks the same one.
	pub(super) fn sample_mesh(need_uvs: bool) -> PathBuf {
		let mut paths: Vec<PathBuf> = std::fs::read_dir(SAMPLE_MESHES)
			.unwrap()
			.map(|e| e.unwrap().path())
			.collect();
		paths.sort();
		paths
			.into_iter()
			.find(|p| {
				std::fs::read(p)
					.ok()
					.and_then(|b| geometry::load(&b).ok())
					.is_some_and(|l| {
						!l.geometry.triangles.is_empty() && (!need_uvs || l.geometry.uvs.is_some())
					})
			})
			.expect("a sample mesh")
	}

	/// Splits a binary glTF into its JSON document and binary chunk,
	/// checking the header and chunk lengths on the way.
	pub(super) fn parse_glb(glb: &[u8]) -> (Value, &[u8]) {
		let word = |at: usize| u32::from_le_bytes(glb[at..at + 4].try_into().unwrap()) as usize;
		assert_eq!(&glb[..4], b"glTF");
		assert_eq!(word(4), 2, "version");
		assert_eq!(word(8), glb.len(), "total length");
		let json_len = word(12);
		assert_eq!(&glb[16..20], b"JSON");
		let json = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
		let at = 20 + json_len;
		let bin_len = word(at);
		assert_eq!(&glb[at + 4..at + 8], b"BIN\0");
		assert_eq!(
			at + 8 + bin_len,
			glb.len(),
			"the binary chunk ends the file"
		);
		(json, &glb[at + 8..])
	}
}
