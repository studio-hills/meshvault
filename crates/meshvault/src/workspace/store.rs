//! Writing downloaded assets into the workspace layout: picking a file
//! extension from the content, the raw cache of each asset, and the
//! organised path of each target (written beside the cache by the worker,
//! then moved into place by the workspace).

use std::path::{Path, PathBuf};

use crate::studio::export_file::read_limited;
use crate::workspace::paths::{
	IMAGES_DIR, MODELS_DIR, check_no_links, check_path, replace_file, saved_file, write_temp,
};
use crate::workspace::{AssetKind, CACHE_DIR, Job, cache_path};

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

/// [`cache_path`] relative to the workspace's root.
fn cache_rel(asset_id: u64) -> PathBuf {
	Path::new(CACHE_DIR).join(asset_id.to_string())
}

/// The cache's format, kept in [`FORMAT_FILE`]: 2 since entries are written
/// whole. Before that an entry was written in place, so one cut short by a
/// crash could hold only the start of its asset, and was used as it was.
const CACHE_FORMAT: &str = "2";
const FORMAT_FILE: &str = "format";
/// The longest marker read: it only ever holds [`CACHE_FORMAT`].
const MAX_FORMAT_BYTES: usize = 16;

/// The largest asset read from a workspace, its cache or a file opened in
/// the viewer: the largest download accepted.
pub const MAX_ASSET_BYTES: usize = crate::workspace::download::MAX_ASSET_BYTES as usize;

/// Reads an asset (a cached download, a mesh or a texture) whole, unless it
/// is larger than [`MAX_ASSET_BYTES`].
pub fn read_asset(path: &Path) -> std::io::Result<Vec<u8>> {
	read_limited(path, MAX_ASSET_BYTES)
}

/// Whether the cache's marker `marker` names [`CACHE_FORMAT`]. A marker
/// longer than `limit` bytes does not.
fn is_current(marker: &Path, limit: usize) -> bool {
	read_limited(marker, limit).is_ok_and(|s| s.trim_ascii() == CACHE_FORMAT.as_bytes())
}

/// Brings the workspace's cache up to [`CACHE_FORMAT`]: entries from
/// before it are deleted once, since one cut short cannot be told from a
/// whole one, so each is downloaded again when next needed. Best effort:
/// if one cannot be deleted, the format is not marked and it is tried again
/// next time. Thumbnails, facts and organised files are left alone.
/// Nothing is done if the cache or its marker is a link (see
/// [`check_no_links`]).
pub fn upgrade_cache(root: &Path) {
	if check_no_links(root, &Path::new(CACHE_DIR).join(FORMAT_FILE)).is_err() {
		return;
	}
	let dir = root.join(CACHE_DIR);
	let marker = dir.join(FORMAT_FILE);
	if is_current(&marker, MAX_FORMAT_BYTES) {
		return;
	}
	let mut cleared = true;
	match std::fs::read_dir(&dir) {
		Ok(entries) => {
			for entry in entries.flatten() {
				let name = entry.file_name();
				let is_asset = name
					.to_str()
					.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
				if is_asset
					&& let Err(e) = std::fs::remove_file(entry.path())
					&& e.kind() != std::io::ErrorKind::NotFound
				{
					cleared = false;
				}
			}
		}
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
		Err(_) => cleared = false,
	}
	if cleared {
		let _ = std::fs::create_dir_all(&dir);
		let _ = replace_file(
			root,
			&Path::new(CACHE_DIR).join(FORMAT_FILE),
			CACHE_FORMAT.as_bytes(),
		);
	}
}

/// The raw download of an asset, if it is cached. An empty file is not
/// trusted: it is what a write cut short leaves behind. Nor is one read
/// through a link (see [`saved_file`]), or one larger than
/// [`MAX_ASSET_BYTES`]: like a missing one, it is downloaded again and
/// replaced.
pub fn read_cache(root: &Path, asset_id: u64) -> Option<Vec<u8>> {
	read_cache_within(root, asset_id, MAX_ASSET_BYTES)
}

