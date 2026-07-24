// GPU frontend: wgpu surface + an instanced solid-quad pipeline for cell
// backgrounds/selection/cursor/status, layered under GPU-rasterized glyphs
// rendered by glyphon. All rendering is per-frame from WorkspaceScene.

use std::collections::{BTreeMap, BTreeSet};
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
    ArtifactState, CellOccupancy, CellProgram, CellSelection, OverlayScene, PaneContent,
    ProgramCell, RasterSurface, SceneColor, SceneRect, Theme, WorkspaceScene, compile_cell_program,
    layout,
};
use winit::window::Window;

const DEFAULT_FG: [u8; 3] = [220, 220, 224];
const DEFAULT_BG: [u8; 3] = [18, 18, 22];
const BASE_FONT_PT: f32 = 15.0;
const MAX_GPU_PANES: usize = 256;
const MAX_GPU_FRAME_CELLS: usize = 262_144;
const MAX_GPU_CELL_INSTRUCTIONS: usize = 4_000_000;
const MAX_GPU_ROWS: usize = 4_096;
const MAX_GPU_TEXT_BUFFERS: usize = 32_768;
const MAX_GPU_RASTER_DIMENSION: usize = 4_096;
const MAX_GPU_RASTER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct NativeTextSettings {
    family: String,
    font_size: f32,
}

impl Default for NativeTextSettings {
    fn default() -> Self {
        Self {
            family: "monospace".to_owned(),
            font_size: BASE_FONT_PT,
        }
    }
}

