## [Unreleased]

### Added

- Mesh v6.00 and v7.00 support (chunked format: COREMESH, LODS, SKINNING, FACS, HSRAVIS)
- Draco decoding of v7 COREMESH chunks via `CoreMeshDraco7::decode`, behind the default `mesh-v7-draco` feature

### Changed

- `Mesh` has new `V6` and `V7` variants (breaking for exhaustive matches)
- CSGPHS v8 edgebreaker errors are now `HullDecodeError` (wrapping `BitCounterError`)

### Fixed

- CSGPHS v8: zstd output is capped at `MAX_DECODED_LEN` (64 MiB) for both the `zstd` and `ruzstd` backends; larger streams are an error
- Counts no longer size allocations before the data is checked: mesh v1 (also `3 * face_count` overflow), CSGMDL5 face indices, CSGPHS v8 face/hull buffers (bounded by the symbol bits and 4,194,304 faces), mesh v5 FACS matrix `x * y` overflow, and Draco decoding (face, point and attribute-byte ceilings)
- Counted byte fields (bone names, FACS names, Draco bitstreams, HSRAVIS bits, unknown chunk data, CSGMDL5 index data, CSGPHS v8 symbols) check the bytes remaining before reserving, instead of reserving up to 4 GiB
- CSGPHS v8 edgebreaker decoding returns errors instead of panicking on inconsistent hull/face counts or bad edges, decodes Split branches without recursion, and caps boundary-walk steps
- CSGPHS v8 hull range tables are checked when read (matching lengths, starting at 0, non-decreasing, whole triangles, within the data), so `iter_hulls` cannot panic on a parsed mesh

## [0.8.2] - Oct 16th 2026

### Removed

- Regex dependency dropped (#22)

## [0.8.1] - Oct 8th 2026

### Added

- Add GeomType7::Type6

## [0.8.0] - May 25th 2026

### Added

- Add CSGPHS8 support

### Changed

- Mesh v1 `Mesh1.face_count` is automatically calculated and hidden from the public interface
- CSGMDL2 `Mesh2.face_count` is automatically calculated and hidden from the public interface
- CSGMDL5 `TwoPoseCorrective5` and `ThreePoseCorrective5` use an array of `ControlId5` instead of a tuple
- CSGPHS7 constant magic number `3` changed to `geom_type` field with 3 observed variants

## [0.7.0] - May 8th 2026

### Added

- Disabling mesh-v1 feature allows regex dependency to be dropped
- mesh, union-graphics, union-physics features to disable parts of the library you don't need

### Changed

- `Mesh1` is implemented via binrw::BinRead trait to provide a consistent interface
- `Mesh2` & `Mesh3` vertices are changed to use an enum `Vertices2`
- `Mesh2` through `Mesh5` faces `Face2` use an array of `VertexId2` instead of a tuple
- `read_versioned` is renamed to `read_mesh_versioned`
- `read_mesh_data_versioned` is renamed to `read_union_graphics_versioned`
- `read_physics_data_versioned` is renamed to `read_union_physics_versioned`
- `MeshData` is renamed to `UnionGraphics`
- `PhysicsData` is renamed to `UnionPhysics`
- `CSGPHS3` mesh field `Mesh.vertices` is renamed to `Mesh.positions` for consistency with other mesh definitions
- CSGPHS & CSGMDL variants are flattened into each union enum to reduce match depth
- Magic numbers are moved onto the individual mesh structs, meaning each one acts as its own encoder/decoder and can be used independently of the versioned mesh enum.  For example, if you know ahead of time that a mesh is using the version 2.00 format, you can write `let mesh: Mesh2 = data.read_le()?;`
- count & len fields have been removed from the public structs, they are calculated based on the contained data.  For example, vertex_count is just vertices.len(), so it is calculated from the vertices Vec upon serialization.

### Removed

- Mesh v2-v5 "fix tangents" procedure that replaces Vertex2.tangents default value `[-128, -128, -128, -128]` with `[0, 0, -128, 127]`.  If you depended on this behaviour, you will need to handle this yourself.  None of the ~10000 meshes in my dataset have default tangents. If you are using old meshes and need tangents, realistically you should be generating tangents via some algorithmic process rather than filling in default values.

## [0.6.0] - March 3rd 2026

### Changed

- Fix CSGMDL5 range markers

## [0.5.1] - Nov 9th 2025

### Changed

- Drop lazy_regex dependency

## [0.5.0] - July 23rd 2025

### Added

- Add CSGMDL5 support

### Changed

- Union mesh data NormalId2 now uses an inner type NormalId. This is so that NormalId5 can share the inner type. NormalId2 is serialized as u32, whereas NormalId5 is u8, so the wrapper types are needed. Why Roblox chose to count the NormalId from 1 instead of using their own Enum.NormalId values starting at 0 is beyond me.

## [0.4.0] - May 26th 2025

### Added

- Add CSGPHS7 support
- Add CSGPHS6 support

### Changed

- Rename a bunch of fields and struct names for clarity and conciseness

## [0.3.1] - Jan 31st 2025

### Changed

- Rename a bunch of fields and struct names for clarity and conciseness

## [0.3.0] - Jan 31st 2025

### Added

- Add union graphics support (CSGMDL)
  - CSGMDL2
  - CSGMDL4

## [0.2.0] - Jan 24th 2025

### Added

- Add union physics support (CSGPHS)
  - CSGPHS3
  - CSGPHS5

## [0.1.1] - March 29th 2024

### Changed

- Simplify mesh v1 regex

## [0.1.0] - March 12th 2024

### Added

- Add mesh support
  - version 1.00
  - version 1.01
  - version 2.00
  - version 3.00
  - version 3.01
  - version 4.00
  - version 4.01
  - version 5.00

## [Inital commit] - March 10th 2024
