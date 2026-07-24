use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use mandatum_app::{AppConfig, FrameSnapshot, FrontendEffect, FrontendHost};
use mandatum_gpu_renderer_spike::{PreparedScene, prepare_scene};
use mandatum_scene::{
    ArtifactState, CellOccupancy, CellSelection, HitTargetKind, OverlayScene, PaneContent,
    SceneRect, SceneSize, WorkspaceScene,
    input::{InputEvent, Key, KeyCode, Modifiers, PointerButton, PointerEvent, PointerKind},
    layout,
};

fn dispatch_palette_command(host: &mut FrontendHost, key: char) {
    host.handle_input(InputEvent::Key(Key::ctrl('p')));
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char(key))));
}

fn wait_for_frame(
    host: &mut FrontendHost,
    wake_rx: &mpsc::Receiver<()>,
    size: SceneSize,
    predicate: impl Fn(&FrameSnapshot) -> bool,
) -> FrameSnapshot {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if wake_rx.recv_timeout(Duration::from_millis(20)).is_ok() {
            while host.drain_runtime() > 0 {}
        }
        host.heartbeat();
        let snapshot = host.frame(size);
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "real host did not reach the expected frame before the deadline"
        );
    }
}

fn terminal_pane(
    snapshot: &FrameSnapshot,
) -> (&mandatum_scene::PaneScene, &mandatum_scene::TerminalSurface) {
    snapshot
        .scene
        .panes
        .iter()
        .find_map(|pane| match &pane.content {
            PaneContent::Terminal(surface) => Some((pane, surface)),
            _ => None,
        })
        .expect("real host frame did not contain a terminal pane")
}

fn assert_scene_reaches_cell_program(prepared: &PreparedScene, scene: &WorkspaceScene) {
    let program = prepared.cell_program();
    assert_eq!(program.size(), scene.size);
    for pane in &scene.panes {
        assert!(
            program.cell_at(pane.area.x, pane.area.y).is_some(),
            "pane {} did not reach the neutral cell program",
            pane.id
        );
    }
    if let Some(overlay) = &scene.overlay {
        let area = match overlay {
            OverlayScene::Palette(overlay) => overlay.area,
            OverlayScene::ContextMenu(overlay) => overlay.area,
            OverlayScene::Timeline(overlay) => overlay.area,
            OverlayScene::SessionMap(overlay) => overlay.area,
            OverlayScene::Prompt(overlay) => overlay.area,
            OverlayScene::Search(overlay) => overlay.area,
            OverlayScene::Help(overlay) => overlay.area,
            OverlayScene::Welcome(overlay) => overlay.area,
        };
        assert!(
            program.cell_at(area.x, area.y).is_some(),
            "overlay did not reach the neutral cell program"
        );
    }
}

fn cell_program_text(prepared: &PreparedScene, area: SceneRect) -> String {
    let program = prepared.cell_program();
    let mut text = String::new();
    for y in area.y..area.bottom().min(program.size().height) {
        for x in area.x..area.right().min(program.size().width) {
            text.push(match program.cell_at(x, y).map(|cell| cell.occupancy) {
                Some(CellOccupancy::Glyph(character)) => character,
                Some(CellOccupancy::WideContinuation) | None => ' ',
            });
        }
        text.push('\n');
    }
    text
}

fn assert_cell_program_contains(prepared: &PreparedScene, area: SceneRect, expected: &str) {
    let text = cell_program_text(prepared, area);
    assert!(
        text.contains(expected),
        "final cell program did not contain {expected:?} in {area:?}; got:\n{text}"
    );
}

fn assert_cell_program_has_item_selection(prepared: &PreparedScene, area: SceneRect) {
    let program = prepared.cell_program();
    assert!(
        (area.y..area.bottom()).any(|y| {
            (area.x..area.right()).any(|x| {
                program
                    .cell_at(x, y)
                    .is_some_and(|cell| cell.selection == Some(CellSelection::Item))
            })
        }),
        "final cell program did not retain selected-item style in {area:?}"
    );
}

