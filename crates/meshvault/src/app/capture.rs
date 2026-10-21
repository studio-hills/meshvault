//! Rendering the view offscreen to copy it or save it as a PNG or GIF.

use std::f32::consts::TAU;

use eframe::egui;

use super::{App, DEFAULT_VIEW_SIZE};
use crate::render::camera::Camera;
use crate::render::maths::IDENTITY;
use crate::render::viewport::{ScenePart, render_image};
use crate::scene::geometry::GroupKind;
use crate::scene::surface::{PART_COLOUR, Surface};
use crate::ui::theme;

impl App {
	/// Draws the view as it is now into `frames` images of `size`; with more
	/// than one, the camera turns once around the model while a selected
	/// clip plays on, `step` seconds a frame.
	fn render_view(
		&mut self,
		size: [u32; 2],
		frames: usize,
		step: f32,
	) -> Option<Vec<image::RgbaImage>> {
		let renderer = self.gpu.clone()?;
		let p = theme::palette(&self.ctx);
		let mut settings = self.view_settings(&p);
		settings.show_grid = false;
		settings.show_axes = false;
		let plain_material = self.no_material.clone();
		let plain = (!self.show_maps).then_some(&plain_material);
		let turn = |camera: &Camera, k: usize| {
			let mut camera = camera.clone();
			camera.yaw += TAU * k as f32 / frames as f32;
			camera
		};
		let mut out = Vec::with_capacity(frames);
		if let Some(model) = &mut self.model {
			let (camera, start) = (model.camera.clone(), model.time);
			let duration = model.duration();
			for k in 0..frames {
				if frames > 1 && model.current.is_some() && duration > 0.0 {
					model.set_time((start + k as f32 * step).rem_euclid(duration));
				}
				settings.lights = model.scene_lights();
				let mut parts = model.scene_parts(plain);
				out.push(render_image(
					&mut parts,
					&turn(&camera, k),
					&settings,
					&renderer,
					size,
				));
			}
			model.set_time(start);
		} else if let Some(doc) = &mut self.doc {
			let groups = doc.visible_groups();
			let geometry = &doc.loaded.geometry;
			for k in 0..frames {
				let mut parts = [ScenePart {
					geometry,
					groups: &groups,
					transform: IDENTITY,
					material: plain.unwrap_or(&doc.material),
					colour_groups: geometry.group_kind == GroupKind::Hull,
					colour: PART_COLOUR,
					scene: &mut doc.gpu,
					moved: false,
					surface: Surface {
						alpha_mode: doc.alpha,
						..Surface::OPAQUE
					},
					decal: None,
					highlight: false,
				}];
				out.push(render_image(
					&mut parts,
					&turn(&doc.camera, k),
					&settings,
					&renderer,
					size,
				));
			}
		} else {
			return None;
		}
		Some(out)
	}

	/// Copies the view as it is now, over the viewport's background.
	pub fn copy_image(&mut self) {
		let size = frame_size(self.viewport, STILL_HEIGHT);
		let Some(image) = self.render_view(size, 1, 0.0).and_then(|mut f| f.pop()) else {
			self.toasts.error("Copying images needs the GPU renderer.");
			return;
		};
		let p = theme::palette(&self.ctx);
		let image = over_gradient(&image, p.viewport_top, p.viewport_bottom);
		let pixels = egui::ColorImage::from_rgba_unmultiplied(
			[image.width() as usize, image.height() as usize],
			image.as_raw(),
		);
		self.ctx.copy_image(pixels);
		self.toasts.success("Copied the view to the clipboard");
	}

	/// Saves the view as it is now as a PNG with a transparent background.
	pub fn save_image(&mut self) {
		if self.gpu.is_none() {
			self.toasts.error("Saving images needs the GPU renderer.");
			return;
		}
		let Some(path) = self.save_dialog("Save image", "png", "PNG image") else {
			return;
		};
		let size = frame_size(self.viewport, STILL_HEIGHT);
		let Some(image) = self.render_view(size, 1, 0.0).and_then(|mut f| f.pop()) else {
			return;
		};
		self.in_background(move |_| {
			image
				.save(&path)
				.map_err(|e| format!("Could not save {}: {e}", path.display()))?;
			Ok(format!("Saved {}", path.display()))
		});
	}

	/// Saves a GIF of the camera turning once around the model, playing
	/// the selected clip, over the viewport's background.
	pub fn save_turntable(&mut self) {
		if self.gpu.is_none() {
			self.toasts
				.error("Saving turntables needs the GPU renderer.");
			return;
		}
		let Some(path) = self.save_dialog("Save turntable", "gif", "GIF animation") else {
			return;
		};
		let size = frame_size(self.viewport, TURNTABLE_HEIGHT);
		let delay = 1.0 / TURNTABLE_FPS;
		let Some(frames) = self.render_view(size, TURNTABLE_FRAMES, delay) else {
			return;
		};
		let p = theme::palette(&self.ctx);
		let (top, bottom) = (p.viewport_top, p.viewport_bottom);
		self.in_background(move |_| {
			use image::codecs::gif::{GifEncoder, Repeat};
			let file = std::fs::File::create(&path)
				.map_err(|e| format!("Could not create {}: {e}", path.display()))?;
			let mut encoder =
				GifEncoder::new_with_speed(std::io::BufWriter::new(file), GIF_ENCODER_SPEED);
			encoder
				.set_repeat(Repeat::Infinite)
				.map_err(|e| e.to_string())?;
			for frame in frames {
				let frame = over_gradient(&frame, top, bottom);
				let delay = image::Delay::from_numer_denom_ms((delay * 1000.0).round() as u32, 1);
				encoder
					.encode_frame(image::Frame::from_parts(frame, 0, 0, delay))
					.map_err(|e| format!("Could not write {}: {e}", path.display()))?;
			}
			Ok(format!("Saved {}", path.display()))
		});
	}
}

