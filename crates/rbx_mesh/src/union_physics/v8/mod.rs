mod bit_buffer;
mod clers_symbol;
mod edgebreaker;
mod raw_hulls;
mod roblox_bit_reader;
#[cfg(test)]
mod tests;

use binrw::{BinRead, BinReaderExt};
use std::io::Read;

use crate::read::MAX_FACES;

use super::v7::GeomType7;
pub use edgebreaker::{Hull, HullDecodeError};
pub use raw_hulls::Hulls;
pub use roblox_bit_reader::BitCounterError;

/// Largest accepted decompressed size of the zstd payload. Real physics
/// data is far smaller; this stops a small compressed stream from expanding
/// without bound.
pub const MAX_DECODED_LEN: u64 = 64 << 20;

/// Hull information is accessed via mesh.hulls.iter_hulls()
#[binrw::binread]
#[br(little)]
#[br(magic = b"CSGPHS\x08\0\0\0")]
#[derive(Debug, Clone)]
pub struct CSGPHS8 {
	pub geom_type: GeomType7,
	#[br(temp)]
	#[bw(ignore)]
	#[brw(magic = 0u8)]
	_padding: (),
	#[br(parse_with = read_mesh)]
	pub mesh: Mesh8,
}

#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct Aabb {
	pub min: [f32; 3],
	pub max: [f32; 3],
}

#[binrw::binread]
#[br(little)]
struct RawMesh8 {
	hull_count: u32,
	#[br(temp)]
	position_count: u32,
	face_count: u32,
	#[br(temp)]
	first_hull_pos_count: u32,
	#[br(temp)]
	first_hull_face_count: u32,
	#[br(temp)]
	raw_hulls_len: u32,
	clers_bit_count: u32,
	#[br(temp)]
	clers_buffer_len: u32,
	#[br(temp)]
	positions_len: u32,
	aabb: Aabb,
	#[br(if(raw_hulls_len != 0))]
	raw_hulls: Hulls,
	#[br(parse_with = crate::read::bytes, args(clers_buffer_len))]
	clers_buffer: Vec<u8>,
	#[br(count = u64::from(position_count) * 3)]
	positions: Vec<f32>,
}

/// The mesh format encodes hulls in two different ways: raw, and edgebreaker.
/// This is an implementation detail of the mesh format.  The edgebreaker
/// hulls have been appended to the raw hulls for simplicity, meaning the
/// raw hulls come first in hulls.iter_hulls().
#[derive(Debug, Clone)]
pub struct Mesh8 {
	pub raw_hull_count: u32,
	pub aabb: Aabb,
	pub hulls: Hulls,
}

/// zstd's own default window limit (128 MiB), set explicitly so the
/// decoder's window buffer stays bounded whatever the frame asks for.
#[cfg(feature = "csgphs-v8-zstd")]
const WINDOW_LOG_MAX: u32 = 27;

fn invalid(pos: u64, message: String) -> binrw::Error {
	binrw::Error::AssertFail { pos, message }
}

/// Decompresses the zstd payload, refusing more than [`MAX_DECODED_LEN`]
/// bytes of output.
fn decompress<R: BinReaderExt>(reader: &mut R) -> binrw::BinResult<Vec<u8>> {
	let pos = reader.stream_position()?;
	// with both backends enabled, zstd is used
	#[cfg(all(feature = "csgphs-v8-ruzstd", not(feature = "csgphs-v8-zstd")))]
	let decoder =
		ruzstd::decoding::StreamingDecoder::new(reader).map_err(|e| binrw::Error::Custom {
			pos,
			err: Box::new(e),
		})?;
	#[cfg(feature = "csgphs-v8-zstd")]
	let decoder = {
		let custom = |e: std::io::Error| binrw::Error::Custom {
			pos,
			err: Box::new(e),
		};
		let mut decoder = zstd::stream::Decoder::new(reader).map_err(custom)?;
		decoder.window_log_max(WINDOW_LOG_MAX).map_err(custom)?;
		decoder
	};

	// reading one byte past the limit tells a stream that is too large from
	// one that ends exactly at it
	let mut decoded = Vec::new();
	decoder
		.take(MAX_DECODED_LEN + 1)
		.read_to_end(&mut decoded)?;
	if MAX_DECODED_LEN < decoded.len() as u64 {
		return Err(invalid(
			pos,
			format!("zstd data decompresses to more than {MAX_DECODED_LEN} bytes"),
		));
	}
	Ok(decoded)
}

