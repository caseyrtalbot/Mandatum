use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;
use crate::{LoadedConfig, keymap::parse_chord};
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

#[test]
fn ime_preedit_is_transient_and_commit_updates_the_locked_palette_once() {
    let mut state = state();
    state.handle_key(ctrl('p'));
    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "界e\u{301}".into(),
        cursor: Some(TextRange {
            start: 0,
            end: "界e\u{301}".len(),
        }),
    }));

    let scene = state.build_scene(POINTER_FRAME);
    let Some(mandatum_scene::OverlayScene::Palette(palette)) = &scene.overlay else {
        panic!("palette must stay open");
    };
    assert!(
        palette.query.is_empty(),
        "preedit is not committed filter text"
    );
    let text_input = scene.text_input.expect("palette text input scene");
    assert_eq!(
        text_input
            .preedit
            .as_ref()
            .map(|preedit| preedit.text.as_str()),
        Some("界e\u{301}")
    );

    // Winit clears the visible preedit immediately before Commit. The locked
    // target survives that empty event.
    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: String::new(),
        cursor: None,
    }));
    state.handle_event(InputEvent::Composition(CompositionEvent::Commit(
        "界e\u{301}".into(),
    )));
    let palette = state
        .palette_overlay(POINTER_FRAME)
        .expect("palette remains open");
    assert_eq!(palette.query, "界e\u{301}");
    assert_eq!(state.workspace().active_session().panes().len(), 1);
}

#[test]
fn ime_invalid_range_cancels_visually_without_mutating_the_target() {
    let mut state = state();
    state.handle_key(ctrl('p'));
    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "old".into(),
        cursor: None,
    }));
    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "é".into(),
        cursor: Some(TextRange { start: 1, end: 2 }),
    }));

    let scene = state.build_scene(POINTER_FRAME);
    assert!(
        scene
            .text_input
            .as_ref()
            .is_some_and(|input| input.preedit.is_none())
    );
    assert!(
        state
            .palette_overlay(POINTER_FRAME)
            .unwrap()
            .query
            .is_empty()
    );
    state.handle_event(InputEvent::Composition(CompositionEvent::Commit(
        "late".into(),
    )));
    assert!(
        state
            .palette_overlay(POINTER_FRAME)
            .unwrap()
            .query
            .is_empty(),
        "late commit after an invalid preedit is rejected"
    );
}

#[test]
fn ime_preedit_survives_resize_and_reanchors_to_the_resized_overlay() {
    let mut state = state();
    state.handle_key(ctrl('p'));
    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "界".into(),
        cursor: None,
    }));
    let before = state
        .build_scene(SceneSize::new(80, 24))
        .text_input
        .expect("preedit before resize");

    state.handle_event(InputEvent::Resize(SceneSize::new(120, 40)));
    let after = state
        .build_scene(SceneSize::new(120, 40))
        .text_input
        .expect("preedit after resize");

    assert_eq!(after.preedit, before.preedit);
    assert_ne!(after.area, before.area);
}

#[test]
fn composition_events_round_trip_through_the_neutral_input_contract() {
    let input = InputEvent::Composition(CompositionEvent::Preedit {
        text: "e\u{301}界".into(),
        cursor: Some(TextRange {
            start: 0,
            end: "e\u{301}".len(),
        }),
    });
    let encoded = serde_json::to_string(&input).expect("serialize composition input");
    let decoded: InputEvent =
        serde_json::from_str(&encoded).expect("deserialize composition input");
    assert_eq!(decoded, input);
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
        "[keymap]\nsplit-right = \"ctrl+alt+s\"\n\n\
         [theme]\nname = \"mandatum-light\"\n\n\
         [theme.terminal]\nforeground = \"#010203\"\nbackground = \"#040506\"\nbright-blue = \"#070809\"\n\n\
         [shell]\nprogram = \"/configured/shell\"\n\n\
         [task]\ndefault_command = \"configured-task\"\n\n\
         [agent]\nconnector = \"claude\"\nmodel = \"configured-model\"\n",
    )
    .unwrap();

    state.dispatch(CommandId::ReloadConfig);

    assert_eq!(state.status(), "config reloaded");
    assert_eq!(state.theme().name, "mandatum-light");
    assert_eq!(state.theme().terminal_palette.foreground, [1, 2, 3]);
    assert_eq!(state.theme().terminal_palette.background, [4, 5, 6]);
    assert_eq!(state.theme().terminal_palette.ansi[12], [7, 8, 9]);
    assert_eq!(state.shell_program, "/configured/shell");
    assert_eq!(state.task_command, "configured-task");
    assert_eq!(state.agent_connector_label(), "claude");
    assert_eq!(state.agent_model.as_deref(), Some("configured-model"));
    state.handle_key(Key::new(
        KeyCode::Char('s'),
        Modifiers {
            control: true,
            alt: true,
            ..Modifiers::NONE
        },
    ));
    assert_eq!(state.workspace().active_session().panes().len(), 2);

    // Deleting every override re-resolves the full product defaults instead
    // of leaving the previously loaded runtime settings sticky.
    fs::remove_file(&config_file).unwrap();
    state.dispatch(CommandId::ReloadConfig);
    let defaults = effective_runtime_settings(&LoadedConfig::default());
    assert_eq!(state.status(), "config reloaded");
    assert_eq!(state.keymap, Keymap::default());
    assert_eq!(state.theme().name, "mandatum-dark");
    assert_eq!(
        state.theme().terminal_palette,
        Theme::default().terminal_palette
    );
    assert_eq!(state.shell_program, defaults.shell_program);
    assert_eq!(state.task_command, defaults.task_command);
    assert_eq!(
        state.agent_connector_label(),
        connector_kind_label(defaults.agent_connector)
    );
    assert_eq!(state.agent_model, defaults.agent_model);

    // Load the overrides again so the malformed-file check proves it also
    // clears runtime settings rather than merely retaining existing defaults.
    fs::write(
        &config_file,
        "[shell]\nprogram = \"/configured/shell\"\n\n\
         [task]\ndefault_command = \"configured-task\"\n\n\
         [agent]\nconnector = \"fake\"\nmodel = \"configured-model\"\n",
    )
    .unwrap();
    state.dispatch(CommandId::ReloadConfig);
    assert_eq!(state.shell_program, "/configured/shell");
    assert_eq!(state.task_command, "configured-task");
    assert_eq!(state.agent_connector_label(), "fake");
    assert_eq!(state.agent_model.as_deref(), Some("configured-model"));

    // A now-broken config reloads onto defaults with the problem named.
    fs::write(&config_file, "{{ not toml").unwrap();
    state.dispatch(CommandId::ReloadConfig);
    assert!(state.status().starts_with("config reloaded;"));
    assert!(state.status().contains("not valid TOML"));
    assert_eq!(state.keymap, Keymap::default());
    assert_eq!(state.theme().name, "mandatum-dark");
    assert_eq!(
        state.theme().terminal_palette,
        Theme::default().terminal_palette
    );
    assert_eq!(state.shell_program, defaults.shell_program);
    assert_eq!(state.task_command, defaults.task_command);
    assert_eq!(
        state.agent_connector_label(),
        connector_kind_label(defaults.agent_connector)
    );
    assert_eq!(state.agent_model, defaults.agent_model);
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

fn separator_node(scene: &WorkspaceScene) -> &mandatum_scene::PresentationNode {
    scene
        .presentation
        .nodes
        .iter()
        .find(|node| node.role == mandatum_scene::PresentationNodeRole::Separator)
        .expect("separator presentation node")
}

#[test]
fn separator_presentation_tracks_hover_drag_and_focus_loss_without_pane_hover() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    frame(&mut state);

    let separator = state
        .build_scene(POINTER_FRAME)
        .hit_targets
        .into_iter()
        .find(|target| matches!(target.kind, HitTargetKind::Separator { .. }))
        .expect("split separator target");
    send_pointer(
        &mut state,
        pointer_event(PointerKind::Move, None, separator.rect.x, separator.rect.y),
    );
    let hovered = state.build_scene(POINTER_FRAME);
    assert!(separator_node(&hovered).state.hovered);
    assert_eq!(
        separator_node(&hovered).logical_rect.size.width_units(),
        64,
        "visible split rule is one logical pixel"
    );
    let logical_target = hovered
        .presentation
        .logical_hit_targets
        .iter()
        .find(|target| matches!(target.kind, HitTargetKind::Separator { .. }))
        .expect("logical separator target");
    assert_eq!(
        logical_target.logical_rect.size.width_units(),
        6 * 64,
        "logical hit target is six pixels while the cell target stays unchanged"
    );
    assert_eq!(separator.rect, SceneRect::new(49, 1, 2, 28));

    send_pointer(&mut state, pointer_event(PointerKind::Move, None, 10, 10));
    let pane_body = state.build_scene(POINTER_FRAME);
    assert!(!separator_node(&pane_body).state.hovered);
    assert!(
        pane_body
            .presentation
            .nodes
            .iter()
            .filter(|node| node.role == mandatum_scene::PresentationNodeRole::PaneBody)
            .all(|node| !node.state.hovered),
        "pane bodies never claim workspace hover"
    );

    send_pointer(
        &mut state,
        pointer_event(PointerKind::Move, None, separator.rect.x, separator.rect.y),
    );
    send_pointer(
        &mut state,
        left(PointerKind::Down, separator.rect.x, separator.rect.y),
    );
    let dragging = state.build_scene(POINTER_FRAME);
    assert!(separator_node(&dragging).state.hovered);
    assert!(separator_node(&dragging).state.dragging);

    send_pointer(
        &mut state,
        left(
            PointerKind::Drag,
            separator.rect.x.saturating_add(5),
            separator.rect.y,
        ),
    );
    let dragged = state.build_scene(POINTER_FRAME);
    assert!(separator_node(&dragged).state.dragging);

    send_pointer(
        &mut state,
        left(
            PointerKind::Up,
            separator.rect.x.saturating_add(5),
            separator.rect.y,
        ),
    );
    let released = state.build_scene(POINTER_FRAME);
    assert!(!separator_node(&released).state.dragging);

    state.handle_event(InputEvent::FocusLost);
    let unfocused = state.build_scene(POINTER_FRAME);
    assert!(!separator_node(&unfocused).state.hovered);
    assert!(!separator_node(&unfocused).state.dragging);
}

#[test]
fn native_logical_pointer_uses_six_pixel_separator_target_with_cell_terminal_fallback() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    frame(&mut state);
    let scene = state.build_scene(POINTER_FRAME);
    let logical_target = scene
        .presentation
        .logical_hit_targets
        .iter()
        .find(|target| matches!(target.kind, HitTargetKind::Separator { .. }))
        .expect("logical separator target");
    let logical_position = mandatum_scene::LogicalPoint::from_units(
        logical_target.logical_rect.origin.x_units()
            + (logical_target.logical_rect.size.width_units() / 2) as i64,
        logical_target.logical_rect.origin.y_units() + 64,
    );

    send_pointer(&mut state, pointer_event(PointerKind::Move, None, 10, 10));
    assert_eq!(
        state.hovered_separator(),
        None,
        "the terminal-compatible cell fallback remains outside the split target"
    );
    state.handle_pointer_at_logical(
        pointer_event(PointerKind::Move, None, 10, 10),
        logical_position,
    );

    assert_eq!(state.hovered_separator(), Some(0));
    assert!(
        !state.pointer_move_needs_redraw(),
        "separator hover uses identity comparison instead of continuous redraw"
    );
}

#[test]
fn pointer_leave_clears_idle_separator_hover_but_preserves_active_drag() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    frame(&mut state);
    let separator = state
        .hit_targets
        .iter()
        .find(|target| matches!(target.kind, HitTargetKind::Separator { .. }))
        .expect("split separator target")
        .clone();

    send_pointer(
        &mut state,
        pointer_event(PointerKind::Move, None, separator.rect.x, separator.rect.y),
    );
    let hovered_generation = state.scene_generation();
    assert!(state.pointer_left());
    assert_eq!(state.hovered_separator(), None);
    assert!(!state.pointer_move_needs_redraw());
    assert!(
        state.scene_generation() > hovered_generation,
        "clearing the hover highlight is scene-visible and must bump the generation, \
         or the skip guard leaves the separator painted in its hovered tone"
    );
    let cleared_generation = state.scene_generation();
    assert!(!state.pointer_left());
    assert_eq!(
        state.scene_generation(),
        cleared_generation,
        "a leave with nothing hovered changes no scene state"
    );

    send_pointer(
        &mut state,
        left(PointerKind::Down, separator.rect.x, separator.rect.y),
    );
    let drag_generation = state.scene_generation();
    assert!(!state.pointer_left());
    assert_eq!(state.hovered_separator(), Some(0));
    assert_eq!(
        state.scene_generation(),
        drag_generation,
        "a leave during an active drag preserves the hover and the generation"
    );
}

