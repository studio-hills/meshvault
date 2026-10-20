//! GPU mesh rendering through eframe's wgpu backend: depth-tested,
//! perspective-correct, mipmapped, with physically based shading of
//! SurfaceAppearance maps. Drawn inside egui via a paint callback.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui_wgpu::{self, wgpu};
use wgpu::util::DeviceExt;

use crate::geometry::Geometry;
use crate::maps::Slot;
use crate::texture::Material;

/// Must match `NativeOptions` in main.
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
	/// (0 Overlay, 1 Transparency, 2 TintMask, 3 Opaque).
	pub tint: [f32; 4],
	/// Decal draws: mesh to part space (column-major), then mode (0 none,
	/// 1 Decal, 2 Texture), face, transparency; the part's size; studs per
	/// tile and offset (U, V, U, V); and the linear tint.
	pub part_from_mesh: [[f32; 4]; 4],
	pub decal: [f32; 4],
	pub decal_size: [f32; 4],
	pub decal_tile: [f32; 4],
	pub decal_colour: [f32; 4],
	/// The model's lights (see `viewport::light_uniforms`), and how many.
	pub lights: [[f32; 4]; crate::viewport::MAX_LIGHTS * 3],
	pub light_count: [f32; 4],
}

pub struct GpuRenderer {
	device: wgpu::Device,
	queue: wgpu::Queue,
	/// [opaque, see-through, decal][no culling, back-face culling]
	pipelines: [[wgpu::RenderPipeline; 2]; 3],
	layout: wgpu::BindGroupLayout,
	sampler: wgpu::Sampler,
	/// Bound to slots without a map.
	white: wgpu::TextureView,
	flat_normal: wgpu::TextureView,
	/// egui renders to a non-sRGB target and expects gamma-encoded colour.
	gamma_output: bool,
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

fn srgb_to_linear(c: u8) -> f32 {
	let c = c as f32 / 255.0;
	if c <= 0.04045 {
		c / 12.92
	} else {
		((c + 0.055) / 1.055).powf(2.4)
	}
}

pub fn linear_rgb(c: eframe::egui::Color32) -> [f32; 3] {
	[
		srgb_to_linear(c.r()),
		srgb_to_linear(c.g()),
		srgb_to_linear(c.b()),
	]
}

impl GpuRenderer {
	pub fn new(render_state: &egui_wgpu::RenderState) -> Arc<Self> {
		let device = render_state.device.clone();
		let queue = render_state.queue.clone();
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("rbx_mesh shader"),
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
			label: Some("rbx_mesh bind group layout"),
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
			label: Some("rbx_mesh pipeline layout"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let target_format = render_state.target_format;
		// see-through parts blend over what is behind them and leave the depth
		// buffer alone, so they are drawn last, from the back to the front;
		// decals blend over their part's own surface, at the same depth
		let pipeline = |see_through: bool, decal: bool, cull_mode| {
			device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
				label: Some("rbx_mesh pipeline"),
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
					alpha_to_coverage_enabled: false,
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
		let pipelines =
			[(false, false), (true, false), (true, true)].map(|(see_through, decal)| {
				[
					pipeline(see_through, decal, None),
					pipeline(see_through, decal, Some(wgpu::Face::Back)),
				]
			});
		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("rbx_mesh sampler"),
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
				label: Some("rbx_mesh vertices"),
				contents: bytemuck::cast_slice(&vertices),
				usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
			});
		let uniforms = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("rbx_mesh uniforms"),
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
			label: Some("rbx_mesh bind group"),
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
				Draw {
					pass: if uniforms.decal[0] > 0.0 {
						Pass::Decal
					} else if uniforms.surface[0] < 1.0 {
						Pass::SeeThrough
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
			pipelines: self.pipelines.each_ref().map(|p| p[cull as usize].clone()),
			draws,
		}
	}
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
		label: Some("rbx_mesh map"),
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

/// Drawn in this order: opaque surfaces, the decals on them, then
/// see-through surfaces.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
	Opaque,
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
	/// [opaque, see-through, decal]
	pipelines: [wgpu::RenderPipeline; 3],
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
		for (pass, pipeline) in [(Pass::Opaque, 0), (Pass::Decal, 2), (Pass::SeeThrough, 1)] {
			render_pass.set_pipeline(&self.pipelines[pipeline]);
			for draw in self.draws.iter().filter(|d| d.pass == pass) {
				render_pass.set_bind_group(0, &draw.bind_group, &[]);
				render_pass.set_vertex_buffer(0, draw.vertices.slice(..));
				for range in &draw.ranges {
					if !range.is_empty() {
						render_pass.draw(range.clone(), 0..1);
					}
				}
			}
		}
	}
}

#[cfg(test)]
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

	/// The Rust uniform struct must match the WGSL one byte for byte.
	#[test]
	fn uniform_layout_matches_shader() {
		let module = naga::front::wgsl::parse_str(include_str!("mesh.wgsl")).unwrap();
		let size = module
			.types
			.iter()
			.find(|(_, t)| t.name.as_deref() == Some("Uniforms"))
			.map(|(_, t)| t.inner.size(module.to_ctx()))
			.unwrap();
		assert_eq!(size as usize, std::mem::size_of::<super::Uniforms>());
	}
}
