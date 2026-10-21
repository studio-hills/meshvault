//! Writes Wavefront OBJ files with their MTL: a mesh with its colour and
//! normal maps copied beside it, or an animated model as one OBJ of all
//! its parts as posed, with each part's map or colour.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::MeshJob;
use crate::render::maths::{IDENTITY, apply, apply_dir};
use crate::scene::geometry::{self, Geometry};
use crate::scene::maps::Slot;
use crate::scene::model_view::ModelView;
use crate::scene::surface::PART_COLOUR;
use crate::scene::texture::Material;

pub(super) fn export_mesh(dir: &Path, name: &str, job: &MeshJob) -> Result<(), String> {
	let bytes = std::fs::read(&job.mesh).map_err(|e| e.to_string())?;
	let geometry = geometry::load(&bytes)?.geometry;
	if geometry.triangles.is_empty() {
		return Err("the mesh has no triangles".into());
	}
	let groups = geometry.default_groups();
	write_one(
		dir,
		name,
		&geometry,
		&groups,
		map_of(&job.maps, Slot::Base),
		map_of(&job.maps, Slot::Normal),
	)
}

/// Writes `groups` of a mesh to the OBJ file `path`, with an MTL file and
/// `material`'s colour and normal maps beside it.
pub fn write_mesh(
	path: &Path,
	geometry: &Geometry,
	groups: &[usize],
	material: &Material,
) -> Result<(), String> {
	let (dir, name) = split(path)?;
	write_one(
		&dir,
		&name,
		geometry,
		groups,
		map_path(material, Slot::Base),
		map_path(material, Slot::Normal),
	)
}

/// Writes `groups` of one mesh, unmoved, as the OBJ `name` in `dir`, in
/// the part colour or showing the colour map `base`, with the maps copied
/// beside it. A mesh without texture coordinates cannot show maps, so it
/// gets none, as in the viewer and the glTF export.
fn write_one(
	dir: &Path,
	name: &str,
	geometry: &Geometry,
	groups: &[usize],
	base: Option<&Path>,
	normal: Option<&Path>,
) -> Result<(), String> {
	let mut obj = Obj::new(dir, name);
	let (base, normal) = if geometry.uvs.is_some() {
		(base, normal)
	} else {
		(None, None)
	};
	let colour = [PART_COLOUR.r(), PART_COLOUR.g(), PART_COLOUR.b()].map(|c| c as f32 / 255.0);
	let material = obj.material(base, normal, colour)?;
	obj.add(name, geometry, groups, &IDENTITY, &material);
	obj.write()
}

/// The first of `maps` for `slot`.
fn map_of(maps: &[(Slot, PathBuf)], slot: Slot) -> Option<&Path> {
	maps.iter()
		.find(|(s, _)| *s == slot)
		.map(|(_, p)| p.as_path())
}

/// Writes an animated model as posed now to the OBJ file `path`.
pub fn write_model(path: &Path, model: &mut ModelView) -> Result<(), String> {
	let (dir, name) = split(path)?;
	export_model(&dir, &name, model)
}

/// A file path's folder and its name without the extension.
fn split(path: &Path) -> Result<(PathBuf, String), String> {
	let dir = path
		.parent()
		.ok_or("no folder to write into")?
		.to_path_buf();
	let name = path
		.file_stem()
		.map(|s| s.to_string_lossy().into_owned())
		.ok_or("no file name")?;
	Ok((dir, name))
}

pub(super) fn export_model(dir: &Path, name: &str, model: &mut ModelView) -> Result<(), String> {
	let mut obj = Obj::new(dir, name);
	let parts = model.scene_parts(None);
	let mut count = 0;
	// decals and clothing are drawn over a part; the OBJ has the part alone
	for part in parts.iter().filter(|p| p.decal.is_none()) {
		let colour = [part.colour.r(), part.colour.g(), part.colour.b()].map(|c| c as f32 / 255.0);
		let material = obj.material(
			map_path(part.material, Slot::Base),
			map_path(part.material, Slot::Normal),
			colour,
		)?;
		count += 1;
		obj.add(
			&format!("part_{count}"),
			part.geometry,
			part.groups,
			&part.transform,
			&material,
		);
	}
	if count == 0 {
		return Err("nothing in the model is drawn".into());
	}
	obj.write()
}

fn map_path(material: &Material, slot: Slot) -> Option<&Path> {
	material.active(slot).map(|t| t.path.as_path())
}

