use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;
use crate::keymap::parse_chord;
use mandatum_core::CoreAction;
use mandatum_scene::input::{Modifiers, PointerButton, PointerEvent, PointerKind};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

fn state() -> AppState {
    AppState::new(test_config())
}

/// The shared test baseline: fake connector, no PTY spawning, no
/// restore, default keymap and theme (see `AppConfig::default`).
///
/// The baseline directory is unique per test-process run: a fixed
/// `/tmp/mandatum` path grew a real timeline file across runs and let
/// concurrent test runs interfere with each other.
fn test_config() -> AppConfig {
    use std::sync::OnceLock;
    static BASELINE_DIR: OnceLock<PathBuf> = OnceLock::new();
    let base = BASELINE_DIR.get_or_init(|| {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mandatum-app-baseline-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("baseline temp dir should be created");
        path
    });
    AppConfig {
        project_path: base.clone(),
        workspace_file: base.join(".mandatum").join("workspace.json"),
        task_command: "printf TASK_OK".to_owned(),
        agent_objective: "test objective".to_owned(),
        ..AppConfig::default()
    }
}

/// Neutral key-event helpers: every input test speaks the scene input
/// contract, never a platform event type.
fn key(code: KeyCode) -> Key {
    Key::plain(code)
}

fn ctrl(code: char) -> Key {
    Key::ctrl(code)
}

struct TestWorkspaceDir {
    path: PathBuf,
}

impl TestWorkspaceDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mandatum-app-test-{}-{stamp}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test temp dir should be created");
        Self { path }
    }

    fn project_path(&self) -> PathBuf {
        self.path.join("project")
    }

    fn workspace_file(&self) -> PathBuf {
        self.path.join(".mandatum").join("workspace.json")
    }

    fn app_config(&self, spawn_pty: bool, restore_on_startup: bool) -> AppConfig {
        let project_path = self.project_path();
        fs::create_dir_all(&project_path).expect("test project dir should be created");
        AppConfig {
            project_path,
            workspace_file: self.workspace_file(),
            task_command: "printf TASK_OK".to_owned(),
            agent_objective: "test objective".to_owned(),
            spawn_pty,
            restore_on_startup,
            ..AppConfig::default()
        }
    }
}

impl Drop for TestWorkspaceDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn keymap_keeps_workspace_controls_in_palette_mode() {
    assert_eq!(key_to_input(ctrl('q')), RuntimeInput::Quit);
    assert_eq!(key_to_input(ctrl('p')), RuntimeInput::TogglePalette);

    // Single-letter fast paths on an empty palette input: bound letters
    // dispatch exactly as the pre-fuzzy palette did.
    let mut state = state();
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('v')));
    assert!(!state.palette_open());
    assert_eq!(state.workspace().active_session().panes().len(), 2);
    assert!(state.status().contains("Split pane right"));

    // Ctrl+Q still quits over an open palette.
    state.handle_key(ctrl('p'));
    state.handle_key(ctrl('q'));
    assert!(state.should_quit());
}

#[test]
fn palette_fast_paths_keep_task_context_substitution() {
    let mut state = state();
    state.dispatch(CommandId::RunTask);
    assert!(state.focused_pane_is_task());

    // 'r' on a focused task pane means Rerun Task (spawning is disabled
    // in the test baseline, so the rerun path reports that).
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('r')));
    assert!(!state.palette_open());
    assert!(
        state.status().contains("rerun unavailable"),
        "{}",
        state.status()
    );

    // 'c' on a focused task pane means Stop Task — but nothing is
    // running here, so the fast path reports the same greyed reason the
    // palette row shows and stays open instead of fire-and-failing.
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('c')));
    assert!(state.palette_open());
    assert!(
        state
            .status()
            .contains("Stop task is unavailable: task is not running"),
        "{}",
        state.status()
    );
    state.handle_key(key(KeyCode::Escape));
}

#[test]
fn keymap_chord_override_changes_dispatch() {
    let mut config = test_config();
    config
        .keymap
        .bind_chord(CommandId::SplitRight, parse_chord("ctrl+shift+r").unwrap());
    let mut state = AppState::new(config);

    state.handle_key(Key::new(
        KeyCode::Char('r'),
        Modifiers {
            control: true,
            shift: true,
            ..Modifiers::NONE
        },
    ));

    assert_eq!(state.workspace().active_session().panes().len(), 2);
    assert!(state.status().contains("Split pane right"));
}

#[test]
fn keymap_palette_override_changes_palette_dispatch() {
    let mut config = test_config();
    config.keymap.palette.rebind(CommandId::SplitRight, 'e');
    let mut state = AppState::new(config);

    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('e')));
    assert_eq!(state.workspace().active_session().panes().len(), 2);

    // The displaced default letter no longer splits.
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('v')));
    assert_eq!(state.workspace().active_session().panes().len(), 2);
}

#[test]
fn reload_config_applies_project_config_live() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(false, false));
    let config_file = temp.project_path().join(".mandatum").join("config.toml");
    fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    fs::write(
        &config_file,
        "[keymap]\nsplit-right = \"ctrl+alt+s\"\n\n[theme]\nname = \"mandatum-light\"\n",
    )
    .unwrap();

    state.dispatch(CommandId::ReloadConfig);

    assert_eq!(state.status(), "config reloaded");
    assert_eq!(state.theme().name, "mandatum-light");
    state.handle_key(Key::new(
        KeyCode::Char('s'),
        Modifiers {
            control: true,
            alt: true,
            ..Modifiers::NONE
        },
    ));
    assert_eq!(state.workspace().active_session().panes().len(), 2);

    // A now-broken config reloads onto defaults with the problem named.
    fs::write(&config_file, "{{ not toml").unwrap();
    state.dispatch(CommandId::ReloadConfig);
    assert!(state.status().starts_with("config reloaded;"));
    assert!(state.status().contains("not valid TOML"));
    assert_eq!(state.theme().name, "mandatum-dark");
}

#[test]
fn config_warnings_surface_as_startup_status_and_survive_first_resize() {
    let mut config = test_config();
    config.config_warnings = vec!["user config: unknown config section [wat]".to_owned()];
    let mut state = AppState::new(config);

    assert!(state.status().contains("unknown config section [wat]"));
    state.handle_terminal_resize(80, 24);
    assert!(state.status().contains("unknown config section [wat]"));
}

#[test]
fn palette_entries_show_their_bound_keys() {
    let mut config = test_config();
    config
        .keymap
        .bind_chord(CommandId::SplitRight, parse_chord("ctrl+shift+r").unwrap());
    let mut state = AppState::new(config);
    state.handle_key(ctrl('p'));

    let overlay = state.palette_overlay(SceneSize::new(100, 30)).unwrap();
    let split = overlay
        .items
        .iter()
        .find(|item| item.label == "Split pane right")
        .unwrap();
    assert_eq!(split.key_hint.as_deref(), Some("v · ctrl+shift+r"));
    // The footer names the palette's own keys.
    assert!(overlay.footer.contains("esc close"), "{}", overlay.footer);
}

// --- Pointer routing ---------------------------------------------------

/// A 100x30 frame: workspace area rows 1..=28, status row 29.
const POINTER_FRAME: SceneSize = SceneSize {
    width: 100,
    height: 30,
};

fn pointer_event(
    kind: PointerKind,
    button: Option<PointerButton>,
    column: u16,
    row: u16,
) -> PointerEvent {
    PointerEvent {
        kind,
        button,
        column,
        row,
        mods: Modifiers::NONE,
    }
}

fn send_pointer(state: &mut AppState, event: PointerEvent) {
    state.handle_event(InputEvent::Pointer(event));
}

fn left(kind: PointerKind, column: u16, row: u16) -> PointerEvent {
    pointer_event(kind, Some(PointerButton::Left), column, row)
}

fn right_down(column: u16, row: u16) -> PointerEvent {
    pointer_event(PointerKind::Down, Some(PointerButton::Right), column, row)
}

/// Resize and build one frame so hit targets exist, like the run loop.
fn frame(state: &mut AppState) {
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    state.build_scene(POINTER_FRAME);
}

fn focused(state: &AppState) -> String {
    state
        .workspace()
        .active_session()
        .focused_pane_id()
        .as_str()
        .to_owned()
}

// Pointer events with no scene built yet (no hit targets) do nothing.
#[test]
fn pointer_without_hit_targets_is_inert() {
    let mut state = state();
    let before_status = state.status().to_owned();

    for kind in [
        PointerKind::Down,
        PointerKind::Up,
        PointerKind::Move,
        PointerKind::Drag,
        PointerKind::Wheel { dx: 0, dy: 1 },
    ] {
        send_pointer(&mut state, left(kind, 2, 2));
    }

    assert_eq!(state.workspace().active_session().panes().len(), 1);
    assert!(!state.palette_open());
    assert!(!state.should_quit());
    assert_eq!(state.status(), before_status);
}

#[test]
fn click_on_pane_body_focuses_that_pane() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    assert_eq!(focused(&state), "pane-2");
    frame(&mut state);

    // pane-1 tiles the left half; its body starts at (1, 2).
    send_pointer(&mut state, left(PointerKind::Down, 5, 5));

    assert_eq!(focused(&state), "pane-1");
    assert!(state.status().contains("focused pane-1"));

    // Clicking the title focuses too.
    state.build_scene(POINTER_FRAME);
    send_pointer(&mut state, left(PointerKind::Down, 55, 1));
    assert_eq!(focused(&state), "pane-2");
}

#[test]
fn double_click_on_pane_title_toggles_zoom() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    frame(&mut state);

    send_pointer(&mut state, left(PointerKind::Down, 5, 1));
    send_pointer(&mut state, left(PointerKind::Up, 5, 1));
    send_pointer(&mut state, left(PointerKind::Down, 5, 1));
    send_pointer(&mut state, left(PointerKind::Up, 5, 1));

    let session = state.workspace().active_session();
    assert_eq!(
        session.layout().zoomed(),
        Some(&PaneId::new("pane-1")),
        "double-click on the title must zoom the pane"
    );
}

#[test]
fn separator_drag_resizes_the_split_live() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    frame(&mut state);

    // The 50% boundary of the 100-wide area sits at column 50; the
    // separator strip covers columns 49-50.
    send_pointer(&mut state, left(PointerKind::Down, 49, 10));
    send_pointer(&mut state, left(PointerKind::Drag, 30, 10));

    let mandatum_core::LayoutNode::Split { first_percent, .. } =
        state.workspace().active_session().layout().root()
    else {
        panic!("root must be a split");
    };
    assert_eq!(*first_percent, 30);
    assert!(state.status().contains("split resized to 30%"));

    // The next frame draws the moved boundary and its separator.
    let scene = state.build_scene(POINTER_FRAME);
    let pane_1 = scene
        .panes
        .iter()
        .find(|pane| pane.id == PaneId::new("pane-1"))
        .unwrap();
    assert_eq!(pane_1.area.width, 30);

    // Dragging further keeps resizing until release; percentages clamp.
    send_pointer(&mut state, left(PointerKind::Drag, 1, 10));
    send_pointer(&mut state, left(PointerKind::Up, 1, 10));
    let mandatum_core::LayoutNode::Split { first_percent, .. } =
        state.workspace().active_session().layout().root()
    else {
        panic!("root must be a split");
    };
    assert_eq!(*first_percent, 5);
}

#[test]
fn floating_title_drag_moves_the_float() {
    let mut state = state();
    state.dispatch(CommandId::NewTerminal); // floating pane-2 at (8, 4)
    frame(&mut state);

    // The float's title row is at screen y = 1 (area top) + 4 = 5.
    send_pointer(&mut state, left(PointerKind::Down, 10, 5));
    send_pointer(&mut state, left(PointerKind::Drag, 15, 8));
    send_pointer(&mut state, left(PointerKind::Up, 15, 8));

    let layout = state.workspace().active_session().layout();
    let rect = &layout.floating()[0].rect;
    assert_eq!((rect.x, rect.y), (13, 7));
    assert!(state.status().contains("moved pane-2"));
}

#[test]
fn right_click_opens_context_menu_and_escape_dismisses() {
    let mut state = state();
    frame(&mut state);

    send_pointer(&mut state, right_down(5, 5));

    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::ContextMenu(menu)) = &scene.overlay else {
        panic!("right-click must open the context menu overlay");
    };
    let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "Command palette",
            "Enter copy mode",
            "Copy selection",
            "Restart pane",
            "New terminal",
            "Split pane right",
            "Split pane down",
            "Zoom pane",
            "Float pane",
            "Close pane",
            "Search session output",
            "Help",
        ]
    );
    // Every row names its keyboard route; the palette gateway row leads
    // so the mouse always has a door into the full command surface.
    assert_eq!(menu.items[0].chord_hint, "ctrl+p");
    let zoom = menu.items.iter().find(|i| i.label == "Zoom pane").unwrap();
    assert_eq!(zoom.chord_hint, "ctrl+p z");

    // While the menu is open, typing does not reach the shell and Esc
    // closes.
    state.handle_key(key(KeyCode::Char('x')));
    assert_eq!(state.workspace().active_session().panes().len(), 1);
    state.handle_key(key(KeyCode::Escape));
    let scene = state.build_scene(POINTER_FRAME);
    assert!(scene.overlay.is_none());
}

#[test]
fn context_menu_keyboard_navigates_and_dispatches() {
    let mut state = state();
    frame(&mut state);
    send_pointer(&mut state, right_down(5, 5));

    // Down to "Zoom pane" (index 7), then Enter runs it.
    for _ in 0..7 {
        state.handle_key(key(KeyCode::Down));
    }
    state.handle_key(key(KeyCode::Enter));

    let session = state.workspace().active_session();
    assert_eq!(session.layout().zoomed(), Some(&PaneId::new("pane-1")));
    let scene = state.build_scene(POINTER_FRAME);
    assert!(scene.overlay.is_none(), "menu closes after dispatch");
}

#[test]
fn context_menu_rows_are_clickable() {
    let mut state = state();
    frame(&mut state);
    send_pointer(&mut state, right_down(5, 5));
    let scene = state.build_scene(POINTER_FRAME);

    // Click the "Zoom pane" row (index 7) through its hit target.
    let zoom_row = scene
        .hit_targets
        .iter()
        .find(|target| target.kind == HitTargetKind::ContextMenuItem(7))
        .expect("menu rows must be hit targets");
    send_pointer(
        &mut state,
        left(PointerKind::Down, zoom_row.rect.x + 1, zoom_row.rect.y),
    );

    let session = state.workspace().active_session();
    assert_eq!(session.layout().zoomed(), Some(&PaneId::new("pane-1")));

    // Click-away dismisses without running anything.
    send_pointer(&mut state, right_down(5, 5));
    state.build_scene(POINTER_FRAME);
    send_pointer(&mut state, left(PointerKind::Down, 90, 28));
    let scene = state.build_scene(POINTER_FRAME);
    assert!(scene.overlay.is_none());
    assert_eq!(
        state.workspace().active_session().layout().zoomed(),
        Some(&PaneId::new("pane-1")),
        "click-away must not dispatch a row"
    );
}

