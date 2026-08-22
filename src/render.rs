use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use winit::window::Window;

use crate::atlas::{BLANK, GlyphAtlas, LOD_FAR, LOD_MID, LOD_NEAR, Slot};
use crate::camera::Camera;
use crate::complete::{Completion, VISIBLE_ROWS};
use crate::extrude::Vertex;
use crate::font::Font;
use crate::hud::{Hud, HudViewport, Label, Quad};
use crate::layout::LineLayout;
use crate::lsp::protocol::Severity;
use crate::syntax::{Highlighter, STYLE_COLORS, STYLE_TEXT};
use crate::text::{Match, TAB_WIDTH, TextBuffer};

pub const VISIBLE_RADIUS: usize = 160;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const BG_TOP: [f32; 4] = [0.055, 0.062, 0.098, 1.0];
const BG_BOTTOM: [f32; 4] = [0.012, 0.014, 0.028, 1.0];
const INK: [f32; 4] = [0.86, 0.89, 0.95, 1.0];
const ACCENT: [f32; 4] = [0.40, 0.78, 0.96, 1.0];
const HIGHLIGHT: [f32; 4] = [1.0, 0.76, 0.40, 1.0];

const POPUP_Z: f32 = 1.1;
const PANEL_FILL: [u8; 4] = [16, 19, 33, 242];
const PANEL_EDGE: [u8; 4] = [70, 108, 150, 235];
const PANEL_SELECTED: [u8; 4] = [58, 96, 140, 235];
const TAG_COLOR: [u8; 4] = [110, 122, 150, 255];
const DETAIL_COLOR: [u8; 4] = [122, 134, 158, 255];
const DETAIL_MAX_CHARS: usize = 44;
const TAG_COLUMNS: f32 = 4.4;

const SHADOW_SIZE: u32 = 2048;
const SHADOW_MARGIN: f32 = 0.5;
const SHADOW_QUANTUM: f32 = 4.0;
const SHADOW_CONTACT: f32 = 0.55;

const GLYPH_HALF_DEPTH: f32 = 0.13;
const CURRENT_LINE_LIFT: f32 = 0.34;
const WAVE_AMPLITUDE: f32 = 0.014;
const INDENT_DEPTH: f32 = 0.09;
const MAX_INDENT_LEVEL: usize = 8;

const LOD_UNSET: u8 = u8::MAX;
const LOD_NEAR_LIMIT: f32 = 26.0;
const LOD_FAR_LIMIT: f32 = 70.0;
const LOD_HYSTERESIS: f32 = 0.15;

const HIGHLIGHT_Z: f32 = -0.02;
const SELECTION_COLOR: [u8; 4] = [62, 92, 148, 190];
const MATCH_COLOR: [u8; 4] = [132, 108, 40, 190];
const CURRENT_MATCH_COLOR: [u8; 4] = [196, 150, 52, 220];
const CURSOR_COLOR: [u8; 4] = [255, 194, 102, 255];

const FIND_PLANE: f32 = 0.82;
const FIND_TEXT_FRACTION: f32 = 0.042;
const FIND_PAD: f32 = 0.42;
const FIND_GAP: f32 = 1.4;
const FIND_LABEL_COLOR: [u8; 4] = [138, 156, 190, 255];
const FIND_VALUE_COLOR: [u8; 4] = [226, 234, 248, 255];
const FIND_COUNT_COLOR: [u8; 4] = [196, 150, 52, 255];
const FIND_CARET_WIDTH: f32 = 0.07;

const HUD_EM_HEIGHT: f32 = 44.0;
const HUD_LAYERS: usize = 8;
const HUD_LAYER_STEP: f32 = 0.4;
const HUD_TEXT_LIFT: f32 = 0.16;

const GUTTER_GAP: f32 = 0.9;
const GUTTER_MARK_GAP: f32 = 0.34;
const GUTTER_MARK_WIDTH: f32 = 0.26;
const GUTTER_MARK_HEIGHT: f32 = 0.62;
const NUMBER_COLOR: [u8; 4] = [96, 108, 134, 255];
const NUMBER_CURRENT_COLOR: [u8; 4] = [198, 212, 240, 255];

const DIAGNOSTIC_TINT: u16 = 55;
const MARK_LIFT: f32 = GLYPH_HALF_DEPTH + 0.015;
const UNDERLINE_HEIGHT: f32 = 0.06;
const UNDERLINE_DROP: f32 = 0.11;
const UNDERLINE_MIN_WIDTH: f32 = 0.4;
const UNDERLINE_ALPHA: u8 = 226;
const SEVERITY_COLORS: [[u8; 4]; 4] = [
    [236, 98, 92, 255],
    [236, 176, 74, 255],
    [96, 172, 236, 255],
    [130, 150, 182, 255],
];

const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const INSTANCE_SIZE: u64 = std::mem::size_of::<Instance>() as u64;
const PANEL_VERTEX_SIZE: u64 = std::mem::size_of::<PanelVertex>() as u64;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    light_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    light_dir: [f32; 4],
    fog: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
    ink: [f32; 4],
    accent: [f32; 4],
    highlight: [f32; 4],
    ground: [f32; 4],
    params: [f32; 4],
    shadow: [f32; 4],
    find_anchor: [f32; 4],
    screen_right: [f32; 4],
    screen_up: [f32; 4],
    hud: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    offset: [f32; 3],
    scale: [f32; 2],
    color: [u8; 4],
}

impl Instance {
    const ZERO: Instance = Instance {
        offset: [0.0; 3],
        scale: [0.0; 2],
        color: [0; 4],
    };
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PanelVertex {
    position: [f32; 3],
    color: [u8; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DiagnosticSpan {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GutterMark {
    pub line: usize,
    pub severity: Severity,
}

pub struct FindBarView<'a> {
    pub query: &'a str,
    pub replacement: &'a str,
    pub replacing: bool,
    pub current: Option<usize>,
    pub total: usize,
}

pub struct RenderStats {
    pub instances: u32,
    pub triangles: u32,
    pub atlas_bytes: usize,
    pub draws: u32,
}

pub struct MeshGroup {
    pub color: [u8; 4],
    pub start: u32,
    pub count: u32,
}

#[derive(Default)]
pub struct SceneMesh {
    pub vertices: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub colors: Vec<[u8; 4]>,
    pub groups: Vec<MeshGroup>,
}

struct InstanceSet {
    values: Vec<Instance>,
    offsets: Vec<u32>,
    scratch: Vec<(u32, Instance)>,
    cursors: Vec<u32>,
    buffer: wgpu::Buffer,
    capacity: u32,
    draws: u32,
    triangles: u32,
    label: &'static str,
}

impl InstanceSet {
    fn new(device: &wgpu::Device, label: &'static str) -> InstanceSet {
        InstanceSet {
            values: Vec::new(),
            offsets: Vec::new(),
            scratch: Vec::new(),
            cursors: Vec::new(),
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: INSTANCE_SIZE * 64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: 64,
            draws: 0,
            triangles: 0,
            label,
        }
    }

    fn discard(&mut self) {
        self.scratch.clear();
        self.values.clear();
        self.offsets.clear();
        self.draws = 0;
        self.triangles = 0;
    }

    fn group(&mut self, slots: &[Slot]) {
        let slot_count = slots.len();
        self.offsets.clear();
        self.offsets.resize(slot_count + 1, 0);
        for (slot, _) in &self.scratch {
            self.offsets[*slot as usize + 1] += 1;
        }
        for index in 1..=slot_count {
            self.offsets[index] += self.offsets[index - 1];
        }

        self.values.clear();
        self.values.resize(self.scratch.len(), Instance::ZERO);
        self.cursors.clear();
        self.cursors.extend_from_slice(&self.offsets);
        for (slot, instance) in &self.scratch {
            let at = &mut self.cursors[*slot as usize];
            self.values[*at as usize] = *instance;
            *at += 1;
        }

        self.draws = 0;
        self.triangles = 0;
        for (index, slot) in slots.iter().enumerate() {
            let count = self.offsets[index + 1] - self.offsets[index];
            if count == 0 {
                continue;
            }
            self.draws += 1;
            self.triangles += count * (slot.index_count / 3);
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.values.is_empty() {
            return;
        }
        let needed = self.values.len() as u32;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: u64::from(self.capacity) * INSTANCE_SIZE,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.values));
    }
}

struct FlatInstances {
    values: Vec<Instance>,
    buffer: wgpu::Buffer,
    capacity: u32,
    label: &'static str,
}

impl FlatInstances {
    fn new(device: &wgpu::Device, label: &'static str) -> FlatInstances {
        FlatInstances {
            values: Vec::new(),
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: INSTANCE_SIZE * 64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: 64,
            label,
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.values.is_empty() {
            return;
        }
        let needed = self.values.len() as u32;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: u64::from(self.capacity) * INSTANCE_SIZE,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.values));
    }
}

struct HudRange {
    slot: u32,
    layer: u8,
    start: u32,
    end: u32,
}

struct LayeredText {
    scratch: Vec<(u64, Instance)>,
    values: Vec<Instance>,
    ranges: Vec<HudRange>,
    buffer: wgpu::Buffer,
    capacity: u32,
    triangles: u32,
    label: &'static str,
}

impl LayeredText {
    fn new(device: &wgpu::Device, label: &'static str) -> LayeredText {
        LayeredText {
            scratch: Vec::new(),
            values: Vec::new(),
            ranges: Vec::new(),
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: INSTANCE_SIZE * 64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: 64,
            triangles: 0,
            label,
        }
    }

    fn discard(&mut self) {
        self.scratch.clear();
        self.values.clear();
        self.ranges.clear();
        self.triangles = 0;
    }

    fn group(&mut self, slots: &[Slot]) {
        self.triangles = group_hud_text(
            &mut self.scratch,
            slots,
            &mut self.values,
            &mut self.ranges,
        );
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.values.is_empty() {
            return;
        }
        let needed = self.values.len() as u32;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: u64::from(self.capacity) * INSTANCE_SIZE,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.values));
    }
}

struct PanelMesh {
    vertices: Vec<PanelVertex>,
    buffer: wgpu::Buffer,
    capacity: u32,
    label: &'static str,
}

impl PanelMesh {
    fn new(device: &wgpu::Device, label: &'static str) -> PanelMesh {
        PanelMesh {
            vertices: Vec::new(),
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: PANEL_VERTEX_SIZE * 64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: 64,
            label,
        }
    }

