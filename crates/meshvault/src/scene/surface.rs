//! How a Roblox part's surface looks beyond its maps: transparency,
//! material and alpha mode, and the decals, textures and clothing drawn
//! over it.

use eframe::egui::Color32;

type V3 = [f32; 3];

/// A Decal or Texture drawn on one face of its part: the part's geometry
/// again, with the image projected onto the face as Roblox does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalDraw {
	/// Row-major mesh-to-part transform: positions in the part's own studs.
	pub part_from_mesh: [[f32; 4]; 4],
	/// The part's size.
	pub size: V3,
	/// `Enum.NormalId` value: Right, Top, Back, Left, Bottom, Front.
	pub face: u8,
	/// A Texture: studs per tile (U, V) and offset in studs (U, V). A Decal
	/// stretches over the whole face.
	pub tiles: Option<[f32; 4]>,
	/// `Color3` (sRGB 0..1) and `Transparency`.
	pub colour: [f32; 3],
	pub transparency: f32,
	/// A MaterialVariant rather than a decal: its maps tile over every face
	/// (`tiles`), tinted by the part's colour.
	pub variant: bool,
	/// Clothing rather than a decal: `part_from_mesh` then places the mesh
	/// in its body section's box, whose faces take these template regions.
	pub clothing: Option<ClothingDraw>,
}

/// How a body section's box wears a clothing template.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClothingDraw {
	/// Each face's template region (x, y, width, height in 0..1), in
	/// `Enum.NormalId` order; a zero width leaves the face bare.
	pub regions: [[f32; 4]; 6],
	pub centre: V3,
	pub half: V3,
}

/// How a Roblox part's surface is drawn, beyond its colour and maps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
	/// 1 − `Transparency`.
	pub opacity: f32,
	/// Neon parts glow in their colour, unlit.
	pub neon: bool,
	/// `Reflectance`: how much of the sky the part mirrors.
	pub reflectance: f32,
	/// A metal material: reflections take the part's colour.
	pub metal: bool,
	/// `SurfaceAppearance.Color`, which tints the colour map (sRGB 0..1).
	pub tint: [f32; 3],
	pub alpha_mode: AlphaMode,
}

impl Surface {
	pub const OPAQUE: Surface = Surface {
		opacity: 1.0,
		neon: false,
		reflectance: 0.0,
		metal: false,
		tint: [1.0; 3],
		alpha_mode: AlphaMode::Overlay,
	};
}

/// What a colour map's alpha means (`SurfaceAppearance.AlphaMode`), as
/// measured in Studio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
	/// The map is laid over the part's colour by its alpha.
	Overlay,
	/// Alpha below one half cuts the surface away; the rest is opaque.
	Transparency,
	/// Alpha says where the tint applies; the part's colour is not used.
	TintMask,
	/// Alpha is ignored.
	Opaque,
	/// The map's alpha makes the surface see-through. Not a Roblox mode:
	/// offered for meshes shown on their own (hair cards, leaves).
	Blend,
}

impl AlphaMode {
	pub fn from_name(name: &str) -> AlphaMode {
		match name {
			"Transparency" => AlphaMode::Transparency,
			"TintMask" => AlphaMode::TintMask,
			"Opaque" => AlphaMode::Opaque,
			_ => AlphaMode::Overlay,
		}
	}
}

/// Roblox's default part colour ("Medium stone grey"), shown where a
/// colour map is transparent.
pub const PART_COLOUR: Color32 = Color32::from_rgb(163, 162, 165);
