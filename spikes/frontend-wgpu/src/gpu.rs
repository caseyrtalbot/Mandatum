// GPU frontend: wgpu surface + an instanced solid-quad pipeline for cell
// backgrounds/selection/cursor/status, layered under GPU-rasterized glyphs
// rendered by glyphon. All rendering is per-frame from WorkspaceScene.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    Style as FontStyle, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
    Weight, Wrap, cosmic_text::UnderlineStyle,
};
// The renderer consumes ONLY the scene contract. It never imports
// mandatum-terminal-vt: the real app host converts its grids before the
// snapshot reaches this crate, so no parser type crosses into paint.
use mandatum_scene::{
    CellOccupancy, CellProgram, CellSelection, OverlayScene, ProgramCell, SceneColor, SceneRect,
    Theme, WorkspaceScene, compile_cell_program, layout,
};
use winit::window::Window;

const DEFAULT_FG: [u8; 3] = [220, 220, 224];
const DEFAULT_BG: [u8; 3] = [18, 18, 22];
const BASE_FONT_PT: f32 = 15.0;
const MAX_GPU_PANES: usize = 256;
const MAX_GPU_FRAME_CELLS: usize = 262_144;
const MAX_GPU_CELL_INSTRUCTIONS: usize = 4_000_000;
const MAX_GPU_ROW_BUFFERS: usize = 4_096;

#[derive(Debug, PartialEq, Eq)]
pub enum SceneCompileError {
    NoVisiblePane,
    ResourceLimit {
        resource: &'static str,
        actual: usize,
        maximum: usize,
    },
    InvalidGeometry(&'static str),
}

impl std::fmt::Display for SceneCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoVisiblePane => f.write_str("scene has no visible pane"),
            Self::ResourceLimit {
                resource,
                actual,
                maximum,
            } => write!(
                f,
                "scene {resource} exceed the renderer limit: {actual} > {maximum}"
            ),
            Self::InvalidGeometry(reason) => write!(f, "invalid scene geometry: {reason}"),
        }
    }
}

#[derive(Debug)]
pub struct PreparedScene {
    cell_program: CellProgram,
}

impl PreparedScene {
    pub fn cell_program(&self) -> &CellProgram {
        &self.cell_program
    }
}

/// Validate renderer resource and geometry boundaries, then compile the shared
/// renderer-neutral cell program without touching a window or GPU.
pub fn prepare_scene(
    scene: &WorkspaceScene,
    theme: &Theme,
) -> Result<PreparedScene, SceneCompileError> {
    validate_scene_structure(scene)?;
    let cell_program = compile_cell_program(scene, theme);
    validate_compiled_program(&cell_program)?;
    Ok(PreparedScene { cell_program })
}

fn validate_scene_structure(scene: &WorkspaceScene) -> Result<(), SceneCompileError> {
    let workspace = layout::workspace_scene_area(scene.size);
    if scene.panes.is_empty() {
        return Err(SceneCompileError::NoVisiblePane);
    }
    if scene.panes.len() > MAX_GPU_PANES {
        return Err(SceneCompileError::ResourceLimit {
            resource: "panes",
            actual: scene.panes.len(),
            maximum: MAX_GPU_PANES,
        });
    }
    let Some(workspace_right) = rect_right_checked(workspace) else {
        return Err(SceneCompileError::InvalidGeometry(
            "workspace geometry overflows",
        ));
    };
    let Some(workspace_bottom) = rect_bottom_checked(workspace) else {
        return Err(SceneCompileError::InvalidGeometry(
            "workspace geometry overflows",
        ));
    };

    for pane in &scene.panes {
        if !pane_has_usable_interior(pane.area) {
            return Err(SceneCompileError::InvalidGeometry(
                "pane has no usable bordered interior",
            ));
        }
        let Some(right) = rect_right_checked(pane.area) else {
            return Err(SceneCompileError::InvalidGeometry(
                "pane geometry overflows",
            ));
        };
        let Some(bottom) = rect_bottom_checked(pane.area) else {
            return Err(SceneCompileError::InvalidGeometry(
                "pane geometry overflows",
            ));
        };
        if pane.area.x < workspace.x
            || pane.area.y < workspace.y
            || right > workspace_right
            || bottom > workspace_bottom
        {
            return Err(SceneCompileError::InvalidGeometry(
                "pane lies outside the workspace",
            ));
        }
    }

    validate_precompile_resources(scene)?;
    Ok(())
}

