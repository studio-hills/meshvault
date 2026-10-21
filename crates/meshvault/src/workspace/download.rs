//! Downloading assets from Roblox and writing them into a workspace.
//!
//! Roblox requires authentication for asset delivery. Two methods work:
//! - an Open Cloud API key with the `legacy-asset:manage` scope (recommended),
//!   via `apis.roblox.com/asset-delivery-api/v1/assetId/{id}`, which answers
//!   with a temporary CDN `location` to fetch the content from;
//! - a `.ROBLOSECURITY` cookie, via `assetdelivery.roblox.com/v1/asset/?id={id}`,
//!   which redirects to the CDN.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::studio::export_file::parse_asset_id;
use crate::workspace::{AssetKind, Job};

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
			.timeout_global(Some(Duration::from_secs(60)))
			.user_agent(USER_AGENT)
			.build()
			.into();
		let cdn = ureq::Agent::config_builder()
			.http_status_as_error(false)
			.timeout_global(Some(Duration::from_secs(120)))
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
				Err(FetchError::Transient(msg, wait)) if attempt < 4 => {
					let wait = wait.unwrap_or(Duration::from_millis(500 << attempt));
					attempt += 1;
					// sleep in slices so cancelling stays responsive
					let mut left = wait.min(Duration::from_secs(60));
					while !left.is_zero() {
						if cancel.load(Ordering::Relaxed) {
							return Err(FetchError::Transient(msg, None));
						}
						let step = left.min(Duration::from_millis(100));
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

/// One file written for a job: which item and use it was for, and where it went.
pub struct Stored {
	pub stem: String,
	pub kind: AssetKind,
	/// Relative to the workspace root.
	pub path: String,
}

/// Writes a downloaded asset to each item's organized path.
pub fn store(root: &Path, job: &Job, bytes: &[u8]) -> Result<Vec<Stored>, String> {
	let mut written = Vec::new();
	for (stem, kind) in &job.targets {
		let ext = sniff_extension(bytes, *kind);
		let rel = format!("{stem}{}.{ext}", kind.file_suffix());
		let path = root.join(&rel);
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)
				.map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
		}
		std::fs::write(&path, bytes)
			.map_err(|e| format!("Could not write {}: {e}", path.display()))?;
		written.push(Stored {
			stem: stem.clone(),
			kind: *kind,
			path: rel,
		});
	}
	Ok(written)
}

pub enum Event {
	Finished {
		job: Job,
		result: Result<Vec<Stored>, String>,
	},
	/// Credentials were rejected; the rest of the queue was cancelled.
	AuthFailed(String),
}

#[derive(Default)]
struct Shared {
	queue: Mutex<VecDeque<Job>>,
	cancel: AtomicBool,
	workers: AtomicUsize,
}

pub struct Downloader {
	shared: Arc<Shared>,
	tx: Sender<Event>,
	rx: Receiver<Event>,
	pub total: usize,
	pub completed: usize,
	pub failed: usize,
}

impl Default for Downloader {
	fn default() -> Self {
		let (tx, rx) = channel();
		Self {
			shared: Arc::default(),
			tx,
			rx,
			total: 0,
			completed: 0,
			failed: 0,
		}
	}
}

impl Downloader {
	pub fn is_running(&self) -> bool {
		self.shared.workers.load(Ordering::SeqCst) > 0
	}

	/// Queues jobs (skipping ones already queued) and starts workers.
	pub fn start(
		&mut self,
		ctx: &eframe::egui::Context,
		root: PathBuf,
		client: Arc<Client>,
		jobs: Vec<Job>,
		concurrency: usize,
	) {
		if !self.is_running() {
			self.total = 0;
			self.completed = 0;
			self.failed = 0;
			self.shared.cancel.store(false, Ordering::SeqCst);
		}
		let mut queue = self.shared.queue.lock().unwrap();
		for job in jobs {
			// already queued: make sure every target gets written
			if let Some(queued) = queue.iter_mut().find(|q| q.asset_id == job.asset_id) {
				for target in job.targets {
					if !queued.targets.contains(&target) {
						queued.targets.push(target);
					}
				}
			} else {
				queue.push_back(job);
				self.total += 1;
			}
		}
		let wanted = concurrency.clamp(1, 16).min(queue.len());
		drop(queue);
		while self.shared.workers.load(Ordering::SeqCst) < wanted {
			self.shared.workers.fetch_add(1, Ordering::SeqCst);
			let shared = self.shared.clone();
			let tx = self.tx.clone();
			let ctx = ctx.clone();
			let root = root.clone();
			let client = client.clone();
			std::thread::spawn(move || {
				worker(&shared, &tx, &ctx, &root, &client);
				shared.workers.fetch_sub(1, Ordering::SeqCst);
				ctx.request_repaint();
			});
		}
	}