// State-aware menu labels: a zoomed pane's menu offers "Unzoom pane",
// and docking/floating already flips its row — the menu never names an
// action that would do the opposite of its label.
#[test]
fn context_menu_labels_reflect_zoom_state() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    state.dispatch(CommandId::ZoomPane);
    frame(&mut state);

    send_pointer(&mut state, right_down(5, 5));
    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::ContextMenu(menu)) = &scene.overlay else {
        panic!("right-click must open the context menu overlay");
    };
    let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
    assert!(labels.contains(&"Unzoom pane"), "{labels:?}");
    assert!(!labels.contains(&"Zoom pane"), "{labels:?}");
    state.handle_key(key(KeyCode::Escape));

    // Unzoomed, the plain label returns.
    state.dispatch(CommandId::ZoomPane);
    state.build_scene(POINTER_FRAME);
    send_pointer(&mut state, right_down(5, 5));
    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::ContextMenu(menu)) = &scene.overlay else {
        panic!("right-click must open the context menu overlay");
    };
    assert!(menu.items.iter().any(|item| item.label == "Zoom pane"));
}

// Every context-menu row names its keyboard route — on a task pane the
// Rerun row shows the restart letter it rides ("Rerun task" had none).
#[test]
fn every_context_menu_row_names_its_keyboard_route() {
    let mut state = state();
    state.dispatch(CommandId::RunTask);
    let task_pane = state.workspace().active_session().focused_pane_id().clone();
    frame(&mut state);

    let scene = state.build_scene(POINTER_FRAME);
    let title = scene
        .hit_targets
        .iter()
        .find(|target| target.kind == HitTargetKind::PaneTitle(task_pane.clone()))
        .expect("task pane title target");
    send_pointer(&mut state, right_down(title.rect.x + 1, title.rect.y));

    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::ContextMenu(menu)) = &scene.overlay else {
        panic!("right-click must open the context menu overlay");
    };
    for item in &menu.items {
        assert!(
            !item.chord_hint.is_empty(),
            "menu row {:?} names no keyboard route",
            item.label
        );
    }
    let rerun = menu
        .items
        .iter()
        .find(|item| item.label == "Rerun task")
        .expect("task menu offers Rerun task");
    assert_eq!(rerun.chord_hint, "ctrl+p r");
}

// A press on the menu's own border is a near-miss, not a dismissal:
// it neither runs a row nor swallows the menu.
#[test]
fn context_menu_border_click_does_not_dismiss() {
    let mut state = state();
    frame(&mut state);
    send_pointer(&mut state, right_down(5, 5));
    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::ContextMenu(menu)) = &scene.overlay else {
        panic!("right-click must open the context menu overlay");
    };
    let area = menu.area;

    // Top border cell: inside the menu rect, not a row.
    send_pointer(&mut state, left(PointerKind::Down, area.x, area.y));
    let scene = state.build_scene(POINTER_FRAME);
    assert!(
        matches!(
            &scene.overlay,
            Some(mandatum_scene::OverlayScene::ContextMenu(_))
        ),
        "a border press must not dismiss the menu"
    );
    assert_eq!(
        state.workspace().active_session().layout().zoomed(),
        None,
        "a border press must not run a row"
    );

    // A genuine click-away still dismisses.
    send_pointer(&mut state, left(PointerKind::Down, 95, 28));
    let scene = state.build_scene(POINTER_FRAME);
    assert!(scene.overlay.is_none());
}

// The status strip is a clickable front door: left-click opens the
// palette the permanent hint names.
#[test]
fn status_strip_click_opens_the_palette() {
    let mut state = state();
    frame(&mut state);

    // Status row is the bottom row of the 100x30 frame.
    send_pointer(&mut state, left(PointerKind::Down, 50, 29));

    assert!(state.palette_open());
}

// The menu's gateway row gives the mouse a path into the full command
// surface (new terminal, splits, save/restore) without any chord.
#[test]
fn context_menu_gateway_row_opens_the_palette() {
    let mut state = state();
    frame(&mut state);
    send_pointer(&mut state, right_down(5, 5));

    // "Command palette" is the selected first row.
    state.handle_key(key(KeyCode::Enter));

    assert!(state.palette_open());
    assert!(state.context_menu.is_none());
}

#[test]
fn quit_quits_from_the_palette_by_letter_and_by_row() {
    // The classic fast path: bare 'q' on the empty input.
    let mut fast = state();
    fast.handle_key(ctrl('p'));
    fast.handle_key(key(KeyCode::Char('q')));
    assert!(fast.should_quit());

    // The discoverable path: type its name, Enter runs the listed row.
    let mut typed = state();
    typed.handle_key(ctrl('p'));
    typed.handle_key(Key::new(
        KeyCode::Char('Q'),
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    ));
    for character in "uit".chars() {
        typed.handle_key(key(KeyCode::Char(character)));
    }
    let overlay = typed.palette_overlay(SceneSize::new(100, 30)).unwrap();
    assert_eq!(overlay.items[0].label, "Quit Mandatum");
    typed.handle_key(key(KeyCode::Enter));
    assert!(typed.should_quit());
}

// The wheel moves the palette selection (the item window follows), so
// entries below the fold are reachable by mouse; the footer counts them.
#[test]
fn wheel_scrolls_the_open_palette_and_the_footer_counts_the_overflow() {
    let mut state = state();
    frame(&mut state);
    state.handle_key(ctrl('p'));
    state.build_scene(POINTER_FRAME);

    let overlay = state.palette_overlay(POINTER_FRAME).unwrap();
    assert!(
        overlay.footer.contains("more"),
        "overflow must be marked, got {:?}",
        overlay.footer
    );

    send_pointer(
        &mut state,
        pointer_event(PointerKind::Wheel { dx: 0, dy: 2 }, None, 50, 15),
    );
    assert_eq!(
        state.palette_overlay(POINTER_FRAME).unwrap().selected,
        Some(2)
    );
    send_pointer(
        &mut state,
        pointer_event(PointerKind::Wheel { dx: 0, dy: -1 }, None, 50, 15),
    );
    assert_eq!(
        state.palette_overlay(POINTER_FRAME).unwrap().selected,
        Some(1)
    );
    assert!(state.palette_open(), "wheel must not close the palette");
}

// Keyboard resize: Grow/Shrink move the focused pane's nearest split
// boundary, the same durable intent separator drags write.
#[test]
fn grow_and_shrink_resize_the_focused_split_from_the_keyboard() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);

    // Focused pane-2 is the second split side: growing it shrinks the
    // first side's share.
    state.dispatch(CommandId::GrowPane);
    let LayoutNode::Split { first_percent, .. } =
        state.workspace().active_session().layout().root()
    else {
        panic!("root must be a split");
    };
    assert_eq!(*first_percent, 45);

    // The '+' fast key dispatches even when the terminal reports shift
    // (symbols are not the Shift+letter search escape).
    state.handle_key(ctrl('p'));
    state.handle_key(Key::new(
        KeyCode::Char('+'),
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    ));
    let LayoutNode::Split { first_percent, .. } =
        state.workspace().active_session().layout().root()
    else {
        panic!("root must be a split");
    };
    assert_eq!(*first_percent, 40);

    state.dispatch(CommandId::ShrinkPane);
    let LayoutNode::Split { first_percent, .. } =
        state.workspace().active_session().layout().root()
    else {
        panic!("root must be a split");
    };
    assert_eq!(*first_percent, 45);
}

// Float is no longer a one-way door: Dock returns a floating pane to
// the tiled tree, the float letter toggles, and floating an
// already-floating pane reports the problem instead of a false success.
#[test]
fn dock_undoes_float_and_float_never_reports_a_false_success() {
    let mut state = state();
    let pane_2 = PaneId::new("pane-2");
    state.dispatch(CommandId::NewTerminal); // floating, focused

    state.dispatch(CommandId::FloatPane);
    assert!(
        state.status().contains("already floating"),
        "{}",
        state.status()
    );

    state.dispatch(CommandId::DockPane);
    assert!(
        !state
            .workspace()
            .active_session()
            .layout()
            .is_floating(&pane_2)
    );

    // The palette letter is a float/dock toggle.
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('f')));
    assert!(
        state
            .workspace()
            .active_session()
            .layout()
            .is_floating(&pane_2)
    );
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('f')));
    assert!(
        !state
            .workspace()
            .active_session()
            .layout()
            .is_floating(&pane_2)
    );
}

#[test]
fn task_pane_context_menu_offers_rerun_and_stop() {
    let mut state = state();
    state.dispatch(CommandId::RunTask); // floating task pane, focused
    frame(&mut state);
    let scene = state.build_scene(POINTER_FRAME);
    let task_pane = scene.panes.iter().find(|pane| pane.floating).unwrap();
    let inner = mandatum_scene::layout::pane_inner_rect(task_pane.area);

    send_pointer(&mut state, right_down(inner.x + 1, inner.y + 1));

    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::ContextMenu(menu)) = &scene.overlay else {
        panic!("right-click on a task pane must open the menu");
    };
    let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
    assert!(labels.contains(&"Rerun task"));
    assert!(labels.contains(&"Stop task"));
    assert!(!labels.contains(&"Restart pane"));
    // A floating pane's menu offers Dock (the runnable half of the
    // float/dock toggle) and no splits (floats cannot be split).
    assert!(labels.contains(&"Dock pane"));
    assert!(!labels.contains(&"Float pane"));
    assert!(!labels.contains(&"Split pane right"));
}

#[test]
fn resize_clears_pointer_selection_drag_and_menu() {
    let mut state = state();
    frame(&mut state);
    send_pointer(&mut state, right_down(5, 5));
    assert!(state.context_menu.is_some());

    state.handle_terminal_resize(120, 40);

    assert!(state.context_menu.is_none());
    assert!(state.pointer_view.is_none());
    assert!(state.pointer_drag.is_none());
}

// [L5-GATE] Input reaches the child unless explicit workspace control intercepts.
#[test]
fn normal_keys_are_terminal_input_when_palette_is_closed() {
    assert_eq!(
        key_to_input(key(KeyCode::Char('q'))),
        RuntimeInput::SendToTerminal(b"q".to_vec())
    );
    assert_eq!(
        key_to_input(key(KeyCode::Enter)),
        RuntimeInput::SendToTerminal(b"\r".to_vec())
    );
    assert_eq!(
        key_to_input(ctrl('c')),
        RuntimeInput::SendToTerminal(vec![0x03])
    );
}

#[test]
fn input_dispatch_updates_core_workspace_layout_in_palette_mode() {
    let mut state = state();

    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('v')));
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('s')));
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::BackTab));

    let session = state.workspace().active_session();
    assert_eq!(session.panes().len(), 3);
    assert_eq!(session.focused_pane_id().as_str(), "pane-2");
    assert!(state.status().contains("Focus previous pane"));
}

#[test]
fn palette_opens_and_closes_without_mutating_layout() {
    let mut state = state();

    state.handle_key(ctrl('p'));
    assert!(state.palette_open());
    assert_eq!(state.workspace().active_session().panes().len(), 1);

    state.handle_key(key(KeyCode::Escape));
    assert!(!state.palette_open());
}

/// The full open-type-execute flow, driven with neutral keys: Shift+R
/// starts the fuzzy filter (bypassing the fast path), a plain letter
/// extends it, and Enter runs the best match.
#[test]
fn palette_open_type_execute_flow_runs_the_best_fuzzy_match() {
    let mut state = state();

    state.handle_key(ctrl('p'));
    state.handle_key(Key::new(
        KeyCode::Char('R'),
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    ));
    // The filter is non-empty now, so the bound letters 'u' and 'n' type
    // instead of dispatching their fast-path commands.
    state.handle_key(key(KeyCode::Char('u')));
    state.handle_key(key(KeyCode::Char('n')));
    assert!(state.palette_open());
    let overlay = state.palette_overlay(SceneSize::new(100, 30)).unwrap();
    assert_eq!(overlay.query, "Run");
    assert_eq!(overlay.items[0].label, "Run task");
    assert_eq!(overlay.selected, Some(0));

    state.handle_key(key(KeyCode::Enter));
    assert!(!state.palette_open());
    assert_eq!(state.workspace().active_session().panes().len(), 2);
    assert!(state.focused_pane_is_task());
}

/// Shift+letter always starts the filter, so commands whose first letter
/// is a fast path stay reachable by typing.
#[test]
fn shift_letter_bypasses_the_fast_path_and_types_into_the_filter() {
    let mut state = state();

    state.handle_key(ctrl('p'));
    state.handle_key(Key::new(
        KeyCode::Char('S'),
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    ));
    assert!(
        state.palette_open(),
        "shifted letter must type, not dispatch"
    );
    assert_eq!(state.workspace().active_session().panes().len(), 1);

    let overlay = state.palette_overlay(SceneSize::new(100, 30)).unwrap();
    assert_eq!(overlay.query, "S");
    assert_eq!(overlay.items[0].label, "Split pane right");

    state.handle_key(key(KeyCode::Enter));
    assert!(!state.palette_open());
    assert_eq!(state.workspace().active_session().panes().len(), 2);
    assert!(state.status().contains("Split pane right"));
}

/// Ctrl+N/Ctrl+P move the selection while the palette is open (Ctrl+P
/// navigates instead of toggling; Esc closes), and arrows match.
#[test]
fn palette_selection_navigates_with_arrows_and_ctrl_n_p() {
    let mut state = state();
    let size = SceneSize::new(100, 30);

    state.handle_key(ctrl('p'));
    assert_eq!(state.palette_overlay(size).unwrap().selected, Some(0));

    state.handle_key(ctrl('n'));
    assert_eq!(state.palette_overlay(size).unwrap().selected, Some(1));
    state.handle_key(key(KeyCode::Down));
    assert_eq!(state.palette_overlay(size).unwrap().selected, Some(2));
    state.handle_key(ctrl('p'));
    assert!(state.palette_open(), "ctrl+p must navigate, not close");
    assert_eq!(state.palette_overlay(size).unwrap().selected, Some(1));
    state.handle_key(key(KeyCode::Up));
    assert_eq!(state.palette_overlay(size).unwrap().selected, Some(0));
    // Selection clamps at the top instead of wrapping.
    state.handle_key(key(KeyCode::Up));
    assert_eq!(state.palette_overlay(size).unwrap().selected, Some(0));

    // Executing the selected entry works end to end: on a terminal pane
    // the first entry is "New terminal" (pane commands rank first).
    let overlay = state.palette_overlay(size).unwrap();
    assert_eq!(overlay.items[0].label, "New terminal");
    state.handle_key(key(KeyCode::Enter));
    assert!(!state.palette_open());
    assert_eq!(state.workspace().active_session().panes().len(), 2);
}

