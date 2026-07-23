use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use mandatum_app::{AppConfig, FrontendHost};
use mandatum_gpu_renderer_spike::{UnsupportedScene, prepare_scene};
use mandatum_scene::{
    HitTargetKind, OverlayScene, PaneContent, SceneRect, SceneSize,
    input::{InputEvent, Key, KeyCode, Modifiers, PointerButton, PointerEvent, PointerKind},
    layout,
};

fn dispatch_palette_command(host: &mut FrontendHost, key: char) {
    host.handle_input(InputEvent::Key(Key::ctrl('p')));
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char(key))));
}

struct DisposableProject {
    path: std::path::PathBuf,
}

impl DisposableProject {
    fn new(label: &str) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mandatum-gpu-host-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("disposable project directory should be writable");
        Self { path }
    }
}

impl Drop for DisposableProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn real_host_empty_pane_reaches_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    let snapshot = host.frame(frame_size);
    let empty = snapshot
        .scene
        .panes
        .iter()
        .find_map(|pane| match &pane.content {
            PaneContent::Empty(empty) => Some(empty),
            _ => None,
        })
        .expect("fresh real host frame did not contain an empty pane");
    assert_eq!(empty.restart_generation, 0);

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real empty pane");
    assert!(
        prepared
            .pane_text()
            .contains("no live PTY grid is attached"),
        "prepared empty plan did not retain the real scene display data"
    );
}

#[test]
fn real_host_two_horizontal_empty_panes_reach_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, 'v');

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.panes.len(), 2);
    let first = &snapshot.scene.panes[0];
    let second = &snapshot.scene.panes[1];
    assert_eq!(first.id.as_str(), "pane-1");
    assert_eq!(first.title, "terminal");
    assert_eq!(first.area, SceneRect::new(0, 1, 40, 22));
    assert!(!first.focused);
    assert!(!first.floating);
    assert!(!first.stacked);
    assert!(!first.zoomed);
    let first_empty = match &first.content {
        PaneContent::Empty(empty) => empty,
        other => panic!("expected first Empty pane, got {other:?}"),
    };
    let first_details = [
        format!("cwd: {}", first_empty.cwd_label),
        format!("restart generation: {}", first_empty.restart_generation),
        "no live PTY grid is attached to this pane".to_owned(),
    ];
    assert_eq!(first.detail_lines(), first_details);

    assert_eq!(second.id.as_str(), "pane-2");
    assert_eq!(second.title, "terminal 2");
    assert_eq!(second.area, SceneRect::new(40, 1, 40, 22));
    assert!(second.focused);
    assert!(!second.floating);
    assert!(!second.stacked);
    assert!(!second.zoomed);
    let second_empty = match &second.content {
        PaneContent::Empty(empty) => empty,
        other => panic!("expected second Empty pane, got {other:?}"),
    };
    let second_details = [
        format!("cwd: {}", second_empty.cwd_label),
        format!("restart generation: {}", second_empty.restart_generation),
        "no live PTY grid is attached to this pane".to_owned(),
    ];
    assert_eq!(second.detail_lines(), second_details);
    assert_eq!(snapshot.scene.focused_pane, second.id);

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare two horizontal Empty panes");
    assert_eq!(prepared.panes().len(), 2);
    assert_eq!(prepared.panes()[0].scene(), first);
    assert_eq!(prepared.panes()[1].scene(), second);
    for detail in &first_details {
        assert!(prepared.panes()[0].pane_text().contains(detail));
    }
    for detail in &second_details {
        assert!(prepared.panes()[1].pane_text().contains(detail));
    }
    assert!(
        prepared
            .panes()
            .iter()
            .all(|pane| pane.pane_surface().is_none())
    );
}

