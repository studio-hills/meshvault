//! The top bar, which is also the window's title bar: the workspace
//! switcher, the file buttons, the panel toggles and the More menu, whose
//! export items the viewport toolbar shows too.

use eframe::egui::{self, Align, Layout, RichText, Vec2};

use super::layout::TOP_BAR_MARGIN;
use super::theme::{CONTROL, ThemeMode, display, medium, palette};
use super::{icons, widgets};
use crate::app::App;
use crate::scene::maps::Slot;
use crate::ui::ConfirmAction;

impl App {
	/// The top bar is also the window's title bar: its empty space moves the
	/// window, and the caption buttons sit at its right end.
	pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		// the whole bar, margins included
		let bar = ui
			.max_rect()
			.expand2(Vec2::new(TOP_BAR_MARGIN.0 as f32, TOP_BAR_MARGIN.1 as f32));
		super::window::title_bar_drag(ui, bar);
		ui.horizontal(|ui| {
			ui.set_min_height(CONTROL);
			ui.label(RichText::new(icons::CUBE).size(22.0).color(p.accent));
			ui.label(RichText::new("Meshvault").size(17.0).family(display()));
			ui.add_space(10.0);
			self.workspace_switcher(ui);
			ui.add_space(4.0);
			ui.separator();

			if widgets::button(ui, icons::FILE, "Open")
				.on_hover_text("Open a mesh, union or image file  (Ctrl+O)")
				.clicked()
			{
				self.pick_file();
			}
			let has_ws = self.workspace.is_some();
			let import = ui
				.add_enabled_ui(has_ws, |ui| {
					widgets::button(ui, icons::FILE_ARROW_DOWN, "Import")
				})
				.inner
				.on_hover_text("Import models from an export .json  (Ctrl+I)")
				.on_disabled_hover_text("Open or create a workspace to import into");
			if import.clicked() {
				self.pick_import();
			}
			let (pending, failed) = self.workspace_counts();
			if pending > 0 && !self.downloader.is_running() {
				let download = widgets::primary_button(
					ui,
					format!("{}  Download {pending}", icons::CLOUD_ARROW_DOWN),
				)
				.on_hover_text("Download everything not yet downloaded  (Ctrl+D)");
				if download.clicked() {
					self.start_downloads();
				}
			}
			if failed > 0 && !self.downloader.is_running() {
				let retry = ui.add(egui::Button::new(
					RichText::new(format!("{}  Retry {failed}", icons::ARROWS_CLOCKWISE))
						.color(p.danger)
						.family(medium()),
				));
				if retry.on_hover_text("Retry failed downloads").clicked() {
					self.retry_failed();
				}
			}

			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				super::window::caption_buttons(ui);
				ui.separator();
				// a square menu button, matching the icon buttons beside it
				ui.scope(|ui| {
					ui.spacing_mut().button_padding.x = 10.0;
					ui.menu_button(RichText::new(icons::DOTS_THREE_VERTICAL).size(16.0), |ui| {
						self.more_menu(ui)
					})
					.response
					.on_hover_text("More");
				});
				let has_doc = self.doc.is_some() || self.model.is_some();
				let inspector = ui
					.add_enabled_ui(has_doc, |ui| {
						// the icon draws its panel on the left; the inspector is on the right
						widgets::icon_toggle_flipped(
							ui,
							icons::SIDEBAR_SIMPLE,
							"Inspector  (Ctrl+Shift+B)",
							self.show_inspector && has_doc,
						)
					})
					.inner
					.on_disabled_hover_text("Open a model to inspect it");
				if inspector.clicked() {
					self.show_inspector = !self.show_inspector;
				}
				let has_ws = self.workspace.is_some();
				let library = ui
					.add_enabled_ui(has_ws, |ui| {
						widgets::icon_toggle(
							ui,
							icons::TREE_STRUCTURE,
							"Library  (Ctrl+B)",
							self.show_library && has_ws,
						)
					})
					.inner
					.on_disabled_hover_text("Open a workspace to use the library");
				if library.clicked() {
					self.show_library = !self.show_library;
				}
				if widgets::icon_button(ui, icons::KEY, "Roblox access  (Ctrl+,)").clicked() {
					self.open_settings(None);
				}
				let (icon, tip, next) = match self.config.theme {
					ThemeMode::Dark => (icons::SUN, "Switch to light theme", ThemeMode::Light),
					ThemeMode::Light => (icons::MOON, "Switch to dark theme", ThemeMode::Dark),
				};
				if widgets::icon_button(ui, icon, tip).clicked() {
					self.set_theme(next);
				}
			});
		});
	}

	fn workspace_switcher(&mut self, ui: &mut egui::Ui) {
		let label = match &self.workspace {
			Some(ws) => format!(
				"{}  {}  {}",
				icons::FOLDER_SIMPLE,
				ws.name(),
				icons::CARET_DOWN
			),
			None => format!(
				"{}  No workspace  {}",
				icons::FOLDER_SIMPLE,
				icons::CARET_DOWN
			),
		};
		ui.menu_button(RichText::new(label).family(medium()), |ui| {
			widgets::menu_style(ui);
			ui.set_min_width(240.0);
			let current = self.workspace.as_ref().map(|ws| ws.root.clone());
			let recent = self.recent_workspaces();
			if !recent.is_empty() {
				// line the heading up with the menu items' text
				ui.horizontal(|ui| {
					ui.add_space(ui.spacing().button_padding.x);
					ui.label(widgets::overline("Recent").weak());
				});
				for root in recent {
					let name = root
						.file_name()
						.map(|n| n.to_string_lossy().into_owned())
						.unwrap_or_default();
					let selected = current.as_ref() == Some(&root);
					let mark = if selected { icons::CHECK } else { "    " };
					if ui
						.button(format!("{mark}  {name}"))
						.on_hover_text(root.display().to_string())
						.clicked()
					{
						ui.close();
						if !selected {
							self.open_workspace(&root);
						}
					}
				}
				ui.separator();
			}
			if ui
				.button(format!("{}  New workspace…", icons::FOLDER_PLUS))
				.clicked()
			{
				ui.close();
				self.show_new_workspace(None);
			}
			if ui
				.button(format!("{}  Open workspace…", icons::FOLDER_OPEN))
				.clicked()
			{
				ui.close();
				self.pick_open_workspace();
			}
			if self.workspace.is_some() {
				if ui
					.button(format!("{}  Show in file browser", icons::FOLDER))
					.clicked()
				{
					ui.close();
					if let Some(root) = self.workspace.as_ref().map(|ws| ws.root.clone()) {
						self.reveal(&root);
					}
				}
				ui.separator();
				if widgets::danger_item(ui, format!("{}  Close workspace", icons::X)).clicked() {
					ui.close();
					self.ask(ConfirmAction::CloseWorkspace);
				}
			}
		});
	}

	fn more_menu(&mut self, ui: &mut egui::Ui) {
		widgets::menu_style(ui);
		ui.set_min_width(220.0);
		let has_doc = self.doc.is_some();
		ui.add_enabled_ui(has_doc || self.model.is_some(), |ui| self.export_items(ui));
		ui.separator();
		ui.add_enabled_ui(has_doc, |ui| {
			ui.menu_button(format!("{}  Load map", icons::IMAGE), |ui| {
				widgets::menu_style(ui);
				for slot in Slot::ALL {
					if ui.button(format!("{}…", slot.label())).clicked() {
						ui.close();
						self.pick_map(slot);
					}
				}
			});
		});
		let has_maps = self
			.doc
			.as_ref()
			.is_some_and(|d| !d.material.maps.is_empty());
		let remove_all = ui
			.add_enabled_ui(has_maps, |ui| {
				widgets::danger_item(ui, format!("{}  Remove all maps", icons::TRASH))
			})
			.inner;
		if remove_all.clicked() {
			ui.close();
			self.ask(ConfirmAction::RemoveAllMaps);
		}
		ui.separator();
		if ui
			.button(format!(
				"{}  Export from Roblox Studio…",
				icons::TRAY_ARROW_DOWN
			))
			.clicked()
		{
			ui.close();
			self.show_studio_dialog();
		}
		if ui
			.button(format!("{}  Keyboard shortcuts", icons::INFO))
			.clicked()
		{
			ui.close();
			self.show_shortcuts = true;
		}
		ui.separator();
		if widgets::danger_item(ui, format!("{}  Quit", icons::SIGN_OUT)).clicked() {
			ui.close();
			self.ask(ConfirmAction::Quit);
		}
	}

	/// Exporting what is shown, and saving pictures of it.
	pub(super) fn export_items(&mut self, ui: &mut egui::Ui) {
		let blender = widgets::blender_item(ui, "Export for Blender (.glb)…").on_hover_text(
			if self.model.is_some() {
				"Binary glTF: one armature, skinned meshes, every clip and the maps"
			} else {
				"Binary glTF with the mesh's maps"
			},
		);
		if blender.clicked() {
			ui.close();
			self.export_glb();
		}
		let obj = if self.model.is_some() {
			"Export as OBJ (as posed)…"
		} else {
			"Export visible as OBJ…"
		};
		if ui.button(format!("{}  {obj}", icons::EXPORT)).clicked() {
			ui.close();
			self.export_obj();
		}
		ui.separator();
		let gpu = self.gpu.is_some();
		if ui
			.add_enabled(
				gpu,
				egui::Button::new(format!("{}  Save image (PNG)…", icons::IMAGE)),
			)
			.on_hover_text("The view as it is now, with a transparent background")
			.clicked()
		{
			ui.close();
			self.save_image();
		}
		if ui
			.add_enabled(
				gpu,
				egui::Button::new(format!("{}  Copy image", icons::COPY)),
			)
			.on_hover_text("The view as it is now, to paste elsewhere")
			.clicked()
		{
			ui.close();
			self.copy_image();
		}
		if ui
			.add_enabled(
				gpu,
				egui::Button::new(format!(
					"{}  Save turntable (GIF)…",
					icons::ARROWS_CLOCKWISE
				)),
			)
			.on_hover_text("The camera turning once around the model, playing the selected clip")
			.clicked()
		{
			ui.close();
			self.save_turntable();
		}
	}
}
