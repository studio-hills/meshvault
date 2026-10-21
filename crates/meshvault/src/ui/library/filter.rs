//! Which rows the search, status chips and filters let through, and a
//! model's download status rolled up from its meshes.

use super::{Browser, Status, StatusFilter, status};
use crate::workspace::{AssetState, Item, ModelItem, Workspace};

impl Browser {
	/// The items a model's meshes are, where they are still in the workspace.
	pub(super) fn model_items<'a>(
		&'a self,
		ws: &'a Workspace,
		model: &'a ModelItem,
	) -> impl Iterator<Item = (usize, &'a Item)> + 'a {
		model
			.meshes
			.values()
			.filter_map(|stem| self.stems.get(stem))
			.map(|&i| (i, &ws.items()[i]))
	}

	/// A model is done when every mesh it uses is in the workspace and
	/// downloaded, with its maps (so a model with no meshes is done); pending
	/// while any mesh is missing or any mesh or map is still to download;
	/// failed when any mesh or map failed, even if others are pending.
	pub(super) fn model_status(&self, ws: &Workspace, model: &ModelItem) -> Status {
		let mut s = if self.model_items(ws, model).count() < model.meshes.len() {
			Status::Pending
		} else {
			Status::Done
		};
		for (_, item) in self.model_items(ws, model) {
			match status(item) {
				Status::Failed => return Status::Failed,
				Status::Pending => s = Status::Pending,
				Status::Done => {}
			}
		}
		s
	}

	pub(super) fn skinned(&self, item: &Item) -> bool {
		let AssetState::Done { path } = &item.mesh else {
			return false;
		};
		self.facts
			.as_ref()
			.and_then(|f| f.get(path))
			.is_some_and(|f| f.skinned)
	}

	fn has_map(&self, item: &Item) -> bool {
		self.filters.maps.iter().any(|k| item.maps.contains_key(k))
	}

	pub(super) fn matches(&self, item: &Item) -> bool {
		let status_ok = match self.status {
			StatusFilter::All => true,
			StatusFilter::Downloaded => item.is_done(),
			StatusFilter::Pending => item.has_pending() && !item.has_failure(),
			StatusFilter::Failed => item.has_failure(),
		};
		let f = &self.filters;
		if !status_ok
			|| f.animated
			|| (f.skinned && !self.skinned(item))
			|| (f.missing && matches!(item.mesh, AssetState::Done { .. }))
			|| (!f.maps.is_empty() && !self.has_map(item))
		{
			return false;
		}
		if self.filter.is_empty() {
			return true;
		}
		let needle = self.filter.to_lowercase();
		item.name.to_lowercase().contains(&needle)
			|| item
				.folder
				.iter()
				.any(|f| f.to_lowercase().contains(&needle))
			|| item.mesh_id.to_string() == needle
	}

	pub(super) fn model_matches(&self, ws: &Workspace, model: &ModelItem) -> bool {
		let s = self.model_status(ws, model);
		let status_ok = match self.status {
			StatusFilter::All => true,
			StatusFilter::Downloaded => s == Status::Done,
			StatusFilter::Pending => s == Status::Pending,
			StatusFilter::Failed => s == Status::Failed,
		};
		let f = &self.filters;
		let mut items = self.model_items(ws, model);
		if !status_ok
			|| (f.animated && model.playable_clips == 0)
			|| (f.skinned && !items.any(|(_, i)| self.skinned(i)))
			|| (f.missing && s == Status::Done)
			|| (!f.maps.is_empty() && !self.model_items(ws, model).any(|(_, i)| self.has_map(i)))
		{
			return false;
		}
		let needle = self.filter.to_lowercase();
		needle.is_empty()
			|| model.name.to_lowercase().contains(&needle)
			|| model.path.to_lowercase().contains(&needle)
	}

	pub(super) fn narrowed(&self) -> bool {
		!self.filter.is_empty() || self.status != StatusFilter::All || self.filters.count() > 0
	}
}
