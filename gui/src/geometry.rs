//! Converts every format rbx_mesh can decode into one flat triangle list
//! that the viewer can draw and export.

// Geometry::groups is a list of triangle ranges, so a one-element list is intended.
#![allow(clippy::single_range_in_vec_init)]

use std::io::Cursor;
use std::ops::Range;

use rbx_mesh::mesh::{Chunk6, Chunks6, CoreMeshChunk, Mesh, Vertices2};
use rbx_mesh::union_graphics::UnionGraphics;
use rbx_mesh::union_physics::UnionPhysics;

/// What a group of triangles represents, so the UI can label it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
	/// A level of detail in a mesh file. Only one is shown at a time.
	Lod,
	/// A convex hull in a union physics file. All are shown at once.
	Hull,
	/// The whole file.
	Whole,
}

#[derive(Clone, Debug)]
pub struct Geometry {
	pub positions: Vec<[f32; 3]>,
	/// Texture coordinates, one per position, when the format has them.
	pub uvs: Option<Vec<[f32; 2]>>,
	/// Vertex normals, one per position, when the format has them.
	pub normals: Option<Vec<[f32; 3]>>,
	pub triangles: Vec<[u32; 3]>,
	/// Ranges into `triangles`.
	pub groups: Vec<Range<usize>>,
	pub group_kind: GroupKind,
}

impl Geometry {
	fn empty() -> Self {
		Self {
			positions: Vec::new(),
			uvs: None,
			normals: None,
			triangles: Vec::new(),
			groups: Vec::new(),
			group_kind: GroupKind::Whole,
		}
	}

	fn whole(positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> Self {
		let groups = vec![0..triangles.len()];
		Self {
			positions,
			uvs: None,
			normals: None,
			triangles,
			groups,
			group_kind: GroupKind::Whole,
		}
	}

	/// Attaches texture coordinates, ignoring them if the count does not
	/// match the positions (some formats store them separately).
	/// Attaches vertex normals, ignored unless there is one per position.
	fn with_normals(mut self, normals: Vec<[f32; 3]>) -> Self {
		if normals.len() == self.positions.len() && !normals.is_empty() {
			self.normals = Some(normals);
		}
		self
	}

	fn with_uvs(mut self, uvs: Vec<[f32; 2]>) -> Self {
		if uvs.len() == self.positions.len() && !uvs.is_empty() {
			self.uvs = Some(uvs);
		}
		self
	}

	/// Appends a separately indexed piece and records it as its own group.
	fn push_part(&mut self, positions: &[[f32; 3]], triangles: impl IntoIterator<Item = [u32; 3]>) {
		let base = self.positions.len() as u32;
		let start = self.triangles.len();
		self.positions.extend_from_slice(positions);
		self.triangles
			.extend(triangles.into_iter().map(|t| t.map(|i| i + base)));
		self.groups.push(start..self.triangles.len());
	}

	/// Drops triangles that reference missing vertices so a slightly broken
	/// file can still be drawn instead of panicking.
	fn sanitize(&mut self) -> usize {
		let n = self.positions.len() as u32;
		let mut dropped = 0;
		let mut kept = Vec::with_capacity(self.triangles.len());
		let mut groups = Vec::with_capacity(self.groups.len());
		for group in &self.groups {
			let start = kept.len();
			for t in &self.triangles[group.clone()] {
				if t.iter().all(|&i| i < n) {
					kept.push(*t);
				} else {
					dropped += 1;
				}
			}
			groups.push(start..kept.len());
		}
		self.triangles = kept;
		self.groups = groups;
		dropped
	}

	pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
		let mut it = self.positions.iter();
		let first = *it.next()?;
		Some(it.fold((first, first), |(mut lo, mut hi), p| {
			for k in 0..3 {
				lo[k] = lo[k].min(p[k]);
				hi[k] = hi[k].max(p[k]);
			}
			(lo, hi)
		}))
	}
}

#[derive(Clone, Debug)]
pub struct Loaded {
	/// e.g. "Mesh v4.01"
	pub format: String,
	/// Extra facts about the file, shown in the info panel.
	pub details: Vec<(String, String)>,
	pub geometry: Geometry,
	/// Set when the file decoded but carries no drawable mesh.
	pub note: Option<String>,
}

