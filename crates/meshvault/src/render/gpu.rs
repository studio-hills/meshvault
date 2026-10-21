//! GPU mesh rendering through eframe's wgpu backend: depth-tested,
//! perspective-correct, mipmapped, with physically based shading of
//! SurfaceAppearance maps. Drawn inside egui via a paint callback.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui_wgpu::{self, wgpu};
use wgpu::util::DeviceExt;

use crate::scene::colour::{linear_to_srgb, srgb8_to_linear};
use crate::scene::geometry::Geometry;
use crate::scene::maps::Slot;
use crate::scene::surface::AlphaMode;
use crate::scene::texture::Material;

/// The window's multisample count (main passes it to `NativeOptions`);
/// every pipeline and the off-screen colour and depth targets use it.
pub const MSAA_SAMPLES: u32 = 4;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
	pos: [f32; 3],
	normal: [f32; 3],
	uv: [f32; 2],
	bary: [f32; 3],
	color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
	pub view_proj: [[f32; 4]; 4],
	/// Mesh to world, and its inverse transpose for normals (column-major).
	pub model: [[f32; 4]; 4],
	pub normal_model: [[f32; 4]; 4],
	pub eye: [f32; 4],
	pub light_dir: [f32; 4],
	pub wire_color: [f32; 4],
	pub params: [f32; 4],
	pub flags: [u32; 4],
	/// How a Roblox part's surface looks: opacity (1 − Transparency),
	/// 1 for Neon, Reflectance, 1 for metal materials.
	pub surface: [f32; 4],
	/// SurfaceAppearance: linear tint of the colour map, and the alpha mode
	/// (0 Overlay, 1 Transparency, 2 TintMask, 3 Opaque, 4 Blend: the map's
	/// alpha blends the surface, for meshes shown on their own).
	pub tint: [f32; 4],
	/// Decal draws: mesh to part space (column-major), then mode (0 none,
	/// 1 Decal, 2 Texture, 3 MaterialVariant, 4 clothing), face,
	/// transparency; the part's size; studs per tile and offset (U, V, U,
	/// V); and the linear tint.
	pub part_from_mesh: [[f32; 4]; 4],
	pub decal: [f32; 4],
	pub decal_size: [f32; 4],
	pub decal_tile: [f32; 4],
	pub decal_colour: [f32; 4],
	/// The model's lights (see `lighting::light_uniforms`), and how many.
	pub lights: [[f32; 4]; crate::render::lighting::MAX_LIGHTS * 3],
	pub light_count: [f32; 4],
	/// Clothing draws: the template region (x, y, width, height, 0..1) of
	/// each box face (Right, Top, Back, Left, Bottom, Front), then the box's
	/// centre and half size in the body section's space.
	pub clothing: [[f32; 4]; 8],
	/// Lighting and highlight (see `lighting::Lighting`): key light colour
	/// and fill strength; sky colour and exposure; ground colour and how
	/// strongly this draw is highlighted; the highlight colour.
	pub env: [[f32; 4]; 4],
}

/// How a surface is drawn, which picks its pipeline.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
	Opaque,
	/// Cut out by its colour map's alpha, with smooth (multisampled) edges.
	Cutout,
	Decal,
	SeeThrough,
}

/// The render pipelines: each with no culling, then with back faces culled.
#[derive(Clone)]
struct Pipelines {
	opaque: [wgpu::RenderPipeline; 2],
	cutout: [wgpu::RenderPipeline; 2],
	decal: [wgpu::RenderPipeline; 2],
	/// No culling, back faces culled, and front faces culled (to draw a
	/// see-through surface's far side before its near side).
	see_through: [wgpu::RenderPipeline; 3],
}

pub struct GpuRenderer {
	device: wgpu::Device,
	queue: wgpu::Queue,
	pipelines: Pipelines,
	layout: wgpu::BindGroupLayout,
	sampler: wgpu::Sampler,
	/// Bound to slots without a map.
	white: wgpu::TextureView,
	flat_normal: wgpu::TextureView,
	/// egui renders to a non-sRGB target and expects gamma-encoded colour.
	gamma_output: bool,
	/// The format egui renders to, which the pipelines draw into.
	target_format: wgpu::TextureFormat,
}

/// One uploaded mesh with its material, owned by the open document (or by
/// one part of an animated model).
pub struct GpuScene {
	vertices: wgpu::Buffer,
	uniforms: wgpu::Buffer,
	/// Vertex ranges of each geometry group (LOD or hull).
	groups: Vec<Range<u32>>,
	bind_group: wgpu::BindGroup,
	flags: [u32; 4],
	material_generation: u64,
	/// Uploaded maps, keyed by the address of their pixels so toggling a
	/// map off and on does not upload it again.
	textures: BTreeMap<Slot, (usize, wgpu::TextureView)>,
}

