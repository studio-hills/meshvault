// Mesh shading for the viewer: physically based lighting (GGX specular,
// Lambert diffuse) with optional SurfaceAppearance-style maps, plus a
// barycentric wireframe overlay.

struct Uniforms {
	view_proj: mat4x4<f32>,
	// xyz: camera position
	eye: vec4<f32>,
	// xyz: direction towards the key light
	light_dir: vec4<f32>,
	// rgb: linear wire colour, a: overlay strength
	wire_color: vec4<f32>,
	// x: wire mode (0 none, 1 overlay, 2 wire only)
	// y: 1 when the target expects gamma-encoded output
	// z: roughness used without a roughness map
	params: vec4<f32>,
	// has base, normal, roughness, metalness map
	flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var base_tex: texture_2d<f32>;
@group(0) @binding(3) var normal_tex: texture_2d<f32>;
@group(0) @binding(4) var rough_tex: texture_2d<f32>;
@group(0) @binding(5) var metal_tex: texture_2d<f32>;

struct VsIn {
	@location(0) pos: vec3<f32>,
	@location(1) normal: vec3<f32>,
	@location(2) uv: vec2<f32>,
	@location(3) bary: vec3<f32>,
	@location(4) color: vec3<f32>,
};

struct VsOut {
	@builtin(position) clip: vec4<f32>,
	@location(0) world: vec3<f32>,
	@location(1) normal: vec3<f32>,
	@location(2) uv: vec2<f32>,
	@location(3) bary: vec3<f32>,
	@location(4) color: vec3<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
	var out: VsOut;
	out.clip = u.view_proj * vec4<f32>(v.pos, 1.0);
	out.world = v.pos;
	out.normal = v.normal;
	out.uv = v.uv;
	out.bary = v.bary;
	out.color = v.color;
	return out;
}

const PI: f32 = 3.14159265;

// Cook-Torrance GGX specular + Lambert diffuse for one directional light.
fn shade_light(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, radiance: vec3<f32>,
		albedo: vec3<f32>, rough: f32, metal: f32, f0: vec3<f32>) -> vec3<f32> {
	let h = normalize(v + l);
	let nl = max(dot(n, l), 0.0);
	let nv = max(dot(n, v), 1e-4);
	let nh = max(dot(n, h), 0.0);
	let vh = max(dot(v, h), 0.0);
	let a = rough * rough;
	let a2 = a * a;
	let d0 = nh * nh * (a2 - 1.0) + 1.0;
	let d = a2 / (PI * d0 * d0);
	let k = (rough + 1.0) * (rough + 1.0) / 8.0;
	let g = (nv / (nv * (1.0 - k) + k)) * (nl / (nl * (1.0 - k) + k));
	let f = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - vh, 5.0);
	let spec = d * g * f / max(4.0 * nv * nl, 1e-4);
	let kd = (vec3<f32>(1.0) - f) * (1.0 - metal);
	return (kd * albedo / PI + spec) * radiance * nl;
}

// Narkowicz's ACES fit: keeps highlights from clipping harshly.
fn aces(x: vec3<f32>) -> vec3<f32> {
	return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
	// Sample and take derivatives first: both need uniform control flow.
	let base_s = textureSample(base_tex, samp, in.uv);
	let normal_s = textureSample(normal_tex, samp, in.uv).xyz;
	let rough_s = textureSample(rough_tex, samp, in.uv).r;
	let metal_s = textureSample(metal_tex, samp, in.uv).r;
	// Screen-space derivatives along +x (right) and +y (up). wgpu's framebuffer
	// y points down, so dpdy is negated to match the OpenGL convention the
	// tangent frame below was derived in.
	let dp1 = dpdx(in.world);
	let dp2 = -dpdy(in.world);
	let duv1 = dpdx(in.uv);
	let duv2 = -dpdy(in.uv);
	let bary_width = fwidth(in.bary);

	var n: vec3<f32>;
	if (dot(in.normal, in.normal) < 1e-8) {
		// no vertex normal (e.g. physics hulls): the face normal, which
		// right x up always points at the camera
		n = normalize(cross(dp1, dp2));
	} else {
		n = normalize(in.normal);
		if (!front) {
			n = -n;
		}
	}

	// SurfaceAppearance "Overlay": the colour map's alpha blends over the part colour
	var albedo = in.color;
	if (u.flags.x != 0u) {
		albedo = mix(in.color, base_s.rgb, base_s.a);
	}

	if (u.flags.y != 0u) {
		// Tangent frame from screen-space derivatives (no tangent data needed).
		let dp2perp = cross(dp2, n);
		let dp1perp = cross(n, dp1);
		let t = dp2perp * duv1.x + dp1perp * duv2.x;
		let b = dp2perp * duv1.y + dp1perp * duv2.y;
		let inv = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
		var ts = normal_s * 2.0 - vec3<f32>(1.0);
		// Normal maps point +Y up the image, but mesh V grows downwards.
		ts.y = -ts.y;
		n = normalize(t * inv * ts.x + b * inv * ts.y + n * ts.z);
	}

	let rough = select(u.params.z, clamp(rough_s, 0.04, 1.0), u.flags.z != 0u);
	let metal = select(0.0, metal_s, u.flags.w != 0u);
	let v = normalize(u.eye.xyz - in.world);
	let f0 = mix(vec3<f32>(0.04), albedo, metal);

	let key = normalize(u.light_dir.xyz);
	let fill = normalize(vec3<f32>(-key.x, 0.35, -key.z));
	var color = shade_light(n, v, key, vec3<f32>(2.8), albedo, rough, metal, f0);
	color += shade_light(n, v, fill, vec3<f32>(0.7), albedo, rough, metal, f0);

	// hemisphere ambient, with a rough environment reflection for metals
	let sky = vec3<f32>(0.45, 0.48, 0.55);
	let ground = vec3<f32>(0.2, 0.18, 0.16);
	let ambient = mix(ground, sky, n.y * 0.5 + 0.5);
	let nv = max(dot(n, v), 0.0);
	let fresnel = f0 + (max(vec3<f32>(1.0 - rough), f0) - f0) * pow(1.0 - nv, 5.0);
	color += ambient * albedo * (1.0 - metal) + ambient * fresnel * (1.0 - 0.7 * rough);
	color = aces(color);

	// wireframe from barycentric coordinates, about one pixel wide
	let along = smoothstep(vec3<f32>(0.0), bary_width * 1.2, in.bary);
	let edge = 1.0 - min(min(along.x, along.y), along.z);
	if (u.params.x > 1.5) {
		if (edge < 0.05) {
			discard;
		}
		color = u.wire_color.rgb;
	} else if (u.params.x > 0.5) {
		color = mix(color, u.wire_color.rgb, edge * u.wire_color.a);
	}

	if (u.params.y > 0.5) {
		color = pow(color, vec3<f32>(1.0 / 2.2));
	}
	return vec4<f32>(color, 1.0);
}