// A surface-recovery suspend cancels any live pointer selection/hover; that
// mutation is scene-visible, so it must bump the generation — the forced
// post-recovery render keys the prepared-scene cache on the generation, and
// an unchanged one would repaint the cancelled selection.
#[test]
fn suspend_scene_interaction_bumps_generation_only_when_it_cancels_state() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    state.build_scene(POINTER_FRAME);

    send_pointer(&mut state, left(PointerKind::Down, 5, 5));
    send_pointer(&mut state, left(PointerKind::Drag, 12, 5));
    assert!(
        state.pane_view_state(&pane_id).selection.is_some(),
        "the drag must have a live selection to cancel"
    );
    let before = state.scene_generation();
    state.suspend_scene_interaction();
    assert!(state.pane_view_state(&pane_id).selection.is_none());
    assert!(
        state.scene_generation() > before,
        "cancelling a live drag selection must bump the scene generation"
    );

    let idle = state.scene_generation();
    state.suspend_scene_interaction();
    assert_eq!(
        state.scene_generation(),
        idle,
        "a suspend with nothing to cancel changes no scene state"
    );
    state.shutdown();
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
        PointerKind::Wheel {
            dx: 0,
            dy: 1,
            precise: false,
        },
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
    assert!(scene.presentation.motion_policy.direct_geometry);
    assert!(
        scene.presentation.transition_targets.is_empty(),
        "live split drag snaps every presentation transition"
    );
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

    state.last_pane_click = None;
    let scene = state.build_scene(POINTER_FRAME);
    let title = scene
        .hit_targets
        .iter()
        .find(|target| {
            matches!(
                &target.kind,
                HitTargetKind::PaneTitle(pane_id) if pane_id == &PaneId::new("pane-2")
            )
        })
        .unwrap()
        .rect;
    send_pointer(&mut state, left(PointerKind::Down, title.x + 1, title.y));
    send_pointer(&mut state, left(PointerKind::Drag, u16::MAX, u16::MAX));
    send_pointer(&mut state, left(PointerKind::Up, u16::MAX, u16::MAX));
    let rect = &state.workspace().active_session().layout().floating()[0].rect;
    assert_eq!((rect.x, rect.y), (97, 25));
    let scene = state.build_scene(POINTER_FRAME);
    let floating = scene.panes.iter().find(|pane| pane.floating).unwrap();
    assert_eq!((floating.area.width, floating.area.height), (3, 3));
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
            "Adjust appearance",
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

    // Down to "Zoom pane" (index 8), then Enter runs it.
    for _ in 0..8 {
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

    // Click the "Zoom pane" row (index 8) through its hit target.
    let zoom_row = scene
        .hit_targets
        .iter()
        .find(|target| target.kind == HitTargetKind::ContextMenuItem(8))
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

#[test]
fn either_half_of_each_two_row_overlay_target_resolves_to_the_same_item() {
    let mut state = state();
    state.handle_key(ctrl('p'));
    let scene = state.build_scene(POINTER_FRAME);
    let targets = scene
        .hit_targets
        .iter()
        .filter(|target| matches!(target.kind, HitTargetKind::PaletteItem(_)))
        .take(2)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(targets.len(), 2);
    for target in targets {
        assert_eq!(
            target.rect.height,
            mandatum_scene::layout::OVERLAY_CONTROL_ROWS
        );
        for row in target.rect.y..target.rect.bottom() {
            assert_eq!(
                state
                    .pointer_target(target.rect.x.saturating_add(1), row)
                    .map(|resolved| resolved.kind),
                Some(target.kind.clone())
            );
        }
    }
}

#[test]
fn palette_rows_track_pointer_hover_like_the_context_menu() {
    let mut state = state();
    state.handle_key(ctrl('p'));
    let scene = state.build_scene(POINTER_FRAME);
    let (index, rect) = scene
        .hit_targets
        .iter()
        .find_map(|target| match target.kind {
            HitTargetKind::PaletteItem(index) if index != 0 => Some((index, target.rect)),
            _ => None,
        })
        .expect("palette exposes a row beyond the initial selection");

    send_pointer(
        &mut state,
        pointer_event(PointerKind::Move, None, rect.x.saturating_add(1), rect.y),
    );
    assert_eq!(
        state.palette.as_ref().expect("palette stays open").selected,
        index,
        "the highlight follows the pointer onto the row"
    );
    assert!(
        state.build_scene(POINTER_FRAME).overlay.is_some(),
        "hover only moves the highlight; rows run on press alone"
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
    send_pointer(&mut state, right_down(95, 5));
    let viewport = mandatum_scene::ViewportMetrics::new(
        mandatum_scene::LogicalSize::from_pixels(960.0, 480.0).unwrap(),
        mandatum_scene::PhysicalSize::new(1920, 960),
        mandatum_scene::BackingScale::new(2.0).unwrap(),
        mandatum_scene::LogicalSize::from_pixels(8.0, 16.0).unwrap(),
    )
    .unwrap();
    let scene = state.build_scene_with_viewport(viewport);
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
    send_pointer(&mut state, left(PointerKind::Down, 0, 29));
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
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: 2,
                precise: false,
            },
            None,
            50,
            15,
        ),
    );
    assert_eq!(
        state.palette_overlay(POINTER_FRAME).unwrap().selected,
        Some(2)
    );
    send_pointer(
        &mut state,
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: -1,
                precise: false,
            },
            None,
            50,
            15,
        ),
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

// [L5-GATE] Shift+Tab reaches the child unless an explicit workspace chord
// intercepts the same physical key.
#[test]
fn shift_tab_reaches_the_child_unless_a_workspace_chord_intercepts() {
    let shift_only = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    // Crossterm reports Shift+Tab as BackTab + SHIFT. Keep a plain BackTab
    // valid for other frontend adapters, and accept Tab + SHIFT as the
    // equivalent neutral representation.
    for shifted_tab in [
        Key::new(KeyCode::BackTab, shift_only),
        key(KeyCode::BackTab),
        Key::new(KeyCode::Tab, shift_only),
    ] {
        assert_eq!(
            key_to_input(shifted_tab),
            RuntimeInput::SendToTerminal(b"\x1b[Z".to_vec())
        );
    }

    // A crossterm BackTab event must still honor an explicit workspace chord
    // written in the natural `ctrl+shift+tab` form before terminal fallback.
    let mut keymap = Keymap::default();
    keymap.bind_chord(
        CommandId::FocusPrevious,
        parse_chord("ctrl+shift+tab").unwrap(),
    );
    assert_eq!(
        key_to_input_with_keymap(
            Key::new(
                KeyCode::BackTab,
                Modifiers {
                    shift: true,
                    control: true,
                    ..Modifiers::NONE
                }
            ),
            &keymap,
            false,
        ),
        RuntimeInput::Dispatch(CommandId::FocusPrevious)
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
fn paste_filters_palette_instead_of_reaching_the_hidden_terminal() {
    let mut state = state();
    let size = SceneSize::new(100, 30);

    state.handle_key(ctrl('p'));
    state.handle_event(InputEvent::Paste("split pane".to_owned()));

    let overlay = state.palette_overlay(size).unwrap();
    assert_eq!(overlay.query, "split pane");
    assert_eq!(overlay.selected, Some(0));
    assert_eq!(state.status(), "command palette open");
}

#[test]
fn paste_filters_help_instead_of_reaching_the_hidden_terminal() {
    let mut state = state();
    let size = SceneSize::new(100, 30);

    state.dispatch(CommandId::ShowHelp);
    state.handle_event(InputEvent::Paste("split pane".to_owned()));

    let overlay = state.help_overlay_scene(size).unwrap();
    assert_eq!(overlay.query, "split pane");
    assert_eq!(overlay.selected, Some(0));
    assert_eq!(state.status(), "help: type to filter · esc close");
}

#[test]
fn opening_palette_exits_copy_mode_before_keys_or_paste_are_routed() {
    let mut state = live_state();
    let size = SceneSize::new(100, 30);
    state.handle_terminal_resize(size.width, size.height);

    state.dispatch(CommandId::EnterCopyMode);
    assert!(state.copy_mode_active());

    // This is the state transition used by the pointer-clickable status strip.
    state.open_palette();
    assert!(!state.copy_mode_active());
    state.handle_event(InputEvent::Paste("split pane".to_owned()));
    state.handle_key(Key::new(
        KeyCode::Char('x'),
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    ));

    let overlay = state.palette_overlay(size).unwrap();
    assert_eq!(overlay.query, "split panex");
    assert_eq!(state.status(), "command palette open");

    state.shutdown();
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
        .runtime
        .terminals()
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
            .runtime
            .terminals()
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
    let written = state
        .runtime
        .write_terminal(pane_id, SHELL_READY_COMMAND)
        .unwrap_or_else(|error| {
            panic!("failed to send shell readiness probe to {pane_id}: {error}")
        });
    assert!(written, "fresh terminal runtime {pane_id} should exist");

    let ready = pump_runtime_until(state, |state| {
        state
            .runtime
            .terminals()
            .get(pane_id)
            .is_some_and(|runtime| {
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

#[test]
fn ordinary_live_terminal_input_waits_for_output_before_dirtying_the_scene() {
    let mut state = live_state();
    state.handle_terminal_resize(100, 30);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    let before_input = state.scene_generation();

    state.handle_key(key(KeyCode::Char('x')));
    assert_eq!(
        state.scene_generation(),
        before_input,
        "successfully written child input is not itself a scene change"
    );

    state.handle_key(Key::new(
        KeyCode::Char('x'),
        Modifiers {
            super_key: true,
            ..Modifiers::NONE
        },
    ));
    assert_eq!(
        state.scene_generation(),
        before_input,
        "an unbound platform chord is a scene-neutral no-op"
    );

    state.handle_key(ctrl('p'));
    assert!(
        state.scene_generation() > before_input,
        "workspace control still dirties the visible scene"
    );
    state.shutdown();
}

#[test]
fn terminal_write_failure_dirties_only_when_visible_status_changes() {
    let mut state = state();
    let initial = state.scene_generation();

    state.handle_key(key(KeyCode::Char('x')));
    let after_failure = state.scene_generation();
    assert!(after_failure > initial);
    assert!(state.status().contains("has no live PTY"));

    state.handle_key(key(KeyCode::Char('y')));
    assert_eq!(
        state.scene_generation(),
        after_failure,
        "repeating the same visible write failure does not manufacture dirtiness"
    );
}

#[test]
fn ime_generation_tracks_visible_preedit_not_successful_terminal_bytes_or_noop_cancel() {
    let mut state = live_state();
    state.handle_terminal_resize(100, 30);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    let stable = state.scene_generation();

    state.handle_event(InputEvent::Composition(CompositionEvent::Commit(
        "x".to_owned(),
    )));
    assert_eq!(
        state.scene_generation(),
        stable,
        "commit without preedit waits for PTY output before dirtying"
    );
    state.handle_event(InputEvent::Composition(CompositionEvent::Cancel));
    assert_eq!(
        state.scene_generation(),
        stable,
        "cancel without active preedit is presentation-neutral"
    );

    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "visible".to_owned(),
        cursor: None,
    }));
    let with_preedit = state.scene_generation();
    assert!(
        with_preedit > stable,
        "creating visible preedit dirties the scene"
    );
    state.handle_event(InputEvent::Composition(CompositionEvent::Cancel));
    let after_cancel = state.scene_generation();
    assert!(
        after_cancel > with_preedit,
        "removing visible preedit dirties the scene"
    );
    state.handle_event(InputEvent::Composition(CompositionEvent::Cancel));
    assert_eq!(
        state.scene_generation(),
        after_cancel,
        "repeated cancel remains presentation-neutral"
    );
    state.shutdown();
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
        state.runtime.try_recv_event().is_ok(),
        "one drain call must leave events beyond the budget queued"
    );
}

// The event budget bounds event *count*, not cost: one PTY chunk can take
// hundreds of milliseconds to parse, so a drain must also stop at a
// wall-clock deadline. Stopping early is only safe if it re-arms the
// frontend wake, which fires on the empty->non-empty transition alone —
// otherwise the remainder strands until the frontend's next heartbeat.
#[test]
fn drain_stops_at_its_deadline_and_rearms_the_frontend_wake() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&wakes);
    let mut state = AppState::new_with_frontend_wake(
        test_config(),
        Some(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        })),
    );
    let sender = state.event_sender();
    for _ in 0..8 {
        sender.send(stale_pty_output()).unwrap();
    }
    let wakes_before = wakes.load(Ordering::SeqCst);

    let drained = state.drain_events_until(DRAIN_EVENT_BUDGET, Instant::now());

    assert_eq!(drained, 1, "an expired deadline must still apply one event");
    assert!(
        wakes.load(Ordering::SeqCst) > wakes_before,
        "a drain cut short by its deadline must re-arm the frontend wake"
    );
    assert!(
        state.runtime.try_recv_event().is_ok(),
        "the events past the deadline must stay queued"
    );
}