impl GpuRenderer {
	pub fn new(render_state: &egui_wgpu::RenderState) -> Arc<Self> {
		let device = render_state.device.clone();
		let queue = render_state.queue.clone();
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("meshvault mesh shader"),
			source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
		});
		let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
			binding,
			visibility: wgpu::ShaderStages::FRAGMENT,
			ty: wgpu::BindingType::Texture {
				sample_type: wgpu::TextureSampleType::Float { filterable: true },
				view_dimension: wgpu::TextureViewDimension::D2,
				multisampled: false,
			},
			count: None,
		};
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("meshvault bind group layout"),
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
				texture_entry(2),
				texture_entry(3),
				texture_entry(4),
				texture_entry(5),
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("meshvault pipeline layout"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let target_format = render_state.target_format;
		// see-through parts blend over what is behind them and leave the depth
		// buffer alone, so they are drawn last, from the back to the front;
		// decals blend over their part's own surface, at the same depth
		let pipeline = |kind: Kind, cull_mode| {
			let see_through = matches!(kind, Kind::SeeThrough | Kind::Decal);
			let decal = kind == Kind::Decal;
			device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
				label: Some("meshvault pipeline"),
				layout: Some(&pipeline_layout),
				vertex: wgpu::VertexState {
					module: &module,
					entry_point: Some("vs_main"),
					compilation_options: wgpu::PipelineCompilationOptions::default(),
					buffers: &[Some(wgpu::VertexBufferLayout {
						array_stride: std::mem::size_of::<Vertex>() as u64,
						step_mode: wgpu::VertexStepMode::Vertex,
						attributes: &wgpu::vertex_attr_array![
							0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x3, 4 => Float32x3
						],
					})],
				},
				primitive: wgpu::PrimitiveState {
					topology: wgpu::PrimitiveTopology::TriangleList,
					// Roblox meshes wind front faces counter-clockwise
					front_face: wgpu::FrontFace::Ccw,
					cull_mode,
					..Default::default()
				},
				depth_stencil: Some(wgpu::DepthStencilState {
					format: DEPTH_FORMAT,
					depth_write_enabled: Some(!see_through),
					depth_compare: Some(if decal {
						wgpu::CompareFunction::LessEqual
					} else {
						wgpu::CompareFunction::Less
					}),
					stencil: wgpu::StencilState::default(),
					bias: wgpu::DepthBiasState::default(),
				}),
				multisample: wgpu::MultisampleState {
					count: MSAA_SAMPLES,
					mask: !0,
					// a cutout's alpha decides how many samples it covers
					alpha_to_coverage_enabled: kind == Kind::Cutout,
				},
				fragment: Some(wgpu::FragmentState {
					module: &module,
					entry_point: Some("fs_main"),
					compilation_options: wgpu::PipelineCompilationOptions::default(),
					targets: &[Some(wgpu::ColorTargetState {
						format: target_format,
						blend: see_through.then_some(wgpu::BlendState::ALPHA_BLENDING),
						write_mask: wgpu::ColorWrites::ALL,
					})],
				}),
				multiview_mask: None,
				cache: None,
			})
		};
		let both = |kind| [pipeline(kind, None), pipeline(kind, Some(wgpu::Face::Back))];
		let pipelines = Pipelines {
			opaque: both(Kind::Opaque),
			cutout: both(Kind::Cutout),
			decal: both(Kind::Decal),
			see_through: [
				pipeline(Kind::SeeThrough, None),
				pipeline(Kind::SeeThrough, Some(wgpu::Face::Back)),
				pipeline(Kind::SeeThrough, Some(wgpu::Face::Front)),
			],
		};
		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("meshvault sampler"),
			address_mode_u: wgpu::AddressMode::Repeat,
			address_mode_v: wgpu::AddressMode::Repeat,
			address_mode_w: wgpu::AddressMode::Repeat,
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Linear,
			mipmap_filter: wgpu::MipmapFilterMode::Linear,
			anisotropy_clamp: 8,
			..Default::default()
		});
		let solid = |rgba: [u8; 4], format| {
			let image = image::RgbaImage::from_pixel(1, 1, image::Rgba(rgba));
			upload_texture(&device, &queue, &image, format)
		};
		let white = solid([255; 4], wgpu::TextureFormat::Rgba8Unorm);
		let flat_normal = solid([128, 128, 255, 255], wgpu::TextureFormat::Rgba8Unorm);
		Arc::new(Self {
			device,
			queue,
			pipelines,
			layout,
			sampler,
			white,
			flat_normal,
			gamma_output: !target_format.is_srgb(),
			target_format,
		})
	}

	/// Uploads geometry as an unindexed triangle list (each corner gets its
	/// own barycentric coordinate for the wireframe). `colours` gives each
	/// group's base colour.
	pub fn create_scene(&self, geometry: &Geometry, colours: &[[f32; 3]]) -> GpuScene {
		let (vertices, groups) = build_vertices(geometry, colours);
		let vertex_buffer = self
			.device
			.create_buffer_init(&wgpu::util::BufferInitDescriptor {
				label: Some("meshvault vertices"),
				contents: bytemuck::cast_slice(&vertices),
				usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
			});
		let uniforms = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("meshvault uniforms"),
			size: std::mem::size_of::<Uniforms>() as u64,
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		GpuScene {
			bind_group: self.bind_group(&uniforms, &BTreeMap::new()),
			vertices: vertex_buffer,
			uniforms,
			groups,
			flags: [0; 4],
			// forces the first sync_material to upload
			material_generation: u64::MAX,
			textures: BTreeMap::new(),
		}
	}

	/// Replaces the vertices of a scene with moved ones (a skinned mesh
	/// each frame). The triangles must be the same as when it was created.
	pub fn update_vertices(&self, scene: &GpuScene, geometry: &Geometry, colours: &[[f32; 3]]) {
		let (vertices, _) = build_vertices(geometry, colours);
		self.queue
			.write_buffer(&scene.vertices, 0, bytemuck::cast_slice(&vertices));
	}

	fn bind_group(
		&self,
		uniforms: &wgpu::Buffer,
		textures: &BTreeMap<Slot, (usize, wgpu::TextureView)>,
	) -> wgpu::BindGroup {
		let view = |slot: Slot| {
			textures.get(&slot).map(|(_, v)| v).unwrap_or(match slot {
				Slot::Normal => &self.flat_normal,
				_ => &self.white,
			})
		};
		self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("meshvault bind group"),
			layout: &self.layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: uniforms.as_entire_binding(),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::Sampler(&self.sampler),
				},
				wgpu::BindGroupEntry {
					binding: 2,
					resource: wgpu::BindingResource::TextureView(view(Slot::Base)),
				},
				wgpu::BindGroupEntry {
					binding: 3,
					resource: wgpu::BindingResource::TextureView(view(Slot::Normal)),
				},
				wgpu::BindGroupEntry {
					binding: 4,
					resource: wgpu::BindingResource::TextureView(view(Slot::Roughness)),
				},
				wgpu::BindGroupEntry {
					binding: 5,
					resource: wgpu::BindingResource::TextureView(view(Slot::Metalness)),
				},
			],
		})
	}

	/// Re-uploads maps that changed and rebuilds the bind group.
	pub fn sync_material(&self, scene: &mut GpuScene, material: &Material) {
		if scene.material_generation == material.generation {
			return;
		}
		scene.material_generation = material.generation;
		let mut textures = BTreeMap::new();
		let mut flags = [0u32; 4];
		for (i, slot) in Slot::ALL.into_iter().enumerate() {
			let Some(texture) = material.active(slot) else {
				continue;
			};
			let key = Arc::as_ptr(&texture.rgba) as usize;
			let view = match scene.textures.get(&slot) {
				Some((k, view)) if *k == key => view.clone(),
				_ => {
					let format = if slot.is_srgb() {
						wgpu::TextureFormat::Rgba8UnormSrgb
					} else {
						wgpu::TextureFormat::Rgba8Unorm
					};
					upload_texture(&self.device, &self.queue, &texture.rgba, format)
				}
			};
			textures.insert(slot, (key, view));
			flags[i] = 1;
		}
		scene.bind_group = self.bind_group(&scene.uniforms, &textures);
		scene.textures = textures;
		scene.flags = flags;
	}

	/// The paint callback drawing each scene's `groups` with its uniforms:
	/// opaque ones first, then see-through ones in the order given.
	pub fn callback<'a>(
		&self,
		scenes: impl IntoIterator<Item = (&'a GpuScene, &'a [usize], Uniforms)>,
		cull: bool,
	) -> MeshCallback {
		let draws = scenes
			.into_iter()
			.map(|(scene, groups, mut uniforms)| {
				uniforms.flags = scene.flags;
				uniforms.params[1] = if self.gamma_output { 1.0 } else { 0.0 };
				// the alpha mode, compared as mesh.wgsl does
				let mode = uniforms.tint[3];
				let is = |m: AlphaMode| (mode - m.shader_value()).abs() < 0.5;
				let has_map = scene.flags[0] != 0;
				Draw {
					pass: if uniforms.decal[0] > 0.0 {
						Pass::Decal
					} else if uniforms.surface[0] < 1.0 || (has_map && is(AlphaMode::Blend)) {
						Pass::SeeThrough
					} else if has_map && is(AlphaMode::Transparency) {
						Pass::Cutout
					} else {
						Pass::Opaque
					},
					bind_group: scene.bind_group.clone(),
					vertices: scene.vertices.clone(),
					uniform_buffer: scene.uniforms.clone(),
					uniforms,
					ranges: groups
						.iter()
						.filter_map(|&g| scene.groups.get(g).cloned())
						.collect(),
				}
			})
			.collect();
		MeshCallback {
			pipelines: self.pipelines.clone(),
			cull,
			draws,
		}
	}
}