fn assert_cell_program_has_cursor(prepared: &PreparedScene, area: SceneRect) {
    let program = prepared.cell_program();
    assert!(
        (area.y..area.bottom()).any(|y| {
            (area.x..area.right()).any(|x| program.cell_at(x, y).is_some_and(|cell| cell.cursor))
        }),
        "final cell program did not retain cursor state in {area:?}"
    );
}

fn assert_cell_program_has_bold_style(prepared: &PreparedScene, area: SceneRect) {
    let program = prepared.cell_program();
    assert!(
        (area.y..area.bottom()).any(|y| {
            (area.x..area.right())
                .any(|x| program.cell_at(x, y).is_some_and(|cell| cell.style.bold))
        }),
        "final cell program did not retain bold style in {area:?}"
    );
}

fn assert_focused_pane_title_style(prepared: &PreparedScene, pane: &mandatum_scene::PaneScene) {
    let title_cell = prepared
        .cell_program()
        .cell_at(pane.area.x.saturating_add(1), pane.area.y)
        .expect("focused pane title did not reach the final cell program");
    assert!(
        title_cell.style.bold,
        "focused pane title lost its compiled bold style"
    );
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

fn write_test_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("artifact fixture should be writable");
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("artifact fixture header")
        .write_image_data(rgba)
        .expect("artifact fixture pixels");
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    let pane = &snapshot.scene.panes[0];
    assert_cell_program_contains(
        &prepared,
        layout::pane_inner_rect(pane.area),
        "no live PTY grid is attached",
    );
    assert_focused_pane_title_style(&prepared, pane);
}

#[test]
fn real_host_copy_mode_compiles_selection_and_cursor_into_the_gpu_cell_program() {
    let project = DisposableProject::new("copy-mode-cell-program");
    let mut host = FrontendHost::new(AppConfig {
        project_path: project.path.clone(),
        workspace_file: project.path.join("workspace.json"),
        spawn_pty: true,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, '[');
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char('v'))));
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Right)));
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Right)));

    let snapshot = host.frame(frame_size);
    assert!(snapshot.scene.copy_mode);
    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real copy-mode scene");
    let cells = prepared
        .cell_program()
        .cells()
        .map(|(_, _, cell)| cell)
        .collect::<Vec<_>>();

    assert!(
        cells
            .iter()
            .any(|cell| cell.selection == Some(CellSelection::Terminal)),
        "real host selection did not reach the neutral GPU cell program"
    );
    assert!(
        cells.iter().any(|cell| cell.cursor),
        "real host copy cursor did not reach the neutral GPU cell program"
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
}

#[test]
fn real_host_three_tiled_empty_panes_reach_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, 'v');
    dispatch_palette_command(&mut host, 'v');

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.header.pane_count, 3);
    assert_eq!(snapshot.scene.panes.len(), 3);
    assert_eq!(snapshot.scene.panes[0].area, SceneRect::new(0, 1, 40, 22));
    assert_eq!(snapshot.scene.panes[1].area, SceneRect::new(40, 1, 20, 22));
    assert_eq!(snapshot.scene.panes[2].area, SceneRect::new(60, 1, 20, 22));
    assert!(snapshot.scene.panes[2].focused);
    assert!(
        snapshot
            .scene
            .panes
            .iter()
            .all(|pane| matches!(pane.content, PaneContent::Empty(_)))
    );

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare three scene-resolved tiled panes");
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
}

#[test]
fn real_host_two_ordered_floats_reach_the_gpu_render_plan() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, 'n');
    dispatch_palette_command(&mut host, 'n');

    let snapshot = host.frame(frame_size);
    assert_eq!(snapshot.scene.header.pane_count, 3);
    assert_eq!(snapshot.scene.panes.len(), 3);
    assert_eq!(snapshot.scene.panes[0].area, SceneRect::new(0, 1, 80, 22));
    assert!(!snapshot.scene.panes[0].floating);
    for pane in &snapshot.scene.panes[1..] {
        assert_eq!(pane.area, SceneRect::new(8, 5, 72, 18));
        assert!(pane.floating);
    }
    assert!(!snapshot.scene.panes[1].focused);
    assert!(snapshot.scene.panes[2].focused);

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real ordered-float scene");
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
}