/// An OBJ file being written, with its MTL. Both are built in `String`s,
/// which `writeln!` cannot fail to write to, so its results are ignored.
struct Obj {
	dir: PathBuf,
	name: String,
	obj: String,
	mtl: String,
	/// Positions, texture coordinates and normals written so far (OBJ
	/// indices count from 1 across the file, each kind on its own).
	written: [usize; 3],
	/// Material name by its maps and colour.
	materials: HashMap<String, String>,
	/// Copied map file name by source path.
	copied: HashMap<PathBuf, String>,
}

impl Obj {
	fn new(dir: &Path, name: &str) -> Self {
		Self {
			dir: dir.to_path_buf(),
			name: name.to_string(),
			obj: format!("# Exported by Meshvault\nmtllib {name}.mtl\n"),
			mtl: "# Exported by Meshvault\n".to_string(),
			written: [0; 3],
			materials: HashMap::new(),
			copied: HashMap::new(),
		}
	}

	/// The material showing `base` (or `colour` without one) and `normal`,
	/// copying the maps beside the OBJ.
	fn material(
		&mut self,
		base: Option<&Path>,
		normal: Option<&Path>,
		colour: [f32; 3],
	) -> Result<String, String> {
		let key = format!("{base:?}|{normal:?}|{colour:?}");
		if let Some(name) = self.materials.get(&key) {
			return Ok(name.clone());
		}
		let name = format!("material_{}", self.materials.len() + 1);
		let kd = if base.is_some() { [1.0; 3] } else { colour };
		let _ = writeln!(
			self.mtl,
			"\nnewmtl {name}\nKd {} {} {}",
			kd[0], kd[1], kd[2]
		);
		if let Some(base) = base {
			let file = self.copy(base, "")?;
			let _ = writeln!(self.mtl, "map_Kd {file}");
		}
		if let Some(normal) = normal {
			let file = self.copy(normal, ".normal")?;
			let _ = writeln!(self.mtl, "norm {file}\nmap_Bump {file}");
		}
		self.materials.insert(key, name.clone());
		Ok(name)
	}

	/// Copies a map beside the OBJ, once.
	fn copy(&mut self, from: &Path, suffix: &str) -> Result<String, String> {
		if let Some(file) = self.copied.get(from) {
			return Ok(file.clone());
		}
		let ext = from.extension().and_then(|e| e.to_str()).unwrap_or("png");
		let file = match self.copied.len() {
			0 => format!("{}{suffix}.{ext}", self.name),
			n => format!("{}_{}{suffix}.{ext}", self.name, n + 1),
		};
		std::fs::copy(from, self.dir.join(&file))
			.map_err(|e| format!("could not copy {}: {e}", from.display()))?;
		self.copied.insert(from.to_path_buf(), file.clone());
		Ok(file)
	}

	/// Adds `groups` of `geometry`, placed by the row-major `transform`.
	fn add(
		&mut self,
		name: &str,
		geometry: &Geometry,
		groups: &[usize],
		transform: &[[f32; 4]; 4],
		material: &str,
	) {
		for &p in &geometry.positions {
			let p = apply(transform, p);
			let _ = writeln!(self.obj, "v {} {} {}", p[0], p[1], p[2]);
		}
		// OBJ's texture origin is the bottom left; Roblox's the top left
		if let Some(uvs) = &geometry.uvs {
			for uv in uvs {
				let _ = writeln!(self.obj, "vt {} {}", uv[0], 1.0 - uv[1]);
			}
		}
		if let Some(normals) = &geometry.normals {
			for &n in normals {
				let n = apply_dir(transform, n);
				let _ = writeln!(self.obj, "vn {} {} {}", n[0], n[1], n[2]);
			}
		}
		let _ = writeln!(self.obj, "o {name}\nusemtl {material}");
		let [v, vt, vn] = self.written.map(|w| w + 1);
		let corner = |i: u32| {
			let i = i as usize;
			match (geometry.uvs.is_some(), geometry.normals.is_some()) {
				(true, true) => format!("{}/{}/{}", v + i, vt + i, vn + i),
				(true, false) => format!("{}/{}", v + i, vt + i),
				(false, true) => format!("{}//{}", v + i, vn + i),
				(false, false) => (v + i).to_string(),
			}
		};
		for &g in groups {
			let Some(range) = geometry.groups.get(g) else {
				continue;
			};
			for t in &geometry.triangles[range.clone()] {
				let _ = writeln!(
					self.obj,
					"f {} {} {}",
					corner(t[0]),
					corner(t[1]),
					corner(t[2])
				);
			}
		}
		self.written[0] += geometry.positions.len();
		self.written[1] += geometry.uvs.as_ref().map_or(0, Vec::len);
		self.written[2] += geometry.normals.as_ref().map_or(0, Vec::len);
	}

