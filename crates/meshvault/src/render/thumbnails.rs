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
				let _ = std::fs::create_dir_all(&self.dir).map(|_| image.save(&job.file));
				State::Ready(texture(ctx, &job.subject, &image))
			}
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
			let maps = crate::app::item_maps(&ws.root, item);
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
			let load = crate::app::model_loader(ws, index);
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
	// as the viewer opens it: the finest LOD, or every hull
	let groups = match geometry.group_kind {
		GroupKind::Lod => vec![0],
		_ => (0..geometry.groups.len()).collect(),
	};
	let mut material = Material::default();
	if geometry.uvs.is_some() {
		for (slot, path) in maps {
			if let Ok(texture) = std::fs::read(path)
				.map_err(|e| e.to_string())
				.and_then(|b| texture::load(ctx, path, &b))
			{
				material.set(*slot, texture);
			}
		}
	}
	Ok(Prepared::Mesh {
		geometry,
		groups,
		material,
	})
}

/// Draws a thumbnail from a front three-quarter view, as the viewer opens.
fn draw(renderer: &GpuRenderer, prepared: Prepared) -> image::RgbaImage {
	let mut settings = ViewSettings {
		shading: Shading::Solid,
		cull_backfaces: false,
		show_axes: false,
		show_grid: false,
		background: [egui::Color32::BLACK; 2],
		grid_colour: egui::Color32::BLACK,
		lights: Vec::new(),
		lighting: Default::default(),
		highlight: egui::Color32::BLACK,
	};
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
