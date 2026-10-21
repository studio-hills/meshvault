//! The link between Roblox Studio and Meshvault, over `localhost:34875`.
//!
//! - `GET /export.luau[?schema=N]` serves the exporter (the newest schema
//!   unless asked otherwise), so it never has to be kept or pasted by hand.
//!   [`studio_code`] is the Command Bar code that fetches and runs it.
//! - `POST /export` receives the export in pieces, each with these headers:
//!   `X-Meshvault-Export` (an id shared by all pieces of one export),
//!   `X-Meshvault-Chunk` (`i/n`, 1-based) and optionally `X-Meshvault-Name`
//!   (a name for the saved file). Once all `n` pieces have arrived they are
//!   joined in order and handed to the app.
//!
//! What is checked: the server binds the IPv4 loopback address
//! (127.0.0.1), so only programs on this machine can connect, and pieces
//! must carry the custom headers above. A web page on another origin can
//! only send those after a CORS preflight, which is never approved (OPTIONS
//! gets a 404 without CORS headers). Every request must also say it is for
//! `localhost` or `127.0.0.1` (its `Host` header, with the link's port or
//! none), so a page that reaches the port under its own name (DNS
//! rebinding) is refused. There is no authentication: any program on this machine can
//! still send an export.
//!
//! Requests are served one at a time. Each has a capped head (request line
//! and headers) and body, and deadlines for arriving and for its reply, so
//! a slow or oversized request is dropped rather than holding the link.
//! What is kept for the app is bounded too: a few exports may arrive at
//! once, one that stops part-way is forgotten after a while, and pieces
//! still arriving plus received exports the app has not taken share one
//! byte limit. Only received exports wait in the app's bounded queue;
//! progress is kept as the latest count of each export, so however many
//! pieces arrive while the app is busy, they never crowd out an export.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eframe::egui;

pub const PORT: u16 = 34875;
/// Largest single piece accepted; the exporter sends 2 MB.
const MAX_CHUNK: usize = 64 * 1024 * 1024;
/// Largest whole export accepted.
const MAX_EXPORT: usize = 2 * 1024 * 1024 * 1024;
/// Most pieces one export may say it has, so a bad count cannot make a
/// huge list of pieces.
const MAX_PIECES: usize = 100_000;
/// Most exports that may be arriving at once.
const MAX_PENDING: usize = 8;
/// Most bytes held for the app at once: pieces of exports still arriving,
/// and received exports the app has not taken yet.
const MAX_HELD: usize = MAX_EXPORT;
/// Most received exports waiting for the app to take them, and most
/// exports whose progress is waiting.
const MAX_EVENTS: usize = 64;
/// How long an export may go without a new piece before it is forgotten,
/// as one whose sender stopped part-way.
const PENDING_TIMEOUT: Duration = Duration::from_secs(120);
/// How often the server wakes, while exports are arriving, to forget the
/// ones that have stopped.
const EXPIRY_POLL: Duration = Duration::from_millis(10);
/// How long a whole request may take to arrive before the connection is
/// dropped, so a sender that stalls or trickles does not hold the link.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the reply may take to send, so a sender that stops reading
/// does not hold the link.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Most bytes in a request's head (its request line and headers together).
/// Studio's are well under 1 KB.
const MAX_HEAD: usize = 32 * 1024;
/// Most characters kept of an export's name (`X-Meshvault-Name`), which
/// names the saved file and is sent back in the reply.
const MAX_NAME: usize = 100;
/// The folder (in the workspaces folder) that received exports go to.
const EXPORTS_FOLDER: &str = "Exports";
/// What the Studio code says when it cannot fetch the exporter, in two
/// halves so the formatted layout can break the line between them.
const UNREACHABLE: [&str; 2] = [
	"Could not reach Meshvault (is it open, and is Studio in ",
	"Edit mode, not Play?): ",
];

/// The exporters served, by schema version. The newest is the default.
const EXPORTERS: [(u32, &str); 2] = [
	(1, include_str!("../../studio/exporter_v1.luau")),
	(2, include_str!("../../studio/exporter_v2.luau")),
];
/// The schema served by default: the one this build writes.
pub const LATEST_SCHEMA: u32 = super::export_file::SCHEMA_VERSION;
/// The schemas there are exporters for, oldest first.
pub const SCHEMAS: [u32; EXPORTERS.len()] = {
	let mut schemas = [0; EXPORTERS.len()];
	let mut i = 0;
	while i < EXPORTERS.len() {
		schemas[i] = EXPORTERS[i].0;
		i += 1;
	}
	schemas
};
// raising SCHEMA_VERSION needs an exporter for it
const _: () = assert!(EXPORTERS[EXPORTERS.len() - 1].0 == LATEST_SCHEMA);

/// The exporter for a schema version, ready to run: its send address and
/// Meshvault's version (for bug reports) filled in.
pub fn exporter(schema: u32) -> Option<String> {
	let source = EXPORTERS.iter().find(|(v, _)| *v == schema)?.1;
	Some(
		source
			.replace(
				"__MESHVAULT_URL__",
				&format!("http://localhost:{PORT}/export"),
			)
			.replace("__MESHVAULT_VERSION__", env!("CARGO_PKG_VERSION")),
	)
}

/// What the code for Studio does with the export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeMode {
	/// Exports and sends it to Meshvault.
	Send,
	/// Exports without sending, so the export can be changed, then sent
	/// with `send(export)` or saved some other way.
	Edit,
}

/// How the code for Studio is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeLayout {
	/// One line, for the Command Bar.
	OneLine,
	/// One statement a line, with readable names and comments.
	Formatted,
}

/// The code to paste into Studio's Command Bar (or a plugin): it fetches the
/// exporter for `schema` from Meshvault and runs it. `game:HttpGet` is not
/// available everywhere, so it uses HttpService with HttpEnabled switched on
/// just for the fetch.
pub fn studio_code(schema: u32, mode: CodeMode, layout: CodeLayout) -> String {
	let query = if schema == LATEST_SCHEMA {
		String::new()
	} else {
		format!("?schema={schema}")
	};
	let url = format!("http://localhost:{PORT}/export.luau{query}");
	let [unreachable_start, unreachable_end] = UNREACHABLE;
	match layout {
		CodeLayout::OneLine => {
			let fetch = format!(
				"local h=game:GetService(\"HttpService\") local e=h.HttpEnabled h.HttpEnabled=true \
				 local ok,s=pcall(h.GetAsync,h,\"{url}\") h.HttpEnabled=e \
				 assert(ok,\"{unreachable_start}{unreachable_end}\"..tostring(s))"
			);
			match mode {
				CodeMode::Send => format!("{fetch} loadstring(s)()()"),
				CodeMode::Edit => format!(
					"{fetch} local issues,export,send=loadstring(s)()({{send=false}}) send(export)"
				),
			}
		}
		CodeLayout::Formatted => {
			// no line longer than the dialog shows
			let fetch = format!(
				"local http = game:GetService(\"HttpService\")\n\
				 local url = \"{url}\"\n\
				 local enabled = http.HttpEnabled\n\
				 http.HttpEnabled = true\n\
				 local ok, source = pcall(http.GetAsync, http, url)\n\
				 http.HttpEnabled = enabled\n\
				 assert(ok, \"{unreachable_start}\"\n\
				 \t.. \"{unreachable_end}\" .. tostring(source))\n\
				 \n"
			);
			match mode {
				CodeMode::Send => format!("{fetch}loadstring(source)()()"),
				CodeMode::Edit => format!(
					"{fetch}local issues, export, send = loadstring(source)()({{ send = false }})\n\
					 \n\
					 -- change the export here, e.g. keep only the first model:\n\
					 -- export.Models = {{ export.Models[1] }}\n\
					 \n\
					 send(export) -- or save http:JSONEncode(export) your own way"
				),
			}
		}
	}
}

