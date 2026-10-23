//! Library thumbnails: each mesh and animated model drawn once off screen
//! and kept as a small PNG under `.cache/thumbnails`, so the library can
//! show a grid of pictures. Only what the library shows is drawn, one at a
//! time, with its files read on a background thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::SystemTime;

use eframe::egui;

use crate::render::camera::Camera;
use crate::render::gpu::GpuRenderer;
use crate::render::maths::{IDENTITY, apply};
use crate::render::viewport::{self, ScenePart, Shading, ViewSettings};
use crate::scene::geometry::{self, Geometry, GroupKind};
use crate::scene::maps::Slot;
use crate::scene::model_view::ModelView;
use crate::scene::surface::{PART_COLOUR, Surface};
use crate::scene::texture::{self, Material};
use crate::workspace::view_loader::{item_maps, model_loader};
use crate::workspace::{AssetState, CACHE_DIR, Workspace, check_no_links};

/// Pixels on each side of a thumbnail.
const SIZE: u32 = 192;
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

/// A subject's state, and the workspace generation it was last checked
/// against: after the workspace changes it is checked again.
struct Entry {
	state: State,
	generation: u64,
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
	/// What it is drawn from, and the workspace generation then.
	files: Vec<PathBuf>,
	generation: u64,
	/// The [`versions`] of `files` just before they were read.
	read: Vec<Version>,
	rx: Receiver<Result<Prepared, String>>,
}

/// A file's modified time and length, or `None` if it cannot be read.
type Version = Option<(SystemTime, u64)>;

fn versions(files: &[PathBuf]) -> Vec<Version> {
	let version = |f: &PathBuf| {
		let meta = std::fs::metadata(f).ok()?;
		Some((meta.modified().ok()?, meta.len()))
	};
	files.iter().map(version).collect()
}

pub struct Thumbnails {
	root: PathBuf,
	dir: PathBuf,
	states: HashMap<Subject, Entry>,
	/// The workspace generation at the last update.
	generation: u64,
	/// Asked for this frame, in the order shown.
	wanted: Vec<Subject>,
	job: Option<Job>,
}

impl Thumbnails {
	pub fn new(root: &Path) -> Self {
		Self {
			root: root.to_path_buf(),
			dir: root.join(CACHE_DIR).join("thumbnails"),
			states: HashMap::new(),
			generation: 0,
			wanted: Vec::new(),
			job: None,
		}
	}

	/// The thumbnail of `subject` when it is ready; otherwise it is loaded
	/// or drawn over the next frames. One checked before the workspace last
	/// changed is shown while it is checked again (and redrawn if stale).
	pub fn get(&mut self, subject: &Subject) -> Option<egui::TextureHandle> {
		let Some(entry) = self.states.get(subject) else {
			self.wanted.push(subject.clone());
			return None;
		};
		if entry.generation != self.generation {
			self.wanted.push(subject.clone());
		}
		match &entry.state {
			State::Ready(texture) => Some(texture.clone()),
			State::Failed => None,
		}
	}

	/// The cached thumbnail of `subject`.
	fn file(&self, subject: &Subject) -> PathBuf {
		self.dir.join(format!("{:016x}.png", fnv1a(&subject.key())))
	}