fn read_mesh<R: BinReaderExt>(
	reader: &mut R,
	endian: binrw::Endian,
	args: (),
) -> binrw::BinResult<Mesh8> {
	let decoded = decompress(reader)?;

	let mut reader = std::io::Cursor::new(decoded);
	let pos = reader.position();
	let mesh = RawMesh8::read_options(&mut reader, endian, args)?;

	// Each edgebreaker hull is a starting face plus one face per symbol
	// (at least one bit each), and needs at least one symbol, an End of
	// three bits. So the symbol bits bound the hull and face counts, and
	// both are checked before anything is sized from them.
	let hulls = u64::from(mesh.hull_count);
	let faces = u64::from(mesh.face_count);
	let bits = u64::from(mesh.clers_bit_count);
	if bits < hulls * 3 || faces < hulls * 2 {
		return Err(invalid(
			pos,
			format!("{hulls} hulls do not fit {faces} faces and {bits} symbol bits",),
		));
	}
	if hulls + bits < faces || u64::from(MAX_FACES) < faces {
		return Err(invalid(
			pos,
			format!(
				"{faces} faces do not fit {hulls} hulls and {bits} symbol bits, or exceed {MAX_FACES}",
			),
		));
	}

	let symbol_reader = clers_symbol::SymbolReader::new(&mesh.clers_buffer, mesh.clers_bit_count)
		.map_err(|e| binrw::Error::Custom {
		pos,
		err: Box::new(e),
	})?;
	let capacity = mesh.face_count as usize * 3;
	let mut hull_decoder = edgebreaker::HullDecoder::new(symbol_reader, capacity);

	// extend raw_hulls with edgebreaker hulls (raw_hulls was checked on read)
	let Hulls {
		mut face_ranges,
		mut faces,
		mut pos_ranges,
		mut positions,
	} = mesh.raw_hulls;

	let raw_hull_count = face_ranges.len().min(pos_ranges.len()).saturating_sub(1) as u32;

	let hull_count = mesh.hull_count as usize;
	let start_face = if let Some(&start_face) = face_ranges.last() {
		face_ranges.reserve_exact(hull_count);
		start_face
	} else {
		// mesh has no raw hulls, leading zero must be added
		face_ranges.reserve_exact(hull_count + 1);
		face_ranges.push(0);
		0
	};
	let start_pos = if let Some(&start_pos) = pos_ranges.last() {
		pos_ranges.reserve_exact(hull_count);
		start_pos
	} else {
		// mesh has no raw hulls, leading zero must be added
		pos_ranges.reserve_exact(hull_count + 1);
		pos_ranges.push(0);
		0
	};

	// decode the hulls and record the ranges
	let overflow = || invalid(pos, "hull ranges overflow".to_owned());
	for _ in 0..mesh.hull_count {
		hull_decoder
			.decode_hull()
			.map_err(|e| binrw::Error::Custom {
				pos: pos + (mesh.clers_bit_count - hull_decoder.remaining_bits() / u8::BITS) as u64,
				err: Box::new(e),
			})?;
		let face_end = hull_decoder
			.current_face()
			.checked_mul(3)
			.and_then(|end| end.checked_add(start_face))
			.ok_or_else(overflow)?;
		let pos_end = hull_decoder
			.vertex_offset()
			.checked_mul(3)
			.and_then(|end| end.checked_add(start_pos))
			.ok_or_else(overflow)?;
		face_ranges.push(face_end);
		pos_ranges.push(pos_end);
	}

	// This is the final form of the list before it is passed to the user, so allocate exactly
	faces.reserve_exact(capacity);
	positions.reserve_exact(mesh.positions.len());

	faces.extend(hull_decoder.into_indices());
	positions.extend(mesh.positions);

	let hulls = Hulls {
		face_ranges,
		faces,
		pos_ranges,
		positions,
	};
	// the decoded hulls must use only the positions stored
	hulls.check().map_err(|message| invalid(pos, message))?;

	Ok(Mesh8 {
		raw_hull_count,
		aabb: mesh.aabb,
		hulls,
	})
}
