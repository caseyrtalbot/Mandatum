//! Builds the frontend-neutral workspace scene each frame from app state.
//!
//! The `mandatum-terminal-vt` -> `mandatum-scene` conversion lives here on
//! the app side: the scene crate never depends on the terminal engine, so no
//! parser type crosses the frontend seam (L1/L4).

use std::collections::{HashMap, HashSet, hash_map::Entry as HashMapEntry};
use std::sync::Arc;

use mandatum_agent_runtime::RiskLevel;
use mandatum_core::{AgentPaneIntent, PaneId, PaneKind, PaneSpec, Session, TaskPaneIntent};
use mandatum_scene::{
    AccessibilityActionKind, AccessibilityNode, AccessibilityRole, AccessibilityState,
    AgentApprovalPrompt, AgentContent, ArtifactState, CellOccupancy, EmptyContent, HeaderScene,
    HitTarget, HitTargetKind, LogicalHitTarget, LogicalRect, OverlayKind, OverlayNodePart,
    OverlayPresentationKind, OverlayScene, PaneBadgeKind, PaneContent, PaneNodePart, PaneScene,
    PaneSceneKind, PreeditScene, PresentationAxis, PresentationNode, PresentationNodeId,
    PresentationNodeRole, PresentationNodeState, PresentationTone, SceneCell, SceneCellStyle,
    SceneColor, SceneMotionPolicy, ScenePresentation, SceneRect, SceneSize, StatusScene,
    SurfacePosition, TaskContent, TaskStatusRole, TerminalProjection, TerminalSurface,
    TerminalViewportMapping, TextInputKind, TextInputScene, TransitionProperty, TransitionRole,
    TransitionTarget, ViewportMetrics, WorkflowNodePart, WorkspaceNodePart, WorkspaceScene,
    cell_program::display_width,
    layout::{self, PaneLayout},
};
use mandatum_terminal_vt::{
    CellStyle, Color as VtColor, TerminalCell, TerminalCellOccupancy, TerminalCursor, TerminalGrid,
};

use crate::{
    app_state::{AppState, CompositionTarget, agent_status_label},
    attention::header_scene,
    terminal_runtime::resolve_pane_cwd,
};

/// How many changed files an agent pane lists (most recent last).
const AGENT_CHANGED_FILES_SHOWN: usize = 10;

/// Read-only copy-mode view state for one pane, in absolute buffer
/// coordinates. The default follows live output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaneViewState {
    /// Rows scrolled up from the live bottom. `0` follows live output.
    pub(crate) scroll_offset: usize,
    /// Ordered selection span as `(row, column)` pairs.
    pub(crate) selection: Option<((usize, u16), (usize, u16))>,
    /// Copy-mode cursor; `Some` only while the pane is in copy mode.
    pub(crate) copy_cursor: Option<(usize, u16)>,
}

/// Retained per-pane build products, keyed by pane id and owned by
/// [`AppState`] between frames (the pure `build_workspace_scene*` entry
/// points use a throwaway cache and rebuild everything).
///
/// Two independent mechanisms live here:
///
/// 1. **Surface reuse** skips the [`terminal_surface`] grid walk when every
///    input that feeds it is provably unchanged (see [`TerminalSurfaceKey`]).
///    The paths that can change a pane's cells, and how each is covered:
///    - PTY output feeds: the parser's own `screen_changed` signal bumps the
///      per-pane grid revision in `AppState::apply_pty_runtime_event`.
///      Cursor-only motion also sets the parser's dirty flag (every cursor
///      move funnels through `set_cursor`), so cursor moves are covered.
///      Parser failures bump it too.
///    - Grid resize/rewrap and runtime lifecycle changes (spawn, restart,
///      task launch/rerun/stop, child exit, restore, session retire,
///      shutdown): `AppState` calls
///      `PaneSceneCache::invalidate_surfaces` at each of those choke
///      points, and the key additionally carries the pane's restart
///      generation plus the grid's live size/scrollback/cursor facts as
///      belt-and-braces.
///    - Scroll offset, selection, and the copy-mode cursor: the full
///      [`PaneViewState`] is part of the key.
///    - Theme changes never alter surfaces (cells carry semantic
///      `SceneColor`s), so they deliberately do not invalidate; renderers
///      key theme separately.
/// 2. **Revision settlement** decides each pane's published
///    `content_revision` by comparing the freshly assembled [`PaneContent`]
///    against the previous frame's, so the hint is proven by equality
///    rather than inferred from dirt tracking. A missed invalidation above
///    could cost a stale *reuse* only on a full key collision, while the
///    revision itself can only err in the safe direction (a spurious
///    bump). Cheap intent-driven content (task status rows, agent rows,
///    artifact labels) is rebuilt every frame and settled purely by this
///    comparison.
#[derive(Default)]
pub(crate) struct PaneSceneCache {
    entries: HashMap<PaneId, PaneSceneCacheEntry>,
    /// Cumulative surface builds that walked a grid (test observability).
    #[cfg(test)]
    pub(crate) surface_rebuilds: usize,
    /// Cumulative surface builds served from the cache (test observability).
    #[cfg(test)]
    pub(crate) surface_reuses: usize,
}

struct PaneSceneCacheEntry {
    revision: u64,
    content: PaneContent,
    terminal_key: Option<TerminalSurfaceKey>,
    task_output_key: Option<TerminalSurfaceKey>,
}

/// Every input [`terminal_surface`] reads, compared cheaply per frame; any
/// difference rebuilds the surface. [`task_output_surface`] derives its view
/// from grid content, so its key fixes `view` at the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalSurfaceKey {
    /// App-side per-pane feed counter, bumped on `screen_changed`. Never
    /// resets, so within one pane it is collision-free across feeds.
    grid_revision: u64,
    /// A fresh runtime never reuses its predecessor's surface, even before
    /// its first feed.
    restart_generation: u64,
    columns: u16,
    rows: u16,
    total_rows: usize,
    scrollback_len: usize,
    cursor: TerminalCursor,
    view: PaneViewState,
    max_width: u16,
    max_height: u16,
}

fn terminal_surface_key(
    state: &AppState,
    pane: &PaneSpec,
    grid: &TerminalGrid,
    view: PaneViewState,
    max_width: u16,
    max_height: u16,
) -> TerminalSurfaceKey {
    TerminalSurfaceKey {
        grid_revision: state.pane_grid_revision(pane.id()),
        restart_generation: pane.restart_generation(),
        columns: grid.size().columns(),
        rows: grid.size().rows(),
        total_rows: grid.total_rows(),
        scrollback_len: grid.scrollback_len(),
        cursor: grid.cursor(),
        view,
        max_width,
        max_height,
    }
}

impl PaneSceneCache {
    /// Drop every retained surface key so the next build re-walks each grid.
    /// Revisions and retained content survive: the post-build equality
    /// comparison keeps revisions honest across the forced rebuild.
    pub(crate) fn invalidate_surfaces(&mut self) {
        for entry in self.entries.values_mut() {
            entry.terminal_key = None;
            entry.task_output_key = None;
        }
    }

    fn cached_terminal_surface(
        &self,
        pane_id: &PaneId,
        key: &TerminalSurfaceKey,
    ) -> Option<TerminalSurface> {
        let entry = self.entries.get(pane_id)?;
        if entry.terminal_key.as_ref() != Some(key) {
            return None;
        }
        match &entry.content {
            PaneContent::Terminal(surface) => Some(surface.clone()),
            _ => None,
        }
    }

    fn cached_task_output_surface(
        &self,
        pane_id: &PaneId,
        key: &TerminalSurfaceKey,
    ) -> Option<TerminalSurface> {
        let entry = self.entries.get(pane_id)?;
        if entry.task_output_key.as_ref() != Some(key) {
            return None;
        }
        match &entry.content {
            PaneContent::Task(task) => task.output.clone(),
            _ => None,
        }
    }

    /// Settle a pane's published revision once its content is fully
    /// assembled. `terminal_content_reused` marks content cloned verbatim
    /// from this entry (a terminal-pane surface hit), which skips the
    /// equality walk. New and changed panes take a revision of at least the
    /// current scene generation, which keeps revisions from regressing into
    /// previously published values even if an entry was pruned in between
    /// (any pane removal or re-admission moves the generation).
    fn settle(
        &mut self,
        scene_generation: u64,
        pane_id: &PaneId,
        content: &PaneContent,
        terminal_key: Option<TerminalSurfaceKey>,
        task_output_key: Option<TerminalSurfaceKey>,
        terminal_content_reused: bool,
    ) -> u64 {
        match self.entries.entry(pane_id.clone()) {
            HashMapEntry::Occupied(mut occupied) => {
                let entry = occupied.get_mut();
                let unchanged =
                    terminal_content_reused || pane_content_matches(&entry.content, content);
                if !unchanged {
                    entry.revision = entry.revision.saturating_add(1).max(scene_generation);
                    entry.content = content.clone();
                }
                entry.terminal_key = terminal_key;
                entry.task_output_key = task_output_key;
                entry.revision
            }
            HashMapEntry::Vacant(vacant) => {
                vacant
                    .insert(PaneSceneCacheEntry {
                        revision: scene_generation,
                        content: content.clone(),
                        terminal_key,
                        task_output_key,
                    })
                    .revision
            }
        }
    }

    /// Keep only entries for panes present in the built frame, bounding the
    /// retained content to the live workspace.
    fn retain_only(&mut self, panes: &[PaneScene]) {
        self.entries
            .retain(|pane_id, _| panes.iter().any(|pane| &pane.id == pane_id));
    }
}

/// Content equality with the artifact raster compared by identity
/// (dimensions, revision, and shared allocation) instead of by pixel bytes;
/// a false negative merely bumps the revision, which is the safe direction.
fn pane_content_matches(previous: &PaneContent, current: &PaneContent) -> bool {
    match (previous, current) {
        (PaneContent::Artifact(previous), PaneContent::Artifact(current)) => {
            previous.source_label == current.source_label
                && previous.alt_text == current.alt_text
                && previous.fit == current.fit
                && match (&previous.state, &current.state) {
                    (ArtifactState::Ready(previous), ArtifactState::Ready(current)) => {
                        previous.width == current.width
                            && previous.height == current.height
                            && previous.revision == current.revision
                            && Arc::ptr_eq(&previous.rgba8, &current.rgba8)
                    }
                    (previous, current) => previous == current,
                }
        }
        (previous, current) => previous == current,
    }
}

/// Build one frame of workspace scene from live app state.
pub fn build_workspace_scene(state: &AppState, size: SceneSize) -> WorkspaceScene {
    build_workspace_scene_with_viewport(state, ViewportMetrics::from_scene_size(size))
}

/// Build one frame from a coherent shell-provided logical/physical viewport.
///
/// Pure entry point: a throwaway pane cache rebuilds every pane, so repeated
/// calls on one unchanged `&AppState` stay deterministic. The production
/// path (`AppState::build_scene_with_viewport`) threads the retained cache.
pub fn build_workspace_scene_with_viewport(
    state: &AppState,
    viewport: ViewportMetrics,
) -> WorkspaceScene {
    build_workspace_scene_cached(state, viewport, &mut PaneSceneCache::default())
}

/// Build one frame, reusing per-pane surfaces retained in `cache` for panes
/// whose build inputs are provably unchanged.
pub(crate) fn build_workspace_scene_cached(
    state: &AppState,
    viewport: ViewportMetrics,
    cache: &mut PaneSceneCache,
) -> WorkspaceScene {
    let size = viewport.scene_size();
    let workspace = state.workspace();
    let session = workspace.active_session();
    let area = layout::workspace_scene_area(size);

    let panes = layout::layout_panes(workspace, area)
        .into_iter()
        .filter_map(|placed| {
            session
                .pane(&placed.pane_id)
                .map(|pane| pane_scene(state, session, pane, placed, cache))
        })
        .collect::<Vec<_>>();
    cache.retain_only(&panes);

    // Overlay surfaces are mutually exclusive; Welcome and Context Menu keep
    // their distinct non-modal/anchored presentation grammar.
    let mut overlay = state
        .context_menu_overlay(size)
        .map(OverlayScene::ContextMenu)
        .or_else(|| state.palette_overlay(size).map(OverlayScene::Palette))
        .or_else(|| {
            state
                .timeline_overlay_scene(size)
                .map(OverlayScene::Timeline)
        })
        .or_else(|| state.search_overlay_scene(size).map(OverlayScene::Search))
        .or_else(|| {
            state
                .session_map_overlay_scene(size)
                .map(OverlayScene::SessionMap)
        })
        .or_else(|| state.prompt_overlay_scene(size).map(OverlayScene::Prompt))
        .or_else(|| state.help_overlay_scene(size).map(OverlayScene::Help))
        .or_else(|| {
            state
                .appearance_overlay_scene(size)
                .map(OverlayScene::Appearance)
        })
        // The first-run note is last: it is not modal, so any real overlay
        // outranks it (and the action that opened one dismissed it anyway).
        .or_else(|| state.welcome_overlay_scene(size).map(OverlayScene::Welcome));
    if let Some(overlay) = overlay.as_mut() {
        constrain_overlay_area(overlay, viewport, state.theme());
    }

    // The attention strip: approvals, failed tasks, stuck agents — or calm
    // session facts. Composed here so `&WorkspaceScene` alone paints a frame.
    let header = header_scene(state, layout::header_rect(size));
    let status = StatusScene {
        area: layout::status_rect(size),
        text: status_text(state),
    };
    let hit_targets = hit_targets(workspace, &panes, &header, size, overlay.as_ref());
    let text_input = text_input_scene(state, &panes, overlay.as_ref());
    let presentation = scene_presentation(
        state,
        viewport,
        &panes,
        &header,
        &status,
        overlay.as_ref(),
        &hit_targets,
    );

    WorkspaceScene {
        size,
        header,
        panes,
        overlay,
        status,
        focused_pane: session.focused_pane_id().clone(),
        hit_targets,
        copy_mode: state.copy_mode_active(),
        text_input,
        presentation,
    }
}

fn scene_presentation(
    state: &AppState,
    viewport: ViewportMetrics,
    panes: &[PaneScene],
    header: &HeaderScene,
    status: &StatusScene,
    overlay: Option<&OverlayScene>,
    hit_targets: &[HitTarget],
) -> ScenePresentation {
    let motion_policy = state.scene_motion_policy();
    let workspace_id = PresentationNodeId::workspace(WorkspaceNodePart::Surface);
    let mut nodes = vec![presentation_node(
        workspace_id.clone(),
        None,
        PresentationNodeRole::Workspace,
        PresentationNodeState::default(),
        SceneRect::new(
            0,
            0,
            viewport.scene_size().width,
            viewport.scene_size().height,
        ),
        viewport,
    )];

    let header_id = PresentationNodeId::workspace(WorkspaceNodePart::Header);
    nodes.push(presentation_node(
        header_id.clone(),
        Some(workspace_id.clone()),
        PresentationNodeRole::Header,
        PresentationNodeState::default(),
        header.area,
        viewport,
    ));
    let status_id = PresentationNodeId::workspace(WorkspaceNodePart::Status);
    nodes.push(presentation_node(
        status_id.clone(),
        Some(workspace_id.clone()),
        PresentationNodeRole::Status,
        PresentationNodeState::default(),
        status.area,
        viewport,
    ));

    let separators = layout::layout_separators(
        state.workspace(),
        layout::workspace_scene_area(viewport.scene_size()),
    );
    let mut terminal_viewports = Vec::new();
    let mut transition_targets = Vec::new();
    for pane in panes.iter().filter(|pane| !pane.floating) {
        push_pane_presentation(
            state,
            pane,
            viewport,
            &workspace_id,
            &mut nodes,
            &mut terminal_viewports,
            &mut transition_targets,
            motion_policy,
        );
    }

    for separator in &separators {
        let id = PresentationNodeId::workspace(WorkspaceNodePart::Separator {
            split_index: separator.split_index,
            axis: PresentationAxis::from(separator.axis),
        });
        let (visible_rect, _) = separator_logical_rects(separator, viewport);
        let hovered = state.hovered_separator() == Some(separator.split_index);
        let dragging = state.dragged_separator() == Some(separator.split_index);
        nodes.push(presentation_logical_node(
            id,
            Some(workspace_id.clone()),
            PresentationNodeRole::Separator,
            PresentationNodeState {
                hovered,
                dragging,
                tone: if hovered || dragging {
                    PresentationTone::Focus
                } else {
                    PresentationTone::Neutral
                },
                ..PresentationNodeState::default()
            },
            visible_rect,
            TerminalProjection::CellRegions(vec![separator.area]),
        ));
    }

    // Floating panes are above the tiled separator plane. Keeping this order
    // in the typed scene lets every adapter preserve the same occlusion.
    for pane in panes.iter().filter(|pane| pane.floating) {
        push_pane_presentation(
            state,
            pane,
            viewport,
            &workspace_id,
            &mut nodes,
            &mut terminal_viewports,
            &mut transition_targets,
            motion_policy,
        );
    }

    if let Some(overlay) = overlay {
        let kind = overlay.kind();
        let overlay_id = PresentationNodeId::overlay(kind, OverlayNodePart::Surface);
        nodes.push(presentation_node(
            overlay_id.clone(),
            Some(workspace_id.clone()),
            PresentationNodeRole::Overlay,
            PresentationNodeState {
                overlay_kind: Some(overlay_presentation_kind(overlay)),
                ..PresentationNodeState::default()
            },
            overlay_area(overlay),
            viewport,
        ));
        push_overlay_band_nodes(overlay, viewport, state.theme(), &overlay_id, &mut nodes);
    }

    let mut logical_hit_targets = Vec::new();
    for target in hit_targets {
        let Some(node_id) = presentation_id_for_hit_target(target, overlay) else {
            continue;
        };
        if !nodes.iter().any(|node| node.id == node_id) {
            let parent = if matches!(
                target.kind,
                HitTargetKind::PaletteItem(_)
                    | HitTargetKind::ContextMenuItem(_)
                    | HitTargetKind::TimelineItem(_)
                    | HitTargetKind::SessionMapRow(_)
                    | HitTargetKind::SearchItem(_)
            ) {
                overlay.map(|value| {
                    PresentationNodeId::overlay(value.kind(), OverlayNodePart::Surface)
                })
            } else {
                Some(workspace_id.clone())
            };
            let mut node = presentation_node(
                node_id.clone(),
                parent,
                presentation_role_for_hit_target(&target.kind),
                presentation_state_for_hit_target(&target.kind, overlay),
                target.rect,
                viewport,
            );
            if is_overlay_item_target(&target.kind) {
                node.logical_rect = overlay_row_logical_rect(target.rect, viewport, state.theme());
            }
            nodes.push(node);
        }
        let logical_rect = if is_overlay_item_target(&target.kind) {
            nodes
                .iter()
                .find(|node| node.id == node_id)
                .map(|node| node.logical_rect)
                .unwrap_or_else(|| logical_hit_rect(target, &separators, viewport))
        } else {
            logical_hit_rect(target, &separators, viewport)
        };
        logical_hit_targets.push(LogicalHitTarget {
            node_id,
            logical_rect,
            kind: target.kind.clone(),
        });
    }

    // Independent emphasis and family motion are both semantic eligibility.
    // The renderer gives presence-changing family transitions precedence and
    // uses Focus for stable-surface emphasis changes. Selection deliberately
    // emits no transition: a highlight that tracks the pointer or key repeat
    // must land whole on the next frame, not ease toward it.
    if motion_policy.allows(TransitionRole::ApprovalArrival) {
        for pane in panes {
            let Some(sequence) = state.approval_arrival_sequence(&pane.id) else {
                continue;
            };
            let node_id = PresentationNodeId::pane(
                pane.id.clone(),
                PaneNodePart::Workflow(WorkflowNodePart::Approval),
            );
            if nodes.iter().any(|node| node.id == node_id) {
                push_unique_transition_with_sequence(
                    &mut transition_targets,
                    node_id,
                    TransitionRole::ApprovalArrival,
                    TransitionProperty::Scale,
                    sequence,
                );
            }
        }
    }
    if motion_policy.allows(TransitionRole::PaneGeometry) {
        for pane in panes {
            let root = PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Surface);
            for node in transition_family_nodes(&nodes, &root)
                .into_iter()
                .filter(|node| node_has_material_motion_surface(node))
            {
                push_unique_transition(
                    &mut transition_targets,
                    node.id.clone(),
                    TransitionRole::PaneGeometry,
                    TransitionProperty::Geometry,
                );
            }
        }
        for node in nodes
            .iter()
            .filter(|node| node.role == PresentationNodeRole::Separator)
        {
            push_unique_transition(
                &mut transition_targets,
                node.id.clone(),
                TransitionRole::PaneGeometry,
                TransitionProperty::Geometry,
            );
        }
    }
    if motion_policy.allows(TransitionRole::Overlay)
        && let Some(overlay) = overlay
    {
        let root = PresentationNodeId::overlay(overlay.kind(), OverlayNodePart::Surface);
        for node in transition_family_nodes(&nodes, &root) {
            push_unique_transition(
                &mut transition_targets,
                node.id.clone(),
                TransitionRole::Overlay,
                TransitionProperty::Opacity,
            );
            if node_has_material_motion_surface(node) {
                push_unique_transition(
                    &mut transition_targets,
                    node.id.clone(),
                    TransitionRole::Overlay,
                    TransitionProperty::Scale,
                );
            }
        }
    }

    let accessibility_nodes = accessibility_nodes(
        panes,
        header,
        status,
        overlay,
        &nodes,
        viewport,
        &workspace_id,
    );

    ScenePresentation {
        viewport: Some(viewport),
        density: state.density(),
        motion_policy,
        nodes,
        logical_hit_targets,
        terminal_viewports,
        transition_targets,
        accessibility_nodes,
    }
}

