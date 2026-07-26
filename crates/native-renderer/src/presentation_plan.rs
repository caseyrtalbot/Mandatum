//! Pure, headless translation from scene-owned semantics to native paint work.

use std::collections::HashSet;

use mandatum_scene::{
    LogicalRect, OverlayPresentationKind, PresentationNode, PresentationNodeId,
    PresentationNodeRole, PresentationTone, SceneRect, TerminalProjection, Theme,
    TransitionProperty, TransitionRole, UiColor, UiMotionToken, UiShadow, WorkspaceScene,
};

use crate::text_metrics::{NativeTextMetricIdentity, NativeTextMetricRole, NativeTextMetricSet};

pub const MAX_NATIVE_PLAN_NODES: usize = 8_192;
pub const MAX_NATIVE_PLAN_COMMANDS: usize = 32_768;
pub const MAX_NATIVE_PLAN_TEXT_SCOPES: usize = 16_384;
pub const MAX_NATIVE_PLAN_TRANSITIONS: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeMaterialRole {
    Canvas,
    PaneSurface,
    ChromeSurface,
    OverlaySurface,
    ModalScrim,
    OverlayBand,
    BorderSubtle,
    BorderStrong,
    Focus,
    Selection,
    SelectionIndicator,
    Attention,
    Badge,
    WorkflowCallout,
    WorkflowConsole,
    ArtifactInspector,
    ArtifactCanvas,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeBoundary {
    pub width_units: u64,
    pub color: UiColor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeMaterial {
    pub node_id: PresentationNodeId,
    pub role: NativeMaterialRole,
    pub logical_rect: LogicalRect,
    pub clip: LogicalRect,
    pub color: UiColor,
    pub corner_radius_units: u64,
    pub boundary: Option<NativeBoundary>,
    pub raised_shadows: Option<[UiShadow; 2]>,
    pub z_order: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTextScope {
    pub node_id: PresentationNodeId,
    /// Semantic logical geometry for validation and future vector text.
    ///
    /// Current cell-owned glyph placement remains direct; presentation
    /// Geometry and Scale interpolate native materials only.
    pub logical_rect: LogicalRect,
    /// Exact cell projection used to color the already-compiled `CellProgram`
    /// without reconstructing presentation roles in the GPU adapter.
    pub cell_rect: Option<SceneRect>,
    pub clip: LogicalRect,
    pub color: UiColor,
    pub metrics: NativeTextMetricIdentity,
    pub z_order: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTransition {
    pub node_id: PresentationNodeId,
    pub role: TransitionRole,
    pub property: TransitionProperty,
    pub sequence: u64,
    pub timing: UiMotionToken,
    pub exit_timing: UiMotionToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativePlanCommand {
    BeginClip {
        node_id: PresentationNodeId,
        clip: LogicalRect,
        z_order: u32,
    },
    Material(NativeMaterial),
    Text(NativeTextScope),
    EndClip {
        node_id: PresentationNodeId,
        z_order: u32,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativePresentationPlan {
    commands: Vec<NativePlanCommand>,
    transitions: Vec<NativeTransition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeTokenColorRole {
    Canvas,
    PaneSurface,
    ChromeSurface,
    OverlaySurface,
    BorderSubtle,
    BorderStrong,
    TextPrimary,
    TextSecondary,
    TextMuted,
    Focus,
    Running,
    Waiting,
    Failure,
    Complete,
    AgentIdentity,
    SelectionFill,
    ModalScrim,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTokenSwatch {
    pub role: NativeTokenColorRole,
    pub logical_rect: LogicalRect,
    pub clip: LogicalRect,
    pub color: UiColor,
    pub z_order: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTokenSamplerPlan {
    bounds: LogicalRect,
    swatches: Vec<NativeTokenSwatch>,
}

impl NativeTokenSamplerPlan {
    pub fn bounds(&self) -> LogicalRect {
        self.bounds
    }

    pub fn swatches(&self) -> &[NativeTokenSwatch] {
        &self.swatches
    }
}

impl NativePresentationPlan {
    pub fn commands(&self) -> &[NativePlanCommand] {
        &self.commands
    }

    pub fn transitions(&self) -> &[NativeTransition] {
        &self.transitions
    }

    pub fn material_count(&self) -> usize {
        self.commands
            .iter()
            .filter(|command| matches!(command, NativePlanCommand::Material(_)))
            .count()
    }

    pub fn text_scope_count(&self) -> usize {
        self.commands
            .iter()
            .filter(|command| matches!(command, NativePlanCommand::Text(_)))
            .count()
    }

    pub(crate) fn from_resolved_commands(
        commands: Vec<NativePlanCommand>,
        transitions: Vec<NativeTransition>,
    ) -> Self {
        Self {
            commands,
            transitions,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativePresentationPlanError {
    ResourceLimit {
        resource: &'static str,
        actual: usize,
        maximum: usize,
    },
    MissingViewport,
    DuplicateNodeId,
    ParentMustPrecedeChild,
    BoundsOutsideViewport,
    ChildEscapesParentClip,
    CellProjectionOutsideScene,
    TransitionReferencesMissingNode,
    TokenSamplerBoundsTooSmall,
}

/// Compile the native semantic plan without touching a window, GPU, or font
/// system. The current cell program remains an independent terminal-parity
/// projection and is not modified by this translation.
pub fn prepare_native_presentation(
    scene: &WorkspaceScene,
    theme: &Theme,
) -> Result<NativePresentationPlan, NativePresentationPlanError> {
    let presentation = &scene.presentation;
    if presentation.nodes.is_empty() {
        if presentation.transition_targets.is_empty() {
            return Ok(NativePresentationPlan::default());
        }
        return Err(NativePresentationPlanError::TransitionReferencesMissingNode);
    }
    let viewport = presentation
        .viewport
        .ok_or(NativePresentationPlanError::MissingViewport)?;
    enforce_limit(
        "presentation nodes",
        presentation.nodes.len(),
        MAX_NATIVE_PLAN_NODES,
    )?;
    enforce_limit(
        "transitions",
        presentation.transition_targets.len(),
        MAX_NATIVE_PLAN_TRANSITIONS,
    )?;

    let viewport_rect = LogicalRect::from_units(
        0,
        0,
        viewport.logical_size.width_units(),
        viewport.logical_size.height_units(),
    );
    let metric_generation = typography_generation(theme);
    let metrics = NativeTextMetricSet::from_theme(theme, metric_generation);
    let mut seen = HashSet::with_capacity(presentation.nodes.len());
    let mut node_bounds = std::collections::HashMap::with_capacity(presentation.nodes.len());
    let mut commands = Vec::new();
    let mut text_scopes = 0usize;

    for (index, node) in presentation.nodes.iter().enumerate() {
        if !seen.insert(node.id.clone()) {
            return Err(NativePresentationPlanError::DuplicateNodeId);
        }
        if !rect_contains(viewport_rect, node.logical_rect) || node.logical_rect.is_empty() {
            return Err(NativePresentationPlanError::BoundsOutsideViewport);
        }
        if let Some(parent) = &node.parent {
            if !seen.contains(parent) {
                return Err(NativePresentationPlanError::ParentMustPrecedeChild);
            }
            if !rect_contains(
                *node_bounds
                    .get(parent)
                    .expect("seen parents retain their bounds"),
                node.logical_rect,
            ) {
                return Err(NativePresentationPlanError::ChildEscapesParentClip);
            }
        }
        validate_cell_projection(scene, node)?;
        node_bounds.insert(node.id.clone(), node.logical_rect);

        let z_base = u32::try_from(index)
            .unwrap_or(u32::MAX / 8)
            .saturating_mul(8);
        commands.push(NativePlanCommand::BeginClip {
            node_id: node.id.clone(),
            clip: node.logical_rect,
            z_order: z_base,
        });
        let material_specs = materials_for_node(node, theme, viewport_rect);
        for (material_index, spec) in material_specs.into_iter().enumerate() {
            let logical_rect = spec.logical_rect.unwrap_or(node.logical_rect);
            let clip = spec.clip.unwrap_or_else(|| {
                if spec.raised_shadows.is_some() {
                    node.parent
                        .as_ref()
                        .and_then(|parent| node_bounds.get(parent))
                        .copied()
                        .unwrap_or(node.logical_rect)
                } else {
                    node.logical_rect
                }
            });
            commands.push(NativePlanCommand::Material(NativeMaterial {
                node_id: node.id.clone(),
                role: spec.role,
                logical_rect,
                clip,
                color: spec.color,
                corner_radius_units: spec.corner_radius_units,
                boundary: spec.boundary,
                raised_shadows: spec.raised_shadows,
                z_order: z_base + 1 + material_index as u32,
            }));
        }
        if let Some((metric_role, color)) = text_for_node(node, theme) {
            text_scopes = text_scopes.saturating_add(1);
            enforce_limit("text scopes", text_scopes, MAX_NATIVE_PLAN_TEXT_SCOPES)?;
            commands.push(NativePlanCommand::Text(NativeTextScope {
                node_id: node.id.clone(),
                logical_rect: node.logical_rect,
                cell_rect: native_text_color_projection(node),
                clip: node.logical_rect,
                color,
                metrics: metrics.identity(metric_role),
                z_order: z_base + 5,
            }));
        }
        commands.push(NativePlanCommand::EndClip {
            node_id: node.id.clone(),
            z_order: z_base + 7,
        });
        enforce_limit("commands", commands.len(), MAX_NATIVE_PLAN_COMMANDS)?;
    }

    let mut transitions = Vec::with_capacity(presentation.transition_targets.len());
    for target in &presentation.transition_targets {
        if !seen.contains(&target.node_id) {
            return Err(NativePresentationPlanError::TransitionReferencesMissingNode);
        }
        transitions.push(NativeTransition {
            node_id: target.node_id.clone(),
            role: target.role,
            property: target.property,
            sequence: target.sequence,
            timing: match target.role {
                TransitionRole::Focus
                | TransitionRole::Selection
                | TransitionRole::ApprovalArrival => theme.ui.motion.focus_selection,
                TransitionRole::Overlay => theme.ui.motion.overlay_enter,
                TransitionRole::PaneGeometry => theme.ui.motion.pane_change,
            },
            exit_timing: match target.role {
                TransitionRole::Overlay => theme.ui.motion.overlay_exit,
                TransitionRole::Focus
                | TransitionRole::Selection
                | TransitionRole::ApprovalArrival => theme.ui.motion.focus_selection,
                TransitionRole::PaneGeometry => theme.ui.motion.pane_change,
            },
        });
    }

    Ok(NativePresentationPlan {
        commands,
        transitions,
    })
}

/// Pure, deterministic color-token sampler for the real native lab.
///
/// This diagnostic plan deliberately has its own typed token-role identity.
/// It does not manufacture product `PresentationNodeId`s or alter the normal
/// scene/`CellProgram` path.
pub fn prepare_token_sampler(
    theme: &Theme,
    bounds: LogicalRect,
) -> Result<NativeTokenSamplerPlan, NativePresentationPlanError> {
    const COLUMNS: u64 = 3;
    let padding = u64::from(theme.ui.spacing.space_4) * 64;
    let gap = u64::from(theme.ui.spacing.space_2) * 64;
    let row_height = u64::from(theme.ui.spacing.min_control_height) * 64;
    let palette = theme.ui.palette;
    let colors = [
        (NativeTokenColorRole::Canvas, palette.canvas),
        (NativeTokenColorRole::PaneSurface, palette.pane_surface),
        (NativeTokenColorRole::ChromeSurface, palette.chrome_surface),
        (
            NativeTokenColorRole::OverlaySurface,
            palette.overlay_surface,
        ),
        (NativeTokenColorRole::BorderSubtle, palette.border_subtle),
        (NativeTokenColorRole::BorderStrong, palette.border_strong),
        (NativeTokenColorRole::TextPrimary, palette.text_primary),
        (NativeTokenColorRole::TextSecondary, palette.text_secondary),
        (NativeTokenColorRole::TextMuted, palette.text_muted),
        (NativeTokenColorRole::Focus, palette.focus),
        (NativeTokenColorRole::Running, palette.running),
        (NativeTokenColorRole::Waiting, palette.waiting),
        (NativeTokenColorRole::Failure, palette.failure),
        (NativeTokenColorRole::Complete, palette.complete),
        (NativeTokenColorRole::AgentIdentity, palette.agent_identity),
        (NativeTokenColorRole::SelectionFill, palette.selection_fill),
        (NativeTokenColorRole::ModalScrim, palette.modal_scrim),
    ];
    let rows = (colors.len() as u64).div_ceil(COLUMNS);
    let horizontal_fixed = padding
        .saturating_mul(2)
        .saturating_add(gap.saturating_mul(COLUMNS - 1));
    let vertical_fixed = padding
        .saturating_mul(2)
        .saturating_add(gap.saturating_mul(rows.saturating_sub(1)));
    let Some(available_width) = bounds.size.width_units().checked_sub(horizontal_fixed) else {
        return Err(NativePresentationPlanError::TokenSamplerBoundsTooSmall);
    };
    let required_height = vertical_fixed.saturating_add(row_height.saturating_mul(rows));
    if available_width < COLUMNS || bounds.size.height_units() < required_height {
        return Err(NativePresentationPlanError::TokenSamplerBoundsTooSmall);
    }
    let column_width = available_width / COLUMNS;
    let mut swatches = Vec::with_capacity(colors.len());
    for (index, (role, color)) in colors.into_iter().enumerate() {
        let column = index as u64 % COLUMNS;
        let row = index as u64 / COLUMNS;
        let x = bounds
            .origin
            .x_units()
            .checked_add_unsigned(padding + column * (column_width + gap))
            .ok_or(NativePresentationPlanError::TokenSamplerBoundsTooSmall)?;
        let y = bounds
            .origin
            .y_units()
            .checked_add_unsigned(padding + row * (row_height + gap))
            .ok_or(NativePresentationPlanError::TokenSamplerBoundsTooSmall)?;
        let rect = LogicalRect::from_units(x, y, column_width, row_height);
        if !rect_contains(bounds, rect) {
            return Err(NativePresentationPlanError::TokenSamplerBoundsTooSmall);
        }
        swatches.push(NativeTokenSwatch {
            role,
            logical_rect: rect,
            clip: rect,
            color,
            z_order: u32::try_from(index).unwrap_or(u32::MAX),
        });
    }
    Ok(NativeTokenSamplerPlan { bounds, swatches })
}

fn validate_cell_projection(
    scene: &WorkspaceScene,
    node: &PresentationNode,
) -> Result<(), NativePresentationPlanError> {
    if let Some(cell_rect) = node.cell_rect
        && !cell_rect_within_scene(cell_rect, scene.size)
    {
        return Err(NativePresentationPlanError::CellProjectionOutsideScene);
    }
    match &node.terminal_projection {
        TerminalProjection::CellRegions(regions) => {
            if regions
                .iter()
                .any(|region| !cell_rect_within_scene(*region, scene.size))
            {
                return Err(NativePresentationPlanError::CellProjectionOutsideScene);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct NativeMaterialSpec {
    role: NativeMaterialRole,
    color: UiColor,
    corner_radius_units: u64,
    boundary: Option<NativeBoundary>,
    raised_shadows: Option<[UiShadow; 2]>,
    logical_rect: Option<LogicalRect>,
    clip: Option<LogicalRect>,
}

impl NativeMaterialSpec {
    fn flat(role: NativeMaterialRole, color: UiColor) -> Self {
        Self {
            role,
            color,
            corner_radius_units: 0,
            boundary: None,
            raised_shadows: None,
            logical_rect: None,
            clip: None,
        }
    }
}

fn materials_for_node(
    node: &PresentationNode,
    theme: &Theme,
    viewport_rect: LogicalRect,
) -> Vec<NativeMaterialSpec> {
    if node.state.hidden {
        return Vec::new();
    }
    let palette = theme.ui.palette;
    if node.role == PresentationNodeRole::Item && node.state.selected {
        let mut selection =
            NativeMaterialSpec::flat(NativeMaterialRole::Selection, palette.selection_fill);
        selection.corner_radius_units = u64::from(theme.ui.spacing.space_1) * 64;
        let indicator_width = (u64::from(theme.ui.selection.leading_indicator_width) * 64)
            .min(node.logical_rect.size.width_units());
        let mut indicator =
            NativeMaterialSpec::flat(NativeMaterialRole::SelectionIndicator, palette.focus);
        indicator.logical_rect = Some(LogicalRect::from_units(
            node.logical_rect.origin.x_units(),
            node.logical_rect.origin.y_units(),
            indicator_width,
            node.logical_rect.size.height_units(),
        ));
        indicator.corner_radius_units = indicator_width / 2;
        return vec![selection, indicator];
    }
    let material = match node.role {
        PresentationNodeRole::Workspace => Some(NativeMaterialSpec::flat(
            NativeMaterialRole::Canvas,
            palette.canvas,
        )),
        PresentationNodeRole::Header | PresentationNodeRole::Status => Some(
            NativeMaterialSpec::flat(NativeMaterialRole::ChromeSurface, palette.chrome_surface),
        ),
        PresentationNodeRole::Pane => {
            let mut spec =
                NativeMaterialSpec::flat(NativeMaterialRole::PaneSurface, palette.pane_surface);
            if node.state.floating {
                spec.corner_radius_units = u64::from(theme.ui.radii.floating) * 64;
                spec.boundary = Some(NativeBoundary {
                    width_units: u64::from(theme.ui.spacing.tiled_separator.max(1)) * 64,
                    color: palette.border_strong,
                });
                spec.raised_shadows = Some(theme.ui.elevation.raised);
            }
            Some(spec)
        }
        PresentationNodeRole::PaneBody => Some(NativeMaterialSpec::flat(
            NativeMaterialRole::PaneSurface,
            palette.pane_surface,
        )),
        PresentationNodeRole::Overlay => {
            let mut surface = NativeMaterialSpec::flat(
                NativeMaterialRole::OverlaySurface,
                palette.overlay_surface,
            );
            surface.corner_radius_units = match node.state.overlay_kind {
                Some(OverlayPresentationKind::ContextMenu) => {
                    u64::from(theme.ui.radii.context_menu) * 64
                }
                _ => u64::from(theme.ui.radii.overlay) * 64,
            };
            surface.boundary = Some(NativeBoundary {
                width_units: u64::from(theme.ui.spacing.tiled_separator.max(1)) * 64,
                color: palette.border_strong,
            });
            surface.raised_shadows = Some(theme.ui.elevation.raised);
            if node.state.overlay_kind == Some(OverlayPresentationKind::Modal) {
                let mut scrim =
                    NativeMaterialSpec::flat(NativeMaterialRole::ModalScrim, palette.modal_scrim);
                scrim.logical_rect = Some(viewport_rect);
                scrim.clip = Some(viewport_rect);
                return vec![scrim, surface];
            }
            Some(surface)
        }
        PresentationNodeRole::OverlayTitle => {
            // The parent overlay strokes its boundary inside its own top edge,
            // which the title band shares; the band draws later, so it must
            // start below the stroke or it paints the top border out.
            let stroke = (u64::from(theme.ui.spacing.tiled_separator.max(1)) * 64)
                .min(node.logical_rect.size.height_units());
            let mut band =
                NativeMaterialSpec::flat(NativeMaterialRole::OverlayBand, palette.chrome_surface);
            band.logical_rect = Some(LogicalRect::from_units(
                node.logical_rect.origin.x_units(),
                node.logical_rect
                    .origin
                    .y_units()
                    .saturating_add_unsigned(stroke),
                node.logical_rect.size.width_units(),
                node.logical_rect.size.height_units() - stroke,
            ));
            Some(band)
        }
        PresentationNodeRole::OverlayFooter | PresentationNodeRole::TextInput => Some(
            NativeMaterialSpec::flat(NativeMaterialRole::OverlayBand, palette.chrome_surface),
        ),
        PresentationNodeRole::Separator => {
            if node.state.dragging {
                Some(NativeMaterialSpec::flat(
                    NativeMaterialRole::Focus,
                    palette.focus,
                ))
            } else if node.state.hovered {
                Some(NativeMaterialSpec::flat(
                    NativeMaterialRole::BorderStrong,
                    palette.border_strong,
                ))
            } else {
                Some(NativeMaterialSpec::flat(
                    NativeMaterialRole::BorderSubtle,
                    palette.border_subtle,
                ))
            }
        }
        PresentationNodeRole::Attention => {
            semantic_chip_material(NativeMaterialRole::Attention, node.state.tone, theme)
        }
        PresentationNodeRole::PaneTitle if node.state.floating => None,
        PresentationNodeRole::PaneTitle => Some(NativeMaterialSpec::flat(
            NativeMaterialRole::ChromeSurface,
            palette.chrome_surface,
        )),
        PresentationNodeRole::PaneBadge(_) => {
            semantic_chip_material(NativeMaterialRole::Badge, node.state.tone, theme)
        }
        PresentationNodeRole::FocusIndicator => Some(NativeMaterialSpec::flat(
            NativeMaterialRole::Focus,
            palette.focus,
        )),
        PresentationNodeRole::TerminalOutput | PresentationNodeRole::Item => None,
        PresentationNodeRole::TaskOutput => Some(NativeMaterialSpec::flat(
            NativeMaterialRole::WorkflowConsole,
            palette.canvas,
        )),
        PresentationNodeRole::Workflow(role) => match role {
            mandatum_scene::WorkflowRowRole::Heading
            | mandatum_scene::WorkflowRowRole::Metadata
            | mandatum_scene::WorkflowRowRole::Status => None,
            mandatum_scene::WorkflowRowRole::Callout => {
                let mut spec = NativeMaterialSpec::flat(
                    NativeMaterialRole::WorkflowCallout,
                    palette.chrome_surface,
                );
                spec.corner_radius_units = u64::from(theme.ui.spacing.space_1) * 64;
                spec.boundary = Some(NativeBoundary {
                    width_units: u64::from(theme.ui.spacing.tiled_separator.max(1)) * 64,
                    color: tone_color(node.state.tone, theme),
                });
                Some(spec)
            }
            mandatum_scene::WorkflowRowRole::List => Some(NativeMaterialSpec::flat(
                NativeMaterialRole::ArtifactInspector,
                palette.pane_surface,
            )),
            mandatum_scene::WorkflowRowRole::Console => Some(NativeMaterialSpec::flat(
                NativeMaterialRole::WorkflowConsole,
                palette.canvas,
            )),
            mandatum_scene::WorkflowRowRole::ArtifactInspector => Some(NativeMaterialSpec::flat(
                NativeMaterialRole::ArtifactInspector,
                palette.chrome_surface,
            )),
        },
        // The status word reads as tone-colored bold text; a container around
        // a one-line label was chip noise on the first row of every pane.
        PresentationNodeRole::WorkflowStatusBadge => None,
        PresentationNodeRole::ArtifactCanvas => Some(NativeMaterialSpec::flat(
            NativeMaterialRole::ArtifactCanvas,
            palette.canvas,
        )),
    };
    material.into_iter().collect()
}

fn text_for_node(
    node: &PresentationNode,
    theme: &Theme,
) -> Option<(NativeTextMetricRole, UiColor)> {
    if node.state.hidden {
        return None;
    }
    let palette = theme.ui.palette;
    let color = if node.state.disabled {
        palette.text_muted
    } else if node.state.attention {
        palette.failure
    } else {
        palette.text_primary
    };
    match node.role {
        PresentationNodeRole::Header => Some((NativeTextMetricRole::Title, palette.text_primary)),
        PresentationNodeRole::Status => {
            Some((NativeTextMetricRole::Metadata, palette.text_secondary))
        }
        PresentationNodeRole::PaneTitle => {
            if node.state.focused {
                Some((NativeTextMetricRole::PaneTitleFocused, palette.focus))
            } else {
                Some((NativeTextMetricRole::PaneTitle, palette.text_secondary))
            }
        }
        PresentationNodeRole::PaneBadge(_) => Some((
            NativeTextMetricRole::Metadata,
            chip_text_color(node.state.tone, theme),
        )),
        PresentationNodeRole::TerminalOutput => Some((NativeTextMetricRole::Terminal, color)),
        PresentationNodeRole::TaskOutput => Some((NativeTextMetricRole::Body, color)),
        PresentationNodeRole::TextInput => Some((NativeTextMetricRole::Body, color)),
        PresentationNodeRole::Item => Some((NativeTextMetricRole::Body, color)),
        PresentationNodeRole::Workflow(role) => Some(match role {
            mandatum_scene::WorkflowRowRole::Heading => (
                NativeTextMetricRole::Title,
                tone_color(node.state.tone, theme),
            ),
            mandatum_scene::WorkflowRowRole::Status => (
                NativeTextMetricRole::Metadata,
                tone_color(node.state.tone, theme),
            ),
            mandatum_scene::WorkflowRowRole::Metadata
            | mandatum_scene::WorkflowRowRole::ArtifactInspector => {
                (NativeTextMetricRole::Metadata, palette.text_secondary)
            }
            mandatum_scene::WorkflowRowRole::Callout => (
                NativeTextMetricRole::Body,
                tone_color(node.state.tone, theme),
            ),
            mandatum_scene::WorkflowRowRole::List => {
                (NativeTextMetricRole::Metadata, palette.text_secondary)
            }
            mandatum_scene::WorkflowRowRole::Console => {
                (NativeTextMetricRole::Terminal, palette.text_primary)
            }
        }),
        PresentationNodeRole::WorkflowStatusBadge => Some((
            NativeTextMetricRole::Metadata,
            chip_text_color(node.state.tone, theme),
        )),
        PresentationNodeRole::OverlayTitle => {
            Some((NativeTextMetricRole::Title, palette.text_primary))
        }
        PresentationNodeRole::OverlayFooter => {
            Some((NativeTextMetricRole::Metadata, palette.text_muted))
        }
        PresentationNodeRole::Attention => Some((
            NativeTextMetricRole::Metadata,
            chip_text_color(node.state.tone, theme),
        )),
        PresentationNodeRole::Workspace
        | PresentationNodeRole::Pane
        | PresentationNodeRole::PaneBody
        | PresentationNodeRole::Overlay
        | PresentationNodeRole::ArtifactCanvas
        | PresentationNodeRole::FocusIndicator
        | PresentationNodeRole::Separator => None,
    }
}

fn native_text_color_projection(node: &PresentationNode) -> Option<SceneRect> {
    match node.role {
        PresentationNodeRole::Header
        | PresentationNodeRole::Status
        | PresentationNodeRole::PaneTitle
        | PresentationNodeRole::PaneBadge(_)
        | PresentationNodeRole::Attention
        | PresentationNodeRole::OverlayTitle
        | PresentationNodeRole::OverlayFooter
        | PresentationNodeRole::TextInput
        | PresentationNodeRole::Item
        | PresentationNodeRole::Workflow(_)
        | PresentationNodeRole::WorkflowStatusBadge => node.cell_rect,
        PresentationNodeRole::TerminalOutput
        | PresentationNodeRole::TaskOutput
        | PresentationNodeRole::Workspace
        | PresentationNodeRole::Pane
        | PresentationNodeRole::PaneBody
        | PresentationNodeRole::Overlay
        | PresentationNodeRole::ArtifactCanvas
        | PresentationNodeRole::FocusIndicator
        | PresentationNodeRole::Separator => None,
    }
}

/// Low-alpha tone tint blended over the rail behind a chip, following the
/// `selection_fill` treatment: tint the fill, never outline the box. 16 is
/// the strongest tint that keeps every chip tone at normal-text contrast
/// (4.5:1) on its tinted rail across the dark and light themes under the
/// renderer's actual linear-space composite; dark `failure` on chrome is
/// the binding pair. High-contrast never reaches this constant: its 7:1 bar
/// leaves no headroom for any visible tint, so its chips drop the container
/// entirely (below).
const CHIP_FILL_ALPHA: u8 = 16;

fn semantic_chip_material(
    role: NativeMaterialRole,
    tone: PresentationTone,
    theme: &Theme,
) -> Option<NativeMaterialSpec> {
    // Neutral badges carry no state worth a container; they render as plain
    // muted text.
    if tone == PresentationTone::Neutral {
        return None;
    }
    // High-contrast renders chips as plain tone-colored text: a tint strong
    // enough to see cannot hold the theme's 7:1 text bar, and an invisible
    // tint is a container in name only. Name match follows the established
    // high-contrast detection precedent in the theme crate.
    if theme.name == "mandatum-high-contrast" {
        return None;
    }
    let tone = tone_color(tone, theme);
    let mut spec = NativeMaterialSpec::flat(
        role,
        UiColor::rgba(tone.red, tone.green, tone.blue, CHIP_FILL_ALPHA),
    );
    spec.corner_radius_units = u64::from(theme.ui.spacing.space_1) * 64;
    Some(spec)
}

/// Chip glyph color paired with [`semantic_chip_material`]: tone-colored for
/// stateful chips, muted for the container-less neutral badge.
fn chip_text_color(tone: PresentationTone, theme: &Theme) -> UiColor {
    match tone {
        PresentationTone::Neutral => theme.ui.palette.text_muted,
        _ => tone_color(tone, theme),
    }
}

fn tone_color(tone: PresentationTone, theme: &Theme) -> UiColor {
    let palette = theme.ui.palette;
    match tone {
        PresentationTone::Neutral => palette.text_secondary,
        PresentationTone::Focus => palette.focus,
        PresentationTone::Running => palette.running,
        PresentationTone::Waiting => palette.waiting,
        PresentationTone::Failure => palette.failure,
        PresentationTone::Complete => palette.complete,
        PresentationTone::AgentIdentity => palette.agent_identity,
    }
}

fn rect_contains(parent: LogicalRect, child: LogicalRect) -> bool {
    child.origin.x_units() >= parent.origin.x_units()
        && child.origin.y_units() >= parent.origin.y_units()
        && child.right_units() <= parent.right_units()
        && child.bottom_units() <= parent.bottom_units()
}

fn cell_rect_within_scene(
    rect: mandatum_scene::SceneRect,
    size: mandatum_scene::SceneSize,
) -> bool {
    rect.width > 0 && rect.height > 0 && rect.right() <= size.width && rect.bottom() <= size.height
}

fn typography_generation(theme: &Theme) -> u64 {
    let typography = theme.ui.typography;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for (slot, style) in [
        typography.terminal,
        typography.title,
        typography.pane_title,
        typography.pane_title_focused,
        typography.body,
        typography.metadata,
        typography.key,
    ]
    .into_iter()
    .enumerate()
    {
        for byte in [
            slot as u8,
            (style.point_size_x64 & 0xff) as u8,
            (style.point_size_x64 >> 8) as u8,
            (style.line_height & 0xff) as u8,
            (style.line_height >> 8) as u8,
            style.face as u8,
        ] {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

fn enforce_limit(
    resource: &'static str,
    actual: usize,
    maximum: usize,
) -> Result<(), NativePresentationPlanError> {
    if actual > maximum {
        return Err(NativePresentationPlanError::ResourceLimit {
            resource,
            actual,
            maximum,
        });
    }
    Ok(())
}
