use super::v6::{Chunk6, Chunks6};

#[binrw::binread]
#[br(little)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Revision7 {
	#[br(magic = b"version 7.00\n")]
	Version700,
}

/// v7.00 uses the same chunks as v6.00, but the COREMESH chunk is
/// usually Draco compressed. A truncated or over-declared last chunk is
/// treated as for [`Mesh6`](super::Mesh6).
#[binrw::binread]
#[br(little)]
#[derive(Debug, Clone)]
pub struct Mesh7 {
	pub revision: Revision7,
	#[br(parse_with = binrw::helpers::until_eof)]
	pub chunks: Vec<Chunk6>,
}
impl Chunks6 for Mesh7 {
	fn chunks(&self) -> &[Chunk6] {
		&self.chunks
	}
}

#[cfg(feature = "mesh-v7-draco")]
pub use draco::DracoError;

#[cfg(feature = "mesh-v7-draco")]
mod draco {
	use core::error::Error;

	use crate::mesh::{CoreMesh6, CoreMeshDraco7, Face2, Vertex2, VertexId2};
	use draco_core::{DecoderBuffer, GeometryAttributeType, MeshDecoder, PointAttribute};

	#[derive(Debug)]
	pub enum DracoError {
		Decode(draco_core::DracoError),
		MissingPositions,
	}
	impl std::fmt::Display for DracoError {
		fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			match self {
				DracoError::Decode(e) => write!(f, "Draco decode failed: {e}"),
				DracoError::MissingPositions => write!(f, "Draco mesh has no position attribute"),
			}
		}
	}
	impl Error for DracoError {
		fn source(&self) -> Option<&(dyn Error + 'static)> {
			match self {
				DracoError::Decode(e) => Some(e),
				DracoError::MissingPositions => None,
			}
		}
	}

	impl CoreMeshDraco7 {
		/// Decompress the Draco bitstream into the same vertex and face
		/// layout used by uncompressed meshes.
		///
		/// Roblox stores normals and tangents as generic attributes, so
		/// attributes are matched by type and component count:
		/// position, normal (generic f32 x3), uv, tangent (generic u8 x4), colour.
		pub fn decode(&self) -> Result<CoreMesh6, DracoError> {
			let mut buffer = DecoderBuffer::new(&self.bitstream);
			let mut mesh = draco_core::Mesh::new();
			MeshDecoder::new()
				.decode(&mut buffer, &mut mesh)
				.map_err(DracoError::Decode)?;

			let n = mesh.num_points();
			let attributes: Vec<&PointAttribute> = (0..mesh.num_attributes())
				.map(|i| mesh.attribute(i))
				.collect();
			let find = |ty: GeometryAttributeType, components: Option<u8>| {
				attributes.iter().copied().find(|a| {
					a.attribute_type() == ty && components.is_none_or(|c| a.num_components() == c)
				})
			};
			let read = |a: Option<&PointAttribute>, components: usize| {
				a.map(|a| a.read_f32s(n, components))
			};

			let positions = read(find(GeometryAttributeType::Position, None), 3)
				.ok_or(DracoError::MissingPositions)?;
			let normals = read(
				find(GeometryAttributeType::Normal, None)
					.or_else(|| find(GeometryAttributeType::Generic, Some(3))),
				3,
			);
			let uvs = read(find(GeometryAttributeType::TexCoord, None), 2);
			let tangents = read(find(GeometryAttributeType::Generic, Some(4)), 4);
			let colors = read(find(GeometryAttributeType::Color, None), 4);

			let vec3 = |v: &Option<Vec<f32>>, i: usize| match v {
				Some(v) => [v[i * 3], v[i * 3 + 1], v[i * 3 + 2]],
				None => [0.0; 3],
			};
			// u8 components were widened to f32 by read_f32s; narrow them back.
			let bytes4 = |v: &Option<Vec<f32>>, i: usize, default: [u8; 4]| match v {
				Some(v) => core::array::from_fn(|k| v[i * 4 + k] as u8),
				None => default,
			};

			let vertices = (0..n)
				.map(|i| Vertex2 {
					pos: [positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]],
					norm: vec3(&normals, i),
					tex: match &uvs {
						Some(v) => [v[i * 2], v[i * 2 + 1]],
						None => [0.0; 2],
					},
					// tangent bytes are kept exactly as stored, matching Vertex2
					tangent: bytes4(&tangents, i, [0; 4]).map(|b| b as i8),
					color: bytes4(&colors, i, [255; 4]),
				})
				.collect();
			let faces = mesh
				.faces()
				.iter()
				.map(|f| Face2(f.map(|p| VertexId2(p.0))))
				.collect();

			Ok(CoreMesh6 { vertices, faces })
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;

		#[test]
		fn a_decode_error_keeps_the_draco_error_as_its_source() {
			let error = DracoError::Decode(draco_core::DracoError::general("bad bitstream"));
			let source = error.source().expect("a source");
			assert_eq!(source.to_string(), "General error: bad bitstream");
		}

		#[test]
		fn missing_positions_has_no_source() {
			assert!(DracoError::MissingPositions.source().is_none());
		}
	}
}