    fn quad(&mut self, min: [f32; 2], max: [f32; 2], z: f32, color: [u8; 4]) {
        let corners = [
            [min[0], min[1], z],
            [max[0], min[1], z],
            [max[0], max[1], z],
            [min[0], max[1], z],
        ];
        for index in [0, 1, 2, 0, 2, 3] {
            self.vertices.push(PanelVertex { position: corners[index], color });
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.vertices.is_empty() {
            return;
        }
        let needed = self.vertices.len() as u32;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: u64::from(self.capacity) * PANEL_VERTEX_SIZE,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.vertices));
    }
}

struct Metrics {
    line_height: f32,
    ascender: f32,
    descender: f32,
    advance: f32,
}

impl Metrics {
    fn of(font: &Font) -> Metrics {
        Metrics {
            line_height: font.line_height(),
            ascender: font.ascender(),
            descender: font.descender(),
            advance: font.advance(),
        }
    }

    fn band(&self) -> f32 {
        (self.ascender - self.descender).max(1e-3)
    }
}

struct GlyphPass<'a> {
    label: &'a str,
    vertex_entry: &'a str,
    fragment_entry: &'a str,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    sample_count: u32,

    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    shadow_bind_group: wgpu::BindGroup,
    background_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    highlight_pipeline: wgpu::RenderPipeline,
    popup_pipeline: wgpu::RenderPipeline,
    find_pipeline: wgpu::RenderPipeline,
    hud_quad_pipeline: wgpu::RenderPipeline,
    hud_text_pipeline: wgpu::RenderPipeline,
    panel_pipeline: wgpu::RenderPipeline,
    find_panel_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,

    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    index_capacity: u64,
    cursor_buffer: wgpu::Buffer,

    popup_panel: PanelMesh,
    find_panel: PanelMesh,

    text_instances: InstanceSet,
    popup_instances: InstanceSet,
    find_instances: InstanceSet,
    highlights: FlatInstances,
    marks: FlatInstances,
    hud_quads: FlatInstances,
    hud_text: LayeredText,

    depth_view: wgpu::TextureView,
    msaa_view: Option<wgpu::TextureView>,
    shadow_view: wgpu::TextureView,

    layout: LineLayout,
    number_layout: LineLayout,
    number_text: String,
    lod_state: Vec<u8>,
    scratch_text: String,

    diagnostics: Vec<DiagnosticSpan>,
    gutter: Vec<GutterMark>,
    hud_quad_layers: [u32; HUD_LAYERS + 1],
    hud_key: u64,
    diagnostics_key: u64,
    gutter_key: u64,
    line_numbers: bool,
    wants_rebuild: bool,

    metrics: Metrics,
    cursor_y: f32,
    text_min: Vec3,
    text_max: Vec3,

    shadows: bool,
    depth_by_indent: bool,
    bevel: bool,
    grid_drawn: bool,

    atlas: GlyphAtlas,
}

pub fn visible_lines(text: &TextBuffer) -> Range<usize> {
    let first = text.cursor_line.saturating_sub(VISIBLE_RADIUS);
    let last = (text.cursor_line + VISIBLE_RADIUS + 1).min(text.line_count());
    first..last
}

