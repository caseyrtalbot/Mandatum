//! Builds the frontend-neutral workspace scene each frame from app state.
//!
//! The `mandatum-terminal-vt` -> `mandatum-scene` conversion lives here on
//! the app side: the scene crate never depends on the terminal engine, so no
//! parser type crosses the frontend seam (L1/L4).

use mandatum_agent_runtime::RiskLevel;
use mandatum_core::{AgentPaneIntent, PaneId, PaneKind, PaneSpec, Session, TaskPaneIntent};
use mandatum_scene::{
    AgentApprovalPrompt, AgentContent, EmptyContent, HeaderScene, HitTarget, HitTargetKind,
    OverlayScene, PaletteOverlay, PaneContent, PaneScene, PaneSceneKind, SceneCell, SceneCellStyle,
    SceneColor, SceneRect, SceneSize, SurfacePosition, TaskContent, TerminalSurface,
    WorkspaceScene,
    layout::{self, PaneLayout},
};
use mandatum_terminal_vt::{CellStyle, Color as VtColor, TerminalGrid};

use crate::app_state::{AppState, agent_status_label};

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

    let overlay = state.palette_open().then(|| {
        OverlayScene::Palette(PaletteOverlay {
            area: layout::palette_overlay_rect(size),
            items: state.palette_items(),
            selected: None,
        })
    });

    let hit_targets = hit_targets(&panes, size, overlay.as_ref());

    WorkspaceScene {
        size,
        header: HeaderScene {
            session_name: session.name().to_owned(),
            pane_count: session.panes().len(),
            focused_pane: session.focused_pane_id().clone(),
            zoomed: session.layout().zoomed().is_some(),
        },
        panes,
        overlay,
        status: Some(status_with_agent_attention(state, session)),
        focused_pane: session.focused_pane_id().clone(),
        hit_targets,
        copy_mode: state.copy_mode_active(),
    }
}

/// The status strip text: agent approval attention first, then the app
/// status. A pane waiting for approval must be visible globally.
fn status_with_agent_attention(state: &AppState, session: &Session) -> String {
    let waiting = session
        .panes()
        .iter()
        .filter(|(pane_id, pane)| {
            matches!(pane.kind(), PaneKind::Agent { .. })
                && state
                    .agent_runtime_view(pane_id)
                    .is_some_and(|runtime| runtime.pending_approval.is_some())
        })
        .map(|(pane_id, _)| pane_id.clone())
        .collect::<Vec<_>>();

    match waiting.as_slice() {
        [] => state.status().to_owned(),
        [pane_id] => format!("1 approval waiting — {pane_id} | {}", state.status()),
        panes => format!(
            "{} approvals waiting — {} | {}",
            panes.len(),
            panes[0],
            state.status()
        ),
    }
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
            None => PaneContent::Empty(empty_content(pane)),
        },
        PaneKind::Task { intent } => PaneContent::Task(task_content(state, pane, intent)),
        PaneKind::Agent { intent } => PaneContent::Agent(agent_content(state, pane.id(), intent)),
        PaneKind::StatusLog { .. } => PaneContent::Empty(empty_content(pane)),
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
        task.output = Some(terminal_surface(
            grid,
            PaneViewState::default(),
            inner.width,
            inner.height.saturating_sub(detail_rows),
        ));
    }

    scene
}

