// based on https://devforum.roblox.com/t/roblox-mesh-format/326114
// v6.00 and v7.00 replace the fixed header with a list of chunks.
// Each chunk is: [u8; 8] name, u32 version, u32 size, then `size` bytes.

use binrw::{BinRead, BinReaderExt};

use super::v2::{Face2, Vertex2};
use super::v3::Lod3;
use super::v4::{Bone4, Envelope4, LodType4, Subset4};
use super::v5::Facs5;

#[binrw::binread]
#[br(little)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Revision6 {
	#[br(magic = b"version 6.00\n")]
	Version600,
}

/// Reads the u32 chunk size, parses the payload, and then skips any bytes
/// the payload did not consume so that the next chunk is read from the
/// right position.  Reading past the end of the chunk is an error.
pub(crate) fn sized<T, R>(reader: &mut R, endian: binrw::Endian, args: ()) -> binrw::BinResult<T>
where
	T: for<'a> BinRead<Args<'a> = ()>,
	R: BinReaderExt,
{
	let size = u32::read_options(reader, endian, args)?;
	let start = reader.stream_position()?;
	let end = start + size as u64;
	let value = T::read_options(reader, endian, args)?;
	let pos = reader.stream_position()?;
	if end < pos {
		return Err(binrw::Error::AssertFail {
			pos: start,
			message: format!(
				"chunk payload overran its size of {size} bytes by {}",
				pos - end
			),
		});
	}
	reader.seek(std::io::SeekFrom::Start(end))?;
	Ok(value)
}

/// COREMESH version 1: uncompressed vertices and faces, laid out the same
/// as mesh v4.00 / v5.00.
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct CoreMesh6 {
	#[br(temp)]
	vertex_count: u32,
	#[br(count = vertex_count)]
	pub vertices: Vec<Vertex2>,
	#[br(temp)]
	face_count: u32,
	#[br(count = face_count)]
	pub faces: Vec<Face2>,
}

/// COREMESH version 2: the vertices and faces are stored as a
/// [Draco](https://google.github.io/draco/) compressed bitstream.
/// Call `decode` (requires the `mesh-v7-draco` feature) to get a [`CoreMesh6`].
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct CoreMeshDraco7 {
	#[br(temp)]
	bitstream_len: u32,
	#[br(count = bitstream_len)]
	pub bitstream: Vec<u8>,
}