fn validate_precompile_resources(scene: &WorkspaceScene) -> Result<(), SceneCompileError> {
    let Some(frame_cells) =
        usize::from(scene.size.width).checked_mul(usize::from(scene.size.height))
    else {
        return Err(SceneCompileError::ResourceLimit {
            resource: "frame cells",
            actual: usize::MAX,
            maximum: MAX_GPU_FRAME_CELLS,
        });
    };
    enforce_resource_limit("frame cells", frame_cells, MAX_GPU_FRAME_CELLS)?;
    enforce_resource_limit(
        "row buffers",
        usize::from(scene.size.height),
        MAX_GPU_ROW_BUFFERS,
    )?;

    // The cell compiler retains only final topmost cells, but it still visits
    // each semantic paint surface. Bound that precompile work, including
    // overlaps, with a conservative four-operation charge for fill, border,
    // text, and replacement.
    let mut painted_cells = 0usize;
    add_rect_cells(&mut painted_cells, scene.header.area)?;
    add_rect_cells(&mut painted_cells, scene.status.area)?;
    for segment in &scene.header.attention {
        add_rect_cells(&mut painted_cells, segment.rect)?;
    }
    for pane in &scene.panes {
        add_rect_cells(&mut painted_cells, pane.area)?;
    }
    if let Some(area) = scene.overlay.as_ref().map(overlay_area) {
        add_rect_cells(&mut painted_cells, area)?;
    }
    let Some(estimated_instructions) = painted_cells.checked_mul(4) else {
        return Err(SceneCompileError::ResourceLimit {
            resource: "cell instructions",
            actual: usize::MAX,
            maximum: MAX_GPU_CELL_INSTRUCTIONS,
        });
    };
    enforce_resource_limit(
        "cell instructions",
        estimated_instructions,
        MAX_GPU_CELL_INSTRUCTIONS,
    )
}

fn validate_compiled_program(program: &CellProgram) -> Result<(), SceneCompileError> {
    let instructions = program.cells().count();
    enforce_resource_limit("cell instructions", instructions, MAX_GPU_CELL_INSTRUCTIONS)?;

    let mut occupied_rows = vec![false; usize::from(program.size().height)];
    for (_, y, _) in program.cells() {
        occupied_rows[usize::from(y)] = true;
    }
    let row_buffers = occupied_rows
        .into_iter()
        .filter(|occupied| *occupied)
        .count();
    enforce_resource_limit("row buffers", row_buffers, MAX_GPU_ROW_BUFFERS)
}

fn enforce_resource_limit(
    resource: &'static str,
    actual: usize,
    maximum: usize,
) -> Result<(), SceneCompileError> {
    if actual > maximum {
        return Err(SceneCompileError::ResourceLimit {
            resource,
            actual,
            maximum,
        });
    }
    Ok(())
}

fn add_rect_cells(total: &mut usize, area: SceneRect) -> Result<(), SceneCompileError> {
    let Some(cells) = usize::from(area.width).checked_mul(usize::from(area.height)) else {
        return Err(SceneCompileError::ResourceLimit {
            resource: "cell instructions",
            actual: usize::MAX,
            maximum: MAX_GPU_CELL_INSTRUCTIONS,
        });
    };
    *total = total.checked_add(cells).unwrap_or(usize::MAX);
    if *total == usize::MAX {
        return Err(SceneCompileError::ResourceLimit {
            resource: "cell instructions",
            actual: usize::MAX,
            maximum: MAX_GPU_CELL_INSTRUCTIONS,
        });
    }
    Ok(())
}

