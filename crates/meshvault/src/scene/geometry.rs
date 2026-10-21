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

	/// Attaches vertex normals, ignored unless there is one per position.
	fn with_normals(mut self, normals: Vec<[f32; 3]>) -> Self {
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
	String::from_utf8_lossy(name)
		.trim_end_matches('\0')
		.to_string()
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
			details.push(detail(
				"Draco bitstream",
				format!("{} bytes", draco.bitstream.len()),
			));
			match draco.decode() {
				Ok(core) => Some(core),
				Err(e) => {
					note = Some(format!("Could not decode the Draco compressed mesh: {e}"));
					None
				}
			}
		}
		Some(CoreMeshChunk::Unsupported(u)) => {
			note = Some(format!(
				"COREMESH chunk version {} is not supported.",
				u.version
			));
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
		let hidden = (0..hsr.bit_count as usize)
			.filter(|&i| hsr.get(i) == Some(true))
			.count();
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

/// Per-vertex bone weights from a mesh file (version 4 and later).
#[derive(Clone, Debug, PartialEq)]
pub struct Skin {
	/// Names of the bones the mesh was bound to; matched to the model's Bones.
	pub bones: Vec<String>,
	/// Each bone's parent bone.
	pub parents: Vec<Option<u16>>,
	/// Each bone's bind pose in mesh space, as CFrame components
	/// `[x, y, z, r00 .. r22]`.
	pub binds: Vec<[f32; 12]>,
	/// Up to four (index into `bones`, weight) per vertex, in the same
	/// order as [`Geometry::positions`]. Weights sum to 1, or are all 0
	/// for a vertex no bone moves.
	pub influences: Vec<[(u16, f32); 4]>,
	/// A FACS head's face poses, which move its face bones.
	pub facs: Option<Facs>,
}

/// A FACS head's face poses: how far each face bone moves for each face
/// control, and for each corrective (two or three controls together).
#[derive(Clone, Debug, PartialEq)]
pub struct Facs {
	/// The face bones, by name; they are among the skin's bones.
	pub bones: Vec<String>,
	/// The controls as `FaceControls` names them (JawDrop, LeftEyeClosed, ..).
	pub controls: Vec<String>,
	/// For each pose: the controls whose weights multiply into its weight
	/// (one for a control's own pose, two or three for a corrective).
	pub columns: Vec<Vec<usize>>,
	/// Per face bone and pose, bone-major: position offset in mesh units,
	/// then rotation in degrees about X, Y and Z.
	pub poses: Vec<[f32; 6]>,
}

impl Facs {
	/// Each face bone's offset for the given control weights (0..1), as a
	/// local transform in mesh units: the weighted sum of every pose.
	pub fn bone_offsets(&self, weight: impl Fn(&str) -> f32) -> Vec<(usize, [f32; 6])> {
		let weights: Vec<f32> = self.controls.iter().map(|c| weight(c)).collect();
		let columns = self.columns.len();
		let mut sums = vec![[0.0f32; 6]; self.bones.len()];
		for (j, controls) in self.columns.iter().enumerate() {
			let w: f32 = controls.iter().map(|&c| weights[c]).product();
			if w == 0.0 {
				continue;
			}
			for (b, sum) in sums.iter_mut().enumerate() {
				let pose = &self.poses[b * columns + j];
				for k in 0..6 {
					sum[k] += w * pose[k];
				}
			}
		}
		sums.into_iter()
			.enumerate()
			.filter(|(_, s)| s.iter().any(|v| *v != 0.0))
			.collect()
	}
}

/// The full `FaceControls` name of a FACS control as a mesh abbreviates it
/// (Roblox's FACS pose reference, with the spellings heads use).
fn facs_control_name(code: &str) -> Option<&'static str> {
	let (side, pose) = code.split_once('_')?;
	let pose = pose.to_ascii_uppercase();
	let both = |left: &'static str, right: &'static str| match side {
		"l" | "L" => Some(left),
		"r" | "R" => Some(right),
		_ => None,
	};
	Some(match pose.as_str() {
		"COR" => "Corrugator",
		"CR" => "ChinRaiser",
		"CRUL" | "CRU" => "ChinRaiserUpperLip",
		"ELD" => "EyesLookDown",
		"ELL" => "EyesLookLeft",
		"ELR" => "EyesLookRight",
		"ELU" => "EyesLookUp",
		"FN" | "FU" => "Funneler",
		"FP" => "FlatPucker",
		"JD" => "JawDrop",
		"JL" => "JawLeft",
		"JR" => "JawRight",
		"LLS" => "LowerLipSuck",
		"LP" => "LipPresser",
		"LPT" | "LT" => "LipsTogether",
		"ML" => "MouthLeft",
		"MR" => "MouthRight",
		"PK" => "Pucker",
		"TD" => "TongueDown",
		"TO" => "TongueOut",
		"TU" => "TongueUp",
		"ULS" => "UpperLipSuck",
		"BL" => both("LeftBrowLowerer", "RightBrowLowerer")?,
		"CHP" | "CP" => both("LeftCheekPuff", "RightCheekPuff")?,
		"CHR" => both("LeftCheekRaiser", "RightCheekRaiser")?,
		"DM" | "DP" => both("LeftDimpler", "RightDimpler")?,
		"EC" => both("LeftEyeClosed", "RightEyeClosed")?,
		"EULR" | "EUL" => both("LeftEyeUpperLidRaiser", "RightEyeUpperLidRaiser")?,
		"IBR" => both("LeftInnerBrowRaiser", "RightInnerBrowRaiser")?,
		"LCD" => both("LeftLipCornerDown", "RightLipCornerDown")?,
		"LCP" => both("LeftLipCornerPuller", "RightLipCornerPuller")?,
		"LLD" => both("LeftLowerLipDepressor", "RightLowerLipDepressor")?,
		"LS" => both("LeftLipStretcher", "RightLipStretcher")?,
		"NW" => both("LeftNoseWrinkler", "RightNoseWrinkler")?,
		"OBR" => both("LeftOuterBrowRaiser", "RightOuterBrowRaiser")?,
		"ULR" => both("LeftUpperLipRaiser", "RightUpperLipRaiser")?,
		_ => return None,
	})
}