#[test]
fn real_host_two_vertical_empty_panes_reach_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, 's');

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.panes.len(), 2);
    let first = &snapshot.scene.panes[0];
    let second = &snapshot.scene.panes[1];
    assert_eq!(first.id.as_str(), "pane-1");
    assert_eq!(first.title, "terminal");
    assert_eq!(first.area, SceneRect::new(0, 1, 80, 11));
    assert!(!first.focused);
    assert!(!first.floating);
    assert!(!first.stacked);
    assert!(!first.zoomed);
    let first_empty = match &first.content {
        PaneContent::Empty(empty) => empty,
        other => panic!("expected first Empty pane, got {other:?}"),
    };
    let first_details = [
        format!("cwd: {}", first_empty.cwd_label),
        format!("restart generation: {}", first_empty.restart_generation),
        "no live PTY grid is attached to this pane".to_owned(),
    ];
    assert_eq!(first.detail_lines(), first_details);

    assert_eq!(second.id.as_str(), "pane-2");
    assert_eq!(second.title, "terminal 2");
    assert_eq!(second.area, SceneRect::new(0, 12, 80, 11));
    assert!(second.focused);
    assert!(!second.floating);
    assert!(!second.stacked);
    assert!(!second.zoomed);
    let second_empty = match &second.content {
        PaneContent::Empty(empty) => empty,
        other => panic!("expected second Empty pane, got {other:?}"),
    };
    let second_details = [
        format!("cwd: {}", second_empty.cwd_label),
        format!("restart generation: {}", second_empty.restart_generation),
        "no live PTY grid is attached to this pane".to_owned(),
    ];
    assert_eq!(second.detail_lines(), second_details);
    assert_eq!(snapshot.scene.focused_pane, second.id);

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare two vertical Empty panes");
    assert_eq!(prepared.panes().len(), 2);
    assert_eq!(prepared.panes()[0].scene(), first);
    assert_eq!(prepared.panes()[1].scene(), second);
    for detail in &first_details {
        assert!(prepared.panes()[0].pane_text().contains(detail));
    }
    for detail in &second_details {
        assert!(prepared.panes()[1].pane_text().contains(detail));
    }
    assert!(
        prepared
            .panes()
            .iter()
            .all(|pane| pane.pane_surface().is_none())
    );
}

#[test]
fn real_host_two_pane_floating_empty_layout_reaches_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, 'v');
    dispatch_palette_command(&mut host, 'f');

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.panes.len(), 2);
    let first = &snapshot.scene.panes[0];
    let second = &snapshot.scene.panes[1];
    assert_eq!(first.id.as_str(), "pane-1");
    assert_eq!(first.title, "terminal");
    assert_eq!(first.area, SceneRect::new(0, 1, 80, 22));
    assert!(!first.focused);
    assert!(!first.floating);
    assert!(!first.stacked);
    assert!(!first.zoomed);
    let first_empty = match &first.content {
        PaneContent::Empty(empty) => empty,
        other => panic!("expected first Empty pane, got {other:?}"),
    };
    let first_details = [
        format!("cwd: {}", first_empty.cwd_label),
        format!("restart generation: {}", first_empty.restart_generation),
        "no live PTY grid is attached to this pane".to_owned(),
    ];
    assert_eq!(first.detail_lines(), first_details);

    assert_eq!(second.id.as_str(), "pane-2");
    assert_eq!(second.title, "terminal 2");
    assert_eq!(second.area, SceneRect::new(8, 5, 72, 18));
    assert!(second.focused);
    assert!(second.floating);
    assert!(!second.stacked);
    assert!(!second.zoomed);
    let second_empty = match &second.content {
        PaneContent::Empty(empty) => empty,
        other => panic!("expected second Empty pane, got {other:?}"),
    };
    let second_details = [
        format!("cwd: {}", second_empty.cwd_label),
        format!("restart generation: {}", second_empty.restart_generation),
        "no live PTY grid is attached to this pane".to_owned(),
    ];
    assert_eq!(second.detail_lines(), second_details);
    assert_eq!(snapshot.scene.focused_pane, second.id);

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the two-pane floating Empty layout");
    assert_eq!(prepared.panes().len(), 2);
    assert_eq!(prepared.panes()[0].scene(), first);
    assert_eq!(prepared.panes()[1].scene(), second);
    for detail in &first_details {
        assert!(prepared.panes()[0].pane_text().contains(detail));
    }
    for detail in &second_details {
        assert!(prepared.panes()[1].pane_text().contains(detail));
    }
    assert!(
        prepared
            .panes()
            .iter()
            .all(|pane| pane.pane_surface().is_none())
    );
}

