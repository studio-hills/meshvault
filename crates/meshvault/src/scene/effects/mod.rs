//! A model's visual effects, simulated: ParticleEmitters (and the legacy
//! Fire, Smoke and Sparkles, drawn as emitters with settings like
//! Studio's), Beams and Trails. Each frame they are stepped on from where
//! their parts are, and give an [`EffectFrame`] of sprites and ribbons in
//! world space, which the renderer turns to face the camera.
//!
//! Lighting is not applied: effects are drawn in their own colour, times
//! `Brightness`, as Studio draws them with `LightInfluence` 0. Particles do
//! not inherit their part's velocity, and wind is not simulated.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::anim::effect::{ColourKey, Colours, EffectClass, EffectInfo, NumberKey, Numbers};
use crate::anim::math::{CFrame, V3, cross, dot};
use crate::scene::colour::srgb_to_linear;

mod textures;

pub use self::textures::Builtin;

/// The most particles one emitter keeps, and the most it starts in one
/// step, so a hostile export cannot stall the viewer.
const MAX_PARTICLES: usize = 4_000;
const MAX_SPAWN_PER_STEP: usize = 500;
/// The longest step simulated at once; a longer pause is caught up in steps
/// this long.
const MAX_STEP: f32 = 1.0 / 20.0;
/// How long effects run before they are first shown, so emitters are full
/// as in Studio, not just starting.
pub const WARM_UP: f32 = 2.0;
/// The most points a trail keeps.
const MAX_TRAIL_POINTS: usize = 512;
/// The most segments a beam is drawn with (Roblox allows 1000).
const MAX_BEAM_SEGMENTS: usize = 1_000;

/// The texture an effect draws with.
#[derive(Clone, Debug)]
pub enum EffectTexture {
	/// None: the effect's plain colour (a Beam or Trail without a texture).
	Plain,
	/// A stand-in for a texture built into Studio.
	Builtin(Builtin),
	/// A downloaded image.
	Image(Arc<image::RgbaImage>),
}

impl EffectTexture {
	/// The texture for an effect's `Texture`: `image` gives a downloaded
	/// image by asset id, or `None` when it is not downloaded (or does not
	/// load), drawn with the soft stand-in.
	pub fn for_content(
		content: Option<&str>,
		image: &mut dyn FnMut(u64) -> Option<Arc<image::RgbaImage>>,
	) -> (EffectTexture, bool) {
		let content = content.unwrap_or("").trim();
		if content.is_empty() {
			return (EffectTexture::Plain, true);
		}
		if content.starts_with("rbxasset://") {
			return (EffectTexture::Builtin(Builtin::for_path(content)), true);
		}
		match crate::content_id::parse_asset_id(content).and_then(&mut *image) {
			Some(pixels) => (EffectTexture::Image(pixels), true),
			None => (EffectTexture::Builtin(Builtin::Soft), false),
		}
	}
}

/// Where an effect sits: its part, Attachment or Bone, in the world now.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
	pub world: CFrame,
	/// The part's size, which particles start within; zero for an
	/// Attachment or Bone, which emits from its point.
	pub extent: V3,
}

/// What effects draw this frame, in world space.
#[derive(Clone, Debug, Default)]
pub struct EffectFrame {
	pub sprites: Vec<Sprite>,
	pub ribbons: Vec<Ribbon>,
}

/// How a sprite turns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Facing {
	/// To the camera, rolled by the sprite's rotation.
	Camera,
	/// To the camera, but kept upright.
	CameraWorldUp,
	/// Along the velocity, turned to the camera about it.
	VelocityParallel(V3),
	/// Square to the velocity.
	VelocityPerpendicular(V3),
}

/// One particle, drawn as a quad.
#[derive(Clone, Debug)]
pub struct Sprite {
	pub at: V3,
	/// Width and height in studs.
	pub size: [f32; 2],
	/// Roll in radians.
	pub rotation: f32,
	pub facing: Facing,
	/// The part of the texture shown: u0, v0, u1, v1 (a flipbook frame).
	pub uv: [f32; 4],
	/// Linear colour (times brightness) and alpha.
	pub colour: [f32; 4],
	/// `LightEmission`: 0 blends over what is behind, 1 adds to it.
	pub emission: f32,
	/// Studs it is drawn towards the camera.
	pub z_offset: f32,
	pub texture: EffectTexture,
}

/// A Beam or Trail: a strip through its points.
#[derive(Clone, Debug)]
pub struct Ribbon {
	pub points: Vec<RibbonPoint>,
	pub emission: f32,
	pub z_offset: f32,
	pub texture: EffectTexture,
}

#[derive(Clone, Copy, Debug)]
pub struct RibbonPoint {
	pub at: V3,
	pub half_width: f32,
	/// The direction the width runs; `None` turns it to the camera.
	pub across: Option<V3>,
	/// Texture coordinate along the strip.
	pub u: f32,
	pub colour: [f32; 4],
}

/// A model's effects, each on an anchor the model gives its place every
/// frame.
#[derive(Clone, Debug, Default)]
pub struct Effects {
	effects: Vec<Effect>,
	/// Seconds simulated, for scrolling beam textures and ageing trails.
	time: f32,
}

#[derive(Clone, Debug)]
struct Effect {
	enabled: bool,
	texture: EffectTexture,
	rng: Rng,
	kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
	Emitter(Emitter),
	Beam(Beam),
	Trail(Trail),
}

