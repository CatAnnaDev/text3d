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

@vertex
fn vs_shadow(
    @location(0) position: vec3<f32>,
    @location(2) offset: vec3<f32>,
    @location(3) scale: vec2<f32>,
) -> @builtin(position) vec4<f32> {
    let placed = place_glyph(position, scale, offset);
    return g.light_view_proj * vec4<f32>(placed.world, 1.0);
}
