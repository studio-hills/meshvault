//! Drawing a model's effects on the GPU: each sprite and ribbon of an
//! [`EffectFrame`] turned to face the camera, sorted from the farthest,
//! and drawn after the scene with depth tested but not written, so the
//! scene hides effects behind it and effects never hide each other.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use eframe::egui_wgpu::wgpu;
use wgpu::util::DeviceExt;

use super::gpu::{DEPTH_FORMAT, MSAA_SAMPLES, upload_texture};
use crate::anim::math::{V3, cross, dot};
use crate::scene::effects::{Builtin, EffectFrame, EffectTexture, Facing, Ribbon, Sprite};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct EffectVertex {
	pos: [f32; 3],
	emission: f32,
	uv: [f32; 2],
	color: [f32; 4],
}

/// [`EffectVertex`]'s fields as the shader reads them, in order.
const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
	0 => Float32x3, 1 => Float32, 2 => Float32x2, 3 => Float32x4
];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EffectUniforms {
	view_proj: [[f32; 4]; 4],
	params: [f32; 4],
}

/// A texture an effect draws with, as the GPU cache knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
	Plain,
	Builtin(Builtin),
	/// A downloaded image, by the address of its pixels (kept alive by
	/// the cache).
	Image(usize),
}

fn key(texture: &EffectTexture) -> Key {
	match texture {
		EffectTexture::Plain => Key::Plain,
		EffectTexture::Builtin(b) => Key::Builtin(*b),
		EffectTexture::Image(pixels) => Key::Image(Arc::as_ptr(pixels) as usize),
	}
}

/// Two triangles of one sprite or ribbon segment, how far it is from the
/// camera, and its texture.
struct Quad<'a> {
	distance: f32,
	texture: &'a EffectTexture,
	vertices: [EffectVertex; 6],
}

/// The camera, as the effects need it.
#[derive(Clone, Copy, Debug)]
pub(super) struct View {
	pub eye: V3,
	pub right: V3,
	pub up: V3,
}

/// Every sprite and ribbon of `frame` as triangles facing the camera,
/// sorted from the farthest, with the vertex range of each run that shares
/// a texture.
pub(super) fn triangles(
	frame: &EffectFrame,
	view: View,
) -> (Vec<EffectVertex>, Vec<(&EffectTexture, Range<u32>)>) {
	let mut quads: Vec<Quad> = Vec::new();
	for sprite in &frame.sprites {
		quads.extend(sprite_quad(sprite, view));
	}
	for ribbon in &frame.ribbons {
		ribbon_quads(ribbon, view, &mut quads);
	}
	quads.sort_by(|a, b| b.distance.total_cmp(&a.distance));
	let mut vertices = Vec::with_capacity(quads.len() * 6);
	let mut runs: Vec<(&EffectTexture, Range<u32>)> = Vec::new();
	for q in quads {
		let start = vertices.len() as u32;
		vertices.extend_from_slice(&q.vertices);
		let end = vertices.len() as u32;
		match runs.last_mut() {
			Some((texture, range)) if key(texture) == key(q.texture) => range.end = end,
			_ => runs.push((q.texture, start..end)),
		}
	}
	(vertices, runs)
}

fn vertex(pos: V3, uv: [f32; 2], color: [f32; 4], emission: f32) -> EffectVertex {
	EffectVertex {
		pos,
		emission,
		uv,
		color,
	}
}

/// Two triangles through four corners: top left, top right, bottom left,
/// bottom right.
fn quad(corners: [EffectVertex; 4]) -> [EffectVertex; 6] {
	let [tl, tr, bl, br] = corners;
	[tl, bl, tr, tr, bl, br]
}