impl Effects {
	/// The effects of a model. `anchor` gives the anchor of a part,
	/// Attachment or Bone by path; `image` a downloaded image by asset id.
	/// Effects whose anchor is not in the model are left out. Returns how
	/// many were left out and how many textures were not downloaded.
	pub fn new(
		infos: &[EffectInfo],
		anchor: &dyn Fn(&str) -> Option<usize>,
		image: &mut dyn FnMut(u64) -> Option<Arc<image::RgbaImage>>,
	) -> (Effects, Unplaced) {
		let mut unplaced = Unplaced::default();
		let mut effects = Vec::new();
		for info in infos {
			let Some(class) = info.class() else {
				unplaced.unknown += 1;
				continue;
			};
			let kind = match class {
				EffectClass::Beam | EffectClass::Trail => {
					let ends = info
						.attachment0_path
						.as_deref()
						.and_then(anchor)
						.zip(info.attachment1_path.as_deref().and_then(anchor));
					let Some((a0, a1)) = ends else {
						unplaced.unanchored += 1;
						continue;
					};
					if class == EffectClass::Beam {
						Kind::Beam(Beam::new(info, a0, a1))
					} else {
						Kind::Trail(Trail::new(info, a0, a1))
					}
				}
				_ => {
					let Some(at) = anchor(&info.parent_path) else {
						unplaced.unanchored += 1;
						continue;
					};
					Kind::Emitter(Emitter::new(info, class, at))
				}
			};
			let (texture, found) = match class {
				EffectClass::Fire => (EffectTexture::Builtin(Builtin::Flame), true),
				EffectClass::Smoke => (EffectTexture::Builtin(Builtin::Soft), true),
				EffectClass::Sparkles => (EffectTexture::Builtin(Builtin::Sparkle), true),
				_ => EffectTexture::for_content(info.texture.as_deref(), image),
			};
			if !found {
				unplaced.textures += 1;
			}
			effects.push(Effect {
				enabled: info.is_enabled(),
				texture,
				rng: Rng::new(&info.path),
				kind,
			});
		}
		(Effects { effects, time: 0.0 }, unplaced)
	}

	/// How many effects there are, on or off.
	pub fn count(&self) -> usize {
		self.effects.len()
	}

	/// Whether any effect is on (and so changes from frame to frame).
	pub fn any_enabled(&self) -> bool {
		self.effects.iter().any(|e| e.enabled)
	}

	/// Runs the effects on by `dt` seconds with their anchors at `anchors`,
	/// in steps of at most [`MAX_STEP`].
	pub fn step(&mut self, dt: f32, anchors: &[Anchor]) {
		let mut left = dt.max(0.0);
		while left > 0.0 {
			let dt = left.min(MAX_STEP);
			left -= dt;
			self.time += dt;
			let time = self.time;
			for effect in self.effects.iter_mut().filter(|e| e.enabled) {
				let rng = &mut effect.rng;
				match &mut effect.kind {
					Kind::Emitter(e) => e.step(dt, anchors, rng),
					Kind::Beam(_) => {}
					Kind::Trail(t) => t.step(time, anchors),
				}
			}
		}
	}

	/// What the effects draw now.
	pub fn frame(&self, anchors: &[Anchor]) -> EffectFrame {
		let mut frame = EffectFrame::default();
		for effect in self.effects.iter().filter(|e| e.enabled) {
			match &effect.kind {
				Kind::Emitter(e) => e.sprites(anchors, &effect.texture, &mut frame.sprites),
				Kind::Beam(b) => {
					frame
						.ribbons
						.extend(b.ribbon(self.time, anchors, &effect.texture))
				}
				Kind::Trail(t) => frame.ribbons.extend(t.ribbon(self.time, &effect.texture)),
			}
		}
		frame
	}
}

/// Effects that could not be placed, for the model's warnings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unplaced {
	/// Classes Meshvault does not draw.
	pub unknown: usize,
	/// On, or between, things not in the model.
	pub unanchored: usize,
	/// Textures not downloaded (drawn with a stand-in).
	pub textures: usize,
}

// --- particles

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
	Box,
	Sphere,
	Cylinder,
	Disc,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Flip {
	Loop,
	OneShot,
	PingPong,
	Random,
}