#[test]
fn real_host_horizontal_resize_enforces_usable_pane_interiors_with_palette() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    dispatch_palette_command(&mut host, 'v');

    let minimum = SceneSize::new(6, 5);
    host.handle_input(InputEvent::Resize(minimum));
    host.handle_input(InputEvent::Key(Key::ctrl('p')));
    let minimum_snapshot = host.frame(minimum);
    assert_eq!(
        minimum_snapshot.scene.panes[0].area,
        SceneRect::new(0, 1, 3, 3)
    );
    assert_eq!(
        minimum_snapshot.scene.panes[1].area,
        SceneRect::new(3, 1, 3, 3)
    );
    assert!(matches!(
        minimum_snapshot.scene.overlay,
        Some(OverlayScene::Palette(_))
    ));
    prepare_scene(&minimum_snapshot.scene, &minimum_snapshot.theme)
        .expect("GPU renderer rejected the minimum usable horizontal Palette scene");

    for below_minimum in [SceneSize::new(5, 5), SceneSize::new(6, 4)] {
        host.handle_input(InputEvent::Resize(below_minimum));
        let below_snapshot = host.frame(below_minimum);
        assert!(
            prepare_scene(&below_snapshot.scene, &below_snapshot.theme).is_err(),
            "GPU renderer admitted an unusable horizontal Palette scene at {below_minimum:?}"
        );
    }
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
}

#[test]
fn real_host_vertical_resize_enforces_usable_pane_interiors() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    dispatch_palette_command(&mut host, 's');

    let minimum = SceneSize::new(3, 8);
    host.handle_input(InputEvent::Resize(minimum));
    let minimum_snapshot = host.frame(minimum);
    assert_eq!(
        minimum_snapshot.scene.panes[0].area,
        SceneRect::new(0, 1, 3, 3)
    );
    assert_eq!(
        minimum_snapshot.scene.panes[1].area,
        SceneRect::new(0, 4, 3, 3)
    );
    prepare_scene(&minimum_snapshot.scene, &minimum_snapshot.theme)
        .expect("GPU renderer rejected the minimum usable vertical scene");

    for below_minimum in [SceneSize::new(2, 8), SceneSize::new(3, 7)] {
        host.handle_input(InputEvent::Resize(below_minimum));
        let below_snapshot = host.frame(below_minimum);
        assert!(
            prepare_scene(&below_snapshot.scene, &below_snapshot.theme).is_err(),
            "GPU renderer admitted an unusable vertical scene at {below_minimum:?}"
        );
    }
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
}

#[test]
fn real_host_default_float_resize_preserves_usable_pane_interiors() {
    let mut host = FrontendHost::new(AppConfig {
        spawn_pty: false,
        ..AppConfig::default()
    });
    dispatch_palette_command(&mut host, 'v');
    dispatch_palette_command(&mut host, 'f');

    let minimum = SceneSize::new(11, 9);
    host.handle_input(InputEvent::Resize(minimum));
    let minimum_snapshot = host.frame(minimum);
    assert_eq!(
        minimum_snapshot.scene.panes[0].area,
        SceneRect::new(0, 1, 11, 7)
    );
    assert_eq!(
        minimum_snapshot.scene.panes[1].area,
        SceneRect::new(8, 5, 3, 3)
    );
    prepare_scene(&minimum_snapshot.scene, &minimum_snapshot.theme)
        .expect("GPU renderer rejected the minimum usable default float");

    for below_minimum in [SceneSize::new(10, 9), SceneSize::new(11, 8)] {
        host.handle_input(InputEvent::Resize(below_minimum));
        let below_snapshot = host.frame(below_minimum);
        assert!(
            below_snapshot
                .scene
                .panes
                .iter()
                .all(|pane| pane.area.width >= 3 && pane.area.height >= 3)
        );
        prepare_scene(&below_snapshot.scene, &below_snapshot.theme)
            .expect("GPU renderer rejected a float normalized after shrink");
    }
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
    let Some(OverlayScene::Palette(palette)) = &snapshot.scene.overlay else {
        panic!("command transition did not produce the real Palette frame");
    };
    assert_eq!(snapshot.scene.panes[0].area, SceneRect::new(0, 1, 40, 22));
    assert_eq!(snapshot.scene.panes[1].area, SceneRect::new(40, 1, 40, 22));

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the float command's intermediate Palette frame");
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    let selected = palette.selected.expect("real Palette had no selected item");
    assert_cell_program_contains(&prepared, palette.area, &palette.items[selected].label);
    assert_cell_program_has_item_selection(&prepared, palette.area);
}

