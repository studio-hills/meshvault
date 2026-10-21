//! Model records of schema versions 1 and 2: parts, Motor6D joints, bones
//! and animation clips, as written by the exporter (see the implementation
//! guides). Version 2 adds the fields marked "schema 2"; they are `None`
//! when reading version 1. The same types are written back into
//! workspaces, so a stored model file is itself a model record.

use serde::{Deserialize, Serialize};

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
	pub parts: Vec<ModelPart>,
	pub joints: Vec<MotorJoint>,
	pub bones: Vec<BoneInfo>,
	pub animations: Vec<AnimationClip>,
	/// The model's classic clothing (exporters from 2026-10 on).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub clothing: Option<ClothingInfo>,
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
	/// The part's SurfaceAppearance settings beyond its maps (exporters from
	/// 2026-10 on); absent without one.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub surface_appearance: Option<SurfaceAppearanceInfo>,
	/// The part's Decals and Textures (exporters from 2026-10 on).
	#[serde(skip_serializing_if = "Vec::is_empty")]
	pub decals: Vec<DecalInfo>,
	/// The MaterialVariant the part shows, its own or MaterialService's
	/// override for its material (exporters from 2026-10 on).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub material_variant: Option<MaterialVariantInfo>,
	/// Lights on the part or its attachments (exporters from 2026-10 on).
	#[serde(skip_serializing_if = "Vec::is_empty")]
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
pub struct MotorJoint {
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
	/// The joint's class (exporters from 2026-10 on): "Motor6D",
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
	pub keyframes: Vec<Keyframe>,
	pub curves: Vec<CurveTrack>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Keyframe {
	pub time: f32,
	pub poses: Vec<Pose>,
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
	pub children: Vec<Pose>,
	/// Face control values set at this pose (NumberPoses in its
	/// FaceControls), exporters from 2026-10 on.
	#[serde(skip_serializing_if = "Vec::is_empty")]
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
	#[serde(rename = "X")]
	pub x: Option<Vec<ScalarKey>>,
	#[serde(rename = "Y")]
	pub y: Option<Vec<ScalarKey>>,
	#[serde(rename = "Z")]
	pub z: Option<Vec<ScalarKey>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "Type")]
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
