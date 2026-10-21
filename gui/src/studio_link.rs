//! The link between Roblox Studio and Meshvault, over `localhost:34875`.
//!
//! - `GET /export.luau[?schema=N]` serves the exporter (the newest schema
//!   unless asked otherwise), so it never has to be kept or pasted by hand.
//!   [`studio_command`] is the Command Bar line that fetches and runs it.
//! - `POST /export` receives the export in pieces, each with these headers:
//!   `X-Meshvault-Export` (an id shared by all pieces of one export),
//!   `X-Meshvault-Chunk` (`i/n`, 1-based) and optionally `X-Meshvault-Name`
//!   (a name for the saved file). Once all `n` pieces have arrived they are
//!   joined in order and handed to the app.
//!
//! Only connections from this machine are accepted, and requiring custom
//! headers means a web page cannot send an export (browsers would need a
//! CORS preflight, which is never answered).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use eframe::egui;

pub const PORT: u16 = 34875;
/// Largest single piece accepted; the exporter sends 2 MB.
const MAX_CHUNK: usize = 64 * 1024 * 1024;
/// Largest whole export accepted.
const MAX_EXPORT: usize = 2 * 1024 * 1024 * 1024;

/// The exporters served, by schema version. The newest is the default.
const EXPORTERS: [(u32, &str); 2] = [
	(1, include_str!("../studio/exporter_v1.luau")),
	(2, include_str!("../studio/exporter_v2.luau")),
];
pub const LATEST_SCHEMA: u32 = 2;
pub const SCHEMAS: [u32; 2] = [1, 2];

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

/// The line to paste into Studio's Command Bar: it fetches the exporter
/// from Meshvault and runs it. `game:HttpGet` is not available everywhere,
/// so it uses HttpService with HttpEnabled switched on just for the fetch.
pub fn studio_command(schema: u32) -> String {
	format!("{} loadstring(s)()()", fetch_exporter(schema))
}

/// Code fetching the exporter for `schema` from Meshvault into `s`, with
/// `h` the HttpService. HttpEnabled is on only while fetching.
fn fetch_exporter(schema: u32) -> String {
	let query = if schema == LATEST_SCHEMA {
		String::new()
	} else {
		format!("?schema={schema}")
	};
	format!(
		"local h=game:GetService(\"HttpService\") local e=h.HttpEnabled h.HttpEnabled=true \
		 local ok,s=pcall(h.GetAsync,h,\"http://localhost:{PORT}/export.luau{query}\") h.HttpEnabled=e \
		 assert(ok,\"Could not reach Meshvault (is it open, and is Studio in Edit mode, not Play?): \"..tostring(s))"
	)
}

/// Code for the Command Bar (or a plugin) that exports without sending:
/// the export comes back as a table to change, then `send(export)` sends
/// it to Meshvault, or the user saves its JSON their own way.
pub fn studio_script(schema: u32) -> String {
	format!(
		"{}\n\
		 local issues, export, send = loadstring(s)()({{ send = false }})\n\
		 \n\
		 -- change the export here, e.g. keep only the first model:\n\
		 -- export.Models = {{ export.Models[1] }}\n\
		 \n\
		 send(export) -- or save h:JSONEncode(export) your own way",
		fetch_exporter(schema)
	)
}

/// A complete export received from Studio.
pub struct Received {
	pub name: String,
	pub json: Vec<u8>,
	/// Where the app is to save it.
	pub path: std::path::PathBuf,
}

pub enum Event {
	/// A piece of an export arrived: `done` of `count`.
	Progress {
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
}

impl StudioLink {
	pub fn start(ctx: &egui::Context) -> Self {
		let (tx, rx) = mpsc::channel();
		let status = match TcpListener::bind((Ipv4Addr::LOCALHOST, PORT)) {
			Ok(listener) => {
				let ctx = ctx.clone();
				std::thread::spawn(move || serve(listener, tx, ctx));
				Status::Listening
			}
			Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Status::Unavailable(format!(
				"port {PORT} is already in use, probably by another Meshvault. Close it and restart Meshvault"
			)),
			Err(e) => Status::Unavailable(format!("port {PORT} could not be opened ({e})")),
		};
		Self { status, rx }
	}

	pub fn is_listening(&self) -> bool {
		matches!(self.status, Status::Listening)
	}