#[test]
fn real_host_two_horizontal_empty_palette_clips_long_wrapped_pane_detail() {
    let project = DisposableProject::new("palette-occlusion");
    let long_project_path = project
        .path
        .join("wrapped-project-segment".repeat(5))
        .join("more-wrapped-detail".repeat(5));
    std::fs::create_dir_all(&long_project_path)
        .expect("long disposable project path should be writable");
    let mut host = FrontendHost::new(AppConfig {
        project_path: long_project_path.clone(),
        workspace_file: long_project_path.join(".mandatum").join("workspace.json"),
        spawn_pty: false,
        ..AppConfig::default()
    });
    let frame_size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(frame_size));
    dispatch_palette_command(&mut host, 'v');
    host.handle_input(InputEvent::Key(Key::ctrl('p')));

    let snapshot = host.frame(frame_size);
    let Some(OverlayScene::Palette(palette)) = &snapshot.scene.overlay else {
        panic!("Float command transition did not produce the real Palette frame");
    };
    assert_eq!(snapshot.scene.panes.len(), 2);
    assert_eq!(snapshot.scene.panes[0].area, SceneRect::new(0, 1, 40, 22));
    assert_eq!(snapshot.scene.panes[1].area, SceneRect::new(40, 1, 40, 22));
    assert_eq!(palette.area, layout::palette_overlay_rect(frame_size));

    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real two-pane Palette frame");
    let first_inner = layout::pane_inner_rect(snapshot.scene.panes[0].area);
    assert!(
        snapshot.scene.panes[0]
            .detail_lines()
            .iter()
            .map(|line| line.chars().count())
            .sum::<usize>()
            > usize::from(first_inner.width) * 4,
        "real Empty detail was not long enough to wrap through the Palette area"
    );
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    let selected = palette.selected.expect("real Palette had no selected item");
    assert_cell_program_contains(&prepared, palette.area, &palette.items[selected].label);
    assert_cell_program_has_item_selection(&prepared, palette.area);
}

#[test]
fn real_host_two_pane_stack_reaches_the_gpu_render_plan() {
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
    let prepared = prepare_scene(&snapshot.scene, &snapshot.theme)
        .expect("GPU renderer did not prepare the real stacked pane");
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    assert_cell_program_contains(&prepared, menu.area, &menu.items[menu.selected].label);
    assert_cell_program_has_item_selection(&prepared, menu.area);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    let selected = timeline
        .selected
        .expect("real timeline had no selected item");
    assert_cell_program_contains(&prepared, timeline.area, &timeline.items[selected].text);
    assert_cell_program_has_item_selection(&prepared, timeline.area);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    assert_cell_program_contains(&prepared, map.area, &map.rows[map.selected].label);
    assert_cell_program_has_item_selection(&prepared, map.area);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    assert_cell_program_contains(&prepared, prompt.area, &prompt.input);
    assert_cell_program_has_cursor(&prepared, prompt.area);
}