/// Height in pixels of saved images.
const STILL_HEIGHT: f32 = 1080.0;
/// A turntable: frame count, rate and height in pixels.
const TURNTABLE_FRAMES: usize = 72;
const TURNTABLE_FPS: f32 = 25.0;
const TURNTABLE_HEIGHT: f32 = 480.0;
/// The GIF encoder's trade of quality for speed: 1 is the slowest and
/// best, 30 the fastest.
const GIF_ENCODER_SPEED: i32 = 10;

/// The viewport's shape, scaled to `height` pixels (even sides).
fn frame_size(viewport: egui::Rect, height: f32) -> [u32; 2] {
	let aspect = viewport.width() / viewport.height().max(1.0);
	let aspect = if aspect.is_finite() && aspect > 0.0 {
		aspect
	} else {
		DEFAULT_VIEW_SIZE.x / DEFAULT_VIEW_SIZE.y
	};
	let even = |v: f32| ((v / 2.0).round() as u32).max(1) * 2;
	[even(height * aspect), even(height)]
}

/// `image` laid over a vertical gradient from `top` to `bottom`.
fn over_gradient(
	image: &image::RgbaImage,
	top: egui::Color32,
	bottom: egui::Color32,
) -> image::RgbaImage {
	let h = image.height().max(2) - 1;
	image::RgbaImage::from_fn(image.width(), image.height(), |x, y| {
		let t = y as f32 / h as f32;
		let bg = [top.r(), top.g(), top.b()]
			.iter()
			.zip([bottom.r(), bottom.g(), bottom.b()])
			.map(|(&a, b)| a as f32 + (b as f32 - a as f32) * t)
			.collect::<Vec<f32>>();
		let px = image.get_pixel(x, y);
		let a = px[3] as f32 / 255.0;
		let mix = |k: usize| (px[k] as f32 * a + bg[k] * (1.0 - a)).round() as u8;
		image::Rgba([mix(0), mix(1), mix(2), 255])
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn rect(w: f32, h: f32) -> egui::Rect {
		egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(w, h))
	}

	#[test]
	fn frames_keep_the_viewport_shape_with_even_sides() {
		assert_eq!(frame_size(rect(1600.0, 900.0), 1080.0), [1920, 1080]);
		assert_eq!(frame_size(rect(900.0, 900.0), 480.0), [480, 480]);
		// 721.5 x 481 rounds to the nearest even sides
		assert_eq!(frame_size(rect(300.0, 200.0), 481.0), [722, 482]);
		// never below 2 pixels
		assert_eq!(frame_size(rect(300.0, 200.0), 0.0), [2, 2]);
	}

	#[test]
	fn frames_of_a_degenerate_viewport() {
		// no width, or not a number: 16:9
		assert_eq!(frame_size(egui::Rect::ZERO, 1080.0), [1920, 1080]);
		assert_eq!(frame_size(rect(0.0, 500.0), 1080.0), [1920, 1080]);
		assert_eq!(frame_size(egui::Rect::NAN, 1080.0), [1920, 1080]);
		// no height counts as 1 pixel high
		assert_eq!(frame_size(rect(100.0, 0.0), 10.0), [1000, 10]);
	}

	#[test]
	fn images_are_laid_over_the_gradient() {
		let image = image::RgbaImage::from_raw(
			2,
			3,
			[
				[9, 9, 9, 0],
				[10, 20, 30, 255],
				[255, 255, 255, 64],
				[9, 9, 9, 0],
				[255, 255, 255, 128],
				[40, 50, 60, 255],
			]
			.concat(),
		)
		.unwrap();
		let top = egui::Color32::from_rgb(0, 0, 0);
		let bottom = egui::Color32::from_rgb(200, 100, 50);
		let out = over_gradient(&image, top, bottom);
		let pixels: Vec<[u8; 4]> = out.pixels().map(|p| p.0).collect();
		assert_eq!(
			pixels,
			[
				// top row: the top colour where transparent
				[0, 0, 0, 255],
				[10, 20, 30, 255],
				// half way down, a quarter opaque white and the bare gradient
				[139, 101, 83, 255],
				[100, 50, 25, 255],
				// bottom row: half opaque white
				[228, 178, 153, 255],
				[40, 50, 60, 255],
			]
		);
	}

	#[test]
	fn a_single_row_takes_the_top_colour() {
		let image = image::RgbaImage::from_raw(1, 1, vec![0, 0, 0, 0]).unwrap();
		let top = egui::Color32::from_rgb(12, 34, 56);
		let bottom = egui::Color32::from_rgb(200, 100, 50);
		assert_eq!(
			over_gradient(&image, top, bottom).get_pixel(0, 0).0,
			[12, 34, 56, 255]
		);
	}
}
