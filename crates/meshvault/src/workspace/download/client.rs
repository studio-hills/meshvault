//! The HTTP client for Roblox asset delivery.
//!
//! Roblox requires authentication for asset delivery. Two methods work:
//! - an Open Cloud API key with the `legacy-asset:manage` scope (recommended),
//!   via `apis.roblox.com/asset-delivery-api/v1/assetId/{id}`, which answers
//!   with a temporary CDN `location` to fetch the content from;
//! - a `.ROBLOSECURITY` cookie, via `assetdelivery.roblox.com/v1/asset/?id={id}`,
//!   which redirects to the CDN.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::content_id::parse_asset_id;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuthMethod {
	#[default]
	ApiKey,
	Cookie,
}

#[derive(Clone)]
pub struct Credentials {
	pub method: AuthMethod,
	pub secret: String,
	/// Sent as `Roblox-Place-Id`; lets you fetch assets used in a place you can edit.
	pub place_id: Option<u64>,
}

// The secret must never end up in logs or error messages.
impl std::fmt::Debug for Credentials {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Credentials")
			.field("method", &self.method)
			.field("place_id", &self.place_id)
			.finish_non_exhaustive()
	}
}

/// Base URLs, overridable so tests can point at a local server.
#[derive(Clone, Debug)]
pub struct Endpoints {
	pub open_cloud: String,
	pub asset_delivery: String,
}

impl Default for Endpoints {
	fn default() -> Self {
		Self {
			open_cloud: "https://apis.roblox.com/asset-delivery-api/v1/assetId/".into(),
			asset_delivery: "https://assetdelivery.roblox.com/v1/asset/?id=".into(),
		}
	}
}

#[derive(Debug)]
pub enum FetchError {
	/// The credentials were rejected; retrying other assets is pointless.
	Auth(String),
	/// Worth retrying after a pause.
	Transient(String, Option<Duration>),
	/// This asset cannot be fetched (missing, no permission, ...).
	Permanent(String),
}

impl std::fmt::Display for FetchError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			FetchError::Auth(m) | FetchError::Transient(m, _) | FetchError::Permanent(m) => {
				f.write_str(m)
			}
		}
	}
}

pub struct Client {
	/// Never follows redirects, so credentials are never sent off-site.
	api: ureq::Agent,
	/// For CDN URLs; carries no credentials.
	cdn: ureq::Agent,
	credentials: Credentials,
	endpoints: Endpoints,
}

const USER_AGENT: &str = concat!("Meshvault/", env!("CARGO_PKG_VERSION"));
/// Largest asset accepted; Roblox meshes and textures are far smaller.
const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;
/// Time allowed for a whole request to the asset delivery APIs.
const API_TIMEOUT: Duration = Duration::from_secs(60);
/// Time allowed for a whole download from the CDN.
const CDN_TIMEOUT: Duration = Duration::from_secs(120);
/// Retries of a transient failure, after the first attempt.
const MAX_RETRIES: u32 = 4;
/// The first retry's wait when the server names none, in milliseconds;
/// it doubles with each retry.
const BACKOFF_BASE_MS: u64 = 500;
/// Longest wait before a retry, whatever the server asks for.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);
/// How often a wait before a retry checks for cancelling.
const CANCEL_POLL: Duration = Duration::from_millis(100);

fn status_error(status: u16, retry_after: Option<Duration>, method: AuthMethod) -> FetchError {
	match status {
		401 => FetchError::Auth(match method {
			AuthMethod::ApiKey => "API key rejected (401). Check the key is valid, not expired, \
				and has the legacy-asset:manage scope."
				.into(),
			AuthMethod::Cookie => "Cookie rejected (401). It may have expired; log in again \
				and copy a fresh .ROBLOSECURITY value."
				.into(),
		}),
		403 => FetchError::Permanent(
			"Access denied (403): your account cannot access this asset. \
			Setting a Place ID for a place that uses it may help."
				.into(),
		),
		404 => FetchError::Permanent("Asset not found (404)".into()),
		429 => FetchError::Transient("Rate limited (429)".into(), retry_after),
		500..=599 => FetchError::Transient(format!("Roblox server error ({status})"), None),
		_ => FetchError::Permanent(format!("Unexpected response ({status})")),
	}
}