fn sprite_quad<'a>(s: &'a Sprite, view: View) -> Option<Quad<'a>> {
	let to_eye = normalize(sub(view.eye, s.at))?;
	let at = add(s.at, scale(to_eye, s.z_offset));
	let (right, up) = match s.facing {
		Facing::Camera => (view.right, view.up),
		Facing::CameraWorldUp => {
			let up = [0.0, 1.0, 0.0];
			(normalize(cross(up, to_eye)).unwrap_or(view.right), up)
		}
		Facing::VelocityParallel(v) => match normalize(v) {
			Some(up) => (normalize(cross(up, to_eye)).unwrap_or(view.right), up),
			None => (view.right, view.up),
		},
		Facing::VelocityPerpendicular(v) => match normalize(v) {
			Some(normal) => {
				let helper = if normal[1].abs() < 0.9 {
					[0.0, 1.0, 0.0]
				} else {
					[1.0, 0.0, 0.0]
				};
				let right = normalize(cross(helper, normal)).unwrap_or(view.right);
				(right, cross(normal, right))
			}
			None => (view.right, view.up),
		},
	};
	// roll by the sprite's rotation
	let (sin, cos) = s.rotation.sin_cos();
	let r = add(scale(right, cos), scale(up, sin));
	let u = sub(scale(up, cos), scale(right, sin));
	let (hw, hh) = (s.size[0] * 0.5, s.size[1] * 0.5);
	let corner = |x: f32, y: f32| add(at, add(scale(r, x * hw), scale(u, y * hh)));
	let [u0, v0, u1, v1] = s.uv;
	let v = |pos, uv| vertex(pos, uv, s.colour, s.emission);
	Some(Quad {
		distance: dot(sub(at, view.eye), sub(at, view.eye)),
		texture: &s.texture,
		vertices: quad([
			v(corner(-1.0, 1.0), [u0, v0]),
			v(corner(1.0, 1.0), [u1, v0]),
			v(corner(-1.0, -1.0), [u0, v1]),
			v(corner(1.0, -1.0), [u1, v1]),
		]),
	})
}

fn ribbon_quads<'a>(r: &'a Ribbon, view: View, out: &mut Vec<Quad<'a>>) {
	let points = &r.points;
	// each point's two edges, its width turned to the camera when it has
	// no direction of its own
	let edges: Vec<(V3, V3)> = (0..points.len())
		.map(|i| {
			let p = &points[i];
			let to_eye = normalize(sub(view.eye, p.at)).unwrap_or(view.up);
			let at = add(p.at, scale(to_eye, r.z_offset));
			let along = sub(
				points[(i + 1).min(points.len() - 1)].at,
				points[i.saturating_sub(1)].at,
			);
			let across = p
				.across
				.or_else(|| normalize(cross(along, to_eye)))
				.unwrap_or(view.up);
			let half = scale(across, p.half_width);
			(add(at, half), sub(at, half))
		})
		.collect();
	for i in 1..points.len() {
		let (a, b) = (&points[i - 1], &points[i]);
		let ((a_top, a_bottom), (b_top, b_bottom)) = (edges[i - 1], edges[i]);
		let middle = scale(add(a.at, b.at), 0.5);
		out.push(Quad {
			distance: dot(sub(middle, view.eye), sub(middle, view.eye)),
			texture: &r.texture,
			vertices: quad([
				vertex(a_top, [a.u, 0.0], a.colour, r.emission),
				vertex(b_top, [b.u, 0.0], b.colour, r.emission),
				vertex(a_bottom, [a.u, 1.0], a.colour, r.emission),
				vertex(b_bottom, [b.u, 1.0], b.colour, r.emission),
			]),
		});
	}
}

/// Uploaded textures by what they show, each with the pixels it was made
/// from when they are a downloaded image (so its address, the key, stays
/// that image's).
type Uploads = HashMap<Key, (Option<Arc<image::RgbaImage>>, wgpu::TextureView)>;

/// The pipeline effects draw with, and the textures uploaded for them.
pub(super) struct EffectPipeline {
	pipeline: wgpu::RenderPipeline,
	layout: wgpu::BindGroupLayout,
	sampler: wgpu::Sampler,
	textures: Mutex<Uploads>,
}

