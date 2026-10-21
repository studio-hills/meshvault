//! How the scene is lit: the lighting presets, and the model's own
//! lights packed for the shader.

use crate::render::gpu::Uniforms;
use crate::render::maths::{V3, dot, normalize, sub};
use crate::scene::colour::srgb_to_linear;

/// How the scene is lit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Lighting {
	/// A key light above and left of the camera, a fill, and a soft sky.
	#[default]
	Studio,
	/// A warm sun from the model's front right, under a blue sky.
	Daylight,
	/// Soft, even light from a bright cloudy sky.
	Overcast,
	/// A low orange sun from behind, rimming the model.
	Sunset,
	/// Dim moonlight, so the model's own lights stand out.
	Night,
	/// No shading: every surface in its own colour.
	Flat,
}

impl Lighting {
	pub const ALL: [Lighting; 6] = [
		Lighting::Studio,
		Lighting::Daylight,
		Lighting::Overcast,
		Lighting::Sunset,
		Lighting::Night,
		Lighting::Flat,
	];

	pub fn label(self) -> &'static str {
		match self {
			Lighting::Studio => "Studio",
			Lighting::Daylight => "Daylight",
			Lighting::Overcast => "Overcast",
			Lighting::Sunset => "Sunset",
			Lighting::Night => "Night",
			Lighting::Flat => "Flat (unlit)",
		}
	}

	/// The direction towards the key light, and the shader's lighting rows
	/// (see `Uniforms::env`) without a highlight. Studio and Overcast light
	/// from the camera; the others from fixed places in the world.
	pub(super) fn rig(self, right: V3, up: V3, forward: V3) -> (V3, [[f32; 4]; 4]) {
		let from_camera =
			|r: f32, u: f32| normalize([0, 1, 2].map(|k| -forward[k] + u * up[k] + r * right[k]));
		let rows = |key: V3, fill: f32, sky: V3, exposure: f32, ground: V3| {
			[
				[key[0], key[1], key[2], fill],
				[sky[0], sky[1], sky[2], exposure],
				[ground[0], ground[1], ground[2], 0.0],
				[0.0; 4],
			]
		};
		match self {
			Lighting::Studio => (
				from_camera(-0.4, 0.6),
				rows([2.8; 3], 0.7, [0.45, 0.48, 0.55], 1.0, [0.2, 0.18, 0.16]),
			),
			// Roblox models face -Z: the sun is in front, to the right, high
			Lighting::Daylight => (
				normalize([0.45, 0.8, -0.4]),
				rows(
					[3.8, 3.3, 2.6],
					0.3,
					[0.3, 0.45, 0.85],
					1.0,
					[0.3, 0.26, 0.2],
				),
			),
			Lighting::Overcast => (
				from_camera(0.0, 1.4),
				rows(
					[1.1, 1.1, 1.15],
					0.6,
					[0.85, 0.87, 0.9],
					1.0,
					[0.4, 0.38, 0.36],
				),
			),
			Lighting::Sunset => (
				normalize([-0.7, 0.18, 0.5]),
				rows(
					[3.6, 1.9, 1.0],
					0.25,
					[0.32, 0.3, 0.48],
					1.1,
					[0.16, 0.1, 0.1],
				),
			),
			Lighting::Night => (
				normalize([0.3, 0.7, -0.5]),
				rows(
					[0.35, 0.45, 0.75],
					0.08,
					[0.06, 0.08, 0.14],
					1.3,
					[0.02, 0.02, 0.03],
				),
			),
			Lighting::Flat => (
				from_camera(0.0, 0.0),
				rows([0.0; 3], 0.0, [1.0; 3], -1.0, [1.0; 3]),
			),
		}
	}
}

/// A Roblox light placed in the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneLight {
	pub position: V3,
	/// Where a spot or surface light shines; unused for point lights.
	pub direction: V3,
	/// Colour (sRGB 0..1) and Brightness.
	pub colour: [f32; 3],
	pub brightness: f32,
	pub range: f32,
	/// Half the cone's angle in degrees, or `None` for a point light.
	pub half_angle: Option<f32>,
}

/// The most lights a draw takes; the shader's array holds this many.
pub const MAX_LIGHTS: usize = 8;