fn retry_after(response: &ureq::http::Response<ureq::Body>) -> Option<Duration> {
	response
		.headers()
		.get("retry-after")
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.trim().parse().ok())
		.map(Duration::from_secs)
}

fn transport(e: ureq::Error) -> FetchError {
	FetchError::Transient(format!("Network error: {e}"), None)
}

fn read_body(response: &mut ureq::http::Response<ureq::Body>) -> Result<Vec<u8>, FetchError> {
	let bytes = response
		.body_mut()
		.with_config()
		.limit(MAX_ASSET_BYTES)
		.read_to_vec()
		.map_err(transport)?;
	gunzip_if_needed(bytes)
}

/// The CDN sometimes serves gzip without a Content-Encoding header.
fn gunzip_if_needed(bytes: Vec<u8>) -> Result<Vec<u8>, FetchError> {
	if !bytes.starts_with(&[0x1f, 0x8b]) {
		return Ok(bytes);
	}
	let mut out = Vec::new();
	flate2::read::GzDecoder::new(bytes.as_slice())
		.take(MAX_ASSET_BYTES)
		.read_to_end(&mut out)
		.map_err(|e| FetchError::Permanent(format!("Corrupt gzip data: {e}")))?;
	Ok(out)
}

impl Client {
	// Meshvault installs no logger. If one is ever added, filter out ureq's
	// trace output: ureq-proto logs whole request headers at trace level,
	// which would include the API key or cookie.
	pub fn new(credentials: Credentials, endpoints: Endpoints) -> Self {
		let api = ureq::Agent::config_builder()
			.http_status_as_error(false)
			.max_redirects(0)
			.timeout_global(Some(API_TIMEOUT))
			.user_agent(USER_AGENT)
			.build()
			.into();
		let cdn = ureq::Agent::config_builder()
			.http_status_as_error(false)
			.timeout_global(Some(CDN_TIMEOUT))
			.user_agent(USER_AGENT)
			.build()
			.into();
		Self {
			api,
			cdn,
			credentials,
			endpoints,
		}
	}

	fn cdn_get(&self, url: &str) -> Result<Vec<u8>, FetchError> {
		let mut response = self.cdn.get(url).call().map_err(transport)?;
		let status = response.status().as_u16();
		if status != 200 {
			// CDN errors are never about our credentials
			return Err(
				match status_error(status, retry_after(&response), self.credentials.method) {
					FetchError::Auth(_) => {
						FetchError::Permanent(format!("CDN refused the download ({status})"))
					}
					other => other,
				},
			);
		}
		read_body(&mut response)
	}

	/// Downloads the raw content of one asset.
	pub fn fetch_raw(&self, asset_id: u64) -> Result<Vec<u8>, FetchError> {
		let c = &self.credentials;
		let request = match c.method {
			AuthMethod::ApiKey => self
				.api
				.get(format!("{}{asset_id}", self.endpoints.open_cloud))
				.header("x-api-key", c.secret.trim()),
			AuthMethod::Cookie => {
				let cookie = c.secret.trim().trim_start_matches(".ROBLOSECURITY=");
				self.api
					.get(format!("{}{asset_id}", self.endpoints.asset_delivery))
					.header("Cookie", format!(".ROBLOSECURITY={cookie}"))
			}
		};
		let request = match c.place_id {
			Some(place) => request.header("Roblox-Place-Id", place.to_string()),
			None => request,
		};
		let mut response = request.call().map_err(transport)?;
		let status = response.status().as_u16();
		match status {
			200 => {
				let body = read_body(&mut response)?;
				if c.method == AuthMethod::ApiKey || body.starts_with(b"{\"location\"") {
					#[derive(Deserialize)]
					struct Location {
						location: String,
					}
					let location: Location = serde_json::from_slice(&body).map_err(|_| {
						FetchError::Permanent(
							"Unexpected reply from Roblox (no download location)".into(),
						)
					})?;
					self.cdn_get(&location.location)
				} else {
					Ok(body)
				}
			}
			301 | 302 | 303 | 307 | 308 => {
				let location = response
					.headers()
					.get("location")
					.and_then(|v| v.to_str().ok())
					.ok_or_else(|| FetchError::Permanent("Redirect without a location".into()))?
					.to_string();
				self.cdn_get(&location)
			}
			_ => Err(status_error(status, retry_after(&response), c.method)),
		}
	}

