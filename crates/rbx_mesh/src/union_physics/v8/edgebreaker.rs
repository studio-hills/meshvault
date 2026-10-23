use super::clers_symbol::{Symbol, SymbolReader};
use super::roblox_bit_reader::BitCounterError;

#[derive(Debug, Clone)]
pub struct Hull<'f> {
	pub positions: &'f [[f32; 3]],
	/// 0 based indices into positions
	pub faces: &'f [[u32; 3]],
}

// non-negative edge id
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EdgeId(u32);
impl EdgeId {
	const fn idx(self) -> usize {
		let EdgeId(id) = self;
		id as usize
	}
	// rotate within the face's 3-edge slot (mod-3)
	const fn next(self) -> Self {
		let EdgeId(id) = self;
		// floor group and rotate id +1
		Self(id / 3 * 3 + (id + 1) % 3)
	}
	const fn prev(self) -> Self {
		let EdgeId(id) = self;
		// floor group and rotate id -1
		// +2 is used here to avoid underflow when id = 0
		Self(id / 3 * 3 + (id + 2) % 3)
	}
}
#[test]
fn test_edge_id() {
	assert_eq!(EdgeId(3).next(), EdgeId(4));
	assert_eq!(EdgeId(4).next(), EdgeId(5));
	assert_eq!(EdgeId(5).next(), EdgeId(3));

	assert_eq!(EdgeId(5).prev(), EdgeId(4));
	assert_eq!(EdgeId(4).prev(), EdgeId(3));
	assert_eq!(EdgeId(3).prev(), EdgeId(5));
}

#[derive(Eq, PartialEq)]
enum EdgeType {
	Adjacency(EdgeId),
	Uninit,
	Boundary,
	Processing,
	Invalid,
}

#[derive(Clone, Copy)]
struct Edge(i32);
impl Edge {
	const UNINIT: Self = Edge(-3);
	const BOUNDARY: Self = Edge(-1);
	const PROCESSING: Self = Edge(-2);
	fn ty(self) -> EdgeType {
		match self {
			Edge(id) if 0 <= id => EdgeType::Adjacency(EdgeId(id as u32)),
			Edge(-3) => EdgeType::Uninit,
			Edge(-1) => EdgeType::Boundary,
			Edge(-2) => EdgeType::Processing,
			_ => EdgeType::Invalid,
		}
	}
}
impl From<EdgeId> for Edge {
	fn from(EdgeId(id): EdgeId) -> Self {
		Self(id as i32)
	}
}

/// Why an edgebreaker hull could not be decoded.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum HullDecodeError {
	/// The symbol stream ended early.
	Bits(BitCounterError),
	/// The hulls need more faces than the mesh declares.
	TooManyFaces,
	/// An edge refers to a face that has not been decoded.
	BadEdge,
	/// Zipping the boundary took more steps than a valid hull can need.
	StepBudget,
}
impl core::fmt::Display for HullDecodeError {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		match self {
			HullDecodeError::Bits(e) => write!(f, "{e}"),
			HullDecodeError::TooManyFaces => write!(f, "hulls need more faces than declared"),
			HullDecodeError::BadEdge => write!(f, "edge refers to an undecoded face"),
			HullDecodeError::StepBudget => write!(f, "hull topology does not close"),
		}
	}
}
impl core::error::Error for HullDecodeError {}
impl From<BitCounterError> for HullDecodeError {
	fn from(e: BitCounterError) -> Self {
		Self::Bits(e)
	}
}

/// Boundary-walk steps allowed per edge of capacity. Valid hulls take a few
/// steps per edge; this only stops walks that loop on a bad topology.
const STEPS_PER_EDGE: u64 = 64;

pub struct HullDecoder<'a> {
	symbol_reader: SymbolReader<'a>,
	// adjacency[edge] = twin edge index, or one of SENTINEL_*
	adjacency: Box<[Edge]>,
	// indices[edge] = vertex id at this face corner
	indices: Box<[u32]>,
	current_face: u32,
	vertex_offset: u32,
	// boundary-walk steps left for the whole mesh
	steps_left: u64,
	// gates to resume once the branch opened by a Split ends
	splits: Vec<EdgeId>,
}