/// [`read_cache`], reading at most `limit` bytes.
fn read_cache_within(root: &Path, asset_id: u64, limit: usize) -> Option<Vec<u8>> {
	let file = saved_file(root, cache_rel(asset_id)).ok()?;
	read_limited(&file, limit)
		.ok()
		.filter(|bytes| !bytes.is_empty())
}

/// Caches the raw download of an asset, best effort: the organised files
/// are written from the bytes in hand, so a failed cache write only means
/// downloading again next time. The file is written under a temporary name
/// and renamed, so the cache never holds part of an asset, and nothing is
/// written through a link (see [`replace_file`]).
pub fn write_cache(root: &Path, asset_id: u64, bytes: &[u8]) {
	// before making any folder
	if check_no_links(root, &cache_rel(asset_id)).is_err() {
		return;
	}
	if let Some(dir) = cache_path(root, asset_id).parent() {
		let _ = std::fs::create_dir_all(dir);
	}
	let _ = replace_file(root, &cache_rel(asset_id), bytes);
}

/// One file written for a job, not yet in place: which item and use it is
/// for, where it goes, and where it waits until the workspace takes it
/// ([`Self::publish`]). The waiting file is deleted when this is dropped
/// unless it was published, so a target nothing wants any more, a
/// publication that fails at any step, and a result that never reaches
/// the workspace all leave nothing behind.
pub struct Stored {
	pub stem: String,
	pub kind: AssetKind,
	/// Relative to the workspace root.
	pub path: String,
	/// The written file, under the workspace's cache folder: relative to
	/// the root, and in full.
	staged_rel: PathBuf,
	staged: PathBuf,
	published: bool,
}

impl Stored {
	/// Moves the file into place under `root`, replacing any file there.
	/// Refused unless its path is a plain one under `Images/` (an image) or
	/// `Models/` (see [`check_path`]) with no link on the way (see
	/// [`check_no_links`]), so no stem can put it elsewhere.
	pub fn publish(&mut self, root: &Path) -> Result<(), String> {
		let top = if self.kind == AssetKind::Image {
			IMAGES_DIR
		} else {
			MODELS_DIR
		};
		check_path(&self.path, top).map_err(|e| format!("Could not write {:?}: {e}", self.path))?;
		check_no_links(root, Path::new(&self.path))
			.map_err(|e| format!("Could not write {}: {e}", self.path))?;
		// nor from one: the staged file is this job's own, made new
		check_no_links(root, &self.staged_rel)
			.map_err(|e| format!("Could not write {}: {e}", self.path))?;
		let path = root.join(&self.path);
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)
				.map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
		}
		std::fs::rename(&self.staged, &path)
			.map_err(|e| format!("Could not write {}: {e}", path.display()))?;
		self.published = true;
		Ok(())
	}
}

impl Drop for Stored {
	fn drop(&mut self) {
		if !self.published {
			let _ = std::fs::remove_file(&self.staged);
		}
	}
}