/// Enter on a greyed entry reports the reason and keeps the palette
/// open; the entry stays visible rather than hidden.
#[test]
fn palette_enter_on_greyed_entry_reports_the_reason_and_stays_open() {
    let mut state = state();
    let size = SceneSize::new(100, 30);

    state.handle_key(ctrl('p'));
    // "Approve" begins with the fast-path letter 'a', so start the
    // filter with Shift+A and type the rest plain.
    state.handle_key(Key::new(
        KeyCode::Char('A'),
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    ));
    for character in "pprove".chars() {
        state.handle_key(key(KeyCode::Char(character)));
    }

    let overlay = state.palette_overlay(size).unwrap();
    assert_eq!(overlay.items[0].label, "Approve agent action");
    assert!(!overlay.items[0].enabled);
    assert_eq!(overlay.items[0].detail, "focused pane is not an agent pane");

    state.handle_key(key(KeyCode::Enter));
    assert!(
        state.palette_open(),
        "greyed entries must not close the palette"
    );
    assert!(
        state.status().contains("focused pane is not an agent pane"),
        "{}",
        state.status()
    );
    assert_eq!(state.workspace().active_session().panes().len(), 1);
}

/// Context ranking end to end: on a focused agent pane, agent commands
/// lead the empty-query list.
#[test]
fn palette_ranks_agent_commands_first_on_agent_panes() {
    let mut state = state();
    state.dispatch(CommandId::NewAgentPane);
    let size = SceneSize::new(100, 30);

    state.handle_key(ctrl('p'));
    let overlay = state.palette_overlay(size).unwrap();
    assert_eq!(overlay.items[0].label, "New agent pane");
    assert_eq!(overlay.items[1].label, "Start agent");
    // Approve is greyed with its reason, but present and ranked with its
    // agent siblings — discoverability over minimalism.
    let approve = overlay
        .items
        .iter()
        .position(|item| item.label == "Approve agent action")
        .unwrap();
    assert!(approve < 6, "agent commands must lead, got index {approve}");
    assert!(!overlay.items[approve].enabled);
    assert_eq!(
        overlay.items[approve].detail,
        "no approval is pending in this pane"
    );
}

/// Backspace edits the filter; clearing it restores the fast-path row.
#[test]
fn palette_backspace_edits_the_query() {
    let mut state = state();
    let size = SceneSize::new(100, 30);

    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('i')));
    assert_eq!(state.palette_overlay(size).unwrap().query, "i");
    state.handle_key(key(KeyCode::Backspace));
    let overlay = state.palette_overlay(size).unwrap();
    assert_eq!(overlay.query, "");
    assert_eq!(overlay.items.len(), BUILT_IN_COMMANDS.len());

    // With the query empty again, the fast path is live once more.
    state.handle_key(key(KeyCode::Char('v')));
    assert!(!state.palette_open());
    assert_eq!(state.workspace().active_session().panes().len(), 2);
}

#[test]
fn command_errors_are_reported_as_status_instead_of_panicking() {
    let mut state = state();

    // The fast path is gated: 'x' (Close pane) on the last pane reports
    // the same reason the greyed palette row shows, palette stays open.
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('x')));
    assert!(!state.should_quit());
    assert!(state.palette_open());
    assert!(
        state
            .status()
            .contains("Close pane is unavailable: cannot close the last pane"),
        "{}",
        state.status()
    );
    state.handle_key(key(KeyCode::Escape));

    // A core dispatch error still lands as status, never a panic.
    state.dispatch(CommandId::ClosePane);
    assert!(!state.should_quit());
    assert!(state.status().contains("cannot remove the last tiled pane"));
}

#[test]
fn resize_event_updates_runtime_size_without_core_mutation() {
    let mut state = state();

    state.handle_event(InputEvent::Resize(SceneSize::new(100, 35)));

    assert_eq!(state.terminal_size(), Some((100, 35)));
    assert_eq!(state.workspace().active_session().panes().len(), 1);
    assert!(state.status().contains("100x35"));
}

#[test]
fn save_workspace_writes_durable_json_to_configured_path() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(false, false));

    state.dispatch(CommandId::SplitRight);
    state.dispatch(CommandId::SaveWorkspace);

    let saved = fs::read_to_string(state.workspace_file()).expect("workspace file saved");
    let restored = Workspace::from_json(&saved).expect("saved workspace should round-trip");

    assert!(state.status().contains("workspace saved"));
    assert!(state.status().contains(".mandatum/workspace.json"));
    assert_eq!(restored.active_session().panes().len(), 2);
    for forbidden in [
        "terminal_panes",
        "NativePty",
        "process_id",
        "reader_thread",
        "parser",
        "exit_status",
        "scrollback",
    ] {
        assert!(
            !saved.contains(forbidden),
            "saved workspace leaked runtime field {forbidden}"
        );
    }
}

#[cfg(unix)]
#[test]
fn save_workspace_rejects_symlink_target() {
    use std::os::unix::fs::symlink;

    let temp = TestWorkspaceDir::new();
    let target = temp.path.join("outside.json");
    fs::write(&target, "keep me").unwrap();
    ensure_parent_dir(&temp.workspace_file()).unwrap();
    symlink(&target, temp.workspace_file()).unwrap();

    let mut state = AppState::new(temp.app_config(false, false));
    state.dispatch(CommandId::SaveWorkspace);

    assert!(state.status().contains("workspace save failed"));
    assert!(state.status().contains("must not be a symlink"));
    assert_eq!(fs::read_to_string(target).unwrap(), "keep me");
}

#[cfg(unix)]
#[test]
fn restore_workspace_rejects_symlink_target() {
    use std::os::unix::fs::symlink;

    let temp = TestWorkspaceDir::new();
    let target = temp.path.join("outside.json");
    fs::write(
        &target,
        Workspace::new("Other", temp.project_path())
            .to_json()
            .unwrap(),
    )
    .unwrap();
    ensure_parent_dir(&temp.workspace_file()).unwrap();
    symlink(&target, temp.workspace_file()).unwrap();

    let mut state = AppState::new(temp.app_config(false, false));
    let before = state.workspace().clone();
    state.dispatch(CommandId::RestoreWorkspace);

    assert!(state.status().contains("workspace restore failed"));
    assert!(state.status().contains("must not be a symlink"));
    assert_eq!(state.workspace(), &before);
}

#[test]
fn restore_workspace_rejects_oversized_file() {
    let temp = TestWorkspaceDir::new();
    ensure_parent_dir(&temp.workspace_file()).unwrap();
    fs::write(
        temp.workspace_file(),
        vec![b' '; (MAX_WORKSPACE_FILE_BYTES + 1) as usize],
    )
    .unwrap();

    let mut state = AppState::new(temp.app_config(false, false));
    let before = state.workspace().clone();
    state.dispatch(CommandId::RestoreWorkspace);

    assert!(state.status().contains("workspace restore failed"));
    assert!(state.status().contains("too large"));
    assert_eq!(state.workspace(), &before);
}

#[test]
fn resize_surfaces_runtime_reconciliation_failure() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.shell_program = "/definitely/missing/mandatum-shell".to_owned();
    let mut state = AppState::new(config);

    state.handle_terminal_resize(80, 24);

    assert!(state.status().contains("PTY spawn failed"));
    assert!(!state.status().contains("terminal resized"));
    assert_eq!(state.live_terminal_count(), 0);
}

#[test]
fn explicit_restore_loads_valid_workspace_and_updates_new_terminal_context() {
    let temp = TestWorkspaceDir::new();
    let restored_project = temp.project_path();
    let mut saved_workspace = Workspace::new("Restored", restored_project.clone());
    saved_workspace
        .apply_action(CoreAction::SplitRight)
        .unwrap();
    saved_workspace
        .apply_action(CoreAction::FocusPrevious)
        .unwrap();
    write_workspace_file(&temp.workspace_file(), &saved_workspace).unwrap();

    let mut state = AppState::new(AppConfig {
        workspace_name: "Original".to_owned(),
        project_path: temp.path.join("other-project"),
        workspace_file: temp.workspace_file(),
        task_command: "printf TASK_OK".to_owned(),
        agent_objective: "test objective".to_owned(),
        ..AppConfig::default()
    });

    state.dispatch(CommandId::RestoreWorkspace);

    assert!(state.status().contains("workspace restored"));
    assert_eq!(state.workspace().name(), "Restored");
    assert_eq!(state.workspace().active_session().panes().len(), 2);
    assert_eq!(
        state
            .workspace()
            .active_session()
            .focused_pane_id()
            .as_str(),
        "pane-1"
    );

    state.dispatch(CommandId::NewTerminal);
    let focused = state.workspace().active_session().focused_pane_id().clone();
    let pane = state.workspace().active_session().pane(&focused).unwrap();
    assert_eq!(pane.cwd(), Some(&restored_project));
}

#[test]
fn restore_failure_is_visible_and_preserves_current_workspace() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(false, false));
    state.dispatch(CommandId::SplitRight);
    let before = state.workspace().clone();
    ensure_parent_dir(&temp.workspace_file()).unwrap();
    fs::write(temp.workspace_file(), "{ not json").unwrap();

    state.dispatch(CommandId::RestoreWorkspace);

    assert!(state.status().contains("workspace restore failed"));
    assert_eq!(state.workspace(), &before);
}

#[test]
fn restore_failure_preserves_current_runtime_when_pty_staging_fails() {
    let temp = TestWorkspaceDir::new();
    let saved_workspace = Workspace::new("Restored", temp.project_path());
    write_workspace_file(&temp.workspace_file(), &saved_workspace).unwrap();

    let mut state = AppState::new(temp.app_config(true, false));
    state.handle_terminal_resize(80, 24);
    assert_eq!(state.live_terminal_count(), 1);
    let before = state.workspace().clone();
    let pane_id = PaneId::new("pane-1");
    let before_pid = state
        .terminal_panes
        .get(&pane_id)
        .unwrap()
        .controller
        .process_id();

    state.shell_program = "/definitely/missing/mandatum-shell".to_owned();

    state.dispatch(CommandId::RestoreWorkspace);

    assert!(state.status().contains("workspace restore failed"));
    assert!(state.status().contains("PTY spawn failed"));
    assert_eq!(state.workspace(), &before);
    assert_eq!(state.live_terminal_count(), 1);
    assert_eq!(
        state
            .terminal_panes
            .get(&pane_id)
            .unwrap()
            .controller
            .process_id(),
        before_pid
    );

    state.shutdown();
}

#[test]
fn startup_restore_loads_saved_workspace_and_keeps_status_visible_on_first_resize() {
    let temp = TestWorkspaceDir::new();
    let mut saved_workspace = Workspace::new("Restored", temp.project_path());
    saved_workspace
        .apply_action(CoreAction::SplitRight)
        .unwrap();
    write_workspace_file(&temp.workspace_file(), &saved_workspace).unwrap();

    let mut state = AppState::new(temp.app_config(false, true));

    assert!(state.status().contains("workspace restored"));
    assert_eq!(state.workspace().active_session().panes().len(), 2);

    state.handle_terminal_resize(100, 35);

    assert!(state.status().contains("workspace restored"));
}

#[test]
fn zoom_hides_panes_without_removing_their_runtime_identity() {
    let mut state = state();

    state.handle_event(InputEvent::Resize(SceneSize::new(100, 35)));
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('v')));
    state.handle_key(ctrl('p'));
    state.handle_key(key(KeyCode::Char('z')));

    let terminal_ids = state.terminal_pane_ids();
    let visible_sizes = state.visible_terminal_pane_sizes();

    assert_eq!(terminal_ids.len(), 2);
    assert_eq!(visible_sizes.len(), 1);
    assert!(terminal_ids.contains(&PaneId::new("pane-1")));
    assert!(terminal_ids.contains(&PaneId::new("pane-2")));
}

fn live_state() -> AppState {
    AppState::new(AppConfig {
        spawn_pty: true,
        ..test_config()
    })
}

