struct Globals {
    view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>,
    fog: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    ink: vec4<f32>,
    accent: vec4<f32>,
    highlight: vec4<f32>,
    ground: vec4<f32>,
    params: vec4<f32>,
    shadow: vec4<f32>,
    find_anchor: vec4<f32>,
    screen_right: vec4<f32>,
    screen_up: vec4<f32>,
    hud: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var shadow_map: texture_depth_2d;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;

struct Placed {
    world: vec3<f32>,
    line_factor: f32,
};

fn place_glyph(position: vec3<f32>, scale: vec2<f32>, offset: vec3<f32>) -> Placed {
    let scaled = vec3<f32>(position.x * scale.x, position.y * scale.y, position.z);
    let wave = g.params.y * sin(g.params.x * 1.7 + offset.x * 0.55 + offset.y * 0.9);
    let dy = abs(offset.y - g.fog.z);
    let line_factor = 1.0 - smoothstep(0.0, g.fog.w * 0.6, dy);
    let lift = line_factor * 0.34;
    var placed: Placed;
    placed.world = scaled + offset + vec3<f32>(0.0, 0.0, wave + lift);
    placed.line_factor = line_factor;
    return placed;
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) line_factor: f32,
    @location(4) shadowed: f32,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) offset: vec3<f32>,
    @location(3) scale: vec2<f32>,
    @location(4) color: vec4<f32>,
) -> VsOut {
    let placed = place_glyph(position, scale, offset);
    var out: VsOut;
    out.world = placed.world;
    out.clip = g.view_proj * vec4<f32>(out.world, 1.0);
    out.normal = normal;
    out.color = color;
    out.line_factor = placed.line_factor;
    out.shadowed = 1.0;
    return out;
}

@vertex
fn vs_popup(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) offset: vec3<f32>,
    @location(3) scale: vec2<f32>,
    @location(4) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.world = vec3<f32>(position.x * scale.x, position.y * scale.y, position.z) + offset;
    out.clip = g.view_proj * vec4<f32>(out.world, 1.0);
    out.normal = normal;
    out.color = color;
    out.line_factor = 0.0;
    out.shadowed = 0.0;
    return out;
}

@vertex
fn vs_find(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) offset: vec3<f32>,
    @location(3) scale: vec2<f32>,
    @location(4) color: vec4<f32>,
) -> VsOut {
    let local = vec3<f32>(position.x * scale.x, position.y * scale.y, position.z) + offset;
    let right = g.screen_right.xyz;
    let up = g.screen_up.xyz;
    let toward = cross(right, up);
    var out: VsOut;
    out.world = g.find_anchor.xyz
        + (right * local.x + up * local.y + toward * local.z) * g.find_anchor.w;
    out.clip = g.view_proj * vec4<f32>(out.world, 1.0);
    out.normal = right * normal.x + up * normal.y + toward * normal.z;
    out.color = color;
    out.line_factor = 0.0;
    out.shadowed = 0.0;
    return out;
}

const HUD_BASE_DEPTH: f32 = 0.30;
const HUD_DEPTH_SCALE: f32 = 0.02;

struct HudOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_hud(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) offset: vec3<f32>,
    @location(3) scale: vec2<f32>,
    @location(4) color: vec4<f32>,
) -> HudOut {
    let local = vec3<f32>(position.x * scale.x, position.y * scale.y, position.z) + offset;
    var out: HudOut;
    out.clip = vec4<f32>(
        local.x * g.hud.x + g.hud.y,
        local.y * g.hud.z + g.hud.w,
        HUD_BASE_DEPTH - local.z * HUD_DEPTH_SCALE,
        1.0,
    );
    out.normal = normal;
    out.color = color;
    return out;
}

