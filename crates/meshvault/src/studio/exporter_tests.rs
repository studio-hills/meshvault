//! Runs the exporters Meshvault serves, in Luau, against a small stand-in
//! for Roblox Studio: a place with a MeshPart in ReplicatedStorage and one in
//! Workspace, and an HttpService whose requests go to a real link server, so
//! the link's own checks decide whether an export arrives. This checks what
//! the code for Studio relies on, whatever the exporters' insides look like:
//! the script returns a function of options, which returns the problems, the
//! export and a sender; `send = false` keeps the export here to be changed;
//! the sender posts whatever it is given; and what arrives is an export
//! Meshvault reads.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use mlua::{Function, Lua, MultiValue, Table, Value};

use super::{Event, Exports, MAX_EVENTS, PORT, SCHEMAS, egui, exporter, serve};
use crate::studio::export_file;

/// Just enough of Roblox for an export of loose meshes. Instances are tables
/// with a class, name, parent and children; HttpService encodes JSON as
/// Roblox does (whole numbers without a point, empty tables as `[]`), gives
/// a new GUID each time, keeps
/// each request in `posts` and hands it to `forward` (the link), or refuses
/// it when `refuse` is set.
const STUDIO: &str = r#"
local classes = {
	MeshPart = { "MeshPart", "BasePart", "PVInstance", "Instance" },
	Folder = { "Folder", "Instance" },
	Model = { "Model", "PVInstance", "Instance" },
}
local Instance = {}
Instance.__index = Instance
function Instance.new(class, name, parent, props)
	local inst = setmetatable({ ClassName = class, Name = name, Parent = parent, children = {} }, Instance)
	for k, v in pairs(props or {}) do inst[k] = v end
	if parent then table.insert(parent.children, inst) end
	return inst
end
function Instance:IsA(class)
	for _, c in ipairs(classes[self.ClassName] or { self.ClassName, "Instance" }) do
		if c == class then return true end
	end
	return false
end
function Instance:GetChildren() return table.clone(self.children) end
function Instance:GetDescendants()
	local out = {}
	local function walk(inst)
		for _, child in ipairs(inst.children) do
			table.insert(out, child)
			walk(child)
		end
	end
	walk(self)
	return out
end
function Instance:FindFirstChildOfClass(class)
	for _, child in ipairs(self.children) do
		if child.ClassName == class then return child end
	end
	return nil
end
function Instance:FindFirstAncestorOfClass(class)
	local p = self.Parent
	while p do
		if p.ClassName == class then return p end
		p = p.Parent
	end
	return nil
end
-- for tests that build more of the place
newInstance = Instance.new
function Instance:GetScale() return 1 end
function Instance:GetFullName()
	local names = {}
	local inst = self
	while inst and inst ~= game do
		table.insert(names, 1, inst.Name)
		inst = inst.Parent
	end
	return table.concat(names, ".")
end

local function encode(v)
	local t = type(v)
	if t == "nil" then return "null" end
	if t == "boolean" then return tostring(v) end
	if t == "number" then
		return (v == math.floor(v) and math.abs(v) < 2^53) and ("%d"):format(v) or ("%.17g"):format(v)
	end
	if t == "string" then
		return '"' .. v:gsub('[%c"\\]', function(c)
			return ("\\u%04x"):format(c:byte())
		end) .. '"'
	end
	assert(t == "table", "cannot encode a " .. t)
	local parts = {}
	if next(v) == nil or #v > 0 then
		for _, item in ipairs(v) do table.insert(parts, encode(item)) end
		return "[" .. table.concat(parts, ",") .. "]"
	end
	for k, item in pairs(v) do
		table.insert(parts, encode(tostring(k)) .. ":" .. encode(item))
	end
	return "{" .. table.concat(parts, ",") .. "}"
end

posts = {}
-- the published name GetProductInfo gives, or nil for it to fail (as offline)
productName = nil
productCalls = 0
selectionFails = false
refuse = false
local http = { HttpEnabled = false }
function http:JSONEncode(v) return encode(v) end
local guids = 0
function http:GenerateGUID()
	guids += 1
	return "guid-" .. guids
