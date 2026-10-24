//! Model records of schema versions 1 and 2: parts, joints, bones
//! and animation clips, as written by the exporters. Version 2 adds the fields marked "schema 2"; they are `None`
//! when reading version 1. The same types are written back into
//! workspaces, so a stored model file is itself a model record.

use serde::{Deserialize, Serialize};

use super::budget;
pub use super::effect::EffectInfo;

pub type CFrame12 = [f32; 12];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AnimatedModel {
	pub name: String,
	pub path: String,
	/// `Model:GetScale()` (schema 2). Part sizes and CFrames already include
	/// it; animation translations do not, and are scaled by it when played.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub scale: Option<f32>,
	#[serde(deserialize_with = "budget::parts")]
	pub parts: Vec<ModelPart>,
	#[serde(deserialize_with = "budget::joints")]
	pub joints: Vec<JointInfo>,
	#[serde(deserialize_with = "budget::bones")]
	pub bones: Vec<BoneInfo>,
	#[serde(deserialize_with = "budget::clips")]
	pub animations: Vec<AnimationClip>,
	/// The model's classic clothing (schema 2; absent in older exports).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub clothing: Option<ClothingInfo>,
	/// `Model:GetPivot()` as exported (schema 2; absent in older exports).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub pivot: Option<CFrame12>,
	/// The model's PrimaryPart, by path, when it has one.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub primary_part_path: Option<String>,
	/// The model's Attachments, Bones aside (schema 2; absent in older
	/// exports).
	#[serde(
		skip_serializing_if = "Vec::is_empty",
		deserialize_with = "budget::attachments"
	)]
	pub attachments: Vec<AttachmentInfo>,
	/// The model's particle emitters, beams, trails, fire, smoke and
	/// sparkles (schema 2; absent in older exports).
	#[serde(
		skip_serializing_if = "Vec::is_empty",
		deserialize_with = "budget::effects"
	)]
	pub effects: Vec<EffectInfo>,
}

/// An Attachment: a point and orientation on its parent part, such as where
/// an effect, a tool or a constraint attaches.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AttachmentInfo {
	pub name: String,
	pub path: String,
	/// The part (or Bone) it is on.
	pub parent_path: String,
	/// Where it sits on its parent.
	#[serde(rename = "CFrame")]
	pub cframe: CFrame12,
	/// Where it was in the world when exported.
	#[serde(rename = "WorldCFrame", skip_serializing_if = "Option::is_none")]
	pub world_cframe: Option<CFrame12>,
	/// `Attachment.Visible`: Studio draws it even when not selected.
	pub visible: bool,
}

/// A character's Shirt, Pants and ShirtGraphic: template images in the
/// classic 585 x 559 layout, each tinted by its Color3.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ClothingInfo {
	pub shirt_template: Option<String>,
	pub shirt_color: Option<[f32; 3]>,
	pub pants_template: Option<String>,
	pub pants_color: Option<[f32; 3]>,
	/// A T-shirt: one image over the front of the torso.
	pub graphic: Option<String>,
	pub graphic_color: Option<[f32; 3]>,
}

impl ClothingInfo {
	/// The clothing's images, as content ids.
	pub fn images(&self) -> impl Iterator<Item = &str> {
		[&self.shirt_template, &self.pants_template, &self.graphic]
			.into_iter()
			.flatten()
			.map(String::as_str)
	}
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ModelPart {
	pub path: String,
	pub name: String,
	/// `Meshes[].Id`, or 0 when the part has no file mesh.
	pub mesh_ref: u32,
	/// Initial world transform.
	#[serde(rename = "CFrame")]
	pub cframe: CFrame12,
	pub size: [f32; 3],
	/// `BasePart:GetPivot()` and `PivotOffset`, the pivot in the part's own
	/// space (schema 2; absent in older exports).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub pivot: Option<CFrame12>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub pivot_offset: Option<CFrame12>,
	pub transparency: f32,
	/// e.g. "Part", "MeshPart", "WedgePart" (schema 2).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub class_name: Option<String>,
	/// "Block", "Ball", "Cylinder", "Wedge", "CornerWedge", "Truss", or ""
	/// for classes without a shape (schema 2).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub shape: Option<String>,
	/// `BasePart.Color`, each channel 0..1 (schema 2).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub color: Option<[f32; 3]>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub brick_color: Option<String>,
	/// `Enum.Material` name (schema 2).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub material: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub reflectance: Option<f32>,
	/// Absent when the part has no SpecialMesh (schema 2).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub special_mesh: Option<SpecialMeshInfo>,
	/// The part's SurfaceAppearance settings beyond its maps (schema 2);
	/// absent without one, and in older exports.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub surface_appearance: Option<SurfaceAppearanceInfo>,
	/// The part's Decals and Textures (schema 2; absent in older exports).
	#[serde(
		skip_serializing_if = "Vec::is_empty",
		deserialize_with = "budget::decals"
	)]
	pub decals: Vec<DecalInfo>,
	/// The MaterialVariant the part shows, its own or MaterialService's
	/// override for its material (schema 2; absent in older exports).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub material_variant: Option<MaterialVariantInfo>,
	/// Lights on the part or its attachments (schema 2; absent in older
	/// exports).
	#[serde(
		skip_serializing_if = "Vec::is_empty",
		deserialize_with = "budget::lights"
	)]
	pub lights: Vec<LightInfo>,
}

