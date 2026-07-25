// GPU frontend: wgpu surface + an instanced solid-quad pipeline for cell
// backgrounds/selection/cursor/status, layered under GPU-rasterized glyphs
// rendered by glyphon. All rendering is per-frame from WorkspaceScene.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    Style as FontStyle, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
    Weight, Wrap, cosmic_text::UnderlineStyle,
};
// The renderer consumes ONLY the scene contract. It never imports
// mandatum-terminal-vt: the real app host converts its grids before the
// snapshot reaches this crate, so no parser type crosses into paint.
use mandatum_scene::{
    ArtifactState, CellOccupancy, CellProgram, CellSelection, LogicalRect, OverlayScene,
    PaneContent, PaneNodePart, PresentationNodeId, PresentationNodeRole, ProgramCell,
    RasterSurface, SceneColor, SceneRect, TerminalPalette, TextPaintScopeKind, Theme, UiColor,
    TransitionRole, WorkflowNodePart, WorkspaceScene, compile_cell_program, layout,
};
use winit::window::Window;

use crate::row_run::{
    LayoutGlyphFacts, LayoutRunFacts, NativeTextGeometry, ResolvedGlyphStyle, RowRun,
    RowRunAdmission, RowRunBuildError, RowRunBuildIssue, RowRunFallbackAction, admit_layout,
    anchored_fallback_runs, build_row_runs, partition_around_cluster, slice_run, split_at_span,
};
use crate::shaping_cache::{ShapingCache, ShapingCacheContext, ShapingCacheKey};

