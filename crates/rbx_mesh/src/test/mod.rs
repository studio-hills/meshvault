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
mod chunks;
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
	format!("{value:#?}")
		.bytes()
		.fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
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

// used by the mesh v7, union graphics and union physics v8 tests
#[cfg_attr(
	not(any(
		feature = "mesh",
		feature = "union-graphics",
		feature = "csgphs-v8-zstd",
		feature = "csgphs-v8-ruzstd",
	)),
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

#[cfg(feature = "mesh")]
mod hardening;

/// Test allocator that refuses, on the threads that ask for it, any single
/// allocation larger than a cap. A refused allocation aborts the test binary
/// ("memory allocation of N bytes failed"), so a parser that reserves from a
/// declared count fails loudly here instead of reserving gigabytes.
struct CappedAlloc;

std::thread_local! {
	static ALLOC_CAP: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

fn alloc_cap() -> usize {
	ALLOC_CAP.try_with(|cap| cap.get()).unwrap_or(usize::MAX)
}

// SAFETY: every call is forwarded to the system allocator unchanged, or
// refused with null, which callers must handle.
unsafe impl std::alloc::GlobalAlloc for CappedAlloc {
	unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
		if alloc_cap() < layout.size() {
			return std::ptr::null_mut();
		}
		// SAFETY: forwarded with the caller's layout.
		unsafe { std::alloc::System.alloc(layout) }
	}
	unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
		if alloc_cap() < layout.size() {
			return std::ptr::null_mut();
		}
		// SAFETY: forwarded with the caller's layout.
		unsafe { std::alloc::System.alloc_zeroed(layout) }
	}
	unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
		if alloc_cap() < new_size {
			return std::ptr::null_mut();
		}
		// SAFETY: forwarded with the caller's pointer and layout.
		unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
	}
	unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
		// SAFETY: forwarded with the caller's pointer and layout.
		unsafe { std::alloc::System.dealloc(ptr, layout) }
	}
}

#[global_allocator]
static ALLOC: CappedAlloc = CappedAlloc;

/// Runs `f` with single allocations on this thread capped at `cap` bytes.
#[cfg_attr(
	not(any(
		feature = "mesh",
		feature = "union-graphics",
		feature = "csgphs-v8-zstd",
		feature = "csgphs-v8-ruzstd",
	)),
	expect(unused)
)]
pub fn with_alloc_cap<T>(cap: usize, f: impl FnOnce() -> T) -> T {
	let previous = ALLOC_CAP.with(|c| c.replace(cap));
	let value = f();
	ALLOC_CAP.with(|c| c.set(previous));
	value
}