/// A complete export received from Studio.
pub struct Received {
	/// The id its pieces came with, as in [`Event::Progress`].
	pub export: String,
	pub name: String,
	pub json: Vec<u8>,
	/// Where the app is to save it: a new file.
	pub path: PathBuf,
}

impl Received {
	/// Saves the export to `path`, never over an existing file.
	pub fn save(&self) -> std::io::Result<()> {
		if let Some(folder) = self.path.parent() {
			std::fs::create_dir_all(folder)?;
		}
		std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(&self.path)?
			.write_all(&self.json)
	}
}

pub enum Event {
	/// A piece of the export `export` (its id) arrived: it has `done` of
	/// `count`.
	Progress {
		export: String,
		done: usize,
		count: usize,
	},
	Received(Received),
}

pub enum Status {
	Listening,
	/// Could not listen, usually because the port is taken: another
	/// Meshvault, or another program.
	Unavailable(String),
}

pub struct StudioLink {
	pub status: Status,
	rx: Receiver<Event>,
	/// Bytes of received exports waiting in `rx` (see [`Exports::queued`]).
	queued: Arc<AtomicUsize>,
	/// See [`Exports::progress`].
	progress: Arc<Mutex<Vec<Progress>>>,
}

impl StudioLink {
	pub fn start(ctx: &egui::Context) -> Self {
		let (tx, rx) = mpsc::sync_channel(MAX_EVENTS);
		let exports =
			Exports::new(crate::config::Config::default_workspaces_dir().join(EXPORTS_FOLDER));
		let queued = exports.queued.clone();
		let progress = exports.progress.clone();
		let status = match TcpListener::bind((Ipv4Addr::LOCALHOST, PORT)) {
			Ok(listener) => {
				let ctx = ctx.clone();
				std::thread::spawn(move || serve(listener, tx, exports, ctx));
				Status::Listening
			}
			Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Status::Unavailable(format!(
				"port {PORT} is already in use, probably by another Meshvault. Close it and restart Meshvault"
			)),
			Err(e) => Status::Unavailable(format!("port {PORT} could not be opened ({e})")),
		};
		Self {
			status,
			rx,
			queued,
			progress,
		}
	}

	pub fn is_listening(&self) -> bool {
		matches!(self.status, Status::Listening)
	}

	/// What happened since the last call.
	pub fn poll(&self) -> Vec<Event> {
		// progress first: an export's progress is all from before it was
		// received, and the lock keeps the link from adding progress or an
		// export meanwhile
		let mut progress = locked(&self.progress);
		let mut events: Vec<Event> = progress
			.drain(..)
			.map(|(export, done, count)| Event::Progress {
				export,
				done,
				count,
			})
			.collect();
		events.extend(self.rx.try_iter());
		drop(progress);
		for event in &events {
			if let Event::Received(received) = event {
				// the app has it now, so the link may take more
				self.queued
					.fetch_sub(received.json.len(), Ordering::Relaxed);
			}
		}
		events
	}
}

/// An export's id, and how many of its pieces have arrived of how many.
type Progress = (String, usize, usize);

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
	mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the server keeps between requests.
struct Exports {
	/// Exports still arriving, by id.
	pending: HashMap<String, Pending>,
	/// Bytes of received exports sent to the app and not yet taken by it,
	/// shared with [`StudioLink::poll`] so they count towards [`MAX_HELD`].
	queued: Arc<AtomicUsize>,
	/// The latest progress of each export, oldest news first, until the app
	/// takes it (see [`StudioLink::poll`]). Apart from the app's queue of
	/// received exports, so progress can never fill that.
	progress: Arc<Mutex<Vec<Progress>>>,
	/// The folder received exports are saved in.
	folder: PathBuf,
	/// The second of the file names last handed out, and those names, so
	/// two exports finishing in the same second get different files.
	named: (u64, HashSet<String>),
}

impl Exports {
	fn new(folder: PathBuf) -> Self {
		Self {
			pending: HashMap::new(),
			queued: Arc::default(),
			progress: Arc::default(),
			folder,
			named: (0, HashSet::new()),
		}
	}

	/// Where to save a received export: its name (made safe) and `stamp`
	/// (the time, in seconds), with `-2`, `-3` and so on added when that file
	/// was handed out already or is there on disk.
	fn path_for(&mut self, name: &str, stamp: u64) -> PathBuf {
		let base = if name.trim().is_empty() {
			"studio-export"
		} else {
			name.trim()
		};
		let base = format!("{}-{stamp}", crate::workspace::sanitize(base));
		if self.named.0 != stamp {
			// names carry their second, so older ones cannot clash
			self.named = (stamp, HashSet::new());
		}
		let mut n = 1;
		loop {
			let file = if n == 1 {
				format!("{base}.json")
			} else {
				format!("{base}-{n}.json")
			};
			let path = self.folder.join(&file);
			if !self.named.1.contains(&file) && !path.exists() {
				self.named.1.insert(file);
				return path;
			}
			n += 1;
		}
	}

	/// Bytes held for the app: pieces still arriving and received exports
	/// it has not taken.
	fn held(&self) -> usize {
		self.pending
			.values()
			.fold(self.queued.load(Ordering::Relaxed), |sum, p| {
				sum.saturating_add(p.size)
			})
	}

	/// Forgets exports with no new piece for [`PENDING_TIMEOUT`].
	fn expire(&mut self, now: Instant) {
		self.pending
			.retain(|_, p| now.duration_since(p.last) < PENDING_TIMEOUT);
	}
}

struct Pending {
	name: String,
	chunks: Vec<Option<Vec<u8>>>,
	/// Bytes in `chunks`.
	size: usize,
	/// When the last piece arrived.
	last: Instant,
}

impl Default for Pending {
	fn default() -> Self {
		Self {
			name: String::new(),
			chunks: Vec::new(),
			size: 0,
			last: Instant::now(),
		}
	}
}

struct Request {
	method: String,
	path: String,
	query: HashMap<String, String>,
	headers: HashMap<String, String>,
	body: Vec<u8>,
}

struct Reply {
	code: u16,
	content_type: &'static str,
	body: Vec<u8>,
}

impl Reply {
	fn text(code: u16, body: impl Into<Vec<u8>>) -> Self {
		Self {
			code,
			content_type: "text/plain; charset=utf-8",
			body: body.into(),
		}
	}
}

fn serve(listener: TcpListener, tx: SyncSender<Event>, mut exports: Exports, ctx: egui::Context) {
	// the port asked for, or the one the system chose (tests ask for any)
	let port = listener.local_addr().map_or(PORT, |a| a.port());
	// one connection at a time, each with deadlines, so none can hold the
	// link for long
	loop {
		// while exports are arriving, wake now and then to forget the ones
		// that stopped, even if nothing else connects
		let _ = listener.set_nonblocking(!exports.pending.is_empty());
		let stream = match listener.accept() {
			Ok((stream, _)) => stream,
			Err(e) if e.kind() == ErrorKind::WouldBlock => {
				exports.expire(Instant::now());
				std::thread::sleep(EXPIRY_POLL);
				continue;
			}
			Err(_) => continue,
		};
		exports.expire(Instant::now());
		// a socket accepted while the listener is non-blocking can be too
		// (it is on Windows); the deadlines need it blocking
		let _ = stream.set_nonblocking(false);
		let reply = match read_request(Deadline::new(&stream, READ_TIMEOUT), port) {
			Ok(request) => route(request, &mut exports, &tx),
			Err(reply) => reply,
		};
		ctx.request_repaint();
		respond(&stream, reply);
	}
}

