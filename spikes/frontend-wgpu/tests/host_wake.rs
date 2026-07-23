use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use mandatum_app::{AppConfig, FrontendHost};
use mandatum_gpu_renderer_spike::prepare_scene;
use mandatum_scene::{
    HitTargetKind, OverlayScene, PaneContent, SceneSize,
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