impl Renderer {
    pub fn new(window: Arc<Window>, font: &Font) -> Result<Renderer, String> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .map_err(|e| format!("surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .map_err(|e| format!("adapter: {e}"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("text3d"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| format!("device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let mut config = surface
            .get_default_config(&adapter, width, height)
            .ok_or_else(|| String::from("surface unsupported"))?;
        config.format = format;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let sample_count = if adapter
            .get_texture_format_features(format)
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4)
        {
            4
        } else {
            1
        };

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals-bind"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let shadow_view = create_shadow(&device);
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow-bind"),
            layout: &shadow_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
            ],
        });

        let lit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lit-layout"),
            bind_group_layouts: &[Some(&bind_layout), Some(&shadow_layout)],
            immediate_size: 0,
        });
        let depth_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth-layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let glyph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph"),
            source: wgpu::ShaderSource::Wgsl(include_str!("glyph.wgsl").into()),
        });
        let stage_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stage"),
            source: wgpu::ShaderSource::Wgsl(include_str!("stage.wgsl").into()),
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()),
        });

        let multisample = wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        };
        let single_sample = wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        };

        let mesh_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let instance_attributes =
            wgpu::vertex_attr_array![2 => Float32x3, 3 => Float32x2, 4 => Unorm8x4];
        let glyph_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &mesh_attributes,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_SIZE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attributes,
            }),
        ];

        let glyph_pipeline = make_glyph_pipeline(
            &device,
            &lit_layout,
            &glyph_shader,
            &glyph_buffers,
            format,
            multisample,
            GlyphPass {
                label: "glyph-pipeline",
                vertex_entry: "vs_main",
                fragment_entry: "fs_main",
                blend: None,
                depth_write: true,
            },
        );
        let highlight_pipeline = make_glyph_pipeline(
            &device,
            &lit_layout,
            &glyph_shader,
            &glyph_buffers,
            format,
            multisample,
            GlyphPass {
                label: "highlight-pipeline",
                vertex_entry: "vs_main",
                fragment_entry: "fs_flat",
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                depth_write: false,
            },
        );
        let popup_pipeline = make_glyph_pipeline(
            &device,
            &lit_layout,
            &glyph_shader,
            &glyph_buffers,
            format,
            multisample,
            GlyphPass {
                label: "popup-pipeline",
                vertex_entry: "vs_popup",
                fragment_entry: "fs_main",
                blend: None,
                depth_write: true,
            },
        );
        let find_pipeline = make_glyph_pipeline(
            &device,
            &lit_layout,
            &glyph_shader,
            &glyph_buffers,
            format,
            multisample,
            GlyphPass {
                label: "find-pipeline",
                vertex_entry: "vs_find",
                fragment_entry: "fs_main",
                blend: None,
                depth_write: true,
            },
        );

        let hud_quad_pipeline = make_glyph_pipeline(
            &device,
            &lit_layout,
            &glyph_shader,
            &glyph_buffers,
            format,
            multisample,
            GlyphPass {
                label: "hud-quad-pipeline",
                vertex_entry: "vs_hud",
                fragment_entry: "fs_hud_flat",
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                depth_write: false,
            },
        );
        let hud_text_pipeline = make_glyph_pipeline(
            &device,
            &lit_layout,
            &glyph_shader,
            &glyph_buffers,
            format,
            multisample,
            GlyphPass {
                label: "hud-text-pipeline",
                vertex_entry: "vs_hud",
                fragment_entry: "fs_hud",
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                depth_write: true,
            },
        );

        let background_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("background-pipeline"),
            layout: Some(&lit_layout),
            vertex: wgpu::VertexState {
                module: &stage_shader,
                entry_point: Some("bg_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_state(false, wgpu::CompareFunction::Always)),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &stage_shader,
                entry_point: Some("bg_fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let panel_pipeline = make_panel_pipeline(
            &device, &lit_layout, &stage_shader, "panel-pipeline", "panel_vs", format, multisample,
        );
        let find_panel_pipeline = make_panel_pipeline(
            &device,
            &lit_layout,
            &stage_shader,
            "find-panel-pipeline",
            "find_panel_vs",
            format,
            multisample,
        );

        let grid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grid-pipeline"),
            layout: Some(&lit_layout),
            vertex: wgpu::VertexState {
                module: &stage_shader,
                entry_point: Some("grid_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth_state(false, wgpu::CompareFunction::Less)),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &stage_shader,
                entry_point: Some("grid_fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow-pipeline"),
            layout: Some(&depth_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &glyph_buffers,
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Less)),
            multisample: single_sample,
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        let depth_view = create_depth(&device, &config, sample_count);
        let msaa_view = create_msaa(&device, &config, sample_count);
        let atlas = GlyphAtlas::new(font);

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: 4096,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("indices"),
            size: 4096,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cursor_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cursor"),
            size: INSTANCE_SIZE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Renderer {
            text_instances: InstanceSet::new(&device, "text-instances"),
            popup_instances: InstanceSet::new(&device, "popup-instances"),
            find_instances: InstanceSet::new(&device, "find-instances"),
            highlights: FlatInstances::new(&device, "highlight-instances"),
            marks: FlatInstances::new(&device, "mark-instances"),
            hud_quads: FlatInstances::new(&device, "hud-quad-instances"),
            hud_text: LayeredText::new(&device, "hud-text-instances"),
            popup_panel: PanelMesh::new(&device, "popup-panel"),
            find_panel: PanelMesh::new(&device, "find-panel"),
            surface,
            device,
            queue,
            config,
            sample_count,
            globals,
            bind_group,
            shadow_bind_group,
            background_pipeline,
            glyph_pipeline,
            highlight_pipeline,
            popup_pipeline,
            find_pipeline,
            hud_quad_pipeline,
            hud_text_pipeline,
            panel_pipeline,
            find_panel_pipeline,
            grid_pipeline,
            shadow_pipeline,
            vertex_buffer,
            index_buffer,
            vertex_capacity: 4096,
            index_capacity: 4096,
            cursor_buffer,
            depth_view,
            msaa_view,
            shadow_view,
            layout: LineLayout::default(),
            number_layout: LineLayout::default(),
            number_text: String::new(),
            lod_state: Vec::new(),
            scratch_text: String::new(),
            diagnostics: Vec::new(),
            gutter: Vec::new(),
            hud_quad_layers: [0; HUD_LAYERS + 1],
            hud_key: 0,
            diagnostics_key: 0,
            gutter_key: 0,
            line_numbers: true,
            wants_rebuild: true,
            metrics: Metrics::of(font),
            cursor_y: 0.0,
            text_min: Vec3::splat(f32::INFINITY),
            text_max: Vec3::splat(f32::NEG_INFINITY),
            shadows: true,
            depth_by_indent: true,
            bevel: true,
            grid_drawn: true,
            atlas,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth(&self.device, &self.config, self.sample_count);
        self.msaa_view = create_msaa(&self.device, &self.config, self.sample_count);
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    pub fn viewport_height(&self) -> f32 {
        self.config.height as f32
    }

    pub fn shadows(&self) -> bool {
        self.shadows
    }

    pub fn set_shadows(&mut self, enabled: bool) {
        self.shadows = enabled;
    }

    pub fn depth_by_indent(&self) -> bool {
        self.depth_by_indent
    }

    pub fn set_depth_by_indent(&mut self, enabled: bool) {
        self.depth_by_indent = enabled;
    }

    pub fn bevel(&self) -> bool {
        self.bevel
    }

    pub fn set_bevel(&mut self, enabled: bool, font: &Font) {
        if self.bevel == enabled {
            return;
        }
        self.bevel = enabled;
        self.atlas.set_bevel(enabled, font);
        self.invalidate_slots(font);
    }

    pub fn reset_font(&mut self, font: &Font) {
        self.atlas.reset(font);
        self.invalidate_slots(font);
    }

    fn invalidate_slots(&mut self, font: &Font) {
        self.metrics = Metrics::of(font);
        self.text_instances.discard();
        self.popup_instances.discard();
        self.find_instances.discard();
        self.highlights.values.clear();
        self.marks.values.clear();
        self.hud_quads.values.clear();
        self.hud_text.discard();
        self.hud_quad_layers = [0; HUD_LAYERS + 1];
        self.hud_key = 0;
        self.wants_rebuild = true;
        self.popup_panel.vertices.clear();
        self.find_panel.vertices.clear();
        self.lod_state.clear();
        self.text_min = Vec3::splat(f32::INFINITY);
        self.text_max = Vec3::splat(f32::NEG_INFINITY);
        self.upload_atlas();
    }

    pub fn stats(&self) -> RenderStats {
        let quad = &self.atlas.slots[self.atlas.quad_slot() as usize];
        let cursor = &self.atlas.slots[self.atlas.cursor_slot as usize];
        let highlight_count = self.highlights.values.len() as u32;
        let mark_count = self.marks.values.len() as u32;
        let hud_quad_count = self.hud_quads.values.len() as u32;
        let flat_count = highlight_count + mark_count + hud_quad_count;
        let instances = self.text_instances.values.len() as u32
            + self.popup_instances.values.len() as u32
            + self.find_instances.values.len() as u32
            + self.hud_text.values.len() as u32
            + flat_count
            + 1;
        let triangles = self.text_instances.triangles
            + self.popup_instances.triangles
            + self.find_instances.triangles
            + self.hud_text.triangles
            + flat_count * (quad.index_count / 3)
            + cursor.index_count / 3;
        let mut draws = 2 + self.text_instances.draws;
        draws += self.popup_instances.draws + self.find_instances.draws;
        draws += self.hud_text.ranges.len() as u32;
        for layer in 0..HUD_LAYERS {
            if self.hud_quad_layers[layer] < self.hud_quad_layers[layer + 1] {
                draws += 1;
            }
        }
        if highlight_count > 0 {
            draws += 1;
        }
        if mark_count > 0 {
            draws += 1;
        }
        if self.grid_drawn {
            draws += 1;
        }
        if !self.popup_panel.vertices.is_empty() {
            draws += 1;
        }
        if !self.find_panel.vertices.is_empty() {
            draws += 1;
        }
        RenderStats {
            instances,
            triangles,
            atlas_bytes: self.atlas.memory_bytes(),
            draws,
        }
    }

    pub fn rebuild(
        &mut self,
        text: &TextBuffer,
        font: &Font,
        highlighter: Option<&Highlighter>,
        camera: &Camera,
    ) {
        self.metrics = Metrics::of(font);
        self.wants_rebuild = false;
        let line_height = self.metrics.line_height;
        let descender = self.metrics.descender;
        let ascender = self.metrics.ascender;
        let band = self.metrics.band();
        let band_scale = band / line_height;
        let advance = self.metrics.advance;
        let eye = camera.eye();
        let depth_by_indent = self.depth_by_indent;
        let numbered = self.line_numbers;
        let window = visible_lines(text);

        if self.lod_state.len() != text.line_count() {
            self.lod_state.resize(text.line_count(), LOD_UNSET);
        }

        self.text_instances.scratch.clear();
        self.highlights.values.clear();
        self.marks.values.clear();

        let number_right = -GUTTER_GAP;
        let number_left = if numbered {
            self.number_text.clear();
            for _ in 0..digit_count(text.line_count()) {
                self.number_text.push('0');
            }
            self.number_layout.build(font, &self.number_text);
            number_right - self.number_layout.width
        } else {
            number_right
        };
        let bounds_left = if numbered { number_left } else { 0.0 };
        let mark_x = number_left - GUTTER_MARK_GAP - GUTTER_MARK_WIDTH;
        let mark_y = descender + (band - GUTTER_MARK_HEIGHT) * 0.5;
        let mark_scale = GUTTER_MARK_HEIGHT / line_height;
        let underline_scale = UNDERLINE_HEIGHT / line_height;

        let mut gutter_cursor = self.gutter.partition_point(|mark| mark.line < window.start);
        let mut span_cursor = self
            .diagnostics
            .partition_point(|span| span.line < window.start);

        let current: Option<Match> = text
            .current_match()
            .and_then(|index| text.matches().get(index).copied());

        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);

        for line_index in window {
            let line = text.lines[line_index].as_str();
            self.layout.build(font, line);
            let y = -(line_index as f32) * line_height;
            let indent = if depth_by_indent {
                -(indent_level(line) as f32) * INDENT_DEPTH
            } else {
                0.0
            };

            let center = Vec3::new(self.layout.width * 0.5, y, indent);
            let previous = self.lod_state[line_index];
            let lod = pick_lod(eye.distance(center), previous);
            self.lod_state[line_index] = lod;

            while span_cursor < self.diagnostics.len()
                && self.diagnostics[span_cursor].line < line_index
            {
                span_cursor += 1;
            }
            let span_start = span_cursor;
            let mut span_end = span_cursor;
            while span_end < self.diagnostics.len()
                && self.diagnostics[span_end].line == line_index
            {
                span_end += 1;
            }

            let line_start = text.line_start(line_index);
            for placement in &self.layout.placements {
                let slot = self.atlas.slot_for(font, placement.key, lod);
                if slot == BLANK {
                    continue;
                }
                let style = match highlighter {
                    Some(highlighter) => highlighter.style_at(line_start + placement.byte),
                    None => plain_style(line[placement.byte..].chars().next().unwrap_or(' ')),
                };
                let mut color = STYLE_COLORS[style as usize];
                if let Some(severity) =
                    severity_at(&self.diagnostics[span_start..span_end], placement.column)
                {
                    color = tint_color(color, SEVERITY_COLORS[severity_slot(severity)]);
                }
                self.text_instances.scratch.push((
                    slot,
                    Instance {
                        offset: [placement.x, y, indent],
                        scale: [1.0, 1.0],
                        color,
                    },
                ));
            }

            if numbered {
                self.number_text.clear();
                let _ = write!(self.number_text, "{}", line_index + 1);
                self.number_layout.build(font, &self.number_text);
                let number_x = number_right - self.number_layout.width;
                let color = if line_index == text.cursor_line {
                    NUMBER_CURRENT_COLOR
                } else {
                    NUMBER_COLOR
                };
                for placement in &self.number_layout.placements {
                    let slot = self.atlas.slot_for(font, placement.key, lod);
                    if slot == BLANK {
                        continue;
                    }
                    self.text_instances.scratch.push((
                        slot,
                        Instance {
                            offset: [number_x + placement.x, y, indent],
                            scale: [1.0, 1.0],
                            color,
                        },
                    ));
                }
            }

            if numbered || !self.layout.placements.is_empty() {
                min.x = min.x.min(bounds_left);
                max.x = max.x.max(self.layout.width);
                min.y = min.y.min(y + descender);
                max.y = max.y.max(y + ascender);
                min.z = min.z.min(indent - GLYPH_HALF_DEPTH);
                max.z = max.z.max(indent + GLYPH_HALF_DEPTH + CURRENT_LINE_LIFT);
            }

            while gutter_cursor < self.gutter.len()
                && self.gutter[gutter_cursor].line < line_index
            {
                gutter_cursor += 1;
            }
            if let Some(mark) = self.gutter.get(gutter_cursor)
                && mark.line == line_index
            {
                self.marks.values.push(Instance {
                    offset: [mark_x, y + mark_y, indent + MARK_LIFT],
                    scale: [GUTTER_MARK_WIDTH, mark_scale],
                    color: SEVERITY_COLORS[severity_slot(mark.severity)],
                });
            }

            for span in &self.diagnostics[span_start..span_end] {
                let (left, right) = span_bounds(&self.layout, span.start, span.end, advance);
                let mut color = SEVERITY_COLORS[severity_slot(span.severity)];
                color[3] = UNDERLINE_ALPHA;
                push_span(
                    &mut self.marks.values,
                    left,
                    right.max(left + UNDERLINE_MIN_WIDTH),
                    y - UNDERLINE_DROP,
                    indent + MARK_LIFT,
                    underline_scale,
                    color,
                );
            }

            let quad_y = y + descender;
            let quad_z = indent + HIGHLIGHT_Z;
            for found in text.match_on_line(line_index) {
                let color = match current {
                    Some(active) if active.line == found.line && active.start == found.start => {
                        CURRENT_MATCH_COLOR
                    }
                    _ => MATCH_COLOR,
                };
                let (left, right) = span_bounds(&self.layout, found.start, found.end, advance);
                push_span(
                    &mut self.highlights.values,
                    left,
                    right,
                    quad_y,
                    quad_z,
                    band_scale,
                    color,
                );
            }
            if let Some((start, end)) = text.selection_on_line(line_index) {
                let (left, right) = span_bounds(&self.layout, start, end, advance);
                push_span(
                    &mut self.highlights.values,
                    left,
                    right,
                    quad_y,
                    quad_z,
                    band_scale,
                    SELECTION_COLOR,
                );
            }
        }

        self.text_min = min;
        self.text_max = max;

        self.text_instances.group(&self.atlas.slots);
        self.upload_atlas();
        self.text_instances.upload(&self.device, &self.queue);
        self.highlights.upload(&self.device, &self.queue);
        self.marks.upload(&self.device, &self.queue);

        let cursor_line = match text.lines.get(text.cursor_line) {
            Some(line) => line.as_str(),
            None => "",
        };
        self.layout.build(font, cursor_line);
        let cursor_indent = if depth_by_indent {
            -(indent_level(cursor_line) as f32) * INDENT_DEPTH
        } else {
            0.0
        };
        self.cursor_y = -(text.cursor_line as f32) * line_height;
        let cursor = Instance {
            offset: [
                self.layout.x_of_column(text.cursor_col),
                self.cursor_y,
                cursor_indent,
            ],
            scale: [1.0, 1.0],
            color: CURSOR_COLOR,
        };
        self.queue
            .write_buffer(&self.cursor_buffer, 0, bytemuck::bytes_of(&cursor));
    }

    pub fn set_popup(&mut self, completion: &Completion, text: &TextBuffer, font: &Font) {
        self.popup_instances.scratch.clear();
        self.popup_panel.vertices.clear();
        if !completion.active {
            self.popup_instances.discard();
            return;
        }

        self.metrics = Metrics::of(font);
        let advance = self.metrics.advance;
        let line_height = self.metrics.line_height;
        let cursor_line = match text.lines.get(text.cursor_line) {
            Some(line) => line.as_str(),
            None => "",
        };
        self.layout.build(font, cursor_line);
        let origin_x = self.layout.x_of_column(text.cursor_col);
        let origin_y = -(text.cursor_line as f32) * line_height - line_height;

        let mut widest = 0.0f32;
        let mut shown = 0usize;
        for (row, candidate) in completion
            .items
            .iter()
            .skip(completion.scroll)
            .take(VISIBLE_ROWS)
            .enumerate()
        {
            let y = origin_y - row as f32 * line_height;
            let selected = completion.scroll + row == completion.selected;
            let (name_color, tag_color) = if selected {
                ([255, 226, 176, 255], [198, 214, 240, 255])
            } else {
                (STYLE_COLORS[candidate.kind.style() as usize], TAG_COLOR)
            };
            self.push_popup_text(font, candidate.kind.tag(), origin_x + 0.4 * advance, y, tag_color);
            let width =
                self.push_popup_text(font, &candidate.text, origin_x + TAG_COLUMNS * advance, y, name_color);
            widest = widest.max(width);
            shown += 1;
        }

        let detail_x = origin_x + TAG_COLUMNS * advance + widest + advance;
        let mut detail_widest = 0.0f32;
        for (row, candidate) in completion
            .items
            .iter()
            .skip(completion.scroll)
            .take(VISIBLE_ROWS)
            .enumerate()
        {
            if candidate.detail.is_empty() {
                continue;
            }
            let y = origin_y - row as f32 * line_height;
            let detail = shorten(&candidate.detail, DETAIL_MAX_CHARS);
            let width = self.push_popup_text(font, detail, detail_x, y, DETAIL_COLOR);
            detail_widest = detail_widest.max(width);
        }

        let left = origin_x - 0.3;
        let mut right = origin_x + TAG_COLUMNS * advance + widest + 0.6 * advance;
        if detail_widest > 0.0 {
            right = detail_x + detail_widest + 0.6 * advance;
        }
        let top = origin_y + self.metrics.ascender * 0.85 + 0.14;
        let bottom = origin_y - (shown.saturating_sub(1)) as f32 * line_height
            + self.metrics.descender * 0.85
            - 0.14;

        self.popup_panel.quad(
            [left - 0.08, bottom - 0.08],
            [right + 0.08, top + 0.08],
            POPUP_Z - 0.16,
            PANEL_EDGE,
        );
        self.popup_panel
            .quad([left, bottom], [right, top], POPUP_Z - 0.12, PANEL_FILL);

        let row = completion.selected.saturating_sub(completion.scroll);
        let row_y = origin_y - row as f32 * line_height;
        self.popup_panel.quad(
            [left, row_y + self.metrics.descender * 0.85],
            [right, row_y + self.metrics.ascender * 0.85],
            POPUP_Z - 0.06,
            PANEL_SELECTED,
        );

        self.popup_instances.group(&self.atlas.slots);
        self.upload_atlas();
        self.popup_instances.upload(&self.device, &self.queue);
        self.popup_panel.upload(&self.device, &self.queue);
    }

    fn push_popup_text(&mut self, font: &Font, content: &str, x: f32, y: f32, color: [u8; 4]) -> f32 {
        push_line(
            &mut self.atlas,
            &mut self.layout,
            font,
            content,
            [x, y, POPUP_Z],
            color,
            &mut self.popup_instances.scratch,
        )
    }

    pub fn set_find_bar(&mut self, bar: Option<FindBarView<'_>>, text: &TextBuffer, font: &Font) {
        self.find_instances.scratch.clear();
        self.find_panel.vertices.clear();
        let Some(bar) = bar else {
            self.find_instances.discard();
            return;
        };

        self.metrics = Metrics::of(font);
        let line_height = self.metrics.line_height;
        let descender = self.metrics.descender;
        let band_scale = self.metrics.band() / line_height;
        let rows = if bar.replacing { 2usize } else { 1 };
        let base = FIND_PAD - descender;

        let query_y = base + (rows - 1) as f32 * line_height;
        self.scratch_text.clear();
        self.scratch_text.push_str("chercher: ");
        let prefix = self.scratch_text.chars().count();
        self.scratch_text.push_str(bar.query);
        let query_width = self.push_find_scratch(font, FIND_PAD, query_y, prefix);

        let mut caret_x = FIND_PAD + query_width;
        let mut caret_y = query_y;
        let mut widest = query_width;

        if bar.replacing {
            self.scratch_text.clear();
            self.scratch_text.push_str("remplacer: ");
            let prefix = self.scratch_text.chars().count();
            self.scratch_text.push_str(bar.replacement);
            let width = self.push_find_scratch(font, FIND_PAD, base, prefix);
            widest = widest.max(width);
            caret_x = FIND_PAD + width;
            caret_y = base;
        }

        self.scratch_text.clear();
        if bar.total == 0 {
            self.scratch_text.push_str("aucun");
        } else {
            let position = bar.current.map(|index| index + 1).unwrap_or(0);
            let _ = write!(self.scratch_text, "{}/{}", position, bar.total);
            if let Some(found) = bar.current.and_then(|index| text.matches().get(index)) {
                let _ = write!(self.scratch_text, "  ligne {}", found.line + 1);
            }
        }
        self.layout.build(font, &self.scratch_text);
        let counter = self.layout.width;
        let width = FIND_PAD * 2.0 + widest + FIND_GAP + counter;
        let height = (rows - 1) as f32 * line_height + self.metrics.band() + FIND_PAD * 2.0;
        let counter_x = width - FIND_PAD - counter;
        push_line(
            &mut self.atlas,
            &mut self.layout,
            font,
            &self.scratch_text,
            [counter_x, query_y, 0.0],
            FIND_COUNT_COLOR,
            &mut self.find_instances.scratch,
        );

        self.find_instances.scratch.push((
            self.atlas.quad_slot(),
            Instance {
                offset: [caret_x + 0.06, caret_y + descender, 0.0],
                scale: [FIND_CARET_WIDTH, band_scale],
                color: CURSOR_COLOR,
            },
        ));

        self.find_panel
            .quad([-0.09, -0.09], [width + 0.09, height + 0.09], -0.26, PANEL_EDGE);
        self.find_panel
            .quad([0.0, 0.0], [width, height], -0.22, PANEL_FILL);

        self.find_instances.group(&self.atlas.slots);
        self.upload_atlas();
        self.find_instances.upload(&self.device, &self.queue);
        self.find_panel.upload(&self.device, &self.queue);
    }

    fn push_find_scratch(&mut self, font: &Font, x: f32, y: f32, prefix: usize) -> f32 {
        self.layout.build(font, &self.scratch_text);
        for placement in &self.layout.placements {
            let slot = self.atlas.slot_for(font, placement.key, LOD_NEAR);
            if slot == BLANK {
                continue;
            }
            let color = if placement.column < prefix {
                FIND_LABEL_COLOR
            } else {
                FIND_VALUE_COLOR
            };
            self.find_instances.scratch.push((
                slot,
                Instance {
                    offset: [x + placement.x, y, 0.0],
                    scale: [1.0, 1.0],
                    color,
                },
            ));
        }
        self.layout.width
    }

    pub fn hud_viewport(&self, camera: &Camera) -> HudViewport {
        let aspect = self.aspect();
        let (half_width, half_height) = camera.half_extent(aspect);
        let ratio = if half_height > 1e-4 {
            half_width / half_height
        } else {
            aspect
        };
        HudViewport {
            width: HUD_EM_HEIGHT * ratio.max(0.01),
            height: HUD_EM_HEIGHT,
        }
    }

    pub fn line_numbers(&self) -> bool {
        self.line_numbers
    }

    pub fn set_line_numbers(&mut self, enabled: bool) {
        if self.line_numbers == enabled {
            return;
        }
        self.line_numbers = enabled;
        self.wants_rebuild = true;
    }

    pub fn wants_rebuild(&self) -> bool {
        self.wants_rebuild
    }

    pub fn set_diagnostics(&mut self, spans: &[DiagnosticSpan]) {
        let key = diagnostics_fingerprint(spans);
        if key == self.diagnostics_key {
            return;
        }
        self.diagnostics_key = key;
        self.diagnostics.clear();
        self.diagnostics.extend_from_slice(spans);
        normalize_spans(&mut self.diagnostics);
        self.wants_rebuild = true;
    }

    pub fn set_gutter(&mut self, marks: &[GutterMark]) {
        let key = gutter_fingerprint(marks);
        if key == self.gutter_key {
            return;
        }
        self.gutter_key = key;
        self.gutter.clear();
        self.gutter.extend_from_slice(marks);
        normalize_marks(&mut self.gutter);
        self.wants_rebuild = true;
    }

    pub fn set_hud(&mut self, hud: &Hud, font: &Font) {
        let quads = hud.quads();
        let labels = hud.labels();
        let key = hud_fingerprint(quads, labels);
        if key == self.hud_key {
            return;
        }
        self.hud_key = key;

        build_hud_quads(
            quads,
            font.line_height(),
            &mut self.hud_quads.values,
            &mut self.hud_quad_layers,
        );

        self.hud_text.scratch.clear();
        for label in labels {
            push_hud_label(
                &mut self.atlas,
                &mut self.layout,
                font,
                label,
                &mut self.hud_text.scratch,
            );
        }
        self.hud_text.group(&self.atlas.slots);

        self.upload_atlas();
        self.hud_quads.upload(&self.device, &self.queue);
        self.hud_text.upload(&self.device, &self.queue);
    }

    fn upload_atlas(&mut self) {
        if !self.atlas.dirty {
            return;
        }
        self.atlas.dirty = false;
        let vertex_bytes = bytemuck::cast_slice(&self.atlas.vertices);
        let index_bytes = bytemuck::cast_slice(&self.atlas.indices);
        if vertex_bytes.is_empty() || index_bytes.is_empty() {
            return;
        }
        if vertex_bytes.len() as u64 > self.vertex_capacity {
            self.vertex_capacity = (vertex_bytes.len() as u64).next_power_of_two();
            self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vertices"),
                size: self.vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if index_bytes.len() as u64 > self.index_capacity {
            self.index_capacity = (index_bytes.len() as u64).next_power_of_two();
            self.index_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("indices"),
                size: self.index_capacity,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(&self.vertex_buffer, 0, vertex_bytes);
        self.queue.write_buffer(&self.index_buffer, 0, index_bytes);
    }

    pub fn render(
        &mut self,
        camera: &Camera,
        elapsed: f32,
        wave: bool,
        grid: bool,
    ) -> Result<(), String> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            _ => return Err(String::from("surface indisponible")),
        };

        self.grid_drawn = grid;
        self.write_globals(camera, elapsed, wave);

        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        if self.casts_shadows() {
            self.encode_shadow(&mut encoder);
        }
        let (target, resolve) = match &self.msaa_view {
            Some(msaa) => (msaa, Some(&view)),
            None => (&view, None),
        };
        self.encode_scene(&mut encoder, target, resolve, grid);
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        Ok(())
    }

    pub fn capture(
        &mut self,
        camera: &Camera,
        elapsed: f32,
        wave: bool,
        grid: bool,
    ) -> Result<(u32, u32, Vec<u8>), String> {
        let width = self.config.width.max(1);
        let height = self.config.height.max(1);
        let format = self.config.format;
        let pixel = format
            .block_copy_size(None)
            .ok_or_else(|| String::from("format de surface non copiable"))?;
        if pixel != 4 {
            return Err(String::from("format de surface non pris en charge"));
        }
        let row_bytes = width * pixel;
        let padded = row_bytes.div_ceil(256) * 256;

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture-readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        self.grid_drawn = grid;
        self.write_globals(camera, elapsed, wave);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("capture") });
        if self.casts_shadows() {
            self.encode_shadow(&mut encoder);
        }
        let (target, resolve) = match &self.msaa_view {
            Some(msaa) => (msaa, Some(&view)),
            None => (&view, None),
        };
        self.encode_scene(&mut encoder, target, resolve, grid);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.queue.submit(Some(encoder.finish()));

        let (sender, receiver) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|err| format!("attente du peripherique ({err})"))?;
        receiver
            .recv()
            .map_err(|_| String::from("capture interrompue"))?
            .map_err(|_| String::from("capture illisible"))?;

        let swap = matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut rgba = vec![0u8; (row_bytes * height) as usize];
        {
            let mapped = readback
                .slice(..)
                .get_mapped_range()
                .map_err(|err| format!("acces a la capture ({err})"))?;
            let stride = padded as usize;
            let line = row_bytes as usize;
            for row in 0..height as usize {
                let source = &mapped[row * stride..row * stride + line];
                let destination = &mut rgba[row * line..row * line + line];
                if swap {
                    let (input, _) = source.as_chunks::<4>();
                    let (output, _) = destination.as_chunks_mut::<4>();
                    for (pixel, out) in input.iter().zip(output) {
                        out[0] = pixel[2];
                        out[1] = pixel[1];
                        out[2] = pixel[0];
                        out[3] = pixel[3];
                    }
                } else {
                    destination.copy_from_slice(source);
                }
            }
        }
        readback.unmap();
        Ok((width, height, rgba))
    }

    pub fn scene_mesh(
        &self,
        text: &TextBuffer,
        font: &Font,
        highlighter: Option<&Highlighter>,
    ) -> SceneMesh {
        let mut atlas = GlyphAtlas::new(font);
        atlas.set_bevel(self.bevel, font);
        let mut layout = LineLayout::default();
        let window = visible_lines(text);
        let mut expected = 0usize;
        for line_index in window.clone() {
            expected += text.line_chars(line_index);
        }
        let mut items: Vec<(u32, u32, [f32; 3])> = Vec::with_capacity(expected);
        let line_height = font.line_height();

        for line_index in window {
            let line = text.lines[line_index].as_str();
            layout.build(font, line);
            let y = -(line_index as f32) * line_height;
            let indent = if self.depth_by_indent {
                -(indent_level(line) as f32) * INDENT_DEPTH
            } else {
                0.0
            };
            let line_start = text.line_start(line_index);
            for placement in &layout.placements {
                let slot = atlas.slot_for(font, placement.key, LOD_NEAR);
                if slot == BLANK {
                    continue;
                }
                let style = match highlighter {
                    Some(highlighter) => highlighter.style_at(line_start + placement.byte),
                    None => plain_style(line[placement.byte..].chars().next().unwrap_or(' ')),
                };
                let color = STYLE_COLORS[style as usize];
                items.push((u32::from_le_bytes(color), slot, [placement.x, y, indent]));
            }
        }
        items.sort_unstable_by_key(|item| item.0);

        let spans = slot_spans(&atlas);
        let mut vertex_total = 0usize;
        let mut index_total = 0usize;
        for (_, slot, _) in &items {
            vertex_total += spans[*slot as usize] as usize;
            index_total += atlas.slots[*slot as usize].index_count as usize;
        }

        let mut mesh = SceneMesh::default();
        mesh.vertices.reserve_exact(vertex_total);
        mesh.normals.reserve_exact(vertex_total);
        mesh.colors.reserve_exact(vertex_total);
        mesh.indices.reserve_exact(index_total);
        let mut index = 0usize;
        while index < items.len() {
            let key = items[index].0;
            let color = key.to_le_bytes();
            let start = mesh.indices.len() as u32;
            while index < items.len() && items[index].0 == key {
                let (_, slot, offset) = items[index];
                let entry = &atlas.slots[slot as usize];
                let base = mesh.vertices.len() as u32;
                let first = entry.base_vertex as usize;
                let span = spans[slot as usize] as usize;
                for vertex in &atlas.vertices[first..first + span] {
                    mesh.vertices.push([
                        vertex.position[0] + offset[0],
                        vertex.position[1] + offset[1],
                        vertex.position[2] + offset[2],
                    ]);
                    mesh.normals.push(vertex.normal);
                    mesh.colors.push(color);
                }
                let from = entry.index_start as usize;
                let to = from + entry.index_count as usize;
                for value in &atlas.indices[from..to] {
                    mesh.indices.push(base + value);
                }
                index += 1;
            }
            mesh.groups.push(MeshGroup {
                color,
                start,
                count: mesh.indices.len() as u32 - start,
            });
        }
        mesh
    }

    fn casts_shadows(&self) -> bool {
        self.shadows && !self.atlas.indices.is_empty() && self.text_min.x <= self.text_max.x
    }

    fn write_globals(&self, camera: &Camera, elapsed: f32, wave: bool) {
        let aspect = self.aspect();
        let eye = camera.eye();
        let line_height = self.metrics.line_height;
        let cull_reach = VISIBLE_RADIUS as f32 * line_height * 0.92;
        let fog_end = (camera.distance * 4.2).min(cull_reach);
        let fog_start = (camera.distance * 1.25).min(fog_end * 0.4);
        let light = Vec3::new(0.42, 0.76, 0.55).normalize();
        let (half_width, half_height) = camera.half_extent(aspect);
        let ground_y = camera.target.y - half_height * 1.25;

        let shadows_on = self.casts_shadows();
        let light_view_proj = if shadows_on {
            let reach = Vec3::splat((half_width.max(half_height) * 1.6).max(4.0));
            let low = self.text_min.max(camera.target - reach);
            let high = self.text_max.min(camera.target + reach);
            light_matrix(low, high, ground_y, light)
        } else {
            Mat4::IDENTITY
        };

        let plane = 1.0 - FIND_PLANE;
        let right = camera.right();
        let up = camera.up();
        let center = camera.target - camera.forward() * (camera.distance * FIND_PLANE);
        let anchor = center - right * (half_width * plane * 0.94) - up * (half_height * plane * 0.94);
        let find_scale = 2.0 * half_height * plane * FIND_TEXT_FRACTION / line_height;
        let ratio = if half_height > 1e-4 {
            half_width / half_height
        } else {
            aspect
        };
        let hud = hud_ndc(HUD_EM_HEIGHT * ratio.max(0.01), HUD_EM_HEIGHT);

        let globals = Globals {
            view_proj: camera.view_proj(aspect).to_cols_array_2d(),
            light_view_proj: light_view_proj.to_cols_array_2d(),
            camera_pos: [eye.x, eye.y, eye.z, 1.0],
            light_dir: [light.x, light.y, light.z, 0.0],
            fog: [fog_start, fog_end, self.cursor_y, line_height],
            bg_top: BG_TOP,
            bg_bottom: BG_BOTTOM,
            ink: INK,
            accent: ACCENT,
            highlight: HIGHLIGHT,
            ground: [
                ground_y,
                2.0,
                camera.distance * 0.8,
                camera.distance * 2.6,
            ],
            params: [
                elapsed,
                if wave { WAVE_AMPLITUDE } else { 0.0 },
                self.config.width as f32,
                self.config.height as f32,
            ],
            shadow: [
                1.0 / SHADOW_SIZE as f32,
                if shadows_on { 1.0 } else { 0.0 },
                SHADOW_CONTACT,
                0.0,
            ],
            find_anchor: [anchor.x, anchor.y, anchor.z, find_scale],
            screen_right: [right.x, right.y, right.z, 0.0],
            screen_up: [up.x, up.y, up.z, 0.0],
            hud,
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
    }

    fn encode_shadow(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        if !self.text_instances.values.is_empty() {
            pass.set_vertex_buffer(1, self.text_instances.buffer.slice(..));
            draw_groups(&mut pass, &self.atlas, &self.text_instances.offsets);
        }
        let cursor = &self.atlas.slots[self.atlas.cursor_slot as usize];
        pass.set_vertex_buffer(1, self.cursor_buffer.slice(..));
        pass.draw_indexed(
            cursor.index_start..cursor.index_start + cursor.index_count,
            cursor.base_vertex,
            0..1,
        );
    }

    fn encode_scene(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        grid: bool,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_bind_group(1, &self.shadow_bind_group, &[]);
        pass.set_pipeline(&self.background_pipeline);
        pass.draw(0..3, 0..1);

        if !self.atlas.indices.is_empty() {
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

            if !self.highlights.values.is_empty() {
                let quad = &self.atlas.slots[self.atlas.quad_slot() as usize];
                pass.set_pipeline(&self.highlight_pipeline);
                pass.set_vertex_buffer(1, self.highlights.buffer.slice(..));
                pass.draw_indexed(
                    quad.index_start..quad.index_start + quad.index_count,
                    quad.base_vertex,
                    0..self.highlights.values.len() as u32,
                );
            }

            pass.set_pipeline(&self.glyph_pipeline);
            if !self.text_instances.values.is_empty() {
                pass.set_vertex_buffer(1, self.text_instances.buffer.slice(..));
                draw_groups(&mut pass, &self.atlas, &self.text_instances.offsets);
            }

            let cursor = &self.atlas.slots[self.atlas.cursor_slot as usize];
            pass.set_vertex_buffer(1, self.cursor_buffer.slice(..));
            pass.draw_indexed(
                cursor.index_start..cursor.index_start + cursor.index_count,
                cursor.base_vertex,
                0..1,
            );

            if !self.marks.values.is_empty() {
                let quad = &self.atlas.slots[self.atlas.quad_slot() as usize];
                pass.set_pipeline(&self.highlight_pipeline);
                pass.set_vertex_buffer(1, self.marks.buffer.slice(..));
                pass.draw_indexed(
                    quad.index_start..quad.index_start + quad.index_count,
                    quad.base_vertex,
                    0..self.marks.values.len() as u32,
                );
            }

            if !self.popup_instances.values.is_empty() {
                pass.set_pipeline(&self.popup_pipeline);
                pass.set_vertex_buffer(1, self.popup_instances.buffer.slice(..));
                draw_groups(&mut pass, &self.atlas, &self.popup_instances.offsets);
            }
        }

        if grid {
            pass.set_pipeline(&self.grid_pipeline);
            pass.draw(0..4, 0..1);
        }

        if !self.popup_panel.vertices.is_empty() {
            pass.set_pipeline(&self.panel_pipeline);
            pass.set_vertex_buffer(0, self.popup_panel.buffer.slice(..));
            pass.draw(0..self.popup_panel.vertices.len() as u32, 0..1);
        }

        if !self.find_panel.vertices.is_empty() {
            pass.set_pipeline(&self.find_panel_pipeline);
            pass.set_vertex_buffer(0, self.find_panel.buffer.slice(..));
            pass.draw(0..self.find_panel.vertices.len() as u32, 0..1);
        }

        if !self.find_instances.values.is_empty() && !self.atlas.indices.is_empty() {
            pass.set_pipeline(&self.find_pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(1, self.find_instances.buffer.slice(..));
            draw_groups(&mut pass, &self.atlas, &self.find_instances.offsets);
        }

        drop(pass);

        let mut hud_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hud"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                resolve_target: resolve,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
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
        hud_pass.set_bind_group(0, &self.bind_group, &[]);
        hud_pass.set_bind_group(1, &self.shadow_bind_group, &[]);
        self.encode_hud(&mut hud_pass);
    }

    fn encode_hud(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.atlas.indices.is_empty() {
            return;
        }
        if self.hud_quads.values.is_empty() && self.hud_text.ranges.is_empty() {
            return;
        }

        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        let quad = &self.atlas.slots[self.atlas.quad_slot() as usize];
        let mut range = 0usize;

        for layer in 0..HUD_LAYERS {
            let start = self.hud_quad_layers[layer];
            let end = self.hud_quad_layers[layer + 1];
            if start < end {
                pass.set_pipeline(&self.hud_quad_pipeline);
                pass.set_vertex_buffer(1, self.hud_quads.buffer.slice(..));
                pass.draw_indexed(
                    quad.index_start..quad.index_start + quad.index_count,
                    quad.base_vertex,
                    start..end,
                );
            }

            let first = range;
            while range < self.hud_text.ranges.len()
                && self.hud_text.ranges[range].layer as usize == layer
            {
                if range == first {
                    pass.set_pipeline(&self.hud_text_pipeline);
                    pass.set_vertex_buffer(1, self.hud_text.buffer.slice(..));
                }
                let entry = &self.hud_text.ranges[range];
                let slot = &self.atlas.slots[entry.slot as usize];
                pass.draw_indexed(
                    slot.index_start..slot.index_start + slot.index_count,
                    slot.base_vertex,
                    entry.start..entry.end,
                );
                range += 1;
            }
        }
    }
}

fn push_line(
    atlas: &mut GlyphAtlas,
    layout: &mut LineLayout,
    font: &Font,
    content: &str,
    origin: [f32; 3],
    color: [u8; 4],
    out: &mut Vec<(u32, Instance)>,
) -> f32 {
    layout.build(font, content);
    for placement in &layout.placements {
        let slot = atlas.slot_for(font, placement.key, LOD_NEAR);
        if slot == BLANK {
            continue;
        }
        out.push((
            slot,
            Instance {
                offset: [origin[0] + placement.x, origin[1], origin[2]],
                scale: [1.0, 1.0],
                color,
            },
        ));
    }
    layout.width
}

fn span_bounds(layout: &LineLayout, start: usize, end: usize, advance: f32) -> (f32, f32) {
    let count = layout.placements.len();
    let left = layout.x_of_column(start);
    let mut right = layout.x_of_column(end);
    if end > count {
        right += (end - count) as f32 * advance * 0.5;
    }
    (left, right)
}

fn digit_count(value: usize) -> usize {
    let mut count = 1usize;
    let mut rest = value;
    while rest >= 10 {
        rest /= 10;
        count += 1;
    }
    count
}

fn severity_slot(severity: Severity) -> usize {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Information => 2,
        Severity::Hint => 3,
    }
}