// The deadline is additive: with time to spare, a drain still spends the
// whole event budget in one call.
#[test]
fn drain_with_a_generous_deadline_spends_the_event_budget() {
    let mut state = state();
    let sender = state.event_sender();
    for _ in 0..DRAIN_EVENT_BUDGET + 10 {
        sender.send(stale_pty_output()).unwrap();
    }

    let drained =
        state.drain_events_until(DRAIN_EVENT_BUDGET, Instant::now() + Duration::from_secs(60));

    assert_eq!(drained, DRAIN_EVENT_BUDGET);
    assert!(
        state.runtime.try_recv_event().is_ok(),
        "events beyond the budget must stay queued"
    );
}

/// A PTY event for a pane with no runtime: cheap to apply (the identity
/// check rejects it) so drain-bounding tests measure the bound, not the
/// parser.
fn stale_pty_output() -> AppEvent {
    AppEvent::Pty(
        PtyRuntimeEvent::Output {
            pane_id: PaneId::new("pane-none"),
            restart_generation: 0,
            runtime_token: 0,
            bytes: b"x".to_vec(),
        },
        None,
    )
}

// One drain slice feeds all of a pane's queued output chunks to the parser
// in a single call: with debug diagnostics on, the byte count reported for
// the slice is the total across chunks, not the last chunk's, and the grid
// holds the concatenation in reader order.
#[test]
fn drain_coalesces_a_panes_queued_output_into_one_parser_feed() {
    let mut config = test_config();
    config.spawn_pty = true;
    config.debug_status = true;
    let mut state = AppState::new(config);
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    // Let the shell go idle so its own reader adds no bytes to the slice
    // measured below.
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(10));
        state.drain_events();
    }
    let runtime = state.runtime.terminals().get(&pane_id).unwrap();
    let restart_generation = runtime.restart_generation;
    let runtime_token = runtime.runtime_token;
    let sender = state.event_sender();
    for chunk in [b"AA".as_slice(), b"BB", b"CC"] {
        sender
            .send(AppEvent::Pty(
                PtyRuntimeEvent::Output {
                    pane_id: pane_id.clone(),
                    restart_generation,
                    runtime_token,
                    bytes: chunk.to_vec(),
                },
                None,
            ))
            .unwrap();
    }

    state.drain_events();

    assert!(
        state.status().contains("read 6 byte(s) from pane-1"),
        "three queued chunks must apply as one six-byte feed, got: {}",
        state.status()
    );
    assert!(
        grid_text(&state, &pane_id).contains("AABBCC"),
        "coalesced bytes must reach the grid in reader order"
    );
    state.shutdown();
}

// A ReaderClosed queued behind output chunks was sent after its reader read
// them, so the buffered output must flush to the parser before the close
// applies: the close's status lands last and the tail bytes still render.
#[test]
fn reader_closed_queued_behind_output_applies_after_the_flush() {
    let mut config = test_config();
    config.spawn_pty = true;
    config.debug_status = true;
    let mut state = AppState::new(config);
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    let runtime = state.runtime.terminals().get(&pane_id).unwrap();
    let restart_generation = runtime.restart_generation;
    let runtime_token = runtime.runtime_token;
    let sender = state.event_sender();
    sender
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation,
                runtime_token,
                bytes: b"TAIL_BYTES".to_vec(),
            },
            None,
        ))
        .unwrap();
    sender
        .send(AppEvent::Pty(
            PtyRuntimeEvent::ReaderClosed {
                pane_id: pane_id.clone(),
                restart_generation,
                runtime_token,
            },
            None,
        ))
        .unwrap();

    state.drain_events();

    assert!(
        state.status().contains("PTY reader closed"),
        "the close must apply after the flushed output, got: {}",
        state.status()
    );
    assert!(
        grid_text(&state, &pane_id).contains("TAIL_BYTES"),
        "output queued ahead of the close must still reach the grid"
    );
    state.shutdown();
}

// Coalescing holds each chunk's flow credit until the slice's flush, then
// every credit releases exactly once — even when the identity check rejects
// the buffered bytes — so the reader-side window can never leak capacity.
#[test]
fn drain_releases_every_coalesced_flow_credit() {
    let mut state = state();
    let flow = crate::process_events::PtyFlowControl::new();
    let sender = state.event_sender();
    for _ in 0..3 {
        let credit = flow.acquire(100).expect("gate is open");
        sender
            .send(AppEvent::Pty(
                PtyRuntimeEvent::Output {
                    pane_id: PaneId::new("pane-none"),
                    restart_generation: 0,
                    runtime_token: 0,
                    bytes: b"x".to_vec(),
                },
                Some(credit),
            ))
            .unwrap();
    }
    assert_eq!(flow.in_flight_bytes(), 300);

    let drained = state.drain_events();

    assert_eq!(drained, 3);
    assert_eq!(
        flow.in_flight_bytes(),
        0,
        "every buffered chunk's credit must release at the flush"
    );
}

// Chunks from a pane's pre-restart reader must not coalesce into the fresh
// runtime's feed: buffers key on the full runtime identity, so the stale
// buffer is rejected whole while the fresh one lands.
#[test]
fn pre_restart_output_does_not_coalesce_into_the_fresh_runtime_feed() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    let before = state.runtime.terminals().get(&pane_id).unwrap();
    let old_generation = before.restart_generation;
    let old_token = before.runtime_token;

    state.dispatch(CommandId::RestartPane);
    let after = state.runtime.terminals().get(&pane_id).unwrap();
    let new_generation = after.restart_generation;
    let new_token = after.runtime_token;
    assert_ne!(
        (old_generation, old_token),
        (new_generation, new_token),
        "restart must mint a fresh runtime identity"
    );

    let sender = state.event_sender();
    sender
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation: old_generation,
                runtime_token: old_token,
                bytes: b"OLD_READER_OUTPUT".to_vec(),
            },
            None,
        ))
        .unwrap();
    sender
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation: new_generation,
                runtime_token: new_token,
                bytes: b"FRESH_OUTPUT".to_vec(),
            },
            None,
        ))
        .unwrap();

    state.drain_events();

    let rendered = grid_text(&state, &pane_id);
    assert!(
        rendered.contains("FRESH_OUTPUT"),
        "the fresh runtime's chunk must feed its parser"
    );
    assert!(
        !rendered.contains("OLD_READER_OUTPUT"),
        "a stale-identity chunk must never merge into the fresh feed"
    );
    state.shutdown();
}

// The coalesced flush parses in bounded chunks, re-checks the drain deadline
// between chunks, and carries the unparsed remainder — with its flow credits
// still held — to later slices. This is what keeps one drain slice's
// wall-clock bounded under a full 1 MiB coalesced backlog: without it, the
// flush would parse the whole window after the deadline check, reintroducing
// the multi-second stall DRAIN_DEADLINE exists to prevent.
#[test]
fn coalesced_flush_is_deadline_bounded_and_carries_the_remainder() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    let runtime = state
        .runtime
        .terminals()
        .get(&pane_id)
        .expect("live runtime");
    let restart_generation = runtime.restart_generation;
    let runtime_token = runtime.runtime_token;
    let flow = crate::process_events::PtyFlowControl::new();
    let chunk_bytes = PTY_PARSE_CHUNK_BYTES;
    let chunk_count = crate::process_events::MAX_IN_FLIGHT_BYTES / chunk_bytes;
    for _ in 0..chunk_count {
        let credit = flow.acquire(chunk_bytes).expect("gate is open");
        state.pending_pty_output.push(
            pane_id.clone(),
            restart_generation,
            runtime_token,
            vec![b'a'; chunk_bytes],
            Some(credit),
        );
    }
    assert_eq!(
        flow.in_flight_bytes(),
        crate::process_events::MAX_IN_FLIGHT_BYTES
    );

    // An already-expired deadline still parses exactly one bounded chunk
    // (the progress guarantee), then carries the rest to the next slice.
    state.flush_pending_pty_output_within(Some(Instant::now()));
    assert_eq!(
        state.pending_pty_output.byte_len(),
        (chunk_count - 1) * chunk_bytes,
        "an expired deadline bounds the flush to one parse chunk"
    );
    assert_eq!(
        flow.in_flight_bytes(),
        (chunk_count - 1) * chunk_bytes,
        "credits for carried bytes stay held, so the reader window keeps \
         bounding channel backlog plus carried bytes"
    );

    // A deadline-free flush drains the carried remainder in order and
    // releases every remaining credit.
    state.flush_pending_pty_output_within(None);
    assert_eq!(state.pending_pty_output.byte_len(), 0);
    assert_eq!(flow.in_flight_bytes(), 0);
    state.shutdown();
}

// Causal order: input consults parser state (mouse reporting, DECCKM), so
// output that was dequeued into the coalesced buffer before the input
// arrived must reach the parser first — e.g. a child enabling mouse
// reporting, then the wheel that must route to it by the NEW mode.
#[test]
fn dequeued_output_parses_before_later_input_is_applied() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    state.build_scene(POINTER_FRAME);
    let runtime = state
        .runtime
        .terminals()
        .get(&pane_id)
        .expect("live runtime");
    let restart_generation = runtime.restart_generation;
    let runtime_token = runtime.runtime_token;

    // The child's mouse-enabling output sits dequeued but unparsed.
    state.pending_pty_output.push(
        pane_id.clone(),
        restart_generation,
        runtime_token,
        b"\x1b[?1006h\x1b[?1000h".to_vec(),
        None,
    );
    assert!(
        !state
            .runtime
            .terminal_mouse_mode(&pane_id)
            .is_some_and(|mode| mode.wants_mouse()),
        "the enabling bytes must not have reached the parser yet"
    );

    state.apply_app_event(AppEvent::Input(InputEvent::Pointer(pointer_event(
        PointerKind::Wheel {
            dx: 0,
            dy: 1,
            precise: false,
        },
        None,
        5,
        5,
    ))));

    assert!(
        state
            .runtime
            .terminal_mouse_mode(&pane_id)
            .is_some_and(|mode| mode.wants_mouse()),
        "already-dequeued output must parse before a later input event applies"
    );
    assert!(
        state.pointer_view.is_none(),
        "the wheel routed to the child's fresh mouse mode, not workspace scrollback"
    );
    state.shutdown();
}