#[derive(Clone, Debug)]
struct Emitter {
	anchor: usize,
	rate: f32,
	lifetime: (f32, f32),
	speed: (f32, f32),
	rotation: (f32, f32),
	rot_speed: (f32, f32),
	/// Radians either side of the direction, about two axes.
	spread: [f32; 2],
	/// In the anchor's space, unless `world_direction` is set.
	direction: V3,
	/// Fire and Smoke rise whichever way their part faces.
	world_direction: bool,
	acceleration: V3,
	drag: f32,
	locked: bool,
	orientation: Orientation,
	size: Numbers,
	transparency: Numbers,
	colour: Colours,
	squash: Option<Numbers>,
	shape: Shape,
	surface: bool,
	/// 1 outward, -1 inward, 0 either.
	outward: f32,
	/// Flipbook grid (frames across), mode and frame rate.
	flipbook: Option<(u32, Flip, (f32, f32), bool)>,
	emission: f32,
	brightness: f32,
	time_scale: f32,
	z_offset: f32,
	/// Particles owed for the fraction of one not yet started.
	owed: f32,
	particles: Vec<Particle>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Orientation {
	FacingCamera,
	FacingCameraWorldUp,
	VelocityParallel,
	VelocityPerpendicular,
}

#[derive(Clone, Copy, Debug)]
struct Particle {
	/// In the world, or in the anchor's space when locked to it.
	at: V3,
	velocity: V3,
	age: f32,
	life: f32,
	rotation: f32,
	rot_speed: f32,
	/// Where in each keypoint's envelope this particle's size and
	/// transparency sit (-1..1).
	spread: f32,
	/// Flipbook frames per second, and the frame it started on.
	fps: f32,
	first_frame: u32,
}

fn range(n: Option<&Numbers>, default: (f32, f32)) -> (f32, f32) {
	match n {
		Some(Numbers::Range(lo, hi)) => (*lo, *hi),
		Some(other) => (other.first(), other.first()),
		None => default,
	}
}

fn sequence(values: &[(f32, f32)]) -> Numbers {
	Numbers::Sequence(
		values
			.iter()
			.map(|&(time, value)| NumberKey {
				time,
				value,
				envelope: 0.0,
			})
			.collect(),
	)
}

/// A face's outward direction (`Enum.NormalId`), Top when unknown.
fn normal_of(face: Option<&str>) -> V3 {
	match face.unwrap_or("Top") {
		"Bottom" => [0.0, -1.0, 0.0],
		"Front" => [0.0, 0.0, -1.0],
		"Back" => [0.0, 0.0, 1.0],
		"Left" => [-1.0, 0.0, 0.0],
		"Right" => [1.0, 0.0, 0.0],
		_ => [0.0, 1.0, 0.0],
	}
}

impl Emitter {
	fn new(info: &EffectInfo, class: EffectClass, anchor: usize) -> Emitter {
		let mut e = Emitter {
			anchor,
			rate: info.rate.unwrap_or(20.0),
			lifetime: range(info.lifetime.as_ref(), (5.0, 10.0)),
			speed: range(info.speed.as_ref(), (5.0, 5.0)),
			rotation: range(info.rotation.as_ref(), (0.0, 0.0)),
			rot_speed: range(info.rot_speed.as_ref(), (0.0, 0.0)),
			spread: info.spread_angle.unwrap_or([0.0; 2]).map(f32::to_radians),
			direction: normal_of(info.emission_direction.as_deref()),
			world_direction: false,
			acceleration: info.acceleration.unwrap_or([0.0; 3]),
			drag: info.drag.unwrap_or(0.0),
			locked: info.locked_to_part.unwrap_or(false),
			orientation: match info.orientation.as_deref() {
				Some("FacingCameraWorldUp") => Orientation::FacingCameraWorldUp,
				Some("VelocityParallel") => Orientation::VelocityParallel,
				Some("VelocityPerpendicular") => Orientation::VelocityPerpendicular,
				_ => Orientation::FacingCamera,
			},
			size: info.size.clone().unwrap_or(Numbers::Number(1.0)),
			transparency: info.transparency.clone().unwrap_or(Numbers::Number(0.0)),
			colour: info.color.clone().unwrap_or(Colours::Colour([1.0; 3])),
			squash: info.squash.clone(),
			shape: match info.shape.as_deref() {
				Some("Sphere") => Shape::Sphere,
				Some("Cylinder") => Shape::Cylinder,
				Some("Disc") => Shape::Disc,
				_ => Shape::Box,
			},
			surface: info.shape_style.as_deref() == Some("Surface"),
			outward: match info.shape_in_out.as_deref() {
				Some("Inward") => -1.0,
				Some("InAndOut") => 0.0,
				_ => 1.0,
			},
			flipbook: {
				let grid = match info.flipbook_layout.as_deref() {
					Some("Grid2x2") => 2,
					Some("Grid4x4") => 4,
					Some("Grid8x8") => 8,
					_ => 1,
				};
				let mode = match info.flipbook_mode.as_deref() {
					Some("OneShot") => Flip::OneShot,
					Some("PingPong") => Flip::PingPong,
					Some("Random") => Flip::Random,
					_ => Flip::Loop,
				};
				(grid > 1).then(|| {
					(
						grid,
						mode,
						range(info.flipbook_framerate.as_ref(), (1.0, 1.0)),
						info.flipbook_start_random.unwrap_or(false),
					)
				})
			},
			emission: info.light_emission.unwrap_or(0.0).clamp(0.0, 1.0),
			brightness: info.brightness.unwrap_or(1.0).max(0.0),
			time_scale: info.time_scale.unwrap_or(1.0).max(0.0),
			z_offset: info.z_offset.unwrap_or(0.0),
			owed: 0.0,
			particles: Vec::new(),
		};
		// the legacy effects, with settings that look like Studio's
		match class {
			EffectClass::Fire => {
				let size = info
					.size
					.as_ref()
					.map_or(5.0, Numbers::first)
					.clamp(2.0, 30.0);
				let heat = info.heat.unwrap_or(9.0).clamp(-25.0, 25.0);
				let outer = match &info.color {
					Some(c) => c.at(0.0),
					None => [236.0 / 255.0, 139.0 / 255.0, 70.0 / 255.0],
				};
				let inner =
					info.secondary_color
						.unwrap_or([139.0 / 255.0, 80.0 / 255.0, 55.0 / 255.0]);
				e.rate = 65.0;
				e.lifetime = (0.5, 0.9);
				e.speed = (heat * 0.45, heat * 0.7);
				e.direction = [0.0, 1.0, 0.0];
				e.world_direction = true;
				e.spread = [0.25, 0.25];
				e.rot_speed = (-90.0, 90.0);
				e.size = sequence(&[(0.0, size * 0.32), (1.0, size * 0.08)]);
				e.transparency = sequence(&[(0.0, 0.25), (0.6, 0.45), (1.0, 1.0)]);
				e.colour = Colours::Sequence(vec![
					ColourKey {
						time: 0.0,
						value: outer,
					},
					ColourKey {
						time: 1.0,
						value: inner,
					},
				]);
				e.emission = 0.85;
			}
			EffectClass::Smoke => {
				let size = info
					.size
					.as_ref()
					.map_or(1.0, Numbers::first)
					.clamp(0.1, 100.0);
				let opacity = info.opacity.unwrap_or(0.5).clamp(0.0, 1.0);
				let rise = info.rise_velocity.unwrap_or(1.0).clamp(-25.0, 25.0);
				let colour = match &info.color {
					Some(c) => c.at(0.0),
					None => [1.0; 3],
				};
				e.rate = 10.0;
				e.lifetime = (4.0, 6.0);
				e.speed = (rise * 0.8, rise * 1.2);
				e.direction = [0.0, 1.0, 0.0];
				e.world_direction = true;
				e.spread = [0.35, 0.35];
				e.rot_speed = (-30.0, 30.0);
				e.size = sequence(&[(0.0, size), (1.0, size * 4.0)]);
				e.transparency = sequence(&[(0.0, 1.0), (0.15, 1.0 - opacity), (1.0, 1.0)]);
				e.colour = Colours::Colour(colour);
				e.emission = 0.0;
			}
			EffectClass::Sparkles => {
				let colour = info
					.sparkle_color
					.unwrap_or([144.0 / 255.0, 25.0 / 255.0, 1.0]);
				e.rate = 25.0;
				e.lifetime = (0.6, 1.0);
				e.speed = (2.0, 4.0);
				e.spread = [std::f32::consts::PI; 2];
				e.size = sequence(&[(0.0, 0.5), (1.0, 0.0)]);
				e.transparency = Numbers::Number(0.0);
				e.colour = Colours::Colour(colour);
				e.emission = 1.0;
			}
			_ => {}
		}
		e
	}