impl GpuRenderer {
	/// Draws `callback` off screen into a `size` image whose background is
	/// transparent. It is drawn twice, over black and over white: where the
	/// two differ, the background shows through.
	pub fn render_image(&self, callback: &MeshCallback, size: [u32; 2]) -> image::RgbaImage {
		let black = self.render_over(callback, size, wgpu::Color::BLACK);
		let white = self.render_over(callback, size, wgpu::Color::WHITE);
		// an sRGB target blends in linear light and stores encoded values
		unblend(&black, &white, size, self.target_format.is_srgb())
	}

	fn render_over(
		&self,
		callback: &MeshCallback,
		[width, height]: [u32; 2],
		clear: wgpu::Color,
	) -> image::RgbaImage {
		let extent = wgpu::Extent3d {
			width,
			height,
			depth_or_array_layers: 1,
		};
		let texture = |samples, format, usage| {
			self.device.create_texture(&wgpu::TextureDescriptor {
				label: Some("meshvault offscreen"),
				size: extent,
				mip_level_count: 1,
				sample_count: samples,
				dimension: wgpu::TextureDimension::D2,
				format,
				usage,
				view_formats: &[],
			})
		};
		let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
		let msaa = texture(MSAA_SAMPLES, self.target_format, attachment);
		let resolved = texture(
			1,
			self.target_format,
			attachment | wgpu::TextureUsages::COPY_SRC,
		);
		let depth = texture(MSAA_SAMPLES, DEPTH_FORMAT, attachment);
		let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
		let (msaa_view, resolved_view, depth_view) = (view(&msaa), view(&resolved), view(&depth));
		for draw in &callback.draws {
			self.queue
				.write_buffer(&draw.uniform_buffer, 0, bytemuck::bytes_of(&draw.uniforms));
		}
		let mut encoder = self
			.device
			.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
		{
			let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("meshvault offscreen"),
				color_attachments: &[Some(wgpu::RenderPassColorAttachment {
					view: &msaa_view,
					depth_slice: None,
					resolve_target: Some(&resolved_view),
					ops: wgpu::Operations {
						load: wgpu::LoadOp::Clear(clear),
						store: wgpu::StoreOp::Discard,
					},
				})],
				depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
					view: &depth_view,
					depth_ops: Some(wgpu::Operations {
						load: wgpu::LoadOp::Clear(1.0),
						store: wgpu::StoreOp::Discard,
					}),
					stencil_ops: None,
				}),
				timestamp_writes: None,
				occlusion_query_set: None,
				multiview_mask: None,
			});
			callback.draw(&mut pass);
		}
		// rows of a buffer copy are aligned to 256 bytes
		let row = (4 * width).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
			* wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
		let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("meshvault offscreen readback"),
			size: (row * height) as u64,
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		encoder.copy_texture_to_buffer(
			wgpu::TexelCopyTextureInfo {
				texture: &resolved,
				mip_level: 0,
				origin: wgpu::Origin3d::ZERO,
				aspect: wgpu::TextureAspect::All,
			},
			wgpu::TexelCopyBufferInfo {
				buffer: &buffer,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(row),
					rows_per_image: Some(height),
				},
			},
			extent,
		);
		self.queue.submit([encoder.finish()]);
		buffer.map_async(wgpu::MapMode::Read, .., |_| {});
		let _ = self.device.poll(wgpu::PollType::Wait {
			submission_index: None,
			timeout: None,
		});
		let bgra = matches!(
			self.target_format,
			wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
		);
		// A failed readback is not reported: the map callback's result and
		// the poll's error are dropped above, and an unmapped buffer leaves
		// the image blank (every byte zero). Fixing that is a separate change.
		let image = match buffer.slice(..).get_mapped_range() {
			Ok(data) => read_rows(&data, row as usize, [width, height], bgra),
			Err(_) => image::RgbaImage::new(width, height),
		};
		buffer.unmap();
		image
	}
}

