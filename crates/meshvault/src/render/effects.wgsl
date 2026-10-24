// Effects for the viewer: particles, beams and trails, drawn unlit in their
// own colour over the scene. The output is premultiplied: LightEmission
// moves a fragment from blending over what is behind (0) to adding to it (1).

struct Uniforms {
	view_proj: mat4x4<f32>,
	// x: 1 when the target expects gamma-encoded output
	params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var tex: texture_2d<f32>;

struct VsIn {
	@location(0) pos: vec3<f32>,
	@location(1) emission: f32,
	@location(2) uv: vec2<f32>,
	@location(3) color: vec4<f32>,
};

struct VsOut {
	@builtin(position) clip: vec4<f32>,
	@location(0) uv: vec2<f32>,
	@location(1) color: vec4<f32>,
	@location(2) emission: f32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
	var out: VsOut;
	out.clip = u.view_proj * vec4<f32>(v.pos, 1.0);
	out.uv = v.uv;
	out.color = v.color;
	out.emission = v.emission;
	return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
	let s = textureSample(tex, samp, in.uv);
	var rgb = min(s.rgb * in.color.rgb, vec3<f32>(1.0));
	let a = clamp(s.a * in.color.a, 0.0, 1.0);
	if (u.params.x > 0.5) {
		rgb = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
	}
	return vec4<f32>(rgb * a, a * (1.0 - clamp(in.emission, 0.0, 1.0)));
}
