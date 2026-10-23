//! Where things go in a workspace: the directory names, the organised
//! stems of items, images and rigs, and making names safe as paths.

use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "workspace.json";
pub(super) const MODELS_DIR: &str = "Models";
pub(super) const RIGS_DIR: &str = "Rigs";
pub const CACHE_DIR: &str = ".cache";
/// Images models use directly (decals), one file per asset.
pub(super) const IMAGES_DIR: &str = "Images";

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

/// The most folders an item or model can be in. Deeper ones from an export
/// are cut off when imported, and a manifest with deeper ones is refused.
pub(super) const MAX_FOLDER_DEPTH: usize = 64;
/// The longest folder name, and the longest part of a stored path, in
/// characters (NTFS allows 255).
pub(super) const MAX_NAME: usize = 255;
/// The longest last part of a new stem, leaving room for ` (n)`, a map
/// suffix and an extension within [`MAX_NAME`].
const MAX_STEM_NAME: usize = 200;

fn is_forbidden(c: char) -> bool {
	matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()
}

/// The first `max` characters of `s`.
fn truncate(s: &str, max: usize) -> &str {
	s.char_indices().nth(max).map_or(s, |(i, _)| &s[..i])
}

/// A folder from an export cut to what a manifest may hold: at most
/// [`MAX_FOLDER_DEPTH`] names of at most [`MAX_NAME`] characters.
pub(super) fn fit_folder(folder: &[String]) -> Vec<String> {
	folder
		.iter()
		.take(MAX_FOLDER_DEPTH)
		.map(|f| truncate(f, MAX_NAME).to_string())
		.collect()
}

/// [`sanitize`] for the last part of a new stem, short enough that its
/// files' names fit.
pub(super) fn stem_name(name: &str) -> String {
	sanitize(truncate(&sanitize(name), MAX_STEM_NAME))
}

/// Checks a folder read from a manifest: not too deep or long.
pub(super) fn check_folder(folder: &[String]) -> Result<(), String> {
	if folder.len() > MAX_FOLDER_DEPTH {
		return Err(format!(
			"is {} folders deep (at most {MAX_FOLDER_DEPTH})",
			folder.len()
		));
	}
	match folder.iter().find(|f| f.chars().count() > MAX_NAME) {
		Some(_) => Err(format!("has a name over {MAX_NAME} characters")),
		None => Ok(()),
	}
}

/// Checks a path read from a manifest, which is joined to the workspace's
/// root, so that it stays inside the folder `top` of the workspace: parts
/// separated by `/` (as the app writes them; `\` is refused), none empty,
/// `.` or `..`, no drive, root, UNC or `\\?\` prefix, no `:` (alternate
/// data streams), and each part as [`sanitize`] leaves names: no forbidden
/// or control characters, no leading space or trailing dot or space, no
/// reserved device name. At most [`MAX_FOLDER_DEPTH`] + 2 parts of
/// [`MAX_NAME`] characters, which bounds its length too.
///
/// Older versions put the underscore of a reserved name at the end
/// (`con.txt_`), and did not reserve the superscript COM and LPT names;
/// those names are accepted, since workspaces hold them.
pub(super) fn check_path(path: &str, top: &str) -> Result<(), String> {
	let parts: Vec<&str> = path.split('/').collect();
	if parts.first() != Some(&top) || parts.len() < 2 {
		return Err(format!("{path:?} is not in {top}/"));
	}
	if parts.len() > MAX_FOLDER_DEPTH + 2 {
		return Err(format!("{:?}... is too deep", truncate(path, 64)));
	}
	for part in &parts[1..] {
		let bad = if part.is_empty() {
			Some("an empty part")
		} else if part.chars().count() > MAX_NAME {
			Some("a part that is too long")
		} else if part.chars().any(is_forbidden) {
			Some("a character not allowed in file names")
		} else if part.ends_with(['.', ' ']) || part.trim_start() != *part {
			Some("a part with a trailing dot or a space at either end")
		} else if is_reserved(part) {
			Some("a reserved device name")
		} else {
			None
		};
		if let Some(bad) = bad {
			return Err(format!(
				"{:?} is not a plain path in the workspace: it has {bad}",
				truncate(path, 260)
			));
		}
	}
	Ok(())
}