end
function http:RequestAsync(request)
	assert(self.HttpEnabled, "HttpEnabled is off")
	table.insert(posts, request)
	if refuse then return { Success = false, StatusCode = 503, Body = "busy" } end
	return forward(request)
end
HttpService = http

game = Instance.new("DataModel", "Place")
game.PlaceId = 4242
local workspaceService = Instance.new("Workspace", "Workspace", game)
Instance.new("MeshPart", "Sentinel", workspaceService, { MeshId = "rbxassetid://999", TextureID = "" })
local storage = Instance.new("Folder", "ReplicatedStorage", game)
part = Instance.new("MeshPart", "Rock", storage, {
	MeshId = "rbxassetid://123", TextureID = "rbxassetid://456",
})
empty = Instance.new("Folder", "Empty", storage)
local services = {
	HttpService = http,
	Workspace = workspaceService,
	Selection = { Get = function()
		if selectionFails then error("Selection broke") end
		return {}
	end },
	RunService = { IsRunning = function() return false end },
	AnimationClipProvider = {},
	MarketplaceService = { GetProductInfo = function(_, id)
		assert(id == game.PlaceId, "asked about another place")
		productCalls += 1
		if not productName then error("HTTP 403 (Forbidden)") end
		return { Name = productName }
	end },
	MaterialService = Instance.new("MaterialService", "MaterialService"),
}
function game:GetService(name) return (assert(services[name], name)) end

Enum = { MeshType = { FileMesh = "FileMesh" } }
-- what the exporter prints, a line each; warnings are not kept
printed = {}
print = function(...)
	local parts = {}
	for i = 1, select('#', ...) do parts[i] = tostring((select(i, ...))) end
	table.insert(printed, table.concat(parts, " "))
end
warn = function(...) end
"#;

/// A Luau with the stand-in Studio, whose requests go to a link server of
/// its own, and the function the served exporter for `schema` returns.
struct Studio {
	lua: Lua,
	entry: Function,
	events: Receiver<Event>,
}

impl Studio {
	fn new(schema: u32) -> Self {
		let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
		let port = listener.local_addr().unwrap().port();
		let (tx, events) = mpsc::sync_channel(MAX_EVENTS);
		let folder = std::env::temp_dir().join("meshvault_exporter_tests_never_made");
		std::thread::spawn(move || {
			serve(listener, tx, Exports::new(folder), egui::Context::default())
		});

		let lua = Lua::new();
		let forward = lua
			.create_function(move |lua, request: Table| {
				let (status, body) = forward(port, &request);
				let reply = lua.create_table()?;
				reply.set("Success", (200..300).contains(&status))?;
				reply.set("StatusCode", status)?;
				reply.set("Body", body)?;
				Ok(reply)
			})
			.unwrap();
		lua.globals().set("forward", forward).unwrap();
		lua.load(STUDIO).set_name("studio").exec().unwrap();
		let entry: Function = lua
			.load(exporter(schema).unwrap())
			.set_name(format!("exporter_v{schema}"))
			.eval()
			.unwrap_or_else(|e| panic!("schema {schema}: {e}"));
		Self { lua, entry, events }
	}

	/// The three things the exporter returns, given `options` (Luau).
	fn export(&self, options: &str) -> (Table, Value, Function) {
		let options: Value = self.lua.load(options).eval().unwrap();
		let out: MultiValue = self.entry.call(options).unwrap();
		let out: Vec<Value> = out.into_iter().collect();
		let [Value::Table(issues), export, Value::Function(send)] = &out[..] else {
			panic!("the exporter returns {out:?}, not issues, export and send");
		};
		(issues.clone(), export.clone(), send.clone())
	}

	/// Requests made so far.
	fn posts(&self) -> usize {
		self.global::<Table>("posts").raw_len()
	}

	/// The headers of request `n` (from 1).
	fn headers(&self, n: usize) -> Table {
		let post: Table = self.global::<Table>("posts").get(n).unwrap();
		post.get("Headers").unwrap()
	}

