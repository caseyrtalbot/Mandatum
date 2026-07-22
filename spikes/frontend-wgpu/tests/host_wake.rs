use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use mandatum_app::{AppConfig, FrontendHost};
use mandatum_gpu_renderer_spike::prepare_scene;
use mandatum_scene::{
    PaneContent, SceneSize,
    input::{InputEvent, Key, KeyCode},
};

fn dispatch_palette_command(host: &mut FrontendHost, key: char) {
    host.handle_input(InputEvent::Key(Key::ctrl('p')));
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char(key))));
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