	fn step(&mut self, dt: f32, anchors: &[Anchor], rng: &mut Rng) {
		let Some(anchor) = anchors.get(self.anchor).copied() else {
			return;
		};
		let dt = dt * self.time_scale;
		let decay = if self.drag > 0.0 {
			0.5f32.powf(self.drag * dt)
		} else {
			1.0
		};
		// acceleration is in the world; a locked particle moves in its part's space
		let acceleration = if self.locked {
			anchor.world.inverse().rotate(self.acceleration)
		} else {
			self.acceleration
		};
		self.particles.retain_mut(|p| {
			p.age += dt;
			if p.age >= p.life {
				return false;
			}
			for ((v, a), at) in p.velocity.iter_mut().zip(acceleration).zip(&mut p.at) {
				*v = (*v + a * dt) * decay;
				*at += *v * dt;
			}
			p.rotation += p.rot_speed * dt;
			true
		});
		self.owed += self.rate.max(0.0) * dt;
		let due = (self.owed.floor() as usize).min(MAX_SPAWN_PER_STEP);
		self.owed -= self.owed.floor();
		let room = MAX_PARTICLES.saturating_sub(self.particles.len());
		for _ in 0..due.min(room) {
			let p = self.spawn(&anchor, rng);
			self.particles.push(p);
		}
	}

	/// A new particle at the emitter, in the world (or the anchor's space
	/// when locked).
	fn spawn(&self, anchor: &Anchor, rng: &mut Rng) -> Particle {
		let half = anchor.extent.map(|e| e * 0.5);
		let (local, normal) = self.origin(half, rng);
		let base = match (self.shape, normal) {
			// rounded shapes emit along their surface's normal
			(Shape::Sphere | Shape::Cylinder | Shape::Disc, Some(n)) if half != [0.0; 3] => {
				let sign = if self.outward == 0.0 {
					if rng.next() < 0.5 { -1.0 } else { 1.0 }
				} else {
					self.outward
				};
				n.map(|v| v * sign)
			}
			_ => self.direction,
		};
		let direction = spread(base, self.spread, rng);
		let speed = lerp(self.speed.0, self.speed.1, rng.next());
		let (at, velocity) = if self.locked {
			(local, direction.map(|v| v * speed))
		} else if self.world_direction {
			(anchor.world.point(local), direction.map(|v| v * speed))
		} else {
			(
				anchor.world.point(local),
				anchor.world.rotate(direction).map(|v| v * speed),
			)
		};
		let (fps, first_frame) = match self.flipbook {
			Some((grid, _, fps, random)) => {
				let first = if random {
					(rng.next() * (grid * grid) as f32) as u32
				} else {
					0
				};
				(lerp(fps.0, fps.1, rng.next()), first)
			}
			None => (0.0, 0),
		};
		Particle {
			at,
			velocity,
			age: 0.0,
			life: lerp(self.lifetime.0, self.lifetime.1, rng.next()).max(0.01),
			rotation: lerp(self.rotation.0, self.rotation.1, rng.next()).to_radians(),
			rot_speed: lerp(self.rot_speed.0, self.rot_speed.1, rng.next()).to_radians(),
			spread: rng.next() * 2.0 - 1.0,
			fps,
			first_frame,
		}
	}

	/// Where in the anchor's space a particle starts, within (or on) the
	/// shape that fills `half` either way, and the shape's normal there.
	fn origin(&self, half: V3, rng: &mut Rng) -> (V3, Option<V3>) {
		let mut r = || rng.next() * 2.0 - 1.0;
		match self.shape {
			Shape::Box => {
				let mut p = [r(), r(), r()];
				if self.surface {
					// out to a face, picked by its area
					let areas = [half[1] * half[2], half[0] * half[2], half[0] * half[1]];
					let total: f32 = areas.iter().sum();
					let mut pick = (r() * 0.5 + 0.5) * total;
					let mut axis = 2;
					for (k, a) in areas.iter().enumerate() {
						if pick <= *a {
							axis = k;
							break;
						}
						pick -= a;
					}
					p[axis] = if r() < 0.0 { -1.0 } else { 1.0 };
				}
				([p[0] * half[0], p[1] * half[1], p[2] * half[2]], None)
			}
			Shape::Sphere => {
				let n = normalize([r(), r(), r()]);
				let depth = if self.surface {
					1.0
				} else {
					(r() * 0.5 + 0.5).cbrt()
				};
				(
					[
						n[0] * half[0] * depth,
						n[1] * half[1] * depth,
						n[2] * half[2] * depth,
					],
					Some(n),
				)
			}
			Shape::Cylinder | Shape::Disc => {
				let angle = r() * std::f32::consts::PI;
				let (s, c) = angle.sin_cos();
				let depth = if self.surface {
					1.0
				} else {
					(r() * 0.5 + 0.5).sqrt()
				};
				let height = if self.shape == Shape::Disc {
					0.0
				} else {
					r() * half[1]
				};
				(
					[c * half[0] * depth, height, s * half[2] * depth],
					Some([c, 0.0, s]),
				)
			}
		}
	}

