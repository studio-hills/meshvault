//! Writes Wavefront OBJ files with their MTL: a mesh with its colour and
//! normal maps copied beside it, or a model as one OBJ of all its parts as
//! posed, with each part's map or colour. Vertex colours are written after
//! each position (`v x y z r g b`, the extension Blender and MeshLab read),
//! already multiplied by the part's colour, since OBJ materials cannot.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use eframe::egui::Color32;

use super::{MeshJob, Names};
use crate::render::maths::{IDENTITY, apply, apply_normal};
use crate::scene::colour::tint_srgb;
use crate::scene::geometry::{self, Geometry};
use crate::scene::maps::Slot;
use crate::scene::model_view::ModelView;
use crate::scene::surface::mesh_colour;
use crate::scene::texture::Material;

/// Writes a batch job as `name.obj` and `name.mtl`, which `names` has
/// handed out already.
pub(super) fn export_mesh(
	dir: &Path,
	name: &str,
	job: &MeshJob,
	names: &mut Names,
) -> Result<(), String> {
	let bytes = crate::workspace::read_asset(&job.mesh).map_err(|e| e.to_string())?;
	let geometry = geometry::load(&bytes)?.geometry;
	if geometry.triangles.is_empty() {
		return Err("the mesh has no triangles".into());
	}
	let groups = geometry.default_groups();
	write_one(
		Obj::new(dir, name, format!("{name}.mtl"), names),
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
	let mut names = Names::default();
	let result = Obj::chosen(path, &mut names).and_then(|obj| {
		write_one(
			obj,
			geometry,
			groups,
			map_path(material, Slot::Base),
			map_path(material, Slot::Normal),
		)
	});
	if result.is_err() {
		names.discard_since(0);
	}
	result
}

/// Writes `groups` of one mesh, unmoved, into `obj`, in the part colour or
/// showing the colour map `base`, with the maps copied beside it. A mesh
/// without texture coordinates cannot show maps, so it gets none, as in
/// the viewer and the glTF export.
fn write_one(
	mut obj: Obj,
	geometry: &Geometry,
	groups: &[usize],
	base: Option<&Path>,
	normal: Option<&Path>,
) -> Result<(), String> {
	let (base, normal) = if geometry.uvs.is_some() {
		(base, normal)
	} else {
		(None, None)
	};
	let part = mesh_colour(geometry);
	let colour = [part.r(), part.g(), part.b()].map(|c| c as f32 / 255.0);
	let material = obj.material(base, normal, colour)?;
	let name = obj.name.clone();
	let tint = base.is_none().then_some(part);
	obj.add(&name, geometry, groups, &IDENTITY, &material, tint);
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
	let mut names = Names::default();
	let result = Obj::chosen(path, &mut names).and_then(|obj| write_parts(obj, model));
	if result.is_err() {
		names.discard_since(0);
	}
	result
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

/// Writes a batch job as `name.obj` and `name.mtl`, which `names` has
/// handed out already.
pub(super) fn export_model(
	dir: &Path,
	name: &str,
	model: &mut ModelView,
	names: &mut Names,
) -> Result<(), String> {
	write_parts(Obj::new(dir, name, format!("{name}.mtl"), names), model)
}

/// Writes the parts of `model` as posed now into `obj`.
fn write_parts(mut obj: Obj, model: &mut ModelView) -> Result<(), String> {
	let parts = model.scene_parts(None);
	let mut count = 0;
	// decals and clothing are drawn over a part; the OBJ has the part alone
	for part in parts.iter().filter(|p| p.decal.is_none()) {
		let colour = [part.colour.r(), part.colour.g(), part.colour.b()].map(|c| c as f32 / 255.0);
		let base = map_path(part.material, Slot::Base);
		let material = obj.material(base, map_path(part.material, Slot::Normal), colour)?;
		count += 1;
		obj.add(
			&format!("part_{count}"),
			part.geometry,
			part.groups,
			&part.transform,
			&material,
			base.is_none().then_some(part.colour),
		);
	}
	if count == 0 {
		return Err("nothing in the model is drawn".into());
	}
	obj.write()
}

/// A sidecar's file name: `name` and `tail` (such as `.normal.png`), with
/// `_n` between them from the second on.
pub(super) fn numbered(name: &str, n: usize, tail: &str) -> String {
	match n {
		1 => format!("{name}{tail}"),
		n => format!("{name}_{n}{tail}"),
	}
}

fn map_path(material: &Material, slot: Slot) -> Option<&Path> {
	material.active(slot).map(|t| t.path.as_path())
}

/// An OBJ file being written, with its MTL. Both are built in `String`s,
/// which `writeln!` cannot fail to write to, so its results are ignored.
struct Obj<'a> {
	dir: PathBuf,
	name: String,
	/// The MTL's file name.
	mtl_file: String,
	/// What the export has written into `dir`, so maps get free names.
	names: &'a mut Names,
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

impl<'a> Obj<'a> {
	fn new(dir: &Path, name: &str, mtl_file: String, names: &'a mut Names) -> Self {
		Self {
			dir: dir.to_path_buf(),
			name: name.to_string(),
			obj: format!("# Exported by Meshvault\nmtllib {mtl_file}\n"),
			mtl_file,
			names,
			mtl: "# Exported by Meshvault\n".to_string(),
			written: [0; 3],
			materials: HashMap::new(),
			copied: HashMap::new(),
		}
	}

	/// The OBJ file `path` the user chose (replacing it is theirs to
	/// decide), with its MTL on the first free name of `name.mtl`,
	/// `name_2.mtl`, ... beside it.
	fn chosen(path: &Path, names: &'a mut Names) -> Result<Self, String> {
		let (dir, name) = split(path)?;
		names.reserve(&format!("{name}.obj"));
		let mtl_file = names.free(&dir, 1, |n| numbered(&name, n, ".mtl"));
		Ok(Self::new(&dir, &name, mtl_file, names))
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

	/// Copies a map beside the OBJ, once, numbered by how many came before
	/// it, or past that to a name no file has.
	fn copy(&mut self, from: &Path, suffix: &str) -> Result<String, String> {
		if let Some(file) = self.copied.get(from) {
			return Ok(file.clone());
		}
		let ext = from.extension().and_then(|e| e.to_str()).unwrap_or("png");
		let tail = format!("{suffix}.{ext}");
		let file = self.names.free(&self.dir, self.copied.len() + 1, |n| {
			numbered(&self.name, n, &tail)
		});
		std::fs::copy(from, self.dir.join(&file))
			.map_err(|e| format!("could not copy {}: {e}", from.display()))?;
		self.copied.insert(from.to_path_buf(), file.clone());
		Ok(file)
	}

	/// Adds `groups` of `geometry`, placed by the row-major `transform`,
	/// with its vertex colours times `tint` when it has any and `tint` is
	/// given (no colour map covers them).
	fn add(
		&mut self,
		name: &str,
		geometry: &Geometry,
		groups: &[usize],
		transform: &[[f32; 4]; 4],
		material: &str,
		tint: Option<Color32>,
	) {
		let colours = geometry
			.colors
			.as_ref()
			.filter(|c| c.len() == geometry.positions.len())
			.zip(tint);
		for (i, &p) in geometry.positions.iter().enumerate() {
			let p = apply(transform, p);
			let _ = match colours {
				Some((c, tint)) => {
					let [r, g, b] = tint_srgb(tint, [c[i][0], c[i][1], c[i][2]]);
					writeln!(self.obj, "v {} {} {} {r} {g} {b}", p[0], p[1], p[2])
				}
				None => writeln!(self.obj, "v {} {} {}", p[0], p[1], p[2]),
			};
		}
		// OBJ's texture origin is the bottom left; Roblox's the top left
		if let Some(uvs) = &geometry.uvs {
			for uv in uvs {
				let _ = writeln!(self.obj, "vt {} {}", uv[0], 1.0 - uv[1]);
			}
		}
		if let Some(normals) = &geometry.normals {
			for &n in normals {
				let n = apply_normal(transform, n);
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
		let mtl = self.dir.join(&self.mtl_file);
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
	use crate::scene::surface::PART_COLOUR;
	use crate::test_support::TempDir;

	fn triangle(uvs: bool) -> Geometry {
		Geometry {
			positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
			uvs: uvs.then(|| vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
			normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
			colors: None,
			triangles: vec![[0, 1, 2]],
			groups: vec![0..1],
			group_kind: GroupKind::Whole,
		}
	}

	#[test]
	fn parts_with_and_without_uvs_index_their_own_lists() {
		let dir = TempDir::new("obj_parts");
		let mut names = Names::default();
		let mut obj = Obj::new(&dir, "parts", "parts.mtl".into(), &mut names);
		obj.add("a", &triangle(false), &[0], &IDENTITY, "m", None);
		obj.add("b", &triangle(true), &[0], &IDENTITY, "m", None);
		assert!(obj.obj.contains("f 4/1/4 5/2/5 6/3/6"), "{}", obj.obj);
		check_indices(&obj.obj);
	}

	/// Writes group 0 of `geometry` as `name.obj` in `dir`, as the viewer does.
	fn one(
		dir: &Path,
		name: &str,
		geometry: &Geometry,
		base: Option<&Path>,
		normal: Option<&Path>,
	) {
		let mut names = Names::default();
		let obj = Obj::chosen(&dir.join(format!("{name}.obj")), &mut names).unwrap();
		write_one(obj, geometry, &[0], base, normal).unwrap();
	}

	/// A part stretched to twice its width: its 45° slope gets shallower,
	/// so the normal leans up, to (1, 2, 0) over √5, not (2, 1, 0).
	#[test]
	fn normals_of_a_stretched_part_stay_perpendicular() {
		let dir = TempDir::new("obj_stretched");
		let mut names = Names::default();
		let mut obj = Obj::new(&dir, "s", "s.mtl".into(), &mut names);
		let mut slope = triangle(false);
		let n = std::f32::consts::FRAC_1_SQRT_2;
		slope.normals = Some(vec![[n, n, 0.0]; 3]);
		let mut stretch = IDENTITY;
		stretch[0][0] = 2.0;
		obj.add("a", &slope, &[0], &stretch, "m", None);
		let vn: Vec<f32> = obj
			.obj
			.lines()
			.find_map(|l| l.strip_prefix("vn "))
			.unwrap()
			.split(' ')
			.map(|v| v.parse().unwrap())
			.collect();
		let len = 5f32.sqrt();
		let expected = [1.0 / len, 2.0 / len, 0.0];
		assert!((0..3).all(|k| (vn[k] - expected[k]).abs() < 1e-6), "{vn:?}");
	}

	fn read(path: PathBuf) -> String {
		std::fs::read_to_string(path).unwrap()
	}

	/// The colour after each `v` line's position, if any.
	fn vertex_colours(obj: &str) -> Vec<Vec<f32>> {
		obj.lines()
			.filter_map(|l| l.strip_prefix("v "))
			.map(|l| l.split(' ').skip(3).map(|v| v.parse().unwrap()).collect())
			.collect()
	}

	/// A mesh with vertex colours is written with them, white underneath
	/// (as the viewer shows it); under a colour map, or without vertex
	/// colours, the positions stand alone.
	#[test]
	fn vertex_colours_follow_each_position() {
		let dir = TempDir::new("obj_colours");
		let mut coloured = triangle(true);
		coloured.colors = Some(vec![[102, 115, 72, 255], [16, 21, 29, 255], [255; 4]]);
		one(&dir, "tank", &coloured, None, None);
		let obj = read(dir.join("tank.obj"));
		let expected = [[102.0, 115.0, 72.0], [16.0, 21.0, 29.0], [255.0; 3]];
		for (got, want) in vertex_colours(&obj).iter().zip(expected) {
			let got: Vec<f32> = got.iter().map(|c| (c * 255.0).round()).collect();
			assert_eq!(got, want, "{obj}");
		}
		assert!(read(dir.join("tank.mtl")).contains("Kd 1 1 1\n"));

		let map = dir.join("source.png");
		std::fs::write(&map, "png").unwrap();
		one(&dir, "mapped", &coloured, Some(&map), None);
		one(&dir, "plain", &triangle(true), None, None);
		for name in ["mapped.obj", "plain.obj"] {
			let obj = read(dir.join(name));
			assert!(vertex_colours(&obj).iter().all(Vec::is_empty), "{obj}");
		}
	}

	/// A model part's vertex colours are multiplied by its colour, in
	/// linear light, as the viewer draws them.
	#[test]
	fn a_parts_vertex_colours_take_its_colour() {
		let dir = TempDir::new("obj_tinted");
		let mut names = Names::default();
		let mut obj = Obj::new(&dir, "t", "t.mtl".into(), &mut names);
		let mut g = triangle(false);
		g.colors = Some(vec![
			[255, 255, 255, 255],
			[102, 115, 72, 255],
			[0, 0, 0, 255],
		]);
		let red = Color32::from_rgb(255, 0, 0);
		obj.add("a", &g, &[0], &IDENTITY, "m", Some(red));
		// as bytes: red keeps only the red of each vertex colour
		let colours: Vec<Vec<u8>> = vertex_colours(&obj.obj)
			.iter()
			.map(|c| c.iter().map(|v| (v * 255.0).round() as u8).collect())
			.collect();
		assert_eq!(colours, [vec![255, 0, 0], vec![102, 0, 0], vec![0, 0, 0]]);
		check_indices(&obj.obj);
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
		one(&dir, "plain", &triangle(false), Some(&map), Some(&map));
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
		one(&dir, "mapped", &triangle(true), Some(&map), None);
		let mtl = read(dir.join("mapped.mtl"));
		assert!(mtl.contains("Kd 1 1 1\nmap_Kd mapped.png\n"), "{mtl}");
		assert_eq!(read(dir.join("mapped.png")), "png");
		one(&dir, "bare", &triangle(true), None, None);
		let mtl = read(dir.join("bare.mtl"));
		assert!(mtl.contains(&part_kd()) && !mtl.contains("map_Kd"), "{mtl}");
	}

	/// The OBJ is the file the user chose; its MTL and maps go beside it
	/// without replacing files already there.
	#[test]
	fn a_chosen_obj_keeps_the_files_beside_it() {
		let dir = TempDir::new("obj_chosen");
		for file in ["mesh.mtl", "mesh.png", "mesh_2.mtl"] {
			std::fs::write(dir.join(file), "keep").unwrap();
		}
		let map = dir.join("source.png");
		image::RgbaImage::new(1, 1).save(&map).unwrap();
		let material = crate::scene::texture::load_maps(
			&eframe::egui::Context::default(),
			&[(Slot::Base, map.clone())],
		);
		let path = dir.join("mesh.obj");
		write_mesh(&path, &triangle(true), &[0], &material).unwrap();
		for file in ["mesh.mtl", "mesh.png", "mesh_2.mtl"] {
			assert_eq!(read(dir.join(file)), "keep", "{file}");
		}
		assert!(read(path).contains("mtllib mesh_3.mtl\n"));
		assert!(read(dir.join("mesh_3.mtl")).contains("map_Kd mesh_2.png\n"));
		assert_eq!(
			std::fs::read(dir.join("mesh_2.png")).unwrap(),
			std::fs::read(&map).unwrap()
		);
	}
}