#[test]
fn real_host_two_horizontal_empty_panes_with_float_palette_reach_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, 'v');
    host.handle_input(InputEvent::Key(Key::ctrl('p')));

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.panes.len(), 2);
    assert!(matches!(
        snapshot.scene.overlay,
        Some(OverlayScene::Palette(_))
    ));
    assert_eq!(snapshot.scene.panes[0].area, SceneRect::new(0, 1, 40, 22));
    assert_eq!(snapshot.scene.panes[1].area, SceneRect::new(40, 1, 40, 22));

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the float command's intermediate Palette frame");
    assert_eq!(prepared.panes().len(), 2);
    assert!(prepared.has_palette());
}

#[test]
fn real_host_two_pane_stack_remains_unsupported_by_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, 'v');
    dispatch_palette_command(&mut host, 't');

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.header.pane_count, 2);
    assert_eq!(snapshot.scene.panes.len(), 1);
    assert!(snapshot.scene.panes[0].stacked);
    assert_eq!(
        prepare_scene(&snapshot.scene, &snapshot.theme).unwrap_err(),
        UnsupportedScene::Layout("stacked panes")
    );
}

#[test]
fn real_host_context_menu_reaches_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    let initial = host.frame(frame_size);
    let pane_body = initial
        .scene
        .hit_targets
        .iter()
        .find(|target| matches!(target.kind, HitTargetKind::PaneBody(_)))
        .expect("fresh real host frame did not expose a pane-body hit target");
    host.handle_input(InputEvent::Pointer(PointerEvent {
        kind: PointerKind::Down,
        button: Some(PointerButton::Right),
        column: pane_body.rect.x,
        row: pane_body.rect.y,
        mods: Modifiers::NONE,
    }));

    let snapshot = host.frame(frame_size);
    let Some(OverlayScene::ContextMenu(menu)) = &snapshot.scene.overlay else {
        panic!("neutral right-click did not produce the real context-menu scene");
    };
    assert_eq!(menu.area.x, pane_body.rect.x);
    assert_eq!(menu.area.y, pane_body.rect.y);
    assert_eq!(menu.selected, 0);
    assert_eq!(menu.items[0].label, "Command palette");

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real context-menu scene");
    assert_eq!(prepared.context_menu(), Some(menu));
}

