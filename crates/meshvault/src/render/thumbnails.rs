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
use crate::studio::export_file::read_limited;
use crate::workspace::view_loader::{item_maps, model_loader};
use crate::workspace::{AssetState, CACHE_DIR, Workspace, replace_file, saved_file};

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
			let Some(found) = source(ws, &subject) else {
				continue;
			};
			let generation = self.generation;
			// behind a link: not read, so the tile keeps its icon
			let Ok((source, files)) = found else {
				let state = State::Failed;
				self.states.insert(subject, Entry { state, generation });
				continue;
			};
			if loads == LOADS_PER_FRAME {
				waiting = true;
				continue;
			}
			loads += 1;
			let file = self.file(&subject);
			// nor is a cached thumbnail, or its list, read through a link
			let cached = unlinked(&self.root, &file) && unlinked(&self.root, &drawn_from(&file));
			if cached && is_fresh(&file, &files) {
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
/// at `root` (see [`replace_file`]).
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
	let save = |p: &Path, bytes: &[u8]| {
		p.strip_prefix(root)
			.is_ok_and(|rel| replace_file(root, rel, bytes).is_ok())
	};
	let mut png = Vec::new();
	let encoded = image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);
	if encoded.is_ok()
		&& unlinked(root, &drawn_from(file))
		&& std::fs::create_dir_all(dir).is_ok()
		&& save(file, &png)
	{
		save(&drawn_from(file), list(files).as_bytes());
	}
	Some(image)
}

/// Whether `file`, a path under the workspace at `root`, can be read or
/// looked at: it is not behind a link there (see [`saved_file`]).
fn unlinked(root: &Path, file: &Path) -> bool {
	file.strip_prefix(root)
		.is_ok_and(|rel| saved_file(root, rel).is_ok())
}

/// The file beside the thumbnail `file` listing what it was drawn from.
fn drawn_from(file: &Path) -> PathBuf {
	file.with_extension("from")
}

/// The longest list of what a thumbnail was drawn from that is read: a
/// model's meshes, maps and images, each a path.
const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;

/// The list saved in `file`, if it is no longer than `limit` bytes.
fn read_list(file: &Path, limit: usize) -> Option<String> {
	let bytes = read_limited(file, limit).ok()?;
	String::from_utf8(bytes).ok()
}

fn list(files: &[PathBuf]) -> String {
	let lines: Vec<_> = files.iter().map(|f| f.to_string_lossy()).collect();
	lines.join("\n")
}

/// What `subject` is drawn from, and the files whose changes make its
/// thumbnail stale. `None` while it is not downloaded. Nothing behind a
/// link in the workspace is read or looked at (see
/// [`Workspace::saved_file`]): a mesh there, or a model's rig, is refused
/// with the reason; a model's mesh there, and maps and images there, are
/// drawn without, as the viewer shows them.
fn source(ws: &Workspace, subject: &Subject) -> Option<Result<(Source, Vec<PathBuf>), String>> {
	match subject {
		Subject::Item(stem) => {
			let item = ws.item_by_stem(stem)?;
			let AssetState::Done { path } = &item.mesh else {
				return None;
			};
			let mesh = match ws.saved_file(path) {
				Ok(mesh) => mesh,
				Err(e) => return Some(Err(e)),
			};
			let maps = item_maps(&ws.root, item);
			let mut files = vec![mesh.clone()];
			files.extend(maps.iter().map(|(_, p)| p.clone()));
			Some(Ok((Source::Mesh { mesh, maps }, files)))
		}
		Subject::Model(path) => {
			let index = ws.models().iter().position(|m| &m.path == path)?;
			let model = &ws.models()[index];
			let mut files = match ws.saved_file(&model.file) {
				Ok(rig) => vec![rig],
				Err(e) => return Some(Err(e)),
			};
			// a mesh many parts use is listed once
			let mut seen = std::collections::HashSet::new();
			for stem in model.meshes.values().filter(|s| seen.insert(s.as_str())) {
				// a mesh removed from the workspace, or failed, is drawn without
				let Some(item) = ws.item_by_stem(stem) else {
					continue;
				};
				match &item.mesh {
					AssetState::Pending => return None,
					AssetState::Done { path } => {
						if let Ok(mesh) = ws.saved_file(path) {
							files.push(mesh);
							files.extend(item_maps(&ws.root, item).into_iter().map(|(_, p)| p));
						}
					}
					AssetState::Failed { .. } => {}
				}
			}
			// maps and images still downloading are drawn without; the
			// thumbnail is drawn again once they are here
			let images = model.images.values().filter_map(AssetState::done_path);
			files.extend(images.filter_map(|p| ws.saved_file(p).ok()));
			Some(Ok((Source::Model(index), files)))
		}
	}
}