// Artifact filesystem observation happens on the runtime cadence
// (poll_child_exits / heartbeat), bumps the generation when it changes
// preview state, and never runs inside a frame build — a build is a
// read-only projection, so equal generations keep denoting identical scenes.
#[test]
fn artifact_observation_runs_on_runtime_cadence_and_bumps_generation() {
    let mut state = state();
    let file_name = format!(
        "observed-artifact-{}.png",
        TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let intent = ArtifactPaneIntent {
        source: PathBuf::from(&file_name),
        title: "observed".to_owned(),
        alt_text: "observed".to_owned(),
        fit: ArtifactFit::Contain,
    };
    state
        .workspace
        .apply_action(CoreAction::CreateArtifactPane { intent })
        .expect("artifact pane created");

    // First observation: the missing source records a synchronous failure.
    let created = state.scene_generation();
    state.poll_child_exits();
    assert!(
        state.scene_generation() > created,
        "the first observation changes preview state and must bump"
    );

    // A frame build must not observe the filesystem or mutate preview state.
    let generation = state.scene_generation();
    state.build_scene(POINTER_FRAME);
    assert_eq!(
        state.scene_generation(),
        generation,
        "a frame build is a read-only projection of app state"
    );

    // External write: the next runtime tick observes it and bumps, so the
    // skip guard cannot absorb the repaint while the app is idle.
    let source = test_config().project_path.join(&file_name);
    fs::write(&source, b"externally written bytes").expect("write artifact source");
    state.poll_child_exits();
    assert!(
        state.scene_generation() > generation,
        "an externally written artifact source must bump on observation"
    );

    // External delete: Loading/Ready -> Failed synchronously, same contract.
    let after_write = state.scene_generation();
    fs::remove_file(&source).expect("remove artifact source");
    state.poll_child_exits();
    assert!(
        state.scene_generation() > after_write,
        "an externally deleted artifact source must bump on observation"
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
        .runtime
        .terminals()
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

// A pane whose durable cwd was renamed or deleted must degrade alone: it
// reopens in the project directory and says so. Rejecting the stale cwd at
// the spawn boundary aborted the whole reconcile loop, so every pane ordered
// after the stale one stayed dead until the directory came back.
#[test]
fn stale_pane_cwd_falls_back_and_later_panes_still_spawn() {
    let dir = TestWorkspaceDir::new();
    let config = dir.app_config(true, false);
    let project_dir = config.project_path.clone();
    let renamed_away = dir.path.join("renamed-away");

    let mut state = AppState::new(config);
    state.handle_terminal_resize(120, 40);
    state
        .workspace_mut()
        .apply_action(CoreAction::NewTerminal {
            title: "stale".to_owned(),
            cwd: Some(renamed_away.clone()),
        })
        .unwrap();
    let stale_pane = state.workspace().active_session().focused_pane_id().clone();
    state
        .workspace_mut()
        .apply_action(CoreAction::NewTerminal {
            title: "healthy".to_owned(),
            cwd: Some(project_dir.clone()),
        })
        .unwrap();
    let healthy_pane = state.workspace().active_session().focused_pane_id().clone();
    assert!(
        stale_pane < healthy_pane,
        "the stale pane must reconcile before the healthy one"
    );

    state.handle_terminal_resize(120, 41);

    assert!(
        state.runtime.terminals().get(&stale_pane).is_some(),
        "the stale-cwd pane must come up in the fallback directory"
    );
    assert!(
        state.runtime.terminals().get(&healthy_pane).is_some(),
        "a stale cwd must not wedge the panes ordered after it"
    );
    assert!(
        state.status().contains(&renamed_away.display().to_string())
            && state.status().contains(&project_dir.display().to_string()),
        "the fallback must be visible in the status, got {}",
        state.status()
    );

    // The fallback shell must be live in the fallback directory, not merely
    // spawned: its side effects have to land in the project directory.
    wait_for_shell_ready(&mut state, &stale_pane);
    state
        .runtime
        .write_terminal(&stale_pane, b"touch OPENED_HERE\r")
        .unwrap();
    let landed = pump_runtime_until(&mut state, |_| project_dir.join("OPENED_HERE").exists());
    assert!(
        landed,
        "the fallback shell never ran in the project directory; rows:\n{}",
        grid_text(&state, &stale_pane)
    );

    // Reconcile must converge: the pane is live, so a later pass neither
    // respawns it nor re-reports the fallback.
    state.handle_terminal_resize(120, 42);
    assert_eq!(state.status(), "terminal resized to 120x42");

    state.shutdown();
}

// A preserved status and a pane cwd-fallback notice must combine, not
// replace each other. A Finder launch produces both at once: the launcher
// cwd redirect puts a warning in the preserved config status, and the
// deferred startup restore surfaces the stale-pane fallback on the first
// resize-driven reconcile — which used to drop the preserved line.
#[test]
fn preserved_status_and_cwd_fallback_notice_both_survive_the_first_resize() {
    let dir = TestWorkspaceDir::new();
    let config = dir.app_config(true, false);
    let project_dir = config.project_path.clone();
    let renamed_away = dir.path.join("renamed-away");

    let mut state = AppState::new(config);
    state.handle_terminal_resize(120, 40);
    state
        .workspace_mut()
        .apply_action(CoreAction::NewTerminal {
            title: "stale".to_owned(),
            cwd: Some(renamed_away.clone()),
        })
        .unwrap();

    // The launch shape: a preserved line (config warnings, restore outcome)
    // is pending when the reconcile that trips the fallback runs.
    state.status = "config: launched without a project directory".to_owned();
    state.preserve_status_on_next_resize = true;

    state.handle_terminal_resize(120, 41);

    assert!(
        state
            .status()
            .starts_with("config: launched without a project directory; ")
            && state.status().contains(&renamed_away.display().to_string())
            && state.status().contains(&project_dir.display().to_string()),
        "both notices must survive, got {}",
        state.status()
    );

    state.shutdown();
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
            .runtime
            .tasks()
            .get(&pane_id)
            .is_some_and(|task| task.runtime.exit_status.is_some())
    });
    assert!(exited, "the task never exited");
    let status = state.runtime.tasks().get(&pane_id).unwrap().status.clone();
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
                .runtime
                .tasks()
                .get(&pane_id)
                .is_some_and(|task| task.runtime.exit_status.is_some())
        });
        assert!(exited, "the checks task never exited");
        state.runtime.tasks().get(&pane_id).unwrap().status.clone()
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
        .runtime
        .terminals()
        .get(pane_id)
        .map(|runtime| runtime.parser.grid().snapshot().join("\n"))
        .unwrap_or_default()
}

#[test]
fn ime_terminal_preedit_sends_no_bytes_and_commit_sends_full_text() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    let before = grid_text(&state, &pane_id);

    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "啊👩\u{200d}💻".into(),
        cursor: None,
    }));
    for _ in 0..5 {
        state.tick_runtime();
    }
    assert_eq!(
        grid_text(&state, &pane_id),
        before,
        "preedit must not write child bytes"
    );

    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: String::new(),
        cursor: None,
    }));
    state.handle_event(InputEvent::Composition(CompositionEvent::Commit(
        "printf 'IME_啊_👩\u{200d}💻\\n'\r".into(),
    )));
    assert!(pump_runtime_until(&mut state, |state| {
        grid_text(state, &pane_id).contains("IME_啊_👩\u{200d}💻")
    }));
    state.shutdown();
}

#[test]
fn ime_late_commit_after_modal_or_focus_change_never_leaks_to_terminal() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);

    state.dispatch(CommandId::SearchSession);
    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "late".into(),
        cursor: None,
    }));
    state.handle_key(key(KeyCode::Escape));
    state.handle_event(InputEvent::Composition(CompositionEvent::Commit(
        "SHOULD_NOT_LEAK\r".into(),
    )));
    for _ in 0..10 {
        state.tick_runtime();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!grid_text(&state, &pane_id).contains("SHOULD_NOT_LEAK"));

    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "focus".into(),
        cursor: None,
    }));
    state.handle_event(InputEvent::FocusLost);
    state.handle_event(InputEvent::Composition(CompositionEvent::Commit(
        "FOCUS_LEAK\r".into(),
    )));
    for _ in 0..10 {
        state.tick_runtime();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!grid_text(&state, &pane_id).contains("FOCUS_LEAK"));
    state.shutdown();
}

#[test]
fn ime_native_cancel_then_focus_loss_rejects_a_late_commit() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);

    state.handle_event(InputEvent::Composition(CompositionEvent::Preedit {
        text: "focus".into(),
        cursor: None,
    }));
    state.handle_event(InputEvent::Composition(CompositionEvent::Cancel));
    state.handle_event(InputEvent::FocusLost);
    state.handle_event(InputEvent::Composition(CompositionEvent::Commit(
        "NATIVE_ORDER_LEAK\r".into(),
    )));
    for _ in 0..10 {
        state.tick_runtime();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!grid_text(&state, &pane_id).contains("NATIVE_ORDER_LEAK"));
    state.shutdown();
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
            .runtime
            .terminals()
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

// [L5-GATE] A child requesting any-event mouse tracking receives unbuttoned
// motion; the workspace does not need a hover behavior to keep terminal
// passthrough complete.
#[test]
fn child_any_event_capture_forwards_unbuttoned_motion() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    state.write_to_focused_terminal(b"printf '\\033[?1003h\\033[?1006h'\r");
    let tracking = pump_runtime_until(&mut state, |state| {
        state
            .runtime
            .terminals()
            .get(&pane_id)
            .is_some_and(|runtime| runtime.parser.mouse_mode().wants_mouse())
    });
    assert!(tracking, "child never enabled any-event mouse tracking");
    state.build_scene(POINTER_FRAME);

    send_pointer(&mut state, pointer_event(PointerKind::Move, None, 2, 3));
    let echoed = pump_runtime_until(&mut state, |state| {
        grid_text(state, &pane_id).contains("35;2;2M")
    });
    assert!(
        echoed,
        "unbuttoned motion never reached the child's PTY; grid: {}",
        grid_text(&state, &pane_id)
    );

    state.shutdown();
}

#[test]
fn focus_loss_releases_child_capture_without_completing_a_workspace_drag() {
    let mut state = live_state_with_capturing_child();
    let pane_id = PaneId::new("pane-1");

    send_pointer(&mut state, left(PointerKind::Down, 2, 3));
    assert!(state.pointer_forward.is_some());
    state.handle_event(InputEvent::FocusLost);
    assert!(state.pointer_forward.is_none());

    let echoed = pump_runtime_until(&mut state, |state| {
        grid_text(state, &pane_id).contains("0;2;2m")
    });
    assert!(
        echoed,
        "focus loss did not release the child's mouse capture; grid: {}",
        grid_text(&state, &pane_id)
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
            .runtime
            .terminals()
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
            .runtime
            .terminals()
            .get(&pane_id)
            .is_some_and(|runtime| runtime.parser.grid().scrollback_len() > 10)
    });
    assert!(scrolled, "shell output never reached scrollback");
    state.build_scene(POINTER_FRAME);

    // Wheel up over the pane body scrolls into history without copy mode.
    // Discrete ticks amplify by WHEEL_SCROLL_ROWS.
    send_pointer(
        &mut state,
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: -1,
                precise: false,
            },
            None,
            5,
            5,
        ),
    );
    send_pointer(
        &mut state,
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: -1,
                precise: false,
            },
            None,
            5,
            5,
        ),
    );
    assert!(!state.copy_mode_active());
    assert_eq!(state.pane_view_state(&pane_id).scroll_offset, 6);
    assert!(state.status().contains("scrollback"));

    // Wheel down returns to following live output.
    send_pointer(
        &mut state,
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: 2,
                precise: false,
            },
            None,
            5,
            5,
        ),
    );
    assert_eq!(state.pane_view_state(&pane_id).scroll_offset, 0);
    assert!(state.pointer_view.is_none());
    assert!(state.status().contains("following live output"));

    // Precise (trackpad) deltas arrive pre-quantized to rows and scroll
    // exactly dy rows: no WHEEL_SCROLL_ROWS amplification.
    send_pointer(
        &mut state,
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: -2,
                precise: true,
            },
            None,
            5,
            5,
        ),
    );
    assert_eq!(state.pane_view_state(&pane_id).scroll_offset, 2);

    // Copy-mode wheel honors the same precise/discrete split.
    state.dispatch(CommandId::EnterCopyMode);
    let cursor_before = state.copy_mode.as_ref().unwrap().cursor_row;
    send_pointer(
        &mut state,
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: -2,
                precise: true,
            },
            None,
            5,
            5,
        ),
    );
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor_row,
        cursor_before - 2
    );
    send_pointer(
        &mut state,
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: -1,
                precise: false,
            },
            None,
            5,
            5,
        ),
    );
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor_row,
        cursor_before - 5
    );

    state.shutdown();
}

