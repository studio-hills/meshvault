//! A FACS head's face poses.

use super::nul_separated;

/// A FACS head's face poses: how far each face bone moves for each face
/// control, and for each corrective (two or three controls together).
#[derive(Clone, Debug, PartialEq)]
pub struct Facs {
	/// The face bones, by name; they are among the skin's bones.
	pub bones: Vec<String>,
	/// The controls as `FaceControls` names them (JawDrop, LeftEyeClosed, ..).
	pub controls: Vec<String>,
	/// For each pose: the controls whose weights multiply into its weight
	/// (one for a control's own pose, two or three for a corrective).
	pub columns: Vec<Vec<usize>>,
	/// Per face bone and pose, bone-major: position offset in mesh units,
	/// then rotation in degrees about X, Y and Z.
	pub poses: Vec<[f32; 6]>,
}

impl Facs {
	/// Each face bone's offset for the given control weights (0..1), as a
	/// local transform in mesh units: the weighted sum of every pose.
	pub fn bone_offsets(&self, weight: impl Fn(&str) -> f32) -> Vec<(usize, [f32; 6])> {
		let weights: Vec<f32> = self.controls.iter().map(|c| weight(c)).collect();
		let columns = self.columns.len();
		let mut sums = vec![[0.0f32; 6]; self.bones.len()];
		for (j, controls) in self.columns.iter().enumerate() {
			let w: f32 = controls.iter().map(|&c| weights[c]).product();
			if w == 0.0 {
				continue;
			}
			for (b, sum) in sums.iter_mut().enumerate() {
				let pose = &self.poses[b * columns + j];
				for k in 0..6 {
					sum[k] += w * pose[k];
				}
			}
		}
		sums.into_iter()
			.enumerate()
			.filter(|(_, s)| s.iter().any(|v| *v != 0.0))
			.collect()
	}
}

/// The full `FaceControls` name of a FACS control as a mesh abbreviates it
/// (Roblox's FACS pose reference, with the spellings heads use).
fn facs_control_name(code: &str) -> Option<&'static str> {
	let (side, pose) = code.split_once('_')?;
	let pose = pose.to_ascii_uppercase();
	let both = |left: &'static str, right: &'static str| match side {
		"l" | "L" => Some(left),
		"r" | "R" => Some(right),
		_ => None,
	};
	Some(match pose.as_str() {
		"COR" => "Corrugator",
		"CR" => "ChinRaiser",
		"CRUL" | "CRU" => "ChinRaiserUpperLip",
		"ELD" => "EyesLookDown",
		"ELL" => "EyesLookLeft",
		"ELR" => "EyesLookRight",
		"ELU" => "EyesLookUp",
		"FN" | "FU" => "Funneler",
		"FP" => "FlatPucker",
		"JD" => "JawDrop",
		"JL" => "JawLeft",
		"JR" => "JawRight",
		"LLS" => "LowerLipSuck",
		"LP" => "LipPresser",
		"LPT" | "LT" => "LipsTogether",
		"ML" => "MouthLeft",
		"MR" => "MouthRight",
		"PK" => "Pucker",
		"TD" => "TongueDown",
		"TO" => "TongueOut",
		"TU" => "TongueUp",
		"ULS" => "UpperLipSuck",
		"BL" => both("LeftBrowLowerer", "RightBrowLowerer")?,
		"CHP" | "CP" => both("LeftCheekPuff", "RightCheekPuff")?,
		"CHR" => both("LeftCheekRaiser", "RightCheekRaiser")?,
		"DM" | "DP" => both("LeftDimpler", "RightDimpler")?,
		"EC" => both("LeftEyeClosed", "RightEyeClosed")?,
		"EULR" | "EUL" => both("LeftEyeUpperLidRaiser", "RightEyeUpperLidRaiser")?,
		"IBR" => both("LeftInnerBrowRaiser", "RightInnerBrowRaiser")?,
		"LCD" => both("LeftLipCornerDown", "RightLipCornerDown")?,
		"LCP" => both("LeftLipCornerPuller", "RightLipCornerPuller")?,
		"LLD" => both("LeftLowerLipDepressor", "RightLowerLipDepressor")?,
		"LS" => both("LeftLipStretcher", "RightLipStretcher")?,
		"NW" => both("LeftNoseWrinkler", "RightNoseWrinkler")?,
		"OBR" => both("LeftOuterBrowRaiser", "RightOuterBrowRaiser")?,
		"ULR" => both("LeftUpperLipRaiser", "RightUpperLipRaiser")?,
		_ => return None,
	})
}