fn severity_at(spans: &[DiagnosticSpan], column: usize) -> Option<Severity> {
    let mut best: Option<Severity> = None;
    for span in spans {
        if column < span.start || column >= span.end {
            continue;
        }
        let keep = match best {
            Some(current) => severity_slot(current) <= severity_slot(span.severity),
            None => false,
        };
        if !keep {
            best = Some(span.severity);
        }
    }
    best
}

fn tint_color(base: [u8; 4], accent: [u8; 4]) -> [u8; 4] {
    let blend = |left: u8, right: u8| -> u8 {
        let value = u16::from(left) * (100 - DIAGNOSTIC_TINT) + u16::from(right) * DIAGNOSTIC_TINT;
        ((value + 50) / 100) as u8
    };
    [
        blend(base[0], accent[0]),
        blend(base[1], accent[1]),
        blend(base[2], accent[2]),
        base[3],
    ]
}

fn normalize_spans(spans: &mut [DiagnosticSpan]) {
    for span in spans.iter_mut() {
        if span.end < span.start {
            span.end = span.start;
        }
    }
    spans.sort_unstable_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then(left.start.cmp(&right.start))
            .then(severity_slot(left.severity).cmp(&severity_slot(right.severity)))
    });
}

fn normalize_marks(marks: &mut Vec<GutterMark>) {
    marks.sort_unstable_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then(severity_slot(left.severity).cmp(&severity_slot(right.severity)))
    });
    marks.dedup_by_key(|mark| mark.line);
}