#[test]
fn real_host_timeline_reaches_the_gpu_render_plan() {
    let project = DisposableProject::new("timeline");
    let mut host = FrontendHost::new(AppConfig {
        project_path: project.path.clone(),
        workspace_file: project.path.join(".mandatum").join("workspace.json"),
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    dispatch_palette_command(&mut host, '/');

    let snapshot = host.frame(frame_size);
    let Some(OverlayScene::Timeline(timeline)) = &snapshot.scene.overlay else {
        panic!("Show timeline did not produce the real timeline scene");
    };
    assert_eq!(timeline.area, layout::timeline_overlay_rect(frame_size));
    assert_eq!(timeline.selected, Some(0));
    assert!(
        timeline
            .items
            .iter()
            .any(|item| item.text.contains("dispatched show-timeline")),
        "timeline did not retain the recorded Show timeline dispatch: {:?}",
        timeline.items
    );
    assert!(
        snapshot
            .scene
            .hit_targets
            .iter()
            .any(|target| matches!(target.kind, HitTargetKind::TimelineItem(0)))
    );

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real timeline scene");
    assert_eq!(prepared.timeline(), Some(timeline));
}

#[test]
fn real_host_session_map_reaches_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    dispatch_palette_command(&mut host, 'm');

    let snapshot = host.frame(frame_size);
    let Some(OverlayScene::SessionMap(map)) = &snapshot.scene.overlay else {
        panic!("Show session map did not produce the real session-map scene");
    };
    assert_eq!(map.area, layout::session_map_rect(frame_size));
    assert_eq!(map.rows.len(), 2);
    assert_eq!(map.rows[0].depth, 0);
    assert!(map.rows[0].label.contains("session-1"));
    assert!(map.rows[0].label.contains("(active)"));
    assert_eq!(map.selected, 1);
    assert_eq!(map.rows[1].depth, 1);
    assert!(map.rows[1].label.starts_with("pane-1"));
    assert!(map.rows[1].focused);
    assert!(
        snapshot
            .scene
            .hit_targets
            .iter()
            .any(|target| { matches!(target.kind, HitTargetKind::SessionMapRow(1)) })
    );

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real session-map scene");
    assert_eq!(prepared.session_map(), Some(map));
}

#[test]
fn real_host_objective_prompt_reaches_the_gpu_render_plan() {
    let configured_objective = "Inspect OBJECTIVE_PROMPT_PLAN_OK";
    let mut host = FrontendHost::new(AppConfig {
        agent_objective: configured_objective.to_owned(),
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    dispatch_palette_command(&mut host, 'a');
    dispatch_palette_command(&mut host, 'z');
    dispatch_palette_command(&mut host, 'p');

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.panes.len(), 1);
    let agent_pane = &snapshot.scene.panes[0];
    assert!(agent_pane.focused);
    assert!(agent_pane.zoomed);
    assert!(matches!(agent_pane.content, PaneContent::Agent(_)));

    let Some(OverlayScene::Prompt(prompt)) = &snapshot.scene.overlay else {
        panic!("Set agent objective did not produce the real prompt scene");
    };
    assert_eq!(prompt.area, layout::prompt_rect(frame_size));
    assert!(prompt.title.contains(agent_pane.id.as_str()));
    assert_eq!(prompt.input, configured_objective);
    assert_eq!(prompt.footer, "enter save · esc cancel");

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real objective-prompt scene");
    assert_eq!(prepared.prompt(), Some(prompt));
}

#[test]
fn real_host_search_reaches_the_gpu_render_plan() {
    let project = DisposableProject::new("search");
    let configured_objective = "Inspect SEARCH_PLAN_AGENT_OK";
    let mut host = FrontendHost::new(AppConfig {
        project_path: project.path.clone(),
        workspace_file: project.path.join(".mandatum").join("workspace.json"),
        agent_objective: configured_objective.to_owned(),
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    dispatch_palette_command(&mut host, 'a');
    dispatch_palette_command(&mut host, 'z');
    host.handle_input(InputEvent::Key(Key::new(
        KeyCode::Char('f'),
        Modifiers {
            control: true,
            shift: true,
            ..Modifiers::NONE
        },
    )));
    for character in "kind:timeline search".chars() {
        host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char(character))));
    }

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.panes.len(), 1);
    let agent_pane = &snapshot.scene.panes[0];
    assert!(agent_pane.focused);
    assert!(agent_pane.zoomed);
    let PaneContent::Agent(agent) = &agent_pane.content else {
        panic!("search did not retain the real zoomed agent pane");
    };
    assert_eq!(agent.objective, configured_objective);

    let Some(OverlayScene::Search(search)) = &snapshot.scene.overlay else {
        panic!("Ctrl+Shift+F did not produce the real search scene");
    };
    assert_eq!(search.area, layout::search_overlay_rect(frame_size));
    assert_eq!(search.query, "kind:timeline search");
    assert_eq!(search.selected, Some(0));
    assert_eq!(search.overflow, 0);
    assert_eq!(search.items.len(), 1);
    assert_eq!(search.items[0].source, "timeline");
    assert!(search.items[0].text.contains("dispatched search-session"));
    assert!(!search.items[0].match_indices.is_empty());
    assert_eq!(search.items[0].pane, None);
    assert!(search.footer.contains("type to search"));
    assert!(search.footer.contains("enter jump"));
    assert!(search.footer.contains("esc close"));
    let inner = layout::pane_inner_rect(search.area);
    assert!(snapshot.scene.hit_targets.iter().any(|target| {
        target.kind == HitTargetKind::SearchItem(0)
            && target.rect == SceneRect::new(inner.x, inner.y + 1, inner.width, 1)
    }));

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real search scene");
    assert_eq!(prepared.search(), Some(search));
}