/// Turns a readback buffer, `row` bytes per line, into an RGBA image,
/// swapping the channels of a BGRA target.
fn read_rows(data: &[u8], row: usize, [width, height]: [u32; 2], bgra: bool) -> image::RgbaImage {
	let mut image = image::RgbaImage::new(width, height);
	for (y, line) in data.chunks(row).take(height as usize).enumerate() {
		for x in 0..width as usize {
			let p = &line[x * 4..x * 4 + 4];
			let rgba = if bgra {
				[p[2], p[1], p[0], p[3]]
			} else {
				[p[0], p[1], p[2], p[3]]
			};
			image.put_pixel(x as u32, y as u32, image::Rgba(rgba));
		}
	}
	image
}

/// Recovers a transparent `size` image from the same scene drawn over
/// black and over white. Where both are blank (a failed readback, see
/// `render_over`) the two agree, so the result is opaque black, not empty.
/// `srgb`: the images hold sRGB-encoded values of a linear blend.
fn unblend(
	black: &image::RgbaImage,
	white: &image::RgbaImage,
	size: [u32; 2],
	srgb: bool,
) -> image::RgbaImage {
	let decode = |c: u8| {
		if srgb {
			srgb8_to_linear(c)
		} else {
			c as f32 / 255.0
		}
	};
	let encode = |v: f32| {
		let v = v.clamp(0.0, 1.0);
		let v = if srgb { linear_to_srgb(v) } else { v };
		(v * 255.0).round() as u8
	};
	let mut out = image::RgbaImage::new(size[0], size[1]);
	for ((o, b), w) in out.pixels_mut().zip(black.pixels()).zip(white.pixels()) {
		let b = [0, 1, 2].map(|k| decode(b[k]));
		let w = [0, 1, 2].map(|k| decode(w[k]));
		// over black a surface leaves c·a, over white c·a + (1 − a)
		let alpha = (1.0 - (0..3).map(|k| w[k] - b[k]).sum::<f32>() / 3.0).clamp(0.0, 1.0);
		if alpha < 0.5 / 255.0 {
			continue;
		}
		*o = image::Rgba([
			encode(b[0] / alpha),
			encode(b[1] / alpha),
			encode(b[2] / alpha),
			(alpha * 255.0).round() as u8,
		]);
	}
	out
}

