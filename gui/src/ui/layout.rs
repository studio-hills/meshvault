//! The window layout: top bar, library, viewport, inspector and status bar.

use eframe::egui::{
	self, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Key, KeyboardShortcut, Layout,
	Margin, Modifiers, Order, RichText, Sense, Stroke, Vec2,
};

use super::theme::{CONTROL, ThemeMode, display, medium, palette, semibold};
use super::{icons, widgets};
use crate::app::App;
use crate::geometry::GroupKind;
use crate::maps::Slot;
use crate::ui::ConfirmAction;
use crate::viewport::{self, Camera, ScenePart, Shading, ViewSettings};

impl eframe::App for App {
	fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
		let ctx = ui.ctx().clone();
		self.poll_downloads();
		self.poll_parsing();
		self.poll_model();
		self.poll_studio_link();
		self.handle_dropped_files(&ctx);
		self.shortcuts(&ctx);
		let p = palette(&ctx);

		egui::Panel::top("top_bar")
			.frame(
				Frame::NONE
					.fill(p.panel)
					.inner_margin(Margin::symmetric(12, 8)),
			)
			.show(ui, |ui| self.top_bar(ui));
		egui::Panel::bottom("status_bar")
			.frame(
				Frame::NONE
					.fill(p.panel)
					.inner_margin(Margin::symmetric(12, 4)),
			)
			.show(ui, |ui| self.status_bar(ui));
		// without a workspace there is no library: the welcome screen takes its place
		if self.show_library && self.workspace.is_some() {
			egui::Panel::left("library")
				.default_size(290.0)
				.size_range(220.0..=520.0)
				.frame(Frame::NONE.fill(p.panel).inner_margin(Margin::same(12)))
				.show(ui, |ui| self.library(ui));
		}
		if self.show_inspector && (self.doc.is_some() || self.model.is_some()) {
			egui::Panel::right("inspector")
				.default_size(300.0)
				.size_range(240.0..=520.0)
				.frame(Frame::NONE.fill(p.panel).inner_margin(Margin::same(12)))
				.show(ui, |ui| self.inspector(ui));
		}
		egui::CentralPanel::default()
			.frame(Frame::NONE.fill(p.bg))
			.show(ui, |ui| self.center(ui));

		self.dialogs(&ctx);
		self.toasts.show(&ctx);
		self.drop_overlay(&ctx);
	}
}

