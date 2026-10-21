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

/// What a decal draw is, as `decal.x` in mesh.wgsl, which compares it with
/// the half-way points (0 is a draw with no decal).
#[derive(Clone, Copy)]
#[repr(u8)]
enum DecalMode {
	Decal = 1,
	Texture = 2,
	Variant = 3,
	Clothing = 4,
}

impl DecalDraw {
	/// The draw's mode for the shader (`decal.x` in mesh.wgsl): clothing
	/// first, then a MaterialVariant, then a tiled Texture, else a Decal.
	pub(crate) fn shader_mode(&self) -> f32 {
		let mode = if self.clothing.is_some() {
			DecalMode::Clothing
		} else if self.variant {
			DecalMode::Variant
		} else if self.tiles.is_some() {
			DecalMode::Texture
		} else {
			DecalMode::Decal
		};
		mode as u8 as f32
	}
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

/// The roughness of a surface without a roughness map, in the viewport
/// (`params.z` in mesh.wgsl) and in glTF exports. An `f64` so the glTF
/// writes exactly 0.6; the shader takes it as the nearest `f32`.
pub const DEFAULT_ROUGHNESS: f64 = 0.6;

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
///
/// The values are the shader's: [`AlphaMode::shader_value`] is `tint.w` in
/// mesh.wgsl, which picks the mode by comparing it with the half-way
/// points (`mode < 0.5` is Overlay, and so on up to Blend), and
/// `GpuRenderer::callback` picks the pass the same way. Changing a value
/// here means changing those comparisons too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AlphaMode {
	/// The map is laid over the part's colour by its alpha.
	Overlay = 0,
	/// Alpha below one half cuts the surface away; the rest is opaque.
	Transparency = 1,
	/// Alpha says where the tint applies; the part's colour is not used.
	TintMask = 2,
	/// Alpha is ignored.
	Opaque = 3,
	/// The map's alpha makes the surface see-through. Not a Roblox mode:
	/// offered for meshes shown on their own (hair cards, leaves).
	Blend = 4,
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

	/// The mode as the shader takes it (`tint.w` in mesh.wgsl).
	pub(crate) fn shader_value(self) -> f32 {
		self as u8 as f32
	}
}

/// Roblox's default part colour ("Medium stone grey"), shown where a
/// colour map is transparent.
pub const PART_COLOUR: Color32 = Color32::from_rgb(163, 162, 165);

#[cfg(test)]
mod tests {
	use super::*;

	/// mesh.wgsl and the GPU pass choice compare these numbers.
	#[test]
	fn alpha_modes_keep_their_shader_values() {
		let modes = [
			AlphaMode::Overlay,
			AlphaMode::Transparency,
			AlphaMode::TintMask,
			AlphaMode::Opaque,
			AlphaMode::Blend,
		];
		for (value, mode) in modes.into_iter().enumerate() {
			assert_eq!(mode.shader_value(), value as f32, "{mode:?}");
		}
	}

	#[test]
	fn decal_modes_put_clothing_first_then_variants_then_textures() {
		let decal = DecalDraw {
			part_from_mesh: crate::render::maths::IDENTITY,
			size: [1.0; 3],
			face: 0,
			tiles: None,
			colour: [1.0; 3],
			transparency: 0.0,
			variant: false,
			clothing: None,
		};
		let texture = DecalDraw {
			tiles: Some([1.0, 1.0, 0.0, 0.0]),
			..decal
		};
		let variant = DecalDraw {
			variant: true,
			..texture
		};
		let clothing = DecalDraw {
			clothing: Some(ClothingDraw {
				regions: [[0.0; 4]; 6],
				centre: [0.0; 3],
				half: [1.0; 3],
			}),
			..variant
		};
		let modes = [&decal, &texture, &variant, &clothing].map(DecalDraw::shader_mode);
		assert_eq!(modes, [1.0, 2.0, 3.0, 4.0]);
	}
}
