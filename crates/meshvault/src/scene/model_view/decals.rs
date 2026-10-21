//! The layers drawn over a part: its Decals and Textures, a
//! MaterialVariant, and classic clothing laid out on its body section.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;

use crate::anim::math::{CFrame, V3};
use crate::anim::rig::{Posed, Rig};
use crate::anim::schema::{ClothingInfo, MaterialVariantInfo, ModelPart};
use crate::content_id::parse_asset_id;
use crate::render::gpu::GpuScene;
use crate::scene::maps::Slot;
use crate::scene::surface::ClothingDraw;
use crate::scene::texture::{self, Material, Texture};

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

/// Images by asset id, shared by a model's clothing, decals and
/// MaterialVariants: each is read and decoded once, and one that does not
/// load is not tried again.
pub(super) struct ImageCache<'a> {
	ctx: &'a egui::Context,
	paths: &'a HashMap<u64, PathBuf>,
	loaded: HashMap<u64, Option<Texture>>,
}

impl<'a> ImageCache<'a> {
	pub(super) fn new(ctx: &'a egui::Context, paths: &'a HashMap<u64, PathBuf>) -> Self {
		Self {
			ctx,
			paths,
			loaded: HashMap::new(),
		}
	}

	/// The image, or `None` when it is not in the workspace or does not load.
	pub(super) fn get(&mut self, id: u64) -> Option<Texture> {
		let (ctx, paths) = (self.ctx, self.paths);
		self.loaded
			.entry(id)
			.or_insert_with(|| texture::load_file(ctx, paths.get(&id)?).ok())
			.clone()
	}
}

/// A material with one image as its colour map.
fn base_material(image: Texture) -> Material {
	let mut material = Material::default();
	material.set(Slot::Base, image);
	material
}

/// One clothing image over a body section.
pub(super) struct ClothingLayer {
	section: Section,
	material: Material,
	colour: [f32; 3],
	/// A T-shirt, which covers only the front of the torso.
	graphic: bool,
}

/// The model's clothing over each body section, in the order it is drawn:
/// pants, then the shirt over them, then a T-shirt. Warns when some of its
/// images did not load.
pub(super) fn clothing_layers(
	c: &ClothingInfo,
	images: &mut ImageCache,
	warnings: &mut Vec<String>,
) -> Vec<ClothingLayer> {
	let mut load = |content: &Option<String>| images.get(parse_asset_id(content.as_deref()?)?);
	let pants = load(&c.pants_template);
	let shirt = load(&c.shirt_template);
	let graphic = load(&c.graphic);
	let mut layers = Vec::new();
	for (_, section) in SECTIONS {
		let legs = matches!(section, Section::RightLeg | Section::LeftLeg);
		let arms = matches!(section, Section::RightArm | Section::LeftArm);
		if let Some(t) = &pants
			&& !arms
		{
			layers.push(ClothingLayer {
				section,
				material: base_material(t.clone()),
				colour: c.pants_color.unwrap_or([1.0; 3]),
				graphic: false,
			});
		}
		if let Some(t) = &shirt
			&& !legs
		{
			layers.push(ClothingLayer {
				section,
				material: base_material(t.clone()),
				colour: c.shirt_color.unwrap_or([1.0; 3]),
				graphic: false,
			});
		}
	}
	if let Some(t) = graphic {
		layers.push(ClothingLayer {
			section: Section::Torso,
			material: base_material(t),
			colour: c.graphic_color.unwrap_or([1.0; 3]),
			graphic: true,
		});
	}
	let wanted = [&c.pants_template, &c.shirt_template, &c.graphic]
		.iter()
		.filter(|t| t.is_some())
		.count();
	let got = [
		pants.is_some(),
		shirt.is_some(),
		layers.iter().any(|l| l.graphic),
	]
	.iter()
	.filter(|b| **b)
	.count();
	if got < wanted {
		warnings.push(
			"Some of the model's clothing is not drawn: its images are not downloaded yet.".into(),
		);
	}
	layers
}

/// A body section's box in the joints' rest pose, as one of its parts
/// sees it.
pub(super) struct SectionBox {
	section: Section,
	/// Where the part sits in the section's frame (its first part's).
	in_section: CFrame,
	centre: V3,
	half: V3,
}