impl App {
	fn top_bar(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
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
					ui.label(RichText::new("RECENT").size(11.0).family(semibold()).weak());
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
					if let Some(ws) = &self.workspace {
						crate::workspace_ui::reveal(&ws.root);
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
		if ui
			.add_enabled(
				has_doc,
				egui::Button::new(format!("{}  Export visible as OBJ…", icons::EXPORT)),
			)
			.clicked()
		{
			ui.close();
			self.export_obj();
		}
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

	fn status_bar(&mut self, ui: &mut egui::Ui) {
		let p = palette(ui.ctx());
		ui.horizontal(|ui| {
			ui.spacing_mut().item_spacing.x = 10.0;
			ui.spacing_mut().interact_size.y = 22.0;
			let small =
				|text: String, colour: Color32| RichText::new(text).size(11.5).color(colour);
			match &self.adapter {
				Some(name) if self.gpu.is_some() => {
					ui.label(small(format!("{}  GPU", icons::CIRCLE), p.success))
						.on_hover_text(format!("Rendering on {name}"));
				}
				_ => {
					ui.label(small(format!("{}  CPU renderer", icons::CIRCLE), p.warning))
						.on_hover_text(
							"No GPU renderer: normal, roughness and metalness maps are not shown",
						);
				}
			}
			// an export that stops arriving is forgotten after a while
			let receiving = self
				.studio_progress
				.filter(|(_, _, at)| at.elapsed().as_secs() < 60);
			let studio = match (&self.studio_link.status, receiving) {
				(crate::studio_link::Status::Listening, Some((done, count, _))) => ui
					.add(
						egui::Label::new(small(
							format!("{}  Receiving from Studio {done}/{count}", icons::CIRCLE),
							p.accent,
						))
						.sense(Sense::click()),
					)
					.on_hover_text("An export is arriving from Roblox Studio"),
				(crate::studio_link::Status::Listening, None) => ui
					.add(
						egui::Label::new(small(
							format!("{}  Studio link", icons::CIRCLE),
							p.success,
						))
						.sense(Sense::click()),
					)
					.on_hover_text(format!(
						"Listening on localhost:{} for exports from Roblox Studio. Click for the Command Bar line.",
						crate::studio_link::PORT
					)),
				(crate::studio_link::Status::Unavailable(why), _) => ui
					.add(
						egui::Label::new(small(
							format!("{}  Studio link unavailable", icons::CIRCLE),
							p.warning,
						))
						.sense(Sense::click()),
					)
					.on_hover_text(crate::ui::unavailable_text(why)),
			};
			if widgets::pointer(studio).clicked() {
				self.show_studio_dialog();
			}
			let path = match (&self.doc, &self.model) {
				(Some(doc), _) => doc.path.display().to_string(),
				(None, Some(model)) => model.path.clone(),
				(None, None) => "No model open".into(),
			};
			ui.add(egui::Label::new(small(path, p.text_weak)).truncate());

			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				if self.downloader.is_running() {
					if widgets::icon_button(ui, icons::STOP, "Cancel downloads").clicked() {
						self.downloader.cancel();
					}
					let d = &self.downloader;
					let progress = d.completed as f32 / d.total.max(1) as f32;
					ui.add(
						egui::ProgressBar::new(progress)
							.desired_width(160.0)
							.desired_height(14.0)
							.fill(p.accent)
							.text(RichText::new(format!("{}/{}", d.completed, d.total)).size(10.5)),
					);
					let mut text = format!("{}  Downloading", icons::CLOUD_ARROW_DOWN);
					if d.failed > 0 {
						text.push_str(&format!(" · {} failed", d.failed));
					}
					ui.label(small(text, p.text_weak));
				} else if let Some(ws) = &self.workspace {
					let items = ws.items();
					let done = items.iter().filter(|i| i.is_done()).count();
					let mut text = format!("{} meshes · {done} downloaded", items.len());
					let models = ws.models().len();
					if models > 0 {
						text = format!("{models} animated · {text}");
					}
					ui.label(small(text, p.text_weak));
				}
			});
		});
	}

	fn library(&mut self, ui: &mut egui::Ui) {
		if let Some(ws) = &self.workspace {
			let open_model = self.model.as_ref().map(|m| m.index);
			let actions = self
				.browser
				.show(ui, ws, self.open_stem.as_deref(), open_model);
			self.handle_browser(actions);
		}
	}

	fn center(&mut self, ui: &mut egui::Ui) {
		if self.model.is_some() {
			self.model_center(ui);
			return;
		}
		if self.doc.is_none() {
			self.welcome(ui);
			return;
		}
		let p = palette(ui.ctx());
		let settings = self.view_settings(&p);
		let Some(doc) = &mut self.doc else { return };
		let groups = doc.visible_groups();
		let geometry = &doc.loaded.geometry;
		let mut parts = [ScenePart {
			geometry,
			groups: &groups,
			transform: viewport::IDENTITY,
			material: if self.show_maps {
				&doc.material
			} else {
				&self.no_material
			},
			colour_groups: geometry.group_kind == GroupKind::Hull,
			colour: viewport::PART_COLOUR,
			scene: &mut doc.gpu,
			moved: false,
			surface: viewport::Surface::OPAQUE,
		}];
		let home = || Camera::framing(geometry);
		let rect = viewport::show(
			ui,
			&mut parts,
			&mut doc.camera,
			&settings,
			self.gpu.as_deref(),
			&home,
		);
		self.viewport_toolbar(ui.ctx(), rect);
	}