fn hud_ndc(width: f32, height: f32) -> [f32; 4] {
    [
        2.0 / width.max(1e-4),
        -1.0,
        2.0 / height.max(1e-4),
        1.0,
    ]
}

fn hud_layer(layer: u8) -> usize {
    (layer as usize).min(HUD_LAYERS - 1)
}

fn hud_text_key(layer: usize, slot: u32) -> u64 {
    ((layer as u64) << 32) | u64::from(slot)
}

fn hud_quad_instance(quad: &Quad, line_height: f32) -> Instance {
    let layer = hud_layer(quad.layer);
    let width = quad.rect.w.max(0.0);
    let height = quad.rect.h.max(0.0);
    Instance {
        offset: [
            quad.rect.x,
            -(quad.rect.y + height),
            layer as f32 * HUD_LAYER_STEP,
        ],
        scale: [width, height / line_height.max(1e-4)],
        color: quad.color,
    }
}

fn build_hud_quads(
    quads: &[Quad],
    line_height: f32,
    out: &mut Vec<Instance>,
    layers: &mut [u32; HUD_LAYERS + 1],
) {
    let mut counts = [0u32; HUD_LAYERS];
    for quad in quads {
        counts[hud_layer(quad.layer)] += 1;
    }
    layers[0] = 0;
    for index in 0..HUD_LAYERS {
        layers[index + 1] = layers[index] + counts[index];
    }

    out.clear();
    out.resize(quads.len(), Instance::ZERO);
    let mut cursors = [0u32; HUD_LAYERS];
    cursors.copy_from_slice(&layers[..HUD_LAYERS]);
    for quad in quads {
        let layer = hud_layer(quad.layer);
        let at = &mut cursors[layer];
        out[*at as usize] = hud_quad_instance(quad, line_height);
        *at += 1;
    }
}