/// A PointLight, SpotLight or SurfaceLight.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct LightInfo {
	pub class_name: String,
	pub enabled: bool,
	pub brightness: f32,
	/// Each channel 0..1.
	pub color: [f32; 3],
	pub range: f32,
	pub shadows: bool,
	/// Spot and surface lights: the cone's angle in degrees, and the face
	/// (`Enum.NormalId` name) it shines from.
	pub angle: Option<f32>,
	pub face: Option<String>,
	/// Where the light sits on the part, when it is on an attachment.
	#[serde(rename = "CFrame")]
	pub cframe: Option<CFrame12>,
}

/// A MaterialVariant: maps tiled over the part's surfaces.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct MaterialVariantInfo {
	pub name: String,
	/// The size of one tile, in studs.
	pub studs_per_tile: f32,
	/// Content ids; "" for a map the variant does not have.
	pub color_map: String,
	pub normal_map: String,
	pub roughness_map: String,
	pub metalness_map: String,
}

impl MaterialVariantInfo {
	/// The variant's maps, by the slot each fills.
	pub fn maps(&self) -> [(crate::scene::maps::Slot, &str); 4] {
		use crate::scene::maps::Slot;
		[
			(Slot::Base, &self.color_map),
			(Slot::Normal, &self.normal_map),
			(Slot::Roughness, &self.roughness_map),
			(Slot::Metalness, &self.metalness_map),
		]
	}
}