/// The lights nearest the model, packed for the shader: per light its
/// position and range, linear colour times brightness and whether it is a
/// cone, and its direction and the cosine of the cone's half angle.
pub(super) fn light_uniforms(lights: &[SceneLight], near: V3) -> Uniforms {
	let mut sorted: Vec<&SceneLight> = lights
		.iter()
		.filter(|l| l.brightness > 0.0 && l.range > 0.0)
		.collect();
	let distance = |l: &SceneLight| dot(sub(l.position, near), sub(l.position, near));
	sorted.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
	let mut packed = [[0.0f32; 4]; MAX_LIGHTS * 3];
	for (i, l) in sorted.iter().take(MAX_LIGHTS).enumerate() {
		let c = l
			.colour
			.map(|c| srgb_to_linear(c.clamp(0.0, 1.0)) * l.brightness);
		packed[i * 3] = [l.position[0], l.position[1], l.position[2], l.range];
		packed[i * 3 + 1] = [c[0], c[1], c[2], l.half_angle.is_some() as u8 as f32];
		let cos = l
			.half_angle
			.map_or(-1.0, |a| a.clamp(0.0, 90.0).to_radians().cos());
		packed[i * 3 + 2] = [l.direction[0], l.direction[1], l.direction[2], cos];
	}
	Uniforms {
		lights: packed,
		light_count: [sorted.len().min(MAX_LIGHTS) as f32, 0.0, 0.0, 0.0],
		..bytemuck::Zeroable::zeroed()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::render::maths::close;

	fn light(x: f32) -> SceneLight {
		SceneLight {
			position: [x, 0.0, 0.0],
			direction: [0.0, -1.0, 0.0],
			colour: [1.0, 1.0, 1.0],
			brightness: 1.0,
			range: 10.0,
			half_angle: None,
		}
	}

	#[test]
	fn light_uniforms_keeps_the_nearest_lit_lights() {
		// out of order, with a dark light and a zero-range light nearest
		let mut lights: Vec<SceneLight> = [7.0, 3.0, 10.0, 1.0, 9.0, 5.0, 2.0, 8.0, 4.0, 6.0]
			.into_iter()
			.map(light)
			.collect();
		lights.push(SceneLight {
			brightness: 0.0,
			..light(0.25)
		});
		lights.push(SceneLight {
			range: 0.0,
			..light(0.5)
		});
		let u = light_uniforms(&lights, [0.0; 3]);
		assert_eq!(u.light_count, [MAX_LIGHTS as f32, 0.0, 0.0, 0.0]);
		for i in 0..MAX_LIGHTS {
			assert_eq!(u.lights[i * 3], [i as f32 + 1.0, 0.0, 0.0, 10.0]);
		}
	}

	#[test]
	fn light_uniforms_packs_colour_and_cone() {
		let spot = SceneLight {
			colour: [1.0, 0.0, 0.5],
			brightness: 2.0,
			half_angle: Some(60.0),
			..light(1.0)
		};
		let u = light_uniforms(&[spot, light(2.0)], [0.0; 3]);
		assert_eq!(u.light_count[0], 2.0);
		// sRGB colour to linear, times brightness; 1 marks a cone
		let [r, g, b, cone] = u.lights[1];
		assert!(close(r, 2.0) && close(g, 0.0) && cone == 1.0);
		assert!(close(b, 2.0 * srgb_to_linear(0.5)));
		assert_eq!(&u.lights[2][..3], &[0.0, -1.0, 0.0]);
		assert!(close(u.lights[2][3], 0.5));
		// a point light has no cone, and a cosine of -1
		assert_eq!(u.lights[4][3], 0.0);
		assert_eq!(u.lights[5][3], -1.0);
		assert_eq!(u.lights[6], [0.0; 4]);
	}

	#[test]
	fn light_uniforms_without_lit_lights_is_empty() {
		let dark = SceneLight {
			brightness: -1.0,
			..light(1.0)
		};
		let u = light_uniforms(&[dark], [0.0; 3]);
		assert_eq!(u.light_count, [0.0; 4]);
		assert!(u.lights.iter().all(|l| *l == [0.0; 4]));
	}
}
