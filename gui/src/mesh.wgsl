// Mesh shading for the viewer: physically based lighting (GGX specular,
// Lambert diffuse) with optional SurfaceAppearance-style maps, plus a
// barycentric wireframe overlay.

struct Uniforms {
	view_proj: mat4x4<f32>,
	// mesh to world, and its inverse transpose for normals
	model: mat4x4<f32>,
	normal_model: mat4x4<f32>,
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
	// x: opacity (1 - Transparency), y: 1 for Neon, z: Reflectance,
	// w: 1 for metal materials, whose reflections take the part's colour
	surface: vec4<f32>,
	// SurfaceAppearance: rgb tints the colour map; w is the alpha mode
	// (0 Overlay, 1 Transparency, 2 TintMask, 3 Opaque)
	tint: vec4<f32>,
	// decal draws: mesh to part space
	part_from_mesh: mat4x4<f32>,
	// x: 0 no decal, 1 Decal, 2 Texture, 3 MaterialVariant, 4 clothing;
	// y: face (Enum.NormalId);
	// z: transparency
	decal: vec4<f32>,
	// xyz: the part's size in studs
	decal_size: vec4<f32>,
	// Textures: studs per tile U, V and offset studs U, V
	decal_tile: vec4<f32>,
	// rgb: linear tint (Color3)
	decal_colour: vec4<f32>,
	// the model's lights, three each: position and range; linear colour times
	// brightness and 1 for a cone; direction and the cone's half angle cosine
	lights: array<vec4<f32>, 24>,
	// x: how many lights
	light_count: vec4<f32>,
	// clothing: each box face's template region (x, y, w, h), then the
	// box's centre and half size
	clothing: array<vec4<f32>, 8>,
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
	// the position and normal in the part's own space, for decals
	@location(5) local: vec3<f32>,
	@location(6) local_normal: vec3<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
	var out: VsOut;
	let world = (u.model * vec4<f32>(v.pos, 1.0)).xyz;
	out.clip = u.view_proj * vec4<f32>(world, 1.0);
	out.world = world;
	// stays zero for meshes without normals, which the fragment stage checks
	out.normal = (u.normal_model * vec4<f32>(v.normal, 0.0)).xyz;
	out.uv = v.uv;
	out.bary = v.bary;
	out.color = v.color;
	out.local = (u.part_from_mesh * vec4<f32>(v.pos, 1.0)).xyz;
	out.local_normal = (u.part_from_mesh * vec4<f32>(v.normal, 0.0)).xyz;
	return out;
}

// A face's image axes in part space (measured in Studio): right, down, and
// the face's outward normal, for Enum.NormalId Right, Top, Back, Left,
// Bottom, Front. Seen from outside, side faces read upright with +Y up.
fn decal_axes(face: i32) -> mat3x3<f32> {
	switch face {
		case 0: { return mat3x3<f32>(vec3(0.0, 0.0, -1.0), vec3(0.0, -1.0, 0.0), vec3(1.0, 0.0, 0.0)); }
		case 1: { return mat3x3<f32>(vec3(-1.0, 0.0, 0.0), vec3(0.0, 0.0, -1.0), vec3(0.0, 1.0, 0.0)); }
		case 2: { return mat3x3<f32>(vec3(1.0, 0.0, 0.0), vec3(0.0, -1.0, 0.0), vec3(0.0, 0.0, 1.0)); }
		case 3: { return mat3x3<f32>(vec3(0.0, 0.0, 1.0), vec3(0.0, -1.0, 0.0), vec3(-1.0, 0.0, 0.0)); }
		case 4: { return mat3x3<f32>(vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, -1.0), vec3(0.0, -1.0, 0.0)); }
		default: { return mat3x3<f32>(vec3(-1.0, 0.0, 0.0), vec3(0.0, -1.0, 0.0), vec3(0.0, 0.0, -1.0)); }
	}
}

const PI: f32 = 3.14159265;
// a Roblox light's Brightness to this renderer's radiance
const LIGHT_SCALE: f32 = 3.0;
// cosine of the steepest angle a surface may turn from a decal and still take it
const DECAL_FACING: f32 = 0.55;

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

