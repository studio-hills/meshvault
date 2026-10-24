//! A model's visual effects as exported (schema 2): ParticleEmitters,
//! Beams, Trails and the legacy Fire, Smoke and Sparkles. One record holds
//! any of them; each class fills the fields it has.
//!
//! Some properties change shape between classes (a ParticleEmitter's
//! `Size` is a NumberSequence, Fire's a number), so those are read as a
//! [`Numbers`] or [`Colours`] that takes any of the shapes Roblox writes.

use std::fmt;

use serde::de::{Deserializer, Error, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::anim::budget::{self, Kind};

/// The effect classes Meshvault draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectClass {
	ParticleEmitter,
	Beam,
	Trail,
	Fire,
	Smoke,
	Sparkles,
}

impl EffectClass {
	pub fn from_name(name: &str) -> Option<Self> {
		Some(match name {
			"ParticleEmitter" => Self::ParticleEmitter,
			"Beam" => Self::Beam,
			"Trail" => Self::Trail,
			"Fire" => Self::Fire,
			"Smoke" => Self::Smoke,
			"Sparkles" => Self::Sparkles,
			_ => return None,
		})
	}
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EffectInfo {
	pub class_name: String,
	pub name: String,
	pub path: String,
	/// The part, Attachment or Bone it is in.
	pub parent_path: String,
	/// Absent means on, Roblox's default.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub enabled: Option<bool>,
	/// A content id, a built-in `rbxasset://` texture, or empty.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub texture: Option<String>,
	/// A ColorSequence, or a Color3 (Fire, Smoke).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub color: Option<Colours>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub transparency: Option<Numbers>,
	/// A NumberSequence (ParticleEmitter), or a number (Fire, Smoke).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub size: Option<Numbers>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub light_emission: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub light_influence: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub brightness: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub time_scale: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub z_offset: Option<f32>,

	// ParticleEmitter
	#[serde(skip_serializing_if = "Option::is_none")]
	pub rate: Option<f32>,
	/// A NumberRange (ParticleEmitter), or a number (Trail).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub lifetime: Option<Numbers>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub speed: Option<Numbers>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub rotation: Option<Numbers>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub rot_speed: Option<Numbers>,
	/// Degrees either side of the emission direction, about two axes.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub spread_angle: Option<[f32; 2]>,
	/// `Enum.NormalId` name.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub emission_direction: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub acceleration: Option<[f32; 3]>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub drag: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub velocity_inheritance: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub locked_to_part: Option<bool>,
	/// `Enum.ParticleOrientation` name.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub orientation: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub squash: Option<Numbers>,
	/// `Enum.ParticleEmitterShape`, `...ShapeStyle` and `...ShapeInOut` names.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub shape: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub shape_style: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub shape_in_out: Option<String>,
	/// `Enum.ParticleFlipbookLayout` and `...Mode` names.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub flipbook_layout: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub flipbook_mode: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub flipbook_framerate: Option<Numbers>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub flipbook_start_random: Option<bool>,

	// Beam and Trail
	#[serde(skip_serializing_if = "Option::is_none")]
	pub attachment0_path: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub attachment1_path: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub face_camera: Option<bool>,
	/// `Enum.TextureMode` name.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub texture_mode: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub texture_length: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub texture_speed: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub width0: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub width1: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub curve_size0: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub curve_size1: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub segments: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub min_length: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub max_length: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub width_scale: Option<Numbers>,

	// Fire, Smoke and Sparkles
	#[serde(skip_serializing_if = "Option::is_none")]
	pub secondary_color: Option<[f32; 3]>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub heat: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub opacity: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub rise_velocity: Option<f32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub sparkle_color: Option<[f32; 3]>,
}

impl EffectInfo {
	pub fn class(&self) -> Option<EffectClass> {
		EffectClass::from_name(&self.class_name)
	}

	pub fn is_enabled(&self) -> bool {
		self.enabled.unwrap_or(true)
	}
}

/// A NumberSequence keypoint: its value at `time` (0..1), give or take
/// `envelope`, which each particle picks within once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NumberKey {
	pub time: f32,
	pub value: f32,
	#[serde(default)]
	pub envelope: f32,
}