	/// What happened since the last call.
	pub fn poll(&self) -> Vec<Event> {
		self.rx.try_iter().collect()
	}
}

#[derive(Default)]
struct Pending {
	name: String,
	chunks: Vec<Option<Vec<u8>>>,
	size: usize,
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
	fn text(code: u16, body: &str) -> Self {
		Self {
			code,
			content_type: "text/plain; charset=utf-8",
			body: body.as_bytes().to_vec(),
		}
	}
}

fn serve(listener: TcpListener, tx: Sender<Event>, ctx: egui::Context) {
	let mut pending: HashMap<String, Pending> = HashMap::new();
	for stream in listener.incoming().flatten() {
		let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
		let reply = match read_request(&stream) {
			Ok(request) => route(request, &mut pending, &tx),
			Err(reply) => reply,
		};
		ctx.request_repaint();
		respond(stream, reply);
	}
}

fn respond(mut stream: TcpStream, reply: Reply) {
	let reason = if reply.code == 200 { "OK" } else { "Error" };
	let _ = write!(
		stream,
		"HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
		reply.code,
		reply.content_type,
		reply.body.len()
	);
	let _ = stream.write_all(&reply.body);
	let _ = stream.flush();
	let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// Reads a whole request, body included: closing a socket with unread data
/// resets it, and the sender would never see the reply.
fn read_request(stream: &TcpStream) -> Result<Request, Reply> {
	let mut reader = BufReader::new(stream);
	let mut line = String::new();
	reader
		.read_line(&mut line)
		.map_err(|_| Reply::text(400, "bad request"))?;
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
	loop {
		line.clear();
		reader
			.read_line(&mut line)
			.map_err(|_| Reply::text(400, "bad headers"))?;
		let header = line.trim_end();
		if header.is_empty() {
			break;
		}
		if let Some((k, v)) = header.split_once(':') {
			headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
		}
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
		.map_err(|_| Reply::text(400, "body shorter than Content-Length"))?;
	Ok(Request {
		method,
		path,
		query,
		headers,
		body,
	})
}

fn route(request: Request, pending: &mut HashMap<String, Pending>, tx: &Sender<Event>) -> Reply {
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
			Reply {
				code: 200,
				content_type: "text/plain; charset=utf-8",
				body: script.into_bytes(),
			}
		}
		("GET", "/") => Reply::text(200, "Meshvault Studio link"),
		("POST", "/export") => match receive_piece(request, pending) {
			Ok(Some(done)) => {
				// the exporter prints this, so Studio users know where it went. Studio's
				// Output shows only the last part of a path, so the folder is in words
				let reply = format!(
					"saving it as {} in Documents › Meshvault › Exports",
					done.path
						.file_name()
						.map(|n| n.to_string_lossy())
						.unwrap_or_default()
				);
				let _ = tx.send(Event::Received(done));
				Reply::text(200, &reply)
			}
			Ok(None) => {
				// how far the exports still arriving have got
				for p in pending.values() {
					let _ = tx.send(Event::Progress {
						done: p.chunks.iter().filter(|c| c.is_some()).count(),
						count: p.chunks.len(),
					});
				}
				Reply::text(200, "ok")
			}
			Err(reply) => reply,
		},
		_ => Reply::text(404, "GET /export.luau or POST /export"),
	}
}

/// Stores one piece. Returns the whole export once its last piece arrives.
fn receive_piece(
	request: Request,
	pending: &mut HashMap<String, Pending>,
) -> Result<Option<Received>, Reply> {
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
		.filter(|&(i, n)| i >= 1 && i <= n && n <= 100_000)
		.ok_or(Reply::text(400, "X-Meshvault-Chunk must be i/n"))?;

	let entry = pending.entry(id.clone()).or_insert_with(|| Pending {
		chunks: vec![None; count],
		..Default::default()
	});
	if entry.chunks.len() != count {
		pending.remove(id);
		return Err(Reply::text(400, "piece count changed"));
	}
	if let Some(name) = headers.get("x-meshvault-name") {
		entry.name = name.clone();
	}
	entry.size += request.body.len();
	if entry.size > MAX_EXPORT {
		pending.remove(id);
		return Err(Reply::text(413, "export too large"));
	}
	entry.chunks[index - 1] = Some(request.body);
	if entry.chunks.iter().any(Option::is_none) {
		return Ok(None);
	}
	let done = pending.remove(id).expect("present");
	let json = done.chunks.into_iter().flatten().flatten().collect();
	Ok(Some(Received {
		path: crate::config::Config::default_workspaces_dir()
			.join("Exports")
			.join(file_name(&done.name)),
		name: done.name,
		json,
	}))
}

/// A file name for a received export: its name (made safe) and the time.
fn file_name(name: &str) -> String {
	let stamp = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |d| d.as_secs());
	let base = if name.trim().is_empty() {
		"studio-export"
	} else {
		name.trim()
	};
	format!("{}-{stamp}.json", crate::workspace::sanitize(base))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn start() -> (u16, Receiver<Event>) {
		let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
		let port = listener.local_addr().unwrap().port();
		let (tx, rx) = mpsc::channel();
		std::thread::spawn(move || serve(listener, tx, egui::Context::default()));
		(port, rx)
	}

	fn request(port: u16, head: &str, body: &str) -> String {
		let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
		write!(
			s,
			"{head}\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
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
		// requests without the export headers are refused
		assert!(post(port, "c", "0/1", "x").contains("400"));
	}

	#[test]
	fn serves_exporters() {
		let (port, _rx) = start();
		let latest = request(port, "GET /export.luau HTTP/1.1", "");
		assert!(latest.starts_with("HTTP/1.1 200"));
		assert!(latest.contains("SchemaVersion=2"));
		assert!(latest.contains(&format!("http://localhost:{PORT}/export")));
		assert!(!latest.contains("__MESHVAULT_"));
		assert!(latest.contains(env!("CARGO_PKG_VERSION")));
		let v1 = request(port, "GET /export.luau?schema=1 HTTP/1.1", "");
		assert!(v1.contains("SchemaVersion=1"));
		let unknown = request(port, "GET /export.luau?schema=9 HTTP/1.1", "");
		assert!(unknown.contains("error(\"Meshvault has exporters for schemas 1 and 2, not 9\")"));
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
			assert!(source.contains(&format!("SchemaVersion={v}")));
			assert!(source.trim_end().ends_with("end"));
		}
		let command = studio_command(1);
		assert!(command.contains("/export.luau?schema=1"));
		assert!(command.ends_with("loadstring(s)()()"));
		assert!(!studio_command(LATEST_SCHEMA).contains("schema="));
		// exporting without sending: the export comes back, with a sender
		for v in SCHEMAS {
			let source = exporter(v).unwrap();
			assert!(source.contains("if options.send == false then"));
			assert!(source.contains("return issues, export, send"));
			assert!(source.contains("local function send(export)"));
		}
		let script = studio_script(1);
		assert!(script.contains("/export.luau?schema=1"));
		assert!(script.contains("loadstring(s)()({ send = false })"));
		assert!(script.trim_end().ends_with("your own way"));
	}
}
