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

/// Where the raw download of an asset is kept, shared by every item using it.
pub fn cache_path(root: &Path, asset_id: u64) -> PathBuf {
	root.join(CACHE_DIR).join(asset_id.to_string())
}

pub(super) fn rig_stem(file: &str) -> &str {
	file.strip_suffix(".rig.json").unwrap_or(file)
}

/// Makes one path component safe on Windows, macOS and Linux.
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
	const RESERVED: [&str; 22] = [
		"CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
		"COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
	];
	let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
	if RESERVED.contains(&base.as_str()) {
		format!("{s}_")
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
		assert_eq!(sanitize("con.txt"), "con.txt_");
		assert_eq!(sanitize("a/b\\c"), "a_b_c");
		assert_eq!(sanitize("   "), "_");
	}
}