fn push_hud_label(
    atlas: &mut GlyphAtlas,
    layout: &mut LineLayout,
    font: &Font,
    label: &Label,
    out: &mut Vec<(u64, Instance)>,
) {
    let layer = hud_layer(label.layer);
    let z = layer as f32 * HUD_LAYER_STEP + HUD_TEXT_LIFT;
    let baseline = -label.y;
    layout.build(font, &label.text);
    for placement in &layout.placements {
        let slot = atlas.slot_for(font, placement.key, LOD_NEAR);
        if slot == BLANK {
            continue;
        }
        out.push((
            hud_text_key(layer, slot),
            Instance {
                offset: [label.x + placement.x, baseline, z],
                scale: [1.0, 1.0],
                color: label.color,
            },
        ));
    }
}

fn group_hud_text(
    scratch: &mut [(u64, Instance)],
    slots: &[Slot],
    values: &mut Vec<Instance>,
    ranges: &mut Vec<HudRange>,
) -> u32 {
    scratch.sort_unstable_by_key(|(key, _)| *key);
    values.clear();
    values.extend(scratch.iter().map(|(_, instance)| *instance));
    ranges.clear();

    let mut triangles = 0u32;
    let mut index = 0usize;
    while index < scratch.len() {
        let key = scratch[index].0;
        let start = index;
        while index < scratch.len() && scratch[index].0 == key {
            index += 1;
        }
        let slot = (key & 0xffff_ffff) as u32;
        let count = (index - start) as u32;
        if let Some(entry) = slots.get(slot as usize) {
            triangles += count * (entry.index_count / 3);
        }
        ranges.push(HudRange {
            slot,
            layer: (key >> 32) as u8,
            start: start as u32,
            end: index as u32,
        });
    }
    triangles
}