fn shadow_lit(world: vec3<f32>, n_dot_l: f32) -> f32 {
    if (g.shadow.y < 0.5) {
        return 1.0;
    }
    let light_clip = g.light_view_proj * vec4<f32>(world, 1.0);
    if (light_clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = light_clip.xyz / light_clip.w;
    if (ndc.z <= 0.0 || ndc.z >= 1.0) {
        return 1.0;
    }
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    let edge = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y));
    if (edge <= 0.0) {
        return 1.0;
    }
    let bias = max(0.0035 * (1.0 - n_dot_l), 0.0009);
    let reference = ndc.z - bias;
    let texel = g.shadow.x;
    var sum = 0.0;
    for (var row = -1; row <= 1; row = row + 1) {
        for (var column = -1; column <= 1; column = column + 1) {
            let tap = uv + vec2<f32>(f32(column), f32(row)) * texel;
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_sampler, tap, reference);
        }
    }
    return mix(1.0, sum * (1.0 / 9.0), smoothstep(0.0, 0.05, edge));
}

fn ggx_specular(n: vec3<f32>, view: vec3<f32>, light: vec3<f32>, roughness: f32) -> f32 {
    let half_vector = normalize(view + light);
    let n_dot_h = max(dot(n, half_vector), 0.0);
    let n_dot_v = max(dot(n, view), 1e-4);
    let n_dot_l = max(dot(n, light), 0.0);
    let alpha = roughness * roughness;
    let alpha2 = alpha * alpha;
    let denominator = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
    let distribution = alpha2 / max(3.14159265 * denominator * denominator, 1e-6);
    let k = alpha * 0.5;
    let occlusion_v = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let occlusion_l = n_dot_l / (n_dot_l * (1.0 - k) + k);
    let fresnel = 0.05 + 0.95 * pow(1.0 - max(dot(half_vector, view), 0.0), 5.0);
    return distribution * occlusion_v * occlusion_l * fresnel;
}

fn backdrop(frag: vec4<f32>) -> vec3<f32> {
    let t = clamp(1.0 - frag.y / max(g.params.w, 1.0), 0.0, 1.0);
    return mix(g.bg_bottom.rgb, g.bg_top.rgb, t);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let view = normalize(g.camera_pos.xyz - in.world);

    let base = mix(in.color.rgb, g.highlight.rgb, in.line_factor * 0.75);

    let key_dir = normalize(g.light_dir.xyz);
    let key = max(dot(n, key_dir), 0.0);
    let lit = mix(1.0, shadow_lit(in.world, key), in.shadowed);

    let facing = abs(n.z);
    let bevel = clamp(1.0 - abs(facing - 0.7071) * 2.4, 0.0, 1.0);
    let roughness = mix(0.28, 0.13, bevel);
    let spec = ggx_specular(n, view, key_dir, roughness) * key * lit;

    let fill_dir = normalize(vec3<f32>(-0.55, -0.25, -0.5));
    let fill = max(dot(n, fill_dir), 0.0) * 0.35;

    let hemi = mix(vec3<f32>(0.035, 0.045, 0.075), vec3<f32>(0.14, 0.18, 0.27), n.y * 0.5 + 0.5);
    let rim = pow(1.0 - max(dot(n, view), 0.0), 3.0) * 0.45;

    var color = base * (hemi
        + vec3<f32>(1.0, 0.97, 0.92) * key * lit * 0.95
        + vec3<f32>(0.35, 0.5, 0.8) * fill);
    color += vec3<f32>(1.0, 0.98, 0.95) * spec * 1.9;
    color += g.accent.rgb * rim * (0.28 + in.line_factor * 0.8);

    let dist = distance(in.world, g.camera_pos.xyz);
    let haze = smoothstep(g.fog.x, g.fog.y, dist);
    color = mix(color, backdrop(in.clip), haze);

    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_flat(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}

@fragment
fn fs_hud(in: HudOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let view = vec3<f32>(0.0, 0.0, 1.0);
    let key_dir = normalize(vec3<f32>(-0.34, 0.48, 0.81));
    let key = max(dot(n, key_dir), 0.0);
    let face = max(n.z, 0.0);
    let spec = ggx_specular(n, view, key_dir, 0.24) * key;
    var color = in.color.rgb * (0.46 + 0.32 * key + 0.30 * face);
    color = color + vec3<f32>(1.0, 0.98, 0.94) * spec * 0.8;
    return vec4<f32>(color, in.color.a);
}

@fragment
fn fs_hud_flat(in: HudOut) -> @location(0) vec4<f32> {
    return in.color;
}