#[test]
fn pointer_drag_selects_cells_and_copy_selection_copies_them() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    // Direct write, not a paste: a shell with bracketed paste enabled would
    // hold wrapped pasted text in its editor instead of executing it.
    state.write_to_focused_terminal(b"echo SELECT_ME\r");
    // Wait for the output line: ends with the marker but is not the
    // echoed command line (which contains "echo").
    let printed =
        pump_runtime_until(&mut state, |state| {
            state
                .runtime
                .terminals()
                .get(&pane_id)
                .is_some_and(|runtime| {
                    runtime.parser.grid().snapshot().iter().any(|line| {
                        line.trim_end().ends_with("SELECT_ME") && !line.contains("echo")
                    })
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
        .runtime
        .terminals()
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
            .runtime
            .terminals()
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

    // Copy Selection stages renderer-neutral clipboard text. The dispatch
    // path (the one Cmd+C reaches through FrontendHost::copy_selection)
    // clears the selection and rewrites the status — scene-visible state —
    // so it must bump the generation or the skip guard absorbs the repaint.
    let before_copy = state.scene_generation();
    state.dispatch(CommandId::CopySelection);
    assert!(
        state.scene_generation() > before_copy,
        "copying a pointer selection must bump the scene generation"
    );
    assert_eq!(state.last_copied(), Some("SELECT_ME"));
    assert_eq!(
        state.take_frontend_effects(),
        vec![FrontendEffect::SetClipboard("SELECT_ME".to_owned())]
    );
    assert!(state.take_frontend_effects().is_empty());
    assert!(state.pane_view_state(&pane_id).selection.is_none());

    state.shutdown();
}

#[test]
fn focus_loss_cancels_in_progress_pointer_selection() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    state.build_scene(POINTER_FRAME);

    send_pointer(&mut state, left(PointerKind::Down, 5, 5));
    assert!(state.pointer_drag.is_some());
    state.handle_event(InputEvent::FocusLost);
    assert!(state.pointer_drag.is_none());
    assert!(state.pointer_forward.is_none());
    assert!(state.pane_view_state(&pane_id).selection.is_none());

    // A stale drag/release from the old platform gesture is inert.
    send_pointer(&mut state, left(PointerKind::Drag, 12, 5));
    send_pointer(&mut state, left(PointerKind::Up, 12, 5));
    assert!(state.pane_view_state(&pane_id).selection.is_none());

    state.handle_event(InputEvent::FocusGained);
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
        .runtime
        .terminals()
        .get(&pane_id)
        .unwrap()
        .controller
        .process_id();
    state.dispatch(CommandId::EnterCopyMode);
    state
        .frontend_effects
        .push(FrontendEffect::SetClipboard("pending-clipboard".to_owned()));
    state.last_copied = Some("copied text".to_owned());

    state.dispatch(CommandId::RestoreWorkspace);

    assert_eq!(state.live_terminal_count(), 1);
    let after_pid = state
        .runtime
        .terminals()
        .get(&pane_id)
        .unwrap()
        .controller
        .process_id();
    assert_ne!(before_pid, after_pid);
    assert!(!state.copy_mode_active());
    assert!(state.take_frontend_effects().is_empty());
    assert!(state.last_copied().is_none());

    state.shutdown();
}

#[test]
fn restart_replaces_live_runtime_for_same_pane() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    assert_eq!(state.live_terminal_count(), 1);

    let pane_id = PaneId::new("pane-1");
    let before = state.runtime.terminals().get(&pane_id).unwrap();
    assert_eq!(before.restart_generation, 0);
    let before_pid = before.controller.process_id();

    state.dispatch(CommandId::RestartPane);

    // The same pane identity still has exactly one live runtime, now tracking
    // the bumped restart generation with a fresh child process.
    assert_eq!(state.live_terminal_count(), 1);
    let after = state.runtime.terminals().get(&pane_id).unwrap();
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
        .event_sender()
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
        .runtime
        .terminals()
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

// Output the parser ignores — an OSC title set, queries, mode changes —
// leaves the screen exactly as it was, so it must not bump the scene
// generation: under a flood every such chunk would otherwise buy a full
// scene rebuild and a GPU frame that draws the same pixels.
#[test]
fn pty_output_marks_a_redraw_only_when_the_screen_changed() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    let runtime = state.runtime.terminals().get(&pane_id).unwrap();
    let restart_generation = runtime.restart_generation;
    let runtime_token = runtime.runtime_token;
    let output = |bytes: &[u8]| PtyRuntimeEvent::Output {
        pane_id: pane_id.clone(),
        restart_generation,
        runtime_token,
        bytes: bytes.to_vec(),
    };

    let quiet = state.scene_generation();
    state.apply_pty_runtime_event(output(b"\x1b]0;window title\x07"));
    assert_eq!(
        state.scene_generation(),
        quiet,
        "PTY output that changed no screen state forced a frame"
    );

    state.apply_pty_runtime_event(output(b"visible output"));
    assert!(
        state.scene_generation() > quiet,
        "PTY output that wrote the grid did not mark a redraw"
    );

    state.shutdown();
}

#[test]
fn old_reader_terminal_close_and_error_events_after_restart_are_ignored() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    let before = state.runtime.terminals().get(&pane_id).unwrap();
    let before_generation = before.restart_generation;
    let before_token = before.runtime_token;

    state.dispatch(CommandId::RestartPane);
    state
        .event_sender()
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
        .event_sender()
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

    let after = state.runtime.terminals().get(&pane_id).unwrap();
    assert_ne!(before_token, after.runtime_token);
    assert!(after.error.is_none());
    assert!(!state.status().contains("STALE_TERMINAL_READER_ERROR"));

    state.shutdown();
}

// EOF from the PTY reader almost always means the child exited: applying
// ReaderClosed must record the exit immediately, not leave the pane
// lingering until the next 250ms heartbeat poll.
#[test]
fn reader_closed_records_the_child_exit_without_a_heartbeat_poll() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    let runtime = state.runtime.terminals().get(&pane_id).unwrap();
    let restart_generation = runtime.restart_generation;
    let runtime_token = runtime.runtime_token;

    let written = state
        .runtime
        .write_terminal(&pane_id, b"exit\r")
        .expect("exit command should be written");
    assert!(written, "terminal runtime {pane_id} should exist");

    // Apply ReaderClosed directly (retrying while the child winds down)
    // rather than via tick_runtime, whose heartbeat poll would mask the
    // behavior under test.
    let mut exited = false;
    for _ in 0..300 {
        state.apply_pty_runtime_event(PtyRuntimeEvent::ReaderClosed {
            pane_id: pane_id.clone(),
            restart_generation,
            runtime_token,
        });
        if state
            .runtime
            .terminals()
            .get(&pane_id)
            .unwrap()
            .exit_status
            .is_some()
        {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        exited,
        "ReaderClosed did not record the child exit without a heartbeat poll"
    );
    assert!(
        state.status().contains("exited"),
        "exit status did not reach the status line: {}",
        state.status()
    );

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

    // Start a selection and copy it; copy mode exits and stages raw clipboard
    // text for whichever frontend owns the platform integration.
    state.handle_key(key(KeyCode::Char('v')));
    state.handle_key(key(KeyCode::Char('y')));
    assert!(!state.copy_mode_active());
    assert!(state.last_copied().is_some());

    let effects = state.take_frontend_effects();
    assert_eq!(effects.len(), 1);
    let effect = effects.into_iter().next().expect("frontend effect staged");
    let FrontendEffect::SetClipboard(text) = effect else {
        panic!("copy stages a clipboard effect, got {effect:?}");
    };
    assert_eq!(state.last_copied(), Some(text.as_str()));
    assert!(state.take_frontend_effects().is_empty());

    state.shutdown();
}

// Cmd+C regression: FrontendHost::copy_selection dispatches
// CommandId::CopySelection without going through handle_key's redraw
// marking, so the copy paths themselves must bump the generation at every
// state/status-mutating exit — otherwise the screen keeps showing copy mode
// while input already routes normal-mode.
#[test]
fn copy_selection_dispatch_bumps_generation_for_a_copy_mode_selection() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    state.dispatch(CommandId::EnterCopyMode);
    state.handle_key(key(KeyCode::Char('v')));
    assert!(state.copy_mode_active());

    let before = state.scene_generation();
    state.dispatch(CommandId::CopySelection);
    assert!(!state.copy_mode_active());
    assert!(
        state.scene_generation() > before,
        "leaving copy mode via the dispatch path must bump the scene generation"
    );
    state.shutdown();
}

#[test]
fn frontend_effects_preserve_fifo_order_and_drain_once() {
    let mut state = state();
    state
        .frontend_effects
        .push(FrontendEffect::SetClipboard("first".to_owned()));
    state
        .frontend_effects
        .push(FrontendEffect::SetClipboard("second".to_owned()));

    assert_eq!(
        state.take_frontend_effects(),
        vec![
            FrontendEffect::SetClipboard("first".to_owned()),
            FrontendEffect::SetClipboard("second".to_owned()),
        ]
    );
    assert!(state.take_frontend_effects().is_empty());
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
    let first_size = state.runtime.terminals().get(&pane_id).unwrap().size;

    state.handle_terminal_resize(120, 40);

    // The same live runtime survived and the PTY tracked the new geometry.
    assert_eq!(state.live_terminal_count(), 1);
    let runtime = state.runtime.terminals().get(&pane_id).unwrap();
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
            .runtime
            .terminals()
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
        state.runtime.tasks().get(&pane_id).is_some_and(|task| {
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
    let task = state.runtime.tasks().get(&pane_id).unwrap();
    assert_eq!(task.status, "succeeded: exit 0");
    assert!(state.status().contains("succeeded: exit 0"));

    state.shutdown();
}

#[test]
fn update_mandatum_without_an_updater_says_so_and_creates_no_pane() {
    let mut state = state();
    state.updater = None;
    let panes_before = state.workspace().active_session().panes().len();

    state.dispatch(CommandId::UpdateMandatum);

    assert_eq!(
        state.workspace().active_session().panes().len(),
        panes_before,
        "no pane without an updater"
    );
    assert!(
        state.status().contains("no installed updater"),
        "{}",
        state.status()
    );
    assert!(state.update_pane_id.is_none());
}

#[test]
fn update_mandatum_runs_the_updater_task_and_prompts_relaunch_on_success() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(true, false));
    state.handle_terminal_resize(100, 35);
    // A fake destination bundle proves the success path verifies the
    // installed version instead of trusting exit zero.
    let bundle = temp.project_path().join("Mandatum.app");
    fs::create_dir_all(bundle.join("Contents")).unwrap();
    fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>99.0.0</string>
</dict></plist>
"#,
    )
    .unwrap();
    state.updater = Some(crate::updater::ResolvedUpdater {
        command: "printf 'UPDATE_OK\\n'".to_owned(),
        bundle: Some(bundle),
    });

    state.dispatch(CommandId::UpdateMandatum);

    let pane_id = state.update_pane_id.clone().expect("update pane recorded");
    let pane = state.workspace().active_session().pane(&pane_id).unwrap();
    let PaneKind::Task { intent } = pane.kind() else {
        panic!("update must run as a task pane");
    };
    assert_eq!(intent.command, "printf 'UPDATE_OK\\n'");

    // A second invocation while the update runs does not stack a second run.
    let panes_before = state.workspace().active_session().panes().len();
    state.dispatch(CommandId::UpdateMandatum);
    assert_eq!(
        state.workspace().active_session().panes().len(),
        panes_before,
        "a running update is not duplicated"
    );

    let observed = pump_runtime_until(&mut state, |state| state.update_installed);
    assert!(observed, "update success was not observed");
    assert_eq!(
        state.status(),
        "Mandatum 99.0.0 installed · quit and reopen to finish"
    );
    assert_eq!(
        update_segment(&mut state).map(|segment| (segment.kind, segment.label)),
        Some((
            AttentionKind::UpdateInstalled,
            "updated · reopen to finish".to_owned()
        )),
        "the reopen prompt persists in the header"
    );
    assert!(
        state.update_pane_id.is_none(),
        "the watch ends with the run"
    );

    state.shutdown();
}

#[test]
fn update_task_failure_never_claims_an_install() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(true, false));
    state.handle_terminal_resize(100, 35);
    state.updater = Some(crate::updater::ResolvedUpdater {
        command: "exit 7".to_owned(),
        bundle: None,
    });

    state.dispatch(CommandId::UpdateMandatum);
    let pane_id = state.update_pane_id.clone().expect("update pane recorded");

    let observed = pump_runtime_until(&mut state, |state| {
        state
            .runtime
            .tasks()
            .get(&pane_id)
            .is_some_and(|task| task.runtime.exit_status.is_some())
    });
    assert!(observed, "update failure was not observed");
    assert!(!state.update_installed);
    assert!(
        state.status().contains("failed: exit 7"),
        "{}",
        state.status()
    );
    assert!(
        update_segment(&mut state).is_none(),
        "a failed update must not prompt a relaunch"
    );

    state.shutdown();
}

/// The header's update segment for the current frame, if there is one. Update
/// facts live in the header, so tests read them where a user does.
fn update_segment(state: &mut AppState) -> Option<mandatum_scene::AttentionSegment> {
    state
        .build_scene(POINTER_FRAME)
        .header
        .attention
        .into_iter()
        .find(|segment| !segment.kind.is_blocking())
}