	/// Loads or draws what was asked for this frame. Call once a frame,
	/// after the library is drawn.
	pub fn update(&mut self, ctx: &egui::Context, ws: &Workspace, renderer: Option<&GpuRenderer>) {
		let wanted = std::mem::take(&mut self.wanted);
		self.generation = ws.generation();
		self.finish_job(ctx, |prepared| match renderer {
			Some(renderer) => draw(renderer, prepared),
			None => Err("no GPU".into()),
		});
		let mut loads = 0;
		let mut waiting = false;
		for subject in wanted {
			let checked = self
				.states
				.get(&subject)
				.is_some_and(|e| e.generation == self.generation);
			if checked || self.job.as_ref().is_some_and(|j| j.subject == subject) {
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
			let file = self.file(&subject);
			let generation = self.generation;
			if is_fresh(&file, &files) {
				// what is shown is still what the cache holds
				if let Some(entry) = self.states.get_mut(&subject)
					&& matches!(entry.state, State::Ready(_))
				{
					entry.generation = generation;
					continue;
				}
				if let Some(texture) = load_png(ctx, &file, &subject) {
					let state = State::Ready(texture);
					self.states.insert(subject, Entry { state, generation });
					continue;
				}
			}
			match renderer {
				Some(_) if self.job.is_none() => {
					let read = versions(&files);
					let rx = spawn(ctx, ws, source);
					self.job = Some(Job {
						subject,
						file,
						files,
						generation,
						read,
						rx,
					});
				}
				Some(_) => waiting = true,
				// without a GPU nothing is drawn
				None => {
					let state = State::Failed;
					self.states.insert(subject, Entry { state, generation });
				}
			}
		}
		if waiting || self.job.is_some() {
			ctx.request_repaint();
		}
	}

	/// Takes the job's result once it is read, drawing it with `draw`. The
	/// workspace generation must already be noted in `self.generation`.
	fn finish_job(
		&mut self,
		ctx: &egui::Context,
		draw: impl FnOnce(Prepared) -> Result<image::RgbaImage, String>,
	) {
		let Some(job) = &self.job else { return };
		let result = match job.rx.try_recv() {
			Ok(result) => result,
			Err(TryRecvError::Empty) => return,
			Err(TryRecvError::Disconnected) => Err("the reader stopped".into()),
		};
		let Some(job) = self.job.take() else { return };
		// the workspace changed (a re-import, say) or a file changed while it
		// was read: what was read may be out of date, and saved now the
		// picture would look newer than the change, so it is dropped and
		// drawn again. A file's length and modified time can stay the same
		// through a re-import, so they alone are not enough; they still catch
		// changes made outside the app.
		if job.generation != self.generation || versions(&job.files) != job.read {
			return;
		}
		let state = match result {
			Ok(prepared) => {
				match keep(&self.root, &self.dir, &job.file, &job.files, draw(prepared)) {
					Some(image) => State::Ready(texture(ctx, &job.subject, &image)),
					None => State::Failed,
				}
			}
			// the reader's (or the GPU's) reason is not shown anywhere
			Err(_) => State::Failed,
		};
		let generation = job.generation;
		self.states.insert(job.subject, Entry { state, generation });
	}
}

/// The thumbnail `drawn` from `files`, saved to `file` in `dir` for later
/// runs, with the list of `files` beside it. One the GPU could not hand
/// back is neither saved nor shown, so it is drawn again rather than kept
/// as a black square. Nothing is saved through a link in the workspace
/// at `root` (see [`check_no_links`]).
fn keep(
	root: &Path,
	dir: &Path,
	file: &Path,
	files: &[PathBuf],
	drawn: Result<image::RgbaImage, String>,
) -> Option<image::RgbaImage> {
	let image = drawn.ok()?;
	// a thumbnail that cannot be saved is still shown (and drawn again
	// next time); one whose list cannot be saved is drawn again next run
	let unlinked = |p: &Path| {
		p.strip_prefix(root)
			.is_ok_and(|rel| check_no_links(root, rel).is_ok())
	};
	if unlinked(file)
		&& unlinked(&drawn_from(file))
		&& std::fs::create_dir_all(dir).is_ok()
		&& image.save(file).is_ok()
	{
		let _ = std::fs::write(drawn_from(file), list(files));
	}
	Some(image)
}

/// The file beside the thumbnail `file` listing what it was drawn from.
fn drawn_from(file: &Path) -> PathBuf {
	file.with_extension("from")
}

fn list(files: &[PathBuf]) -> String {
	let lines: Vec<_> = files.iter().map(|f| f.to_string_lossy()).collect();
	lines.join("\n")
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
					AssetState::Done { path } => {
						files.push(ws.root.join(path));
						files.extend(item_maps(&ws.root, item).into_iter().map(|(_, p)| p));
					}
					AssetState::Failed { .. } => {}
				}
			}
			// maps and images still downloading are drawn without; the
			// thumbnail is drawn again once they are here
			let images = model.images.values().filter_map(AssetState::done_path);
			files.extend(images.map(|p| ws.root.join(p)));
			Some((Source::Model(index), files))
		}
	}
}

