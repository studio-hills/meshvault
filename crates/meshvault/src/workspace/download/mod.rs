//! Downloading assets from Roblox into a workspace: a queue of jobs and
//! the worker threads that fetch them. The HTTP client and authentication
//! are in `client`; writing the files is in `workspace::store`.

mod client;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};

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

impl Shared {
	/// The job queue. Only short queue edits run under this lock, so it is
	/// poisoned only if one of them panicked part-way; the queue and the
	/// totals can then no longer be trusted, so this panics too.
	fn queue(&self) -> MutexGuard<'_, VecDeque<Job>> {
		self.queue
			.lock()
			.expect("download queue lock poisoned: a queue edit panicked")
	}
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

	/// Queues jobs; a job already queued gains the new targets. Starts
	/// workers up to `concurrency`.
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
		let mut queue = self.shared.queue();
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
			let mut queue = self.shared.queue();
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
		let Some(job) = shared.queue().pop_front() else {
			return;
		};
		let bytes = match store::read_cache(root, job.asset_id) {
			Some(bytes) => Ok(bytes),
			None => {
				match client.fetch_with_retry(job.asset_id, job.wants_image(), &shared.cancel) {
					Ok(bytes) => {
						store::write_cache(root, job.asset_id, &bytes);
						Ok(bytes)
					}
					Err(FetchError::Auth(message)) => {
						shared.cancel.store(true, Ordering::SeqCst);
						shared.queue().clear();
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
	use std::collections::HashMap;
	use std::io::{BufRead, BufReader, Write};
	use std::net::{TcpListener, TcpStream};
	use std::sync::atomic::{AtomicBool, Ordering};
	use std::sync::mpsc::{Receiver, Sender, channel};
	use std::sync::{Arc, Mutex};
	use std::thread::JoinHandle;
	use std::time::Duration;

	/// One canned reply: (request path, status, extra headers, body).
	pub(crate) type Route = (&'static str, u16, Vec<(&'static str, String)>, Vec<u8>);

	/// How long the server and the tests wait for something that should
	/// happen before giving up. Only a watchdog: the tests wait on events.
	pub(crate) const WATCHDOG: Duration = Duration::from_secs(20);

	/// The server side of a [`Gate`].
	struct Held {
		arrived: Sender<()>,
		release: Mutex<Receiver<()>>,
	}

	type Gates = Arc<Mutex<HashMap<&'static str, Arc<Held>>>>;

	/// Holds every request for one path until the test lets it through, so
	/// a test can pause a download at a known point.
	pub(crate) struct Gate {
		arrived: Receiver<()>,
		release: Sender<()>,
	}

	impl Gate {
		/// Waits until a request for the path is being held.
		pub(crate) fn wait_for_request(&self) {
			self.arrived
				.recv_timeout(WATCHDOG)
				.expect("no request reached the gate");
		}

		/// Lets one held request (or the next one to arrive) through.
		pub(crate) fn release(&self) {
			let _ = self.release.send(());
		}
	}

	/// A minimal HTTP server answering from a list of [`Route`]s (404 for
	/// any other path), one thread per connection. It records each
	/// request's line and headers in `log`.
	///
	/// Dropping it stops the server and waits for its threads. Dropping a
	/// [`Gate`] lets its held requests through, so a failing test still ends.
	pub(crate) struct Server {
		pub(crate) base: String,
		pub(crate) log: Arc<Mutex<Vec<String>>>,
		gates: Gates,
		stop: Arc<AtomicBool>,
		thread: Option<JoinHandle<()>>,
	}

	impl Server {
		/// Holds requests for `path` from now on; see [`Gate`].
		pub(crate) fn gate(&self, path: &'static str) -> Gate {
			let (arrived_tx, arrived) = channel();
			let (release, release_rx) = channel();
			let held = Held {
				arrived: arrived_tx,
				release: Mutex::new(release_rx),
			};
			self.gates.lock().unwrap().insert(path, Arc::new(held));
			Gate { arrived, release }
		}

		/// How many requests began with `prefix`, such as `"GET /oc/1 "`.
		pub(crate) fn count(&self, prefix: &str) -> usize {
			let log = self.log.lock().unwrap();
			log.iter().filter(|r| r.starts_with(prefix)).count()
		}
	}

	impl Drop for Server {
		fn drop(&mut self) {
			self.stop.store(true, Ordering::SeqCst);
			// wake the accept loop so it sees the flag
			if let Some(addr) = self.base.strip_prefix("http://") {
				let _ = TcpStream::connect(addr);
			}
			if let Some(thread) = self.thread.take() {
				let _ = thread.join();
			}
		}
	}

	pub(crate) fn serve(routes: Vec<Route>) -> Server {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let log = Arc::new(Mutex::new(Vec::new()));
		let gates = Gates::default();
		let stop = Arc::new(AtomicBool::new(false));
		let routes = Arc::new(routes);
		let thread = {
			let (log, gates, stop) = (log.clone(), gates.clone(), stop.clone());
			std::thread::spawn(move || {
				let mut connections = Vec::new();
				for stream in listener.incoming() {
					if stop.load(Ordering::SeqCst) {
						break;
					}
					let Ok(stream) = stream else { continue };
					let (routes, log, gates) = (routes.clone(), log.clone(), gates.clone());
					connections.push(std::thread::spawn(move || {
						answer(stream, &routes, &log, &gates);
					}));
				}
				for connection in connections {
					let _ = connection.join();
				}
			})
		};
		Server {
			base,
			log,
			gates,
			stop,
			thread: Some(thread),
		}
	}

	/// Reads one request, holds it at its path's gate if there is one, and
	/// sends the matching route's reply.
	fn answer(mut stream: TcpStream, routes: &[Route], log: &Mutex<Vec<String>>, gates: &Gates) {
		let _ = stream.set_read_timeout(Some(WATCHDOG));
		let Ok(read_half) = stream.try_clone() else {
			return;
		};
		let mut reader = BufReader::new(read_half);
		let mut request = String::new();
		loop {
			let mut line = String::new();
			match reader.read_line(&mut line) {
				Ok(0) | Err(_) => break,
				Ok(_) if line == "\r\n" => break,
				Ok(_) => request.push_str(&line),
			}
		}
		// the connection that wakes a stopping server sends nothing
		if request.is_empty() {
			return;
		}
		let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
		log.lock().unwrap().push(request);
		let held = gates.lock().unwrap().get(path.as_str()).cloned();
		if let Some(held) = held {
			let _ = held.arrived.send(());
			let _ = held.release.lock().unwrap().recv_timeout(WATCHDOG);
		}
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

	pub(crate) fn client(base: &str, method: AuthMethod) -> Client {
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

	mod tests {
		use super::*;
		use std::io::Read;

		fn get(base: &str, path: &str) -> String {
			let mut stream = TcpStream::connect(base.strip_prefix("http://").unwrap()).unwrap();
			write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
			let mut reply = String::new();
			stream.read_to_string(&mut reply).unwrap();
			reply
		}

		/// A gated request is answered only once the test lets it through,
		/// while other paths are answered meanwhile.
		#[test]
		fn a_gate_holds_its_requests_until_released() {
			let server = serve(vec![("/a", 200, vec![], b"A".to_vec())]);
			let gate = server.gate("/a");
			let base = server.base.clone();
			let held = std::thread::spawn(move || get(&base, "/a"));
			gate.wait_for_request();
			assert!(get(&server.base, "/b").starts_with("HTTP/1.1 404"));
			assert!(!held.is_finished());
			gate.release();
			assert!(held.join().unwrap().ends_with("\r\n\r\nA"));
			assert_eq!(server.count("GET /a "), 1);
			assert_eq!(server.count("GET "), 2);
		}

		/// Dropping the server ends its thread, so the port stops answering.
		#[test]
		fn dropping_the_server_stops_it() {
			let server = serve(vec![]);
			let addr = server.base.strip_prefix("http://").unwrap().to_string();
			drop(server);
			assert!(TcpStream::connect(addr).is_err());
		}
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
		let cdn = serve(vec![
			("/cdn/1", 200, vec![], mesh.clone()),
			("/cdn/2", 200, vec![], png.clone()),
			("/cdn/4", 200, vec![], png.clone()),
		]);
		let api = serve(vec![
			(
				"/oc/1",
				200,
				vec![],
				format!(r#"{{"location":"{}/cdn/1"}}"#, cdn.base).into_bytes(),
			),
			(
				"/oc/2",
				200,
				vec![],
				format!(r#"{{"location":"{}/cdn/2"}}"#, cdn.base).into_bytes(),
			),
			("/oc/3", 403, vec![], vec![]),
			(
				"/oc/4",
				200,
				vec![],
				format!(r#"{{"location":"{}/cdn/4"}}"#, cdn.base).into_bytes(),
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
			Arc::new(client(&api.base, AuthMethod::ApiKey)),
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
		let requests = api.log.lock().unwrap();
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

	/// Cancelling drops the queued jobs from the total, so the progress
	/// counts only what was started; the jobs already done stay counted.
	#[test]
	fn cancel_takes_the_queued_jobs_off_the_total() {
		let mut downloader = Downloader::default();
		let job = |asset_id| Job {
			asset_id,
			targets: vec![(
				format!("Models/{asset_id}"),
				crate::workspace::AssetKind::Mesh,
			)],
		};
		downloader.shared.queue().extend([job(1), job(2), job(3)]);
		downloader.total = 5;
		downloader.completed = 2;
		downloader.cancel();
		assert_eq!(downloader.total, 2);
		assert_eq!(downloader.completed, 2);
		assert!(downloader.shared.queue().is_empty());
		assert!(downloader.shared.cancel.load(Ordering::SeqCst));
		// cancelling again with nothing queued changes nothing
		downloader.cancel();
		assert_eq!(downloader.total, 2);
	}

	/// A job writing asset `asset_id` as the mesh of `stem`.
	fn mesh_job(asset_id: u64, stem: &str) -> Job {
		Job {
			asset_id,
			targets: vec![(stem.into(), crate::workspace::AssetKind::Mesh)],
		}
	}

	/// The Open Cloud API and the CDN for some (asset id, content) pairs.
	/// The API's requests for asset `id` are to `/oc/{id}`.
	struct Assets {
		api: test_server::Server,
		_cdn: test_server::Server,
	}

	impl Assets {
		fn new(assets: &[(u64, &[u8])]) -> Self {
			let leak = |s: String| -> &'static str { s.leak() };
			let cdn = serve(
				assets
					.iter()
					.map(|&(id, content)| {
						(leak(format!("/cdn/{id}")), 200, vec![], content.to_vec())
					})
					.collect(),
			);
			let api = serve(
				assets
					.iter()
					.map(|&(id, _)| {
						let location = format!(r#"{{"location":"{}/cdn/{id}"}}"#, cdn.base);
						(
							leak(format!("/oc/{id}")),
							200,
							vec![],
							location.into_bytes(),
						)
					})
					.collect(),
			);
			Self { api, _cdn: cdn }
		}

		fn client(&self) -> Arc<Client> {
			Arc::new(client(&self.api.base, AuthMethod::ApiKey))
		}

		/// How many times the API was asked for asset `id`.
		fn requests(&self, id: u64) -> usize {
			self.api.count(&format!("GET /oc/{id} "))
		}
	}

	/// Polls until the downloader has stopped, returning every event.
	fn run_to_end(downloader: &mut Downloader) -> Vec<Event> {
		let deadline = std::time::Instant::now() + test_server::WATCHDOG;
		let mut events = Vec::new();
		loop {
			events.extend(downloader.poll());
			if !downloader.is_running() {
				events.extend(downloader.poll());
				return events;
			}
			assert!(std::time::Instant::now() < deadline, "downloads timed out");
			std::thread::sleep(Duration::from_millis(5));
		}
	}

	/// An empty cache file, as a write cut short leaves it, is not taken
	/// for the asset: it is downloaded again.
	#[test]
	fn an_empty_cache_file_is_downloaded_again() {
		let dir = TempDir::new("download_empty_cache");
		let cache = crate::workspace::cache_path(&dir, 9);
		std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
		std::fs::write(&cache, b"").unwrap();
		let assets = Assets::new(&[(9, b"version 4.00\nNINE")]);
		let ctx = eframe::egui::Context::default();
		let mut downloader = Downloader::default();
		let jobs = vec![mesh_job(9, "Models/Nine")];
		downloader.start(&ctx, dir.to_path_buf(), assets.client(), jobs, 1);
		let events = run_to_end(&mut downloader);
		assert_eq!(assets.requests(9), 1);
		assert!(matches!(
			&events[..],
			[Event::Finished { result: Ok(_), .. }]
		));
		let written = std::fs::read(dir.join("Models/Nine.mesh")).unwrap();
		assert_eq!(written, b"version 4.00\nNINE");
		assert_eq!(std::fs::read(&cache).unwrap(), b"version 4.00\nNINE");
	}

	/// The cache is written whole under a temporary name, then renamed, so
	/// it never holds part of an asset and no temporary file is left.
	#[test]
	fn the_cache_is_written_whole_without_leftovers() {
		let dir = TempDir::new("download_cache_whole");
		let assets = Assets::new(&[(8, b"version 4.00\nEIGHT")]);
		let ctx = eframe::egui::Context::default();
		let mut downloader = Downloader::default();
		let jobs = vec![mesh_job(8, "Models/Eight")];
		downloader.start(&ctx, dir.to_path_buf(), assets.client(), jobs, 1);
		run_to_end(&mut downloader);
		let cache_dir = dir.join(crate::workspace::CACHE_DIR);
		let names: Vec<_> = std::fs::read_dir(&cache_dir)
			.unwrap()
			.map(|e| e.unwrap().file_name())
			.collect();
		assert_eq!(names, ["8"]);
		let cached = std::fs::read(crate::workspace::cache_path(&dir, 8)).unwrap();
		assert_eq!(cached, b"version 4.00\nEIGHT");
	}
}