	fn global<T: mlua::FromLua>(&self, name: &str) -> T {
		self.lua.globals().get(name).unwrap()
	}

	/// A returned export as the JSON HttpService makes of it.
	fn document(&self, export: &Value) -> String {
		let http: Table = self.global("HttpService");
		let encode: Function = http.get("JSONEncode").unwrap();
		encode.call((http, export)).unwrap()
	}

	/// The next export the link received.
	fn received(&self) -> String {
		loop {
			match self.events.recv_timeout(Duration::from_secs(10)) {
				Ok(Event::Received(r)) => return String::from_utf8(r.json).unwrap(),
				Ok(Event::Progress { .. }) => continue,
				Err(e) => panic!("no export arrived: {e}"),
			}
		}
	}
}

/// Posts a request the exporter made to the link at `port`, giving the
/// status and body of its reply.
fn forward(port: u16, request: &Table) -> (u16, String) {
	assert_eq!(
		request.get::<String>("Url").unwrap(),
		format!("http://localhost:{PORT}/export")
	);
	let method: String = request.get("Method").unwrap();
	let body: mlua::LuaString = request.get("Body").unwrap();
	let body = body.as_bytes().to_vec();
	let mut head = format!("{method} /export HTTP/1.1\r\nHost: localhost:{port}\r\n");
	for pair in request
		.get::<Table>("Headers")
		.unwrap()
		.pairs::<String, String>()
	{
		let (k, v) = pair.unwrap();
		head.push_str(&format!("{k}: {v}\r\n"));
	}
	head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
	let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
	s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
	s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
	s.write_all(head.as_bytes()).unwrap();
	s.write_all(&body).unwrap();
	let mut reply = String::new();
	s.read_to_string(&mut reply).unwrap();
	let status = reply[9..12].parse().unwrap();
	let body = reply.split_once("\r\n\r\n").map_or("", |(_, b)| b);
	(status, body.to_string())
}

/// The meshes of an export from the stand-in Studio as Meshvault reads it:
/// (name, id, folder below the game). Schema 2 names the place it came
/// from, and its game heads every folder.
fn meshes(json: &str, schema: u32) -> Vec<(String, u64, String)> {
	let origin = match schema {
		1 => export_file::Origin::default(),
		_ => export_file::Origin {
			game: Some("Place".into()),
			place_id: Some(4242),
		},
	};
	meshes_from(json, schema, &origin)
}

/// The meshes of an export from `origin` as Meshvault reads it, as for
/// [`meshes`].
fn meshes_from(
	json: &str,
	schema: u32,
	origin: &export_file::Origin,
) -> Vec<(String, u64, String)> {
	let header: serde_json::Value = serde_json::from_str(json).unwrap();
	assert_eq!(header["SchemaVersion"], schema, "{json}");
	let export = export_file::parse(json).unwrap();
	assert!(
		export.models.is_empty() && export.skipped.is_empty(),
		"{json}"
	);
	assert_eq!(&export.origin, origin, "{json}");
	export
		.entries
		.into_iter()
		.map(|e| {
			let below = &e.folder[usize::from(origin.game.is_some())..];
			(e.name, e.mesh_id, below.join("/"))
		})
		.collect()
}

fn rock() -> (String, u64, String) {
	("Rock".into(), 123, "ReplicatedStorage".into())
}

/// The problems, as (kind, message).
fn issues(issues: &Table) -> Vec<(String, String)> {
	issues
		.sequence_values::<Table>()
		.map(|i| {
			let i = i.unwrap();
			(i.get("Kind").unwrap(), i.get("Message").unwrap())
		})
		.collect()
}

/// A returned export's meshes, as (MeshId, TextureId).
fn returned(export: &Value) -> Vec<(String, String)> {
	let Value::Table(export) = export else {
		panic!("the export is {export:?}");
	};
	export
		.get::<Table>("Meshes")
		.unwrap()
		.sequence_values::<Table>()
		.map(|m| {
			let m = m.unwrap();
			(m.get("MeshId").unwrap(), m.get("TextureId").unwrap())
		})
		.collect()
}

