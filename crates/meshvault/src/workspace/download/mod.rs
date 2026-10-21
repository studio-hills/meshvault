//! Downloading assets from Roblox into a workspace: a queue of jobs and
//! the worker threads that fetch them. The HTTP client and authentication
//! are in `client`; writing the files is in `workspace::store`.

mod client;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

pub use self::client::{AuthMethod, Client, Credentials, Endpoints, FetchError};
pub use crate::workspace::store::{Stored, sniff_extension};
use crate::workspace::{Job, store};

/// Most download threads at once. The settings allow fewer, but a
/// hand-edited config can ask for up to this many.
const MAX_WORKERS: usize = 16;
/// Download threads used until the settings say otherwise.
pub const DEFAULT_CONCURRENCY: usize = 4;
/// Most download threads the settings dialog offers, below the
/// downloader's own limit.
pub const MAX_CONCURRENCY_SETTING: usize = 8;

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
		let wanted = concurrency.clamp(1, MAX_WORKERS).min(queue.len());
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
		let result = bytes.and_then(|bytes| store::store(root, &job, &bytes));
		let _ = tx.send(Event::Finished { job, result });
		ctx.request_repaint();
	}
}

/// A local HTTP server and client for the client and pipeline tests.
#[cfg(test)]
mod test_server {
	use super::{AuthMethod, Client, Credentials, Endpoints};
	use std::io::{BufRead, BufReader, Write};
	use std::net::TcpListener;
	use std::sync::{Arc, Mutex};

	/// A minimal HTTP server: `routes` maps a request path to
	/// (status, extra headers, body). Records every request line and headers.
	/// (path, status, extra headers, body)
	pub(super) type Route = (&'static str, u16, Vec<(&'static str, String)>, Vec<u8>);

	pub(super) fn serve(routes: Vec<Route>) -> (String, Arc<Mutex<Vec<String>>>) {
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

	pub(super) fn client(base: &str, method: AuthMethod) -> Client {
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
}

#[cfg(test)]
mod tests {
	use super::test_server::{client, serve};
	use super::*;
	use crate::scene::maps::MapKind;
	use crate::test_support::{SAMPLE_MESHES, TempDir};
	use std::time::Duration;

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
