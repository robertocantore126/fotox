//! Windows resources for `fotox.exe`: the app icon and version information.
//!
//! Ported from `reference/graphite-desktop/platform-win/build.rs`. The icon is
//! `assets/fotox.ico`, generated from the UI's logo mark by
//! `assets/make_icon.py`; the version is the crate version.

fn main() {
	println!("cargo:rerun-if-changed=assets/fotox.ico");
	println!("cargo:rerun-if-changed=build.rs");

	#[cfg(target_os = "windows")]
	windows_resources();
}

#[cfg(target_os = "windows")]
fn windows_resources() {
	let version = env!("CARGO_PKG_VERSION");
	let mut parts = version.split(['.', '-']).map(|p| p.parse::<u64>().unwrap_or(0));
	let (major, minor, patch) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
	let dotted = format!("{major}.{minor}.{patch}.0");

	let mut res = winres::WindowsResource::new();
	res.set_icon("assets/fotox.ico");
	res.set_language(0x0409); // English (US)
	res.set_version_info(winres::VersionInfo::FILEVERSION, (major << 48) | (minor << 32) | (patch << 16));
	res.set_version_info(winres::VersionInfo::PRODUCTVERSION, (major << 48) | (minor << 32) | (patch << 16));
	res.set("FileVersion", &dotted);
	res.set("ProductVersion", &dotted);
	res.set("OriginalFilename", "fotox.exe");
	res.set("FileDescription", "Fotox");
	res.set("ProductName", "Fotox");
	res.set("LegalCopyright", "Copyright © 2026 Rob");
	if let Err(error) = res.compile() {
		// A missing resource compiler must fail the build loudly, not ship an
		// exe without an icon.
		panic!("failed to compile the Windows resources (is the Windows SDK's rc.exe installed?): {error}");
	}
}
