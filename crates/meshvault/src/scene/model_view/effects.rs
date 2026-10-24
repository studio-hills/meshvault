//! A model's effects, placed on its parts, Attachments and Bones: the
//! anchors they emit from, followed as the model is posed.

use std::collections::HashMap;

use super::ModelView;
use super::decals::ImageCache;
use super::markers::{AttachmentView, On};
use crate::anim::math::CFrame;
use crate::anim::rig::Rig;
use crate::anim::schema::AnimatedModel;
use crate::scene::effects::{Anchor, EffectFrame, Effects, Unplaced, WARM_UP};

/// What an effect can be in, and where it sits on what moves it.
#[derive(Clone, Debug)]
pub(super) struct AnchorSource {
	on: On,
	local: CFrame,
	/// A part's size; zero for a point.
	extent: [f32; 3],
}

/// The anchors of every part, Attachment and Bone, and the model's effects
/// on them, with what could not be placed.
pub(super) fn effects(
	model: &AnimatedModel,
	rig: &Rig,
	attachments: &[AttachmentView],
	images: &mut ImageCache,
) -> (Vec<AnchorSource>, Effects, Unplaced) {
	let mut sources = Vec::new();
	let mut by_path: HashMap<&str, usize> = HashMap::new();
	for (i, p) in rig.parts.iter().enumerate() {
		by_path.insert(&p.path, sources.len());
		sources.push(AnchorSource {
			on: On::Part(i),
			local: CFrame::IDENTITY,
			extent: p.size,
		});
	}
	for (i, b) in rig.bones.iter().enumerate() {
		by_path.insert(&b.path, sources.len());
		sources.push(AnchorSource {
			on: On::Bone(i),
			local: CFrame::IDENTITY,
			extent: [0.0; 3],
		});
	}
	for a in attachments {
		by_path.insert(&a.path, sources.len());
		sources.push(AnchorSource {
			on: a.on,
			local: a.local,
			extent: [0.0; 3],
		});
	}
	let (effects, unplaced) = Effects::new(
		&model.effects,
		&|path| by_path.get(path).copied(),
		&mut |id| images.get(id).ok().map(|t| t.rgba.clone()),
	);
	(sources, effects, unplaced)
}

impl ModelView {
	/// Where every anchor is with the model posed now.
	fn anchors(&mut self) -> Vec<Anchor> {
		self.update_pose();
		self.anchor_sources
			.iter()
			.map(|s| Anchor {
				world: match s.on {
					On::Part(p) => self.posed.parts[p].mul(&s.local),
					On::Bone(b) => self.posed.bones[b].mul(&s.local),
					On::World => s.local,
				},
				extent: s.extent,
			})
			.collect()
	}

	/// How many effects the model has, on or off.
	pub fn effect_count(&self) -> usize {
		self.effects.count()
	}

	/// Runs the effects on by `dt` seconds from where the model is posed
	/// now. Returns whether they need drawing again.
	pub fn step_effects(&mut self, dt: f32) -> bool {
		if !self.effects.any_enabled() {
			return false;
		}
		let anchors = self.anchors();
		self.effects.step(dt, &anchors);
		true
	}

	/// Runs the effects for [`WARM_UP`] seconds, so they are shown full, as
	/// Studio shows them, rather than just starting.
	pub(super) fn warm_up_effects(&mut self) {
		self.step_effects(WARM_UP);
	}

	/// What the effects draw now.
	pub fn effect_frame(&mut self) -> EffectFrame {
		if !self.effects.any_enabled() {
			return EffectFrame::default();
		}
		let anchors = self.anchors();
		self.effects.frame(&anchors)
	}
}
