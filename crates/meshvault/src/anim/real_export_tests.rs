//! Ignored tests that read a real export, which is not committed. See
//! `tools/README.md` for where the inputs come from.

use super::clip::Clip;
use super::rig::Rig;
use crate::studio::export_file;
use crate::test_support::real_export;

/// Checks rigs and clips from a real export without committing it:
/// `MESHVAULT_EXPORT=path/to/export.json cargo test -p meshvault --release -- --ignored real_export_tests --nocapture`
#[test]
#[ignore]
fn rigs_rest_pose_and_binding() {
	let path = real_export();
	let started = std::time::Instant::now();
	let doc = export_file::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
	println!(
		"parsed {} models in {:.1?}",
		doc.models.len(),
		started.elapsed()
	);
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
				model.path,
				err,
				rig.motors.len(),
				rig.bones.len()
			);
		}
	}
	println!("largest rest error {:.4} studs ({})", worst.0, worst.1);
	println!(
		"tracks bound {matched}/{tracks}; consistent rigs {consistent}/{}",
		doc.models.len()
	);
	// characters exported mid-animation by schema 1 have offset snapshots (it
	// does not export Motor6D.Transform; schema 2 does), but stored assets
	// should solve back to their CFrames
	assert!(
		consistent * 4 >= doc.models.len() * 3,
		"most rigs should solve to their exported pose"
	);
}

