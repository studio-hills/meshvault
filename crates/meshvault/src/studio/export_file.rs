//! Parsing export files. Two layouts are understood:
//!
//! - the original array (schema 0):
//!   `[{Name, MeshId, Folder, TextureId?, ColorMap?, NormalMap?, RoughnessMap?, MetalnessMap?}, ...]`
//! - schema versions 1 and 2: `{SchemaVersion, Meshes: [{Id, ...the same fields}], Models: [...]}`,
//!   where `Models` holds rigs and animation clips that refer to meshes by `Id`.
//!   Version 2 adds model scale, part appearance, SpecialMesh and joint
//!   transforms; both read into the same types.
//!
//! Missing fields and empty strings both mean "none".
//!
//! Schema 2 exports also name the place they came from (`GameName`,
//! `PlaceId`): the game becomes the top folder of everything imported from
//! it, and its place is sent with the downloads.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Deserialize;

use crate::anim::schema::{AnimatedModel, AnimationClip, CurveData, Pose};
use crate::content_id::parse_asset_id;
use crate::scene::maps::MapKind;

/// The newest `SchemaVersion` this build understands, and the one it writes.
pub const SCHEMA_VERSION: u32 = 2;

/// The most folders an entry or model can be inside, its game included.
/// Real exports are a dozen deep at most; the folder tree is drawn
/// recursively, so a deeper one could overflow the stack.
pub const MAX_FOLDER_DEPTH: usize = 64;
/// The longest folder name, in characters.
pub const MAX_FOLDER_PART: usize = 200;
/// The longest folder path, in characters, `/`s included.
pub const MAX_FOLDER_LEN: usize = 2000;

/// The largest export read, from Studio or from disk.
pub const MAX_EXPORT_BYTES: usize = 2 * 1024 * 1024 * 1024;

/// Reads `path` whole, unless it is longer than `limit` bytes.
pub fn read_limited(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
	use std::io::Read;
	let too_big = |len: u64| {
		std::io::Error::other(format!(
			"it is {}, larger than the {} Meshvault reads",
			crate::ui::format::format_bytes(usize::try_from(len).unwrap_or(usize::MAX)),
			crate::ui::format::format_bytes(limit)
		))
	};
	let file = std::fs::File::open(path)?;
	let len = file.metadata()?.len();
	if len > limit as u64 {
		return Err(too_big(len));
	}
	let mut bytes = Vec::with_capacity(len as usize);
	// the file may grow after its size was read
	file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
	if bytes.len() > limit {
		return Err(too_big(bytes.len() as u64));
	}
	Ok(bytes)
}

/// Reads an export, a rig file or a workspace manifest whole, unless it is
/// larger than [`MAX_EXPORT_BYTES`]: the largest file Meshvault reads.
pub fn read_text(path: &Path) -> Result<String, String> {
	let bytes = read_limited(path, MAX_EXPORT_BYTES)
		.map_err(|e| format!("Could not read {}: {e}", path.display()))?;
	String::from_utf8(bytes)
		.map_err(|_| format!("Could not read {}: it is not UTF-8 text", path.display()))
}

/// The most of each thing an export may hold. Real exports have thousands
/// of meshes and some hundred models; these only stop a document that
/// would take far too long to show or import.
pub struct Limits {
	pub meshes: usize,
	pub models: usize,
	/// Parts in one model.
	pub parts: usize,
	/// Animations of one model.
	pub clips: usize,
	/// Keyframes, poses and curve keys of one animation.
	pub keys: usize,
}

pub const LIMITS: Limits = Limits {
	meshes: 1_000_000,
	models: 100_000,
	parts: 100_000,
	clips: 10_000,
	keys: 5_000_000,
};

/// Why an export of `len` bytes is not read.
pub fn too_big(len: usize) -> String {
	format!(
		"The export is {}, larger than the {} Meshvault reads.",
		crate::ui::format::format_bytes(len),
		crate::ui::format::format_bytes(MAX_EXPORT_BYTES)
	)
}

