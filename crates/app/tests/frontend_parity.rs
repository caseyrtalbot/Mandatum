//! Cross-frontend proof for the scene contract: one real session scene
//! renders through BOTH the ratatui adapter and a plain-text frontend that
//! never touches ratatui, and the same essential content (header strip,
//! pane titles, terminal text, status) appears in both. No frontend type
//! crosses the scene seam in either direction, and `&WorkspaceScene` alone
//! suffices to paint a frame: the strips carry their own areas and text.

use std::time::Duration;

use mandatum_app::{AppConfig, AppState, build_workspace_scene};
use mandatum_scene::{
    OverlayScene, PaneContent, SceneSize, TerminalSurface, Theme, WorkspaceScene, input::InputEvent,
};
use ratatui::{Terminal, backend::TestBackend};

/// A second frontend: renders a scene to plain text lines using only scene
/// types. This is what a GPU or native frontend would do with its own paint
/// calls.
fn render_scene_to_text(scene: &WorkspaceScene) -> Vec<String> {
    let mut lines = Vec::new();

    // The header strip is scene-carried text; attention segments are style
    // metadata over that same text, so painting the text alone is complete.
    lines.push(scene.header.text.clone());

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

    match &scene.overlay {
        Some(OverlayScene::Palette(palette)) => {
            lines.extend(
                palette
                    .items
                    .iter()
                    .map(|item| format!("{}  {}", item.label, item.detail)),
            );
        }
        Some(OverlayScene::Timeline(timeline)) => {
            lines.extend(
                timeline
                    .items
                    .iter()
                    .map(|item| format!("{} {}  {}", item.glyph, item.when, item.text)),
            );
            lines.push(timeline.footer.clone());
        }
        Some(OverlayScene::Search(search)) => {
            lines.extend(
                search
                    .items
                    .iter()
                    .map(|item| format!("{}  {}", item.source, item.text)),
            );
            lines.push(search.footer.clone());
        }
        Some(OverlayScene::SessionMap(map)) => {
            lines.extend(
                map.rows
                    .iter()
                    .map(|row| format!("{} {}  {}", row.glyph, row.label, row.state)),
            );
        }
        Some(OverlayScene::Prompt(prompt)) => {
            lines.push(format!("{}> {}", prompt.title, prompt.input));
        }
        Some(OverlayScene::ContextMenu(menu)) => {
            lines.extend(menu.items.iter().map(|item| item.label.clone()));
        }
        Some(OverlayScene::Help(help)) => {
            lines.extend(
                help.items
                    .iter()
                    .map(|item| format!("{}  {}", item.label, item.keys)),
            );
            lines.push(help.footer.clone());
        }
        Some(OverlayScene::Welcome(welcome)) => {
            lines.push(welcome.introduction.clone());
            lines.extend(
                welcome
                    .entries
                    .iter()
                    .map(|entry| format!("{}  {}", entry.keys, entry.description)),
            );
            lines.push(welcome.dismissal.clone());
        }
        None => {}
    }

    lines.push(scene.status.text.clone());
    lines
}

fn surface_rows(surface: &TerminalSurface) -> Vec<String> {
    surface
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(mandatum_scene::SceneCell::grapheme_text)
                .collect::<String>()
        })
        .collect()
}

/// Render the same scene through the ratatui adapter into a test buffer.
fn render_scene_to_ratatui(scene: &WorkspaceScene) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(scene.size.width, scene.size.height)).unwrap();
    terminal
        .draw(|frame| mandatum_renderer::render(frame, scene, &Theme::default()))
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

/// One isolated directory per test: a fixed temp path would grow a real
/// timeline file across runs and let concurrent test runs interfere.
fn isolated_project_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&path).expect("test temp dir should be created");
    path
}

#[test]
fn same_scene_renders_equivalent_content_in_both_frontends() {
    let project_path = isolated_project_dir("mandatum-frontend-parity-test");
    let mut state = AppState::new(AppConfig {
        workspace_file: project_path.join("workspace.json"),
        project_path,
        task_command: "printf TASK_OK".to_owned(),
        agent_objective: "test objective".to_owned(),
        spawn_pty: true,
        ..AppConfig::default()
    });
    state.handle_terminal_resize(100, 30);
    state.handle_event(InputEvent::Paste("echo PARITY_MARKER\r".to_owned()));

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
    // The scene-carried strips paint identically from the scene alone.
    let header = scene.header.text.trim();
    assert!(!header.is_empty(), "the header strip is never blank");
    for essential in ["PARITY_MARKER", header, scene.status.text.as_str()] {
        assert!(text.contains(essential), "text frontend lost {essential:?}");
        assert!(
            ratatui.contains(essential),
            "ratatui frontend lost {essential:?}"
        );
    }

    state.shutdown();
}

/// The attention strip renders through both frontends from the same scene:
/// a waiting approval becomes a header segment a stranger can read (and a
/// pointer can click) in either frontend. The built-in fake connector
/// script requests an approval and waits, so no bespoke wiring is needed.
#[test]
fn attention_header_reaches_both_frontends() {
    use mandatum_commands::CommandId;

    let project_path = isolated_project_dir("mandatum-frontend-parity-attention");
    let mut state = AppState::new(AppConfig {
        workspace_file: project_path.join("workspace.json"),
        project_path,
        agent_objective: "test objective".to_owned(),
        ..AppConfig::default()
    });
    state.dispatch(CommandId::StartAgent);

    let size = SceneSize::new(100, 30);
    let mut waiting = false;
    for _ in 0..300 {
        state.tick_runtime();
        let scene = build_workspace_scene(&state, size);
        if !scene.header.attention.is_empty() {
            waiting = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(waiting, "the approval never reached the attention strip");

    let scene = build_workspace_scene(&state, size);
    let segment = scene.header.attention.first().unwrap();
    assert!(segment.label.contains("approval waiting"));

    let text = render_scene_to_text(&scene).join("\n");
    let ratatui = render_scene_to_ratatui(&scene).join("\n");
    for output in [&text, &ratatui] {
        assert!(
            output.contains(&segment.label),
            "a frontend lost the attention segment {:?}",
            segment.label
        );
    }

    state.shutdown();
}