#[test]
fn exporters_compile_as_luau() {
	for v in SCHEMAS {
		Lua::new()
			.load(exporter(v).unwrap())
			.set_name(format!("exporter_v{v}"))
			.into_function()
			.unwrap_or_else(|e| panic!("schema {v}: {e}"));
	}
}

// Edit mode: `send = false` sends nothing and gives back the export, which
// can be changed, then sent; the sender sends what it is given, a table or
// JSON text, and the link receives it whole
#[test]
fn keeping_the_export_then_sending_it() {
	for v in SCHEMAS {
		let studio = Studio::new(v);
		let (problems, export, send) = studio.export("{ send = false, roots = { part } }");
		assert_eq!(issues(&problems), [], "schema {v}");
		assert_eq!(studio.posts(), 0, "schema {v}: sent with send = false");
		let export_schema: u32 = match &export {
			Value::Table(t) => t.get("SchemaVersion").unwrap(),
			other => panic!("schema {v}: the export is {other:?}"),
		};
		assert_eq!(export_schema, v);
		let rock_ids = ("rbxassetid://123".into(), "rbxassetid://456".into());
		assert_eq!(returned(&export), [rock_ids], "schema {v}");
		assert_eq!(meshes(&studio.document(&export), v), [rock()], "schema {v}");

		// change it as the code for Studio suggests, then send it
		let edit: Function = studio
			.lua
			.load(
				"return function(export) \
				 local m = table.clone(export.Meshes[1]); m.Name = 'Pebble'; \
				 export.Meshes = { m } end",
			)
			.eval()
			.unwrap();
		edit.call::<()>(&export).unwrap();
		let (sent, reply): (bool, String) = send.call(&export).unwrap();
		assert!(sent, "schema {v}: {reply}");
		assert!(reply.contains("saving it as"), "schema {v}: {reply}");
		let pebble = ("Pebble".to_string(), 123, "ReplicatedStorage".to_string());
		assert_eq!(meshes(&studio.received(), v), [pebble], "schema {v}");
		assert_eq!(studio.posts(), 1, "schema {v}");
		let headers = studio.headers(1);
		assert_eq!(headers.get::<String>("X-Meshvault-Chunk").unwrap(), "1/1");
		assert_eq!(headers.get::<String>("X-Meshvault-Name").unwrap(), "Place");
		let http: Table = studio.global("HttpService");
		assert!(!http.get::<bool>("HttpEnabled").unwrap(), "schema {v}");

		// another export table is sent as it is, not the one the sender came with
		let other: Value = studio
			.lua
			.load(format!(
				"return {{ SchemaVersion = {v}, Models = {{}}, Meshes = {{ {{ Id = 1, 				 Name = 'Other', MeshId = 'rbxassetid://321', TextureId = '', 				 Folder = 'Elsewhere', ColorMap = '', NormalMap = '', RoughnessMap = '', 				 MetalnessMap = '' }} }} }}"
			))
			.eval()
			.unwrap();
		let (sent, reply): (bool, String) = send.call(&other).unwrap();
		assert!(sent, "schema {v}: {reply}");
		let elsewhere = ("Other".to_string(), 321, "Elsewhere".to_string());
		// written by hand, it names no place
		let received = meshes_from(&studio.received(), v, &export_file::Origin::default());
		assert_eq!(received, [elsewhere], "schema {v}");

		// JSON text is sent as it is
		let text = format!(
			"{{\"SchemaVersion\":{v},\"Models\":[],\"Meshes\":[{{\"Id\":1,\"Name\":\"Text\",\
			 \"MeshId\":\"rbxassetid://789\",\"TextureId\":\"\",\"Folder\":\"Typed\",\
			 \"ColorMap\":\"\",\"NormalMap\":\"\",\"RoughnessMap\":\"\",\"MetalnessMap\":\"\"}}]}}"
		);
		let (sent, reply): (bool, String) = send.call(text.as_str()).unwrap();
		assert!(sent, "schema {v}: {reply}");
		assert_eq!(studio.received(), text, "schema {v}");

		// text over the 2 MB a request carries goes in pieces, which the
		// link joins back in order
		let long = format!("{text}{}", " ".repeat(2 * 1024 * 1024));
		let (sent, reply): (bool, String) = send.call(long.as_str()).unwrap();
		assert!(sent, "schema {v}: {reply}");
		assert_eq!(studio.received(), long, "schema {v}");
		assert_eq!(studio.posts(), 5, "schema {v}: two pieces");
	}
}

