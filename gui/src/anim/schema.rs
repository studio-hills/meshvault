//! Schema version 1 model records: parts, Motor6D joints, bones and
//! animation clips, as written by the exporter (see the implementation
//! guide). The same types are written back into workspaces, so a stored
//! model file is itself a v1 model record.

use serde::{Deserialize, Serialize};

pub type CFrame12 = [f32; 12];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AnimatedModel {
	pub name: String,
	pub path: String,
	pub parts: Vec<ModelPart>,
	pub joints: Vec<MotorJoint>,
	pub bones: Vec<BoneInfo>,
	pub animations: Vec<AnimationClip>,
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
		matches!(self, ClipStatus::KeyframeSequence | ClipStatus::CurveAnimation)
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