	fn sprites(&self, anchors: &[Anchor], texture: &EffectTexture, out: &mut Vec<Sprite>) {
		let Some(anchor) = anchors.get(self.anchor) else {
			return;
		};
		for p in &self.particles {
			let t = (p.age / p.life).clamp(0.0, 1.0);
			let size = self.size.at(t, p.spread).max(0.0);
			let alpha = 1.0 - self.transparency.at(t, p.spread).clamp(0.0, 1.0);
			if size <= 0.0 || alpha <= 0.0 {
				continue;
			}
			let (at, velocity) = if self.locked {
				(anchor.world.point(p.at), anchor.world.rotate(p.velocity))
			} else {
				(p.at, p.velocity)
			};
			// Squash: above 0 taller and thinner, below 0 wider and flatter
			let squash = self
				.squash
				.as_ref()
				.map_or(0.0, |s| s.at(t, 0.0))
				.clamp(-3.0, 3.0);
			let stretch = 1.0 + squash.abs();
			let (w, h) = if squash >= 0.0 {
				(size / stretch, size * stretch)
			} else {
				(size * stretch, size / stretch)
			};
			let colour = self
				.colour
				.at(t)
				.map(|c| srgb_to_linear(c.clamp(0.0, 1.0)) * self.brightness);
			out.push(Sprite {
				at,
				size: [w, h],
				rotation: p.rotation,
				facing: match self.orientation {
					Orientation::FacingCamera => Facing::Camera,
					Orientation::FacingCameraWorldUp => Facing::CameraWorldUp,
					Orientation::VelocityParallel => Facing::VelocityParallel(velocity),
					Orientation::VelocityPerpendicular => Facing::VelocityPerpendicular(velocity),
				},
				uv: self.frame_uv(p, t),
				colour: [colour[0], colour[1], colour[2], alpha],
				emission: self.emission,
				z_offset: self.z_offset,
				texture: texture.clone(),
			});
		}
	}

	/// The flipbook frame a particle shows, as a texture rectangle.
	fn frame_uv(&self, p: &Particle, t: f32) -> [f32; 4] {
		let Some((grid, mode, _, _)) = self.flipbook else {
			return [0.0, 0.0, 1.0, 1.0];
		};
		let frames = grid * grid;
		let ticks = (p.age * p.fps) as u32;
		let frame = match mode {
			Flip::Loop => (p.first_frame + ticks) % frames,
			Flip::OneShot => ((t * frames as f32) as u32).min(frames - 1),
			Flip::PingPong => {
				let period = (2 * frames).saturating_sub(2).max(1);
				let i = (p.first_frame + ticks) % period;
				if i < frames { i } else { period - i }
			}
			Flip::Random => {
				hash(
					p.first_frame
						.wrapping_add(ticks)
						.wrapping_mul(2_654_435_761),
				) % frames
			}
		};
		let n = grid as f32;
		let (col, row) = ((frame % grid) as f32, (frame / grid) as f32);
		[col / n, row / n, (col + 1.0) / n, (row + 1.0) / n]
	}
}

// --- beams

#[derive(Clone, Debug)]
struct Beam {
	a0: usize,
	a1: usize,
	width: (f32, f32),
	curve: (f32, f32),
	segments: usize,
	face_camera: bool,
	colour: Colours,
	transparency: Numbers,
	texture_mode: TextureMode,
	texture_length: f32,
	texture_speed: f32,
	emission: f32,
	brightness: f32,
	z_offset: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TextureMode {
	/// The texture repeats `TextureLength` times along the whole length.
	Stretch,
	/// The texture repeats every `TextureLength` studs.
	Wrap,
}

fn texture_mode(info: &EffectInfo) -> TextureMode {
	match info.texture_mode.as_deref() {
		Some("Wrap" | "Static") => TextureMode::Wrap,
		_ => TextureMode::Stretch,
	}
}

impl Beam {
	fn new(info: &EffectInfo, a0: usize, a1: usize) -> Beam {
		Beam {
			a0,
			a1,
			width: (info.width0.unwrap_or(1.0), info.width1.unwrap_or(1.0)),
			curve: (
				info.curve_size0.unwrap_or(0.0),
				info.curve_size1.unwrap_or(0.0),
			),
			segments: (info.segments.unwrap_or(10.0).round().max(1.0) as usize)
				.min(MAX_BEAM_SEGMENTS),
			face_camera: info.face_camera.unwrap_or(false),
			colour: info.color.clone().unwrap_or(Colours::Colour([1.0; 3])),
			transparency: info.transparency.clone().unwrap_or(Numbers::Number(0.5)),
			texture_mode: texture_mode(info),
			texture_length: info.texture_length.unwrap_or(1.0).max(1e-3),
			texture_speed: info.texture_speed.unwrap_or(1.0),
			emission: info.light_emission.unwrap_or(0.0).clamp(0.0, 1.0),
			brightness: info.brightness.unwrap_or(1.0).max(0.0),
			z_offset: info.z_offset.unwrap_or(0.0),
		}
	}