fn facs_of(f: &rbx_mesh::mesh::Facs5) -> Option<Facs> {
	use rbx_mesh::mesh::QuantizedMatrix5;
	let names = |b: &[u8]| -> Vec<String> {
		b.split(|&c| c == 0)
			.filter(|s| !s.is_empty())
			.map(|s| String::from_utf8_lossy(s).into_owned())
			.collect()
	};
	let bones = names(&f.face_bone_names);
	let codes = names(&f.face_control_names);
	let controls: Vec<String> = codes
		.iter()
		.map(|c| facs_control_name(c).map_or_else(|| c.clone(), String::from))
		.collect();
	let mut columns: Vec<Vec<usize>> = (0..controls.len()).map(|c| vec![c]).collect();
	columns.extend(
		f.two_pose_correctives
			.iter()
			.map(|p| p.0.iter().map(|c| c.0 as usize).collect()),
	);
	columns.extend(
		f.three_pose_correctives
			.iter()
			.map(|p| p.0.iter().map(|c| c.0 as usize).collect()),
	);
	let values = |m: &QuantizedMatrix5| -> Vec<f32> {
		match m {
			QuantizedMatrix5::Raw { matrix, .. } => matrix.clone(),
			QuantizedMatrix5::Quantized {
				lerp0,
				lerp1,
				matrix,
				..
			} => matrix
				.iter()
				.map(|&q| lerp0 + (lerp1 - lerp0) * q as f32 / 65535.0)
				.collect(),
		}
	};
	let q = &f.quantized_transforms;
	let parts = [&q.px, &q.py, &q.pz, &q.rx, &q.ry, &q.rz].map(values);
	let count = bones.len() * columns.len();
	if parts.iter().any(|p| p.len() != count)
		|| columns.iter().flatten().any(|&c| c >= controls.len())
	{
		return None;
	}
	let poses = (0..count)
		.map(|i| [0, 1, 2, 3, 4, 5].map(|k| parts[k][i]))
		.collect();
	Some(Facs {
		bones,
		controls,
		columns,
		poses,
	})
}

/// Reads a mesh's skinning. `Ok(None)` when it has none; an error when it
/// has some that cannot be lined up with the decoded vertices.
pub fn skin(bytes: &[u8], vertex_count: usize) -> Result<Option<Skin>, String> {
	if !bytes.starts_with(b"version ") {
		return Ok(None);
	}
	let mesh = rbx_mesh::read_mesh_versioned(Cursor::new(bytes)).map_err(clean)?;
	match &mesh {
		Mesh::V4(m) => build_skin(
			&m.envelopes,
			&m.bones,
			&m.bone_names,
			&m.subsets,
			vertex_count,
		),
		Mesh::V5(m) => build_skin(
			&m.envelopes,
			&m.bones,
			&m.bone_names,
			&m.subsets,
			vertex_count,
		)
		.map(|s| {
			s.map(|s| Skin {
				facs: facs_of(&m.facs),
				..s
			})
		}),
		Mesh::V6(m) => chunked_skin(m, vertex_count),
		Mesh::V7(m) => chunked_skin(m, vertex_count),
		_ => Ok(None),
	}
}

fn chunked_skin(m: &impl Chunks6, vertex_count: usize) -> Result<Option<Skin>, String> {
	let Some(s) = m.skinning() else {
		return Ok(None);
	};
	if s.bones.is_empty() {
		return Ok(None);
	}
	// "DRACO", version major and minor, encoder type, then the method:
	// edgebreaker (1) renumbers vertices, so the weights no longer line up
	if let Some(CoreMeshChunk::V2(draco)) = m.core_mesh()
		&& draco.bitstream.get(8) != Some(&0)
	{
		return Err(
			"its vertices are Draco compressed with edgebreaker, which reorders them away from their bone weights"
				.into(),
		);
	}
	let facs = m.facs().and_then(facs_of);
	build_skin(
		&s.envelopes,
		&s.bones,
		&s.bone_names,
		&s.subsets,
		vertex_count,
	)
	.map(|skin| skin.map(|skin| Skin { facs, ..skin }))
}