/// A number, a NumberRange or a NumberSequence.
#[derive(Clone, Debug, PartialEq)]
pub enum Numbers {
	Number(f32),
	Range(f32, f32),
	Sequence(Vec<NumberKey>),
}

impl Numbers {
	/// A number for a property that is one: a range's lower end, or a
	/// sequence's value at its start.
	pub fn first(&self) -> f32 {
		match self {
			Self::Number(v) => *v,
			Self::Range(lo, _) => *lo,
			Self::Sequence(keys) => keys.first().map_or(0.0, |k| k.value),
		}
	}

	/// The value at `time` (0..1) along a sequence, moved within each
	/// keypoint's envelope by `spread` (-1..1); a number or range is the
	/// same all along.
	pub fn at(&self, time: f32, spread: f32) -> f32 {
		match self {
			Self::Number(v) => *v,
			Self::Range(lo, hi) => lo + (hi - lo) * 0.5,
			Self::Sequence(keys) => {
				let value = |k: &NumberKey| k.value + k.envelope * spread;
				let Some(first) = keys.first() else {
					return 0.0;
				};
				if time <= first.time {
					return value(first);
				}
				for pair in keys.windows(2) {
					let (a, b) = (&pair[0], &pair[1]);
					if time <= b.time {
						let span = (b.time - a.time).max(1e-6);
						let t = (time - a.time) / span;
						return value(a) + (value(b) - value(a)) * t;
					}
				}
				value(keys.last().unwrap_or(first))
			}
		}
	}
}

/// A ColorSequence keypoint: its colour (0..1 each) at `time` (0..1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ColourKey {
	pub time: f32,
	pub value: [f32; 3],
}

/// A Color3 or a ColorSequence.
#[derive(Clone, Debug, PartialEq)]
pub enum Colours {
	Colour([f32; 3]),
	Sequence(Vec<ColourKey>),
}

impl Colours {
	/// The colour at `time` (0..1), sRGB 0..1 each.
	pub fn at(&self, time: f32) -> [f32; 3] {
		match self {
			Self::Colour(c) => *c,
			Self::Sequence(keys) => {
				let Some(first) = keys.first() else {
					return [1.0; 3];
				};
				if time <= first.time {
					return first.value;
				}
				for pair in keys.windows(2) {
					let (a, b) = (&pair[0], &pair[1]);
					if time <= b.time {
						let t = (time - a.time) / (b.time - a.time).max(1e-6);
						return [0, 1, 2].map(|k| a.value[k] + (b.value[k] - a.value[k]) * t);
					}
				}
				keys.last().unwrap_or(first).value
			}
		}
	}
}

// --- reading: each shape Roblox writes, without buffering, within the budget

/// One element of an exported list: a number, or a keypoint.
enum Element {
	Number(f32),
	Key {
		time: f32,
		value: KeyValue,
		envelope: f32,
	},
}

enum KeyValue {
	Number(f32),
	Colour([f32; 3]),
}

impl<'de> Deserialize<'de> for Element {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		struct V;
		impl<'de> Visitor<'de> for V {
			type Value = Element;
			fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
				f.write_str("a number or a keypoint")
			}
			fn visit_f64<E: Error>(self, v: f64) -> Result<Element, E> {
				Ok(Element::Number(v as f32))
			}
			fn visit_i64<E: Error>(self, v: i64) -> Result<Element, E> {
				Ok(Element::Number(v as f32))
			}
			fn visit_u64<E: Error>(self, v: u64) -> Result<Element, E> {
				Ok(Element::Number(v as f32))
			}
			fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Element, A::Error> {
				let (mut time, mut value, mut envelope) = (0.0, None, 0.0);
				while let Some(key) = map.next_key::<String>()? {
					match key.as_str() {
						"Time" => time = map.next_value()?,
						"Envelope" => envelope = map.next_value()?,
						"Value" => value = Some(map.next_value::<KeyValue>()?),
						_ => {
							map.next_value::<IgnoredAny>()?;
						}
					}
				}
				let value = value.ok_or_else(|| A::Error::missing_field("Value"))?;
				Ok(Element::Key {
					time,
					value,
					envelope,
				})
			}
		}
		d.deserialize_any(V)
	}
}

