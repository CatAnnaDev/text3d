struct Globals {
    view_proj: mat4x4<f32>,
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
};

@group(0) @binding(0) var<uniform> g: Globals;

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

@fragment
fn grid_fs(in: GridOut) -> @location(0) vec4<f32> {
    let cell = g.ground.y;
    let minor = grid_line(in.world.xz / cell);
    let major = grid_line(in.world.xz / (cell * 8.0));

    let to_eye = g.camera_pos.xyz - in.world;
    let dist = length(to_eye);
    let grazing = smoothstep(0.0, 0.42, abs(normalize(to_eye).y));
    let fade = 1.0 - smoothstep(g.ground.z, g.ground.w, dist);

    let strength = max(minor * 0.13, major * 0.34) * fade * grazing;
    if (strength < 0.002) {
        discard;
    }
    let color = mix(g.ink.rgb * 0.55, g.accent.rgb, major * 0.8);
    return vec4<f32>(color * strength, strength);
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

@fragment
fn panel_fs(in: PanelOut) -> @location(0) vec4<f32> {
    return in.color;
}