	/// Stops after the downloads in flight; queued jobs stay pending.
	pub fn cancel(&mut self) {
		self.shared.cancel.store(true, Ordering::SeqCst);
		let dropped = {
			let mut queue = self.shared.queue.lock().unwrap();
			let n = queue.len();
			queue.clear();
			n
		};
		self.total -= dropped;
	}

	pub fn poll(&mut self) -> Vec<Event> {
		let events: Vec<Event> = self.rx.try_iter().collect();
		for event in &events {
			if let Event::Finished { result, .. } = event {
				self.completed += 1;
				if result.is_err() {
					self.failed += 1;
				}
			}
		}
		events
	}
}

fn worker(
	shared: &Shared,
	tx: &Sender<Event>,
	ctx: &eframe::egui::Context,
	root: &Path,
	client: &Client,
) {
	loop {
		if shared.cancel.load(Ordering::SeqCst) {
			return;
		}
		let Some(job) = shared.queue.lock().unwrap().pop_front() else {
			return;
		};
		let cache = crate::workspace::cache_path(root, job.asset_id);
		let bytes = match std::fs::read(&cache) {
			Ok(bytes) => Ok(bytes),
			Err(_) => {
				match client.fetch_with_retry(job.asset_id, job.wants_image(), &shared.cancel) {
					Ok(bytes) => {
						if let Some(dir) = cache.parent() {
							let _ = std::fs::create_dir_all(dir);
						}
						let _ = std::fs::write(&cache, &bytes);
						Ok(bytes)
					}
					Err(FetchError::Auth(message)) => {
						shared.cancel.store(true, Ordering::SeqCst);
						shared.queue.lock().unwrap().clear();
						let _ = tx.send(Event::AuthFailed(message));
						ctx.request_repaint();
						return;
					}
					// cancelled mid-retry: leave the item pending
					Err(FetchError::Transient(..)) if shared.cancel.load(Ordering::SeqCst) => {
						return;
					}
					Err(e) => Err(e.to_string()),
				}
			}
		};
		let result = bytes.and_then(|bytes| store(root, &job, &bytes));
		let _ = tx.send(Event::Finished { job, result });
		ctx.request_repaint();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::scene::maps::MapKind;
	use crate::test_support::{SAMPLE_MESHES, TempDir};
	use std::io::{BufRead, BufReader, Write};
	use std::net::TcpListener;

	/// A minimal HTTP server: `routes` maps a request path to
	/// (status, extra headers, body). Records every request line and headers.
	/// (path, status, extra headers, body)
	type Route = (&'static str, u16, Vec<(&'static str, String)>, Vec<u8>);

	fn serve(routes: Vec<Route>) -> (String, Arc<Mutex<Vec<String>>>) {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let log = Arc::new(Mutex::new(Vec::new()));
		let log2 = log.clone();
		std::thread::spawn(move || {
			for stream in listener.incoming() {
				let mut stream = stream.unwrap();
				let mut reader = BufReader::new(stream.try_clone().unwrap());
				let mut request = String::new();
				loop {
					let mut line = String::new();
					if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
						break;
					}
					request.push_str(&line);
				}
				let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
				log2.lock().unwrap().push(request);
				let (status, headers, body) = routes
					.iter()
					.find(|(p, ..)| *p == path)
					.map(|(_, s, h, b)| (*s, h.clone(), b.clone()))
					.unwrap_or((404, vec![], b"not found".to_vec()));
				let mut head = format!(
					"HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
					body.len()
				);
				for (k, v) in headers {
					head.push_str(&format!("{k}: {v}\r\n"));
				}
				head.push_str("\r\n");
				let _ = stream.write_all(head.as_bytes());
				let _ = stream.write_all(&body);
			}
		});
		(base, log)
	}

	fn client(base: &str, method: AuthMethod) -> Client {
		Client::new(
			Credentials {
				method,
				secret: "SECRET".into(),
				place_id: Some(77),
			},
			Endpoints {
				open_cloud: format!("{base}/oc/"),
				asset_delivery: format!("{base}/ad?id="),
			},
		)
	}

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

	/// Full pipeline: export entries -> workspace -> worker pool -> organized files.
	#[test]
	fn downloads_into_organized_workspace() {
		use crate::studio::export_file::Entry;
		use crate::workspace::{AssetState, Workspace};

		let mesh = std::fs::read(Path::new(SAMPLE_MESHES).join("torso.mesh")).unwrap();
		let png = b"\x89PNG\r\n\x1a\nFAKE".to_vec();
		let (cdn, _) = serve(vec![
			("/cdn/1", 200, vec![], mesh.clone()),
			("/cdn/2", 200, vec![], png.clone()),
			("/cdn/4", 200, vec![], png.clone()),
		]);
		let (api, api_log) = serve(vec![
			(
				"/oc/1",
				200,
				vec![],
				format!(r#"{{"location":"{cdn}/cdn/1"}}"#).into_bytes(),
			),
			(
				"/oc/2",
				200,
				vec![],
				format!(r#"{{"location":"{cdn}/cdn/2"}}"#).into_bytes(),
			),
			("/oc/3", 403, vec![], vec![]),
			(
				"/oc/4",
				200,
				vec![],
				format!(r#"{{"location":"{cdn}/cdn/4"}}"#).into_bytes(),
			),
		]);

		let dir = TempDir::new("download");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let entry = |folder: &str, name: &str, mesh_id, maps: &[(MapKind, u64)]| Entry {
			name: name.into(),
			folder: folder.split('/').map(String::from).collect(),
			mesh_id,
			maps: maps.iter().copied().collect(),
			source_id: None,
		};
		ws.add([
			// TextureID and ColorMap are the same image: fetched once, written twice
			entry(
				"Zone_10/8: Scorpion",
				"Body",
				1,
				&[
					(MapKind::Texture, 2),
					(MapKind::Color, 2),
					(MapKind::Normal, 4),
				],
			),
			// same mesh as above: must be downloaded only once
			entry("Eggs", "Body copy", 1, &[]),
			entry("Eggs", "Private", 3, &[]),
		]);

		let ctx = eframe::egui::Context::default();
		let mut downloader = Downloader::default();
		downloader.start(
			&ctx,
			dir.to_path_buf(),
			Arc::new(client(&api, AuthMethod::ApiKey)),
			ws.pending_jobs(),
			3,
		);
		let deadline = std::time::Instant::now() + Duration::from_secs(20);
		let mut events = Vec::new();
		while downloader.is_running() || events.len() < 4 {
			assert!(std::time::Instant::now() < deadline, "downloads timed out");
			events.extend(downloader.poll());
			std::thread::sleep(Duration::from_millis(20));
		}
		for event in events {
			let Event::Finished { job, result } = event else {
				panic!("auth failure")
			};
			ws.record_download(&job, result);
		}
		assert_eq!((downloader.completed, downloader.failed), (4, 1));

		let items = ws.items();
		assert_eq!(
			items[0].mesh,
			AssetState::Done {
				path: "Models/Zone_10/8_ Scorpion/Body.mesh".into()
			}
		);
		let map = |kind| items[0].maps[&kind].state.clone();
		assert_eq!(
			map(MapKind::Texture),
			AssetState::Done {
				path: "Models/Zone_10/8_ Scorpion/Body.png".into()
			}
		);
		assert_eq!(
			map(MapKind::Color),
			AssetState::Done {
				path: "Models/Zone_10/8_ Scorpion/Body.color.png".into()
			}
		);
		assert_eq!(
			map(MapKind::Normal),
			AssetState::Done {
				path: "Models/Zone_10/8_ Scorpion/Body.normal.png".into()
			}
		);
		assert!(items[0].is_done());
		assert_eq!(
			items[1].mesh,
			AssetState::Done {
				path: "Models/Eggs/Body copy.mesh".into()
			}
		);
		assert!(matches!(&items[2].mesh, AssetState::Failed { error } if error.contains("403")));
		assert_eq!(
			std::fs::read(dir.join("Models/Eggs/Body copy.mesh")).unwrap(),
			mesh
		);
		assert_eq!(
			std::fs::read(dir.join("Models/Zone_10/8_ Scorpion/Body.png")).unwrap(),
			png
		);
		// asset 1 was requested once even though two items use it
		let requests = api_log.lock().unwrap();
		assert_eq!(
			requests
				.iter()
				.filter(|r| r.starts_with("GET /oc/1 "))
				.count(),
			1
		);
		assert_eq!(
			requests
				.iter()
				.filter(|r| r.starts_with("GET /oc/2 "))
				.count(),
			1
		);
		drop(requests);
		// the viewer can open what was downloaded
		assert!(crate::scene::geometry::load(&mesh).is_ok());
	}
}