fn mix_bytes(hash: u64, bytes: &[u8]) -> u64 {
    let mut hash = hash;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn mix_f32(hash: u64, value: f32) -> u64 {
    mix_bytes(hash, &value.to_bits().to_ne_bytes())
}

fn mix_usize(hash: u64, value: usize) -> u64 {
    mix_bytes(hash, &(value as u64).to_ne_bytes())
}

fn hud_fingerprint(quads: &[Quad], labels: &[Label]) -> u64 {
    let mut hash = mix_usize(FNV_BASIS, quads.len());
    hash = mix_usize(hash, labels.len());
    for quad in quads {
        hash = mix_f32(hash, quad.rect.x);
        hash = mix_f32(hash, quad.rect.y);
        hash = mix_f32(hash, quad.rect.w);
        hash = mix_f32(hash, quad.rect.h);
        hash = mix_bytes(hash, &quad.color);
        hash = mix_bytes(hash, &[quad.layer]);
    }
    for label in labels {
        hash = mix_f32(hash, label.x);
        hash = mix_f32(hash, label.y);
        hash = mix_bytes(hash, &label.color);
        hash = mix_bytes(hash, &[label.layer]);
        hash = mix_bytes(hash, label.text.as_bytes());
        hash = mix_bytes(hash, &[0]);
    }
    hash | 1
}

fn diagnostics_fingerprint(spans: &[DiagnosticSpan]) -> u64 {
    let mut hash = mix_usize(FNV_BASIS, spans.len());
    for span in spans {
        hash = mix_usize(hash, span.line);
        hash = mix_usize(hash, span.start);
        hash = mix_usize(hash, span.end);
        hash = mix_bytes(hash, &[severity_slot(span.severity) as u8]);
    }
    hash | 1
}

fn gutter_fingerprint(marks: &[GutterMark]) -> u64 {
    let mut hash = mix_usize(FNV_BASIS, marks.len());
    for mark in marks {
        hash = mix_usize(hash, mark.line);
        hash = mix_bytes(hash, &[severity_slot(mark.severity) as u8]);
    }
    hash | 1
}

fn push_span(
    out: &mut Vec<Instance>,
    left: f32,
    right: f32,
    y: f32,
    z: f32,
    band_scale: f32,
    color: [u8; 4],
) {
    let width = right - left;
    if width <= 1e-4 {
        return;
    }
    out.push(Instance {
        offset: [left, y, z],
        scale: [width, band_scale],
        color,
    });
}

fn slot_spans(atlas: &GlyphAtlas) -> Vec<u32> {
    let mut spans = Vec::with_capacity(atlas.slots.len());
    for slot in &atlas.slots {
        let from = slot.index_start as usize;
        let to = from + slot.index_count as usize;
        let mut highest = 0u32;
        for value in &atlas.indices[from..to] {
            highest = highest.max(*value);
        }
        spans.push(if slot.index_count == 0 { 0 } else { highest + 1 });
    }
    spans
}

fn pick_lod(distance: f32, previous: u8) -> u8 {
    let near_out = LOD_NEAR_LIMIT * (1.0 + LOD_HYSTERESIS);
    let near_in = LOD_NEAR_LIMIT * (1.0 - LOD_HYSTERESIS);
    let far_out = LOD_FAR_LIMIT * (1.0 + LOD_HYSTERESIS);
    let far_in = LOD_FAR_LIMIT * (1.0 - LOD_HYSTERESIS);
    match previous {
        LOD_NEAR => {
            if distance > far_out {
                LOD_FAR
            } else if distance > near_out {
                LOD_MID
            } else {
                LOD_NEAR
            }
        }
        LOD_MID => {
            if distance < near_in {
                LOD_NEAR
            } else if distance > far_out {
                LOD_FAR
            } else {
                LOD_MID
            }
        }
        LOD_FAR => {
            if distance < near_in {
                LOD_NEAR
            } else if distance < far_in {
                LOD_MID
            } else {
                LOD_FAR
            }
        }
        _ => {
            if distance < LOD_NEAR_LIMIT {
                LOD_NEAR
            } else if distance < LOD_FAR_LIMIT {
                LOD_MID
            } else {
                LOD_FAR
            }
        }
    }
}

fn indent_level(line: &str) -> usize {
    let mut columns = 0usize;
    for byte in line.as_bytes() {
        match byte {
            b' ' => columns += 1,
            b'\t' => columns += TAB_WIDTH,
            _ => break,
        }
    }
    (columns / TAB_WIDTH).min(MAX_INDENT_LEVEL)
}

fn light_matrix(min: Vec3, max: Vec3, ground_y: f32, light: Vec3) -> Mat4 {
    let low = Vec3::new(min.x, min.y.min(ground_y), min.z);
    let high = max.max(low + Vec3::splat(1e-3));
    let center = (low + high) * 0.5;
    let radius = ((high - low).length() * 0.5).max(1.0);
    let up = if light.y.abs() > 0.95 { Vec3::Z } else { Vec3::Y };
    let view = glam::camera::rh::view::look_at_mat4(center + light * (radius + 2.0), center, up);

    let mut local_min = Vec3::splat(f32::INFINITY);
    let mut local_max = Vec3::splat(f32::NEG_INFINITY);
    for corner in 0..8u32 {
        let point = Vec3::new(
            if corner & 1 == 0 { low.x } else { high.x },
            if corner & 2 == 0 { low.y } else { high.y },
            if corner & 4 == 0 { low.z } else { high.z },
        );
        let local = view.transform_point3(point);
        local_min = local_min.min(local);
        local_max = local_max.max(local);
    }

    let extent_x = quantize(local_max.x - local_min.x + SHADOW_MARGIN * 2.0);
    let extent_y = quantize(local_max.y - local_min.y + SHADOW_MARGIN * 2.0);
    let texel_x = extent_x / SHADOW_SIZE as f32;
    let texel_y = extent_y / SHADOW_SIZE as f32;
    let center_x = (((local_min.x + local_max.x) * 0.5) / texel_x).floor() * texel_x;
    let center_y = (((local_min.y + local_max.y) * 0.5) / texel_y).floor() * texel_y;
    let near = (-local_max.z - SHADOW_MARGIN).max(0.02);
    let far = (-local_min.z + SHADOW_MARGIN).max(near + 0.1);

    let proj = glam::camera::rh::proj::directx::orthographic(
        center_x - extent_x * 0.5,
        center_x + extent_x * 0.5,
        center_y - extent_y * 0.5,
        center_y + extent_y * 0.5,
        near,
        far,
    );
    proj * view
}

fn quantize(extent: f32) -> f32 {
    (extent.max(SHADOW_QUANTUM) / SHADOW_QUANTUM).ceil() * SHADOW_QUANTUM
}

fn draw_groups(pass: &mut wgpu::RenderPass<'_>, atlas: &GlyphAtlas, offsets: &[u32]) {
    let grouped = atlas.slots.len().min(offsets.len().saturating_sub(1));
    for (index, slot) in atlas.slots.iter().take(grouped).enumerate() {
        let start = offsets[index];
        let end = offsets[index + 1];
        if start == end {
            continue;
        }
        pass.draw_indexed(
            slot.index_start..slot.index_start + slot.index_count,
            slot.base_vertex,
            start..end,
        );
    }
}

fn plain_style(ch: char) -> u8 {
    if ch.is_ascii_digit() {
        9
    } else if ch.is_alphabetic() {
        STYLE_TEXT
    } else {
        14
    }
}

fn make_glyph_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    buffers: &[Option<wgpu::VertexBufferLayout<'_>>; 2],
    format: wgpu::TextureFormat,
    multisample: wgpu::MultisampleState,
    pass: GlyphPass<'_>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(pass.label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(pass.vertex_entry),
            compilation_options: Default::default(),
            buffers,
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(depth_state(pass.depth_write, wgpu::CompareFunction::Less)),
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(pass.fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: pass.blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn make_panel_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    entry: &str,
    format: wgpu::TextureFormat,
    multisample: wgpu::MultisampleState,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: PANEL_VERTEX_SIZE,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
            })],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(depth_state(false, wgpu::CompareFunction::Less)),
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("panel_fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

fn create_shadow(device: &wgpu::Device) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow-map"),
            size: wgpu::Extent3d {
                width: SHADOW_SIZE,
                height: SHADOW_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn create_depth(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    sample_count: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: config.width.max(1),
                height: config.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn create_msaa(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    sample_count: u32,
) -> Option<wgpu::TextureView> {
    if sample_count <= 1 {
        return None;
    }
    Some(
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("msaa"),
                size: wgpu::Extent3d {
                    width: config.width.max(1),
                    height: config.height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::GlyphKey;
    use crate::hud::Rect;
    use crate::layout::Placement;

    fn quad_at(x: f32, y: f32, w: f32, h: f32, layer: u8) -> Quad {
        Quad {
            rect: Rect { x, y, w, h },
            color: [1, 2, 3, 4],
            layer,
        }
    }

    fn label_at(x: f32, y: f32, text: &str, layer: u8) -> Label {
        Label {
            x,
            y,
            text: String::from(text),
            color: [9, 8, 7, 255],
            layer,
        }
    }

    fn marked(x: f32) -> Instance {
        Instance {
            offset: [x, 0.0, 0.0],
            scale: [1.0, 1.0],
            color: [0, 0, 0, 255],
        }
    }

    fn layout_of(advances: &[f32]) -> LineLayout {
        let mut layout = LineLayout::default();
        let mut x = 0.0;
        for (column, advance) in advances.iter().enumerate() {
            layout.placements.push(Placement {
                key: GlyphKey { face: 0, gid: 0 },
                x,
                advance: *advance,
                column,
                byte: column,
            });
            x += *advance;
        }
        layout.width = x;
        layout
    }

    #[test]
    fn globals_suit_la_disposition_des_shaders() {
        assert_eq!(std::mem::size_of::<Globals>(), 368);
    }

    #[test]
    fn couche_hud_bornee() {
        assert_eq!(hud_layer(0), 0);
        assert_eq!(hud_layer(7), 7);
        assert_eq!(hud_layer(200), HUD_LAYERS - 1);
    }

    #[test]
    fn quad_hud_retourne_l_axe_vertical() {
        let source = quad_at(3.0, 5.0, 10.0, 2.0, 2);
        let instance = hud_quad_instance(&source, 1.25);
        assert_eq!(instance.offset[0], 3.0);
        assert_eq!(instance.offset[1], -7.0);
        assert!((instance.offset[2] - 2.0 * HUD_LAYER_STEP).abs() < 1e-6);
        assert_eq!(instance.scale[0], 10.0);
        assert!((instance.scale[1] - 2.0 / 1.25).abs() < 1e-6);
        assert_eq!(instance.color, [1, 2, 3, 4]);
    }

    #[test]
    fn quad_hud_refuse_les_tailles_negatives() {
        let instance = hud_quad_instance(&quad_at(0.0, 0.0, -3.0, -2.0, 0), 1.0);
        assert_eq!(instance.scale, [0.0, 0.0]);
    }

    #[test]
    fn quads_hud_groupes_par_couche() {
        let quads = vec![
            quad_at(0.0, 0.0, 1.0, 1.0, 3),
            quad_at(1.0, 0.0, 1.0, 1.0, 0),
            quad_at(2.0, 0.0, 1.0, 1.0, 3),
            quad_at(3.0, 0.0, 1.0, 1.0, 1),
        ];
        let mut out = Vec::new();
        let mut layers = [0u32; HUD_LAYERS + 1];
        build_hud_quads(&quads, 1.0, &mut out, &mut layers);
        assert_eq!(out.len(), 4);
        assert_eq!(layers[0], 0);
        assert_eq!(layers[1], 1);
        assert_eq!(layers[2], 2);
        assert_eq!(layers[3], 2);
        assert_eq!(layers[4], 4);
        assert_eq!(layers[HUD_LAYERS], 4);
        assert_eq!(out[0].offset[0], 1.0);
        assert_eq!(out[1].offset[0], 3.0);
        assert_eq!(out[2].offset[0], 0.0);
        assert_eq!(out[3].offset[0], 2.0);
    }

    #[test]
    fn quads_hud_reutilisent_le_tampon() {
        let mut out = Vec::new();
        let mut layers = [0u32; HUD_LAYERS + 1];
        let quads = vec![quad_at(0.0, 0.0, 1.0, 1.0, 0), quad_at(1.0, 0.0, 1.0, 1.0, 0)];
        build_hud_quads(&quads, 1.0, &mut out, &mut layers);
        let capacity = out.capacity();
        build_hud_quads(&quads[..1], 1.0, &mut out, &mut layers);
        assert_eq!(out.len(), 1);
        assert_eq!(out.capacity(), capacity);
        assert_eq!(layers[HUD_LAYERS], 1);
    }

    #[test]
    fn texte_hud_ordonne_par_couche_puis_par_slot() {
        let slots = vec![
            Slot { index_start: 0, index_count: 6, base_vertex: 0 },
            Slot { index_start: 6, index_count: 6, base_vertex: 4 },
        ];
        let mut scratch = vec![
            (hud_text_key(1, 1), marked(10.0)),
            (hud_text_key(0, 1), marked(20.0)),
            (hud_text_key(0, 0), marked(30.0)),
            (hud_text_key(0, 1), marked(40.0)),
        ];
        let mut values = Vec::new();
        let mut ranges = Vec::new();
        let triangles = group_hud_text(&mut scratch, &slots, &mut values, &mut ranges);
        assert_eq!(ranges.len(), 3);
        assert_eq!((ranges[0].layer, ranges[0].slot, ranges[0].start, ranges[0].end), (0, 0, 0, 1));
        assert_eq!((ranges[1].layer, ranges[1].slot, ranges[1].start, ranges[1].end), (0, 1, 1, 3));
        assert_eq!((ranges[2].layer, ranges[2].slot, ranges[2].start, ranges[2].end), (1, 1, 3, 4));
        assert_eq!(values.len(), 4);
        assert_eq!(values[0].offset[0], 30.0);
        assert_eq!(values[3].offset[0], 10.0);
        assert_eq!(triangles, 8);
    }

    #[test]
    fn empreinte_hud_stable_et_sensible() {
        let quads = vec![quad_at(0.0, 0.0, 4.0, 2.0, 1)];
        let labels = vec![label_at(1.0, 2.0, "onglets", 1)];
        let base = hud_fingerprint(&quads, &labels);
        assert_ne!(base, 0);
        assert_eq!(base, hud_fingerprint(&quads, &labels));
        assert_ne!(base, hud_fingerprint(&[quad_at(0.5, 0.0, 4.0, 2.0, 1)], &labels));
        assert_ne!(base, hud_fingerprint(&[quad_at(0.0, 0.0, 4.0, 2.0, 2)], &labels));
        assert_ne!(base, hud_fingerprint(&quads, &[label_at(1.0, 2.0, "onglet", 1)]));
        assert_ne!(base, hud_fingerprint(&quads, &[label_at(1.0, 2.5, "onglets", 1)]));
        assert_ne!(base, hud_fingerprint(&[], &labels));
        assert_ne!(base, hud_fingerprint(&quads, &[]));
        let recolore = Label {
            x: 1.0,
            y: 2.0,
            text: String::from("onglets"),
            color: [1, 1, 1, 255],
            layer: 1,
        };
        assert_ne!(base, hud_fingerprint(&quads, &[recolore]));
    }

    #[test]
    fn empreinte_hud_separe_les_libelles() {
        let un = vec![label_at(0.0, 0.0, "ab", 0), label_at(0.0, 0.0, "c", 0)];
        let deux = vec![label_at(0.0, 0.0, "a", 0), label_at(0.0, 0.0, "bc", 0)];
        assert_ne!(hud_fingerprint(&[], &un), hud_fingerprint(&[], &deux));
    }

    #[test]
    fn empreintes_de_diagnostics_distinguent_la_severite() {
        let erreur = [DiagnosticSpan { line: 3, start: 1, end: 4, severity: Severity::Error }];
        let alerte = [DiagnosticSpan { line: 3, start: 1, end: 4, severity: Severity::Warning }];
        assert_ne!(diagnostics_fingerprint(&erreur), diagnostics_fingerprint(&alerte));
        assert_eq!(diagnostics_fingerprint(&erreur), diagnostics_fingerprint(&erreur));
        assert_ne!(diagnostics_fingerprint(&[]), 0);

        let une = [GutterMark { line: 2, severity: Severity::Error }];
        let autre = [GutterMark { line: 2, severity: Severity::Hint }];
        assert_ne!(gutter_fingerprint(&une), gutter_fingerprint(&autre));
        assert_ne!(gutter_fingerprint(&[]), 0);
    }

    #[test]
    fn teinte_de_diagnostic_conserve_la_syntaxe() {
        let melange = tint_color([200, 100, 0, 255], [0, 200, 100, 255]);
        assert_eq!(melange, [90, 155, 55, 255]);
        let sature = tint_color([255, 255, 255, 128], [255, 255, 255, 255]);
        assert_eq!(sature, [255, 255, 255, 128]);
    }

    #[test]
    fn severite_la_plus_grave_gagne() {
        let spans = [
            DiagnosticSpan { line: 4, start: 2, end: 8, severity: Severity::Warning },
            DiagnosticSpan { line: 4, start: 4, end: 6, severity: Severity::Error },
        ];
        assert_eq!(severity_at(&spans, 1), None);
        assert_eq!(severity_at(&spans, 2), Some(Severity::Warning));
        assert_eq!(severity_at(&spans, 5), Some(Severity::Error));
        assert_eq!(severity_at(&spans, 7), Some(Severity::Warning));
        assert_eq!(severity_at(&spans, 8), None);
        assert_eq!(severity_at(&[], 0), None);
    }

    #[test]
    fn plage_vide_ne_teinte_aucun_glyphe() {
        let spans = [DiagnosticSpan { line: 0, start: 3, end: 3, severity: Severity::Error }];
        assert_eq!(severity_at(&spans, 2), None);
        assert_eq!(severity_at(&spans, 3), None);
    }

    #[test]
    fn spans_tries_et_bornes_redressees() {
        let mut spans = vec![
            DiagnosticSpan { line: 9, start: 1, end: 2, severity: Severity::Hint },
            DiagnosticSpan { line: 2, start: 7, end: 9, severity: Severity::Warning },
            DiagnosticSpan { line: 2, start: 4, end: 1, severity: Severity::Error },
        ];
        normalize_spans(&mut spans);
        assert_eq!(spans[0].line, 2);
        assert_eq!(spans[0].start, 4);
        assert_eq!(spans[0].end, 4);
        assert_eq!(spans[1].start, 7);
        assert_eq!(spans[2].line, 9);
    }

    #[test]
    fn marques_de_gouttiere_une_par_ligne() {
        let mut marks = vec![
            GutterMark { line: 5, severity: Severity::Warning },
            GutterMark { line: 5, severity: Severity::Error },
            GutterMark { line: 1, severity: Severity::Hint },
            GutterMark { line: 1, severity: Severity::Information },
        ];
        normalize_marks(&mut marks);
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].line, 1);
        assert_eq!(marks[0].severity, Severity::Information);
        assert_eq!(marks[1].line, 5);
        assert_eq!(marks[1].severity, Severity::Error);
    }

    #[test]
    fn compte_de_chiffres() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(999), 3);
        assert_eq!(digit_count(1000), 4);
        assert_eq!(digit_count(123456), 6);
    }

    #[test]
    fn projection_hud_couvre_exactement_l_ecran() {
        let ndc = hud_ndc(80.0, 40.0);
        assert!((0.0 * ndc[0] + ndc[1] + 1.0).abs() < 1e-6);
        assert!((80.0 * ndc[0] + ndc[1] - 1.0).abs() < 1e-6);
        assert!((0.0 * ndc[2] + ndc[3] - 1.0).abs() < 1e-6);
        assert!((-40.0 * ndc[2] + ndc[3] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn les_couches_hud_ne_se_chevauchent_pas_en_profondeur() {
        let sommet = HUD_TEXT_LIFT + GLYPH_HALF_DEPTH;
        assert!(sommet < HUD_LAYER_STEP);
        assert!(HUD_TEXT_LIFT > 0.0);
    }

    #[test]
    fn bornes_de_plage_suivent_la_fonte_proportionnelle() {
        let layout = layout_of(&[0.4, 0.9, 0.5, 0.7]);
        let (left, right) = span_bounds(&layout, 1, 3, 0.6);
        assert!((left - 0.4).abs() < 1e-6);
        assert!((right - 1.8).abs() < 1e-6);
        let (debut, fin) = span_bounds(&layout, 0, 6, 0.6);
        assert!((debut - 0.0).abs() < 1e-6);
        assert!((fin - (2.5 + 2.0 * 0.6 * 0.5)).abs() < 1e-6);
    }

    #[test]
    fn bornes_de_plage_sur_ligne_vide() {
        let layout = layout_of(&[]);
        let (left, right) = span_bounds(&layout, 0, 0, 0.6);
        assert_eq!(left, 0.0);
        assert_eq!(right, 0.0);
    }

    #[test]
    fn severites_couvrent_toute_la_table() {
        assert_eq!(severity_slot(Severity::Error), 0);
        assert_eq!(severity_slot(Severity::Warning), 1);
        assert_eq!(severity_slot(Severity::Information), 2);
        assert_eq!(severity_slot(Severity::Hint), 3);
        assert_eq!(SEVERITY_COLORS.len(), 4);
    }
}

fn shorten(text: &str, limit: usize) -> &str {
    match text.char_indices().nth(limit) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}
