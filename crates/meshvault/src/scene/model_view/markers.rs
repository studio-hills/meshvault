//! A model's Attachments and its pivot: points on the model that follow
//! the part (or Bone) they are on as it is posed, for the attachments
//! overlay.

use super::ModelView;
use crate::anim::math::CFrame;
use crate::anim::rig::Rig;
use crate::anim::schema::AnimatedModel;

/// What an attachment or the pivot moves with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum On {
	Part(usize),
	Bone(usize),
	/// Nothing: it stays where it was exported.
	World,
}

/// An Attachment, placed on what it is on.
#[derive(Clone, Debug)]
pub(super) struct AttachmentView {
	name: String,
	/// Its full path, which effects name it by.
	pub(super) path: String,
	/// The name of the part or Bone it is on.
	on_name: String,
	pub(super) on: On,
	/// Where it sits on what it is on (or in the world, for [`On::World`]).
	pub(super) local: CFrame,
	visible: bool,
}

/// The model's pivot, kept where it is relative to the part it moves with.
#[derive(Clone, Debug)]
pub(super) struct PivotView {
	on: On,
	offset: CFrame,
	/// The PrimaryPart's name, when the model has one.
	primary: Option<String>,
}

/// The model's attachments, each on the part or Bone of its parent path.
/// One whose parent is neither stays where it was exported, if that was
/// recorded, and is left out otherwise.
pub(super) fn attachments(model: &AnimatedModel, rig: &Rig) -> Vec<AttachmentView> {
	model
		.attachments
		.iter()
		.filter_map(|a| {
			let local = CFrame::from_components(&a.cframe);
			let (on, on_name, local) =
				if let Some(p) = rig.parts.iter().position(|p| p.path == a.parent_path) {
					(On::Part(p), rig.parts[p].name.clone(), local)
				} else if let Some(b) = rig.bones.iter().position(|b| b.path == a.parent_path) {
					(On::Bone(b), rig.bones[b].name.clone(), local)
				} else {
					let world = CFrame::from_components(a.world_cframe.as_ref()?);
					(On::World, String::new(), world)
				};
			Some(AttachmentView {
				name: a.name.clone(),
				path: a.path.clone(),
				on_name,
				on,
				local,
				visible: a.visible,
			})
		})
		.collect()
}

/// The model's pivot, moving with its PrimaryPart, else with the root of
/// its first joint tree (as the model moves when animated), else fixed.
pub(super) fn pivot(model: &AnimatedModel, rig: &Rig) -> Option<PivotView> {
	let pivot = CFrame::from_components(model.pivot.as_ref()?);
	let primary = model
		.primary_part_path
		.as_deref()
		.and_then(|path| rig.parts.iter().position(|p| p.path == path));
	let part = primary.or_else(|| rig.part_links().iter().position(Option::is_none));
	// the rig keeps the parts in the record's order
	let (on, offset) = match part.and_then(|p| Some((p, model.parts.get(p)?))) {
		Some((p, record)) => (
			On::Part(p),
			CFrame::from_components(&record.cframe)
				.inverse()
				.mul(&pivot),
		),
		None => (On::World, pivot),
	};
	Some(PivotView {
		on,
		offset,
		primary: primary.map(|p| rig.parts[p].name.clone()),
	})
}

/// Each part's pivot in its own space: its PivotOffset, or its pivot
/// relative to it; `None` for exports without either.
pub(super) fn part_pivots(model: &AnimatedModel) -> Vec<Option<CFrame>> {
	model
		.parts
		.iter()
		.map(|p| match (&p.pivot_offset, &p.pivot) {
			(Some(offset), _) => Some(CFrame::from_components(offset)),
			(None, Some(pivot)) => Some(
				CFrame::from_components(&p.cframe)
					.inverse()
					.mul(&CFrame::from_components(pivot)),
			),
			(None, None) => None,
		})
		.collect()
}

/// What the attachments overlay draws, in world space.
#[derive(Default)]
pub struct Points {
	pub attachments: Vec<Point>,
	pub pivot: Option<Point>,
	/// The pivot of the part highlighted in the parts list.
	pub part_pivot: Option<Point>,
}

/// An attachment, or the model's pivot, as a node of the exported model:
/// on the node of its part or Bone (in [`ModelView::export_nodes`]'s
/// order), or on the model's root with `None`.
pub struct ExportAttachment {
	pub name: String,
	pub node: Option<usize>,
	/// Where it sits on that node.
	pub local: CFrame,
}

/// An attachment or the pivot, where it is now.
pub struct Point {
	pub at: CFrame,
	/// e.g. "Muzzle on Barrel", or "Pivot (PrimaryPart Root)".
	pub label: String,
	/// `Attachment.Visible` (always set for the pivot).
	pub visible: bool,
}

