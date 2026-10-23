//! Asking GitHub whether a newer Meshvault has been released. The app does
//! this once at startup, unless turned off in the ⋮ menu, and shows a notice
//! when there is one. Nothing is sent but the request itself.

use std::time::Duration;

use serde::Deserialize;

/// GitHub's latest published release (drafts and pre-releases excluded).
pub const LATEST_RELEASE_URL: &str =
	"https://api.github.com/repos/studio-hills/meshvault/releases/latest";
/// Release pages are only opened under this prefix.
const RELEASES_PAGE: &str = "https://github.com/studio-hills/meshvault/releases/";
/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const TIMEOUT: Duration = Duration::from_secs(10);
/// A release's JSON is a few kilobytes; anything far larger is not one.
const MAX_BYTES: u64 = 1024 * 1024;

/// A release newer than this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
	/// e.g. "0.2.0", without the tag's "v".
	pub version: String,
	/// Its page on GitHub.
	pub url: String,
}

#[derive(Deserialize)]
struct LatestRelease {
	tag_name: String,
	html_url: String,
	#[serde(default)]
	draft: bool,
	#[serde(default)]
	prerelease: bool,
}

/// `major.minor.patch` from "v1.2.3" or "1.2.3"; a pre-release or build
/// suffix ("-beta.1", "+abc") is ignored.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
	let text = text.trim().strip_prefix('v').unwrap_or(text.trim());
	let core = text.split(['-', '+']).next()?;
	let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
	let version = (parts.next()??, parts.next()??, parts.next()??);
	parts.next().is_none().then_some(version)
}

/// The release described by GitHub's `releases/latest` JSON, when it is
/// newer than `current`.
pub fn newer_release(json: &[u8], current: &str) -> Result<Option<Release>, String> {
	let latest: LatestRelease =
		serde_json::from_slice(json).map_err(|e| format!("Unexpected release data: {e}"))?;
	if latest.draft || latest.prerelease || !latest.html_url.starts_with(RELEASES_PAGE) {
		return Ok(None);
	}
	let (Some(theirs), Some(ours)) = (parse_version(&latest.tag_name), parse_version(current))
	else {
		return Ok(None);
	};
	Ok((theirs > ours).then(|| Release {
		version: latest.tag_name.trim().trim_start_matches('v').to_owned(),
		url: latest.html_url,
	}))
}

/// Asks `url` (GitHub's latest release) whether there is a newer release
/// than this build. No release yet (404) is not an error.
pub fn check(url: &str) -> Result<Option<Release>, String> {
	let agent: ureq::Agent = ureq::Agent::config_builder()
		.http_status_as_error(false)
		.timeout_global(Some(TIMEOUT))
		.user_agent(concat!("Meshvault/", env!("CARGO_PKG_VERSION")))
		.build()
		.into();
	let mut response = agent
		.get(url)
		.header("Accept", "application/vnd.github+json")
		.call()
		.map_err(|e| format!("Could not check for updates: {e}"))?;
	match response.status().as_u16() {
		200 => {}
		404 => return Ok(None),
		status => return Err(format!("Could not check for updates ({status})")),
	}
	let body = response
		.body_mut()
		.with_config()
		.limit(MAX_BYTES)
		.read_to_vec()
		.map_err(|e| format!("Could not check for updates: {e}"))?;
	newer_release(&body, CURRENT)
}

#[cfg(test)]
mod tests {
	use std::io::{BufRead, BufReader, Write};
	use std::net::TcpListener;

	use super::*;

	fn release(tag: &str) -> String {
		format!(
			r#"{{"tag_name":"{tag}","html_url":"https://github.com/studio-hills/meshvault/releases/tag/{tag}","draft":false,"prerelease":false,"assets":[]}}"#
		)
	}

	#[test]
	fn versions_compare_by_number() {
		for (tag, current, newer) in [
			("v0.2.0", "0.1.0", true),
			("v0.1.1", "0.1.0", true),
			("v1.0.0", "0.9.9", true),
			("v0.10.0", "0.9.0", true),
			("0.2.0", "0.1.0", true),
			("v0.1.0", "0.1.0", false),
			("v0.0.9", "0.1.0", false),
			("v0.2.0-beta.1", "0.2.0", false),
			("v0.3", "0.1.0", false),
			("latest", "0.1.0", false),
		] {
			let found = newer_release(release(tag).as_bytes(), current).unwrap();
			assert_eq!(found.is_some(), newer, "{tag} against {current}");
		}
		let found = newer_release(release("v0.2.0").as_bytes(), "0.1.0").unwrap();
		assert_eq!(
			found,
			Some(Release {
				version: "0.2.0".into(),
				url: "https://github.com/studio-hills/meshvault/releases/tag/v0.2.0".into(),
			})
		);
	}

	/// Only published releases count, and only pages of this repository are
	/// ever offered to open.
	#[test]
	fn drafts_prereleases_and_other_pages_are_ignored() {
		let base = release("v9.0.0");
		for json in [
			base.replace(r#""draft":false"#, r#""draft":true"#),
			base.replace(r#""prerelease":false"#, r#""prerelease":true"#),
			base.replace(
				"https://github.com/studio-hills/meshvault/",
				"https://example.com/",
			),
		] {
			assert_eq!(newer_release(json.as_bytes(), "0.1.0"), Ok(None), "{json}");
		}
		assert!(newer_release(b"<html>", "0.1.0").is_err());
	}

	/// Serves one canned response on a local port and returns its URL.
	fn serve_once(status: &str, body: String) -> String {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let url = format!("http://{}/latest", listener.local_addr().unwrap());
		let status = status.to_owned();
		std::thread::spawn(move || {
			let (stream, _) = listener.accept().unwrap();
			let mut reader = BufReader::new(stream.try_clone().unwrap());
			let mut line = String::new();
			while reader.read_line(&mut line).unwrap() > 2 {
				line.clear();
			}
			let mut stream = stream;
			write!(
				stream,
				"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
				body.len()
			)
			.unwrap();
		});
		url
	}

	#[test]
	fn checking_reads_the_latest_release() {
		let newer = serve_once("200 OK", release("v99.0.0"));
		assert_eq!(check(&newer).unwrap().unwrap().version, "99.0.0");
		let same = serve_once("200 OK", release(&format!("v{CURRENT}")));
		assert_eq!(check(&same), Ok(None));
		// no release published yet
		let none = serve_once("404 Not Found", "{}".into());
		assert_eq!(check(&none), Ok(None));
		let failing = serve_once("503 Service Unavailable", String::new());
		assert!(check(&failing).unwrap_err().contains("503"));
	}
}