fn respond(stream: &TcpStream, reply: Reply) {
	let mut out = Deadline::new(stream, WRITE_TIMEOUT);
	let reason = if reply.code == 200 { "OK" } else { "Error" };
	let _ = write!(
		out,
		"HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
		reply.code,
		reply.content_type,
		reply.body.len()
	);
	let _ = out.write_all(&reply.body);
	let _ = out.flush();
	let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// A connection with one deadline for everything read from it or written
/// to it, rather than a timeout for each read or write: a sender that
/// trickles bytes, or a reader that takes them slowly, cannot hold the
/// link past it.
struct Deadline<'a> {
	stream: &'a TcpStream,
	until: Instant,
}

impl<'a> Deadline<'a> {
	fn new(stream: &'a TcpStream, within: Duration) -> Self {
		Self {
			stream,
			until: Instant::now() + within,
		}
	}

	/// The time left, or an error once there is none.
	fn left(&self) -> std::io::Result<Duration> {
		let left = self.until.saturating_duration_since(Instant::now());
		if left.is_zero() {
			return Err(ErrorKind::TimedOut.into());
		}
		Ok(left)
	}
}

impl Read for Deadline<'_> {
	fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
		self.stream.set_read_timeout(Some(self.left()?))?;
		let mut stream = self.stream;
		stream.read(buf)
	}
}

impl Write for Deadline<'_> {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		self.stream.set_write_timeout(Some(self.left()?))?;
		let mut stream = self.stream;
		stream.write(buf)
	}

	fn flush(&mut self) -> std::io::Result<()> {
		let mut stream = self.stream;
		stream.flush()
	}
}

/// The refusal for a read that failed: too slow, or else `otherwise`.
fn read_failed(e: &std::io::Error, otherwise: &'static str) -> Reply {
	match e.kind() {
		// Windows says TimedOut, others WouldBlock
		ErrorKind::TimedOut | ErrorKind::WouldBlock => Reply::text(408, "request took too long"),
		_ => Reply::text(400, otherwise),
	}
}

/// Reads one line of a request's head (its request line and headers) into
/// `line`, refusing a head longer than [`MAX_HEAD`] or one the stream ends
/// in.
fn head_line<R: Read>(
	head: &mut std::io::Take<&mut BufReader<R>>,
	line: &mut String,
	bad: &'static str,
) -> Result<(), Reply> {
	line.clear();
	head.read_line(line).map_err(|e| read_failed(&e, bad))?;
	if line.ends_with('\n') {
		Ok(())
	} else if head.limit() == 0 {
		Err(Reply::text(431, "request head too large"))
	} else {
		Err(Reply::text(400, "incomplete request"))
	}
}

/// Reads a whole request, body included: closing a socket with unread data
/// resets it, and the sender would never see the reply. A request that is
/// not for the link (see [`for_the_link`]) is refused before its body is
/// read.
fn read_request(stream: impl Read, port: u16) -> Result<Request, Reply> {
	let mut reader = BufReader::new(stream);
	let mut head = reader.by_ref().take(MAX_HEAD as u64);
	let mut line = String::new();
	head_line(&mut head, &mut line, "bad request")?;
	let mut words = line.split_whitespace();
	let method = words.next().unwrap_or("").to_string();
	let target = words.next().unwrap_or("");
	let (path, query) = target.split_once('?').unwrap_or((target, ""));
	let query = query
		.split('&')
		.filter_map(|kv| kv.split_once('='))
		.map(|(k, v)| (k.to_string(), v.to_string()))
		.collect();
	let path = path.to_string();
	let mut headers = HashMap::new();
	// every Host header, so a second one cannot hide behind the last
	let mut hosts = Vec::new();
	loop {
		head_line(&mut head, &mut line, "bad headers")?;
		let header = line.trim_end();
		if header.is_empty() {
			break;
		}
		if let Some((k, v)) = header.split_once(':') {
			let name = k.trim().to_ascii_lowercase();
			if name == "host" {
				hosts.push(v.trim().to_string());
			}
			headers.insert(name, v.trim().to_string());
		}
	}
	if !matches!(hosts.as_slice(), [host] if for_the_link(host, port)) {
		return Err(Reply::text(403, format!("Host must be localhost:{port}")));
	}
	let length: usize = headers
		.get("content-length")
		.map_or(Ok(0), |v| v.parse())
		.map_err(|_| Reply::text(400, "bad Content-Length"))?;
	if length > MAX_CHUNK {
		return Err(Reply::text(413, "piece too large"));
	}
	let mut body = vec![0; length];
	reader
		.read_exact(&mut body)
		.map_err(|e| read_failed(&e, "body shorter than Content-Length"))?;
	Ok(Request {
		method,
		path,
		query,
		headers,
		body,
	})
}

