//! Helpers that hand work to the operating system.

use std::io;
use std::path::Path;
use std::process::Command;

/// Opens a file browser at `path` (selecting it if it is a file). Only a
/// failure to start the file browser is reported; it is not waited for.
pub fn reveal(path: &Path) -> io::Result<()> {
	#[cfg(target_os = "windows")]
	let mut cmd = {
		let mut cmd = Command::new("explorer");
		if path.is_file() {
			cmd.arg(format!("/select,{}", path.display()));
		} else {
			cmd.arg(path);
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
