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

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) line_factor: f32,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) offset: vec3<f32>,
    @location(3) color: vec4<f32>,
) -> VsOut {
    let wave = g.params.y * sin(g.params.x * 1.7 + offset.x * 0.55 + offset.y * 0.9);
    let dy = abs(offset.y - g.fog.z);
    let line_factor = 1.0 - smoothstep(0.0, g.fog.w * 0.6, dy);
    let lift = line_factor * 0.34;

    var out: VsOut;
    out.world = position + offset + vec3<f32>(0.0, 0.0, wave + lift);
    out.clip = g.view_proj * vec4<f32>(out.world, 1.0);
    out.normal = normal;
    out.color = color.rgb;
    out.line_factor = line_factor;
    return out;
}

@vertex
fn vs_popup(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) offset: vec3<f32>,
    @location(3) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.world = position + offset;
    out.clip = g.view_proj * vec4<f32>(out.world, 1.0);
    out.normal = normal;
    out.color = color.rgb;
    out.line_factor = 0.0;
    return out;
}

fn backdrop(frag: vec4<f32>) -> vec3<f32> {
    let t = clamp(1.0 - frag.y / max(g.params.w, 1.0), 0.0, 1.0);
    return mix(g.bg_bottom.rgb, g.bg_top.rgb, t);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let view = normalize(g.camera_pos.xyz - in.world);

    var base = mix(in.color, g.highlight.rgb, in.line_factor * 0.75);

    let key_dir = normalize(g.light_dir.xyz);
    let key = max(dot(n, key_dir), 0.0);
    let key_half = normalize(key_dir + view);
    let key_spec = pow(max(dot(n, key_half), 0.0), 72.0) * 0.55;

    let fill_dir = normalize(vec3<f32>(-0.55, -0.25, -0.5));
    let fill = max(dot(n, fill_dir), 0.0) * 0.35;

    let hemi = mix(vec3<f32>(0.035, 0.045, 0.075), vec3<f32>(0.14, 0.18, 0.27), n.y * 0.5 + 0.5);
    let rim = pow(1.0 - max(dot(n, view), 0.0), 3.0) * 0.45;

    var color = base * (hemi + vec3<f32>(1.0, 0.97, 0.92) * key * 0.95 + vec3<f32>(0.35, 0.5, 0.8) * fill);
    color += vec3<f32>(1.0, 0.98, 0.95) * key_spec;
    color += g.accent.rgb * rim * (0.28 + in.line_factor * 0.8);

    let dist = distance(in.world, g.camera_pos.xyz);
    let haze = smoothstep(g.fog.x, g.fog.y, dist);
    color = mix(color, backdrop(in.clip), haze);

    return vec4<f32>(color, 1.0);
}