impl ModelView {
	/// The model's attachments and pivot where it is posed now.
	pub fn points(&mut self) -> Points {
		self.update_pose();
		let place = |on: On, local: &CFrame| match on {
			On::Part(p) => self.posed.parts[p].mul(local),
			On::Bone(b) => self.posed.bones[b].mul(local),
			On::World => *local,
		};
		Points {
			attachments: self
				.attachments
				.iter()
				.map(|a| Point {
					at: place(a.on, &a.local),
					label: if a.on_name.is_empty() {
						a.name.clone()
					} else {
						format!("{} on {}", a.name, a.on_name)
					},
					visible: a.visible,
				})
				.collect(),
			pivot: self.pivot.as_ref().map(|p| Point {
				at: place(p.on, &p.offset),
				label: match &p.primary {
					Some(name) => format!("Pivot (PrimaryPart {name})"),
					None => "Pivot".into(),
				},
				visible: true,
			}),
			part_pivot: self.highlight.and_then(|i| {
				let offset = self.part_pivots.get(i)?.as_ref()?;
				Some(Point {
					at: self.posed.parts[i].mul(offset),
					label: format!("Pivot of {}", self.rig.parts[i].name),
					visible: true,
				})
			}),
		}
	}

	/// The attachments and pivot as nodes of the exported model, each where
	/// it sits on its part or Bone, so it follows it in every clip.
	pub fn export_attachments(&self) -> Vec<ExportAttachment> {
		let parts = self.rig.parts.len();
		let node = |on: On| match on {
			On::Part(p) => Some(p),
			On::Bone(b) => Some(parts + b),
			On::World => None,
		};
		let attachments = self.attachments.iter().map(|a| ExportAttachment {
			name: a.name.clone(),
			node: node(a.on),
			local: a.local,
		});
		let pivot = self.pivot.iter().map(|p| ExportAttachment {
			name: "Pivot".into(),
			node: node(p.on),
			local: p.offset,
		});
		attachments.chain(pivot).collect()
	}

	/// How many attachments the model has, for the inspector.
	pub fn attachment_count(&self) -> usize {
		self.attachments.len()
	}

	/// Whether the export recorded the model's pivot (schema 2).
	pub fn has_pivot(&self) -> bool {
		self.pivot.is_some()
	}

	/// The PrimaryPart's name, when the model has one.
	pub fn primary_part(&self) -> Option<&str> {
		self.pivot.as_ref()?.primary.as_deref()
	}
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;

	use eframe::egui;

	use super::super::fixtures::{cf, placed_part};
	use super::*;
	use crate::anim::schema::AttachmentInfo;

	fn attachment(
		name: &str,
		parent: &str,
		at: [f32; 3],
		world: Option<[f32; 3]>,
	) -> AttachmentInfo {
		AttachmentInfo {
			name: name.into(),
			path: format!("{parent}/{name}"),
			parent_path: parent.into(),
			cframe: cf(at),
			world_cframe: world.map(cf),
			visible: true,
		}
	}

	fn view() -> ModelView {
		let model = AnimatedModel {
			name: "Turret".into(),
			path: "Workspace/Dummy".into(),
			parts: vec![
				placed_part(Some("Part"), Some("Block"), "Base", [0.0, 5.0, 0.0]),
				placed_part(Some("Part"), Some("Block"), "Barrel", [0.0, 6.0, -1.0]),
			],
			attachments: vec![
				attachment("Muzzle", "Workspace/Dummy/Barrel", [0.0, 0.0, -2.0], None),
				// on nothing in the model: where it was, or left out
				attachment(
					"Loose",
					"Workspace/Elsewhere",
					[9.0; 3],
					Some([1.0, 2.0, 3.0]),
				),
				attachment("Lost", "Workspace/Elsewhere", [9.0; 3], None),
			],
			pivot: Some(cf([0.0, 4.0, 0.0])),
			primary_part_path: Some("Workspace/Dummy/Base".into()),
			..Default::default()
		};
		let ctx = egui::Context::default();
		ModelView::load(&ctx, 0, &model, &HashMap::new(), &HashMap::new())
	}

