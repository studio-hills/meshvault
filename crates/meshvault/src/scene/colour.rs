//! The sRGB transfer function, shared by the renderer and the exporters so
//! both turn colours into linear light the same way. (The shader has its
//! own cheaper approximation; this is the exact piecewise curve.)

use eframe::egui::Color32;

/// One sRGB-encoded channel in 0..=1 to linear light.
pub fn srgb_to_linear(c: f32) -> f32 {
	if c <= 0.04045 {
		c / 12.92
	} else {
		((c + 0.055) / 1.055).powf(2.4)
	}
}

/// One linear channel in 0..=1 to sRGB encoding.
pub fn linear_to_srgb(c: f32) -> f32 {
	if c <= 0.003_130_8 {
		c * 12.92
	} else {
		1.055 * c.powf(1.0 / 2.4) - 0.055
	}
}

/// One sRGB byte to linear light.
pub fn srgb8_to_linear(c: u8) -> f32 {
	srgb_to_linear(c as f32 / 255.0)
}

/// A colour's red, green and blue in linear light (alpha is dropped).
pub fn linear_rgb(c: Color32) -> [f32; 3] {
	[
		srgb8_to_linear(c.r()),
		srgb8_to_linear(c.g()),
		srgb8_to_linear(c.b()),
	]
}

#[cfg(test)]
mod tests {
	use super::*;

	fn close(a: f32, b: f32) -> bool {
		(a - b).abs() < 1e-6
	}

	#[test]
	fn the_ends_map_to_themselves() {
		assert!(close(srgb_to_linear(0.0), 0.0));
		assert!(close(srgb_to_linear(1.0), 1.0));
		assert!(close(linear_to_srgb(0.0), 0.0));
		assert!(close(linear_to_srgb(1.0), 1.0));
		assert!(close(srgb8_to_linear(255), 1.0));
	}

	/// The straight piece and the curve join up at each threshold (to the
	/// rounding of the published constants).
	#[test]
	fn the_two_pieces_meet() {
		let below = 0.04045_f32 / 12.92;
		let above = ((0.04045_f32 + 0.055) / 1.055).powf(2.4);
		assert!((below - above).abs() < 1e-5);
		let below = 0.003_130_8_f32 * 12.92;
		let above = 1.055 * 0.003_130_8_f32.powf(1.0 / 2.4) - 0.055;
		assert!((below - above).abs() < 1e-5);
	}

	#[test]
	fn every_byte_round_trips() {
		for c in 0..=255u8 {
			let back = (linear_to_srgb(srgb8_to_linear(c)) * 255.0).round() as u8;
			assert_eq!(back, c);
		}
	}

	#[test]
	fn linear_rgb_converts_each_channel() {
		let c = Color32::from_rgb(255, 128, 0);
		assert_eq!(
			linear_rgb(c),
			[srgb8_to_linear(255), srgb8_to_linear(128), 0.0]
		);
	}
}
