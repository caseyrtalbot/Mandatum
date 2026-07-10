//! Cross-frontend proof for the scene contract: one real session scene
//! renders through BOTH the ratatui adapter and a plain-text frontend that
//! never touches ratatui, and the same essential content (pane titles,
//! terminal text, status) appears in both. No frontend type crosses the
//! scene seam in either direction.

use std::time::Duration;

use crossterm::event::Event;
use mandatum_app::{AgentConnectorKind, AppConfig, AppState, build_workspace_scene};
use mandatum_scene::{OverlayScene, PaneContent, SceneSize, TerminalSurface, WorkspaceScene};
use ratatui::{Terminal, backend::TestBackend};

/// A second frontend: renders a scene to plain text lines using only scene
/// types. This is what a GPU or native frontend would do with its own paint
/// calls.
fn render_scene_to_text(scene: &WorkspaceScene) -> Vec<String> {
    let mut lines = Vec::new();

    let zoom = if scene.header.zoomed { " | zoom" } else { "" };
    lines.push(format!(
        "Mandatum | {} | panes {} | focused {}{}",
        scene.header.session_name, scene.header.pane_count, scene.header.focused_pane, zoom
    ));

    for pane in &scene.panes {
        lines.push(format!("[{}] {}", pane.title, pane.kind.label()));
        lines.extend(pane.detail_lines());
        let surface = match &pane.content {
            PaneContent::Terminal(surface) => Some(surface),
            PaneContent::Task(task) => task.output.as_ref(),
            _ => None,
        };
        if let Some(surface) = surface {
            lines.extend(surface_rows(surface));
        }
    }

    if let Some(OverlayScene::Palette(palette)) = &scene.overlay {
        lines.extend(
            palette
                .items
                .iter()
                .map(|item| format!("{}  {}", item.label, item.detail)),
        );
    }

    lines.push(scene.status.clone().unwrap_or_else(|| "ready".to_owned()));
    lines
}

fn surface_rows(surface: &TerminalSurface) -> Vec<String> {
    surface
        .rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.character).collect::<String>())
        .collect()
}

/// Render the same scene through the ratatui adapter into a test buffer.
fn render_scene_to_ratatui(scene: &WorkspaceScene) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(scene.size.width, scene.size.height)).unwrap();
    terminal
        .draw(|frame| mandatum_renderer::render(frame, scene))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer.cell((x, y)).unwrap().symbol().to_owned())
                .collect()
        })
        .collect()
}

#[test]
fn same_scene_renders_equivalent_content_in_both_frontends() {
    let project_path = std::env::temp_dir();
    let mut state = AppState::new(AppConfig {
        workspace_name: "Mandatum".to_owned(),
        workspace_file: project_path.join("mandatum-frontend-parity-test.json"),
        project_path,
        shell_program: "/bin/sh".to_owned(),
        task_command: "printf TASK_OK".to_owned(),
        agent_connector: AgentConnectorKind::Fake,
        agent_objective: "test objective".to_owned(),
        agent_model: None,
        spawn_pty: true,
        restore_on_startup: false,
    });
    state.handle_terminal_resize(100, 30);
    state.handle_event(Event::Paste("echo PARITY_MARKER\r".to_owned()));

    // Pump the live runtime until the shell's output reaches the scene.
    let size = SceneSize::new(100, 30);
    let mut observed = false;
    for _ in 0..300 {
        state.tick_runtime();
        let scene = build_workspace_scene(&state, size);
        if scene.panes.iter().any(|pane| {
            matches!(&pane.content, PaneContent::Terminal(surface)
                if surface_rows(surface).join("\n").contains("PARITY_MARKER"))
        }) {
            observed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(observed, "live shell output did not reach the scene");

    let scene = build_workspace_scene(&state, size);
    let text = render_scene_to_text(&scene).join("\n");
    let ratatui = render_scene_to_ratatui(&scene).join("\n");

    for pane in &scene.panes {
        assert!(text.contains(&pane.title), "text frontend lost pane title");
        assert!(
            ratatui.contains(&pane.title),
            "ratatui frontend lost pane title"
        );
    }
    let status = scene.status.clone().expect("scene carries a status line");
    for essential in ["PARITY_MARKER", status.as_str()] {
        assert!(text.contains(essential), "text frontend lost {essential:?}");
        assert!(
            ratatui.contains(essential),
            "ratatui frontend lost {essential:?}"
        );
    }

    state.shutdown();
}
