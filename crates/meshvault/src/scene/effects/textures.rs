//! Stand-ins for the textures built into Roblox Studio (`rbxasset://`),
//! which are not assets anyone can download and are not Meshvault's to
//! ship: drawn here, white with the shape in alpha, so an effect's colour
//! tints them as it tints Studio's.

use std::sync::{Arc, OnceLock};

use image::{Rgba, RgbaImage};

/// The size each stand-in is drawn at.
const SIZE: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
	/// A soft round puff: smoke, and built-in textures Meshvault has no
	/// closer stand-in for.
	Soft,
	/// A bright core with four spikes: a ParticleEmitter's default
	/// texture, and Sparkles.
	Sparkle,
	/// A soft tongue of flame, narrowing upwards: Fire.
	Flame,
}

impl Builtin {
	/// The stand-in for the built-in texture at `path`
	/// (`rbxasset://textures/particles/sparkles_main.dds`...).
	pub fn for_path(path: &str) -> Builtin {
		let path = path.to_ascii_lowercase();
		if path.contains("sparkle") || path.contains("star") {
			Builtin::Sparkle
		} else if path.contains("fire") || path.contains("flame") {
			Builtin::Flame
		} else {
			Builtin::Soft
		}
	}

	/// The image, drawn once.
	pub fn image(self) -> Arc<RgbaImage> {
		static IMAGES: [OnceLock<Arc<RgbaImage>>; 3] =
			[OnceLock::new(), OnceLock::new(), OnceLock::new()];
		IMAGES[self as usize]
			.get_or_init(|| Arc::new(draw(self)))
			.clone()
	}
}

/// White, with `alpha(x, y)` (x and y from -1 to 1, y up) as alpha.
fn draw(kind: Builtin) -> RgbaImage {
	let alpha = |x: f32, y: f32| -> f32 {
		let r = (x * x + y * y).sqrt();
		match kind {
			Builtin::Soft => (1.0 - r).max(0.0).powf(1.6),
			Builtin::Sparkle => {
				let core = (1.0 - r * 2.2).max(0.0);
				// thin spikes along both axes, fading out to the edge
				let spike = |along: f32, across: f32| {
					(1.0 - along.abs()).max(0.0) * (-(across * 18.0).powi(2)).exp()
				};
				let glow = (1.0 - r).max(0.0).powi(3) * 0.35;
				(core + spike(x, y) + spike(y, x) + glow).min(1.0)
			}
			Builtin::Flame => {
				// wider at the bottom, coming to a point at the top
				let width = 0.75 * (1.0 - (y + 1.0) / 2.0).max(0.0).sqrt() + 0.05;
				let across = (x / width).abs();
				let up = (y + 0.6).max(0.0) / 1.6;
				((1.0 - across).max(0.0) * (1.0 - up).max(0.0) * 1.4).min(1.0)
					* (1.0 - r * 0.5).max(0.0)
			}
		}
	};
	RgbaImage::from_fn(SIZE, SIZE, |px, py| {
		let at = |p: u32| (p as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
		let a = alpha(at(px), -at(py)).clamp(0.0, 1.0);
		Rgba([255, 255, 255, (a * 255.0).round() as u8])
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn built_in_paths_have_stand_ins() {
		assert_eq!(
			Builtin::for_path("rbxasset://textures/particles/sparkles_main.dds"),
			Builtin::Sparkle
		);
		assert_eq!(
			Builtin::for_path("rbxasset://textures/particles/fire_main.dds"),
			Builtin::Flame
		);
		assert_eq!(
			Builtin::for_path("rbxasset://textures/particles/smoke_main.dds"),
			Builtin::Soft
		);
	}

	/// Each is solid in the middle and clear at the corners, and drawn once.
	#[test]
	fn stand_ins_fade_out_to_their_edges() {
		for kind in [Builtin::Soft, Builtin::Sparkle, Builtin::Flame] {
			let image = kind.image();
			let middle = image.get_pixel(SIZE / 2, SIZE / 2 + 4)[3];
			let corner = image.get_pixel(0, 0)[3];
			assert!(middle > 100 && corner == 0, "{kind:?}: {middle} {corner}");
			assert!(Arc::ptr_eq(&image, &kind.image()));
		}
	}
}