/// Writes a downloaded asset once for each target of the job, beside the
/// cache, ready to move to each item's organised path. Nothing in the
/// organised folders changes until the workspace publishes them, so a job
/// that finishes after its item was removed (and its stem perhaps reused)
/// cannot touch another item's files.
pub fn stage(root: &Path, job: &Job, bytes: &[u8]) -> Result<Vec<Stored>, String> {
	// the staged files go in the cache folder
	check_no_links(root, &cache_rel(job.asset_id)).map_err(|e| {
		format!(
			"Could not write {}: {e}",
			cache_path(root, job.asset_id).display()
		)
	})?;
	// dropped, and so deleted, if a later target cannot be written
	let mut written: Vec<Stored> = Vec::new();
	for (stem, kind) in &job.targets {
		let ext = sniff_extension(bytes, *kind);
		let beside = cache_rel(job.asset_id);
		let staged = std::fs::create_dir_all(root.join(CACHE_DIR))
			.and_then(|()| write_temp(root, &beside, bytes))
			.map_err(|e| format!("Could not write {}: {e}", root.join(&beside).display()))?;
		let staged_rel = staged.strip_prefix(root).unwrap_or(&staged).to_path_buf();
		written.push(Stored {
			stem: stem.clone(),
			kind: *kind,
			path: format!("{stem}{}.{ext}", kind.file_suffix()),
			staged_rel,
			staged,
			published: false,
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

	/// A cache entry is replaced by renaming a whole new file over it, not
	/// rewritten in place: a second name for the old file still reads the
	/// old asset, and no temporary file is left.
	#[test]
	fn the_cache_is_replaced_whole_without_leftovers() {
		let dir = crate::test_support::TempDir::new("store_cache_whole");
		write_cache(&dir, 8, b"OLD");
		let old = dir.join("old");
		std::fs::hard_link(cache_path(&dir, 8), &old).unwrap();
		write_cache(&dir, 8, b"NEW");
		assert_eq!(std::fs::read(&old).unwrap(), b"OLD");
		assert_eq!(read_cache(&dir, 8).as_deref(), Some(&b"NEW"[..]));
		let names: Vec<_> = std::fs::read_dir(dir.join(CACHE_DIR))
			.unwrap()
			.map(|e| e.unwrap().file_name())
			.collect();
		assert_eq!(names, ["8"]);
	}
	/// R3-01: a link already at the temporary name a cache entry or a
	/// staged download is written under is never written through: the file
	/// it points to outside the workspace is left alone, and the asset is
	/// still written (under a fresh name).
	#[test]
	fn temporary_files_are_never_written_through_a_link() {
		use crate::workspace::paths::{file_link, set_temp_tags};
		let parent = crate::test_support::TempDir::new("store_temp_link");
		let dir = parent.join("ws");
		std::fs::create_dir_all(dir.join(CACHE_DIR)).unwrap();
		let outside = parent.join("outside.txt");
		std::fs::write(&outside, b"keep me").unwrap();
		let kind = file_link(&dir.join(CACHE_DIR).join("8.a.tmp"), &outside);
		file_link(&dir.join(CACHE_DIR).join("8.c.tmp"), &outside);

		set_temp_tags(Some(&["a", "b", "c", "d"]));
		write_cache(&dir, 8, b"version 4.00\n");
		assert_eq!(std::fs::read(&outside).unwrap(), b"keep me", "{kind}");
		assert_eq!(read_cache(&dir, 8).as_deref(), Some(&b"version 4.00\n"[..]));

		let job = Job {
			asset_id: 8,
			targets: vec![("Models/A".into(), AssetKind::Mesh)],
			place_id: None,
		};
		let staged = stage(&dir, &job, b"version 4.00\n");
		set_temp_tags(None);
		assert_eq!(std::fs::read(&outside).unwrap(), b"keep me", "{kind}");
		staged.unwrap()[0].publish(&dir).unwrap();
		assert_eq!(
			std::fs::read(dir.join("Models/A.mesh")).unwrap(),
			b"version 4.00\n"
		);
	}

	/// N02: a cache entry or format marker longer than its limit is not
	/// read: the entry is a miss, so it is downloaded again, and the marker
	/// is not current, so the cache is upgraded.
	#[test]
	fn oversized_cache_files_are_not_read() {
		let dir = crate::test_support::TempDir::new("store_cache_limit");
		write_cache(&dir, 8, b"version 4.00\n");
		assert_eq!(
			read_cache_within(&dir, 8, 13).as_deref(),
			Some(&b"version 4.00\n"[..])
		);
		assert_eq!(read_cache_within(&dir, 8, 12), None);
		let marker = dir.join(CACHE_DIR).join(FORMAT_FILE);
		std::fs::write(&marker, "2\n").unwrap();
		assert!(is_current(&marker, 2));
		assert!(!is_current(&marker, 1));
	}
}