fn pump_runtime_until(state: &mut AppState, mut predicate: impl FnMut(&AppState) -> bool) -> bool {
    for _ in 0..300 {
        state.tick_runtime();
        if predicate(state) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

const SHELL_READY_MARKER: &str = "__MANDATUM_SHELL_READY__";
const SHELL_READY_COMMAND: &[u8] = b"printf '%s%s\\n' '__MANDATUM_SHELL_' 'READY__'\r";

/// Prove a freshly spawned shell is reading commands before interacting.
/// The command's echoed input contains the marker only as two separated
/// fragments, so only `printf` output can satisfy the suffix check even when
/// the startup prompt shares its row.
fn wait_for_shell_ready(state: &mut AppState, pane_id: &PaneId) {
    state
        .terminal_panes
        .get_mut(pane_id)
        .unwrap_or_else(|| panic!("fresh terminal runtime {pane_id} should exist"))
        .write_input(SHELL_READY_COMMAND)
        .unwrap_or_else(|error| {
            panic!("failed to send shell readiness probe to {pane_id}: {error}")
        });

    let ready = pump_runtime_until(state, |state| {
        state.terminal_panes.get(pane_id).is_some_and(|runtime| {
            runtime
                .parser
                .grid()
                .snapshot()
                .iter()
                // dash can paint its prompt before the command's output on
                // the same row. The echoed input cannot contain the assembled
                // marker, so a suffix match still proves `printf` executed.
                .any(|line| line.trim_end().ends_with(SHELL_READY_MARKER))
        })
    });
    assert!(
        ready,
        "shell readiness marker never appeared for {pane_id}; rows:\n{}",
        grid_text(state, pane_id)
    );
}

// A PTY output flood must not overwrite meaningful status with
// byte-count diagnostics: a failure status persists until something
// meaningful supersedes it, not until the next read.
#[test]
fn pty_output_flood_does_not_bury_meaningful_status() {
    let mut state = live_state();
    state.handle_terminal_resize(100, 30);
    let pane_id = PaneId::new("pane-1");

    // A meaningful status: a command that failed.
    state.dispatch(CommandId::StopTask);
    assert!(state.status().contains("not a task pane"));

    // Flood the pane with output and drain it all.
    state.write_to_focused_terminal(
        b"i=1; while [ $i -le 50 ]; do echo NOISE_$i; i=$((i+1)); done\r",
    );
    let flooded = pump_runtime_until(&mut state, |state| {
        grid_text(state, &pane_id).contains("NOISE_50")
    });
    assert!(flooded, "flood output never reached the grid");

    assert!(
        state.status().contains("not a task pane"),
        "diagnostics buried the failure status: {}",
        state.status()
    );
    assert!(!state.status().contains("byte(s)"));

    state.shutdown();
}

// `[ui] debug_status = true` restores the byte-level diagnostics for
// debugging sessions.
#[test]
fn debug_status_config_restores_byte_diagnostics() {
    let mut config = test_config();
    config.spawn_pty = true;
    config.debug_status = true;
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 30);

    let observed = pump_runtime_until(&mut state, |state| {
        state.status().contains("byte(s) from pane-1")
    });
    assert!(
        observed,
        "debug diagnostics never surfaced: {}",
        state.status()
    );

    state.shutdown();
}

// One drain call applies at most the budget, so a channel that never
// empties (a producer outrunning the consumer) can never pin the main
// loop inside drain_events and starve drawing.
#[test]
fn drain_events_bounds_work_per_call() {
    let mut state = state();
    let sender = state.event_sender();
    let backlog = DRAIN_EVENT_BUDGET + 10;
    for _ in 0..backlog {
        sender
            .send(AppEvent::Pty(
                PtyRuntimeEvent::Output {
                    pane_id: PaneId::new("pane-none"),
                    restart_generation: 0,
                    runtime_token: 0,
                    bytes: b"x".to_vec(),
                },
                None,
            ))
            .unwrap();
    }

    state.drain_events();
    assert!(
        state.event_rx.try_recv().is_ok(),
        "one drain call must leave events beyond the budget queued"
    );
}

// The flood regression the stranger test found: an infinite producer
// (`yes`) must leave the workstation bounded in memory, responsive to
// input, and quittable — the reader-side flow gate plus the bounded
// drain are what guarantee it.
#[test]
fn pty_flood_stays_bounded_responsive_and_quittable() {
    let mut state = live_state();
    state.handle_terminal_resize(100, 30);
    let pane_id = PaneId::new("pane-1");
    state.write_to_focused_terminal(b"yes\r");

    // Pump the shell loop's shape against the live flood for a while.
    let flood_window = Instant::now();
    let mut saw_output = false;
    while flood_window.elapsed() < Duration::from_millis(400) {
        state.wait_event(Duration::from_millis(8));
        state.drain_events();
        saw_output = saw_output || grid_text(&state, &pane_id).contains('y');
    }
    assert!(saw_output, "the flood never reached the grid");
    let in_flight = state
        .terminal_panes
        .get(&pane_id)
        .expect("pane-1 runtime")
        .flow
        .in_flight_bytes();
    assert!(
        in_flight <= crate::process_events::MAX_IN_FLIGHT_BYTES,
        "in-flight PTY bytes must stay under the gate cap, got {in_flight}"
    );

    // Input queued during the flood must land promptly: the quit chord
    // takes effect within the shell's next few frames, not never.
    state
        .event_sender()
        .send(AppEvent::Input(InputEvent::Key(Key::ctrl('q'))))
        .unwrap();
    let quit_wait = Instant::now();
    while !state.should_quit() && quit_wait.elapsed() < Duration::from_secs(2) {
        state.wait_event(Duration::from_millis(8));
        state.drain_events();
    }
    assert!(
        state.should_quit(),
        "the quit chord starved behind the flood"
    );

    // And shutdown must join the flooded reader thread instead of
    // deadlocking on its full flow gate.
    let shutdown_wait = Instant::now();
    state.shutdown();
    assert!(
        shutdown_wait.elapsed() < Duration::from_secs(5),
        "shutdown took {:?} under flood",
        shutdown_wait.elapsed()
    );
}

// A task whose intent names no cwd must run in the project directory —
// never portable-pty's `$HOME` fallback, which silently ran user task
// commands in the wrong directory (the live-slice demo's checks pane
// exited 127 because `./flaky-check.sh` resolved against `$HOME`).
#[test]
fn task_with_unset_cwd_runs_in_the_project_directory_not_home() {
    let mut config = test_config();
    config.spawn_pty = true;
    let project_dir = config.project_path.clone();
    // An anchor only the project directory contains: the command exits 0
    // only when it actually runs there.
    fs::write(project_dir.join("cwd-anchor"), b"here").unwrap();

    let mut state = AppState::new(config);
    state.handle_terminal_resize(120, 40);
    state
        .workspace_mut()
        .apply_action(CoreAction::CreateTaskPane {
            title: "checks".to_owned(),
            intent: TaskPaneIntent {
                recipe_id: Some("checks".to_owned()),
                command: "test -f ./cwd-anchor && touch RAN_IN_PROJECT".to_owned(),
                cwd: None,
            },
        })
        .unwrap();
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    state.dispatch(CommandId::RerunTask);

    let exited = pump_runtime_until(&mut state, |state| {
        state
            .task_panes
            .get(&pane_id)
            .is_some_and(|task| task.runtime.exit_status.is_some())
    });
    assert!(exited, "the task never exited");
    let status = state.task_panes.get(&pane_id).unwrap().status.clone();
    assert_eq!(status, "succeeded: exit 0", "task ran outside the project");
    assert!(
        project_dir.join("RAN_IN_PROJECT").exists(),
        "the task's side effect must land in the project directory"
    );

    state.shutdown();
}

// The live-slice demo's smoke path: rerunning the checks pane (intent
// cwd unset, flaky script in the project dir) alternates exit 0 / exit
// 3, exactly as the stranger-test walkthrough promises.
#[test]
fn demo_checks_pane_reruns_alternate_exit_0_and_exit_3() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos();
    let project_dir = std::env::temp_dir().join(format!(
        "mandatum-demo-checks-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&project_dir).unwrap();
    // The demo's flaky check: first run plants the marker and passes,
    // the next sees it, removes it, and fails with exit 3.
    fs::write(
        project_dir.join("flaky-check.sh"),
        "if [ -f .flip ]; then rm .flip; echo 'FAIL: marker present'; exit 3; \
             else touch .flip; echo OK; fi\n",
    )
    .unwrap();

    let mut config = test_config();
    config.spawn_pty = true;
    config.workspace_file = project_dir.join(".mandatum").join("workspace.json");
    config.project_path = project_dir.clone();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(120, 40);
    state
        .workspace_mut()
        .apply_action(CoreAction::CreateTaskPane {
            title: "checks".to_owned(),
            intent: TaskPaneIntent {
                recipe_id: Some("checks".to_owned()),
                command: "sh ./flaky-check.sh".to_owned(),
                cwd: None,
            },
        })
        .unwrap();
    let pane_id = state.workspace().active_session().focused_pane_id().clone();

    let rerun_status = |state: &mut AppState| -> String {
        state.dispatch(CommandId::RerunTask);
        let exited = pump_runtime_until(state, |state| {
            state
                .task_panes
                .get(&pane_id)
                .is_some_and(|task| task.runtime.exit_status.is_some())
        });
        assert!(exited, "the checks task never exited");
        state.task_panes.get(&pane_id).unwrap().status.clone()
    };

    assert_eq!(rerun_status(&mut state), "succeeded: exit 0");
    assert_eq!(rerun_status(&mut state), "failed: exit 3");
    assert_eq!(rerun_status(&mut state), "succeeded: exit 0");

    state.shutdown();
    let _ = fs::remove_dir_all(&project_dir);
}

// --- Pointer routing against live children ------------------------------

/// The rendered grid text of a live terminal pane.
fn grid_text(state: &AppState, pane_id: &PaneId) -> String {
    state
        .terminal_panes
        .get(pane_id)
        .map(|runtime| runtime.parser.grid().snapshot().join("\n"))
        .unwrap_or_default()
}

/// Two live panes, pane-1's child tracking the mouse (SGR), pane-2
/// focused. The tty echoes forwarded mouse bytes as visible `^[[<...`
/// text, so forwarding is observable in pane-1's grid.
fn live_state_with_capturing_child() -> AppState {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    state.dispatch(CommandId::SplitRight);

    state
        .workspace_mut()
        .apply_action(CoreAction::FocusPane {
            pane_id: PaneId::new("pane-1"),
        })
        .unwrap();
    state.write_to_focused_terminal(b"printf '\\033[?1000h\\033[?1006h'\r");
    let tracking = pump_runtime_until(&mut state, |state| {
        state
            .terminal_panes
            .get(&PaneId::new("pane-1"))
            .is_some_and(|runtime| runtime.parser.mouse_mode().wants_mouse())
    });
    assert!(tracking, "child never enabled mouse tracking");

    state
        .workspace_mut()
        .apply_action(CoreAction::FocusPane {
            pane_id: PaneId::new("pane-2"),
        })
        .unwrap();
    state.build_scene(POINTER_FRAME);
    state
}

// [L5-GATE] Child mouse capture on: a click over the child's grid is
// forwarded to its PTY as mouse bytes and steals no focus.
#[test]
fn child_capture_forwards_clicks_to_pty_without_focus_steal() {
    let mut state = live_state_with_capturing_child();
    let pane_1 = PaneId::new("pane-1");
    assert_eq!(focused(&state), "pane-2");

    // Click inside pane-1's body: inner rect starts at (1, 2), so the
    // click at (2, 3) is grid cell (1, 1) -> SGR "\x1b[<0;2;2M".
    send_pointer(&mut state, left(PointerKind::Down, 2, 3));
    send_pointer(&mut state, left(PointerKind::Up, 2, 3));

    assert_eq!(focused(&state), "pane-2", "click must not steal focus");
    // The shell's line editor echoes the forwarded SGR press/release
    // back as visible text (minus the escape prefix it consumed), so
    // the bytes reaching the PTY are observable in the child's grid.
    let echoed = pump_runtime_until(&mut state, |state| {
        grid_text(state, &pane_1).contains("0;2;2M")
    });
    assert!(
        echoed,
        "forwarded mouse press never reached the child's PTY; grid: {}",
        grid_text(&state, &pane_1)
    );

    state.shutdown();
}

// [L5-GATE] alt+click is always explicit workspace control, even over a
// mouse-capturing child.
#[test]
fn alt_click_is_workspace_control_despite_child_capture() {
    let mut state = live_state_with_capturing_child();

    send_pointer(
        &mut state,
        PointerEvent {
            mods: Modifiers::ALT,
            ..left(PointerKind::Down, 2, 3)
        },
    );

    assert_eq!(focused(&state), "pane-1", "alt+click must focus the pane");

    state.shutdown();
}

// [L5-GATE] Child capture off: the workspace handles clicks (focus).
#[test]
fn clicks_are_workspace_control_when_child_does_not_capture() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    state.dispatch(CommandId::SplitRight);
    assert_eq!(focused(&state), "pane-2");
    state.build_scene(POINTER_FRAME);
    let pane_1 = PaneId::new("pane-1");
    assert!(
        !state
            .terminal_panes
            .get(&pane_1)
            .unwrap()
            .parser
            .mouse_mode()
            .wants_mouse()
    );

    send_pointer(&mut state, left(PointerKind::Down, 2, 3));

    assert_eq!(focused(&state), "pane-1");
    assert!(!grid_text(&state, &pane_1).contains("0;2;2M"));

    state.shutdown();
}

#[test]
fn wheel_scrolls_terminal_scrollback_and_returns_to_live() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    state.write_to_focused_terminal(
        b"i=1; while [ $i -le 60 ]; do echo LINE_$i; i=$((i+1)); done\r",
    );
    let scrolled = pump_runtime_until(&mut state, |state| {
        state
            .terminal_panes
            .get(&pane_id)
            .is_some_and(|runtime| runtime.parser.grid().scrollback_len() > 10)
    });
    assert!(scrolled, "shell output never reached scrollback");
    state.build_scene(POINTER_FRAME);

    // Wheel up over the pane body scrolls into history without copy mode.
    send_pointer(
        &mut state,
        pointer_event(PointerKind::Wheel { dx: 0, dy: -1 }, None, 5, 5),
    );
    send_pointer(
        &mut state,
        pointer_event(PointerKind::Wheel { dx: 0, dy: -1 }, None, 5, 5),
    );
    assert!(!state.copy_mode_active());
    assert_eq!(state.pane_view_state(&pane_id).scroll_offset, 6);
    assert!(state.status().contains("scrollback"));

    // Wheel down returns to following live output.
    send_pointer(
        &mut state,
        pointer_event(PointerKind::Wheel { dx: 0, dy: 2 }, None, 5, 5),
    );
    assert_eq!(state.pane_view_state(&pane_id).scroll_offset, 0);
    assert!(state.pointer_view.is_none());
    assert!(state.status().contains("following live output"));

    state.shutdown();
}

#[test]
fn pointer_drag_selects_cells_and_copy_selection_copies_them() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    state.handle_event(InputEvent::Paste("echo SELECT_ME\r".to_owned()));
    // Wait for the output line: ends with the marker but is not the
    // echoed command line (which contains "echo").
    let printed = pump_runtime_until(&mut state, |state| {
        state.terminal_panes.get(&pane_id).is_some_and(|runtime| {
            runtime
                .parser
                .grid()
                .snapshot()
                .iter()
                .any(|line| line.trim_end().ends_with("SELECT_ME") && !line.contains("echo"))
        })
    });
    assert!(
        printed,
        "marker output never reached the grid; rows:\n{}",
        grid_text(&state, &pane_id)
    );
    state.build_scene(POINTER_FRAME);

    // Locate the echoed marker in the visible grid: pane-1 inner rect
    // starts at (1, 2), and with no scrollback the visible row N is
    // screen row 2 + N.
    let snapshot = state
        .terminal_panes
        .get(&pane_id)
        .unwrap()
        .parser
        .grid()
        .snapshot();
    let (grid_row, line) = snapshot
        .iter()
        .enumerate()
        .find(|(_, line)| line.trim_end().ends_with("SELECT_ME") && !line.contains("echo"))
        .expect("marker row visible");
    assert_eq!(
        state
            .terminal_panes
            .get(&pane_id)
            .unwrap()
            .parser
            .grid()
            .scrollback_len(),
        0
    );
    let start_column = line.find("SELECT_ME").unwrap() as u16;
    let screen_row = 2 + grid_row as u16;
    let screen_start = 1 + start_column;

    // Drag across the marker; releasing keeps the selection visible.
    send_pointer(
        &mut state,
        left(PointerKind::Down, screen_start, screen_row),
    );
    send_pointer(
        &mut state,
        left(PointerKind::Drag, screen_start + 8, screen_row),
    );
    send_pointer(
        &mut state,
        left(PointerKind::Up, screen_start + 8, screen_row),
    );
    let view = state.pane_view_state(&pane_id);
    assert!(view.selection.is_some(), "selection survives release");
    assert!(
        view.copy_cursor.is_none(),
        "pointer selection has no cursor"
    );
    assert!(!state.copy_mode_active());

    // Copy Selection stages the OSC 52 payload with the selected text.
    state.dispatch(CommandId::CopySelection);
    assert_eq!(state.last_copied(), Some("SELECT_ME"));
    let payload = state.take_clipboard_payload().expect("payload staged");
    assert!(payload.starts_with(b"\x1b]52;c;"));
    assert!(state.pane_view_state(&pane_id).selection.is_none());

    state.shutdown();
}

