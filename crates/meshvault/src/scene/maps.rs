//! The image maps a model can reference.
//!
//! `Texture` is `MeshPart.TextureID`. The others come from a
//! `SurfaceAppearance`, which Roblox draws instead of `TextureID` when present.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapKind {
	Texture,
	Color,
	Normal,
	Roughness,
	Metalness,
}

impl MapKind {
	pub const ALL: [MapKind; 5] = [
		MapKind::Texture,
		MapKind::Color,
		MapKind::Normal,
		MapKind::Roughness,
		MapKind::Metalness,
	];

	/// Name of the field in the export JSON.
	pub fn export_field(self) -> &'static str {
		match self {
			MapKind::Texture => "TextureId",
			MapKind::Color => "ColorMap",
			MapKind::Normal => "NormalMap",
			MapKind::Roughness => "RoughnessMap",
			MapKind::Metalness => "MetalnessMap",
		}
	}

	pub fn label(self) -> &'static str {
		match self {
			MapKind::Texture => "Texture",
			MapKind::Color => "Color",
			MapKind::Normal => "Normal",
			MapKind::Roughness => "Roughness",
			MapKind::Metalness => "Metalness",
		}
	}

	/// Inserted before the extension of the downloaded file, so a model's
	/// files sit together: `Name.mesh`, `Name.png`, `Name.normal.png`, ...
	/// The texture keeps the bare name so the viewer finds it next to the mesh.
	pub fn file_suffix(self) -> &'static str {
		match self {
			MapKind::Texture => "",
			MapKind::Color => ".color",
			MapKind::Normal => ".normal",
			MapKind::Roughness => ".roughness",
			MapKind::Metalness => ".metalness",
		}
	}
}

/// The material slots the renderer uses. Base colour comes from the
/// `Color` map, or the `Texture` when there is no `SurfaceAppearance`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
	Base,
	Normal,
	Roughness,
	Metalness,
}

impl Slot {
	pub const ALL: [Slot; 4] = [Slot::Base, Slot::Normal, Slot::Roughness, Slot::Metalness];

	pub fn label(self) -> &'static str {
		match self {
			Slot::Base => "Base color",
			Slot::Normal => "Normal",
			Slot::Roughness => "Roughness",
			Slot::Metalness => "Metalness",
		}
	}

	/// Colour maps are stored in sRGB; data maps are linear.
	pub fn is_srgb(self) -> bool {
		self == Slot::Base
	}

	/// Which downloaded maps feed this slot, in priority order.
	pub fn sources(self) -> &'static [MapKind] {
		match self {
			Slot::Base => &[MapKind::Color, MapKind::Texture],
			Slot::Normal => &[MapKind::Normal],
			Slot::Roughness => &[MapKind::Roughness],
			Slot::Metalness => &[MapKind::Metalness],
		}
	}
}