/// One frame's effects, ready to draw.
pub(super) struct EffectDraw {
	pipeline: wgpu::RenderPipeline,
	vertices: wgpu::Buffer,
	uniform_buffer: wgpu::Buffer,
	uniforms: EffectUniforms,
	runs: Vec<(wgpu::BindGroup, Range<u32>)>,
}

impl EffectPipeline {
	pub(super) fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("meshvault effects shader"),
			source: wgpu::ShaderSource::Wgsl(include_str!("effects.wgsl").into()),
		});
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("meshvault effects layout"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Uniform,
						has_dynamic_offset: false,
						min_binding_size: None,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 2,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Float { filterable: true },
						view_dimension: wgpu::TextureViewDimension::D2,
						multisampled: false,
					},
					count: None,
				},
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("meshvault effects pipeline layout"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		// premultiplied: what the fragment adds, over what its alpha leaves
		let premultiplied = wgpu::BlendComponent {
			src_factor: wgpu::BlendFactor::One,
			dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
			operation: wgpu::BlendOperation::Add,
		};
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("meshvault effects pipeline"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &module,
				entry_point: Some("vs_main"),
				compilation_options: wgpu::PipelineCompilationOptions::default(),
				buffers: &[Some(wgpu::VertexBufferLayout {
					array_stride: std::mem::size_of::<EffectVertex>() as u64,
					step_mode: wgpu::VertexStepMode::Vertex,
					attributes: &ATTRIBUTES,
				})],
			},
			primitive: wgpu::PrimitiveState {
				topology: wgpu::PrimitiveTopology::TriangleList,
				cull_mode: None,
				..Default::default()
			},
			depth_stencil: Some(wgpu::DepthStencilState {
				format: DEPTH_FORMAT,
				depth_write_enabled: Some(false),
				depth_compare: Some(wgpu::CompareFunction::LessEqual),
				stencil: wgpu::StencilState::default(),
				bias: wgpu::DepthBiasState::default(),
			}),
			multisample: wgpu::MultisampleState {
				count: MSAA_SAMPLES,
				mask: !0,
				alpha_to_coverage_enabled: false,
			},
			fragment: Some(wgpu::FragmentState {
				module: &module,
				entry_point: Some("fs_main"),
				compilation_options: wgpu::PipelineCompilationOptions::default(),
				targets: &[Some(wgpu::ColorTargetState {
					format: target_format,
					blend: Some(wgpu::BlendState {
						color: premultiplied,
						alpha: premultiplied,
					}),
					write_mask: wgpu::ColorWrites::ALL,
				})],
			}),
			multiview_mask: None,
			cache: None,
		});
		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("meshvault effects sampler"),
			address_mode_u: wgpu::AddressMode::Repeat,
			address_mode_v: wgpu::AddressMode::ClampToEdge,
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Linear,
			mipmap_filter: wgpu::MipmapFilterMode::Linear,
			..Default::default()
		});
		Self {
			pipeline,
			layout,
			sampler,
			textures: Mutex::new(HashMap::new()),
		}
	}

	/// The texture's upload, made the first time it is drawn.
	fn texture(
		&self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		texture: &EffectTexture,
	) -> wgpu::TextureView {
		let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
		textures
			.entry(key(texture))
			.or_insert_with(|| {
				let (keep, pixels) = match texture {
					EffectTexture::Plain => (
						None,
						Arc::new(image::RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))),
					),
					EffectTexture::Builtin(b) => (None, b.image()),
					EffectTexture::Image(pixels) => (Some(pixels.clone()), pixels.clone()),
				};
				let view =
					upload_texture(device, queue, &pixels, wgpu::TextureFormat::Rgba8UnormSrgb);
				(keep, view)
			})
			.1
			.clone()
	}

	/// `frame` ready to draw from `view` with `view_proj` (column-major),
	/// or `None` when there is nothing to draw.
	pub(super) fn prepare(
		&self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		frame: &EffectFrame,
		view: View,
		view_proj: [[f32; 4]; 4],
		gamma_output: bool,
	) -> Option<EffectDraw> {
		let (vertices, runs) = triangles(frame, view);
		if vertices.is_empty() {
			return None;
		}
		let uniforms = EffectUniforms {
			view_proj,
			params: [if gamma_output { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
		};
		let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
			label: Some("meshvault effects uniforms"),
			contents: bytemuck::bytes_of(&uniforms),
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
		});
		let runs = runs
			.into_iter()
			.map(|(texture, range)| {
				let view = self.texture(device, queue, texture);
				let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
					label: Some("meshvault effects bind group"),
					layout: &self.layout,
					entries: &[
						wgpu::BindGroupEntry {
							binding: 0,
							resource: uniform_buffer.as_entire_binding(),
						},
						wgpu::BindGroupEntry {
							binding: 1,
							resource: wgpu::BindingResource::Sampler(&self.sampler),
						},
						wgpu::BindGroupEntry {
							binding: 2,
							resource: wgpu::BindingResource::TextureView(&view),
						},
					],
				});
				(bind_group, range)
			})
			.collect();
		let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
			label: Some("meshvault effects vertices"),
			contents: bytemuck::cast_slice(&vertices),
			usage: wgpu::BufferUsages::VERTEX,
		});
		Some(EffectDraw {
			pipeline: self.pipeline.clone(),
			vertices,
			uniform_buffer,
			uniforms,
			runs,
		})
	}
}

