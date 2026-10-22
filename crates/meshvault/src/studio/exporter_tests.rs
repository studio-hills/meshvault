//! Runs the exporters Meshvault serves, in Luau, against a small stand-in
//! for Roblox Studio: a place with one MeshPart and an HttpService that
//! records what it is asked to post. This checks what the code for Studio
//! relies on, whatever the exporters' insides look like: the script returns
//! a function of options, which returns the problems, the export and a
//! sender; `send = false` keeps the export here; and what the sender posts
//! is an export Meshvault reads.

use mlua::{Function, Lua, MultiValue, Table, Value};

use super::export_file;
use super::link::{PORT, SCHEMAS, exporter};

/// Just enough of Roblox for an export of loose meshes. Instances are tables
/// with a class, name, parent and children; HttpService encodes JSON as
/// Roblox does (whole numbers without a point, empty tables as `[]`) and
/// keeps each request in `posts`.
const STUDIO: &str = r#"
local classes = {
	MeshPart = { "MeshPart", "BasePart", "PVInstance", "Instance" },
	Folder = { "Folder", "Instance" },
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
output = {}
selectionFails = false
local http = { HttpEnabled = false }
function http:JSONEncode(v) return encode(v) end
function http:GenerateGUID() return "guid" end
function http:RequestAsync(request)
	assert(self.HttpEnabled, "HttpEnabled is off")
	table.insert(posts, request)
	return { Success = true, StatusCode = 200, Body = "saved" }
end
HttpService = http

game = Instance.new("DataModel", "Place")
workspaceService = Instance.new("Workspace", "Workspace", game)
storage = Instance.new("Folder", "ReplicatedStorage", game)
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
	MaterialService = Instance.new("MaterialService", "MaterialService"),
}
function game:GetService(name) return (assert(services[name], name)) end

Enum = { MeshType = { FileMesh = "FileMesh" } }
print = function(...) table.insert(output, table.concat({ ... }, " ")) end
warn = print
"#;

/// A fresh Luau with the stand-in Studio and the served exporter for
/// `schema`, giving the function the exporter returns.
fn load(schema: u32) -> (Lua, Function) {
	let lua = Lua::new();
	lua.load(STUDIO).set_name("studio").exec().unwrap();
	let source = exporter(schema).unwrap();
	let entry: Function = lua
		.load(&source)
		.set_name(format!("exporter_v{schema}"))
		.eval()
		.unwrap_or_else(|e| panic!("schema {schema}: {e}"));
	(lua, entry)
}

fn global<T: mlua::FromLua>(lua: &Lua, name: &str) -> T {
	lua.globals().get(name).unwrap()
}

/// The three things the exporter returns.
fn call(entry: &Function, options: Value) -> (Table, Value, Function) {
	let out: MultiValue = entry.call(options).unwrap();
	let out: Vec<Value> = out.into_iter().collect();
	let [Value::Table(issues), export, Value::Function(send)] = &out[..] else {
		panic!("the exporter returns {out:?}, not issues, export and send");
	};
	(issues.clone(), export.clone(), send.clone())
}

fn kinds(issues: &Table) -> Vec<String> {
	issues
		.sequence_values::<Table>()
		.map(|i| i.unwrap().get::<String>("Kind").unwrap())
		.collect()
}

/// The problems, with what each says, for failure messages.
fn said(issues: &Table) -> Vec<String> {
	issues
		.sequence_values::<Table>()
		.map(|i| i.unwrap().get::<String>("Message").unwrap())
		.collect()
}

/// The requests posted so far, as (headers, body).
fn posts(lua: &Lua) -> Vec<(Table, String)> {
	global::<Table>(lua, "posts")
		.sequence_values::<Table>()
		.map(|p| {
			let p = p.unwrap();
			assert_eq!(p.get::<String>("Method").unwrap(), "POST");
			assert_eq!(
				p.get::<String>("Url").unwrap(),
				format!("http://localhost:{PORT}/export")
			);
			(p.get("Headers").unwrap(), p.get("Body").unwrap())
		})
		.collect()
}