fn overlay_area(overlay: &OverlayScene) -> SceneRect {
    match overlay {
        OverlayScene::Palette(overlay) => overlay.area,
        OverlayScene::ContextMenu(overlay) => overlay.area,
        OverlayScene::Timeline(overlay) => overlay.area,
        OverlayScene::SessionMap(overlay) => overlay.area,
        OverlayScene::Prompt(overlay) => overlay.area,
        OverlayScene::Search(overlay) => overlay.area,
        OverlayScene::Help(overlay) => overlay.area,
        OverlayScene::Welcome(overlay) => overlay.area,
    }
}

fn pane_has_usable_interior(area: SceneRect) -> bool {
    area.width >= 3 && area.height >= 3
}

fn rect_right_checked(area: SceneRect) -> Option<u16> {
    area.x.checked_add(area.width)
}

fn rect_bottom_checked(area: SceneRect) -> Option<u16> {
    area.y.checked_add(area.height)
}

#[derive(Default)]
struct RowBufferPool {
    rows: Vec<Buffer>,
}

impl RowBufferPool {
    fn new() -> Self {
        Self::default()
    }

    fn ensure_len(&mut self, len: usize, font_system: &mut FontSystem, metrics: Metrics) {
        while self.rows.len() < len {
            self.rows.push(Buffer::new(font_system, metrics));
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.rows.len()
    }

    fn set_metrics(&mut self, metrics: Metrics) {
        for buffer in &mut self.rows {
            buffer.set_metrics(metrics);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResolvedCell {
    glyph: char,
    foreground: [u8; 4],
    background: [u8; 4],
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GlyphStyle {
    foreground: [u8; 4],
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

impl From<ResolvedCell> for GlyphStyle {
    fn from(cell: ResolvedCell) -> Self {
        Self {
            foreground: cell.foreground,
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
            strikethrough: cell.strikethrough,
        }
    }
}

#[derive(Debug)]
struct ProgramRow {
    y: u16,
    x: u16,
    text: String,
    runs: Vec<(std::ops::Range<usize>, GlyphStyle)>,
}

#[derive(Debug)]
struct PreparedCellProgram {
    cells: Vec<(u16, u16, ResolvedCell)>,
    rows: Vec<ProgramRow>,
}

fn resolve_program_cell(cell: &ProgramCell, theme: &Theme) -> ResolvedCell {
    let mut foreground = resolve(cell.style.foreground, DEFAULT_FG);
    let mut background = resolve(cell.style.background, DEFAULT_BG);
    let terminal_selection_reverses = cell.selection == Some(CellSelection::Terminal)
        && theme.selection_highlight == SceneColor::Default;
    if cell.selection == Some(CellSelection::Terminal)
        && theme.selection_highlight != SceneColor::Default
    {
        background = resolve(theme.selection_highlight, DEFAULT_BG);
    }
    // Item selection is already represented by the compiled style. A cursor
    // and fallback terminal selection add the same reverse-video modifier as
    // base inverse. Ratatui modifiers are presence bits, not XOR toggles, so
    // any combination reverses exactly once.
    if cell.style.inverse || terminal_selection_reverses || cell.cursor {
        std::mem::swap(&mut foreground, &mut background);
    }

    let glyph = if cell.style.hidden {
        ' '
    } else {
        match cell.occupancy {
            CellOccupancy::Glyph('\r' | '\n') | CellOccupancy::WideContinuation => ' ',
            CellOccupancy::Glyph(character) => character,
        }
    };
    let alpha = if cell.style.dim { 150 } else { 255 };
    ResolvedCell {
        glyph,
        foreground: [foreground[0], foreground[1], foreground[2], alpha],
        background: [background[0], background[1], background[2], 255],
        bold: cell.style.bold,
        italic: cell.style.italic,
        underline: cell.style.underline,
        strikethrough: cell.style.strikethrough,
    }
}

fn prepare_cell_program(program: &CellProgram, theme: &Theme) -> PreparedCellProgram {
    let mut topmost = BTreeMap::new();
    for (x, y, cell) in program.cells() {
        topmost.insert((y, x), resolve_program_cell(cell, theme));
    }

    let cells = topmost
        .iter()
        .map(|(&(y, x), &cell)| (x, y, cell))
        .collect::<Vec<_>>();
    let mut rows_by_y: BTreeMap<u16, Vec<(u16, ResolvedCell)>> = BTreeMap::new();
    for (&(y, x), &cell) in &topmost {
        rows_by_y.entry(y).or_default().push((x, cell));
    }
    let rows = rows_by_y
        .into_iter()
        .filter_map(|(y, cells)| {
            let first_x = cells.first()?.0;
            let last_x = cells.last()?.0;
            let by_x = cells.into_iter().collect::<BTreeMap<_, _>>();
            let fallback = ResolvedCell {
                glyph: ' ',
                foreground: [DEFAULT_FG[0], DEFAULT_FG[1], DEFAULT_FG[2], 255],
                background: [DEFAULT_BG[0], DEFAULT_BG[1], DEFAULT_BG[2], 255],
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
            };
            let mut text = String::new();
            let mut runs = Vec::new();
            let mut run_start = 0;
            let mut run_style = None;
            for x in first_x..=last_x {
                let cell = by_x.get(&x).copied().unwrap_or(fallback);
                let style = GlyphStyle::from(cell);
                if run_style != Some(style) {
                    if let Some(previous) = run_style.replace(style) {
                        runs.push((run_start..text.len(), previous));
                    }
                    run_start = text.len();
                }
                text.push(cell.glyph);
            }
            if let Some(style) = run_style {
                runs.push((run_start..text.len(), style));
            }
            Some(ProgramRow {
                y,
                x: first_x,
                text,
                runs,
            })
        })
        .collect();

    PreparedCellProgram { cells, rows }
}

fn glyph_attrs(style: GlyphStyle) -> Attrs<'static> {
    let mut attrs = Attrs::new().family(Family::Monospace).color(GColor::rgba(
        style.foreground[0],
        style.foreground[1],
        style.foreground[2],
        style.foreground[3],
    ));
    if style.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if style.italic {
        attrs = attrs.style(FontStyle::Italic);
    }
    if style.underline {
        attrs = attrs.underline(UnderlineStyle::Single);
    }
    if style.strikethrough {
        attrs = attrs.strikethrough();
    }
    attrs
}

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
    row_buffers: RowBufferPool,

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
            row_buffers: RowBufferPool::new(),
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
        self.row_buffers.set_metrics(metrics);
        self.cell_w = measure_cell_width(&mut self.font_system, metrics);
        self.cell_h = line_height;
    }

    /// Render one frame from a `WorkspaceScene`. Consumes only scene types: the
    /// visible cells, styles, cursor/selection marks, and status come from the
    /// scene, never from a grid or parser. Returns the instant right after
    /// `present()` for input-to-present measurement. `Ok(None)` means the
    /// swapchain frame could not be acquired; invalid geometry or resource
    /// limits return a visible adapter error instead of being skipped or
    /// panicking.
    pub fn render(
        &mut self,
        scene: &WorkspaceScene,
        theme: &Theme,
    ) -> Result<Option<Instant>, SceneCompileError> {
        let prepared = prepare_scene(scene, theme)?;
        let program = prepare_cell_program(prepared.cell_program(), theme);
        let metrics = Metrics::new(self.font_size, self.cell_h);
        self.row_buffers
            .ensure_len(program.rows.len(), &mut self.font_system, metrics);

        // The cell compiler has already applied pane order, opacity, chrome,
        // content, overlay, selection, and cursor semantics. The GPU adapter
        // only translates final topmost cells into solid backgrounds and
        // glyphon rows.
        let mut quads = Vec::with_capacity(program.cells.len().saturating_mul(8));
        for (x, y, cell) in &program.cells {
            push_quad(
                &mut quads,
                f32::from(*x) * self.cell_w,
                f32::from(*y) * self.cell_h,
                self.cell_w,
                self.cell_h,
                cell.background,
            );
        }

        for (buffer, row) in self.row_buffers.rows.iter_mut().zip(program.rows.iter()) {
            let width = row.text.chars().count() as f32 * self.cell_w;
            buffer.set_wrap(Wrap::None);
            buffer.set_size(Some(width.max(1.0)), Some(self.cell_h));
            let spans = row
                .runs
                .iter()
                .map(|(range, style)| (&row.text[range.clone()], glyph_attrs(*style)));
            buffer.set_rich_text(
                spans,
                &Attrs::new().family(Family::Monospace),
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(&mut self.font_system, false);
        }

        if quads.len() > self.inst_capacity_floats {
            self.inst_capacity_floats = quads.len().next_power_of_two();
            self.inst_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad-instances"),
                size: (self.inst_capacity_floats * 4) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue
            .write_buffer(&self.inst_buf, 0, bytes_of_slice(&quads));
        let instance_count = (quads.len() / 8) as u32;

        let resolution = [
            self.config.width as f32,
            self.config.height as f32,
            0.0,
            0.0,
        ];
        self.queue
            .write_buffer(&self.res_buf, 0, bytes_of(&resolution));
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );

        let text_areas = program.rows.iter().enumerate().map(|(index, row)| {
            let left = f32::from(row.x) * self.cell_w;
            let top = f32::from(row.y) * self.cell_h;
            TextArea {
                buffer: &self.row_buffers.rows[index],
                left,
                top,
                scale: 1.0,
                bounds: TextBounds {
                    left: left.floor() as i32,
                    top: top.floor() as i32,
                    right: self.config.width as i32,
                    bottom: (top + self.cell_h).ceil() as i32,
                },
                default_color: GColor::rgb(DEFAULT_FG[0], DEFAULT_FG[1], DEFAULT_FG[2]),
                custom_glyphs: &[],
            }
        });
        if self
            .text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .is_err()
        {
            return Ok(None);
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            _ => return Ok(None),
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
        Ok(Some(Instant::now()))
    }
}

/// Map a scene color onto RGB, using the given default for
/// `SceneColor::Default`, the standard xterm palette for ANSI/indexed colors,
/// and a passthrough for direct RGB.
fn resolve(color: SceneColor, default: [u8; 3]) -> [u8; 3] {
    match color {
        SceneColor::Default => default,
        SceneColor::Rgb(r, g, b) => [r, g, b],
        SceneColor::Ansi(i) => palette(i),
        SceneColor::Indexed(i) => palette(i),
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
    unsafe { std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>()) }
}

fn bytes_of_slice<T: Copy>(slice: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mandatum_scene::{
        EmptyContent, HeaderScene, OverlayScene, PaletteOverlay, PaneContent, PaneId, PaneScene,
        PaneSceneKind, SceneCell, SceneRect, SceneSize, StatusScene, TerminalSurface,
    };

    #[test]
    fn row_buffer_pool_grows_to_the_program_and_retains_high_water_capacity() {
        let mut font_system = FontSystem::new();
        let metrics = Metrics::new(15.0, 20.0);
        let mut pool = RowBufferPool::new();

        pool.ensure_len(3, &mut font_system, metrics);
        assert_eq!(pool.len(), 3);

        pool.ensure_len(5, &mut font_system, metrics);
        assert_eq!(pool.len(), 5);

        pool.ensure_len(2, &mut font_system, metrics);
        assert_eq!(pool.len(), 5);
    }

    #[test]
    fn generic_program_cell_mapping_honors_color_modifiers_and_terminal_selection() {
        let theme = Theme {
            selection_highlight: SceneColor::Rgb(90, 91, 92),
            ..Theme::default()
        };
        let cell = ProgramCell {
            occupancy: CellOccupancy::Glyph('X'),
            style: mandatum_scene::SceneCellStyle {
                foreground: SceneColor::Rgb(1, 2, 3),
                background: SceneColor::Rgb(4, 5, 6),
                bold: true,
                dim: true,
                italic: true,
                underline: true,
                inverse: false,
                hidden: false,
                strikethrough: true,
            },
            selection: Some(CellSelection::Terminal),
            cursor: false,
        };

        let resolved = resolve_program_cell(&cell, &theme);
        assert_eq!(resolved.glyph, 'X');
        assert_eq!(resolved.foreground, [1, 2, 3, 150]);
        assert_eq!(resolved.background, [90, 91, 92, 255]);
        assert!(resolved.bold);
        assert!(resolved.italic);
        assert!(resolved.underline);
        assert!(resolved.strikethrough);

        let attrs = glyph_attrs(GlyphStyle::from(resolved));
        assert_eq!(attrs.weight, Weight::BOLD);
        assert_eq!(attrs.style, FontStyle::Italic);
        assert_eq!(attrs.text_decoration.underline, UnderlineStyle::Single);
        assert!(attrs.text_decoration.strikethrough);
    }

    #[test]
    fn base_inverse_terminal_selection_fallback_and_cursor_reverse_once_by_presence() {
        let cell = ProgramCell {
            occupancy: CellOccupancy::Glyph('X'),
            style: mandatum_scene::SceneCellStyle {
                foreground: SceneColor::Rgb(1, 2, 3),
                background: SceneColor::Rgb(4, 5, 6),
                inverse: true,
                ..mandatum_scene::SceneCellStyle::default()
            },
            selection: Some(CellSelection::Terminal),
            cursor: true,
        };

        let resolved = resolve_program_cell(&cell, &Theme::default());

        // Base inverse, fallback terminal selection, and the cursor all add
        // the same reverse-video bit; their combination reverses once.
        assert_eq!(resolved.foreground, [4, 5, 6, 255]);
        assert_eq!(resolved.background, [1, 2, 3, 255]);
    }

    #[test]
    fn item_selection_uses_compiled_style_and_hidden_or_continuation_cells_are_blank() {
        let item = ProgramCell {
            occupancy: CellOccupancy::Glyph('I'),
            style: mandatum_scene::SceneCellStyle {
                foreground: SceneColor::Rgb(1, 2, 3),
                background: SceneColor::Rgb(4, 5, 6),
                inverse: true,
                ..mandatum_scene::SceneCellStyle::default()
            },
            selection: Some(CellSelection::Item),
            cursor: false,
        };
        let hidden = ProgramCell {
            occupancy: CellOccupancy::Glyph('H'),
            style: mandatum_scene::SceneCellStyle {
                hidden: true,
                ..mandatum_scene::SceneCellStyle::default()
            },
            selection: None,
            cursor: false,
        };
        let continuation = ProgramCell {
            occupancy: CellOccupancy::WideContinuation,
            style: mandatum_scene::SceneCellStyle::default(),
            selection: None,
            cursor: false,
        };

        let resolved_item = resolve_program_cell(&item, &Theme::default());
        assert_eq!(resolved_item.foreground, [4, 5, 6, 255]);
        assert_eq!(resolved_item.background, [1, 2, 3, 255]);
        assert_eq!(resolve_program_cell(&hidden, &Theme::default()).glyph, ' ');
        assert_eq!(
            resolve_program_cell(&continuation, &Theme::default()).glyph,
            ' '
        );
    }

    #[test]
    fn generic_program_translation_keeps_only_the_topmost_opaque_cell() {
        let surface = TerminalSurface {
            rows: vec![
                vec![
                    SceneCell {
                        character: 'X',
                        style: mandatum_scene::SceneCellStyle::default(),
                    };
                    20
                ];
                10
            ],
            ..TerminalSurface::default()
        };
        let tiled = pane(PaneSceneKind::Terminal, PaneContent::Terminal(surface));
        let mut floating = pane(
            PaneSceneKind::StatusLog,
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            }),
        );
        floating.id = PaneId::new("pane-2");
        floating.area = SceneRect::new(1, 2, 12, 8);
        floating.focused = false;
        floating.floating = true;
        let scene = scene(vec![tiled, floating]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();

        let translated = prepare_cell_program(prepared.cell_program(), &theme);
        let final_cells = translated
            .cells
            .iter()
            .filter(|(x, y, _)| (*x, *y) == (3, 6))
            .collect::<Vec<_>>();

        assert_eq!(final_cells.len(), 1);
        assert_eq!(final_cells[0].2.glyph, ' ');
        assert_eq!(
            final_cells[0].2.background,
            [DEFAULT_BG[0], DEFAULT_BG[1], DEFAULT_BG[2], 255]
        );
    }

    fn terminal_content() -> PaneContent {
        PaneContent::Terminal(TerminalSurface {
            rows: vec![vec![SceneCell::default(); 2]],
            ..TerminalSurface::default()
        })
    }

    fn pane(kind: PaneSceneKind, content: PaneContent) -> PaneScene {
        PaneScene {
            id: PaneId::new("pane-1"),
            title: kind.label().to_owned(),
            kind,
            area: SceneRect::new(0, 1, 80, 22),
            focused: true,
            floating: false,
            stacked: false,
            zoomed: false,
            content,
        }
    }

    fn scene(panes: Vec<PaneScene>) -> WorkspaceScene {
        let focused_pane = panes
            .first()
            .map(|pane| pane.id.clone())
            .unwrap_or_else(|| PaneId::new("none"));
        WorkspaceScene {
            size: SceneSize::new(80, 24),
            header: HeaderScene {
                area: SceneRect::new(0, 0, 80, 1),
                workspace_name: "test".to_owned(),
                session_name: "session".to_owned(),
                pane_count: panes.len(),
                focused_pane: focused_pane.clone(),
                zoomed: false,
                connector_label: "none".to_owned(),
                text: "test header".to_owned(),
                attention: Vec::new(),
            },
            panes,
            overlay: None,
            status: StatusScene {
                area: SceneRect::new(0, 23, 80, 1),
                text: "test status".to_owned(),
            },
            focused_pane,
            hit_targets: Vec::new(),
            copy_mode: false,
        }
    }

    #[test]
    fn current_single_terminal_scene_is_supported_headlessly() {
        let scene = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();

        assert_eq!(prepared.cell_program().size(), scene.size);
        let inner = layout::pane_inner_rect(scene.panes[0].area);
        assert!(prepared.cell_program().cell_at(inner.x, inner.y).is_some());
        assert_eq!(scene.status.text, "test status");
    }

    #[test]
    fn scene_compiler_accepts_the_layout_capability_family() {
        let empty = || {
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            })
        };

        let mut horizontal_first = pane(PaneSceneKind::Terminal, terminal_content());
        horizontal_first.area = SceneRect::new(0, 1, 40, 22);
        horizontal_first.focused = false;
        let mut horizontal_second = pane(PaneSceneKind::StatusLog, empty());
        horizontal_second.id = PaneId::new("pane-2");
        horizontal_second.area = SceneRect::new(40, 1, 40, 22);
        let mut horizontal = scene(vec![horizontal_first, horizontal_second]);
        horizontal.overlay = Some(OverlayScene::Palette(PaletteOverlay {
            area: SceneRect::new(13, 5, 56, 14),
            query: String::new(),
            items: Vec::new(),
            selected: None,
            footer: String::new(),
        }));

        let mut vertical_first = pane(PaneSceneKind::Terminal, empty());
        vertical_first.area = SceneRect::new(0, 1, 80, 11);
        vertical_first.focused = false;
        let mut vertical_second = pane(PaneSceneKind::Terminal, empty());
        vertical_second.id = PaneId::new("pane-2");
        vertical_second.area = SceneRect::new(0, 12, 80, 11);
        vertical_second.stacked = true;
        let vertical = scene(vec![vertical_first, vertical_second]);

        let mut tiled = pane(PaneSceneKind::Terminal, empty());
        tiled.focused = false;
        let mut first_float = pane(PaneSceneKind::Task, empty());
        first_float.id = PaneId::new("pane-2");
        first_float.area = SceneRect::new(8, 5, 50, 15);
        first_float.focused = false;
        first_float.floating = true;
        let mut second_float = pane(PaneSceneKind::Agent, terminal_content());
        second_float.id = PaneId::new("pane-3");
        second_float.area = SceneRect::new(20, 8, 55, 13);
        second_float.floating = true;
        second_float.zoomed = true;
        let multiple_floats = scene(vec![tiled, first_float, second_float]);

        let theme = Theme::default();
        for (label, candidate, expected_panes) in [
            ("horizontal mixed content plus overlay", horizontal, 2),
            ("vertical scene-owned flags", vertical, 2),
            ("ordered overlapping floats", multiple_floats, 3),
        ] {
            let prepared = prepare_scene(&candidate, &theme)
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            assert_eq!(candidate.panes.len(), expected_panes, "{label}");
            assert_eq!(prepared.cell_program().size(), candidate.size, "{label}");
            for pane in &candidate.panes {
                assert!(
                    prepared
                        .cell_program()
                        .cell_at(pane.area.x, pane.area.y)
                        .is_some(),
                    "{label}: pane {} did not reach the cell program",
                    pane.id
                );
            }
        }
    }

    #[test]
    fn scene_compiler_rejects_only_structural_resource_hazards() {
        let mut no_interior = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        no_interior.panes[0].area.width = 2;

        let mut outside_workspace = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        outside_workspace.panes[0].area = SceneRect::new(79, 1, 3, 3);

        let mut right_overflow = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        right_overflow.size = SceneSize::new(u16::MAX, 5);
        right_overflow.panes[0].area = SceneRect::new(u16::MAX - 1, 1, 3, 3);

        let mut bottom_overflow = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        bottom_overflow.size = SceneSize::new(3, u16::MAX);
        bottom_overflow.panes[0].area = SceneRect::new(0, u16::MAX - 1, 3, 3);

        let too_many = scene(
            (0..=MAX_GPU_PANES)
                .map(|index| {
                    let mut pane = pane(PaneSceneKind::Terminal, terminal_content());
                    pane.id = PaneId::new(format!("pane-{index}"));
                    pane.area = SceneRect::new(0, 1, 3, 3);
                    pane
                })
                .collect(),
        );

        for (label, candidate, expected) in [
            (
                "bordered interior",
                no_interior,
                SceneCompileError::InvalidGeometry("pane has no usable bordered interior"),
            ),
            (
                "workspace containment",
                outside_workspace,
                SceneCompileError::InvalidGeometry("pane lies outside the workspace"),
            ),
            (
                "checked right edge",
                right_overflow,
                SceneCompileError::InvalidGeometry("pane geometry overflows"),
            ),
            (
                "checked bottom edge",
                bottom_overflow,
                SceneCompileError::InvalidGeometry("pane geometry overflows"),
            ),
            (
                "aggregate pane limit",
                too_many,
                SceneCompileError::ResourceLimit {
                    resource: "panes",
                    actual: MAX_GPU_PANES + 1,
                    maximum: MAX_GPU_PANES,
                },
            ),
        ] {
            assert_eq!(
                prepare_scene(&candidate, &Theme::default()).unwrap_err(),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn scene_compiler_rejects_aggregate_gpu_resource_hazards_before_compiling() {
        let mut oversized_frame = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        oversized_frame.size = SceneSize::new(513, 512);

        let mut too_many_rows = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        too_many_rows.size = SceneSize::new(3, (MAX_GPU_ROW_BUFFERS + 1) as u16);
        too_many_rows.panes[0].area = SceneRect::new(0, 1, 3, 3);

        let mut instruction_heavy = scene(
            (0..5)
                .map(|index| {
                    let mut pane = pane(PaneSceneKind::Terminal, terminal_content());
                    pane.id = PaneId::new(format!("pane-{index}"));
                    pane.area = SceneRect::new(0, 1, 500, 498);
                    pane
                })
                .collect(),
        );
        instruction_heavy.size = SceneSize::new(500, 500);

        assert_eq!(
            prepare_scene(&oversized_frame, &Theme::default()).unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "frame cells",
                actual: 513 * 512,
                maximum: MAX_GPU_FRAME_CELLS,
            }
        );
        assert_eq!(
            prepare_scene(&too_many_rows, &Theme::default()).unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "row buffers",
                actual: MAX_GPU_ROW_BUFFERS + 1,
                maximum: MAX_GPU_ROW_BUFFERS,
            }
        );
        assert!(matches!(
            prepare_scene(&instruction_heavy, &Theme::default()).unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "cell instructions",
                actual,
                maximum: MAX_GPU_CELL_INSTRUCTIONS,
            } if actual > MAX_GPU_CELL_INSTRUCTIONS
        ));
    }
}
