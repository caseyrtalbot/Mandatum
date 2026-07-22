use std::{sync::mpsc, time::Duration};

use mandatum_app::{AppConfig, FrontendHost};
use mandatum_gpu_renderer_spike::prepare_scene;
use mandatum_scene::{
    PaneContent, SceneSize,
    input::{InputEvent, Key, KeyCode},
};

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
