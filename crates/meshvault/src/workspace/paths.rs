//! Where things go in a workspace: the directory names, the organised
//! stems of items, images and rigs, and making names safe as paths.

use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "workspace.json";
pub(super) const MODELS_DIR: &str = "Models";
pub(super) const RIGS_DIR: &str = "Rigs";
pub const CACHE_DIR: &str = ".cache";
/// Images models use directly (decals), one file per asset.
const IMAGES_DIR: &str = "Images";

/// Where an image a model uses is stored, without its extension.
pub(super) fn image_stem(asset_id: u64) -> String {
	format!("{IMAGES_DIR}/{asset_id}")
}

/// The asset id back from an [`image_stem`]. Only the part after the last
/// `/` is read, so any folder (or none) is accepted.
pub(super) fn image_id(stem: &str) -> Option<u64> {
	stem.rsplit('/').next().and_then(|s| s.parse::<u64>().ok())
}

/// Where the raw download of an asset is kept, shared by every item using it.
pub fn cache_path(root: &Path, asset_id: u64) -> PathBuf {
	root.join(CACHE_DIR).join(asset_id.to_string())
}

pub(super) fn rig_stem(file: &str) -> &str {
	file.strip_suffix(".rig.json").unwrap_or(file)
}

/// Makes one path component safe on Windows, macOS and Linux. The result
/// is never empty: a name with nothing left becomes `_`.
pub fn sanitize(name: &str) -> String {
	let mut s: String = name
		.chars()
		.map(|c| match c {
			'<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
			c if c.is_control() => '_',
			c => c,
		})
		.collect();
	// Windows silently drops trailing dots and spaces
	while s.ends_with(['.', ' ']) {
		s.pop();
	}
	let s = s.trim_start().to_string();
	if s.is_empty() {
		return "_".into();
	}
	// Windows also counts the superscript digits as COM and LPT numbers
	const RESERVED: [&str; 28] = [
		"CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
		"COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
		"LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
	];
	// a reserved name is still reserved with an extension (`con.txt`), so
	// the `_` goes before the first dot
	let (base, rest) = s.split_at(s.find('.').unwrap_or(s.len()));
	if RESERVED.contains(&base.to_ascii_uppercase().as_str()) {
		format!("{base}_{rest}")
	} else {
		s
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sanitizes_names() {
		assert_eq!(sanitize("8: CrystalScorpion"), "8_ CrystalScorpion");
		assert_eq!(sanitize("Demon Egg.013"), "Demon Egg.013");
		assert_eq!(sanitize("trailing. "), "trailing");
		assert_eq!(sanitize("CON"), "CON_");
		assert_eq!(sanitize("a/b\\c"), "a_b_c");
		assert_eq!(sanitize("   "), "_");
	}

	/// Reserved device names are made safe with or without an extension,
	/// so a stem's `.mesh` or `.png` cannot make one a device again.
	#[test]
	fn reserved_names_get_the_underscore_before_the_extension() {
		assert_eq!(sanitize("con.txt"), "con_.txt");
		assert_eq!(sanitize("com1.part"), "com1_.part");
		assert_eq!(sanitize("NUL.a.b"), "NUL_.a.b");
		assert_eq!(sanitize("COM¹"), "COM¹_");
		assert_eq!(sanitize("lpt³.png"), "lpt³_.png");
		// names that only start like one are left alone
		assert_eq!(sanitize("console.txt"), "console.txt");
		assert_eq!(sanitize("COM10"), "COM10");
		assert_eq!(sanitize("CON_"), "CON_");
	}

	#[test]
	fn image_ids_round_trip() {
		for id in [0, 77, u64::MAX] {
			assert_eq!(image_id(&image_stem(id)), Some(id));
		}
		assert_eq!(image_id("Images/decal"), None);
		assert_eq!(image_id("Images/"), None);
	}

	/// Today only the last part is read, so these parse too.
	#[test]
	fn image_ids_ignore_the_folder() {
		assert_eq!(image_id("77"), Some(77));
		assert_eq!(image_id("Other/77"), Some(77));
		assert_eq!(image_id("Images/sub/77"), Some(77));
	}
}