impl EffectDraw {
	/// Uploads the uniforms (the draw may be painted more than once).
	pub(super) fn write(&self, queue: &wgpu::Queue) {
		queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&self.uniforms));
	}

	pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
		pass.set_pipeline(&self.pipeline);
		pass.set_vertex_buffer(0, self.vertices.slice(..));
		for (bind_group, range) in &self.runs {
			pass.set_bind_group(0, bind_group, &[]);
			pass.draw(range.clone(), 0..1);
		}
	}
}

fn add(a: V3, b: V3) -> V3 {
	[a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
	[a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, s: f32) -> V3 {
	a.map(|v| v * s)
}

fn normalize(a: V3) -> Option<V3> {
	let l = dot(a, a).sqrt();
	(l > 1e-8).then(|| a.map(|v| v / l))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::scene::effects::RibbonPoint;

	/// The attributes are read where the vertex's fields are: a field
	/// added without its attribute (or padding) would shift the colour.
	#[test]
	fn attributes_match_the_vertex_layout() {
		use std::mem::offset_of;
		let offsets: Vec<u64> = ATTRIBUTES.iter().map(|a| a.offset).collect();
		assert_eq!(
			offsets,
			[
				offset_of!(EffectVertex, pos),
				offset_of!(EffectVertex, emission),
				offset_of!(EffectVertex, uv),
				offset_of!(EffectVertex, color),
			]
			.map(|o| o as u64)
		);
		let last = ATTRIBUTES[3].offset + ATTRIBUTES[3].format.size();
		assert_eq!(last, std::mem::size_of::<EffectVertex>() as u64);
	}

	#[test]
	fn shader_is_valid_wgsl() {
		let source = include_str!("effects.wgsl");
		let module = naga::front::wgsl::parse_str(source)
			.unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
		naga::valid::Validator::new(
			naga::valid::ValidationFlags::all(),
			naga::valid::Capabilities::default(),
		)
		.validate(&module)
		.unwrap();
	}

	const VIEW: View = View {
		eye: [0.0, 0.0, 10.0],
		right: [1.0, 0.0, 0.0],
		up: [0.0, 1.0, 0.0],
	};

	fn sprite(at: V3, texture: EffectTexture) -> Sprite {
		Sprite {
			at,
			size: [2.0, 4.0],
			rotation: 0.0,
			facing: Facing::Camera,
			uv: [0.0, 0.0, 1.0, 1.0],
			colour: [1.0, 0.5, 0.25, 0.75],
			emission: 0.5,
			z_offset: 0.0,
			texture,
		}
	}

	/// A sprite faces the camera at its size, the texture upright; sprites
	/// come farthest first, runs sharing a texture drawn together.
	#[test]
	fn sprites_face_the_camera_farthest_first() {
		let frame = EffectFrame {
			sprites: vec![
				sprite([0.0; 3], EffectTexture::Builtin(Builtin::Soft)),
				sprite([0.0, 0.0, -5.0], EffectTexture::Builtin(Builtin::Soft)),
				sprite([0.0, 0.0, 5.0], EffectTexture::Plain),
			],
			ribbons: vec![],
		};
		let (vertices, runs) = triangles(&frame, VIEW);
		assert_eq!(vertices.len(), 18);
		// the farthest (z -5) first; its top left corner is up and left
		assert_eq!(vertices[0].pos, [-1.0, 2.0, -5.0]);
		assert_eq!(vertices[0].uv, [0.0, 0.0]);
		assert_eq!(
			(vertices[0].color, vertices[0].emission),
			([1.0, 0.5, 0.25, 0.75], 0.5)
		);
		let ranges: Vec<_> = runs.iter().map(|(_, r)| r.clone()).collect();
		assert_eq!(ranges, [0..12, 12..18]);
		assert!(matches!(runs[1].0, EffectTexture::Plain));
	}

	/// A rotated sprite rolls; z offset moves it towards the camera.
	#[test]
	fn sprites_roll_and_come_forward() {
		let mut s = sprite([0.0; 3], EffectTexture::Plain);
		s.rotation = std::f32::consts::FRAC_PI_2;
		s.z_offset = 1.0;
		let q = sprite_quad(&s, VIEW).unwrap();
		// a quarter turn: the top left corner is now down and left... of a 2 x 4 sprite
		let tl = q.vertices[0].pos;
		assert!(
			(tl[0] + 2.0).abs() < 1e-5 && (tl[1] + 1.0).abs() < 1e-5 && (tl[2] - 1.0).abs() < 1e-5,
			"{tl:?}"
		);
	}

	/// A ribbon facing the camera is a strip across its line of sight; one
	/// with a direction of its own keeps it.
	#[test]
	fn ribbons_are_strips_between_their_points() {
		let point = |x: f32, across: Option<V3>| RibbonPoint {
			at: [x, 0.0, 0.0],
			half_width: 0.5,
			across,
			u: x,
			colour: [1.0; 4],
		};
		let ribbon = |across| Ribbon {
			points: vec![point(0.0, across), point(1.0, across), point(2.0, across)],
			emission: 0.0,
			z_offset: 0.0,
			texture: EffectTexture::Plain,
		};
		let mut quads = Vec::new();
		let facing = ribbon(None);
		ribbon_quads(&facing, VIEW, &mut quads);
		assert_eq!(quads.len(), 2);
		// along X, seen from +Z: its width runs up and down
		let ys: Vec<f32> = quads[0].vertices.iter().map(|v| v.pos[1]).collect();
		assert!(ys.iter().all(|y| (y.abs() - 0.5).abs() < 1e-5), "{ys:?}");
		let mut quads = Vec::new();
		let fixed = ribbon(Some([0.0, 0.0, 1.0]));
		ribbon_quads(&fixed, VIEW, &mut quads);
		let zs: Vec<f32> = quads[0].vertices.iter().map(|v| v.pos[2]).collect();
		assert!(zs.iter().all(|z| (z.abs() - 0.5).abs() < 1e-5), "{zs:?}");
		// corners go top left, bottom left, top right...: the far top corner is third
		assert_eq!(quads[1].vertices[2].uv, [2.0, 0.0]);
	}
}
