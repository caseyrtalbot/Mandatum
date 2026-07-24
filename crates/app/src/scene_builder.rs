//! Builds the frontend-neutral workspace scene each frame from app state.
//!
//! The `mandatum-terminal-vt` -> `mandatum-scene` conversion lives here on
//! the app side: the scene crate never depends on the terminal engine, so no
//! parser type crosses the frontend seam (L1/L4).

use mandatum_agent_runtime::RiskLevel;
use mandatum_core::{AgentPaneIntent, PaneId, PaneKind, PaneSpec, Session, TaskPaneIntent};
use mandatum_scene::{
    AgentApprovalPrompt, AgentContent, CellOccupancy, EmptyContent, HeaderScene, HitTarget,
    HitTargetKind, OverlayScene, PaneContent, PaneScene, PaneSceneKind, PreeditScene, SceneCell,
    SceneCellStyle, SceneColor, SceneRect, SceneSize, StatusScene, SurfacePosition, TaskContent,
    TerminalSurface, TextInputKind, TextInputScene, WorkspaceScene,
    cell_program::display_width,
    layout::{self, PaneLayout},
};
use mandatum_terminal_vt::{CellStyle, Color as VtColor, TerminalCellOccupancy, TerminalGrid};

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

/// Build one frame of workspace scene from live app state.
pub fn build_workspace_scene(state: &AppState, size: SceneSize) -> WorkspaceScene {
    let workspace = state.workspace();
    let session = workspace.active_session();
    let area = layout::workspace_scene_area(size);

    let panes = layout::layout_panes(workspace, area)
        .into_iter()
        .filter_map(|placed| {
            session
                .pane(&placed.pane_id)
                .map(|pane| pane_scene(state, session, pane, placed))
        })
        .collect::<Vec<_>>();

    // The overlays are all modal; opening one closes the others, so at most
    // one overlay exists per frame.
    let overlay = state
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
        // The first-run note is last: it is not modal, so any real overlay
        // outranks it (and the action that opened one dismissed it anyway).
        .or_else(|| state.welcome_overlay_scene(size).map(OverlayScene::Welcome));

    // The attention strip: approvals, failed tasks, stuck agents — or calm
    // session facts. Composed here so `&WorkspaceScene` alone paints a frame.
    let header = header_scene(state, layout::header_rect(size));
    let hit_targets = hit_targets(workspace, &panes, &header, size, overlay.as_ref());
    let text_input = text_input_scene(state, &panes, overlay.as_ref());

    WorkspaceScene {
        size,
        header,
        panes,
        overlay,
        status: StatusScene {
            area: layout::status_rect(size),
            text: status_text(state),
        },
        focused_pane: session.focused_pane_id().clone(),
        hit_targets,
        copy_mode: state.copy_mode_active(),
        text_input,
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
) -> PaneScene {
    let inner = layout::pane_inner_rect(placed.area);
    let content = match pane.kind() {
        PaneKind::Terminal { .. } => match state.terminal_grid(pane.id()) {
            Some(grid) => PaneContent::Terminal(terminal_surface(
                grid,
                state.pane_view_state(pane.id()),
                inner.width,
                inner.height,
            )),
            None => PaneContent::Empty(empty_content(state, pane)),
        },
        PaneKind::Task { intent } => PaneContent::Task(task_content(state, pane, intent)),
        PaneKind::Agent { intent } => PaneContent::Agent(agent_content(state, pane.id(), intent)),
        PaneKind::Artifact { intent } => {
            PaneContent::Artifact(state.artifact_content(pane.id(), intent))
        }
        PaneKind::StatusLog { .. } => PaneContent::Empty(empty_content(state, pane)),
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
        content,
    };

    // Window a task's live output to the rows left under its detail lines.
    // The detail line count is stable whether or not the output surface is
    // attached (the "output:" marker replaces "output: no live grid
    // attached"), so measuring before attaching is exact.
    let detail_rows = scene.detail_lines().len() as u16;
    if let PaneContent::Task(task) = &mut scene.content
        && let Some((_, Some(grid))) = state.task_view(&scene.id)
    {
        task.output = Some(task_output_surface(
            grid,
            inner.width,
            inner.height.saturating_sub(detail_rows),
        ));
    }

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
    TaskContent {
        command: intent.command.clone(),
        // The directory the command actually runs in — the same resolution
        // the spawn path uses (intent -> pane -> project), never "unset".
        cwd_label: resolve_pane_cwd(state.workspace(), pane, intent.cwd.as_ref())
            .display()
            .to_string(),
        recipe_label: intent.recipe_id.clone(),
        status_label: state
            .task_view(pane.id())
            .map(|(status, _)| status.to_owned()),
        // The live keyboard route to Rerun task; the scene shows it on
        // failed tasks next to the right-click route.
        rerun_hint: Some(state.command_key_hint(mandatum_commands::CommandId::RerunTask))
            .filter(|hint| !hint.is_empty()),
        output: None,
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
        last_error: live.and_then(|runtime| runtime.last_error.map(str::to_owned)),
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
                // The product's single motion: the approval header pulses
                // at ~1 Hz off the wall clock (steady under reduced
                // motion). The heartbeat repaint keeps it ticking when the
                // workspace is otherwise idle.
                pulse_on: approval_pulse_on(crate::timeline::now_ms(), state.reduced_motion()),
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

/// Whether the approval header draws emphasized at this instant: a ~1 Hz
/// alternation off the wall clock, held steady (always on) under reduced
/// motion.
pub(crate) fn approval_pulse_on(now_ms: u64, reduced_motion: bool) -> bool {
    reduced_motion || (now_ms / 1_000).is_multiple_of(2)
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
        PaneKind::StatusLog { .. } => PaneSceneKind::StatusLog,
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

    let rows = (0..view_rows)
        .map(|line| {
            let absolute_row = first_row + line;
            let mut row = (0..columns)
                .map(|column| {
                    let cell = grid.history_cell(absolute_row, column).unwrap_or_default();
                    SceneCell {
                        occupancy: match cell.occupancy() {
                            TerminalCellOccupancy::Grapheme(grapheme) => {
                                CellOccupancy::Grapheme(grapheme.clone())
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
                    .history_cell(absolute_row, columns)
                    .is_some_and(|cell| {
                        matches!(cell.occupancy(), TerminalCellOccupancy::WideContinuation)
                    })
                && let Some(last) = row.last_mut()
            {
                last.occupancy = CellOccupancy::Grapheme("\u{fffd}".to_owned());
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
                targets.push(HitTarget {
                    rect: SceneRect::new(inner.x, inner.y + 1 + row as u16, inner.width, 1),
                    kind: HitTargetKind::PaletteItem(index),
                });
            }
        }
        Some(OverlayScene::ContextMenu(menu)) => {
            let inner = layout::pane_inner_rect(menu.area);
            for index in 0..menu.items.len().min(usize::from(inner.height)) {
                targets.push(HitTarget {
                    rect: SceneRect::new(inner.x, inner.y + index as u16, inner.width, 1),
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
                targets.push(HitTarget {
                    rect: SceneRect::new(inner.x, inner.y + 1 + row as u16, inner.width, 1),
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
                targets.push(HitTarget {
                    rect: SceneRect::new(inner.x, inner.y + 1 + row as u16, inner.width, 1),
                    kind: HitTargetKind::SearchItem(index),
                });
            }
        }
        Some(OverlayScene::SessionMap(map)) => {
            let inner = layout::pane_inner_rect(map.area);
            let window = layout::session_map_item_window(inner, map.rows.len(), Some(map.selected));
            for (row, index) in window.enumerate() {
                targets.push(HitTarget {
                    rect: SceneRect::new(inner.x, inner.y + row as u16, inner.width, 1),
                    kind: HitTargetKind::SessionMapRow(index),
                });
            }
        }
        // The prompt and help have no row targets (click-away dismisses
        // them); the first-run note is not even modal.
        Some(OverlayScene::Prompt(_) | OverlayScene::Help(_) | OverlayScene::Welcome(_)) | None => {
        }
    }

    targets
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use mandatum_commands::CommandId;
    use mandatum_core::AgentStatus;
    use mandatum_scene::input::{InputEvent, Key, KeyCode};
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
            CellOccupancy::Grapheme("\u{fffd}".to_owned())
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
            scene.header.text.contains("2 pane(s)"),
            "{}",
            scene.header.text
        );
        assert!(
            scene.header.text.contains("agent: fake"),
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
            SceneRect::new(inner.x, inner.y + 1, inner.width, 1)
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

        // Any action dismisses it, and the action itself still lands.
        state.handle_key(ctrl('p'));
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
    fn reduced_motion_kills_the_pulse_and_no_other_motion_exists() {
        // The [ui] reduced_motion contract, in two halves.
        //
        // (1) The approval pulse — the product's single animation — holds
        // steady (always emphasized) under reduced motion, at every instant
        // of its 1 Hz cycle.
        for now_ms in [0u64, 500, 1_000, 1_500, 2_000, 999_999_999] {
            assert!(
                approval_pulse_on(now_ms, true),
                "reduced motion must hold the pulse steady at t={now_ms}"
            );
        }
        assert!(approval_pulse_on(0, false));
        assert!(
            !approval_pulse_on(1_000, false),
            "without reduced motion the pulse alternates"
        );

        // (2) No other motion exists: outside the pulse (no live approval
        // here), the built scene is byte-identical with the flag on and
        // off, attention strip included — this fails the moment someone
        // adds motion without gating it on the flag.
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
        assert_eq!(
            plain, reduced,
            "outside the gated pulse, reduced_motion must have nothing left to disable"
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
                && target.rect == SceneRect::new(inner.x, inner.y + 1, inner.width, 1)
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
                    HitTargetKind::AttentionSegment { index: 0, pane: Some(pane) } if pane == &pane_id
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
            lines.iter().any(|line| line.starts_with("command: ")),
            "{lines:?}"
        );
        assert!(lines.contains(&"runtime status: failed: exit 3".to_owned()));
        assert!(
            lines.contains(&"rerun: ctrl+p r · right-click menu".to_owned()),
            "{lines:?}"
        );
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
}
