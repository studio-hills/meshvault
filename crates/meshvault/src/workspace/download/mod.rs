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

/// What a worker sends: an event, and the run it came from.
struct Message {
	run: Arc<Run>,
	event: Event,
}

/// One run of downloads: the workspace and credential it downloads with,
/// its queue and its workers. A new run starts when downloads start after
/// the last one stopped or was cancelled, or for another workspace or
/// credential, so workers still finishing an old run never take a new
/// run's jobs, and never leave them stranded.
struct Run {
	root: PathBuf,
	client: Arc<Client>,
	queue: Mutex<VecDeque<Job>>,
	cancel: AtomicBool,
	/// Workers that have not yet decided to stop. Only changed with the
	/// queue locked, so a worker that finds the queue empty stops counting
	/// at once, and `start` never counts on a worker that is stopping.
	workers: AtomicUsize,
}

/// Points in the workers a test can act at.
#[cfg(test)]
#[derive(Default)]
struct Hooks {
	/// Called once, by the first worker to stop, after it has decided to
	/// stop and before it is gone, so a test can act in that gap.
	on_stop: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Run {
	fn new(root: PathBuf, client: Arc<Client>) -> Self {
		Self {
			root,
			client,
			queue: Mutex::default(),
			cancel: AtomicBool::new(false),
			workers: AtomicUsize::new(0),
		}
	}

	/// The job queue. Only short queue edits run under this lock, so it is
	/// poisoned only if one of them panicked part-way; the queue and the
	/// totals can then no longer be trusted, so this panics too.
	fn queue(&self) -> MutexGuard<'_, VecDeque<Job>> {
		self.queue
			.lock()
			.expect("download queue lock poisoned: a queue edit panicked")
	}

	/// Whether the run is over or ending: no new job would be picked up.
	fn stopped(&self) -> bool {
		self.cancel.load(Ordering::SeqCst) || self.workers.load(Ordering::SeqCst) == 0
	}
}

pub struct Downloader {
	/// The latest run; its events are the ones counted.
	run: Option<Arc<Run>>,
	tx: Sender<Message>,
	rx: Receiver<Message>,
	pub total: usize,
	pub completed: usize,
	pub failed: usize,
	#[cfg(test)]
	hooks: Arc<Hooks>,
}

impl Default for Downloader {
	fn default() -> Self {
		let (tx, rx) = channel();
		Self {
			run: None,
			tx,
			rx,
			total: 0,
			completed: 0,
			failed: 0,
			#[cfg(test)]
			hooks: Arc::default(),
		}
	}
}

impl Downloader {
	pub fn is_running(&self) -> bool {
		self.run
			.as_ref()
			.is_some_and(|run| run.workers.load(Ordering::SeqCst) > 0)
	}

	/// Makes a new run the latest, with its counts at zero.
	fn new_run(&mut self, root: PathBuf, client: Arc<Client>) -> Arc<Run> {
		let run = Arc::new(Run::new(root, client));
		self.run = Some(run.clone());
		self.total = 0;
		self.completed = 0;
		self.failed = 0;
		run
	}

	/// Queues jobs for the workspace at `root`; a job already queued gains
	/// the new targets. Starts workers up to `concurrency`. Starts a new
	/// run if the last one has stopped or was cancelled, or was for another
	/// workspace or credential (which is cancelled).
	pub fn start(
		&mut self,
		ctx: &eframe::egui::Context,
		root: PathBuf,
		client: Arc<Client>,
		jobs: Vec<Job>,
		concurrency: usize,
	) {
		let same = |run: &Run| run.root == root && run.client.same_as(&client);
		if self.run.as_deref().is_some_and(|run| !same(run)) {
			self.cancel();
		}
		let (mut run, fresh) = match self.run.clone() {
			Some(run) => (run, false),
			None => (self.new_run(root.clone(), client.clone()), true),
		};
		let mut queue = run.queue();
		// with the queue locked, a run seen live stays live: a run only
		// ends with its queue locked
		if !fresh && run.stopped() {
			drop(queue);
			run = self.new_run(root, client);
			queue = run.queue();
		}
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
		while run.workers.load(Ordering::SeqCst) < wanted {
			run.workers.fetch_add(1, Ordering::SeqCst);
			let run = run.clone();
			let tx = self.tx.clone();
			let ctx = ctx.clone();
			#[cfg(test)]
			let hooks = self.hooks.clone();
			std::thread::spawn(move || {
				worker(&run, &tx, &ctx);
				#[cfg(test)]
				if let Some(on_stop) = hooks.on_stop.lock().unwrap().take() {
					on_stop();
				}
				ctx.request_repaint();
			});
		}
		drop(queue);
	}