/// Why an export holds more than `limits` allow.
fn over_limits(meshes: usize, models: &[AnimatedModel], limits: &Limits) -> Option<String> {
	let over = |count: usize, limit: usize, what: &str| {
		(count > limit).then(|| {
			format!("This export has {count} {what}, more than the {limit} Meshvault reads.")
		})
	};
	over(meshes, limits.meshes, "meshes")
		.or_else(|| over(models.len(), limits.models, "models"))
		.or_else(|| {
			models.iter().find_map(|m| {
				over(m.parts.len(), limits.parts, "parts in one model")
					.or_else(|| over(m.animations.len(), limits.clips, "animations in one model"))
					.or_else(|| {
						m.animations
							.iter()
							.find_map(|a| over(clip_keys(a), limits.keys, "keys in one animation"))
					})
			})
		})
}

/// Keyframes, poses at every depth, and curve keys in `clip`.
fn clip_keys(clip: &AnimationClip) -> usize {
	let mut count = clip.keyframes.len();
	let mut poses: Vec<&Pose> = clip.keyframes.iter().flat_map(|k| &k.poses).collect();
	while let Some(pose) = poses.pop() {
		count += 1;
		poses.extend(&pose.children);
	}
	for track in &clip.curves {
		count += match &track.curve {
			CurveData::FloatCurve { keys } => keys.len(),
			CurveData::RotationCurve { keys } => keys.len(),
			CurveData::Vector3Curve { channels }
			| CurveData::EulerRotationCurve { channels, .. } => [&channels.x, &channels.y, &channels.z]
				.into_iter()
				.flatten()
				.map(Vec::len)
				.sum(),
			CurveData::Unknown => 0,
		};
	}
	count
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawEntry {
	/// `Meshes[].Id` in schemas 1 and 2.
	#[serde(default)]
	id: Option<u32>,
	name: String,
	mesh_id: String,
	#[serde(default)]
	folder: String,
	#[serde(default)]
	texture_id: Option<String>,
	#[serde(default)]
	color_map: Option<String>,
	#[serde(default)]
	normal_map: Option<String>,
	#[serde(default)]
	roughness_map: Option<String>,
	#[serde(default)]
	metalness_map: Option<String>,
}

impl RawEntry {
	fn map(&self, kind: MapKind) -> Option<&str> {
		match kind {
			MapKind::Texture => self.texture_id.as_deref(),
			MapKind::Color => self.color_map.as_deref(),
			MapKind::Normal => self.normal_map.as_deref(),
			MapKind::Roughness => self.roughness_map.as_deref(),
			MapKind::Metalness => self.metalness_map.as_deref(),
		}
		.filter(|s| !s.trim().is_empty())
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
	pub name: String,
	/// Folder path components, e.g. `["Zone_10", "8: CrystalScorpion"]`.
	pub folder: Vec<String>,
	pub mesh_id: u64,
	/// Asset id of each map this model references.
	pub maps: BTreeMap<MapKind, u64>,
	/// `Meshes[].Id` in a schema 1 or 2 export, which `Parts[].MeshRef` refers to.
	pub source_id: Option<u32>,
}

/// Where an export came from: the place it was exported from, in schema 2.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origin {
	/// `game.Name`; the top folder of everything imported from it.
	pub game: Option<String>,
	/// `game.PlaceId`, sent as `Roblox-Place-Id` with its downloads. An
	/// unpublished place's 0 is `None`.
	pub place_id: Option<u64>,
}

pub struct Export {
	/// Where it came from; entries' folders already start with its game.
	pub origin: Origin,
	pub entries: Vec<Entry>,
	/// Animated models (schemas 1 and 2).
	pub models: Vec<AnimatedModel>,
	/// Entries that were skipped, with the reason.
	pub skipped: Vec<String>,
	/// Map ids that could not be understood and were ignored.
	pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct Header {
	#[serde(rename = "SchemaVersion")]
	version: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Document {
	#[serde(rename = "GameName", default)]
	game_name: Option<String>,
	#[serde(rename = "PlaceId", default)]
	place_id: Option<u64>,
	#[serde(rename = "Meshes")]
	meshes: Vec<RawEntry>,
	#[serde(rename = "Models")]
	models: Vec<AnimatedModel>,
}

/// The Studio exporter prints long output as `=== JSON CHUNK i/n ===`
/// blocks, then `=== END JSON EXPORT ===`; joins the chunk contents back
/// together.
fn join_chunks(text: &str) -> Option<String> {
	if !text.trim_start().starts_with("=== JSON CHUNK") {
		return None;
	}
	let mut out = String::with_capacity(text.len());
	for line in text.lines() {
		let marker = line.trim();
		// the exporter prints the end marker after the last chunk, and users
		// copy it with the rest
		if (marker.starts_with("=== JSON CHUNK") && marker.ends_with("==="))
			|| marker == "=== END JSON EXPORT ==="
		{
			continue;
		}
		// chunks are split mid-text, so they are joined with nothing between
		out.push_str(line.trim_end_matches('\r'));
	}
	Some(out)
}

pub fn parse(text: &str) -> Result<Export, String> {
	parse_with(text, &LIMITS)
}

/// [`parse`], allowing at most `limits`.
fn parse_with(text: &str, limits: &Limits) -> Result<Export, String> {
	if text.len() > MAX_EXPORT_BYTES {
		return Err(too_big(text.len()));
	}
	let text = text.trim_start_matches('\u{feff}');
	let joined = join_chunks(text);
	let json = joined.as_deref().unwrap_or(text).trim_start();
	if json.starts_with('[') {
		let raw: Vec<RawEntry> =
			serde_json::from_str(json).map_err(|e| format!("Not a valid export file: {e}"))?;
		if let Some(why) = over_limits(raw.len(), &[], limits) {
			return Err(why);
		}
		return Ok(convert(raw, Vec::new(), Origin::default()));
	}
	let header: Header =
		serde_json::from_str(json).map_err(|e| format!("Not a valid export file: {e}"))?;
	let version = match &header.version {
		None => return Err("Not a valid export file: SchemaVersion is missing".into()),
		Some(v) => v
			.as_u64()
			.ok_or_else(|| format!("Unrecognised SchemaVersion {v}"))?,
	};
	if !(1..=u64::from(SCHEMA_VERSION)).contains(&version) {
		return Err(format!(
			"This export uses schema version {version}, which this version of Meshvault cannot read (it reads versions 0 to {SCHEMA_VERSION})."
		));
	}
	let doc: Document = serde_json::from_str(json)
		.map_err(|e| format!("Not a valid schema {version} export: {e}"))?;
	if let Some(why) = over_limits(doc.meshes.len(), &doc.models, limits) {
		return Err(why);
	}
	let origin = Origin {
		game: doc
			.game_name
			.map(|g| g.trim().to_owned())
			.filter(|g| !g.is_empty()),
		place_id: doc.place_id.filter(|&id| id != 0),
	};
	Ok(convert(doc.meshes, doc.models, origin))
}

fn convert(raw: Vec<RawEntry>, models: Vec<AnimatedModel>, origin: Origin) -> Export {
	let mut entries = Vec::with_capacity(raw.len());
	let mut skipped = Vec::new();
	let mut warnings = Vec::new();
	let mut ids = HashSet::new();
	for r in raw {
		if let Some(id) = r.id
			&& !ids.insert(id)
		{
			skipped.push(format!("{}: duplicate mesh Id {id}", r.name));
			continue;
		}
		let Some(mesh_id) = parse_asset_id(&r.mesh_id) else {
			skipped.push(format!("{}: unrecognised MeshId {:?}", r.name, r.mesh_id));
			continue;
		};
		let mut maps = BTreeMap::new();
		for kind in MapKind::ALL {
			let Some(value) = r.map(kind) else { continue };
			match parse_asset_id(value) {
				Some(id) => {
					maps.insert(kind, id);
				}
				None => warnings.push(format!(
					"{}: ignored unrecognised {} {value:?}",
					r.name,
					kind.export_field()
				)),
			}
		}
		let folder: Vec<String> = origin
			.game
			.iter()
			.cloned()
			.chain(
				r.folder
					.split('/')
					.map(str::trim)
					.filter(|s| !s.is_empty())
					.map(String::from),
			)
			.collect();
		if let Some(why) = folder_problem(&folder) {
			skipped.push(format!("{}: {why}", r.name));
			continue;
		}
		entries.push(Entry {
			name: r.name,
			folder,
			mesh_id,
			maps,
			source_id: r.id,
		});
	}
	// a model shows in the folder holding it (`workspace::model_folder`)
	let models = models
		.into_iter()
		.filter(|m| {
			let path: Vec<&str> = origin
				.game
				.iter()
				.map(String::as_str)
				.chain(m.path.split('/').filter(|s| !s.is_empty()))
				.collect();
			let why = path
				.split_last()
				.and_then(|(_, folder)| folder_problem(folder));
			if let Some(why) = &why {
				skipped.push(format!("{}: {why}", m.name));
			}
			why.is_none()
		})
		.collect();
	Export {
		origin,
		entries,
		models,
		skipped,
		warnings,
	}
}

/// Why a folder path cannot be imported: too deep or too long for the
/// folder tree and for paths on disk.
fn folder_problem(folder: &[impl AsRef<str>]) -> Option<String> {
	if folder.len() > MAX_FOLDER_DEPTH {
		return Some(format!(
			"its folder is {} levels deep, more than the {MAX_FOLDER_DEPTH} allowed",
			folder.len()
		));
	}
	// the `/`s between the names
	let mut len = folder.len().saturating_sub(1);
	for part in folder {
		let chars = part.as_ref().chars().count();
		if chars > MAX_FOLDER_PART {
			return Some(format!(
				"a folder name is {chars} characters long, more than the {MAX_FOLDER_PART} allowed"
			));
		}
		len += chars;
	}
	(len > MAX_FOLDER_LEN).then(|| {
		format!("its folder path is {len} characters long, more than the {MAX_FOLDER_LEN} allowed")
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Checks a real export without committing it:
	/// `MESHVAULT_EXPORT=path/to/export.json cargo test -p meshvault -- --ignored parses_real_export`
	#[test]
	#[ignore]
	fn parses_real_export() {
		let path = crate::test_support::real_export();
		let export = parse(&std::fs::read_to_string(path).unwrap()).unwrap();
		assert!(export.skipped.is_empty(), "skipped: {:?}", export.skipped);
		let tree =
			crate::ui::tree::FolderTree::build(export.entries.iter().map(|e| e.folder.as_slice()));
		assert_eq!(tree.all_items().len(), export.entries.len());
		assert!(
			export.warnings.is_empty(),
			"warnings: {:?}",
			export.warnings
		);
		let count = |kind| {
			export
				.entries
				.iter()
				.filter(|e| e.maps.contains_key(&kind))
				.count()
		};
		println!(
			"{} entries in {} top-level folders; maps: {}",
			export.entries.len(),
			tree.folders.len(),
			MapKind::ALL
				.iter()
				.map(|&k| format!("{} {}", k.label(), count(k)))
				.collect::<Vec<_>>()
				.join(", ")
		);
	}

	#[test]
	fn parses_export() {
		let json = r#"[
			{"Folder":"Zone_10/8: CrystalScorpion","MeshId":"rbxassetid://1","Name":"Body"},
			{"Folder":"Eggs/Angel Egg","MeshId":"http://www.roblox.com/asset/?id=2 ","Name":"Egg","TextureId":"rbxassetid://3"},
			{"Folder":"","MeshId":"rbxassetid://4","Name":"Root item","TextureId":""},
			{"Folder":"X","MeshId":"","Name":"Broken"}
		]"#;
		let export = parse(json).unwrap();
		assert_eq!(export.entries.len(), 3);
		assert_eq!(export.skipped.len(), 1);
		assert_eq!(export.entries[0].folder, ["Zone_10", "8: CrystalScorpion"]);
		assert_eq!(export.entries[1].mesh_id, 2);
		assert_eq!(
			export.entries[1].maps,
			BTreeMap::from([(MapKind::Texture, 3)])
		);
		assert!(export.entries[2].folder.is_empty());
		assert!(export.entries[2].maps.is_empty());
	}

	#[test]
	fn parses_schema_1() {
		let json = r#"{"SchemaVersion":1,
			"Meshes":[
				{"Id":1,"Name":"Torso","MeshId":"rbxassetid://10","TextureId":"","Folder":"Workspace/Wolf",
				 "ColorMap":"rbxassetid://11","NormalMap":"","RoughnessMap":"","MetalnessMap":""},
				{"Id":2,"Name":"Head","MeshId":"","TextureId":"","Folder":"Workspace/Wolf",
				 "ColorMap":"","NormalMap":"","RoughnessMap":"","MetalnessMap":""}
			],
			"Models":[{"Name":"Wolf","Path":"Workspace/Wolf",
				"Parts":[{"Path":"Workspace/Wolf/Torso","Name":"Torso","MeshRef":1,
					"CFrame":[0,3,0,1,0,0,0,1,0,0,0,1],"Size":[2,2,1],"Transparency":0}],
				"Joints":[],"Bones":[],
				"Animations":[{"Name":"Idle","AnimationId":"rbxassetid://5","Status":"Unavailable",
					"Duration":0,"Loop":false,"Priority":"","Keyframes":[],"Curves":[]}]}]}"#;
		let export = parse(json).unwrap();
		assert_eq!(export.models.len(), 1);
		assert_eq!(export.entries.len(), 1);
		assert_eq!(export.entries[0].source_id, Some(1));
		assert_eq!(
			export.entries[0].maps,
			BTreeMap::from([(MapKind::Color, 11)])
		);
		assert_eq!(export.skipped.len(), 1);
		assert_eq!(export.models.len(), 1);
		assert_eq!(export.models[0].parts[0].mesh_ref, 1);
		assert_eq!(export.models[0].parts[0].cframe[1], 3.0);
	}

	/// Schema 2 adds model scale, part appearance, SpecialMesh and joint
	/// transforms; Roblox's encoder leaves out a nil SpecialMesh entirely.
	#[test]
	fn parses_schema_2() {
		let json = r#"{"SchemaVersion":2,
			"Meshes":[{"Id":1,"Name":"Body","MeshId":"rbxassetid://10","Folder":"RS/Wolf"}],
			"Models":[{"Name":"Wolf","Path":"RS/Wolf","Scale":0.25,
				"Parts":[
					{"Path":"RS/Wolf/Body","Name":"Body","MeshRef":1,"ClassName":"MeshPart","Shape":"",
					 "CFrame":[0,0,0,1,0,0,0,1,0,0,0,1],"Size":[1,1,1],"Color":[0.6,0.4,0.25],
					 "BrickColor":"Reddish brown","Material":"Plastic","Transparency":0,"Reflectance":0},
					{"Path":"RS/Wolf/Head","Name":"Head","MeshRef":0,"ClassName":"Part","Shape":"Block",
					 "CFrame":[0,1,0,1,0,0,0,1,0,0,0,1],"Size":[2,1,1],"Color":[1,1,1],
					 "BrickColor":"White","Material":"Plastic","Transparency":0,"Reflectance":0.5,
					 "SpecialMesh":{"MeshType":"Head","Scale":[1.25,1.25,1.25],"Offset":[0,0.1,0]}}],
				"Joints":[{"Name":"Neck","Path":"RS/Wolf/Head/Neck","Part0":"RS/Wolf/Body","Part1":"RS/Wolf/Head",
					"C0":[0,0.5,0,1,0,0,0,1,0,0,0,1],"C1":[0,-0.5,0,1,0,0,0,1,0,0,0,1],
					"Transform":[0,0,0,1,0,0,0,0,-1,0,1,0]}],
				"Bones":[],"Animations":[]}]}"#;
		let export = parse(json).unwrap();
		let m = &export.models[0];
		assert_eq!(m.scale, Some(0.25));
		assert_eq!(m.parts[0].class_name.as_deref(), Some("MeshPart"));
		assert_eq!(m.parts[0].color, Some([0.6, 0.4, 0.25]));
		assert!(m.parts[0].special_mesh.is_none());
		let special = m.parts[1].special_mesh.as_ref().unwrap();
		assert_eq!(special.mesh_type, "Head");
		assert_eq!(special.offset, [0.0, 0.1, 0.0]);
		assert_eq!(m.joints[0].transform.unwrap()[8], -1.0);
		// both versions are read into the same records, so a version 1 header
		// with these fields still keeps them
		let v1 = parse(&json.replace("\"SchemaVersion\":2", "\"SchemaVersion\":1")).unwrap();
		assert_eq!(v1.models.len(), 1);
		assert_eq!(v1.models[0].scale, Some(0.25));
	}

	/// A schema 2 export names its place: the game heads every folder, and
	/// an unpublished place (0) has no place id.
	#[test]
	fn the_place_heads_the_folders() {
		let json = |game: &str, place: u64| {
			format!(
				r#"{{"SchemaVersion":2,"GameName":"{game}","PlaceId":{place},
				"Meshes":[{{"Id":1,"Name":"Body","MeshId":"rbxassetid://10","Folder":"RS/Wolf"}}],
				"Models":[]}}"#
			)
		};
		let export = parse(&json(" Pet Sim ", 1234)).unwrap();
		assert_eq!(
			export.origin,
			Origin {
				game: Some("Pet Sim".into()),
				place_id: Some(1234),
			}
		);
		assert_eq!(export.entries[0].folder, ["Pet Sim", "RS", "Wolf"]);
		let unpublished = parse(&json("Place1", 0)).unwrap();
		assert_eq!(unpublished.origin.place_id, None);
		// an empty name, or none (older exports), adds no folder
		let nameless = parse(&json("", 5)).unwrap();
		assert_eq!(nameless.origin.game, None);
		assert_eq!(nameless.entries[0].folder, ["RS", "Wolf"]);
		let older = parse(r#"{"SchemaVersion":1,"Meshes":[{"Id":1,"Name":"B","MeshId":"rbxassetid://1","Folder":"X"}],"Models":[]}"#).unwrap();
		assert_eq!(older.origin, Origin::default());
		assert_eq!(older.entries[0].folder, ["X"]);
	}

	/// A short document can name a folder tens of thousands of levels deep;
	/// such entries and models are skipped, saying why, and the rest kept.
	#[test]
	fn skips_folders_too_deep_or_long() {
		let deep = "a/".repeat(40_000);
		let long = "x".repeat(MAX_FOLDER_PART + 1);
		let wide =
			vec!["y".repeat(MAX_FOLDER_PART); MAX_FOLDER_LEN / MAX_FOLDER_PART + 1].join("/");
		let ok = vec!["z"; MAX_FOLDER_DEPTH - 1].join("/");
		let json = serde_json::json!({
			"SchemaVersion": 2,
			"GameName": "G",
			"Meshes": [
				{"Id": 1, "Name": "Deep", "MeshId": "rbxassetid://1", "Folder": deep},
				{"Id": 2, "Name": "Long", "MeshId": "rbxassetid://2", "Folder": long},
				{"Id": 3, "Name": "Wide", "MeshId": "rbxassetid://3", "Folder": wide},
				{"Id": 4, "Name": "Fine", "MeshId": "rbxassetid://4", "Folder": ok},
			],
			"Models": [
				{"Name": "DeepRig", "Path": format!("{deep}DeepRig")},
				{"Name": "Rig", "Path": "Workspace/Rig"},
			],
		});
		let export = parse(&json.to_string()).unwrap();
		let names: Vec<_> = export.entries.iter().map(|e| e.name.as_str()).collect();
		assert_eq!(names, ["Fine"]);
		assert_eq!(
			export.entries[0].folder.len(),
			MAX_FOLDER_DEPTH,
			"the game counts"
		);
		assert_eq!(export.models.len(), 1);
		assert_eq!(export.models[0].name, "Rig");
		assert_eq!(export.skipped.len(), 4, "{:?}", export.skipped);
		assert!(
			export.skipped.iter().all(|s| s.len() < 200),
			"{:?}",
			export.skipped
		);
	}

	/// More meshes, models, parts, clips or keys than Meshvault reads is an
	/// error naming the count; the real limits are checked the same way.
	#[test]
	fn rejects_exports_over_the_counts() {
		let tiny = Limits {
			meshes: 2,
			models: 1,
			parts: 1,
			clips: 1,
			keys: 3,
		};
		let mesh = r#"{"Id":1,"Name":"B","MeshId":"rbxassetid://1"}"#;
		let doc = |meshes: usize, models: &str| {
			format!(
				r#"{{"SchemaVersion":2,"Meshes":[{}],"Models":[{models}]}}"#,
				vec![mesh; meshes].join(",")
			)
		};
		let check = |json: &str| parse_with(json, &tiny).err();
		assert_eq!(check(&doc(2, "")), None);
		let too_many = check(&doc(3, "")).unwrap();
		assert!(too_many.contains("3 meshes"), "{too_many}");
		// schema 0 is counted too
		let old = format!("[{}]", [mesh; 3].join(","));
		assert!(check(&old).unwrap().contains("3 meshes"));
		assert!(check(&doc(0, r#"{},{}"#)).unwrap().contains("2 models"));
		assert!(
			check(&doc(0, r#"{"Parts":[{},{}]}"#))
				.unwrap()
				.contains("2 parts")
		);
		assert!(
			check(&doc(0, r#"{"Animations":[{},{}]}"#))
				.unwrap()
				.contains("2 animations")
		);
		// keyframes, their poses at every depth, and curve keys all count
		let keys = r#"{"Animations":[{"Keyframes":[{"Poses":[{"Children":[{}]}]}],
			"Curves":[{"Curve":{"Type":"FloatCurve","Keys":[{}]}}]}]}"#;
		assert!(check(&doc(0, keys)).unwrap().contains("4 keys"));
		let fits = r#"{"Animations":[{"Keyframes":[{"Poses":[{}]}],
			"Curves":[{"Curve":{"Type":"Vector3Curve","Channels":{"X":[{}]}}}]}]}"#;
		assert_eq!(check(&doc(0, fits)), None);
	}

	#[test]
	fn rejects_unknown_schema() {
		let err = parse(r#"{"SchemaVersion":3,"Meshes":[],"Models":[]}"#)
			.err()
			.unwrap();
		assert!(err.contains("schema version 3"), "{err}");
		assert!(parse(r#"{"Meshes":[]}"#).is_err());
		assert!(parse(r#"{"SchemaVersion":1,"Meshes":[]}"#).is_err());
		assert!(parse(r#"{"SchemaVersion":1,"Meshes":[],"Models":[]}"#).is_ok());
	}

	#[test]
	fn joins_studio_chunks() {
		let text = "=== JSON CHUNK 1/2 ===\r\n{\"SchemaVersion\":1,\"Mes\r\n=== JSON CHUNK 2/2 ===\nhes\":[],\"Models\":[]}\n";
		assert!(parse(text).unwrap().models.is_empty());
		// the whole printed block, end marker included, as copied from the Output
		let copied = format!("{text}=== END JSON EXPORT ===\r\n");
		assert!(parse(&copied).unwrap().models.is_empty());
		// only that exact line is skipped; anything else after the chunks is still an error
		let other = format!("{text}=== END ===\n");
		assert!(parse(&other).is_err());
	}

	/// The v2 export: every key present, empty strings for missing maps.
	#[test]
	fn parses_surface_appearance_maps() {
		let json = r#"[
			{"Folder":"Eggs/Angel Egg","NormalMap":"","Name":"Demon Egg.013","MetalnessMap":"",
			 "RoughnessMap":"rbxassetid://119858836123090","MeshId":"rbxassetid://124515348153818",
			 "ColorMap":"rbxassetid://113080540026291","TextureId":"rbxassetid://113080540026291"},
			{"Folder":"X","NormalMap":"rbxassetid://88205193273398","Name":"Toad.001","MetalnessMap":"",
			 "RoughnessMap":"","MeshId":"rbxassetid://5","ColorMap":"","TextureId":""},
			{"Folder":"X","NormalMap":"oops","Name":"Bad map","MetalnessMap":"","RoughnessMap":"",
			 "MeshId":"rbxassetid://6","ColorMap":"","TextureId":""}
		]"#;
		let export = parse(json).unwrap();
		assert_eq!(export.entries.len(), 3);
		assert_eq!(
			export.entries[0].maps,
			BTreeMap::from([
				(MapKind::Texture, 113080540026291),
				(MapKind::Color, 113080540026291),
				(MapKind::Roughness, 119858836123090),
			])
		);
		assert_eq!(
			export.entries[1].maps,
			BTreeMap::from([(MapKind::Normal, 88205193273398)])
		);
		// a bad map id is reported but does not drop the model
		assert!(export.entries[2].maps.is_empty());
		assert_eq!(export.warnings.len(), 1);
	}
}