impl<'de> Deserialize<'de> for KeyValue {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		struct V;
		impl<'de> Visitor<'de> for V {
			type Value = KeyValue;
			fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
				f.write_str("a number or a colour")
			}
			fn visit_f64<E: Error>(self, v: f64) -> Result<KeyValue, E> {
				Ok(KeyValue::Number(v as f32))
			}
			fn visit_i64<E: Error>(self, v: i64) -> Result<KeyValue, E> {
				Ok(KeyValue::Number(v as f32))
			}
			fn visit_u64<E: Error>(self, v: u64) -> Result<KeyValue, E> {
				Ok(KeyValue::Number(v as f32))
			}
			fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<KeyValue, A::Error> {
				Ok(KeyValue::Colour(rgb(seq)?))
			}
		}
		d.deserialize_any(V)
	}
}

/// Three numbers from `seq`, ignoring any more (bounded: at most three are
/// kept).
fn rgb<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<[f32; 3], A::Error> {
	let mut out = [0.0; 3];
	let mut n = 0;
	while let Some(v) = seq.next_element::<f32>()? {
		if n < 3 {
			out[n] = v;
		}
		n += 1;
	}
	if n < 3 {
		return Err(A::Error::invalid_length(n, &"three numbers"));
	}
	Ok(out)
}

/// A number, or a list of numbers or keypoints, read within the keypoint
/// cap.
enum Raw {
	Number(f32),
	List(Vec<Element>),
}

impl<'de> Deserialize<'de> for Raw {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		struct V;
		impl<'de> Visitor<'de> for V {
			type Value = Raw;
			fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
				f.write_str("a number or a list")
			}
			fn visit_f64<E: Error>(self, v: f64) -> Result<Raw, E> {
				Ok(Raw::Number(v as f32))
			}
			fn visit_i64<E: Error>(self, v: i64) -> Result<Raw, E> {
				Ok(Raw::Number(v as f32))
			}
			fn visit_u64<E: Error>(self, v: u64) -> Result<Raw, E> {
				Ok(Raw::Number(v as f32))
			}
			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Raw, A::Error> {
				let mut list = Vec::new();
				while let Some(e) = seq.next_element::<Element>()? {
					budget::push(&mut list, e, Kind::Keypoints).map_err(A::Error::custom)?;
				}
				Ok(Raw::List(list))
			}
		}
		d.deserialize_any(V)
	}
}

impl<'de> Deserialize<'de> for Numbers {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		Ok(match Raw::deserialize(d)? {
			Raw::Number(v) => Numbers::Number(v),
			Raw::List(list) => {
				if list.iter().all(|e| matches!(e, Element::Number(_))) {
					let at = |i: usize| match list.get(i) {
						Some(Element::Number(v)) => *v,
						_ => 0.0,
					};
					match list.len() {
						0 => Numbers::Sequence(Vec::new()),
						1 => Numbers::Number(at(0)),
						_ => Numbers::Range(at(0), at(1)),
					}
				} else {
					Numbers::Sequence(
						list.into_iter()
							.filter_map(|e| match e {
								Element::Key {
									time,
									value: KeyValue::Number(value),
									envelope,
								} => Some(NumberKey {
									time,
									value,
									envelope,
								}),
								_ => None,
							})
							.collect(),
					)
				}
			}
		})
	}
}

impl<'de> Deserialize<'de> for Colours {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		match Raw::deserialize(d)? {
			Raw::Number(_) => Err(D::Error::custom("a colour cannot be a number")),
			Raw::List(list) => {
				if list.iter().all(|e| matches!(e, Element::Number(_))) {
					let channels: Vec<f32> = list
						.iter()
						.map(|e| match e {
							Element::Number(v) => *v,
							_ => 0.0,
						})
						.collect();
					if channels.len() < 3 {
						return Err(D::Error::custom("a colour needs three numbers"));
					}
					Ok(Colours::Colour([channels[0], channels[1], channels[2]]))
				} else {
					Ok(Colours::Sequence(
						list.into_iter()
							.filter_map(|e| match e {
								Element::Key {
									time,
									value: KeyValue::Colour(value),
									..
								} => Some(ColourKey { time, value }),
								_ => None,
							})
							.collect(),
					))
				}
			}
		}
	}
}

impl Serialize for Numbers {
	fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		match self {
			Self::Number(v) => v.serialize(s),
			Self::Range(lo, hi) => [lo, hi].serialize(s),
			Self::Sequence(keys) => keys.serialize(s),
		}
	}
}