fn transition_family_nodes<'a>(
    nodes: &'a [PresentationNode],
    root: &PresentationNodeId,
) -> Vec<&'a PresentationNode> {
    let mut family = HashSet::new();
    let mut result = Vec::new();
    for node in nodes {
        if &node.id == root
            || node
                .parent
                .as_ref()
                .is_some_and(|parent| family.contains(parent))
        {
            family.insert(node.id.clone());
            result.push(node);
        }
    }
    result
}

/// Geometry and scale are material-only until the native text renderer owns
/// plan-driven glyph transforms. Text scopes, terminal/task output, and
/// artifact pixels remain cell/raster-positioned directly even when a
/// material-backed node also advertises motion for its surface.
fn node_has_material_motion_surface(node: &PresentationNode) -> bool {
    match node.role {
        PresentationNodeRole::Pane
        | PresentationNodeRole::PaneBody
        | PresentationNodeRole::PaneBadge(_)
        | PresentationNodeRole::FocusIndicator
        | PresentationNodeRole::Separator
        | PresentationNodeRole::WorkflowStatusBadge
        | PresentationNodeRole::ArtifactCanvas
        | PresentationNodeRole::Overlay
        | PresentationNodeRole::OverlayTitle
        | PresentationNodeRole::OverlayFooter
        | PresentationNodeRole::TextInput => true,
        PresentationNodeRole::PaneTitle => !node.state.floating,
        PresentationNodeRole::Item => node.state.selected,
        PresentationNodeRole::Workflow(role) => matches!(
            role,
            mandatum_scene::WorkflowRowRole::Callout
                | mandatum_scene::WorkflowRowRole::List
                | mandatum_scene::WorkflowRowRole::Console
                | mandatum_scene::WorkflowRowRole::ArtifactInspector
        ),
        PresentationNodeRole::Workspace
        | PresentationNodeRole::Header
        | PresentationNodeRole::Status
        | PresentationNodeRole::TerminalOutput
        | PresentationNodeRole::TaskOutput
        | PresentationNodeRole::Attention => false,
    }
}

fn push_unique_transition(
    targets: &mut Vec<TransitionTarget>,
    node_id: PresentationNodeId,
    role: TransitionRole,
    property: TransitionProperty,
) {
    push_unique_transition_with_sequence(targets, node_id, role, property, 0);
}

fn push_unique_transition_with_sequence(
    targets: &mut Vec<TransitionTarget>,
    node_id: PresentationNodeId,
    role: TransitionRole,
    property: TransitionProperty,
    sequence: u64,
) {
    let target = TransitionTarget {
        node_id,
        role,
        property,
        sequence,
    };
    if !targets.contains(&target) {
        targets.push(target);
    }
}

fn is_overlay_item_target(kind: &HitTargetKind) -> bool {
    matches!(
        kind,
        HitTargetKind::PaletteItem(_)
            | HitTargetKind::ContextMenuItem(_)
            | HitTargetKind::TimelineItem(_)
            | HitTargetKind::SessionMapRow(_)
            | HitTargetKind::SearchItem(_)
    )
}

fn overlay_presentation_kind(overlay: &OverlayScene) -> OverlayPresentationKind {
    match overlay {
        OverlayScene::Welcome(_) => OverlayPresentationKind::Welcome,
        OverlayScene::ContextMenu(_) => OverlayPresentationKind::ContextMenu,
        OverlayScene::Palette(_)
        | OverlayScene::Timeline(_)
        | OverlayScene::SessionMap(_)
        | OverlayScene::Prompt(_)
        | OverlayScene::Search(_)
        | OverlayScene::Help(_)
        | OverlayScene::Appearance(_) => OverlayPresentationKind::Modal,
    }
}

fn push_overlay_band_nodes(
    overlay: &OverlayScene,
    viewport: ViewportMetrics,
    theme: &mandatum_scene::Theme,
    overlay_id: &PresentationNodeId,
    nodes: &mut Vec<PresentationNode>,
) {
    let area = overlay_area(overlay);
    let kind = overlay.kind();
    if !matches!(overlay, OverlayScene::ContextMenu(_)) {
        let title = SceneRect::new(area.x, area.y, area.width, area.height.min(1));
        if !title.is_empty() {
            nodes.push(presentation_cell_logical_node(
                PresentationNodeId::overlay(kind, OverlayNodePart::Title),
                Some(overlay_id.clone()),
                PresentationNodeRole::OverlayTitle,
                PresentationNodeState::default(),
                title,
                inset_logical_rect(
                    viewport.logical_rect_for_cells(title),
                    theme.ui.radii.overlay,
                ),
            ));
        }
    }

    let inner = layout::pane_inner_rect(area);
    if matches!(
        overlay,
        OverlayScene::Palette(_)
            | OverlayScene::Timeline(_)
            | OverlayScene::Prompt(_)
            | OverlayScene::Search(_)
            | OverlayScene::Help(_)
    ) && !inner.is_empty()
    {
        let input = layout::filtered_overlay_input_rect(inner);
        nodes.push(presentation_cell_logical_node(
            PresentationNodeId::overlay(kind, OverlayNodePart::Input),
            Some(overlay_id.clone()),
            PresentationNodeRole::TextInput,
            PresentationNodeState::default(),
            input,
            inset_logical_rect(
                viewport.logical_rect_for_cells(input),
                theme.ui.spacing.overlay_row_padding_x,
            ),
        ));
    }

    let footer = match overlay {
        OverlayScene::Palette(_)
        | OverlayScene::Timeline(_)
        | OverlayScene::Prompt(_)
        | OverlayScene::Search(_)
        | OverlayScene::Help(_) => layout::filtered_overlay_footer_rect(inner),
        OverlayScene::SessionMap(_) => layout::footer_only_overlay_footer_rect(inner),
        OverlayScene::Welcome(welcome) => {
            let row = welcome.entries.len().saturating_add(3) as u16;
            (row < inner.height).then_some(SceneRect::new(
                inner.x,
                inner.y.saturating_add(row),
                inner.width,
                1,
            ))
        }
        _ => None,
    };
    if let Some(footer) = footer {
        nodes.push(presentation_cell_logical_node(
            PresentationNodeId::overlay(kind, OverlayNodePart::Footer),
            Some(overlay_id.clone()),
            PresentationNodeRole::OverlayFooter,
            PresentationNodeState::default(),
            footer,
            inset_logical_rect(
                viewport.logical_rect_for_cells(footer),
                theme.ui.spacing.overlay_row_padding_x,
            ),
        ));
    }

    if let OverlayScene::Help(help) = overlay {
        for (row, index) in
            layout::palette_item_window(inner, help.items.len(), help.selected).enumerate()
        {
            let item = &help.items[index];
            let Some(cell_rect) = layout::palette_item_rect(inner, row) else {
                continue;
            };
            nodes.push(presentation_cell_logical_node(
                PresentationNodeId::overlay_item(OverlayKind::Help, item.key.clone()),
                Some(overlay_id.clone()),
                PresentationNodeRole::Item,
                PresentationNodeState {
                    selected: help.selected == Some(index),
                    ..PresentationNodeState::default()
                },
                cell_rect,
                overlay_row_logical_rect(cell_rect, viewport, theme),
            ));
        }
    }
}

fn inset_logical_rect(rect: LogicalRect, inset_pixels: u16) -> LogicalRect {
    let inset = (u64::from(inset_pixels) * 64)
        .min(rect.size.width_units().saturating_sub(64).saturating_div(2));
    LogicalRect::from_units(
        rect.origin.x_units().saturating_add_unsigned(inset),
        rect.origin.y_units(),
        rect.size.width_units().saturating_sub(inset * 2),
        rect.size.height_units(),
    )
}

fn overlay_row_logical_rect(
    cell_rect: SceneRect,
    viewport: ViewportMetrics,
    theme: &mandatum_scene::Theme,
) -> LogicalRect {
    let rect = viewport.logical_rect_for_cells(cell_rect);
    // Half the band padding: row text starts one cell (~overlay_row_padding_x)
    // in, so the selection fill and its leading indicator must begin left of
    // the first glyph or the highlight edge lands exactly on it. The smaller
    // inset leaves visible fill padding before the text, the way native menu
    // selection reads everywhere else on the platform.
    inset_logical_rect(rect, theme.ui.spacing.space_1)
}

#[allow(clippy::too_many_arguments)]
fn push_pane_presentation(
    state: &AppState,
    pane: &PaneScene,
    viewport: ViewportMetrics,
    workspace_id: &PresentationNodeId,
    nodes: &mut Vec<PresentationNode>,
    terminal_viewports: &mut Vec<TerminalViewportMapping>,
    transition_targets: &mut Vec<TransitionTarget>,
    motion_policy: SceneMotionPolicy,
) {
    let pane_id = PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Surface);
    nodes.push(presentation_node(
        pane_id.clone(),
        Some(workspace_id.clone()),
        PresentationNodeRole::Pane,
        PresentationNodeState {
            focused: pane.focused,
            floating: pane.floating,
            ..PresentationNodeState::default()
        },
        pane.area,
        viewport,
    ));

    let title_id = PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Title);
    let title_rect = SceneRect::new(pane.area.x, pane.area.y, pane.area.width, 1);
    let title_cell_rect = viewport.logical_rect_for_cells(title_rect);
    // Density changes native rail breathing room while the honest one-cell
    // terminal projection and PTY geometry remain unchanged.
    let vertical_inset = match state.density() {
        mandatum_scene::UiDensity::Compact => 2 * 64,
        mandatum_scene::UiDensity::Comfortable => 0,
    };
    let title_logical_rect = LogicalRect::from_units(
        title_cell_rect.origin.x_units(),
        title_cell_rect
            .origin
            .y_units()
            .saturating_add(vertical_inset),
        title_cell_rect.size.width_units(),
        title_cell_rect
            .size
            .height_units()
            .saturating_sub((vertical_inset as u64).saturating_mul(2))
            .max(64),
    );
    nodes.push(presentation_cell_logical_node(
        title_id.clone(),
        Some(pane_id.clone()),
        PresentationNodeRole::PaneTitle,
        PresentationNodeState {
            focused: pane.focused,
            floating: pane.floating,
            tone: if pane.focused {
                PresentationTone::Focus
            } else {
                PresentationTone::Neutral
            },
            ..PresentationNodeState::default()
        },
        title_rect,
        title_logical_rect,
    ));
    if pane.focused {
        let focus_id = PresentationNodeId::pane(pane.id.clone(), PaneNodePart::FocusIndicator);
        nodes.push(presentation_logical_node(
            focus_id.clone(),
            Some(title_id.clone()),
            PresentationNodeRole::FocusIndicator,
            PresentationNodeState {
                focused: true,
                floating: pane.floating,
                tone: PresentationTone::Focus,
                ..PresentationNodeState::default()
            },
            LogicalRect::from_units(
                title_logical_rect.origin.x_units(),
                title_logical_rect.origin.y_units(),
                2 * 64,
                title_logical_rect.size.height_units(),
            ),
            TerminalProjection::CellRegions(vec![title_rect]),
        ));
        if motion_policy.allows(TransitionRole::Focus) {
            transition_targets.push(TransitionTarget {
                node_id: focus_id,
                role: TransitionRole::Focus,
                property: TransitionProperty::Scale,
                sequence: 0,
            });
        }
    }
    for (kind, cell_rect) in pane.badge_rects() {
        // Default panes are terminals; a "terminal" chip on every rail is
        // pure noise natively. The same holds for any kind chip that only
        // repeats the pane's own title ("agent" pane titled "agent"). The
        // terminal fallback keeps its badge cells.
        let repeats_title = matches!(
            kind,
            PaneBadgeKind::Task | PaneBadgeKind::Agent | PaneBadgeKind::Artifact
        ) && pane.title.trim().eq_ignore_ascii_case(kind.label());
        if kind == PaneBadgeKind::Terminal || repeats_title {
            continue;
        }
        let badge_cell_rect = viewport.logical_rect_for_cells(cell_rect);
        nodes.push(presentation_cell_logical_node(
            PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Badge(kind)),
            Some(title_id.clone()),
            PresentationNodeRole::PaneBadge(kind),
            PresentationNodeState {
                focused: pane.focused,
                floating: pane.floating,
                tone: pane_badge_tone(kind),
                ..PresentationNodeState::default()
            },
            cell_rect,
            LogicalRect::from_units(
                badge_cell_rect.origin.x_units(),
                title_logical_rect.origin.y_units(),
                badge_cell_rect.size.width_units(),
                title_logical_rect.size.height_units(),
            ),
        ));
    }

    let body_id = PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Body);
    nodes.push(presentation_node(
        body_id,
        Some(pane_id.clone()),
        PresentationNodeRole::PaneBody,
        PresentationNodeState {
            focused: pane.focused,
            floating: pane.floating,
            ..PresentationNodeState::default()
        },
        layout::pane_inner_rect(pane.area),
        viewport,
    ));
    push_workflow_presentation(pane, viewport, &pane_id, nodes);

    if let Some(mapping) = terminal_viewport_mapping(state, pane, viewport) {
        let role = match pane.content {
            PaneContent::Task(_) => PresentationNodeRole::TaskOutput,
            _ => PresentationNodeRole::TerminalOutput,
        };
        nodes.push(presentation_node(
            mapping.node_id.clone(),
            Some(pane_id),
            role,
            PresentationNodeState {
                focused: pane.focused,
                floating: pane.floating,
                ..PresentationNodeState::default()
            },
            mapping.visible_cell_rect,
            viewport,
        ));
        terminal_viewports.push(mapping);
    }
}

fn push_workflow_presentation(
    pane: &PaneScene,
    viewport: ViewportMetrics,
    pane_id: &PresentationNodeId,
    nodes: &mut Vec<PresentationNode>,
) {
    let inner = layout::pane_inner_rect(pane.area);
    let rows = pane.workflow_rows();
    if pane.area.is_empty() {
        return;
    }
    let hidden_anchor = SceneRect::new(pane.area.x, pane.area.y, 1, 1);
    let mut start = 0usize;
    while start < rows.len() {
        let seed = &rows[start];
        let mut end = start + 1;
        while end < rows.len()
            && rows[end].part == seed.part
            && rows[end].role == seed.role
            && rows[end].tone == seed.tone
        {
            end += 1;
        }
        let visible = start < usize::from(inner.height) && !inner.is_empty();
        let id = PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Workflow(seed.part));
        let state = PresentationNodeState {
            attention: seed.role == mandatum_scene::WorkflowRowRole::Callout
                && matches!(
                    seed.tone,
                    PresentationTone::Failure | PresentationTone::Waiting
                ),
            tone: seed.tone,
            hidden: !visible,
            ..PresentationNodeState::default()
        };
        if visible {
            let remaining = usize::from(inner.height).saturating_sub(start);
            let height = u16::try_from((end - start).min(remaining)).unwrap_or(u16::MAX);
            nodes.push(presentation_node(
                id,
                Some(pane_id.clone()),
                PresentationNodeRole::Workflow(seed.role),
                state,
                SceneRect::new(
                    inner.x,
                    inner.y.saturating_add(start as u16),
                    inner.width,
                    height,
                ),
                viewport,
            ));
        } else {
            nodes.push(presentation_logical_node(
                id,
                Some(pane_id.clone()),
                PresentationNodeRole::Workflow(seed.role),
                state,
                viewport.logical_rect_for_cells(hidden_anchor),
                TerminalProjection::CellRegions(Vec::new()),
            ));
        }
        start = end;
    }

    if let Some(badge) = pane.workflow_status_badge() {
        let visible = badge.row < usize::from(inner.height) && !inner.is_empty();
        let id = PresentationNodeId::pane(
            pane.id.clone(),
            PaneNodePart::Workflow(WorkflowNodePart::Status),
        );
        let state = PresentationNodeState {
            tone: badge.tone,
            hidden: !visible,
            ..PresentationNodeState::default()
        };
        if visible {
            // The status renders as tone-colored bold text with no container,
            // so the node covers exactly the label's cells.
            let width = u16::try_from(display_width(&badge.label))
                .unwrap_or(u16::MAX)
                .min(inner.width);
            nodes.push(presentation_node(
                id,
                Some(pane_id.clone()),
                PresentationNodeRole::WorkflowStatusBadge,
                state,
                SceneRect::new(
                    inner.x,
                    inner.y.saturating_add(badge.row as u16),
                    width.max(1),
                    1,
                ),
                viewport,
            ));
        } else {
            nodes.push(presentation_logical_node(
                id,
                Some(pane_id.clone()),
                PresentationNodeRole::WorkflowStatusBadge,
                state,
                viewport.logical_rect_for_cells(hidden_anchor),
                TerminalProjection::CellRegions(Vec::new()),
            ));
        }
    }

    if matches!(pane.content, PaneContent::Artifact(_)) {
        let canvas_y = inner
            .y
            .saturating_add(u16::try_from(rows.len()).unwrap_or(u16::MAX));
        let canvas = SceneRect::new(
            inner.x,
            canvas_y.min(inner.bottom()),
            inner.width,
            inner.bottom().saturating_sub(canvas_y),
        );
        if canvas.is_empty() {
            nodes.push(presentation_logical_node(
                PresentationNodeId::pane(
                    pane.id.clone(),
                    PaneNodePart::Workflow(WorkflowNodePart::ArtifactCanvas),
                ),
                Some(pane_id.clone()),
                PresentationNodeRole::ArtifactCanvas,
                PresentationNodeState {
                    hidden: true,
                    ..PresentationNodeState::default()
                },
                viewport.logical_rect_for_cells(hidden_anchor),
                TerminalProjection::CellRegions(Vec::new()),
            ));
        } else {
            nodes.push(presentation_node(
                PresentationNodeId::pane(
                    pane.id.clone(),
                    PaneNodePart::Workflow(WorkflowNodePart::ArtifactCanvas),
                ),
                Some(pane_id.clone()),
                PresentationNodeRole::ArtifactCanvas,
                PresentationNodeState::default(),
                canvas,
                viewport,
            ));
        }
    }
}

