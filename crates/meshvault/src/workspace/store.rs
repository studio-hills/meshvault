//! Writing downloaded assets into the workspace layout: picking a file
//! extension from the content, and the organised path of each target.

use std::path::Path;

use crate::workspace::{AssetKind, Job};

/// Picks a file extension from the content.
pub fn sniff_extension(bytes: &[u8], kind: AssetKind) -> &'static str {
	if bytes.starts_with(b"version ") {
		"mesh"
	} else if bytes.starts_with(b"\x89PNG") {
		"png"
	} else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
		"jpg"
	} else if bytes.starts_with(b"BM") && kind.is_image() {
		"bmp"
	} else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
		"webp"
	} else if bytes.starts_with(b"<roblox!") {
		"rbxm"
	} else if bytes.starts_with(b"<roblox") {
		"rbxmx"
	} else {
		"bin"
	}
}

/// One file written for a job: which item and use it was for, and where it went.
pub struct Stored {
	pub stem: String,
	pub kind: AssetKind,
	/// Relative to the workspace root.
	pub path: String,
}

/// Writes a downloaded asset to each item's organized path.
pub fn store(root: &Path, job: &Job, bytes: &[u8]) -> Result<Vec<Stored>, String> {
	let mut written = Vec::new();
	for (stem, kind) in &job.targets {
		let ext = sniff_extension(bytes, *kind);
		let rel = format!("{stem}{}.{ext}", kind.file_suffix());
		let path = root.join(&rel);
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)
				.map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
		}
		std::fs::write(&path, bytes)
			.map_err(|e| format!("Could not write {}: {e}", path.display()))?;
		written.push(Stored {
			stem: stem.clone(),
			kind: *kind,
			path: rel,
		});
	}
	Ok(written)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::scene::maps::MapKind;

	#[test]
	fn sniffs_each_kind_of_file() {
		let mesh = AssetKind::Mesh;
		let map = AssetKind::Map(MapKind::Color);
		assert_eq!(sniff_extension(b"version 4.00\n", mesh), "mesh");
		assert_eq!(sniff_extension(b"\x89PNG\r\n\x1a\n", map), "png");
		assert_eq!(sniff_extension(&[0xFF, 0xD8, 0xFF, 0xE0], map), "jpg");
		assert_eq!(sniff_extension(b"RIFF\0\0\0\0WEBPVP8 ", map), "webp");
		assert_eq!(sniff_extension(b"<roblox!\x89\xff", mesh), "rbxm");
		assert_eq!(sniff_extension(b"<roblox xmlns:xmime=", map), "rbxmx");
		assert_eq!(sniff_extension(b"something else", mesh), "bin");
		assert_eq!(sniff_extension(b"", map), "bin");
	}

	/// "BM" starts too many other files to trust it for a mesh.
	#[test]
	fn bmp_is_only_sniffed_for_images() {
		assert_eq!(
			sniff_extension(b"BM\x36\0", AssetKind::Map(MapKind::Normal)),
			"bmp"
		);
		assert_eq!(sniff_extension(b"BM\x36\0", AssetKind::Image), "bmp");
		assert_eq!(sniff_extension(b"BM\x36\0", AssetKind::Mesh), "bin");
	}

	/// WEBP needs more than its 12-byte header, and "WEBP" in the right place.
	#[test]
	fn webp_needs_its_whole_header() {
		let map = AssetKind::Map(MapKind::Color);
		assert_eq!(sniff_extension(b"RIFF\0\0\0\0WEBP", map), "bin");
		assert_eq!(sniff_extension(b"RIFF\0\0\0\0WAVEfmt ", map), "bin");
	}
}
