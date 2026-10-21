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

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;

use crate::anim::schema::AnimatedModel;
use crate::content_id::parse_asset_id;
use crate::scene::maps::MapKind;

/// The newest `SchemaVersion` this build understands, and the one it writes.
pub const SCHEMA_VERSION: u32 = 2;

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

pub struct Export {
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
	#[serde(rename = "Meshes")]
	meshes: Vec<RawEntry>,
	#[serde(rename = "Models")]
	models: Vec<AnimatedModel>,
}

/// The Studio exporter prints long output as `=== JSON CHUNK i/n ===`
/// blocks; joins the chunk contents back together.
fn join_chunks(text: &str) -> Option<String> {
	if !text.trim_start().starts_with("=== JSON CHUNK") {
		return None;
	}
	let mut out = String::with_capacity(text.len());
	for line in text.lines() {
		let marker = line.trim();
		if marker.starts_with("=== JSON CHUNK") && marker.ends_with("===") {
			continue;
		}
		// chunks are split mid-text, so they are joined with nothing between
		out.push_str(line.trim_end_matches('\r'));
	}
	Some(out)
}

pub fn parse(text: &str) -> Result<Export, String> {
	let text = text.trim_start_matches('\u{feff}');
	let joined = join_chunks(text);
	let json = joined.as_deref().unwrap_or(text).trim_start();
	if json.starts_with('[') {
		let raw: Vec<RawEntry> =
			serde_json::from_str(json).map_err(|e| format!("Not a valid export file: {e}"))?;
		return Ok(convert(raw, Vec::new()));
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
	Ok(convert(doc.meshes, doc.models))
}

fn convert(raw: Vec<RawEntry>, models: Vec<AnimatedModel>) -> Export {
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
		let folder = r
			.folder
			.split('/')
			.map(str::trim)
			.filter(|s| !s.is_empty())
			.map(String::from)
			.collect();
		entries.push(Entry {
			name: r.name,
			folder,
			mesh_id,
			maps,
			source_id: r.id,
		});
	}
	Export {
		entries,
		models,
		skipped,
		warnings,
	}
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
		// a version 1 record leaves them all unset
		let v1 = parse(&json.replace("\"SchemaVersion\":2", "\"SchemaVersion\":1")).unwrap();
		assert_eq!(v1.models.len(), 1);
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