#[test]
fn plain_click_clears_selection_and_typing_still_reaches_the_shell() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    // The typed-marker proof below relies on pure kernel echo (no \r is
    // sent), which a mid-init shell can have disabled.
    wait_for_shell_ready(&mut state, &pane_id);
    state.build_scene(POINTER_FRAME);

    // Drag a selection, then plain-click: the selection clears.
    send_pointer(&mut state, left(PointerKind::Down, 5, 5));
    send_pointer(&mut state, left(PointerKind::Drag, 12, 5));
    send_pointer(&mut state, left(PointerKind::Up, 12, 5));
    assert!(state.pane_view_state(&pane_id).selection.is_some());
    send_pointer(&mut state, left(PointerKind::Down, 5, 6));
    send_pointer(&mut state, left(PointerKind::Up, 5, 6));
    assert!(state.pane_view_state(&pane_id).selection.is_none());

    // Selection is not a mode: keys still flow to the child (L5). The
    // proof is end-to-end — the typed marker echoes in the child's grid
    // (byte-count diagnostics no longer surface in the status line).
    send_pointer(&mut state, left(PointerKind::Down, 5, 5));
    send_pointer(&mut state, left(PointerKind::Drag, 12, 5));
    send_pointer(&mut state, left(PointerKind::Up, 12, 5));
    for character in "TYPEDMARK".chars() {
        state.handle_key(key(KeyCode::Char(character)));
    }
    let echoed = pump_runtime_until(&mut state, |state| {
        grid_text(state, &pane_id).contains("TYPEDMARK")
    });
    assert!(echoed, "typed keys never reached the child's PTY");

    state.shutdown();
}

#[test]
fn agent_pane_context_menu_offers_approval_decisions() {
    let mut state = state();
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
            "appr-1",
            "rm -rf target",
        ))),
        FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![AgentSessionEvent::Completed {
                summary: "cleaned".to_owned(),
            }],
            then_on_reject: vec![],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        state
            .agent_runtime_view(&pane_id)
            .is_some_and(|runtime| runtime.pending_approval.is_some())
    });
    assert!(observed, "approval request was not observed");

    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let scene = state.build_scene(POINTER_FRAME);
    let agent_pane = scene.panes.iter().find(|pane| pane.floating).unwrap();
    let inner = mandatum_scene::layout::pane_inner_rect(agent_pane.area);

    send_pointer(&mut state, right_down(inner.x + 1, inner.y + 1));

    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::ContextMenu(menu)) = &scene.overlay else {
        panic!("right-click on a waiting agent pane must open the menu");
    };
    let items: Vec<(&str, &str)> = menu
        .items
        .iter()
        .map(|item| (item.label.as_str(), item.chord_hint.as_str()))
        .collect();
    assert!(items.contains(&("Approve agent action", "y")));
    assert!(items.contains(&("Reject agent action", "n")));
    assert!(
        menu.items.iter().any(|item| item.label == "Stop agent"),
        "a live session offers Stop agent"
    );

    // Down past the "Command palette" gateway row to Approve, then
    // Enter decides the approval.
    let mut approved = false;
    for _ in 0..300 {
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Enter));
        if state.status().starts_with("approved") {
            approved = true;
            break;
        }
        // The fake connector's worker may not have parked on the
        // approval yet; reopen the menu and retry.
        state.tick_runtime();
        state.build_scene(POINTER_FRAME);
        if state.context_menu.is_none() {
            send_pointer(&mut state, right_down(inner.x + 1, inner.y + 1));
            state.build_scene(POINTER_FRAME);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(approved, "menu approval never applied: {}", state.status());

    state.shutdown();
}

#[test]
fn restore_spawns_fresh_live_runtime_and_clears_runtime_presentation_state() {
    let temp = TestWorkspaceDir::new();
    let saved_workspace = Workspace::new("Restored", temp.project_path());
    write_workspace_file(&temp.workspace_file(), &saved_workspace).unwrap();

    let mut state = AppState::new(temp.app_config(true, false));
    state.handle_terminal_resize(80, 24);
    assert_eq!(state.live_terminal_count(), 1);

    let pane_id = PaneId::new("pane-1");
    let before_pid = state
        .terminal_panes
        .get(&pane_id)
        .unwrap()
        .controller
        .process_id();
    state.dispatch(CommandId::EnterCopyMode);
    state.clipboard_payload = Some(b"pending-clipboard".to_vec());
    state.last_copied = Some("copied text".to_owned());

    state.dispatch(CommandId::RestoreWorkspace);

    assert_eq!(state.live_terminal_count(), 1);
    let after_pid = state
        .terminal_panes
        .get(&pane_id)
        .unwrap()
        .controller
        .process_id();
    assert_ne!(before_pid, after_pid);
    assert!(!state.copy_mode_active());
    assert!(state.take_clipboard_payload().is_none());
    assert!(state.last_copied().is_none());

    state.shutdown();
}

#[test]
fn restart_replaces_live_runtime_for_same_pane() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    assert_eq!(state.live_terminal_count(), 1);

    let pane_id = PaneId::new("pane-1");
    let before = state.terminal_panes.get(&pane_id).unwrap();
    assert_eq!(before.restart_generation, 0);
    let before_pid = before.controller.process_id();

    state.dispatch(CommandId::RestartPane);

    // The same pane identity still has exactly one live runtime, now tracking
    // the bumped restart generation with a fresh child process.
    assert_eq!(state.live_terminal_count(), 1);
    let after = state.terminal_panes.get(&pane_id).unwrap();
    assert_eq!(after.restart_generation, 1);
    assert_ne!(before_pid, after.controller.process_id());
    assert_eq!(
        state.workspace().active_session().panes().len(),
        1,
        "restart must not change core layout"
    );
    assert!(state.status().contains("restarted shell"));

    state.shutdown();
}

// [L3-GATE] Events from a replaced runtime are rejected.
#[test]
fn old_reader_events_after_restart_are_ignored() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");

    state.dispatch(CommandId::RestartPane);
    state
        .event_tx
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation: 0,
                runtime_token: 0,
                bytes: b"OLD_READER_OUTPUT".to_vec(),
            },
            None,
        ))
        .unwrap();
    state.tick_runtime();

    let rendered = state
        .terminal_panes
        .get(&pane_id)
        .unwrap()
        .parser
        .grid()
        .snapshot()
        .join("\n");
    assert!(
        !rendered.contains("OLD_READER_OUTPUT"),
        "old pre-restart output was applied to the fresh runtime"
    );

    state.shutdown();
}

#[test]
fn old_reader_terminal_close_and_error_events_after_restart_are_ignored() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    let before = state.terminal_panes.get(&pane_id).unwrap();
    let before_generation = before.restart_generation;
    let before_token = before.runtime_token;

    state.dispatch(CommandId::RestartPane);
    state
        .event_tx
        .send(AppEvent::Pty(
            PtyRuntimeEvent::ReaderClosed {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
            },
            None,
        ))
        .unwrap();
    state
        .event_tx
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Error {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
                message: "STALE_TERMINAL_READER_ERROR".to_owned(),
            },
            None,
        ))
        .unwrap();
    state.tick_runtime();

    let after = state.terminal_panes.get(&pane_id).unwrap();
    assert_ne!(before_token, after.runtime_token);
    assert!(after.error.is_none());
    assert!(!state.status().contains("STALE_TERMINAL_READER_ERROR"));

    state.shutdown();
}

#[test]
fn enter_copy_mode_without_live_terminal_is_a_noop() {
    let mut state = state(); // spawn_pty = false, so no runtimes exist
    state.dispatch(CommandId::EnterCopyMode);
    assert!(!state.copy_mode_active());
    assert!(state.status().contains("no live terminal"));
}

#[test]
fn copy_mode_enters_selects_and_copies_to_clipboard() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);

    // Enter copy mode through the palette command path.
    state.dispatch(CommandId::EnterCopyMode);
    assert!(state.copy_mode_active());

    // Start a selection and copy it; copy mode exits and stages an OSC 52
    // clipboard payload for the run loop to write.
    state.handle_key(key(KeyCode::Char('v')));
    state.handle_key(key(KeyCode::Char('y')));
    assert!(!state.copy_mode_active());
    assert!(state.last_copied().is_some());

    let payload = state
        .take_clipboard_payload()
        .expect("clipboard payload staged");
    assert_eq!(payload.first(), Some(&0x1b));
    assert!(payload.starts_with(b"\x1b]52;c;"));

    state.shutdown();
}

#[test]
fn copy_mode_input_does_not_reach_the_shell() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    state.dispatch(CommandId::EnterCopyMode);

    // A normal character key in copy mode is navigation, not shell input.
    state.handle_key(key(KeyCode::Char('j')));
    assert!(state.copy_mode_active());
    assert!(!state.status().contains("sent"));

    state.shutdown();
}

#[test]
fn live_pane_survives_resize_and_tracks_new_geometry() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    let first_size = state.terminal_panes.get(&pane_id).unwrap().size;

    state.handle_terminal_resize(120, 40);

    // The same live runtime survived and the PTY tracked the new geometry.
    assert_eq!(state.live_terminal_count(), 1);
    let runtime = state.terminal_panes.get(&pane_id).unwrap();
    assert_ne!(
        first_size, runtime.size,
        "PTY size should follow pane geometry"
    );
    assert!(runtime.error.is_none(), "resize must not error the runtime");
    assert_eq!(state.workspace().active_session().panes().len(), 1);

    state.shutdown();
}

#[test]
fn exited_child_is_surfaced_as_visible_status() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");

    // Ask the shell to exit, then pump the runtime until the exit is observed.
    state.write_to_focused_terminal(b"exit\r");
    let mut observed = false;
    for _ in 0..300 {
        state.tick_runtime();
        if state
            .terminal_panes
            .get(&pane_id)
            .and_then(|runtime| runtime.exit_status)
            .is_some()
        {
            observed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(observed, "child process exit was not observed");
    assert!(
        state.status().contains("exited"),
        "exit must be visible in status, got {:?}",
        state.status()
    );

    state.shutdown();
}

#[test]
fn run_task_launches_configured_shell_command_and_surfaces_success_status() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'TASK_OK\\n'".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);

    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    assert_eq!(state.live_task_count(), 1);
    let pane = state.workspace().active_session().pane(&pane_id).unwrap();
    let PaneKind::Task { intent } = pane.kind() else {
        panic!("run task should create a task pane");
    };
    assert_eq!(intent.command, "printf 'TASK_OK\\n'");
    assert!(state.status().contains("running"));

    let observed = pump_runtime_until(&mut state, |state| {
        state.task_panes.get(&pane_id).is_some_and(|task| {
            task.runtime.exit_status.is_some()
                && task
                    .runtime
                    .parser
                    .grid()
                    .snapshot()
                    .join("\n")
                    .contains("TASK_OK")
        })
    });

    assert!(observed, "task success output/status was not observed");
    let task = state.task_panes.get(&pane_id).unwrap();
    assert_eq!(task.status, "succeeded: exit 0");
    assert!(state.status().contains("succeeded: exit 0"));

    state.shutdown();
}

#[test]
fn run_task_surfaces_nonzero_exit_as_failure_status() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'TASK_FAIL\\n'; exit 7".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);

    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        state
            .task_panes
            .get(&pane_id)
            .is_some_and(|task| task.status == "failed: exit 7")
    });

    assert!(observed, "task failure status was not observed");
    assert!(state.status().contains("task"));
    assert!(state.status().contains("failed: exit 7"));

    state.shutdown();
}

#[test]
fn hidden_task_launch_stays_pending_until_task_pane_becomes_visible() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'PENDING_TASK_OK\\n'".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);
    state.dispatch(CommandId::SplitRight);
    state.dispatch(CommandId::ZoomPane);
    assert!(
        state
            .workspace()
            .active_session()
            .layout()
            .zoomed()
            .is_some()
    );

    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    assert_eq!(state.live_task_count(), 0);
    assert!(state.task_panes.pending_launches.contains(&pane_id));
    assert_eq!(
        state.task_panes.statuses.get(&pane_id).map(String::as_str),
        Some("pending launch: waiting for visible pane size")
    );

    state.dispatch(CommandId::ZoomPane);

    let observed = pump_runtime_until(&mut state, |state| {
        state.task_panes.get(&pane_id).is_some_and(|task| {
            task.status == "succeeded: exit 0"
                && task
                    .runtime
                    .parser
                    .grid()
                    .snapshot()
                    .join("\n")
                    .contains("PENDING_TASK_OK")
        })
    });

    assert!(observed, "pending task did not launch when visible");
    assert!(!state.task_panes.pending_launches.contains(&pane_id));
    assert!(!state.task_panes.statuses.contains_key(&pane_id));

    state.shutdown();
}

#[test]
fn task_spawn_failure_sets_nonserialized_runtime_status_for_task_pane() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.shell_program = "/definitely/missing/mandatum-shell".to_owned();
    config.task_command = "printf SHOULD_NOT_RUN".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);

    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    assert_eq!(state.live_task_count(), 0);
    assert!(
        state
            .task_panes
            .statuses
            .get(&pane_id)
            .is_some_and(|status| status.contains("task launch failed"))
    );
    assert!(state.status().contains("task launch failed"));

    state.dispatch(CommandId::SaveWorkspace);
    let saved = fs::read_to_string(state.workspace_file()).expect("workspace file saved");
    assert!(saved.contains(r#""type": "task""#));
    assert!(!saved.contains("task launch failed"));
    assert!(!saved.contains("task_statuses"));

    state.shutdown();
}

#[test]
fn restart_pane_is_blocked_for_task_panes_because_rerun_is_explicit() {
    let mut state = state();
    state.dispatch(CommandId::RunTask);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let before_generation = state
        .workspace()
        .active_session()
        .pane(&pane_id)
        .unwrap()
        .restart_generation();

    state.dispatch(CommandId::RestartPane);

    let after_generation = state
        .workspace()
        .active_session()
        .pane(&pane_id)
        .unwrap()
        .restart_generation();
    assert_eq!(after_generation, before_generation);
    assert!(state.status().contains("Rerun Task"));
}

#[test]
fn rerun_task_replaces_live_runtime_for_same_task_pane_and_ignores_old_events() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'TASK_ORIGINAL\\n'; sleep 5".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);

    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let before = state.task_panes.get(&pane_id).unwrap();
    let before_token = before.runtime.runtime_token;
    let before_generation = before.runtime.restart_generation;
    let pane_count = state.workspace().active_session().panes().len();

    state.task_command = "printf 'TASK_CHANGED\\n'; sleep 5".to_owned();
    state.dispatch(CommandId::RerunTask);

    assert_eq!(state.workspace().active_session().panes().len(), pane_count);
    assert_eq!(state.live_task_count(), 1);
    let after = state.task_panes.get(&pane_id).unwrap();
    assert_ne!(before_token, after.runtime.runtime_token);
    assert_eq!(before_generation, after.runtime.restart_generation);
    let PaneKind::Task { intent } = state
        .workspace()
        .active_session()
        .pane(&pane_id)
        .unwrap()
        .kind()
    else {
        panic!("focused pane should still be a task pane");
    };
    assert_eq!(intent.command, "printf 'TASK_ORIGINAL\\n'; sleep 5");

    state
        .event_tx
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
                bytes: b"OLD_TASK_OUTPUT".to_vec(),
            },
            None,
        ))
        .unwrap();

    let observed = pump_runtime_until(&mut state, |state| {
        state.task_panes.get(&pane_id).is_some_and(|task| {
            task.runtime
                .parser
                .grid()
                .snapshot()
                .join("\n")
                .contains("TASK_ORIGINAL")
        })
    });

    assert!(observed, "rerun task output was not observed");
    let rendered = state
        .task_panes
        .get(&pane_id)
        .unwrap()
        .runtime
        .parser
        .grid()
        .snapshot()
        .join("\n");
    assert!(!rendered.contains("OLD_TASK_OUTPUT"));
    assert!(!rendered.contains("TASK_CHANGED"));

    state.shutdown();
}

