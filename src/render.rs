use std::ops::Range;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use winit::window::Window;

use crate::atlas::{BLANK, GlyphAtlas};
use crate::camera::Camera;
use crate::complete::{Completion, VISIBLE_ROWS};
use crate::extrude::Vertex;
use crate::font::Font;
use crate::syntax::{Highlighter, STYLE_COLORS, STYLE_TEXT};
use crate::text::TextBuffer;

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
const TAG_COLUMNS: f32 = 4.4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
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
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    offset: [f32; 3],
    color: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PanelVertex {
    position: [f32; 3],
    color: [u8; 4],
}

struct InstanceSet {
    values: Vec<Instance>,
    offsets: Vec<u32>,
    scratch: Vec<(u32, Instance)>,
    cursors: Vec<u32>,
    buffer: wgpu::Buffer,
    capacity: u32,
}

impl InstanceSet {
    fn new(device: &wgpu::Device, label: &str) -> InstanceSet {
        InstanceSet {
            values: Vec::new(),
            offsets: Vec::new(),
            scratch: Vec::new(),
            cursors: Vec::new(),
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 1024,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: 64,
        }
    }

    fn group(&mut self, slot_count: usize) {
        self.offsets.clear();
        self.offsets.resize(slot_count + 1, 0);
        for (slot, _) in &self.scratch {
            self.offsets[*slot as usize + 1] += 1;
        }
        for index in 1..=slot_count {
            self.offsets[index] += self.offsets[index - 1];
        }

        self.values.clear();
        self.values
            .resize(self.scratch.len(), Instance { offset: [0.0; 3], color: [0; 4] });
        self.cursors.clear();
        self.cursors.extend_from_slice(&self.offsets);
        for (slot, instance) in &self.scratch {
            let at = &mut self.cursors[*slot as usize];
            self.values[*at as usize] = *instance;
            *at += 1;
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str) {
        if self.values.is_empty() {
            return;
        }
        let needed = self.values.len() as u32;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(self.capacity) * 16,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.values));
    }
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    sample_count: u32,

    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    background_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    popup_pipeline: wgpu::RenderPipeline,
    panel_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,

    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    cursor_buffer: wgpu::Buffer,
    panel_buffer: wgpu::Buffer,
    panel_capacity: u32,
    panel_vertices: Vec<PanelVertex>,

    text_instances: InstanceSet,
    popup_instances: InstanceSet,

    depth_view: wgpu::TextureView,
    msaa_view: Option<wgpu::TextureView>,

    pub atlas: GlyphAtlas,
    pub show_grid: bool,
    pub wave: f32,
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

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals-bind"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline-layout"),
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

        let multisample = wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        };

        let glyph_buffers = [
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
            },
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![2 => Float32x3, 3 => Unorm8x4],
            },
        ];

        let glyph_pipeline = make_glyph_pipeline(
            &device, &layout, &glyph_shader, "vs_main", &glyph_buffers, format, multisample,
        );
        let popup_pipeline = make_glyph_pipeline(
            &device, &layout, &glyph_shader, "vs_popup", &glyph_buffers, format, multisample,
        );

        let background_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("background-pipeline"),
            layout: Some(&layout),
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

        let panel_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("panel-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &stage_shader,
                entry_point: Some("panel_vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PanelVertex>() as u64,
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
                module: &stage_shader,
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
        });

        let grid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grid-pipeline"),
            layout: Some(&layout),
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

        let depth_view = create_depth(&device, &config, sample_count);
        let msaa_view = create_msaa(&device, &config, sample_count);
        let atlas = GlyphAtlas::new(font);

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("indices"),
            size: 64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cursor_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cursor"),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let panel_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("panel"),
            size: 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let text_instances = InstanceSet::new(&device, "text-instances");
        let popup_instances = InstanceSet::new(&device, "popup-instances");

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            sample_count,
            globals,
            bind_group,
            background_pipeline,
            glyph_pipeline,
            popup_pipeline,
            panel_pipeline,
            grid_pipeline,
            vertex_buffer,
            index_buffer,
            cursor_buffer,
            panel_buffer,
            panel_capacity: 64,
            panel_vertices: Vec::new(),
            text_instances,
            popup_instances,
            depth_view,
            msaa_view,
            atlas,
            show_grid: true,
            wave: 0.014,
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

    pub fn rebuild(&mut self, text: &TextBuffer, font: &Font, highlighter: Option<&Highlighter>) {
        let advance = font.advance();
        let line_height = font.line_height();
        let window = visible_lines(text);

        self.text_instances.scratch.clear();
        for line_index in window {
            let y = -(line_index as f32) * line_height;
            let line_start = text.line_start(line_index);
            for (column, (byte, ch)) in text.lines[line_index].char_indices().enumerate() {
                if ch == ' ' {
                    continue;
                }
                let slot = self.atlas.slot_for(font, ch);
                if slot == BLANK {
                    continue;
                }
                let style = match highlighter {
                    Some(highlighter) => highlighter.style_at(line_start + byte),
                    None => plain_style(ch),
                };
                self.text_instances.scratch.push((
                    slot,
                    Instance {
                        offset: [column as f32 * advance, y, 0.0],
                        color: STYLE_COLORS[style as usize],
                    },
                ));
            }
        }

        self.text_instances.group(self.atlas.slots.len());
        self.upload_atlas();
        self.text_instances
            .upload(&self.device, &self.queue, "text-instances");

        let cursor = Instance {
            offset: [
                text.cursor_col as f32 * advance,
                -(text.cursor_line as f32) * line_height,
                0.0,
            ],
            color: [255, 194, 102, 255],
        };
        self.queue
            .write_buffer(&self.cursor_buffer, 0, bytemuck::bytes_of(&cursor));
    }

    pub fn set_popup(&mut self, completion: &Completion, text: &TextBuffer, font: &Font) {
        self.popup_instances.scratch.clear();
        self.panel_vertices.clear();
        if !completion.active {
            self.popup_instances.values.clear();
            return;
        }

        let advance = font.advance();
        let line_height = font.line_height();
        let origin_x = text.cursor_col as f32 * advance;
        let origin_y = -(text.cursor_line as f32) * line_height - line_height;

        let rows = completion
            .items
            .iter()
            .skip(completion.scroll)
            .take(VISIBLE_ROWS)
            .enumerate();
        let mut widest = 0usize;

        for (row, candidate) in rows {
            let y = origin_y - row as f32 * line_height;
            let selected = completion.scroll + row == completion.selected;
            widest = widest.max(candidate.text.chars().count());

            let (name_color, tag_color) = if selected {
                ([255, 226, 176, 255], [198, 214, 240, 255])
            } else {
                (STYLE_COLORS[candidate.kind.style() as usize], TAG_COLOR)
            };
            self.push_popup_text(font, candidate.kind.tag(), origin_x + 0.4 * advance, y, tag_color);
            self.push_popup_text(
                font,
                &candidate.text,
                origin_x + TAG_COLUMNS * advance,
                y,
                name_color,
            );
        }

        let shown = completion.items.len().saturating_sub(completion.scroll).min(VISIBLE_ROWS);
        let left = origin_x - 0.3;
        let right = origin_x + (TAG_COLUMNS + widest as f32 + 0.6) * advance;
        let top = origin_y + font.ascender() * 0.85 + 0.14;
        let bottom = origin_y - (shown.saturating_sub(1)) as f32 * line_height
            + font.descender() * 0.85
            - 0.14;

        push_quad(
            &mut self.panel_vertices,
            [left - 0.08, bottom - 0.08],
            [right + 0.08, top + 0.08],
            POPUP_Z - 0.16,
            PANEL_EDGE,
        );
        push_quad(
            &mut self.panel_vertices,
            [left, bottom],
            [right, top],
            POPUP_Z - 0.12,
            PANEL_FILL,
        );

        let row = completion.selected.saturating_sub(completion.scroll);
        let row_y = origin_y - row as f32 * line_height;
        push_quad(
            &mut self.panel_vertices,
            [left, row_y + font.descender() * 0.85],
            [right, row_y + font.ascender() * 0.85],
            POPUP_Z - 0.06,
            PANEL_SELECTED,
        );

        self.popup_instances.group(self.atlas.slots.len());
        self.upload_atlas();
        self.popup_instances
            .upload(&self.device, &self.queue, "popup-instances");
        self.upload_panel();
    }

    fn push_popup_text(&mut self, font: &Font, text: &str, x: f32, y: f32, color: [u8; 4]) {
        let advance = font.advance();
        for (column, ch) in text.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let slot = self.atlas.slot_for(font, ch);
            if slot == BLANK {
                continue;
            }
            self.popup_instances.scratch.push((
                slot,
                Instance {
                    offset: [x + column as f32 * advance, y, POPUP_Z],
                    color,
                },
            ));
        }
    }

    fn upload_panel(&mut self) {
        if self.panel_vertices.is_empty() {
            return;
        }
        let needed = self.panel_vertices.len() as u32;
        if needed > self.panel_capacity {
            self.panel_capacity = needed.next_power_of_two();
            self.panel_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("panel"),
                size: u64::from(self.panel_capacity) * 16,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(
            &self.panel_buffer,
            0,
            bytemuck::cast_slice(&self.panel_vertices),
        );
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
        self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.index_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("indices"),
            size: index_bytes.len() as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&self.vertex_buffer, 0, vertex_bytes);
        self.queue.write_buffer(&self.index_buffer, 0, index_bytes);
    }

    pub fn render(
        &mut self,
        camera: &Camera,
        font: &Font,
        cursor_y: f32,
        time: f32,
        show_cursor: bool,
    ) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };

        let eye = camera.eye();
        let line_height = font.line_height();
        let cull_reach = VISIBLE_RADIUS as f32 * line_height * 0.92;
        let fog_end = (camera.distance * 4.2).min(cull_reach);
        let fog_start = (camera.distance * 1.25).min(fog_end * 0.4);
        let light = Vec3::new(0.42, 0.76, 0.55).normalize();
        let (_, half_height) = camera.half_extent(self.aspect());

        let globals = Globals {
            view_proj: camera.view_proj(self.aspect()).to_cols_array_2d(),
            camera_pos: [eye.x, eye.y, eye.z, 1.0],
            light_dir: [light.x, light.y, light.z, 0.0],
            fog: [fog_start, fog_end, cursor_y, line_height],
            bg_top: BG_TOP,
            bg_bottom: BG_BOTTOM,
            ink: INK,
            accent: ACCENT,
            highlight: HIGHLIGHT,
            ground: [
                camera.target.y - half_height * 1.25,
                2.0,
                camera.distance * 0.8,
                camera.distance * 2.6,
            ],
            params: [
                time,
                self.wave,
                self.config.width as f32,
                self.config.height as f32,
            ],
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        {
            let (target, resolve) = match &self.msaa_view {
                Some(msaa) => (msaa, Some(&view)),
                None => (&view, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: resolve,
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
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_pipeline(&self.background_pipeline);
            pass.draw(0..3, 0..1);

            if !self.atlas.indices.is_empty() {
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                pass.set_pipeline(&self.glyph_pipeline);
                if !self.text_instances.values.is_empty() {
                    pass.set_vertex_buffer(1, self.text_instances.buffer.slice(..));
                    draw_groups(&mut pass, &self.atlas, &self.text_instances.offsets);
                }

                if show_cursor {
                    let slot = &self.atlas.slots[self.atlas.cursor_slot as usize];
                    pass.set_vertex_buffer(1, self.cursor_buffer.slice(..));
                    pass.draw_indexed(
                        slot.index_start..slot.index_start + slot.index_count,
                        slot.base_vertex,
                        0..1,
                    );
                }

                if !self.popup_instances.values.is_empty() {
                    pass.set_pipeline(&self.popup_pipeline);
                    pass.set_vertex_buffer(1, self.popup_instances.buffer.slice(..));
                    draw_groups(&mut pass, &self.atlas, &self.popup_instances.offsets);
                }
            }

            if self.show_grid {
                pass.set_pipeline(&self.grid_pipeline);
                pass.draw(0..4, 0..1);
            }

            if !self.panel_vertices.is_empty() {
                pass.set_pipeline(&self.panel_pipeline);
                pass.set_vertex_buffer(0, self.panel_buffer.slice(..));
                pass.draw(0..self.panel_vertices.len() as u32, 0..1);
            }
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
    }
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

fn push_quad(out: &mut Vec<PanelVertex>, min: [f32; 2], max: [f32; 2], z: f32, color: [u8; 4]) {
    let corners = [
        [min[0], min[1], z],
        [max[0], min[1], z],
        [max[0], max[1], z],
        [min[0], max[1], z],
    ];
    for index in [0, 1, 2, 0, 2, 3] {
        out.push(PanelVertex { position: corners[index], color });
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
    entry: &str,
    buffers: &[wgpu::VertexBufferLayout<'_>; 2],
    format: wgpu::TextureFormat,
    multisample: wgpu::MultisampleState,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            buffers: &[Some(buffers[0].clone()), Some(buffers[1].clone())],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Less)),
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
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