#[test]
fn update_available_event_writes_the_persistent_header_note_for_newer_versions_only() {
    let mut state = state();
    assert!(update_segment(&mut state).is_none());

    // An older or equal version is ignored.
    state
        .runtime
        .event_sender()
        .send(AppEvent::UpdateAvailable("0.0.1".to_owned()))
        .unwrap();
    state.drain_events();
    assert!(state.update_available.is_none());
    assert!(update_segment(&mut state).is_none());

    state
        .runtime
        .event_sender()
        .send(AppEvent::UpdateAvailable("99.0.0".to_owned()))
        .unwrap();
    state.drain_events();
    assert_eq!(state.update_available.as_deref(), Some("99.0.0"));
    let segment = update_segment(&mut state).expect("the header carries the available update");
    assert_eq!(segment.label, "99.0.0 available");
    assert_eq!(segment.kind, AttentionKind::UpdateAvailable);
    // A calm tone, not the failure/waiting emphasis: an update blocks nothing.
    assert_eq!(segment.tone, mandatum_scene::PresentationTone::Complete);
    assert!(
        state.status().contains("Update Mandatum"),
        "{}",
        state.status()
    );
    // The status strip keeps its breadcrumbs only — update facts have exactly
    // one home.
    assert!(
        !state.control_hint().contains("available"),
        "{}",
        state.control_hint()
    );
}

#[test]
fn the_update_note_rides_beside_session_facts_and_behind_real_attention() {
    let mut state = state();
    state
        .runtime
        .event_sender()
        .send(AppEvent::UpdateAvailable("99.0.0".to_owned()))
        .unwrap();
    state.drain_events();

    // Calm header: the session facts keep their place and the update note
    // appends after them.
    let calm = state.build_scene(POINTER_FRAME).header;
    assert!(calm.text.contains("1 pane"), "{}", calm.text);
    assert!(calm.text.ends_with("99.0.0 available"), "{}", calm.text);
    assert_eq!(calm.attention.len(), 1);

    // A blocking condition takes the line, and the update note still rides
    // last rather than being displaced.
    state.dispatch(CommandId::RunTask);
    let task_pane = state.workspace().active_session().focused_pane_id().clone();
    state.set_task_status_for_test(&task_pane, "failed: exit 3");

    let busy = state.build_scene(POINTER_FRAME).header;
    let kinds: Vec<_> = busy
        .attention
        .iter()
        .map(|segment| segment.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![AttentionKind::TaskFailed, AttentionKind::UpdateAvailable],
        "the update note sorts behind every blocking condition"
    );
    assert!(busy.text.ends_with("99.0.0 available"), "{}", busy.text);
    // Each segment's rect must land on its own label in the composed text.
    // Every glyph in this fixture is single-width, so a char offset is the
    // display column the rect carries (`attention.rs` covers the wide-glyph
    // case directly).
    for segment in &busy.attention {
        let column = usize::from(segment.rect.x.saturating_sub(busy.area.x));
        let at_rect: String = busy.text.chars().skip(column).collect();
        assert!(
            at_rect.starts_with(&segment.label),
            "segment {:?} rect must point at its label in {:?}",
            segment.label,
            busy.text
        );
    }

    state.shutdown();
}