	/// The beam now: a Bézier curve from Attachment0 to Attachment1, its
	/// middle control points `CurveSize` studs along each attachment's X
	/// axis (the second one back along it), as Roblox draws it.
	fn ribbon(&self, time: f32, anchors: &[Anchor], texture: &EffectTexture) -> Option<Ribbon> {
		let (a0, a1) = (anchors.get(self.a0)?.world, anchors.get(self.a1)?.world);
		let x0 = a0.rotate([1.0, 0.0, 0.0]);
		let x1 = a1.rotate([1.0, 0.0, 0.0]);
		let p0 = a0.pos;
		let p3 = a1.pos;
		let p1 = add(p0, scale(x0, self.curve.0));
		let p2 = sub(p3, scale(x1, self.curve.1));
		let (up0, up1) = (a0.rotate([0.0, 1.0, 0.0]), a1.rotate([0.0, 1.0, 0.0]));
		let n = self.segments;
		let at = |t: f32| {
			let s = 1.0 - t;
			[0, 1, 2].map(|k| {
				s * s * s * p0[k]
					+ 3.0 * s * s * t * p1[k]
					+ 3.0 * s * t * t * p2[k]
					+ t * t * t * p3[k]
			})
		};
		let length: f32 = (0..n)
			.map(|i| distance(at(i as f32 / n as f32), at((i + 1) as f32 / n as f32)))
			.sum();
		let repeats = match self.texture_mode {
			TextureMode::Stretch => self.texture_length,
			TextureMode::Wrap => length / self.texture_length,
		};
		let scroll = time * self.texture_speed;
		let points = (0..=n)
			.map(|i| {
				let t = i as f32 / n as f32;
				let colour = self
					.colour
					.at(t)
					.map(|c| srgb_to_linear(c.clamp(0.0, 1.0)) * self.brightness);
				let alpha = 1.0 - self.transparency.at(t, 0.0).clamp(0.0, 1.0);
				RibbonPoint {
					at: at(t),
					half_width: lerp(self.width.0, self.width.1, t).abs() * 0.5,
					across: (!self.face_camera).then(|| normalize(lerp3(up0, up1, t))),
					u: t * repeats - scroll,
					colour: [colour[0], colour[1], colour[2], alpha],
				}
			})
			.collect();
		Some(Ribbon {
			points,
			emission: self.emission,
			z_offset: self.z_offset,
			texture: texture.clone(),
		})
	}
}

// --- trails

#[derive(Clone, Debug)]
struct Trail {
	a0: usize,
	a1: usize,
	lifetime: f32,
	min_length: f32,
	max_length: f32,
	face_camera: bool,
	colour: Colours,
	transparency: Numbers,
	width_scale: Numbers,
	texture_mode: TextureMode,
	texture_length: f32,
	emission: f32,
	brightness: f32,
	/// Where the attachments were, newest first, and when.
	points: VecDeque<(V3, V3, f32)>,
	/// Where they are now.
	head: Option<(V3, V3)>,
}

impl Trail {
	fn new(info: &EffectInfo, a0: usize, a1: usize) -> Trail {
		Trail {
			a0,
			a1,
			lifetime: info.lifetime.as_ref().map_or(2.0, Numbers::first).max(0.01),
			min_length: info.min_length.unwrap_or(0.1).max(0.0),
			max_length: info.max_length.unwrap_or(0.0).max(0.0),
			face_camera: info.face_camera.unwrap_or(false),
			colour: info.color.clone().unwrap_or(Colours::Colour([1.0; 3])),
			transparency: info.transparency.clone().unwrap_or(Numbers::Number(0.5)),
			width_scale: info.width_scale.clone().unwrap_or(Numbers::Number(1.0)),
			texture_mode: texture_mode(info),
			texture_length: info.texture_length.unwrap_or(1.0).max(1e-3),
			emission: info.light_emission.unwrap_or(0.0).clamp(0.0, 1.0),
			brightness: info.brightness.unwrap_or(1.0).max(0.0),
			points: VecDeque::new(),
			head: None,
		}
	}

	/// Records where the attachments are when they have moved far enough,
	/// and forgets what is older than the lifetime.
	fn step(&mut self, time: f32, anchors: &[Anchor]) {
		let (Some(a), Some(b)) = (anchors.get(self.a0), anchors.get(self.a1)) else {
			return;
		};
		let (a, b) = (a.world.pos, b.world.pos);
		self.head = Some((a, b));
		let moved = self.points.front().is_none_or(|(pa, pb, _)| {
			distance(mid(*pa, *pb), mid(a, b)) >= self.min_length.max(1e-4)
		});
		if moved {
			self.points.push_front((a, b, time));
			self.points.truncate(MAX_TRAIL_POINTS);
		}
		while self
			.points
			.back()
			.is_some_and(|(_, _, born)| time - born > self.lifetime)
		{
			self.points.pop_back();
		}
	}