#[test]
fn real_host_artifact_load_and_reload_reach_the_gpu_render_plan() {
    let project = DisposableProject::new("artifact");
    let artifact_path = project.path.join("preview.png");
    write_test_png(&artifact_path, 2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]);
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let mut host = FrontendHost::new_with_wake_callback(
        AppConfig {
            project_path: project.path.clone(),
            workspace_file: project.path.join(".mandatum/workspace.json"),
            spawn_pty: false,
            ..AppConfig::default()
        },
        move || {
            let _ = wake_tx.try_send(());
        },
    );
    let size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(size));
    host.handle_input(InputEvent::Key(Key::ctrl('p')));
    host.handle_input(InputEvent::Key(Key::new(
        KeyCode::Char('o'),
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    )));
    for character in "pen artifact preview".chars() {
        host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char(character))));
    }
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Enter)));
    host.handle_input(InputEvent::Paste("preview.png".to_owned()));
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Enter)));

    let loaded = wait_for_frame(&mut host, &wake_rx, size, |snapshot| {
        snapshot.scene.panes.iter().any(|pane| {
            matches!(
                &pane.content,
                PaneContent::Artifact(content)
                    if matches!(content.state, ArtifactState::Ready(_))
            )
        })
    });
    let first = loaded
        .scene
        .panes
        .iter()
        .find_map(|pane| match &pane.content {
            PaneContent::Artifact(content) => match &content.state {
                ArtifactState::Ready(surface) => Some(surface),
                _ => None,
            },
            _ => None,
        })
        .expect("loaded artifact should be ready");
    assert_eq!((first.width, first.height), (2, 1));
    assert_eq!(first.rgba8.as_ref(), &[255, 0, 0, 255, 0, 0, 255, 255]);
    let first_revision = first.revision;
    let prepared = prepare_scene(&loaded.scene, &loaded.theme)
        .expect("real loaded artifact should reach GPU preparation");
    assert_eq!(prepared.artifacts().len(), 1);

    write_test_png(&artifact_path, 1, 2, &[0, 255, 0, 255, 255, 255, 0, 255]);
    dispatch_palette_command(&mut host, 'r');
    let reloaded = wait_for_frame(&mut host, &wake_rx, size, |snapshot| {
        snapshot.scene.panes.iter().any(|pane| {
            matches!(
                &pane.content,
                PaneContent::Artifact(content)
                    if matches!(
                        &content.state,
                        ArtifactState::Ready(surface)
                            if surface.width == 1
                                && surface.height == 2
                                && surface.revision > first_revision
                    )
            )
        })
    });
    let prepared = prepare_scene(&reloaded.scene, &reloaded.theme)
        .expect("real reloaded artifact should reach GPU preparation");
    assert_eq!(prepared.artifacts().len(), 1);
    assert_eq!(
        (
            prepared.artifacts()[0].width(),
            prepared.artifacts()[0].height()
        ),
        (1, 2)
    );
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    assert_cell_program_contains(&prepared, search.area, &search.query);
    assert_cell_program_contains(&prepared, search.area, &search.items[0].source);
    assert_cell_program_has_item_selection(&prepared, search.area);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    assert_cell_program_contains(&prepared, help.area, &help.query);
    assert_cell_program_contains(&prepared, help.area, &help.items[0].label);
    assert_cell_program_has_item_selection(&prepared, help.area);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    assert_cell_program_contains(&prepared, welcome.area, &welcome.introduction);
    assert_cell_program_contains(&prepared, welcome.area, "Command palette");
    assert_cell_program_has_bold_style(&prepared, welcome.area);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);

    host.handle_input(InputEvent::Key(Key::ctrl('p')));
    let palette_snapshot = host.frame(frame_size);
    let Some(OverlayScene::Palette(palette)) = &palette_snapshot.scene.overlay else {
        panic!("Ctrl+P did not produce the real Palette frame");
    };
    let prepared_palette = prepare_scene(&palette_snapshot.scene, &palette_snapshot.theme)
        .expect("GPU renderer did not prepare the real command palette");
    assert_scene_reaches_cell_program(&prepared_palette, &palette_snapshot.scene);
    let selected = palette.selected.expect("real Palette had no selected item");
    assert_cell_program_contains(
        &prepared_palette,
        palette.area,
        &palette.items[selected].label,
    );
    assert_cell_program_has_item_selection(&prepared_palette, palette.area);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    let pane = snapshot
        .scene
        .panes
        .iter()
        .find(|pane| matches!(pane.content, PaneContent::Task(_)))
        .expect("real task pane disappeared before cell-program assertions");
    let body = layout::pane_inner_rect(pane.area);
    assert_cell_program_contains(&prepared, body, "command: printf TASK_PLAN_OK");
    let output_row = body.y.saturating_add(pane.detail_lines().len() as u16);
    assert_cell_program_contains(
        &prepared,
        SceneRect::new(body.x, output_row, body.width, 1),
        "TASK_PLAN_OK",
    );
    assert_focused_pane_title_style(&prepared, pane);
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
    assert_scene_reaches_cell_program(&prepared, &snapshot.scene);
    let pane = snapshot
        .scene
        .panes
        .iter()
        .find(|pane| matches!(pane.content, PaneContent::Agent(_)))
        .expect("real agent pane disappeared before cell-program assertions");
    assert_cell_program_contains(
        &prepared,
        layout::pane_inner_rect(pane.area),
        "objective: Inspect AGENT_PLAN_OK",
    );
    assert_focused_pane_title_style(&prepared, pane);
}

