//! Headless frame measurements on a real Studio export.

use super::*;
use crate::test_support::{TempDir, real_export};
use crate::ui::library::Browser;
use crate::workspace::{AssetState, Item, MapRef, ModelItem};
use std::time::Instant;

fn measure(label: &str, ctx: &egui::Context, mut draw: impl FnMut(&mut egui::Ui)) {
	let mut times = Vec::new();
	let mut drawn = 0;
	let mut offset: f32 = 0.0;
	let time = ctx.input(|i| i.time);
	for frame in 0..36 {
		let mut input = egui::RawInput {
			screen_rect: Some(egui::Rect::from_min_size(
				egui::Pos2::ZERO,
				egui::vec2(1280.0, 800.0),
			)),
			time: Some(time + (frame + 1) as f64 / 60.0),
			events: vec![
				egui::Event::PointerMoved(egui::pos2(480.0, 400.0)),
				egui::Event::MouseWheel {
					source: egui::MouseWheelSource::Wheel,
					phase: egui::TouchPhase::Move,
					unit: egui::MouseWheelUnit::Point,
					delta: egui::vec2(0.0, -240.0),
					modifiers: Default::default(),
				},
			],
			..Default::default()
		};
		if label.ends_with("/selected") && frame == 3 {
			input
				.events
				.push(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
			input.events.push(egui::Event::Key {
				key: egui::Key::A,
				physical_key: None,
				pressed: true,
				repeat: false,
				modifiers: egui::Modifiers::COMMAND,
			});
		}
		let start = Instant::now();
		let mut output = ctx.run_ui(input, |ui| draw(ui));
		output.textures_delta.clear();
		std::hint::black_box(&output);
		let metrics = crate::ui::widgets::SCROLL_METRICS.get();
		drawn = drawn.max(metrics.0);
		offset = offset.max(metrics.1);
		if frame >= 6 {
			times.push(start.elapsed().as_secs_f64() * 1000.0);
		}
	}
	assert!(
		offset > 100.0,
		"{label}: scroll input must actually move the viewport"
	);
	assert!(
		drawn < 100,
		"{label}: only viewport rows should be laid out"
	);
	times.sort_by(f64::total_cmp);
	println!(
		"TREE_REPLAY {label}: median={:.3}ms p95={:.3}ms max={:.3}ms laid_out={drawn} scroll={offset:.0}px",
		times[15], times[28], times[29]
	);
	if std::env::var_os("MESHVAULT_SCROLL_BUDGET").is_some() {
		assert!(times[28] < 16.667, "{label} exceeds a 60 Hz frame budget");
	}
}

fn folders(ctx: &egui::Context, scope: &str, root: &FolderTree, expanded: bool) {
	let mut stack = vec![(root, String::new(), 0)];
	let mut count = 0;
	let mut depth = 0;
	while let Some((node, prefix, level)) = stack.pop() {
		for (name, child) in &node.folders {
			let path = tree::child_path(&prefix, name);
			tree::set_open(ctx, tree::folder_id(scope, &path), expanded || level < 3);
			count += 1;
			depth = depth.max(level + 1);
			stack.push((child, path, level + 1));
		}
	}
	println!(
		"TREE_REPLAY {scope}: items={} folders={count} depth={depth}",
		root.all_items().len()
	);
}

/// CPU frame timings through the real UI, without a window, GPU, asset
/// downloads or user configuration. Set MESHVAULT_EXPORT to the fixture;
/// MESHVAULT_SCROLL_BUDGET also enforces a 60 Hz p95 budget in debug.
#[test]
#[ignore = "reads MESHVAULT_EXPORT; headless scrolling measurement"]
fn tree_scroll_replay() {
	let source = real_export();
	let text = crate::studio::export_file::read_text(&source).unwrap();
	let export = crate::studio::export_file::parse(&text).unwrap();
	drop(text);
	let dir = TempDir::new("tree_scroll_replay");
	let mut ws = Workspace::create(&dir, "Replay").unwrap();
	let mut dialog = ImportDialog::new(source, export, &ws);
	dialog.selected.fill(true);
	dialog.first_frame = false;
	let items: Vec<Item> = dialog
		.entries
		.iter()
		.enumerate()
		.map(|(i, e)| {
			let mut item: Item = serde_json::from_value(serde_json::json!({
				"name": e.name, "folder": e.folder, "mesh_id": e.mesh_id,
				"stem": format!("{}/replay{i}", e.folder.join("/")), "mesh": {"state":"pending"}
			}))
			.unwrap();
			item.maps = e
				.maps
				.iter()
				.map(|(&k, &id)| {
					(
						k,
						MapRef {
							id,
							state: AssetState::Pending,
						},
					)
				})
				.collect();
			item
		})
		.collect();
	let by_source: HashMap<_, _> = dialog
		.entries
		.iter()
		.enumerate()
		.filter_map(|(i, e)| Some((e.source_id?, i)))
		.collect();
	let models: Vec<ModelItem> = dialog
		.models
		.iter()
		.enumerate()
		.map(|(i, m)| ModelItem {
			name: m.name.clone(),
			path: m.path.clone(),
			folder: model_folder(&dialog.origin, &m.path),
			game: dialog.origin.game.clone(),
			place_id: dialog.origin.place_id,
			file: format!("Rigs/replay{i}.rig.json"),
			meshes: m
				.parts
				.iter()
				.filter_map(|p| {
					Some((p.mesh_ref, items[*by_source.get(&p.mesh_ref)?].stem.clone()))
				})
				.collect(),
			parts: m.parts.len(),
			clips: m.animations.len(),
			playable_clips: m
				.animations
				.iter()
				.filter(|a| a.status.is_playable())
				.count(),
			images: Default::default(),
			version: Default::default(),
		})
		.collect();
	ws.set_contents(items, models);
	println!(
		"TREE_REPLAY export: meshes={} models={} import_rows={}",
		ws.items().len(),
		ws.models().len(),
		dialog.rows.len()
	);
	for expanded in [true, false] {
		let label = if expanded { "expanded" } else { "typical" };
		let ctx = egui::Context::default();
		crate::ui::theme::install_fonts(&ctx);
		crate::ui::theme::apply(&ctx, Default::default());
		folders(&ctx, SCOPE, &dialog.tree, expanded);
		measure(&format!("import/{label}"), &ctx, |ui| {
			std::hint::black_box(dialog.show(ui.ctx()));
		});
		let mut browser = Browser::default();
		let tree = FolderTree::build(ws.items().iter().map(|i| i.folder.as_slice()));
		folders(&ctx, "library", &tree, expanded);
		measure(&format!("library/{label}"), &ctx, |ui| {
			egui::CentralPanel::default().show(ui, |ui| {
				std::hint::black_box(browser.show(ui, &ws, None, None, None));
			});
		});
		if expanded && std::env::var_os("MESHVAULT_SCROLL_SELECTION").is_some() {
			measure("library/selected", &ctx, |ui| {
				egui::CentralPanel::default().show(ui, |ui| {
					std::hint::black_box(browser.show(ui, &ws, None, None, None));
				});
			});
		}
	}
}