fn build_vertices(geometry: &Geometry, colours: &[[f32; 3]]) -> (Vec<Vertex>, Vec<Range<u32>>) {
	let mut vertices = Vec::with_capacity(geometry.triangles.len() * 3);
	let mut groups = Vec::with_capacity(geometry.groups.len());
	const BARY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
	for (g, range) in geometry.groups.iter().enumerate() {
		let start = vertices.len() as u32;
		let color = colours.get(g).copied().unwrap_or([0.8; 3]);
		for t in &geometry.triangles[range.clone()] {
			for (corner, &i) in t.iter().enumerate() {
				let i = i as usize;
				vertices.push(Vertex {
					pos: geometry.positions[i],
					normal: geometry.normals.as_ref().map_or([0.0; 3], |n| n[i]),
					uv: geometry.uvs.as_ref().map_or([0.0; 2], |u| u[i]),
					bary: BARY[corner],
					color,
				});
			}
		}
		groups.push(start..vertices.len() as u32);
	}
	// wgpu rejects empty buffers
	if vertices.is_empty() {
		vertices.push(bytemuck::Zeroable::zeroed());
	}
	(vertices, groups)
}

/// Uploads an image with a full mip chain (generated on the CPU, since
/// wgpu has no built-in mipmap generation).
fn upload_texture(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	image: &image::RgbaImage,
	format: wgpu::TextureFormat,
) -> wgpu::TextureView {
	let (w, h) = image.dimensions();
	let levels = 32 - w.max(h).max(1).leading_zeros();
	let texture = device.create_texture(&wgpu::TextureDescriptor {
		label: Some("meshvault map"),
		size: wgpu::Extent3d {
			width: w,
			height: h,
			depth_or_array_layers: 1,
		},
		mip_level_count: levels,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format,
		usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
		view_formats: &[],
	});
	let mut level_image = image.clone();
	for level in 0..levels {
		let (lw, lh) = level_image.dimensions();
		queue.write_texture(
			wgpu::TexelCopyTextureInfo {
				texture: &texture,
				mip_level: level,
				origin: wgpu::Origin3d::ZERO,
				aspect: wgpu::TextureAspect::All,
			},
			level_image.as_raw(),
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(4 * lw),
				rows_per_image: Some(lh),
			},
			wgpu::Extent3d {
				width: lw,
				height: lh,
				depth_or_array_layers: 1,
			},
		);
		if level + 1 < levels {
			let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
			level_image = image::imageops::resize(
				&level_image,
				nw,
				nh,
				image::imageops::FilterType::Triangle,
			);
		}
	}
	texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Drawn in this order: opaque surfaces, cutouts, the decals on them,
/// then see-through surfaces.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
	Opaque,
	Cutout,
	Decal,
	SeeThrough,
}

struct Draw {
	pass: Pass,
	bind_group: wgpu::BindGroup,
	vertices: wgpu::Buffer,
	uniform_buffer: wgpu::Buffer,
	uniforms: Uniforms,
	ranges: Vec<Range<u32>>,
}