	pub(super) fn view_settings(&self, p: &super::theme::Palette) -> ViewSettings {
		ViewSettings {
			shading: self.shading,
			cull_backfaces: self.cull_backfaces,
			show_axes: self.show_axes,
			show_grid: self.show_grid,
			background: [p.viewport_top, p.viewport_bottom],
			grid_colour: p.grid,
		}
	}

	/// Floating controls over the top-left of the viewport.
	pub(super) fn viewport_toolbar(&mut self, ctx: &egui::Context, rect: egui::Rect) {
		let p = palette(ctx);
		egui::Area::new(Id::unique("viewport_toolbar"))
			.fixed_pos(rect.left_top() + Vec2::new(12.0, 12.0))
			.order(Order::Middle)
			.show(ctx, |ui| {
				Frame::NONE
					.fill(p.panel.gamma_multiply(0.94))
					.stroke(Stroke::new(1.0, p.border))
					.corner_radius(CornerRadius::same(10))
					.inner_margin(Margin::same(4))
					.shadow(ctx.global_style().visuals.popup_shadow)
					.show(ui, |ui| {
						ui.spacing_mut().interact_size.y = 30.0;
						ui.horizontal(|ui| {
							ui.spacing_mut().item_spacing.x = 4.0;
							widgets::segmented(
								ui,
								&mut self.shading,
								&[
									(Shading::Solid, icons::CUBE, "Solid  (1)"),
									(
										Shading::SolidWireframe,
										icons::BOUNDING_BOX,
										"Solid + wireframe  (2)",
									),
									(Shading::Wireframe, icons::POLYGON, "Wireframe  (3)"),
								],
							);
							ui.separator();
							let toggles: [(&mut bool, &str, &str); 4] = [
								(&mut self.show_maps, icons::IMAGE, "Maps  (M)"),
								(&mut self.show_grid, icons::GRID_FOUR, "Ground grid  (G)"),
								(&mut self.show_axes, icons::CROSSHAIR, "Axes"),
								(
									&mut self.cull_backfaces,
									icons::EYE_SLASH,
									"Hide back faces",
								),
							];
							for (value, icon, tip) in toggles {
								if widgets::icon_toggle(ui, icon, tip, *value).clicked() {
									*value = !*value;
								}
							}
							ui.separator();
							if widgets::icon_button(ui, icons::CORNERS_OUT, "Frame model  (F)")
								.clicked()
							{
								self.frame_model();
							}
							widgets::icon_button(ui, icons::INFO, "").on_hover_text(
								"Left-drag: orbit\nRight or middle-drag: pan\nScroll: zoom\nDouble-click: frame model",
							);
						});
					});
			});
	}

	fn handle_dropped_files(&mut self, ctx: &egui::Context) {
		let dropped = ctx.input(|i| i.raw.dropped_files.clone());
		for file in dropped {
			let path = file.path().to_path_buf();
			match file.bytes() {
				Ok(bytes) => self.open_bytes(path, &bytes),
				Err(e) => self
					.toasts
					.error(format!("Could not read {}: {e}", path.display())),
			}
		}
	}

