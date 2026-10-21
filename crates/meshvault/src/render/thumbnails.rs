//! Library thumbnails: each mesh and animated model drawn once off screen
//! and kept as a small PNG under `.cache/thumbnails`, so the library can
//! show a grid of pictures. Only what the library shows is drawn, one at a
//! time, with its files read on a background thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::SystemTime;

use eframe::egui;

use crate::render::gpu::GpuRenderer;
use crate::render::viewport::{self, Camera, ScenePart, Shading, ViewSettings};
use crate::scene::geometry::{self, Geometry, GroupKind};
use crate::scene::maps::Slot;
use crate::scene::model_view::ModelView;
use crate::scene::texture::{self, Material};
use crate::workspace::view_loader::{item_maps, model_loader};
use crate::workspace::{AssetState, CACHE_DIR, Workspace};

/// Pixels on each side of a thumbnail.
pub const SIZE: u32 = 192;
/// Drawn this many times bigger, then scaled down, for smooth edges.
const SUPERSAMPLE: u32 = 2;
/// Cached thumbnails read from disk per frame, so scrolling stays smooth.
const LOADS_PER_FRAME: usize = 6;
/// The viewer's framing leaves room to orbit; a thumbnail is filled more.
const ZOOM: f32 = 0.85;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Subject {
	/// A mesh, by its stem.
	Item(String),
	/// An animated model, by its instance path.
	Model(String),
}

impl Subject {
	fn key(&self) -> String {
		match self {
			Subject::Item(stem) => format!("item:{stem}"),
			Subject::Model(path) => format!("model:{path}"),
		}
	}
}

enum State {
	Ready(egui::TextureHandle),
	/// Could not be drawn; the tile keeps its icon.
	Failed,
}

/// What to draw a subject from, once it is downloaded.
enum Source {
	Mesh {
		mesh: PathBuf,
		maps: Vec<(Slot, PathBuf)>,
	},
	Model(usize),
}

/// What the background thread read, ready to draw.
enum Prepared {
	Mesh {
		geometry: Geometry,
		groups: Vec<usize>,
		material: Material,
	},
	Model(Box<ModelView>),
}

struct Job {
	subject: Subject,
	file: PathBuf,
	rx: Receiver<Result<Prepared, String>>,
}

pub struct Thumbnails {
	dir: PathBuf,
	states: HashMap<Subject, State>,
	/// Asked for this frame, in the order shown.
	wanted: Vec<Subject>,
	job: Option<Job>,
}

impl Thumbnails {
	pub fn new(root: &Path) -> Self {
		Self {
			dir: root.join(CACHE_DIR).join("thumbnails"),
			states: HashMap::new(),
			wanted: Vec::new(),
			job: None,
		}
	}

	/// The thumbnail of `subject` when it is ready; otherwise it is loaded
	/// or drawn over the next frames.
	pub fn get(&mut self, subject: &Subject) -> Option<egui::TextureHandle> {
		match self.states.get(subject) {
			Some(State::Ready(texture)) => Some(texture.clone()),
			Some(State::Failed) => None,
			None => {
				self.wanted.push(subject.clone());
				None
			}
		}
	}

	/// Loads or draws what was asked for this frame. Call once a frame,
	/// after the library is drawn.
	pub fn update(&mut self, ctx: &egui::Context, ws: &Workspace, renderer: Option<&GpuRenderer>) {
		let wanted = std::mem::take(&mut self.wanted);
		self.finish_job(ctx, renderer);
		let mut loads = 0;
		let mut waiting = false;
		for subject in wanted {
			if self.states.contains_key(&subject)
				|| self.job.as_ref().is_some_and(|j| j.subject == subject)
			{
				continue;
			}
			// not downloaded yet: asked for again once it is
			let Some((source, files)) = source(ws, &subject) else {
				continue;
			};
			if loads == LOADS_PER_FRAME {
				waiting = true;
				continue;
			}
			loads += 1;
			let file = self.dir.join(format!("{:016x}.png", fnv1a(&subject.key())));
			if is_fresh(&file, &files)
				&& let Some(texture) = load_png(ctx, &file, &subject)
			{
				self.states.insert(subject, State::Ready(texture));
				continue;
			}
			match renderer {
				Some(_) if self.job.is_none() => {
					self.job = Some(spawn(ctx, ws, subject, file, source));
				}
				Some(_) => waiting = true,
				// without a GPU nothing is drawn
				None => {
					self.states.insert(subject, State::Failed);
				}
			}
		}
		if waiting || self.job.is_some() {
			ctx.request_repaint();
		}
	}