pub(super) fn facs_of(f: &rbx_mesh::mesh::Facs5) -> Option<Facs> {
	use rbx_mesh::mesh::QuantizedMatrix5;
	let bones = nul_separated(&f.face_bone_names);
	let codes = nul_separated(&f.face_control_names);
	let controls: Vec<String> = codes
		.iter()
		.map(|c| facs_control_name(c).map_or_else(|| c.clone(), String::from))
		.collect();
	let mut columns: Vec<Vec<usize>> = (0..controls.len()).map(|c| vec![c]).collect();
	columns.extend(
		f.two_pose_correctives
			.iter()
			.map(|p| p.0.iter().map(|c| c.0 as usize).collect()),
	);
	columns.extend(
		f.three_pose_correctives
			.iter()
			.map(|p| p.0.iter().map(|c| c.0 as usize).collect()),
	);
	let values = |m: &QuantizedMatrix5| -> Vec<f32> {
		match m {
			QuantizedMatrix5::Raw { matrix, .. } => matrix.clone(),
			QuantizedMatrix5::Quantized {
				lerp0,
				lerp1,
				matrix,
				..
			} => matrix
				.iter()
				// quantised over the whole u16 range
				.map(|&q| lerp0 + (lerp1 - lerp0) * q as f32 / u16::MAX as f32)
				.collect(),
		}
	};
	let q = &f.quantized_transforms;
	let parts = [&q.px, &q.py, &q.pz, &q.rx, &q.ry, &q.rz].map(values);
	let count = bones.len() * columns.len();
	if parts.iter().any(|p| p.len() != count)
		|| columns.iter().flatten().any(|&c| c >= controls.len())
	{
		return None;
	}
	let poses = (0..count)
		.map(|i| [0, 1, 2, 3, 4, 5].map(|k| parts[k][i]))
		.collect();
	Some(Facs {
		bones,
		controls,
		columns,
		poses,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A FACS block is dropped when its pose matrices do not hold one value
	/// per bone and pose, or a corrective names a control that is not there.
	#[test]
	fn facs_rejects_mismatched_poses_and_correctives() {
		use rbx_mesh::mesh::{
			ControlId5, Facs5, QuantizedMatrix5, QuantizedTransforms5, TwoPoseCorrective5,
		};
		let matrix = |n: usize| QuantizedMatrix5::Raw {
			x: n as u32,
			y: 1,
			matrix: vec![0.5; n],
		};
		// one bone and two controls; `poses` is how many values each of the
		// six pose matrices holds (one bone times three poses is right)
		let facs = |poses: [usize; 6], correctives: Vec<[u16; 2]>| Facs5 {
			face_bone_names: b"jaw\0".to_vec(),
			face_control_names: b"c_JD\0c_PK\0".to_vec(),
			quantized_transforms: QuantizedTransforms5 {
				px: matrix(poses[0]),
				py: matrix(poses[1]),
				pz: matrix(poses[2]),
				rx: matrix(poses[3]),
				ry: matrix(poses[4]),
				rz: matrix(poses[5]),
			},
			two_pose_correctives: correctives
				.into_iter()
				.map(|[a, b]| TwoPoseCorrective5([ControlId5(a), ControlId5(b)]))
				.collect(),
			three_pose_correctives: Vec::new(),
		};
		let ok = facs_of(&facs([3; 6], vec![[0, 1]])).expect("a valid block");
		assert_eq!(ok.controls, ["JawDrop", "Pucker"]);
		assert_eq!(ok.columns, [vec![0], vec![1], vec![0, 1]]);
		assert_eq!(ok.poses.len(), 3);
		// one matrix a value short
		assert!(facs_of(&facs([3, 3, 3, 3, 2, 3], vec![[0, 1]])).is_none());
		// a corrective of control 2, of two
		assert!(facs_of(&facs([3; 6], vec![[0, 2]])).is_none());
	}

	/// A FACS pose adds its offsets by its control's weight; a corrective
	/// adds by the product of its controls' weights; the abbreviations heads
	/// use become FaceControls names.
	#[test]
	fn facs_poses_add_by_weight() {
		let facs = Facs {
			bones: vec!["jaw".into(), "lip".into()],
			controls: vec!["JawDrop".into(), "Pucker".into()],
			columns: vec![vec![0], vec![1], vec![0, 1]],
			// bone-major: jaw [JawDrop, Pucker, both], then lip
			poses: vec![
				[0.0, -1.0, 0.0, 0.0, 0.0, 0.0],
				[0.0; 6],
				[0.0, 0.5, 0.0, 0.0, 0.0, 0.0],
				[0.0; 6],
				[0.0, 0.0, -1.0, 0.0, 0.0, 0.0],
				[0.0; 6],
			],
		};
		let offsets = facs.bone_offsets(|c| match c {
			"JawDrop" => 0.5,
			"Pucker" => 1.0,
			_ => 0.0,
		});
		// jaw: -0.5 from JawDrop, +0.25 from the corrective; lip: -1 from Pucker
		assert_eq!(offsets.len(), 2);
		assert!((offsets[0].1[1] + 0.25).abs() < 1e-6);
		assert!((offsets[1].1[2] + 1.0).abs() < 1e-6);
		assert!(facs.bone_offsets(|_| 0.0).is_empty());
		assert_eq!(facs_control_name("c_JD"), Some("JawDrop"));
		assert_eq!(facs_control_name("l_EULR"), Some("LeftEyeUpperLidRaiser"));
		assert_eq!(facs_control_name("r_CHP"), Some("RightCheekPuff"));
		assert_eq!(facs_control_name("x_Unknown"), None);
	}
}