impl NativeTextSettings {
    pub fn new(family: impl Into<String>, font_size: f32) -> Result<Self, String> {
        let family = family.into();
        let family = family.trim();
        if family.is_empty() || family.len() > 128 || family.chars().any(char::is_control) {
            return Err("font family must be 1..=128 visible characters".to_owned());
        }
        if !font_size.is_finite() || !(6.0..=72.0).contains(&font_size) {
            return Err("font size must be finite and between 6 and 72 points".to_owned());
        }
        Ok(Self {
            family: family.to_owned(),
            font_size,
        })
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SceneCompileError {
    NoVisiblePane,
    ResourceLimit {
        resource: &'static str,
        actual: usize,
        maximum: usize,
    },
    InvalidGeometry(&'static str),
    InvalidRasterSurface {
        layer: u16,
        reason: &'static str,
    },
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
            Self::InvalidRasterSurface { layer, reason } => {
                write!(f, "invalid raster surface at layer {layer}: {reason}")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedArtifact {
    layer: u16,
    body: SceneRect,
    visible_clips: Vec<SceneRect>,
    width: u32,
    height: u32,
    revision: u64,
    rgba8: Arc<[u8]>,
}

impl PreparedArtifact {
    pub fn layer(&self) -> u16 {
        self.layer
    }

    pub fn body(&self) -> SceneRect {
        self.body
    }

    pub fn visible_clips(&self) -> &[SceneRect] {
        &self.visible_clips
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn rgba8(&self) -> &[u8] {
        &self.rgba8
    }
}

#[derive(Debug)]
pub struct PreparedScene {
    cell_program: CellProgram,
    artifacts: Vec<PreparedArtifact>,
}

impl PreparedScene {
    pub fn cell_program(&self) -> &CellProgram {
        &self.cell_program
    }

    pub fn artifacts(&self) -> &[PreparedArtifact] {
        &self.artifacts
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
    let artifacts = prepare_artifacts(scene, &cell_program);
    Ok(PreparedScene {
        cell_program,
        artifacts,
    })
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
    enforce_resource_limit("frame rows", usize::from(scene.size.height), MAX_GPU_ROWS)?;
    validate_raster_resources(scene)?;

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

fn validate_raster_resources(scene: &WorkspaceScene) -> Result<(), SceneCompileError> {
    let mut aggregate_bytes = 0usize;
    for (draw_index, pane) in scene.panes.iter().enumerate() {
        let PaneContent::Artifact(artifact) = &pane.content else {
            continue;
        };
        let ArtifactState::Ready(surface) = &artifact.state else {
            continue;
        };
        let layer = u16::try_from(draw_index).map_err(|_| SceneCompileError::ResourceLimit {
            resource: "panes",
            actual: scene.panes.len(),
            maximum: MAX_GPU_PANES,
        })?;
        let surface_bytes = validate_raster_surface(layer, surface)?;
        aggregate_bytes =
            aggregate_bytes
                .checked_add(surface_bytes)
                .ok_or(SceneCompileError::ResourceLimit {
                    resource: "artifact RGBA bytes",
                    actual: usize::MAX,
                    maximum: MAX_GPU_RASTER_BYTES,
                })?;
        enforce_resource_limit("artifact RGBA bytes", aggregate_bytes, MAX_GPU_RASTER_BYTES)?;
    }
    Ok(())
}

fn validate_raster_surface(
    layer: u16,
    surface: &RasterSurface,
) -> Result<usize, SceneCompileError> {
    if surface.width == 0 || surface.height == 0 {
        return Err(SceneCompileError::InvalidRasterSurface {
            layer,
            reason: "dimensions must be nonzero",
        });
    }
    enforce_resource_limit(
        "artifact width",
        surface.width as usize,
        MAX_GPU_RASTER_DIMENSION,
    )?;
    enforce_resource_limit(
        "artifact height",
        surface.height as usize,
        MAX_GPU_RASTER_DIMENSION,
    )?;
    let expected = usize::try_from(surface.width)
        .ok()
        .and_then(|width| {
            usize::try_from(surface.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(SceneCompileError::InvalidRasterSurface {
            layer,
            reason: "decoded byte length overflows",
        })?;
    if surface.rgba8.len() != expected {
        return Err(SceneCompileError::InvalidRasterSurface {
            layer,
            reason: "decoded byte length does not match dimensions",
        });
    }
    Ok(expected)
}

fn prepare_artifacts(scene: &WorkspaceScene, program: &CellProgram) -> Vec<PreparedArtifact> {
    scene
        .panes
        .iter()
        .enumerate()
        .filter_map(|(draw_index, pane)| {
            let layer = u16::try_from(draw_index).ok()?;
            let PaneContent::Artifact(artifact) = &pane.content else {
                return None;
            };
            let ArtifactState::Ready(surface) = &artifact.state else {
                return None;
            };
            let visible_clips = raster_clip_runs(program, layer);
            if visible_clips.is_empty() {
                return None;
            }
            let inner = layout::pane_inner_rect(pane.area);
            let detail_rows = u16::try_from(pane.detail_lines().len()).unwrap_or(u16::MAX);
            let body_y = inner.y.saturating_add(detail_rows).min(inner.bottom());
            let body = SceneRect::new(
                inner.x,
                body_y,
                inner.width,
                inner.bottom().saturating_sub(body_y),
            );
            Some(PreparedArtifact {
                layer,
                body,
                visible_clips,
                width: surface.width,
                height: surface.height,
                revision: surface.revision,
                rgba8: surface.rgba8.clone(),
            })
        })
        .collect()
}

fn raster_clip_runs(program: &CellProgram, layer: u16) -> Vec<SceneRect> {
    let mut clips = Vec::new();
    let mut current: Option<SceneRect> = None;
    for (x, y, cell) in program.cells() {
        if cell.raster_layer != Some(layer) {
            continue;
        }
        match current {
            Some(mut run) if run.y == y && run.right() == x => {
                run.width = run.width.saturating_add(1);
                current = Some(run);
            }
            Some(run) => {
                clips.push(run);
                current = Some(SceneRect::new(x, y, 1, 1));
            }
            None => current = Some(SceneRect::new(x, y, 1, 1)),
        }
    }
    if let Some(run) = current {
        clips.push(run);
    }
    clips
}

fn validate_compiled_program(program: &CellProgram) -> Result<(), SceneCompileError> {
    let instructions = program.cells().count();
    enforce_resource_limit("cell instructions", instructions, MAX_GPU_CELL_INSTRUCTIONS)?;

    let text_buffers = program
        .cells()
        .filter(|(_, _, cell)| {
            matches!(
                &cell.occupancy,
                CellOccupancy::Grapheme(grapheme)
                    if (grapheme != " " || cell.style.underline || cell.style.strikethrough)
                        && grapheme != "\r"
                        && grapheme != "\n"
            )
        })
        .count();
    enforce_resource_limit("text buffers", text_buffers, MAX_GPU_TEXT_BUFFERS)
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedCell {
    grapheme: String,
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

impl From<&ResolvedCell> for GlyphStyle {
    fn from(cell: &ResolvedCell) -> Self {
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
    width: u8,
    text: String,
    runs: Vec<(std::ops::Range<usize>, GlyphStyle)>,
}

#[derive(Debug)]
struct PreparedCellProgram {
    cells: Vec<(u16, u16, ResolvedCell)>,
    rows: Vec<ProgramRow>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PixelRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn contain_fit(source_width: u32, source_height: u32, target: PixelRect) -> Option<PixelRect> {
    if source_width == 0
        || source_height == 0
        || !target.x.is_finite()
        || !target.y.is_finite()
        || !target.width.is_finite()
        || !target.height.is_finite()
        || target.width <= 0.0
        || target.height <= 0.0
    {
        return None;
    }
    let scale = (f64::from(target.width) / f64::from(source_width))
        .min(f64::from(target.height) / f64::from(source_height));
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let width = (f64::from(source_width) * scale).min(f64::from(target.width)) as f32;
    let height = (f64::from(source_height) * scale).min(f64::from(target.height)) as f32;
    Some(PixelRect {
        x: target.x + (target.width - width) / 2.0,
        y: target.y + (target.height - height) / 2.0,
        width,
        height,
    })
}

#[derive(Debug)]
struct CachedRaster {
    revision: u64,
    width: u32,
    height: u32,
    rgba8: Arc<[u8]>,
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RasterIdentity {
    revision: u64,
    width: u32,
    height: u32,
    rgba_ptr: usize,
}

impl RasterIdentity {
    fn prepared(artifact: &PreparedArtifact) -> Self {
        Self {
            revision: artifact.revision,
            width: artifact.width,
            height: artifact.height,
            rgba_ptr: Arc::as_ptr(&artifact.rgba8) as *const u8 as usize,
        }
    }

    fn cached(raster: &CachedRaster) -> Self {
        Self {
            revision: raster.revision,
            width: raster.width,
            height: raster.height,
            rgba_ptr: Arc::as_ptr(&raster.rgba8) as *const u8 as usize,
        }
    }
}

fn raster_replacement_layers(
    cached: impl IntoIterator<Item = (u16, RasterIdentity)>,
    artifacts: &[PreparedArtifact],
) -> BTreeSet<u16> {
    let cached = cached.into_iter().collect::<BTreeMap<_, _>>();
    artifacts
        .iter()
        .filter_map(|artifact| {
            let identity = RasterIdentity::prepared(artifact);
            (cached.get(&artifact.layer) != Some(&identity)).then_some(artifact.layer)
        })
        .collect()
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

    let grapheme = if cell.style.hidden {
        " ".to_owned()
    } else {
        match &cell.occupancy {
            CellOccupancy::Grapheme(grapheme) if grapheme == "\r" || grapheme == "\n" => {
                " ".to_owned()
            }
            CellOccupancy::WideContinuation => String::new(),
            CellOccupancy::Grapheme(grapheme) => grapheme.clone(),
        }
    };
    let alpha = if cell.style.dim { 150 } else { 255 };
    ResolvedCell {
        grapheme,
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
        .map(|(&(y, x), cell)| (x, y, cell.clone()))
        .collect::<Vec<_>>();
    let rows = topmost
        .iter()
        .filter_map(|(&(y, x), cell)| {
            if cell.grapheme.is_empty()
                || (cell.grapheme == " " && !cell.underline && !cell.strikethrough)
            {
                return None;
            }
            let width = if topmost
                .get(&(y, x.saturating_add(1)))
                .is_some_and(|next| next.grapheme.is_empty())
            {
                2
            } else {
                1
            };
            let style = GlyphStyle::from(cell);
            Some(ProgramRow {
                y,
                x,
                width,
                text: cell.grapheme.clone(),
                runs: vec![(0..cell.grapheme.len(), style)],
            })
        })
        .collect();

    PreparedCellProgram { cells, rows }
}

fn glyph_attrs<'a>(style: GlyphStyle, family: &'a str) -> Attrs<'a> {
    let mut attrs = Attrs::new().family(font_family(family)).color(GColor::rgba(
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

    // Ready artifact surface pipeline and revision-aware texture cache.
    raster_pipeline: wgpu::RenderPipeline,
    raster_bind_layout: wgpu::BindGroupLayout,
    raster_sampler: wgpu::Sampler,
    raster_inst_buf: wgpu::Buffer,
    raster_inst_capacity_floats: usize,
    raster_cache: BTreeMap<u16, CachedRaster>,

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
    base_font_size: f32,
    font_family: String,
    font_size: f32,
    cell_w: f32,
    cell_h: f32,
}

impl GpuText {
    pub async fn new(
        window: Arc<Window>,
        text_settings: NativeTextSettings,
    ) -> Result<Self, String> {
        NativeTextSettings::new(text_settings.family.clone(), text_settings.font_size)?;
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        validate_scale(scale)?;

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

        let raster_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("artifact-raster-shader"),
            source: wgpu::ShaderSource::Wgsl(RASTER_WGSL.into()),
        });
        let raster_bind_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("artifact-raster-bind-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let raster_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("artifact-raster-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let raster_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("artifact-raster-pipeline-layout"),
                bind_group_layouts: &[Some(&bind_layout), Some(&raster_bind_layout)],
                immediate_size: 0,
            });
        const RASTER_INST_ATTRS: [wgpu::VertexAttribute; 1] =
            wgpu::vertex_attr_array![1 => Float32x4];
        let raster_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("artifact-raster-pipeline"),
            layout: Some(&raster_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &raster_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &UNIT_ATTRS,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 16,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &RASTER_INST_ATTRS,
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &raster_shader,
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
        let raster_inst_capacity_floats = 4 * MAX_GPU_PANES;
        let raster_inst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("artifact-raster-instances"),
            size: (raster_inst_capacity_floats * 4) as u64,
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

        let font_size = (text_settings.font_size * scale).round();
        let line_height = (font_size * 1.3).round();
        let metrics = Metrics::new(font_size, line_height);
        let cell_w = measure_cell_width(&mut font_system, metrics, &text_settings.family);
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
            raster_pipeline,
            raster_bind_layout,
            raster_sampler,
            raster_inst_buf,
            raster_inst_capacity_floats,
            raster_cache: BTreeMap::new(),
            font_system,
            swash_cache,
            cache,
            viewport,
            atlas,
            text_renderer,
            row_buffers: RowBufferPool::new(),
            scale,
            base_font_size: text_settings.font_size,
            font_family: text_settings.family,
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

    pub fn set_scale(&mut self, scale: f32) -> Result<(), String> {
        validate_scale(scale)?;
        if (scale - self.scale).abs() < f32::EPSILON {
            return Ok(());
        }
        self.scale = scale;
        self.font_size = (self.base_font_size * scale).round();
        let line_height = (self.font_size * 1.3).round();
        let metrics = Metrics::new(self.font_size, line_height);
        self.row_buffers.set_metrics(metrics);
        self.cell_w = measure_cell_width(&mut self.font_system, metrics, &self.font_family);
        self.cell_h = line_height;
        Ok(())
    }

    fn sync_raster_cache(&mut self, artifacts: &[PreparedArtifact]) {
        let live_layers = artifacts
            .iter()
            .map(PreparedArtifact::layer)
            .collect::<BTreeSet<_>>();
        self.raster_cache
            .retain(|layer, _| live_layers.contains(layer));
        let replacement_layers = raster_replacement_layers(
            self.raster_cache
                .iter()
                .map(|(&layer, cached)| (layer, RasterIdentity::cached(cached))),
            artifacts,
        );
        // Evict every stale live texture before allocating any replacement.
        // This keeps reload high-water usage under the same admitted aggregate
        // ceiling even when bytes are redistributed between artifact layers.
        for layer in &replacement_layers {
            self.raster_cache.remove(layer);
        }

        for artifact in artifacts {
            if self.raster_cache.contains_key(&artifact.layer) {
                continue;
            }

            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("artifact-rgba8-srgb"),
                size: wgpu::Extent3d {
                    width: artifact.width,
                    height: artifact.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &artifact.rgba8,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(artifact.width * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: artifact.width,
                    height: artifact.height,
                    depth_or_array_layers: 1,
                },
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("artifact-raster-bind-group"),
                layout: &self.raster_bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.raster_sampler),
                    },
                ],
            });
            self.raster_cache.insert(
                artifact.layer,
                CachedRaster {
                    revision: artifact.revision,
                    width: artifact.width,
                    height: artifact.height,
                    rgba8: artifact.rgba8.clone(),
                    _texture: texture,
                    bind_group,
                },
            );
        }
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
        self.sync_raster_cache(prepared.artifacts());
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
            let width = f32::from(row.width) * self.cell_w;
            buffer.set_wrap(Wrap::None);
            buffer.set_size(Some(width.max(1.0)), Some(self.cell_h));
            let spans = row.runs.iter().map(|(range, style)| {
                (
                    &row.text[range.clone()],
                    glyph_attrs(*style, &self.font_family),
                )
            });
            buffer.set_rich_text(
                spans,
                &Attrs::new().family(font_family(&self.font_family)),
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

        let raster_rects = prepared
            .artifacts()
            .iter()
            .enumerate()
            .filter_map(|artifact| {
                let (index, artifact) = artifact;
                contain_fit(
                    artifact.width,
                    artifact.height,
                    PixelRect {
                        x: f32::from(artifact.body.x) * self.cell_w,
                        y: f32::from(artifact.body.y) * self.cell_h,
                        width: f32::from(artifact.body.width) * self.cell_w,
                        height: f32::from(artifact.body.height) * self.cell_h,
                    },
                )
                .map(|rect| (index, rect))
            })
            .collect::<Vec<_>>();
        let mut raster_instances = Vec::with_capacity(raster_rects.len().saturating_mul(4));
        for (_, rect) in &raster_rects {
            raster_instances.extend_from_slice(&[rect.x, rect.y, rect.width, rect.height]);
        }
        if raster_instances.len() > self.raster_inst_capacity_floats {
            self.raster_inst_capacity_floats = raster_instances.len().next_power_of_two();
            self.raster_inst_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("artifact-raster-instances"),
                size: (self.raster_inst_capacity_floats * 4) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !raster_instances.is_empty() {
            self.queue
                .write_buffer(&self.raster_inst_buf, 0, bytes_of_slice(&raster_instances));
        }

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
                bounds: glyph_text_bounds(
                    row.x,
                    row.y,
                    row.width,
                    self.cell_w,
                    self.cell_h,
                    self.config.width,
                    self.config.height,
                ),
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
            if !raster_rects.is_empty() {
                pass.set_pipeline(&self.raster_pipeline);
                pass.set_bind_group(0, &self.res_bind_group, &[]);
                pass.set_vertex_buffer(0, self.unit_buf.slice(..));
                pass.set_vertex_buffer(1, self.raster_inst_buf.slice(..));
                for (instance, (artifact_index, _)) in raster_rects.iter().enumerate() {
                    let artifact = &prepared.artifacts()[*artifact_index];
                    let Some(cached) = self.raster_cache.get(&artifact.layer) else {
                        continue;
                    };
                    pass.set_bind_group(1, &cached.bind_group, &[]);
                    for clip in &artifact.visible_clips {
                        let Some((x, y, width, height)) = cell_clip_scissor(
                            *clip,
                            self.cell_w,
                            self.cell_h,
                            self.config.width,
                            self.config.height,
                        ) else {
                            continue;
                        };
                        pass.set_scissor_rect(x, y, width, height);
                        pass.draw(0..4, instance as u32..instance as u32 + 1);
                    }
                }
                pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
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

fn cell_clip_scissor(
    clip: SceneRect,
    cell_width: f32,
    cell_height: f32,
    surface_width: u32,
    surface_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    if clip.is_empty()
        || !cell_width.is_finite()
        || !cell_height.is_finite()
        || cell_width <= 0.0
        || cell_height <= 0.0
    {
        return None;
    }
    // Choose the integer boundary by pixel center. Reusing the exact same
    // conversion for adjacent clips prevents fractional cell metrics from
    // creating a one-pixel overlap where a lower artifact could bleed through
    // a later opaque pane.
    let left = pixel_boundary(f32::from(clip.x) * cell_width, surface_width);
    let top = pixel_boundary(f32::from(clip.y) * cell_height, surface_height);
    let right = pixel_boundary(f32::from(clip.right()) * cell_width, surface_width);
    let bottom = pixel_boundary(f32::from(clip.bottom()) * cell_height, surface_height);
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

fn glyph_text_bounds(
    x: u16,
    y: u16,
    width: u8,
    cell_width: f32,
    cell_height: f32,
    surface_width: u32,
    surface_height: u32,
) -> TextBounds {
    let left = f32::from(x) * cell_width;
    let top = f32::from(y) * cell_height;
    TextBounds {
        left: pixel_boundary(left, surface_width) as i32,
        top: pixel_boundary(top, surface_height) as i32,
        right: pixel_boundary(
            left + f32::from(width.clamp(1, 2)) * cell_width,
            surface_width,
        ) as i32,
        bottom: pixel_boundary(top + cell_height, surface_height) as i32,
    }
}

fn pixel_boundary(position: f32, maximum: u32) -> u32 {
    (position - 0.5).ceil().clamp(0.0, maximum as f32) as u32
}

/// Measure a monospace advance width by shaping a run of identical glyphs and
/// dividing the laid-out line width by the glyph count.
fn measure_cell_width(font_system: &mut FontSystem, metrics: Metrics, family: &str) -> f32 {
    let mut buffer = Buffer::new(font_system, metrics);
    let mono = Attrs::new().family(font_family(family));
    buffer.set_text("MMMMMMMMMMMMMMMMMMMM", &mono, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    let width = buffer
        .layout_runs()
        .next()
        .map(|run| run.line_w)
        .unwrap_or(metrics.font_size * 0.6);
    (width / 20.0).max(1.0)
}

fn validate_scale(scale: f32) -> Result<(), String> {
    if scale.is_finite() && (0.25..=8.0).contains(&scale) {
        Ok(())
    } else {
        Err("display scale must be finite and between 0.25 and 8.0".to_owned())
    }
}

fn font_family(family: &str) -> Family<'_> {
    if family.eq_ignore_ascii_case("monospace") {
        Family::Monospace
    } else {
        Family::Name(family)
    }
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

const RASTER_WGSL: &str = r#"
struct Res { size: vec4<f32> };
@group(0) @binding(0) var<uniform> res: Res;
@group(1) @binding(0) var raster: texture_2d<f32>;
@group(1) @binding(1) var raster_sampler: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) unit: vec2<f32>,
      @location(1) rect: vec4<f32>) -> VOut {
    let px = rect.xy + unit * rect.zw;
    let ndc = vec2<f32>(px.x / res.size.x * 2.0 - 1.0, 1.0 - px.y / res.size.y * 2.0);
    var out: VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = unit;
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    return textureSample(raster, raster_sampler, in.uv);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use mandatum_scene::{
        ArtifactContent, ArtifactFit, ArtifactState, EmptyContent, HeaderScene, OverlayScene,
        PaletteOverlay, PaneContent, PaneId, PaneScene, PaneSceneKind, RasterSurface, SceneCell,
        SceneRect, SceneSize, StatusScene, TerminalSurface,
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

    fn ready_artifact(width: u32, height: u32, revision: u64) -> PaneContent {
        let bytes = usize::try_from(width)
            .unwrap()
            .checked_mul(usize::try_from(height).unwrap())
            .and_then(|pixels| pixels.checked_mul(4))
            .unwrap();
        PaneContent::Artifact(ArtifactContent {
            source_label: "artifacts/preview.png".to_owned(),
            alt_text: "Preview".to_owned(),
            fit: ArtifactFit::Contain,
            state: ArtifactState::Ready(RasterSurface {
                width,
                height,
                revision,
                rgba8: vec![0x7f; bytes].into(),
            }),
        })
    }

    fn assert_rect_close(actual: PixelRect, expected: PixelRect) {
        for (actual, expected) in [
            (actual.x, expected.x),
            (actual.y, expected.y),
            (actual.width, expected.width),
            (actual.height, expected.height),
        ] {
            assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
        }
    }

    #[test]
    fn contain_fit_centers_landscape_portrait_and_square_surfaces() {
        let target = PixelRect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 100.0,
        };
        assert_rect_close(
            contain_fit(200, 100, target).unwrap(),
            PixelRect {
                x: 10.0,
                y: 45.0,
                width: 100.0,
                height: 50.0,
            },
        );
        assert_rect_close(
            contain_fit(100, 200, target).unwrap(),
            PixelRect {
                x: 35.0,
                y: 20.0,
                width: 50.0,
                height: 100.0,
            },
        );
        assert_rect_close(contain_fit(1, 1, target).unwrap(), target);
        assert!(contain_fit(0, 1, target).is_none());

        let left = cell_clip_scissor(SceneRect::new(0, 0, 10, 1), 8.23, 19.5, 200, 100)
            .expect("left clip should be visible");
        let right = cell_clip_scissor(SceneRect::new(10, 0, 1, 1), 8.23, 19.5, 200, 100)
            .expect("right clip should be visible");
        assert_eq!(left.0 + left.2, right.0, "adjacent clips cannot overlap");
    }

    #[test]
    fn ready_raster_reaches_the_headless_plan_without_copying_pixels() {
        let content = ready_artifact(4, 2, 7);
        let source_ptr = match &content {
            PaneContent::Artifact(ArtifactContent {
                state: ArtifactState::Ready(surface),
                ..
            }) => surface.rgba8.as_ptr(),
            _ => unreachable!(),
        };
        let workspace = scene(vec![pane(PaneSceneKind::Artifact, content)]);

        let prepared = prepare_scene(&workspace, &Theme::default()).unwrap();
        let [artifact] = prepared.artifacts() else {
            panic!("ready artifact did not reach the headless GPU plan");
        };
        assert_eq!(artifact.layer(), 0);
        assert_eq!(artifact.body(), SceneRect::new(1, 5, 78, 17));
        assert_eq!((artifact.width(), artifact.height()), (4, 2));
        assert_eq!(artifact.revision(), 7);
        assert_eq!(artifact.rgba8().as_ptr(), source_ptr);
        assert_eq!(artifact.visible_clips().len(), 17);
        assert!(
            artifact
                .visible_clips()
                .iter()
                .all(|clip| clip.width == 78 && clip.height == 1)
        );
    }

    #[test]
    fn aggregate_raster_bytes_cannot_be_bypassed_by_multiple_valid_surfaces() {
        let mut first = pane(PaneSceneKind::Artifact, ready_artifact(4096, 2048, 1));
        first.id = PaneId::new("artifact-1");
        let mut second = pane(PaneSceneKind::Artifact, ready_artifact(4096, 2048, 1));
        second.id = PaneId::new("artifact-2");
        second.focused = false;
        let exact_limit = scene(vec![first.clone(), second.clone()]);
        prepare_scene(&exact_limit, &Theme::default())
            .expect("the exact aggregate RGBA byte ceiling should be admitted");

        let mut one_more = pane(PaneSceneKind::Artifact, ready_artifact(1, 1, 1));
        one_more.id = PaneId::new("artifact-3");
        one_more.focused = false;
        let over_limit = scene(vec![first, second, one_more]);
        assert_eq!(
            prepare_scene(&over_limit, &Theme::default()).unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "artifact RGBA bytes",
                actual: MAX_GPU_RASTER_BYTES + 4,
                maximum: MAX_GPU_RASTER_BYTES,
            }
        );
    }

    #[test]
    fn cache_reload_plan_evicts_all_stale_layers_before_replacement() {
        let old_first = Arc::<[u8]>::from([1, 2, 3, 4]);
        let old_second = Arc::<[u8]>::from([5, 6, 7, 8]);
        let artifacts = vec![
            PreparedArtifact {
                layer: 0,
                body: SceneRect::new(0, 0, 1, 1),
                visible_clips: vec![SceneRect::new(0, 0, 1, 1)],
                width: 1,
                height: 1,
                revision: 2,
                rgba8: Arc::from([9, 10, 11, 12]),
            },
            PreparedArtifact {
                layer: 1,
                body: SceneRect::new(1, 0, 1, 1),
                visible_clips: vec![SceneRect::new(1, 0, 1, 1)],
                width: 1,
                height: 1,
                revision: 2,
                rgba8: Arc::from([13, 14, 15, 16]),
            },
        ];
        let cached = [
            (
                0,
                RasterIdentity {
                    revision: 1,
                    width: 1,
                    height: 1,
                    rgba_ptr: Arc::as_ptr(&old_first) as *const u8 as usize,
                },
            ),
            (
                1,
                RasterIdentity {
                    revision: 1,
                    width: 1,
                    height: 1,
                    rgba_ptr: Arc::as_ptr(&old_second) as *const u8 as usize,
                },
            ),
        ];

        assert_eq!(
            raster_replacement_layers(cached, &artifacts),
            BTreeSet::from([0, 1]),
            "every stale live texture must be dropped before the first replacement allocates"
        );
    }

    #[test]
    fn malformed_scene_rasters_fail_before_gpu_allocation() {
        let malformed = PaneContent::Artifact(ArtifactContent {
            source_label: "artifacts/bad.png".to_owned(),
            alt_text: "Bad".to_owned(),
            fit: ArtifactFit::Contain,
            state: ArtifactState::Ready(RasterSurface {
                width: 2,
                height: 2,
                revision: 1,
                rgba8: vec![0; 15].into(),
            }),
        });
        let zero = PaneContent::Artifact(ArtifactContent {
            source_label: "artifacts/zero.png".to_owned(),
            alt_text: "Zero".to_owned(),
            fit: ArtifactFit::Contain,
            state: ArtifactState::Ready(RasterSurface {
                width: 0,
                height: 1,
                revision: 1,
                rgba8: Arc::from([]),
            }),
        });
        let too_wide = PaneContent::Artifact(ArtifactContent {
            source_label: "artifacts/wide.png".to_owned(),
            alt_text: "Wide".to_owned(),
            fit: ArtifactFit::Contain,
            state: ArtifactState::Ready(RasterSurface {
                width: (MAX_GPU_RASTER_DIMENSION + 1) as u32,
                height: 1,
                revision: 1,
                rgba8: Arc::from([]),
            }),
        });

        assert_eq!(
            prepare_scene(
                &scene(vec![pane(PaneSceneKind::Artifact, malformed)]),
                &Theme::default()
            )
            .unwrap_err(),
            SceneCompileError::InvalidRasterSurface {
                layer: 0,
                reason: "decoded byte length does not match dimensions",
            }
        );
        assert_eq!(
            prepare_scene(
                &scene(vec![pane(PaneSceneKind::Artifact, zero)]),
                &Theme::default()
            )
            .unwrap_err(),
            SceneCompileError::InvalidRasterSurface {
                layer: 0,
                reason: "dimensions must be nonzero",
            }
        );
        assert_eq!(
            prepare_scene(
                &scene(vec![pane(PaneSceneKind::Artifact, too_wide)]),
                &Theme::default()
            )
            .unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "artifact width",
                actual: MAX_GPU_RASTER_DIMENSION + 1,
                maximum: MAX_GPU_RASTER_DIMENSION,
            }
        );
    }

    #[test]
    fn final_cell_markers_clip_artifacts_behind_later_panes() {
        let artifact = pane(PaneSceneKind::Artifact, ready_artifact(4, 2, 1));
        let mut covering = pane(
            PaneSceneKind::StatusLog,
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            }),
        );
        covering.id = PaneId::new("covering-pane");
        covering.area = SceneRect::new(10, 6, 10, 6);
        covering.focused = false;
        covering.floating = true;
        let workspace = scene(vec![artifact, covering]);

        let prepared = prepare_scene(&workspace, &Theme::default()).unwrap();
        let [artifact] = prepared.artifacts() else {
            panic!("partially visible artifact did not reach the GPU plan");
        };
        assert_eq!(
            prepared
                .cell_program()
                .cell_at(2, 8)
                .and_then(|cell| cell.raster_layer),
            Some(0)
        );
        assert_eq!(
            prepared
                .cell_program()
                .cell_at(12, 8)
                .and_then(|cell| cell.raster_layer),
            None
        );
        assert!(
            artifact
                .visible_clips()
                .iter()
                .all(|clip| !clip.contains(12, 8)),
            "covering pane coordinates leaked into artifact clip runs"
        );
    }

    #[test]
    fn generic_program_cell_mapping_honors_color_modifiers_and_terminal_selection() {
        let theme = Theme {
            selection_highlight: SceneColor::Rgb(90, 91, 92),
            ..Theme::default()
        };
        let cell = ProgramCell {
            occupancy: CellOccupancy::Grapheme('X'.to_string()),
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
            raster_layer: None,
        };

        let resolved = resolve_program_cell(&cell, &theme);
        assert_eq!(resolved.grapheme, "X");
        assert_eq!(resolved.foreground, [1, 2, 3, 150]);
        assert_eq!(resolved.background, [90, 91, 92, 255]);
        assert!(resolved.bold);
        assert!(resolved.italic);
        assert!(resolved.underline);
        assert!(resolved.strikethrough);

        let attrs = glyph_attrs(GlyphStyle::from(&resolved), "monospace");
        assert_eq!(attrs.weight, Weight::BOLD);
        assert_eq!(attrs.style, FontStyle::Italic);
        assert_eq!(attrs.text_decoration.underline, UnderlineStyle::Single);
        assert!(attrs.text_decoration.strikethrough);
    }

    #[test]
    fn base_inverse_terminal_selection_fallback_and_cursor_reverse_once_by_presence() {
        let cell = ProgramCell {
            occupancy: CellOccupancy::Grapheme('X'.to_string()),
            style: mandatum_scene::SceneCellStyle {
                foreground: SceneColor::Rgb(1, 2, 3),
                background: SceneColor::Rgb(4, 5, 6),
                inverse: true,
                ..mandatum_scene::SceneCellStyle::default()
            },
            selection: Some(CellSelection::Terminal),
            cursor: true,
            raster_layer: None,
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
            occupancy: CellOccupancy::Grapheme('I'.to_string()),
            style: mandatum_scene::SceneCellStyle {
                foreground: SceneColor::Rgb(1, 2, 3),
                background: SceneColor::Rgb(4, 5, 6),
                inverse: true,
                ..mandatum_scene::SceneCellStyle::default()
            },
            selection: Some(CellSelection::Item),
            cursor: false,
            raster_layer: None,
        };
        let hidden = ProgramCell {
            occupancy: CellOccupancy::Grapheme('H'.to_string()),
            style: mandatum_scene::SceneCellStyle {
                hidden: true,
                ..mandatum_scene::SceneCellStyle::default()
            },
            selection: None,
            cursor: false,
            raster_layer: None,
        };
        let continuation = ProgramCell {
            occupancy: CellOccupancy::WideContinuation,
            style: mandatum_scene::SceneCellStyle::default(),
            selection: None,
            cursor: false,
            raster_layer: None,
        };

        let resolved_item = resolve_program_cell(&item, &Theme::default());
        assert_eq!(resolved_item.foreground, [4, 5, 6, 255]);
        assert_eq!(resolved_item.background, [1, 2, 3, 255]);
        assert_eq!(
            resolve_program_cell(&hidden, &Theme::default()).grapheme,
            " "
        );
        assert_eq!(
            resolve_program_cell(&continuation, &Theme::default()).grapheme,
            ""
        );
    }

    #[test]
    fn generic_program_translation_keeps_only_the_topmost_opaque_cell() {
        let surface = TerminalSurface {
            rows: vec![
                vec![
                    SceneCell {
                        occupancy: CellOccupancy::Grapheme('X'.to_string()),
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
        assert_eq!(final_cells[0].2.grapheme, " ");
        assert_eq!(
            final_cells[0].2.background,
            [DEFAULT_BG[0], DEFAULT_BG[1], DEFAULT_BG[2], 255]
        );
    }

    #[test]
    fn advanced_text_graphemes_are_anchored_to_declared_cell_spans() {
        let decorated_space = mandatum_scene::SceneCellStyle {
            underline: true,
            ..mandatum_scene::SceneCellStyle::default()
        };
        let surface = TerminalSurface {
            rows: vec![vec![
                SceneCell::grapheme("A", mandatum_scene::SceneCellStyle::default()),
                SceneCell::grapheme("界", mandatum_scene::SceneCellStyle::default()),
                SceneCell::wide_continuation(mandatum_scene::SceneCellStyle::default()),
                SceneCell::grapheme("e\u{301}", mandatum_scene::SceneCellStyle::default()),
                SceneCell::grapheme("👩\u{200d}💻", mandatum_scene::SceneCellStyle::default()),
                SceneCell::wide_continuation(mandatum_scene::SceneCellStyle::default()),
                SceneCell::grapheme(" ", decorated_space),
            ]],
            ..TerminalSurface::default()
        };
        let pane = pane(PaneSceneKind::Terminal, PaneContent::Terminal(surface));
        let scene = scene(vec![pane]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let translated = prepare_cell_program(prepared.cell_program(), &theme);
        let inner = layout::pane_inner_rect(scene.panes[0].area);
        let runs = translated
            .rows
            .iter()
            .filter(|row| row.y == inner.y)
            .map(|row| (row.x, row.width, row.text.as_str()))
            .collect::<Vec<_>>();

        for expected in [
            (inner.x, 1, "A"),
            (inner.x + 1, 2, "界"),
            (inner.x + 3, 1, "e\u{301}"),
            (inner.x + 4, 2, "👩\u{200d}💻"),
            (inner.x + 6, 1, " "),
        ] {
            assert!(
                runs.contains(&expected),
                "missing grid-anchored grapheme {expected:?}; got {runs:?}"
            );
        }
        assert!(
            runs.iter().all(|(_, _, text)| !text.is_empty()),
            "continuations reserve cells but never become shaped glyph runs"
        );
    }

    #[test]
    fn native_text_settings_validate_at_the_renderer_boundary() {
        assert!(NativeTextSettings::new("Menlo", 16.0).is_ok());
        assert!(NativeTextSettings::new("", 16.0).is_err());
        assert!(NativeTextSettings::new("bad\nfamily", 16.0).is_err());
        assert!(NativeTextSettings::new("Menlo", 0.0).is_err());
        assert!(NativeTextSettings::new("Menlo", f32::NAN).is_err());
        assert!(validate_scale(1.5).is_ok());
        assert!(validate_scale(0.0).is_err());
        assert!(validate_scale(f32::INFINITY).is_err());
    }

    #[test]
    fn glyph_raster_bounds_are_clipped_to_the_declared_cell_span() {
        let narrow = glyph_text_bounds(3, 2, 1, 9.5, 18.0, 100, 100);
        assert_eq!((narrow.left, narrow.right), (28, 38));
        let wide = glyph_text_bounds(3, 2, 2, 9.5, 18.0, 100, 100);
        assert_eq!((wide.left, wide.right), (28, 47));
        let adjacent = glyph_text_bounds(5, 2, 1, 9.5, 18.0, 100, 100);
        assert_eq!(wide.right, adjacent.left);
        let next_row = glyph_text_bounds(5, 3, 1, 9.5, 18.0, 100, 100);
        assert_eq!(adjacent.bottom, next_row.top);
        let edge = glyph_text_bounds(10, 2, 2, 9.5, 18.0, 100, 100);
        assert_eq!(edge.right, 100);
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
            text_input: None,
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
    fn dense_normal_terminal_stays_within_the_text_buffer_budget() {
        let rows =
            vec![
                vec![SceneCell::grapheme("X", mandatum_scene::SceneCellStyle::default()); 118];
                36
            ];
        let mut dense = scene(vec![pane(
            PaneSceneKind::Terminal,
            PaneContent::Terminal(TerminalSurface {
                rows,
                ..TerminalSurface::default()
            }),
        )]);
        dense.size = SceneSize::new(120, 40);
        dense.header.area = SceneRect::new(0, 0, 120, 1);
        dense.status.area = SceneRect::new(0, 39, 120, 1);
        dense.panes[0].area = SceneRect::new(0, 1, 120, 38);
        prepare_scene(&dense, &Theme::default())
            .expect("a dense ordinary 120x40 terminal must remain renderable");
    }

    #[test]
    fn pathological_dense_terminal_hits_the_explicit_text_buffer_budget() {
        let rows =
            vec![
                vec![SceneCell::grapheme("X", mandatum_scene::SceneCellStyle::default()); 510];
                66
            ];
        let mut dense = scene(vec![pane(
            PaneSceneKind::Terminal,
            PaneContent::Terminal(TerminalSurface {
                rows,
                ..TerminalSurface::default()
            }),
        )]);
        dense.size = SceneSize::new(512, 70);
        dense.header.area = SceneRect::new(0, 0, 512, 1);
        dense.status.area = SceneRect::new(0, 69, 512, 1);
        dense.panes[0].area = SceneRect::new(0, 1, 512, 68);
        assert!(matches!(
            prepare_scene(&dense, &Theme::default()).unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "text buffers",
                actual,
                maximum: MAX_GPU_TEXT_BUFFERS,
            } if actual > MAX_GPU_TEXT_BUFFERS
        ));
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
        too_many_rows.size = SceneSize::new(3, (MAX_GPU_ROWS + 1) as u16);
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
                resource: "frame rows",
                actual: MAX_GPU_ROWS + 1,
                maximum: MAX_GPU_ROWS,
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