fn task_content(state: &AppState, pane: &PaneSpec, intent: &TaskPaneIntent) -> TaskContent {
    TaskContent {
        command: intent.command.clone(),
        cwd_label: intent
            .cwd
            .as_ref()
            .or_else(|| pane.cwd())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unset".to_owned()),
        recipe_label: intent
            .recipe_id
            .clone()
            .unwrap_or_else(|| "ad hoc".to_owned()),
        status_label: state
            .task_view(pane.id())
            .map(|(status, _)| status.to_owned()),
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
        pending_approvals: intent.pending_approvals,
        changed_file_count: intent.changed_files.len(),
        changed_files,
        latest_summary: intent.latest_summary.clone(),
        current_action: live.and_then(|runtime| runtime.current_action.clone()),
        pending_approval: live
            .and_then(|runtime| runtime.pending_approval.as_ref())
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

fn empty_content(pane: &PaneSpec) -> EmptyContent {
    EmptyContent {
        // "cwd: unset" (not "unset") preserves the exact fallback line the
        // pre-scene renderer displayed.
        cwd_label: pane
            .cwd()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "cwd: unset".to_owned()),
        restart_generation: pane.restart_generation(),
    }
}

fn pane_scene_kind(kind: &PaneKind) -> PaneSceneKind {
    match kind {
        PaneKind::Terminal { .. } => PaneSceneKind::Terminal,
        PaneKind::Task { .. } => PaneSceneKind::Task,
        PaneKind::Agent { .. } => PaneSceneKind::Agent,
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
            (0..columns)
                .map(|column| {
                    let cell = grid.history_cell(absolute_row, column).unwrap_or_default();
                    SceneCell {
                        character: cell.character(),
                        style: scene_cell_style(cell.style()),
                    }
                })
                .collect()
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

fn hit_targets(
    panes: &[PaneScene],
    size: SceneSize,
    overlay: Option<&OverlayScene>,
) -> Vec<HitTarget> {
    let mut targets = Vec::new();

    for pane in panes {
        if pane.area.is_empty() {
            continue;
        }
        targets.push(HitTarget {
            rect: SceneRect::new(pane.area.x, pane.area.y, pane.area.width, 1),
            kind: HitTargetKind::PaneTitle(pane.id.clone()),
        });
        targets.push(HitTarget {
            rect: layout::pane_inner_rect(pane.area),
            kind: HitTargetKind::PaneBody(pane.id.clone()),
        });
    }

    let status = layout::status_rect(size);
    if !status.is_empty() {
        targets.push(HitTarget {
            rect: status,
            kind: HitTargetKind::StatusStrip,
        });
    }

    if let Some(OverlayScene::Palette(palette)) = overlay {
        let inner = layout::pane_inner_rect(palette.area);
        for index in 0..palette.items.len().min(usize::from(inner.height)) {
            targets.push(HitTarget {
                rect: SceneRect::new(inner.x, inner.y + index as u16, inner.width, 1),
                kind: HitTargetKind::PaletteItem(index),
            });
        }
    }

    targets
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use mandatum_commands::CommandId;
    use mandatum_core::AgentStatus;

    use super::*;
    use crate::app_shell::{AgentConnectorKind, AppConfig};

    fn config(spawn_pty: bool) -> AppConfig {
        // The system temp dir always exists, so live PTY spawns get a valid cwd
        // without per-test directory setup (nothing here persists a workspace).
        let project_path = std::env::temp_dir();
        AppConfig {
            workspace_name: "Mandatum".to_owned(),
            workspace_file: project_path.join("mandatum-scene-builder-test.json"),
            project_path,
            shell_program: "/bin/sh".to_owned(),
            task_command: "printf 'TASK_OK\\n'".to_owned(),
            agent_connector: AgentConnectorKind::Fake,
            agent_objective: "test objective".to_owned(),
            agent_model: None,
            spawn_pty,
            restore_on_startup: false,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL)
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
                    .map(|cell| cell.character)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
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
        assert_eq!(scene.status.as_deref(), Some(state.status()));
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
    fn palette_overlay_carries_items_and_item_targets() {
        let mut state = AppState::new(config(false));
        state.handle_key(ctrl('p'));

        let size = SceneSize::new(120, 40);
        let scene = build_workspace_scene(&state, size);

        let Some(OverlayScene::Palette(palette)) = &scene.overlay else {
            panic!("palette must be open in the scene");
        };
        assert_eq!(palette.area, layout::palette_overlay_rect(size));
        assert_eq!(palette.items.len(), state.palette_items().len());
        assert!(palette.selected.is_none());
        let item_targets = scene
            .hit_targets
            .iter()
            .filter(|target| matches!(target.kind, HitTargetKind::PaletteItem(_)))
            .count();
        assert!(item_targets > 0);
        assert!(item_targets <= palette.items.len());
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
        state.handle_event(Event::Paste("echo SCENE_LIVE_OK\r".to_owned()));

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
        assert_eq!(task.recipe_label, "configured");
        // Output is windowed to the inner rows left under the detail lines.
        let inner = layout::pane_inner_rect(pane.area);
        let expected_rows = usize::from(inner.height) - pane.detail_lines().len();
        assert_eq!(task.output.as_ref().unwrap().rows.len(), expected_rows);

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

        // The waiting pane surfaces globally in the status strip.
        let status = scene.status.as_deref().unwrap();
        assert!(
            status.starts_with(&format!("1 approval waiting — {pane_id}")),
            "status strip must lead with the waiting approval, got {status:?}"
        );

        state.shutdown();
    }
}