fn build_skin(
	envelopes: &[rbx_mesh::mesh::Envelope4],
	bones: &[rbx_mesh::mesh::Bone4],
	names: &[u8],
	subsets: &[rbx_mesh::mesh::Subset4],
	vertex_count: usize,
) -> Result<Option<Skin>, String> {
	if bones.is_empty() {
		return Ok(None);
	}
	if envelopes.len() != vertex_count {
		return Err(format!(
			"it has {} bone weights for {vertex_count} vertices",
			envelopes.len()
		));
	}
	let name_at = |pos: u32| {
		let rest = names.get(pos as usize..).unwrap_or_default();
		let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
		String::from_utf8_lossy(&rest[..end]).into_owned()
	};
	let bone_names = bones.iter().map(|b| name_at(b.bone_name_pos)).collect();
	let parents = bones
		.iter()
		.map(|b| b.parent.get().filter(|&p| (p as usize) < bones.len()))
		.collect();
	let binds = bones
		.iter()
		.map(|b| {
			let c = &b.cframe;
			[
				c.x, c.y, c.z, c.r00, c.r01, c.r02, c.r10, c.r11, c.r12, c.r20, c.r21, c.r22,
			]
		})
		.collect();
	let mut influences = vec![[(0u16, 0.0f32); 4]; vertex_count];
	// envelope bone numbers index the bone table of the subset holding the vertex
	for subset in subsets {
		let start = subset.vertices_offset as usize;
		let end = (start + subset.vertices_len as usize).min(vertex_count);
		for v in start.min(end)..end {
			let e = &envelopes[v];
			let mut out = [(0u16, 0.0f32); 4];
			let mut total = 0.0;
			for ((slot, &local), &weight) in out.iter_mut().zip(&e.bones).zip(&e.weights) {
				let weight = weight as f32;
				let bone = subset
					.bones
					.get(local as usize)
					.and_then(|b| b.get())
					.filter(|&b| (b as usize) < bones.len());
				if let (true, Some(bone)) = (weight > 0.0, bone) {
					*slot = (bone, weight);
					total += weight;
				}
			}
			if total > 0.0 {
				for o in &mut out {
					o.1 /= total;
				}
			}
			influences[v] = out;
		}
	}
	Ok(Some(Skin {
		bones: bone_names,
		parents,
		binds,
		influences,
		facs: None,
	}))
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
	details.push(detail(
		"Centre of gravity",
		format!("{x:.3}, {y:.3}, {z:.3}"),
	));
}