fn presentation_node(
    id: PresentationNodeId,
    parent: Option<PresentationNodeId>,
    role: PresentationNodeRole,
    state: PresentationNodeState,
    cell_rect: SceneRect,
    viewport: ViewportMetrics,
) -> PresentationNode {
    PresentationNode {
        id,
        parent,
        role,
        state,
        logical_rect: viewport.logical_rect_for_cells(cell_rect),
        cell_rect: Some(cell_rect),
        terminal_projection: TerminalProjection::CellRegions(vec![cell_rect]),
    }
}

fn presentation_logical_node(
    id: PresentationNodeId,
    parent: Option<PresentationNodeId>,
    role: PresentationNodeRole,
    state: PresentationNodeState,
    logical_rect: LogicalRect,
    terminal_projection: TerminalProjection,
) -> PresentationNode {
    PresentationNode {
        id,
        parent,
        role,
        state,
        logical_rect,
        cell_rect: None,
        terminal_projection,
    }
}

fn presentation_cell_logical_node(
    id: PresentationNodeId,
    parent: Option<PresentationNodeId>,
    role: PresentationNodeRole,
    state: PresentationNodeState,
    cell_rect: SceneRect,
    logical_rect: LogicalRect,
) -> PresentationNode {
    PresentationNode {
        id,
        parent,
        role,
        state,
        logical_rect,
        cell_rect: Some(cell_rect),
        terminal_projection: TerminalProjection::CellRegions(vec![cell_rect]),
    }
}

fn pane_badge_tone(kind: PaneBadgeKind) -> PresentationTone {
    match kind {
        PaneBadgeKind::Agent => PresentationTone::AgentIdentity,
        PaneBadgeKind::Approval => PresentationTone::Waiting,
        _ => PresentationTone::Neutral,
    }
}

fn separator_logical_rects(
    separator: &layout::SeparatorLayout,
    viewport: ViewportMetrics,
) -> (LogicalRect, LogicalRect) {
    const RULE_WIDTH: u64 = 64;
    const TARGET_WIDTH: u64 = 6 * 64;
    let split = viewport.logical_rect_for_cells(separator.split_area);
    match separator.axis {
        mandatum_core::SplitAxis::Horizontal => {
            let boundary = i64::from(separator.area.x.saturating_add(1))
                .saturating_mul(viewport.measured_cell_metrics.width_units() as i64);
            (
                LogicalRect::from_units(
                    boundary.saturating_sub((RULE_WIDTH / 2) as i64),
                    split.origin.y_units(),
                    RULE_WIDTH,
                    split.size.height_units(),
                ),
                LogicalRect::from_units(
                    boundary.saturating_sub((TARGET_WIDTH / 2) as i64),
                    split.origin.y_units(),
                    TARGET_WIDTH,
                    split.size.height_units(),
                ),
            )
        }
        mandatum_core::SplitAxis::Vertical => {
            let boundary = i64::from(separator.area.y.saturating_add(1))
                .saturating_mul(viewport.measured_cell_metrics.height_units() as i64);
            (
                LogicalRect::from_units(
                    split.origin.x_units(),
                    boundary.saturating_sub((RULE_WIDTH / 2) as i64),
                    split.size.width_units(),
                    RULE_WIDTH,
                ),
                LogicalRect::from_units(
                    split.origin.x_units(),
                    boundary.saturating_sub((TARGET_WIDTH / 2) as i64),
                    split.size.width_units(),
                    TARGET_WIDTH,
                ),
            )
        }
    }
}

fn logical_hit_rect(
    target: &HitTarget,
    separators: &[layout::SeparatorLayout],
    viewport: ViewportMetrics,
) -> LogicalRect {
    if let HitTargetKind::Separator { split_index, .. } = target.kind
        && let Some(separator) = separators
            .iter()
            .find(|separator| separator.split_index == split_index)
    {
        return separator_logical_rects(separator, viewport).1;
    }
    viewport.logical_rect_for_cells(target.rect)
}

fn terminal_viewport_mapping(
    state: &AppState,
    pane: &PaneScene,
    viewport: ViewportMetrics,
) -> Option<TerminalViewportMapping> {
    let inner = layout::pane_inner_rect(pane.area);
    let (pty_size, visible_cell_rect, first_visible_surface_row) = match &pane.content {
        PaneContent::Terminal(surface) => {
            let grid = state.terminal_grid(&pane.id)?;
            (
                SceneSize::new(grid.size().columns(), grid.size().rows()),
                inner,
                surface.first_row,
            )
        }
        PaneContent::Task(task) => {
            let output = task.output.as_ref()?;
            let (_, grid) = state.task_view(&pane.id)?;
            let grid = grid?;
            let detail_rows = u16::try_from(pane.terminal_fallback_row_count()).unwrap_or(u16::MAX);
            (
                SceneSize::new(grid.size().columns(), grid.size().rows()),
                SceneRect::new(
                    inner.x,
                    inner.y.saturating_add(detail_rows),
                    inner.width,
                    inner.height.saturating_sub(detail_rows),
                ),
                output.first_row,
            )
        }
        _ => return None,
    };
    let node_id = PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Output);
    Some(TerminalViewportMapping {
        node_id,
        pane_id: pane.id.clone(),
        pty_size,
        visible_cell_rect,
        logical_rect: viewport.logical_rect_for_cells(visible_cell_rect),
        first_visible_surface_row,
    })
}

fn overlay_area(overlay: &OverlayScene) -> SceneRect {
    match overlay {
        OverlayScene::Palette(value) => value.area,
        OverlayScene::ContextMenu(value) => value.area,
        OverlayScene::Timeline(value) => value.area,
        OverlayScene::SessionMap(value) => value.area,
        OverlayScene::Prompt(value) => value.area,
        OverlayScene::Search(value) => value.area,
        OverlayScene::Help(value) => value.area,
        OverlayScene::Appearance(value) => value.area,
        OverlayScene::Welcome(value) => value.area,
    }
}

fn constrain_overlay_area(
    overlay: &mut OverlayScene,
    viewport: ViewportMetrics,
    theme: &mandatum_scene::Theme,
) {
    let size = viewport.scene_size();
    if size.width == 0 || size.height == 0 {
        return;
    }
    let original = overlay_area(overlay);
    let cell_width = viewport.measured_cell_metrics.width_units().max(1);
    let cell_height = viewport.measured_cell_metrics.height_units().max(1);
    // Cell-only frontends and fixtures deliberately model one logical pixel
    // per terminal cell. Preserve their established projection; native
    // logical constraints activate only with real measured font metrics.
    if cell_width == 64 && cell_height == 64 {
        return;
    }
    let edge_units = u64::from(theme.ui.spacing.viewport_edge_margin) * 64;
    let margin_x = edge_units
        .div_ceil(cell_width)
        .min(u64::from(size.width.saturating_sub(1) / 2)) as u16;
    let margin_y = edge_units
        .div_ceil(cell_height)
        .min(u64::from(size.height.saturating_sub(1) / 2)) as u16;
    let available_width = size.width.saturating_sub(margin_x.saturating_mul(2)).max(1);
    let available_height = size
        .height
        .saturating_sub(margin_y.saturating_mul(2))
        .max(1);
    let max_width_pixels: Option<u16> = match overlay {
        OverlayScene::Palette(_) => Some(720),
        OverlayScene::Timeline(_) | OverlayScene::Search(_) => Some(920),
        OverlayScene::SessionMap(_) => Some(680),
        OverlayScene::Help(_) => Some(960),
        OverlayScene::Prompt(_) | OverlayScene::Welcome(_) | OverlayScene::Appearance(_) => {
            Some(720)
        }
        OverlayScene::ContextMenu(_) => None,
    };
    let maximum_columns = max_width_pixels
        .map(|pixels| (u64::from(pixels) * 64 / cell_width).max(1))
        .unwrap_or(u64::from(available_width))
        .min(u64::from(u16::MAX)) as u16;
    let minimum_width = available_width.min(3);
    let minimum_height = available_height.min(3);
    let width = original
        .width
        .min(maximum_columns)
        .min(available_width)
        .max(minimum_width);
    let height = original.height.min(available_height).max(minimum_height);
    let max_x = size.width.saturating_sub(margin_x).saturating_sub(width);
    let max_y = size.height.saturating_sub(margin_y).saturating_sub(height);
    let anchored = matches!(overlay, OverlayScene::ContextMenu(_));
    let x = if anchored {
        original.x.clamp(margin_x.min(max_x), max_x.max(margin_x))
    } else {
        size.width.saturating_sub(width) / 2
    };
    let y = if anchored {
        original.y.clamp(margin_y.min(max_y), max_y.max(margin_y))
    } else {
        size.height.saturating_sub(height) / 2
    };
    let constrained = SceneRect::new(x, y, width, height);
    match overlay {
        OverlayScene::Palette(value) => value.area = constrained,
        OverlayScene::ContextMenu(value) => value.area = constrained,
        OverlayScene::Timeline(value) => value.area = constrained,
        OverlayScene::SessionMap(value) => value.area = constrained,
        OverlayScene::Prompt(value) => value.area = constrained,
        OverlayScene::Search(value) => value.area = constrained,
        OverlayScene::Help(value) => value.area = constrained,
        OverlayScene::Appearance(value) => value.area = constrained,
        OverlayScene::Welcome(value) => value.area = constrained,
    }
}

fn presentation_id_for_hit_target(
    target: &HitTarget,
    overlay: Option<&OverlayScene>,
) -> Option<PresentationNodeId> {
    Some(match &target.kind {
        HitTargetKind::PaneBody(pane_id) => {
            PresentationNodeId::pane(pane_id.clone(), PaneNodePart::Body)
        }
        HitTargetKind::PaneTitle(pane_id) => {
            PresentationNodeId::pane(pane_id.clone(), PaneNodePart::Title)
        }
        HitTargetKind::Separator { split_index, axis } => {
            PresentationNodeId::workspace(WorkspaceNodePart::Separator {
                split_index: *split_index,
                axis: PresentationAxis::from(*axis),
            })
        }
        HitTargetKind::StatusStrip => PresentationNodeId::workspace(WorkspaceNodePart::Status),
        HitTargetKind::AttentionSegment { pane, kind, .. } => {
            PresentationNodeId::workspace(WorkspaceNodePart::Attention {
                pane: pane.clone(),
                kind: *kind,
            })
        }
        HitTargetKind::PaletteItem(index) => PresentationNodeId::overlay_item(
            OverlayKind::Palette,
            overlay_item_key(overlay?, *index)?,
        ),
        HitTargetKind::ContextMenuItem(index) => PresentationNodeId::overlay_item(
            OverlayKind::ContextMenu,
            overlay_item_key(overlay?, *index)?,
        ),
        HitTargetKind::TimelineItem(index) => PresentationNodeId::overlay_item(
            OverlayKind::Timeline,
            overlay_item_key(overlay?, *index)?,
        ),
        HitTargetKind::SessionMapRow(index) => PresentationNodeId::overlay_item(
            OverlayKind::SessionMap,
            overlay_item_key(overlay?, *index)?,
        ),
        HitTargetKind::SearchItem(index) => PresentationNodeId::overlay_item(
            OverlayKind::Search,
            overlay_item_key(overlay?, *index)?,
        ),
    })
}

fn overlay_item_key(overlay: &OverlayScene, index: usize) -> Option<mandatum_scene::SemanticKey> {
    match overlay {
        OverlayScene::Palette(value) => value.item_keys.get(index),
        OverlayScene::ContextMenu(value) => value.item_keys.get(index),
        OverlayScene::Timeline(value) => value.item_keys.get(index),
        OverlayScene::SessionMap(value) => value.item_keys.get(index),
        OverlayScene::Search(value) => value.item_keys.get(index),
        OverlayScene::Prompt(_)
        | OverlayScene::Help(_)
        | OverlayScene::Appearance(_)
        | OverlayScene::Welcome(_) => None,
    }
    .cloned()
}

fn presentation_role_for_hit_target(kind: &HitTargetKind) -> PresentationNodeRole {
    match kind {
        HitTargetKind::PaneBody(_) => PresentationNodeRole::PaneBody,
        HitTargetKind::PaneTitle(_) => PresentationNodeRole::PaneTitle,
        HitTargetKind::Separator { .. } => PresentationNodeRole::Separator,
        HitTargetKind::StatusStrip => PresentationNodeRole::Status,
        HitTargetKind::AttentionSegment { .. } => PresentationNodeRole::Attention,
        HitTargetKind::PaletteItem(_)
        | HitTargetKind::ContextMenuItem(_)
        | HitTargetKind::TimelineItem(_)
        | HitTargetKind::SessionMapRow(_)
        | HitTargetKind::SearchItem(_) => PresentationNodeRole::Item,
    }
}

fn presentation_state_for_hit_target(
    kind: &HitTargetKind,
    overlay: Option<&OverlayScene>,
) -> PresentationNodeState {
    let (selected, disabled) = match (kind, overlay) {
        (HitTargetKind::PaletteItem(index), Some(OverlayScene::Palette(value))) => (
            value.selected == Some(*index),
            value.items.get(*index).is_some_and(|item| !item.enabled),
        ),
        (HitTargetKind::ContextMenuItem(index), Some(OverlayScene::ContextMenu(value))) => {
            (value.selected == *index, false)
        }
        (HitTargetKind::TimelineItem(index), Some(OverlayScene::Timeline(value))) => {
            (value.selected == Some(*index), false)
        }
        (HitTargetKind::SessionMapRow(index), Some(OverlayScene::SessionMap(value))) => {
            (value.selected == *index, false)
        }
        (HitTargetKind::SearchItem(index), Some(OverlayScene::Search(value))) => {
            (value.selected == Some(*index), false)
        }
        _ => (false, false),
    };
    let (attention, tone) = match kind {
        HitTargetKind::AttentionSegment {
            kind: mandatum_scene::AttentionKind::ApprovalWaiting,
            ..
        } => (true, mandatum_scene::PresentationTone::Waiting),
        HitTargetKind::AttentionSegment { .. } => (true, mandatum_scene::PresentationTone::Failure),
        _ => (false, mandatum_scene::PresentationTone::Neutral),
    };
    PresentationNodeState {
        selected,
        disabled,
        attention,
        tone,
        ..PresentationNodeState::default()
    }
}

fn accessibility_nodes(
    panes: &[PaneScene],
    header: &HeaderScene,
    status: &StatusScene,
    overlay: Option<&OverlayScene>,
    nodes: &[PresentationNode],
    viewport: ViewportMetrics,
    workspace_id: &PresentationNodeId,
) -> Vec<AccessibilityNode> {
    let mut result = vec![AccessibilityNode {
        id: workspace_id.clone(),
        parent: None,
        role: AccessibilityRole::Workspace,
        label: "Mandatum workspace".to_owned(),
        value: None,
        state: AccessibilityState::default(),
        logical_rect: viewport.logical_rect_for_cells(SceneRect::new(
            0,
            0,
            viewport.scene_size().width,
            viewport.scene_size().height,
        )),
        supported_actions: Vec::new(),
    }];
    result.push(AccessibilityNode {
        id: PresentationNodeId::workspace(WorkspaceNodePart::Header),
        parent: Some(workspace_id.clone()),
        role: AccessibilityRole::Header,
        label: header.workspace_name.clone(),
        value: Some(header.text.clone()),
        state: AccessibilityState::default(),
        logical_rect: viewport.logical_rect_for_cells(header.area),
        supported_actions: Vec::new(),
    });
    result.push(AccessibilityNode {
        id: PresentationNodeId::workspace(WorkspaceNodePart::Status),
        parent: Some(workspace_id.clone()),
        role: AccessibilityRole::Status,
        label: "Workspace status".to_owned(),
        value: Some(status.text.clone()),
        state: AccessibilityState::default(),
        logical_rect: viewport.logical_rect_for_cells(status.area),
        supported_actions: vec![AccessibilityActionKind::Activate],
    });
    for pane in panes {
        result.push(AccessibilityNode {
            id: PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Surface),
            parent: Some(workspace_id.clone()),
            role: match pane.content {
                PaneContent::Terminal(_) => AccessibilityRole::Terminal,
                _ => AccessibilityRole::Pane,
            },
            label: pane.title.clone(),
            value: Some(pane.kind.label().to_owned()),
            state: AccessibilityState {
                focused: pane.focused,
                busy: matches!(
                    &pane.content,
                    PaneContent::Task(task)
                        if task.status_label.as_deref().is_some_and(|status| status.contains("running"))
                ),
                ..AccessibilityState::default()
            },
            logical_rect: viewport.logical_rect_for_cells(pane.area),
            supported_actions: vec![AccessibilityActionKind::Focus],
        });
    }
    if let Some(overlay) = overlay {
        let overlay_id = PresentationNodeId::overlay(overlay.kind(), OverlayNodePart::Surface);
        result.push(AccessibilityNode {
            id: overlay_id.clone(),
            parent: Some(workspace_id.clone()),
            role: AccessibilityRole::Dialog,
            label: overlay_accessibility_label(overlay).to_owned(),
            value: None,
            state: AccessibilityState::default(),
            logical_rect: viewport.logical_rect_for_cells(overlay_area(overlay)),
            supported_actions: Vec::new(),
        });
        for node in nodes
            .iter()
            .filter(|node| node.role == PresentationNodeRole::Item)
        {
            result.push(AccessibilityNode {
                id: node.id.clone(),
                parent: Some(overlay_id.clone()),
                role: AccessibilityRole::ListItem,
                label: overlay_item_label(overlay, &node.id)
                    .unwrap_or("Item")
                    .to_owned(),
                value: None,
                state: AccessibilityState {
                    selected: node.state.selected,
                    disabled: node.state.disabled,
                    ..AccessibilityState::default()
                },
                logical_rect: node.logical_rect,
                supported_actions: vec![
                    AccessibilityActionKind::Focus,
                    AccessibilityActionKind::Activate,
                ],
            });
        }
    }
    result
}