/// A developer diagnostic, not a regression test: it prints differences
/// and never fails on them. Its input needs a private Studio test place
/// (see `tools/studio/sample_animator.luau`).
///
/// Compares playback with Roblox's own: `MESHVAULT_STUDIO_SAMPLES` holds
/// every Motor6D.Transform and Bone.Transform that Studio's Animator set
/// at fixed clip times, for models of the export in `MESHVAULT_EXPORT`:
/// `{"Models": {name: {"Scale", "Rest": [joints, bones], "Clips": {name:
/// {"Length", "Samples": [{"Time", "Joints": {name: cframe}, "Bones"}]}}}}}`.
/// `MESHVAULT_DETAIL=<joint, bone or part>` also prints that target at
/// every sample.
///
/// Each clip is replayed much as the viewer plays it, but not exactly:
/// every translation is scaled by `Model.Scale` as exported (1 without
/// one), where the viewer ignores a scale that is not finite and positive,
/// and schema 1 bones are not scaled by the mesh they deform as the viewer
/// infers. Bones only meshes have, and face controls, are not compared.
#[test]
#[ignore]
fn matches_studio_samples() {
	use super::math::CFrame;
	use super::rig::Target;
	use std::collections::{BTreeMap, HashMap};

	/// Studio writes an empty table as `[]`, so anything but an object is no pose.
	#[derive(Default)]
	struct Pose(HashMap<String, [f32; 12]>);
	impl<'de> serde::Deserialize<'de> for Pose {
		fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
			let value = serde_json::Value::deserialize(d)?;
			Ok(Pose(serde_json::from_value(value).unwrap_or_default()))
		}
	}
	#[derive(serde::Deserialize)]
	#[serde(rename_all = "PascalCase")]
	struct Sample {
		time: f32,
		joints: Pose,
		bones: Pose,
		/// Every part relative to the root part, by its path in the model.
		#[serde(default)]
		parts: Pose,
	}
	#[derive(serde::Deserialize)]
	#[serde(rename_all = "PascalCase")]
	struct StudioClip {
		length: f32,
		samples: Vec<Sample>,
	}
	#[derive(serde::Deserialize)]
	#[serde(rename_all = "PascalCase")]
	struct StudioModel {
		/// Joints, bones (and parts) before any clip plays.
		rest: Vec<Pose>,
		clips: BTreeMap<String, StudioClip>,
		#[serde(default)]
		root_part: Option<String>,
	}
	#[derive(serde::Deserialize)]
	#[serde(rename_all = "PascalCase")]
	struct Samples {
		models: BTreeMap<String, StudioModel>,
	}
	let export = real_export();
	let samples = std::env::var_os("MESHVAULT_STUDIO_SAMPLES")
		.expect("set MESHVAULT_STUDIO_SAMPLES to a Studio samples file");
	let doc = export_file::parse(&std::fs::read_to_string(export).unwrap()).unwrap();
	let studio: Samples = serde_json::from_slice(&std::fs::read(samples).unwrap()).unwrap();
	let detail = std::env::var("MESHVAULT_DETAIL").ok();

	// the largest rotation difference (matrix entries) and position difference (studs)
	#[derive(Default)]
	struct Worst {
		rot: (f32, String),
		pos: (f32, String),
	}
	impl Worst {
		fn add(&mut self, ours: &CFrame, theirs: &[f32; 12], what: String) {
			let theirs = CFrame::from_components(theirs);
			let rot = (0..3)
				.flat_map(|r| (0..3).map(move |c| (r, c)))
				.map(|(r, c)| (ours.rot[r][c] - theirs.rot[r][c]).abs())
				.fold(0.0, f32::max);
			let pos = (0..3)
				.map(|k| (ours.pos[k] - theirs.pos[k]).powi(2))
				.sum::<f32>()
				.sqrt();
			if rot > self.rot.0 {
				self.rot = (rot, what.clone());
			}
			if pos > self.pos.0 {
				self.pos = (pos, what);
			}
		}
		fn line(&self) -> String {
			format!(
				"rotation {:.4} ({}), position {:.4} ({})",
				self.rot.0, self.rot.1, self.pos.0, self.pos.1
			)
		}
	}

	for (name, theirs) in &studio.models {
		let Some(model) = doc.models.iter().find(|m| &m.name == name) else {
			println!("{name}: not in the export");
			continue;
		};
		let rig = Rig::build(model);
		// the viewer scales animated translations by Model.Scale (schema 2)
		let scale = model.scale.unwrap_or(1.0);
		let mut rest = Worst::default();
		for m in &rig.motors {
			if let Some(t) = theirs.rest.first().and_then(|j| j.0.get(&m.name)) {
				rest.add(&m.captured, t, m.name.clone());
			}
		}
		println!("{name}: captured pose {}", rest.line());
		for (clip_name, clip_samples) in &theirs.clips {
			let Some(record) = model.animations.iter().find(|a| &a.name == clip_name) else {
				println!("  {clip_name}: not in the export");
				continue;
			};
			let clip = match Clip::from_record(record) {
				Ok(clip) => clip,
				Err(why) => {
					println!("  {clip_name}: {why}");
					continue;
				}
			};
			let binding = rig.bind(&clip);
			let mut worst = Worst::default();
			// where the parts end up, relative to the root part: checks the
			// whole solve, welds included
			let mut placed = Worst::default();
			let root = theirs.root_part.as_ref().and_then(|r| {
				rig.parts
					.iter()
					.position(|p| p.path.ends_with(&format!("/{r}")))
			});
			for s in &clip_samples.samples {
				// what the viewer applies: sampled tracks, identity elsewhere
				let mut motors = vec![CFrame::IDENTITY; rig.motors.len()];
				let mut bones = vec![CFrame::IDENTITY; rig.bones.len()];
				let mut scaled = Vec::new();
				for ((_, mut t), target) in clip.sample(s.time).zip(&binding.targets) {
					for v in &mut t.pos {
						*v *= scale;
					}
					scaled.push(t);
					match target {
						Some(Target::Motor(i)) => motors[*i] = t,
						Some(Target::Bone(i)) => bones[*i] = t,
						None => {}
					}
				}
				let show = |what: &str, ours: &CFrame, theirs: &[f32; 12]| {
					if detail.as_deref() == Some(what) {
						println!(
							"    {clip_name} {what} at {:.3}s\n      ours   {:?}\n      Studio {:?}",
							s.time,
							ours.components().map(|v| (v * 1e4).round() / 1e4),
							theirs
						);
					}
				};
				for (i, m) in rig.motors.iter().enumerate() {
					if let Some(t) = s.joints.0.get(&m.name) {
						show(&m.name, &motors[i], t);
						worst.add(&motors[i], t, format!("{} at {:.2}s", m.name, s.time));
					}
				}
				for (i, b) in rig.bones.iter().enumerate() {
					if let Some(t) = s.bones.0.get(&b.name) {
						show(&b.name, &bones[i], t);
						worst.add(&bones[i], t, format!("{} at {:.2}s", b.name, s.time));
					}
				}
				if let Some(root) = root {
					let posed = rig.pose_from(&binding, &scaled);
					let from_root = posed.parts[root].inverse();
					for (path, theirs) in &s.parts.0 {
						let Some(i) = rig
							.parts
							.iter()
							.position(|p| p.path.ends_with(&format!("/{path}")))
						else {
							continue;
						};
						let ours = from_root.mul(&posed.parts[i]);
						show(path, &ours, theirs);
						placed.add(&ours, theirs, format!("{path} at {:.2}s", s.time));
					}
				}
			}
			println!(
				"  {clip_name}: length {:.3}s (Studio {:.3}s), bound {}/{}, {}",
				clip.duration,
				clip_samples.length,
				binding.matched(),
				clip.tracks.len(),
				worst.line()
			);
			if root.is_some() {
				println!("    parts: {}", placed.line());
			}
		}
	}
}