#[test]
fn hidden_task_rerun_stays_pending_until_task_pane_becomes_visible() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'HIDDEN_RERUN_OK\\n'; sleep 5".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);

    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    assert_eq!(state.live_task_count(), 1);
    let before = state.task_panes.get(&pane_id).unwrap();
    let before_token = before.runtime.runtime_token;
    let before_generation = before.runtime.restart_generation;
    let PaneKind::Task { intent } = state
        .workspace()
        .active_session()
        .pane(&pane_id)
        .unwrap()
        .kind()
    else {
        panic!("run task should create a task pane");
    };
    let command = intent.command.clone();

    state
        .workspace
        .apply_action(CoreAction::FocusPane {
            pane_id: PaneId::new("pane-1"),
        })
        .unwrap();
    state.dispatch(CommandId::ZoomPane);
    state
        .workspace
        .apply_action(CoreAction::FocusPane {
            pane_id: pane_id.clone(),
        })
        .unwrap();
    assert!(state.visible_task_size(&pane_id).is_none());

    state.dispatch(CommandId::RerunTask);

    assert_eq!(state.live_task_count(), 0);
    assert!(state.task_panes.pending_launches.contains(&pane_id));
    assert_eq!(
        state.task_panes.statuses.get(&pane_id).map(String::as_str),
        Some("pending rerun: waiting for visible pane size")
    );
    let pane = state.workspace().active_session().pane(&pane_id).unwrap();
    assert_eq!(pane.restart_generation(), before_generation);
    let PaneKind::Task { intent } = pane.kind() else {
        panic!("focused pane should still be a task pane");
    };
    assert_eq!(intent.command, command);

    state
        .event_tx
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
                bytes: b"OLD_HIDDEN_RERUN_OUTPUT".to_vec(),
            },
            None,
        ))
        .unwrap();
    state.tick_runtime();
    assert_eq!(
        state.task_panes.statuses.get(&pane_id).map(String::as_str),
        Some("pending rerun: waiting for visible pane size")
    );

    state.dispatch(CommandId::ZoomPane);

    let observed = pump_runtime_until(&mut state, |state| {
        state.task_panes.get(&pane_id).is_some_and(|task| {
            task.runtime
                .parser
                .grid()
                .snapshot()
                .join("\n")
                .contains("HIDDEN_RERUN_OK")
        })
    });

    assert!(observed, "pending hidden rerun did not launch when visible");
    assert!(!state.task_panes.pending_launches.contains(&pane_id));
    assert!(!state.task_panes.statuses.contains_key(&pane_id));
    let rendered = state
        .task_panes
        .get(&pane_id)
        .unwrap()
        .runtime
        .parser
        .grid()
        .snapshot()
        .join("\n");
    assert!(!rendered.contains("OLD_HIDDEN_RERUN_OUTPUT"));

    state.shutdown();
}

#[test]
fn restored_task_pane_stays_inert_until_explicit_rerun() {
    let temp = TestWorkspaceDir::new();
    let mut save_config = temp.app_config(false, false);
    save_config.task_command = "printf 'RESTORED_TASK_OK\\n'".to_owned();
    let mut saved_state = AppState::new(save_config);
    saved_state.dispatch(CommandId::RunTask);
    saved_state.dispatch(CommandId::SaveWorkspace);
    drop(saved_state);

    let mut state = AppState::new(temp.app_config(true, true));
    state.handle_terminal_resize(100, 35);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    assert_eq!(state.live_task_count(), 0);
    assert!(!state.task_panes.pending_launches.contains(&pane_id));

    state.dispatch(CommandId::RerunTask);

    let observed = pump_runtime_until(&mut state, |state| {
        state.task_panes.get(&pane_id).is_some_and(|task| {
            task.status == "succeeded: exit 0"
                && task
                    .runtime
                    .parser
                    .grid()
                    .snapshot()
                    .join("\n")
                    .contains("RESTORED_TASK_OK")
        })
    });

    assert!(
        observed,
        "restored task did not rerun after explicit command"
    );

    state.shutdown();
}

#[test]
fn stop_task_terminates_live_runtime_and_surfaces_nonserialized_status() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'TASK_RUNNING\\n'; sleep 5".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);
    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let task = state.task_panes.get(&pane_id).unwrap();
    let restart_generation = task.runtime.restart_generation;
    let runtime_token = task.runtime.runtime_token;

    state.dispatch(CommandId::StopTask);

    assert_eq!(state.live_task_count(), 0);
    assert_eq!(
        state.task_panes.statuses.get(&pane_id).map(String::as_str),
        Some("stopped")
    );
    assert!(state.status().contains("stopped"));

    state
        .event_tx
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Error {
                pane_id: pane_id.clone(),
                restart_generation,
                runtime_token,
                message: "late reader error".to_owned(),
            },
            None,
        ))
        .unwrap();
    state.tick_runtime();
    assert_eq!(
        state.task_panes.statuses.get(&pane_id).map(String::as_str),
        Some("stopped")
    );

    state.dispatch(CommandId::SaveWorkspace);
    let saved = fs::read_to_string(state.workspace_file()).expect("workspace file saved");
    assert!(saved.contains(r#""type": "task""#));
    assert!(!saved.contains("stopped"));
    assert!(!saved.contains("task_statuses"));
    assert!(!saved.contains("runtime_token"));

    state.shutdown();
}

#[test]
fn stop_task_clears_pending_hidden_launch() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'SHOULD_NOT_RUN\\n'".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);
    state.dispatch(CommandId::SplitRight);
    state.dispatch(CommandId::ZoomPane);
    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    assert!(state.task_panes.pending_launches.contains(&pane_id));

    state.dispatch(CommandId::StopTask);

    assert!(!state.task_panes.pending_launches.contains(&pane_id));
    assert_eq!(
        state.task_panes.statuses.get(&pane_id).map(String::as_str),
        Some("stopped before launch")
    );

    state.dispatch(CommandId::ZoomPane);
    for _ in 0..30 {
        state.tick_runtime();
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(state.live_task_count(), 0);
    assert_eq!(
        state.task_panes.statuses.get(&pane_id).map(String::as_str),
        Some("stopped before launch")
    );

    state.shutdown();
}

#[test]
fn late_task_reader_closed_event_does_not_overwrite_exit_status() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "exit 0".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);
    state.dispatch(CommandId::RunTask);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        state
            .task_panes
            .get(&pane_id)
            .is_some_and(|task| task.status == "succeeded: exit 0")
    });
    assert!(observed, "task success status was not observed");

    let task = state.task_panes.get(&pane_id).unwrap();
    state
        .event_tx
        .send(AppEvent::Pty(
            PtyRuntimeEvent::ReaderClosed {
                pane_id: pane_id.clone(),
                restart_generation: task.runtime.restart_generation,
                runtime_token: task.runtime.runtime_token,
            },
            None,
        ))
        .unwrap();
    state.tick_runtime();

    assert_eq!(
        state.task_panes.get(&pane_id).unwrap().status,
        "succeeded: exit 0"
    );

    state.shutdown();
}

// [L3-GATE] Live runtime state never becomes durable truth.
#[test]
fn task_runtime_state_is_not_serialized_with_workspace_intent() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'TASK_PERSIST_OK\\n'".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);
    state.dispatch(CommandId::RunTask);
    assert_eq!(state.live_task_count(), 1);

    state.dispatch(CommandId::SaveWorkspace);

    let saved = fs::read_to_string(state.workspace_file()).expect("workspace file saved");
    assert!(saved.contains(r#""type": "task""#));
    assert!(saved.contains(r#""command": "printf 'TASK_PERSIST_OK\\n'""#));
    for forbidden in [
        "task_panes",
        "runtime_token",
        "NativePty",
        "process_id",
        "reader_thread",
        "parser",
        "exit_status",
        "scrollback",
        r#""status":"#,
    ] {
        assert!(
            !saved.contains(forbidden),
            "saved workspace leaked task runtime field {forbidden}"
        );
    }

    state.shutdown();
}

// --- Agent runtime -----------------------------------------------------

use mandatum_agent_runtime::{
    AgentConnectorError, AgentSession, ApprovalRequest, ApprovalScope, FakeConnector, FakeStep,
    FileChange, FileChangeKind, RiskAssessment, RiskLevel,
};

fn approval_request(id: &str, command: &str) -> ApprovalRequest {
    ApprovalRequest {
        approval_id: id.to_owned(),
        command: command.to_owned(),
        scope: ApprovalScope {
            cwd: PathBuf::from("/tmp/project"),
            affected_path: Some(PathBuf::from("target")),
        },
        risk: RiskAssessment {
            level: RiskLevel::High,
            basis: "removes files (rm)".to_owned(),
        },
    }
}

fn agent_intent(state: &AppState, pane_id: &PaneId) -> mandatum_core::AgentPaneIntent {
    let PaneKind::Agent { intent } = state
        .workspace()
        .active_session()
        .pane(pane_id)
        .expect("agent pane exists")
        .kind()
    else {
        panic!("pane {pane_id} is not an agent pane");
    };
    intent.clone()
}

/// Dispatch an approve/reject command until the decision lands. The fake
/// connector's worker may not have parked on its approval yet when the
/// requesting event arrives, so a decision can race it once.
fn dispatch_decision_until_applied(state: &mut AppState, command_id: CommandId) {
    for _ in 0..300 {
        state.dispatch(command_id);
        if state.status().starts_with("approved") || state.status().starts_with("rejected") {
            return;
        }
        state.tick_runtime();
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "approval decision was never applied, last status: {}",
        state.status()
    );
}

#[test]
fn start_agent_creates_pane_with_default_objective_and_updates_status_through_events() {
    let mut state = state();
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::Summary("exploring the repo".to_owned())),
        FakeStep::Emit(AgentSessionEvent::FilesChanged(vec![FileChange {
            path: PathBuf::from("src/lib.rs"),
            change_kind: FileChangeKind::Modified,
        }])),
        FakeStep::Emit(AgentSessionEvent::Completed {
            summary: "agent run done".to_owned(),
        }),
    ])));

    // No agent pane exists: StartAgent creates one with the configured
    // default objective, then launches it.
    state.dispatch(CommandId::StartAgent);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let intent = agent_intent(&state, &pane_id);
    assert_eq!(intent.objective, "test objective");
    assert_eq!(intent.status, AgentStatus::Running);
    assert_eq!(state.live_agent_count(), 1);

    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::Complete
    });
    assert!(observed, "agent completion was not observed");
    let intent = agent_intent(&state, &pane_id);
    assert_eq!(intent.latest_summary.as_deref(), Some("agent run done"));
    assert_eq!(intent.changed_files, vec![PathBuf::from("src/lib.rs")]);

    state.shutdown();
}

#[test]
fn approve_agent_action_resolves_and_the_script_continues() {
    let mut state = state();
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
            "appr-1",
            "rm -rf target",
        ))),
        FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![
                AgentSessionEvent::CommandRun {
                    command: "rm -rf target".to_owned(),
                },
                AgentSessionEvent::Completed {
                    summary: "cleaned".to_owned(),
                },
            ],
            then_on_reject: vec![AgentSessionEvent::Failed {
                error: "user rejected".to_owned(),
            }],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();

    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::WaitingForApproval
    });
    assert!(observed, "approval request was not observed");
    let intent = agent_intent(&state, &pane_id);
    assert_eq!(intent.pending_approvals, 1);
    assert_eq!(intent.pending_approval_ids, vec!["appr-1".to_owned()]);

    dispatch_decision_until_applied(&mut state, CommandId::ApproveAgentAction);

    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::Complete
    });
    assert!(observed, "script did not continue after approval");
    let intent = agent_intent(&state, &pane_id);
    assert_eq!(intent.pending_approvals, 0);
    assert!(intent.pending_approval_ids.is_empty());
    assert_eq!(
        intent.approval_history,
        vec![AgentApprovalRecord {
            approval_id: "appr-1".to_owned(),
            command: "rm -rf target".to_owned(),
            approved: true,
        }]
    );
    assert_eq!(intent.latest_summary.as_deref(), Some("cleaned"));

    state.shutdown();
}

#[test]
fn reject_agent_action_via_direct_key_records_the_rejection() {
    let mut state = state();
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
            "appr-1",
            "rm -rf target",
        ))),
        FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![AgentSessionEvent::Completed {
                summary: "cleaned".to_owned(),
            }],
            then_on_reject: vec![AgentSessionEvent::Failed {
                error: "user rejected".to_owned(),
            }],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();

    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::WaitingForApproval
    });
    assert!(observed, "approval request was not observed");

    // The focused pane awaits an approval: a bare 'n' key is the direct
    // reject path, no palette involved.
    let mut rejected = false;
    for _ in 0..300 {
        state.handle_key(key(KeyCode::Char('n')));
        if state.status().starts_with("rejected") {
            rejected = true;
            break;
        }
        state.tick_runtime();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(rejected, "direct reject key never applied");

    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::Failed
    });
    assert!(observed, "reject branch was not observed");
    let intent = agent_intent(&state, &pane_id);
    assert_eq!(
        intent.approval_history,
        vec![AgentApprovalRecord {
            approval_id: "appr-1".to_owned(),
            command: "rm -rf target".to_owned(),
            approved: false,
        }]
    );

    state.shutdown();
}

#[test]
fn stop_agent_shuts_down_the_live_session() {
    let mut state = state();
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::AwaitApproval {
            approval_id: "appr-never".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::Running
    });
    assert!(observed);
    assert_eq!(state.live_agent_count(), 1);

    state.dispatch(CommandId::StopAgent);

    assert_eq!(state.live_agent_count(), 0);
    assert_eq!(agent_intent(&state, &pane_id).status, AgentStatus::Unknown);
    assert!(state.status().contains("stopped"));

    // The buffered Closed event from the killed session is dropped.
    state.tick_runtime();
    assert_eq!(state.live_agent_count(), 0);
}

// [L3-GATE] Events from a replaced agent runtime are rejected.
#[test]
fn stale_agent_events_after_restart_are_ignored() {
    let mut state = state();
    let script = vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::AwaitApproval {
            approval_id: "appr-never".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        },
    ];
    state.set_agent_connector(Box::new(FakeConnector::new(script)));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let before = state.agent_runtime_view(&pane_id).unwrap();
    let before_generation = before.restart_generation;
    let before_token = before.runtime_token;

    // Kill the runtime, then restart: the replacement runs under a new
    // generation and token.
    state.dispatch(CommandId::StartAgent);
    let after = state.agent_runtime_view(&pane_id).unwrap();
    assert_ne!(before_token, after.runtime_token);
    assert!(after.restart_generation > before_generation);

    // A stale buffered event from the killed session must be dropped.
    state
        .event_tx
        .send(AppEvent::Agent(crate::agent_runtime::AgentRuntimeEvent {
            pane_id: pane_id.clone(),
            restart_generation: before_generation,
            runtime_token: before_token,
            event: AgentSessionEvent::Summary("STALE_AGENT_SUMMARY".to_owned()),
        }))
        .unwrap();
    state.tick_runtime();

    assert_ne!(
        agent_intent(&state, &pane_id).latest_summary.as_deref(),
        Some("STALE_AGENT_SUMMARY"),
        "a stale pre-restart agent event was applied to durable intent"
    );

    state.shutdown();
}