	/// Stops after the downloads in flight; queued jobs stay pending.
	pub fn cancel(&mut self) {
		let Some(run) = &self.run else { return };
		let dropped = {
			let mut queue = run.queue();
			run.cancel.store(true, Ordering::SeqCst);
			let n = queue.len();
			queue.clear();
			n
		};
		self.total -= dropped;
	}

	/// The events since the last poll for the workspace at `root` (none
	/// when no workspace is open). Events for any other workspace are
	/// dropped, and only the latest run's are counted or report a
	/// credential failure.
	pub fn poll(&mut self, root: Option<&Path>) -> Vec<Event> {
		let mut events = Vec::new();
		for Message { run, event } in self.rx.try_iter() {
			if root != Some(run.root.as_path()) {
				continue;
			}
			let latest = self.run.as_ref().is_some_and(|r| Arc::ptr_eq(r, &run));
			match &event {
				Event::Finished { result, .. } if latest => {
					self.completed += 1;
					if result.is_err() {
						self.failed += 1;
					}
				}
				Event::AuthFailed(_) if !latest => continue,
				_ => {}
			}
			events.push(event);
		}
		events
	}
}

fn worker(run: &Arc<Run>, tx: &Sender<Message>, ctx: &eframe::egui::Context) {
	let send = |event| {
		let _ = tx.send(Message {
			run: run.clone(),
			event,
		});
		ctx.request_repaint();
	};
	let root = &run.root;
	loop {
		// deciding to stop and no longer counting happen under one lock
		let job = {
			let mut queue = run.queue();
			let next = if run.cancel.load(Ordering::SeqCst) {
				None
			} else {
				queue.pop_front()
			};
			match next {
				Some(job) => job,
				None => {
					run.workers.fetch_sub(1, Ordering::SeqCst);
					return;
				}
			}
		};
		let bytes = match store::read_cache(root, job.asset_id) {
			Some(bytes) => Ok(bytes),
			None => {
				match run
					.client
					.fetch_with_retry(job.asset_id, job.wants_image(), &run.cancel)
				{
					Ok(bytes) => {
						store::write_cache(root, job.asset_id, &bytes);
						Ok(bytes)
					}
					Err(FetchError::Auth(message)) => {
						{
							let mut queue = run.queue();
							run.cancel.store(true, Ordering::SeqCst);
							queue.clear();
						}
						send(Event::AuthFailed(message));
						continue;
					}
					// cancelled mid-retry: leave the item pending
					Err(FetchError::Transient(..)) if run.cancel.load(Ordering::SeqCst) => {
						continue;
					}
					Err(e) => Err(e.to_string()),
				}
			}
		};
		let result = bytes.and_then(|bytes| store::store(root, &job, &bytes));
		send(Event::Finished { job, result });
	}
}