/// Tries each decoder in turn and returns the first one that understands the file.
pub fn load(bytes: &[u8]) -> Result<Loaded, String> {
	// Mesh files start with a readable "version x.yy" line, so decode them
	// directly to give a focused error instead of one per decoder.
	if let Some(rest) = bytes.strip_prefix(b"version ") {
		let version = String::from_utf8_lossy(&rest[..rest.len().min(4)]).into_owned();
		return match rbx_mesh::read_mesh_versioned(Cursor::new(bytes)) {
			Ok(mesh) => Ok(finish(from_mesh(mesh))),
			Err(_) if !matches!(version.as_bytes().first(), Some(b'1'..=b'7')) => Err(format!(
				"Mesh version {version} is not supported by rbx_mesh yet (supported: 1.00 – 7.00)."
			)),
			Err(e) => Err(format!("Mesh version {version} failed to decode:\n{}", clean(e))),
		};
	}
	if bytes.starts_with(b"CSGPHS") {
		return rbx_mesh::read_union_physics_versioned(Cursor::new(bytes))
			.map(|m| finish(from_union_physics(m)))
			.map_err(|e| format!("Union physics data failed to decode:\n{}", clean(e)));
	}
	let mut errors = Vec::new();
	match rbx_mesh::read_union_graphics_versioned(Cursor::new(bytes)) {
		Ok(mesh) => return Ok(finish(from_union_graphics(mesh))),
		Err(e) => errors.push(format!("union graphics: {}", clean(e))),
	}
	match rbx_mesh::read_union_physics_versioned(Cursor::new(bytes)) {
		Ok(mesh) => return Ok(finish(from_union_physics(mesh))),
		Err(e) => errors.push(format!("union physics: {}", clean(e))),
	}
	Err(format!(
		"Not a recognised Roblox mesh, union graphics or union physics file.\n\n{}",
		errors.join("\n\n")
	))
}

/// binrw errors embed ANSI colour codes; strip them for display.
fn clean(e: binrw::Error) -> String {
	let s = e.to_string();
	let mut out = String::with_capacity(s.len());
	let mut chars = s.chars();
	while let Some(c) = chars.next() {
		if c == '\x1b' {
			chars.by_ref().find(|c| c.is_ascii_alphabetic());
		} else {
			out.push(c);
		}
	}
	out
}

fn finish(mut loaded: Loaded) -> Loaded {
	let dropped = loaded.geometry.sanitize();
	if dropped > 0 {
		loaded
			.details
			.push(("Invalid triangles skipped".into(), dropped.to_string()));
	}
	loaded
}

fn detail(name: &str, value: impl ToString) -> (String, String) {
	(name.to_string(), value.to_string())
}

fn faces2(faces: &[rbx_mesh::mesh::Face2]) -> Vec<[u32; 3]> {
	faces
		.iter()
		.map(|f| [f.0[0].0, f.0[1].0, f.0[2].0])
		.collect()
}

type Attributes = (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>);

fn vertices2(vertices: &Vertices2) -> (Attributes, &'static str) {
	match vertices {
		Vertices2::Full(v) => (
			(
				v.iter().map(|v| v.pos).collect(),
				v.iter().map(|v| v.tex).collect(),
				v.iter().map(|v| v.norm).collect(),
			),
			"40 bytes (with colour)",
		),
		Vertices2::Truncated(v) => (
			(
				v.iter().map(|v| v.pos).collect(),
				v.iter().map(|v| v.tex).collect(),
				v.iter().map(|v| v.norm).collect(),
			),
			"36 bytes (no colour)",
		),
	}
}

/// Splits faces into LOD groups. LOD values are face indices marking where
/// each level starts, with the next value marking where it ends.
fn lod_groups(lods: &[rbx_mesh::mesh::Lod3], face_count: usize) -> Vec<Range<usize>> {
	let groups: Vec<_> = lods
		.windows(2)
		.map(|w| (w[0].0 as usize).min(face_count)..(w[1].0 as usize).min(face_count))
		.filter(|r| r.start < r.end)
		.collect();
	if groups.is_empty() {
		vec![0..face_count]
	} else {
		groups
	}
}

fn bone_names(bytes: &[u8]) -> String {
	bytes
		.split(|&b| b == 0)
		.filter(|s| !s.is_empty())
		.map(String::from_utf8_lossy)
		.collect::<Vec<_>>()
		.join(", ")
}