#[test]
fn real_host_pointer_selection_copy_and_wheel_scrollback_reach_the_gpu_boundary() {
    let project = DisposableProject::new("pointer-lifecycle");
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let mut host = FrontendHost::new_with_wake_callback(
        AppConfig {
            project_path: project.path.clone(),
            workspace_file: project.path.join("workspace.json"),
            shell_program: "/bin/sh".to_owned(),
            spawn_pty: true,
            ..AppConfig::default()
        },
        move || {
            let _ = wake_tx.try_send(());
        },
    );
    let size = SceneSize::new(80, 24);
    host.handle_input(InputEvent::Resize(size));
    host.handle_input(InputEvent::Paste("echo SELECT_ME\r".to_owned()));

    let snapshot = wait_for_frame(&mut host, &wake_rx, size, |snapshot| {
        let (_, surface) = terminal_pane(snapshot);
        surface.rows.iter().any(|row| {
            let text = row.iter().map(|cell| cell.character).collect::<String>();
            text.trim_end().ends_with("SELECT_ME") && !text.contains("echo")
        })
    });
    let (pane, surface) = terminal_pane(&snapshot);
    let inner = layout::pane_inner_rect(pane.area);
    let (row, text) = surface
        .rows
        .iter()
        .enumerate()
        .find_map(|(row, cells)| {
            let text = cells.iter().map(|cell| cell.character).collect::<String>();
            (text.trim_end().ends_with("SELECT_ME") && !text.contains("echo"))
                .then_some((row, text))
        })
        .expect("printed selection marker was not visible");
    let column = text.find("SELECT_ME").expect("selection marker column") as u16;
    let start = (inner.x + column, inner.y + row as u16);
    let pointer = |kind, column| {
        InputEvent::Pointer(PointerEvent {
            kind,
            button: Some(PointerButton::Left),
            column,
            row: start.1,
            mods: Modifiers::NONE,
        })
    };
    host.handle_input(pointer(PointerKind::Down, start.0));
    host.handle_input(pointer(PointerKind::Drag, start.0 + 8));
    host.handle_input(pointer(PointerKind::Up, start.0 + 8));

    let selected = host.frame(size);
    let prepared = prepare_scene(&selected.scene, &selected.theme)
        .expect("pointer selection did not prepare through the GPU boundary");
    assert!(
        prepared
            .cell_program()
            .cells()
            .any(|(_, _, cell)| cell.selection == Some(CellSelection::Terminal)),
        "pointer drag did not reach final selected cells"
    );

    host.copy_selection();
    assert_eq!(
        host.take_effects(),
        vec![FrontendEffect::SetClipboard("SELECT_ME".to_owned())]
    );
    assert!(host.take_effects().is_empty());

    host.handle_input(InputEvent::Paste(
        "i=1; while [ $i -le 80 ]; do echo LINE_$i; i=$((i+1)); done\r".to_owned(),
    ));
    let scrolled_ready = wait_for_frame(&mut host, &wake_rx, size, |snapshot| {
        terminal_pane(snapshot).1.scrollback_len > 10
    });
    let body = layout::pane_inner_rect(terminal_pane(&scrolled_ready).0.area);
    let wheel = |dy| {
        InputEvent::Pointer(PointerEvent {
            kind: PointerKind::Wheel { dx: 0, dy },
            button: None,
            column: body.x,
            row: body.y,
            mods: Modifiers::NONE,
        })
    };
    host.handle_input(wheel(-1));
    let scrolled = host.frame(size);
    assert_eq!(terminal_pane(&scrolled).1.scroll_offset, 3);
    assert!(scrolled.scene.status.text.contains("scrollback"));
    host.handle_input(wheel(1));
    let live = host.frame(size);
    assert_eq!(terminal_pane(&live).1.scroll_offset, 0);
    assert!(live.scene.status.text.contains("following live output"));

    assert!(host.shutdown());
    assert!(!host.shutdown());
}