#[test]
fn agent_intent_with_approval_history_survives_save_restore_round_trip() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(false, false));
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::FilesChanged(vec![FileChange {
            path: PathBuf::from("src/lib.rs"),
            change_kind: FileChangeKind::Modified,
        }])),
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
            "appr-1",
            "rm -rf target",
        ))),
        FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![AgentSessionEvent::Completed {
                summary: "cleaned".to_owned(),
            }],
            then_on_reject: vec![],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::WaitingForApproval
    });
    assert!(observed);
    dispatch_decision_until_applied(&mut state, CommandId::ApproveAgentAction);
    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::Complete
    });
    assert!(observed);

    state.dispatch(CommandId::SaveWorkspace);
    state.shutdown();
    drop(state);

    let restored = AppState::new(temp.app_config(false, true));
    assert!(restored.status().contains("workspace restored"));
    let intent = agent_intent(&restored, &pane_id);
    assert_eq!(intent.objective, "test objective");
    assert_eq!(intent.status, AgentStatus::Complete);
    assert_eq!(intent.latest_summary.as_deref(), Some("cleaned"));
    assert_eq!(intent.changed_files, vec![PathBuf::from("src/lib.rs")]);
    // Past decisions remain visible after restart.
    assert_eq!(
        intent.approval_history,
        vec![AgentApprovalRecord {
            approval_id: "appr-1".to_owned(),
            command: "rm -rf target".to_owned(),
            approved: true,
        }]
    );
    // Restore invents no live runtime.
    assert_eq!(restored.live_agent_count(), 0);
}

// [L3-GATE] Live agent runtime state never becomes durable truth.
#[test]
fn agent_runtime_state_is_not_serialized_with_workspace_intent() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(false, false));
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::Action {
            description: "LIVE_ACTION_MARKER".to_owned(),
        }),
        FakeStep::Emit(AgentSessionEvent::OutputChunk(
            "LIVE_TAIL_MARKER".to_owned(),
        )),
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
            "appr-live",
            "rm -rf LIVE_ONLY_COMMAND",
        ))),
        FakeStep::AwaitApproval {
            approval_id: "appr-live".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::WaitingForApproval
    });
    assert!(observed);

    state.dispatch(CommandId::SaveWorkspace);

    let saved = fs::read_to_string(state.workspace_file()).expect("workspace file saved");
    assert!(saved.contains(r#""type": "agent""#));
    assert!(saved.contains("test objective"));
    // The pending approval id is durable; its live detail is not.
    assert!(saved.contains("appr-live"));
    for forbidden in [
        "LIVE_ACTION_MARKER",
        "LIVE_TAIL_MARKER",
        "LIVE_ONLY_COMMAND",
        "output_tail",
        "current_action",
        "runtime_token",
        "forwarder",
        "removes files (rm)",
    ] {
        assert!(
            !saved.contains(forbidden),
            "saved workspace leaked agent runtime field {forbidden}"
        );
    }

    state.shutdown();
}

#[test]
fn focus_next_waiting_agent_jumps_to_the_waiting_pane() {
    let mut state = state();
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
            "appr-1",
            "rm -rf target",
        ))),
        FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let waiting_pane = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &waiting_pane).status == AgentStatus::WaitingForApproval
    });
    assert!(observed);

    // Move focus away, then jump back to the waiting agent.
    state
        .workspace_mut()
        .apply_action(CoreAction::FocusPane {
            pane_id: PaneId::new("pane-1"),
        })
        .unwrap();
    state.dispatch(CommandId::FocusNextWaitingAgent);

    assert_eq!(
        state.workspace().active_session().focused_pane_id(),
        &waiting_pane
    );
    assert!(state.status().contains("focused waiting agent"));

    state.shutdown();
}

#[test]
fn new_agent_pane_creates_a_draft_pane_without_launching_a_runtime() {
    let mut state = state();

    state.dispatch(CommandId::NewAgentPane);

    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let intent = agent_intent(&state, &pane_id);
    assert_eq!(intent.objective, "test objective");
    assert_eq!(intent.status, AgentStatus::Draft);
    assert_eq!(state.live_agent_count(), 0);
    assert!(state.status().contains("agent pane"));
}

/// Succeeds on the first launch (delegating to a fake script), fails
/// every launch after it — models a relaunch attempt that cannot spawn.
struct FailsSecondLaunch {
    inner: FakeConnector,
    launches: AtomicU64,
}

impl AgentConnector for FailsSecondLaunch {
    fn launch(&self, spec: &AgentLaunchSpec) -> Result<AgentSession, AgentConnectorError> {
        if self.launches.fetch_add(1, Ordering::SeqCst) == 0 {
            self.inner.launch(spec)
        } else {
            Err(AgentConnectorError::LaunchFailed {
                message: "relaunch refused".to_owned(),
            })
        }
    }

    fn name(&self) -> &str {
        "fails-second-launch"
    }
}

// [L3-GATE] A failed relaunch must not retire the live session's
// generation: the previous session stays authoritative, and the pane's
// core generation keeps matching the generation of accepted events.
#[test]
fn failed_relaunch_keeps_the_previous_session_authoritative() {
    let mut state = state();
    state.set_agent_connector(Box::new(FailsSecondLaunch {
        inner: FakeConnector::new(vec![
            FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
            FakeStep::AwaitApproval {
                approval_id: "appr-never".to_owned(),
                then_on_approve: vec![],
                then_on_reject: vec![],
            },
        ]),
        launches: AtomicU64::new(0),
    }));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::Running
    });
    assert!(observed);
    let generation_before = state
        .agent_runtime_view(&pane_id)
        .unwrap()
        .restart_generation;

    state.dispatch(CommandId::StartAgent);

    assert!(
        state.status().contains("relaunch failed"),
        "unexpected status: {}",
        state.status()
    );
    assert_eq!(state.live_agent_count(), 1);
    let runtime = state.agent_runtime_view(&pane_id).unwrap();
    assert_eq!(runtime.restart_generation, generation_before);
    assert_eq!(
        state.pane_restart_generation(&pane_id),
        runtime.restart_generation,
        "pane generation diverged from the live runtime's generation"
    );
    // Durable truth keeps reflecting the still-live previous session.
    assert_eq!(agent_intent(&state, &pane_id).status, AgentStatus::Running);

    state.shutdown();
}

// [L3-GATE] Pending-approval claims are live-session state: a workspace
// loaded from disk has no live session behind it, so a restore must not
// resurrect them as actionable durable truth.
#[test]
fn restore_detaches_live_session_claims_from_agent_intents() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(false, false));
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
            "appr-live",
            "rm -rf target",
        ))),
        FakeStep::AwaitApproval {
            approval_id: "appr-live".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::WaitingForApproval
    });
    assert!(observed);

    state.dispatch(CommandId::SaveWorkspace);
    state.shutdown();
    drop(state);

    let restored = AppState::new(temp.app_config(false, true));
    assert!(restored.status().contains("workspace restored"));
    assert_eq!(restored.live_agent_count(), 0);
    let intent = agent_intent(&restored, &pane_id);
    // A surviving claim would drive real behavior (FocusNextWaitingAgent,
    // y/n keys) toward an approval no runtime can ever satisfy.
    assert_eq!(intent.status, AgentStatus::Unknown);
    assert_eq!(intent.pending_approvals, 0);
    assert!(intent.pending_approval_ids.is_empty());
}

// [L3-GATE] OpenProject discards the live agent session; the pane left
// behind in the now-inactive session must not keep claiming "running".
#[test]
fn open_project_shuts_down_the_agent_and_detaches_its_durable_claim() {
    let mut state = state();
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::AwaitApproval {
            approval_id: "appr-never".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        },
    ])));
    state.dispatch(CommandId::StartAgent);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let observed = pump_runtime_until(&mut state, |state| {
        agent_intent(state, &pane_id).status == AgentStatus::Running
    });
    assert!(observed);
    assert_eq!(state.live_agent_count(), 1);
    let old_session_id = state.workspace().active_session().id().clone();

    state.dispatch(CommandId::OpenProject);

    assert_ne!(state.workspace().active_session().id(), &old_session_id);
    assert_eq!(state.live_agent_count(), 0);
    let old_session = state
        .workspace()
        .sessions()
        .get(&old_session_id)
        .expect("the replaced session stays in the workspace");
    let PaneKind::Agent { intent } = old_session
        .pane(&pane_id)
        .expect("agent pane persists in the old session")
        .kind()
    else {
        panic!("pane {pane_id} is not an agent pane");
    };
    assert_eq!(intent.status, AgentStatus::Unknown);
    assert_eq!(intent.pending_approvals, 0);
    assert!(intent.pending_approval_ids.is_empty());

    state.shutdown();
}

// --- Visibility slice: timeline, session map, attention, objective ----

/// An isolated state whose timeline writes into its own temp dir.
fn isolated_state(temp: &TestWorkspaceDir) -> AppState {
    AppState::new(temp.app_config(false, false))
}

fn timeline_overlay_of(state: &mut AppState) -> mandatum_scene::TimelineOverlay {
    let scene = state.build_scene(SceneSize::new(120, 40));
    match scene.overlay {
        Some(mandatum_scene::OverlayScene::Timeline(timeline)) => timeline,
        other => panic!("expected the timeline overlay, got {other:?}"),
    }
}

#[test]
fn timeline_records_dispatches_filters_and_jumps_to_the_named_pane() {
    let temp = TestWorkspaceDir::new();
    let mut state = isolated_state(&temp);
    state.dispatch(CommandId::SplitRight); // creates + focuses pane-2
    state.dispatch(CommandId::FocusPrevious); // back to pane-1

    state.dispatch(CommandId::ShowTimeline);
    let overlay = timeline_overlay_of(&mut state);
    // Newest first: the show-timeline dispatch itself leads.
    assert!(
        overlay.items[0].text.contains("show-timeline"),
        "{:?}",
        overlay.items[0].text
    );
    assert_eq!(overlay.skipped_malformed, 0);
    // The durable log holds the split, the created pane, and the focus
    // moves.
    let texts: Vec<&str> = overlay
        .items
        .iter()
        .map(|item| item.text.as_str())
        .collect();
    assert!(texts.iter().any(|text| text.contains("split-right")));
    assert!(
        texts
            .iter()
            .any(|text| text.contains("pane pane-2 created (terminal)"))
    );

    // Structured filtering narrows to the pane-creation fact.
    for character in "kind:pane pane:pane-2".chars() {
        state.handle_key(key(KeyCode::Char(character)));
    }
    let overlay = timeline_overlay_of(&mut state);
    assert_eq!(overlay.items.len(), 1, "{:?}", overlay.items);
    assert!(overlay.items[0].text.contains("pane-2 created"));
    assert!(!overlay.items[0].when.is_empty());

    // Enter jumps focus to the pane the fact names and closes the
    // overlay.
    state.handle_key(key(KeyCode::Enter));
    assert_eq!(focused(&state), "pane-2");
    let scene = state.build_scene(SceneSize::new(120, 40));
    assert!(scene.overlay.is_none());
    assert!(state.status().contains("focused pane-2"));
}

#[test]
fn timeline_survives_restarts_because_the_log_is_durable() {
    let temp = TestWorkspaceDir::new();
    {
        let mut first = isolated_state(&temp);
        first.dispatch(CommandId::SplitRight);
    }
    // A fresh app over the same project reads the previous run's facts.
    let mut second = isolated_state(&temp);
    second.dispatch(CommandId::ShowTimeline);
    let overlay = timeline_overlay_of(&mut second);
    assert!(
        overlay
            .items
            .iter()
            .any(|item| item.text.contains("split-right")),
        "facts recorded before the restart must still be readable"
    );
}

// --- Session search ----------------------------------------------------

fn search_overlay_of(state: &mut AppState) -> mandatum_scene::SearchOverlay {
    let scene = state.build_scene(SceneSize::new(120, 40));
    match scene.overlay {
        Some(mandatum_scene::OverlayScene::Search(search)) => search,
        other => panic!("expected the search overlay, got {other:?}"),
    }
}

fn type_into_search(state: &mut AppState, text: &str) {
    for character in text.chars() {
        state.handle_key(key(KeyCode::Char(character)));
    }
}

#[test]
fn search_opens_from_command_and_chord_stays_calm_on_zero_hits_and_esc_returns() {
    let temp = TestWorkspaceDir::new();
    let mut state = isolated_state(&temp);

    // The default chord opens it (the palette letter and menu row are
    // alternate doors to the same command).
    state.handle_key(parse_chord("ctrl+shift+f").unwrap());
    let overlay = search_overlay_of(&mut state);
    assert_eq!(overlay.query, "");
    assert!(overlay.items.is_empty(), "empty query matches nothing");
    assert!(overlay.footer.contains("enter jump · esc close"));
    assert!(state.status().contains("search: snapshot"));

    // Zero hits stay calm: Enter reports, the overlay stays open.
    type_into_search(&mut state, "zzqxv");
    state.handle_key(key(KeyCode::Enter));
    assert!(state.status().contains("no output matches 'zzqxv'"));
    let overlay = search_overlay_of(&mut state);
    assert!(overlay.items.is_empty());

    // Esc returns to the workspace.
    state.handle_key(key(KeyCode::Escape));
    let scene = state.build_scene(SceneSize::new(120, 40));
    assert!(scene.overlay.is_none());
    assert_eq!(state.status(), "search closed");

    // The context menu offers the same command with its chord hint.
    let items = state.context_menu_items(&PaneId::new("pane-1"));
    let row = items
        .iter()
        .find(|item| item.label == "Search session output")
        .expect("the pane menu offers session search");
    assert_eq!(row.hint, "ctrl+shift+f");
}

#[test]
fn search_timeline_hits_open_the_timeline_at_the_matched_entry() {
    let temp = TestWorkspaceDir::new();
    let mut state = isolated_state(&temp);
    state.dispatch(CommandId::SplitRight); // records dispatch + pane-created

    state.dispatch(CommandId::SearchSession);
    type_into_search(&mut state, "kind:timeline created");
    let overlay = search_overlay_of(&mut state);
    assert!(!overlay.items.is_empty());
    assert_eq!(overlay.items[0].source, "timeline");
    assert!(overlay.items[0].text.contains("pane pane-2 created"));
    assert_eq!(overlay.items[0].pane, None);

    state.handle_key(key(KeyCode::Enter));
    assert!(
        state
            .status()
            .contains("timeline opened at the matched event")
    );
    let timeline = timeline_overlay_of(&mut state);
    let selected = timeline.selected.expect("an entry is selected");
    assert!(
        timeline.items[selected]
            .text
            .contains("pane pane-2 created"),
        "{:?}",
        timeline.items[selected].text
    );
}