fn from_mesh(mesh: Mesh) -> Loaded {
	match mesh {
		Mesh::V1(m) => {
			let revision = match m.revision {
				rbx_mesh::mesh::Revision1::Version100 => "1.00",
				rbx_mesh::mesh::Revision1::Version101 => "1.01",
			};
			let positions: Vec<_> = m.vertices.iter().map(|v| v.pos).collect();
			let uvs = m.vertices.iter().map(|v| [v.tex[0], v.tex[1]]).collect();
			let normals = m.vertices.iter().map(|v| v.norm).collect();
			let triangles = (0..positions.len() as u32 / 3)
				.map(|i| [i * 3, i * 3 + 1, i * 3 + 2])
				.collect();
			Loaded {
				format: format!("Mesh v{revision} (ASCII)"),
				details: vec![detail("Vertices", positions.len())],
				geometry: Geometry::whole(positions, triangles)
					.with_uvs(uvs)
					.with_normals(normals),
				note: None,
			}
		}
		Mesh::V2(m) => {
			let ((positions, uvs, normals), vertex_size) = vertices2(&m.vertices);
			let triangles = faces2(&m.faces);
			Loaded {
				format: "Mesh v2.00".into(),
				details: vec![
					detail("Vertices", positions.len()),
					detail("Faces", triangles.len()),
					detail("Vertex size", vertex_size),
				],
				geometry: Geometry::whole(positions, triangles)
					.with_uvs(uvs)
					.with_normals(normals),
				note: None,
			}
		}
		Mesh::V3(m) => {
			let revision = match m.revision {
				rbx_mesh::mesh::Revision3::Version300 => "3.00",
				rbx_mesh::mesh::Revision3::Version301 => "3.01",
			};
			let ((positions, uvs, normals), vertex_size) = vertices2(&m.vertices);
			let triangles = faces2(&m.faces);
			let groups = lod_groups(&m.lods, triangles.len());
			Loaded {
				format: format!("Mesh v{revision}"),
				details: vec![
					detail("Vertices", positions.len()),
					detail("Faces", triangles.len()),
					detail("Vertex size", vertex_size),
					detail("LODs", groups.len()),
				],
				geometry: Geometry {
					positions,
					uvs: None,
					normals: None,
					triangles,
					groups,
					group_kind: GroupKind::Lod,
				}
				.with_uvs(uvs)
				.with_normals(normals),
				note: None,
			}
		}
		Mesh::V4(m) => {
			let revision = match m.revision {
				rbx_mesh::mesh::Revision4::Version400 => "4.00",
				rbx_mesh::mesh::Revision4::Version401 => "4.01",
			};
			let positions: Vec<_> = m.vertices.iter().map(|v| v.pos).collect();
			let uvs = m.vertices.iter().map(|v| v.tex).collect();
			let normals = m.vertices.iter().map(|v| v.norm).collect();
			let triangles = faces2(&m.faces);
			let groups = lod_groups(&m.lods, triangles.len());
			let mut details = vec![
				detail("Vertices", positions.len()),
				detail("Faces", triangles.len()),
				detail("LODs", groups.len()),
				detail("LOD type", format!("{:?}", m.lod_type)),
				detail("Bones", m.bones.len()),
				detail("Subsets", m.subsets.len()),
			];
			if !m.bones.is_empty() {
				details.push(detail("Bone names", bone_names(&m.bone_names)));
			}
			Loaded {
				format: format!("Mesh v{revision}"),
				details,
				geometry: Geometry {
					positions,
					uvs: None,
					normals: None,
					triangles,
					groups,
					group_kind: GroupKind::Lod,
				}
				.with_uvs(uvs)
				.with_normals(normals),
				note: None,
			}
		}
		Mesh::V5(m) => {
			let positions: Vec<_> = m.vertices.iter().map(|v| v.pos).collect();
			let uvs = m.vertices.iter().map(|v| v.tex).collect();
			let normals = m.vertices.iter().map(|v| v.norm).collect();
			let triangles = faces2(&m.faces);
			let groups = lod_groups(&m.lods, triangles.len());
			let mut details = vec![
				detail("Vertices", positions.len()),
				detail("Faces", triangles.len()),
				detail("LODs", groups.len()),
				detail("LOD type", format!("{:?}", m.lod_type)),
				detail("Bones", m.bones.len()),
				detail("Subsets", m.subsets.len()),
				detail("FACS data", format!("{} bytes", m.sizeof_facs)),
			];
			if !m.bones.is_empty() {
				details.push(detail("Bone names", bone_names(&m.bone_names)));
			}
			let controls = bone_names(&m.facs.face_control_names);
			if !controls.is_empty() {
				details.push(detail("FACS controls", controls));
			}
			Loaded {
				format: "Mesh v5.00".into(),
				details,
				geometry: Geometry {
					positions,
					uvs: None,
					normals: None,
					triangles,
					groups,
					group_kind: GroupKind::Lod,
				}
				.with_uvs(uvs)
				.with_normals(normals),
				note: None,
			}
		}
		Mesh::V6(m) => from_chunks(&m, "6.00"),
		Mesh::V7(m) => from_chunks(&m, "7.00"),
	}
}

