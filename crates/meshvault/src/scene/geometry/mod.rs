//! Converts every format rbx_mesh can decode into one flat triangle list
//! that the viewer can draw and export.

use std::io::Cursor;
use std::ops::Range;

mod facs;
mod mesh;
mod skin;
mod union;

pub use self::facs::Facs;
pub use self::skin::{Skin, skin};
pub use crate::scene::primitive::Primitive;

use self::mesh::from_mesh;
use self::union::{from_union_graphics, from_union_physics};

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
	/// The groups the viewer opens the mesh with: the finest LOD, or every
	/// group (every hull).
	pub fn default_groups(&self) -> Vec<usize> {
		match self.group_kind {
			GroupKind::Lod => vec![0],
			_ => (0..self.groups.len()).collect(),
		}
	}

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

	pub(super) fn whole(positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> Self {
		let groups = single_group(triangles.len());
		Self {
			positions,
			uvs: None,
			normals: None,
			triangles,
			groups,
			group_kind: GroupKind::Whole,
		}
	}

	/// Levels of detail of one mesh: each group is a LOD, finest first.
	fn lods(positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>, groups: Vec<Range<usize>>) -> Self {
		Self {
			positions,
			uvs: None,
			normals: None,
			triangles,
			groups,
			group_kind: GroupKind::Lod,
		}
	}

	/// Attaches vertex normals, ignored unless there is one per position.
	pub(super) fn with_normals(mut self, normals: Vec<[f32; 3]>) -> Self {
		if normals.len() == self.positions.len() && !normals.is_empty() {
			self.normals = Some(normals);
		}
		self
	}

	/// Attaches texture coordinates, ignoring them if the count does not
	/// match the positions (some formats store them separately).
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

/// Positions, texture coordinates and normals, one of each per vertex.
type Attributes = (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>);

/// Splits a vertex list into its positions, texture coordinates and normals.
fn attributes<V>(
	vertices: &[V],
	pos: impl Fn(&V) -> [f32; 3],
	tex: impl Fn(&V) -> [f32; 2],
	norm: impl Fn(&V) -> [f32; 3],
) -> Attributes {
	(
		vertices.iter().map(pos).collect(),
		vertices.iter().map(tex).collect(),
		vertices.iter().map(norm).collect(),
	)
}

/// The names in a list of NUL-terminated strings, in order, skipping empty
/// ones. Bytes that are not UTF-8 become U+FFFD.
fn nul_separated(bytes: &[u8]) -> Vec<String> {
	bytes
		.split(|&b| b == 0)
		.filter(|s| !s.is_empty())
		.map(|s| String::from_utf8_lossy(s).into_owned())
		.collect()
}

/// One group of every triangle, for `len` triangles.
// `Geometry::groups` is a list of triangle ranges, so a one-range list is
// what is meant here
#[allow(clippy::single_range_in_vec_init)]
fn single_group(len: usize) -> Vec<Range<usize>> {
	vec![0..len]
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
			Err(e) => Err(format!(
				"Mesh version {version} failed to decode:\n{}",
				clean(e)
			)),
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
fn clean(e: rbx_mesh::mesh::Error) -> String {
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

#[cfg(test)]
mod tests {
	use super::*;

	/// LOD meshes open on the finest level, even with no groups at all;
	/// other kinds open with every group.
	#[test]
	fn default_groups_are_the_finest_lod_or_all() {
		let mut g = Geometry::empty();
		g.group_kind = GroupKind::Lod;
		assert_eq!(g.default_groups(), [0]);
		g.groups = vec![0..3, 3..4];
		assert_eq!(g.default_groups(), [0]);
		g.group_kind = GroupKind::Hull;
		assert_eq!(g.default_groups(), [0, 1]);
		g.group_kind = GroupKind::Whole;
		g.groups.clear();
		assert!(g.default_groups().is_empty());
	}
}