impl<'a> HullDecoder<'a> {
	/// `capacity` is the number of edges (three per face) to decode into.
	/// The caller bounds it; decoding more faces than fit is an error.
	pub fn new(symbol_reader: SymbolReader<'a>, capacity: usize) -> Self {
		// whole faces only, and edge ids must fit an `Edge`
		let capacity = capacity.min(i32::MAX as usize) / 3 * 3;
		Self {
			symbol_reader,
			adjacency: vec![Edge::UNINIT; capacity].into_boxed_slice(),
			indices: vec![0; capacity].into_boxed_slice(),
			current_face: 0,
			vertex_offset: 0,
			steps_left: (capacity as u64 + 3) * STEPS_PER_EDGE,
			splits: Vec::new(),
		}
	}
	pub const fn remaining_bits(&self) -> u32 {
		self.symbol_reader.remaining_bits()
	}
	pub const fn current_face(&self) -> u32 {
		self.current_face
	}
	pub const fn vertex_offset(&self) -> u32 {
		self.vertex_offset
	}
	pub fn into_indices(self) -> Box<[u32]> {
		self.indices
	}
	// edges of the faces decoded so far
	const fn decoded_edges(&self) -> usize {
		3 * self.current_face as usize
	}
	fn adjacency(&self, edge: EdgeId) -> Result<Edge, HullDecodeError> {
		if self.decoded_edges() <= edge.idx() {
			return Err(HullDecodeError::BadEdge);
		}
		Ok(self.adjacency[edge.idx()])
	}
	fn set_adjacency(&mut self, edge: EdgeId, value: Edge) -> Result<(), HullDecodeError> {
		if self.decoded_edges() <= edge.idx() {
			return Err(HullDecodeError::BadEdge);
		}
		self.adjacency[edge.idx()] = value;
		Ok(())
	}
	fn index(&self, edge: EdgeId) -> Result<u32, HullDecodeError> {
		if self.decoded_edges() <= edge.idx() {
			return Err(HullDecodeError::BadEdge);
		}
		Ok(self.indices[edge.idx()])
	}
	fn set_index(&mut self, edge: EdgeId, value: u32) -> Result<(), HullDecodeError> {
		if self.decoded_edges() <= edge.idx() {
			return Err(HullDecodeError::BadEdge);
		}
		self.indices[edge.idx()] = value;
		Ok(())
	}
	fn step(&mut self) -> Result<(), HullDecodeError> {
		self.steps_left = self
			.steps_left
			.checked_sub(1)
			.ok_or(HullDecodeError::StepBudget)?;
		Ok(())
	}
	/// Claims the next face, returning its first edge.
	fn emit_face(&mut self) -> Result<EdgeId, HullDecodeError> {
		let edge = self.decoded_edges();
		if self.adjacency.len() < edge + 3 {
			return Err(HullDecodeError::TooManyFaces);
		}
		self.current_face += 1;
		Ok(EdgeId(edge as u32))
	}
	fn zip_boundary(&mut self, mut current_edge: EdgeId) -> Result<(), HullDecodeError> {
		// loop while a SENTINEL_PROCESSING edge still needs to be paired;
		// every walk below takes a step, so a looping topology runs out
		while self.adjacency(current_edge)?.ty() == EdgeType::Processing {
			self.step()?;
			let mut candidate_edge = current_edge.next();

			// walk the fan via twin+next until we reach a boundary edge
			while let EdgeType::Adjacency(opposite_edge) = self.adjacency(candidate_edge)?.ty() {
				self.step()?;
				candidate_edge = opposite_edge.next();
			}

			if self.adjacency(candidate_edge)?.ty() != EdgeType::Boundary {
				break;
			}

			// link the two boundary edges as twins (zip them shut)
			self.set_adjacency(current_edge, candidate_edge.into())?;
			self.set_adjacency(candidate_edge, current_edge.into())?;

			current_edge = current_edge.prev();

			let mut prev_edge = current_edge;
			let donor = self.index(candidate_edge.prev())?;

			// rewrite the merged corner with the surviving (donor) vertex id
			self.set_index(current_edge.prev(), donor)?;

			// propagate that vertex id around the rest of the merged fan
			let mut connected_edge = self.adjacency(current_edge)?;
			while let EdgeType::Adjacency(connected_edge_id) = connected_edge.ty()
				&& candidate_edge != prev_edge
			{
				self.step()?;
				prev_edge = connected_edge_id.prev();
				self.set_index(prev_edge.prev(), donor)?;
				connected_edge = self.adjacency(prev_edge)?;
			}

			// hop along the connected fan to the next still-unzipped edge
			while let EdgeType::Adjacency(linked_edge) = self.adjacency(current_edge)?.ty()
				&& current_edge != candidate_edge
			{
				self.step()?;
				current_edge = linked_edge.prev();
			}
		}

		Ok(())
	}
	/// Reads symbols until the End that closes the hull. Symbols S and E
	/// match like parentheses: each Split opens a branch that an End closes,
	/// after which decoding resumes at the Split's other gate. The gates are
	/// kept on `self.splits`, not the call stack, so a long chain of Splits
	/// cannot overflow it; each Split also claims a face, so the list never
	/// outgrows the face capacity.
	fn decode_symbols(
		&mut self,
		vertex_count: &mut u32,
		mut cursor: EdgeId,
	) -> Result<(), HullDecodeError> {
		self.splits.clear();
		loop {
			// emit a new face and glue its edge 0 to cursor_edge as twins;
			// edges 1 and 2 inherit the corner vertices from the gate edge
			let current_edge_0 = self.emit_face()?;
			let current_edge_1 = current_edge_0.next();
			let current_edge_2 = current_edge_1.next();
			self.set_adjacency(current_edge_0, cursor.into())?;
			self.set_adjacency(current_edge_1, Edge::UNINIT)?;
			self.set_adjacency(current_edge_2, Edge::UNINIT)?;

			self.set_adjacency(cursor, current_edge_0.into())?;

			let corner = self.index(cursor.prev())?;
			self.set_index(current_edge_1, corner)?;
			let corner = self.index(cursor.next())?;
			self.set_index(current_edge_2, corner)?;

			cursor = current_edge_1;

			let symbol = self.symbol_reader.read()?;

			match symbol {
				Symbol::Continue => {
					// Create a new vertex
					self.set_index(current_edge_0, *vertex_count)?;
					self.set_adjacency(cursor.next(), Edge::BOUNDARY)?;
					*vertex_count += 1;
				}
				Symbol::Split => {
					// decode the branch from cursor, then resume at its next edge
					self.splits.push(cursor.next());
				}
				Symbol::Left => {
					self.set_adjacency(cursor, Edge::PROCESSING)?;
					cursor = cursor.next();
				}
				Symbol::Right => {
					let next_edge = cursor.next();
					self.set_adjacency(next_edge, Edge::PROCESSING)?;
					self.zip_boundary(next_edge)?;
				}
				Symbol::End => {
					self.set_adjacency(cursor, Edge::PROCESSING)?;
					let next_edge = cursor.next();
					self.set_adjacency(next_edge, Edge::PROCESSING)?;
					self.zip_boundary(next_edge)?;
					match self.splits.pop() {
						Some(gate) => cursor = gate,
						None => return Ok(()),
					}
				}
			}
		}
	}
	pub fn decode_hull(&mut self) -> Result<(), HullDecodeError> {
		// Create the starting face
		let edge = self.emit_face()?;
		let e = edge.idx();
		self.adjacency[e..e + 3].copy_from_slice(&[Edge::BOUNDARY, Edge::UNINIT, Edge::BOUNDARY]);

		// Create the starting face's vertex indices
		self.indices[e..e + 3].copy_from_slice(&[0, 1, 2]);
		let mut vertex_count = 3;

		self.decode_symbols(&mut vertex_count, edge.next())?;

		// a hull has at most two more vertices than faces, so this stays
		// within the edge capacity
		self.vertex_offset += vertex_count;

		Ok(())
	}
}