/// The workspace's own folders and files, under its root. A workspace in
/// which any of them is a link is refused when opened (see
/// [`check_no_links`]).
pub(super) const OWN_ENTRIES: [&str; 8] = [
	MANIFEST,
	CACHE_DIR,
	MODELS_DIR,
	RIGS_DIR,
	IMAGES_DIR,
	".cache/format",
	".cache/thumbnails",
	".cache/facts.json",
];

/// Refuses `rel`, a path inside the workspace at `root`, if any part of it
/// below `root` is a symbolic link or a junction, which could take a read,
/// write or delete out of the workspace. Parts that do not exist yet are
/// fine: Meshvault makes them as plain folders and files. `root` itself
/// may be a link: it is the folder the user chose.
///
/// On Windows a junction and a symbolic link are both "name surrogate"
/// reparse points, which is what [`std::fs::FileType::is_symlink`] tests
/// for. Other reparse points (OneDrive's placeholders, deduplicated files)
/// redirect nothing, so they are allowed.
///
/// This checks the folder as it is now: a link put in while Meshvault is
/// writing is not caught.
pub(crate) fn check_no_links(root: &Path, rel: &Path) -> Result<(), String> {
	let mut path = root.to_path_buf();
	let mut shown = String::new();
	for part in rel.components() {
		let std::path::Component::Normal(name) = part else {
			return Err(format!(
				"{} is not a path inside the workspace",
				rel.display()
			));
		};
		path.push(name);
		if !shown.is_empty() {
			shown.push('/');
		}
		shown.push_str(&name.to_string_lossy());
		match std::fs::symlink_metadata(&path) {
			Ok(meta) if meta.file_type().is_symlink() => {
				return Err(format!(
					"{shown} in the workspace is a link to somewhere else (a junction or \
					 symbolic link), which Meshvault does not follow"
				));
			}
			Ok(_) => {}
			// nothing below a missing part exists either
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
			Err(e) => return Err(format!("Could not check {shown} in the workspace: {e}")),
		}
	}
	Ok(())
}

/// Whether a name read from a manifest is a device name: the part before
/// its first dot is one, unless a later part ends with the underscore
/// older versions added at the end.
fn is_reserved(name: &str) -> bool {
	let mut parts = name.split('.');
	let base = parts.next().unwrap_or("").to_ascii_uppercase();
	RESERVED[..22].contains(&base.as_str()) && !parts.any(|p| p.ends_with('_'))
}

// The superscript names come last: older versions did not reserve them.
const RESERVED: [&str; 28] = [
	"CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
	"COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "COM¹", "COM²",
	"COM³", "LPT¹", "LPT²", "LPT³",
];

/// Makes one path component safe on Windows, macOS and Linux. The result
/// is never empty: a name with nothing left becomes `_`.
pub fn sanitize(name: &str) -> String {
	let mut s: String = name
		.chars()
		.map(|c| if is_forbidden(c) { '_' } else { c })
		.collect();
	// Windows silently drops trailing dots and spaces
	while s.ends_with(['.', ' ']) {
		s.pop();
	}
	let s = s.trim_start().to_string();
	if s.is_empty() {
		return "_".into();
	}
	// Windows also counts the superscript digits as COM and LPT numbers;
	// a reserved name is still reserved with an extension (`con.txt`), so
	// the `_` goes before the first dot
	let (base, rest) = s.split_at(s.find('.').unwrap_or(s.len()));
	if RESERVED.contains(&base.to_ascii_uppercase().as_str()) {
		format!("{base}_{rest}")
	} else {
		s
	}
}