	/// Downloads an asset. When an image is wanted, Decal ids are resolved
	/// to the image they wrap.
	pub fn fetch(&self, asset_id: u64, want_image: bool) -> Result<Vec<u8>, FetchError> {
		let bytes = self.fetch_raw(asset_id)?;
		if want_image
			&& bytes.starts_with(b"<roblox")
			&& let Some(image_id) = decal_image_id(&bytes)
			&& image_id != asset_id
		{
			return self.fetch_raw(image_id);
		}
		Ok(bytes)
	}

	/// Retries transient failures with exponential backoff.
	pub fn fetch_with_retry(
		&self,
		asset_id: u64,
		want_image: bool,
		cancel: &AtomicBool,
	) -> Result<Vec<u8>, FetchError> {
		let mut attempt = 0;
		loop {
			match self.fetch(asset_id, want_image) {
				Err(FetchError::Transient(msg, wait)) if attempt < MAX_RETRIES => {
					let wait = wait.unwrap_or(Duration::from_millis(BACKOFF_BASE_MS << attempt));
					attempt += 1;
					// sleep in slices so cancelling stays responsive
					let mut left = wait.min(MAX_RETRY_WAIT);
					while !left.is_zero() {
						// Relaxed is enough: the flag publishes no other data,
						// it only ends this wait early
						if cancel.load(Ordering::Relaxed) {
							return Err(FetchError::Transient(msg, None));
						}
						let step = left.min(CANCEL_POLL);
						std::thread::sleep(step);
						left -= step;
					}
				}
				other => return other,
			}
		}
	}
}