	fn ribbon(&self, time: f32, texture: &EffectTexture) -> Option<Ribbon> {
		let (head_a, head_b) = self.head?;
		let mut list: Vec<(V3, V3, f32)> = vec![(head_a, head_b, time)];
		list.extend(self.points.iter().copied());
		let mut points = Vec::with_capacity(list.len());
		let mut travelled = 0.0;
		let mut last: Option<V3> = None;
		for (a, b, born) in list {
			let centre = mid(a, b);
			if let Some(prev) = last {
				let step = distance(prev, centre);
				// a point where the last one is adds nothing (a trail at rest)
				if step < 1e-4 {
					continue;
				}
				travelled += step;
				if self.max_length > 0.0 && travelled > self.max_length {
					break;
				}
			}
			last = Some(centre);
			let t = ((time - born) / self.lifetime).clamp(0.0, 1.0);
			let colour = self
				.colour
				.at(t)
				.map(|c| srgb_to_linear(c.clamp(0.0, 1.0)) * self.brightness);
			let alpha = 1.0 - self.transparency.at(t, 0.0).clamp(0.0, 1.0);
			let span = sub(b, a);
			points.push(RibbonPoint {
				at: centre,
				half_width: length(span) * 0.5 * self.width_scale.at(t, 0.0).max(0.0),
				across: (!self.face_camera).then(|| normalize(span)),
				u: match self.texture_mode {
					TextureMode::Stretch => t * self.texture_length,
					TextureMode::Wrap => travelled / self.texture_length,
				},
				colour: [colour[0], colour[1], colour[2], alpha],
			});
		}
		// one point is no strip: the trail shows once its attachments move
		(points.len() >= 2).then(|| Ribbon {
			points,
			emission: self.emission,
			z_offset: 0.0,
			texture: texture.clone(),
		})
	}
}

// --- small helpers

/// A direction within `spread` (radians, about two axes) of `base`.
fn spread(base: V3, spread: [f32; 2], rng: &mut Rng) -> V3 {
	if spread == [0.0; 2] {
		return base;
	}
	let helper = if base[1].abs() < 0.9 {
		[0.0, 1.0, 0.0]
	} else {
		[1.0, 0.0, 0.0]
	};
	let u = normalize(cross(base, helper));
	let v = cross(u, base);
	let ax = (rng.next() * 2.0 - 1.0) * spread[0];
	let ay = (rng.next() * 2.0 - 1.0) * spread[1];
	normalize(
		[0, 1, 2]
			.map(|k| base[k] * ax.cos() * ay.cos() + u[k] * ax.sin() + v[k] * ay.sin() * ax.cos()),
	)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
	a + (b - a) * t
}

fn lerp3(a: V3, b: V3, t: f32) -> V3 {
	[0, 1, 2].map(|k| lerp(a[k], b[k], t))
}

fn add(a: V3, b: V3) -> V3 {
	[a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
	[a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, s: f32) -> V3 {
	a.map(|v| v * s)
}

fn mid(a: V3, b: V3) -> V3 {
	lerp3(a, b, 0.5)
}

fn length(a: V3) -> f32 {
	dot(a, a).sqrt()
}

fn distance(a: V3, b: V3) -> f32 {
	length(sub(a, b))
}

fn normalize(a: V3) -> V3 {
	let l = length(a);
	if l > 1e-8 {
		a.map(|v| v / l)
	} else {
		[0.0, 1.0, 0.0]
	}
}

fn hash(mut x: u32) -> u32 {
	x ^= x >> 16;
	x = x.wrapping_mul(0x7feb_352d);
	x ^= x >> 15;
	x = x.wrapping_mul(0x846c_a68b);
	x ^ (x >> 16)
}

/// A small random number generator, seeded from the effect's path so a
/// model's effects look the same each time it opens.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
	fn new(seed: &str) -> Rng {
		let mut h: u64 = 0xcbf2_9ce4_8422_2325;
		for b in seed.bytes() {
			h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
		}
		Rng(h | 1)
	}

	/// A number from 0 to just under 1.
	fn next(&mut self) -> f32 {
		self.0 ^= self.0 << 13;
		self.0 ^= self.0 >> 7;
		self.0 ^= self.0 << 17;
		(self.0 >> 40) as f32 / (1u64 << 24) as f32
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The effects Studio exported from its test model.
	fn fixture() -> Vec<EffectInfo> {
		serde_json::from_str(include_str!("../../anim/effects_fixture.json")).unwrap()
	}

	const PARTS: [&str; 7] = [
		"Workspace/MeshvaultEffectsTest/Base",
		"Workspace/MeshvaultEffectsTest/Emitter",
		"Workspace/MeshvaultEffectsTest/Emitter/Nozzle",
		"Workspace/MeshvaultEffectsTest/PostA/BeamStart",
		"Workspace/MeshvaultEffectsTest/PostB/BeamEnd",
		"Workspace/MeshvaultEffectsTest/FirePit",
		"Workspace/MeshvaultEffectsTest/Chimney",
	];

	fn anchor_of(path: &str) -> Option<usize> {
		PARTS.iter().position(|p| *p == path)
	}

	fn at(pos: V3, extent: V3) -> Anchor {
		Anchor {
			world: CFrame::from_position(pos),
			extent,
		}
	}

	fn anchors() -> Vec<Anchor> {
		vec![
			at([0.0; 3], [30.0, 1.0, 10.0]),
			at([-12.0, 1.5, 0.0], [2.0; 3]),
			at([-12.0, 2.5, 0.0], [0.0; 3]),
			at([-4.0, 7.0, 0.0], [0.0; 3]),
			at([4.0, 7.0, 0.0], [0.0; 3]),
			at([9.0, 1.0, 0.0], [2.0, 1.0, 2.0]),
			at([13.0, 2.0, 0.0], [1.5, 3.0, 1.5]),
		]
	}

	fn effects() -> (Effects, Unplaced) {
		Effects::new(&fixture(), &anchor_of, &mut |_| None)
	}

	/// Each effect is placed on what it is in. The Gem (and its Sparkles)
	/// and the trail's attachments are left out of this test's anchors.
	#[test]
	fn effects_are_placed_on_their_anchors() {
		let (effects, unplaced) = effects();
		assert_eq!(effects.count(), 6);
		assert_eq!(
			unplaced,
			Unplaced {
				unknown: 0,
				unanchored: 2,
				textures: 0
			}
		);
		// the disabled emitter is kept, but not run
		assert_eq!(effects.effects.iter().filter(|e| !e.enabled).count(), 1);
	}

	fn emitter(effects: &Effects, n: usize) -> &Emitter {
		match &effects.effects[n].kind {
			Kind::Emitter(e) => e,
			_ => panic!("effect {n} is not an emitter"),
		}
	}

	/// The default emitter starts Rate particles a second at 5 studs a
	/// second up out of its part, the disabled one none; they live 5 to 10
	/// seconds.
	#[test]
	fn emitters_start_particles_at_their_rate() {
		let (mut effects, _) = effects();
		let anchors = anchors();
		effects.step(2.0, &anchors);
		assert!(emitter(&effects, 0).particles.is_empty());
		let default = emitter(&effects, 1);
		let n = default.particles.len();
		assert!((38..=40).contains(&n), "{n}");
		for p in &default.particles {
			assert_eq!(p.velocity, [0.0, 5.0, 0.0]);
			assert!((5.0..=10.0).contains(&p.life));
			// started within the 2 stud part, then rose
			assert!(
				(-13.0..=-11.0).contains(&p.at[0]) && p.at[1] >= 0.5,
				"{:?}",
				p.at
			);
		}
		// none outlives its life
		effects.step(10.5, &anchors);
		assert!(
			emitter(&effects, 1)
				.particles
				.iter()
				.all(|p| p.age < p.life)
		);
	}

	/// The plume falls under its acceleration, spreads within 15 degrees,
	/// shrinks and fades over its life, and glows.
	#[test]
	fn particles_follow_their_sequences() {
		let (mut effects, _) = effects();
		let anchors = anchors();
		effects.step(1.0, &anchors);
		let plume = emitter(&effects, 2);
		for p in &plume.particles {
			let horizontal = (p.velocity[0].powi(2) + p.velocity[2].powi(2)).sqrt();
			let up = p.velocity[1] + 4.0 * p.age;
			assert!(
				horizontal <= up * 15f32.to_radians().tan() * 1.5 + 1e-3,
				"{:?}",
				p.velocity
			);
		}
		let mut sprites = Vec::new();
		plume.sprites(&anchors, &EffectTexture::Plain, &mut sprites);
		assert!(!sprites.is_empty());
		for (s, p) in sprites.iter().zip(&plume.particles) {
			let t = p.age / p.life;
			assert!((s.size[0] - (1.5 + (0.2 - 1.5) * t)).abs() < 1e-4);
			assert!((s.colour[3] - (1.0 - t)).abs() < 1e-4);
			assert_eq!(s.emission, 0.8);
		}
	}

	/// A beam runs from its first attachment to its second; facing the
	/// camera it has no width direction of its own, and its colour runs
	/// along it.
	#[test]
	fn beams_run_between_their_attachments() {
		let (effects, _) = effects();
		let frame = effects.frame(&anchors());
		assert_eq!(frame.ribbons.len(), 1);
		let beam = &frame.ribbons[0];
		assert_eq!(beam.points.len(), 21);
		assert_eq!(beam.points[0].at, [-4.0, 7.0, 0.0]);
		let end = beam.points[20].at;
		assert!((end[0] - 4.0).abs() < 1e-4 && (end[1] - 7.0).abs() < 1e-4);
		assert!(beam.points.iter().all(|p| p.across.is_none()));
		assert!((beam.points[0].half_width - 0.3).abs() < 1e-6);
		assert!((beam.points[20].half_width - 0.7).abs() < 1e-6);
		// cyan at the start, magenta at the end, half see-through
		assert!(beam.points[0].colour[0] < 0.01 && beam.points[20].colour[0] > 0.99);
		assert_eq!(beam.points[0].colour[3], 0.5);
	}

	/// With FaceCamera off a beam's width runs along its attachments' Y
	/// axis, as measured in Studio.
	#[test]
	fn a_beam_not_facing_the_camera_is_as_wide_as_its_attachments_up() {
		let mut infos = fixture();
		let beam = infos.iter_mut().find(|e| e.name == "Arc").unwrap();
		beam.face_camera = Some(false);
		let (effects, _) = Effects::new(&infos, &anchor_of, &mut |_| None);
		let frame = effects.frame(&anchors());
		assert!(
			frame.ribbons[0]
				.points
				.iter()
				.all(|p| p.across == Some([0.0, 1.0, 0.0]))
		);
	}

	/// A trail draws nothing while its attachments stay put, and a strip
	/// behind them once they move.
	#[test]
	fn trails_show_where_their_attachments_have_been() {
		let info = fixture()
			.into_iter()
			.find(|e| e.class_name == "Trail")
			.unwrap();
		let mut trail = Trail::new(&info, 0, 1);
		let pair = |x: f32| [at([x, 1.0, 0.0], [0.0; 3]), at([x, -1.0, 0.0], [0.0; 3])];
		trail.step(0.0, &pair(0.0));
		trail.step(0.1, &pair(0.0));
		assert!(trail.ribbon(0.1, &EffectTexture::Plain).is_none());
		for i in 1..=5 {
			let x = i as f32;
			trail.step(0.1 + 0.1 * x, &pair(x));
		}
		let ribbon = trail.ribbon(0.6, &EffectTexture::Plain).unwrap();
		assert!(ribbon.points.len() >= 5);
		// as wide as the attachments are apart, newest first
		assert_eq!(ribbon.points[0].half_width, 1.0);
		assert_eq!(ribbon.points[0].at, [5.0, 0.0, 0.0]);
		// once its lifetime (1 second) has passed with no movement, it is gone
		trail.step(2.0, &pair(5.0));
		assert!(trail.ribbon(2.0, &EffectTexture::Plain).is_none());
	}

	/// Fire and smoke rise up the world, whichever way their part faces.
	#[test]
	fn fire_and_smoke_rise() {
		let (mut effects, _) = effects();
		let mut anchors = anchors();
		// the fire pit on its side
		anchors[5].world =
			CFrame::from_position([9.0, 1.0, 0.0]).mul(&CFrame::axis_rotation(2, 1.5));
		effects.step(1.0, &anchors);
		for n in [4, 5] {
			let e = emitter(&effects, n);
			assert!(!e.particles.is_empty());
			assert!(e.particles.iter().all(|p| p.velocity[1] > 0.0), "{n}");
		}
	}

	/// An emitter keeps at most MAX_PARTICLES however high its rate.
	#[test]
	fn emitters_are_capped() {
		let mut infos = fixture();
		let e = infos.iter_mut().find(|e| e.name == "Default").unwrap();
		e.rate = Some(1.0e9);
		let (mut effects, _) = Effects::new(&infos, &anchor_of, &mut |_| None);
		effects.step(10.0, &anchors());
		assert!(emitter(&effects, 1).particles.len() <= MAX_PARTICLES);
	}

	/// Particles locked to their part move with it.
	#[test]
	fn locked_particles_follow_their_part() {
		let mut infos = fixture();
		let e = infos.iter_mut().find(|e| e.name == "Default").unwrap();
		e.locked_to_part = Some(true);
		e.speed = Some(Numbers::Range(0.0, 0.0));
		let (mut effects, _) = Effects::new(&infos, &anchor_of, &mut |_| None);
		let mut anchors = anchors();
		effects.step(1.0, &anchors);
		let sprites = |effects: &Effects, anchors: &[Anchor]| {
			let mut out = Vec::new();
			emitter(effects, 1).sprites(anchors, &EffectTexture::Plain, &mut out);
			out.into_iter().map(|s| s.at).collect::<Vec<V3>>()
		};
		let before = sprites(&effects, &anchors);
		anchors[1].world = CFrame::from_position([-12.0, 11.5, 0.0]);
		let after = sprites(&effects, &anchors);
		assert!(!before.is_empty());
		for (b, a) in before.iter().zip(&after) {
			assert!((a[1] - b[1] - 10.0).abs() < 1e-3);
		}
	}

	/// A downloaded texture is used; one not downloaded is counted and drawn
	/// with the soft stand-in; built-in ones get theirs.
	#[test]
	fn textures_come_from_downloads_or_stand_ins() {
		let mut lookup = |id: u64| (id == 7).then(|| Arc::new(image::RgbaImage::new(1, 1)));
		let mut tex = |content: &str| EffectTexture::for_content(Some(content), &mut lookup);
		assert!(matches!(
			tex("rbxassetid://7"),
			(EffectTexture::Image(_), true)
		));
		assert!(matches!(
			tex("rbxassetid://8"),
			(EffectTexture::Builtin(Builtin::Soft), false)
		));
		assert!(matches!(
			tex("rbxasset://textures/particles/sparkles_main.dds"),
			(EffectTexture::Builtin(Builtin::Sparkle), true)
		));
		assert!(matches!(tex(""), (EffectTexture::Plain, true)));
	}
}
