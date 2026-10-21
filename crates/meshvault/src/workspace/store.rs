//! Writing downloaded assets into the workspace layout: picking a file
//! extension from the content, the raw cache of each asset, and the
//! organised path of each target (written beside the cache by the worker,
//! then moved into place by the workspace).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::workspace::{AssetKind, Job, cache_path};

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

/// A name beside `path` for a file being written, unique within this run of
/// the app and across apps (by process id), so writers never share one.
fn temp_beside(path: &Path) -> PathBuf {
	static NEXT: AtomicU64 = AtomicU64::new(0);
	let n = NEXT.fetch_add(1, Ordering::Relaxed);
	let name = path.file_name().unwrap_or_default().to_string_lossy();
	path.with_file_name(format!("{name}.{}-{n}.tmp", std::process::id()))
}

/// The raw download of an asset, if it is cached. An empty file is not
/// trusted: it is what a write cut short leaves behind.
pub fn read_cache(root: &Path, asset_id: u64) -> Option<Vec<u8>> {
	std::fs::read(cache_path(root, asset_id))
		.ok()
		.filter(|bytes| !bytes.is_empty())
}

/// Caches the raw download of an asset, best effort: the organised files
/// are written from the bytes in hand, so a failed cache write only means
/// downloading again next time. The file is written under a temporary name
/// and renamed, so the cache never holds part of an asset.
pub fn write_cache(root: &Path, asset_id: u64, bytes: &[u8]) {
	let cache = cache_path(root, asset_id);
	if let Some(dir) = cache.parent() {
		let _ = std::fs::create_dir_all(dir);
	}
	let tmp = temp_beside(&cache);
	if std::fs::write(&tmp, bytes)
		.and_then(|()| std::fs::rename(&tmp, &cache))
		.is_err()
	{
		let _ = std::fs::remove_file(&tmp);
	}
}

/// One file written for a job, not yet in place: which item and use it is
/// for, where it goes, and where it waits until the workspace takes it
/// ([`Self::publish`]) or turns it down ([`Self::discard`]).
pub struct Stored {
	pub stem: String,
	pub kind: AssetKind,
	/// Relative to the workspace root.
	pub path: String,
	/// The written file, under the workspace's cache folder.
	staged: PathBuf,
}

impl Stored {
	/// Moves the file into place under `root`, replacing any file there.
	pub fn publish(&self, root: &Path) -> Result<(), String> {
		let path = root.join(&self.path);
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)
				.map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
		}
		std::fs::rename(&self.staged, &path).map_err(|e| {
			self.discard();
			format!("Could not write {}: {e}", path.display())
		})
	}

	/// Deletes the file, for a target nothing wants any more.
	pub fn discard(&self) {
		let _ = std::fs::remove_file(&self.staged);
	}
}

/// Writes a downloaded asset once for each target of the job, beside the
/// cache, ready to move to each item's organised path. Nothing in the
/// organised folders changes until the workspace publishes them, so a job
/// that finishes after its item was removed (and its stem perhaps reused)
/// cannot touch another item's files.
pub fn stage(root: &Path, job: &Job, bytes: &[u8]) -> Result<Vec<Stored>, String> {
	let mut written: Vec<Stored> = Vec::new();
	for (stem, kind) in &job.targets {
		let ext = sniff_extension(bytes, *kind);
		let staged = temp_beside(&cache_path(root, job.asset_id));
		let result = staged
			.parent()
			.map_or(Ok(()), std::fs::create_dir_all)
			.and_then(|()| std::fs::write(&staged, bytes));
		if let Err(e) = result {
			let _ = std::fs::remove_file(&staged);
			written.iter().for_each(Stored::discard);
			return Err(format!("Could not write {}: {e}", staged.display()));
		}
		written.push(Stored {
			stem: stem.clone(),
			kind: *kind,
			path: format!("{stem}{}.{ext}", kind.file_suffix()),
			staged,
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
