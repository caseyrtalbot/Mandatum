//! GPU frontend: wgpu surface + an instanced solid-quad pipeline for cell
//! backgrounds/selection/cursor/status, layered under GPU-rasterized glyphs
//! rendered by glyphon. All rendering is per-frame from a grid snapshot.

use std::sync::Arc;
use std::time::Instant;

use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use mandatum_terminal_vt::{Color, TerminalCell};
use winit::window::Window;

use crate::terminal::{Selection, TerminalSession};

const DEFAULT_FG: [u8; 3] = [220, 220, 224];
const DEFAULT_BG: [u8; 3] = [18, 18, 22];
const SELECTION_BG: [u8; 4] = [70, 100, 180, 150];
const CURSOR_BG: [u8; 4] = [210, 210, 220, 150];
const STATUS_BG: [u8; 3] = [30, 32, 40];
const STATUS_FG: [u8; 3] = [170, 176, 190];
const BASE_FONT_PT: f32 = 15.0;

pub struct GpuText {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    // Solid-quad pipeline.
    quad_pipeline: wgpu::RenderPipeline,
    unit_buf: wgpu::Buffer,
    inst_buf: wgpu::Buffer,
    inst_capacity_floats: usize,
    res_buf: wgpu::Buffer,
    res_bind_group: wgpu::BindGroup,

    // Text stack.
    font_system: FontSystem,
    swash_cache: SwashCache,
    #[allow(dead_code)]
    cache: Cache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffer: Buffer,
    status_buffer: Buffer,

    scale: f32,
    font_size: f32,
    cell_w: f32,
    cell_h: f32,
}