/// That a posted body is an export of the one mesh, of this schema.
fn reads_as_the_rock(body: &str, schema: u32) {
	let json: serde_json::Value = serde_json::from_str(body).unwrap();
	assert_eq!(json["SchemaVersion"], schema, "{body}");
	let export = export_file::parse(body).unwrap();
	let [rock] = &export.entries[..] else {
		panic!("schema {schema}: {body}");
	};
	assert_eq!((rock.name.as_str(), rock.mesh_id), ("Rock", 123));
	assert_eq!(rock.folder, ["ReplicatedStorage"]);
	assert!(export.models.is_empty() && export.skipped.is_empty());
}

#[test]
fn exporters_compile_as_luau() {
	for v in SCHEMAS {
		let source = exporter(v).unwrap();
		Lua::new()
			.load(&source)
			.set_name(format!("exporter_v{v}"))
			.into_function()
			.unwrap_or_else(|e| panic!("schema {v}: {e}"));
	}
}

// Edit mode: `send = false` sends nothing and gives back the export, which
// the returned sender then posts as JSON Meshvault reads
#[test]
fn keeping_the_export_then_sending_it() {
	for v in SCHEMAS {
		let (lua, entry) = load(v);
		let options = lua.create_table().unwrap();
		options.set("send", false).unwrap();
		options
			.set(
				"roots",
				lua.create_sequence_from([global::<Table>(&lua, "part")])
					.unwrap(),
			)
			.unwrap();
		let (issues, export, send) = call(&entry, Value::Table(options));
		assert!(kinds(&issues).is_empty(), "schema {v}: {:?}", said(&issues));
		assert!(posts(&lua).is_empty(), "schema {v}: sent with send = false");
		let Value::Table(export) = export else {
			panic!("schema {v}: the export is {export:?}");
		};
		assert_eq!(export.get::<u32>("SchemaVersion").unwrap(), v);
		let meshes: Table = export.get("Meshes").unwrap();
		assert_eq!(meshes.raw_len(), 1, "schema {v}");
		let mesh: Table = meshes.get(1).unwrap();
		assert_eq!(mesh.get::<String>("MeshId").unwrap(), "rbxassetid://123");

		let (sent, reply): (bool, String) = send.call(export).unwrap();
		assert!(sent, "schema {v}: {reply}");
		assert_eq!(reply, "saved");
		let [(headers, body)] = &posts(&lua)[..] else {
			panic!("schema {v}: one request expected");
		};
		assert_eq!(headers.get::<String>("X-Meshvault-Chunk").unwrap(), "1/1");
		assert_eq!(headers.get::<String>("X-Meshvault-Name").unwrap(), "Place");
		reads_as_the_rock(body, v);
		let http: Table = global(&lua, "HttpService");
		assert!(
			!http.get::<bool>("HttpEnabled").unwrap(),
			"schema {v}: HttpEnabled left on"
		);
	}
}

// Send mode, the default: with nothing selected the whole game outside
// Workspace is exported and sent, and the export is still returned
#[test]
fn exporting_the_game_sends_it() {
	for v in SCHEMAS {
		let (lua, entry) = load(v);
		let (issues, export, _send) = call(&entry, Value::Nil);
		assert!(kinds(&issues).is_empty(), "schema {v}: {:?}", said(&issues));
		let [(_, body)] = &posts(&lua)[..] else {
			panic!("schema {v}: one request expected");
		};
		reads_as_the_rock(body, v);
		let Value::Table(export) = export else {
			panic!("schema {v}: the export is {export:?}");
		};
		assert_eq!(export.get::<u32>("SchemaVersion").unwrap(), v);
	}
}

// Nothing to export, and an error part way: no export, a problem saying why,
// and a sender all the same
#[test]
fn no_export_says_why() {
	for v in SCHEMAS {
		let (lua, entry) = load(v);
		let options = lua.create_table().unwrap();
		options
			.set(
				"roots",
				lua.create_sequence_from([global::<Table>(&lua, "empty")])
					.unwrap(),
			)
			.unwrap();
		let (issues, export, _) = call(&entry, Value::Table(options));
		assert_eq!(kinds(&issues), ["NothingToExport"], "schema {v}");
		assert!(export.is_nil() && posts(&lua).is_empty(), "schema {v}");

		let (lua, entry) = load(v);
		lua.globals().set("selectionFails", true).unwrap();
		let (issues, export, _) = call(&entry, Value::Nil);
		assert_eq!(kinds(&issues), ["Error"], "schema {v}");
		assert!(export.is_nil() && posts(&lua).is_empty(), "schema {v}");
	}
}
