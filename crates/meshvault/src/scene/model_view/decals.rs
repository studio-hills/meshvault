//! The layers drawn over a part: its Decals and Textures, a
//! MaterialVariant, and classic clothing laid out on its body section.

use crate::anim::math::CFrame;
use crate::render::gpu::GpuScene;
use crate::scene::surface::ClothingDraw;
use crate::scene::texture::Material;

/// A Decal or Texture of a part, with its image loaded.
pub(super) struct PartDecal {
	pub(super) face: u8,
	pub(super) tiles: Option<[f32; 4]>,
	pub(super) colour: [f32; 3],
	pub(super) transparency: f32,
	/// The image, as the colour map (a MaterialVariant's maps fill their slots).
	pub(super) material: Material,
	pub(super) gpu: Option<GpuScene>,
	/// A MaterialVariant over the whole part, drawn before its decals.
	pub(super) variant: bool,
	/// Clothing: how the part's body section wears the template, and where
	/// the part sits in that section.
	pub(super) clothing: Option<(ClothingDraw, CFrame)>,
}

/// The classic clothing layout: which body sections there are, the parts
/// of each (R15 pieces, or the R6 part), and whether the shirt or the
/// pants covers it.
pub(super) const SECTIONS: [(&[&str], Section); 5] = [
	(&["UpperTorso", "LowerTorso", "Torso"], Section::Torso),
	(
		&["RightUpperArm", "RightLowerArm", "RightHand", "Right Arm"],
		Section::RightArm,
	),
	(
		&["LeftUpperArm", "LeftLowerArm", "LeftHand", "Left Arm"],
		Section::LeftArm,
	),
	(
		&["RightUpperLeg", "RightLowerLeg", "RightFoot", "Right Leg"],
		Section::RightLeg,
	),
	(
		&["LeftUpperLeg", "LeftLowerLeg", "LeftFoot", "Left Leg"],
		Section::LeftLeg,
	),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
	Torso,
	RightArm,
	LeftArm,
	RightLeg,
	LeftLeg,
}

impl Section {
	/// Each box face's region of the 585 x 559 template (pixels), in
	/// `Enum.NormalId` order: Right (+X), Top, Back, Left (-X), Bottom,
	/// Front. The template unfolds each section's box around its front.
	pub(super) fn regions(self) -> [[f32; 4]; 6] {
		let torso = [
			[165.0, 74.0, 64.0, 128.0],
			[231.0, 8.0, 128.0, 64.0],
			[427.0, 74.0, 128.0, 128.0],
			[361.0, 74.0, 64.0, 128.0],
			[231.0, 204.0, 128.0, 64.0],
			[231.0, 74.0, 128.0, 128.0],
		];
		let right = [
			[151.0, 355.0, 64.0, 128.0],
			[217.0, 289.0, 64.0, 64.0],
			[85.0, 355.0, 64.0, 128.0],
			[19.0, 355.0, 64.0, 128.0],
			[217.0, 485.0, 64.0, 64.0],
			[217.0, 355.0, 64.0, 128.0],
		];
		let left = [
			[506.0, 355.0, 64.0, 128.0],
			[308.0, 289.0, 64.0, 64.0],
			[440.0, 355.0, 64.0, 128.0],
			[374.0, 355.0, 64.0, 128.0],
			[308.0, 485.0, 64.0, 64.0],
			[308.0, 355.0, 64.0, 128.0],
		];
		let pixels = match self {
			Section::Torso => torso,
			Section::RightArm | Section::RightLeg => right,
			Section::LeftArm | Section::LeftLeg => left,
		};
		pixels.map(|[x, y, w, h]| [x / 585.0, y / 559.0, w / 585.0, h / 559.0])
	}
}

/// `Enum.NormalId` value of a face name.
pub(super) fn face_index(name: &str) -> u8 {
	match name {
		"Right" => 0,
		"Top" => 1,
		"Back" => 2,
		"Left" => 3,
		"Bottom" => 4,
		_ => 5,
	}
}