// The face a MaterialVariant tiles on: the box face the surface turns towards.
fn box_face(n: vec3<f32>) -> i32 {
	let a = abs(n);
	if (a.x >= a.y && a.x >= a.z) {
		return select(3, 0, n.x > 0.0);
	}
	if (a.y >= a.z) {
		return select(4, 1, n.y > 0.0);
	}
	return select(5, 2, n.z > 0.0);
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
	// the surface's own normal in part space, for geometry without normals;
	// it faces the camera, like the face normal below
	let local_face = cross(dpdx(in.local), -dpdy(in.local));
	let variant = u.decal.x > 2.5 && u.decal.x < 3.5;
	let clothing = u.decal.x > 3.5;
	// where a decal's image lands: measured from the face's top-left corner.
	// A MaterialVariant tiles on whichever box face the surface turns to.
	var face = i32(u.decal.y);
	if (variant || clothing) {
		face = box_face(select(local_face, in.local_normal, dot(in.local_normal, in.local_normal) > 1e-8));
	}
	var axes = decal_axes(face);
	// a clothing template unfolds the box with the bottom below the front
	if (clothing && face == 4) {
		axes = mat3x3<f32>(vec3(-1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(0.0, -1.0, 0.0));
	}
	var size = u.decal_size.xyz;
	var at = in.local;
	if (clothing) {
		// the body section's box, which the part is one piece of
		size = u.clothing[7].xyz * 2.0;
		at = in.local - u.clothing[6].xyz;
	}
	let face_size = vec2<f32>(abs(dot(size, axes[0])), abs(dot(size, axes[1])));
	let from_corner = vec2<f32>(dot(at, axes[0]), dot(at, axes[1])) + face_size * 0.5;
	var decal_uv = from_corner / max(face_size, vec2<f32>(1e-6));
	if (clothing) {
		// the face's region of the template
		let region = u.clothing[face];
		decal_uv = region.xy + clamp(decal_uv, vec2<f32>(0.0), vec2<f32>(1.0)) * region.zw;
	} else if (u.decal.x > 1.5) {
		// a Texture (or a MaterialVariant) tiles from the corner, moved by its offset
		decal_uv = (from_corner + u.decal_tile.zw) / max(u.decal_tile.xy, vec2<f32>(1e-6));
	}

	// a MaterialVariant's maps are read where its tiles land
	let map_uv = select(in.uv, decal_uv, variant);

	// Sample and take derivatives first: both need uniform control flow.
	let base_s = textureSample(base_tex, samp, map_uv);
	let decal_s = textureSample(base_tex, samp, decal_uv);
	let normal_s = textureSample(normal_tex, samp, map_uv).xyz;
	let rough_s = textureSample(rough_tex, samp, map_uv).r;
	let metal_s = textureSample(metal_tex, samp, map_uv).r;
	// Screen-space derivatives along +x (right) and +y (up). wgpu's framebuffer
	// y points down, so dpdy is negated to match the OpenGL convention the
	// tangent frame below was derived in.
	let dp1 = dpdx(in.world);
	let dp2 = -dpdy(in.world);
	let duv1 = dpdx(map_uv);
	let duv2 = -dpdy(map_uv);
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

	// the colour map, by its SurfaceAppearance alpha mode (as measured in Studio)
	var albedo = in.color;
	var cut = false;
	var alpha = u.surface.x;
	if (u.flags.x != 0u) {
		let tinted = base_s.rgb * u.tint.rgb;
		let mode = u.tint.w;
		if (mode < 0.5) {
			// Overlay: the map over the part colour by its alpha
			albedo = mix(in.color, tinted, base_s.a);
		} else if (mode < 1.5) {
			// Transparency: cut away below half alpha
			albedo = tinted;
			cut = base_s.a < 0.5;
		} else if (mode < 2.5) {
			// TintMask: alpha says where the tint applies
			albedo = base_s.rgb * mix(vec3<f32>(1.0), u.tint.rgb, base_s.a);
		} else {
			albedo = tinted;
		}
	}

	if (clothing) {
		// clothing over the body, tinted by its Color3; faces without a region stay bare
		albedo = decal_s.rgb * u.decal_colour.rgb;
		alpha = decal_s.a;
		cut = u.clothing[face].z <= 0.0;
	} else if (variant) {
		// a MaterialVariant: its colour map tinted by the part's colour, opaque
		if (u.flags.x != 0u) {
			albedo = base_s.rgb * in.color;
		}
		alpha = 1.0;
	} else if (u.decal.x > 0.5) {
		// a decal: its image over the part's own face only
		albedo = decal_s.rgb * u.decal_colour.rgb;
		alpha = decal_s.a * (1.0 - u.decal.z);
		let outside = any(decal_uv < vec2<f32>(0.0)) || any(decal_uv > vec2<f32>(1.0));
		// Roblox decides per triangle (its own normal, not the smoothed one), and
		// leaves surfaces turned more than about CUTOFF away from the decal bare
		let facing = dot(local_face, local_face) > 1e-12
			&& dot(normalize(local_face), axes[2]) > DECAL_FACING;
		cut = !facing || (u.decal.x < 1.5 && outside);
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

	// the model's own lights: fading out to their range, cones softening at
	// their edge
	for (var i = 0; i < i32(u.light_count.x); i++) {
		let at = u.lights[i * 3];
		let tint = u.lights[i * 3 + 1];
		let aim = u.lights[i * 3 + 2];
		let to_light = at.xyz - in.world;
		let d = length(to_light);
		if (d < at.w && d > 1e-4) {
			let l = to_light / d;
			var fade = 1.0 - d / at.w;
			fade = fade * fade;
			if (tint.w > 0.5) {
				fade *= smoothstep(aim.w, min(aim.w + 0.1, 1.0), dot(-l, normalize(aim.xyz)));
			}
			color += shade_light(n, v, l, tint.rgb * fade * LIGHT_SCALE, albedo, rough, metal, f0);
		}
	}

	// hemisphere ambient, with a rough environment reflection for metals
	let sky = vec3<f32>(0.45, 0.48, 0.55);
	let ground = vec3<f32>(0.2, 0.18, 0.16);
	let ambient = mix(ground, sky, n.y * 0.5 + 0.5);
	let nv = max(dot(n, v), 0.0);
	let fresnel = f0 + (max(vec3<f32>(1.0 - rough), f0) - f0) * pow(1.0 - nv, 5.0);
	color += ambient * albedo * (1.0 - metal) + ambient * fresnel * (1.0 - 0.7 * rough);
	// Reflectance mirrors the sky: Roblox blends the reflection over the part
	if (u.surface.z > 0.0) {
		let r = reflect(-v, n);
		let env = mix(ground * 0.6, sky * 1.4, smoothstep(-0.2, 0.3, r.y));
		let tint = mix(vec3<f32>(1.0), albedo * 1.6, u.surface.w);
		color = mix(color, env * tint, u.surface.z);
	}
	color = aces(color);
	// Neon glows in its own colour whatever the light, untouched by tone mapping
	if (u.surface.y > 0.5) {
		color = min(albedo * 1.25, vec3<f32>(1.0));
	}

	if (cut) {
		discard;
	}

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
	return vec4<f32>(color, alpha);
}