// Send mode, the default: with nothing selected the whole game except
// Workspace is exported and sent, and that export is returned; leaving
// nothing out takes Workspace too
#[test]
fn exporting_the_game_sends_it() {
	for v in SCHEMAS {
		let studio = Studio::new(v);
		let (problems, export, _) = studio.export("nil");
		assert_eq!(issues(&problems), [], "schema {v}");
		assert_eq!(meshes(&studio.received(), v), [rock()], "schema {v}");
		let rock_ids = ("rbxassetid://123".into(), "rbxassetid://456".into());
		assert_eq!(returned(&export), [rock_ids], "schema {v}");
		assert_eq!(meshes(&studio.document(&export), v), [rock()], "schema {v}");

		let studio = Studio::new(v);
		let (problems, _, _) = studio.export("{ exclude = {} }");
		assert_eq!(issues(&problems), [], "schema {v}");
		let mut all = meshes(&studio.received(), v);
		all.sort();
		let sentinel = ("Sentinel".to_string(), 999, "Workspace".to_string());
		assert_eq!(all, [rock(), sentinel], "schema {v}");
	}
}

// Meshvault refusing an export: the sender says so, and HttpEnabled goes
// back to what it was, here on
#[test]
fn a_refused_send_says_so() {
	for v in SCHEMAS {
		let studio = Studio::new(v);
		let http: Table = studio.global("HttpService");
		http.set("HttpEnabled", true).unwrap();
		let (_, export, send) = studio.export("{ send = false, roots = { part } }");
		studio.lua.globals().set("refuse", true).unwrap();
		let (sent, reply): (bool, String) = send.call(&export).unwrap();
		assert!(!sent, "schema {v}");
		assert!(reply.contains("HTTP 503"), "schema {v}: {reply}");
		assert!(http.get::<bool>("HttpEnabled").unwrap(), "schema {v}");
	}
}

// Meshvault not taking the export when the exporter sends it itself: the
// export is printed as chunks, which import as they are, so it is not lost
#[test]
fn a_failed_send_prints_the_export_to_import() {
	for v in SCHEMAS {
		let studio = Studio::new(v);
		studio.lua.globals().set("refuse", true).unwrap();
		let (problems, _, _) = studio.export("nil");
		let kinds: Vec<_> = issues(&problems).into_iter().map(|(k, _)| k).collect();
		assert_eq!(kinds, ["SendFailed"], "schema {v}");
		let printed: Vec<String> = studio
			.global::<Table>("printed")
			.sequence_values()
			.map(Result::unwrap)
			.collect();
		let start = printed
			.iter()
			.position(|l| l.starts_with("=== JSON CHUNK"))
			.unwrap_or_else(|| panic!("schema {v}: no chunks in {printed:?}"));
		let export = export_file::parse(&printed[start..].join(
			"
",
		))
		.unwrap();
		let names: Vec<_> = export
			.entries
			.iter()
			.map(|e| (e.name.as_str(), e.mesh_id))
			.collect();
		assert_eq!(names, [("Rock", 123)], "schema {v}");
	}
}

