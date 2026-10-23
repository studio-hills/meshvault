//! Helpers that hand work to the operating system.

use std::io;
use std::path::Path;
use std::process::Command;

/// Opens a file browser at `path` (selecting it if it is a file). Only a
/// failure to start the file browser is reported; it is not waited for.
pub fn reveal(path: &Path) -> io::Result<()> {
	#[cfg(target_os = "windows")]
	let mut cmd = {
		use std::os::windows::process::CommandExt;
		let mut cmd = Command::new("explorer");
		if path.is_file() {
			cmd.raw_arg(select_arg(path));
		} else {
			cmd.arg(native(path));
		}
		cmd
	};
	#[cfg(target_os = "macos")]
	let mut cmd = {
		let mut cmd = Command::new("open");
		cmd.arg("-R").arg(path);
		cmd
	};
	#[cfg(not(any(target_os = "windows", target_os = "macos")))]
	let mut cmd = {
		let dir = if path.is_file() {
			path.parent().unwrap_or(path)
		} else {
			path
		};
		let mut cmd = Command::new("xdg-open");
		cmd.arg(dir);
		cmd
	};
	cmd.spawn().map(drop)
}

/// `path` with the platform's own separators. Workspaces store their files'
/// paths with `/`, which Explorer does not understand: given one it opens
/// Documents instead.
#[cfg(any(target_os = "windows", test))]
fn native(path: &Path) -> std::path::PathBuf {
	path.components().collect()
}

/// Explorer's argument to open a file's folder with it selected:
/// `/select,"<path>"`. It goes on the command line as it is, because
/// Explorer reads its own command line and only knows `/select,` when it
/// is not quoted, while `Command::arg` quotes the whole argument when the
/// path has a space in it. A Windows path cannot hold `"`, so quoting the
/// path alone is safe, and keeps a comma in it from ending the path.
#[cfg(any(target_os = "windows", test))]
fn select_arg(path: &Path) -> std::ffi::OsString {
	let mut arg = std::ffi::OsString::from("/select,\"");
	arg.push(native(path));
	arg.push("\"");
	arg
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn explorer_selects_a_path_with_spaces_quoted_after_the_switch() {
		assert_eq!(
			select_arg(Path::new(r"C:\My workspace\Models\a, b.mesh")),
			r#"/select,"C:\My workspace\Models\a, b.mesh""#
		);
	}

	/// Workspaces store their files' paths with `/`, which Explorer does
	/// not understand: given one, it opened Documents instead.
	#[cfg(windows)]
	#[test]
	fn explorer_gets_backslashes_only() {
		let rig = Path::new(r"C:\Users\me\Meshvault\testing")
			.join("Rigs/ReplicatedStorage/Animals/Bat.rig.json");
		assert_eq!(
			select_arg(&rig),
			r#"/select,"C:\Users\me\Meshvault\testing\Rigs\ReplicatedStorage\Animals\Bat.rig.json""#
		);
	}
}