impl GpuText {
    pub async fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("mandatum-spike-device"),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("request_device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // --- Quad pipeline ---------------------------------------------------
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad-shader"),
            source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
        });
        let res_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resolution-uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("res-bind-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let res_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("res-bind-group"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: res_buf.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        const UNIT_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
        const INST_ATTRS: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4];
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &UNIT_ATTRS,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 32,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &INST_ATTRS,
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let unit: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let unit_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unit-quad"),
            size: 32,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&unit_buf, 0, bytes_of(&unit));

        let inst_capacity_floats = 8 * 4096;
        let inst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad-instances"),
            size: (inst_capacity_floats * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Text stack ------------------------------------------------------
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        let font_size = (BASE_FONT_PT * scale).round();
        let line_height = (font_size * 1.3).round();
        let metrics = Metrics::new(font_size, line_height);
        let text_buffer = Buffer::new(&mut font_system, metrics);
        let status_buffer = Buffer::new(&mut font_system, metrics);
        let cell_w = measure_cell_width(&mut font_system, metrics);
        let cell_h = line_height;

        Ok(Self {
            surface,
            device,
            queue,
            config,
            quad_pipeline,
            unit_buf,
            inst_buf,
            inst_capacity_floats,
            res_buf,
            res_bind_group,
            font_system,
            swash_cache,
            cache,
            viewport,
            atlas,
            text_renderer,
            text_buffer,
            status_buffer,
            scale,
            font_size,
            cell_w,
            cell_h,
        })
    }

    pub fn cell_w(&self) -> f32 {
        self.cell_w
    }

    pub fn cell_h(&self) -> f32 {
        self.cell_h
    }

    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize_surface(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() < f32::EPSILON {
            return;
        }
        self.scale = scale;
        self.font_size = (BASE_FONT_PT * scale).round();
        let line_height = (self.font_size * 1.3).round();
        let metrics = Metrics::new(self.font_size, line_height);
        self.text_buffer.set_metrics(metrics);
        self.status_buffer.set_metrics(metrics);
        self.cell_w = measure_cell_width(&mut self.font_system, metrics);
        self.cell_h = line_height;
    }

    /// Render one frame from the session snapshot. Returns the instant right
    /// after `present()` so the caller can measure input-to-present latency, or
    /// `None` when the swapchain frame could not be acquired (skip).
    pub fn render(
        &mut self,
        session: &TerminalSession,
        selection: Option<&Selection>,
        status: &str,
    ) -> Option<Instant> {
        let cols = session.cols();
        let rows = session.rows();
        let grid = session.grid();
        let top_abs = session.top_absolute_row();
        let cursor = grid.cursor();

        // Assemble foreground text (rich-text color runs) and background quads.
        let mut screen_text = String::with_capacity(usize::from(cols + 1) * usize::from(rows));
        let mut runs: Vec<(std::ops::Range<usize>, GColor)> = Vec::new();
        let mut quads: Vec<f32> = Vec::with_capacity(1024);

        for screen_row in 0..rows {
            let abs = top_abs + screen_row as isize;
            let mut run_start = screen_text.len();
            let mut run_color: Option<GColor> = None;
            for col in 0..cols {
                let cell = if abs >= 0 {
                    grid.history_cell(abs as usize, col)
                } else {
                    None
                }
                .unwrap_or_else(TerminalCell::blank);
                let style = cell.style();
                let (mut fg, mut bg) = (
                    resolve(style.foreground, DEFAULT_FG),
                    resolve(style.background, DEFAULT_BG),
                );
                if style.inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let selected = selection.map(|s| s.contains(abs, col)).unwrap_or(false);
                let px = col as f32 * self.cell_w;
                let py = screen_row as f32 * self.cell_h;
                if bg != DEFAULT_BG {
                    push_quad(&mut quads, px, py, self.cell_w, self.cell_h, [bg[0], bg[1], bg[2], 255]);
                }
                if selected {
                    push_quad(&mut quads, px, py, self.cell_w, self.cell_h, SELECTION_BG);
                }

                let ch = cell.character();
                let gc = GColor::rgb(fg[0], fg[1], fg[2]);
                if run_color != Some(gc) {
                    if let Some(prev) = run_color.take() {
                        runs.push((run_start..screen_text.len(), prev));
                    }
                    run_start = screen_text.len();
                    run_color = Some(gc);
                }
                screen_text.push(ch);
            }
            if let Some(prev) = run_color.take() {
                runs.push((run_start..screen_text.len(), prev));
            }
            screen_text.push('\n');
        }

        // Cursor block (only while following live output).
        if session.at_live_bottom() && cursor.visible() {
            let px = cursor.column() as f32 * self.cell_w;
            let py = cursor.row() as f32 * self.cell_h;
            push_quad(&mut quads, px, py, self.cell_w, self.cell_h, CURSOR_BG);
        }

        // Status strip background across the last line.
        let status_y = rows as f32 * self.cell_h;
        push_quad(
            &mut quads,
            0.0,
            status_y,
            self.config.width as f32,
            self.cell_h,
            [STATUS_BG[0], STATUS_BG[1], STATUS_BG[2], 255],
        );

        // Upload text. `Attrs::color` consumes self, so build fresh per span.
        let default_attrs = Attrs::new().family(Family::Monospace);
        let spans = runs.iter().map(|(range, color)| {
            (
                &screen_text[range.clone()],
                Attrs::new().family(Family::Monospace).color(*color),
            )
        });
        self.text_buffer
            .set_size(Some(self.config.width as f32), Some(status_y.max(1.0)));
        self.text_buffer
            .set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        self.text_buffer
            .shape_until_scroll(&mut self.font_system, false);

        self.status_buffer
            .set_size(Some(self.config.width as f32), Some(self.cell_h));
        self.status_buffer.set_text(
            status,
            &Attrs::new()
                .family(Family::Monospace)
                .color(GColor::rgb(STATUS_FG[0], STATUS_FG[1], STATUS_FG[2])),
            Shaping::Advanced,
            None,
        );
        self.status_buffer
            .shape_until_scroll(&mut self.font_system, false);

        // Upload quad instances (grow buffer if needed).
        if quads.len() > self.inst_capacity_floats {
            self.inst_capacity_floats = quads.len().next_power_of_two();
            self.inst_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad-instances"),
                size: (self.inst_capacity_floats * 4) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(&self.inst_buf, 0, bytes_of_slice(&quads));
        let instance_count = (quads.len() / 8) as u32;

        let res = [self.config.width as f32, self.config.height as f32, 0.0, 0.0];
        self.queue.write_buffer(&self.res_buf, 0, bytes_of(&res));

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let full = TextBounds {
            left: 0,
            top: 0,
            right: self.config.width as i32,
            bottom: self.config.height as i32,
        };
        if self
            .text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                [
                    TextArea {
                        buffer: &self.text_buffer,
                        left: 0.0,
                        top: 0.0,
                        scale: 1.0,
                        bounds: full,
                        default_color: GColor::rgb(DEFAULT_FG[0], DEFAULT_FG[1], DEFAULT_FG[2]),
                        custom_glyphs: &[],
                    },
                    TextArea {
                        buffer: &self.status_buffer,
                        left: 6.0,
                        top: status_y,
                        scale: 1.0,
                        bounds: full,
                        default_color: GColor::rgb(STATUS_FG[0], STATUS_FG[1], STATUS_FG[2]),
                        custom_glyphs: &[],
                    },
                ],
                &mut self.swash_cache,
            )
            .is_err()
        {
            return None;
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => return None,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: DEFAULT_BG[0] as f64 / 255.0,
                            g: DEFAULT_BG[1] as f64 / 255.0,
                            b: DEFAULT_BG[2] as f64 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if instance_count > 0 {
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_bind_group(0, &self.res_bind_group, &[]);
                pass.set_vertex_buffer(0, self.unit_buf.slice(..));
                pass.set_vertex_buffer(1, self.inst_buf.slice(..));
                pass.draw(0..4, 0..instance_count);
            }
            let _ = self
                .text_renderer
                .render(&self.atlas, &self.viewport, &mut pass);
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        Some(Instant::now())
    }
}

