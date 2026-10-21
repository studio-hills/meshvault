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

	/// Published sRGB values (IEC 61966-2-1), either side of each threshold
	/// and mid-range.
	#[test]
	fn matches_reference_values() {
		for (encoded, linear) in [
			(0.02, 0.001_547_99),
			(0.04045, 0.003_130_8),
			(0.05, 0.003_935_2),
			(0.5, 0.214_041_1),
			(0.8, 0.603_827_4),
		] {
			assert!((srgb_to_linear(encoded) - linear).abs() < 2e-6, "{encoded}");
			assert!((linear_to_srgb(linear) - encoded).abs() < 2e-5, "{linear}");
		}
	}

	/// The straight piece runs up to and including each threshold; the curve
	/// starts just above it.
	#[test]
	fn thresholds_belong_to_the_straight_piece() {
		let t = 0.04045_f32;
		assert_eq!(srgb_to_linear(t), t / 12.92);
		let up = t.next_up();
		assert_eq!(srgb_to_linear(up), ((up + 0.055) / 1.055).powf(2.4));
		let t = 0.003_130_8_f32;
		assert_eq!(linear_to_srgb(t), t * 12.92);
		let up = t.next_up();
		assert_eq!(linear_to_srgb(up), 1.055 * up.powf(1.0 / 2.4) - 0.055);
		// and the two pieces meet there, to the rounding of the constants
		assert!((linear_to_srgb(t.next_up()) - linear_to_srgb(t)).abs() < 1e-5);
		assert!((srgb_to_linear(0.04045_f32.next_up()) - srgb_to_linear(0.04045)).abs() < 1e-5);
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