pub struct MeshCallback {
	pipelines: Pipelines,
	/// Back faces are culled.
	cull: bool,
	draws: Vec<Draw>,
}

impl egui_wgpu::CallbackTrait for MeshCallback {
	fn prepare(
		&self,
		_device: &wgpu::Device,
		queue: &wgpu::Queue,
		_screen_descriptor: &egui_wgpu::ScreenDescriptor,
		_egui_encoder: &mut wgpu::CommandEncoder,
		_resources: &mut egui_wgpu::CallbackResources,
	) -> Vec<wgpu::CommandBuffer> {
		for draw in &self.draws {
			queue.write_buffer(&draw.uniform_buffer, 0, bytemuck::bytes_of(&draw.uniforms));
		}
		Vec::new()
	}

	fn paint(
		&self,
		_info: eframe::egui::PaintCallbackInfo,
		render_pass: &mut wgpu::RenderPass<'static>,
		_resources: &egui_wgpu::CallbackResources,
	) {
		self.draw(render_pass);
	}
}

impl MeshCallback {
	fn draw(&self, render_pass: &mut wgpu::RenderPass<'_>) {
		let p = &self.pipelines;
		let cull = self.cull as usize;
		let one = |render_pass: &mut wgpu::RenderPass<'_>, draw: &Draw| {
			render_pass.set_bind_group(0, &draw.bind_group, &[]);
			render_pass.set_vertex_buffer(0, draw.vertices.slice(..));
			for range in &draw.ranges {
				if !range.is_empty() {
					render_pass.draw(range.clone(), 0..1);
				}
			}
		};
		for (pass, pipeline) in [
			(Pass::Opaque, &p.opaque[cull]),
			(Pass::Cutout, &p.cutout[cull]),
			(Pass::Decal, &p.decal[cull]),
		] {
			render_pass.set_pipeline(pipeline);
			for draw in self.draws.iter().filter(|d| d.pass == pass) {
				one(render_pass, draw);
			}
		}
		// see-through surfaces come sorted from the farthest; each draws its
		// far side before its near side, so it blends over itself in order
		let sides: &[&wgpu::RenderPipeline] = if self.cull {
			&[&p.see_through[1]]
		} else {
			&[&p.see_through[2], &p.see_through[1]]
		};
		for draw in self.draws.iter().filter(|d| d.pass == Pass::SeeThrough) {
			for pipeline in sides {
				render_pass.set_pipeline(pipeline);
				one(render_pass, draw);
			}
		}
	}
}

#[cfg(test)]
// geometry groups are lists of ranges, so one-element lists are intended
#[allow(clippy::single_range_in_vec_init)]
mod tests {
	/// The shader is only compiled when the app starts; validate it here so a
	/// mistake fails the test suite instead of crashing the viewer.
	#[test]
	fn shader_is_valid_wgsl() {
		let module = naga::front::wgsl::parse_str(include_str!("mesh.wgsl"))
			.unwrap_or_else(|e| panic!("{}", e.emit_to_string(include_str!("mesh.wgsl"))));
		naga::valid::Validator::new(
			naga::valid::ValidationFlags::all(),
			naga::valid::Capabilities::default(),
		)
		.validate(&module)
		.unwrap_or_else(|e| panic!("{e:?}"));
	}