	fn write(self) -> Result<(), String> {
		let obj = self.dir.join(format!("{}.obj", self.name));
		std::fs::write(&obj, &self.obj)
			.map_err(|e| format!("could not write {}: {e}", obj.display()))?;
		let mtl = self.dir.join(format!("{}.mtl", self.name));
		std::fs::write(&mtl, &self.mtl)
			.map_err(|e| format!("could not write {}: {e}", mtl.display()))
	}
}

#[cfg(test)]
// geometry groups are lists of ranges, so one-element lists are intended
#[allow(clippy::single_range_in_vec_init)]
mod tests {
	use super::*;
	use crate::export::fixtures::check_indices;
	use crate::scene::geometry::GroupKind;
	use crate::test_support::TempDir;

	fn triangle(uvs: bool) -> Geometry {
		Geometry {
			positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
			uvs: uvs.then(|| vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
			normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
			triangles: vec![[0, 1, 2]],
			groups: vec![0..1],
			group_kind: GroupKind::Whole,
		}
	}

	#[test]
	fn parts_with_and_without_uvs_index_their_own_lists() {
		let dir = TempDir::new("obj_parts");
		let mut obj = Obj::new(&dir, "parts");
		obj.add("a", &triangle(false), &[0], &IDENTITY, "m");
		obj.add("b", &triangle(true), &[0], &IDENTITY, "m");
		assert!(obj.obj.contains("f 4/1/4 5/2/5 6/3/6"), "{}", obj.obj);
		check_indices(&obj.obj);
	}

	fn read(path: PathBuf) -> String {
		std::fs::read_to_string(path).unwrap()
	}

	/// The `Kd` line of the part colour, as the MTL writes it.
	fn part_kd() -> String {
		let [r, g, b] =
			[PART_COLOUR.r(), PART_COLOUR.g(), PART_COLOUR.b()].map(|c| c as f32 / 255.0);
		format!("Kd {r} {g} {b}\n")
	}

	#[test]
	fn a_mesh_without_uvs_gets_the_part_colour_and_no_maps() {
		let dir = TempDir::new("obj_no_uvs");
		let map = dir.join("source.png");
		std::fs::write(&map, "png").unwrap();
		write_one(
			&dir,
			"plain",
			&triangle(false),
			&[0],
			Some(&map),
			Some(&map),
		)
		.unwrap();
		let mtl = read(dir.join("plain.mtl"));
		assert!(mtl.contains(&part_kd()), "{mtl}");
		assert!(!mtl.contains("map_Kd") && !mtl.contains("norm"), "{mtl}");
		assert!(!dir.join("plain.png").exists() && !dir.join("plain.normal.png").exists());
	}

	#[test]
	fn a_mesh_with_uvs_is_white_under_its_map_and_the_part_colour_without() {
		let dir = TempDir::new("obj_uvs");
		let map = dir.join("source.png");
		std::fs::write(&map, "png").unwrap();
		write_one(&dir, "mapped", &triangle(true), &[0], Some(&map), None).unwrap();
		let mtl = read(dir.join("mapped.mtl"));
		assert!(mtl.contains("Kd 1 1 1\nmap_Kd mapped.png\n"), "{mtl}");
		assert_eq!(read(dir.join("mapped.png")), "png");
		write_one(&dir, "bare", &triangle(true), &[0], None, None).unwrap();
		let mtl = read(dir.join("bare.mtl"));
		assert!(mtl.contains(&part_kd()) && !mtl.contains("map_Kd"), "{mtl}");
	}

	#[test]
	fn split_gives_the_folder_and_stem() {
		let path = Path::new("out").join("mesh.obj");
		assert_eq!(split(&path), Ok((PathBuf::from("out"), "mesh".into())));
		// a bare file name has an empty folder, which is fine
		assert_eq!(
			split(Path::new("mesh.obj")),
			Ok((PathBuf::new(), "mesh".into()))
		);
		assert_eq!(
			split(Path::new("mesh")),
			Ok((PathBuf::new(), "mesh".into()))
		);
	}

	#[test]
	fn split_rejects_a_path_without_folder_or_name() {
		for path in ["", "/"] {
			assert_eq!(
				split(Path::new(path)),
				Err("no folder to write into".into()),
				"{path:?}"
			);
		}
		assert_eq!(split(Path::new("out/..")), Err("no file name".into()));
	}
}