#[test]
fn clicking_the_update_note_runs_the_updater() {
    let mut state = state();
    state.updater = Some(crate::updater::ResolvedUpdater {
        command: "printf 'UPDATE_OK\\n'".to_owned(),
        bundle: None,
    });
    state
        .runtime
        .event_sender()
        .send(AppEvent::UpdateAvailable("99.0.0".to_owned()))
        .unwrap();
    state.drain_events();

    frame(&mut state);
    let segment = update_segment(&mut state).expect("the header carries the available update");
    let scene = state.build_scene(POINTER_FRAME);
    let target = scene
        .hit_targets
        .iter()
        .find(|target| {
            matches!(
                &target.kind,
                HitTargetKind::AttentionSegment {
                    kind: AttentionKind::UpdateAvailable,
                    ..
                }
            )
        })
        .expect("the update note is clickable")
        .clone();
    assert_eq!(target.rect, segment.rect);
    assert!(
        !target.rect.is_empty(),
        "the note must have a clickable rect"
    );

    send_pointer(
        &mut state,
        left(PointerKind::Down, target.rect.x, target.rect.y),
    );

    let pane_id = state
        .update_pane_id
        .clone()
        .expect("the click launched the updater");
    let pane = state.workspace().active_session().pane(&pane_id).unwrap();
    let PaneKind::Task { intent } = pane.kind() else {
        panic!("update must run as a task pane");
    };
    assert_eq!(intent.command, "printf 'UPDATE_OK\\n'");

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
            .runtime
            .tasks()
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
    assert!(state.runtime.tasks().pending_launches.contains(&pane_id));
    assert_eq!(
        state
            .runtime
            .tasks()
            .statuses
            .get(&pane_id)
            .map(String::as_str),
        Some("pending launch: waiting for visible pane size")
    );

    state.dispatch(CommandId::ZoomPane);

    let observed = pump_runtime_until(&mut state, |state| {
        state.runtime.tasks().get(&pane_id).is_some_and(|task| {
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
    assert!(!state.runtime.tasks().pending_launches.contains(&pane_id));
    assert!(!state.runtime.tasks().statuses.contains_key(&pane_id));

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
            .runtime
            .tasks()
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
    let before = state.runtime.tasks().get(&pane_id).unwrap();
    let before_token = before.runtime.runtime_token;
    let before_generation = before.runtime.restart_generation;
    let pane_count = state.workspace().active_session().panes().len();

    state.task_command = "printf 'TASK_CHANGED\\n'; sleep 5".to_owned();
    state.dispatch(CommandId::RerunTask);

    assert_eq!(state.workspace().active_session().panes().len(), pane_count);
    assert_eq!(state.live_task_count(), 1);
    let after = state.runtime.tasks().get(&pane_id).unwrap();
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
        .event_sender()
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
        state.runtime.tasks().get(&pane_id).is_some_and(|task| {
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
        .runtime
        .tasks()
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
    let before = state.runtime.tasks().get(&pane_id).unwrap();
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
    assert!(state.runtime.tasks().pending_launches.contains(&pane_id));
    assert_eq!(
        state
            .runtime
            .tasks()
            .statuses
            .get(&pane_id)
            .map(String::as_str),
        Some("pending rerun: waiting for visible pane size")
    );
    let pane = state.workspace().active_session().pane(&pane_id).unwrap();
    assert_eq!(pane.restart_generation(), before_generation);
    let PaneKind::Task { intent } = pane.kind() else {
        panic!("focused pane should still be a task pane");
    };
    assert_eq!(intent.command, command);

    state
        .event_sender()
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
        state
            .runtime
            .tasks()
            .statuses
            .get(&pane_id)
            .map(String::as_str),
        Some("pending rerun: waiting for visible pane size")
    );

    state.dispatch(CommandId::ZoomPane);

    let observed = pump_runtime_until(&mut state, |state| {
        state.runtime.tasks().get(&pane_id).is_some_and(|task| {
            task.runtime
                .parser
                .grid()
                .snapshot()
                .join("\n")
                .contains("HIDDEN_RERUN_OK")
        })
    });

    assert!(observed, "pending hidden rerun did not launch when visible");
    assert!(!state.runtime.tasks().pending_launches.contains(&pane_id));
    assert!(!state.runtime.tasks().statuses.contains_key(&pane_id));
    let rendered = state
        .runtime
        .tasks()
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
    assert!(!state.runtime.tasks().pending_launches.contains(&pane_id));

    state.dispatch(CommandId::RerunTask);

    let observed = pump_runtime_until(&mut state, |state| {
        state.runtime.tasks().get(&pane_id).is_some_and(|task| {
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
    let task = state.runtime.tasks().get(&pane_id).unwrap();
    let restart_generation = task.runtime.restart_generation;
    let runtime_token = task.runtime.runtime_token;

    state.dispatch(CommandId::StopTask);

    assert_eq!(state.live_task_count(), 0);
    assert_eq!(
        state
            .runtime
            .tasks()
            .statuses
            .get(&pane_id)
            .map(String::as_str),
        Some("stopped")
    );
    assert!(state.status().contains("stopped"));

    state
        .event_sender()
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
        state
            .runtime
            .tasks()
            .statuses
            .get(&pane_id)
            .map(String::as_str),
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
    assert!(state.runtime.tasks().pending_launches.contains(&pane_id));

    state.dispatch(CommandId::StopTask);

    assert!(!state.runtime.tasks().pending_launches.contains(&pane_id));
    assert_eq!(
        state
            .runtime
            .tasks()
            .statuses
            .get(&pane_id)
            .map(String::as_str),
        Some("stopped before launch")
    );

    state.dispatch(CommandId::ZoomPane);
    for _ in 0..30 {
        state.tick_runtime();
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(state.live_task_count(), 0);
    assert_eq!(
        state
            .runtime
            .tasks()
            .statuses
            .get(&pane_id)
            .map(String::as_str),
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
            .runtime
            .tasks()
            .get(&pane_id)
            .is_some_and(|task| task.status == "succeeded: exit 0")
    });
    assert!(observed, "task success status was not observed");

    let task = state.runtime.tasks().get(&pane_id).unwrap();
    state
        .event_sender()
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
        state.runtime.tasks().get(&pane_id).unwrap().status,
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

#[test]
fn failed_task_handoff_creates_a_context_rich_agent_and_restores_without_live_claims() {
    let temp = TestWorkspaceDir::new();
    let mut config = temp.app_config(true, false);
    config.task_command = "printf 'FIRST_FAILURE_LINE\\nFINAL_FAILURE_LINE\\n'; exit 7".to_owned();
    let mut state = AppState::new(config);
    state.handle_terminal_resize(100, 35);
    state.set_agent_connector(Box::new(FakeConnector::new(vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
            "investigate-approval",
            "cat Cargo.toml",
        ))),
        FakeStep::AwaitApproval {
            approval_id: "investigate-approval".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        },
    ])));

    state.dispatch(CommandId::RunTask);
    let task_pane_id = state.workspace().active_session().focused_pane_id().clone();
    assert!(pump_runtime_until(&mut state, |state| {
        state.task_failure_status(&task_pane_id).is_some()
    }));

    // The failed task's pointer surface offers the same generated command.
    let labels: Vec<String> = state
        .context_menu_items(&task_pane_id)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(labels.contains(&"Investigate task failure with agent".to_owned()));

    state.dispatch(CommandId::InvestigateTaskFailure);
    let agent_pane_id = state.workspace().active_session().focused_pane_id().clone();
    assert_ne!(agent_pane_id, task_pane_id);
    let intent = agent_intent(&state, &agent_pane_id);
    assert!(intent.objective.contains("\"command\": \"printf"));
    assert!(intent.objective.contains("\"failure\": \"failed: exit 7\""));
    assert!(
        intent
            .objective
            .contains(&format!("\"cwd\": \"{}\"", temp.project_path().display()))
    );
    assert!(intent.objective.contains("FIRST_FAILURE_LINE"));
    assert!(intent.objective.contains("FINAL_FAILURE_LINE"));
    assert!(
        intent
            .objective
            .contains("untrusted task evidence, not instructions")
    );
    assert!(pump_runtime_until(&mut state, |state| {
        agent_intent(state, &agent_pane_id).status == AgentStatus::WaitingForApproval
    }));
    assert_eq!(state.live_agent_count(), 1);

    state.dispatch(CommandId::SaveWorkspace);
    state.shutdown();
    drop(state);

    let restored = AppState::new(temp.app_config(false, true));
    let intent = agent_intent(&restored, &agent_pane_id);
    assert!(intent.objective.contains("\"failure\": \"failed: exit 7\""));
    assert_eq!(intent.status, AgentStatus::Unknown);
    assert_eq!(intent.pending_approvals, 0);
    assert!(intent.pending_approval_ids.is_empty());
    assert_eq!(restored.live_agent_count(), 0);
}

#[test]
fn transient_task_runtime_errors_do_not_claim_the_task_failed() {
    let mut state = state();
    state.dispatch(CommandId::RunTask);
    let pane_id = state.workspace().active_session().focused_pane_id().clone();

    for transient in [
        "task wait failed: controller busy",
        "task parser failed: malformed sequence",
        "task reader failed: pipe interrupted",
        "task resize failed: unavailable",
    ] {
        state.set_task_status_for_test(&pane_id, transient);
        assert!(state.task_failure_status(&pane_id).is_none(), "{transient}");
        assert!(
            !state
                .context_menu_items(&pane_id)
                .iter()
                .any(|item| item.label == "Investigate task failure with agent"),
            "{transient}"
        );
    }

    state.set_task_status_for_test(&pane_id, "failed: exit 3");
    assert_eq!(
        state.task_failure_status(&pane_id).as_deref(),
        Some("failed: exit 3")
    );
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
        .event_sender()
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

// [L3-GATE] NewSession discards the live agent session; the pane left
// behind in the now-inactive session must not keep claiming "running".
#[test]
fn new_session_shuts_down_the_agent_and_detaches_its_durable_claim() {
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

    state.dispatch(CommandId::NewSession);

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

// [L3-GATE] Pane ids repeat across sessions, but live runtime identity must
// not. NewSession and session-map activation both retire the previous active
// session's registry before attaching a fresh process to same-id `pane-1`.
#[test]
fn session_switches_replace_same_id_terminal_runtimes() {
    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(true, false));
    state.handle_terminal_resize(80, 24);

    let first_session = state.workspace().active_session().id().clone();
    let pane_id = state.workspace().active_session().focused_pane_id().clone();
    let first_token = state
        .runtime
        .terminals()
        .get(&pane_id)
        .expect("first session has a live shell")
        .runtime_token;

    state.dispatch(CommandId::NewSession);

    let second_session = state.workspace().active_session().id().clone();
    let second_token = state
        .runtime
        .terminals()
        .get(&pane_id)
        .expect("new session has its own live shell")
        .runtime_token;
    assert_ne!(second_session, first_session);
    assert_ne!(second_token, first_token);
    assert_eq!(state.live_terminal_count(), 1);

    // Two sessions with one pane each produce rows: session-1, pane-1,
    // session-2, pane-1. The active pane starts selected at the last row.
    state.dispatch(CommandId::ShowSessionMap);
    state.handle_key(key(KeyCode::Up));
    state.handle_key(key(KeyCode::Up));
    state.handle_key(key(KeyCode::Enter));

    assert_eq!(state.workspace().active_session().id(), &first_session);
    let reactivated_token = state
        .runtime
        .terminals()
        .get(&pane_id)
        .expect("reactivated session launches a fresh shell")
        .runtime_token;
    assert_ne!(reactivated_token, first_token);
    assert_ne!(reactivated_token, second_token);
    assert_eq!(state.live_terminal_count(), 1);

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
    // Print a marker, then bury it in scrollback with filler lines. Direct
    // write, not a paste: a shell with bracketed paste enabled would hold
    // wrapped pasted text in its editor instead of executing it.
    state.write_to_focused_terminal(
        b"echo SEARCH_MARK_XYZ; i=1; while [ $i -le 60 ]; do echo filler_$i; i=$((i+1)); done\r",
    );
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
        .map(|row| {
            row.iter()
                .map(mandatum_scene::SceneCell::grapheme_text)
                .collect::<String>()
        })
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
    // Direct write, not a paste: a shell with bracketed paste enabled would
    // hold wrapped pasted text in its editor instead of executing it.
    state.write_to_focused_terminal(b"echo FLOOD_TARGET_ABC\r");
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
            .runtime
            .terminals()
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
    state.dispatch(CommandId::NewSession); // session-2 (active): pane-1

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

#[test]
fn appearance_overlay_adjusts_the_live_theme_and_owns_input_while_open() {
    let mut state = state();
    let size = SceneSize::new(100, 30);

    state.dispatch(CommandId::AdjustAppearance);
    let overlay = state
        .appearance_overlay_scene(size)
        .expect("dispatch opens the appearance overlay");
    assert_eq!(overlay.selected, 0);
    let before = state.theme().name.clone();

    // Right on the theme row selects the next built-in as a full snapshot;
    // Left returns to the dark default so color adjustments stay visible.
    state.handle_key(key(KeyCode::Right));
    assert_ne!(state.theme().name, before);
    assert!(state.status().starts_with("theme "), "{}", state.status());
    state.handle_key(key(KeyCode::Left));
    assert_eq!(state.theme().name, before);

    // Down to the saturation row, raise it well clear of near-gray, then
    // confirm the hue row moves the background color and reports the hex.
    state.handle_key(key(KeyCode::Down));
    state.handle_key(key(KeyCode::Down));
    for _ in 0..8 {
        state.handle_key(key(KeyCode::Right));
    }
    let saturated = state.theme().terminal_palette.background;
    state.handle_key(key(KeyCode::Up));
    for _ in 0..5 {
        state.handle_key(key(KeyCode::Right));
    }
    assert_ne!(state.theme().terminal_palette.background, saturated);
    assert!(
        state.status().starts_with("background #"),
        "{}",
        state.status()
    );

    // Paste is consumed while the modal owns input; nothing reaches a
    // terminal and the overlay stays open.
    state.handle_event(InputEvent::Paste("ls\n".to_owned()));
    assert!(state.appearance_overlay_scene(size).is_some());

    // The overlay scene reflects the adjusted state each frame.
    let overlay = state.appearance_overlay_scene(size).unwrap();
    assert_eq!(overlay.selected, 1);

    state.handle_key(key(KeyCode::Escape));
    assert!(state.appearance_overlay_scene(size).is_none());
    assert_eq!(state.status(), "appearance closed");
}

#[test]
fn opening_appearance_closes_other_overlays_and_vice_versa() {
    let mut state = state();
    let size = SceneSize::new(100, 30);

    state.dispatch(CommandId::ShowHelp);
    state.dispatch(CommandId::AdjustAppearance);
    assert!(state.help_overlay_scene(size).is_none());
    assert!(state.appearance_overlay_scene(size).is_some());

    state.dispatch(CommandId::ShowHelp);
    assert!(state.appearance_overlay_scene(size).is_none());
    assert!(state.help_overlay_scene(size).is_some());
}

#[test]
fn appearance_adjustments_persist_to_the_user_config_file() {
    let temp = std::env::temp_dir().join(format!(
        "mandatum-appearance-persist-{}-{}",
        std::process::id(),
        line!()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let user_file = temp.join("config.toml");
    let mut config = test_config();
    config.user_config_file = Some(user_file.clone());
    let mut state = AppState::new(config);

    state.dispatch(CommandId::AdjustAppearance);
    state.handle_key(key(KeyCode::Right));

    let text = std::fs::read_to_string(&user_file).expect("adjustment wrote the user config");
    let expected = format!("name = \"{}\"", state.theme().name);
    assert!(text.contains(&expected), "{text}");
    assert!(text.contains("[theme.terminal]"), "{text}");

    // The written file reproduces the live values through the real loader.
    let loaded = crate::config::load_config(Some(&user_file), &temp.join("missing.toml"));
    assert_eq!(loaded.theme.name, state.theme().name);
    assert_eq!(
        loaded.theme.terminal_palette.background,
        state.theme().terminal_palette.background
    );

    let _ = std::fs::remove_dir_all(&temp);
}

// --- P7: per-pane content revisions and retained surfaces ----------------
//
// `content_revision` is settled by equality against the previous frame's
// content, and terminal surfaces are reused only when every build input
// (feed counter, restart generation, grid facts, view state, window dims)
// matches. These tests pin the two directions that matter: unchanged panes
// keep their revision and skip the grid walk, while every content-changing
// path — feeds, copy-mode view state, restarts — is visible in the very
// next frame (the stale-surface hazard from the P7 brief).

fn scene_pane_snapshot(
    scene: &WorkspaceScene,
    pane_id: &str,
) -> (u64, mandatum_scene::PaneContent) {
    let pane = scene
        .panes
        .iter()
        .find(|pane| pane.id.as_str() == pane_id)
        .unwrap_or_else(|| panic!("{pane_id} missing from scene"));
    (pane.content_revision, pane.content.clone())
}

fn surface_text(content: &mandatum_scene::PaneContent) -> String {
    let mandatum_scene::PaneContent::Terminal(surface) = content else {
        panic!("terminal content expected, got {content:?}");
    };
    surface
        .rows
        .iter()
        .flat_map(|row| row.iter())
        .map(mandatum_scene::SceneCell::grapheme_text)
        .collect()
}

// An unrelated redraw (focus, status text) advances the scene generation
// but changes no pane content, so no pane's revision may move.
#[test]
fn unrelated_redraws_keep_pane_content_revisions_stable() {
    let mut state = state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let first = state.build_scene(POINTER_FRAME);
    let generation_before = state.scene_generation();
    state.handle_event(InputEvent::FocusGained);
    assert!(state.scene_generation() > generation_before);
    let second = state.build_scene(POINTER_FRAME);

    let (first_revision, first_content) = scene_pane_snapshot(&first, "pane-1");
    let (second_revision, second_content) = scene_pane_snapshot(&second, "pane-1");
    assert_eq!(second_revision, first_revision);
    assert_eq!(second_content, first_content);
}

// Typing into one pane bumps only that pane; the untouched pane keeps its
// revision, its content, and its retained surface (no grid walk), while the
// fed pane's rebuilt surface provably carries the new bytes.
#[test]
fn pty_output_bumps_only_the_fed_panes_content_revision() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_1 = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_1);
    state.dispatch(CommandId::SplitRight);
    let pane_2 = PaneId::new("pane-2");
    wait_for_shell_ready(&mut state, &pane_2);
    // Let both shells go idle so nothing feeds between the builds below.
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(10));
        state.drain_events();
    }

    let first = state.build_scene(POINTER_FRAME);
    let (rebuilds_after_first, _) = state.pane_surface_cache_counters();

    // Feed pane-1 through its real runtime identity, exactly as its reader
    // thread would.
    let runtime = state.runtime.terminals().get(&pane_1).unwrap();
    let restart_generation = runtime.restart_generation;
    let runtime_token = runtime.runtime_token;
    let sender = state.event_sender();
    sender
        .send(AppEvent::Pty(
            PtyRuntimeEvent::Output {
                pane_id: pane_1.clone(),
                restart_generation,
                runtime_token,
                bytes: b"P7_MARKER".to_vec(),
            },
            None,
        ))
        .unwrap();
    state.drain_events();

    let second = state.build_scene(POINTER_FRAME);
    let (rebuilds_after_second, reuses_after_second) = state.pane_surface_cache_counters();

    let (first_revision_1, _) = scene_pane_snapshot(&first, "pane-1");
    let (first_revision_2, first_content_2) = scene_pane_snapshot(&first, "pane-2");
    let (second_revision_1, second_content_1) = scene_pane_snapshot(&second, "pane-1");
    let (second_revision_2, second_content_2) = scene_pane_snapshot(&second, "pane-2");
    assert_ne!(
        second_revision_1, first_revision_1,
        "the fed pane must bump"
    );
    assert_eq!(
        second_revision_2, first_revision_2,
        "the untouched pane must keep its revision"
    );
    assert_eq!(second_content_2, first_content_2);
    assert!(
        surface_text(&second_content_1).contains("P7_MARKER"),
        "the rebuilt surface must reflect the feed"
    );
    assert_eq!(
        rebuilds_after_second - rebuilds_after_first,
        1,
        "only the fed pane may walk its grid"
    );

    // An unrelated redraw reuses both surfaces and bumps neither revision.
    state.handle_event(InputEvent::FocusGained);
    let third = state.build_scene(POINTER_FRAME);
    let (rebuilds_after_third, reuses_after_third) = state.pane_surface_cache_counters();
    assert_eq!(
        rebuilds_after_third, rebuilds_after_second,
        "an idle rebuild must not walk any grid"
    );
    assert_eq!(reuses_after_third - reuses_after_second, 2);
    assert_eq!(scene_pane_snapshot(&third, "pane-1").0, second_revision_1);
    assert_eq!(scene_pane_snapshot(&third, "pane-2").0, second_revision_2);
    state.shutdown();
}

// Copy-mode view state (cursor, scroll, selection) alters the pane's
// surface without any PTY feed; a retained surface must never survive it.
// This is exactly the stale path where pointer input would resolve against
// wrong geometry if the cache missed a bump.
#[test]
fn copy_mode_view_changes_bump_the_viewed_panes_revision() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(10));
        state.drain_events();
    }

    let live = state.build_scene(POINTER_FRAME);
    state.dispatch(CommandId::EnterCopyMode);
    let entered = state.build_scene(POINTER_FRAME);
    // The ready marker plus prompt put the copy cursor below row 0, so one
    // step up always moves it.
    state.handle_key(Key::plain(KeyCode::Char('k')));
    let moved = state.build_scene(POINTER_FRAME);
    state.handle_key(Key::plain(KeyCode::Escape));
    let exited = state.build_scene(POINTER_FRAME);

    let revision = |scene: &WorkspaceScene| scene_pane_snapshot(scene, "pane-1").0;
    let copy_cursor = |scene: &WorkspaceScene| match scene_pane_snapshot(scene, "pane-1").1 {
        mandatum_scene::PaneContent::Terminal(surface) => surface.copy_cursor,
        content => panic!("terminal content expected, got {content:?}"),
    };
    assert_ne!(revision(&entered), revision(&live));
    assert!(copy_cursor(&entered).is_some());
    assert_ne!(revision(&moved), revision(&entered));
    assert_ne!(copy_cursor(&moved), copy_cursor(&entered));
    assert_ne!(revision(&exited), revision(&moved));
    assert!(copy_cursor(&exited).is_none());
    state.shutdown();
}

// A restarted pane owns a fresh grid: the revision moves and the previous
// shell's surface is never republished, even before the new shell prints
// its first byte.
#[test]
fn pane_restart_bumps_the_revision_and_drops_the_retained_surface() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    let pane_id = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_id);
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(10));
        state.drain_events();
    }

    let before = state.build_scene(POINTER_FRAME);
    state.dispatch(CommandId::RestartPane);
    let after = state.build_scene(POINTER_FRAME);

    let (before_revision, before_content) = scene_pane_snapshot(&before, "pane-1");
    let (after_revision, after_content) = scene_pane_snapshot(&after, "pane-1");
    assert!(surface_text(&before_content).contains(SHELL_READY_MARKER));
    assert_ne!(after_revision, before_revision);
    assert_ne!(after_content, before_content);
    assert!(
        !surface_text(&after_content).contains(SHELL_READY_MARKER),
        "the pre-restart surface must not survive into the fresh runtime"
    );
    state.shutdown();
}