const BASE_FONT_PT: f32 = 15.0;
const MAX_GPU_PANES: usize = 256;
const MAX_GPU_FRAME_CELLS: usize = 262_144;
const MAX_GPU_CELL_INSTRUCTIONS: usize = 4_000_000;
const MAX_GPU_ROWS: usize = 4_096;
const MAX_GPU_TEXT_BUFFERS: usize = 32_768;
const MAX_GPU_RASTER_DIMENSION: usize = 4_096;
const MAX_GPU_RASTER_BYTES: usize = 64 * 1024 * 1024;
const SHAPING_POLICY_GENERATION: u64 = 1;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuStartupErrorKind {
    NoDisplay,
    NoAdapter,
    DeviceRequest,
    InvalidConfiguration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuStartupError {
    kind: GpuStartupErrorKind,
    message: String,
}

impl GpuStartupError {
    pub fn no_display(message: impl Into<String>) -> Self {
        Self {
            kind: GpuStartupErrorKind::NoDisplay,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> GpuStartupErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for GpuStartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stage = match self.kind {
            GpuStartupErrorKind::NoDisplay => "no display",
            GpuStartupErrorKind::NoAdapter => "no GPU adapter",
            GpuStartupErrorKind::DeviceRequest => "GPU device request failed",
            GpuStartupErrorKind::InvalidConfiguration => "invalid GPU configuration",
        };
        write!(f, "{stage}: {}", self.message)
    }
}

impl std::error::Error for GpuStartupError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuAdapterMetadata {
    pub name: String,
    pub backend: &'static str,
    pub device_type: &'static str,
    pub driver: String,
    pub driver_info: String,
    pub vendor: u32,
    pub device: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuFrameSkip {
    Timeout,
    Occluded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuSurfaceRecovery {
    Outdated,
    Lost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuDeviceLossReason {
    Unknown,
    Destroyed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuFrameTimings {
    pub shaping: Duration,
    pub frame_prepare: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuRenderOutcome {
    Presented {
        at: Instant,
        timings: GpuFrameTimings,
    },
    Skipped {
        reason: GpuFrameSkip,
        timings: GpuFrameTimings,
    },
    SurfaceReconfigured {
        recovery: GpuSurfaceRecovery,
        timings: GpuFrameTimings,
    },
}

#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuFaultInjection {
    SurfaceOutdated,
    SurfaceLost,
    DeviceLost,
    OutOfMemory,
}

#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuFaultInjectionResult {
    SurfaceReconfigured(GpuSurfaceRecovery),
    FaultQueued,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuLifecycleSnapshot {
    pub device_generation: u64,
    pub surface_generation: u64,
    pub surface_reconfigurations: u64,
    pub device_recreations: u64,
    pub injected_faults: u64,
    pub quad_capacity_floats: usize,
    pub raster_capacity_floats: usize,
    pub text_row_capacity: usize,
    pub raster_cache_entries: usize,
    pub raster_cache_entries_high_water: usize,
    pub raster_cache_bytes: usize,
    pub raster_cache_bytes_high_water: usize,
    pub shaping_cache_entries: usize,
    pub shaping_cache_entries_high_water: usize,
    pub shaping_cache_accounted_bytes: usize,
    pub shaping_cache_accounted_bytes_high_water: usize,
    pub shaping_cache_hits: u64,
    pub shaping_cache_misses: u64,
    pub shaping_cache_evictions: u64,
    pub shaping_cache_rejections: u64,
    pub shaping_cache_invalidations: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GpuRenderError {
    Scene(SceneCompileError),
    OutOfMemory {
        message: String,
    },
    DeviceLost {
        reason: GpuDeviceLossReason,
        message: String,
    },
    Validation {
        message: String,
    },
    Internal {
        message: String,
    },
    SurfaceValidation,
    SurfaceRecreation {
        message: String,
    },
    TextAtlasFull,
    TextRender {
        message: String,
    },
    #[cfg(feature = "fault-injection")]
    FaultInjection {
        message: String,
    },
}

impl std::fmt::Display for GpuRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scene(error) => error.fmt(f),
            Self::OutOfMemory { message } => write!(f, "GPU out of memory: {message}"),
            Self::DeviceLost { reason, message } => {
                write!(f, "GPU device lost ({reason:?}): {message}")
            }
            Self::Validation { message } => write!(f, "GPU validation error: {message}"),
            Self::Internal { message } => write!(f, "internal GPU error: {message}"),
            Self::SurfaceValidation => f.write_str("GPU surface validation failed"),
            Self::SurfaceRecreation { message } => {
                write!(f, "GPU surface recreation failed: {message}")
            }
            Self::TextAtlasFull => f.write_str("GPU text atlas is full"),
            Self::TextRender { message } => write!(f, "GPU text render failed: {message}"),
            #[cfg(feature = "fault-injection")]
            Self::FaultInjection { message } => {
                write!(f, "GPU fault injection failed: {message}")
            }
        }
    }
}

impl std::error::Error for GpuRenderError {}

impl From<SceneCompileError> for GpuRenderError {
    fn from(error: SceneCompileError) -> Self {
        Self::Scene(error)
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
    InvalidTextProgram(&'static str),
    NativePresentation(NativePresentationPlanError),
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
            Self::InvalidTextProgram(reason) => {
                write!(f, "invalid compiled text program: {reason}")
            }
            Self::NativePresentation(error) => {
                write!(f, "invalid native presentation plan: {error:?}")
            }
        }
    }
}

impl std::error::Error for SceneCompileError {}

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
    presentation_plan: NativePresentationPlan,
}

impl PreparedScene {
    pub fn cell_program(&self) -> &CellProgram {
        &self.cell_program
    }

    pub fn artifacts(&self) -> &[PreparedArtifact] {
        &self.artifacts
    }

    pub fn presentation_plan(&self) -> &NativePresentationPlan {
        &self.presentation_plan
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
    let artifacts = prepare_artifacts(scene, &cell_program)?;
    let presentation_plan =
        prepare_native_presentation(scene, theme).map_err(SceneCompileError::NativePresentation)?;
    Ok(PreparedScene {
        cell_program,
        artifacts,
        presentation_plan,
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

fn prepare_artifacts(
    scene: &WorkspaceScene,
    program: &CellProgram,
) -> Result<Vec<PreparedArtifact>, SceneCompileError> {
    let mut prepared = Vec::new();
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
        let expected = expected_artifact_canvas(pane);
        let body = if scene.presentation.nodes.is_empty() {
            expected
        } else {
            let canvas_id = PresentationNodeId::pane(
                pane.id.clone(),
                PaneNodePart::Workflow(WorkflowNodePart::ArtifactCanvas),
            );
            let node = scene
                .presentation
                .nodes
                .iter()
                .find(|node| node.id == canvas_id)
                .ok_or(SceneCompileError::InvalidGeometry(
                    "ready artifact is missing its typed canvas",
                ))?;
            if node.role != PresentationNodeRole::ArtifactCanvas {
                return Err(SceneCompileError::InvalidGeometry(
                    "ready artifact canvas has the wrong semantic role",
                ));
            }
            if expected.is_empty() {
                if !node.state.hidden || node.cell_rect.is_some() {
                    return Err(SceneCompileError::InvalidGeometry(
                        "hidden artifact canvas does not match empty geometry",
                    ));
                }
                continue;
            }
            if node.state.hidden || node.cell_rect != Some(expected) {
                return Err(SceneCompileError::InvalidGeometry(
                    "ready artifact canvas does not match typed geometry",
                ));
            }
            expected
        };

        if body.is_empty() {
            continue;
        }
        let visible_clips = raster_clip_runs(program, layer);
        if visible_clips
            .iter()
            .any(|clip| !rect_contains(body, *clip))
        {
            return Err(SceneCompileError::InvalidGeometry(
                "artifact raster clip lies outside its typed canvas",
            ));
        }
        if visible_clips.is_empty() {
            continue;
        }
        prepared.push(PreparedArtifact {
            layer,
            body,
            visible_clips,
            width: surface.width,
            height: surface.height,
            revision: surface.revision,
            rgba8: surface.rgba8.clone(),
        });
    }
    Ok(prepared)
}

fn expected_artifact_canvas(pane: &mandatum_scene::PaneScene) -> SceneRect {
    let inner = layout::pane_inner_rect(pane.area);
    let rows = u16::try_from(pane.terminal_fallback_row_count()).unwrap_or(u16::MAX);
    let y = inner.y.saturating_add(rows).min(inner.bottom());
    SceneRect::new(
        inner.x,
        y,
        inner.width,
        inner.bottom().saturating_sub(y),
    )
}

fn rect_contains(outer: SceneRect, inner: SceneRect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
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

/// The text-buffer budget is enforced once, on the single row-run plan built
/// by `prepare_cell_program`; building a throwaway plan here only to count it
/// doubled the per-frame planning cost.
fn validate_compiled_program(program: &CellProgram) -> Result<(), SceneCompileError> {
    let instructions = program.cells().count();
    enforce_resource_limit("cell instructions", instructions, MAX_GPU_CELL_INSTRUCTIONS)
}

fn text_program_error(error: RowRunBuildError) -> SceneCompileError {
    let reason = match error {
        RowRunBuildError::ByteLengthOverflow => "row-run byte length overflow",
        RowRunBuildError::RunWidthOverflow => "row-run width overflow",
        RowRunBuildError::InvalidSplitBoundary { .. } => "invalid row-run split boundary",
    };
    SceneCompileError::InvalidTextProgram(reason)
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

fn enforce_text_buffer_work_limit(
    accepted: usize,
    pending: usize,
    expansion: usize,
) -> Result<(), SceneCompileError> {
    let actual = accepted
        .checked_add(pending)
        .and_then(|count| count.checked_add(expansion))
        .unwrap_or(usize::MAX);
    enforce_resource_limit("text buffers", actual, MAX_GPU_TEXT_BUFFERS)
}

fn anchored_fallback_runs_within_budget(
    run: &RowRun,
    accepted: usize,
    pending: usize,
) -> Result<Vec<RowRun>, SceneCompileError> {
    enforce_text_buffer_work_limit(accepted, pending, run.byte_cells.len())?;
    anchored_fallback_runs(run).map_err(text_program_error)
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
        OverlayScene::Appearance(overlay) => overlay.area,
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

/// Theme colors consumed outside cell materialization for each native frame.
///
/// Keeping this projection pure gives the clear pass and glyphon's default
/// text color one deterministic headless verification seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeFrameColors {
    default_foreground: [u8; 3],
    clear_background: [u8; 3],
}

fn native_frame_colors(theme: &Theme) -> NativeFrameColors {
    NativeFrameColors {
        default_foreground: theme.terminal_palette.foreground,
        clear_background: theme.terminal_palette.background,
    }
}

#[derive(Debug)]
struct PreparedCellProgram {
    cells: Vec<(u16, u16, ResolvedCell, bool, TextPaintScopeKind, bool)>,
    rows: Vec<RowRun>,
    issues: Vec<RowRunBuildIssue>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PixelRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MaterialQuad {
    role: crate::NativeMaterialRole,
    draw_rect: PixelRect,
    shape_rect: PixelRect,
    clip: (u32, u32, u32, u32),
    fill: [f32; 4],
    boundary: [f32; 4],
    corner_radius: f32,
    boundary_width: f32,
    blur_radius: f32,
    shadow: bool,
}

impl MaterialQuad {
    const FLOATS: usize = 20;

    fn write_instance(self, output: &mut Vec<f32>) {
        output.extend_from_slice(&[
            self.draw_rect.x,
            self.draw_rect.y,
            self.draw_rect.width,
            self.draw_rect.height,
            self.shape_rect.x,
            self.shape_rect.y,
            self.shape_rect.width,
            self.shape_rect.height,
            self.fill[0],
            self.fill[1],
            self.fill[2],
            self.fill[3],
            self.boundary[0],
            self.boundary[1],
            self.boundary[2],
            self.boundary[3],
            self.corner_radius,
            self.boundary_width,
            self.blur_radius,
            if self.shadow { 1.0 } else { 0.0 },
        ]);
    }
}

fn prepare_material_quads(
    plan: &NativePresentationPlan,
    scale: f32,
    surface_width: u32,
    surface_height: u32,
) -> Result<Vec<MaterialQuad>, SceneCompileError> {
    if !scale.is_finite() || scale <= 0.0 || surface_width == 0 || surface_height == 0 {
        return Ok(Vec::new());
    }
    let mut quads = Vec::new();
    let materials = plan
        .commands()
        .iter()
        .filter_map(|command| match command {
            NativePlanCommand::Material(material) => Some(material),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (material_index, material) in materials.iter().enumerate() {
        let Some(shape_rect) = logical_rect_to_physical(material.logical_rect, scale) else {
            continue;
        };
        let Some(clip) =
            logical_clip_to_scissor(material.clip, scale, surface_width, surface_height)
        else {
            continue;
        };
        if let Some(shadows) = material.raised_shadows {
            for shadow in shadows {
                let blur_radius = f32::from(shadow.blur_radius) * scale;
                let shape_rect = PixelRect {
                    x: shape_rect.x + f32::from(shadow.offset_x) * scale,
                    y: shape_rect.y + f32::from(shadow.offset_y) * scale,
                    ..shape_rect
                };
                let draw_rect = PixelRect {
                    x: shape_rect.x - blur_radius,
                    y: shape_rect.y - blur_radius,
                    width: shape_rect.width + blur_radius * 2.0,
                    height: shape_rect.height + blur_radius * 2.0,
                };
                let Some(draw_clip) =
                    pixel_rect_to_scissor(draw_rect, surface_width, surface_height)
                        .and_then(|draw| intersect_scissors(clip, draw))
                else {
                    continue;
                };
                let mut visible_clips = vec![draw_clip];
                for later in materials.iter().skip(material_index + 1) {
                    if later.raised_shadows.is_none() {
                        continue;
                    }
                    let Some(later_rect) = logical_rect_to_physical(later.logical_rect, scale)
                        .and_then(|rect| {
                            pixel_rect_to_scissor(rect, surface_width, surface_height)
                        })
                    else {
                        continue;
                    };
                    let mut next = Vec::with_capacity(visible_clips.len().saturating_mul(2));
                    for visible in visible_clips {
                        next.extend(subtract_scissor(visible, later_rect));
                        enforce_resource_limit(
                            "native material quad fragments",
                            quads.len().saturating_add(next.len()),
                            MAX_GPU_CELL_INSTRUCTIONS,
                        )?;
                    }
                    visible_clips = next;
                }
                for visible_clip in visible_clips {
                    quads.push(MaterialQuad {
                        role: material.role,
                        draw_rect,
                        shape_rect,
                        clip: visible_clip,
                        fill: ui_color_f32(shadow.color),
                        boundary: [0.0; 4],
                        corner_radius: material.corner_radius_units as f32 / 64.0 * scale,
                        boundary_width: 0.0,
                        blur_radius,
                        shadow: true,
                    });
                    enforce_resource_limit(
                        "native material quad fragments",
                        quads.len(),
                        MAX_GPU_CELL_INSTRUCTIONS,
                    )?;
                }
            }
        }
        quads.push(MaterialQuad {
            role: material.role,
            draw_rect: shape_rect,
            shape_rect,
            clip,
            fill: ui_color_f32(material.color),
            boundary: material
                .boundary
                .map(|boundary| ui_color_f32(boundary.color))
                .unwrap_or([0.0; 4]),
            corner_radius: material.corner_radius_units as f32 / 64.0 * scale,
            boundary_width: material
                .boundary
                .map(|boundary| boundary.width_units as f32 / 64.0 * scale)
                .unwrap_or(0.0),
            blur_radius: 0.0,
            shadow: false,
        });
        enforce_resource_limit(
            "native material quad fragments",
            quads.len(),
            MAX_GPU_CELL_INSTRUCTIONS,
        )?;
    }
    Ok(quads)
}

fn is_overlay_material(role: crate::NativeMaterialRole) -> bool {
    matches!(
        role,
        crate::NativeMaterialRole::OverlaySurface
            | crate::NativeMaterialRole::OverlayBand
            | crate::NativeMaterialRole::Selection
            | crate::NativeMaterialRole::SelectionIndicator
    )
}

fn logical_rect_to_physical(rect: LogicalRect, scale: f32) -> Option<PixelRect> {
    let unit_scale = scale / 64.0;
    let x = rect.origin.x_units() as f32 * unit_scale;
    let y = rect.origin.y_units() as f32 * unit_scale;
    let width = rect.size.width_units() as f32 * unit_scale;
    let height = rect.size.height_units() as f32 * unit_scale;
    (x.is_finite()
        && y.is_finite()
        && width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0)
        .then_some(PixelRect {
            x,
            y,
            width,
            height,
        })
}

fn logical_clip_to_scissor(
    clip: LogicalRect,
    scale: f32,
    surface_width: u32,
    surface_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let rect = logical_rect_to_physical(clip, scale)?;
    let left = rect.x.floor().max(0.0).min(surface_width as f32) as u32;
    let top = rect.y.floor().max(0.0).min(surface_height as f32) as u32;
    let right = (rect.x + rect.width)
        .ceil()
        .max(0.0)
        .min(surface_width as f32) as u32;
    let bottom = (rect.y + rect.height)
        .ceil()
        .max(0.0)
        .min(surface_height as f32) as u32;
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

fn pixel_rect_to_scissor(
    rect: PixelRect,
    surface_width: u32,
    surface_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let left = rect.x.floor().max(0.0).min(surface_width as f32) as u32;
    let top = rect.y.floor().max(0.0).min(surface_height as f32) as u32;
    let right = (rect.x + rect.width)
        .ceil()
        .max(0.0)
        .min(surface_width as f32) as u32;
    let bottom = (rect.y + rect.height)
        .ceil()
        .max(0.0)
        .min(surface_height as f32) as u32;
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

fn intersect_scissors(
    left: (u32, u32, u32, u32),
    right: (u32, u32, u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let x = left.0.max(right.0);
    let y = left.1.max(right.1);
    let right_edge = left
        .0
        .saturating_add(left.2)
        .min(right.0.saturating_add(right.2));
    let bottom_edge = left
        .1
        .saturating_add(left.3)
        .min(right.1.saturating_add(right.3));
    (right_edge > x && bottom_edge > y).then_some((x, y, right_edge - x, bottom_edge - y))
}

fn subtract_scissor(
    base: (u32, u32, u32, u32),
    cut: (u32, u32, u32, u32),
) -> Vec<(u32, u32, u32, u32)> {
    let Some(overlap) = intersect_scissors(base, cut) else {
        return vec![base];
    };
    let base_right = base.0 + base.2;
    let base_bottom = base.1 + base.3;
    let overlap_right = overlap.0 + overlap.2;
    let overlap_bottom = overlap.1 + overlap.3;
    let mut pieces = Vec::with_capacity(4);
    if overlap.1 > base.1 {
        pieces.push((base.0, base.1, base.2, overlap.1 - base.1));
    }
    if overlap_bottom < base_bottom {
        pieces.push((
            base.0,
            overlap_bottom,
            base.2,
            base_bottom - overlap_bottom,
        ));
    }
    if overlap.0 > base.0 {
        pieces.push((base.0, overlap.1, overlap.0 - base.0, overlap.3));
    }
    if overlap_right < base_right {
        pieces.push((
            overlap_right,
            overlap.1,
            base_right - overlap_right,
            overlap.3,
        ));
    }
    pieces
}

fn ui_color_f32(color: UiColor) -> [f32; 4] {
    let [red, green, blue, alpha] = color.to_array();
    [
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
        f32::from(alpha) / 255.0,
    ]
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
    let palette = &theme.terminal_palette;
    let mut foreground = resolve(cell.style.foreground, palette.foreground, palette);
    let mut background = resolve(cell.style.background, palette.background, palette);
    let terminal_selection_reverses = cell.selection == Some(CellSelection::Terminal)
        && theme.selection_highlight == SceneColor::Default;
    if cell.selection == Some(CellSelection::Terminal)
        && theme.selection_highlight != SceneColor::Default
    {
        background = resolve(theme.selection_highlight, palette.background, palette);
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

fn resolved_glyph_style(cell: &ProgramCell, theme: &Theme) -> ResolvedGlyphStyle {
    let cell = resolve_program_cell(cell, theme);
    ResolvedGlyphStyle {
        foreground: cell.foreground,
        bold: cell.bold,
        italic: cell.italic,
        underline: cell.underline,
        strikethrough: cell.strikethrough,
    }
}

fn prepare_cell_program(
    program: &CellProgram,
    scene: &WorkspaceScene,
    theme: &Theme,
    presentation_plan: &NativePresentationPlan,
) -> Result<PreparedCellProgram, SceneCompileError> {
    // `program.scoped_cells()` already yields final topmost cells in row-major
    // order, so there is nothing left to resolve by coordinate here.
    let cells = program
        .scoped_cells()
        .map(|(x, y, cell, scope)| {
            (
                x,
                y,
                resolve_program_cell(cell, theme),
                should_paint_legacy_background(scene, x, y, cell, scope.kind),
                scope.kind,
                cell.cursor,
            )
        })
        .collect::<Vec<_>>();
    let mut plan = build_row_runs(program, |cell| resolved_glyph_style(cell, theme))
        .map_err(text_program_error)?;
    plan.runs
        .retain(|run| {
            !matches!(
                run.paint_scope.kind,
                TextPaintScopeKind::PaneDecoration | TextPaintScopeKind::OverlayDecoration
            )
        });
    apply_native_text_scopes(&mut plan.runs, scene, presentation_plan)?;
    apply_modal_scrim_to_base_text(&mut plan.runs, presentation_plan);
    enforce_resource_limit("text buffers", plan.runs.len(), MAX_GPU_TEXT_BUFFERS)?;
    Ok(PreparedCellProgram {
        cells,
        rows: plan.runs,
        issues: plan.issues,
    })
}

fn apply_modal_scrim_to_base_text(rows: &mut [RowRun], plan: &NativePresentationPlan) {
    let scrim = plan.commands().iter().find_map(|command| match command {
        NativePlanCommand::Material(material)
            if material.role == crate::NativeMaterialRole::ModalScrim =>
        {
            Some(material.color.to_array())
        }
        _ => None,
    });
    let Some(scrim) = scrim else {
        return;
    };
    for row in rows {
        if matches!(
            row.paint_scope.kind,
            TextPaintScopeKind::Overlay
                | TextPaintScopeKind::OverlayDecoration
                | TextPaintScopeKind::TextInput
        ) {
            continue;
        }
        row.glyph_style.foreground = composite_scrim(row.glyph_style.foreground, scrim);
        for range in &mut row.style_ranges {
            range.style.foreground = composite_scrim(range.style.foreground, scrim);
        }
    }
}

fn composite_scrim(base: [u8; 4], scrim: [u8; 4]) -> [u8; 4] {
    let alpha = u16::from(scrim[3]);
    let inverse = 255u16.saturating_sub(alpha);
    [
        ((u16::from(scrim[0]) * alpha + u16::from(base[0]) * inverse + 127) / 255) as u8,
        ((u16::from(scrim[1]) * alpha + u16::from(base[1]) * inverse + 127) / 255) as u8,
        ((u16::from(scrim[2]) * alpha + u16::from(base[2]) * inverse + 127) / 255) as u8,
        base[3],
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProjectedNativeText {
    scope_index: usize,
    cell_rect: SceneRect,
    color: [u8; 4],
    metrics: crate::NativeTextMetricIdentity,
    geometry: NativeTextGeometry,
}

fn add_native_face(style: &mut ResolvedGlyphStyle, face: crate::NativeFontFace) {
    match face {
        crate::NativeFontFace::Regular => {}
        crate::NativeFontFace::Bold => style.bold = true,
        crate::NativeFontFace::Italic => style.italic = true,
        crate::NativeFontFace::BoldItalic => {
            style.bold = true;
            style.italic = true;
        }
    }
}

fn projected_row_geometry(
    projection: ProjectedNativeText,
    cell_y: u16,
    multirow: bool,
) -> Result<NativeTextGeometry, SceneCompileError> {
    if !multirow {
        return Ok(projection.geometry);
    }
    let relative_row = cell_y.checked_sub(projection.cell_rect.y).ok_or(
        SceneCompileError::InvalidTextProgram("native text row precedes its scope"),
    )?;
    if relative_row >= projection.cell_rect.height {
        return Err(SceneCompileError::InvalidTextProgram(
            "native text row escapes its scope",
        ));
    }
    let row_count = u128::from(projection.cell_rect.height);
    let total_height = u128::from(projection.geometry.logical_rect.size.height_units());
    let band_start = total_height
        .saturating_mul(u128::from(relative_row))
        / row_count;
    let band_end = total_height
        .saturating_mul(u128::from(relative_row) + 1)
        / row_count;
    let band_start = u64::try_from(band_start).map_err(|_| {
        SceneCompileError::InvalidTextProgram("native text row geometry overflows")
    })?;
    let band_end = u64::try_from(band_end).map_err(|_| {
        SceneCompileError::InvalidTextProgram("native text row geometry overflows")
    })?;
    let band_height = band_end.checked_sub(band_start).ok_or(
        SceneCompileError::InvalidTextProgram("native text row geometry is inverted"),
    )?;
    if band_height == 0 {
        return Err(SceneCompileError::InvalidTextProgram(
            "native text row geometry is empty",
        ));
    }
    let band_y = projection
        .geometry
        .logical_rect
        .origin
        .y_units()
        .checked_add_unsigned(band_start)
        .ok_or(SceneCompileError::InvalidTextProgram(
            "native text row geometry overflows",
        ))?;
    Ok(NativeTextGeometry {
        logical_rect: LogicalRect::from_units(
            projection.geometry.logical_rect.origin.x_units(),
            band_y,
            projection.geometry.logical_rect.size.width_units(),
            band_height,
        ),
        clip: projection.geometry.clip,
    })
}

fn apply_native_text_scopes(
    rows: &mut Vec<RowRun>,
    scene: &WorkspaceScene,
    plan: &NativePresentationPlan,
) -> Result<(), SceneCompileError> {
    let cell_count = usize::from(scene.size.width)
        .checked_mul(usize::from(scene.size.height))
        .unwrap_or(0);
    let mut projections = vec![None; cell_count];
    let mut scope_occupied_rows = Vec::<Vec<u16>>::new();
    let mut aggregate_cells = 0usize;
    for scope in plan.commands().iter().filter_map(|command| match command {
        NativePlanCommand::Text(scope) => Some(scope),
        _ => None,
    }) {
        let Some(rect) = scope.cell_rect else {
            continue;
        };
        let scope_index = scope_occupied_rows.len();
        scope_occupied_rows.push(Vec::new());
        let projection = ProjectedNativeText {
            scope_index,
            cell_rect: rect,
            color: scope.color.to_array(),
            metrics: scope.metrics,
            geometry: NativeTextGeometry {
                logical_rect: scope.logical_rect,
                clip: scope.clip,
            },
        };
        let area = usize::from(rect.width).saturating_mul(usize::from(rect.height));
        aggregate_cells = aggregate_cells.saturating_add(area);
        enforce_resource_limit(
            "native text-scope cell projections",
            aggregate_cells,
            MAX_GPU_CELL_INSTRUCTIONS,
        )?;
        for y in rect.y..rect.bottom().min(scene.size.height) {
            let row = usize::from(y) * usize::from(scene.size.width);
            for x in rect.x..rect.right().min(scene.size.width) {
                projections[row + usize::from(x)] = Some(projection);
            }
        }
    }

    let frame_width = usize::from(scene.size.width);
    let mut rows_with_spans = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        let mut spans = Vec::with_capacity(row.byte_cells.len());
        for span in &row.byte_cells {
            let first_x = row.x.checked_add(span.cells.start).ok_or(
                SceneCompileError::InvalidTextProgram("native text projection overflows row"),
            )?;
            let last_x = row.x.checked_add(span.cells.end).ok_or(
                SceneCompileError::InvalidTextProgram("native text projection overflows row"),
            )?;
            let mut projection = None;
            for x in first_x..last_x {
                let index = usize::from(row.y)
                    .checked_mul(frame_width)
                    .and_then(|base| base.checked_add(usize::from(x)));
                let cell_projection = index
                    .and_then(|index| projections.get(index))
                    .copied()
                    .flatten();
                if x == first_x {
                    projection = cell_projection;
                } else if projection != cell_projection {
                    return Err(SceneCompileError::InvalidTextProgram(
                        "native text scope splits a grapheme cell span",
                    ));
                }
            }
            spans.push(projection);
        }
        for projection in spans.iter().flatten() {
            let occupied = &mut scope_occupied_rows[projection.scope_index];
            if occupied.last().copied() != Some(row.y) {
                occupied.push(row.y);
            }
        }
        rows_with_spans.push((row, spans));
    }

    let mut projected = Vec::with_capacity(rows_with_spans.len());
    for (row, spans) in rows_with_spans {
        let mut remaining = Some(row);
        let mut span_start = 0usize;
        while span_start < spans.len() {
            let projection = spans[span_start];
            let segment_len = spans[span_start..]
                .iter()
                .take_while(|candidate| **candidate == projection)
                .count();
            let rest = remaining
                .take()
                .expect("projected span segments retain a remainder");
            let mut segment = if segment_len == rest.byte_cells.len() {
                rest
            } else {
                let (left, right) = split_at_span(&rest, segment_len).map_err(text_program_error)?;
                remaining = Some(right);
                left
            };
            if let Some(projection) = projection {
                segment.native_metrics = Some(projection.metrics);
                segment.native_geometry = Some(projected_row_geometry(
                    projection,
                    segment.y,
                    scope_occupied_rows[projection.scope_index].len() > 1,
                )?);
                if !segment.cursor {
                    segment.glyph_style.foreground = projection.color;
                }
                add_native_face(&mut segment.glyph_style, projection.metrics.style.face);
                for range in &mut segment.style_ranges {
                    if !segment.cursor {
                        range.style.foreground = projection.color;
                    }
                    add_native_face(&mut range.style, projection.metrics.style.face);
                }
            }
            projected.push(segment);
            span_start += segment_len;
        }
    }
    *rows = projected;
    Ok(())
}

fn should_paint_legacy_background(
    scene: &WorkspaceScene,
    x: u16,
    y: u16,
    cell: &ProgramCell,
    scope: TextPaintScopeKind,
) -> bool {
    if scene.presentation == mandatum_scene::ScenePresentation::default() {
        return true;
    }
    if cell.cursor
        || cell.selection == Some(CellSelection::Terminal)
        || cell.raster_layer.is_some()
    {
        return true;
    }
    match scope {
        TextPaintScopeKind::Header
        | TextPaintScopeKind::Status
        | TextPaintScopeKind::PaneChrome
        | TextPaintScopeKind::PaneDecoration => false,
        TextPaintScopeKind::PaneContent => {
            cell.style.background != SceneColor::Default
                || scene
                    .presentation
                    .terminal_viewports
                    .iter()
                    .any(|mapping| mapping.visible_cell_rect.contains(x, y))
        }
        TextPaintScopeKind::Overlay
        | TextPaintScopeKind::OverlayDecoration
        | TextPaintScopeKind::TextInput => false,
    }
}

fn glyph_attrs<'a>(style: ResolvedGlyphStyle, family: &'a str) -> Attrs<'a> {
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

#[derive(Clone, Debug)]
struct FontObservation {
    font_id: glyphon::fontdb::ID,
    glyph_id: u16,
    sample: String,
}

/// One retained shaping outcome.
///
/// Anchored buffers and fallback decompositions are cached alongside admitted
/// buffers: a permanently inadmissible run (a fallback face whose advance can
/// never match the cell) otherwise re-shaped and re-split on every frame, and
/// a braille spinner alone drives ~10 redraws a second.
#[derive(Clone, Debug)]
enum CachedShaping {
    Shaped {
        buffer: Arc<Buffer>,
        observations: Arc<[FontObservation]>,
    },
    /// The final decomposition a fallback cascade produced for this run,
    /// stored under the original parent's key so later frames materialize the
    /// sub-runs directly instead of re-running the cascade.
    Decomposed(Arc<[CachedDecompositionPart]>),
}

/// One leaf of a cached decomposition, positioned by span interval so the
/// entry stays independent of where the parent run lands on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CachedDecompositionPart {
    spans: Range<usize>,
    forced_anchor: bool,
}

#[derive(Debug)]
struct ShapedRow {
    row: RowRun,
    buffer: ShapedBuffer,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RowShapingProfile {
    metrics: Metrics,
    cell_advance: f32,
    metric_generation: u64,
    metric_slot: u8,
}

fn row_shaping_profile(
    row: &RowRun,
    terminal_metrics: Metrics,
    terminal_cell_advance: f32,
    scale: f32,
) -> RowShapingProfile {
    let Some(identity) = row.native_metrics else {
        return RowShapingProfile {
            metrics: terminal_metrics,
            cell_advance: terminal_cell_advance,
            metric_generation: 0,
            metric_slot: 0,
        };
    };
    // Cosmic-text snaps every monospace advance to `round(font_size)` em
    // widths, so a fractional physical size (theme Body is 12.5pt) shapes
    // narrower than the derived advance and fails admission on every row.
    // Round to whole physical pixels exactly like the terminal path, and
    // derive the advance from that same rounded size so shaping, admission,
    // and text-area geometry all agree. Line height stays unrounded: the
    // native line box must keep matching the planner's logical rect.
    let font_size = (f32::from(identity.style.point_size_x64) / 64.0 * scale).round();
    let metrics = Metrics::new(
        font_size,
        identity.style.line_height_units as f32 / 64.0 * scale,
    );
    RowShapingProfile {
        metrics,
        cell_advance: terminal_cell_advance * (font_size / terminal_metrics.font_size),
        metric_generation: identity.generation,
        metric_slot: identity.role as u8,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RowTextAreaGeometry {
    left: f32,
    top: f32,
    bounds: TextBounds,
}

fn row_text_area_geometry(
    row: &RowRun,
    profile: RowShapingProfile,
    cell_width: f32,
    cell_height: f32,
    scale: f32,
    surface_width: u32,
    surface_height: u32,
) -> RowTextAreaGeometry {
    let left = f32::from(row.x) * cell_width;
    let cell_top = f32::from(row.y) * cell_height;
    let cell_clip = row
        .clipped_cell_bounds()
        .unwrap_or_else(|| SceneRect::new(row.x, row.y, row.width, 1));
    let cell_bounds = glyph_text_bounds(
        cell_clip.x,
        cell_clip.y,
        cell_clip.width,
        cell_width,
        cell_height,
        surface_width,
        surface_height,
    );
    let Some(geometry) = row.native_geometry else {
        return RowTextAreaGeometry {
            left,
            top: cell_top,
            bounds: cell_bounds,
        };
    };
    let Some(logical_rect) = logical_rect_to_physical(geometry.logical_rect, scale) else {
        return RowTextAreaGeometry {
            left,
            top: cell_top,
            bounds: cell_bounds,
        };
    };
    let Some(node_scissor) =
        logical_clip_to_scissor(geometry.logical_rect, scale, surface_width, surface_height)
    else {
        return RowTextAreaGeometry {
            left,
            top: cell_top,
            bounds: cell_bounds,
        };
    };
    let Some(scope_scissor) =
        logical_clip_to_scissor(geometry.clip, scale, surface_width, surface_height)
    else {
        return RowTextAreaGeometry {
            left,
            top: cell_top,
            bounds: cell_bounds,
        };
    };
    let Some(vertical_clip) = intersect_scissors(node_scissor, scope_scissor) else {
        return RowTextAreaGeometry {
            left,
            top: cell_top,
            bounds: cell_bounds,
        };
    };
    let centered_top =
        logical_rect.y + ((logical_rect.height - profile.metrics.line_height).max(0.0) / 2.0);
    let semantic_left = i32::try_from(vertical_clip.0).unwrap_or(i32::MAX);
    let semantic_top = i32::try_from(vertical_clip.1).unwrap_or(i32::MAX);
    let semantic_right =
        i32::try_from(vertical_clip.0.saturating_add(vertical_clip.2)).unwrap_or(i32::MAX);
    let semantic_bottom =
        i32::try_from(vertical_clip.1.saturating_add(vertical_clip.3)).unwrap_or(i32::MAX);

    RowTextAreaGeometry {
        left,
        top: centered_top,
        bounds: TextBounds {
            left: cell_bounds.left.max(semantic_left),
            top: semantic_top,
            right: cell_bounds.right.min(semantic_right),
            bottom: semantic_bottom,
        },
    }
}

#[derive(Debug)]
enum ShapedBuffer {
    Shared(Arc<Buffer>),
    RowPool(usize),
}

/// Conservative retained-byte charge for one cached shaping outcome.
///
/// Cosmic-text deliberately hides allocation capacities inside `Buffer`.
/// Charge every directly retained key/output byte, a fixed buffer floor, and
/// conservative per-input/per-glyph/per-style expansion. This is an explicit
/// resource-accounting contract rather than an allocator-specific heap probe.
/// A decomposition retains no buffer, so it is charged for its key and its
/// leaf list only.
fn shaping_cache_accounted_bytes(key: &ShapingCacheKey, value: &CachedShaping) -> usize {
    const BUFFER_FLOOR: usize = 1_024;
    const BYTES_PER_INPUT_BYTE: usize = 16;
    const BYTES_PER_GLYPH: usize = 512;
    const BYTES_PER_STYLE_RANGE: usize = 256;
    const BYTES_PER_CELL_SPAN: usize = 128;

    match value {
        CachedShaping::Shaped { observations, .. } => observations.iter().fold(
            key.owned_bytes()
                .saturating_add(BUFFER_FLOOR)
                .saturating_add(key.text_len().saturating_mul(BYTES_PER_INPUT_BYTE))
                .saturating_add(observations.len().saturating_mul(BYTES_PER_GLYPH))
                .saturating_add(key.style_count().saturating_mul(BYTES_PER_STYLE_RANGE))
                .saturating_add(key.span_count().saturating_mul(BYTES_PER_CELL_SPAN)),
            |bytes, observation| {
                bytes
                    .saturating_add(std::mem::size_of::<FontObservation>())
                    .saturating_add(observation.sample.len())
            },
        ),
        CachedShaping::Decomposed(parts) => key.owned_bytes().saturating_add(
            parts
                .len()
                .saturating_mul(std::mem::size_of::<CachedDecompositionPart>()),
        ),
    }
}

fn shape_row_buffer(
    buffer: &mut Buffer,
    row: &RowRun,
    font_system: &mut FontSystem,
    metrics: Metrics,
    cell_width: f32,
    cell_height: f32,
    family: &str,
) {
    buffer.set_metrics(metrics);
    buffer.set_wrap(Wrap::None);
    // Quantizes advances to a `cell_width / font_size` grid, which is far finer
    // than a cell and so does not force whole cells. Proportional fallback
    // picks — Apple Braille for spinners, Webdings, the STIX faces — still fail
    // admission, which is what the anchored decomposition handles.
    buffer.set_monospace_width(Some(cell_width));
    buffer.set_size(
        Some((f32::from(row.width) * cell_width).max(1.0)),
        Some(cell_height),
    );
    let spans = row.style_ranges.iter().map(|range| {
        (
            &row.text[range.bytes.clone()],
            glyph_attrs(range.style, family),
        )
    });
    buffer.set_rich_text(
        spans,
        &Attrs::new().family(font_family(family)),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
}

fn layout_facts_and_observations(
    buffer: &Buffer,
    row: &RowRun,
) -> (LayoutRunFacts, Vec<FontObservation>) {
    let Some(layout) = buffer.layout_runs().next() else {
        return (
            LayoutRunFacts {
                rtl: false,
                line_width: 0.0,
                glyphs: Vec::new(),
            },
            Vec::new(),
        );
    };
    let facts = LayoutRunFacts {
        rtl: layout.rtl,
        line_width: layout.line_w,
        glyphs: layout
            .glyphs
            .iter()
            .map(|glyph| LayoutGlyphFacts {
                bytes: glyph.start..glyph.end,
                x: glyph.x,
                advance: glyph.w,
                rtl: glyph.level.is_rtl(),
            })
            .collect(),
    };
    let observations = layout
        .glyphs
        .iter()
        .map(|glyph| FontObservation {
            font_id: glyph.font_id,
            glyph_id: glyph.glyph_id,
            sample: row
                .text
                .get(glyph.start..glyph.end)
                .map(bounded_sample)
                .unwrap_or_else(|| "<invalid-cluster>".to_owned()),
        })
        .collect();
    (facts, observations)
}

fn bounded_sample(value: &str) -> String {
    value.chars().take(32).collect()
}

fn record_row_run_diagnostic(diagnostics: &mut BTreeSet<String>, message: String) {
    const MAX_ROW_RUN_DIAGNOSTICS: usize = 16;
    if diagnostics.len() >= MAX_ROW_RUN_DIAGNOSTICS || !diagnostics.insert(message.clone()) {
        return;
    }
    eprintln!("mandatum-native-renderer: {message}");
}

/// One shaping candidate waiting in a fallback cascade.
#[derive(Debug)]
struct PendingRun {
    run: RowRun,
    forced_anchor: bool,
    /// Span interval this piece occupies inside the top-level run whose
    /// decomposition is being recorded.
    spans: Range<usize>,
}

/// Borrowed text-stack state for one row-run shaping pass.
///
/// Split out of `GpuText` so the fallback/split protocol and its cache
/// bookkeeping stay exercisable without a GPU device.
struct RowShapingPass<'a> {
    font_system: &'a mut FontSystem,
    row_buffers: &'a mut RowBufferPool,
    shaping_cache: &'a mut ShapingCache<CachedShaping>,
    fallback_report: &'a mut FallbackReport,
    diagnostics: &'a mut BTreeSet<String>,
    font_profile: &'a ResolvedFontProfile,
    font_family: &'a str,
    cache_enabled: bool,
    terminal_metrics: Metrics,
    cell_advance: f32,
    cell_height: f32,
    scale: f32,
    scale_generation: u64,
}

impl RowShapingPass<'_> {
    fn row_profile(&self, run: &RowRun) -> RowShapingProfile {
        row_shaping_profile(run, self.terminal_metrics, self.cell_advance, self.scale)
    }

    fn cache_context(&self, profile: RowShapingProfile) -> ShapingCacheContext {
        ShapingCacheContext {
            font_generation: self.font_profile.generation(),
            scale_generation: self.scale_generation,
            metric_generation: profile.metric_generation,
            metric_slot: profile.metric_slot,
            renderer_config_generation: SHAPING_POLICY_GENERATION,
            font_size_bits: profile.metrics.font_size.to_bits(),
            line_height_bits: profile.metrics.line_height.to_bits(),
            cell_width_bits: profile.cell_advance.to_bits(),
            cell_height_bits: self.cell_height.to_bits(),
        }
    }

    fn observe_font_output(&mut self, run_text: &str, observations: &[FontObservation]) {
        if observations.is_empty() && self.fallback_report.observe_missing_glyph(run_text) {
            self.emit_new_fallback_record();
        }
        for observation in observations {
            if observation.glyph_id == 0
                && self
                    .fallback_report
                    .observe_missing_glyph(&observation.sample)
            {
                self.emit_new_fallback_record();
            }
            if self.fallback_report.observe_face(
                self.font_profile,
                observation.font_id,
                &observation.sample,
            ) {
                self.emit_new_fallback_record();
            }
        }
    }

    fn emit_new_fallback_record(&self) {
        let Some(record) = self.fallback_report.records().last() else {
            return;
        };
        match record {
            FallbackRecord::Face {
                family,
                postscript_name,
                sample,
                ..
            } => eprintln!(
                "mandatum-native-renderer: font fallback family={family:?} postscript={postscript_name:?} sample={sample:?}"
            ),
            FallbackRecord::MissingGlyph { sample } => {
                eprintln!("mandatum-native-renderer: missing glyph sample={sample:?}")
            }
        }
    }

    /// Shape every row run, cascading rejected runs into admissible pieces.
    ///
    /// Each top-level run owns one cascade, so the leaves it ends up with are
    /// exactly the decomposition retained under its own cache key.
    fn run(&mut self, initial: Vec<RowRun>) -> Result<Vec<ShapedRow>, GpuRenderError> {
        let mut accepted: Vec<ShapedRow> = Vec::new();
        let mut pending: VecDeque<PendingRun> = VecDeque::new();
        let mut queued = initial.len();

        for top in initial {
            queued = queued.saturating_sub(1);
            let top_spans = top.byte_cells.len();
            let top_key = shaping_cache_key_for_candidate(
                self.cache_enabled,
                false,
                &top,
                self.cache_context(self.row_profile(&top)),
            );
            let mut leaves: Vec<CachedDecompositionPart> = Vec::new();
            let mut decomposed = false;
            let mut reused_decomposition = false;
            let mut next_is_top = true;

            pending.push_back(PendingRun {
                run: top,
                forced_anchor: false,
                spans: 0..top_spans,
            });
            while let Some(PendingRun {
                run,
                forced_anchor,
                spans,
            }) = pending.pop_front()
            {
                enforce_text_buffer_work_limit(accepted.len(), queued + pending.len(), 1)?;
                let is_top = std::mem::replace(&mut next_is_top, false);
                let profile = self.row_profile(&run);
                let metrics = profile.metrics;
                let cache_key = shaping_cache_key_for_candidate(
                    self.cache_enabled,
                    forced_anchor,
                    &run,
                    self.cache_context(profile),
                );

                if let Some(cached) = cache_key
                    .as_ref()
                    .and_then(|key| self.shaping_cache.get_cloned(key))
                {
                    match cached {
                        CachedShaping::Shaped {
                            buffer,
                            observations,
                        } => {
                            self.observe_font_output(&run.text, &observations);
                            leaves.push(CachedDecompositionPart {
                                spans,
                                forced_anchor,
                            });
                            accepted.push(ShapedRow {
                                row: run,
                                buffer: ShapedBuffer::Shared(buffer),
                            });
                        }
                        CachedShaping::Decomposed(parts) => {
                            // A reused decomposition still pays the buffer
                            // budget for everything it materializes.
                            enforce_text_buffer_work_limit(
                                accepted.len(),
                                queued + pending.len(),
                                parts.len(),
                            )?;
                            decomposed = true;
                            reused_decomposition |= is_top;
                            for part in parts.iter().rev() {
                                let piece = slice_run(&run, part.spans.clone())
                                    .map_err(text_program_error)?;
                                pending.push_front(PendingRun {
                                    run: piece,
                                    forced_anchor: part.forced_anchor,
                                    spans: spans.start + part.spans.start
                                        ..spans.start + part.spans.end,
                                });
                            }
                        }
                    }
                    continue;
                }

                let buffer_index = accepted.len();
                self.row_buffers
                    .ensure_len(buffer_index + 1, self.font_system, metrics);
                let (layout, observations) = {
                    let buffer = &mut self.row_buffers.rows[buffer_index];
                    let buffer_height = if run.native_metrics.is_some() {
                        metrics.line_height
                    } else {
                        self.cell_height
                    };
                    shape_row_buffer(
                        buffer,
                        &run,
                        self.font_system,
                        metrics,
                        profile.cell_advance,
                        buffer_height,
                        self.font_family,
                    );
                    layout_facts_and_observations(buffer, &run)
                };

                self.observe_font_output(&run.text, &observations);

                let admission = if forced_anchor {
                    RowRunAdmission::Accepted
                } else {
                    // Font metrics are already scaled to physical pixels and
                    // TextArea uses scale 1.0, so layout facts are physical here.
                    admit_layout(&run, &layout, profile.cell_advance, 1.0)
                };
                match admission {
                    RowRunAdmission::Accepted => {
                        leaves.push(CachedDecompositionPart {
                            spans,
                            forced_anchor,
                        });
                        // Anchored buffers are cached too: they are what the
                        // live cascade would rebuild verbatim next frame.
                        let buffer = if let Some(key) = cache_key {
                            let replacement = Buffer::new(self.font_system, metrics);
                            let buffer = Arc::new(std::mem::replace(
                                &mut self.row_buffers.rows[buffer_index],
                                replacement,
                            ));
                            let value = CachedShaping::Shaped {
                                buffer: buffer.clone(),
                                observations: Arc::<[FontObservation]>::from(observations),
                            };
                            let accounted_bytes = shaping_cache_accounted_bytes(&key, &value);
                            self.shaping_cache.insert(key, value, accounted_bytes);
                            ShapedBuffer::Shared(buffer)
                        } else {
                            ShapedBuffer::RowPool(buffer_index)
                        };
                        accepted.push(ShapedRow { row: run, buffer });
                    }
                    RowRunAdmission::Fallback { reason, action } => {
                        decomposed = true;
                        record_row_run_diagnostic(
                            self.diagnostics,
                            format!(
                                "row-run fallback {reason:?} for {:?}",
                                bounded_sample(&run.text)
                            ),
                        );
                        match action {
                            RowRunFallbackAction::SplitAroundCluster { cluster } => {
                                let pieces = partition_around_cluster(&run, cluster)
                                    .map_err(text_program_error)?;
                                enforce_text_buffer_work_limit(
                                    accepted.len(),
                                    queued + pending.len(),
                                    pieces.len(),
                                )?;
                                for piece in pieces.into_iter().rev() {
                                    pending.push_front(PendingRun {
                                        run: piece.run,
                                        forced_anchor: piece.forced_anchor,
                                        spans: spans.start + piece.spans.start
                                            ..spans.start + piece.spans.end,
                                    });
                                }
                            }
                            RowRunFallbackAction::AnchorAll => {
                                let anchored = anchored_fallback_runs_within_budget(
                                    &run,
                                    accepted.len(),
                                    queued + pending.len(),
                                )?;
                                for (index, part) in anchored.into_iter().enumerate().rev() {
                                    pending.push_front(PendingRun {
                                        run: part,
                                        forced_anchor: true,
                                        spans: spans.start + index..spans.start + index + 1,
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // An admitted top-level run is already retained under its own key;
            // only a decomposition needs recording, or a permanently
            // inadmissible run re-shapes and re-splits on every frame.
            if decomposed
                && !reused_decomposition
                && let Some(key) = top_key
            {
                let value = CachedShaping::Decomposed(Arc::from(leaves));
                let accounted_bytes = shaping_cache_accounted_bytes(&key, &value);
                self.shaping_cache.insert(key, value, accounted_bytes);
            }
        }

        Ok(accepted)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupFailureStage {
    Configuration,
    Surface,
    Adapter,
    Device,
}

fn startup_error(stage: StartupFailureStage, message: impl Into<String>) -> GpuStartupError {
    let kind = match stage {
        StartupFailureStage::Configuration => GpuStartupErrorKind::InvalidConfiguration,
        StartupFailureStage::Surface => GpuStartupErrorKind::NoDisplay,
        StartupFailureStage::Adapter => GpuStartupErrorKind::NoAdapter,
        StartupFailureStage::Device => GpuStartupErrorKind::DeviceRequest,
    };
    GpuStartupError {
        kind,
        message: message.into(),
    }
}

#[derive(Debug)]
struct GpuFaultState {
    active_device_generation: u64,
    pending: Option<GpuRenderError>,
}

type GpuFaultSlot = Arc<Mutex<GpuFaultState>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UncapturedErrorKind {
    OutOfMemory,
    Validation,
    Internal,
}

fn uncaptured_gpu_error(kind: UncapturedErrorKind, message: String) -> GpuRenderError {
    match kind {
        UncapturedErrorKind::OutOfMemory => GpuRenderError::OutOfMemory { message },
        UncapturedErrorKind::Validation => GpuRenderError::Validation { message },
        UncapturedErrorKind::Internal => GpuRenderError::Internal { message },
    }
}

fn device_lost_error(reason: wgpu::DeviceLostReason, message: String) -> GpuRenderError {
    let reason = match reason {
        wgpu::DeviceLostReason::Unknown => GpuDeviceLossReason::Unknown,
        wgpu::DeviceLostReason::Destroyed => GpuDeviceLossReason::Destroyed,
    };
    GpuRenderError::DeviceLost { reason, message }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceAcquireSignal {
    Timeout,
    Occluded,
    Outdated,
    Lost,
    Validation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceAcquireDirective {
    Skip(GpuFrameSkip),
    Recover(GpuSurfaceRecovery),
    FailValidation,
}

fn surface_acquire_directive(signal: SurfaceAcquireSignal) -> SurfaceAcquireDirective {
    match signal {
        SurfaceAcquireSignal::Timeout => SurfaceAcquireDirective::Skip(GpuFrameSkip::Timeout),
        SurfaceAcquireSignal::Occluded => SurfaceAcquireDirective::Skip(GpuFrameSkip::Occluded),
        SurfaceAcquireSignal::Outdated => {
            SurfaceAcquireDirective::Recover(GpuSurfaceRecovery::Outdated)
        }
        SurfaceAcquireSignal::Lost => SurfaceAcquireDirective::Recover(GpuSurfaceRecovery::Lost),
        SurfaceAcquireSignal::Validation => SurfaceAcquireDirective::FailValidation,
    }
}

#[cfg(feature = "fault-injection")]
fn injected_surface_recovery(injection: GpuFaultInjection) -> Option<GpuSurfaceRecovery> {
    match injection {
        GpuFaultInjection::SurfaceOutdated => Some(GpuSurfaceRecovery::Outdated),
        GpuFaultInjection::SurfaceLost => Some(GpuSurfaceRecovery::Lost),
        GpuFaultInjection::DeviceLost | GpuFaultInjection::OutOfMemory => None,
    }
}

fn gpu_fault_priority(fault: &GpuRenderError) -> u8 {
    match fault {
        GpuRenderError::OutOfMemory { .. } => 4,
        GpuRenderError::DeviceLost { .. } => 3,
        GpuRenderError::Internal { .. } => 2,
        GpuRenderError::Validation { .. } => 1,
        _ => 0,
    }
}

fn record_gpu_fault(slot: &GpuFaultSlot, generation: u64, fault: GpuRenderError) {
    let mut state = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.active_device_generation != generation {
        return;
    }

    let should_replace = state
        .pending
        .as_ref()
        .is_none_or(|pending| gpu_fault_priority(&fault) > gpu_fault_priority(pending));
    if should_replace {
        state.pending = Some(fault);
    }
}

#[cfg(any(test, feature = "fault-injection"))]
fn has_gpu_fault(slot: &GpuFaultSlot, generation: u64) -> bool {
    let state = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    state.active_device_generation == generation && state.pending.is_some()
}

fn take_gpu_fault(slot: &GpuFaultSlot, generation: u64) -> Option<GpuRenderError> {
    let mut state = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    (state.active_device_generation == generation)
        .then(|| state.pending.take())
        .flatten()
}

fn retire_gpu_generation(slot: &GpuFaultSlot, next_generation: u64) {
    let mut state = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    state.active_device_generation = next_generation;
    state.pending = None;
}

fn adapter_metadata(info: wgpu::AdapterInfo) -> GpuAdapterMetadata {
    let device_type = match info.device_type {
        wgpu::DeviceType::Other => "other",
        wgpu::DeviceType::IntegratedGpu => "integrated-gpu",
        wgpu::DeviceType::DiscreteGpu => "discrete-gpu",
        wgpu::DeviceType::VirtualGpu => "virtual-gpu",
        wgpu::DeviceType::Cpu => "cpu",
    };
    GpuAdapterMetadata {
        name: info.name,
        backend: info.backend.to_str(),
        device_type,
        driver: info.driver,
        driver_info: info.driver_info,
        vendor: info.vendor,
        device: info.device,
    }
}

pub struct GpuText {
    instance: wgpu::Instance,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    adapter_metadata: GpuAdapterMetadata,
    device_fault: GpuFaultSlot,
    device_generation: u64,
    surface_generation: u64,
    surface_reconfigurations: u64,
    device_recreations: u64,
    injected_faults: u64,
    raster_cache_entries_high_water: usize,
    raster_cache_bytes_high_water: usize,
    presentation_motion: crate::PresentationMotion,

    // Solid-quad pipeline.
    quad_pipeline: wgpu::RenderPipeline,
    material_pipeline: wgpu::RenderPipeline,
    unit_buf: wgpu::Buffer,
    inst_buf: wgpu::Buffer,
    inst_capacity_floats: usize,
    material_inst_buf: wgpu::Buffer,
    material_inst_capacity_floats: usize,
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
    shaping_cache: ShapingCache<CachedShaping>,
    shaping_cache_palette: Option<TerminalPalette>,
    shaping_cache_enabled: bool,
    scale_generation: u64,

    font_profile: ResolvedFontProfile,
    fallback_report: FallbackReport,
    row_run_diagnostics: BTreeSet<String>,
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
    ) -> Result<Self, GpuStartupError> {
        NativeTextSettings::new(text_settings.family.clone(), text_settings.font_size)
            .map_err(|error| startup_error(StartupFailureStage::Configuration, error))?;
        let request = if text_settings.family.eq_ignore_ascii_case("monospace") {
            FontRequest::BundledDefault {
                size: text_settings.font_size,
            }
        } else {
            FontRequest::installed(text_settings.family, text_settings.font_size)
        };
        let profile = ResolvedFontProfile::resolve(request).map_err(|error| {
            startup_error(StartupFailureStage::Configuration, error.to_string())
        })?;
        Self::new_with_profile(window, profile).await
    }

    pub async fn new_with_profile(
        window: Arc<Window>,
        profile: ResolvedFontProfile,
    ) -> Result<Self, GpuStartupError> {
        Self::new_with_device_generation(window, profile, 1).await
    }

    async fn new_with_device_generation(
        window: Arc<Window>,
        font_profile: ResolvedFontProfile,
        device_generation: u64,
    ) -> Result<Self, GpuStartupError> {
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        validate_scale(scale)
            .map_err(|error| startup_error(StartupFailureStage::Configuration, error))?;

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| startup_error(StartupFailureStage::Surface, error.to_string()))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| startup_error(StartupFailureStage::Adapter, error.to_string()))?;
        let adapter_metadata = adapter_metadata(adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("mandatum-native-device"),
                ..Default::default()
            })
            .await
            .map_err(|error| startup_error(StartupFailureStage::Device, error.to_string()))?;

        let device_fault = Arc::new(Mutex::new(GpuFaultState {
            active_device_generation: device_generation,
            pending: None,
        }));
        let uncaptured_fault = device_fault.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            let (kind, message) = match error {
                wgpu::Error::OutOfMemory { source } => {
                    (UncapturedErrorKind::OutOfMemory, source.to_string())
                }
                wgpu::Error::Validation { description, .. } => {
                    (UncapturedErrorKind::Validation, description)
                }
                wgpu::Error::Internal { description, .. } => {
                    (UncapturedErrorKind::Internal, description)
                }
            };
            record_gpu_fault(
                &uncaptured_fault,
                device_generation,
                uncaptured_gpu_error(kind, message),
            );
        }));
        let lost_fault = device_fault.clone();
        device.set_device_lost_callback(move |reason, message| {
            record_gpu_fault(
                &lost_fault,
                device_generation,
                device_lost_error(reason, message),
            );
        });

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| {
                startup_error(
                    StartupFailureStage::Configuration,
                    "surface exposes no texture formats",
                )
            })?;
        let alpha_mode = caps.alpha_modes.first().copied().ok_or_else(|| {
            startup_error(
                StartupFailureStage::Configuration,
                "surface exposes no alpha modes",
            )
        })?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
            // One frame of queue, not wgpu's default two: hover and typing
            // respond a full refresh sooner, and the scene is cheap enough
            // that we never need the second frame of slack.
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);

        // --- Quad pipeline ---------------------------------------------------
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad-shader"),
            source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
        });
        let material_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native-material-shader"),
            source: wgpu::ShaderSource::Wgsl(MATERIAL_WGSL.into()),
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
        const MATERIAL_INST_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            1 => Float32x4,
            2 => Float32x4,
            3 => Float32x4,
            4 => Float32x4,
            5 => Float32x4
        ];
        let material_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native-material-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &material_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &UNIT_ATTRS,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: (MaterialQuad::FLOATS * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &MATERIAL_INST_ATTRS,
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &material_shader,
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
        let material_inst_capacity_floats = MaterialQuad::FLOATS * 4096;
        let material_inst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native-material-instances"),
            size: (material_inst_capacity_floats * 4) as u64,
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
        let mut font_system = font_profile.create_font_system();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        let font_size = (font_profile.size() * scale).round();
        let line_height = (font_size * 1.3).round();
        let metrics = Metrics::new(font_size, line_height);
        let cell_w = measure_cell_width(&mut font_system, metrics, font_profile.family());
        let cell_h = line_height;

        Ok(Self {
            instance,
            window,
            surface,
            adapter,
            device,
            queue,
            config,
            adapter_metadata,
            device_fault,
            device_generation,
            surface_generation: 1,
            surface_reconfigurations: 0,
            device_recreations: 0,
            injected_faults: 0,
            raster_cache_entries_high_water: 0,
            raster_cache_bytes_high_water: 0,
            presentation_motion: crate::PresentationMotion::default(),
            quad_pipeline,
            material_pipeline,
            unit_buf,
            inst_buf,
            inst_capacity_floats,
            material_inst_buf,
            material_inst_capacity_floats,
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
            shaping_cache: ShapingCache::new(),
            shaping_cache_palette: None,
            shaping_cache_enabled: true,
            scale_generation: 1,
            fallback_report: FallbackReport::new(font_profile.generation()),
            row_run_diagnostics: BTreeSet::new(),
            scale,
            base_font_size: font_profile.size(),
            font_family: font_profile.family().to_owned(),
            font_profile,
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

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn adapter_metadata(&self) -> &GpuAdapterMetadata {
        &self.adapter_metadata
    }

    pub fn lifecycle_snapshot(&self) -> GpuLifecycleSnapshot {
        let raster_cache_bytes = self
            .raster_cache
            .values()
            .map(|raster| raster.rgba8.len())
            .sum();
        GpuLifecycleSnapshot {
            device_generation: self.device_generation,
            surface_generation: self.surface_generation,
            surface_reconfigurations: self.surface_reconfigurations,
            device_recreations: self.device_recreations,
            injected_faults: self.injected_faults,
            quad_capacity_floats: self.inst_capacity_floats,
            raster_capacity_floats: self.raster_inst_capacity_floats,
            text_row_capacity: self.row_buffers.len(),
            raster_cache_entries: self.raster_cache.len(),
            raster_cache_entries_high_water: self.raster_cache_entries_high_water,
            raster_cache_bytes,
            raster_cache_bytes_high_water: self.raster_cache_bytes_high_water,
            shaping_cache_entries: self.shaping_cache.len(),
            shaping_cache_entries_high_water: self.shaping_cache.stats().entries_high_water,
            shaping_cache_accounted_bytes: self.shaping_cache.accounted_bytes(),
            shaping_cache_accounted_bytes_high_water: self
                .shaping_cache
                .stats()
                .accounted_bytes_high_water,
            shaping_cache_hits: self.shaping_cache.stats().hits,
            shaping_cache_misses: self.shaping_cache.stats().misses,
            shaping_cache_evictions: self.shaping_cache.stats().evictions,
            shaping_cache_rejections: self.shaping_cache.stats().rejections,
            shaping_cache_invalidations: self.shaping_cache.stats().invalidations,
        }
    }

    /// Lab-only control for paired cached/uncached renderer measurements.
    ///
    /// The production feature closure has no cache-disable surface.
    #[cfg(feature = "fault-injection")]
    pub fn set_shaping_cache_enabled(&mut self, enabled: bool) {
        if self.shaping_cache_enabled != enabled {
            self.shaping_cache.invalidate();
            self.shaping_cache_enabled = enabled;
        }
    }

    /// Drive the lab harness's fault matrix through the same renderer paths
    /// used by real surface and device failures.
    #[cfg(feature = "fault-injection")]
    pub fn inject_fault(
        &mut self,
        injection: GpuFaultInjection,
    ) -> Result<GpuFaultInjectionResult, GpuRenderError> {
        if let Some(recovery) = injected_surface_recovery(injection) {
            self.recover_surface(recovery)?;
            self.injected_faults = self.injected_faults.saturating_add(1);
            return Ok(GpuFaultInjectionResult::SurfaceReconfigured(recovery));
        }
        let result: Result<GpuFaultInjectionResult, GpuRenderError> = match injection {
            GpuFaultInjection::OutOfMemory => {
                record_gpu_fault(
                    &self.device_fault,
                    self.device_generation,
                    GpuRenderError::OutOfMemory {
                        message: "fault injection".to_owned(),
                    },
                );
                Ok(GpuFaultInjectionResult::FaultQueued)
            }
            GpuFaultInjection::DeviceLost => {
                self.device.destroy();
                let _ = self.device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(1)),
                });
                if !has_gpu_fault(&self.device_fault, self.device_generation) {
                    return Err(GpuRenderError::FaultInjection {
                        message:
                            "device destroy did not invoke the generation-stamped loss callback"
                                .to_owned(),
                    });
                }
                Ok(GpuFaultInjectionResult::FaultQueued)
            }
            GpuFaultInjection::SurfaceOutdated | GpuFaultInjection::SurfaceLost => {
                unreachable!("surface injections return above")
            }
        };
        let result = result?;
        self.injected_faults = self.injected_faults.saturating_add(1);
        Ok(result)
    }

    /// Rebuild every GPU-owned resource after device loss while preserving
    /// only the window and renderer settings. Product state remains owned by
    /// the caller and can submit its next `WorkspaceScene` to the replacement.
    pub async fn recreate_device(&mut self) -> Result<(), GpuStartupError> {
        let (width, height) = self.surface_size();
        let scale = self.scale;
        let base_font_size = self.base_font_size;
        let next_device_generation = self.device_generation.saturating_add(1);
        let next_surface_generation = self.surface_generation.saturating_add(1);
        let next_device_recreations = self.device_recreations.saturating_add(1);
        let shaping_cache_stats = self.shaping_cache.stats();
        let shaping_cache_had_entries = self.shaping_cache.len() > 0;
        let mut replacement = Self::new_with_device_generation(
            self.window.clone(),
            self.font_profile.clone_for_device_recreation(),
            next_device_generation,
        )
        .await?;
        replacement.resize_surface(width, height);
        replacement
            .set_scale(scale)
            .map_err(|error| startup_error(StartupFailureStage::Configuration, error))?;
        replacement
            .set_base_font_size(base_font_size)
            .map_err(|error| startup_error(StartupFailureStage::Configuration, error))?;
        replacement.surface_generation = next_surface_generation;
        replacement.surface_reconfigurations = self.surface_reconfigurations;
        replacement.device_recreations = next_device_recreations;
        replacement.injected_faults = self.injected_faults;
        replacement.raster_cache_entries_high_water = self.raster_cache_entries_high_water;
        replacement.raster_cache_bytes_high_water = self.raster_cache_bytes_high_water;
        replacement.fallback_report = self.fallback_report.clone();
        replacement.row_run_diagnostics = self.row_run_diagnostics.clone();
        replacement.shaping_cache_enabled = self.shaping_cache_enabled;
        replacement
            .shaping_cache
            .preserve_stats_after_cold_reset(shaping_cache_stats, shaping_cache_had_entries);
        retire_gpu_generation(&self.device_fault, next_device_generation);
        *self = replacement;
        Ok(())
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
        self.refresh_text_metrics();
        Ok(())
    }

    /// Change the live point size. Same invalidation contract as a scale
    /// change: shaping-cache flush plus metric recomputation; glyphon's
    /// atlas re-rasterizes on demand, so no GPU resource rebuild is needed.
    pub fn set_base_font_size(&mut self, size: f32) -> Result<(), String> {
        if !size.is_finite() || !(6.0..=72.0).contains(&size) {
            return Err(format!(
                "font size must be finite and between 6 and 72 points, got {size}"
            ));
        }
        if (size - self.base_font_size).abs() < f32::EPSILON {
            return Ok(());
        }
        self.base_font_size = size;
        self.refresh_text_metrics();
        Ok(())
    }

    /// The unscaled configured point size the next device recreation and
    /// font-facts declaration must preserve.
    pub fn base_font_size(&self) -> f32 {
        self.base_font_size
    }

    fn refresh_text_metrics(&mut self) {
        self.scale_generation = self.scale_generation.saturating_add(1);
        self.shaping_cache.invalidate();
        self.font_size = (self.base_font_size * self.scale).round();
        let line_height = (self.font_size * 1.3).round();
        let metrics = Metrics::new(self.font_size, line_height);
        self.row_buffers.set_metrics(metrics);
        self.cell_w = measure_cell_width(&mut self.font_system, metrics, &self.font_family);
        self.cell_h = line_height;
    }

    /// Swap the live font family by rebuilding the text stack around a
    /// freshly resolved profile. This reuses the device-recreation path —
    /// heavier than a family change strictly needs, but every invariant of
    /// that tested path (generation retirement, cache identity, surface
    /// reconfiguration) carries over unchanged. The configured point size
    /// survives the swap.
    pub async fn apply_font_profile(
        &mut self,
        profile: ResolvedFontProfile,
    ) -> Result<(), GpuStartupError> {
        let base_font_size = self.base_font_size;
        let (width, height) = self.surface_size();
        let scale = self.scale;
        let next_device_generation = self.device_generation.saturating_add(1);
        let next_surface_generation = self.surface_generation.saturating_add(1);
        let mut replacement =
            Self::new_with_device_generation(self.window.clone(), profile, next_device_generation)
                .await?;
        replacement.resize_surface(width, height);
        replacement
            .set_scale(scale)
            .map_err(|error| startup_error(StartupFailureStage::Configuration, error))?;
        replacement
            .set_base_font_size(base_font_size)
            .map_err(|error| startup_error(StartupFailureStage::Configuration, error))?;
        replacement.surface_generation = next_surface_generation;
        replacement.surface_reconfigurations = self.surface_reconfigurations;
        replacement.device_recreations = self.device_recreations;
        replacement.injected_faults = self.injected_faults;
        replacement.shaping_cache_enabled = self.shaping_cache_enabled;
        retire_gpu_generation(&self.device_fault, next_device_generation);
        *self = replacement;
        Ok(())
    }

    fn recover_surface(&mut self, recovery: GpuSurfaceRecovery) -> Result<(), GpuRenderError> {
        if recovery == GpuSurfaceRecovery::Lost {
            let surface = self
                .instance
                .create_surface(self.window.clone())
                .map_err(|error| GpuRenderError::SurfaceRecreation {
                    message: error.to_string(),
                })?;
            let capabilities = surface.get_capabilities(&self.adapter);
            if !capabilities.formats.contains(&self.config.format)
                || !capabilities.alpha_modes.contains(&self.config.alpha_mode)
            {
                return Err(GpuRenderError::SurfaceRecreation {
                    message: "replacement surface is incompatible with the active GPU pipelines"
                        .to_owned(),
                });
            }
            self.surface = surface;
            self.surface_generation = self.surface_generation.saturating_add(1);
        }
        self.surface.configure(&self.device, &self.config);
        self.surface_reconfigurations = self.surface_reconfigurations.saturating_add(1);
        Ok(())
    }

    fn handle_surface_signal(
        &mut self,
        signal: SurfaceAcquireSignal,
        timings: GpuFrameTimings,
    ) -> Result<GpuRenderOutcome, GpuRenderError> {
        match surface_acquire_directive(signal) {
            SurfaceAcquireDirective::Skip(reason) => {
                Ok(GpuRenderOutcome::Skipped { reason, timings })
            }
            SurfaceAcquireDirective::Recover(recovery) => {
                self.recover_surface(recovery)?;
                Ok(GpuRenderOutcome::SurfaceReconfigured { recovery, timings })
            }
            SurfaceAcquireDirective::FailValidation => Err(GpuRenderError::SurfaceValidation),
        }
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
        self.raster_cache_entries_high_water = self
            .raster_cache_entries_high_water
            .max(self.raster_cache.len());
        let cached_bytes = self
            .raster_cache
            .values()
            .map(|raster| raster.rgba8.len())
            .sum();
        self.raster_cache_bytes_high_water = self.raster_cache_bytes_high_water.max(cached_bytes);
    }

    fn sync_shaping_cache_palette(&mut self, palette: TerminalPalette) {
        if self
            .shaping_cache_palette
            .is_some_and(|previous| previous != palette)
        {
            self.shaping_cache.invalidate();
        }
        self.shaping_cache_palette = Some(palette);
    }

    fn shape_row_runs(
        &mut self,
        initial: Vec<RowRun>,
        terminal_metrics: Metrics,
    ) -> Result<Vec<ShapedRow>, GpuRenderError> {
        RowShapingPass {
            font_system: &mut self.font_system,
            row_buffers: &mut self.row_buffers,
            shaping_cache: &mut self.shaping_cache,
            fallback_report: &mut self.fallback_report,
            diagnostics: &mut self.row_run_diagnostics,
            font_profile: &self.font_profile,
            font_family: &self.font_family,
            cache_enabled: self.shaping_cache_enabled,
            terminal_metrics,
            cell_advance: self.cell_w,
            cell_height: self.cell_h,
            scale: self.scale,
            scale_generation: self.scale_generation,
        }
        .run(initial)
    }

    /// Render one frame from a `WorkspaceScene`. Consumes only scene types: the
    /// visible cells, styles, cursor/selection marks, and status come from the
    /// scene, never from a grid or parser. Returns the instant right after
    /// `present()` for input-to-present measurement. Transient occlusion and
    /// timeouts, deterministic surface recovery, scene-contract failures, and
    /// fatal device faults are distinct typed outcomes.
    pub fn render(
        &mut self,
        scene: &WorkspaceScene,
        theme: &Theme,
    ) -> Result<GpuRenderOutcome, GpuRenderError> {
        self.render_at(scene, theme, Instant::now())
    }

    /// Render at one caller-supplied monotonic instant.
    ///
    /// The product shell injects this time so animation tests and scheduling
    /// do not depend on hidden wall-clock reads inside presentation logic.
    pub fn render_at(
        &mut self,
        scene: &WorkspaceScene,
        theme: &Theme,
        visual_now: Instant,
    ) -> Result<GpuRenderOutcome, GpuRenderError> {
        if let Some(fault) = take_gpu_fault(&self.device_fault, self.device_generation) {
            return Err(fault);
        }
        let frame_prepare_started = Instant::now();
        let prepared = prepare_scene(scene, theme)?;
        let presentation_plan = self.presentation_motion.resolve(
            prepared.presentation_plan().clone(),
            scene.presentation.motion_policy,
            visual_now,
        );
        let program = prepare_cell_program(
            prepared.cell_program(),
            scene,
            theme,
            &presentation_plan,
        )?;
        let material_quads = prepare_material_quads(
            &presentation_plan,
            self.scale,
            self.config.width,
            self.config.height,
        )?;
        let mut material_instances =
            Vec::with_capacity(material_quads.len().saturating_mul(MaterialQuad::FLOATS));
        for quad in &material_quads {
            quad.write_instance(&mut material_instances);
        }
        if material_instances.len() > self.material_inst_capacity_floats {
            self.material_inst_capacity_floats = material_instances.len().next_power_of_two();
            self.material_inst_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("native-material-instances"),
                size: (self.material_inst_capacity_floats * 4) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !material_instances.is_empty() {
            self.queue.write_buffer(
                &self.material_inst_buf,
                0,
                bytes_of_slice(&material_instances),
            );
        }
        let frame_colors = native_frame_colors(theme);
        self.sync_raster_cache(prepared.artifacts());
        self.sync_shaping_cache_palette(theme.terminal_palette);
        let metrics = Metrics::new(self.font_size, self.cell_h);
        for issue in &program.issues {
            record_row_run_diagnostic(
                &mut self.row_run_diagnostics,
                format!("row-run build issue: {issue:?}"),
            );
        }
        let shaping_started = Instant::now();
        let rows = self.shape_row_runs(program.rows, metrics)?;
        let shaping = shaping_started.elapsed();

        // The cell compiler has already applied pane order, opacity, chrome,
        // content, overlay, selection, and cursor semantics. The GPU adapter
        // only translates final topmost cells into solid backgrounds and
        // glyphon rows.
        let mut quads = Vec::with_capacity(program.cells.len().saturating_mul(8));
        let mut foreground_quads = Vec::new();
        for (x, y, cell, background_visible, scope, cursor) in &program.cells {
            if !background_visible {
                continue;
            }
            let target = if *cursor
                && matches!(
                    scope,
                    TextPaintScopeKind::Overlay | TextPaintScopeKind::TextInput
                ) {
                &mut foreground_quads
            } else {
                &mut quads
            };
            push_quad(
                target,
                f32::from(*x) * self.cell_w,
                f32::from(*y) * self.cell_h,
                self.cell_w,
                self.cell_h,
                cell.background,
            );
        }
        let base_instance_count = (quads.len() / 8) as u32;
        quads.extend(foreground_quads);

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

        let text_areas = rows.iter().map(|row| {
            let profile = row_shaping_profile(&row.row, metrics, self.cell_w, self.scale);
            let area = row_text_area_geometry(
                &row.row,
                profile,
                self.cell_w,
                self.cell_h,
                self.scale,
                self.config.width,
                self.config.height,
            );
            TextArea {
                buffer: match &row.buffer {
                    ShapedBuffer::Shared(buffer) => buffer.as_ref(),
                    ShapedBuffer::RowPool(index) => &self.row_buffers.rows[*index],
                },
                left: area.left,
                top: area.top,
                scale: 1.0,
                bounds: area.bounds,
                default_color: GColor::rgb(
                    frame_colors.default_foreground[0],
                    frame_colors.default_foreground[1],
                    frame_colors.default_foreground[2],
                ),
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
            if let Some(fault) = take_gpu_fault(&self.device_fault, self.device_generation) {
                return Err(fault);
            }
            return Err(GpuRenderError::TextAtlasFull);
        }
        let frame_prepare = frame_prepare_started.elapsed();
        let timings = GpuFrameTimings {
            shaping,
            frame_prepare,
        };

        if let Some(fault) = take_gpu_fault(&self.device_fault, self.device_generation) {
            return Err(fault);
        }
        let (frame, reconfigure_after_present) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Timeout => {
                return self.handle_surface_signal(SurfaceAcquireSignal::Timeout, timings);
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                return self.handle_surface_signal(SurfaceAcquireSignal::Occluded, timings);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                return self.handle_surface_signal(SurfaceAcquireSignal::Outdated, timings);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return self.handle_surface_signal(SurfaceAcquireSignal::Lost, timings);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return self.handle_surface_signal(SurfaceAcquireSignal::Validation, timings);
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let text_render_error = {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: frame_colors.clear_background[0] as f64 / 255.0,
                            g: frame_colors.clear_background[1] as f64 / 255.0,
                            b: frame_colors.clear_background[2] as f64 / 255.0,
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
            if !material_quads.is_empty() {
                pass.set_pipeline(&self.material_pipeline);
                pass.set_bind_group(0, &self.res_bind_group, &[]);
                pass.set_vertex_buffer(0, self.unit_buf.slice(..));
                pass.set_vertex_buffer(1, self.material_inst_buf.slice(..));
                for (instance, quad) in material_quads.iter().enumerate() {
                    if quad.shadow
                        || quad.role == crate::NativeMaterialRole::ModalScrim
                        || is_overlay_material(quad.role)
                    {
                        continue;
                    }
                    let (x, y, width, height) = quad.clip;
                    pass.set_scissor_rect(x, y, width, height);
                    pass.draw(0..4, instance as u32..instance as u32 + 1);
                }
                pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
            }
            if base_instance_count > 0 {
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_bind_group(0, &self.res_bind_group, &[]);
                pass.set_vertex_buffer(0, self.unit_buf.slice(..));
                pass.set_vertex_buffer(1, self.inst_buf.slice(..));
                pass.draw(0..4, 0..base_instance_count);
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
            if material_quads.iter().any(|quad| {
                quad.shadow || quad.role == crate::NativeMaterialRole::ModalScrim
                    || is_overlay_material(quad.role)
            }) {
                pass.set_pipeline(&self.material_pipeline);
                pass.set_bind_group(0, &self.res_bind_group, &[]);
                pass.set_vertex_buffer(0, self.unit_buf.slice(..));
                pass.set_vertex_buffer(1, self.material_inst_buf.slice(..));

                // Raised workspace surfaces are base content and therefore
                // sit below a modal scrim.
                for (instance, quad) in material_quads.iter().enumerate() {
                    if !quad.shadow || is_overlay_material(quad.role) {
                        continue;
                    }
                    let (x, y, width, height) = quad.clip;
                    pass.set_scissor_rect(x, y, width, height);
                    pass.draw(0..4, instance as u32..instance as u32 + 1);
                }

                // The scrim dims completed workspace materials, terminal
                // backgrounds, and raster artifacts. Base glyph colors are
                // composited by `apply_modal_scrim_to_base_text` because the
                // shared glyphon pass occurs last.
                for (instance, quad) in material_quads.iter().enumerate() {
                    if quad.shadow || quad.role != crate::NativeMaterialRole::ModalScrim {
                        continue;
                    }
                    let (x, y, width, height) = quad.clip;
                    pass.set_scissor_rect(x, y, width, height);
                    pass.draw(0..4, instance as u32..instance as u32 + 1);
                }

                // Overlay elevation belongs above the scrim but below the
                // raised shell and its bands/selection materials.
                for (instance, quad) in material_quads.iter().enumerate() {
                    if !quad.shadow || !is_overlay_material(quad.role) {
                        continue;
                    }
                    let (x, y, width, height) = quad.clip;
                    pass.set_scissor_rect(x, y, width, height);
                    pass.draw(0..4, instance as u32..instance as u32 + 1);
                }
                for (instance, quad) in material_quads.iter().enumerate() {
                    if quad.shadow || !is_overlay_material(quad.role) {
                        continue;
                    }
                    let (x, y, width, height) = quad.clip;
                    pass.set_scissor_rect(x, y, width, height);
                    pass.draw(0..4, instance as u32..instance as u32 + 1);
                }
                pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
            }
            if instance_count > base_instance_count {
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_bind_group(0, &self.res_bind_group, &[]);
                pass.set_vertex_buffer(0, self.unit_buf.slice(..));
                pass.set_vertex_buffer(1, self.inst_buf.slice(..));
                pass.draw(0..4, base_instance_count..instance_count);
            }
            let result = self
                .text_renderer
                .render(&self.atlas, &self.viewport, &mut pass);
            result.err()
        };
        if let Some(error) = text_render_error {
            return Err(GpuRenderError::TextRender {
                message: error.to_string(),
            });
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        let present = Instant::now();
        if reconfigure_after_present {
            self.recover_surface(GpuSurfaceRecovery::Outdated)?;
        }
        Ok(GpuRenderOutcome::Presented {
            at: present,
            timings,
        })
    }

    pub fn next_animation_deadline(&self) -> Option<Instant> {
        self.presentation_motion.next_deadline()
    }

    pub fn animation_is_active(&self) -> bool {
        self.presentation_motion.is_active()
    }

    pub fn pointer_geometry_is_moving(&self) -> bool {
        self.presentation_motion.pointer_geometry_is_moving()
    }

    pub fn active_transition_window(
        &self,
        role: TransitionRole,
    ) -> Option<ActiveTransitionWindow> {
        self.presentation_motion.active_transition_window(role)
    }

    pub fn snap_presentation_motion(&mut self) {
        self.presentation_motion.snap();
    }
}

/// Identify one shaping candidate for retention.
///
/// Anchored candidates are cacheable, but only inside their own key
/// namespace: an anchored buffer was never admitted, so it must never satisfy
/// an ordinary lookup.
fn shaping_cache_key_for_candidate(
    cache_enabled: bool,
    forced_anchor: bool,
    run: &RowRun,
    context: ShapingCacheContext,
) -> Option<ShapingCacheKey> {
    cache_enabled.then(|| ShapingCacheKey::from_run(run, context, forced_anchor))
}

/// Map a scene color onto RGB, using the given default for
/// `SceneColor::Default`, the standard xterm palette for ANSI/indexed colors,
/// and a passthrough for direct RGB.
fn resolve(color: SceneColor, default: [u8; 3], terminal_palette: &TerminalPalette) -> [u8; 3] {
    match color {
        SceneColor::Default => default,
        SceneColor::Rgb(r, g, b) => [r, g, b],
        SceneColor::Ansi(i @ 0..=15) => terminal_palette.ansi[i as usize],
        SceneColor::Ansi(i) | SceneColor::Indexed(i) => indexed_palette(i),
    }
}

fn indexed_palette(i: u8) -> [u8; 3] {
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
    width: u16,
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
        right: pixel_boundary(left + f32::from(width.max(1)) * cell_width, surface_width) as i32,
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

const MATERIAL_WGSL: &str = r#"
struct Res { size: vec4<f32> };
@group(0) @binding(0) var<uniform> res: Res;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) pixel: vec2<f32>,
    @location(1) shape: vec4<f32>,
    @location(2) fill: vec4<f32>,
    @location(3) boundary: vec4<f32>,
    @location(4) params: vec4<f32>,
};

@vertex
fn vs(@location(0) unit: vec2<f32>,
      @location(1) draw_rect: vec4<f32>,
      @location(2) shape_rect: vec4<f32>,
      @location(3) fill: vec4<f32>,
      @location(4) boundary: vec4<f32>,
      @location(5) params: vec4<f32>) -> VOut {
    let px = draw_rect.xy + unit * draw_rect.zw;
    let ndc = vec2<f32>(px.x / res.size.x * 2.0 - 1.0, 1.0 - px.y / res.size.y * 2.0);
    var out: VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.pixel = px;
    out.shape = shape_rect;
    out.fill = fill;
    out.boundary = boundary;
    out.params = params;
    return out;
}

fn rounded_box_distance(pixel: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let half_size = rect.zw * 0.5;
    let center = rect.xy + half_size;
    let bounded_radius = min(max(radius, 0.0), min(half_size.x, half_size.y));
    let q = abs(pixel - center) - (half_size - vec2<f32>(bounded_radius));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - bounded_radius;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let distance = rounded_box_distance(in.pixel, in.shape, in.params.x);
    let blur = max(in.params.z, 0.0);
    if in.params.w > 0.5 {
        let softness = max(blur, 0.75);
        let outside = smoothstep(-0.75, 0.75, distance);
        let alpha = (1.0 - smoothstep(0.0, softness, distance)) * outside;
        return vec4<f32>(in.fill.rgb, in.fill.a * alpha);
    }

    let edge_alpha = 1.0 - smoothstep(-0.75, 0.75, distance);
    let boundary_width = max(in.params.y, 0.0);
    var color = in.fill;
    if boundary_width > 0.0 {
        let boundary_mix = smoothstep(-boundary_width - 0.75, -boundary_width + 0.75, distance);
        color = mix(in.fill, in.boundary, boundary_mix);
    }
    return vec4<f32>(color.rgb, color.a * edge_alpha);
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
        ArtifactContent, ArtifactFit, ArtifactState, BackingScale, EmptyContent, HeaderScene,
        LogicalSize, OverlayScene, PaletteOverlay, PaneContent, PaneId, PaneScene, PaneSceneKind,
        PhysicalSize, PresentationNode, PresentationNodeId, PresentationNodeRole,
        PresentationNodeState, RasterSurface, SceneCell, ScenePresentation, SceneRect, SceneSize,
        StatusScene, TerminalProjection, TerminalSurface, TerminalViewportMapping, ViewportMetrics,
        WorkspaceNodePart,
    };

    #[test]
    fn native_material_plan_converts_logical_geometry_to_clipped_physical_quads() {
        let mut scene = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let logical_rect = LogicalRect::from_units(4 * 64, 6 * 64, 200 * 64, 100 * 64);
        scene.presentation = ScenePresentation {
            viewport: Some(
                ViewportMetrics::new(
                    LogicalSize::from_units(800 * 64, 600 * 64),
                    PhysicalSize::new(1_600, 1_200),
                    BackingScale::new(2.0).unwrap(),
                    LogicalSize::from_units(8 * 64, 20 * 64),
                )
                .unwrap(),
            ),
            nodes: vec![PresentationNode {
                id: PresentationNodeId::workspace(WorkspaceNodePart::Surface),
                parent: None,
                role: PresentationNodeRole::Pane,
                state: PresentationNodeState {
                    floating: true,
                    ..PresentationNodeState::default()
                },
                logical_rect,
                cell_rect: Some(SceneRect::new(0, 0, 25, 5)),
                terminal_projection: TerminalProjection::CellRegions(vec![SceneRect::new(
                    0, 0, 25, 5,
                )]),
            }],
            ..ScenePresentation::default()
        };

        let plan = prepare_native_presentation(&scene, &Theme::default()).unwrap();
        let quads = prepare_material_quads(&plan, 2.0, 1_600, 1_200).unwrap();

        assert_eq!(quads.len(), 3);
        assert!(quads[0].shadow);
        assert!(quads[1].shadow);
        assert!(quads[0].draw_rect.width > quads[0].shape_rect.width);
        assert!(quads[1].draw_rect.height > quads[1].shape_rect.height);
        let surface = quads[2];
        assert!(!surface.shadow);
        assert_eq!(
            surface.draw_rect,
            PixelRect {
                x: 8.0,
                y: 12.0,
                width: 400.0,
                height: 200.0,
            }
        );
        assert_eq!(surface.shape_rect, surface.draw_rect);
        assert_eq!(surface.clip, (8, 12, 400, 200));
        assert_eq!(
            surface.fill,
            ui_color_f32(Theme::default().ui.palette.pane_surface)
        );
        assert_eq!(surface.corner_radius, 20.0);
        assert_eq!(surface.boundary_width, 2.0);
        assert_eq!(surface.blur_radius, 0.0);
    }

    #[test]
    fn shadow_scissors_exclude_later_floating_surfaces_without_losing_surrounding_shadow() {
        let base = (0, 0, 100, 100);
        let later_floating_surface = (25, 25, 50, 50);
        let visible = subtract_scissor(base, later_floating_surface);

        assert_eq!(visible.len(), 4);
        assert!(
            visible
                .iter()
                .all(|piece| intersect_scissors(*piece, later_floating_surface).is_none())
        );
        assert_eq!(
            visible
                .iter()
                .map(|(_, _, width, height)| width * height)
                .sum::<u32>(),
            7_500
        );
    }

    #[test]
    fn semantic_materials_replace_only_default_nonterminal_backgrounds() {
        let mut scene = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let pane_id = scene.panes[0].id.clone();
        scene.presentation.terminal_viewports = vec![TerminalViewportMapping {
            node_id: PresentationNodeId::pane(
                pane_id.clone(),
                mandatum_scene::PaneNodePart::Output,
            ),
            pane_id,
            pty_size: SceneSize::new(2, 1),
            visible_cell_rect: SceneRect::new(1, 2, 2, 1),
            logical_rect: LogicalRect::from_units(8 * 64, 40 * 64, 16 * 64, 20 * 64),
            first_visible_surface_row: 0,
        }];
        let default_cell = ProgramCell {
            occupancy: CellOccupancy::Grapheme(" ".to_owned()),
            style: mandatum_scene::SceneCellStyle::default(),
            selection: None,
            cursor: false,
            raster_layer: None,
        };

        assert!(!should_paint_legacy_background(
            &scene,
            0,
            0,
            &default_cell,
            TextPaintScopeKind::Header
        ));
        assert!(!should_paint_legacy_background(
            &scene,
            0,
            1,
            &default_cell,
            TextPaintScopeKind::PaneChrome
        ));
        assert!(should_paint_legacy_background(
            &scene,
            1,
            2,
            &default_cell,
            TextPaintScopeKind::PaneContent
        ));
        assert!(!should_paint_legacy_background(
            &scene,
            4,
            2,
            &default_cell,
            TextPaintScopeKind::PaneContent
        ));

        let cursor_cell = ProgramCell {
            cursor: true,
            ..default_cell.clone()
        };
        assert!(should_paint_legacy_background(
            &scene,
            4,
            2,
            &cursor_cell,
            TextPaintScopeKind::PaneContent
        ));
        let item_selection = ProgramCell {
            selection: Some(CellSelection::Item),
            ..default_cell.clone()
        };
        assert!(
            !should_paint_legacy_background(
                &scene,
                1,
                2,
                &item_selection,
                TextPaintScopeKind::Overlay
            ),
            "native soft-selection material owns overlay item backgrounds"
        );
        let terminal_selection = ProgramCell {
            selection: Some(CellSelection::Terminal),
            ..default_cell.clone()
        };
        assert!(should_paint_legacy_background(
            &scene,
            1,
            2,
            &terminal_selection,
            TextPaintScopeKind::PaneContent
        ));
        assert!(!should_paint_legacy_background(
            &scene,
            1,
            2,
            &default_cell,
            TextPaintScopeKind::OverlayDecoration
        ));
        let custom_background = ProgramCell {
            style: mandatum_scene::SceneCellStyle {
                background: SceneColor::Rgb(12, 34, 56),
                ..mandatum_scene::SceneCellStyle::default()
            },
            ..default_cell
        };
        assert!(should_paint_legacy_background(
            &scene,
            4,
            2,
            &custom_background,
            TextPaintScopeKind::PaneContent
        ));
        assert!(
            !should_paint_legacy_background(
                &scene,
                0,
                0,
                &custom_background,
                TextPaintScopeKind::Header
            ),
            "native chrome material owns even explicitly colored legacy header cells"
        );

        let mut modern_without_terminal_projection = scene.clone();
        modern_without_terminal_projection
            .presentation
            .terminal_viewports
            .clear();
        modern_without_terminal_projection.presentation.viewport = Some(
            ViewportMetrics::new(
                LogicalSize::from_units(640 * 64, 480 * 64),
                PhysicalSize::new(1_280, 960),
                BackingScale::new(2.0).unwrap(),
                LogicalSize::from_units(8 * 64, 20 * 64),
            )
            .unwrap(),
        );
        assert!(!should_paint_legacy_background(
            &modern_without_terminal_projection,
            1,
            2,
            &ProgramCell {
                occupancy: CellOccupancy::Grapheme(" ".to_owned()),
                style: mandatum_scene::SceneCellStyle::default(),
                selection: None,
                cursor: false,
                raster_layer: None,
            },
            TextPaintScopeKind::PaneContent
        ));

        let mut legacy_fixture = modern_without_terminal_projection;
        legacy_fixture.presentation = mandatum_scene::ScenePresentation::default();
        assert!(should_paint_legacy_background(
            &legacy_fixture,
            1,
            2,
            &ProgramCell {
                occupancy: CellOccupancy::Grapheme(" ".to_owned()),
                style: mandatum_scene::SceneCellStyle::default(),
                selection: None,
                cursor: false,
                raster_layer: None,
            },
            TextPaintScopeKind::PaneContent
        ));
    }

    #[test]
    fn app_owned_chrome_glyphs_use_ui_palette_and_typed_tones_without_label_parsing() {
        let mut scene = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let pane_id = scene.panes[0].id.clone();
        let badge_rect = scene.panes[0]
            .badge_rects()
            .into_iter()
            .find(|(kind, _)| *kind == mandatum_scene::PaneBadgeKind::Terminal)
            .expect("terminal badge rect")
            .1;
        let make_node = |id: PresentationNodeId,
                         role: PresentationNodeRole,
                         state: PresentationNodeState,
                         rect: SceneRect| PresentationNode {
            id,
            parent: None,
            role,
            state,
            logical_rect: LogicalRect::from_units(
                i64::from(rect.x) * 8 * 64,
                i64::from(rect.y) * 20 * 64,
                u64::from(rect.width) * 8 * 64,
                u64::from(rect.height) * 20 * 64,
            ),
            cell_rect: Some(rect),
            terminal_projection: TerminalProjection::CellRegions(vec![rect]),
        };
        scene.presentation.nodes = vec![
            make_node(
                PresentationNodeId::workspace(WorkspaceNodePart::Header),
                PresentationNodeRole::Header,
                PresentationNodeState::default(),
                scene.header.area,
            ),
            make_node(
                PresentationNodeId::workspace(WorkspaceNodePart::Status),
                PresentationNodeRole::Status,
                PresentationNodeState::default(),
                scene.status.area,
            ),
            make_node(
                PresentationNodeId::pane(pane_id.clone(), mandatum_scene::PaneNodePart::Title),
                PresentationNodeRole::PaneTitle,
                PresentationNodeState {
                    focused: true,
                    ..PresentationNodeState::default()
                },
                SceneRect::new(0, 1, 80, 1),
            ),
            make_node(
                PresentationNodeId::pane(
                    pane_id,
                    mandatum_scene::PaneNodePart::Badge(mandatum_scene::PaneBadgeKind::Terminal),
                ),
                PresentationNodeRole::PaneBadge(mandatum_scene::PaneBadgeKind::Terminal),
                PresentationNodeState {
                    tone: mandatum_scene::PresentationTone::AgentIdentity,
                    ..PresentationNodeState::default()
                },
                badge_rect,
            ),
        ];
        scene.presentation.viewport = Some(
            ViewportMetrics::new(
                LogicalSize::from_units(640 * 64, 480 * 64),
                PhysicalSize::new(1_280, 960),
                BackingScale::new(2.0).unwrap(),
                LogicalSize::from_units(8 * 64, 20 * 64),
            )
            .unwrap(),
        );
        let theme = Theme::default();
        let program = compile_cell_program(&scene, &theme);
        let presentation_plan = prepare_native_presentation(&scene, &theme).unwrap();
        let translated =
            prepare_cell_program(&program, &scene, &theme, &presentation_plan).unwrap();
        assert!(
            translated
                .rows
                .iter()
                .all(|run| run.paint_scope.kind != TextPaintScopeKind::PaneDecoration),
            "typed terminal-parity pane decoration never reaches native shaping"
        );
        let header = translated
            .rows
            .iter()
            .find(|run| run.paint_scope.kind == TextPaintScopeKind::Header)
            .expect("header row");
        let status = translated
            .rows
            .iter()
            .find(|run| run.paint_scope.kind == TextPaintScopeKind::Status)
            .expect("status row");
        let title = translated
            .rows
            .iter()
            .find(|run| run.paint_scope.kind == TextPaintScopeKind::PaneChrome)
            .expect("pane title row");
        let badge = translated
            .rows
            .iter()
            .find(|run| {
                run.paint_scope.kind == TextPaintScopeKind::PaneChrome
                    && run.x >= badge_rect.x
                    && run.x < badge_rect.right()
            })
            .expect("aligned badge glyph row");

        assert_eq!(
            header.style_ranges[0].style.foreground,
            theme.ui.palette.text_primary.to_array()
        );
        assert_eq!(
            status.style_ranges[0].style.foreground,
            theme.ui.palette.text_secondary.to_array()
        );
        assert_eq!(
            title.style_ranges[0].style.foreground,
            theme.ui.palette.focus.to_array()
        );
        assert_eq!(
            badge.style_ranges[0].style.foreground,
            theme.ui.palette.agent_identity.to_array(),
            "the aligned badge cell rect must override the title rail glyph tone"
        );
    }

    #[test]
    fn app_owned_text_uses_scope_metrics_without_changing_terminal_metrics() {
        let terminal = PaneContent::Terminal(TerminalSurface {
            rows: vec![vec![SceneCell::grapheme(
                "X",
                mandatum_scene::SceneCellStyle::default(),
            )]],
            ..TerminalSurface::default()
        });
        let mut scene = scene(vec![pane(PaneSceneKind::Terminal, terminal)]);
        let pane_id = scene.panes[0].id.clone();
        let header_rect = scene.header.area;
        let title_rect = SceneRect::new(0, 1, scene.size.width, 1);
        let make_node = |id: PresentationNodeId,
                         role: PresentationNodeRole,
                         state: PresentationNodeState,
                         rect: SceneRect| PresentationNode {
            id,
            parent: None,
            role,
            state,
            logical_rect: LogicalRect::from_units(
                i64::from(rect.x) * 8 * 64,
                i64::from(rect.y) * 20 * 64,
                u64::from(rect.width) * 8 * 64,
                u64::from(rect.height) * 20 * 64,
            ),
            cell_rect: Some(rect),
            terminal_projection: TerminalProjection::CellRegions(vec![rect]),
        };
        scene.presentation.nodes = vec![
            make_node(
                PresentationNodeId::workspace(WorkspaceNodePart::Header),
                PresentationNodeRole::Header,
                PresentationNodeState::default(),
                header_rect,
            ),
            make_node(
                PresentationNodeId::pane(pane_id, PaneNodePart::Title),
                PresentationNodeRole::PaneTitle,
                PresentationNodeState {
                    focused: true,
                    ..PresentationNodeState::default()
                },
                title_rect,
            ),
        ];
        scene.presentation.nodes[0].logical_rect =
            LogicalRect::from_units(0, 0, 640 * 64, 40 * 64);
        scene.presentation.viewport = Some(
            ViewportMetrics::new(
                LogicalSize::from_units(640 * 64, 480 * 64),
                PhysicalSize::new(1_280, 960),
                BackingScale::new(2.0).unwrap(),
                LogicalSize::from_units(8 * 64, 20 * 64),
            )
            .unwrap(),
        );

        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();

        let header = translated
            .rows
            .iter()
            .find(|run| run.paint_scope.kind == TextPaintScopeKind::Header)
            .expect("header row");
        assert_eq!(
            header.native_metrics,
            Some(
                prepared
                    .presentation_plan()
                    .commands()
                    .iter()
                    .find_map(|command| match command {
                        NativePlanCommand::Text(scope)
                            if scope.metrics.role == crate::NativeTextMetricRole::Title =>
                        {
                            Some(scope.metrics)
                        }
                        _ => None,
                    })
                    .expect("title metric scope")
            )
        );
        assert!(header.glyph_style.bold);
        assert!(!header.glyph_style.italic);
        assert_eq!(
            header.clipped_cell_bounds(),
            Some(SceneRect::new(
                header.x,
                header.y,
                header.width,
                1
            ))
        );

        let terminal = translated
            .rows
            .iter()
            .find(|run| run.paint_scope.kind == TextPaintScopeKind::PaneContent)
            .expect("terminal output row");
        assert_eq!(
            terminal.native_metrics, None,
            "child terminal output keeps the renderer's configured terminal metrics"
        );

        let configured_terminal = Metrics::new(36.0, 47.0);
        let header_profile = row_shaping_profile(header, configured_terminal, 18.0, 2.0);
        assert_eq!(header_profile.metrics, Metrics::new(26.0, 36.0));
        assert_eq!(header_profile.cell_advance, 13.0);
        assert_eq!(
            header_profile.metric_generation,
            header.native_metrics.unwrap().generation
        );
        assert_eq!(
            header_profile.metric_slot,
            crate::NativeTextMetricRole::Title as u8
        );
        let header_area =
            row_text_area_geometry(header, header_profile, 18.0, 47.0, 2.0, 1_280, 960);
        assert_eq!(header_area.top, 22.0);
        assert_eq!((header_area.bounds.top, header_area.bounds.bottom), (0, 80));
        let mut right_aligned = header.clone();
        right_aligned.x = 60;
        let right_aligned_area =
            row_text_area_geometry(&right_aligned, header_profile, 18.0, 47.0, 2.0, 1_280, 960);
        assert_eq!(
            right_aligned_area.left, 1_080.0,
            "semantic alignment keeps the declared terminal-grid start"
        );

        let terminal_profile = row_shaping_profile(terminal, configured_terminal, 18.0, 2.0);
        assert_eq!(terminal_profile.metrics, configured_terminal);
        assert_eq!(terminal_profile.cell_advance, 18.0);
        assert_eq!(terminal_profile.metric_generation, 0);
        assert_eq!(terminal_profile.metric_slot, 0);
        let terminal_area =
            row_text_area_geometry(terminal, terminal_profile, 18.0, 47.0, 2.0, 1_280, 960);
        assert_eq!(terminal_area.top, f32::from(terminal.y) * 47.0);
        assert_eq!(
            terminal_area.bounds.bottom - terminal_area.bounds.top,
            47
        );
    }

    #[test]
    fn native_role_faces_add_to_cell_emphasis_instead_of_erasing_it() {
        let mut style = ResolvedGlyphStyle {
            foreground: [1, 2, 3, 255],
            bold: true,
            italic: true,
            underline: false,
            strikethrough: false,
        };

        add_native_face(&mut style, crate::NativeFontFace::Regular);

        assert!(style.bold);
        assert!(style.italic);
    }

    #[test]
    fn multirow_native_scope_preserves_distinct_nonoverlapping_row_tops() {
        let terminal = PaneContent::Terminal(TerminalSurface {
            rows: vec![
                vec![SceneCell::grapheme(
                    "X",
                    mandatum_scene::SceneCellStyle::default(),
                )],
                vec![SceneCell::grapheme(
                    "Y",
                    mandatum_scene::SceneCellStyle::default(),
                )],
            ],
            ..TerminalSurface::default()
        });
        let scene = scene(vec![pane(PaneSceneKind::Terminal, terminal)]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let inner = layout::pane_inner_rect(scene.panes[0].area);
        let logical_rect = LogicalRect::from_units(
            i64::from(inner.x) * 8 * 64,
            i64::from(inner.y) * 20 * 64,
            8 * 64,
            40 * 64,
        );
        let metrics = crate::NativeTextMetricSet::from_theme(&theme, 77)
            .identity(crate::NativeTextMetricRole::Body);
        let plan = NativePresentationPlan::from_resolved_commands(
            vec![NativePlanCommand::Text(crate::NativeTextScope {
                node_id: PresentationNodeId::workspace(WorkspaceNodePart::Header),
                logical_rect,
                cell_rect: Some(SceneRect::new(inner.x, inner.y, 1, 2)),
                clip: logical_rect,
                color: theme.ui.palette.text_primary,
                metrics,
                z_order: 0,
            })],
            Vec::new(),
        );
        let translated =
            prepare_cell_program(prepared.cell_program(), &scene, &theme, &plan).unwrap();
        let first = translated
            .rows
            .iter()
            .find(|row| row.text == "X")
            .expect("first scoped row");
        let second = translated
            .rows
            .iter()
            .find(|row| row.text == "Y")
            .expect("second scoped row");
        let terminal_metrics = Metrics::new(30.0, 40.0);
        let first_profile = row_shaping_profile(first, terminal_metrics, 16.0, 2.0);
        let second_profile = row_shaping_profile(second, terminal_metrics, 16.0, 2.0);
        let first_area =
            row_text_area_geometry(first, first_profile, 16.0, 40.0, 2.0, 1_280, 960);
        let second_area =
            row_text_area_geometry(second, second_profile, 16.0, 40.0, 2.0, 1_280, 960);

        assert!(first_area.top < second_area.top);
        assert!(
            first_area.top + first_profile.metrics.line_height <= second_area.top,
            "multirow line boxes must not overlap: {first_area:?} then {second_area:?}"
        );
        assert!(first_area.bounds.bottom <= second_area.bounds.top);
    }

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
    fn startup_failures_have_stable_visible_classifications() {
        for (stage, kind, label) in [
            (
                StartupFailureStage::Surface,
                GpuStartupErrorKind::NoDisplay,
                "no display",
            ),
            (
                StartupFailureStage::Adapter,
                GpuStartupErrorKind::NoAdapter,
                "no GPU adapter",
            ),
            (
                StartupFailureStage::Device,
                GpuStartupErrorKind::DeviceRequest,
                "GPU device request failed",
            ),
            (
                StartupFailureStage::Configuration,
                GpuStartupErrorKind::InvalidConfiguration,
                "invalid GPU configuration",
            ),
        ] {
            let error = startup_error(stage, "fault injected");
            assert_eq!(error.kind(), kind);
            assert_eq!(error.message(), "fault injected");
            assert_eq!(error.to_string(), format!("{label}: fault injected"));
        }

        let no_window = GpuStartupError::no_display("window creation failed");
        assert_eq!(no_window.kind(), GpuStartupErrorKind::NoDisplay);
        assert_eq!(
            no_window.to_string(),
            "no display: window creation failed".to_owned()
        );
    }

    #[test]
    fn surface_acquire_faults_map_to_deterministic_retry_policy() {
        assert_eq!(
            surface_acquire_directive(SurfaceAcquireSignal::Timeout),
            SurfaceAcquireDirective::Skip(GpuFrameSkip::Timeout)
        );
        assert_eq!(
            surface_acquire_directive(SurfaceAcquireSignal::Occluded),
            SurfaceAcquireDirective::Skip(GpuFrameSkip::Occluded)
        );
        assert_eq!(
            surface_acquire_directive(SurfaceAcquireSignal::Outdated),
            SurfaceAcquireDirective::Recover(GpuSurfaceRecovery::Outdated)
        );
        assert_eq!(
            surface_acquire_directive(SurfaceAcquireSignal::Lost),
            SurfaceAcquireDirective::Recover(GpuSurfaceRecovery::Lost)
        );
        assert_eq!(
            surface_acquire_directive(SurfaceAcquireSignal::Validation),
            SurfaceAcquireDirective::FailValidation
        );
        #[cfg(feature = "fault-injection")]
        {
            assert_eq!(
                injected_surface_recovery(GpuFaultInjection::SurfaceOutdated),
                Some(GpuSurfaceRecovery::Outdated)
            );
            assert_eq!(
                injected_surface_recovery(GpuFaultInjection::SurfaceLost),
                Some(GpuSurfaceRecovery::Lost)
            );
            assert_eq!(
                injected_surface_recovery(GpuFaultInjection::DeviceLost),
                None
            );
            assert_eq!(
                injected_surface_recovery(GpuFaultInjection::OutOfMemory),
                None
            );
        }
    }

    #[test]
    fn uncaptured_out_of_memory_and_device_loss_remain_explicit() {
        assert_eq!(
            uncaptured_gpu_error(
                UncapturedErrorKind::OutOfMemory,
                "heap exhausted".to_owned()
            ),
            GpuRenderError::OutOfMemory {
                message: "heap exhausted".to_owned()
            }
        );
        assert_eq!(
            uncaptured_gpu_error(UncapturedErrorKind::Validation, "bad binding".to_owned()),
            GpuRenderError::Validation {
                message: "bad binding".to_owned()
            }
        );
        assert_eq!(
            uncaptured_gpu_error(UncapturedErrorKind::Internal, "driver fault".to_owned()),
            GpuRenderError::Internal {
                message: "driver fault".to_owned()
            }
        );
        assert_eq!(
            device_lost_error(wgpu::DeviceLostReason::Unknown, "reset".to_owned()),
            GpuRenderError::DeviceLost {
                reason: GpuDeviceLossReason::Unknown,
                message: "reset".to_owned()
            }
        );
    }

    #[test]
    fn higher_priority_fault_wins_in_both_arrival_orders() {
        let slot = Arc::new(Mutex::new(GpuFaultState {
            active_device_generation: 7,
            pending: None,
        }));
        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::Validation {
                message: "validation first".to_owned(),
            },
        );
        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::OutOfMemory {
                message: "oom second".to_owned(),
            },
        );
        assert_eq!(
            take_gpu_fault(&slot, 7),
            Some(GpuRenderError::OutOfMemory {
                message: "oom second".to_owned()
            })
        );

        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::OutOfMemory {
                message: "oom first".to_owned(),
            },
        );
        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::Validation {
                message: "validation second".to_owned(),
            },
        );
        assert_eq!(
            take_gpu_fault(&slot, 7),
            Some(GpuRenderError::OutOfMemory {
                message: "oom first".to_owned()
            })
        );

        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::Internal {
                message: "internal first".to_owned(),
            },
        );
        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::DeviceLost {
                reason: GpuDeviceLossReason::Unknown,
                message: "device second".to_owned(),
            },
        );
        assert_eq!(
            take_gpu_fault(&slot, 7),
            Some(GpuRenderError::DeviceLost {
                reason: GpuDeviceLossReason::Unknown,
                message: "device second".to_owned()
            })
        );

        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::DeviceLost {
                reason: GpuDeviceLossReason::Destroyed,
                message: "device first".to_owned(),
            },
        );
        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::Internal {
                message: "internal second".to_owned(),
            },
        );
        assert_eq!(
            take_gpu_fault(&slot, 7),
            Some(GpuRenderError::DeviceLost {
                reason: GpuDeviceLossReason::Destroyed,
                message: "device first".to_owned()
            })
        );
    }

    #[test]
    fn generation_stamped_faults_reject_stale_destroyed_callbacks() {
        let slot = Arc::new(Mutex::new(GpuFaultState {
            active_device_generation: 7,
            pending: None,
        }));
        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::Validation {
                message: "active validation".to_owned(),
            },
        );
        record_gpu_fault(
            &slot,
            6,
            GpuRenderError::OutOfMemory {
                message: "stale oom".to_owned(),
            },
        );

        assert_eq!(
            take_gpu_fault(&slot, 7),
            Some(GpuRenderError::Validation {
                message: "active validation".to_owned()
            })
        );
        assert_eq!(take_gpu_fault(&slot, 7), None);

        retire_gpu_generation(&slot, 8);
        record_gpu_fault(
            &slot,
            7,
            GpuRenderError::DeviceLost {
                reason: GpuDeviceLossReason::Destroyed,
                message: "stale old device".to_owned(),
            },
        );
        assert!(!has_gpu_fault(&slot, 8));

        record_gpu_fault(
            &slot,
            8,
            GpuRenderError::DeviceLost {
                reason: GpuDeviceLossReason::Unknown,
                message: "active device".to_owned(),
            },
        );
        assert!(has_gpu_fault(&slot, 8));
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
        assert_eq!(artifact.body(), SceneRect::new(1, 7, 78, 15));
        assert_eq!((artifact.width(), artifact.height()), (4, 2));
        assert_eq!(artifact.revision(), 7);
        assert_eq!(artifact.rgba8().as_ptr(), source_ptr);
        assert_eq!(artifact.visible_clips().len(), 15);
        assert!(
            artifact
                .visible_clips()
                .iter()
                .all(|clip| clip.width == 78 && clip.height == 1)
        );
    }

    #[test]
    fn ready_raster_requires_the_exact_typed_canvas_in_product_scenes() {
        let mut missing = scene(vec![pane(
            PaneSceneKind::Artifact,
            ready_artifact(4, 2, 7),
        )]);
        missing.presentation.nodes.push(PresentationNode {
            id: PresentationNodeId::workspace(WorkspaceNodePart::Surface),
            parent: None,
            role: PresentationNodeRole::Pane,
            state: PresentationNodeState::default(),
            logical_rect: LogicalRect::from_units(0, 0, 64, 64),
            cell_rect: Some(SceneRect::new(0, 0, 1, 1)),
            terminal_projection: TerminalProjection::CellRegions(Vec::new()),
        });
        assert_eq!(
            prepare_scene(&missing, &Theme::default()).unwrap_err(),
            SceneCompileError::InvalidGeometry("ready artifact is missing its typed canvas")
        );

        let mut mismatched = scene(vec![pane(
            PaneSceneKind::Artifact,
            ready_artifact(4, 2, 7),
        )]);
        let pane_id = mismatched.panes[0].id.clone();
        mismatched.presentation.nodes.push(PresentationNode {
            id: PresentationNodeId::pane(
                pane_id,
                PaneNodePart::Workflow(WorkflowNodePart::ArtifactCanvas),
            ),
            parent: None,
            role: PresentationNodeRole::ArtifactCanvas,
            state: PresentationNodeState::default(),
            logical_rect: LogicalRect::from_units(64, 8 * 64, 78 * 64, 14 * 64),
            cell_rect: Some(SceneRect::new(1, 8, 78, 14)),
            terminal_projection: TerminalProjection::CellRegions(Vec::new()),
        });
        assert_eq!(
            prepare_scene(&mismatched, &Theme::default()).unwrap_err(),
            SceneCompileError::InvalidGeometry(
                "ready artifact canvas does not match typed geometry"
            )
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

        let attrs = glyph_attrs(
            ResolvedGlyphStyle {
                foreground: resolved.foreground,
                bold: resolved.bold,
                italic: resolved.italic,
                underline: resolved.underline,
                strikethrough: resolved.strikethrough,
            },
            "monospace",
        );
        assert_eq!(attrs.weight, Weight::BOLD);
        assert_eq!(attrs.style, FontStyle::Italic);
        assert_eq!(attrs.text_decoration.underline, UnderlineStyle::Single);
        assert!(attrs.text_decoration.strikethrough);
    }

    #[test]
    fn native_materializes_all_terminal_palette_colors_and_keeps_indexed_rgb_meaning() {
        let ansi = std::array::from_fn(|index| {
            [
                index as u8,
                (index as u8).saturating_add(32),
                (index as u8).saturating_add(64),
            ]
        });
        let theme = Theme {
            terminal_palette: TerminalPalette {
                foreground: [1, 2, 3],
                background: [4, 5, 6],
                ansi,
            },
            ..Theme::default()
        };
        let mut cell = ProgramCell {
            occupancy: CellOccupancy::Grapheme("X".to_owned()),
            style: mandatum_scene::SceneCellStyle::default(),
            selection: None,
            cursor: false,
            raster_layer: None,
        };

        let defaults = resolve_program_cell(&cell, &theme);
        assert_eq!(defaults.foreground, [1, 2, 3, 255]);
        assert_eq!(defaults.background, [4, 5, 6, 255]);
        assert_eq!(
            native_frame_colors(&theme),
            NativeFrameColors {
                default_foreground: [1, 2, 3],
                clear_background: [4, 5, 6],
            }
        );

        for index in 0..16 {
            cell.style.foreground = SceneColor::Ansi(index);
            assert_eq!(
                resolve_program_cell(&cell, &theme).foreground,
                [
                    ansi[index as usize][0],
                    ansi[index as usize][1],
                    ansi[index as usize][2],
                    255
                ]
            );
        }
        for (index, expected) in [
            (16, [0, 0, 0]),
            (231, [255, 255, 255]),
            (232, [8, 8, 8]),
            (255, [238, 238, 238]),
        ] {
            cell.style.foreground = SceneColor::Indexed(index);
            assert_eq!(
                resolve_program_cell(&cell, &theme).foreground,
                [expected[0], expected[1], expected[2], 255]
            );
        }
        cell.style.foreground = SceneColor::Rgb(7, 8, 9);
        assert_eq!(
            resolve_program_cell(&cell, &theme).foreground,
            [7, 8, 9, 255]
        );

        let mut chrome_theme = theme.clone();
        chrome_theme.header = SceneColor::Ansi(12);
        let scene = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let prepared = prepare_scene(&scene, &chrome_theme).unwrap();
        let header = prepared
            .cell_program()
            .cell_at(scene.header.area.x, scene.header.area.y)
            .expect("compiled header cell");
        assert_eq!(header.style.foreground, SceneColor::Ansi(12));
        assert_eq!(
            resolve_program_cell(header, &chrome_theme).foreground,
            [ansi[12][0], ansi[12][1], ansi[12][2], 255],
            "compiled semantic chrome must materialize through the active terminal palette"
        );
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

        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();
        let final_cells = translated
            .cells
            .iter()
            .filter(|(x, y, _, _, _, _)| (*x, *y) == (3, 6))
            .collect::<Vec<_>>();

        assert_eq!(final_cells.len(), 1);
        assert_eq!(final_cells[0].2.grapheme, " ");
        assert_eq!(
            final_cells[0].2.background,
            [
                Theme::default().terminal_palette.background[0],
                Theme::default().terminal_palette.background[1],
                Theme::default().terminal_palette.background[2],
                255
            ]
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
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();
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
    fn fractional_native_ui_point_sizes_still_admit_shaped_rows() {
        let surface = TerminalSurface {
            rows: vec![
                "letters"
                    .chars()
                    .map(|character| {
                        SceneCell::grapheme(
                            character.to_string(),
                            mandatum_scene::SceneCellStyle::default(),
                        )
                    })
                    .collect(),
            ],
            ..TerminalSurface::default()
        };
        let scene = scene(vec![pane(
            PaneSceneKind::Terminal,
            PaneContent::Terminal(surface),
        )]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();
        let base = translated
            .rows
            .iter()
            .find(|row| row.text == "letters")
            .expect("adjacent same-style cells form one row run");

        let font_profile = ResolvedFontProfile::resolve(FontRequest::default()).unwrap();
        let mut font_system = font_profile.create_font_system();
        let metric_set = crate::NativeTextMetricSet::from_theme(&theme, 1);

        // Body is 12.5pt and every role lands on a fractional physical size at
        // fractional display scales.
        for (scale, role) in [
            (1.0, crate::NativeTextMetricRole::Body),
            (1.25, crate::NativeTextMetricRole::Title),
            (1.5, crate::NativeTextMetricRole::Body),
            (1.75, crate::NativeTextMetricRole::Metadata),
        ] {
            let mut row = base.clone();
            row.native_metrics = Some(metric_set.identity(role));

            let terminal_font_size = (font_profile.size() * scale).round();
            let terminal_metrics =
                Metrics::new(terminal_font_size, (terminal_font_size * 1.3).round());
            let terminal_advance =
                measure_cell_width(&mut font_system, terminal_metrics, font_profile.family());
            let profile =
                row_shaping_profile(&row, terminal_metrics, terminal_advance, scale);
            assert_eq!(
                profile.metrics.font_size,
                profile.metrics.font_size.round(),
                "native UI text must shape at whole physical pixels ({role:?} at {scale})"
            );

            let mut buffer = Buffer::new(&mut font_system, profile.metrics);
            shape_row_buffer(
                &mut buffer,
                &row,
                &mut font_system,
                profile.metrics,
                profile.cell_advance,
                profile.metrics.line_height,
                font_profile.family(),
            );
            let (facts, _) = layout_facts_and_observations(&buffer, &row);

            assert_eq!(
                admit_layout(&row, &facts, profile.cell_advance, 1.0),
                RowRunAdmission::Accepted,
                "{role:?} at scale {scale} must not cascade into anchored fallback"
            );
        }
    }

    #[test]
    fn bundled_row_run_shapes_a_real_multicell_ligature_and_passes_admission() {
        let surface = TerminalSurface {
            rows: vec![vec![
                SceneCell::grapheme("-", mandatum_scene::SceneCellStyle::default()),
                SceneCell::grapheme(">", mandatum_scene::SceneCellStyle::default()),
            ]],
            ..TerminalSurface::default()
        };
        let pane = pane(PaneSceneKind::Terminal, PaneContent::Terminal(surface));
        let scene = scene(vec![pane]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();
        let row = translated
            .rows
            .iter()
            .find(|row| row.text == "->")
            .expect("adjacent same-style cells form one row run");

        let profile = ResolvedFontProfile::resolve(FontRequest::default()).unwrap();
        let mut font_system = profile.create_font_system();
        let line_height = (profile.size() * 1.3).round();
        let metrics = Metrics::new(profile.size(), line_height);
        let cell_width = measure_cell_width(&mut font_system, metrics, profile.family());
        let mut buffer = Buffer::new(&mut font_system, metrics);
        shape_row_buffer(
            &mut buffer,
            row,
            &mut font_system,
            metrics,
            cell_width,
            line_height,
            profile.family(),
        );
        let (facts, observations) = layout_facts_and_observations(&buffer, row);

        assert_eq!(
            admit_layout(row, &facts, cell_width, 1.0),
            RowRunAdmission::Accepted
        );
        let contextual_glyphs = observations
            .iter()
            .map(|observation| observation.glyph_id)
            .collect::<Vec<_>>();
        let mut anchored_glyphs = Vec::new();
        for anchored in anchored_fallback_runs(row).unwrap() {
            let mut anchored_buffer = Buffer::new(&mut font_system, metrics);
            shape_row_buffer(
                &mut anchored_buffer,
                &anchored,
                &mut font_system,
                metrics,
                cell_width,
                line_height,
                profile.family(),
            );
            anchored_glyphs.extend(
                layout_facts_and_observations(&anchored_buffer, &anchored)
                    .1
                    .into_iter()
                    .map(|observation| observation.glyph_id),
            );
        }
        assert_ne!(
            contextual_glyphs, anchored_glyphs,
            "JetBrains Mono should apply contextual ligature forms only when -> shapes as one run"
        );
        assert!(
            observations
                .iter()
                .all(|observation| observation.font_id == profile.selected_faces().regular)
        );

        let context = ShapingCacheContext {
            font_generation: profile.generation(),
            scale_generation: 1,
            metric_generation: 0,
            metric_slot: 0,
            renderer_config_generation: SHAPING_POLICY_GENERATION,
            font_size_bits: metrics.font_size.to_bits(),
            line_height_bits: metrics.line_height.to_bits(),
            cell_width_bits: cell_width.to_bits(),
            cell_height_bits: line_height.to_bits(),
        };
        let key = shaping_cache_key_for_candidate(true, false, row, context)
            .expect("normally admitted shaping units are cache candidates");
        let anchored_key = shaping_cache_key_for_candidate(true, true, row, context)
            .expect("anchored buffers are cached in their own namespace");
        assert_ne!(
            key, anchored_key,
            "an anchored buffer was never admitted and must not share the admitted key"
        );
        assert!(
            shaping_cache_key_for_candidate(false, false, row, context).is_none(),
            "the lab bypass must preserve the direct uncached path"
        );

        let cached_buffer = Arc::new(buffer);
        let cached_observations = Arc::<[FontObservation]>::from(observations);
        let value = CachedShaping::Shaped {
            buffer: cached_buffer.clone(),
            observations: cached_observations.clone(),
        };
        let accounted = shaping_cache_accounted_bytes(&key, &value);
        let mut cache = ShapingCache::new();
        assert!(cache.insert(key.clone(), value.clone(), accounted));
        let Some(CachedShaping::Shaped {
            buffer: hit_buffer,
            observations: hit_observations,
        }) = cache.get_cloned(&key)
        else {
            panic!("admitted shaping cache hit");
        };
        assert!(Arc::ptr_eq(&hit_buffer, &cached_buffer));
        assert_eq!(hit_observations.len(), cached_observations.len());
        assert!(
            hit_observations
                .iter()
                .zip(cached_observations.iter())
                .all(|(hit, expected)| hit.font_id == expected.font_id
                    && hit.glyph_id == expected.glyph_id
                    && hit.sample == expected.sample)
        );

        // Namespace separation, in both directions: an anchored entry never
        // satisfies an ordinary lookup, and an admitted entry never satisfies
        // an anchored one.
        let mut namespaced = ShapingCache::new();
        assert!(namespaced.insert(anchored_key.clone(), value.clone(), accounted));
        assert!(namespaced.get_cloned(&key).is_none());
        assert!(namespaced.get_cloned(&anchored_key).is_some());
        assert!(namespaced.insert(key.clone(), value, accounted));
        assert!(namespaced.get_cloned(&key).is_some());
        assert_eq!(namespaced.len(), 2);
    }

    #[test]
    fn a_repeat_frame_of_permanently_inadmissible_rows_reshapes_nothing() {
        use crate::row_run::ByteCellSpan;

        let surface = TerminalSurface {
            rows: vec![
                "abcd"
                    .chars()
                    .map(|character| {
                        SceneCell::grapheme(
                            character.to_string(),
                            mandatum_scene::SceneCellStyle::default(),
                        )
                    })
                    .collect(),
            ],
            ..TerminalSurface::default()
        };
        let scene = scene(vec![pane(
            PaneSceneKind::Terminal,
            PaneContent::Terminal(surface),
        )]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();
        let mut row = translated
            .rows
            .iter()
            .find(|row| row.text == "abcd")
            .expect("same-style graphemes share one row run")
            .clone();
        // Declare two cells per grapheme. No monospace advance can satisfy
        // that, so every cluster of every sub-run fails admission forever —
        // the shape of a braille spinner or a nerd icon whose fallback face
        // cannot match the cell.
        row.width = 8;
        row.byte_cells = (0..4)
            .map(|index| ByteCellSpan {
                bytes: index..index + 1,
                cells: index as u16 * 2..index as u16 * 2 + 2,
            })
            .collect();

        let font_profile = ResolvedFontProfile::resolve(FontRequest::default()).unwrap();
        let mut font_system = font_profile.create_font_system();
        let line_height = (font_profile.size() * 1.3).round();
        let metrics = Metrics::new(font_profile.size(), line_height);
        let cell_advance = measure_cell_width(&mut font_system, metrics, font_profile.family());
        let family = font_profile.family().to_owned();
        let mut row_buffers = RowBufferPool::new();
        let mut shaping_cache = ShapingCache::new();
        let mut fallback_report = FallbackReport::new(font_profile.generation());
        let mut diagnostics = BTreeSet::new();
        let pass = |font_system: &mut FontSystem,
                        row_buffers: &mut RowBufferPool,
                        shaping_cache: &mut ShapingCache<CachedShaping>,
                        fallback_report: &mut FallbackReport,
                        diagnostics: &mut BTreeSet<String>| {
            RowShapingPass {
                font_system,
                row_buffers,
                shaping_cache,
                fallback_report,
                diagnostics,
                font_profile: &font_profile,
                font_family: &family,
                cache_enabled: true,
                terminal_metrics: metrics,
                cell_advance,
                cell_height: line_height,
                scale: 1.0,
                scale_generation: 1,
            }
            .run(vec![row.clone()])
            .unwrap()
        };

        let placement = |rows: &[ShapedRow]| {
            rows.iter()
                .map(|shaped| (shaped.row.text.clone(), shaped.row.x, shaped.row.width))
                .collect::<Vec<_>>()
        };
        let first = pass(
            &mut font_system,
            &mut row_buffers,
            &mut shaping_cache,
            &mut fallback_report,
            &mut diagnostics,
        );
        let first_stats = shaping_cache.stats();

        assert_eq!(
            placement(&first),
            "abcd"
                .chars()
                .enumerate()
                .map(|(index, character)| (
                    character.to_string(),
                    row.x + index as u16 * 2,
                    2
                ))
                .collect::<Vec<_>>()
        );
        // One shaping attempt per surviving suffix plus one per anchored leaf.
        // The one-grapheme peel needed a doomed solo admission per grapheme on
        // top of that, and repeated all of it on every frame.
        assert_eq!(first_stats.misses, 8);
        assert_eq!(first_stats.hits, 0);
        assert_eq!(row_buffers.len(), 4);
        // Four anchored leaves plus the parent decomposition.
        assert_eq!(shaping_cache.len(), 5);

        let second = pass(
            &mut font_system,
            &mut row_buffers,
            &mut shaping_cache,
            &mut fallback_report,
            &mut diagnostics,
        );
        let second_stats = shaping_cache.stats();

        assert_eq!(placement(&second), placement(&first));
        assert_eq!(
            second_stats.misses, first_stats.misses,
            "a repeat frame must not shape anything: every miss is a shaping call"
        );
        assert_eq!(second_stats.hits, 5);
        assert_eq!(row_buffers.len(), 4);
        assert_eq!(shaping_cache.len(), 5);
    }

    #[test]
    fn one_inadmissible_glyph_anchors_alone_and_survives_in_the_cache() {
        // U+F0E7 renders from the bundled family at an advance the cell cannot
        // match, so it reproduces the icon/spinner fallback flood exactly.
        let surface = TerminalSurface {
            rows: vec![
                "ab\u{f0e7}cd"
                    .chars()
                    .map(|character| {
                        SceneCell::grapheme(
                            character.to_string(),
                            mandatum_scene::SceneCellStyle::default(),
                        )
                    })
                    .collect(),
            ],
            ..TerminalSurface::default()
        };
        let scene = scene(vec![pane(
            PaneSceneKind::Terminal,
            PaneContent::Terminal(surface),
        )]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();
        let row = translated
            .rows
            .iter()
            .find(|row| row.text == "ab\u{f0e7}cd")
            .expect("same-style graphemes share one row run")
            .clone();

        let font_profile = ResolvedFontProfile::resolve(FontRequest::default()).unwrap();
        let mut font_system = font_profile.create_font_system();
        let line_height = (font_profile.size() * 1.3).round();
        let metrics = Metrics::new(font_profile.size(), line_height);
        let cell_advance = measure_cell_width(&mut font_system, metrics, font_profile.family());
        let family = font_profile.family().to_owned();
        let mut row_buffers = RowBufferPool::new();
        let mut shaping_cache = ShapingCache::new();
        let mut fallback_report = FallbackReport::new(font_profile.generation());
        let mut diagnostics = BTreeSet::new();
        let pass = |font_system: &mut FontSystem,
                    row_buffers: &mut RowBufferPool,
                    shaping_cache: &mut ShapingCache<CachedShaping>,
                    fallback_report: &mut FallbackReport,
                    diagnostics: &mut BTreeSet<String>| {
            RowShapingPass {
                font_system,
                row_buffers,
                shaping_cache,
                fallback_report,
                diagnostics,
                font_profile: &font_profile,
                font_family: &family,
                cache_enabled: true,
                terminal_metrics: metrics,
                cell_advance,
                cell_height: line_height,
                scale: 1.0,
                scale_generation: 1,
            }
            .run(vec![row.clone()])
            .unwrap()
        };

        let first = pass(
            &mut font_system,
            &mut row_buffers,
            &mut shaping_cache,
            &mut fallback_report,
            &mut diagnostics,
        );
        let first_stats = shaping_cache.stats();

        // The offending cluster is retired on its own; the validated prefix
        // and the untested tail keep contextual shaping.
        assert_eq!(
            first
                .iter()
                .map(|shaped| (shaped.row.text.as_str(), shaped.row.x))
                .collect::<Vec<_>>(),
            vec![("ab", row.x), ("\u{f0e7}", row.x + 2), ("cd", row.x + 3)]
        );
        assert_eq!(first_stats.misses, 4);
        assert_eq!(shaping_cache.len(), 4);

        // Anchored entries retain their observations, so fallback and
        // missing-glyph reporting still sees them on a cache hit.
        let context = ShapingCacheContext {
            font_generation: font_profile.generation(),
            scale_generation: 1,
            metric_generation: 0,
            metric_slot: 0,
            renderer_config_generation: SHAPING_POLICY_GENERATION,
            font_size_bits: metrics.font_size.to_bits(),
            line_height_bits: metrics.line_height.to_bits(),
            cell_width_bits: cell_advance.to_bits(),
            cell_height_bits: line_height.to_bits(),
        };
        let anchored = slice_run(&row, 2..3).unwrap();
        let anchored_key = shaping_cache_key_for_candidate(true, true, &anchored, context)
            .expect("anchored candidates are cacheable");
        let Some(CachedShaping::Shaped { observations, .. }) =
            shaping_cache.get_cloned(&anchored_key)
        else {
            panic!("the anchored icon must be retained under its anchored key");
        };
        assert!(!observations.is_empty());
        assert!(
            shaping_cache
                .get_cloned(
                    &shaping_cache_key_for_candidate(true, false, &anchored, context).unwrap()
                )
                .is_none(),
            "an unadmitted buffer must never satisfy an ordinary lookup"
        );

        // The probes above are themselves lookups, so the repeat frame is
        // measured from here.
        let probed_stats = shaping_cache.stats();
        let second = pass(
            &mut font_system,
            &mut row_buffers,
            &mut shaping_cache,
            &mut fallback_report,
            &mut diagnostics,
        );

        assert_eq!(
            second
                .iter()
                .map(|shaped| (shaped.row.text.clone(), shaped.row.x))
                .collect::<Vec<_>>(),
            first
                .iter()
                .map(|shaped| (shaped.row.text.clone(), shaped.row.x))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            shaping_cache.stats().misses,
            probed_stats.misses,
            "the repeat frame must not shape anything"
        );
        assert_eq!(shaping_cache.stats().hits, probed_stats.hits + 4);
    }

    #[test]
    fn braille_spinner_glyphs_shape_admitted_from_the_bundled_fallback() {
        // Spinner frames from the Braille block. JetBrains Mono has no
        // Braille coverage and Apple Braille's 0.692em advance can never
        // match the 0.6em cell, so before the bundled fallback these runs
        // permanently failed admission and took the anchored path.
        let text = "\u{280b}\u{2819}\u{28ff}";
        let surface = TerminalSurface {
            rows: vec![
                text.chars()
                    .map(|character| {
                        SceneCell::grapheme(
                            character.to_string(),
                            mandatum_scene::SceneCellStyle::default(),
                        )
                    })
                    .collect(),
            ],
            ..TerminalSurface::default()
        };
        let scene = scene(vec![pane(
            PaneSceneKind::Terminal,
            PaneContent::Terminal(surface),
        )]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();
        let row = translated
            .rows
            .iter()
            .find(|row| row.text == text)
            .expect("same-style graphemes share one row run")
            .clone();

        let font_profile = ResolvedFontProfile::resolve(FontRequest::default()).unwrap();
        let mut font_system = font_profile.create_font_system();
        let line_height = (font_profile.size() * 1.3).round();
        let metrics = Metrics::new(font_profile.size(), line_height);
        let cell_advance = measure_cell_width(&mut font_system, metrics, font_profile.family());
        let family = font_profile.family().to_owned();
        let mut row_buffers = RowBufferPool::new();
        let mut shaping_cache = ShapingCache::new();
        let mut fallback_report = FallbackReport::new(font_profile.generation());
        let mut diagnostics = BTreeSet::new();
        let shaped = RowShapingPass {
            font_system: &mut font_system,
            row_buffers: &mut row_buffers,
            shaping_cache: &mut shaping_cache,
            fallback_report: &mut fallback_report,
            diagnostics: &mut diagnostics,
            font_profile: &font_profile,
            font_family: &family,
            cache_enabled: true,
            terminal_metrics: metrics,
            cell_advance,
            cell_height: line_height,
            scale: 1.0,
            scale_generation: 1,
        }
        .run(vec![row.clone()])
        .unwrap();

        // Admitted as one grid-aligned run — no anchored retirement.
        assert_eq!(
            shaped
                .iter()
                .map(|shaped| (shaped.row.text.as_str(), shaped.row.x))
                .collect::<Vec<_>>(),
            vec![(text, row.x)]
        );
        let context = ShapingCacheContext {
            font_generation: font_profile.generation(),
            scale_generation: 1,
            metric_generation: 0,
            metric_slot: 0,
            renderer_config_generation: SHAPING_POLICY_GENERATION,
            font_size_bits: metrics.font_size.to_bits(),
            line_height_bits: metrics.line_height.to_bits(),
            cell_width_bits: cell_advance.to_bits(),
            cell_height_bits: line_height.to_bits(),
        };
        let key = shaping_cache_key_for_candidate(true, false, &row, context)
            .expect("admitted candidates are cacheable");
        let Some(CachedShaping::Shaped { observations, .. }) = shaping_cache.get_cloned(&key)
        else {
            panic!("the braille run must be retained under its ordinary key");
        };
        assert!(!observations.is_empty());
        for observation in observations.iter() {
            let face = font_profile
                .database()
                .face(observation.font_id)
                .expect("observed face is in the resolved catalog");
            assert_eq!(
                face.post_script_name, "MandatumBraille-Regular",
                "braille glyphs must come from the bundled metric-matched face"
            );
        }
    }

    #[test]
    fn bundled_profile_shapes_regular_bold_italic_and_bold_italic_from_selected_faces() {
        let style = |bold, italic| mandatum_scene::SceneCellStyle {
            bold,
            italic,
            ..mandatum_scene::SceneCellStyle::default()
        };
        let surface = TerminalSurface {
            rows: vec![vec![
                SceneCell::grapheme("R", style(false, false)),
                SceneCell::grapheme("B", style(true, false)),
                SceneCell::grapheme("I", style(false, true)),
                SceneCell::grapheme("Z", style(true, true)),
            ]],
            ..TerminalSurface::default()
        };
        let scene = scene(vec![pane(
            PaneSceneKind::Terminal,
            PaneContent::Terminal(surface),
        )]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &theme,
            prepared.presentation_plan(),
        )
        .unwrap();
        let profile = ResolvedFontProfile::resolve(FontRequest::default()).unwrap();
        let selected = profile.selected_faces();
        let expected = [
            ("R", selected.regular),
            ("B", selected.bold),
            ("I", selected.italic),
            ("Z", selected.bold_italic),
        ];
        let mut font_system = profile.create_font_system();
        let line_height = (profile.size() * 1.3).round();
        let metrics = Metrics::new(profile.size(), line_height);
        let cell_width = measure_cell_width(&mut font_system, metrics, profile.family());

        for (text, expected_id) in expected {
            let row = translated
                .rows
                .iter()
                .find(|row| row.text == text)
                .expect("style boundary creates one run");
            let mut buffer = Buffer::new(&mut font_system, metrics);
            shape_row_buffer(
                &mut buffer,
                row,
                &mut font_system,
                metrics,
                cell_width,
                line_height,
                profile.family(),
            );
            let (_, observations) = layout_facts_and_observations(&buffer, row);
            assert!(
                !observations.is_empty(),
                "{text:?} should produce a shaped glyph"
            );
            assert!(
                observations
                    .iter()
                    .all(|observation| observation.font_id == expected_id),
                "{text:?} did not use the selected face"
            );
        }
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
                project_name: "project".to_owned(),
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
            presentation: mandatum_scene::ScenePresentation::default(),
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
        let alternating_row = (0..510)
            .map(|index| {
                SceneCell::grapheme(
                    "X",
                    mandatum_scene::SceneCellStyle {
                        bold: index % 2 == 0,
                        ..mandatum_scene::SceneCellStyle::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        let rows = vec![alternating_row; 66];
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
        let theme = Theme::default();
        // The budget is enforced on the one row-run plan the render path
        // builds, so the rejection now lands in `prepare_cell_program`.
        let prepared = prepare_scene(&dense, &theme)
            .expect("the cell program itself stays inside the instruction budget");
        assert!(matches!(
            prepare_cell_program(
                prepared.cell_program(),
                &dense,
                &theme,
                prepared.presentation_plan(),
            )
            .unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "text buffers",
                actual,
                maximum: MAX_GPU_TEXT_BUFFERS,
            } if actual > MAX_GPU_TEXT_BUFFERS
        ));
    }

    #[test]
    fn anchor_all_expansion_cannot_enqueue_beyond_the_text_buffer_budget() {
        let surface = TerminalSurface {
            rows: vec![vec![
                SceneCell::grapheme("A", mandatum_scene::SceneCellStyle::default()),
                SceneCell::grapheme("B", mandatum_scene::SceneCellStyle::default()),
            ]],
            ..TerminalSurface::default()
        };
        let pane = pane(PaneSceneKind::Terminal, PaneContent::Terminal(surface));
        let scene = scene(vec![pane]);
        let prepared = prepare_scene(&scene, &Theme::default()).unwrap();
        let translated = prepare_cell_program(
            prepared.cell_program(),
            &scene,
            &Theme::default(),
            prepared.presentation_plan(),
        )
        .unwrap();
        let row = translated
            .rows
            .iter()
            .find(|row| row.text == "AB")
            .expect("same-style graphemes share one row run");

        let admitted =
            anchored_fallback_runs_within_budget(row, MAX_GPU_TEXT_BUFFERS - 2, 0).unwrap();
        assert_eq!(admitted.len(), 2);

        assert_eq!(
            anchored_fallback_runs_within_budget(row, MAX_GPU_TEXT_BUFFERS - 1, 0).unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "text buffers",
                actual: MAX_GPU_TEXT_BUFFERS + 1,
                maximum: MAX_GPU_TEXT_BUFFERS,
            }
        );
        assert_eq!(
            enforce_text_buffer_work_limit(usize::MAX, 1, 1).unwrap_err(),
            SceneCompileError::ResourceLimit {
                resource: "text buffers",
                actual: usize::MAX,
                maximum: MAX_GPU_TEXT_BUFFERS,
            }
        );
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
            item_keys: Vec::new(),
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