#[test]
fn real_host_startup_restore_recreates_processes_then_resizes_and_quits_cleanly() {
    let project = DisposableProject::new("startup-restore");
    let workspace_file = project.path.join("workspace.json");
    let size = SceneSize::new(80, 24);

    let mut first = FrontendHost::new(AppConfig {
        project_path: project.path.clone(),
        workspace_file: workspace_file.clone(),
        spawn_pty: false,
        restore_on_startup: false,
        ..AppConfig::default()
    });
    first.handle_input(InputEvent::Resize(size));
    dispatch_palette_command(&mut first, 'v');
    dispatch_palette_command(&mut first, 'w');
    assert!(workspace_file.exists(), "workspace save did not reach disk");
    assert_eq!(first.frame(size).scene.panes.len(), 2);
    first.shutdown();

    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let mut restored = FrontendHost::new_with_wake_callback(
        AppConfig {
            project_path: project.path.clone(),
            workspace_file,
            shell_program: "/bin/cat".to_owned(),
            spawn_pty: true,
            restore_on_startup: true,
            ..AppConfig::default()
        },
        move || {
            let _ = wake_tx.try_send(());
        },
    );
    restored.handle_input(InputEvent::Resize(size));
    let restored_frame = restored.frame(size);
    assert_eq!(restored_frame.scene.panes.len(), 2);
    assert_eq!(
        restored_frame
            .scene
            .panes
            .iter()
            .filter(|pane| matches!(pane.content, PaneContent::Terminal(_)))
            .count(),
        2,
        "startup restore did not recreate both terminal processes"
    );
    assert!(
        restored_frame
            .scene
            .status
            .text
            .contains("workspace restored"),
        "startup restore status was lost: {}",
        restored_frame.scene.status.text
    );

    restored.handle_input(InputEvent::Paste("RESTORED_PROCESS_OK\n".to_owned()));
    let echoed = wait_for_frame(&mut restored, &wake_rx, size, |snapshot| {
        snapshot.scene.panes.iter().any(|pane| {
            let PaneContent::Terminal(surface) = &pane.content else {
                return false;
            };
            surface.rows.iter().any(|row| {
                row.iter()
                    .map(|cell| cell.character)
                    .collect::<String>()
                    .contains("RESTORED_PROCESS_OK")
            })
        })
    });
    assert_eq!(echoed.scene.panes.len(), 2);
    assert_eq!(
        echoed
            .scene
            .panes
            .iter()
            .filter(|pane| matches!(pane.content, PaneContent::Terminal(_)))
            .count(),
        2
    );

    let resized = SceneSize::new(100, 30);
    restored.handle_input(InputEvent::Resize(resized));
    let resized_frame = restored.frame(resized);
    assert_eq!(resized_frame.scene.size, resized);
    assert_eq!(resized_frame.scene.panes.len(), 2);
    restored.handle_input(InputEvent::FocusLost);
    restored.handle_input(InputEvent::FocusGained);
    restored.handle_input(InputEvent::Key(Key::ctrl('q')));
    assert!(restored.should_quit());
    assert!(restored.shutdown());
    assert!(!restored.shutdown());
    assert_eq!(restored.drain_runtime(), 0);
}