fn overlay_accessibility_label(overlay: &OverlayScene) -> &'static str {
    match overlay {
        OverlayScene::Palette(_) => "Command palette",
        OverlayScene::ContextMenu(_) => "Pane menu",
        OverlayScene::Timeline(_) => "Timeline",
        OverlayScene::SessionMap(_) => "Session map",
        OverlayScene::Prompt(_) => "Text prompt",
        OverlayScene::Search(_) => "Search",
        OverlayScene::Help(_) => "Help",
        OverlayScene::Appearance(_) => "Appearance",
        OverlayScene::Welcome(_) => "Welcome",
    }
}

fn overlay_item_label<'a>(
    overlay: &'a OverlayScene,
    node_id: &PresentationNodeId,
) -> Option<&'a str> {
    match overlay {
        OverlayScene::Palette(value) => value
            .item_keys
            .iter()
            .position(|key| {
                PresentationNodeId::overlay_item(OverlayKind::Palette, key.clone()) == *node_id
            })
            .and_then(|index| value.items.get(index).map(|item| item.label.as_str())),
        OverlayScene::ContextMenu(value) => value
            .item_keys
            .iter()
            .position(|key| {
                PresentationNodeId::overlay_item(OverlayKind::ContextMenu, key.clone()) == *node_id
            })
            .and_then(|index| value.items.get(index).map(|item| item.label.as_str())),
        OverlayScene::Timeline(value) => value
            .item_keys
            .iter()
            .position(|key| {
                PresentationNodeId::overlay_item(OverlayKind::Timeline, key.clone()) == *node_id
            })
            .and_then(|index| value.items.get(index).map(|item| item.text.as_str())),
        OverlayScene::SessionMap(value) => value
            .item_keys
            .iter()
            .position(|key| {
                PresentationNodeId::overlay_item(OverlayKind::SessionMap, key.clone()) == *node_id
            })
            .and_then(|index| value.rows.get(index).map(|item| item.label.as_str())),
        OverlayScene::Search(value) => value
            .item_keys
            .iter()
            .position(|key| {
                PresentationNodeId::overlay_item(OverlayKind::Search, key.clone()) == *node_id
            })
            .and_then(|index| value.items.get(index).map(|item| item.text.as_str())),
        OverlayScene::Help(value) => value
            .items
            .iter()
            .find(|item| {
                PresentationNodeId::overlay_item(OverlayKind::Help, item.key.clone()) == *node_id
            })
            .map(|item| item.label.as_str()),
        OverlayScene::Prompt(_) | OverlayScene::Appearance(_) | OverlayScene::Welcome(_) => None,
    }
}

fn text_input_scene(
    state: &AppState,
    panes: &[PaneScene],
    overlay: Option<&OverlayScene>,
) -> Option<TextInputScene> {
    let target = state.current_composition_target()?;
    let preedit = state
        .composition_preedit_for(&target)
        .map(|(text, cursor)| PreeditScene {
            text: text.to_owned(),
            cursor,
        });

    let (area, kind) = match &target {
        CompositionTarget::Terminal(pane_id) => {
            let pane = panes.iter().find(|pane| &pane.id == pane_id)?;
            let PaneContent::Terminal(surface) = &pane.content else {
                return None;
            };
            let cursor = surface.cursor?;
            let visible_row = cursor.row.checked_sub(surface.first_row)?;
            let inner = layout::pane_inner_rect(pane.area);
            if visible_row >= usize::from(inner.height) {
                return None;
            }
            let column = cursor.column.min(inner.width.saturating_sub(1));
            let style = surface
                .rows
                .get(visible_row)
                .and_then(|row| row.get(usize::from(column)))
                .map_or_else(SceneCellStyle::default, |cell| cell.style);
            let x = inner.x.saturating_add(column);
            (
                SceneRect::new(
                    x,
                    inner.y.saturating_add(visible_row as u16),
                    inner.right().saturating_sub(x),
                    1,
                ),
                TextInputKind::Terminal { style },
            )
        }
        CompositionTarget::Prompt => {
            let OverlayScene::Prompt(prompt) = overlay? else {
                return None;
            };
            overlay_text_input_area(prompt.area, &prompt.input)
        }
        CompositionTarget::Timeline => {
            let OverlayScene::Timeline(timeline) = overlay? else {
                return None;
            };
            overlay_text_input_area(timeline.area, &timeline.query)
        }
        CompositionTarget::Search => {
            let OverlayScene::Search(search) = overlay? else {
                return None;
            };
            overlay_text_input_area(search.area, &search.query)
        }
        CompositionTarget::Palette => {
            let OverlayScene::Palette(palette) = overlay? else {
                return None;
            };
            overlay_text_input_area(palette.area, &palette.query)
        }
        CompositionTarget::Help => {
            let OverlayScene::Help(help) = overlay? else {
                return None;
            };
            overlay_text_input_area(help.area, &help.query)
        }
    };

    Some(TextInputScene {
        area,
        kind,
        preedit,
    })
}

fn overlay_text_input_area(area: SceneRect, input: &str) -> (SceneRect, TextInputKind) {
    let inner = layout::pane_inner_rect(area);
    let column = 2usize
        .saturating_add(display_width(input))
        .min(usize::from(inner.width.saturating_sub(1))) as u16;
    let x = inner.x.saturating_add(column);
    (
        SceneRect::new(
            x,
            inner.y,
            inner.right().saturating_sub(x),
            inner.height.min(1),
        ),
        TextInputKind::Overlay,
    )
}

/// The status strip text: state-only app status plus the permanent
/// workspace-control hint, so a stranger always has the palette chord,
/// right-click menu, and help route written on screen exactly once. Attention
/// lives in the header.
fn status_text(state: &AppState) -> String {
    format!("{} — {}", state.status(), state.control_hint())
}

fn pane_scene(
    state: &AppState,
    session: &Session,
    pane: &PaneSpec,
    placed: PaneLayout,
    cache: &mut PaneSceneCache,
) -> PaneScene {
    let inner = layout::pane_inner_rect(placed.area);
    let mut terminal_key = None;
    let mut terminal_content_reused = false;
    let content = match pane.kind() {
        PaneKind::Terminal { .. } => match state.terminal_grid(pane.id()) {
            Some(grid) => {
                let view = state.pane_view_state(pane.id());
                let key = terminal_surface_key(state, pane, grid, view, inner.width, inner.height);
                let surface = match cache.cached_terminal_surface(pane.id(), &key) {
                    Some(surface) => {
                        terminal_content_reused = true;
                        #[cfg(test)]
                        {
                            cache.surface_reuses += 1;
                        }
                        surface
                    }
                    None => {
                        #[cfg(test)]
                        {
                            cache.surface_rebuilds += 1;
                        }
                        terminal_surface(grid, view, inner.width, inner.height)
                    }
                };
                terminal_key = Some(key);
                PaneContent::Terminal(surface)
            }
            None => PaneContent::Empty(empty_content(state, pane)),
        },
        PaneKind::Task { intent } => PaneContent::Task(task_content(state, pane, intent)),
        PaneKind::Agent { intent } => PaneContent::Agent(agent_content(state, pane.id(), intent)),
        PaneKind::Artifact { intent } => {
            PaneContent::Artifact(state.artifact_content(pane.id(), intent))
        }
    };

    let mut scene = PaneScene {
        id: placed.pane_id,
        title: pane.title().to_owned(),
        kind: pane_scene_kind(pane.kind()),
        area: placed.area,
        focused: pane.id() == session.focused_pane_id(),
        floating: placed.floating,
        stacked: placed.stacked,
        zoomed: placed.zoomed,
        content_revision: 0,
        content,
    };

    // Window a task's live output to the rows left under its detail lines.
    // The detail line count is stable whether or not the output surface is
    // attached (the "output:" marker replaces "output: no live grid
    // attached"), so measuring before attaching is exact.
    let detail_rows = u16::try_from(scene.terminal_fallback_row_count()).unwrap_or(u16::MAX);
    let mut task_output_key = None;
    if let PaneContent::Task(task) = &mut scene.content
        && let Some((_, Some(grid))) = state.task_view(&scene.id)
    {
        let max_height = inner.height.saturating_sub(detail_rows);
        // The output window's scroll offset is derived from grid content
        // (`task_output_surface` anchors to the content tail), so the key's
        // view component is fixed at the default.
        let key = terminal_surface_key(
            state,
            pane,
            grid,
            PaneViewState::default(),
            inner.width,
            max_height,
        );
        task.output = Some(match cache.cached_task_output_surface(&scene.id, &key) {
            Some(surface) => {
                #[cfg(test)]
                {
                    cache.surface_reuses += 1;
                }
                surface
            }
            None => {
                #[cfg(test)]
                {
                    cache.surface_rebuilds += 1;
                }
                task_output_surface(grid, inner.width, max_height)
            }
        });
        task_output_key = Some(key);
    }

    scene.content_revision = cache.settle(
        state.scene_generation(),
        &scene.id,
        &scene.content,
        terminal_key,
        task_output_key,
        terminal_content_reused,
    );
    scene
}

/// Window a task grid into its output surface, anchored to the content tail
/// rather than the grid bottom. The task PTY is sized to the pane's full
/// inner rect while the visible window sits below the detail rows, so
/// bottom-anchoring would permanently hide the first rows of every task's
/// output — a one-line failure diagnostic would render as an empty "output:"
/// section. Content shows from the top until it outgrows the window, then
/// the window follows the tail.
fn task_output_surface(grid: &TerminalGrid, max_width: u16, max_height: u16) -> TerminalSurface {
    let view_rows = usize::from(grid.size().rows().min(max_height));
    let scrollback_len = grid.scrollback_len();

    // Where content ends: the last screen row with visible text, or the
    // cursor row if it sits lower (a spinner redrawing a blank row).
    let last_text_row = (0..grid.size().rows())
        .rev()
        .find(|row| {
            grid.row_text(*row)
                .is_some_and(|text| !text.trim().is_empty())
        })
        .map(usize::from)
        .unwrap_or(0);
    let content_end = scrollback_len + last_text_row.max(usize::from(grid.cursor().row()));

    let max_top = grid.total_rows().saturating_sub(view_rows);
    let first_row = (content_end + 1).saturating_sub(view_rows).min(max_top);
    terminal_surface(
        grid,
        PaneViewState {
            scroll_offset: max_top - first_row,
            ..PaneViewState::default()
        },
        max_width,
        max_height,
    )
}

fn task_content(state: &AppState, pane: &PaneSpec, intent: &TaskPaneIntent) -> TaskContent {
    let task_view = state.task_view(pane.id());
    let status_label = task_view.map(|(status, _)| status.to_owned());
    TaskContent {
        command: intent.command.clone(),
        // The directory the command actually runs in — the same resolution
        // the spawn path uses (intent -> pane -> project), never "unset".
        cwd_label: resolve_pane_cwd(state.workspace(), pane, intent.cwd.as_ref())
            .display()
            .to_string(),
        recipe_label: intent.recipe_id.clone(),
        status_role: task_status_role(
            status_label.as_deref(),
            state.task_failure_status(pane.id()).is_some(),
            task_view.is_some_and(|(_, grid)| grid.is_some()),
        ),
        status_label,
        // The live keyboard route to Rerun task; the scene shows it on
        // failed tasks next to the right-click route.
        rerun_hint: Some(state.command_key_hint(mandatum_commands::CommandId::RerunTask))
            .filter(|hint| !hint.is_empty()),
        output: None,
    }
}

fn task_status_role(
    status: Option<&str>,
    has_task_failure: bool,
    has_live_grid: bool,
) -> TaskStatusRole {
    if has_task_failure {
        return TaskStatusRole::Failed;
    }
    match status {
        Some(status) if status.starts_with("succeeded:") => TaskStatusRole::Succeeded,
        Some("running") => TaskStatusRole::Running,
        Some("pending launch: waiting for visible pane size")
        | Some("pending rerun: waiting for visible pane size") => TaskStatusRole::Waiting,
        Some(_) if has_live_grid => TaskStatusRole::Diagnostic,
        Some(_) | None => TaskStatusRole::Detached,
    }
}

/// Agent pane content: the durable intent summary plus whatever live session
/// surface (action, approval detail, output tail) the runtime registry holds.
fn agent_content(state: &AppState, pane_id: &PaneId, intent: &AgentPaneIntent) -> AgentContent {
    let live = state.agent_runtime_view(pane_id);
    // The most recent files, oldest first.
    let skip = intent
        .changed_files
        .len()
        .saturating_sub(AGENT_CHANGED_FILES_SHOWN);
    let changed_files = intent
        .changed_files
        .iter()
        .skip(skip)
        .map(|path| path.display().to_string())
        .collect();

    AgentContent {
        objective: intent.objective.clone(),
        status_label: agent_status_label(&intent.status).to_owned(),
        status_role: intent.status.clone(),
        pending_approvals: intent.pending_approvals,
        changed_file_count: intent.changed_files.len(),
        changed_files,
        latest_summary: intent.latest_summary.clone(),
        current_action: live.and_then(|runtime| runtime.current_action.map(str::to_owned)),
        // A launch that never produced a session has no live runtime; the
        // durable intent still carries why it failed.
        last_error: live
            .and_then(|runtime| runtime.last_error.map(str::to_owned))
            .or_else(|| intent.last_error.clone()),
        relaunch_hint: Some(state.command_key_hint(mandatum_commands::CommandId::StartAgent))
            .filter(|hint| !hint.is_empty()),
        pending_approval: live
            .and_then(|runtime| runtime.pending_approval)
            .map(|request| AgentApprovalPrompt {
                command: request.command.clone(),
                cwd: request.scope.cwd.display().to_string(),
                affected_path: request
                    .scope
                    .affected_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                risk_label: risk_label(request.risk.level).to_owned(),
                risk_basis: request.risk.basis.clone(),
                key_hint: "y approve / n reject".to_owned(),
                // Waiting approval stays statically high-salience. A separate
                // one-shot typed ApprovalArrival target provides brief motion
                // when permitted; no wall clock leaks into scene content.
                pulse_on: true,
            }),
        output_tail: live
            .map(|runtime| runtime.output_tail.iter().cloned().collect())
            .unwrap_or_default(),
    }
}

fn risk_label(level: RiskLevel) -> &'static str {
    match level {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
    }
}

fn empty_content(state: &AppState, pane: &PaneSpec) -> EmptyContent {
    EmptyContent {
        // The directory a spawned shell would run in — the same resolution
        // the spawn path uses (pane -> project), never "unset".
        cwd_label: resolve_pane_cwd(state.workspace(), pane, None)
            .display()
            .to_string(),
        restart_generation: pane.restart_generation(),
    }
}

fn pane_scene_kind(kind: &PaneKind) -> PaneSceneKind {
    match kind {
        PaneKind::Terminal { .. } => PaneSceneKind::Terminal,
        PaneKind::Task { .. } => PaneSceneKind::Task,
        PaneKind::Agent { .. } => PaneSceneKind::Agent,
        PaneKind::Artifact { .. } => PaneSceneKind::Artifact,
    }
}

/// Window a terminal grid into a scene surface: the rows visible in a pane
/// viewport of `max_width` x `max_height`, in absolute buffer coordinates.
fn terminal_surface(
    grid: &TerminalGrid,
    view: PaneViewState,
    max_width: u16,
    max_height: u16,
) -> TerminalSurface {
    let view_rows = usize::from(grid.size().rows().min(max_height));
    let columns = grid.size().columns().min(max_width);
    let total_rows = grid.total_rows();
    let scrollback_len = grid.scrollback_len();

    // Top visible absolute row, clamped so the viewport never runs off the end.
    let max_top = total_rows.saturating_sub(view_rows);
    let first_row = max_top.saturating_sub(view.scroll_offset);

    // Borrowed lookups (`history_cell_ref`) keep this per-frame hot loop at one
    // grapheme clone per visible cell; `blank` backs out-of-range rows.
    let blank = TerminalCell::blank();
    let rows = (0..view_rows)
        .map(|line| {
            let absolute_row = first_row + line;
            let mut row = (0..columns)
                .map(|column| {
                    let cell = grid
                        .history_cell_ref(absolute_row, column)
                        .unwrap_or(&blank);
                    SceneCell {
                        occupancy: match cell.occupancy() {
                            // Single-scalar graphemes (the overwhelming
                            // majority) inline into `Char` without cloning.
                            TerminalCellOccupancy::Grapheme(grapheme) => {
                                CellOccupancy::grapheme(grapheme.as_str())
                            }
                            TerminalCellOccupancy::WideContinuation => {
                                CellOccupancy::WideContinuation
                            }
                        },
                        style: scene_cell_style(cell.style()),
                    }
                })
                .collect::<Vec<_>>();
            if columns < grid.size().columns()
                && grid
                    .history_cell_ref(absolute_row, columns)
                    .is_some_and(|cell| {
                        matches!(cell.occupancy(), TerminalCellOccupancy::WideContinuation)
                    })
                && let Some(last) = row.last_mut()
            {
                last.occupancy = CellOccupancy::Char('\u{fffd}');
            }
            row
        })
        .collect();

    let cursor = grid.cursor();
    TerminalSurface {
        rows,
        first_row,
        cursor: cursor.visible().then(|| {
            SurfacePosition::new(scrollback_len + usize::from(cursor.row()), cursor.column())
        }),
        scroll_offset: view.scroll_offset,
        scrollback_len,
        selection: view.selection.map(|(start, end)| {
            (
                SurfacePosition::new(start.0, start.1),
                SurfacePosition::new(end.0, end.1),
            )
        }),
        copy_cursor: view
            .copy_cursor
            .map(|(row, column)| SurfacePosition::new(row, column)),
    }
}

fn scene_cell_style(style: CellStyle) -> SceneCellStyle {
    SceneCellStyle {
        foreground: scene_color(style.foreground),
        background: scene_color(style.background),
        bold: style.bold,
        dim: style.dim,
        italic: style.italic,
        underline: style.underline,
        inverse: style.inverse,
        hidden: style.hidden,
        strikethrough: style.strikethrough,
    }
}