fn chunk_name(name: &[u8; 8]) -> String {
	String::from_utf8_lossy(name).trim_end_matches('\0').to_string()
}

/// v6.00 / v7.00 meshes: geometry comes from the COREMESH chunk, which may
/// be Draco compressed, with LODs, bones and FACS in their own chunks.
fn from_chunks(m: &impl Chunks6, revision: &str) -> Loaded {
	let chunk_names: Vec<String> = m
		.chunks()
		.iter()
		.map(|c| match c {
			Chunk6::CoreMesh(CoreMeshChunk::V1(_)) => "COREMESH v1".into(),
			Chunk6::CoreMesh(CoreMeshChunk::V2(_)) => "COREMESH v2 (Draco)".into(),
			Chunk6::CoreMesh(CoreMeshChunk::Unsupported(u)) => format!("COREMESH v{}?", u.version),
			Chunk6::Lods(_) => "LODS".into(),
			Chunk6::Skinning(_) => "SKINNING".into(),
			Chunk6::Facs(_) => "FACS".into(),
			Chunk6::HsrAvis(_) => "HSRAVIS".into(),
			Chunk6::Unknown(u) => format!("{}?", chunk_name(&u.name)),
		})
		.collect();
	let mut details = vec![detail("Chunks", chunk_names.join(", "))];
	let mut note = None;

	let core = match m.core_mesh() {
		Some(CoreMeshChunk::V1(core)) => Some(core.clone()),
		Some(CoreMeshChunk::V2(draco)) => {
			details.push(detail("Draco bitstream", format!("{} bytes", draco.bitstream.len())));
			match draco.decode() {
				Ok(core) => Some(core),
				Err(e) => {
					note = Some(format!("Could not decode the Draco compressed mesh: {e}"));
					None
				}
			}
		}
		Some(CoreMeshChunk::Unsupported(u)) => {
			note = Some(format!("COREMESH chunk version {} is not supported.", u.version));
			None
		}
		None => {
			note = Some("This file has no COREMESH chunk.".into());
			None
		}
	};

	let (positions, uvs, normals, triangles) = match &core {
		Some(core) => (
			core.vertices.iter().map(|v| v.pos).collect(),
			core.vertices.iter().map(|v| v.tex).collect(),
			core.vertices.iter().map(|v| v.norm).collect(),
			faces2(&core.faces),
		),
		None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
	};
	details.push(detail("Vertices", positions.len()));
	details.push(detail("Faces", triangles.len()));

	let groups = match m.lods() {
		Some(lods) => {
			let groups = lod_groups(&lods.lods, triangles.len());
			details.push(detail("LODs", groups.len()));
			details.push(detail("LOD type", format!("{:?}", lods.lod_type)));
			groups
		}
		None => vec![0..triangles.len()],
	};
	if let Some(skinning) = m.skinning() {
		details.push(detail("Bones", skinning.bones.len()));
		details.push(detail("Subsets", skinning.subsets.len()));
		if !skinning.bones.is_empty() {
			details.push(detail("Bone names", bone_names(&skinning.bone_names)));
		}
	}
	if let Some(facs) = m.facs() {
		let controls = bone_names(&facs.face_control_names);
		if !controls.is_empty() {
			details.push(detail("FACS controls", controls));
		}
	}
	if let Some(hsr) = m.hsr_avis() {
		let hidden = (0..hsr.bit_count as usize).filter(|&i| hsr.get(i) == Some(true)).count();
		details.push(detail("HSR culled faces", hidden));
	}

	Loaded {
		format: format!("Mesh v{revision}"),
		details,
		geometry: Geometry {
			positions,
			uvs: None,
			normals: None,
			triangles,
			groups,
			group_kind: GroupKind::Lod,
		}
		.with_uvs(uvs)
		.with_normals(normals),
		note,
	}
}

