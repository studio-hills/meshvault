//! Crafted CSGPHS v8 inputs: inconsistent counts and range tables, a
//! decompression bomb and hostile edgebreaker strings must all be errors.

use super::clers_symbol::{SymbolReader, pack};
use super::edgebreaker::HullDecoder;
use super::{CSGPHS8, MAX_DECODED_LEN};
use crate::test::with_alloc_cap;
use binrw::BinReaderExt;
use std::io::Cursor;

const CAP: usize = 16 << 20;
// zstd blocks hold at most 128 KiB
const BLOCK: usize = 128 << 10;

/// A zstd frame of raw (stored) blocks holding `payload`.
fn zstd_raw(payload: &[u8]) -> Vec<u8> {
	// magic, no content size, 128 KiB window
	let mut out = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x38];
	let blocks: Vec<&[u8]> = if payload.is_empty() {
		vec![&[]]
	} else {
		payload.chunks(BLOCK).collect()
	};
	for (i, block) in blocks.iter().enumerate() {
		let last = u32::from(i + 1 == blocks.len());
		let header = (block.len() as u32) << 3 | last;
		out.extend(&header.to_le_bytes()[..3]);
		out.extend(*block);
	}
	out
}

/// A zstd frame of RLE blocks that decodes to `len` zero bytes.
fn zstd_zeros(len: usize) -> Vec<u8> {
	let mut out = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x38];
	let mut left = len;
	while left != 0 {
		let size = left.min(BLOCK);
		left -= size;
		let header = (size as u32) << 3 | 1 << 1 | u32::from(left == 0);
		out.extend(&header.to_le_bytes()[..3]);
		out.push(0);
	}
	out
}

fn csgphs8(zstd: Vec<u8>) -> Vec<u8> {
	let mut b = b"CSGPHS\x08\0\0\0\0\0".to_vec();
	b.extend(zstd);
	b
}

#[derive(Default)]
struct Payload {
	hull_count: u32,
	face_count: u32,
	raw_hulls: Vec<u8>,
	clers: Vec<u8>,
	clers_bits: u32,
	positions: Vec<f32>,
}
impl Payload {
	fn bytes(&self) -> Vec<u8> {
		let mut b = Vec::new();
		for v in [
			self.hull_count,
			self.positions.len() as u32 / 3,
			self.face_count,
			0,
			0,
			self.raw_hulls.len() as u32,
			self.clers_bits,
			self.clers.len() as u32,
			self.positions.len() as u32 * 4,
		] {
			b.extend(v.to_le_bytes());
		}
		b.extend([0u8; 24]); // aabb
		b.extend(&self.raw_hulls);
		b.extend(&self.clers);
		for p in &self.positions {
			b.extend(p.to_le_bytes());
		}
		b
	}
	fn read(&self) -> binrw::BinResult<CSGPHS8> {
		let bytes = csgphs8(zstd_raw(&self.bytes()));
		with_alloc_cap(CAP, || Cursor::new(bytes).read_le::<CSGPHS8>())
	}
}

fn raw_hulls(face_ranges: &[u32], faces: &[u32], pos_ranges: &[u32], positions: &[f32]) -> Vec<u8> {
	let mut b = Vec::new();
	b.extend((face_ranges.len() as u32).to_le_bytes());
	face_ranges.iter().for_each(|v| b.extend(v.to_le_bytes()));
	faces.iter().for_each(|v| b.extend(v.to_le_bytes()));
	b.extend((pos_ranges.len() as u32).to_le_bytes());
	pos_ranges.iter().for_each(|v| b.extend(v.to_le_bytes()));
	positions.iter().for_each(|v| b.extend(v.to_le_bytes()));
	b
}

/// Parses, then walks every hull the way callers do.
fn read_and_walk(payload: &Payload) -> Result<usize, String> {
	let mesh = payload.read().map_err(|e| e.to_string())?;
	Ok(mesh
		.mesh
		.hulls
		.iter_hulls()
		.map(|h| h.faces.len() + h.positions.len())
		.sum())
}

#[test]
fn a_valid_crafted_hull_reads() {
	// one tetrahedron-ish hull: start face plus an End
	let (clers, clers_bits) = pack("E");
	let payload = Payload {
		hull_count: 1,
		face_count: 2,
		clers,
		clers_bits,
		positions: vec![0.0; 9],
		..Default::default()
	};
	let mesh = payload.read().unwrap();
	let hulls: Vec<_> = mesh.mesh.hulls.iter_hulls().collect();
	assert_eq!(hulls.len(), 1);
	assert_eq!(hulls[0].faces.len(), 2);
	assert_eq!(hulls[0].positions.len(), 3);
}

