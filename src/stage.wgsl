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

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn bg_vs(@builtin(vertex_index) index: u32) -> BgOut {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    var out: BgOut;
    out.clip = vec4<f32>(x, y, 1.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn bg_fs(in: BgOut) -> @location(0) vec4<f32> {
    let t = clamp(in.uv.y * 0.5 + 0.5, 0.0, 1.0);
    var color = mix(g.bg_bottom.rgb, g.bg_top.rgb, t);
    let vignette = 1.0 - 0.35 * dot(in.uv, in.uv) * 0.5;
    color *= vignette;
    return vec4<f32>(color, 1.0);
}

struct GridOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn grid_vs(@builtin(vertex_index) index: u32) -> GridOut {
    let extent = g.ground.w * 1.3;
    let x = select(-1.0, 1.0, (index & 1u) == 1u);
    let z = select(-1.0, 1.0, (index & 2u) == 2u);
    var out: GridOut;
    out.world = vec3<f32>(g.camera_pos.x + x * extent, g.ground.x, g.camera_pos.z + z * extent);
    out.clip = g.view_proj * vec4<f32>(out.world, 1.0);
    return out;
}

fn grid_line(coord: vec2<f32>) -> f32 {
    let deriv = fwidth(coord);
    let wrapped = abs(fract(coord - 0.5) - 0.5) / max(deriv, vec2<f32>(1e-6));
    return 1.0 - min(min(wrapped.x, wrapped.y), 1.0);
}

fn ground_lit(world: vec3<f32>) -> f32 {
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
    let reference = ndc.z - 0.0009;
    let texel = g.shadow.x * 1.7;
    var sum = 0.0;
    for (var row = -1; row <= 1; row = row + 1) {
        for (var column = -1; column <= 1; column = column + 1) {
            let tap = uv + vec2<f32>(f32(column), f32(row)) * texel;
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_sampler, tap, reference);
        }
    }
    return mix(1.0, sum * (1.0 / 9.0), smoothstep(0.0, 0.05, edge));
}

@fragment
fn grid_fs(in: GridOut) -> @location(0) vec4<f32> {
    let cell = g.ground.y;
    let minor = grid_line(in.world.xz / cell);
    let major = grid_line(in.world.xz / (cell * 8.0));

    let to_eye = g.camera_pos.xyz - in.world;
    let dist = length(to_eye);
    let grazing = smoothstep(0.0, 0.42, abs(normalize(to_eye).y));
    let fade = 1.0 - smoothstep(g.ground.z, g.ground.w, dist);

    let lit = ground_lit(in.world);
    let contact = (1.0 - lit) * g.shadow.z * fade;
    let strength = max(minor * 0.13, major * 0.34) * fade * grazing * mix(0.4, 1.0, lit);
    let alpha = min(strength + contact, 1.0);
    if (alpha < 0.002) {
        discard;
    }
    let color = mix(g.ink.rgb * 0.55, g.accent.rgb, major * 0.8);
    return vec4<f32>(color * strength, alpha);
}

struct PanelOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn panel_vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> PanelOut {
    var out: PanelOut;
    out.clip = g.view_proj * vec4<f32>(position, 1.0);
    out.color = color;
    return out;
}

@vertex
fn find_panel_vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> PanelOut {
    let right = g.screen_right.xyz;
    let up = g.screen_up.xyz;
    let toward = cross(right, up);
    let world = g.find_anchor.xyz
        + (right * position.x + up * position.y + toward * position.z) * g.find_anchor.w;
    var out: PanelOut;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.color = color;
    return out;
}

@fragment
fn panel_fs(in: PanelOut) -> @location(0) vec4<f32> {
    return in.color;
}