fn csgk(uuid: &[u8; 32], kind: &str) -> Loaded {
	Loaded {
		format: format!("{kind} CSGK"),
		details: vec![detail("UUID", String::from_utf8_lossy(uuid))],
		geometry: Geometry::empty(),
		note: Some("CSGK files only hold a reference UUID and contain no mesh data.".into()),
	}
}

fn union_mesh2(m: &rbx_mesh::union_graphics::Mesh2) -> Geometry {
	let positions = m.vertices.iter().map(|v| v.pos).collect();
	let uvs = m.vertices.iter().map(|v| v.tex).collect();
	let normals = m.vertices.iter().map(|v| v.norm).collect();
	let triangles = m.faces.iter().map(|f| [f[0].0, f[1].0, f[2].0]).collect();
	Geometry::whole(positions, triangles)
		.with_uvs(uvs)
		.with_normals(normals)
}

fn from_union_graphics(mesh: UnionGraphics) -> Loaded {
	match mesh {
		UnionGraphics::CSGK(c) => csgk(&c.uuid_ascii_hex, "Union graphics"),
		UnionGraphics::V2(m) => {
			let geometry = union_mesh2(&m.mesh);
			Loaded {
				format: "Union graphics CSGMDL v2".into(),
				details: vec![
					detail("Vertices", geometry.positions.len()),
					detail("Faces", geometry.triangles.len()),
				],
				geometry,
				note: None,
			}
		}
		UnionGraphics::V4(m) => {
			let geometry = union_mesh2(&m.mesh);
			Loaded {
				format: "Union graphics CSGMDL v4".into(),
				details: vec![
					detail("Vertices", geometry.positions.len()),
					detail("Faces", geometry.triangles.len()),
				],
				geometry,
				note: None,
			}
		}
		UnionGraphics::V5(m) => {
			let triangles: Vec<_> = m
				.faces
				.indices
				.chunks_exact(3)
				.map(|c| [c[0], c[1], c[2]])
				.collect();
			Loaded {
				format: "Union graphics CSGMDL v5".into(),
				details: vec![
					detail("Positions", m.positions.len()),
					detail("Normals", m.normals.len()),
					detail("Faces", triangles.len()),
					detail("Extra index lists", m.faces._unknown.len()),
				],
				geometry: Geometry::whole(m.positions, triangles)
					.with_uvs(m.tex)
					.with_normals(m.normals.iter().map(|n| n.0).collect()),
				note: None,
			}
		}
	}
}

fn physics_info(details: &mut Vec<(String, String)>, info: &rbx_mesh::union_physics::PhysicsInfo) {
	let [x, y, z] = info.center_of_gravity;
	details.push(detail("Volume", format!("{:.4}", info.volume)));
	details.push(detail("Centre of gravity", format!("{x:.3}, {y:.3}, {z:.3}")));
}

fn hulls_from_meshes(meshes: &[rbx_mesh::union_physics::Mesh]) -> Geometry {
	let mut geometry = Geometry::empty();
	geometry.group_kind = GroupKind::Hull;
	for m in meshes {
		geometry.push_part(&m.positions, m.faces.iter().map(|f| [f[0].0, f[1].0, f[2].0]));
	}
	geometry
}

fn hull_details(geometry: &Geometry) -> Vec<(String, String)> {
	vec![
		detail("Hulls", geometry.groups.len()),
		detail("Vertices", geometry.positions.len()),
		detail("Faces", geometry.triangles.len()),
	]
}

/// A unit cube, used to visualise the BLOCK physics shape.
fn unit_cube() -> Geometry {
	let positions = (0..8)
		.map(|i| {
			[
				if i & 1 == 0 { -0.5 } else { 0.5 },
				if i & 2 == 0 { -0.5 } else { 0.5 },
				if i & 4 == 0 { -0.5 } else { 0.5 },
			]
		})
		.collect();
	let quads = [
		[0, 2, 3, 1],
		[4, 5, 7, 6],
		[0, 1, 5, 4],
		[2, 6, 7, 3],
		[0, 4, 6, 2],
		[1, 3, 7, 5],
	];
	let triangles = quads
		.iter()
		.flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]])
		.collect();
	Geometry::whole(positions, triangles)
}