#[test]
fn decompression_bomb_is_an_error() {
	// zeros decode as an empty mesh, so only the size limit can refuse this
	let len = MAX_DECODED_LEN as usize + BLOCK;
	let bytes = csgphs8(zstd_zeros(len));
	assert!(bytes.len() < 4096);
	let read = Cursor::new(bytes).read_le::<CSGPHS8>();
	assert!(
		read.is_err(),
		"a {len} byte decompression bomb was accepted"
	);
}

#[test]
fn decompressed_data_at_the_limit_is_not_refused_for_size() {
	let mut payload = Payload::default().bytes();
	payload.resize(MAX_DECODED_LEN as usize, 0);
	let bytes = csgphs8(zstd_raw(&payload));
	Cursor::new(bytes).read_le::<CSGPHS8>().unwrap();
}

#[test]
fn a_corrupt_zstd_stream_is_an_error() {
	let mut zstd = zstd_raw(&Payload::default().bytes());
	zstd.truncate(zstd.len() - 10);
	assert!(Cursor::new(csgphs8(zstd)).read_le::<CSGPHS8>().is_err());
}

#[test]
fn huge_face_count_is_an_error() {
	let (clers, clers_bits) = pack("E");
	let payload = Payload {
		hull_count: 1,
		face_count: u32::MAX,
		clers,
		clers_bits,
		positions: vec![0.0; 9],
		..Default::default()
	};
	assert!(payload.read().is_err());
}

#[test]
fn huge_hull_count_is_an_error() {
	let payload = Payload {
		hull_count: u32::MAX,
		..Default::default()
	};
	assert!(payload.read().is_err());
}

#[test]
fn hull_count_without_faces_is_an_error() {
	// MV-04: one hull, no faces, no symbols
	let payload = Payload {
		hull_count: 1,
		..Default::default()
	};
	assert!(read_and_walk(&payload).is_err());
}

#[test]
fn more_faces_than_declared_is_an_error() {
	// start face plus three more, but only two declared
	let (clers, clers_bits) = pack("CCE");
	let payload = Payload {
		hull_count: 1,
		face_count: 2,
		clers,
		clers_bits,
		positions: vec![0.0; 15],
		..Default::default()
	};
	assert!(read_and_walk(&payload).is_err());
}

#[test]
fn hull_positions_beyond_the_data_are_an_error() {
	// the hull uses three vertices, but no positions are stored
	let (clers, clers_bits) = pack("E");
	let payload = Payload {
		hull_count: 1,
		face_count: 2,
		clers,
		clers_bits,
		..Default::default()
	};
	assert!(read_and_walk(&payload).is_err());
}

#[test]
fn decreasing_raw_face_ranges_are_an_error() {
	// MV-05: face_ranges [3, 0] needs no payload but slices 3..0
	let payload = Payload {
		raw_hulls: raw_hulls(&[3, 0], &[], &[0, 0], &[]),
		..Default::default()
	};
	assert!(read_and_walk(&payload).is_err());
}

/// face ranges, faces, position ranges, positions
type Tables = (
	&'static [u32],
	&'static [u32],
	&'static [u32],
	&'static [f32],
);

#[test]
fn bad_raw_range_tables_are_errors() {
	let cases: [Tables; 5] = [
		// hull counts differ
		(&[0, 3], &[0, 1, 2], &[0, 9, 9], &[0.0; 9]),
		// does not start at zero
		(&[3, 3], &[0, 1, 2], &[0, 9], &[0.0; 9]),
		// not whole triangles
		(&[0, 2], &[0, 1], &[0, 9], &[0.0; 9]),
		// not whole positions
		(&[0, 3], &[0, 1, 2], &[0, 8], &[0.0; 8]),
		// a position range runs backwards
		(&[0, 3, 3], &[0, 1, 2], &[0, 9, 6], &[0.0; 9]),
	];
	for (face_ranges, faces, pos_ranges, positions) in cases {
		let payload = Payload {
			raw_hulls: raw_hulls(face_ranges, faces, pos_ranges, positions),
			..Default::default()
		};
		assert!(
			read_and_walk(&payload).is_err(),
			"accepted {face_ranges:?} {pos_ranges:?}"
		);
	}
}

#[test]
fn valid_raw_hulls_read() {
	let payload = Payload {
		raw_hulls: raw_hulls(&[0, 3], &[0, 1, 2], &[0, 9], &[0.0; 9]),
		..Default::default()
	};
	let mesh = payload.read().unwrap();
	assert_eq!(mesh.mesh.raw_hull_count, 1);
	assert_eq!(read_and_walk(&payload), Ok(4));
}