/// Map an engine terminal color onto RGB, using the given default for
/// `Color::Default`, the standard xterm palette for indexed colors, and a
/// passthrough for direct RGB.
fn resolve(color: Color, default: [u8; 3]) -> [u8; 3] {
    match color {
        Color::Default => default,
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(i) => palette(i),
    }
}

fn palette(i: u8) -> [u8; 3] {
    const BASE: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [128, 128, 128],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let n = i - 16;
            let steps = [0u8, 95, 135, 175, 215, 255];
            [
                steps[(n / 36) as usize],
                steps[((n / 6) % 6) as usize],
                steps[(n % 6) as usize],
            ]
        }
        _ => {
            let v = 8 + 10 * (i - 232);
            [v, v, v]
        }
    }
}

fn push_quad(buf: &mut Vec<f32>, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) {
    buf.extend_from_slice(&[
        x,
        y,
        w,
        h,
        rgba[0] as f32 / 255.0,
        rgba[1] as f32 / 255.0,
        rgba[2] as f32 / 255.0,
        rgba[3] as f32 / 255.0,
    ]);
}

/// Measure a monospace advance width by shaping a run of identical glyphs and
/// dividing the laid-out line width by the glyph count.
fn measure_cell_width(font_system: &mut FontSystem, metrics: Metrics) -> f32 {
    let mut buffer = Buffer::new(font_system, metrics);
    let mono = Attrs::new().family(Family::Monospace);
    buffer.set_text("MMMMMMMMMMMMMMMMMMMM", &mono, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    let width = buffer
        .layout_runs()
        .next()
        .map(|run| run.line_w)
        .unwrap_or(metrics.font_size * 0.6);
    (width / 20.0).max(1.0)
}

fn bytes_of<T: Copy>(value: &T) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>())
    }
}

fn bytes_of_slice<T: Copy>(slice: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            slice.as_ptr() as *const u8,
            std::mem::size_of_val(slice),
        )
    }
}

const QUAD_WGSL: &str = r#"
struct Res { size: vec4<f32> };
@group(0) @binding(0) var<uniform> res: Res;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(@location(0) unit: vec2<f32>,
      @location(1) rect: vec4<f32>,
      @location(2) color: vec4<f32>) -> VOut {
    let px = rect.xy + unit * rect.zw;
    let ndc = vec2<f32>(px.x / res.size.x * 2.0 - 1.0, 1.0 - px.y / res.size.y * 2.0);
    var out: VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;