fn from_union_physics(mesh: UnionPhysics) -> Loaded {
	match mesh {
		UnionPhysics::CSGK(c) => csgk(&c.uuid_ascii_hex, "Union physics"),
		UnionPhysics::Block(_) => Loaded {
			format: "Union physics BLOCK".into(),
			details: Vec::new(),
			geometry: unit_cube(),
			note: Some("The physics shape is the part's own box. Shown as a unit cube.".into()),
		},
		UnionPhysics::V3(m) => {
			let geometry = hulls_from_meshes(&m.meshes);
			Loaded {
				format: "Union physics CSGPHS v3".into(),
				details: hull_details(&geometry),
				geometry,
				note: None,
			}
		}
		UnionPhysics::V5(m) => {
			let geometry = hulls_from_meshes(&m.meshes);
			Loaded {
				format: "Union physics CSGPHS v5".into(),
				details: hull_details(&geometry),
				geometry,
				note: None,
			}
		}
		UnionPhysics::V6(m) => {
			let geometry = hulls_from_meshes(std::slice::from_ref(&m.mesh));
			let mut details = hull_details(&geometry);
			physics_info(&mut details, &m.physics_info);
			Loaded {
				format: "Union physics CSGPHS v6".into(),
				details,
				geometry,
				note: None,
			}
		}
		UnionPhysics::V7(m) => {
			let geometry = hulls_from_meshes(&m.meshes);
			let mut details = hull_details(&geometry);
			details.push(detail("Geometry type", format!("{:?}", m.geom_type)));
			physics_info(&mut details, &m.physics_info);
			Loaded {
				format: "Union physics CSGPHS v7".into(),
				details,
				geometry,
				note: None,
			}
		}
		UnionPhysics::V8(m) => {
			let mut geometry = Geometry::empty();
			geometry.group_kind = GroupKind::Hull;
			for hull in m.mesh.hulls.iter_hulls() {
				geometry.push_part(hull.positions, hull.faces.iter().copied());
			}
			let mut details = hull_details(&geometry);
			details.push(detail("Raw hulls", m.mesh.raw_hull_count));
			details.push(detail("Geometry type", format!("{:?}", m.geom_type)));
			Loaded {
				format: "Union physics CSGPHS v8".into(),
				details,
				geometry,
				note: None,
			}
		}
	}
}

/// Writes the visible triangles as a Wavefront OBJ, one object per group.
pub fn to_obj(geometry: &Geometry, groups: &[usize]) -> String {
	use std::fmt::Write;
	let mut out = String::from("# Exported by rbx_mesh_gui\n");
	for p in &geometry.positions {
		let _ = writeln!(out, "v {} {} {}", p[0], p[1], p[2]);
	}
	for &g in groups {
		let label = match geometry.group_kind {
			GroupKind::Lod => "lod",
			GroupKind::Hull => "hull",
			GroupKind::Whole => "mesh",
		};
		let _ = writeln!(out, "o {label}_{g}");
		for t in &geometry.triangles[geometry.groups[g].clone()] {
			let _ = writeln!(out, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1);
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Every sample file in the repo should either decode with geometry
	/// or be a CSGK placeholder.
	#[test]
	fn loads_all_sample_meshes() {
		let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../meshes");
		for entry in std::fs::read_dir(dir).unwrap() {
			let path = entry.unwrap().path();
			if !path.is_file() {
				continue;
			}
			let bytes = std::fs::read(&path).unwrap();
			let loaded = load(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
			assert!(
				!loaded.geometry.triangles.is_empty() || loaded.note.is_some(),
				"{} produced no triangles",
				path.display()
			);
			println!(
				"{}: {} ({} tris)",
				path.display(),
				loaded.format,
				loaded.geometry.triangles.len()
			);
		}
	}
}

#[cfg(test)]
mod uv_tests {
	use super::*;

	fn load_sample(name: &str) -> Loaded {
		let path = format!("{}/../meshes/{name}", env!("CARGO_MANIFEST_DIR"));
		load(&std::fs::read(path).unwrap()).unwrap()
	}

	/// Formats that store texture coordinates should expose one per vertex.
	#[test]
	fn textured_formats_have_uvs() {
		for name in ["torso.mesh", "sphere.mesh", "13674780763", "mesh7_127279296594138.bin", "4500696697_4.meshdata"] {
			let geometry = load_sample(name).geometry;
			let uvs = geometry.uvs.unwrap_or_else(|| panic!("{name} has no uvs"));
			assert_eq!(uvs.len(), geometry.positions.len(), "{name}");
		}
	}

	#[test]
	fn physics_has_no_uvs() {
		assert!(load_sample("CSGPHS_7.data").geometry.uvs.is_none());
	}
}