	fn finish_job(&mut self, ctx: &egui::Context, renderer: Option<&GpuRenderer>) {
		let Some(job) = &self.job else { return };
		let result = match job.rx.try_recv() {
			Ok(result) => result,
			Err(TryRecvError::Empty) => return,
			Err(TryRecvError::Disconnected) => Err("the reader stopped".into()),
		};
		let Some(job) = self.job.take() else { return };
		let state = match (result, renderer) {
			(Ok(prepared), Some(renderer)) => {
				let image = draw(renderer, prepared);
				// a thumbnail that cannot be saved is still shown
				if std::fs::create_dir_all(&self.dir).is_ok() {
					let _ = image.save(&job.file);
				}
				State::Ready(texture(ctx, &job.subject, &image))
			}
			// the reader's reason is not shown anywhere
			_ => State::Failed,
		};
		self.states.insert(job.subject, state);
	}
}

/// What `subject` is drawn from, and the files whose changes make its
/// thumbnail stale. `None` while it is not downloaded.
fn source(ws: &Workspace, subject: &Subject) -> Option<(Source, Vec<PathBuf>)> {
	match subject {
		Subject::Item(stem) => {
			let item = ws.items().iter().find(|i| &i.stem == stem)?;
			let AssetState::Done { path } = &item.mesh else {
				return None;
			};
			let mesh = ws.root.join(path);
			let maps = item_maps(&ws.root, item);
			let mut files = vec![mesh.clone()];
			files.extend(maps.iter().map(|(_, p)| p.clone()));
			Some((Source::Mesh { mesh, maps }, files))
		}
		Subject::Model(path) => {
			let index = ws.models().iter().position(|m| &m.path == path)?;
			let model = &ws.models()[index];
			let mut files = vec![ws.root.join(&model.file)];
			for stem in model.meshes.values() {
				// a mesh removed from the workspace, or failed, is drawn without
				let Some(item) = ws.items().iter().find(|i| &i.stem == stem) else {
					continue;
				};
				match &item.mesh {
					AssetState::Pending => return None,
					AssetState::Done { path } => files.push(ws.root.join(path)),
					AssetState::Failed { .. } => {}
				}
			}
			Some((Source::Model(index), files))
		}
	}
}

/// Whether the thumbnail at `file` is newer than everything it shows.
fn is_fresh(file: &Path, files: &[PathBuf]) -> bool {
	let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
	let Some(drawn) = modified(file) else {
		return false;
	};
	files
		.iter()
		.filter_map(|f| modified(f))
		.all(|m: SystemTime| m <= drawn)
}

fn load_png(ctx: &egui::Context, file: &Path, subject: &Subject) -> Option<egui::TextureHandle> {
	let image = image::open(file).ok()?.to_rgba8();
	Some(texture(ctx, subject, &image))
}

fn texture(
	ctx: &egui::Context,
	subject: &Subject,
	image: &image::RgbaImage,
) -> egui::TextureHandle {
	let size = [image.width() as usize, image.height() as usize];
	let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
	ctx.load_texture(subject.key(), pixels, egui::TextureOptions::LINEAR)
}

/// Reads `source` on a background thread.
fn spawn(
	ctx: &egui::Context,
	ws: &Workspace,
	subject: Subject,
	file: PathBuf,
	source: Source,
) -> Job {
	let (tx, rx) = std::sync::mpsc::channel();
	let ctx = ctx.clone();
	match source {
		Source::Mesh { mesh, maps } => {
			std::thread::spawn(move || {
				let _ = tx.send(read_mesh(&ctx, &mesh, &maps));
				ctx.request_repaint();
			});
		}
		Source::Model(index) => {
			let load = model_loader(ws, index);
			std::thread::spawn(move || {
				let _ = tx.send(load(&ctx).map(|m| Prepared::Model(Box::new(m))));
				ctx.request_repaint();
			});
		}
	}
	Job { subject, file, rx }
}

