//! Parsing the JSON export format:
//! `[{Name, MeshId, Folder, TextureId?, ColorMap?, NormalMap?, RoughnessMap?, MetalnessMap?}, ...]`.
//! Missing fields and empty strings both mean "none".

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::maps::MapKind;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawEntry {
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
}

pub struct Export {
	pub entries: Vec<Entry>,
	/// Entries that were skipped, with the reason.
	pub skipped: Vec<String>,
	/// Map ids that could not be understood and were ignored.
	pub warnings: Vec<String>,
}

/// Extracts the numeric asset id from the forms Roblox content ids take:
/// `rbxassetid://N`, `http(s)://www.roblox.com/asset/?id=N`,
/// `https://assetdelivery.roblox.com/v1/asset/?id=N`, or a bare `N`.
pub fn parse_asset_id(s: &str) -> Option<u64> {
	let s = s.trim();
	if s.is_empty() {
		return None;
	}
	if let Ok(id) = s.parse() {
		return Some(id);
	}
	if let Some(rest) = s.strip_prefix("rbxassetid://") {
		return rest.trim().parse().ok();
	}
	// any URL with an id= query parameter
	let query = s.split_once('?')?.1;
	query
		.split('&')
		.find_map(|kv| kv.strip_prefix("id=").or_else(|| kv.strip_prefix("ID=")))
		.and_then(|id| id.trim().parse().ok())
}

pub fn parse(json: &str) -> Result<Export, String> {
	let raw: Vec<RawEntry> =
		serde_json::from_str(json).map_err(|e| format!("Not a valid export file: {e}"))?;
	let mut entries = Vec::with_capacity(raw.len());
	let mut skipped = Vec::new();
	let mut warnings = Vec::new();
	for r in raw {
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
		});
	}
	Ok(Export {
		entries,
		skipped,
		warnings,
	})
}

/// Folder hierarchy over a list of entries, for tree views.
#[derive(Default)]
pub struct FolderTree {
	pub folders: BTreeMap<String, FolderTree>,
	/// Indices into the entry list.
	pub items: Vec<usize>,
}

impl FolderTree {
	pub fn build<'a>(folders: impl IntoIterator<Item = &'a [String]>) -> Self {
		let mut root = Self::default();
		for (i, path) in folders.into_iter().enumerate() {
			let mut node = &mut root;
			for part in path {
				node = node.folders.entry(part.clone()).or_default();
			}
			node.items.push(i);
		}
		root
	}

	/// Subfolders in display order: case-insensitive, with numbers compared
	/// by value so "2: Lizard" comes before "10: Wolf".
	pub fn sorted_folders(&self) -> Vec<(&String, &FolderTree)> {
		let mut folders: Vec<_> = self.folders.iter().collect();
		folders.sort_by(|a, b| natural_cmp(a.0, b.0));
		folders
	}

	/// Every entry index in this folder and below.
	pub fn all_items(&self) -> Vec<usize> {
		let mut out = self.items.clone();
		for child in self.folders.values() {
			out.extend(child.all_items());
		}
		out
	}
}

/// Case-insensitive comparison that orders runs of digits numerically.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
	use std::cmp::Ordering;
	let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
	loop {
		match (a.peek().copied(), b.peek().copied()) {
			(None, None) => return Ordering::Equal,
			(None, Some(_)) => return Ordering::Less,
			(Some(_), None) => return Ordering::Greater,
			(Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
				let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
					let mut digits = String::new();
					while let Some(c) = it.peek().filter(|c| c.is_ascii_digit()) {
						digits.push(*c);
						it.next();
					}
					digits
				};
				let (da, db) = (take(&mut a), take(&mut b));
				let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
				let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
				if ord != Ordering::Equal {
					return ord;
				}
			}
			(Some(x), Some(y)) => {
				let ord = x.to_lowercase().cmp(y.to_lowercase());
				if ord != Ordering::Equal {
					return ord;
				}
				a.next();
				b.next();
			}
		}
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn natural_order() {
		let mut v = vec![
			"10: Wolf",
			"monkisrokBoss",
			"2: Lizard",
			"Zone_10",
			"1: Bug",
			"Alien",
		];
		v.sort_by(|a, b| super::natural_cmp(a, b));
		assert_eq!(
			v,
			[
				"1: Bug",
				"2: Lizard",
				"10: Wolf",
				"Alien",
				"monkisrokBoss",
				"Zone_10"
			]
		);
	}

	use super::*;

	#[test]
	fn asset_id_forms() {
		assert_eq!(
			parse_asset_id("rbxassetid://136440628894302"),
			Some(136440628894302)
		);
		assert_eq!(
			parse_asset_id("http://www.roblox.com/asset/?id=123 "),
			Some(123)
		);
		assert_eq!(parse_asset_id("http://roblox.com/asset/?id=55"), Some(55));
		assert_eq!(
			parse_asset_id("https://assetdelivery.roblox.com/v1/asset/?id=77"),
			Some(77)
		);
		assert_eq!(parse_asset_id("https://x/asset/?version=1&id=9"), Some(9));
		assert_eq!(parse_asset_id(" 42 "), Some(42));
		assert_eq!(parse_asset_id(""), None);
		assert_eq!(parse_asset_id("rbxasset://textures/foo.png"), None);
	}

	/// Checks a real export without committing it:
	/// `RBX_EXPORT=path/to/OUTPUT.json cargo test -p meshvault -- --ignored`
	#[test]
	#[ignore]
	fn parses_real_export() {
		let Ok(path) = std::env::var("RBX_EXPORT") else {
			return;
		};
		let export = parse(&std::fs::read_to_string(path).unwrap()).unwrap();
		assert!(export.skipped.is_empty(), "skipped: {:?}", export.skipped);
		let tree = FolderTree::build(export.entries.iter().map(|e| e.folder.as_slice()));
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

		let tree = FolderTree::build(export.entries.iter().map(|e| e.folder.as_slice()));
		assert_eq!(tree.items, [2]);
		assert_eq!(
			tree.folders["Zone_10"].folders["8: CrystalScorpion"].items,
			[0]
		);
		assert_eq!(tree.all_items().len(), 3);
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
