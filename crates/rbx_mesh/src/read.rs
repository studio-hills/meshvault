//! Reading helpers that size buffers by the data actually present rather
//! than by the counts a file declares.

use std::io::{Read, Seek, SeekFrom};

/// Largest decoded face count accepted from a compressed format (CSGPHS v8
/// edgebreaker, Draco). Uncompressed formats need no such cap: their faces
/// are read one by one, so a count is bounded by the file's own size.
#[cfg_attr(
	not(any(
		feature = "mesh-v7-draco",
		feature = "csgphs-v8-zstd",
		feature = "csgphs-v8-ruzstd"
	)),
	expect(unused)
)]
pub(crate) const MAX_FACES: u32 = 1 << 22;

/// Reads `len` bytes into a `Vec<u8>`, first checking that the stream holds
/// them. binrw's `count` reserves the whole declared length up front (up to
/// 4 GiB from a u32), so counted byte fields use this instead:
/// `#[br(parse_with = crate::read::bytes, args(len))]`.
///
/// A short stream is the same `UnexpectedEof` error binrw would give.
#[cfg_attr(
	not(any(
		feature = "mesh",
		feature = "union-graphics",
		feature = "csgphs-v8-zstd",
		feature = "csgphs-v8-ruzstd"
	)),
	expect(unused)
)]
pub(crate) fn bytes<R: Read + Seek>(
	reader: &mut R,
	_endian: binrw::Endian,
	(len,): (u32,),
) -> binrw::BinResult<Vec<u8>> {
	let pos = reader.stream_position()?;
	let end = reader.seek(SeekFrom::End(0))?;
	reader.seek(SeekFrom::Start(pos))?;
	let left = end.saturating_sub(pos);
	if left < u64::from(len) {
		return Err(std::io::Error::new(
			std::io::ErrorKind::UnexpectedEof,
			format!("{len} bytes declared, {left} left"),
		)
		.into());
	}
	let mut out = Vec::with_capacity(len as usize);
	reader.take(len.into()).read_to_end(&mut out)?;
	if out.len() != len as usize {
		return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
	}
	Ok(out)
}