/// Whether the thumbnail at `file` was drawn from just these `files` and
/// is newer than each of them.
fn is_fresh(file: &Path, files: &[PathBuf]) -> bool {
	if read_list(&drawn_from(file), MAX_LIST_BYTES) != Some(list(files)) {
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
	let image = read_thumbnail(file)?;
	Some(texture(ctx, subject, &image))
}

/// The longest cached thumbnail read. One is a [`SIZE`]-pixel square PNG,
/// a fraction of this.
const MAX_PNG_BYTES: usize = 1024 * 1024;

/// The cached thumbnail in `file`, if it is a PNG of just the size the app
/// draws them. The cache can come with a shared workspace, so anything
/// else (a picture wider than the GPU takes, say) is a miss and is drawn
/// again; it is refused by its header, before its pixels are decoded.
fn read_thumbnail(file: &Path) -> Option<image::RgbaImage> {
	let bytes = read_limited(file, MAX_PNG_BYTES).ok()?;
	let mut reader =
		image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
	let mut limits = image::Limits::default();
	limits.max_image_width = Some(SIZE);
	limits.max_image_height = Some(SIZE);
	limits.max_alloc = Some(4 * u64::from(SIZE * SIZE) * 4);
	reader.limits(limits);
	let image = reader.decode().ok()?;
	if image.width() != SIZE || image.height() != SIZE {
		return None;
	}
	Some(image.to_rgba8())
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
	let bytes = crate::workspace::read_asset(mesh).map_err(|e| e.to_string())?;
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
		let (_, files) = source(&ws, &Subject::Model("Workspace.M".into()))
			.unwrap()
			.unwrap();
		let expected: Vec<PathBuf> = ["Rigs/M.rig.json", "A.mesh", "A.png", "Images/7.png"]
			.iter()
			.map(|f| ws.root.join(f))
			.collect();
		assert_eq!(files, expected);
		// maps and images not downloaded yet are drawn without, not waited for
		let (_dir, ws) = workspace(done("A.mesh"), AssetState::Pending, AssetState::Pending);
		let (_, files) = source(&ws, &Subject::Model("Workspace.M".into()))
			.unwrap()
			.unwrap();
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
		let image = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([1, 2, 3, 255]));
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
		let image = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([1, 2, 3, 255]));
		let kept = keep(&root, &cache, &cache.join("t.png"), &[], Ok(image.clone()));
		assert_eq!(kept, Some(image));
		assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
	}

	/// R3-04: what a model's thumbnail is drawn from is found through the
	/// workspace's stem index, not by looking through every item for each
	/// mesh ref (on the UI thread, every frame it is wanted); a mesh used by
	/// many refs is read and listed once.
	#[test]
	fn a_models_sources_are_found_with_linear_work() {
		let (_dir, mut ws) = workspace(done("A.mesh"), AssetState::Pending, AssetState::Pending);
		const N: usize = 5_000;
		let mut items = ws.items().to_vec();
		let mut models = ws.models().to_vec();
		for i in 0..N {
			let mut item = items[0].clone();
			item.stem = format!("I{i}");
			items.push(item);
			models[0].meshes.insert(i as u32 + 10, "Missing".into());
		}
		models[0].meshes.insert(2, "A".into());
		ws.set_contents(items, models);
		crate::workspace::ITEMS_VISITED.with(|n| n.set(0));
		let (_, files) = source(&ws, &Subject::Model("Workspace.M".into()))
			.unwrap()
			.unwrap();
		let visited = crate::workspace::ITEMS_VISITED.with(|n| n.get());
		assert!(visited <= 2 * (N + 1), "{visited} items looked at");
		assert_eq!(
			files,
			[ws.root.join("Rigs/M.rig.json"), ws.root.join("A.mesh")]
		);
	}

	/// R3-01: a thumbnail and its list are saved under fresh temporary
	/// names and renamed into place, so a link at any of those names is
	/// never written through.
	#[test]
	fn thumbnails_are_never_saved_through_a_file_link() {
		use crate::workspace::{file_link, set_temp_tags};
		let dir = TempDir::new("thumb_file_link");
		let root = dir.join("ws");
		let cache = root.join(CACHE_DIR).join("thumbnails");
		std::fs::create_dir_all(&cache).unwrap();
		let outside = dir.join("outside.txt");
		std::fs::write(&outside, b"keep me").unwrap();
		let file = cache.join("t.png");
		let kind = file_link(&file, &outside);
		for name in ["t.from", "t.png.a.tmp", "t.from.c.tmp"] {
			file_link(&cache.join(name), &outside);
		}
		let image = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([1, 2, 3, 255]));
		set_temp_tags(Some(&["a", "b", "c", "d"]));
		let kept = keep(&root, &cache, &file, &[], Ok(image.clone()));
		set_temp_tags(None);
		assert_eq!(kept, Some(image.clone()));
		assert_eq!(std::fs::read(&outside).unwrap(), b"keep me", "{kind}");
		if kind == "hard link" {
			assert_eq!(read_thumbnail(&file), Some(image));
			assert!(is_fresh(&file, &[]));
		}
	}

	/// R3-02: a cached thumbnail is read only at the size the app draws
	/// them. A very wide one (here 100,000 by 1, a few hundred bytes) was
	/// handed to the GPU as it was, larger than any texture it allows; now
	/// it is a cache miss and drawn again. One of the right size still
	/// loads.
	#[test]
	fn a_cached_thumbnail_of_another_size_is_not_loaded() {
		let (dir, ws) = workspace(done("A.mesh"), AssetState::Pending, AssetState::Pending);
		touch(
			&dir.join("A.mesh"),
			SystemTime::now() - Duration::from_secs(60),
		);
		let subject = Subject::Item("A".into());
		let mut thumbnails = Thumbnails::new(&ws.root);
		let (_, files) = source(&ws, &subject).unwrap().unwrap();
		let file = thumbnails.file(&subject);
		let wide = image::RgbaImage::new(100_000, 1);
		let right = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([1, 2, 3, 255]));
		keep(&dir, &thumbnails.dir, &file, &files, Ok(wide));
		assert!(is_fresh(&file, &files));
		assert!(std::fs::metadata(&file).unwrap().len() < 4096);
		assert_eq!(read_thumbnail(&file), None);
		let ctx = egui::Context::default();
		thumbnails.get(&subject);
		thumbnails.update(&ctx, &ws, None);
		assert!(thumbnails.get(&subject).is_none());

		keep(&dir, &thumbnails.dir, &file, &files, Ok(right.clone()));
		assert_eq!(read_thumbnail(&file), Some(right));
		let mut thumbnails = Thumbnails::new(&ws.root);
		thumbnails.get(&subject);
		thumbnails.update(&ctx, &ws, None);
		assert!(thumbnails.get(&subject).is_some());
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
		let (_, files) = source(&ws, &subject).unwrap().unwrap();
		let picture = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([1, 2, 3, 255]));
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
		let picture = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([1, 2, 3, 255]));
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
		let picture = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([1, 2, 3, 255]));
		let mut thumbnails = Thumbnails::new(&ws.root);
		let file = thumbnails.file(&subject);
		// a job started on the rig as it is, and the result it sends
		let start = |thumbnails: &mut Thumbnails, ws: &Workspace| {
			let (tx, rx) = std::sync::mpsc::channel();
			let (_, files) = source(ws, &subject).unwrap().unwrap();
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
	/// N02: a list of what a thumbnail was drawn from that is longer than
	/// its limit is not read, so the thumbnail is drawn again.
	#[test]
	fn an_oversized_list_is_not_read() {
		let dir = TempDir::new("thumb_list_limit");
		let file = dir.join("t.from");
		std::fs::write(&file, "a.mesh\nb.png").unwrap();
		assert_eq!(read_list(&file, 12).as_deref(), Some("a.mesh\nb.png"));
		assert_eq!(read_list(&file, 11), None);
	}

	/// R4-03: a mesh, map, rig or image behind a link below the
	/// workspace's own folders was looked at and read for its thumbnail.
	/// Now a mesh's thumbnail is refused, and a model's is drawn without
	/// what is behind a link, or refused if its rig is.
	#[test]
	fn thumbnails_are_not_drawn_from_behind_a_link() {
		let (dir, mut ws) = workspace(
			done("Models/External/A.mesh"),
			done("Models/External/A.png"),
			done("Images/External/7.png"),
		);
		let outside = dir.join("outside");
		std::fs::create_dir_all(&outside).unwrap();
		for folder in ["Models", "Rigs", "Images"] {
			std::fs::create_dir_all(dir.join(folder)).unwrap();
			if !crate::workspace::junction(&dir.join(folder).join("External"), &outside) {
				return;
			}
		}
		let Some(Err(error)) = source(&ws, &Subject::Item("A".into())) else {
			panic!("a mesh behind a link is drawn from");
		};
		assert!(
			error.starts_with("Models/External in the workspace is a link"),
			"{error}"
		);
		let model = Subject::Model("Workspace.M".into());
		let (_, files) = source(&ws, &model).unwrap().unwrap();
		assert_eq!(files, [ws.root.join("Rigs/M.rig.json")]);
		let mut models = ws.models().to_vec();
		models[0].file = "Rigs/External/M.rig.json".into();
		ws.set_contents(ws.items().to_vec(), models);
		assert!(matches!(source(&ws, &model), Some(Err(_))));
	}

	/// R4-03: a cached thumbnail and its list were read through a link (a
	/// junction for `.cache/thumbnails` here; Workspace::open refuses that
	/// one, but not a symbolic link at a thumbnail's own name, which needs
	/// rights to make that tests may lack). Now it is a miss, here failing
	/// without a GPU, and nothing behind the link is read.
	#[test]
	fn a_cached_thumbnail_behind_a_link_is_not_read() {
		let (dir, ws) = workspace(done("A.mesh"), AssetState::Pending, AssetState::Pending);
		touch(
			&dir.join("A.mesh"),
			SystemTime::now() - Duration::from_secs(60),
		);
		let subject = Subject::Item("A".into());
		let mut thumbnails = Thumbnails::new(&ws.root);
		let (_, files) = source(&ws, &subject).unwrap().unwrap();
		let outside = TempDir::new("thumb_outside");
		let file = thumbnails.file(&subject);
		let name = |p: &Path| outside.join(p.file_name().unwrap());
		let picture = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([1, 2, 3, 255]));
		picture.save(name(&file)).unwrap();
		std::fs::write(name(&drawn_from(&file)), list(&files)).unwrap();
		let _ = std::fs::remove_dir_all(&thumbnails.dir);
		if !crate::workspace::junction(&thumbnails.dir, &outside) {
			return;
		}
		assert!(is_fresh(&file, &files), "the cache is not set up as fresh");
		let ctx = egui::Context::default();
		thumbnails.get(&subject);
		thumbnails.update(&ctx, &ws, None);
		assert!(
			thumbnails.get(&subject).is_none(),
			"the thumbnail behind the link was read"
		);
	}
}