	/// Dims the window and explains what dropping will do.
	fn drop_overlay(&self, ctx: &egui::Context) {
		let hovered = ctx.input(|i| i.raw.hovered_files.clone());
		let Some(first) = hovered.first() else { return };
		let p = palette(ctx);
		let path = first.path.clone().unwrap_or_default();
		let ext = path
			.extension()
			.map(|e| e.to_string_lossy().to_lowercase())
			.unwrap_or_default();
		let is_rig = path
			.file_name()
			.is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(".rig.json"));
		let (icon, title) = if is_rig {
			(icons::PERSON_SIMPLE_RUN, "Open animated model")
		} else if ext == "json" {
			(icons::FILE_ARROW_DOWN, "Import export file")
		} else if crate::texture::is_image(&path) {
			(icons::IMAGE, "Use as material map")
		} else {
			(icons::CUBE, "Open model")
		};
		let screen = ctx.content_rect();
		let painter = ctx.layer_painter(egui::LayerId::new(
			Order::Foreground,
			Id::unique("drop_overlay"),
		));
		painter.rect_filled(screen, 0.0, Color32::from_black_alpha(150));
		let inner = screen.shrink(24.0);
		let corners = [
			inner.left_top(),
			inner.right_top(),
			inner.right_bottom(),
			inner.left_bottom(),
			inner.left_top(),
		];
		for pair in corners.windows(2) {
			painter.extend(egui::Shape::dashed_line(
				pair,
				Stroke::new(2.0, p.accent),
				10.0,
				8.0,
			));
		}
		painter.text(
			screen.center() - Vec2::new(0.0, 24.0),
			Align2::CENTER_CENTER,
			icon,
			FontId::proportional(48.0),
			p.accent,
		);
		painter.text(
			screen.center() + Vec2::new(0.0, 24.0),
			Align2::CENTER_CENTER,
			title,
			FontId::new(18.0, semibold()),
			Color32::WHITE,
		);
	}

	fn shortcuts(&mut self, ctx: &egui::Context) {
		let shortcut = |modifiers, key| KeyboardShortcut::new(modifiers, key);
		let cmd = Modifiers::COMMAND;
		let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
		let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
		// shifted variants first so they are not taken by the plain ones
		if pressed(shortcut(cmd_shift, Key::O)) {
			self.pick_open_workspace();
		}
		if pressed(shortcut(cmd_shift, Key::B)) {
			self.show_inspector = !self.show_inspector;
		}
		if pressed(shortcut(cmd, Key::O)) {
			self.pick_file();
		}
		if pressed(shortcut(cmd, Key::N)) {
			self.show_new_workspace(None);
		}
		if pressed(shortcut(cmd, Key::I)) && self.workspace.is_some() {
			self.pick_import();
		}
		if pressed(shortcut(cmd, Key::D)) {
			self.start_downloads();
		}
		if pressed(shortcut(cmd, Key::E)) {
			self.export_obj();
		}
		if pressed(shortcut(cmd, Key::B)) && self.workspace.is_some() {
			self.show_library = !self.show_library;
		}
		if pressed(shortcut(cmd, Key::Comma)) {
			self.open_settings(None);
		}
		// single keys only while no text field has focus
		if ctx.egui_wants_keyboard_input() {
			return;
		}
		let key = |k| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
		if key(Key::F) {
			self.frame_model();
		}
		if let Some(model) = &mut self.model
			&& key(Key::Space)
		{
			model.toggle_play();
		}
		if key(Key::Num1) {
			self.shading = Shading::Solid;
		}
		if key(Key::Num2) {
			self.shading = Shading::SolidWireframe;
		}
		if key(Key::Num3) {
			self.shading = Shading::Wireframe;
		}
		if key(Key::M) {
			self.show_maps = !self.show_maps;
		}
		if key(Key::G) {
			self.show_grid = !self.show_grid;
		}
		if ctx.input_mut(|i| {
			i.consume_key(Modifiers::SHIFT, Key::Questionmark)
				|| i.consume_key(Modifiers::NONE, Key::Questionmark)
		}) {
			self.show_shortcuts = true;
		}
	}
}

/// Keyboard shortcuts, for the help dialog.
pub const SHORTCUTS: &[(&str, &str)] = &[
	("Ctrl+O", "Open a file"),
	("Ctrl+Shift+O", "Open a workspace"),
	("Ctrl+N", "New workspace"),
	("Ctrl+I", "Import an export .json"),
	("Ctrl+D", "Download pending"),
	("Ctrl+E", "Export visible geometry as OBJ"),
	("Ctrl+,", "Roblox access"),
	("Ctrl+B", "Toggle library"),
	("Ctrl+Shift+B", "Toggle inspector"),
	("F", "Frame model"),
	("Space", "Play or pause the animation"),
	("1 / 2 / 3", "Solid / solid + wireframe / wireframe"),
	("M", "Toggle maps"),
	("G", "Toggle ground grid"),
	("?", "Show this list"),
];