// A local HTTP server and client for the client and pipeline tests.
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
			events.extend(downloader.poll(Some(&dir)));
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
		let run = Arc::new(Run::new(
			PathBuf::new(),
			Arc::new(client("http://0.0.0.0:9", AuthMethod::ApiKey)),
		));
		downloader.run = Some(run.clone());
		run.queue().extend([job(1), job(2), job(3)]);
		downloader.total = 5;
		downloader.completed = 2;
		downloader.cancel();
		assert_eq!(downloader.total, 2);
		assert_eq!(downloader.completed, 2);
		assert!(run.queue().is_empty());
		assert!(run.cancel.load(Ordering::SeqCst));
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
	fn run_to_end(downloader: &mut Downloader, root: &Path) -> Vec<Event> {
		let deadline = std::time::Instant::now() + test_server::WATCHDOG;
		let mut events = Vec::new();
		loop {
			events.extend(downloader.poll(Some(root)));
			if !downloader.is_running() {
				events.extend(downloader.poll(Some(root)));
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
		let events = run_to_end(&mut downloader, &dir);
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
		run_to_end(&mut downloader, &dir);
		let cache_dir = dir.join(crate::workspace::CACHE_DIR);
		let names: Vec<_> = std::fs::read_dir(&cache_dir)
			.unwrap()
			.map(|e| e.unwrap().file_name())
			.collect();
		assert_eq!(names, ["8"]);
		let cached = std::fs::read(crate::workspace::cache_path(&dir, 8)).unwrap();
		assert_eq!(cached, b"version 4.00\nEIGHT");
	}

	/// Makes the first worker to stop wait, once it has decided to stop,
	/// until the returned sender sends (or is dropped). The receiver hears
	/// when it gets there.
	fn pause_first_stop(downloader: &Downloader) -> (Receiver<()>, Sender<()>) {
		let (stopping_tx, stopping) = channel();
		let (go, go_rx) = channel::<()>();
		*downloader.hooks.on_stop.lock().unwrap() = Some(Box::new(move || {
			let _ = stopping_tx.send(());
			let _ = go_rx.recv_timeout(test_server::WATCHDOG);
		}));
		(stopping, go)
	}

	/// A job queued just as the last worker finds the queue empty and
	/// stops is still downloaded: the worker no longer counts as running
	/// once it has decided to stop, so a new one is started (B04).
	#[test]
	fn a_job_queued_as_the_last_worker_stops_is_downloaded() {
		let dir = TempDir::new("download_stop_gap");
		let assets = Assets::new(&[(1, b"version 4.00\nONE"), (2, b"version 4.00\nTWO")]);
		let ctx = eframe::egui::Context::default();
		let mut downloader = Downloader::default();
		let (stopping, go) = pause_first_stop(&downloader);
		let root = dir.to_path_buf();
		downloader.start(
			&ctx,
			root.clone(),
			assets.client(),
			vec![mesh_job(1, "Models/One")],
			1,
		);
		stopping
			.recv_timeout(test_server::WATCHDOG)
			.expect("the worker never stopped");
		downloader.start(
			&ctx,
			root,
			assets.client(),
			vec![mesh_job(2, "Models/Two")],
			1,
		);
		drop(go);
		run_to_end(&mut downloader, &dir);
		assert_eq!(assets.requests(2), 1, "job 2 was stranded in the queue");
		assert!(dir.join("Models/Two.mesh").is_file());
	}

	/// Downloads started while cancelled workers are still finishing their
	/// last job run on new workers, and the total counts only them (B03).
	#[test]
	fn a_start_after_cancel_runs_while_cancelled_workers_finish() {
		let dir = TempDir::new("download_cancel_restart");
		let assets = Assets::new(&[(1, b"version 4.00\nONE"), (2, b"version 4.00\nTWO")]);
		let gate = assets.api.gate("/oc/1");
		let ctx = eframe::egui::Context::default();
		let mut downloader = Downloader::default();
		let root = dir.to_path_buf();
		downloader.start(
			&ctx,
			root.clone(),
			assets.client(),
			vec![mesh_job(1, "Models/One")],
			1,
		);
		gate.wait_for_request();
		downloader.cancel();
		downloader.start(
			&ctx,
			root,
			assets.client(),
			vec![mesh_job(2, "Models/Two")],
			1,
		);
		assert_eq!(downloader.total, 1);
		gate.release();
		run_to_end(&mut downloader, &dir);
		assert_eq!(assets.requests(2), 1, "job 2 was stranded in the queue");
		assert!(dir.join("Models/Two.mesh").is_file());
	}

	/// Jobs for another workspace go to new workers, not to the old
	/// workspace's workers, which would write them under the old folder
	/// (B01).
	#[test]
	fn old_workers_never_take_another_workspaces_jobs() {
		let (old, new) = (
			TempDir::new("download_old_ws"),
			TempDir::new("download_new_ws"),
		);
		let assets = Assets::new(&[(1, b"version 4.00\nONE"), (2, b"version 4.00\nTWO")]);
		let gate = assets.api.gate("/oc/1");
		let ctx = eframe::egui::Context::default();
		let mut downloader = Downloader::default();
		let jobs = vec![mesh_job(1, "Models/One")];
		downloader.start(&ctx, old.to_path_buf(), assets.client(), jobs, 1);
		gate.wait_for_request();
		let jobs = vec![mesh_job(2, "Models/Two")];
		downloader.start(&ctx, new.to_path_buf(), assets.client(), jobs, 1);
		gate.release();
		let events = run_to_end(&mut downloader, &new);
		assert!(new.join("Models/Two.mesh").is_file());
		assert!(!old.join("Models/Two.mesh").exists());
		// the old workspace's job is not reported to the new one
		assert!(
			events
				.iter()
				.all(|e| matches!(e, Event::Finished { job, .. } if job.asset_id == 2))
		);
	}

	/// Jobs queued with a new credential are fetched with it, not by
	/// workers still holding the old one (B01).
	#[test]
	fn jobs_queued_with_a_new_credential_use_it() {
		let dir = TempDir::new("download_new_credential");
		let assets = Assets::new(&[(1, b"version 4.00\nONE"), (2, b"version 4.00\nTWO")]);
		let gate = assets.api.gate("/oc/1");
		let ctx = eframe::egui::Context::default();
		let mut downloader = Downloader::default();
		let root = dir.to_path_buf();
		downloader.start(
			&ctx,
			root.clone(),
			assets.client(),
			vec![mesh_job(1, "Models/One")],
			1,
		);
		gate.wait_for_request();
		let other = Client::new(
			Credentials {
				method: AuthMethod::ApiKey,
				secret: "OTHER".into(),
				place_id: Some(77),
			},
			Endpoints {
				open_cloud: format!("{}/oc/", assets.api.base),
				asset_delivery: format!("{}/ad?id=", assets.api.base),
			},
		);
		assert!(!other.same_as(&assets.client()));
		downloader.start(
			&ctx,
			root,
			Arc::new(other),
			vec![mesh_job(2, "Models/Two")],
			1,
		);
		gate.release();
		run_to_end(&mut downloader, &dir);
		let log = assets.api.log.lock().unwrap();
		let two: Vec<_> = log.iter().filter(|r| r.starts_with("GET /oc/2 ")).collect();
		assert_eq!(two.len(), 1);
		assert!(two[0].to_lowercase().contains("x-api-key: other"));
	}

	/// Only events for the open workspace are returned, and only the latest
	/// run's are counted or report a rejected credential (B01).
	#[test]
	fn poll_keeps_to_the_open_workspace_and_counts_the_latest_run() {
		let (a, b) = (PathBuf::from("A"), PathBuf::from("B"));
		let client = Arc::new(client("http://0.0.0.0:9", AuthMethod::ApiKey));
		let mut downloader = Downloader::default();
		let old = Arc::new(Run::new(b.clone(), client.clone()));
		let other = Arc::new(Run::new(a.clone(), client.clone()));
		let latest = downloader.new_run(b.clone(), client);
		let tx = downloader.tx.clone();
		let send = |run: &Arc<Run>, event| {
			let run = run.clone();
			tx.send(Message { run, event }).unwrap();
		};
		let finished = |id| Event::Finished {
			job: mesh_job(id, "Models/X"),
			result: Ok(Vec::new()),
		};
		send(&other, finished(1));
		send(&other, Event::AuthFailed("A".into()));
		send(&old, finished(2));
		send(&old, Event::AuthFailed("old".into()));
		send(&latest, finished(3));
		send(&latest, Event::AuthFailed("latest".into()));
		let events = downloader.poll(Some(&b));
		let seen: Vec<String> = events
			.iter()
			.map(|e| match e {
				Event::Finished { job, .. } => format!("finished {}", job.asset_id),
				Event::AuthFailed(m) => format!("auth {m}"),
			})
			.collect();
		assert_eq!(seen, ["finished 2", "finished 3", "auth latest"]);
		assert_eq!((downloader.completed, downloader.failed), (1, 0));
		// with no workspace open, nothing is returned
		send(&latest, finished(4));
		assert!(downloader.poll(None).is_empty());
	}
}
