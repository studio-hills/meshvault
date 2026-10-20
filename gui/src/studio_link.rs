//! Receives exports straight from Roblox Studio, so they need not be copied
//! out of the Output window.
//!
//! The exporter POSTs its JSON to `http://localhost:34875/export` in
//! pieces, each with these headers:
//!
//! - `X-Meshvault-Export`: an id shared by all pieces of one export
//! - `X-Meshvault-Chunk`: `i/n`, 1-based
//! - `X-Meshvault-Name` (optional): a name for the saved file
//!
//! Once all `n` pieces have arrived they are joined in order and handed to
//! the app. Only connections from this machine are accepted, and requiring
//! custom headers means a web page cannot send one (browsers would need a
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

/// A complete export received from Studio.
pub struct Received {
	pub name: String,
	pub json: Vec<u8>,
}

pub enum Status {
	Listening,
	/// Could not listen, usually because another Meshvault already is.
	Unavailable(String),
}

pub struct StudioLink {
	pub status: Status,
	rx: Receiver<Received>,
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
			Err(e) => Status::Unavailable(e.to_string()),
		};
		Self { status, rx }
	}

	/// Exports that finished arriving since the last call.
	pub fn poll(&self) -> Vec<Received> {
		self.rx.try_iter().collect()
	}
}

#[derive(Default)]
struct Pending {
	name: String,
	chunks: Vec<Option<Vec<u8>>>,
	size: usize,
}

fn serve(listener: TcpListener, tx: Sender<Received>, ctx: egui::Context) {
	let mut pending: HashMap<String, Pending> = HashMap::new();
	for stream in listener.incoming().flatten() {
		let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
		let reply = match handle(&stream, &mut pending) {
			Ok(Some(done)) => {
				let _ = tx.send(done);
				ctx.request_repaint();
				(200, "done")
			}
			Ok(None) => (200, "ok"),
			Err((code, message)) => (code, message),
		};
		respond(stream, reply.0, reply.1);
	}
}

fn respond(mut stream: TcpStream, code: u16, body: &str) {
	let reason = if code == 200 { "OK" } else { "Error" };
	let _ = write!(
		stream,
		"HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
		body.len()
	);
	let _ = stream.flush();
	let _ = stream.shutdown(std::net::Shutdown::Write);
}

type Failure = (u16, &'static str);

/// Reads one request. Returns the whole export once its last piece arrives.
fn handle(
	stream: &TcpStream,
	pending: &mut HashMap<String, Pending>,
) -> Result<Option<Received>, Failure> {
	let mut reader = BufReader::new(stream);
	let mut line = String::new();
	reader
		.read_line(&mut line)
		.map_err(|_| (400, "bad request"))?;
	let mut words = line.split_whitespace();
	let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
	if method != "POST" || path != "/export" {
		return Err((404, "POST /export"));
	}
	let mut headers = HashMap::new();
	loop {
		line.clear();
		reader
			.read_line(&mut line)
			.map_err(|_| (400, "bad headers"))?;
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
		.and_then(|v| v.parse().ok())
		.ok_or((411, "Content-Length required"))?;
	if length > MAX_CHUNK {
		return Err((413, "piece too large"));
	}
	// read the whole body before judging the request: closing a socket with
	// unread data resets it, and the sender never sees the reply
	let mut body = vec![0; length];
	reader
		.read_exact(&mut body)
		.map_err(|_| (400, "body shorter than Content-Length"))?;
	let id = headers
		.get("x-meshvault-export")
		.ok_or((400, "X-Meshvault-Export required"))?;
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
		.ok_or((400, "X-Meshvault-Chunk must be i/n"))?;

	let entry = pending.entry(id.clone()).or_insert_with(|| Pending {
		chunks: vec![None; count],
		..Default::default()
	});
	if entry.chunks.len() != count {
		pending.remove(id);
		return Err((400, "piece count changed"));
	}
	if let Some(name) = headers.get("x-meshvault-name") {
		entry.name = name.clone();
	}
	entry.size += body.len();
	if entry.size > MAX_EXPORT {
		pending.remove(id);
		return Err((413, "export too large"));
	}
	entry.chunks[index - 1] = Some(body);
	if entry.chunks.iter().any(Option::is_none) {
		return Ok(None);
	}
	let done = pending.remove(id).expect("present");
	let json = done.chunks.into_iter().flatten().flatten().collect();
	Ok(Some(Received {
		name: done.name,
		json,
	}))
}

/// A file name for a received export: its name (made safe) and the time.
pub fn file_name(name: &str) -> String {
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

	fn post(port: u16, id: &str, chunk: &str, body: &str) -> String {
		let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
		write!(
			s,
			"POST /export HTTP/1.1\r\nHost: localhost\r\nX-Meshvault-Export: {id}\r\nX-Meshvault-Chunk: {chunk}\r\nX-Meshvault-Name: Test Place\r\nContent-Length: {}\r\n\r\n{body}",
			body.len()
		)
		.unwrap();
		let mut reply = String::new();
		s.read_to_string(&mut reply).unwrap();
		reply
	}

	#[test]
	fn joins_pieces_in_order() {
		let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
		let port = listener.local_addr().unwrap().port();
		let (tx, rx) = mpsc::channel();
		std::thread::spawn(move || serve(listener, tx, egui::Context::default()));
		// out of order, and another export interleaved
		assert!(post(port, "a", "2/3", "\"Models\":[],").contains("ok"));
		assert!(post(port, "b", "1/1", "{}").contains("done"));
		assert!(post(port, "a", "1/3", "{\"SchemaVersion\":2,").contains("ok"));
		assert!(post(port, "a", "3/3", "\"Meshes\":[]}").contains("done"));
		let first = rx.recv().unwrap();
		assert_eq!(first.json, b"{}");
		let second = rx.recv().unwrap();
		assert_eq!(second.name, "Test Place");
		assert_eq!(
			String::from_utf8(second.json).unwrap(),
			"{\"SchemaVersion\":2,\"Models\":[],\"Meshes\":[]}"
		);
		// requests without the export headers are refused
		assert!(post(port, "c", "0/1", "x").contains("400"));
	}
}