impl Serialize for Colours {
	fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		match self {
			Self::Colour(c) => c.serialize(s),
			Self::Sequence(keys) => keys.serialize(s),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::anim::budget::{LIMITS, Limits, with_budget};

	/// The effects Studio exported from a test model: one of each class,
	/// and an emitter turned off.
	fn fixture() -> Vec<EffectInfo> {
		serde_json::from_str(include_str!("effects_fixture.json")).unwrap()
	}

	fn named<'a>(effects: &'a [EffectInfo], name: &str) -> &'a EffectInfo {
		effects.iter().find(|e| e.name == name).unwrap()
	}

	#[test]
	fn reads_every_class_as_studio_exports_it() {
		let effects = fixture();
		let classes: Vec<_> = effects.iter().map(|e| e.class().unwrap()).collect();
		use EffectClass::*;
		assert_eq!(
			classes,
			[
				Trail,
				ParticleEmitter,
				ParticleEmitter,
				ParticleEmitter,
				Beam,
				Fire,
				Smoke,
				Sparkles
			]
		);
		// a ParticleEmitter's sequences and ranges
		let plume = named(&effects, "Plume");
		assert_eq!(plume.lifetime, Some(Numbers::Range(1.5, 2.5)));
		assert_eq!(plume.size.as_ref().unwrap().at(0.5, 0.0), (1.5 + 0.2) / 2.0);
		assert_eq!(plume.transparency.as_ref().unwrap().at(1.0, 0.0), 1.0);
		let orange = plume.color.as_ref().unwrap().at(0.0);
		assert!((orange[1] - 120.0 / 255.0).abs() < 1e-6);
		assert_eq!(plume.spread_angle, Some([15.0, 15.0]));
		assert_eq!(plume.acceleration, Some([0.0, -4.0, 0.0]));
		assert!(plume.is_enabled());
		assert!(!named(&effects, "Off").is_enabled());
		// the same names in other shapes: Fire's Size and Color, a Trail's Lifetime
		let fire = named(&effects, "Fire");
		assert_eq!(fire.size, Some(Numbers::Number(5.0)));
		assert!(matches!(fire.color, Some(Colours::Colour(_))));
		assert_eq!(
			named(&effects, "Trail").lifetime,
			Some(Numbers::Number(1.0))
		);
		let beam = named(&effects, "Arc");
		assert_eq!(
			beam.attachment1_path.as_deref(),
			Some("Workspace/MeshvaultEffectsTest/PostB/BeamEnd")
		);
		assert_eq!(
			(beam.width0, beam.curve_size1, beam.face_camera),
			(Some(0.6), Some(-3.0), Some(true))
		);
	}

	/// What is read is written back the same, as rig files keep effects.
	#[test]
	fn effects_round_trip() {
		let effects = fixture();
		let text = serde_json::to_string(&effects).unwrap();
		let again: Vec<EffectInfo> = serde_json::from_str(&text).unwrap();
		assert_eq!(format!("{effects:?}"), format!("{again:?}"));
	}

	/// Envelopes move a sequence's values by each particle's spread.
	#[test]
	fn sequences_interpolate_and_spread() {
		let size = Numbers::Sequence(vec![
			NumberKey {
				time: 0.0,
				value: 1.0,
				envelope: 0.5,
			},
			NumberKey {
				time: 1.0,
				value: 3.0,
				envelope: 0.0,
			},
		]);
		assert_eq!(size.at(0.5, 0.0), 2.0);
		assert_eq!(size.at(0.0, 1.0), 1.5);
		assert_eq!(size.at(0.0, -1.0), 0.5);
		assert_eq!(size.at(2.0, 0.0), 3.0);
	}

	/// A sequence longer than the cap is refused, not read whole.
	#[test]
	fn a_sequence_over_the_cap_is_refused() {
		let limits = Limits {
			keypoints: 2,
			..LIMITS
		};
		let keys = |n: usize| format!("[{}]", vec![r#"{"Time":0,"Value":1}"#; n].join(","));
		for (n, ok) in [(2, true), (3, false)] {
			let text = keys(n);
			let (read, over) = with_budget(&limits, text.len(), || {
				serde_json::from_str::<Numbers>(&text).map_err(|e| e.to_string())
			});
			assert_eq!(read.is_ok(), ok, "{n}: {read:?}");
			if !ok {
				assert!(over.unwrap().contains("keypoints in one sequence"));
			}
		}
	}
}