/// The body section of each part that is in one, by part index.
pub(super) fn body_sections(rig: &Rig, joint_rest: &Posed) -> HashMap<usize, SectionBox> {
	let mut sections = HashMap::new();
	for (names, section) in SECTIONS {
		let members: Vec<usize> = names
			.iter()
			.filter_map(|n| rig.parts.iter().position(|p| p.name == *n))
			.collect();
		let Some(&first) = members.first() else {
			continue;
		};
		let frame = joint_rest.parts[first];
		let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
		for &m in &members {
			let in_frame = frame.inverse().mul(&joint_rest.parts[m]);
			let h = rig.parts[m].size.map(|s| s * 0.5);
			for corner in 0..8 {
				let p = in_frame.point([
					if corner & 1 == 0 { -h[0] } else { h[0] },
					if corner & 2 == 0 { -h[1] } else { h[1] },
					if corner & 4 == 0 { -h[2] } else { h[2] },
				]);
				for k in 0..3 {
					lo[k] = lo[k].min(p[k]);
					hi[k] = hi[k].max(p[k]);
				}
			}
		}
		let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5);
		let half = [0, 1, 2].map(|k| (hi[k] - lo[k]) * 0.5);
		for &m in &members {
			let in_frame = frame.inverse().mul(&joint_rest.parts[m]);
			sections.insert(
				m,
				SectionBox {
					section,
					in_section: in_frame,
					centre,
					half,
				},
			);
		}
	}
	sections
}

/// A part's Decals and Textures whose images loaded, with their ZIndex,
/// lowest first; ties keep the order they were exported in. Counts the
/// ones not drawn in `missing`.
pub(super) fn part_decals(
	record: &ModelPart,
	images: &mut ImageCache,
	missing: &mut usize,
) -> Vec<(i32, PartDecal)> {
	let mut decals = Vec::new();
	for d in &record.decals {
		let image = parse_asset_id(&d.texture).and_then(|id| images.get(id));
		let Some(image) = image else {
			*missing += 1;
			continue;
		};
		let tiles = d.studs_per_tile_u.map(|u| {
			[
				u,
				d.studs_per_tile_v.unwrap_or(u),
				d.offset_studs_u.unwrap_or(0.0),
				d.offset_studs_v.unwrap_or(0.0),
			]
		});
		decals.push((
			d.z_index,
			PartDecal {
				face: face_index(&d.face),
				tiles: if d.class_name == "Texture" {
					tiles
				} else {
					None
				},
				colour: d.color,
				transparency: d.transparency,
				material: base_material(image),
				gpu: None,
				variant: false,
				clothing: None,
			},
		));
	}
	// a stable sort: equal ZIndexes keep their order
	decals.sort_by_key(|(z, _)| *z);
	decals
}

/// A MaterialVariant over the whole part with the maps that loaded, or
/// `None` when none did. Counts it in `missing` when some did not; a map
/// that is not an asset id is not counted.
pub(super) fn variant_layer(
	v: &MaterialVariantInfo,
	images: &mut ImageCache,
	missing: &mut usize,
) -> Option<PartDecal> {
	let mut material = Material::default();
	let mut wanted = 0;
	for (slot, content) in v.maps() {
		let Some(id) = parse_asset_id(content) else {
			continue;
		};
		wanted += 1;
		if let Some(image) = images.get(id) {
			material.set(slot, image);
		}
	}
	if material.maps.len() < wanted {
		*missing += 1;
	}
	if material.maps.is_empty() {
		return None;
	}
	let tile = v.studs_per_tile.max(0.01);
	Some(PartDecal {
		face: 0,
		tiles: Some([tile, tile, 0.0, 0.0]),
		colour: [1.0; 3],
		transparency: 0.0,
		material,
		gpu: None,
		variant: true,
		clothing: None,
	})
}

/// Puts the clothing of a part's body section over the part, pants first:
/// above every layer at the lowest ZIndex (a MaterialVariant, and any decal
/// there), under the other decals.
pub(super) fn insert_clothing(
	decals: &mut Vec<(i32, PartDecal)>,
	section: &SectionBox,
	layers: &[ClothingLayer],
) {
	let at = decals.iter().take_while(|(z, _)| *z == i32::MIN).count();
	// each insert at `at` goes under the ones before it: top layer first
	let layers = layers.iter().filter(|l| l.section == section.section).rev();
	for layer in layers {
		let mut regions = section.section.regions();
		if layer.graphic {
			// a T-shirt covers the front of the torso
			regions = [[0.0; 4]; 6];
			regions[5] = [0.0, 0.0, 1.0, 1.0];
		}
		decals.insert(
			at,
			(
				i32::MIN,
				PartDecal {
					face: 0,
					tiles: None,
					colour: layer.colour,
					transparency: 0.0,
					material: layer.material.clone(),
					gpu: None,
					variant: false,
					clothing: Some((
						ClothingDraw {
							regions,
							centre: section.centre,
							half: section.half,
						},
						section.in_section,
					)),
				},
			),
		);
	}
}
