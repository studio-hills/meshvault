//! Shared fixtures for Meshvault's tests: where the sample meshes are, and a
//! temporary folder that cleans itself up.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The sample meshes vendored with rbx_mesh. Join file names onto it with
/// `Path::new(SAMPLE_MESHES).join(name)`.
pub(crate) const SAMPLE_MESHES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../rbx_mesh/meshes");

/// Decodes the sample mesh with this file name.
pub(crate) fn load_sample(name: &str) -> crate::scene::geometry::Loaded {
	let path = std::path::Path::new(crate::test_support::SAMPLE_MESHES).join(name);
	crate::scene::geometry::load(&std::fs::read(path).unwrap()).unwrap()
}

/// An empty folder under the system temp folder, deleted (best effort) when
/// dropped, so a failing test does not leave it behind. Each call gets its
/// own folder, so tests running in parallel never share one.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
	/// Creates `meshvault_<tag>_<pid>_<n>`, emptying anything left there by an
	/// earlier run with the same process id.
	pub(crate) fn new(tag: &str) -> Self {
		static NEXT: AtomicUsize = AtomicUsize::new(0);
		let n = NEXT.fetch_add(1, Ordering::Relaxed);
		let dir = std::env::temp_dir().join(format!("meshvault_{tag}_{}_{n}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		Self(dir)
	}
}

impl Deref for TempDir {
	type Target = Path;

	fn deref(&self) -> &Path {
		&self.0
	}
}

impl AsRef<Path> for TempDir {
	fn as_ref(&self) -> &Path {
		&self.0
	}
}

impl Drop for TempDir {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.0);
	}
}

/// The export file the ignored real-export tests read, from
/// `MESHVAULT_EXPORT`. The older names `RBX_V1_EXPORT` and `RBX_EXPORT` are
/// still accepted, for one release. Panics when none is set, so an ignored
/// run without one fails instead of passing having tested nothing.
pub(crate) fn real_export() -> PathBuf {
	["MESHVAULT_EXPORT", "RBX_V1_EXPORT", "RBX_EXPORT"]
		.into_iter()
		.find_map(std::env::var_os)
		.map(PathBuf::from)
		.expect("set MESHVAULT_EXPORT to an export file")
}