#[test]
fn real_host_help_reaches_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Function(1))));
    for character in "search session output".chars() {
        host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char(character))));
    }

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.panes.len(), 1);
    assert!(matches!(
        snapshot.scene.panes[0].content,
        PaneContent::Empty(_)
    ));

    let Some(OverlayScene::Help(help)) = &snapshot.scene.overlay else {
        panic!("F1 did not produce the real help scene");
    };
    assert_eq!(help.area, layout::help_overlay_rect(frame_size));
    assert_eq!(help.query, "search session output");
    assert_eq!(help.selected, Some(0));
    assert_eq!(help.items.len(), 2);
    assert!(help.items[0].heading);
    assert_eq!(help.items[0].label, "App");
    assert_eq!(help.items[0].keys, "");
    assert!(!help.items[1].heading);
    assert_eq!(help.items[1].label, "Search session output");
    assert_eq!(help.items[1].keys, "ctrl+shift+f");
    assert_eq!(help.footer, "type to filter · ↑/↓ scroll · esc close");

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real help scene");
    assert_eq!(prepared.help(), Some(help));
}

#[test]
fn real_host_welcome_reaches_the_gpu_render_plan() {
    let project = DisposableProject::new("welcome");
    let workspace_file = project.path.join(".mandatum").join("workspace.json");
    assert!(
        !workspace_file.exists(),
        "disposable project unexpectedly contained a saved workspace"
    );
    let mut host = FrontendHost::new(AppConfig {
        project_path: project.path.clone(),
        workspace_file,
        restore_on_startup: true,
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.panes.len(), 1);
    assert!(matches!(
        snapshot.scene.panes[0].content,
        PaneContent::Empty(_)
    ));

    let Some(OverlayScene::Welcome(welcome)) = &snapshot.scene.overlay else {
        panic!("missing startup workspace did not produce the real welcome scene");
    };
    assert_eq!(
        welcome.area,
        layout::welcome_rect(frame_size, welcome.entries.len() as u16 + 4)
    );
    assert_eq!(
        welcome.introduction,
        "A workspace for terminals, tasks, and agents."
    );
    assert_eq!(
        welcome
            .entries
            .iter()
            .map(|entry| (entry.keys.as_str(), entry.description.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("ctrl+p", "Command palette — every command, searchable"),
            ("right-click", "Pane menu"),
            ("f1", "Help — keys, mouse, and glyphs"),
            ("ctrl+q", "Quit Mandatum"),
        ]
    );
    assert_eq!(welcome.dismissal, "Any key or click dismisses this note");

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real welcome scene");
    assert_eq!(prepared.welcome(), Some(welcome));
}

#[test]
fn real_host_pty_output_wakes_without_polling_and_reaches_a_frame() {
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let config = AppConfig {
        shell_program: "/bin/cat".to_owned(),
        spawn_pty: true,
        ..AppConfig::default()
    };

    let mut host = FrontendHost::new_with_wake_callback(config, move || {
        let _ = wake_tx.try_send(());
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char('G'))));

    wake_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("real PTY output did not wake the frontend");
    assert!(host.drain_runtime() > 0);

    let snapshot = host.frame(frame_size);
    let terminal = snapshot
        .scene
        .panes
        .iter()
        .find_map(|pane| match &pane.content {
            PaneContent::Terminal(surface) => Some(surface),
            _ => None,
        })
        .expect("real host frame did not contain a terminal pane");
    assert!(
        terminal
            .rows
            .iter()
            .flatten()
            .any(|cell| cell.character == 'G'),
        "real PTY output did not reach the host frame"
    );

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real host frame");
    assert_eq!(prepared.header_text(), snapshot.scene.header.text);
    assert_eq!(prepared.pane_title(), snapshot.scene.panes[0].title);
    assert_eq!(prepared.theme_name(), snapshot.theme.name);
    assert!(!prepared.has_palette());

    host.handle_input(InputEvent::Key(Key::ctrl('p')));
    let palette_snapshot = host.frame(frame_size);
    let prepared_palette = prepare_scene(&palette_snapshot.scene, &palette_snapshot.theme)
        .expect("GPU renderer did not prepare the real command palette");
    assert!(prepared_palette.has_palette());
}

#[test]
fn real_host_task_pane_reaches_the_gpu_render_plan() {
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let mut host = FrontendHost::new_with_wake_callback(
        AppConfig {
            task_command: "printf TASK_PLAN_OK".to_owned(),
            spawn_pty: true,
            ..AppConfig::default()
        },
        move || {
            let _ = wake_tx.try_send(());
        },
    );
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    dispatch_palette_command(&mut host, 'b');
    dispatch_palette_command(&mut host, 'z');

    let deadline = Instant::now() + Duration::from_secs(2);
    let snapshot = loop {
        if wake_rx.recv_timeout(Duration::from_millis(20)).is_ok() {
            host.drain_runtime();
        }
        host.heartbeat();
        let snapshot = host.frame(frame_size);
        let output_arrived = snapshot.scene.panes.iter().any(|pane| {
            let PaneContent::Task(task) = &pane.content else {
                return false;
            };
            task.output.as_ref().is_some_and(|surface| {
                surface.rows.iter().any(|row| {
                    row.iter()
                        .map(|cell| cell.character)
                        .collect::<String>()
                        .contains("TASK_PLAN_OK")
                })
            })
        });
        if output_arrived {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "real task output did not reach the host frame"
        );
    };
    let task = snapshot
        .scene
        .panes
        .iter()
        .find_map(|pane| match &pane.content {
            PaneContent::Task(task) => Some(task),
            _ => None,
        })
        .expect("real host frame did not contain a task pane");
    assert_eq!(task.command, "printf TASK_PLAN_OK");

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real task pane");
    assert!(
        prepared
            .pane_text()
            .contains("command: printf TASK_PLAN_OK"),
        "prepared task plan did not retain the real scene display data"
    );
    let output = prepared
        .pane_surface()
        .expect("prepared task plan did not retain the live output surface");
    assert!(
        output.rows.iter().any(|row| {
            row.iter()
                .map(|cell| cell.character)
                .collect::<String>()
                .contains("TASK_PLAN_OK")
        }),
        "prepared task plan did not retain the real task output"
    );
}

#[test]
fn real_host_agent_pane_reaches_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        agent_objective: "Inspect AGENT_PLAN_OK".to_owned(),
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));

    dispatch_palette_command(&mut host, 'a');
    dispatch_palette_command(&mut host, 'z');

    let snapshot = host.frame(frame_size);
    let agent = snapshot
        .scene
        .panes
        .iter()
        .find_map(|pane| match &pane.content {
            PaneContent::Agent(agent) => Some(agent),
            _ => None,
        })
        .expect("real host frame did not contain an agent pane");
    assert_eq!(agent.objective, "Inspect AGENT_PLAN_OK");

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real agent pane");
    assert!(
        prepared
            .pane_text()
            .contains("objective: Inspect AGENT_PLAN_OK"),
        "prepared agent plan did not retain the real scene display data"
    );
}