fn read_mesh(
	ctx: &egui::Context,
	mesh: &Path,
	maps: &[(Slot, PathBuf)],
) -> Result<Prepared, String> {
	let bytes = std::fs::read(mesh).map_err(|e| e.to_string())?;
	let geometry = geometry::load(&bytes)?.geometry;
	let groups = geometry.default_groups();
	let material = if geometry.uvs.is_some() {
		texture::load_maps(ctx, maps)
	} else {
		Material::default()
	};
	Ok(Prepared::Mesh {
		geometry,
		groups,
		material,
	})
}

/// Thumbnails ignore the viewer's choices: solid shading on black, no grid
/// or axes, default lighting and no model lights (a model adds its own).
fn thumbnail_settings() -> ViewSettings {
	ViewSettings {
		shading: Shading::Solid,
		cull_backfaces: false,
		show_axes: false,
		show_grid: false,
		background: [egui::Color32::BLACK; 2],
		grid_colour: egui::Color32::BLACK,
		lights: Vec::new(),
		lighting: Default::default(),
		highlight: egui::Color32::BLACK,
	}
}

/// Draws a thumbnail from a front three-quarter view, as the viewer opens.
fn draw(renderer: &GpuRenderer, prepared: Prepared) -> image::RgbaImage {
	let mut settings = thumbnail_settings();
	let big = [SIZE * SUPERSAMPLE; 2];
	let image = match prepared {
		Prepared::Mesh {
			geometry,
			groups,
			material,
		} => {
			let mut camera = Camera::framing(&geometry);
			camera.distance *= ZOOM;
			let mut scene = None;
			let mut parts = [ScenePart {
				geometry: &geometry,
				groups: &groups,
				transform: viewport::IDENTITY,
				material: &material,
				colour_groups: geometry.group_kind == GroupKind::Hull,
				colour: viewport::PART_COLOUR,
				scene: &mut scene,
				moved: false,
				surface: viewport::Surface::OPAQUE,
				decal: None,
				highlight: false,
			}];
			viewport::render_image(&mut parts, &camera, &settings, renderer, big)
		}
		Prepared::Model(mut model) => {
			settings.lights = model.scene_lights();
			let mut parts = model.scene_parts(None);
			// framed on what is drawn, in the pose it is drawn in
			let mut camera = Camera::framing_bounds(drawn_bounds(&parts));
			camera.distance *= ZOOM;
			viewport::render_image(&mut parts, &camera, &settings, renderer, big)
		}
	};
	image::imageops::resize(&image, SIZE, SIZE, image::imageops::FilterType::Triangle)
}

/// The box around every drawn vertex (decals lie on their parts).
fn drawn_bounds(parts: &[ScenePart]) -> Option<([f32; 3], [f32; 3])> {
	let mut bounds: Option<([f32; 3], [f32; 3])> = None;
	for part in parts.iter().filter(|p| p.decal.is_none()) {
		let m = &part.transform;
		for &g in part.groups {
			let Some(range) = part.geometry.groups.get(g) else {
				continue;
			};
			for t in &part.geometry.triangles[range.clone()] {
				for &i in t {
					let v = part.geometry.positions[i as usize];
					let p = [0, 1, 2]
						.map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + m[r][3]);
					let (lo, hi) = bounds.get_or_insert((p, p));
					for k in 0..3 {
						lo[k] = lo[k].min(p[k]);
						hi[k] = hi[k].max(p[k]);
					}
				}
			}
		}
	}
	bounds
}