/// Whether a `Host` header names the link: `localhost` or `127.0.0.1`, with
/// the link's port or with none. A web page whose own name has been pointed
/// at 127.0.0.1 sends that name instead, so it is refused; it is the name
/// that matters, so a client that leaves the port out is still let in.
fn for_the_link(host: &str, port: u16) -> bool {
	let (name, p) = match host.rsplit_once(':') {
		Some((name, p)) => (name, Some(p)),
		None => (host, None),
	};
	p.is_none_or(|p| p == port.to_string())
		&& (name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1")
}

fn route(request: Request, exports: &mut Exports, tx: &SyncSender<Event>) -> Reply {
	match (request.method.as_str(), request.path.as_str()) {
		("GET", "/export.luau") => {
			let schema = request
				.query
				.get("schema")
				.and_then(|v| v.parse().ok())
				.unwrap_or(LATEST_SCHEMA);
			let script = exporter(schema).unwrap_or_else(|| {
				// still a script, so Studio shows the reason
				format!(
					"error(\"Meshvault has exporters for schemas {}, not {schema}\")",
					SCHEMAS.map(|v| v.to_string()).join(" and ")
				)
			});
			Reply::text(200, script)
		}
		("GET", "/") => Reply::text(200, "Meshvault Studio link"),
		("POST", "/export") => {
			let id = request.headers.get("x-meshvault-export").cloned();
			match receive_piece(request, exports) {
				Ok(Some(done)) => {
					// the exporter prints this, so Studio users know where it went. Studio's
					// Output shows only the last part of a path, so the folder is in words
					// (the same folder receive_piece saves to)
					let reply = format!(
						"saving it as {} in Documents › Meshvault › {EXPORTS_FOLDER}",
						done.path
							.file_name()
							.map(|n| n.to_string_lossy())
							.unwrap_or_default()
					);
					// counted until the app takes it; never wait for the app, as
					// that would hold the link
					let size = done.json.len();
					// its progress is over; while locked, so the app cannot take
					// that progress after the export
					let mut progress = locked(&exports.progress);
					progress.retain(|(id, ..)| *id != done.export);
					exports.queued.fetch_add(size, Ordering::Relaxed);
					if tx.try_send(Event::Received(done)).is_err() {
						exports.queued.fetch_sub(size, Ordering::Relaxed);
						return Reply::text(503, "Meshvault is busy, send it again later");
					}
					Reply::text(200, reply)
				}
				Ok(None) => {
					// how far this export has got (only this one: the others have
					// not changed), replacing its last count and moved to the end
					// as the newest; dropped if too many exports are waiting
					if let Some(id) = id
						&& let Some(p) = exports.pending.get(&id)
					{
						let now = (
							id,
							p.chunks.iter().filter(|c| c.is_some()).count(),
							p.chunks.len(),
						);
						let mut progress = locked(&exports.progress);
						progress.retain(|(id, ..)| *id != now.0);
						if progress.len() < MAX_EVENTS {
							progress.push(now);
						}
					}
					Reply::text(200, "ok")
				}
				Err(reply) => reply,
			}
		}
		_ => Reply::text(404, "GET /export.luau or POST /export"),
	}
}

/// Stores one piece. Returns the whole export once its last piece arrives.
/// A piece sent again replaces the first copy.
fn receive_piece(request: Request, exports: &mut Exports) -> Result<Option<Received>, Reply> {
	let headers = &request.headers;
	let id = headers
		.get("x-meshvault-export")
		.ok_or(Reply::text(400, "X-Meshvault-Export required"))?;
	let (index, count) = headers
		.get("x-meshvault-chunk")
		.and_then(|v| v.split_once('/'))
		.and_then(|(i, n)| {
			Some((
				i.trim().parse::<usize>().ok()?,
				n.trim().parse::<usize>().ok()?,
			))
		})
		.filter(|&(i, n)| i >= 1 && i <= n && n <= MAX_PIECES)
		.ok_or(Reply::text(400, "X-Meshvault-Chunk must be i/n"))?;

	if !exports.pending.contains_key(id) && exports.pending.len() >= MAX_PENDING {
		return Err(Reply::text(503, "too many exports arriving at once"));
	}
	let held = exports.held();
	let pending = &mut exports.pending;
	let entry = pending.entry(id.clone()).or_insert_with(|| Pending {
		chunks: vec![None; count],
		..Default::default()
	});
	if entry.chunks.len() != count {
		pending.remove(id);
		return Err(Reply::text(400, "piece count changed"));
	}
	if let Some(name) = headers.get("x-meshvault-name") {
		entry.name = name.chars().take(MAX_NAME).collect();
	}
	let replaced = entry.chunks[index - 1].as_ref().map_or(0, Vec::len);
	entry.size = entry.size - replaced + request.body.len();
	if entry.size > MAX_EXPORT {
		pending.remove(id);
		return Err(Reply::text(413, "export too large"));
	}
	if (held - replaced).saturating_add(request.body.len()) > MAX_HELD {
		pending.remove(id);
		return Err(Reply::text(
			503,
			"too much arriving at once, send it again later",
		));
	}
	entry.last = Instant::now();
	entry.chunks[index - 1] = Some(request.body);
	if entry.chunks.iter().any(Option::is_none) {
		return Ok(None);
	}
	// still there: `entry` was this id's entry a moment ago
	let done = pending.remove(id).expect("present");
	let json = done.chunks.into_iter().flatten().flatten().collect();
	let stamp = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |d| d.as_secs());
	Ok(Some(Received {
		export: id.clone(),
		path: exports.path_for(&done.name, stamp),
		name: done.name,
		json,
	}))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The server's state, saving to a folder that is never made.
	fn exports() -> Exports {
		Exports::new(std::env::temp_dir().join("meshvault_link_tests_never_made"))
	}

	fn start() -> (u16, Receiver<Event>) {
		let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
		let port = listener.local_addr().unwrap().port();
		let (tx, rx) = mpsc::sync_channel(MAX_EVENTS);
		std::thread::spawn(move || serve(listener, tx, exports(), egui::Context::default()));
		(port, rx)
	}

	fn request(port: u16, head: &str, body: &str) -> String {
		request_to(port, &format!("localhost:{port}"), head, body)
	}

	fn request_to(port: u16, host: &str, head: &str, body: &str) -> String {
		let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
		write!(
			s,
			"{head}\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{body}",
			body.len()
		)
		.unwrap();
		let mut reply = String::new();
		s.read_to_string(&mut reply).unwrap();
		reply
	}

	fn post(port: u16, id: &str, chunk: &str, body: &str) -> String {
		request(
			port,
			&format!(
				"POST /export HTTP/1.1\r\nX-Meshvault-Export: {id}\r\nX-Meshvault-Chunk: {chunk}\r\nX-Meshvault-Name: Test Place"
			),
			body,
		)
	}

	fn received(rx: &Receiver<Event>) -> Received {
		loop {
			if let Event::Received(r) = rx.recv().unwrap() {
				return r;
			}
		}
	}

	#[test]
	fn joins_pieces_in_order() {
		let (port, rx) = start();
		// out of order, and another export interleaved
		assert!(post(port, "a", "2/3", "\"Models\":[],").contains("ok"));
		assert!(post(port, "b", "1/1", "{}").contains("saving it as"));
		assert!(post(port, "a", "1/3", "{\"SchemaVersion\":2,").contains("ok"));
		assert!(post(port, "a", "3/3", "\"Meshes\":[]}").contains("saving it as"));
		assert_eq!(received(&rx).json, b"{}");
		let second = received(&rx);
		assert_eq!(second.name, "Test Place");
		assert_eq!(
			String::from_utf8(second.json).unwrap(),
			"{\"SchemaVersion\":2,\"Models\":[],\"Meshes\":[]}"
		);
		// invalid piece indices are refused
		assert!(post(port, "c", "0/1", "x").contains("400"));
	}

	#[test]
	fn serves_exporters() {
		let (port, _rx) = start();
		let latest = request(port, "GET /export.luau HTTP/1.1", "");
		assert!(latest.starts_with("HTTP/1.1 200"));
		assert!(latest.contains("local SCHEMA = 2\n"));
		assert!(latest.contains("{SchemaVersion=SCHEMA,"));
		assert!(latest.contains(&format!("http://localhost:{PORT}/export")));
		assert!(!latest.contains("__MESHVAULT_"));
		assert!(latest.contains(env!("CARGO_PKG_VERSION")));
		let v1 = request(port, "GET /export.luau?schema=1 HTTP/1.1", "");
		assert!(v1.contains("local SCHEMA = 1\n"));
		assert!(v1.contains("{SchemaVersion=SCHEMA,"));
		let unknown = request(port, "GET /export.luau?schema=9 HTTP/1.1", "");
		assert!(unknown.contains("error(\"Meshvault has exporters for schemas 1 and 2, not 9\")"));
	}

	// Studio is the only thing that runs the exporters, so check here that
	// what is served at least parses as Luau
	#[test]
	fn exporters_parse_as_luau() {
		for v in SCHEMAS {
			let source = exporter(v).unwrap();
			let parsed = full_moon::parse_fallible(&source, full_moon::LuaVersion::luau());
			let errors: Vec<String> = parsed.errors().iter().map(|e| e.to_string()).collect();
			assert!(errors.is_empty(), "schema {v} exporter: {errors:#?}");
		}
	}

	#[test]
	fn exporters_are_functions_of_options() {
		for v in SCHEMAS {
			let source = exporter(v).unwrap();
			assert!(source.contains("return function(options)"));
			// the problems are returned to the caller, an empty list when none
			assert!(source.contains("return issues"));
			// a clip that never loads must not stall the export
			assert!(source.contains("local ok, clip = loadClip(animationId)"));
			// current avatars' limbs are AnimationConstraints
			assert!(source.contains("inst:IsA(\"AnimationConstraint\")"));
			// the declared schema, and the payload saying it
			assert!(source.contains(&format!("local SCHEMA = {v}\n")));
			assert!(source.contains("local payload = {SchemaVersion=SCHEMA,"));
			assert!(source.trim_end().ends_with("end"));
		}
		let command = studio_code(1, CodeMode::Send, CodeLayout::OneLine);
		assert!(command.contains("/export.luau?schema=1"));
		assert!(command.ends_with("loadstring(s)()()"));
		assert!(
			!studio_code(LATEST_SCHEMA, CodeMode::Send, CodeLayout::OneLine).contains("schema=")
		);
		// exporting without sending: the export comes back, with a sender
		for v in SCHEMAS {
			let source = exporter(v).unwrap();
			assert!(source.contains("if options.send == false then"));
			assert!(source.contains("return issues, export, send"));
			assert!(source.contains("local function send(export)"));
		}
		// every mode and layout fetches the same exporter and runs it
		for mode in [CodeMode::Send, CodeMode::Edit] {
			for layout in [CodeLayout::OneLine, CodeLayout::Formatted] {
				let code = studio_code(1, mode, layout);
				assert!(code.contains("/export.luau?schema=1"), "{code}");
				assert_eq!(
					code.contains('\n'),
					layout == CodeLayout::Formatted,
					"{code}"
				);
				let edits = code.contains("send = false") || code.contains("send=false");
				assert_eq!(edits, mode == CodeMode::Edit, "{code}");
			}
		}
		let script = studio_code(1, CodeMode::Edit, CodeLayout::Formatted);
		assert!(script.contains("loadstring(source)()({ send = false })"));
		assert!(script.trim_end().ends_with("your own way"));
		assert!(studio_code(1, CodeMode::Edit, CodeLayout::OneLine).ends_with("send(export)"));
	}

	// The exact code users paste into Studio, for every mode and layout.
	const SEND_ONE_LINE: &str = "local h=game:GetService(\"HttpService\") local e=h.HttpEnabled \
		h.HttpEnabled=true local ok,s=pcall(h.GetAsync,h,\"http://localhost:34875/export.luau\") \
		h.HttpEnabled=e assert(ok,\"Could not reach Meshvault (is it open, and is Studio in Edit \
		mode, not Play?): \"..tostring(s)) loadstring(s)()()";
	const EDIT_ONE_LINE: &str = "local h=game:GetService(\"HttpService\") local e=h.HttpEnabled \
		h.HttpEnabled=true local ok,s=pcall(h.GetAsync,h,\"http://localhost:34875/export.luau\") \
		h.HttpEnabled=e assert(ok,\"Could not reach Meshvault (is it open, and is Studio in Edit \
		mode, not Play?): \"..tostring(s)) local issues,export,send=loadstring(s)()({send=false}) \
		send(export)";
	const FORMATTED_FETCH: &str = concat!(
		"local http = game:GetService(\"HttpService\")\n",
		"local url = \"http://localhost:34875/export.luau\"\n",
		"local enabled = http.HttpEnabled\n",
		"http.HttpEnabled = true\n",
		"local ok, source = pcall(http.GetAsync, http, url)\n",
		"http.HttpEnabled = enabled\n",
		"assert(ok, \"Could not reach Meshvault (is it open, and is Studio in \"\n",
		"\t.. \"Edit mode, not Play?): \" .. tostring(source))\n",
		"\n",
	);

	#[test]
	fn studio_code_is_exact() {
		let send_formatted = format!("{FORMATTED_FETCH}loadstring(source)()()");
		let edit_formatted = format!(
			"{FORMATTED_FETCH}{}",
			concat!(
				"local issues, export, send = loadstring(source)()({ send = false })\n",
				"\n",
				"-- change the export here, e.g. keep only the first model:\n",
				"-- export.Models = { export.Models[1] }\n",
				"\n",
				"send(export) -- or save http:JSONEncode(export) your own way",
			)
		);
		let cases = [
			(
				CodeMode::Send,
				CodeLayout::OneLine,
				SEND_ONE_LINE.to_string(),
			),
			(
				CodeMode::Edit,
				CodeLayout::OneLine,
				EDIT_ONE_LINE.to_string(),
			),
			(CodeMode::Send, CodeLayout::Formatted, send_formatted),
			(CodeMode::Edit, CodeLayout::Formatted, edit_formatted),
		];
		for (mode, layout, latest) in cases {
			// the newest schema has no query; any other asks for its schema
			assert_eq!(studio_code(LATEST_SCHEMA, mode, layout), latest);
			let explicit = latest.replace("/export.luau\"", "/export.luau?schema=1\"");
			assert_ne!(explicit, latest);
			assert_eq!(studio_code(1, mode, layout), explicit);
		}
	}

	fn read(bytes: &[u8]) -> Result<Request, Reply> {
		read_request(std::io::Cursor::new(bytes), PORT)
	}

	/// The request, or a panic naming the refusal (`Reply` has no `Debug`).
	fn parsed(bytes: &[u8]) -> Request {
		match read(bytes) {
			Ok(request) => request,
			Err(reply) => panic!(
				"refused with {} {}",
				reply.code,
				String::from_utf8_lossy(&reply.body)
			),
		}
	}

	/// The refusal's code and text, or a panic if it was accepted.
	fn refusal<T>(result: Result<T, Reply>) -> (u16, String) {
		match result {
			Ok(_) => panic!("accepted"),
			Err(reply) => {
				assert_eq!(reply.content_type, "text/plain; charset=utf-8");
				(reply.code, String::from_utf8(reply.body).unwrap())
			}
		}
	}

	#[test]
	fn reads_requests() {
		let r = parsed(
			b"POST /export?schema=1&a=b=c&flag&=x HTTP/1.1\r\n\
			  Host: localhost:34875\r\n\
			  X-MESHVAULT-Export:  id 1 \r\n\
			  no colon here\r\n\
			  x-meshvault-chunk: 1/2\r\n\
			  X-Meshvault-Chunk: 2/2\r\n\
			  content-LENGTH: 5\r\n\
			  \r\n\
			  hello and more",
		);
		assert_eq!(r.method, "POST");
		assert_eq!(r.path, "/export");
		// pairs without `=` are dropped; the first `=` splits
		let mut query: Vec<_> = r.query.iter().collect();
		query.sort();
		assert_eq!(
			query,
			[
				(&String::new(), &"x".to_string()),
				(&"a".to_string(), &"b=c".to_string()),
				(&"schema".to_string(), &"1".to_string()),
			]
		);
		// names are lower-cased, values trimmed, a repeated header keeps the last,
		// and lines without a colon are skipped
		let mut headers: Vec<_> = r.headers.iter().collect();
		headers.sort();
		assert_eq!(
			headers,
			[
				(&"content-length".to_string(), &"5".to_string()),
				(&"host".to_string(), &"localhost:34875".to_string()),
				(&"x-meshvault-chunk".to_string(), &"2/2".to_string()),
				(&"x-meshvault-export".to_string(), &"id 1".to_string()),
			]
		);
		// only Content-Length bytes are read
		assert_eq!(r.body, b"hello");
	}

	#[test]
	fn reads_requests_leniently() {
		// bare newlines work, and without Content-Length the body is empty
		let r = parsed(b"GET /export.luau HTTP/1.1\nHost: localhost:34875\n\nignored");
		assert_eq!(
			(r.method.as_str(), r.path.as_str()),
			("GET", "/export.luau")
		);
		assert!(r.query.is_empty());
		assert!(r.body.is_empty());
		// a missing target is an empty path
		assert_eq!(parsed(b"GET\r\nHost: localhost:34875\r\n\r\n").path, "");
	}

	#[test]
	fn refuses_bad_requests() {
		for (bytes, code, text) in [
			// a stream that ends before the blank line after the headers is not
			// a request, however much of one it has
			(&b""[..], 400, "incomplete request"),
			(b"GET / HTTP/1.1", 400, "incomplete request"),
			(
				b"GET / HTTP/1.1\r\nHost: localhost:34875\r\n",
				400,
				"incomplete request",
			),
			(
				b"GET / HTTP/1.1\r\nHost: localhost:34875\r\n\r",
				400,
				"incomplete request",
			),
			(b"GET \xff HTTP/1.1\r\n\r\n", 400, "bad request"),
			(b"GET / HTTP/1.1\r\nX-A: \xff\r\n\r\n", 400, "bad headers"),
			(
				b"POST / HTTP/1.1\r\nHost: localhost:34875\r\nContent-Length: x\r\n\r\n",
				400,
				"bad Content-Length",
			),
			(
				b"POST / HTTP/1.1\r\nHost: localhost:34875\r\nContent-Length: -1\r\n\r\n",
				400,
				"bad Content-Length",
			),
			(
				b"POST / HTTP/1.1\r\nHost: localhost:34875\r\nContent-Length:\r\n\r\n",
				400,
				"bad Content-Length",
			),
			(
				b"POST / HTTP/1.1\r\nHost: localhost:34875\r\nContent-Length: 5\r\n\r\nabc",
				400,
				"body shorter than Content-Length",
			),
		] {
			assert_eq!(refusal(read(bytes)), (code, text.to_string()));
		}
		let over = format!(
			"POST / HTTP/1.1\r\nHost: localhost:34875\r\nContent-Length: {}\r\n\r\n",
			MAX_CHUNK + 1
		);
		assert_eq!(
			refusal(read(over.as_bytes())),
			(413, "piece too large".to_string())
		);
	}

	// A page whose own name points at 127.0.0.1 (DNS rebinding) sends that
	// name as its Host, so only the link's own names and port are let in,
	// before the body is read.
	#[test]
	fn refuses_requests_for_other_hosts() {
		let refused = (403, "Host must be localhost:34875".to_string());
		for hosts in [
			"",
			"Host: evil.example:34875\r\n",
			"Host: evil.example\r\n",
			"Host: localhost:80\r\n",
			"Host: localhost:034875\r\n",
			"Host: localhost.evil.example:34875\r\n",
			"Host: evil.localhost:34875\r\n",
			"Host: 127.0.0.2:34875\r\n",
			"Host: [::1]:34875\r\n",
			"Host: \r\n",
			// a second Host, either way round
			"Host: localhost:34875\r\nHost: evil.example:34875\r\n",
			"Host: evil.example:34875\r\nHost: localhost:34875\r\n",
		] {
			// the body is never read, so its absence is not the refusal
			let bytes = format!("POST /export HTTP/1.1\r\n{hosts}Content-Length: 9\r\n\r\n");
			assert_eq!(refusal(read(bytes.as_bytes())), refused, "{hosts:?}");
		}
		// the name is what keeps a rebound web page out, so no port is fine
		for host in [
			"localhost:34875",
			"LocalHost:34875",
			"127.0.0.1:34875",
			"localhost",
			"127.0.0.1",
		] {
			let r = parsed(format!("GET / HTTP/1.1\r\nhost: {host}\r\n\r\n").as_bytes());
			assert_eq!(r.headers["host"], host);
		}
		// and over a socket, with the port the server listens on
		let (port, _rx) = start();
		let wrong = request_to(port, "evil.example", "GET / HTTP/1.1", "");
		assert!(wrong.starts_with("HTTP/1.1 403"), "{wrong}");
		assert!(
			wrong.ends_with(&format!("Host must be localhost:{port}")),
			"{wrong}"
		);
		let ip = request_to(port, &format!("127.0.0.1:{port}"), "GET / HTTP/1.1", "");
		assert!(ip.starts_with("HTTP/1.1 200"), "{ip}");
		assert!(ip.ends_with("Meshvault Studio link"), "{ip}");
	}

	// The request line and headers together are capped, so a line with no
	// end, or endless headers, cannot grow without bound.
	#[test]
	fn refuses_heads_past_the_limit() {
		let too_large = (431, "request head too large".to_string());
		// a head of exactly the limit, blank line included, is read
		let start = "GET / HTTP/1.1\r\nHost: localhost:34875\r\nX-Pad: ";
		let pad = "a".repeat(MAX_HEAD - start.len() - "\r\n\r\n".len());
		let full = format!("{start}{pad}\r\n\r\n");
		assert_eq!(full.len(), MAX_HEAD);
		assert_eq!(parsed(full.as_bytes()).headers["x-pad"], pad);
		// one byte more is not
		let over = format!("{start}{pad}a\r\n\r\n");
		assert_eq!(refusal(read(over.as_bytes())), too_large);
		// nor is a long request line, or many short headers
		let long = "a".repeat(1 << 20);
		let line = format!("GET /{long} HTTP/1.1\r\nHost: localhost:34875\r\n\r\n");
		assert_eq!(refusal(read(line.as_bytes())), too_large);
		let many = format!(
			"GET / HTTP/1.1\r\nHost: localhost:34875\r\n{}\r\n",
			"X-A: b\r\n".repeat(MAX_HEAD)
		);
		assert_eq!(refusal(read(many.as_bytes())), too_large);
		// the body is not part of the head
		let body = "b".repeat(MAX_HEAD * 2);
		let post = format!(
			"POST / HTTP/1.1\r\nHost: localhost:34875\r\nContent-Length: {}\r\n\r\n{body}",
			body.len()
		);
		assert_eq!(parsed(post.as_bytes()).body.len(), body.len());
	}

	/// Both ends of a connection over loopback.
	fn connected() -> (TcpStream, TcpStream) {
		let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
		let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
		let (server, _) = listener.accept().unwrap();
		(client, server)
	}

	// The deadline is for the whole request, so a sender that keeps sending a
	// little at a time, each piece well within it, is still cut off.
	#[test]
	fn a_request_that_trickles_in_is_cut_off_at_the_deadline() {
		let (mut client, server) = connected();
		let (stop, stopped) = mpsc::channel::<()>();
		let sender = std::thread::spawn(move || {
			write!(client, "GET / HTTP/1.1\r\nHost: localhost:34875\r\n").unwrap();
			// a header every 10 ms, never the blank line, for up to 5 s
			for _ in 0..500 {
				if stopped.try_recv().is_ok() || client.write_all(b"X-A: b\r\n").is_err() {
					return;
				}
				std::thread::sleep(Duration::from_millis(10));
			}
		});
		let started = Instant::now();
		let result = read_request(Deadline::new(&server, Duration::from_millis(200)), PORT);
		let took = started.elapsed();
		let _ = stop.send(());
		sender.join().unwrap();
		// had there been no deadline, the request would have ended with the
		// sender, as an incomplete one
		assert_eq!(refusal(result), (408, "request took too long".to_string()));
		assert!(took < Duration::from_secs(4), "{took:?}");
	}

	#[test]
	fn nothing_is_read_or_written_after_the_deadline() {
		let (mut client, server) = connected();
		client.write_all(b"GET").unwrap();
		let mut late = Deadline::new(&server, Duration::ZERO);
		assert_eq!(
			late.read(&mut [0; 8]).unwrap_err().kind(),
			ErrorKind::TimedOut
		);
		assert_eq!(late.write(b"x").unwrap_err().kind(), ErrorKind::TimedOut);
		assert_eq!(
			refusal(read_request(late, PORT)),
			(408, "request took too long".to_string())
		);
	}

	fn piece(headers: &[(&str, &str)], body: &[u8]) -> Request {
		Request {
			method: "POST".into(),
			path: "/export".into(),
			query: HashMap::new(),
			headers: headers
				.iter()
				.map(|(k, v)| (k.to_string(), v.to_string()))
				.collect(),
			body: body.to_vec(),
		}
	}

	fn chunk(id: &str, chunk: &str, body: &[u8]) -> Request {
		piece(
			&[("x-meshvault-export", id), ("x-meshvault-chunk", chunk)],
			body,
		)
	}

	#[test]
	fn refuses_pieces_without_valid_headers() {
		let mut exports = exports();
		assert_eq!(
			refusal(receive_piece(
				piece(&[("x-meshvault-chunk", "1/1")], b"x"),
				&mut exports
			)),
			(400, "X-Meshvault-Export required".to_string())
		);
		let must = (400, "X-Meshvault-Chunk must be i/n".to_string());
		assert_eq!(
			refusal(receive_piece(
				piece(&[("x-meshvault-export", "a")], b"x"),
				&mut exports
			)),
			must
		);
		for bad in [
			"", "1", "/", "1/", "/1", "a/1", "1/b", "1/2/3", "-1/2", "1.0/2", "0/0", "1/0", "0/1",
			"3/2", "1/100001",
		] {
			assert_eq!(
				refusal(receive_piece(chunk("a", bad, b"x"), &mut exports)),
				must,
				"{bad:?}"
			);
		}
		// nothing is kept for a refused piece
		assert!(exports.pending.is_empty());
		// the parts are trimmed, and the largest count is allowed
		assert!(matches!(
			receive_piece(chunk("a", " 1 / 100000 ", b"x"), &mut exports),
			Ok(None)
		));
		assert_eq!(exports.pending["a"].chunks.len(), 100_000);
	}

	#[test]
	fn a_changed_piece_count_drops_the_export() {
		let mut exports = exports();
		assert!(matches!(
			receive_piece(chunk("a", "1/2", b"x"), &mut exports),
			Ok(None)
		));
		assert!(matches!(
			receive_piece(chunk("b", "1/2", b"y"), &mut exports),
			Ok(None)
		));
		assert_eq!(
			refusal(receive_piece(chunk("a", "2/3", b"z"), &mut exports)),
			(400, "piece count changed".to_string())
		);
		assert!(!exports.pending.contains_key("a"));
		assert!(exports.pending.contains_key("b"));
	}

	#[test]
	fn an_export_may_reach_the_size_limit_but_not_pass_it() {
		let mut exports = exports();
		// a nearly full export, without allocating one
		let nearly_full = || Pending {
			chunks: vec![None; 2],
			size: MAX_EXPORT - 1,
			..Default::default()
		};
		exports.pending.insert("at".to_string(), nearly_full());
		assert!(matches!(
			receive_piece(chunk("at", "1/2", b"x"), &mut exports),
			Ok(None)
		));
		assert_eq!(exports.pending["at"].size, MAX_EXPORT);
		exports.pending.insert("over".to_string(), nearly_full());
		assert_eq!(
			refusal(receive_piece(chunk("over", "1/2", b"xy"), &mut exports)),
			(413, "export too large".to_string())
		);
		assert!(!exports.pending.contains_key("over"));
		assert!(exports.pending.contains_key("at"));
	}

	// A piece sent again replaces the first copy, and only the copy kept
	// counts towards the size.
	#[test]
	fn a_resent_piece_replaces_the_first_and_counts_once() {
		let mut exports = exports();
		for body in [&b"abc"[..], b"def", b"ghij"] {
			assert!(matches!(
				receive_piece(chunk("a", "1/2", body), &mut exports),
				Ok(None)
			));
		}
		assert_eq!(exports.pending["a"].size, 4);
		assert_eq!(
			exports.pending["a"].chunks[0].as_deref(),
			Some(&b"ghij"[..])
		);
		// so resending cannot push an export that fits over the limit
		exports.pending.get_mut("a").unwrap().size = MAX_EXPORT;
		assert!(matches!(
			receive_piece(chunk("a", "1/2", b"wxyz"), &mut exports),
			Ok(None)
		));
		assert_eq!(exports.pending["a"].size, MAX_EXPORT);
	}

	// An export whose sender stopped part-way is forgotten once no piece has
	// come for a while; one still arriving is kept.
	#[test]
	fn abandoned_exports_are_forgotten() {
		let mut exports = exports();
		for id in ["stopped", "arriving"] {
			assert!(matches!(
				receive_piece(chunk(id, "1/2", b"x"), &mut exports),
				Ok(None)
			));
		}
		let later = exports.pending["stopped"].last + PENDING_TIMEOUT;
		exports.pending.get_mut("arriving").unwrap().last = later;
		exports.expire(later - Duration::from_millis(1));
		assert_eq!(exports.pending.len(), 2);
		exports.expire(later);
		assert!(!exports.pending.contains_key("stopped"));
		assert!(exports.pending.contains_key("arriving"));
		exports.expire(later + PENDING_TIMEOUT);
		assert!(exports.pending.is_empty());
	}

	#[test]
	fn only_so_many_exports_may_arrive_at_once() {
		let mut exports = exports();
		for i in 0..MAX_PENDING {
			assert!(matches!(
				receive_piece(chunk(&i.to_string(), "1/2", b"x"), &mut exports),
				Ok(None)
			));
		}
		assert_eq!(
			refusal(receive_piece(chunk("one more", "1/2", b"x"), &mut exports)),
			(503, "too many exports arriving at once".to_string())
		);
		// those already arriving carry on
		assert!(matches!(
			receive_piece(chunk("0", "2/2", b"y"), &mut exports),
			Ok(Some(_))
		));
		assert!(matches!(
			receive_piece(chunk("one more", "1/2", b"x"), &mut exports),
			Ok(None)
		));
	}

	// Pieces of every export arriving, and received exports the app has not
	// taken, share one limit.
	#[test]
	fn the_bytes_held_for_the_app_are_capped() {
		let too_much = (
			503,
			"too much arriving at once, send it again later".to_string(),
		);
		let mut exports = exports();
		// a nearly full export, without allocating one
		exports.pending.insert(
			"big".to_string(),
			Pending {
				chunks: vec![None; 2],
				size: MAX_HELD - 2,
				..Default::default()
			},
		);
		assert!(matches!(
			receive_piece(chunk("small", "1/2", b"x"), &mut exports),
			Ok(None)
		));
		// replacing a piece only adds the difference
		assert!(matches!(
			receive_piece(chunk("small", "1/2", b"y"), &mut exports),
			Ok(None)
		));
		assert!(matches!(
			receive_piece(chunk("small", "1/2", b"yz"), &mut exports),
			Ok(None)
		));
		assert_eq!(exports.held(), MAX_HELD);
		assert_eq!(
			refusal(receive_piece(chunk("other", "1/2", b"x"), &mut exports)),
			too_much
		);
		assert!(!exports.pending.contains_key("other"));
		// the export that would pass the limit is dropped, freeing its bytes
		assert_eq!(
			refusal(receive_piece(chunk("small", "2/2", b"x"), &mut exports)),
			too_much
		);
		assert!(!exports.pending.contains_key("small"));
		assert!(exports.pending.contains_key("big"));
		// received exports the app has not taken count too
		exports.pending.clear();
		exports.queued.store(MAX_HELD, Ordering::Relaxed);
		assert_eq!(
			refusal(receive_piece(chunk("next", "1/1", b"x"), &mut exports)),
			too_much
		);
		exports.queued.store(MAX_HELD - 1, Ordering::Relaxed);
		assert!(matches!(
			receive_piece(chunk("next", "1/1", b"x"), &mut exports),
			Ok(Some(_))
		));
	}

	/// The app's side of the link, taking what `exports` hands it.
	fn link(exports: &Exports) -> (SyncSender<Event>, StudioLink) {
		let (tx, rx) = mpsc::sync_channel(MAX_EVENTS);
		let link = StudioLink {
			status: Status::Listening,
			rx,
			queued: exports.queued.clone(),
			progress: exports.progress.clone(),
		};
		(tx, link)
	}

	fn describe(events: &[Event]) -> Vec<String> {
		events
			.iter()
			.map(|event| match event {
				Event::Progress {
					export,
					done,
					count,
				} => format!("{export} {done}/{count}"),
				Event::Received(r) => format!("{} received", r.export),
			})
			.collect()
	}

	// With two exports arriving at once, each piece reports only its own
	// export's progress, named, and the finished export says which it was.
	#[test]
	fn progress_is_for_the_export_that_moved() {
		let mut exports = exports();
		let (tx, link) = link(&exports);
		let mut send = |id: &str, chunk_of: &str| {
			let reply = route(chunk(id, chunk_of, b"x"), &mut exports, &tx);
			assert_eq!(reply.code, 200);
			describe(&link.poll())
		};
		assert_eq!(send("a", "1/3"), ["a 1/3"]);
		assert_eq!(send("b", "1/2"), ["b 1/2"]);
		assert_eq!(send("a", "2/3"), ["a 2/3"]);
		assert_eq!(send("b", "2/2"), ["b received"]);
		assert_eq!(send("a", "3/3"), ["a received"]);
	}

	// Progress waiting for a busy app is only each export's latest count, the
	// export that moved last coming last, and an export's progress goes once
	// it is received.
	#[test]
	fn waiting_progress_is_the_latest_of_each_export() {
		let mut exports = exports();
		let (tx, link) = link(&exports);
		for (id, chunk_of) in [("a", "1/3"), ("b", "1/3"), ("a", "2/3"), ("c", "1/1")] {
			assert_eq!(
				route(chunk(id, chunk_of, b"x"), &mut exports, &tx).code,
				200
			);
		}
		assert_eq!(describe(&link.poll()), ["b 1/3", "a 2/3", "c received"]);
		assert_eq!(route(chunk("b", "2/3", b"x"), &mut exports, &tx).code, 200);
		assert_eq!(route(chunk("b", "3/3", b"x"), &mut exports, &tx).code, 200);
		assert_eq!(describe(&link.poll()), ["b received"]);
	}

	// Bug: progress shared the app's queue with received exports, so 64
	// pieces arriving while the app was busy (a 128 MiB export, in Studio's
	// 2 MiB pieces) filled it and the export's last piece was refused.
	#[test]
	fn progress_never_crowds_out_a_received_export() {
		let mut exports = exports();
		let (tx, link) = link(&exports);
		let count = MAX_EVENTS + 1;
		for i in 1..=count {
			let reply = route(
				chunk("big", &format!("{i}/{count}"), b"x"),
				&mut exports,
				&tx,
			);
			assert_eq!(reply.code, 200, "piece {i}");
		}
		let events = link.poll();
		// its progress went when it was received
		assert_eq!(describe(&events), ["big received"]);
		assert!(matches!(&events[0], Event::Received(r) if r.json.len() == count));
		assert_eq!(exports.held(), 0);
	}

	// The app's queue is bounded: when it is full of received exports, a
	// finished export is refused (Studio then prints it) rather than the link
	// waiting for the app, and the app taking an export frees its bytes.
	#[test]
	fn received_exports_wait_in_a_bounded_queue() {
		let mut exports = exports();
		let (tx, link) = link(&exports);
		for i in 0..MAX_EVENTS {
			let reply = route(chunk(&format!("r{i}"), "1/1", b"x"), &mut exports, &tx);
			assert_eq!(reply.code, 200);
		}
		assert_eq!(exports.held(), MAX_EVENTS);
		let reply = route(chunk("a", "1/1", b"{}"), &mut exports, &tx);
		assert_eq!(reply.code, 503);
		assert_eq!(reply.body, b"Meshvault is busy, send it again later");
		assert_eq!(exports.held(), MAX_EVENTS);
		assert!(exports.pending.is_empty());
		// a part-way export still makes progress
		let reply = route(chunk("b", "1/2", b"{"), &mut exports, &tx);
		assert_eq!(reply.code, 200);
		let events = link.poll();
		assert_eq!(events.len(), MAX_EVENTS + 1);
		assert_eq!(describe(&events[..1]), ["b 1/2"]);
		assert_eq!(exports.held(), 1);
		let reply = route(chunk("b", "2/2", b"}"), &mut exports, &tx);
		assert_eq!(reply.code, 200);
		assert_eq!(exports.held(), 2);
		let events = link.poll();
		assert!(matches!(&events[..], [Event::Received(r)] if r.json == b"{}"));
		assert_eq!(exports.held(), 0);
	}

	#[test]
	fn joins_the_last_piece_into_an_export() {
		let mut exports = exports();
		let named = |chunk, name, body| {
			piece(
				&[
					("x-meshvault-export", "a"),
					("x-meshvault-chunk", chunk),
					("x-meshvault-name", name),
				],
				body,
			)
		};
		assert!(matches!(
			receive_piece(named("3/3", "First", b"c"), &mut exports),
			Ok(None)
		));
		// a piece without a name keeps the one given before; a later name wins
		assert!(matches!(
			receive_piece(chunk("a", "1/3", b"a"), &mut exports),
			Ok(None)
		));
		assert_eq!(exports.pending["a"].name, "First");
		let Ok(Some(done)) = receive_piece(named("2/3", "My/Place", b"b"), &mut exports) else {
			panic!("the last piece completes the export");
		};
		assert!(exports.pending.is_empty());
		assert_eq!(done.name, "My/Place");
		assert_eq!(done.json, b"abc");
		let file = done.path.file_name().unwrap().to_str().unwrap();
		assert!(file.starts_with("My_Place-"), "{file}");
		assert!(file.ends_with(".json"), "{file}");
		assert_eq!(done.path.parent(), Some(exports.folder.as_path()));
	}

	// The name becomes the file name and comes back in the reply, so only its
	// start is kept.
	#[test]
	fn a_long_name_is_cut_short() {
		let mut exports = exports();
		let long = "é".repeat(MAX_NAME * 10);
		let Ok(Some(done)) = receive_piece(
			piece(
				&[
					("x-meshvault-export", "a"),
					("x-meshvault-chunk", "1/1"),
					("x-meshvault-name", &long),
				],
				b"{}",
			),
			&mut exports,
		) else {
			panic!("one piece is the whole export");
		};
		assert_eq!(done.name, "é".repeat(MAX_NAME));
		let file = done.path.file_name().unwrap().to_str().unwrap();
		assert!(file.starts_with(&format!("{}-", done.name)), "{file}");
	}

	// Two exports with the same name (or names that are made safe alike)
	// finishing in the same second must not be saved over each other.
	#[test]
	fn exports_with_the_same_name_get_their_own_files() {
		let mut exports = exports();
		let mut paths = Vec::new();
		for (id, name) in [("a", "Place"), ("b", "Place"), ("c", "a/b"), ("d", "a\\b")] {
			let named = piece(
				&[
					("x-meshvault-export", id),
					("x-meshvault-chunk", "1/1"),
					("x-meshvault-name", name),
				],
				b"{}",
			);
			let Ok(Some(done)) = receive_piece(named, &mut exports) else {
				panic!("one piece is the whole export");
			};
			paths.push(done.path);
		}
		assert_ne!(paths[0], paths[1]);
		assert_ne!(paths[2], paths[3]);
	}

	#[test]
	fn names_received_files() {
		let folder = crate::test_support::TempDir::new("link_names");
		let mut exports = Exports::new(folder.to_path_buf());
		let mut file = |name: &str, stamp: u64| {
			let path = exports.path_for(name, stamp);
			assert_eq!(path.parent(), Some(&*folder));
			path.file_name().unwrap().to_str().unwrap().to_string()
		};
		assert_eq!(file("", 7), "studio-export-7.json");
		assert_eq!(file("  ", 7), "studio-export-7-2.json");
		assert_eq!(file("  Place  ", 7), "Place-7.json");
		// names made safe alike, in the same second, are told apart
		assert_eq!(file("a/b", 7), "a_b-7.json");
		assert_eq!(file(r"a\b", 7), "a_b-7-2.json");
		assert_eq!(file("a_b", 7), "a_b-7-3.json");
		// a new second starts again
		assert_eq!(file("Place", 8), "Place-8.json");
		// and a file already there (from an earlier run, say) is not reused
		std::fs::write(folder.join("Place-9.json"), "kept").unwrap();
		assert_eq!(file("Place", 9), "Place-9-2.json");
	}

	#[test]
	fn saving_never_replaces_a_file() {
		let folder = crate::test_support::TempDir::new("link_save");
		let received = Received {
			export: "a".into(),
			name: "Place".into(),
			json: b"{}".to_vec(),
			path: folder.join("Exports").join("Place-7.json"),
		};
		// the folder is made when needed
		received.save().unwrap();
		assert_eq!(std::fs::read(&received.path).unwrap(), b"{}");
		let again = Received {
			json: b"[]".to_vec(),
			..received
		};
		assert_eq!(again.save().unwrap_err().kind(), ErrorKind::AlreadyExists);
		assert_eq!(std::fs::read(&again.path).unwrap(), b"{}");
	}
}
