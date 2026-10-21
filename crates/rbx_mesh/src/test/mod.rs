/// Asserts that `value`'s pretty Debug output (`{:#?}`) has the given 64-bit
/// FNV-1a digest. This pins a whole decoded file without keeping a copy of its
/// Debug output in the repository. When a change to the output is intended,
/// print `{:#?}` to review it and take the new digest from the failure message.
macro_rules! assert_debug_digest {
	($value:expr, $digest:expr) => {
		let digest = $crate::test::debug_digest(&$value);
		assert_eq!(
			digest,
			$digest,
			"Debug output of `{}` changed (digest {digest:#018x})",
			stringify!($value)
		);
	};
}

#[cfg(feature = "mesh")]
mod mesh;
#[cfg(feature = "union-graphics")]
mod union_graphics;
#[cfg(feature = "union-physics")]
mod union_physics;

use binrw::{BinRead, BinReaderExt, BinWrite, BinWriterExt};
use std::io::Cursor;

/// 64-bit FNV-1a of `value`'s pretty Debug output.
pub fn debug_digest(value: &impl std::fmt::Debug) -> u64 {
	format!("{value:#?}").bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
		(hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
	})
}

fn binread<M>(bytes: Vec<u8>) -> binrw::BinResult<(M, Cursor<Vec<u8>>)>
where
	M: std::fmt::Debug,
	M: for<'a> BinRead<Args<'a> = ()>,
{
	let bytes_len = bytes.len() as u64;

	let mut rbuf = Cursor::new(bytes);
	let mesh: M = rbuf.read_le()?;
	assert_eq!(rbuf.position(), bytes_len, "Unread data in file");

	Ok((mesh, rbuf))
}
fn binwrite<M>(mesh: &M) -> binrw::BinResult<Cursor<Vec<u8>>>
where
	M: std::fmt::Debug,
	M: for<'a> BinWrite<Args<'a> = ()>,
{
	let mut wbuf = Cursor::new(Vec::new());
	wbuf.write_le(mesh)?;
	Ok(wbuf)
}

#[cfg_attr(
	not(any(feature = "mesh-v1", feature = "union-graphics")),
	expect(unused)
)]
pub fn readonly<M>(bytes: Vec<u8>) -> binrw::BinResult<M>
where
	M: std::fmt::Debug,
	M: for<'a> BinRead<Args<'a> = ()>,
{
	let (mesh, _rbuf) = binread(bytes)?;
	Ok(mesh)
}
pub fn roundtrip<M>(bytes: Vec<u8>) -> binrw::BinResult<M>
where
	M: std::fmt::Debug,
	M: for<'a> BinRead<Args<'a> = ()>,
	M: for<'a> BinWrite<Args<'a> = ()>,
{
	let (mesh, rbuf) = binread(bytes)?;
	let wbuf = binwrite(&mesh)?;

	assert_eq!(rbuf, wbuf, "Round trip failed");
	Ok(mesh)
}
