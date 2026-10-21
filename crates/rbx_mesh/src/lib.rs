#[cfg(feature = "mesh")]
pub mod mesh;
#[cfg(feature = "mesh")]
pub use mesh::read_versioned as read_mesh_versioned;

// shared code between union formats
#[cfg(any(feature = "union-graphics", feature = "union-physics"))]
mod csgk;

#[cfg(feature = "union-graphics")]
pub mod union_graphics;
#[cfg(feature = "union-graphics")]
pub use union_graphics::read_versioned as read_union_graphics_versioned;

#[cfg(feature = "union-physics")]
pub mod union_physics;
#[cfg(feature = "union-physics")]
pub use union_physics::read_versioned as read_union_physics_versioned;

// with no format enabled there is nothing to test, and the helpers would be unused
#[cfg(all(
	test,
	any(
		feature = "mesh",
		feature = "union-graphics",
		feature = "union-physics"
	)
))]
mod test;

// test readme (its v7 arm decodes Draco)
#[cfg_attr(
	all(
		feature = "mesh-v1",
		feature = "union-graphics",
		feature = "union-physics",
		any(feature = "csgphs-v8-ruzstd", feature = "csgphs-v8-zstd"),
		feature = "mesh-v7-draco",
	),
	cfg(doctest),
	doc = include_str!("../README.md")
)]
#[cfg_attr(
	not(all(
		feature = "mesh-v1",
		feature = "union-graphics",
		feature = "union-physics",
		any(feature = "csgphs-v8-ruzstd", feature = "csgphs-v8-zstd"),
		feature = "mesh-v7-draco",
	)),
	expect(unused)
)]
struct ReadmeDoctests;