	/// The Rust uniform struct must match the WGSL one byte for byte: the
	/// same members in the same order, each at the same offset with the same
	/// scalar type and size, so reordering two same-sized members fails here.
	#[test]
	fn uniform_layout_matches_shader() {
		use std::mem::{offset_of, size_of};

		use super::Uniforms;

		let module = naga::front::wgsl::parse_str(include_str!("mesh.wgsl")).unwrap();
		let (_, ty) = module
			.types
			.iter()
			.find(|(_, t)| t.name.as_deref() == Some("Uniforms"))
			.unwrap();
		let naga::TypeInner::Struct { members, span } = &ty.inner else {
			panic!("Uniforms is not a struct");
		};
		assert_eq!(*span as usize, size_of::<Uniforms>());

		// Rust's side: each field's name, offset, size and how many vec4s of
		// which scalar it holds (a mat4x4 and an array of four vec4s are laid
		// out alike). The scalar and count come from the field's own type, so
		// changing a field from u32 to f32 (or i32) fails here too.
		trait Scalar {
			const KIND: naga::ScalarKind;
		}
		impl Scalar for f32 {
			const KIND: naga::ScalarKind = naga::ScalarKind::Float;
		}
		impl Scalar for u32 {
			const KIND: naga::ScalarKind = naga::ScalarKind::Uint;
		}
		impl Scalar for i32 {
			const KIND: naga::ScalarKind = naga::ScalarKind::Sint;
		}
		trait Shape {
			const SHAPE: (naga::ScalarKind, u32);
		}
		impl<S: Scalar> Shape for [S; 4] {
			const SHAPE: (naga::ScalarKind, u32) = (S::KIND, 1);
		}
		impl<S: Scalar, const N: usize> Shape for [[S; 4]; N] {
			const SHAPE: (naga::ScalarKind, u32) = (S::KIND, N as u32);
		}
		fn size_and_shape<T: Shape>(_: fn(&Uniforms) -> &T) -> (u32, (naga::ScalarKind, u32)) {
			(size_of::<T>() as u32, T::SHAPE)
		}
		macro_rules! field {
			($name:ident) => {{
				let (size, shape) = size_and_shape(|u: &Uniforms| &u.$name);
				(
					stringify!($name),
					offset_of!(Uniforms, $name) as u32,
					size,
					shape,
				)
			}};
		}
		let rust = [
			field!(view_proj),
			field!(model),
			field!(normal_model),
			field!(eye),
			field!(light_dir),
			field!(wire_color),
			field!(params),
			field!(flags),
			field!(surface),
			field!(tint),
			field!(part_from_mesh),
			field!(decal),
			field!(decal_size),
			field!(decal_tile),
			field!(decal_colour),
			field!(lights),
			field!(light_count),
			field!(clothing),
			field!(env),
		];
		// each Rust field is 4-byte scalars, so its size follows from the shape
		for (name, _, size, (_, vec4s)) in rust {
			assert_eq!(size, vec4s * 16, "{name}");
		}

		// the shader's side, in the same terms
		let vec4 = |inner: &naga::TypeInner| match *inner {
			naga::TypeInner::Vector {
				size: naga::VectorSize::Quad,
				scalar,
			} if scalar.width == 4 => Some(scalar.kind),
			_ => None,
		};
		let shape = |inner: &naga::TypeInner| match *inner {
			naga::TypeInner::Matrix {
				columns: naga::VectorSize::Quad,
				rows: naga::VectorSize::Quad,
				scalar,
			} if scalar.width == 4 => Some((scalar.kind, 4)),
			naga::TypeInner::Array {
				base,
				size: naga::ArraySize::Constant(n),
				stride: 16,
			} => vec4(&module.types[base].inner).map(|kind| (kind, n.get())),
			_ => vec4(inner).map(|kind| (kind, 1)),
		};
		let shader: Vec<_> = members
			.iter()
			.map(|m| {
				let inner = &module.types[m.ty].inner;
				(
					m.name.clone().unwrap_or_default(),
					m.offset,
					inner.size(module.to_ctx()),
					shape(inner).unwrap_or_else(|| panic!("{:?} has an unexpected type", m.name)),
				)
			})
			.collect();

		assert_eq!(shader.len(), rust.len(), "member count");
		for ((name, offset, size, shape), wgsl) in rust.into_iter().zip(&shader) {
			assert_eq!((name.to_string(), offset, size, shape), *wgsl);
		}
	}

	use super::{Vertex, build_vertices};
	use crate::scene::geometry::{Geometry, GroupKind};

	fn geometry(groups: Vec<std::ops::Range<usize>>) -> Geometry {
		Geometry {
			positions: vec![
				[0.0, 0.0, 0.0],
				[1.0, 0.0, 0.0],
				[0.0, 1.0, 0.0],
				[0.0, 0.0, 1.0],
			],
			uvs: None,
			normals: None,
			triangles: vec![[0, 1, 2], [0, 2, 3], [1, 3, 2]],
			groups,
			group_kind: GroupKind::Lod,
		}
	}

	const BARY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