/// Whether the thumbnail at `file` was drawn from just these `files` and
/// is newer than each of them.
fn is_fresh(file: &Path, files: &[PathBuf]) -> bool {
	if std::fs::read_to_string(drawn_from(file)).ok() != Some(list(files)) {
		return false;
	}
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
	source: Source,
) -> Receiver<Result<Prepared, String>> {
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
			// `source` found the index in this same workspace
			let load = model_loader(ws, index).expect("the model is in the workspace");
			std::thread::spawn(move || {
				let _ = tx.send(load(&ctx).map(|m| Prepared::Model(Box::new(m))));
				ctx.request_repaint();
			});
		}
	}
	rx
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
fn draw(renderer: &GpuRenderer, prepared: Prepared) -> Result<image::RgbaImage, String> {
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
				transform: IDENTITY,
				material: &material,
				colour_groups: geometry.group_kind == GroupKind::Hull,
				colour: PART_COLOUR,
				scene: &mut scene,
				moved: false,
				surface: Surface::OPAQUE,
				decal: None,
				highlight: false,
			}];
			viewport::render_image(&mut parts, &camera, &settings, renderer, big)?
		}
		Prepared::Model(mut model) => {
			settings.lights = model.scene_lights();
			let mut parts = model.scene_parts(None);
			// framed on what is drawn, in the pose it is drawn in
			let mut camera = Camera::framing_bounds(drawn_bounds(&parts));
			camera.distance *= ZOOM;
			viewport::render_image(&mut parts, &camera, &settings, renderer, big)?
		}
	};
	Ok(image::imageops::resize(
		&image,
		SIZE,
		SIZE,
		image::imageops::FilterType::Triangle,
	))
}