#[test]
fn search_jumps_a_terminal_pane_to_the_matched_scrollback_row() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(true, false));
    state.handle_terminal_resize(100, 30);
    // Print a marker, then bury it in scrollback with filler lines.
    state.handle_event(InputEvent::Paste(
        "echo SEARCH_MARK_XYZ; i=1; while [ $i -le 60 ]; do echo filler_$i; i=$((i+1)); done\r"
            .to_owned(),
    ));
    assert!(
        pump_runtime_until(&mut state, |state| {
            grid_text(state, &PaneId::new("pane-1")).contains("filler_60")
        }),
        "shell output did not arrive"
    );

    state.dispatch(CommandId::SearchSession);
    type_into_search(&mut state, "SEARCH_MARK_XYZ");
    let overlay = search_overlay_of(&mut state);
    assert!(!overlay.items.is_empty());
    assert_eq!(overlay.items[0].pane, Some(PaneId::new("pane-1")));

    state.handle_key(key(KeyCode::Enter));
    assert!(
        state.status().contains("jumped to the matched row"),
        "{}",
        state.status()
    );
    // The overlay closed, the pane is focused, and its viewport now
    // shows the matched row (scrolled up from the live bottom).
    let scene = state.build_scene(SceneSize::new(100, 30));
    assert!(scene.overlay.is_none());
    assert_eq!(focused(&state), "pane-1");
    let pane = scene
        .panes
        .iter()
        .find(|pane| pane.id == PaneId::new("pane-1"))
        .expect("pane-1 in scene");
    let mandatum_scene::PaneContent::Terminal(surface) = &pane.content else {
        panic!("pane-1 must carry a terminal surface");
    };
    assert!(surface.scroll_offset > 0, "viewport must leave the bottom");
    let visible: String = surface
        .rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.character).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        visible.contains("SEARCH_MARK_XYZ"),
        "the matched row must be inside the viewport:\n{visible}"
    );
    // The matched span is selected, so the hit is visibly marked.
    assert!(surface.selection.is_some());

    state.shutdown();
}

#[test]
fn search_results_stay_stable_and_jumps_clamp_while_a_pane_floods() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(true, false));
    state.handle_terminal_resize(100, 30);
    state.handle_event(InputEvent::Paste("echo FLOOD_TARGET_ABC\r".to_owned()));
    assert!(pump_runtime_until(&mut state, |state| {
        grid_text(state, &PaneId::new("pane-1")).contains("FLOOD_TARGET_ABC")
    }));

    // Snapshot, then flood the pane past the scrollback bound so the
    // matched row's absolute coordinates are evicted.
    state.dispatch(CommandId::SearchSession);
    type_into_search(&mut state, "FLOOD_TARGET_ABC");
    let before = search_overlay_of(&mut state);
    assert!(!before.items.is_empty());
    // While the overlay is open a paste edits the query, so the flood
    // is written straight to the child's PTY — exactly a child that
    // keeps producing output while the user reads search results.
    state.write_to_focused_terminal(b"seq 1 2200\r");
    let ring_full = |state: &AppState| {
        state
            .terminal_panes
            .get(&PaneId::new("pane-1"))
            .is_some_and(|runtime| {
                runtime.parser.grid().scrollback_len() >= runtime.parser.grid().scrollback_limit()
            })
    };
    // A dedicated deadline: 2200 lines through a real PTY can outlast
    // the standard pump budget on a loaded machine.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ring_full(&state) && Instant::now() < deadline {
        state.tick_runtime();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ring_full(&state),
        "the flood never filled the scrollback ring"
    );

    // Results are a snapshot: the flood changes nothing on screen.
    let after = search_overlay_of(&mut state);
    assert_eq!(before.items, after.items);
    assert_eq!(before.overflow, after.overflow);

    // Enter still lands calmly: the row's text moved, so the jump says
    // so instead of pretending — and never panics.
    state.handle_key(key(KeyCode::Enter));
    assert!(
        state
            .status()
            .contains("output moved since the search snapshot"),
        "{}",
        state.status()
    );
    assert!(!state.should_quit());

    state.shutdown();
}

#[test]
fn search_snapshot_spans_agent_output_and_pane_filters_narrow_it() {
    use mandatum_agent_runtime::{FakeConnector, FakeStep};

    let temp = TestWorkspaceDir::new();
    let mut state = isolated_state(&temp);
    state.set_agent_connector(Box::new(FakeConnector::new(vec![FakeStep::Emit(
        AgentSessionEvent::OutputChunk("AGENT_NEEDLE_42 in the tail".to_owned()),
    )])));
    state.dispatch(CommandId::StartAgent);
    let agent_pane = state.workspace().active_session().focused_pane_id().clone();
    assert!(pump_runtime_until(&mut state, |state| {
        state
            .agent_runtime_view(&agent_pane)
            .is_some_and(|runtime| !runtime.output_tail.is_empty())
    }));

    state.dispatch(CommandId::SearchSession);
    type_into_search(&mut state, "AGENT_NEEDLE_42");
    let overlay = search_overlay_of(&mut state);
    assert!(!overlay.items.is_empty());
    assert!(overlay.items[0].source.contains("agent"));
    assert_eq!(overlay.items[0].pane, Some(agent_pane.clone()));

    // kind:/pane: filters narrow the same query.
    state.handle_key(key(KeyCode::Escape));
    state.dispatch(CommandId::SearchSession);
    type_into_search(&mut state, "kind:terminal AGENT_NEEDLE_42");
    let overlay = search_overlay_of(&mut state);
    assert!(
        overlay.items.is_empty(),
        "agent output must not match kind:terminal"
    );

    // Enter on an agent hit focuses the pane (tails have no viewport).
    state.handle_key(key(KeyCode::Escape));
    state.dispatch(CommandId::SearchSession);
    type_into_search(&mut state, &format!("pane:{agent_pane} NEEDLE"));
    let overlay = search_overlay_of(&mut state);
    assert!(!overlay.items.is_empty());
    state.handle_key(key(KeyCode::Enter));
    assert_eq!(focused(&state), agent_pane.as_str());
    assert!(
        state.status().contains("shows the tail"),
        "{}",
        state.status()
    );

    state.shutdown();
}

#[test]
fn search_rows_are_clickable_and_click_away_dismisses() {
    let temp = TestWorkspaceDir::new();
    let mut state = isolated_state(&temp);
    state.handle_terminal_resize(120, 40);
    state.dispatch(CommandId::SplitRight);

    // Rows carry hit targets aligned with the drawn window; a click on
    // the first row activates it like Enter.
    state.dispatch(CommandId::SearchSession);
    type_into_search(&mut state, "kind:timeline created");
    let scene = state.build_scene(SceneSize::new(120, 40));
    let target = scene
        .hit_targets
        .iter()
        .find(|target| matches!(target.kind, HitTargetKind::SearchItem(0)))
        .expect("the first search row must be clickable")
        .clone();
    state.handle_event(InputEvent::Pointer(PointerEvent {
        kind: PointerKind::Down,
        button: Some(PointerButton::Left),
        column: target.rect.x,
        row: target.rect.y,
        mods: Modifiers::NONE,
    }));
    assert!(
        state
            .status()
            .contains("timeline opened at the matched event"),
        "{}",
        state.status()
    );

    // Click-away dismisses the reopened overlay.
    state.handle_key(key(KeyCode::Escape));
    state.dispatch(CommandId::SearchSession);
    state.build_scene(SceneSize::new(120, 40));
    state.handle_event(InputEvent::Pointer(PointerEvent {
        kind: PointerKind::Down,
        button: Some(PointerButton::Left),
        column: 0,
        row: 0,
        mods: Modifiers::NONE,
    }));
    let scene = state.build_scene(SceneSize::new(120, 40));
    assert!(scene.overlay.is_none());
    assert_eq!(state.status(), "search closed");
}

#[test]
fn session_map_navigates_and_focuses_across_sessions() {
    let temp = TestWorkspaceDir::new();
    let mut state = isolated_state(&temp);
    state.dispatch(CommandId::SplitRight); // session-1: pane-1, pane-2
    state.dispatch(CommandId::OpenProject); // session-2 (active): pane-1

    state.dispatch(CommandId::ShowSessionMap);
    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::SessionMap(map)) = &scene.overlay else {
        panic!("session map must be open");
    };
    // Tree: session-1, its two panes, session-2 (active), its pane.
    assert_eq!(map.rows.len(), 5);
    assert!(map.rows[3].label.contains("(active)"));
    // The active session's focused pane starts selected.
    assert_eq!(map.selected, 4);
    assert!(map.rows[4].focused);

    // Walk up to session-1's pane-2 and Enter: the active session
    // switches and focus lands on that pane.
    state.handle_key(key(KeyCode::Up));
    state.handle_key(key(KeyCode::Up));
    state.handle_key(key(KeyCode::Enter));

    assert_eq!(
        state.workspace().active_session().id().as_str(),
        "session-1"
    );
    assert_eq!(focused(&state), "pane-2");
    let scene = state.build_scene(POINTER_FRAME);
    assert!(scene.overlay.is_none(), "the map closes after the jump");

    // Rows are clickable too: reopen and click session-2's pane row.
    state.dispatch(CommandId::ShowSessionMap);
    let scene = state.build_scene(POINTER_FRAME);
    let row_target = scene
        .hit_targets
        .iter()
        .find(|target| target.kind == HitTargetKind::SessionMapRow(4))
        .expect("session-map rows must be hit targets");
    send_pointer(
        &mut state,
        left(PointerKind::Down, row_target.rect.x + 1, row_target.rect.y),
    );
    assert_eq!(
        state.workspace().active_session().id().as_str(),
        "session-2"
    );
}

#[test]
fn objective_prompt_round_trips_into_durable_intent_and_the_next_launch() {
    let temp = TestWorkspaceDir::new();
    let mut state = isolated_state(&temp);
    state.dispatch(CommandId::NewAgentPane);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();

    // The prompt opens pre-filled with the current objective.
    state.dispatch(CommandId::SetAgentObjective);
    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::Prompt(prompt)) = &scene.overlay else {
        panic!("the objective prompt must be open");
    };
    assert_eq!(prompt.input, "test objective");

    // Edit it: clear, retype, Enter.
    for _ in 0.."test objective".len() {
        state.handle_key(key(KeyCode::Backspace));
    }
    for character in "ship the demo".chars() {
        state.handle_key(key(KeyCode::Char(character)));
    }
    state.handle_key(key(KeyCode::Enter));

    let PaneKind::Agent { intent } = state
        .workspace()
        .active_session()
        .pane(&pane_id)
        .unwrap()
        .kind()
    else {
        panic!("pane must be an agent pane");
    };
    assert_eq!(intent.objective, "ship the demo");
    assert!(state.status().contains("objective set"));

    // The edit is a durable timeline fact, and the next launch uses it.
    state.dispatch(CommandId::ShowTimeline);
    let overlay = timeline_overlay_of(&mut state);
    assert!(
        overlay
            .items
            .iter()
            .any(|item| item.text.contains("objective set: ship the demo"))
    );
    state.handle_key(key(KeyCode::Escape));

    state.dispatch(CommandId::StartAgent);
    assert!(
        state.status().contains("started: ship the demo"),
        "{}",
        state.status()
    );
    state.shutdown();
}

#[test]
fn empty_objective_is_rejected_and_escape_cancels_without_changes() {
    let temp = TestWorkspaceDir::new();
    let mut state = isolated_state(&temp);
    state.dispatch(CommandId::NewAgentPane);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    state.dispatch(CommandId::SetAgentObjective);

    for _ in 0.."test objective".len() {
        state.handle_key(key(KeyCode::Backspace));
    }
    state.handle_key(key(KeyCode::Enter));
    assert!(state.status().contains("objective cannot be empty"));
    let scene = state.build_scene(POINTER_FRAME);
    assert!(
        matches!(scene.overlay, Some(mandatum_scene::OverlayScene::Prompt(_))),
        "an empty commit keeps the prompt open"
    );

    state.handle_key(key(KeyCode::Escape));
    let PaneKind::Agent { intent } = state
        .workspace()
        .active_session()
        .pane(&pane_id)
        .unwrap()
        .kind()
    else {
        panic!("pane must be an agent pane");
    };
    assert_eq!(intent.objective, "test objective", "cancel changes nothing");
}

#[test]
fn attention_segment_click_jumps_to_the_waiting_pane() {
    let mut state = state();
    let mut waiting = AgentPaneIntent::draft("needs approval");
    waiting.status = AgentStatus::WaitingForApproval;
    state
        .workspace_mut()
        .active_session_mut()
        .add_floating_pane("agent", PaneKind::Agent { intent: waiting }, None);
    state.dispatch(CommandId::FocusPrevious); // back to pane-1
    frame(&mut state);

    let scene = state.build_scene(POINTER_FRAME);
    let segment = scene
        .hit_targets
        .iter()
        .find(|target| matches!(target.kind, HitTargetKind::AttentionSegment { .. }))
        .expect("a waiting approval must produce a clickable header segment");
    send_pointer(
        &mut state,
        left(PointerKind::Down, segment.rect.x, segment.rect.y),
    );

    assert_eq!(focused(&state), "pane-2");
}

// A live shell sitting at a prompt is not "running" anything: the
// session map labels it "open" (exit states and task "running" keep
// their words).
#[test]
fn session_map_labels_a_live_shell_open_not_running() {
    let mut state = live_state();
    state.handle_terminal_resize(100, 30);
    assert_eq!(state.live_terminal_count(), 1);

    let rows = state.session_map_row_models();
    let shell_row = rows
        .iter()
        .find(|model| {
            matches!(
                &model.target,
                SessionMapTarget::Pane { pane_id, .. } if pane_id == &PaneId::new("pane-1")
            )
        })
        .expect("the live shell has a session-map row");
    assert_eq!(shell_row.row.state, "open");

    state.shutdown();
}

// The failed-task attention segment is a jump too: one click lands on
// the failing pane.
#[test]
fn attention_failed_task_segment_click_jumps_to_the_failed_pane() {
    let mut state = state();
    state.dispatch(CommandId::RunTask);
    let task_pane = state.workspace().active_session().focused_pane_id().clone();
    state.set_task_status_for_test(&task_pane, "failed: exit 3");
    state.dispatch(CommandId::FocusPrevious); // look away from the task
    assert_ne!(focused(&state), task_pane.as_str());
    frame(&mut state);

    let scene = state.build_scene(POINTER_FRAME);
    let segment = scene
        .hit_targets
        .iter()
        .find(|target| {
            matches!(
                &target.kind,
                HitTargetKind::AttentionSegment { pane: Some(pane), .. } if pane == &task_pane
            )
        })
        .expect("a failed task must produce a clickable header segment");
    send_pointer(
        &mut state,
        left(PointerKind::Down, segment.rect.x, segment.rect.y),
    );

    assert_eq!(focused(&state), task_pane.as_str());
}