	#[test]
	fn build_vertices_expands_each_group_triangle() {
		let g = geometry(vec![0..1, 1..3]);
		let (vertices, groups) = build_vertices(&g, &[[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
		assert_eq!(groups, vec![0..3, 3..9]);
		assert_eq!(vertices.len(), 9);
		let corners = g.triangles.iter().flatten();
		for (k, (v, &i)) in vertices.iter().zip(corners).enumerate() {
			assert_eq!(v.pos, g.positions[i as usize]);
			assert_eq!(v.bary, BARY[k % 3], "vertex {k}");
			// no normals or UVs: zeros
			assert_eq!((v.normal, v.uv), ([0.0; 3], [0.0; 2]));
			let colour = if k < 3 {
				[1.0, 0.0, 0.0]
			} else {
				[0.0, 0.0, 1.0]
			};
			assert_eq!(v.color, colour, "vertex {k}");
		}
	}

	#[test]
	fn build_vertices_copies_normals_and_uvs() {
		let mut g = geometry(vec![2..3]);
		g.normals = Some(vec![
			[0.0, 0.0, 1.0],
			[0.0, 1.0, 0.0],
			[1.0, 0.0, 0.0],
			[0.5; 3],
		]);
		g.uvs = Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]);
		// a group without a colour of its own is grey
		let (vertices, groups) = build_vertices(&g, &[]);
		assert_eq!(groups, vec![0..3]);
		let normals: Vec<_> = vertices.iter().map(|v| v.normal).collect();
		let uvs: Vec<_> = vertices.iter().map(|v| v.uv).collect();
		assert_eq!(normals, vec![[0.0, 1.0, 0.0], [0.5; 3], [1.0, 0.0, 0.0]]);
		assert_eq!(uvs, vec![[1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
		assert!(vertices.iter().all(|v| v.color == [0.8; 3]));
	}

	/// Only triangles in a group are drawn; with none, one zeroed vertex
	/// stands in (wgpu rejects empty buffers), drawn by no range.
	#[test]
	fn build_vertices_without_groups_gives_one_dummy_vertex() {
		for g in [geometry(Vec::new()), geometry(vec![1..1])] {
			let (vertices, groups) = build_vertices(&g, &[]);
			assert_eq!(vertices.len(), 1);
			let zero: Vertex = bytemuck::Zeroable::zeroed();
			assert_eq!(bytemuck::bytes_of(&vertices[0]), bytemuck::bytes_of(&zero));
			assert!(groups.iter().all(|r| r.is_empty()), "{groups:?}");
		}
	}

	use super::{read_rows, unblend};
	use crate::scene::colour::linear_to_srgb;

	/// One pixel drawn over black and over white, unblended.
	fn unblend_one(black: [u8; 3], white: [u8; 3], srgb: bool) -> [u8; 4] {
		let image =
			|[r, g, b]: [u8; 3]| image::RgbaImage::from_pixel(1, 1, image::Rgba([r, g, b, 255]));
		unblend(&image(black), &image(white), [1, 1], srgb)
			.get_pixel(0, 0)
			.0
	}

	fn near(actual: [u8; 4], expected: [u8; 4], by: u8) {
		let ok = (0..4).all(|k| actual[k].abs_diff(expected[k]) <= by);
		assert!(ok, "{actual:?} is not within {by} of {expected:?}");
	}

	#[test]
	fn unblend_leaves_the_background_transparent() {
		for srgb in [false, true] {
			assert_eq!(unblend_one([0; 3], [255; 3], srgb), [0; 4]);
		}
		// a difference under half a step is still background
		assert_eq!(unblend_one([0; 3], [255, 255, 254], false), [0; 4]);
	}

	#[test]
	fn unblend_keeps_opaque_colour() {
		for srgb in [false, true] {
			let c = [200, 30, 90];
			assert_eq!(unblend_one(c, c, srgb), [200, 30, 90, 255]);
		}
	}

	#[test]
	fn unblend_recovers_partial_alpha() {
		// unorm: 200/255 at half alpha leaves 100 over black, 227.5 over white
		near(
			unblend_one([100; 3], [228; 3], false),
			[200, 200, 200, 127],
			1,
		);
		// sRGB: linear 0.5 at a quarter alpha, blended in linear light
		let encode = |v: f32| (linear_to_srgb(v) * 255.0).round() as u8;
		let (black, white) = (encode(0.125), encode(0.875));
		let expected = encode(0.5);
		near(
			unblend_one([black; 3], [white; 3], true),
			[expected, expected, expected, 64],
			2,
		);
	}

	/// White darker than black (not a real blend) clamps to opaque.
	#[test]
	fn unblend_clamps_alpha_to_opaque() {
		assert_eq!(unblend_one([100; 3], [50; 3], false), [100, 100, 100, 255]);
	}

	/// Two blank readbacks agree everywhere, so they read as opaque black.
	#[test]
	fn unblend_of_two_blank_images_is_opaque_black() {
		let blank = image::RgbaImage::new(2, 2);
		let out = unblend(&blank, &blank, [2, 2], true);
		assert!(out.pixels().all(|p| p.0 == [0, 0, 0, 255]));
	}

	#[test]
	fn read_rows_skips_row_padding_and_swaps_bgra() {
		// two pixels per line, padded to 12 bytes
		let data = [
			1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, //
			9, 10, 11, 12, 13, 14, 15, 16, 99, 99, 99, 99,
		];
		let rgba = read_rows(&data, 12, [2, 2], false);
		assert_eq!(rgba.get_pixel(1, 0).0, [5, 6, 7, 8]);
		assert_eq!(rgba.get_pixel(0, 1).0, [9, 10, 11, 12]);
		let bgra = read_rows(&data, 12, [2, 2], true);
		assert_eq!(bgra.get_pixel(0, 0).0, [3, 2, 1, 4]);
		assert_eq!(bgra.get_pixel(1, 1).0, [15, 14, 13, 16]);
	}
}