fn scene_color(color: VtColor) -> SceneColor {
    match color {
        VtColor::Default => SceneColor::Default,
        VtColor::Indexed(index) if index < 16 => SceneColor::Ansi(index),
        VtColor::Indexed(index) => SceneColor::Indexed(index),
        VtColor::Rgb(red, green, blue) => SceneColor::Rgb(red, green, blue),
    }
}

/// Hit targets in stacking order, bottom first: status strip, header
/// attention segments, tiled panes, split separators, floating panes, then
/// overlay rows. Pointer resolution scans this list in reverse, so later
/// targets win where rects overlap (floats over separators, overlays over
/// everything).
fn hit_targets(
    workspace: &mandatum_core::Workspace,
    panes: &[PaneScene],
    header: &HeaderScene,
    size: SceneSize,
    overlay: Option<&OverlayScene>,
) -> Vec<HitTarget> {
    let mut targets = Vec::new();

    let status = layout::status_rect(size);
    if !status.is_empty() {
        targets.push(HitTarget {
            rect: status,
            kind: HitTargetKind::StatusStrip,
        });
    }

    // Header attention segments are clickable jumps to the pane in need.
    for (index, segment) in header.attention.iter().enumerate() {
        if segment.rect.is_empty() {
            continue;
        }
        targets.push(HitTarget {
            rect: segment.rect,
            kind: HitTargetKind::AttentionSegment {
                index,
                pane: segment.pane.clone(),
                kind: segment.kind,
            },
        });
    }

    let pane_targets = |targets: &mut Vec<HitTarget>, pane: &PaneScene| {
        if pane.area.is_empty() {
            return;
        }
        targets.push(HitTarget {
            rect: SceneRect::new(pane.area.x, pane.area.y, pane.area.width, 1),
            kind: HitTargetKind::PaneTitle(pane.id.clone()),
        });
        targets.push(HitTarget {
            rect: layout::pane_inner_rect(pane.area),
            kind: HitTargetKind::PaneBody(pane.id.clone()),
        });
    };

    for pane in panes.iter().filter(|pane| !pane.floating) {
        pane_targets(&mut targets, pane);
    }

    for separator in layout::layout_separators(workspace, layout::workspace_scene_area(size)) {
        targets.push(HitTarget {
            rect: separator.area,
            kind: HitTargetKind::Separator {
                split_index: separator.split_index,
                axis: separator.axis,
            },
        });
    }

    for pane in panes.iter().filter(|pane| pane.floating) {
        pane_targets(&mut targets, pane);
    }

    match overlay {
        Some(OverlayScene::Palette(palette)) => {
            // Item rows start one row below the filter input; the shared window
            // math keeps these rects aligned with what the frontend draws.
            let inner = layout::pane_inner_rect(palette.area);
            let window = layout::palette_item_window(inner, palette.items.len(), palette.selected);
            for (row, index) in window.enumerate() {
                let Some(rect) = layout::palette_item_rect(inner, row) else {
                    continue;
                };
                targets.push(HitTarget {
                    rect,
                    kind: HitTargetKind::PaletteItem(index),
                });
            }
        }
        Some(OverlayScene::ContextMenu(menu)) => {
            let inner = layout::pane_inner_rect(menu.area);
            let window =
                layout::context_menu_item_window(inner, menu.items.len(), Some(menu.selected));
            for (row, index) in window.enumerate() {
                let Some(rect) = layout::context_menu_item_rect(inner, row) else {
                    continue;
                };
                targets.push(HitTarget {
                    rect,
                    kind: HitTargetKind::ContextMenuItem(index),
                });
            }
        }
        Some(OverlayScene::Timeline(timeline)) => {
            // Same shape as the palette: filter input on the top inner row,
            // footer on the bottom, entry rows between.
            let inner = layout::pane_inner_rect(timeline.area);
            let window =
                layout::palette_item_window(inner, timeline.items.len(), timeline.selected);
            for (row, index) in window.enumerate() {
                let Some(rect) = layout::palette_item_rect(inner, row) else {
                    continue;
                };
                targets.push(HitTarget {
                    rect,
                    kind: HitTargetKind::TimelineItem(index),
                });
            }
        }
        Some(OverlayScene::Search(search)) => {
            // Same shape as the palette/timeline: filter input on the top
            // inner row, footer on the bottom, result rows between.
            let inner = layout::pane_inner_rect(search.area);
            let window = layout::palette_item_window(inner, search.items.len(), search.selected);
            for (row, index) in window.enumerate() {
                let Some(rect) = layout::palette_item_rect(inner, row) else {
                    continue;
                };
                targets.push(HitTarget {
                    rect,
                    kind: HitTargetKind::SearchItem(index),
                });
            }
        }
        Some(OverlayScene::SessionMap(map)) => {
            let inner = layout::pane_inner_rect(map.area);
            let window = layout::session_map_item_window(inner, map.rows.len(), Some(map.selected));
            for (row, index) in window.enumerate() {
                let Some(rect) = layout::session_map_item_rect(inner, row) else {
                    continue;
                };
                targets.push(HitTarget {
                    rect,
                    kind: HitTargetKind::SessionMapRow(index),
                });
            }
        }
        // The prompt, help, and appearance overlays have no row targets
        // (click-away dismisses them); the first-run note is not even modal.
        Some(
            OverlayScene::Prompt(_)
            | OverlayScene::Help(_)
            | OverlayScene::Appearance(_)
            | OverlayScene::Welcome(_),
        )
        | None => {}
    }

    targets
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use mandatum_commands::CommandId;
    use mandatum_core::AgentStatus;
    use mandatum_scene::input::{
        InputEvent, Key, KeyCode, Modifiers, PointerButton, PointerEvent, PointerKind,
    };
    use mandatum_scene::{
        AccessibilityRole, ArtifactState, BackingScale, LogicalPoint, LogicalSize, PhysicalSize,
        PresentationNodeRole, compile_cell_program,
    };
    use mandatum_terminal_vt::{TerminalParser, TerminalSize};

    use super::*;
    use crate::app_shell::AppConfig;

    fn config(spawn_pty: bool) -> AppConfig {
        // One isolated directory per test-process run: a fixed temp path
        // would grow a real timeline file across runs and let concurrent
        // test runs interfere (nothing here persists a workspace).
        use std::sync::OnceLock;
        static BASELINE_DIR: OnceLock<PathBuf> = OnceLock::new();
        let project_path = BASELINE_DIR.get_or_init(|| {
            let path = std::env::temp_dir().join(format!(
                "mandatum-scene-builder-test-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("test temp dir should be created");
            path
        });
        AppConfig {
            workspace_file: project_path.join("workspace.json"),
            project_path: project_path.clone(),
            task_command: "printf 'TASK_OK\\n'".to_owned(),
            agent_objective: "test objective".to_owned(),
            spawn_pty,
            ..AppConfig::default()
        }
    }

    fn key(code: KeyCode) -> Key {
        Key::plain(code)
    }

    fn ctrl(code: char) -> Key {
        Key::ctrl(code)
    }

    fn viewport(scale: f64) -> ViewportMetrics {
        ViewportMetrics::new(
            LogicalSize::from_pixels(960.0, 640.0).unwrap(),
            PhysicalSize::new((960.0 * scale) as u32, (640.0 * scale) as u32),
            BackingScale::new(scale).unwrap(),
            LogicalSize::from_pixels(8.0, 16.0).unwrap(),
        )
        .unwrap()
    }

    fn pump_until(state: &mut AppState, mut predicate: impl FnMut(&AppState) -> bool) -> bool {
        for _ in 0..300 {
            state.tick_runtime();
            if predicate(state) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn scene_pane<'a>(scene: &'a WorkspaceScene, pane_id: &str) -> &'a PaneScene {
        scene
            .panes
            .iter()
            .find(|pane| pane.id == PaneId::new(pane_id))
            .expect("pane must be in the scene")
    }

    fn surface_text(surface: &TerminalSurface) -> String {
        surface
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(SceneCell::grapheme_text)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn narrowed_terminal_surface_replaces_a_truncated_wide_pair() {
        let mut parser = TerminalParser::new(TerminalSize::new(4, 1).unwrap());
        parser.feed_pty_bytes("界X".as_bytes()).unwrap();
        let surface = terminal_surface(parser.grid(), PaneViewState::default(), 1, 1);
        assert_eq!(
            surface.rows[0][0].occupancy,
            CellOccupancy::Char('\u{fffd}')
        );
    }

    #[test]
    fn logical_geometry_semantics_and_cell_program_match_at_1x_and_2x() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::SplitRight);
        state.dispatch(CommandId::SplitDown);

        let scene_1x = build_workspace_scene_with_viewport(&state, viewport(1.0));
        let scene_2x = build_workspace_scene_with_viewport(&state, viewport(2.0));

        assert_eq!(scene_1x.size, SceneSize::new(120, 40));
        assert_eq!(scene_1x.size, scene_2x.size);
        assert_eq!(
            compile_cell_program(&scene_1x, state.theme()),
            compile_cell_program(&scene_2x, state.theme()),
            "presentation geometry must not alter CellProgram bytes or topology"
        );
        assert_eq!(
            scene_1x
                .presentation
                .nodes
                .iter()
                .map(|node| (&node.id, node.role, node.state))
                .collect::<Vec<_>>(),
            scene_2x
                .presentation
                .nodes
                .iter()
                .map(|node| (&node.id, node.role, node.state))
                .collect::<Vec<_>>(),
            "semantic identity and state must not include backing scale"
        );
        assert_eq!(
            scene_1x.hit_targets.len(),
            scene_1x.presentation.logical_hit_targets.len(),
            "every cell hit target must have a matching logical target"
        );
        for cell_target in &scene_1x.hit_targets {
            let logical_target = scene_1x
                .presentation
                .logical_hit_targets
                .iter()
                .find(|target| target.kind == cell_target.kind)
                .expect("matching logical target");
            match cell_target.kind {
                HitTargetKind::Separator {
                    axis: mandatum_core::SplitAxis::Horizontal,
                    ..
                } => assert_eq!(logical_target.logical_rect.size.width_units(), 6 * 64),
                HitTargetKind::Separator {
                    axis: mandatum_core::SplitAxis::Vertical,
                    ..
                } => assert_eq!(logical_target.logical_rect.size.height_units(), 6 * 64),
                _ => assert_eq!(
                    logical_target.logical_rect,
                    viewport(1.0).logical_rect_for_cells(cell_target.rect)
                ),
            }
        }
        assert!(
            scene_1x
                .presentation
                .nodes
                .iter()
                .any(|node| { node.role == PresentationNodeRole::PaneTitle && node.state.focused }),
            "focused title state is typed, not inferred from its label"
        );
    }

    #[test]
    fn configured_density_changes_native_rail_geometry_without_changing_cell_projection() {
        let compact = AppState::new(config(false));
        let compact_scene = build_workspace_scene_with_viewport(&compact, viewport(1.0));
        assert_eq!(
            compact_scene.presentation.density,
            mandatum_scene::UiDensity::Compact
        );

        let comfortable = AppState::new(AppConfig {
            density: mandatum_scene::UiDensity::Comfortable,
            ..config(false)
        });
        let comfortable_scene = build_workspace_scene_with_viewport(&comfortable, viewport(1.0));
        assert_eq!(
            comfortable_scene.presentation.density,
            mandatum_scene::UiDensity::Comfortable
        );
        let title_height = |scene: &WorkspaceScene| {
            scene
                .presentation
                .nodes
                .iter()
                .find(|node| node.role == PresentationNodeRole::PaneTitle)
                .expect("pane title")
                .logical_rect
                .size
                .height_units()
        };
        assert!(
            title_height(&compact_scene) < title_height(&comfortable_scene),
            "compact rails must consume less native vertical material"
        );
        assert_eq!(
            compile_cell_program(&compact_scene, compact.theme()),
            compile_cell_program(&comfortable_scene, comfortable.theme()),
            "density is native presentation policy and cannot change PTY/cell geometry"
        );
    }

    #[test]
    fn pane_chrome_projects_typed_badges_and_a_two_pixel_focus_cue() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::SplitRight);
        state.dispatch(CommandId::NewTerminal);

        let scene = build_workspace_scene_with_viewport(&state, viewport(1.0));
        let program = compile_cell_program(&scene, state.theme());
        for pane in &scene.panes {
            let badges = pane
                .badge_kinds()
                .into_iter()
                .filter_map(|kind| {
                    scene.presentation.nodes.iter().find_map(|node| {
                        (node.id
                            == PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Badge(kind)))
                        .then_some(node.role)
                    })
                })
                .filter_map(|role| match role {
                    PresentationNodeRole::PaneBadge(kind) => Some(kind),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let native_badges = pane
                .badge_kinds()
                .into_iter()
                .filter(|kind| *kind != PaneBadgeKind::Terminal)
                .collect::<Vec<_>>();
            assert_eq!(badges, native_badges);
            assert!(
                scene.presentation.nodes.iter().all(|node| node.id
                    != PresentationNodeId::pane(
                        pane.id.clone(),
                        PaneNodePart::Badge(PaneBadgeKind::Terminal)
                    )),
                "the redundant terminal kind badge stays out of native presentation"
            );
            for (kind, rect) in pane.badge_rects() {
                if kind == PaneBadgeKind::Terminal {
                    continue;
                }
                let node = scene
                    .presentation
                    .nodes
                    .iter()
                    .find(|node| {
                        node.id
                            == PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Badge(kind))
                    })
                    .expect("typed badge node");
                assert_eq!(node.cell_rect, Some(rect));
                let projected = viewport(1.0).logical_rect_for_cells(rect);
                assert!(
                    node.logical_rect.origin.y_units() >= projected.origin.y_units()
                        && node.logical_rect.bottom_units() <= projected.bottom_units(),
                    "native density may inset a badge vertically but cannot escape its cell projection"
                );
                let text = (rect.x..rect.right())
                    .filter_map(|x| program.cell_at(x, rect.y))
                    .filter_map(|cell| match &cell.occupancy {
                        CellOccupancy::Char(character) => Some(character.to_string()),
                        CellOccupancy::Cluster(cluster) => Some(cluster.clone()),
                        CellOccupancy::WideContinuation => None,
                    })
                    .collect::<String>();
                assert_eq!(text, format!(" {} ", kind.label()));
            }

            let pane_node = scene
                .presentation
                .nodes
                .iter()
                .find(|node| {
                    node.id == PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Surface)
                })
                .expect("pane node");
            assert_eq!(pane_node.state.floating, pane.floating);
        }

        let focus = scene
            .presentation
            .nodes
            .iter()
            .find(|node| node.role == PresentationNodeRole::FocusIndicator)
            .expect("focused pane has an explicit non-color cue");
        assert_eq!(focus.logical_rect.size.width_units(), 2 * 64);
        assert_eq!(focus.state.tone, mandatum_scene::PresentationTone::Focus);
        assert_eq!(focus.cell_rect, None);
    }

    #[test]
    fn filtered_palette_items_keep_semantic_identity_when_position_changes() {
        let mut state = AppState::new(config(false));
        state.handle_event(InputEvent::Key(Key::ctrl('p')));
        state.handle_event(InputEvent::Key(Key::new(
            KeyCode::Char('s'),
            Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
        )));
        let first = build_workspace_scene_with_viewport(&state, viewport(1.0));
        let split_id = first
            .presentation
            .accessibility_nodes
            .iter()
            .find(|node| node.label == "Split pane right")
            .expect("split command must be represented")
            .id
            .clone();

        for character in ['p', 'l', 'i', 't'] {
            state.handle_event(InputEvent::Key(Key::plain(KeyCode::Char(character))));
        }
        let filtered = build_workspace_scene_with_viewport(&state, viewport(2.0));
        let filtered_save = filtered
            .presentation
            .accessibility_nodes
            .iter()
            .find(|node| node.label == "Split pane right")
            .expect("split command remains represented");

        assert_eq!(split_id, filtered_save.id);
        assert_eq!(filtered_save.role, AccessibilityRole::ListItem);
    }

    #[test]
    fn terminal_viewport_maps_only_its_exact_logical_content_rectangle() {
        let mut state = AppState::new(config(true));
        state.handle_event(InputEvent::Resize(SceneSize::new(120, 40)));
        assert!(
            pump_until(&mut state, |state| state
                .terminal_grid(&PaneId::new("pane-1"))
                .is_some()),
            "terminal grid should become live"
        );

        let scene = build_workspace_scene_with_viewport(&state, viewport(2.0));
        let mapping = scene
            .presentation
            .terminal_viewports
            .iter()
            .find(|mapping| mapping.pane_id == PaneId::new("pane-1"))
            .expect("live terminal has an explicit viewport mapping");
        assert_eq!(
            mapping.logical_rect,
            viewport(2.0).logical_rect_for_cells(mapping.visible_cell_rect)
        );
        let inside = LogicalPoint::from_units(
            mapping.logical_rect.origin.x_units(),
            mapping.logical_rect.origin.y_units(),
        );
        assert_eq!(
            mapping.logical_point_to_child_cell(viewport(2.0), inside),
            Some((0, 0))
        );
        let outside = LogicalPoint::from_units(
            mapping.logical_rect.right_units(),
            mapping.logical_rect.origin.y_units(),
        );
        assert_eq!(
            mapping.logical_point_to_child_cell(viewport(2.0), outside),
            None
        );

        let mapping_before_overlay = mapping.clone();
        state.handle_event(InputEvent::Key(ctrl('p')));
        let overlay_scene = build_workspace_scene_with_viewport(&state, viewport(2.0));
        let mapping_with_overlay = overlay_scene
            .presentation
            .terminal_viewports
            .iter()
            .find(|mapping| mapping.pane_id == PaneId::new("pane-1"))
            .expect("overlay cannot replace terminal viewport geometry");
        assert_eq!(
            mapping_with_overlay, &mapping_before_overlay,
            "overlay row stride is presentation-only and cannot resize the PTY viewport"
        );
    }

    #[test]
    fn scene_reflects_header_status_focus_and_copy_mode_flag() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::SplitRight);

        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));

        assert_eq!(scene.size, SceneSize::new(100, 30));
        assert_eq!(
            scene.header.session_name,
            state.workspace().active_session().name()
        );
        assert_eq!(
            scene.header.project_name,
            state.workspace().active_project_name()
        );
        assert_eq!(scene.header.pane_count, 2);
        assert_eq!(scene.header.focused_pane, PaneId::new("pane-2"));
        assert!(!scene.header.zoomed);
        assert_eq!(scene.focused_pane, PaneId::new("pane-2"));
        // The header carries its own area and composed calm text, so a
        // frontend paints it without deriving anything.
        assert_eq!(scene.header.area, layout::header_rect(scene.size));
        assert!(
            scene.header.text.contains("Mandatum"),
            "{}",
            scene.header.text
        );
        assert!(
            scene.header.text.contains("2 panes"),
            "{}",
            scene.header.text
        );
        // No agent pane exists, so the header must not claim an agent: the
        // connector label is configuration, announced only alongside activity.
        assert!(
            !scene.header.text.contains("agent:"),
            "{}",
            scene.header.text
        );
        assert!(scene.header.attention.is_empty(), "nothing needs attention");
        // The status strip carries its area plus the app status and the
        // permanent workspace-control hint (palette chord + right-click menu).
        assert_eq!(scene.status.area, layout::status_rect(scene.size));
        let status = &scene.status.text;
        assert!(status.starts_with(state.status()), "{status:?}");
        assert!(status.contains("ctrl+p commands"), "{status:?}");
        assert!(status.contains("right-click menu"), "{status:?}");
        assert!(!scene.copy_mode);
        assert!(scene_pane(&scene, "pane-2").focused);
        assert!(!scene_pane(&scene, "pane-1").focused);
    }

    #[test]
    fn every_visible_pane_yields_body_and_title_hit_targets() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::SplitRight);
        state.dispatch(CommandId::SplitDown);

        let scene = build_workspace_scene(&state, SceneSize::new(120, 40));

        assert_eq!(scene.panes.len(), 3);
        for pane in &scene.panes {
            assert!(
                scene.hit_targets.iter().any(|target| {
                    target.kind == HitTargetKind::PaneBody(pane.id.clone())
                        && target.rect == layout::pane_inner_rect(pane.area)
                }),
                "pane {} must have a body hit target",
                pane.id
            );
            assert!(
                scene
                    .hit_targets
                    .iter()
                    .any(|target| target.kind == HitTargetKind::PaneTitle(pane.id.clone())),
                "pane {} must have a title hit target",
                pane.id
            );
        }
        assert!(
            scene
                .hit_targets
                .iter()
                .any(|target| target.kind == HitTargetKind::StatusStrip)
        );
    }

    #[test]
    fn split_boundaries_yield_separator_hit_targets_with_identity() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::SplitRight);

        let scene = build_workspace_scene(&state, SceneSize::new(120, 40));

        let separator = scene
            .hit_targets
            .iter()
            .find(|target| matches!(target.kind, HitTargetKind::Separator { .. }))
            .expect("a split must yield a separator target");
        assert_eq!(
            separator.kind,
            HitTargetKind::Separator {
                split_index: 0,
                axis: mandatum_core::SplitAxis::Horizontal,
            }
        );
        // The strip covers the two adjacent border columns at the boundary.
        assert_eq!(separator.rect, SceneRect::new(59, 1, 2, 38));
    }

    #[test]
    fn hit_target_order_stacks_floats_over_separators_over_tiled_panes() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::SplitRight);
        state.dispatch(CommandId::NewTerminal); // floating pane on top

        let scene = build_workspace_scene(&state, SceneSize::new(120, 40));

        let position = |predicate: &dyn Fn(&HitTargetKind) -> bool| {
            scene
                .hit_targets
                .iter()
                .position(|target| predicate(&target.kind))
                .expect("target present")
        };
        let tiled_body = position(
            &|kind| matches!(kind, HitTargetKind::PaneBody(id) if id.as_str() == "pane-1"),
        );
        let separator = position(&|kind| matches!(kind, HitTargetKind::Separator { .. }));
        let float_body = position(
            &|kind| matches!(kind, HitTargetKind::PaneBody(id) if id.as_str() == "pane-3"),
        );

        // Reverse-scan hit testing means later targets win overlaps: floats
        // beat separators beat tiled panes.
        assert!(tiled_body < separator);
        assert!(separator < float_body);

        let separator_node = scene
            .presentation
            .nodes
            .iter()
            .position(|node| node.role == PresentationNodeRole::Separator)
            .expect("separator presentation node");
        let floating_node = scene
            .presentation
            .nodes
            .iter()
            .position(|node| node.role == PresentationNodeRole::Pane && node.state.floating)
            .expect("floating pane presentation node");
        assert!(
            separator_node < floating_node,
            "floating materials must occlude the tiled separator plane"
        );
    }

    #[test]
    fn zoomed_layout_emits_no_separator_targets() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::SplitRight);
        state.dispatch(CommandId::ZoomPane);

        let scene = build_workspace_scene(&state, SceneSize::new(120, 40));

        assert!(
            !scene
                .hit_targets
                .iter()
                .any(|target| matches!(target.kind, HitTargetKind::Separator { .. }))
        );
    }

    #[test]
    fn palette_overlay_carries_items_and_item_targets() {
        let mut state = AppState::new(config(false));
        state.handle_key(ctrl('p'));

        let size = SceneSize::new(120, 40);
        let scene = build_workspace_scene(&state, size);

        let Some(OverlayScene::Palette(palette)) = &scene.overlay else {
            panic!("palette must be open in the scene");
        };
        assert_eq!(palette.area, layout::palette_overlay_rect(size));
        // An empty query lists every built-in command with the first selected.
        assert_eq!(palette.query, "");
        assert_eq!(
            palette.items.len(),
            mandatum_commands::BUILT_IN_COMMANDS.len()
        );
        assert_eq!(palette.selected, Some(0));
        assert!(!palette.footer.is_empty());

        // Item hit targets cover exactly the visible window, one row below
        // the filter input, aligned with the shared window math.
        let inner = layout::pane_inner_rect(palette.area);
        let window = layout::palette_item_window(inner, palette.items.len(), palette.selected);
        let item_targets: Vec<_> = scene
            .hit_targets
            .iter()
            .filter(|target| matches!(target.kind, HitTargetKind::PaletteItem(_)))
            .collect();
        assert_eq!(item_targets.len(), window.len());
        assert!(!item_targets.is_empty());
        assert_eq!(
            item_targets[0].rect,
            layout::palette_item_rect(inner, 0).expect("first palette row")
        );
    }

    #[test]
    fn phase_four_overlay_family_presentation_contract_is_scene_owned() {
        let viewport = ViewportMetrics::new(
            LogicalSize::from_pixels(1_600.0, 800.0).unwrap(),
            PhysicalSize::new(3_200, 1_600),
            BackingScale::new(2.0).unwrap(),
            LogicalSize::from_pixels(8.0, 16.0).unwrap(),
        )
        .unwrap();
        fn overlay_surface(scene: &WorkspaceScene) -> &PresentationNode {
            scene
                .presentation
                .nodes
                .iter()
                .find(|node| node.role == PresentationNodeRole::Overlay)
                .expect("overlay surface presentation node")
        }

        let mut palette_state = AppState::new(config(false));
        palette_state.handle_key(ctrl('p'));
        let palette_scene = palette_state.build_scene_with_viewport(viewport);
        let palette_surface = overlay_surface(&palette_scene);
        assert_eq!(
            palette_surface.state.overlay_kind,
            Some(OverlayPresentationKind::Modal)
        );
        let edge_units = 24 * 64;
        assert_eq!(palette_surface.logical_rect.size.width_units(), 720 * 64);
        assert!(palette_surface.logical_rect.origin.x_units() >= edge_units);
        assert!(palette_surface.logical_rect.origin.y_units() >= edge_units);
        assert!(
            palette_surface.logical_rect.right_units()
                <= viewport.logical_size.width_units() as i64 - edge_units
        );
        assert!(
            palette_surface.logical_rect.bottom_units()
                <= viewport.logical_size.height_units() as i64 - edge_units
        );
        for role in [
            PresentationNodeRole::OverlayTitle,
            PresentationNodeRole::TextInput,
            PresentationNodeRole::OverlayFooter,
        ] {
            assert!(
                palette_scene
                    .presentation
                    .nodes
                    .iter()
                    .any(|node| node.role == role
                        && node.parent.as_ref() == Some(&palette_surface.id)),
                "Palette must emit its {role:?} band"
            );
        }

        let selected = palette_scene
            .presentation
            .nodes
            .iter()
            .find(|node| node.role == PresentationNodeRole::Item && node.state.selected)
            .expect("selected Palette row presentation node");
        let logical_target = palette_scene
            .presentation
            .logical_hit_targets
            .iter()
            .find(|target| target.node_id == selected.id)
            .expect("selected Palette row logical hit target");
        assert_eq!(selected.logical_rect, logical_target.logical_rect);
        let cell_target = palette_scene
            .hit_targets
            .iter()
            .find(|target| target.kind == logical_target.kind)
            .expect("selected Palette row cell target");
        let unpadded = viewport.logical_rect_for_cells(cell_target.rect);
        assert!(selected.logical_rect.origin.x_units() > unpadded.origin.x_units());
        assert!(selected.logical_rect.right_units() < unpadded.right_units());
        assert_eq!(cell_target.rect.height, layout::OVERLAY_CONTROL_ROWS);
        assert!(
            selected.logical_rect.size.height_units() >= 28 * 64,
            "default native overlay controls meet the product pointer minimum"
        );
        let mut item_targets = palette_scene
            .presentation
            .logical_hit_targets
            .iter()
            .filter(|target| matches!(target.kind, HitTargetKind::PaletteItem(_)))
            .collect::<Vec<_>>();
        item_targets.sort_by_key(|target| target.logical_rect.origin.y_units());
        for pair in item_targets.windows(2) {
            assert!(
                pair[0].logical_rect.bottom_units() <= pair[1].logical_rect.origin.y_units(),
                "adjacent overlay controls cannot overlap"
            );
        }
        for role in [
            PresentationNodeRole::TextInput,
            PresentationNodeRole::OverlayFooter,
        ] {
            let band = palette_scene
                .presentation
                .nodes
                .iter()
                .find(|node| node.role == role)
                .expect("overlay control band");
            assert_eq!(
                band.cell_rect.expect("cell projection").height,
                layout::OVERLAY_CONTROL_ROWS
            );
            assert!(band.logical_rect.size.height_units() >= 28 * 64);
        }
        let welcome_dir = FreshDir::new("phase-four-overlay-presentation");
        let mut welcome_state = AppState::new(welcome_dir.config());
        let welcome_scene = welcome_state.build_scene_with_viewport(viewport);
        assert_eq!(
            overlay_surface(&welcome_scene).state.overlay_kind,
            Some(OverlayPresentationKind::Welcome)
        );
        assert!(
            !welcome_scene
                .presentation
                .nodes
                .iter()
                .any(|node| node.role == PresentationNodeRole::TextInput),
            "Welcome must not claim an input band"
        );

        let mut menu_state = AppState::new(config(false));
        menu_state.build_scene_with_viewport(viewport);
        menu_state.handle_event(InputEvent::Pointer(PointerEvent {
            kind: PointerKind::Down,
            button: Some(PointerButton::Right),
            column: 5,
            row: 5,
            mods: Modifiers::NONE,
        }));
        let menu_scene = menu_state.build_scene_with_viewport(viewport);
        assert_eq!(
            overlay_surface(&menu_scene).state.overlay_kind,
            Some(OverlayPresentationKind::ContextMenu)
        );
    }

    #[test]
    fn visibility_overlays_reach_the_scene_with_row_hit_targets() {
        let mut state = AppState::new(config(false));
        let size = SceneSize::new(100, 30);

        // Timeline: the dispatch itself is recorded, so at least one entry
        // exists; rows carry hit targets aligned with the drawn window.
        state.dispatch(CommandId::ShowTimeline);
        let scene = build_workspace_scene(&state, size);
        let Some(OverlayScene::Timeline(timeline)) = &scene.overlay else {
            panic!("timeline overlay must be in the scene");
        };
        assert!(!timeline.items.is_empty());
        assert!(
            scene
                .hit_targets
                .iter()
                .any(|target| matches!(target.kind, HitTargetKind::TimelineItem(0)))
        );

        // Session map replaces it (modal exclusivity).
        state.dispatch(CommandId::ShowSessionMap);
        let scene = build_workspace_scene(&state, size);
        let Some(OverlayScene::SessionMap(map)) = &scene.overlay else {
            panic!("session map overlay must be in the scene");
        };
        assert!(map.rows.len() >= 2, "session heading plus its panes");
        assert!(
            scene
                .hit_targets
                .iter()
                .any(|target| matches!(target.kind, HitTargetKind::SessionMapRow(0)))
        );

        // The objective prompt renders for a focused agent pane.
        state.handle_key(Key::plain(KeyCode::Escape));
        state.dispatch(CommandId::NewAgentPane);
        state.dispatch(CommandId::SetAgentObjective);
        let scene = build_workspace_scene(&state, size);
        let Some(OverlayScene::Prompt(prompt)) = &scene.overlay else {
            panic!("prompt overlay must be in the scene");
        };
        assert_eq!(prompt.input, "test objective");
        assert!(prompt.title.contains("Set agent objective"));

        state.handle_event(InputEvent::Composition(
            mandatum_scene::input::CompositionEvent::Preedit {
                text: "界".to_owned(),
                cursor: None,
            },
        ));
        let scene = build_workspace_scene(&state, size);
        let Some(OverlayScene::Prompt(prompt)) = &scene.overlay else {
            panic!("prompt overlay stays open during composition");
        };
        let input_block = layout::filtered_overlay_input_rect(layout::pane_inner_rect(prompt.area));
        let text_input = scene.text_input.expect("prompt owns its IME target");
        assert_eq!(text_input.area.y, input_block.y);
        assert_eq!(text_input.area.height, 1);
        assert_eq!(
            text_input.preedit.map(|preedit| preedit.text),
            Some("界".to_owned())
        );
    }

    /// A unique empty project dir per test, removed on drop.
    struct FreshDir {
        path: std::path::PathBuf,
    }

    impl FreshDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("mandatum-first-run-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("test dir");
            Self { path }
        }

        fn config(&self) -> AppConfig {
            AppConfig {
                workspace_file: self.path.join(".mandatum").join("workspace.json"),
                project_path: self.path.clone(),
                restore_on_startup: true,
                ..AppConfig::default()
            }
        }
    }

    impl Drop for FreshDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    // [L5-GATE] The visible welcome note owns one explicit bare-Escape
    // dismissal; terminal routing resumes immediately afterward.
    #[test]
    fn first_run_escape_is_consumed_while_other_actions_still_fall_through() {
        let escape_dir = FreshDir::new("escape-consumed");
        let mut escape_state = AppState::new(escape_dir.config());
        assert!(matches!(
            build_workspace_scene(&escape_state, SceneSize::new(100, 30)).overlay,
            Some(OverlayScene::Welcome(_))
        ));

        escape_state.handle_event(InputEvent::Key(key(KeyCode::Escape)));

        assert_eq!(
            escape_state.status(),
            "new workspace",
            "first-run Escape must not reach the focused child"
        );
        assert!(
            !matches!(
                build_workspace_scene(&escape_state, SceneSize::new(100, 30)).overlay,
                Some(OverlayScene::Welcome(_))
            ),
            "the frame after Escape must omit the welcome note"
        );
        escape_state.handle_event(InputEvent::Key(key(KeyCode::Escape)));
        assert_eq!(
            escape_state.status(),
            "pane pane-1 has no live PTY",
            "Escape must return to the normal child route after dismissal"
        );

        let palette_dir = FreshDir::new("palette-fallthrough");
        let mut palette_state = AppState::new(palette_dir.config());
        palette_state.handle_event(InputEvent::Key(ctrl('p')));
        assert!(
            palette_state.palette_open(),
            "the first action must still reach workspace controls"
        );
        palette_state.handle_event(InputEvent::Key(key(KeyCode::Escape)));
        assert!(
            !palette_state.palette_open(),
            "Escape after another first action must retain its modal behavior"
        );

        let help_dir = FreshDir::new("direct-help");
        let mut help_state = AppState::new(help_dir.config());
        help_state.dispatch(CommandId::ShowHelp);
        assert!(matches!(
            build_workspace_scene(&help_state, SceneSize::new(100, 30)).overlay,
            Some(OverlayScene::Help(_))
        ));
        help_state.handle_event(InputEvent::Key(key(KeyCode::Escape)));
        assert!(
            build_workspace_scene(&help_state, SceneSize::new(100, 30))
                .overlay
                .is_none(),
            "a directly opened modal must dismiss the latent welcome note and own Escape"
        );

        let ordinary_dir = FreshDir::new("ordinary-fallthrough");
        let mut ordinary_state = AppState::new(ordinary_dir.config());
        ordinary_state.handle_event(InputEvent::Key(key(KeyCode::Char('x'))));
        assert_eq!(
            ordinary_state.status(),
            "pane pane-1 has no live PTY",
            "ordinary first actions must still fall through to the child route"
        );
    }

    #[test]
    fn first_run_note_shows_on_a_fresh_dir_dismisses_on_action_and_never_returns() {
        let dir = FreshDir::new("gating");

        // Fresh dir, no saved workspace: the note is up; the base status names
        // the state, and the composed scene adds each generated route once.
        let mut state = AppState::new(dir.config());
        assert_eq!(state.status(), "new workspace");
        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        assert_eq!(
            scene.status.text,
            "new workspace — ctrl+p commands · right-click menu · f1 help"
        );
        assert_eq!(
            scene.status.text.matches("ctrl+p commands").count(),
            1,
            "the composed footer must name the palette once: {}",
            scene.status.text
        );
        assert_eq!(
            scene.status.text.matches("f1 help").count(),
            1,
            "the composed footer must name help once: {}",
            scene.status.text
        );
        let Some(OverlayScene::Welcome(welcome)) = &scene.overlay else {
            panic!("a fresh dir must show the first-run note");
        };
        assert!(
            welcome.entries.len() + 4 <= 8,
            "the note stays under 8 lines"
        );
        assert!(welcome.entries.iter().any(|entry| entry.keys == "ctrl+p"));
        assert!(welcome.dismissal.contains("dismisses"));

        // A non-Escape action dismisses it, and the action itself still lands.
        state.handle_event(InputEvent::Key(ctrl('p')));
        assert!(state.palette_open(), "the dismissing action still runs");
        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        assert!(
            !matches!(&scene.overlay, Some(OverlayScene::Welcome(_))),
            "any action dismisses the note"
        );
        state.handle_key(key(KeyCode::Escape));

        // Once a workspace is saved, a fresh launch never shows it again.
        state.dispatch(CommandId::SaveWorkspace);
        assert!(state.workspace_file().exists(), "{}", state.status());
        let state = AppState::new(dir.config());
        assert!(!state.status().contains("new workspace"));
        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        assert!(!matches!(&scene.overlay, Some(OverlayScene::Welcome(_))));
    }

    #[test]
    fn reduced_motion_emits_no_transition_targets() {
        let build = |reduced_motion: bool| {
            let mut state = AppState::new(AppConfig {
                reduced_motion,
                ..config(false)
            });
            let mut waiting = AgentPaneIntent::draft("needs approval");
            waiting.status = AgentStatus::WaitingForApproval;
            state
                .workspace_mut()
                .active_session_mut()
                .add_floating_pane("agent", PaneKind::Agent { intent: waiting }, None);
            build_workspace_scene(&state, SceneSize::new(100, 30))
        };
        let plain = build(false);
        let reduced = build(true);
        assert!(!plain.header.attention.is_empty());
        assert!(
            !plain.presentation.transition_targets.is_empty(),
            "ordinary presentation advertises typed motion"
        );
        assert!(
            reduced.presentation.transition_targets.is_empty(),
            "reduced motion must snap every typed transition"
        );
        assert!(reduced.presentation.motion_policy.reduced_motion);
    }

    #[test]
    fn typed_motion_roles_cover_focus_selection_overlay_and_programmatic_geometry() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::SplitRight);
        let first = state.build_scene(SceneSize::new(100, 30));
        let roles = first
            .presentation
            .transition_targets
            .iter()
            .map(|target| target.role)
            .collect::<Vec<_>>();
        assert!(roles.contains(&TransitionRole::Focus));
        assert!(roles.contains(&TransitionRole::PaneGeometry));
        for pane in &first.panes {
            let root = PresentationNodeId::pane(pane.id.clone(), PaneNodePart::Surface);
            for node in transition_family_nodes(&first.presentation.nodes, &root) {
                let has_geometry = first.presentation.transition_targets.iter().any(|target| {
                    target.node_id == node.id
                        && target.role == TransitionRole::PaneGeometry
                        && target.property == TransitionProperty::Geometry
                });
                assert_eq!(
                    has_geometry,
                    node_has_material_motion_surface(node),
                    "only material-backed pane-family surfaces may advertise geometry: {:?}",
                    node.role,
                );
            }
        }
        for separator in first
            .presentation
            .nodes
            .iter()
            .filter(|node| node.role == PresentationNodeRole::Separator)
        {
            assert!(first.presentation.transition_targets.iter().any(|target| {
                target.node_id == separator.id
                    && target.role == TransitionRole::PaneGeometry
                    && target.property == TransitionProperty::Geometry
            }));
        }
        let focus = first
            .presentation
            .nodes
            .iter()
            .find(|node| node.role == PresentationNodeRole::FocusIndicator)
            .expect("focused pane has a focus indicator");
        assert!(
            first.presentation.transition_targets.iter().any(|target| {
                target.node_id == focus.id && target.role == TransitionRole::Focus
            })
        );
        assert!(first.presentation.transition_targets.iter().any(|target| {
            target.node_id == focus.id && target.role == TransitionRole::PaneGeometry
        }));

        state.handle_event(InputEvent::Key(ctrl('p')));
        let overlay = state.build_scene(SceneSize::new(100, 30));
        let roles = overlay
            .presentation
            .transition_targets
            .iter()
            .map(|target| target.role)
            .collect::<Vec<_>>();
        assert!(roles.contains(&TransitionRole::Overlay));
        assert!(
            !roles.contains(&TransitionRole::Selection),
            "selection tracks the pointer and key repeat, so it never eases"
        );
        let overlay_scene = overlay.overlay.as_ref().expect("palette is open");
        let overlay_root =
            PresentationNodeId::overlay(overlay_scene.kind(), OverlayNodePart::Surface);
        for node in transition_family_nodes(&overlay.presentation.nodes, &overlay_root) {
            assert!(
                overlay
                    .presentation
                    .transition_targets
                    .iter()
                    .any(|target| {
                        target.node_id == node.id
                            && target.role == TransitionRole::Overlay
                            && target.property == TransitionProperty::Opacity
                    }),
                "every overlay-family node must share opacity: {:?}",
                node.role
            );
            let has_scale = overlay
                .presentation
                .transition_targets
                .iter()
                .any(|target| {
                    target.node_id == node.id
                        && target.role == TransitionRole::Overlay
                        && target.property == TransitionProperty::Scale
                });
            assert_eq!(
                has_scale,
                node_has_material_motion_surface(node),
                "only material-backed overlay surfaces may advertise scale: {:?}",
                node.role
            );
        }
        let selected = overlay
            .presentation
            .nodes
            .iter()
            .find(|node| node.state.selected)
            .expect("palette has a selected item");
        assert!(
            !overlay
                .presentation
                .transition_targets
                .iter()
                .any(|target| target.role == TransitionRole::Selection),
            "the selected row lands whole on the next frame, never eased"
        );
        assert!(
            overlay
                .presentation
                .transition_targets
                .iter()
                .any(|target| {
                    target.node_id == selected.id && target.role == TransitionRole::Overlay
                })
        );
        for (index, target) in overlay.presentation.transition_targets.iter().enumerate() {
            assert_eq!(
                target.sequence, 0,
                "ordinary stable transitions carry no event sequence"
            );
            assert!(
                !overlay.presentation.transition_targets[index + 1..].contains(target),
                "typed transition targets must be unique"
            );
        }
    }

    #[test]
    fn resize_frame_snaps_pane_geometry_then_restores_programmatic_policy() {
        let mut state = AppState::new(config(false));
        let size = SceneSize::new(100, 30);
        state.build_scene(size);
        state.handle_event(InputEvent::Pointer(PointerEvent {
            kind: PointerKind::Down,
            button: Some(PointerButton::Right),
            column: 5,
            row: 5,
            mods: Modifiers::NONE,
        }));
        let menu = state.build_scene(size);
        assert!(matches!(menu.overlay, Some(OverlayScene::ContextMenu(_))));
        assert!(
            menu.presentation
                .transition_targets
                .iter()
                .any(|target| target.role == TransitionRole::Overlay)
        );

        state.handle_event(InputEvent::Resize(SceneSize::new(120, 40)));

        let direct = state.build_scene(SceneSize::new(120, 40));
        assert!(direct.presentation.motion_policy.direct_geometry);
        assert!(
            direct.overlay.is_none(),
            "resize removes the geometry-anchored context menu"
        );
        assert!(
            direct.presentation.transition_targets.is_empty(),
            "a direct resize frame must suppress overlay, selection, focus, and geometry motion"
        );

        let later = state.build_scene(SceneSize::new(120, 40));
        assert!(!later.presentation.motion_policy.direct_geometry);
        assert!(
            later
                .presentation
                .transition_targets
                .iter()
                .any(|target| { target.role == TransitionRole::PaneGeometry })
        );
    }

    #[test]
    fn help_overlay_reaches_the_scene_and_reflects_the_live_keymap() {
        let mut app_config = config(false);
        app_config.keymap.bind_chord(
            mandatum_commands::CommandId::SplitRight,
            crate::keymap::parse_chord("ctrl+shift+r").unwrap(),
        );
        let mut state = AppState::new(app_config);

        // The status strip always names the help route.
        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        assert!(
            scene.status.text.contains("f1 help"),
            "{}",
            scene.status.text
        );

        state.dispatch(CommandId::ShowHelp);
        let scene = build_workspace_scene(&state, SceneSize::new(100, 40));
        let Some(OverlayScene::Help(help)) = &scene.overlay else {
            panic!("help overlay must be in the scene");
        };
        let split = help
            .items
            .iter()
            .find(|item| item.label == "Split pane right")
            .expect("every command is listed");
        assert_eq!(
            split.keys, "ctrl+shift+r · ctrl+p v",
            "help shows the REBOUND chord, not the default"
        );

        // Filterable with the palette input pattern.
        for character in "split".chars() {
            state.handle_key(key(KeyCode::Char(character)));
        }
        let scene = build_workspace_scene(&state, SceneSize::new(100, 40));
        let Some(OverlayScene::Help(help)) = &scene.overlay else {
            panic!("help overlay stays open while filtering");
        };
        assert!(
            help.items
                .iter()
                .any(|item| item.label == "Split pane right")
        );
        assert!(
            !help.items.iter().any(|item| item.label == "Run task"),
            "non-matching rows drop out"
        );

        // Esc closes.
        state.handle_key(key(KeyCode::Escape));
        let scene = build_workspace_scene(&state, SceneSize::new(100, 40));
        assert!(!matches!(&scene.overlay, Some(OverlayScene::Help(_))));
    }

    #[test]
    fn f1_opens_help_and_toggles_it_closed() {
        let mut state = AppState::new(config(false));
        state.handle_key(key(KeyCode::Function(1)));
        let scene = build_workspace_scene(&state, SceneSize::new(100, 40));
        assert!(matches!(&scene.overlay, Some(OverlayScene::Help(_))));
        state.handle_key(key(KeyCode::Function(1)));
        let scene = build_workspace_scene(&state, SceneSize::new(100, 40));
        assert!(!matches!(&scene.overlay, Some(OverlayScene::Help(_))));
    }

    #[test]
    fn float_moves_by_keyboard_match_the_pointer_drag_intent() {
        let mut state = AppState::new(config(false));
        state.handle_terminal_resize(100, 30);
        // Floating requires another tiled pane to remain.
        state.dispatch(CommandId::SplitRight);
        state.dispatch(CommandId::FloatPane);
        let pane_id = state.workspace().active_session().focused_pane_id().clone();
        let rect_of = |state: &AppState| {
            state
                .workspace()
                .active_session()
                .layout()
                .floating()
                .iter()
                .find(|floating| floating.pane_id == pane_id)
                .map(|floating| (floating.rect.x, floating.rect.y))
                .expect("focused pane is floating")
        };
        let (x0, y0) = rect_of(&state);

        state.dispatch(CommandId::MoveFloatRight);
        state.dispatch(CommandId::MoveFloatDown);
        assert_eq!(rect_of(&state), (x0 + 2, y0 + 1));
        state.dispatch(CommandId::MoveFloatLeft);
        state.dispatch(CommandId::MoveFloatUp);
        assert_eq!(rect_of(&state), (x0, y0));

        // Left/up movement clamps at the workspace-area origin, like a drag.
        for _ in 0..200 {
            state.dispatch(CommandId::MoveFloatLeft);
            state.dispatch(CommandId::MoveFloatUp);
        }
        assert_eq!(rect_of(&state), (0, 0));

        // Docked panes report the honest refusal.
        state.dispatch(CommandId::DockPane);
        state.dispatch(CommandId::MoveFloatRight);
        assert!(
            state.status().contains("not floating"),
            "{}",
            state.status()
        );
    }

    #[test]
    fn search_overlay_reaches_the_scene_with_row_hit_targets() {
        let mut state = AppState::new(config(false));
        let size = SceneSize::new(100, 30);

        state.dispatch(CommandId::SearchSession);
        // The dispatch itself is a timeline fact, so this query always has
        // at least one hit even with no live grids.
        for character in "kind:timeline search".chars() {
            state.handle_key(key(KeyCode::Char(character)));
        }
        let scene = build_workspace_scene(&state, size);
        let Some(OverlayScene::Search(search)) = &scene.overlay else {
            panic!("search overlay must be in the scene");
        };
        assert!(!search.items.is_empty());
        assert!(search.footer.contains("esc close"));
        // Row hit targets align with the drawn window, one row below the
        // search input (the shared palette window math).
        let inner = layout::pane_inner_rect(search.area);
        assert!(scene.hit_targets.iter().any(|target| {
            target.kind == HitTargetKind::SearchItem(0)
                && target.rect == layout::palette_item_rect(inner, 0).expect("first search row")
        }));
    }

    #[test]
    fn terminal_pane_without_runtime_renders_empty_fallback() {
        let state = AppState::new(config(false));

        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        let pane = scene_pane(&scene, "pane-1");

        assert_eq!(pane.kind, PaneSceneKind::Terminal);
        let PaneContent::Empty(empty) = &pane.content else {
            panic!("terminal pane without a PTY must render the empty fallback");
        };
        assert_eq!(empty.restart_generation, 0);
        assert!(!empty.cwd_label.is_empty());
    }

    #[test]
    fn live_terminal_pane_carries_windowed_grid_content() {
        let mut state = AppState::new(config(true));
        state.handle_terminal_resize(100, 30);
        state.handle_event(InputEvent::Paste("echo SCENE_LIVE_OK\r".to_owned()));

        let size = SceneSize::new(100, 30);
        let observed = pump_until(&mut state, |state| {
            let scene = build_workspace_scene(state, size);
            matches!(
                &scene_pane(&scene, "pane-1").content,
                PaneContent::Terminal(surface) if surface_text(surface).contains("SCENE_LIVE_OK")
            )
        });
        assert!(observed, "live shell output did not reach the scene");

        let scene = build_workspace_scene(&state, size);
        let PaneContent::Terminal(surface) = &scene_pane(&scene, "pane-1").content else {
            panic!("live terminal pane must carry a surface");
        };
        // Windowed to the pane's inner area: (100-2) x (28-2).
        assert_eq!(surface.rows.len(), 26);
        assert_eq!(surface.rows[0].len(), 98);
        assert!(surface.cursor.is_some());
        assert!(surface.following_live());
        assert!(!surface.in_copy_mode());
        let output_node = scene
            .presentation
            .nodes
            .iter()
            .find(|node| node.role == PresentationNodeRole::TerminalOutput)
            .expect("live terminal has a typed output node");
        assert!(
            !scene.presentation.transition_targets.iter().any(|target| {
                target.node_id == output_node.id && target.role == TransitionRole::PaneGeometry
            }),
            "terminal glyph content stays direct during pane geometry motion"
        );

        state.shutdown();
    }

    #[test]
    fn copy_mode_reaches_the_surface_as_selection_and_cursor() {
        let mut state = AppState::new(config(true));
        state.handle_terminal_resize(100, 30);
        state.dispatch(CommandId::EnterCopyMode);
        assert!(state.copy_mode_active());
        state.handle_key(key(KeyCode::Char('v')));
        state.handle_key(key(KeyCode::Right));
        state.handle_key(key(KeyCode::Right));

        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        assert!(scene.copy_mode);
        let PaneContent::Terminal(surface) = &scene_pane(&scene, "pane-1").content else {
            panic!("copy-mode pane must carry a surface");
        };
        // The copy cursor starts at the bottom-left of a fresh 26-row grid.
        assert_eq!(surface.copy_cursor, Some(SurfacePosition::new(25, 2)));
        assert_eq!(
            surface.selection,
            Some((SurfacePosition::new(25, 0), SurfacePosition::new(25, 2)))
        );
        assert!(surface.in_copy_mode());

        state.shutdown();
    }

    #[test]
    fn task_pane_reports_status_and_windowed_output() {
        // The output surface shows the bottom rows of the task grid (parity
        // with the pre-scene renderer), so print enough lines for the marker
        // to land inside the visible window.
        let mut config = config(true);
        config.task_command =
            "i=1; while [ \"$i\" -le 20 ]; do echo \"FILL_$i\"; i=$((i+1)); done; echo TASK_OK"
                .to_owned();
        let mut state = AppState::new(config);
        state.handle_terminal_resize(100, 30);
        state.dispatch(CommandId::RunTask);
        let pane_id = state.workspace().active_session().focused_pane_id().clone();

        let size = SceneSize::new(100, 30);
        let observed = pump_until(&mut state, |state| {
            let scene = build_workspace_scene(state, size);
            matches!(
                &scene_pane(&scene, pane_id.as_str()).content,
                PaneContent::Task(task) if task.status_label.as_deref() == Some("succeeded: exit 0")
                    && task.output.as_ref().is_some_and(|output| surface_text(output).contains("TASK_OK"))
            )
        });
        assert!(observed, "task status/output did not reach the scene");

        let scene = build_workspace_scene(&state, size);
        let pane = scene_pane(&scene, pane_id.as_str());
        assert_eq!(pane.kind, PaneSceneKind::Task);
        let PaneContent::Task(task) = &pane.content else {
            panic!("task pane must carry task content");
        };
        assert!(task.command.ends_with("echo TASK_OK"));
        assert_eq!(task.recipe_label, None, "an ad-hoc run names no recipe");
        // The pane states the RESOLVED directory the command runs in.
        assert_eq!(
            task.cwd_label,
            state
                .workspace()
                .active_project_path()
                .display()
                .to_string()
        );
        // Output is windowed to the inner rows left under the detail lines.
        let inner = layout::pane_inner_rect(pane.area);
        let expected_rows = usize::from(inner.height) - pane.detail_lines().len();
        assert_eq!(task.output.as_ref().unwrap().rows.len(), expected_rows);
        let output_node = scene
            .presentation
            .nodes
            .iter()
            .find(|node| node.role == PresentationNodeRole::TaskOutput)
            .expect("live task has a typed output node");
        assert!(
            !scene.presentation.transition_targets.iter().any(|target| {
                target.node_id == output_node.id && target.role == TransitionRole::PaneGeometry
            }),
            "task output stays direct during pane geometry motion"
        );

        state.shutdown();
    }

    // The stranger-test blocker: output shorter than the detail block must
    // still be visible. A task that prints exactly one line and fails must
    // show that line in its output surface — the window anchors to the
    // content, not the bottom of a grid taller than the window.
    #[test]
    fn one_line_failed_task_output_is_visible_in_the_scene() {
        let mut config = config(true);
        config.task_command = "echo ONLY_DIAGNOSTIC_LINE; exit 3".to_owned();
        let mut state = AppState::new(config);
        state.handle_terminal_resize(100, 30);
        state.dispatch(CommandId::RunTask);
        let pane_id = state.workspace().active_session().focused_pane_id().clone();

        let size = SceneSize::new(100, 30);
        let observed = pump_until(&mut state, |state| {
            let scene = build_workspace_scene(state, size);
            matches!(
                &scene_pane(&scene, pane_id.as_str()).content,
                PaneContent::Task(task) if task.status_label.as_deref() == Some("failed: exit 3")
            )
        });
        assert!(observed, "the task never reported its failure");

        let scene = build_workspace_scene(&state, size);
        let PaneContent::Task(task) = &scene_pane(&scene, pane_id.as_str()).content else {
            panic!("task pane must carry task content");
        };
        let output = surface_text(task.output.as_ref().expect("output surface"));
        assert!(
            output.contains("ONLY_DIAGNOSTIC_LINE"),
            "the single diagnostic line must be visible, got:\n{output}"
        );

        state.shutdown();
    }

    #[test]
    fn task_pane_without_runtime_reports_unavailable() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::RunTask);
        let pane_id = state.workspace().active_session().focused_pane_id().clone();

        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        let PaneContent::Task(task) = &scene_pane(&scene, pane_id.as_str()).content else {
            panic!("task pane must carry task content");
        };
        assert!(task.status_label.is_none());
        assert!(task.output.is_none());
    }

    #[test]
    fn agent_pane_summarizes_durable_intent() {
        let mut state = AppState::new(config(false));
        let mut intent = AgentPaneIntent::draft("review failing tests");
        intent.thread_id = Some("thread-1".to_owned());
        intent.status = AgentStatus::WaitingForApproval;
        intent.pending_approvals = 2;
        intent.changed_files = vec![PathBuf::from("src/lib.rs"), PathBuf::from("src/x.rs")];
        intent.latest_summary = Some("waiting for approval".to_owned());
        state
            .workspace_mut()
            .active_session_mut()
            .add_floating_pane("agent", PaneKind::Agent { intent }, None);

        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        let pane = scene_pane(&scene, "pane-2");
        assert_eq!(pane.kind, PaneSceneKind::Agent);
        let PaneContent::Agent(agent) = &pane.content else {
            panic!("agent pane must carry agent content");
        };
        assert_eq!(agent.objective, "review failing tests");
        assert_eq!(agent.status_label, "waiting for approval");
        assert_eq!(agent.status_role, AgentStatus::WaitingForApproval);
        assert_eq!(agent.pending_approvals, 2);
        assert_eq!(agent.changed_file_count, 2);
        assert_eq!(agent.changed_files, vec!["src/lib.rs", "src/x.rs"]);
        assert_eq!(
            agent.latest_summary.as_deref(),
            Some("waiting for approval")
        );
        // No live runtime is attached: live-only fields stay empty.
        assert!(agent.current_action.is_none());
        assert!(agent.pending_approval.is_none());
        assert!(agent.output_tail.is_empty());
    }

    #[test]
    fn waiting_agent_surfaces_approval_detail_in_scene_and_status_strip() {
        use mandatum_agent_runtime::{
            AgentSessionEvent, ApprovalRequest, ApprovalScope, FakeConnector, FakeStep,
            RiskAssessment,
        };

        let request = ApprovalRequest {
            approval_id: "appr-1".to_owned(),
            command: "rm -rf target".to_owned(),
            scope: ApprovalScope {
                cwd: PathBuf::from("/tmp/project"),
                affected_path: Some(PathBuf::from("target")),
            },
            risk: RiskAssessment {
                level: RiskLevel::High,
                basis: "removes files (rm)".to_owned(),
            },
        };
        let mut state = AppState::new(config(false));
        state.set_agent_connector(Box::new(FakeConnector::new(vec![
            FakeStep::Emit(AgentSessionEvent::Action {
                description: "asking to clean the target dir".to_owned(),
            }),
            FakeStep::Emit(AgentSessionEvent::OutputChunk("probing target".to_owned())),
            FakeStep::Emit(AgentSessionEvent::ApprovalRequested(request)),
            FakeStep::AwaitApproval {
                approval_id: "appr-1".to_owned(),
                then_on_approve: vec![],
                then_on_reject: vec![],
            },
        ])));

        state.dispatch(CommandId::StartAgent);
        let pane_id = state.workspace().active_session().focused_pane_id().clone();

        let size = SceneSize::new(100, 30);
        let observed = pump_until(&mut state, |state| {
            let scene = build_workspace_scene(state, size);
            matches!(
                &scene_pane(&scene, pane_id.as_str()).content,
                PaneContent::Agent(agent) if agent.pending_approval.is_some()
            )
        });
        assert!(observed, "approval request did not reach the scene");

        let scene = build_workspace_scene(&state, size);
        let PaneContent::Agent(agent) = &scene_pane(&scene, pane_id.as_str()).content else {
            panic!("agent pane must carry agent content");
        };
        assert_eq!(agent.status_label, "waiting for approval");
        assert_eq!(agent.status_role, AgentStatus::WaitingForApproval);
        assert_eq!(
            agent.current_action.as_deref(),
            Some("asking to clean the target dir")
        );
        assert_eq!(agent.output_tail, vec!["probing target"]);
        let prompt = agent.pending_approval.as_ref().unwrap();
        assert_eq!(prompt.command, "rm -rf target");
        assert_eq!(prompt.cwd, "/tmp/project");
        assert_eq!(prompt.affected_path.as_deref(), Some("target"));
        assert_eq!(prompt.risk_label, "high");
        assert_eq!(prompt.risk_basis, "removes files (rm)");
        assert_eq!(prompt.key_hint, "y approve / n reject");
        assert!(
            prompt.pulse_on,
            "waiting approval remains emphasized after arrival motion settles"
        );
        let approval_node = scene
            .presentation
            .nodes
            .iter()
            .find(|node| {
                node.id
                    == PresentationNodeId::pane(
                        pane_id.clone(),
                        PaneNodePart::Workflow(WorkflowNodePart::Approval),
                    )
            })
            .expect("approval must have one stable typed callout");
        assert_eq!(
            approval_node.role,
            PresentationNodeRole::Workflow(mandatum_scene::WorkflowRowRole::Callout)
        );
        assert_eq!(approval_node.state.tone, PresentationTone::Waiting);
        let arrival = scene
            .presentation
            .transition_targets
            .iter()
            .find(|target| {
                target.node_id == approval_node.id
                    && target.role == TransitionRole::ApprovalArrival
                    && target.property == TransitionProperty::Scale
            })
            .expect("visible approval has typed arrival motion");
        assert!(arrival.sequence > 0);
        for target in scene
            .presentation
            .transition_targets
            .iter()
            .filter(|target| {
                matches!(
                    target.property,
                    TransitionProperty::Geometry | TransitionProperty::Scale
                )
            })
        {
            let node = scene
                .presentation
                .nodes
                .iter()
                .find(|node| node.id == target.node_id)
                .expect("transition target references a presentation node");
            assert!(
                node_has_material_motion_surface(node),
                "text/output-only node {:?} must not advertise material motion",
                node.role
            );
        }
        let compact = state.build_scene(SceneSize::new(100, 8));
        let compact_approval = compact
            .presentation
            .nodes
            .iter()
            .find(|node| node.id == approval_node.id)
            .expect("retained approval identity survives compact layout");
        assert!(
            compact_approval.state.hidden,
            "compact layout retains the workflow identity as hidden"
        );
        let compact_arrival = compact
            .presentation
            .transition_targets
            .iter()
            .find(|target| {
                target.node_id == compact_approval.id
                    && target.role == TransitionRole::ApprovalArrival
            })
            .expect("hidden retained approval keeps transition identity continuous");
        assert_eq!(compact_arrival.sequence, arrival.sequence);
        let visible_again = state.build_scene(size);
        let visible_arrival = visible_again
            .presentation
            .transition_targets
            .iter()
            .find(|target| {
                target.node_id == approval_node.id && target.role == TransitionRole::ApprovalArrival
            })
            .expect("layout polling must not consume pending arrival eligibility");
        assert_eq!(visible_arrival.sequence, arrival.sequence);

        // The waiting pane surfaces globally in the attention strip, with a
        // clickable jump target.
        let segment = scene
            .header
            .attention
            .first()
            .expect("waiting approval must produce an attention segment");
        // The label names the pane by its title; the segment still jumps to
        // the pane by id.
        let title = state
            .workspace()
            .active_session()
            .pane(&pane_id)
            .expect("agent pane exists")
            .title()
            .to_owned();
        assert_eq!(segment.label, format!("1 approval waiting · {title}"));
        assert_eq!(segment.pane.as_ref(), Some(&pane_id));
        assert!(scene.header.text.contains(&segment.label));
        assert!(
            scene.hit_targets.iter().any(|target| {
                matches!(
                    &target.kind,
                    HitTargetKind::AttentionSegment { index: 0, pane: Some(pane), .. } if pane == &pane_id
                ) && target.rect == segment.rect
            }),
            "the attention segment must be clickable"
        );

        state.shutdown();
    }

    #[test]
    fn attention_strip_aggregates_simultaneous_conditions_in_severity_order() {
        let mut state = AppState::new(config(false));
        // A waiting-approval agent, a failed agent, and a blocked agent.
        let mut waiting = AgentPaneIntent::draft("needs approval");
        waiting.status = AgentStatus::WaitingForApproval;
        state
            .workspace_mut()
            .active_session_mut()
            .add_floating_pane("agent", PaneKind::Agent { intent: waiting }, None);
        let mut failed = AgentPaneIntent::draft("failed one");
        failed.status = AgentStatus::Failed;
        state
            .workspace_mut()
            .active_session_mut()
            .add_floating_pane("agent", PaneKind::Agent { intent: failed }, None);
        let mut blocked = AgentPaneIntent::draft("blocked one");
        blocked.status = AgentStatus::Blocked;
        state
            .workspace_mut()
            .active_session_mut()
            .add_floating_pane("agent", PaneKind::Agent { intent: blocked }, None);
        // A failed task (retained status; no live runtime needed).
        state.dispatch(CommandId::RunTask);
        let task_pane = state.workspace().active_session().focused_pane_id().clone();
        state.set_task_status_for_test(&task_pane, "failed: exit 3");

        let scene = build_workspace_scene(&state, SceneSize::new(120, 30));
        let labels: Vec<&str> = scene
            .header
            .attention
            .iter()
            .map(|segment| segment.label.as_str())
            .collect();
        // Segments name panes by their user-facing titles, not pane ids: a
        // glance at the strip says WHICH pane needs eyes.
        assert_eq!(
            labels,
            vec![
                "1 approval waiting · agent",
                "1 task failed · task",
                "2 agents blocked/failed",
            ]
        );
        assert_eq!(
            scene
                .header
                .attention
                .iter()
                .map(|segment| (segment.kind, segment.tone))
                .collect::<Vec<_>>(),
            vec![
                (
                    mandatum_scene::AttentionKind::ApprovalWaiting,
                    mandatum_scene::PresentationTone::Waiting,
                ),
                (
                    mandatum_scene::AttentionKind::TaskFailed,
                    mandatum_scene::PresentationTone::Failure,
                ),
                (
                    mandatum_scene::AttentionKind::AgentBlockedOrFailed,
                    mandatum_scene::PresentationTone::Failure,
                ),
            ]
        );
        assert_eq!(
            scene.header.attention[1].pane,
            Some(PaneId::new("pane-5")),
            "the failed-task segment jumps to the failing pane"
        );
        // Segments land inside the composed header text at their rects.
        for segment in &scene.header.attention {
            assert!(scene.header.text.contains(&segment.label));
            assert!(!segment.rect.is_empty());
        }
        // The count-only agents segment has no single jump pane.
        assert_eq!(scene.header.attention[2].pane, None);
    }

    // A failed task pane states the failing command, the exit status, and
    // the rerun route (live keymap + right-click) in its metadata rows.
    #[test]
    fn failed_task_pane_states_command_exit_and_rerun_route() {
        let mut state = AppState::new(config(false));
        state.dispatch(CommandId::RunTask);
        let pane_id = state.workspace().active_session().focused_pane_id().clone();
        state.set_task_status_for_test(&pane_id, "failed: exit 3");

        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        let pane = scene_pane(&scene, pane_id.as_str());
        let lines = pane.detail_lines();
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("failed: exit 3 · ")),
            "{lines:?}"
        );
        assert!(
            lines.contains(
                &"failure: failed: exit 3 · rerun: ctrl+p r · right-click menu".to_owned()
            ),
            "{lines:?}"
        );
        let failure = scene
            .presentation
            .nodes
            .iter()
            .find(|node| {
                node.id
                    == PresentationNodeId::pane(
                        pane_id.clone(),
                        PaneNodePart::Workflow(WorkflowNodePart::Failure),
                    )
            })
            .expect("failed task must have one stable typed callout");
        assert_eq!(
            failure.role,
            PresentationNodeRole::Workflow(mandatum_scene::WorkflowRowRole::Callout)
        );
        assert_eq!(failure.state.tone, PresentationTone::Failure);
    }

    #[test]
    fn task_status_semantics_distinguish_waiting_runtime_and_diagnostics() {
        assert_eq!(
            task_status_role(
                Some("pending launch: waiting for visible pane size"),
                false,
                false,
            ),
            TaskStatusRole::Waiting
        );
        assert_eq!(
            task_status_role(Some("reader closed unexpectedly"), false, true),
            TaskStatusRole::Diagnostic
        );
        assert_eq!(
            task_status_role(Some("reader closed unexpectedly"), true, true),
            TaskStatusRole::Failed
        );
        assert_eq!(
            task_status_role(Some("running"), false, true),
            TaskStatusRole::Running
        );
    }

    #[test]
    fn artifact_canvas_geometry_is_stable_across_loading_ready_and_failed_states() {
        let states = [
            ArtifactState::Loading,
            ArtifactState::Ready(mandatum_scene::RasterSurface {
                width: 2,
                height: 1,
                revision: 7,
                rgba8: Arc::from([255, 0, 0, 255, 0, 255, 0, 255]),
            }),
            ArtifactState::Failed {
                message: "missing".to_owned(),
            },
        ];
        let viewport = ViewportMetrics::from_scene_size(SceneSize::new(80, 24));
        let pane_id = PaneId::new("artifact");
        let parent = PresentationNodeId::pane(pane_id.clone(), PaneNodePart::Surface);
        let mut canvases = Vec::new();
        for state in states {
            let pane = PaneScene {
                content_revision: 0,
                id: pane_id.clone(),
                title: "artifact".to_owned(),
                kind: PaneSceneKind::Artifact,
                area: SceneRect::new(0, 1, 80, 22),
                focused: true,
                floating: false,
                stacked: false,
                zoomed: false,
                content: PaneContent::Artifact(mandatum_scene::ArtifactContent {
                    source_label: "shot.png".to_owned(),
                    alt_text: "shot".to_owned(),
                    fit: mandatum_core::ArtifactFit::Contain,
                    state,
                }),
            };
            let mut nodes = Vec::new();
            push_workflow_presentation(&pane, viewport, &parent, &mut nodes);
            let canvas = nodes
                .iter()
                .find(|node| node.role == PresentationNodeRole::ArtifactCanvas)
                .expect("every artifact state retains its canvas");
            canvases.push((canvas.id.clone(), canvas.cell_rect, canvas.logical_rect));
            assert!(nodes.iter().any(|node| {
                node.id
                    == PresentationNodeId::pane(
                        pane_id.clone(),
                        PaneNodePart::Workflow(WorkflowNodePart::ArtifactInspector),
                    )
            }));
        }
        assert!(canvases.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn workflow_ids_survive_resize_and_status_badges_stay_compact() {
        let pane_id = PaneId::new("agent");
        let parent = PresentationNodeId::pane(pane_id.clone(), PaneNodePart::Surface);
        let content = PaneContent::Agent(mandatum_scene::AgentContent {
            objective: "review the failing workflow".to_owned(),
            status_label: "waiting for approval".to_owned(),
            status_role: AgentStatus::WaitingForApproval,
            pending_approvals: 1,
            changed_file_count: 1,
            changed_files: vec!["src/lib.rs".to_owned()],
            latest_summary: Some("prepared the repair".to_owned()),
            current_action: Some("waiting for a decision".to_owned()),
            last_error: None,
            relaunch_hint: None,
            pending_approval: Some(mandatum_scene::AgentApprovalPrompt {
                command: "cargo test".to_owned(),
                cwd: "/tmp/project".to_owned(),
                affected_path: None,
                risk_label: "low".to_owned(),
                risk_basis: "test execution".to_owned(),
                key_hint: "y approve / n reject".to_owned(),
                pulse_on: false,
            }),
            output_tail: vec!["one".to_owned(), "two".to_owned()],
        });
        let make_pane = |height| PaneScene {
            content_revision: 0,
            id: pane_id.clone(),
            title: "agent".to_owned(),
            kind: PaneSceneKind::Agent,
            area: SceneRect::new(0, 1, 80, height),
            focused: true,
            floating: false,
            stacked: false,
            zoomed: false,
            content: content.clone(),
        };
        let mut tall = Vec::new();
        push_workflow_presentation(
            &make_pane(22),
            ViewportMetrics::from_scene_size(SceneSize::new(80, 24)),
            &parent,
            &mut tall,
        );
        let mut short = Vec::new();
        push_workflow_presentation(
            &make_pane(5),
            ViewportMetrics::from_scene_size(SceneSize::new(80, 7)),
            &parent,
            &mut short,
        );

        assert_eq!(
            tall.iter().map(|node| &node.id).collect::<Vec<_>>(),
            short.iter().map(|node| &node.id).collect::<Vec<_>>()
        );
        assert!(short.iter().any(|node| node.state.hidden));
        let badge = tall
            .iter()
            .find(|node| node.role == PresentationNodeRole::WorkflowStatusBadge)
            .expect("agent status badge");
        assert_eq!(badge.cell_rect.unwrap().height, 1);
        assert!(badge.cell_rect.unwrap().width < 78);
        assert_eq!(badge.state.tone, PresentationTone::Waiting);
    }

    // A failed agent pane keeps the failure reason from its Failed event
    // and the relaunch route on screen, frame after frame.
    #[test]
    fn failed_agent_pane_states_the_error_and_relaunch_route_persistently() {
        use mandatum_agent_runtime::{AgentSessionEvent, FakeConnector, FakeStep};

        let mut state = AppState::new(config(false));
        state.set_agent_connector(Box::new(FakeConnector::new(vec![
            FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
            FakeStep::Emit(AgentSessionEvent::Failed {
                error: "model quota exhausted".to_owned(),
            }),
        ])));
        state.dispatch(CommandId::StartAgent);
        let pane_id = state.workspace().active_session().focused_pane_id().clone();

        let size = SceneSize::new(100, 30);
        let observed = pump_until(&mut state, |state| {
            let scene = build_workspace_scene(state, size);
            matches!(
                &scene_pane(&scene, pane_id.as_str()).content,
                PaneContent::Agent(agent) if agent.status_role == AgentStatus::Failed
            )
        });
        assert!(observed, "the failure never reached the scene");

        // Frame after frame — including after other status churn — the
        // failure stays legible on the pane itself.
        state.dispatch(CommandId::ShowSessionMap);
        state.handle_event(InputEvent::Key(Key::plain(KeyCode::Escape)));
        let scene = build_workspace_scene(&state, size);
        let lines = scene_pane(&scene, pane_id.as_str()).detail_lines();
        assert!(lines.contains(&"status: failed".to_owned()), "{lines:?}");
        assert!(
            lines.contains(&"error: model quota exhausted".to_owned()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"relaunch: ctrl+p g · right-click menu".to_owned()),
            "{lines:?}"
        );

        state.shutdown();
    }

    // A launch that never produced a session has no live runtime to ask, so
    // the failure reason must reach the card from durable intent alone.
    #[test]
    fn launch_failure_without_a_session_still_states_the_error_on_the_card() {
        use mandatum_agent_runtime::{
            AgentConnector, AgentConnectorError, AgentLaunchSpec, AgentSession,
        };

        struct RefusingConnector;
        impl AgentConnector for RefusingConnector {
            fn launch(&self, _: &AgentLaunchSpec) -> Result<AgentSession, AgentConnectorError> {
                Err(AgentConnectorError::LaunchFailed {
                    message: "`claude` was not found on PATH — install Claude Code or add it \
                              to PATH"
                        .to_owned(),
                })
            }
            fn name(&self) -> &str {
                "refusing"
            }
        }

        let mut state = AppState::new(config(false));
        state.set_agent_connector(Box::new(RefusingConnector));
        state.dispatch(CommandId::StartAgent);
        let pane_id = state.workspace().active_session().focused_pane_id().clone();

        let scene = build_workspace_scene(&state, SceneSize::new(100, 30));
        let lines = scene_pane(&scene, pane_id.as_str()).detail_lines();
        assert!(lines.contains(&"status: failed".to_owned()), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("error: `claude` was not found on PATH")),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"relaunch: ctrl+p g · right-click menu".to_owned()),
            "{lines:?}"
        );
        // The transient status line names the pane and the reason exactly
        // once — no doubled "launch failed" prefix.
        assert!(
            state.status().starts_with(&format!(
                "agent launch failed for {pane_id}: `claude` was not found"
            )),
            "{}",
            state.status()
        );

        state.shutdown();
    }
}