#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub enum CoreMeshChunk {
	#[br(magic = 1u32)]
	V1(#[br(parse_with = sized)] CoreMesh6),
	#[br(magic = 2u32)]
	V2(#[br(parse_with = sized)] CoreMeshDraco7),
	Unsupported(#[br(args(&[1, 2]))] UnsupportedVersion),
}

/// Lods are indices into faces.  See [`Lod3`].
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct Lods6 {
	pub lod_type: LodType4,
	pub lod_hq_count: u8,
	#[br(temp)]
	lod_count: u32,
	#[br(count = lod_count)]
	pub lods: Vec<Lod3>,
}

#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub enum LodsChunk {
	#[br(magic = 1u32)]
	V1(#[br(parse_with = sized)] Lods6),
	Unsupported(#[br(args(&[1]))] UnsupportedVersion),
}

/// envelopes has the same length as vertices when there are bones.
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct Skinning6 {
	#[br(temp)]
	envelope_count: u32,
	#[br(count = envelope_count)]
	pub envelopes: Vec<Envelope4>,
	#[br(temp)]
	bone_count: u32,
	#[br(count = bone_count)]
	pub bones: Vec<Bone4>,
	#[br(temp)]
	bone_names_len: u32,
	#[br(count = bone_names_len)]
	pub bone_names: Vec<u8>,
	#[br(temp)]
	subset_count: u32,
	#[br(count = subset_count)]
	pub subsets: Vec<Subset4>,
}

#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub enum SkinningChunk {
	#[br(magic = 1u32)]
	V1(#[br(parse_with = sized)] Skinning6),
	Unsupported(#[br(args(&[1]))] UnsupportedVersion),
}

#[expect(clippy::large_enum_variant)]
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub enum FacsChunk {
	#[br(magic = 1u32)]
	V1(#[br(parse_with = sized)] Facs6),
	Unsupported(#[br(args(&[1]))] UnsupportedVersion),
}

#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct Facs6 {
	// the payload repeats its own size before the FACS data
	#[br(temp)]
	facs_len: u32,
	#[br(pad_size_to = facs_len)]
	pub facs: Facs5,
}

/// One bit per face.  Faces with their bit set are culled by
/// Hidden Surface Removal when the mesh is worn as layered clothing.
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct HsrAvis6 {
	pub bit_count: u32,
	#[br(count = bit_count.div_ceil(8))]
	pub bits: Vec<u8>,
}
impl HsrAvis6 {
	pub fn get(&self, face: usize) -> Option<bool> {
		if (self.bit_count as usize) <= face {
			return None;
		}
		Some(self.bits[face / 8] & (1 << (face % 8)) != 0)
	}
}

#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub enum HsrAvisChunk {
	#[br(magic = 1u32)]
	V1(#[br(parse_with = sized)] HsrAvis6),
	Unsupported(#[br(args(&[1]))] UnsupportedVersion),
}

/// A known chunk with a version rbx_mesh does not understand.
#[binrw::binread]
#[br(little, import(known: &[u32]))]
#[derive(Debug, Clone)]
pub struct UnsupportedVersion {
	#[br(assert(!known.contains(&version), "chunk version {version} failed to parse"))]
	pub version: u32,
	#[br(temp)]
	size: u32,
	#[br(count = size)]
	pub data: Vec<u8>,
}

const KNOWN_CHUNKS: [&[u8; 8]; 5] = [
	b"COREMESH",
	b"LODS\0\0\0\0",
	b"SKINNING",
	b"FACS\0\0\0\0",
	b"HSRAVIS\0",
];

/// A chunk with a name rbx_mesh does not understand.
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct UnknownChunk {
	#[br(assert(!KNOWN_CHUNKS.contains(&&name)))]
	pub name: [u8; 8],
	pub version: u32,
	#[br(temp)]
	size: u32,
	#[br(count = size)]
	pub data: Vec<u8>,
}

#[expect(clippy::large_enum_variant)]
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub enum Chunk6 {
	#[br(magic = b"COREMESH")]
	CoreMesh(CoreMeshChunk),
	#[br(magic = b"LODS\0\0\0\0")]
	Lods(LodsChunk),
	#[br(magic = b"SKINNING")]
	Skinning(SkinningChunk),
	#[br(magic = b"FACS\0\0\0\0")]
	Facs(FacsChunk),
	#[br(magic = b"HSRAVIS\0")]
	HsrAvis(HsrAvisChunk),
	/// Chunks with unrecognised names land here.  Known chunks with an
	/// unsupported version use the `Unsupported` variant of their own enum.
	Unknown(UnknownChunk),
}

/// Typed access to the chunks of a v6.00 / v7.00 mesh.
pub trait Chunks6 {
	fn chunks(&self) -> &[Chunk6];
	fn core_mesh(&self) -> Option<&CoreMeshChunk> {
		self.chunks().iter().find_map(|c| match c {
			Chunk6::CoreMesh(c) => Some(c),
			_ => None,
		})
	}
	fn lods(&self) -> Option<&Lods6> {
		self.chunks().iter().find_map(|c| match c {
			Chunk6::Lods(LodsChunk::V1(c)) => Some(c),
			_ => None,
		})
	}
	fn skinning(&self) -> Option<&Skinning6> {
		self.chunks().iter().find_map(|c| match c {
			Chunk6::Skinning(SkinningChunk::V1(c)) => Some(c),
			_ => None,
		})
	}
	fn facs(&self) -> Option<&Facs5> {
		self.chunks().iter().find_map(|c| match c {
			Chunk6::Facs(FacsChunk::V1(c)) => Some(&c.facs),
			_ => None,
		})
	}
	fn hsr_avis(&self) -> Option<&HsrAvis6> {
		self.chunks().iter().find_map(|c| match c {
			Chunk6::HsrAvis(HsrAvisChunk::V1(c)) => Some(c),
			_ => None,
		})
	}
}

#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct Mesh6 {
	pub revision: Revision6,
	#[br(parse_with = binrw::helpers::until_eof)]
	pub chunks: Vec<Chunk6>,
}
impl Chunks6 for Mesh6 {
	fn chunks(&self) -> &[Chunk6] {
		&self.chunks
	}
}