/// A stable 64-bit hash (FNV-1a) naming the cached files.
fn fnv1a(text: &str) -> u64 {
	text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
		(h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
	})
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;
	use crate::render::viewport::{DecalDraw, IDENTITY, Surface};
	use crate::test_support::TempDir;

	/// Cached thumbnails are named by these hashes, so they must never change.
	#[test]
	fn fnv1a_is_stable() {
		assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
		assert_eq!(fnv1a("item:abc"), 0xdaa7_b814_1855_7efc);
		assert_eq!(
			fnv1a(&Subject::Item("abc".into()).key()),
			0xdaa7_b814_1855_7efc
		);
	}

	fn geometry() -> Geometry {
		Geometry {
			positions: vec![
				[0.0, 0.0, 0.0],
				[1.0, 0.0, 0.0],
				[0.0, 2.0, 0.0],
				[0.0, 0.0, 3.0],
				// in no triangle
				[100.0, 100.0, 100.0],
			],
			uvs: None,
			normals: None,
			triangles: vec![[0, 1, 2], [0, 2, 3]],
			groups: vec![0..1, 1..2],
			group_kind: GroupKind::Lod,
		}
	}

	fn part<'a>(
		geometry: &'a Geometry,
		groups: &'a [usize],
		material: &'a Material,
		scene: &'a mut Option<crate::render::gpu::GpuScene>,
	) -> ScenePart<'a> {
		ScenePart {
			geometry,
			groups,
			transform: IDENTITY,
			material,
			colour_groups: false,
			colour: egui::Color32::WHITE,
			scene,
			moved: false,
			surface: Surface::OPAQUE,
			decal: None,
			highlight: false,
		}
	}

	#[test]
	fn drawn_bounds_of_nothing_is_none() {
		assert_eq!(drawn_bounds(&[]), None);
		let (g, material) = (geometry(), Material::default());
		let mut scene = None;
		// a group index past the geometry's groups is skipped
		assert_eq!(drawn_bounds(&[part(&g, &[5], &material, &mut scene)]), None);
	}

	#[test]
	fn drawn_bounds_covers_the_drawn_groups_moved_and_skips_decals() {
		let (g, material) = (geometry(), Material::default());
		let (mut a, mut b, mut c) = (None, None, None);
		// only group 0 (the first triangle), moved 10 along X and doubled in Y
		let mut moved = part(&g, &[0], &material, &mut a);
		moved.transform[0][3] = 10.0;
		moved.transform[1][1] = 2.0;
		// group 1 only, as it is
		let other = part(&g, &[1], &material, &mut b);
		let mut decal = part(&g, &[0, 1], &material, &mut c);
		decal.transform[0][3] = -50.0;
		decal.decal = Some(DecalDraw {
			part_from_mesh: IDENTITY,
			size: [1.0; 3],
			face: 0,
			tiles: None,
			colour: [1.0; 3],
			transparency: 0.0,
			variant: false,
			clothing: None,
		});
		let parts = [moved, other, decal];
		assert_eq!(
			drawn_bounds(&parts[..1]),
			Some(([10.0, 0.0, 0.0], [11.0, 4.0, 0.0]))
		);
		assert_eq!(
			drawn_bounds(&parts),
			Some(([0.0, 0.0, 0.0], [11.0, 4.0, 3.0]))
		);
	}

	fn touch(path: &Path, when: SystemTime) {
		let file = std::fs::File::create(path).unwrap();
		file.set_modified(when).unwrap();
	}

	#[test]
	fn is_fresh_compares_modified_times() {
		let dir = TempDir::new("thumb_fresh");
		let now = SystemTime::now();
		let (thumb, mesh, map) = (dir.join("t.png"), dir.join("a.mesh"), dir.join("a.png"));
		// no thumbnail yet
		assert!(!is_fresh(&thumb, &[]));
		touch(&thumb, now);
		touch(&mesh, now - Duration::from_secs(60));
		touch(&map, now);
		// as new as the thumbnail still counts as fresh
		assert!(is_fresh(&thumb, &[mesh.clone(), map.clone()]));
		assert!(is_fresh(&thumb, &[]));
		touch(&map, now + Duration::from_secs(60));
		assert!(!is_fresh(&thumb, &[mesh.clone(), map.clone()]));
	}

	/// Files that cannot be read are left out of the comparison rather than
	/// making the thumbnail stale (characterised as it is today).
	#[test]
	fn is_fresh_ignores_missing_files() {
		let dir = TempDir::new("thumb_missing");
		let thumb = dir.join("t.png");
		touch(&thumb, SystemTime::now());
		assert!(is_fresh(&thumb, &[dir.join("gone.mesh")]));
	}
}