// The appearance overlay is keyboard-modal, so the pointer must be modal
// too: a press is consumed (closing the overlay, like help) instead of
// stealing focus or starting a selection on an obscured pane.
#[test]
fn appearance_overlay_consumes_pointer_presses_without_focus_steal() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    frame(&mut state);
    assert_eq!(focused(&state), "pane-2");

    state.dispatch(CommandId::AdjustAppearance);
    state.build_scene(POINTER_FRAME);
    // Pane-1's body sits under (2, 3).
    send_pointer(&mut state, left(PointerKind::Down, 2, 3));
    assert!(
        state.appearance_overlay_scene(POINTER_FRAME).is_none(),
        "a press closes the modal overlay"
    );
    assert_eq!(focused(&state), "pane-2", "the press must not steal focus");
    assert!(state.pointer_view.is_none());
    assert!(state.pointer_drag.is_none());

    // A right press is consumed the same way: no context menu underneath.
    state.dispatch(CommandId::AdjustAppearance);
    state.build_scene(POINTER_FRAME);
    send_pointer(&mut state, right_down(2, 3));
    assert!(state.appearance_overlay_scene(POINTER_FRAME).is_none());
    assert!(state.context_menu.is_none());
}

#[test]
fn appearance_overlay_wheel_moves_its_selection_instead_of_scrolling_panes() {
    let mut state = state();
    frame(&mut state);
    state.dispatch(CommandId::AdjustAppearance);
    state.build_scene(POINTER_FRAME);

    send_pointer(
        &mut state,
        pointer_event(
            PointerKind::Wheel {
                dx: 0,
                dy: 1,
                precise: false,
            },
            None,
            5,
            5,
        ),
    );

    let overlay = state
        .appearance_overlay_scene(POINTER_FRAME)
        .expect("the wheel must not close the overlay");
    assert_eq!(overlay.selected, 1);
    assert!(
        state.pointer_view.is_none(),
        "the wheel must not reach the pane underneath"
    );
}

#[test]
fn strip_paste_guards_removes_recombining_guard_fragments() {
    assert_eq!(strip_paste_guards("plain"), "plain");
    assert_eq!(strip_paste_guards("a\x1b[200~b\x1b[201~c"), "abc");
    // Removing an inner guard must not splice a fresh one together from
    // the surrounding bytes: these need a second pass.
    assert_eq!(strip_paste_guards("\x1b[201\x1b[201~~"), "");
    assert_eq!(strip_paste_guards("\x1b[20\x1b[200~1~x"), "x");
}

// Paste is wrapped in DEC 2004 guards exactly while the focused child has
// bracketed paste enabled, with embedded guard sequences stripped so the
// clipboard cannot break out of the bracket and smuggle typed input.
#[test]
fn paste_honors_the_childs_bracketed_paste_mode_and_strips_embedded_guards() {
    let mut state = live_state();
    state.handle_terminal_resize(80, 24);
    let pane_id = PaneId::new("pane-1");
    let runtime = state.runtime.terminals().get(&pane_id).unwrap();
    let restart_generation = runtime.restart_generation;
    let runtime_token = runtime.runtime_token;
    let output = |bytes: &[u8]| PtyRuntimeEvent::Output {
        pane_id: pane_id.clone(),
        restart_generation,
        runtime_token,
        bytes: bytes.to_vec(),
    };

    // Mode off (the spawn default): paste forwards raw.
    assert_eq!(
        state.encode_paste_for_focused_child("plain text"),
        b"plain text".to_vec()
    );

    state.apply_pty_runtime_event(output(b"\x1b[?2004h"));
    assert_eq!(state.runtime.terminal_bracketed_paste(&pane_id), Some(true));
    assert_eq!(
        state.encode_paste_for_focused_child("evil\x1b[201~breakout"),
        b"\x1b[200~evilbreakout\x1b[201~".to_vec()
    );

    state.apply_pty_runtime_event(output(b"\x1b[?2004l"));
    assert_eq!(
        state.runtime.terminal_bracketed_paste(&pane_id),
        Some(false)
    );
    assert_eq!(
        state.encode_paste_for_focused_child("plain again"),
        b"plain again".to_vec()
    );
    state.shutdown();
}

// Copy mode on pane A: a press on pane B moves focus AND leaves copy mode,
// so subsequent keys reach B's shell instead of A's copy-mode keymap (the
// policy `open_palette` already applies).
#[test]
fn clicking_another_pane_exits_copy_mode_and_keys_follow_focus() {
    let mut state = live_state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    state.dispatch(CommandId::SplitRight);
    let pane_1 = PaneId::new("pane-1");
    wait_for_shell_ready(&mut state, &pane_1);

    // Copy mode on the focused pane-2.
    state.dispatch(CommandId::EnterCopyMode);
    assert!(state.copy_mode_active());
    state.build_scene(POINTER_FRAME);

    // Pane-1's body sits under (2, 3).
    send_pointer(&mut state, left(PointerKind::Down, 2, 3));
    send_pointer(&mut state, left(PointerKind::Up, 2, 3));
    assert!(
        !state.copy_mode_active(),
        "a press on another pane must exit copy mode"
    );
    assert_eq!(focused(&state), "pane-1");

    for character in "echo COPY_EXIT_OK".chars() {
        state.handle_key(key(KeyCode::Char(character)));
    }
    state.handle_key(key(KeyCode::Enter));
    let reached = pump_runtime_until(&mut state, |state| {
        grid_text(state, &pane_1).contains("COPY_EXIT_OK")
    });
    assert!(
        reached,
        "typed keys must reach the newly focused pane; grid: {}",
        grid_text(&state, &pane_1)
    );
    state.shutdown();
}

// update_pane_id must not survive the seams where pane ids repeat (sessions
// and restores both restart the `pane-N` sequence): an unrelated same-id
// task pane's success would otherwise read as a completed update.
#[test]
fn session_switch_and_workspace_restore_forget_the_update_pane() {
    let mut state = state();
    state.update_pane_id = Some(PaneId::new("pane-1"));
    state.dispatch(CommandId::NewSession);
    assert!(state.update_pane_id.is_none());

    let temp = TestWorkspaceDir::new();
    let mut state = AppState::new(temp.app_config(false, false));
    state.dispatch(CommandId::SaveWorkspace);
    state.update_pane_id = Some(PaneId::new("pane-1"));
    state.dispatch(CommandId::RestoreWorkspace);
    assert!(
        state.status().contains("workspace restored"),
        "{}",
        state.status()
    );
    assert!(state.update_pane_id.is_none());
}

// The pane-keyed presentation maps are pruned at every seam a pane leaves
// through: close, session switch, and restore. Stale entries would address
// unrelated same-id panes later.
#[test]
fn pane_close_and_session_seams_prune_pane_keyed_presentation_maps() {
    let mut state = state();
    state.dispatch(CommandId::SplitRight);
    let pane_2 = PaneId::new("pane-2");
    state.approval_arrivals.insert(pane_2.clone(), 7);
    state.pane_grid_revisions.insert(pane_2.clone(), 3);

    state.dispatch(CommandId::ClosePane);
    assert!(!state.approval_arrivals.contains_key(&pane_2));
    assert!(!state.pane_grid_revisions.contains_key(&pane_2));

    state.approval_arrivals.insert(PaneId::new("pane-1"), 9);
    state.pane_grid_revisions.insert(PaneId::new("pane-1"), 4);
    state.dispatch(CommandId::NewSession);
    assert!(state.approval_arrivals.is_empty());
    assert!(state.pane_grid_revisions.is_empty());
}

// Keyboard float movement mirrors the drag path: one durable step per
// dispatch, and a tiled pane reports instead of moving.
#[test]
fn keyboard_float_movement_steps_the_focused_float_and_reports_non_floats() {
    let mut state = state();
    state.handle_terminal_resize(POINTER_FRAME.width, POINTER_FRAME.height);
    state.dispatch(CommandId::SplitRight);

    state.dispatch(CommandId::MoveFloatRight);
    assert!(
        state.status().contains("not floating"),
        "{}",
        state.status()
    );

    state.dispatch(CommandId::FloatPane);
    let pane_2 = PaneId::new("pane-2");
    let rect_of = |state: &AppState| {
        state
            .workspace()
            .active_session()
            .layout()
            .floating()
            .iter()
            .find(|floating| floating.pane_id == pane_2)
            .expect("pane-2 floats")
            .rect
            .clone()
    };
    let start = rect_of(&state);

    state.dispatch(CommandId::MoveFloatRight);
    state.dispatch(CommandId::MoveFloatDown);
    let moved = rect_of(&state);
    assert_eq!(moved.x, start.x + 2, "one horizontal step is two columns");
    assert_eq!(moved.y, start.y + 1, "one vertical step is one row");

    state.dispatch(CommandId::MoveFloatLeft);
    state.dispatch(CommandId::MoveFloatUp);
    let back = rect_of(&state);
    assert_eq!((back.x, back.y), (start.x, start.y));
}

// Stack panes folds the focused tiled pane and its neighbor into one
// stacked node; without a neighbor it reports instead of mutating.
#[test]
fn stack_panes_stacks_the_focused_pane_with_its_neighbor() {
    let mut state = state();
    state.dispatch(CommandId::StackPanes);
    assert!(
        state.status().contains("command failed"),
        "{}",
        state.status()
    );

    state.dispatch(CommandId::SplitRight);
    state.dispatch(CommandId::StackPanes);
    assert!(
        matches!(
            state.workspace().active_session().layout().root(),
            LayoutNode::Stack { .. }
        ),
        "{}",
        state.status()
    );
}

// The first-run note's Escape is consumed: it dismisses the note without
// reaching the shell, and the next key proceeds normally.
#[test]
fn first_run_note_escape_is_consumed_and_only_dismisses_the_note() {
    let mut state = state();
    state.first_run_note = true;
    let status_before = state.status().to_owned();

    state.handle_event(InputEvent::Key(key(KeyCode::Escape)));
    assert!(!state.first_run_note);
    assert_eq!(
        state.status(),
        status_before,
        "the consumed Escape must not act behind the note"
    );

    // With the note gone the same key routes normally: the write attempt
    // reaches the (not spawned) focused terminal and reports.
    state.handle_event(InputEvent::Key(key(KeyCode::Escape)));
    assert!(state.status().contains("no live PTY"), "{}", state.status());
}