/// The box around every drawn vertex (decals lie on their parts).
fn drawn_bounds(parts: &[ScenePart]) -> Option<([f32; 3], [f32; 3])> {
	let mut bounds: Option<([f32; 3], [f32; 3])> = None;
	for part in parts.iter().filter(|p| p.decal.is_none()) {
		for &g in part.groups {
			let Some(range) = part.geometry.groups.get(g) else {
				continue;
			};
			for t in &part.geometry.triangles[range.clone()] {
				for &i in t {
					let p = apply(&part.transform, part.geometry.positions[i as usize]);
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
	use crate::render::maths::IDENTITY;
	use crate::scene::maps::MapKind;
	use crate::scene::surface::{DecalDraw, Surface};
	use crate::studio::export_file::Origin;
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

	/// A workspace in a temp dir with the mesh `A` (its mesh and colour map
	/// in the given states) and the model `M` using it, with one decal
	/// image in `image`'s state.
	fn workspace(mesh: AssetState, map: AssetState, image: AssetState) -> (TempDir, Workspace) {
		let dir = TempDir::new("thumb_ws");
		let mut ws = Workspace::create(&dir, "Test").unwrap();
		let item = serde_json::from_value(serde_json::json!({
			"name": "A",
			"folder": [],
			"mesh_id": 1,
			"stem": "A",
			"mesh": mesh,
			"maps": { "color": { "id": 2, "state": map } },
		}))
		.unwrap();
		let model = serde_json::from_value(serde_json::json!({
			"name": "M",
			"path": "Workspace.M",
			"folder": [],
			"file": "Rigs/M.rig.json",
			"meshes": { "1": "A" },
			"parts": 1,
			"clips": 0,
			"playable_clips": 0,
			"images": { "7": image },
		}))
		.unwrap();
		ws.set_contents(vec![item], vec![model]);
		(dir, ws)
	}

	fn done(path: &str) -> AssetState {
		AssetState::Done { path: path.into() }
	}

	/// A model's thumbnail shows its meshes' maps and its decal images too,
	/// so they are among the files that make it stale (B30).
	#[test]
	fn a_model_depends_on_its_meshes_maps_and_images() {
		let (_dir, ws) = workspace(done("A.mesh"), done("A.png"), done("Images/7.png"));
		let (_, files) = source(&ws, &Subject::Model("Workspace.M".into())).unwrap();
		let expected: Vec<PathBuf> = ["Rigs/M.rig.json", "A.mesh", "A.png", "Images/7.png"]
			.iter()
			.map(|f| ws.root.join(f))
			.collect();
		assert_eq!(files, expected);
		// maps and images not downloaded yet are drawn without, not waited for
		let (_dir, ws) = workspace(done("A.mesh"), AssetState::Pending, AssetState::Pending);
		let (_, files) = source(&ws, &Subject::Model("Workspace.M".into())).unwrap();
		assert_eq!(
			files,
			[ws.root.join("Rigs/M.rig.json"), ws.root.join("A.mesh")]
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

	/// A drawing the GPU could not hand back is not cached (B31).
	#[test]
	fn only_drawn_thumbnails_are_kept() {
		let dir = TempDir::new("thumb_keep");
		let (cache, file) = (dir.join("cache"), dir.join("cache").join("t.png"));
		let files = [dir.join("a.mesh")];
		assert_eq!(keep(&dir, &cache, &file, &files, Err("lost".into())), None);
		assert!(!file.exists() && !drawn_from(&file).exists());
		let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
		assert_eq!(
			keep(&dir, &cache, &file, &files, Ok(image.clone())),
			Some(image.clone())
		);
		assert_eq!(image::open(&file).unwrap().to_rgba8(), image);
		assert!(is_fresh(&file, &files));
	}

	/// A thumbnail is shown but not saved when its folder in the workspace
	/// links elsewhere (N01).
	#[test]
	fn thumbnails_are_not_saved_through_a_link() {
		let dir = TempDir::new("thumb_linked");
		let (root, outside) = (dir.join("ws"), dir.join("outside"));
		std::fs::create_dir_all(root.join(CACHE_DIR)).unwrap();
		std::fs::create_dir_all(&outside).unwrap();
		let cache = root.join(CACHE_DIR).join("thumbnails");
		if !crate::workspace::junction(&cache, &outside) {
			return;
		}
		let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
		let kept = keep(&root, &cache, &cache.join("t.png"), &[], Ok(image.clone()));
		assert_eq!(kept, Some(image));
		assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
	}

	/// A thumbnail drawn before a map or image arrived (or after one went)
	/// is stale even though no file it was drawn from changed (B30).
	#[test]
	fn is_fresh_needs_the_files_it_was_drawn_from() {
		let dir = TempDir::new("thumb_from");
		let (thumb, mesh, map) = (dir.join("t.png"), dir.join("a.mesh"), dir.join("a.png"));
		touch(&mesh, SystemTime::now() - Duration::from_secs(60));
		touch(&map, SystemTime::now() - Duration::from_secs(60));
		touch(&thumb, SystemTime::now());
		let (just_mesh, both) = ([mesh.clone()], [mesh.clone(), map.clone()]);
		// a thumbnail cached before the list was kept is drawn again once
		assert!(!is_fresh(&thumb, &just_mesh));
		std::fs::write(drawn_from(&thumb), list(&just_mesh)).unwrap();
		assert!(is_fresh(&thumb, &just_mesh));
		assert!(!is_fresh(&thumb, &both));
		assert!(!is_fresh(&thumb, &[]));
	}

	/// A thumbnail in memory is checked again when the workspace changes,
	/// and kept only while it is fresh: once the mesh's colour map has
	/// arrived, it is drawn again (here, without a GPU, it fails) instead
	/// of staying as it was for the rest of the session (B30).
	#[test]
	fn a_shown_thumbnail_is_checked_again_when_the_workspace_changes() {
		let (dir, mut ws) = workspace(done("A.mesh"), AssetState::Pending, AssetState::Pending);
		touch(
			&dir.join("A.mesh"),
			SystemTime::now() - Duration::from_secs(60),
		);
		let ctx = egui::Context::default();
		let subject = Subject::Item("A".into());
		let mut thumbnails = Thumbnails::new(&ws.root);
		let (_, files) = source(&ws, &subject).unwrap();
		let picture = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
		let cache = thumbnails.dir.clone();
		keep(
			&dir,
			&cache,
			&thumbnails.file(&subject),
			&files,
			Ok(picture),
		);
		assert!(thumbnails.get(&subject).is_none());
		thumbnails.update(&ctx, &ws, None);
		let shown = thumbnails.get(&subject).expect("read from the cache");
		// a change that leaves its files as they were keeps it
		ws.add([]);
		assert!(thumbnails.get(&subject).is_some());
		thumbnails.update(&ctx, &ws, None);
		assert_eq!(thumbnails.get(&subject).map(|t| t.id()), Some(shown.id()));
		// the colour map arrives
		touch(
			&dir.join("A.png"),
			SystemTime::now() - Duration::from_secs(60),
		);
		let mut item = ws.items()[0].clone();
		item.maps.get_mut(&MapKind::Color).unwrap().state = done("A.png");
		let models = ws.models().to_vec();
		ws.set_contents(vec![item], models);
		ws.add([]);
		// still shown until it has been checked
		assert!(thumbnails.get(&subject).is_some());
		thumbnails.update(&ctx, &ws, None);
		assert!(thumbnails.get(&subject).is_none());
	}

	/// A job whose files changed after they were read is dropped, not
	/// drawn: saving the old picture after the change would make it look
	/// fresh for good. Bug: a model re-imported while its thumbnail was
	/// being read kept the old picture, even after a restart.
	#[test]
	fn a_job_read_before_its_files_changed_is_not_kept() {
		let (dir, ws) = workspace(done("A.mesh"), AssetState::Pending, AssetState::Pending);
		let rig = dir.join("Rigs").join("M.rig.json");
		std::fs::create_dir_all(rig.parent().unwrap()).unwrap();
		let now = SystemTime::now();
		let ctx = egui::Context::default();
		let subject = Subject::Model("Workspace.M".into());
		let picture = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
		let prepared = || {
			let geometry = geometry();
			let groups = geometry.default_groups();
			Ok(Prepared::Mesh {
				geometry,
				groups,
				material: Material::default(),
			})
		};
		// a job started with the rig as it was, and the result it sends
		let start = |thumbnails: &mut Thumbnails| {
			let (tx, rx) = std::sync::mpsc::channel();
			let files = vec![rig.clone()];
			thumbnails.job = Some(Job {
				subject: subject.clone(),
				file: thumbnails.file(&subject),
				read: versions(&files),
				files,
				generation: ws.generation(),
				rx,
			});
			tx
		};
		std::fs::write(&rig, "rig A").unwrap();
		std::fs::File::options()
			.write(true)
			.open(&rig)
			.unwrap()
			.set_modified(now - Duration::from_secs(60))
			.unwrap();
		let mut thumbnails = Thumbnails::new(&ws.root);
		let file = thumbnails.file(&subject);
		let tx = start(&mut thumbnails);
		// re-imported (same path, a little later, still older than any new
		// thumbnail) before the old result is taken
		std::fs::write(&rig, "rig B, moved").unwrap();
		std::fs::File::options()
			.write(true)
			.open(&rig)
			.unwrap()
			.set_modified(now - Duration::from_secs(30))
			.unwrap();
		tx.send(prepared()).unwrap();
		thumbnails.finish_job(&ctx, |_| Ok(picture.clone()));
		assert!(thumbnails.job.is_none());
		assert!(!is_fresh(&file, std::slice::from_ref(&rig)));
		assert!(!file.exists() && !drawn_from(&file).exists());
		assert!(thumbnails.get(&subject).is_none());
		// a job read after the change is kept, and fresh
		let tx = start(&mut thumbnails);
		tx.send(prepared()).unwrap();
		thumbnails.finish_job(&ctx, |_| Ok(picture.clone()));
		assert!(thumbnails.get(&subject).is_some());
		assert!(is_fresh(&file, std::slice::from_ref(&rig)));
	}

	/// A model re-imported while its thumbnail is read is drawn again even
	/// when its rig file keeps its length and modified time (the same size
	/// of change, written within the file system's time resolution): the
	/// import changed the workspace, so what was read may be out of date.
	#[test]
	fn a_job_read_before_a_reimport_is_not_kept_even_if_the_rig_looks_the_same() {
		let dir = TempDir::new("thumb_reimport");
		let mut ws = Workspace::create(&dir.join("ws"), "Test").unwrap();
		let rig_model = |size| crate::anim::schema::AnimatedModel {
			name: "M".into(),
			path: "Workspace/M".into(),
			parts: vec![crate::anim::schema::ModelPart {
				size,
				..Default::default()
			}],
			..Default::default()
		};
		ws.add_model(&rig_model([1.0, 2.0, 3.0]), &[], &Origin::default())
			.unwrap();
		let rig = ws.root.join(&ws.models()[0].file);
		let then = SystemTime::now() - Duration::from_secs(60);
		let set_modified = || {
			let file = std::fs::File::options().write(true).open(&rig).unwrap();
			file.set_modified(then).unwrap();
		};
		set_modified();
		let ctx = egui::Context::default();
		let subject = Subject::Model("Workspace/M".into());
		let picture = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
		let mut thumbnails = Thumbnails::new(&ws.root);
		let file = thumbnails.file(&subject);
		// a job started on the rig as it is, and the result it sends
		let start = |thumbnails: &mut Thumbnails, ws: &Workspace| {
			let (tx, rx) = std::sync::mpsc::channel();
			let (_, files) = source(ws, &subject).unwrap();
			thumbnails.job = Some(Job {
				subject: subject.clone(),
				file: thumbnails.file(&subject),
				read: versions(&files),
				files,
				generation: ws.generation(),
				rx,
			});
			let geometry = geometry();
			let groups = geometry.default_groups();
			tx.send(Ok(Prepared::Mesh {
				geometry,
				groups,
				material: Material::default(),
			}))
			.unwrap();
		};
		// the result is taken as `update` takes it, after noting the generation
		let finish = |thumbnails: &mut Thumbnails, ws: &Workspace| {
			thumbnails.generation = ws.generation();
			thumbnails.finish_job(&ctx, |_| Ok(picture.clone()));
			assert!(thumbnails.job.is_none());
		};
		start(&mut thumbnails, &ws);
		let read = thumbnails.job.as_ref().unwrap().read.clone();
		let old = std::fs::read(&rig).unwrap();
		// re-imported with the part's size changed, before the result is taken
		ws.add_model(&rig_model([3.0, 2.0, 1.0]), &[], &Origin::default())
			.unwrap();
		set_modified();
		assert_ne!(std::fs::read(&rig).unwrap(), old);
		assert_eq!(versions(std::slice::from_ref(&rig)), read);
		finish(&mut thumbnails, &ws);
		assert!(!file.exists() && !drawn_from(&file).exists());
		assert!(thumbnails.get(&subject).is_none());
		// a job read after the re-import is kept, and fresh
		start(&mut thumbnails, &ws);
		finish(&mut thumbnails, &ws);
		assert!(thumbnails.get(&subject).is_some());
		assert!(is_fresh(&file, std::slice::from_ref(&rig)));
	}

	#[test]
	fn is_fresh_compares_modified_times() {
		let dir = TempDir::new("thumb_fresh");
		let now = SystemTime::now();
		let (thumb, mesh, map) = (dir.join("t.png"), dir.join("a.mesh"), dir.join("a.png"));
		let files = [mesh.clone(), map.clone()];
		std::fs::write(drawn_from(&thumb), list(&files)).unwrap();
		// no thumbnail yet
		assert!(!is_fresh(&thumb, &files));
		touch(&thumb, now);
		touch(&mesh, now - Duration::from_secs(60));
		touch(&map, now);
		// as new as the thumbnail still counts as fresh
		assert!(is_fresh(&thumb, &files));
		touch(&map, now + Duration::from_secs(60));
		assert!(!is_fresh(&thumb, &files));
	}

	/// Files that cannot be read are left out of the comparison rather than
	/// making the thumbnail stale (characterised as it is today).
	#[test]
	fn is_fresh_ignores_missing_files() {
		let dir = TempDir::new("thumb_missing");
		let thumb = dir.join("t.png");
		touch(&thumb, SystemTime::now());
		let files = [dir.join("gone.mesh")];
		std::fs::write(drawn_from(&thumb), list(&files)).unwrap();
		assert!(is_fresh(&thumb, &files));
	}
}