	fn near(a: [f32; 3], b: [f32; 3]) -> bool {
		(0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
	}

	/// Attachments sit on their part, the pivot is kept relative to the
	/// PrimaryPart, and both follow their parts as they move.
	#[test]
	fn attachments_and_the_pivot_follow_their_parts() {
		let mut view = view();
		assert_eq!(view.attachment_count(), 2);
		assert_eq!(view.primary_part(), Some("Base"));
		let points = view.points();
		let labels: Vec<&str> = points
			.attachments
			.iter()
			.map(|a| a.label.as_str())
			.collect();
		assert_eq!(labels, ["Muzzle on Barrel", "Loose"]);
		let rest = |p: &Points| {
			(
				p.attachments[0].at.pos,
				p.attachments[1].at.pos,
				p.pivot.as_ref().unwrap().at.pos,
			)
		};
		let (muzzle, loose, pivot) = rest(&points);
		let barrel = view.posed.parts[1].pos;
		assert!(
			near(muzzle, [barrel[0], barrel[1], barrel[2] - 2.0]),
			"{muzzle:?}"
		);
		assert!(near(loose, [1.0, 2.0, 3.0]));
		assert!(near(pivot, [0.0, 4.0, 0.0]), "{pivot:?}");
		assert_eq!(points.pivot.unwrap().label, "Pivot (PrimaryPart Base)");

		// lift the whole model 10 studs: the muzzle and the pivot come with
		// it, the loose attachment stays
		let up = CFrame::from_position([0.0, 10.0, 0.0]);
		for part in view.posed.parts.iter_mut() {
			*part = up.mul(part);
		}
		let (moved_muzzle, moved_loose, moved_pivot) = rest(&view.points());
		assert!(near(moved_muzzle, [muzzle[0], muzzle[1] + 10.0, muzzle[2]]));
		assert!(near(moved_pivot, [0.0, 14.0, 0.0]), "{moved_pivot:?}");
		assert!(near(moved_loose, loose));
	}

	/// The part highlighted in the parts list shows its pivot: its
	/// PivotOffset on it, or its exported pivot relative to it; none
	/// without a highlight or without either.
	#[test]
	fn the_highlighted_part_shows_its_pivot() {
		let part = |name: &str, at: [f32; 3]| placed_part(Some("Part"), Some("Block"), name, at);
		let model = AnimatedModel {
			parts: vec![
				crate::anim::schema::ModelPart {
					pivot_offset: Some(cf([0.0, -0.5, 0.0])),
					..part("Base", [0.0, 5.0, 0.0])
				},
				crate::anim::schema::ModelPart {
					pivot: Some(cf([2.0, 7.0, 0.0])),
					..part("Lid", [0.0, 6.0, 0.0])
				},
				part("Old", [0.0, 8.0, 0.0]),
			],
			..Default::default()
		};
		let ctx = egui::Context::default();
		let mut view = ModelView::load(&ctx, 0, &model, &HashMap::new(), &HashMap::new());
		assert!(view.points().part_pivot.is_none());
		let pivot_of = |view: &mut ModelView, i: usize| {
			view.highlight = Some(i);
			view.points().part_pivot.map(|p| (p.at.pos, p.label))
		};
		let (at, label) = pivot_of(&mut view, 0).unwrap();
		let base = view.posed.parts[0].pos;
		assert!(near(at, [base[0], base[1] - 0.5, base[2]]), "{at:?}");
		assert_eq!(label, "Pivot of Base");
		let lid = view.posed.parts[1].pos;
		let (at, _) = pivot_of(&mut view, 1).unwrap();
		assert!(near(at, [lid[0] + 2.0, lid[1] + 1.0, lid[2]]), "{at:?}");
		assert!(pivot_of(&mut view, 2).is_none());
	}

	/// The attachments and the pivot export onto their part's node, the
	/// pivot by its offset from the PrimaryPart.
	#[test]
	fn attachments_export_onto_their_parts() {
		let view = view();
		let nodes: Vec<(String, Option<usize>, [f32; 3])> = view
			.export_attachments()
			.into_iter()
			.map(|a| (a.name, a.node, a.local.pos))
			.collect();
		assert_eq!(
			nodes,
			[
				("Muzzle".to_string(), Some(1), [0.0, 0.0, -2.0]),
				("Loose".to_string(), None, [1.0, 2.0, 3.0]),
				("Pivot".to_string(), Some(0), [0.0, -1.0, 0.0]),
			]
		);
	}

	/// Older exports have neither: no pivot, no attachments.
	#[test]
	fn exports_without_them_have_none() {
		let model = AnimatedModel {
			parts: vec![placed_part(Some("Part"), Some("Block"), "Base", [0.0; 3])],
			..Default::default()
		};
		let ctx = egui::Context::default();
		let mut view = ModelView::load(&ctx, 0, &model, &HashMap::new(), &HashMap::new());
		assert!(!view.has_pivot());
		let points = view.points();
		assert!(points.attachments.is_empty() && points.pivot.is_none());
	}
}