/// A texture id is often a Decal, whose content is XML pointing at the real image.
fn decal_image_id(xml: &[u8]) -> Option<u64> {
	let text = String::from_utf8_lossy(xml);
	let start = text.find("<url>")? + "<url>".len();
	let end = start + text[start..].find("</url>")?;
	parse_asset_id(&text[start..end])
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::scene::maps::MapKind;
	use crate::workspace::AssetKind;
	use crate::workspace::download::test_server::{client, serve};
	use crate::workspace::store::sniff_extension;
	use std::io::Write;
	use std::sync::Arc;

	#[test]
	fn api_key_follows_location_without_leaking_key() {
		let (cdn, cdn_log) = serve(vec![(
			"/cdn/5",
			200,
			vec![],
			b"version 4.00\nDATA".to_vec(),
		)]);
		let (api, api_log) = serve(vec![(
			"/oc/5",
			200,
			vec![],
			format!(r#"{{"location":"{cdn}/cdn/5"}}"#).into_bytes(),
		)]);
		let c = client(&api, AuthMethod::ApiKey);
		let bytes = c.fetch(5, false).unwrap();
		assert_eq!(bytes, b"version 4.00\nDATA");
		let api_req = api_log.lock().unwrap()[0].to_lowercase();
		assert!(api_req.contains("x-api-key: secret"));
		assert!(api_req.contains("roblox-place-id: 77"));
		let cdn_req = cdn_log.lock().unwrap()[0].to_lowercase();
		assert!(!cdn_req.contains("secret"), "credentials leaked to CDN");
	}

	#[test]
	fn cookie_redirect_strips_cookie_and_resolves_decal() {
		let (cdn, cdn_log) = serve(vec![
			("/cdn/10", 200, vec![], b"<roblox><Item class=\"Decal\"><Content name=\"Texture\"><url>http://www.roblox.com/asset/?id=11</url></Content></Item></roblox>".to_vec()),
			("/cdn/11", 200, vec![], b"\x89PNG\r\n\x1a\nIMAGE".to_vec()),
		]);
		let (base, log) = serve(vec![
			(
				"/ad?id=10",
				302,
				vec![("Location", format!("{cdn}/cdn/10"))],
				vec![],
			),
			(
				"/ad?id=11",
				302,
				vec![("Location", format!("{cdn}/cdn/11"))],
				vec![],
			),
		]);
		let c = client(&base, AuthMethod::Cookie);
		let bytes = c.fetch(10, true).unwrap();
		assert_eq!(
			sniff_extension(&bytes, AssetKind::Map(MapKind::Color)),
			"png"
		);
		assert!(log.lock().unwrap()[0].contains(".ROBLOSECURITY=SECRET"));
		for req in cdn_log.lock().unwrap().iter() {
			assert!(!req.contains("SECRET"), "cookie leaked to CDN");
		}
	}

	#[test]
	fn maps_errors() {
		let (base, _) = serve(vec![
			("/oc/1", 401, vec![], vec![]),
			("/oc/2", 403, vec![], vec![]),
			("/oc/3", 429, vec![("Retry-After", "0".into())], vec![]),
		]);
		let c = client(&base, AuthMethod::ApiKey);
		assert!(matches!(c.fetch(1, false), Err(FetchError::Auth(_))));
		assert!(matches!(c.fetch(2, false), Err(FetchError::Permanent(_))));
		assert!(matches!(
			c.fetch(3, false),
			Err(FetchError::Transient(_, Some(_)))
		));
		assert!(matches!(c.fetch(4, false), Err(FetchError::Permanent(_))));
		let cancel = AtomicBool::new(false);
		// retries a few times then gives up
		assert!(matches!(
			c.fetch_with_retry(3, false, &cancel),
			Err(FetchError::Transient(..))
		));
	}

	/// Cancelling while waiting to retry ends the wait at once, after the
	/// one request already made.
	#[test]
	fn cancel_ends_a_retry_wait() {
		let (base, log) = serve(vec![(
			"/oc/3",
			429,
			vec![("Retry-After", "30".into())],
			vec![],
		)]);
		let c = client(&base, AuthMethod::ApiKey);
		let cancel = Arc::new(AtomicBool::new(false));
		let canceller = {
			let cancel = cancel.clone();
			std::thread::spawn(move || {
				std::thread::sleep(Duration::from_millis(200));
				cancel.store(true, Ordering::Relaxed);
			})
		};
		let started = std::time::Instant::now();
		let result = c.fetch_with_retry(3, false, &cancel);
		canceller.join().unwrap();
		assert!(
			started.elapsed() < Duration::from_secs(10),
			"the 30 s wait ran on"
		);
		assert!(matches!(result, Err(FetchError::Transient(_, None))));
		assert_eq!(log.lock().unwrap().len(), 1);
	}

	/// Today a cancel set before the call still lets the first request
	/// through; it is only seen in the wait before a retry.
	#[test]
	fn cancel_set_first_still_makes_one_request() {
		let (base, log) = serve(vec![(
			"/oc/3",
			429,
			vec![("Retry-After", "30".into())],
			vec![],
		)]);
		let c = client(&base, AuthMethod::ApiKey);
		let cancel = AtomicBool::new(true);
		let result = c.fetch_with_retry(3, false, &cancel);
		assert!(matches!(result, Err(FetchError::Transient(_, None))));
		assert_eq!(log.lock().unwrap().len(), 1);
	}

	#[test]
	fn reads_the_image_id_from_a_decal() {
		let decal = |url: &str| {
			format!(
				"<roblox><Item class=\"Decal\"><Content name=\"Texture\"><url>{url}</url></Content></Item></roblox>"
			)
		};
		let id = |xml: String| decal_image_id(xml.as_bytes());
		assert_eq!(id(decal("http://www.roblox.com/asset/?id=11")), Some(11));
		assert_eq!(id(decal("rbxassetid://12")), Some(12));
		assert_eq!(id(decal(" 13 ")), Some(13));
		assert_eq!(id(decal("rbxasset://textures/face.png")), None);
		assert_eq!(id(decal("")), None);
		// the first url is the one used
		assert_eq!(id(format!("{}{}", decal("14"), decal("15"))), Some(14));
		assert_eq!(id("<roblox><url>16".into()), None);
		assert_eq!(id("<roblox></roblox>".into()), None);
	}

	#[test]
	fn gunzips_unlabelled_gzip() {
		let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
		enc.write_all(b"version 7.00\n").unwrap();
		let gz = enc.finish().unwrap();
		assert_eq!(gunzip_if_needed(gz).unwrap(), b"version 7.00\n");
	}

	#[test]
	fn credentials_debug_hides_secret() {
		let c = Credentials {
			method: AuthMethod::Cookie,
			secret: "TOPSECRET".into(),
			place_id: None,
		};
		assert!(!format!("{c:?}").contains("TOPSECRET"));
	}
}
