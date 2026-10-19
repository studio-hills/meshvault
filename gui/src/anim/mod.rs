//! Animated models from schema v1 exports: data types, transform maths,
//! clip sampling and rig solving. Independent of the UI and renderer.

pub mod clip;
pub mod math;
pub mod rig;
pub mod schema;

#[cfg(test)]
mod real_export {
	use super::clip::Clip;
	use super::rig::Rig;
	use super::schema::AnimatedModel;

	/// Checks rigs and clips from a real export without committing it:
	/// `RBX_V1_EXPORT=path/to/export.json cargo test -p meshvault --release -- --ignored real_export --nocapture`
	#[test]
	#[ignore]
	fn rigs_rest_pose_and_binding() {
		#[derive(serde::Deserialize)]
		#[serde(rename_all = "PascalCase")]
		struct Doc {
			models: Vec<AnimatedModel>,
		}
		let Ok(path) = std::env::var("RBX_V1_EXPORT") else { return };
		let started = std::time::Instant::now();
		let doc: Doc = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
		println!("parsed {} models in {:.1?}", doc.models.len(), started.elapsed());
		let (mut tracks, mut matched, mut worst) = (0, 0, (0.0f32, String::new()));
		let mut consistent = 0;
		for model in &doc.models {
			let rig = Rig::build(model);
			let err = rig.rest_error();
			if err < 0.01 {
				consistent += 1;
			}
			if err > worst.0 {
				worst = (err, model.path.clone());
			}
			let (mut t, mut m) = (0, 0);
			let mut unmatched = std::collections::BTreeSet::new();
			for record in &model.animations {
				if let Ok(clip) = Clip::from_record(record) {
					let binding = rig.bind(&clip);
					t += clip.tracks.len();
					m += binding.matched();
					unmatched.extend(binding.unmatched.iter().cloned());
				}
			}
			tracks += t;
			matched += m;
			if err > 0.01 || m < t {
				let sample: Vec<_> = unmatched.iter().take(5).collect();
				println!(
					"{:<40} rest err {:>7.3}  bound {m:>4}/{t:<4} motors {:>3} bones {:>3} unmatched e.g. {sample:?}",
					model.path, err, rig.motors.len(), rig.bones.len()
				);
			}
		}
		println!("largest rest error {:.4} studs ({})", worst.0, worst.1);
		println!("tracks bound {matched}/{tracks}; consistent rigs {consistent}/{}", doc.models.len());
		// characters exported mid-animation have offset snapshots (Motor6D.Transform
		// is not exported), but stored assets should solve back to their CFrames
		assert!(consistent * 4 >= doc.models.len() * 3, "most rigs should solve to their exported pose");
	}
}