#[test]
fn a_long_split_chain_does_not_overflow_the_stack() {
	// recursion per Split overflows a small stack long before this
	const SPLITS: usize = 10_000;
	let symbols = "S".repeat(SPLITS) + &"E".repeat(SPLITS + 1);
	let (bytes, bits) = pack(&symbols);
	let thread = std::thread::Builder::new()
		.stack_size(64 << 10)
		.spawn(move || {
			let reader = SymbolReader::new(&bytes, bits).unwrap();
			let mut decoder = HullDecoder::new(reader, (2 * SPLITS + 2) * 3);
			decoder.decode_hull().is_ok()
		})
		.unwrap();
	thread.join().expect("decoding panicked");
}

#[test]
fn random_symbol_strings_never_panic() {
	// xorshift, so failures reproduce
	let mut state = 0x2545_f491_4f6c_dd1du64;
	let mut next = move || {
		state ^= state << 13;
		state ^= state >> 7;
		state ^= state << 17;
		state
	};
	for _ in 0..3000 {
		let len = (next() % 64) as usize;
		let symbols: String = (0..len)
			.map(|_| ['C', 'S', 'L', 'R', 'E'][(next() % 5) as usize])
			.collect();
		let faces = (next() % 80) as usize;
		let hulls = 1 + next() % 3;
		let (bytes, bits) = pack(&symbols);
		let result = std::panic::catch_unwind(|| {
			let reader = SymbolReader::new(&bytes, bits).unwrap();
			let mut decoder = HullDecoder::new(reader, faces * 3);
			for _ in 0..hulls {
				if decoder.decode_hull().is_err() {
					break;
				}
			}
		});
		assert!(result.is_ok(), "panicked on {symbols:?} with {faces} faces");
	}
}

/// A zstd frame of RLE zero blocks after `header` (magic and descriptor
/// fields), `len` bytes in all.
fn zstd_zeros_after(header: &[u8], len: usize) -> Vec<u8> {
	let mut out = header.to_vec();
	let mut left = len;
	while left != 0 {
		let size = left.min(BLOCK);
		left -= size;
		let header = (size as u32) << 3 | 1 << 1 | u32::from(left == 0);
		out.extend(&header.to_le_bytes()[..3]);
		out.push(0);
	}
	out
}

#[test]
fn a_huge_zstd_window_is_refused_before_decoding() {
	// N06: a 1 GiB window (log 30) and 256 MiB of RLE zeros; ruzstd buffers
	// a whole window before yielding output, so the output cap is too late
	let header = [0x28, 0xb5, 0x2f, 0xfd, 0x00, 20 << 3];
	let bytes = csgphs8(zstd_zeros_after(&header, 256 << 20));
	assert!(bytes.len() < 16 << 10);
	let read = with_alloc_cap(CAP, || Cursor::new(bytes).read_le::<CSGPHS8>());
	assert!(read.is_err(), "a 1 GiB zstd window was accepted");
}

#[test]
fn a_huge_single_segment_content_size_is_refused_before_decoding() {
	// single segment: the window is the declared content size, here 1 GiB
	let mut header = vec![0x28, 0xb5, 0x2f, 0xfd, 0b1010_0000];
	header.extend((1u32 << 30).to_le_bytes());
	let bytes = csgphs8(zstd_zeros_after(&header, 256 << 20));
	let read = with_alloc_cap(CAP, || Cursor::new(bytes).read_le::<CSGPHS8>());
	assert!(read.is_err(), "a 1 GiB single segment frame was accepted");
}

#[test]
fn concatenated_zstd_frames_read() {
	// both backends decode every frame, within one output budget
	let payload = Payload::default().bytes();
	let (head, tail) = payload.split_at(10);
	let mut zstd = zstd_raw(head);
	zstd.extend(zstd_raw(tail));
	with_alloc_cap(CAP, || Cursor::new(csgphs8(zstd)).read_le::<CSGPHS8>()).unwrap();
}

#[test]
fn concatenated_zstd_frames_share_the_output_limit() {
	let half = MAX_DECODED_LEN as usize / 2 + BLOCK;
	let mut zstd = zstd_zeros(half);
	zstd.extend(zstd_zeros(half));
	assert!(Cursor::new(csgphs8(zstd)).read_le::<CSGPHS8>().is_err());
}

#[cfg(not(feature = "csgphs-v8-zstd"))]
#[test]
fn a_skippable_zstd_frame_is_refused() {
	let mut zstd = vec![0x50, 0x2a, 0x4d, 0x18];
	zstd.extend(u32::MAX.to_le_bytes());
	zstd.extend(zstd_raw(&Payload::default().bytes()));
	assert!(Cursor::new(csgphs8(zstd)).read_le::<CSGPHS8>().is_err());
}
