//! Gives the Windows executable its icon and version information.

fn main() {
	println!("cargo:rerun-if-changed=assets/icons/meshvault.ico");
	if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
		let mut res = winresource::WindowsResource::new();
		res.set_icon("assets/icons/meshvault.ico")
			.set("ProductName", "Meshvault")
			.set("FileDescription", "Meshvault")
			.set("OriginalFilename", "meshvault.exe")
			.set("InternalName", "meshvault")
			.set(
				"LegalCopyright",
				"Copyright (c) 2026 Jaden Hills. MIT licence.",
			);
		if let Err(e) = res.compile() {
			panic!("could not embed the Windows resources: {e}");
		}
	}
}