fn hulls_from_meshes(meshes: &[rbx_mesh::union_physics::Mesh]) -> Geometry {
	let mut geometry = Geometry::empty();
	geometry.group_kind = GroupKind::Hull;
	for m in meshes {
		geometry.push_part(
			&m.positions,
			m.faces.iter().map(|f| [f[0].0, f[1].0, f[2].0]),
		);
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

/// Roblox part shapes, as unit-sized solids centred on the origin; a part
/// scales one to its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primitive {
	Block,
	/// A sphere (Ball parts, and SpecialMesh spheres, which stretch).
	Ball,
	/// A cylinder along X, as Part.Shape = Cylinder.
	CylinderX,
	/// A SpecialMesh head: an upright cylinder whose top and bottom edges
	/// are rounded by a quarter of its height, as measured in Roblox Studio.
	Head,
	/// Full height at the back (+Z), sloping down to the front (-Z).
	Wedge,
	/// A pyramid with its peak over the front right corner (+X, -Z): the
	/// top is at height `min(x, -z)`, measured in Roblox Studio.
	CornerWedge,
	/// A TrussPart (AlternatingSupports): 2-stud cubes stacked `segments`
	/// times along `axis` (0 = X, 1 = Y, 2 = Z), each framed by corner
	/// posts and rings and crossed by an X brace on every side.
	Truss {
		segments: u16,
		axis: u8,
	},
}

impl Primitive {
	pub fn geometry(self) -> &'static Geometry {
		use std::sync::{Mutex, OnceLock};
		if let Primitive::Truss { segments, axis } = self {
			// one per length in use, kept for the life of the app
			static TRUSSES: Mutex<Vec<((u16, u8), &'static Geometry)>> = Mutex::new(Vec::new());
			let mut built = TRUSSES.lock().unwrap_or_else(|e| e.into_inner());
			if let Some((_, g)) = built.iter().find(|(k, _)| *k == (segments, axis)) {
				return g;
			}
			let g: &'static Geometry = Box::leak(Box::new(unit_truss(segments, axis)));
			built.push(((segments, axis), g));
			return g;
		}
		static SHAPES: [OnceLock<Geometry>; 6] = [const { OnceLock::new() }; 6];
		let (slot, make): (usize, fn() -> Geometry) = match self {
			Primitive::Block => (0, unit_cube),
			Primitive::Ball => (1, unit_sphere),
			Primitive::CylinderX => (2, unit_cylinder),
			Primitive::Head => (3, unit_head),
			Primitive::Wedge => (4, unit_wedge),
			Primitive::CornerWedge => (5, unit_corner_wedge),
			Primitive::Truss { .. } => unreachable!(),
		};
		SHAPES[slot].get_or_init(make)
	}
}

/// Turns every triangle of a convex solid centred on the origin to face
/// outwards (counter-clockwise from outside).
fn face_outwards(positions: &[[f32; 3]], triangles: &mut [[u32; 3]]) {
	for t in triangles {
		let [a, b, c] = t.map(|i| positions[i as usize]);
		let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
		let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
		let n = [
			ab[1] * ac[2] - ab[2] * ac[1],
			ab[2] * ac[0] - ab[0] * ac[2],
			ab[0] * ac[1] - ab[1] * ac[0],
		];
		let centre = [
			(a[0] + b[0] + c[0]) / 3.0,
			(a[1] + b[1] + c[1]) / 3.0,
			(a[2] + b[2] + c[2]) / 3.0,
		];
		if n[0] * centre[0] + n[1] * centre[1] + n[2] * centre[2] < 0.0 {
			t.swap(1, 2);
		}
	}
}

/// A sphere of diameter 1 with smooth normals.
fn unit_sphere() -> Geometry {
	const RINGS: u32 = 16;
	const SEGMENTS: u32 = 32;
	let mut positions = Vec::new();
	let mut normals = Vec::new();
	for r in 0..=RINGS {
		let phi = std::f32::consts::PI * r as f32 / RINGS as f32;
		for s in 0..=SEGMENTS {
			let theta = std::f32::consts::TAU * s as f32 / SEGMENTS as f32;
			let n = [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()];
			positions.push(n.map(|v| v * 0.5));
			normals.push(n);
		}
	}
	let row = SEGMENTS + 1;
	let mut triangles = Vec::new();
	for r in 0..RINGS {
		for s in 0..SEGMENTS {
			let (a, b) = (r * row + s, (r + 1) * row + s);
			if r > 0 {
				triangles.push([a, b, a + 1]);
			}
			if r + 1 < RINGS {
				triangles.push([a + 1, b, b + 1]);
			}
		}
	}
	face_outwards(&positions, &mut triangles);
	Geometry::whole(positions, triangles).with_normals(normals)
}

/// The SpecialMesh head, height and diameter 1 along Y: a surface of
/// revolution whose profile is straight in the middle half and a quarter
/// circle of radius 0.25 at the top and bottom, closed by flat caps.
fn unit_head() -> Geometry {
	use std::f32::consts::{FRAC_PI_2, TAU};
	const SEGMENTS: u32 = 32;
	const ARC: u32 = 6;
	const R: f32 = 0.25;
	// the profile from the bottom cap's edge to the top cap's edge:
	// (radius, height, normal radius, normal height)
	let mut profile = Vec::new();
	for (centre, start) in [(-R, -FRAC_PI_2), (R, 0.0)] {
		for k in 0..=ARC {
			let a = start + FRAC_PI_2 * k as f32 / ARC as f32;
			profile.push((R + R * a.cos(), centre + R * a.sin(), a.cos(), a.sin()));
		}
	}
	let mut positions = Vec::new();
	let mut normals = Vec::new();
	let mut triangles = Vec::new();
	let row = profile.len() as u32;
	for s in 0..=SEGMENTS {
		let (sin, cos) = (TAU * s as f32 / SEGMENTS as f32).sin_cos();
		for &(r, y, nr, ny) in &profile {
			positions.push([r * cos, y, r * sin]);
			normals.push([nr * cos, ny, nr * sin]);
		}
	}
	for s in 0..SEGMENTS {
		for k in 0..row - 1 {
			let (a, b) = (s * row + k, (s + 1) * row + k);
			triangles.push([a, b, a + 1]);
			triangles.push([a + 1, b, b + 1]);
		}
	}
	// flat caps inside the rounded edges
	for y in [-0.5f32, 0.5] {
		let centre = positions.len() as u32;
		positions.push([0.0, y, 0.0]);
		normals.push([0.0, y.signum(), 0.0]);
		for s in 0..=SEGMENTS {
			let (sin, cos) = (TAU * s as f32 / SEGMENTS as f32).sin_cos();
			positions.push([R * cos, y, R * sin]);
			normals.push([0.0, y.signum(), 0.0]);
		}
		for s in 0..SEGMENTS {
			triangles.push([centre, centre + 1 + s, centre + 2 + s]);
		}
	}
	face_outwards(&positions, &mut triangles);
	Geometry::whole(positions, triangles).with_normals(normals)
}

/// A cylinder of length and diameter 1 along X, with smooth sides and flat caps.
fn unit_cylinder() -> Geometry {
	const SEGMENTS: u32 = 32;
	let mut positions = Vec::new();
	let mut normals = Vec::new();
	let mut triangles = Vec::new();
	let ring = |x: f32, s: u32| {
		let a = std::f32::consts::TAU * s as f32 / SEGMENTS as f32;
		([x, 0.5 * a.cos(), 0.5 * a.sin()], [0.0, a.cos(), a.sin()])
	};
	// sides
	for s in 0..=SEGMENTS {
		for x in [-0.5, 0.5] {
			let (p, n) = ring(x, s);
			positions.push(p);
			normals.push(n);
		}
	}
	for s in 0..SEGMENTS {
		let i = s * 2;
		triangles.push([i, i + 1, i + 2]);
		triangles.push([i + 1, i + 3, i + 2]);
	}
	// caps, each a fan around its centre
	for x in [-0.5f32, 0.5] {
		let centre = positions.len() as u32;
		positions.push([x, 0.0, 0.0]);
		normals.push([x.signum(), 0.0, 0.0]);
		for s in 0..=SEGMENTS {
			positions.push(ring(x, s).0);
			normals.push([x.signum(), 0.0, 0.0]);
		}
		for s in 0..SEGMENTS {
			triangles.push([centre, centre + 1 + s, centre + 2 + s]);
		}
	}
	face_outwards(&positions, &mut triangles);
	Geometry::whole(positions, triangles).with_normals(normals)
}

/// A truss of `segments` 2-stud cubes along `axis`, built in studs and then
/// scaled to the unit box (a part scales it back to its size). Beam sizes
/// are from Roblox Studio: posts and rings 0.3 studs, braces 0.2.
fn unit_truss(segments: u16, axis: u8) -> Geometry {
	const POST: f32 = 0.3;
	const BRACE: f32 = 0.2;
	let n = segments.max(1) as f32;
	let length = 2.0 * n;
	let mut positions: Vec<[f32; 3]> = Vec::new();
	let mut triangles: Vec<[u32; 3]> = Vec::new();
	// a box from its centre and three half-axes (studs, truss along Y)
	let mut add_box = |c: [f32; 3], a: [f32; 3], b: [f32; 3], d: [f32; 3]| {
		let base = positions.len() as u32;
		for i in 0..8 {
			let s = |bit: u32| if i & bit == 0 { -1.0 } else { 1.0 };
			positions.push([0, 1, 2].map(|k| c[k] + s(1) * a[k] + s(2) * b[k] + s(4) * d[k]));
		}
		let mut faces: Vec<[u32; 3]> = [
			[0, 2, 6, 4],
			[1, 5, 7, 3],
			[0, 4, 5, 1],
			[2, 3, 7, 6],
			[0, 1, 3, 2],
			[4, 6, 7, 5],
		]
		.iter()
		.flat_map(|f| [[f[0], f[1], f[2]], [f[0], f[2], f[3]]])
		.map(|t| t.map(|i| base + i))
		.collect();
		let local: Vec<[f32; 3]> = positions[base as usize..]
			.iter()
			.map(|p| [p[0] - c[0], p[1] - c[1], p[2] - c[2]])
			.collect();
		let mut shifted = faces.clone();
		for t in &mut shifted {
			*t = t.map(|i| i - base);
		}
		face_outwards(&local, &mut shifted);
		for (t, s) in faces.iter_mut().zip(shifted) {
			*t = s.map(|i| i + base);
		}
		triangles.extend(faces);
	};
	let h = 1.0 - POST / 2.0;
	let half = length / 2.0;
	// corner posts, full length
	for (x, z) in [(-h, -h), (h, -h), (h, h), (-h, h)] {
		add_box(
			[x, 0.0, z],
			[POST / 2.0, 0.0, 0.0],
			[0.0, half, 0.0],
			[0.0, 0.0, POST / 2.0],
		);
	}
	for k in 0..=segments.max(1) {
		let y = (-half + 2.0 * k as f32).clamp(-half + POST / 2.0, half - POST / 2.0);
		// a ring of four bars at each segment boundary
		for (c, a, d) in [
			([0.0, y, -h], [1.0, 0.0, 0.0], [0.0, 0.0, POST / 2.0]),
			([0.0, y, h], [1.0, 0.0, 0.0], [0.0, 0.0, POST / 2.0]),
			([-h, y, 0.0], [POST / 2.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
			([h, y, 0.0], [POST / 2.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
		] {
			add_box(c, a, [0.0, POST / 2.0, 0.0], d);
		}
		if k == segments.max(1) {
			break;
		}
		// an X brace on each side of the segment above
		let mid = -half + 2.0 * k as f32 + 1.0;
		for (along, normal, at) in [
			([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], -h),
			([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], h),
			([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], -h),
			([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], h),
		] {
			for slope in [-1.0f32, 1.0] {
				// from corner to corner of the 2 x 2 face, inside the frame
				let dir = [along[0] * slope, 1.0, along[2] * slope];
				let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
				let u = dir.map(|v| v / len);
				// across the bar, in the face
				let p = [
					u[1] * normal[2] - u[2] * normal[1],
					u[2] * normal[0] - u[0] * normal[2],
					u[0] * normal[1] - u[1] * normal[0],
				];
				let c = [normal[0] * at, mid, normal[2] * at];
				add_box(
					c,
					u.map(|v| v * (h * std::f32::consts::SQRT_2 - POST / 2.0)),
					p.map(|v| v * BRACE / 2.0),
					normal.map(|v| v * BRACE / 2.0),
				);
			}
		}
	}
	// studs to the unit box, then the long side onto `axis`
	let unit = |p: [f32; 3]| {
		let q = [p[0] / 2.0, p[1] / length, p[2] / 2.0];
		match axis {
			0 => [q[1], q[0], q[2]],
			2 => [q[0], q[2], q[1]],
			_ => q,
		}
	};
	let positions = positions.into_iter().map(unit).collect();
	if axis != 1 {
		// swapping two axes mirrors the solid: turn the faces back outwards
		for t in &mut triangles {
			t.swap(1, 2);
		}
	}
	Geometry::whole(positions, triangles)
}

/// Flat-shaded (no normals) solid from corner points and faces.
fn solid(positions: Vec<[f32; 3]>, faces: &[&[u32]]) -> Geometry {
	let mut triangles: Vec<[u32; 3]> = faces
		.iter()
		.flat_map(|f| (1..f.len() - 1).map(|i| [f[0], f[i], f[i + 1]]))
		.collect();
	// the centre of a wedge is not inside all its faces' planes the same way
	// a box's is, so orient against the solid's centroid instead
	let n = positions.len() as f32;
	let c = positions.iter().fold([0.0f32; 3], |acc, p| {
		[acc[0] + p[0] / n, acc[1] + p[1] / n, acc[2] + p[2] / n]
	});
	let shifted: Vec<[f32; 3]> = positions
		.iter()
		.map(|p| [p[0] - c[0], p[1] - c[1], p[2] - c[2]])
		.collect();
	face_outwards(&shifted, &mut triangles);
	Geometry::whole(positions, triangles)
}

fn unit_wedge() -> Geometry {
	let h = 0.5;
	solid(
		vec![
			[-h, -h, -h],
			[h, -h, -h],
			[h, -h, h],
			[-h, -h, h],
			[h, h, h],
			[-h, h, h],
		],
		&[
			&[0, 1, 2, 3], // bottom
			&[2, 4, 5, 3], // back
			&[0, 5, 4, 1], // slope
			&[0, 3, 5],    // left
			&[1, 4, 2],    // right
		],
	)
}

fn unit_corner_wedge() -> Geometry {
	let h = 0.5;
	solid(
		vec![
			[-h, -h, -h],
			[h, -h, -h],
			[h, -h, h],
			[-h, -h, h],
			[h, h, -h],
		],
		&[
			&[0, 1, 2, 3], // bottom
			&[0, 4, 1],    // front, upright
			&[1, 4, 2],    // right, upright
			&[0, 3, 4],    // left slope
			&[3, 2, 4],    // back slope
		],
	)
}

/// A unit cube: the Block primitive, and the BLOCK physics shape.
pub fn unit_cube() -> Geometry {
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

#[cfg(test)]
mod tests {
	use super::*;

	/// Every sample file in the repo should either decode with geometry
	/// or be a CSGK placeholder.
	#[test]
	fn loads_all_sample_meshes() {
		let dir = crate::test_support::SAMPLE_MESHES;
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

	#[test]
	fn sample_skins_line_up_with_vertices() {
		let dir = crate::test_support::SAMPLE_MESHES;
		let mut skinned = Vec::new();
		for entry in std::fs::read_dir(dir).unwrap() {
			let path = entry.unwrap().path();
			if !path.is_file() {
				continue;
			}
			let bytes = std::fs::read(&path).unwrap();
			let Ok(loaded) = load(&bytes) else { continue };
			let n = loaded.geometry.positions.len();
			match skin(&bytes, n) {
				Ok(Some(s)) => {
					skinned.push(path.file_name().unwrap().to_string_lossy().into_owned());
					assert_eq!(s.influences.len(), n);
					for inf in &s.influences {
						let total: f32 = inf.iter().map(|i| i.1).sum();
						assert!(total == 0.0 || (total - 1.0).abs() < 1e-4);
						assert!(inf.iter().all(|i| (i.0 as usize) < s.bones.len()));
					}
					println!(
						"{}: {} bones {:?}",
						path.display(),
						s.bones.len(),
						&s.bones[..s.bones.len().min(4)]
					);
				}
				Ok(None) => {}
				Err(e) => println!("{}: skin unavailable: {e}", path.display()),
			}
		}
		println!("{} skinned samples", skinned.len());
		// skin errors are printed, not counted: a known skinned sample must
		// really skin
		assert!(!skinned.is_empty());
		assert!(
			skinned.iter().any(|n| n == "13674780763"),
			"the skinned head should skin: {skinned:?}"
		);
	}

	/// A FACS block is dropped when its pose matrices do not hold one value
	/// per bone and pose, or a corrective names a control that is not there.
	#[test]
	fn facs_rejects_mismatched_poses_and_correctives() {
		use rbx_mesh::mesh::{
			ControlId5, Facs5, QuantizedMatrix5, QuantizedTransforms5, TwoPoseCorrective5,
		};
		let matrix = |n: usize| QuantizedMatrix5::Raw {
			x: n as u32,
			y: 1,
			matrix: vec![0.5; n],
		};
		// one bone and two controls; `poses` is how many values each of the
		// six pose matrices holds (one bone times three poses is right)
		let facs = |poses: [usize; 6], correctives: Vec<[u16; 2]>| Facs5 {
			face_bone_names: b"jaw\0".to_vec(),
			face_control_names: b"c_JD\0c_PK\0".to_vec(),
			quantized_transforms: QuantizedTransforms5 {
				px: matrix(poses[0]),
				py: matrix(poses[1]),
				pz: matrix(poses[2]),
				rx: matrix(poses[3]),
				ry: matrix(poses[4]),
				rz: matrix(poses[5]),
			},
			two_pose_correctives: correctives
				.into_iter()
				.map(|[a, b]| TwoPoseCorrective5([ControlId5(a), ControlId5(b)]))
				.collect(),
			three_pose_correctives: Vec::new(),
		};
		let ok = facs_of(&facs([3; 6], vec![[0, 1]])).expect("a valid block");
		assert_eq!(ok.controls, ["JawDrop", "Pucker"]);
		assert_eq!(ok.columns, [vec![0], vec![1], vec![0, 1]]);
		assert_eq!(ok.poses.len(), 3);
		// one matrix a value short
		assert!(facs_of(&facs([3, 3, 3, 3, 2, 3], vec![[0, 1]])).is_none());
		// a corrective of control 2, of two
		assert!(facs_of(&facs([3; 6], vec![[0, 2]])).is_none());
	}

	/// A truss fills its unit box with beams: every box faces outwards (its
	/// signed volume is positive) and lengthening it adds segments.
	#[test]
	fn trusses_are_framed_segments() {
		for axis in 0..3u8 {
			let g = Primitive::Truss { segments: 3, axis }.geometry();
			let (lo, hi) = g.bounds().unwrap();
			for k in 0..3 {
				assert!(
					(lo[k] + 0.5).abs() < 1e-5 && (hi[k] - 0.5).abs() < 1e-5,
					"axis {axis}"
				);
			}
			let volume: f32 = g
				.triangles
				.iter()
				.map(|t| {
					let [a, b, c] = t.map(|i| g.positions[i as usize]);
					(a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
						+ a[2] * (b[0] * c[1] - b[1] * c[0]))
						/ 6.0
				})
				.sum();
			// open beams, far from a solid box
			assert!(volume > 0.05 && volume < 0.5, "axis {axis}: {volume}");
		}
		let one = Primitive::Truss {
			segments: 1,
			axis: 1,
		}
		.geometry();
		let three = Primitive::Truss {
			segments: 3,
			axis: 1,
		}
		.geometry();
		assert!(three.triangles.len() > 2 * one.triangles.len());
	}

	/// A FACS pose adds its offsets by its control's weight; a corrective
	/// adds by the product of its controls' weights; the abbreviations heads
	/// use become FaceControls names.
	#[test]
	fn facs_poses_add_by_weight() {
		let facs = Facs {
			bones: vec!["jaw".into(), "lip".into()],
			controls: vec!["JawDrop".into(), "Pucker".into()],
			columns: vec![vec![0], vec![1], vec![0, 1]],
			// bone-major: jaw [JawDrop, Pucker, both], then lip
			poses: vec![
				[0.0, -1.0, 0.0, 0.0, 0.0, 0.0],
				[0.0; 6],
				[0.0, 0.5, 0.0, 0.0, 0.0, 0.0],
				[0.0; 6],
				[0.0, 0.0, -1.0, 0.0, 0.0, 0.0],
				[0.0; 6],
			],
		};
		let offsets = facs.bone_offsets(|c| match c {
			"JawDrop" => 0.5,
			"Pucker" => 1.0,
			_ => 0.0,
		});
		// jaw: -0.5 from JawDrop, +0.25 from the corrective; lip: -1 from Pucker
		assert_eq!(offsets.len(), 2);
		assert!((offsets[0].1[1] + 0.25).abs() < 1e-6);
		assert!((offsets[1].1[2] + 1.0).abs() < 1e-6);
		assert!(facs.bone_offsets(|_| 0.0).is_empty());
		assert_eq!(facs_control_name("c_JD"), Some("JawDrop"));
		assert_eq!(facs_control_name("l_EULR"), Some("LeftEyeUpperLidRaiser"));
		assert_eq!(facs_control_name("r_CHP"), Some("RightCheekPuff"));
		assert_eq!(facs_control_name("x_Unknown"), None);
	}

	/// Signed volume by the divergence theorem: right only when the solid is
	/// closed and every triangle faces outwards.
	#[test]
	fn primitives_are_closed_outward_unit_solids() {
		use std::f32::consts::PI;
		for (shape, volume) in [
			(Primitive::Block, 1.0),
			(Primitive::Ball, PI / 6.0),
			(Primitive::CylinderX, PI / 4.0),
			// measured profile: see unit_head
			(Primitive::Head, 0.7105),
			(Primitive::Wedge, 0.5),
			(Primitive::CornerWedge, 1.0 / 3.0),
		] {
			let g = shape.geometry();
			let v: f32 = g
				.triangles
				.iter()
				.map(|t| {
					let [a, b, c] = t.map(|i| g.positions[i as usize]);
					(a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
						+ a[2] * (b[0] * c[1] - b[1] * c[0]))
						/ 6.0
				})
				.sum();
			// polygonal spheres and cylinders fall a little short
			assert!(
				(v - volume).abs() < volume * 0.03,
				"{shape:?}: volume {v}, want {volume}"
			);
			let (lo, hi) = g.bounds().unwrap();
			assert!(
				lo.iter().chain(&hi).all(|c| c.abs() <= 0.5 + 1e-6),
				"{shape:?} fits the unit box"
			);
		}
		// a cylinder part runs along X
		let (lo, hi) = Primitive::CylinderX.geometry().bounds().unwrap();
		assert!((hi[0] - lo[0] - 1.0).abs() < 1e-6 && (hi[1] - lo[1] - 1.0).abs() < 1e-3);
		// a corner wedge peaks over its front right corner, as measured in Studio
		let peak = Primitive::CornerWedge
			.geometry()
			.positions
			.iter()
			.copied()
			.max_by(|a, b| a[1].total_cmp(&b[1]))
			.unwrap();
		assert_eq!(peak, [0.5, 0.5, -0.5]);
	}

	#[test]
	fn skin_uses_subset_bone_tables() {
		use rbx_mesh::mesh::{Bone4, BoneId4, CFrame4, Envelope4, Subset4};
		let cf = CFrame4 {
			r00: 1.0,
			r01: 0.0,
			r02: 0.0,
			r10: 0.0,
			r11: 1.0,
			r12: 0.0,
			r20: 0.0,
			r21: 0.0,
			r22: 1.0,
			x: 0.0,
			y: 0.0,
			z: 0.0,
		};
		let bone = |pos| Bone4 {
			bone_name_pos: pos,
			parent: BoneId4::new(None),
			lod_parent: BoneId4::new(None),
			cull_distance: 0.0,
			cframe: cf.clone(),
		};
		let mut table = std::array::from_fn(|_| BoneId4::new(None));
		table[0] = BoneId4::new(Some(1));
		table[1] = BoneId4::new(Some(0));
		let subsets = [Subset4 {
			faces_offset: 0,
			faces_len: 0,
			vertices_offset: 0,
			vertices_len: 2,
			bone_count: 2,
			bones: table,
		}];
		let envelopes = [
			Envelope4 {
				bones: [0, 1, 0, 0],
				weights: [51, 204, 0, 0],
			},
			Envelope4 {
				bones: [0; 4],
				weights: [0; 4],
			},
		];
		let skin = build_skin(
			&envelopes,
			&[bone(0), bone(5)],
			b"Root\0Neck\0",
			&subsets,
			2,
		)
		.unwrap()
		.unwrap();
		assert_eq!(skin.bones, ["Root", "Neck"]);
		// subset slot 0 is mesh bone 1 (Neck), slot 1 is bone 0 (Root)
		assert_eq!(skin.influences[0][0].0, 1);
		assert!((skin.influences[0][0].1 - 0.2).abs() < 1e-6);
		assert_eq!(skin.influences[0][1].0, 0);
		assert!((skin.influences[0][1].1 - 0.8).abs() < 1e-6);
		assert_eq!(skin.influences[1], [(0, 0.0); 4]);
		assert!(build_skin(&envelopes, &[bone(0)], b"Root\0", &subsets, 3).is_err());
	}

	fn load_sample(name: &str) -> Loaded {
		let path = std::path::Path::new(crate::test_support::SAMPLE_MESHES).join(name);
		load(&std::fs::read(path).unwrap()).unwrap()
	}

	/// Formats that store texture coordinates should expose one per vertex.
	#[test]
	fn textured_formats_have_uvs() {
		for name in [
			"torso.mesh",
			"sphere.mesh",
			"13674780763",
			"mesh7_127279296594138.bin",
			"4500696697_4.meshdata",
		] {
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
