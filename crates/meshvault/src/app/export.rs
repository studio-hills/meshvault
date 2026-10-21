//! Exporting the library, or what is shown, as glTF or OBJ.

use std::path::PathBuf;

use super::App;
use crate::export::Format;
use crate::ui::format::plural;
use crate::workspace::view_loader::{item_maps, model_loader};
use crate::workspace::{self, AssetState};

impl App {
	/// Exports library meshes and animated models into a folder the user
	/// picks, on a background thread.
	pub fn export_library(&mut self, items: &[usize], models: &[usize], format: Format) {
		use crate::export::{MeshJob, ModelJob};
		let Some(ws) = &self.workspace else { return };
		let meshes: Vec<MeshJob> = items
			.iter()
			.filter_map(|&i| {
				let item = ws.items().get(i)?;
				let AssetState::Done { path } = &item.mesh else {
					return None;
				};
				Some(MeshJob {
					name: item.name.clone(),
					mesh: ws.root.join(path),
					maps: item_maps(&ws.root, item),
				})
			})
			.collect();
		let skipped = items.len() - meshes.len();
		let models: Vec<ModelJob> = models
			.iter()
			.filter_map(|&i| {
				Some(ModelJob {
					name: ws.models().get(i)?.name.clone(),
					load: Box::new(model_loader(ws, i)?),
				})
			})
			.collect();
		if meshes.is_empty() && models.is_empty() {
			self.toasts
				.info("Nothing to export: the selected meshes are not downloaded yet.");
			return;
		}
		let what = match format {
			Format::Glb => "for Blender (.glb)",
			Format::Obj => "as OBJ",
		};
		let Some(dir) = rfd::FileDialog::new()
			.set_title(format!("Export {what} into"))
			.pick_folder()
		else {
			return;
		};
		if skipped > 0 {
			self.toasts.info(format!(
				"{skipped} selected meshes are not downloaded yet and are skipped."
			));
		}
		self.in_background(move |ctx| {
			let report = crate::export::export(ctx, &dir, meshes, models, format);
			let written = plural(report.written, "model", "models");
			if report.failed.is_empty() {
				Ok(format!(
					"Exported {written} {what} to {}",
					report.dir.display()
				))
			} else {
				Err(format!(
					"Exported {written}; {} failed:\n{}",
					report.failed.len(),
					report.failed.join("\n")
				))
			}
		});
	}

	/// The open mesh's or model's name, for file names.
	pub(super) fn shown_name(&self) -> String {
		if let Some(model) = &self.model {
			return workspace::sanitize(&model.name);
		}
		self.doc
			.as_ref()
			.and_then(|d| d.path.file_stem())
			.map(|s| s.to_string_lossy().into_owned())
			.unwrap_or_else(|| "mesh".into())
	}

	pub(super) fn save_dialog(
		&self,
		title: &str,
		extension: &str,
		filter: &str,
	) -> Option<PathBuf> {
		rfd::FileDialog::new()
			.set_title(title)
			.set_file_name(format!("{}.{extension}", self.shown_name()))
			.add_filter(filter, &[extension])
			.save_file()
	}

	/// Exports what is shown as binary glTF for Blender: an animated model
	/// with its skeleton, skin weights and every clip; a mesh with its maps.
	pub fn export_glb(&mut self) {
		if let Some(model) = &self.model {
			let Some(ws) = &self.workspace else { return };
			// a stale index still panics here, as before (bug B13)
			let load = model_loader(ws, model.index).expect("the open model is in the workspace");
			let name = model.name.clone();
			let Some(path) = self.save_dialog("Export for Blender", "glb", "glTF binary") else {
				return;
			};
			self.in_background(move |ctx| {
				let model = load(ctx)?;
				crate::export::gltf::export_model(&model, &path)?;
				Ok(format!(
					"Exported {name} with {} for Blender to {}. In Blender: File › Import › glTF 2.0.",
					plural(
						model.clips.iter().filter(|c| c.clip.is_ok()).count(),
						"clip",
						"clips"
					),
					path.display()
				))
			});
		} else if self.doc.is_some() {
			let Some(path) = self.save_dialog("Export for Blender", "glb", "glTF binary") else {
				return;
			};
			let Some(doc) = &self.doc else { return };
			let name = self.shown_name();
			let result = crate::export::gltf::export_mesh(
				&doc.loaded.geometry,
				&doc.visible_groups(),
				&doc.material,
				crate::scene::surface::PART_COLOUR,
				&name,
				&path,
			);
			match result {
				Ok(()) => self.toasts.success(format!(
					"Exported {} for Blender. In Blender: File › Import › glTF 2.0.",
					path.display()
				)),
				Err(e) => self.toasts.error(format!("Export failed: {e}")),
			}
		}
	}

	/// Exports what is shown as OBJ, with an MTL file and its maps beside
	/// it: the visible LODs or hulls of a mesh, or a model as posed now.
	pub fn export_obj(&mut self) {
		if self.doc.is_none() && self.model.is_none() {
			return;
		}
		let Some(path) = self.save_dialog("Export OBJ", "obj", "Wavefront OBJ") else {
			return;
		};
		let result = if let Some(model) = &mut self.model {
			crate::export::obj::write_model(&path, model)
		} else if let Some(doc) = &self.doc {
			crate::export::obj::write_mesh(
				&path,
				&doc.loaded.geometry,
				&doc.visible_groups(),
				&doc.material,
			)
		} else {
			return;
		};
		match result {
			Ok(()) => self.toasts.success(format!("Exported {}", path.display())),
			Err(e) => self.toasts.error(format!("Export failed: {e}")),
		}
	}
}
