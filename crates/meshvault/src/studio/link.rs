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
//! `localhost` or `127.0.0.1` on the link's port (its `Host` header), so a
//! page that reaches the port under its own name (DNS rebinding) is
//! refused. There is no authentication: any program on this machine can
//! still send an export.

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
/// Most pieces one export may say it has, so a bad count cannot make a
/// huge list of pieces.
const MAX_PIECES: usize = 100_000;
/// How long one read of a request may wait before the connection is
/// dropped, so a sender that stalls does not hold the link.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
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
	fn text(code: u16, body: impl Into<Vec<u8>>) -> Self {
		Self {
			code,
			content_type: "text/plain; charset=utf-8",
			body: body.into(),
		}
	}
}

fn serve(listener: TcpListener, tx: Sender<Event>, ctx: egui::Context) {
	let mut pending: HashMap<String, Pending> = HashMap::new();
	// the port asked for, or the one the system chose (tests ask for any)
	let port = listener.local_addr().map_or(PORT, |a| a.port());
	for stream in listener.incoming().flatten() {
		let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
		let reply = match read_request(&stream, port) {
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
/// resets it, and the sender would never see the reply. A request that is
/// not for the link (see [`for_the_link`]) is refused before its body is
/// read.
fn read_request(stream: impl Read, port: u16) -> Result<Request, Reply> {
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
	// every Host header, so a second one cannot hide behind the last
	let mut hosts = Vec::new();
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
		.map_err(|_| Reply::text(400, "body shorter than Content-Length"))?;
	Ok(Request {
		method,
		path,
		query,
		headers,
		body,
	})
}

/// Whether a `Host` header names the link: `localhost` or `127.0.0.1` with
/// the link's port, exactly. A web page whose own name has been pointed at
/// 127.0.0.1 sends that name instead, so it is refused.
fn for_the_link(host: &str, port: u16) -> bool {
	let Some((name, p)) = host.rsplit_once(':') else {
		return false;
	};
	p == port.to_string() && (name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1")
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
			Reply::text(200, script)
		}
		("GET", "/") => Reply::text(200, "Meshvault Studio link"),
		("POST", "/export") => match receive_piece(request, pending) {
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
				let _ = tx.send(Event::Received(done));
				Reply::text(200, reply)
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
		.filter(|&(i, n)| i >= 1 && i <= n && n <= MAX_PIECES)
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
	// still there: `entry` was this id's entry a moment ago
	let done = pending.remove(id).expect("present");
	let json = done.chunks.into_iter().flatten().flatten().collect();
	Ok(Some(Received {
		path: crate::config::Config::default_workspaces_dir()
			.join(EXPORTS_FOLDER)
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
		// a stream that ends inside the headers is an empty request (pinned until
		// B10 is fixed; nothing at all is refused now, as it has no Host)
		let r = parsed(b"GET / HTTP/1.1\r\nHost: localhost:34875\r\n");
		assert!(r.body.is_empty());
		// a missing target is an empty path
		assert_eq!(parsed(b"GET\r\nHost: localhost:34875\r\n\r\n").path, "");
	}

	#[test]
	fn refuses_bad_requests() {
		for (bytes, code, text) in [
			(&b"GET \xff HTTP/1.1\r\n\r\n"[..], 400, "bad request"),
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
			"Host: localhost\r\n",
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
		for host in ["localhost:34875", "LocalHost:34875", "127.0.0.1:34875"] {
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

	// Pinned until bug B10 is fixed: request and header lines have no length
	// limit.
	#[test]
	fn reads_any_line_length() {
		let long = "a".repeat(1 << 20);
		let r = parsed(
			format!("GET /{long} HTTP/1.1\r\nHost: localhost:34875\r\nX-Long: {long}\r\n\r\n")
				.as_bytes(),
		);
		assert_eq!(r.path.len(), long.len() + 1);
		assert_eq!(r.headers["x-long"].len(), long.len());
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
		let mut pending = HashMap::new();
		assert_eq!(
			refusal(receive_piece(
				piece(&[("x-meshvault-chunk", "1/1")], b"x"),
				&mut pending
			)),
			(400, "X-Meshvault-Export required".to_string())
		);
		let must = (400, "X-Meshvault-Chunk must be i/n".to_string());
		assert_eq!(
			refusal(receive_piece(
				piece(&[("x-meshvault-export", "a")], b"x"),
				&mut pending
			)),
			must
		);
		for bad in [
			"", "1", "/", "1/", "/1", "a/1", "1/b", "1/2/3", "-1/2", "1.0/2", "0/0", "1/0", "0/1",
			"3/2", "1/100001",
		] {
			assert_eq!(
				refusal(receive_piece(chunk("a", bad, b"x"), &mut pending)),
				must,
				"{bad:?}"
			);
		}
		// nothing is kept for a refused piece
		assert!(pending.is_empty());
		// the parts are trimmed, and the largest count is allowed
		assert!(matches!(
			receive_piece(chunk("a", " 1 / 100000 ", b"x"), &mut pending),
			Ok(None)
		));
		assert_eq!(pending["a"].chunks.len(), 100_000);
	}

	#[test]
	fn a_changed_piece_count_drops_the_export() {
		let mut pending = HashMap::new();
		assert!(matches!(
			receive_piece(chunk("a", "1/2", b"x"), &mut pending),
			Ok(None)
		));
		assert!(matches!(
			receive_piece(chunk("b", "1/2", b"y"), &mut pending),
			Ok(None)
		));
		assert_eq!(
			refusal(receive_piece(chunk("a", "2/3", b"z"), &mut pending)),
			(400, "piece count changed".to_string())
		);
		assert!(!pending.contains_key("a"));
		assert!(pending.contains_key("b"));
	}

	#[test]
	fn an_export_may_reach_the_size_limit_but_not_pass_it() {
		let mut pending = HashMap::new();
		// a nearly full export, without allocating one
		let nearly_full = || Pending {
			chunks: vec![None; 2],
			size: MAX_EXPORT - 1,
			..Default::default()
		};
		pending.insert("at".to_string(), nearly_full());
		assert!(matches!(
			receive_piece(chunk("at", "1/2", b"x"), &mut pending),
			Ok(None)
		));
		assert_eq!(pending["at"].size, MAX_EXPORT);
		pending.insert("over".to_string(), nearly_full());
		assert_eq!(
			refusal(receive_piece(chunk("over", "1/2", b"xy"), &mut pending)),
			(413, "export too large".to_string())
		);
		assert!(!pending.contains_key("over"));
		assert!(pending.contains_key("at"));
	}

	// Pinned until bug B11 is fixed: a piece sent again replaces the first
	// copy but is counted towards the size twice.
	#[test]
	fn a_resent_piece_replaces_the_first_but_counts_twice() {
		let mut pending = HashMap::new();
		for body in [&b"abc"[..], b"def"] {
			assert!(matches!(
				receive_piece(chunk("a", "1/2", body), &mut pending),
				Ok(None)
			));
		}
		assert_eq!(pending["a"].size, 6);
		assert_eq!(pending["a"].chunks[0].as_deref(), Some(&b"def"[..]));
	}

	#[test]
	fn joins_the_last_piece_into_an_export() {
		let mut pending = HashMap::new();
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
			receive_piece(named("3/3", "First", b"c"), &mut pending),
			Ok(None)
		));
		// a piece without a name keeps the one given before; a later name wins
		assert!(matches!(
			receive_piece(chunk("a", "1/3", b"a"), &mut pending),
			Ok(None)
		));
		assert_eq!(pending["a"].name, "First");
		let Ok(Some(done)) = receive_piece(named("2/3", "My/Place", b"b"), &mut pending) else {
			panic!("the last piece completes the export");
		};
		assert!(pending.is_empty());
		assert_eq!(done.name, "My/Place");
		assert_eq!(done.json, b"abc");
		let file = done.path.file_name().unwrap().to_str().unwrap();
		assert!(file.starts_with("My_Place-"), "{file}");
		assert!(file.ends_with(".json"), "{file}");
		assert_eq!(
			done.path.parent(),
			Some(
				crate::config::Config::default_workspaces_dir()
					.join("Exports")
					.as_path()
			)
		);
	}

	#[test]
	fn names_received_files() {
		// the stamp is the current second, so only its shape is checked
		let stamp = |name: &str, prefix: &str| {
			let file = file_name(name);
			let stamp = file
				.strip_prefix(prefix)
				.and_then(|s| s.strip_suffix(".json"))
				.unwrap_or_else(|| panic!("{file}"));
			assert!(stamp.parse::<u64>().is_ok(), "{file}");
		};
		stamp("", "studio-export-");
		stamp("  ", "studio-export-");
		stamp("a/b", "a_b-");
		stamp("  Place  ", "Place-");
	}
}