// Nothing to export, and an error part way: no export, a problem saying why,
// and nothing sent
#[test]
fn no_export_says_why() {
	for v in SCHEMAS {
		let studio = Studio::new(v);
		let (problems, export, _) = studio.export("{ roots = { empty } }");
		let [(kind, message)] = &issues(&problems)[..] else {
			panic!("schema {v}: {:?}", issues(&problems));
		};
		assert_eq!(kind, "NothingToExport");
		assert!(
			message.starts_with("nothing to export"),
			"schema {v}: {message}"
		);
		assert!(export.is_nil() && studio.posts() == 0, "schema {v}");

		let studio = Studio::new(v);
		studio.lua.globals().set("selectionFails", true).unwrap();
		let (problems, export, _) = studio.export("nil");
		let [(kind, message)] = &issues(&problems)[..] else {
			panic!("schema {v}: {:?}", issues(&problems));
		};
		assert_eq!(kind, "Error");
		assert!(message.contains("Selection broke"), "schema {v}: {message}");
		assert!(export.is_nil() && studio.posts() == 0, "schema {v}");
	}
}

// Schema 2 names the game by its published name, else the place's name in
// Studio, else its ID; the export and its requests say the same, and an
// unpublished place is not looked up
#[test]
fn the_game_is_named_by_its_published_name() {
	let named = |studio: &Studio| {
		let (problems, _, _) = studio.export("nil");
		assert_eq!(issues(&problems), []);
		let export = export_file::parse(&studio.received()).unwrap();
		let header: String = studio.headers(1).get("X-Meshvault-Name").unwrap();
		assert_eq!(export.origin.game.as_ref(), Some(&header));
		(header, export.origin.place_id)
	};

	let studio = Studio::new(2);
	studio
		.lua
		.globals()
		.set("productName", "Rock Game")
		.unwrap();
	assert_eq!(named(&studio), ("Rock Game".into(), Some(4242)));

	let studio = Studio::new(2);
	assert_eq!(named(&studio), ("Place".into(), Some(4242)));

	let studio = Studio::new(2);
	studio.lua.load("game.Name = ''").exec().unwrap();
	assert_eq!(named(&studio), ("game_4242".into(), Some(4242)));

	let studio = Studio::new(2);
	studio
		.lua
		.load("game.PlaceId = 0; productName = 'Unused'")
		.exec()
		.unwrap();
	assert_eq!(named(&studio), ("Place".into(), None));
	assert_eq!(studio.global::<u32>("productCalls"), 0);
}

// Every model with parts is exported, not only animated ones, once each as
// its outermost model below what was picked; parts outside any model are
// meshes only
#[test]
fn static_models_are_exported_whole() {
	let studio = Studio::new(2);
	studio
		.lua
		.load(
			r#"
			local function block(name, parent)
				return newInstance("MeshPart", name, parent, {
					MeshId = "rbxassetid://77", TextureID = "",
					CFrame = { GetComponents = function() return 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1 end },
					Size = { X = 1, Y = 1, Z = 1 }, Color = { R = 1, G = 1, B = 1 },
					BrickColor = { Name = "White" }, Material = { Name = "Plastic" },
					Transparency = 0, Reflectance = 0, MaterialVariant = "",
				})
			end
			local storage = part.Parent
			crate = newInstance("Model", "Crate", storage)
			block("Lid", crate)
			block("Hinge", newInstance("Model", "Hardware", crate))
			"#,
		)
		.exec()
		.unwrap();
	let (problems, _, _) = studio.export("nil");
	assert_eq!(issues(&problems), []);
	let export = export_file::parse(&studio.received()).unwrap();
	let models: Vec<_> = export
		.models
		.iter()
		.map(|m| (m.name.as_str(), m.parts.len()))
		.collect();
	assert_eq!(models, [("Crate", 2)]);

	// picked on its own, the inner model stands alone
	let studio_inner = studio
		.lua
		.load("return { roots = { crate.children[2] }, send = false }")
		.eval::<Value>()
		.unwrap();
	let out: MultiValue = studio.entry.call(studio_inner).unwrap();
	let Some(Value::Table(export)) = out.into_iter().nth(1) else {
		panic!("no export");
	};
	let names: Vec<String> = export
		.get::<Table>("Models")
		.unwrap()
		.sequence_values::<Table>()
		.map(|m| m.unwrap().get("Name").unwrap())
		.collect();
	assert_eq!(names, ["Hardware"]);
}