/// A Decal, or a Texture (a Decal that tiles), on one face of a part.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DecalInfo {
	/// "Decal" or "Texture".
	pub class_name: String,
	/// Content id of the image (or of a Decal asset, resolved on download).
	pub texture: String,
	/// `Enum.NormalId` name: "Front", "Back", "Left", "Right", "Top", "Bottom".
	pub face: String,
	/// `Color3`, each channel 0..1: tints the image.
	pub color: [f32; 3],
	pub transparency: f32,
	#[serde(rename = "ZIndex")]
	pub z_index: i32,
	/// Textures only: the size of one tile, and how far the tiles are moved.
	pub studs_per_tile_u: Option<f32>,
	pub studs_per_tile_v: Option<f32>,
	pub offset_studs_u: Option<f32>,
	pub offset_studs_v: Option<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SurfaceAppearanceInfo {
	/// `SurfaceAppearance.Color`, each channel 0..1: tints the colour map.
	pub color: [f32; 3],
	/// `Enum.AlphaMode` name: "Overlay", "Transparency", "TintMask" or "Opaque".
	pub alpha_mode: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SpecialMeshInfo {
	/// `Enum.MeshType` name, e.g. "FileMesh", "Head", "Sphere", "Brick".
	pub mesh_type: String,
	pub scale: [f32; 3],
	pub offset: [f32; 3],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct JointInfo {
	pub name: String,
	pub path: String,
	/// Path of the BasePart, or "".
	pub part0: String,
	pub part1: String,
	#[serde(rename = "C0")]
	pub c0: CFrame12,
	#[serde(rename = "C1")]
	pub c1: CFrame12,
	/// `Motor6D.Transform` when exported (schema 2): the pose the joint was
	/// in at that moment, not part of any clip.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub transform: Option<CFrame12>,
	/// The joint's class (current schema 1 and 2 exporters; absent in older
	/// exports): "Motor6D",
	/// "AnimationConstraint", or a weld ("Weld", "WeldConstraint",
	/// "RigidConstraint", ...) that only holds its parts together. Absent
	/// means a Motor6D.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub kind: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct BoneInfo {
	pub name: String,
	pub path: String,
	pub parent_path: String,
	/// Local transform relative to the parent part or bone.
	#[serde(rename = "CFrame")]
	pub cframe: CFrame12,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipStatus {
	KeyframeSequence,
	CurveAnimation,
	#[default]
	Unavailable,
	CurveAnimationExportFailed,
	UnsupportedClipType,
	/// A status this version of the app does not know.
	#[serde(other)]
	Unknown,
}

impl ClipStatus {
	pub fn is_playable(self) -> bool {
		matches!(
			self,
			ClipStatus::KeyframeSequence | ClipStatus::CurveAnimation
		)
	}

	/// Why a clip with this status cannot play, or `None` when it can.
	pub fn unplayable_reason(self) -> Option<&'static str> {
		match self {
			ClipStatus::KeyframeSequence | ClipStatus::CurveAnimation => None,
			ClipStatus::Unavailable => Some("The clip could not be loaded when it was exported."),
			ClipStatus::CurveAnimationExportFailed => Some("The curve animation failed to export."),
			ClipStatus::UnsupportedClipType | ClipStatus::Unknown => {
				Some("This clip type is not supported.")
			}
		}
	}

	pub fn label(self) -> &'static str {
		match self {
			ClipStatus::KeyframeSequence => "Keyframes",
			ClipStatus::CurveAnimation => "Curves",
			ClipStatus::Unavailable => "Unavailable",
			ClipStatus::CurveAnimationExportFailed => "Export failed",
			ClipStatus::UnsupportedClipType => "Unsupported",
			ClipStatus::Unknown => "Unknown",
		}
	}
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AnimationClip {
	pub name: String,
	pub animation_id: String,
	pub status: ClipStatus,
	/// Seconds; the last exported key time.
	pub duration: f32,
	#[serde(rename = "Loop")]
	pub looped: bool,
	pub priority: String,
	#[serde(deserialize_with = "budget::keyframes")]
	pub keyframes: Vec<Keyframe>,
	#[serde(deserialize_with = "budget::curves")]
	pub curves: Vec<CurveTrack>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Keyframe {
	pub time: f32,
	#[serde(deserialize_with = "budget::poses")]
	pub poses: Vec<Pose>,
	#[serde(deserialize_with = "budget::markers")]
	pub markers: Vec<Marker>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Marker {
	pub name: String,
	pub value: String,
}

/// Roblox `PoseEasingStyle` (plus the general easing styles, in case a
/// future exporter writes them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EasingStyle {
	#[default]
	Linear,
	Constant,
	Elastic,
	Cubic,
	Bounce,
	CubicV2,
	Sine,
	Quad,
	Quart,
	Quint,
	Exponential,
	Circular,
	Back,
	#[serde(other)]
	Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EasingDirection {
	#[default]
	In,
	Out,
	InOut,
	#[serde(other)]
	Other,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Pose {
	/// The rig part or bone this pose drives.
	pub name: String,
	pub transform: CFrame12,
	/// 0 marks a placeholder pose that only carries children.
	pub weight: f32,
	pub easing_style: EasingStyle,
	pub easing_direction: EasingDirection,
	#[serde(deserialize_with = "budget::poses")]
	pub children: Vec<Pose>,
	/// Face control values set at this pose (NumberPoses in its
	/// FaceControls), from the current schema 1 and 2 exporters; absent in
	/// older exports.
	#[serde(
		skip_serializing_if = "Vec::is_empty",
		deserialize_with = "budget::face_controls"
	)]
	pub face_controls: Vec<FaceControlPose>,
}

/// A NumberPose for one face control (JawDrop, LeftEyeClosed, ..).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FaceControlPose {
	pub name: String,
	pub value: f32,
	pub easing_style: EasingStyle,
	pub easing_direction: EasingDirection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CurveTrack {
	/// Folder path inside the clip; the last component names the target.
	#[serde(default)]
	pub path: String,
	/// Usually "Position" or "Rotation".
	#[serde(default)]
	pub name: String,
	pub curve: CurveData,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ScalarKey {
	pub time: f32,
	pub value: f32,
	pub interpolation: String,
	pub left_tangent: f32,
	pub right_tangent: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RotationKey {
	pub time: f32,
	pub value: CFrame12,
	pub interpolation: String,
	pub left_tangent: f32,
	pub right_tangent: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Channels {
	#[serde(rename = "X", deserialize_with = "budget::channel_keys")]
	pub x: Option<Vec<ScalarKey>>,
	#[serde(rename = "Y", deserialize_with = "budget::channel_keys")]
	pub y: Option<Vec<ScalarKey>>,
	#[serde(rename = "Z", deserialize_with = "budget::channel_keys")]
	pub z: Option<Vec<ScalarKey>>,
}

/// A curve, written with its `Type` beside its fields. It is read through
/// [`RawCurve`]: serde would otherwise buffer the whole curve, keys and all,
/// before it knew the type, outside any budget.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "Type", try_from = "RawCurve")]
pub enum CurveData {
	FloatCurve {
		#[serde(rename = "Keys", default)]
		keys: Vec<ScalarKey>,
	},
	RotationCurve {
		#[serde(rename = "Keys", default)]
		keys: Vec<RotationKey>,
	},
	Vector3Curve {
		#[serde(rename = "Channels", default)]
		channels: Channels,
	},
	EulerRotationCurve {
		#[serde(rename = "RotationOrder", default)]
		rotation_order: String,
		#[serde(rename = "Channels", default)]
		channels: Channels,
	},
	/// A curve type this version of the app does not know.
	#[serde(other)]
	Unknown,
}

/// Every field any curve type has, read in one pass.
#[derive(Deserialize)]
struct RawCurve {
	#[serde(rename = "Type")]
	kind: String,
	#[serde(rename = "Keys", default, deserialize_with = "budget::keys")]
	keys: Vec<RawKey>,
	#[serde(rename = "Channels", default)]
	channels: Channels,
	#[serde(rename = "RotationOrder", default)]
	rotation_order: String,
}

/// A float or a rotation key, by its `Value`.
#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct RawKey {
	time: f32,
	value: Option<KeyValue>,
	interpolation: String,
	left_tangent: f32,
	right_tangent: f32,
}

/// A key's `Value`: a float key's number or a rotation key's CFrame, or
/// something else, which only a curve type this app does not know may have.
enum KeyValue {
	Float(f32),
	Rotation(CFrame12),
	Other,
}

impl<'de> Deserialize<'de> for KeyValue {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
		struct V;
		impl<'de> Visitor<'de> for V {
			type Value = KeyValue;
			fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
				f.write_str("a key value")
			}
			fn visit_f64<E>(self, v: f64) -> Result<KeyValue, E> {
				Ok(KeyValue::Float(v as f32))
			}
			fn visit_i64<E>(self, v: i64) -> Result<KeyValue, E> {
				Ok(KeyValue::Float(v as f32))
			}
			fn visit_u64<E>(self, v: u64) -> Result<KeyValue, E> {
				Ok(KeyValue::Float(v as f32))
			}
			fn visit_bool<E>(self, _: bool) -> Result<KeyValue, E> {
				Ok(KeyValue::Other)
			}
			fn visit_str<E>(self, _: &str) -> Result<KeyValue, E> {
				Ok(KeyValue::Other)
			}
			fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<KeyValue, A::Error> {
				while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
				Ok(KeyValue::Other)
			}
			/// 12 numbers are a CFrame; anything else in a list is skipped
			/// without being kept.
			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<KeyValue, A::Error> {
				let mut cf = [0.0; 12];
				let mut count = 0;
				let mut numbers = true;
				while let Some(item) = seq.next_element::<Number>()? {
					match item {
						Number(Some(v)) if count < 12 => cf[count] = v,
						_ => numbers = false,
					}
					count += 1;
				}
				Ok(if numbers && count == 12 {
					KeyValue::Rotation(cf)
				} else {
					KeyValue::Other
				})
			}
		}
		d.deserialize_any(V)
	}
}

/// A number in a key's `Value` list, or `None` for anything else (skipped).
struct Number(Option<f32>);

impl<'de> Deserialize<'de> for Number {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		Ok(match KeyValue::deserialize(d)? {
			KeyValue::Float(v) => Number(Some(v)),
			// anything nested was walked without being kept
			KeyValue::Rotation(_) | KeyValue::Other => Number(None),
		})
	}
}

impl TryFrom<RawCurve> for CurveData {
	type Error = String;

	fn try_from(raw: RawCurve) -> Result<Self, String> {
		let scalar = |k: RawKey| {
			let value = match k.value {
				Some(KeyValue::Float(v)) => v,
				None => 0.0,
				Some(_) => return Err("a FloatCurve key's Value is not a number".to_owned()),
			};
			Ok(ScalarKey {
				time: k.time,
				value,
				interpolation: k.interpolation,
				left_tangent: k.left_tangent,
				right_tangent: k.right_tangent,
			})
		};
		let rotation = |k: RawKey| {
			let value = match k.value {
				Some(KeyValue::Rotation(cf)) => cf,
				None => CFrame12::default(),
				Some(_) => {
					return Err(
						"a RotationCurve key's Value is not a CFrame of 12 numbers".to_owned()
					);
				}
			};
			Ok(RotationKey {
				time: k.time,
				value,
				interpolation: k.interpolation,
				left_tangent: k.left_tangent,
				right_tangent: k.right_tangent,
			})
		};
		Ok(match raw.kind.as_str() {
			"FloatCurve" => CurveData::FloatCurve {
				keys: raw.keys.into_iter().map(scalar).collect::<Result<_, _>>()?,
			},
			"RotationCurve" => CurveData::RotationCurve {
				keys: raw
					.keys
					.into_iter()
					.map(rotation)
					.collect::<Result<_, _>>()?,
			},
			"Vector3Curve" => CurveData::Vector3Curve {
				channels: raw.channels,
			},
			"EulerRotationCurve" => CurveData::EulerRotationCurve {
				rotation_order: raw.rotation_order,
				channels: raw.channels,
			},
			_ => CurveData::Unknown,
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Curves are read in one pass by their `Type`, wherever it is, and
	/// written back the same way.
	#[test]
	fn curves_read_by_their_type() {
		let identity = [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1];
		let read = |v: serde_json::Value| serde_json::from_value::<CurveData>(v);
		let float =
			read(serde_json::json!({"Keys": [{"Time": 1, "Value": 2}], "Type": "FloatCurve"}))
				.unwrap();
		let CurveData::FloatCurve { keys } = &float else {
			panic!("{float:?}")
		};
		assert_eq!((keys[0].time, keys[0].value), (1.0, 2.0));
		let rotation =
			read(serde_json::json!({"Type": "RotationCurve", "Keys": [{"Value": identity}, {}]}))
				.unwrap();
		let CurveData::RotationCurve { keys } = &rotation else {
			panic!("{rotation:?}")
		};
		assert_eq!(keys[0].value[3], 1.0);
		assert_eq!(keys[1].value, [0.0; 12]);
		let euler = read(
			serde_json::json!({"Type": "EulerRotationCurve", "RotationOrder": "XYZ",
			"Channels": {"X": [{"Value": 1}], "Y": null}}),
		)
		.unwrap();
		let CurveData::EulerRotationCurve {
			rotation_order,
			channels,
		} = &euler
		else {
			panic!("{euler:?}")
		};
		assert_eq!(rotation_order, "XYZ");
		assert_eq!(channels.x.as_ref().unwrap()[0].value, 1.0);
		assert!(channels.y.is_none() && channels.z.is_none());
		assert!(matches!(
			read(serde_json::json!({"Type": "Vector3Curve"})).unwrap(),
			CurveData::Vector3Curve { .. }
		));
		assert!(matches!(
			read(serde_json::json!({"Type": "Future", "Keys": [{"Value": "x"}]})).unwrap(),
			CurveData::Unknown
		));
		// a value of the wrong kind, a short CFrame, or no type
		assert!(
			read(serde_json::json!({"Type": "FloatCurve", "Keys": [{"Value": identity}]})).is_err()
		);
		assert!(
			read(serde_json::json!({"Type": "RotationCurve", "Keys": [{"Value": [1, 2]}]}))
				.is_err()
		);
		assert!(read(serde_json::json!({"Keys": []})).is_err());
		// written with the type beside the fields, and read back
		let out = serde_json::to_value(&rotation).unwrap();
		assert_eq!(out["Type"], "RotationCurve");
		assert!(matches!(read(out).unwrap(), CurveData::RotationCurve { keys } if keys.len() == 2));
	}

	/// Stored models write their keys back out, so an interpolation name
	/// must come back as it was read, even one the player does not know, and
	/// a missing one as "".
	#[test]
	fn key_interpolations_are_written_back_as_read() {
		let identity = [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1];
		for (read, written) in [
			(Some("Linear"), "Linear"),
			(Some("Constant"), "Constant"),
			(Some("Cubic"), "Cubic"),
			(Some("Bezier"), "Bezier"),
			(None, ""),
		] {
			let mut scalar = serde_json::json!({"Time": 1, "Value": 2});
			let mut rotation = serde_json::json!({"Time": 1, "Value": identity});
			if let Some(name) = read {
				scalar["Interpolation"] = name.into();
				rotation["Interpolation"] = name.into();
			}
			let scalar: ScalarKey = serde_json::from_value(scalar).unwrap();
			let out = serde_json::to_value(&scalar).unwrap();
			assert_eq!(out["Interpolation"], written, "{read:?}");
			let rotation: RotationKey = serde_json::from_value(rotation).unwrap();
			let out = serde_json::to_value(&rotation).unwrap();
			assert_eq!(out["Interpolation"], written, "{read:?}");
		}
	}
}