/// Makes `link` a directory junction (a symbolic link elsewhere) to
/// `target`, for tests. Junctions need no admin rights, but some machines
/// still refuse them: then this prints a note and returns false, and the
/// test calling it is skipped.
#[cfg(test)]
pub(crate) fn junction(link: &Path, target: &Path) -> bool {
	// mklink reads a `/` in a path as the start of an option
	#[cfg(windows)]
	let backslashed = |p: &Path| p.components().collect::<PathBuf>();
	#[cfg(windows)]
	let made = std::process::Command::new("cmd")
		.arg("/C")
		.arg("mklink")
		.arg("/J")
		.arg(backslashed(link))
		.arg(backslashed(target))
		.output()
		.is_ok_and(|out| out.status.success());
	#[cfg(not(windows))]
	let made = std::os::unix::fs::symlink(target, link).is_ok();
	if !made {
		println!("skipped: could not make a junction at {}", link.display());
	}
	made
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

	/// Only plain paths under the expected folder pass; see the workspace's
	/// tests for each manifest field.
	#[test]
	fn checks_paths_from_manifests() {
		for good in [
			"Models/A",
			"Models/Zone_10/8_ Scorpion/Body (2).normal.png",
			"Models/con_.mesh",
			"Models/con.txt_.mesh",
			"Models/COM¹.mesh",
		] {
			assert_eq!(check_path(good, MODELS_DIR), Ok(()), "{good}");
		}
		for bad in [
			"Models",
			"Models/",
			"Models//A",
			"models/A",
			"Rigs/A",
			"../A",
			"Models/../A",
			"Models/./A",
			"/Models/A",
			r"Models\A",
			r"C:\A",
			"C:/A",
			"C:A",
			r"\\server\share\A",
			"//server/share/A",
			r"\\?\C:\A",
			r"\\.\COM1",
			"Models/A.mesh:evil",
			"Models/A:Zone.Identifier:$DATA",
			"Models/A.",
			"Models/A ",
			"Models/ A",
			"Models/a\u{1}b",
			"Models/a*b",
			"Models/CON",
			"Models/nul.mesh",
			"Models/Lpt9.a.b",
		] {
			assert!(check_path(bad, MODELS_DIR).is_err(), "{bad}");
		}
		let deep = format!("Models/{}", ["a"; MAX_FOLDER_DEPTH + 2].join("/"));
		assert!(check_path(&deep, MODELS_DIR).is_err());
		let long = format!("Models/{}", "a".repeat(MAX_NAME + 1));
		assert!(check_path(&long, MODELS_DIR).is_err());
		let deepest = format!("Models/{}", ["a"; MAX_FOLDER_DEPTH + 1].join("/"));
		assert_eq!(check_path(&deepest, MODELS_DIR), Ok(()));
	}

	/// New stems and folders always pass the checks a manifest gets, however
	/// deep or long the export's names.
	#[test]
	fn new_names_fit_the_checks() {
		let folder: Vec<String> = (0..100)
			.map(|i| format!("{i}{}", "x".repeat(300)))
			.collect();
		let fitted = fit_folder(&folder);
		assert_eq!(check_folder(&fitted), Ok(()));
		assert!(check_folder(&folder).is_err());
		let name = stem_name(&format!("{}.  ", "é".repeat(400)));
		assert!(name.chars().count() <= MAX_STEM_NAME);
		let parts: Vec<String> = fitted.iter().map(|f| sanitize(f)).collect();
		let file = format!("Models/{}/{name} (99).metalness.rbxmx", parts.join("/"));
		assert_eq!(check_path(&file, MODELS_DIR), Ok(()));
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
	/// A link anywhere below the root is refused, at the end of the path or
	/// on the way; missing parts are fine, and so is a root that is a link.
	#[test]
	fn finds_links_in_a_path() {
		let dir = crate::test_support::TempDir::new("paths_links");
		let (root, outside) = (dir.join("ws"), dir.join("outside"));
		std::fs::create_dir_all(root.join("Models/F")).unwrap();
		std::fs::create_dir_all(&outside).unwrap();
		let check = |rel: &str| check_no_links(&root, Path::new(rel));
		assert_eq!(check("Models/F/A.mesh"), Ok(()));
		assert_eq!(check("Models/new/deeper/A.mesh"), Ok(()));
		assert!(check("Models/../x").is_err());
		if !junction(&root.join("Models/L"), &outside) {
			return;
		}
		let error = check("Models/L/A.mesh").unwrap_err();
		assert!(
			error.starts_with("Models/L in the workspace is a link"),
			"{error}"
		);
		assert!(check("Models/L").is_err());
		assert_eq!(check("Models/F/A.mesh"), Ok(()));

		let linked_root = dir.join("chosen");
		if !junction(&linked_root, &root) {
			return;
		}
		assert_eq!(check_no_links(&linked_root, Path::new("Models/F")), Ok(()));
		assert!(check_no_links(&linked_root, Path::new("Models/L/A.mesh")).is_err());
	}
}
