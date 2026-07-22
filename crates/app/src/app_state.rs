use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

use mandatum_agent_runtime::{AgentConnector, AgentLaunchSpec, AgentSessionEvent};
use mandatum_commands::{
    BUILT_IN_COMMANDS, CommandContext, CommandId, CommandTarget, PaletteContext, PaletteInput,
    PaletteKey, RuntimeAgentCommand, RuntimeCommand, RuntimeTaskCommand, command_target,
    dispatch_command, resolve_palette_key_with_bindings,
};
use mandatum_core::{
    ActionOutcome, AgentApprovalRecord, AgentPaneIntent, AgentStatus, CoreAction, LayoutNode,
    PaneId, PaneKind, PersistenceRequest, SessionId, SplitAxis, TaskPaneIntent, Workspace,
};
use mandatum_pty::PtySize;
use mandatum_scene::{
    ContextMenuEntry, ContextMenuOverlay, HelpOverlay, HitTarget, HitTargetKind, PaletteOverlay,
    PaneSceneKind, PromptOverlay, SceneRect, SceneSize, SearchOverlay, SessionMapOverlay, Theme,
    TimelineOverlay, WelcomeOverlay, WorkspaceScene,
    input::{InputEvent, Key, KeyCode, PointerButton, PointerEvent, PointerKind},
    layout::{
        context_menu_rect, help_overlay_rect, layout_separators, palette_item_window,
        palette_overlay_rect, pane_content_rect, pane_inner_rect, prompt_rect, welcome_rect,
        workspace_scene_area,
    },
};
use mandatum_terminal_vt::TerminalGrid;
use mandatum_workflows::TaskFailureHandoff;

use crate::{
    agent_runtime::{AgentRuntimeEvent, connector_for_kind},
    app_shell::AppConfig,
    config::{AgentConnectorKind, effective_runtime_settings, load_config, project_config_file},
    copy_mode::CopyModeState,
    events::AppEvent,
    frontend_effect::FrontendEffect,
    help::{HelpViewState, filter_help_rows, help_route, help_rows, welcome_entries},
    input::{RuntimeInput, key_to_input_with_keymap},
    keymap::{ChordAction, Keymap, format_chord},
    palette::{PaletteRow, PaletteState, PaletteWorkspaceView, palette_footer, palette_rows},
    persistence::{PersistenceCoordinator, WorkspaceFileError},
    pointer::{encode_mouse_event, split_percent_for_pointer},
    process_events::PtyRuntimeEvent,
    runtime_engine::{
        AgentApprovalError, AgentRuntimeView, PreparedRuntimeRestore, RestoreGeometry,
        RestoreRuntimeError, RuntimeEngine, RuntimeExitEffect, RuntimeLifecycleTrigger,
        RuntimePtyEffect, RuntimeReconcileError, RuntimeReconcileNotice, TaskAttempt,
        TaskLaunchOutcome, TaskStopOutcome,
    },
    scene_builder::PaneViewState,
    search::{
        SearchCorpus, SearchHitTarget, SearchSource, SearchSourceKind, SearchViewState,
        scroll_offset_for_row, search_overlay,
    },
    session_map::{
        SessionMapRowModel, SessionMapState, SessionMapTarget, session_map_overlay,
        session_map_rows,
    },
    terminal_runtime::exit_status_label,
    timeline::{TimelineEventKind, TimelineLog, now_ms},
    timeline_view::{TimelineViewState, timeline_overlay},
};

#[cfg(test)]
use crate::{
    input::key_to_input,
    persistence::{MAX_WORKSPACE_FILE_BYTES, ensure_parent_dir, write_workspace_file},
    task_runtime::TaskInvestigationFailure,
};

/// The most events one `drain_events` call applies. Bounding per-call work
/// keeps a flooding producer from starving the draw/redraw checks in the
/// shell loop; the reader-side flow gates bound how much can queue at all.
const DRAIN_EVENT_BUDGET: usize = 256;

pub struct AppState {
    workspace: Workspace,
    command_context: CommandContext,
    persistence: PersistenceCoordinator,
    shell_program: String,
    task_command: String,
    spawn_pty: bool,
    palette: Option<PaletteState>,
    should_quit: bool,
    terminal_size: Option<(u16, u16)>,
    status: String,
    preserve_status_on_next_resize: bool,
    last_redraw: Instant,
    runtime: RuntimeEngine,
    agent_connector: Option<Box<dyn AgentConnector>>,
    agent_connector_label: &'static str,
    agent_objective: String,
    agent_model: Option<String>,
    /// The durable execution-timeline log (append side).
    timeline: TimelineLog,
    /// The open timeline overlay, if any (modal, like the palette).
    timeline_view: Option<TimelineViewState>,
    /// The open session-search overlay, if any (modal, like the palette).
    search_view: Option<SearchViewState>,
    /// The open session-map overlay, if any (modal).
    session_map: Option<SessionMapState>,
    /// The open help overlay, if any (modal, like the palette).
    help_view: Option<HelpViewState>,
    /// Whether the one-time first-run note is on screen. Set only when a
    /// launch that asked to restore found no saved workspace; cleared by any
    /// action (a saved workspace suppresses it on every later launch).
    first_run_note: bool,
    /// The open Set-agent-objective prompt, if any (modal).
    objective_prompt: Option<ObjectivePrompt>,
    keymap: Keymap,
    theme: Theme,
    reduced_motion: bool,
    /// Surface byte-level PTY diagnostics in the status line. Off by
    /// default: they are noise that would bury meaningful status.
    debug_status: bool,
    user_config_file: Option<PathBuf>,
    copy_mode: Option<CopyModeState>,
    frontend_effects: Vec<FrontendEffect>,
    last_copied: Option<String>,
    // --- Pointer state (runtime presentation only, never serialized) ------
    /// Hit targets of the last built scene; pointer events resolve against
    /// them in reverse (topmost target wins).
    hit_targets: Vec<HitTarget>,
    /// The in-flight workspace drag, armed on a button press over a target.
    pointer_drag: Option<PointerDrag>,
    /// While a mouse-capturing child owns the pointer: the pane its button
    /// press was forwarded to (and its inner rect for coordinates), so drags
    /// and the release reach the same child ([L5-GATE]).
    pointer_forward: Option<(PaneId, SceneRect)>,
    /// Pointer-driven viewport scroll and selection for one pane.
    pointer_view: Option<PointerView>,
    /// The open right-click menu, if any (modal, like the palette).
    context_menu: Option<ContextMenuState>,
    /// The previous button press, for double-click detection.
    last_pane_click: Option<PaneClick>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let command_context =
            CommandContext::for_project(config.workspace_name.clone(), config.project_path.clone());
        let workspace = Workspace::new(config.workspace_name, config.project_path);
        let restore_on_startup = config.restore_on_startup;
        let config_warnings = config.config_warnings;
        // The timeline lives beside the workspace file; a baseline with no
        // workspace directory (unit tests) simply disables recording.
        let timeline_file = config
            .workspace_file
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.join("timeline.jsonl"));

        let mut state = Self {
            workspace,
            command_context,
            persistence: PersistenceCoordinator::new(config.workspace_file),
            shell_program: config.shell_program,
            task_command: config.task_command,
            spawn_pty: config.spawn_pty,
            palette: None,
            should_quit: false,
            terminal_size: None,
            status: "ready".to_owned(),
            preserve_status_on_next_resize: false,
            last_redraw: Instant::now(),
            runtime: RuntimeEngine::new(),
            agent_connector: connector_for_kind(config.agent_connector),
            agent_connector_label: connector_kind_label(config.agent_connector),
            agent_objective: config.agent_objective,
            agent_model: config.agent_model,
            timeline: TimelineLog::new(timeline_file),
            timeline_view: None,
            search_view: None,
            session_map: None,
            help_view: None,
            first_run_note: false,
            objective_prompt: None,
            keymap: config.keymap,
            theme: config.theme,
            reduced_motion: config.reduced_motion,
            debug_status: config.debug_status,
            user_config_file: config.user_config_file,
            copy_mode: None,
            frontend_effects: Vec::new(),
            last_copied: None,
            hit_targets: Vec::new(),
            pointer_drag: None,
            pointer_forward: None,
            pointer_view: None,
            context_menu: None,
            last_pane_click: None,
        };

        if restore_on_startup {
            state.restore_workspace_at_startup();
        }

        // A broken config never blocks launch; it launches on defaults with
        // the exact problems named in the status line.
        if !config_warnings.is_empty() {
            state.status = format!("config: {}", config_warnings.join("; "));
            state.preserve_status_on_next_resize = true;
        }

        state
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn terminal_size(&self) -> Option<(u16, u16)> {
        self.terminal_size
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn workspace_file(&self) -> &Path {
        self.persistence.workspace_file()
    }

    pub fn live_terminal_count(&self) -> usize {
        self.runtime.terminal_count()
    }

    pub fn live_task_count(&self) -> usize {
        self.runtime.task_count()
    }

    pub fn live_agent_count(&self) -> usize {
        self.runtime.agent_count()
    }

    pub fn copy_mode_active(&self) -> bool {
        self.copy_mode.is_some()
    }

    /// The text most recently copied via copy mode, for verification and tests.
    pub fn last_copied(&self) -> Option<&str> {
        self.last_copied.as_deref()
    }

    /// Take all pending platform effects in request order. Clears the queue so
    /// the active frontend applies each effect exactly once.
    pub fn take_frontend_effects(&mut self) -> Vec<FrontendEffect> {
        std::mem::take(&mut self.frontend_effects)
    }

    #[cfg(test)]
    pub(crate) fn stage_frontend_effect_for_test(&mut self, effect: FrontendEffect) {
        self.frontend_effects.push(effect);
    }

    /// The active theme, resolved from config, for the frontend adapter.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The configured agent connector kind, for the calm header strip.
    pub(crate) fn agent_connector_label(&self) -> &'static str {
        self.agent_connector_label
    }

    /// A task pane's current status text, live or retained.
    /// The permanent status-strip hint naming the workspace's entry points,
    /// from the live keymap. A stranger's first breadcrumb: the palette
    /// chord, the right-click menu, and the help key are always written on
    /// screen.
    pub(crate) fn control_hint(&self) -> String {
        format!(
            "{} commands · right-click menu · {} help",
            format_chord(self.keymap.toggle_palette),
            help_route(&self.keymap)
        )
    }

    /// Whether the config asked for reduced motion. Nothing animates yet;
    /// frontends must consult this before they ever do.
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// The palette overlay scene for the current frame, `None` while the
    /// palette is closed: the query, the fuzzy-filtered context-ranked
    /// entries, the selection, and the key-hint footer.
    pub(crate) fn palette_overlay(&self, size: SceneSize) -> Option<PaletteOverlay> {
        let palette = self.palette.as_ref()?;
        let rows = self.current_palette_rows();
        let selected = if rows.is_empty() {
            None
        } else {
            Some(palette.selected.min(rows.len() - 1))
        };
        let area = palette_overlay_rect(size);
        // The footer counts the entries scrolled out of the visible window
        // (the same window math the frontend and hit targets use), so the
        // list never looks complete when it is not.
        let window = mandatum_scene::layout::palette_item_window(
            mandatum_scene::layout::pane_inner_rect(area),
            rows.len(),
            selected,
        );
        Some(PaletteOverlay {
            area,
            query: palette.query.clone(),
            footer: palette_footer(window.start, rows.len().saturating_sub(window.end)),
            items: rows.into_iter().map(|row| row.entry).collect(),
            selected,
        })
    }

    /// The palette rows for the live query, ranked and availability-gated.
    fn current_palette_rows(&self) -> Vec<PaletteRow> {
        let Some(palette) = self.palette.as_ref() else {
            return Vec::new();
        };
        palette_rows(
            &palette.query,
            palette.selected,
            &self.palette_workspace_view(),
            &self.keymap,
        )
    }

    /// Snapshot the workspace facts the palette ranks and gates on.
    fn palette_workspace_view(&self) -> PaletteWorkspaceView {
        let session = self.workspace.active_session();
        let focused_id = session.focused_pane_id().clone();
        let focused = session.pane(&focused_id);
        let focused_kind = focused
            .map(|pane| match pane.kind() {
                PaneKind::Terminal { .. } => PaneSceneKind::Terminal,
                PaneKind::Task { .. } => PaneSceneKind::Task,
                PaneKind::Agent { .. } => PaneSceneKind::Agent,
                PaneKind::StatusLog { .. } => PaneSceneKind::StatusLog,
            })
            .unwrap_or(PaneSceneKind::Terminal);
        let focused_pane_label = focused
            .map(|pane| format!("{} ({focused_id})", pane.title()))
            .unwrap_or_else(|| focused_id.to_string());
        let agent_runtime = self.runtime.agent_view(&focused_id);
        let focused_is_floating = session.layout().is_floating(&focused_id);

        PaletteWorkspaceView {
            focused_kind,
            focused_pane_label,
            focused_agent_session_live: agent_runtime.is_some(),
            focused_agent_pending_approval: agent_runtime
                .is_some_and(|runtime| runtime.pending_approval.is_some()),
            agent_connector_configured: self.agent_connector.is_some(),
            agent_panes_exist: !self.agent_pane_ids().is_empty(),
            any_agent_waiting: session
                .panes()
                .iter()
                .any(|(pane_id, _)| self.pane_waiting_for_approval(pane_id)),
            focused_task_running: self.runtime.task_running_or_pending(&focused_id),
            focused_task_failed: self.task_failure_status(&focused_id).is_some(),
            focused_has_live_terminal: self.runtime.has_terminal(&focused_id),
            focused_is_floating,
            timeline_available: self.timeline.enabled(),
            // Any tiled pane sits inside a split exactly when the tiled root
            // is one.
            focused_in_tiled_split: !focused_is_floating
                && matches!(session.layout().root(), LayoutNode::Split { .. }),
            pane_count: session.panes().len(),
        }
    }

    pub fn handle_event(&mut self, event: InputEvent) {
        // The first-run note dismisses on any action — a key, a paste, or a
        // pointer press — and the action itself proceeds normally (the note
        // is never modal). Resize and pointer motion are not actions.
        if self.first_run_note
            && matches!(
                event,
                InputEvent::Key(_)
                    | InputEvent::Paste(_)
                    | InputEvent::Pointer(PointerEvent {
                        kind: PointerKind::Down | PointerKind::Wheel { .. },
                        ..
                    })
            )
        {
            self.first_run_note = false;
        }
        match event {
            InputEvent::Key(key) => self.handle_key(key),
            InputEvent::Resize(size) => self.handle_terminal_resize(size.width, size.height),
            // Text-input overlays receive pasted text into their input.
            InputEvent::Paste(text) if self.objective_prompt.is_some() => {
                if let Some(prompt) = self.objective_prompt.as_mut() {
                    prompt.input.push_str(&text);
                }
                self.mark_redraw();
            }
            InputEvent::Paste(text) if self.timeline_view.is_some() => {
                if let Some(view) = self.timeline_view.as_mut() {
                    view.push_query(&text, now_ms());
                }
                self.mark_redraw();
            }
            InputEvent::Paste(text) if self.search_view.is_some() => {
                if let Some(view) = self.search_view.as_mut() {
                    view.query.push_str(&text);
                    view.refresh();
                }
                self.mark_redraw();
            }
            // Paste only reaches the shell in normal mode; copy mode, the
            // context menu, and the session map own input while open.
            InputEvent::Paste(text)
                if self.copy_mode.is_none()
                    && self.context_menu.is_none()
                    && self.session_map.is_none() =>
            {
                self.write_to_focused_terminal(text.as_bytes())
            }
            // [L5-GATE] Pointer events resolve against the last scene's hit
            // targets; when the child under the pointer requested mouse
            // reporting they forward to its PTY instead, unless the user
            // invokes explicit workspace control (alt, copy mode, menu).
            InputEvent::Pointer(pointer) => self.handle_pointer(pointer),
            _ => {}
        }
    }

    /// Build one frame of scene and retain its hit targets, so pointer
    /// events resolve against exactly what was last drawn.
    pub fn build_scene(&mut self, size: SceneSize) -> WorkspaceScene {
        let scene = crate::scene_builder::build_workspace_scene(self, size);
        self.hit_targets = scene.hit_targets.clone();
        scene
    }

    pub fn handle_terminal_resize(&mut self, columns: u16, rows: u16) {
        self.terminal_size = Some((columns, rows));
        // Copy-mode coordinates address a specific grid geometry; a resize
        // reshapes the buffer, so leave copy mode rather than track moved coordinates.
        if self.copy_mode.is_some() {
            self.copy_mode = None;
        }
        // Pointer state addresses the old geometry too: selections and menu
        // anchors would point at moved cells, and any drag loses its frame.
        self.pointer_view = None;
        self.pointer_drag = None;
        self.context_menu = None;
        if self.preserve_status_on_next_resize {
            let status = self.status.clone();
            if let Err(error) = self.reconcile_runtimes() {
                self.status = format!("{status}; {error}");
            } else {
                self.status = status;
            }
            self.preserve_status_on_next_resize = false;
        } else {
            match self.reconcile_runtimes() {
                Ok(()) => self.status = format!("terminal resized to {columns}x{rows}"),
                Err(error) => self.status = error.to_string(),
            }
        }
        self.mark_redraw();
    }

    pub fn handle_key(&mut self, key: Key) {
        // The context menu is the topmost modal surface.
        if self.context_menu.is_some() {
            self.handle_context_menu_key(key);
            self.mark_redraw();
            return;
        }

        if self.copy_mode.is_some() {
            self.handle_copy_mode_key(key);
            self.mark_redraw();
            return;
        }

        if self.palette.is_some() {
            self.handle_palette_key(key);
            self.mark_redraw();
            return;
        }

        // The visibility overlays are modal too: at most one is open, and it
        // owns the keyboard until Esc/Enter closes it.
        if self.objective_prompt.is_some() {
            self.handle_objective_prompt_key(key);
            self.mark_redraw();
            return;
        }
        if self.timeline_view.is_some() {
            self.handle_timeline_key(key);
            self.mark_redraw();
            return;
        }
        if self.search_view.is_some() {
            self.handle_search_key(key);
            self.mark_redraw();
            return;
        }
        if self.session_map.is_some() {
            self.handle_session_map_key(key);
            self.mark_redraw();
            return;
        }
        if self.help_view.is_some() {
            self.handle_help_key(key);
            self.mark_redraw();
            return;
        }

        // Direct approval keys: while the focused pane is an agent pane with
        // a pending approval, y/n decide it without opening the palette. An
        // agent pane has no terminal input to shadow.
        if key.mods.is_empty() && self.focused_agent_has_pending_approval() {
            match key.code {
                KeyCode::Char('y') => {
                    self.dispatch(CommandId::ApproveAgentAction);
                    self.mark_redraw();
                    return;
                }
                KeyCode::Char('n') => {
                    self.dispatch(CommandId::RejectAgentAction);
                    self.mark_redraw();
                    return;
                }
                _ => {}
            }
        }

        match key_to_input_with_keymap(key, &self.keymap) {
            RuntimeInput::Quit => {
                self.should_quit = true;
                self.status = "quitting".to_owned();
            }
            RuntimeInput::TogglePalette => self.open_palette(),
            RuntimeInput::Dispatch(command_id) => self.dispatch(command_id),
            RuntimeInput::SendToTerminal(bytes) => self.write_to_focused_terminal(&bytes),
            RuntimeInput::Noop => {}
        }
        self.mark_redraw();
    }

    /// Palette-mode key routing. See `crate::palette` for the full
    /// interaction contract this implements.
    fn handle_palette_key(&mut self, key: Key) {
        // Fixed navigation keys win over chords while the palette is open,
        // so Ctrl+P (the default toggle chord) moves the selection up here;
        // Esc closes the palette.
        let ctrl_only = key.mods.control && !key.mods.shift && !key.mods.alt && !key.mods.super_key;
        if key.code == KeyCode::Up || (ctrl_only && key.code == KeyCode::Char('p')) {
            self.move_palette_selection(-1);
            return;
        }
        if key.code == KeyCode::Down || (ctrl_only && key.code == KeyCode::Char('n')) {
            self.move_palette_selection(1);
            return;
        }

        // Workspace chords keep working over the open palette: quit quits, a
        // (non-default) toggle chord closes, command chords dispatch.
        match self.keymap.chord_action(key) {
            Some(ChordAction::Quit) => {
                self.should_quit = true;
                self.status = "quitting".to_owned();
                return;
            }
            Some(ChordAction::TogglePalette) => {
                self.close_palette();
                return;
            }
            Some(ChordAction::Dispatch(command_id)) => {
                self.palette = None;
                self.dispatch(command_id);
                return;
            }
            None => {}
        }

        let query_is_empty = self
            .palette
            .as_ref()
            .is_none_or(|palette| palette.query.is_empty());
        match key.code {
            KeyCode::Escape => self.close_palette(),
            KeyCode::Enter => self.execute_palette_selection(),
            KeyCode::Backspace => {
                if let Some(palette) = self.palette.as_mut() {
                    palette.query.pop();
                    palette.selected = 0;
                }
            }
            // First-keystroke muscle memory: with an empty input, Tab and
            // BackTab cycle pane focus exactly as the pre-fuzzy palette did.
            KeyCode::Tab if query_is_empty => {
                self.palette = None;
                self.dispatch(CommandId::FocusNext);
            }
            KeyCode::BackTab if query_is_empty => {
                self.palette = None;
                self.dispatch(CommandId::FocusPrevious);
            }
            KeyCode::Tab => self.move_palette_selection(1),
            KeyCode::BackTab => self.move_palette_selection(-1),
            KeyCode::Char(character)
                if !key.mods.control && !key.mods.alt && !key.mods.super_key =>
            {
                // First-keystroke muscle memory: with an empty input, a bare
                // key resolves through the classic single-letter bindings
                // (bound keys dispatch, `q` quits). An unbound key — or any
                // Shift+letter — starts the fuzzy filter instead. Shift only
                // suppresses the fast path for letters: symbol bindings
                // (like +/-) legitimately arrive with shift held.
                if query_is_empty && !(key.mods.shift && character.is_ascii_alphabetic()) {
                    match resolve_palette_key_with_bindings(
                        PaletteKey::Character(character),
                        self.palette_context(),
                        &self.keymap.palette,
                    ) {
                        PaletteInput::Dispatch(command_id) => {
                            // Fast paths honor the availability gate the
                            // listed rows honor: a bare letter never
                            // fire-and-fails where the row would be greyed,
                            // and it reports the same reason.
                            if let Err(reason) = crate::palette::availability(
                                command_id,
                                &self.palette_workspace_view(),
                            ) {
                                let label = mandatum_commands::command_for_id(command_id)
                                    .map(|command| command.label)
                                    .unwrap_or("Command");
                                self.status = format!("{label} is unavailable: {reason}");
                                return;
                            }
                            self.palette = None;
                            self.dispatch(command_id);
                            return;
                        }
                        PaletteInput::Quit => {
                            self.should_quit = true;
                            self.status = "quitting".to_owned();
                            return;
                        }
                        PaletteInput::Close => {
                            self.close_palette();
                            return;
                        }
                        PaletteInput::Noop => {}
                    }
                }
                if let Some(palette) = self.palette.as_mut() {
                    palette.query.push(character);
                    palette.selected = 0;
                }
            }
            _ => {}
        }
    }

    fn open_palette(&mut self) {
        self.palette = Some(PaletteState::default());
        self.status = "command palette open".to_owned();
    }

    fn close_palette(&mut self) {
        self.palette = None;
        self.status = "command palette closed".to_owned();
    }

    fn move_palette_selection(&mut self, delta: isize) {
        let row_count = self.current_palette_rows().len();
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if row_count == 0 {
            palette.selected = 0;
            return;
        }
        let current = palette.selected.min(row_count - 1) as isize;
        palette.selected = (current + delta).clamp(0, row_count as isize - 1) as usize;
    }

    /// Run the selected palette entry: dispatch it and close, or surface the
    /// reason a greyed entry cannot run (the palette stays open).
    fn execute_palette_selection(&mut self) {
        let rows = self.current_palette_rows();
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        if rows.is_empty() {
            self.status = format!("no command matches '{}'", palette.query.trim());
            return;
        }
        let row = &rows[palette.selected.min(rows.len() - 1)];
        if !row.enabled {
            self.status = format!("{} is unavailable: {}", row.entry.label, row.entry.detail);
            return;
        }
        let command_id = row.command_id;
        self.palette = None;
        self.dispatch(command_id);
    }

    pub fn dispatch(&mut self, command_id: CommandId) {
        // Runtime commands change app presentation state, not durable core state,
        // so they never go through the core dispatch path.
        if command_id == CommandId::RestartPane && self.focused_pane_is_task() {
            self.status = "task panes use Rerun Task; Restart Pane is shell-only".to_owned();
            return;
        }

        // Timeline fact: what was asked for, and where focus was when it was.
        self.timeline.record(TimelineEventKind::CommandDispatched {
            command: mandatum_commands::command_for_id(command_id)
                .map(|command| command.name.to_owned())
                .unwrap_or_else(|| format!("{command_id:?}")),
            pane: Some(
                self.workspace
                    .active_session()
                    .focused_pane_id()
                    .to_string(),
            ),
        });

        match command_target(command_id) {
            CommandTarget::Runtime(runtime_command) => {
                self.dispatch_runtime_command(runtime_command);
                return;
            }
            CommandTarget::RuntimeTask(task_command) => {
                self.dispatch_runtime_task_command(task_command);
                return;
            }
            CommandTarget::RuntimeAgent(agent_command) => {
                self.dispatch_runtime_agent_command(agent_command);
                return;
            }
            CommandTarget::Core => {}
        }

        // Pane lifecycle facts come from the before/after diff, so every
        // core path (splits, new terminal, close) records without bespoke
        // hooks. A session switch is not a pane diff.
        let session_before = self.workspace.active_session().id().clone();
        let panes_before: BTreeSet<PaneId> = self
            .workspace
            .active_session()
            .panes()
            .keys()
            .cloned()
            .collect();

        match dispatch_command(&mut self.workspace, &self.command_context, command_id) {
            Ok(outcome) => {
                if self.workspace.active_session().id() == &session_before {
                    self.record_pane_diff(&panes_before);
                } else {
                    self.retire_runtimes_for_session_switch(&session_before);
                }
                self.handle_command_outcome(command_id, outcome);
            }
            Err(error) => {
                self.status = format!("command failed: {error}");
            }
        }
    }

    /// Record created/closed panes against a pre-dispatch snapshot.
    fn record_pane_diff(&mut self, before: &BTreeSet<PaneId>) {
        let session = self.workspace.active_session();
        let after: BTreeSet<PaneId> = session.panes().keys().cloned().collect();
        let created: Vec<(String, String)> = after
            .difference(before)
            .filter_map(|pane_id| {
                session.pane(pane_id).map(|pane| {
                    let kind = match pane.kind() {
                        PaneKind::Terminal { .. } => "terminal",
                        PaneKind::Task { .. } => "task",
                        PaneKind::Agent { .. } => "agent",
                        PaneKind::StatusLog { .. } => "status",
                    };
                    (pane_id.to_string(), kind.to_owned())
                })
            })
            .collect();
        let closed: Vec<String> = before.difference(&after).map(ToString::to_string).collect();
        for (pane, kind) in created {
            self.timeline
                .record(TimelineEventKind::PaneCreated { pane, kind });
        }
        for pane in closed {
            self.timeline.record(TimelineEventKind::PaneClosed { pane });
        }
    }

    fn focused_pane_is_task(&self) -> bool {
        self.focused_task_intent().is_some()
    }

    fn palette_context(&self) -> PaletteContext {
        let session = self.workspace.active_session();
        PaletteContext {
            focused_pane_is_task: self.focused_pane_is_task(),
            focused_pane_is_floating: session.layout().is_floating(session.focused_pane_id()),
        }
    }

    fn focused_task_intent(&self) -> Option<(PaneId, TaskPaneIntent)> {
        let pane_id = self.workspace.active_session().focused_pane_id().clone();
        self.workspace
            .active_session()
            .pane(&pane_id)
            .and_then(|pane| match pane.kind() {
                PaneKind::Task { intent } => Some((pane_id, intent.clone())),
                _ => None,
            })
    }

    fn handle_command_outcome(&mut self, command_id: CommandId, outcome: ActionOutcome) {
        match outcome {
            ActionOutcome::PersistenceRequested(PersistenceRequest::SaveWorkspace) => {
                self.save_workspace_to_disk();
            }
            ActionOutcome::PersistenceRequested(PersistenceRequest::RestoreWorkspace) => {
                self.restore_workspace_from_disk();
            }
            outcome => {
                self.status = status_for_outcome(command_id, outcome);
                if let Err(error) = self.reconcile_runtimes() {
                    self.status = error.to_string();
                }
            }
        }
    }

    fn dispatch_runtime_command(&mut self, runtime_command: RuntimeCommand) {
        match runtime_command {
            RuntimeCommand::EnterCopyMode => self.enter_copy_mode(),
            RuntimeCommand::ReloadConfig => self.reload_config(),
            RuntimeCommand::ShowTimeline => self.open_timeline(),
            RuntimeCommand::ShowSessionMap => self.open_session_map(),
            RuntimeCommand::SearchSession => self.open_search(),
            RuntimeCommand::ShowHelp => self.open_help(),
            RuntimeCommand::MoveFloatLeft => {
                self.move_focused_float(-i32::from(MOVE_FLOAT_STEP_COLUMNS), 0)
            }
            RuntimeCommand::MoveFloatRight => {
                self.move_focused_float(i32::from(MOVE_FLOAT_STEP_COLUMNS), 0)
            }
            RuntimeCommand::MoveFloatUp => self.move_focused_float(0, -1),
            RuntimeCommand::MoveFloatDown => self.move_focused_float(0, 1),
            RuntimeCommand::Quit => {
                self.should_quit = true;
                self.status = "quitting".to_owned();
            }
            RuntimeCommand::CopySelection => {
                if let Some(pane_id) = self.copy_mode.as_ref().map(|state| state.pane_id.clone()) {
                    self.copy_selection(&pane_id);
                } else {
                    self.copy_pointer_selection();
                }
            }
        }
    }

    /// Re-read the config files live. Keymap, theme and UI settings apply
    /// immediately; shell/task/agent settings apply to future launches.
    fn reload_config(&mut self) {
        let project_file = project_config_file(&self.command_context.project_path);
        let loaded = load_config(self.user_config_file.as_deref(), &project_file);
        let runtime = effective_runtime_settings(&loaded);
        self.keymap = loaded.keymap;
        self.theme = loaded.theme;
        self.reduced_motion = loaded.reduced_motion;
        self.debug_status = loaded.debug_status;
        self.shell_program = runtime.shell_program;
        self.task_command = runtime.task_command;
        self.agent_connector = connector_for_kind(runtime.agent_connector);
        self.agent_connector_label = connector_kind_label(runtime.agent_connector);
        self.agent_model = runtime.agent_model;
        self.timeline.record(TimelineEventKind::ConfigReloaded {
            warnings: loaded.warnings.len(),
        });
        self.status = if loaded.warnings.is_empty() {
            "config reloaded".to_owned()
        } else {
            format!("config reloaded; {}", loaded.warnings.join("; "))
        };
        self.mark_redraw();
    }

    fn dispatch_runtime_task_command(&mut self, task_command: RuntimeTaskCommand) {
        match task_command {
            RuntimeTaskCommand::RunConfiguredTask => self.run_configured_task(),
            RuntimeTaskCommand::RerunFocusedTask => self.rerun_focused_task(),
            RuntimeTaskCommand::StopFocusedTask => self.stop_focused_task(),
            RuntimeTaskCommand::InvestigateFocusedTaskFailure => {
                self.investigate_focused_task_failure()
            }
        }
    }

    fn dispatch_runtime_agent_command(&mut self, agent_command: RuntimeAgentCommand) {
        match agent_command {
            RuntimeAgentCommand::NewAgentPane => self.new_agent_pane(),
            RuntimeAgentCommand::StartFocusedAgent => self.start_focused_agent(),
            RuntimeAgentCommand::StopFocusedAgent => self.stop_focused_agent(),
            RuntimeAgentCommand::ApproveFocusedAgentAction => {
                self.decide_focused_agent_approval(true)
            }
            RuntimeAgentCommand::RejectFocusedAgentAction => {
                self.decide_focused_agent_approval(false)
            }
            RuntimeAgentCommand::FocusNextWaitingAgent => self.focus_next_waiting_agent(),
            RuntimeAgentCommand::SetFocusedAgentObjective => self.open_objective_prompt(),
        }
        self.mark_redraw();
    }

    pub fn tick_runtime(&mut self) {
        self.drain_events();
        self.poll_child_exits();
    }

    /// A clone of the unified event channel's send side, for the frontend's
    /// input thread.
    pub(crate) fn event_sender(&self) -> Sender<AppEvent> {
        self.runtime.event_sender()
    }

    /// Block until the next event arrives (and apply it) or `timeout`
    /// elapses. Returns whether an event was applied. This is the shell's
    /// one blocking wait: input, PTY output, and agent events all land on
    /// the same channel, so nothing can arrive without waking the loop.
    pub(crate) fn wait_event(&mut self, timeout: Duration) -> bool {
        match self.runtime.recv_event_timeout(timeout) {
            Ok(event) => {
                self.apply_app_event(event);
                true
            }
            Err(_) => false,
        }
    }

    /// Apply what is already buffered without blocking (burst drain: pointer
    /// drags and PTY floods arrive faster than any redraw), bounded to
    /// [`DRAIN_EVENT_BUDGET`] events per call. The bound is what keeps a
    /// producer that outruns the consumer (a `yes`/`cat` flood) from pinning
    /// the loop in here forever: the shell always gets back to draw() and
    /// the redraw-cap check between drains.
    pub(crate) fn drain_events(&mut self) -> usize {
        let mut drained = 0;
        for _ in 0..DRAIN_EVENT_BUDGET {
            if self.should_quit {
                break;
            }
            match self.runtime.try_recv_event() {
                Ok(event) => {
                    self.apply_app_event(event);
                    drained += 1;
                }
                Err(_) => break,
            }
        }
        drained
    }

    fn apply_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Input(input) => self.handle_event(input),
            AppEvent::Pty(event, credit) => {
                // Release the flow credit before parsing so the reader can
                // admit its next chunk while this one is applied.
                drop(credit);
                self.apply_pty_runtime_event(event);
            }
            AppEvent::Agent(event) => self.apply_agent_runtime_event(event),
        }
    }

    pub fn shutdown(&mut self) {
        self.runtime.shutdown_all();
        self.status = "terminal sessions stopped".to_owned();
    }

    fn save_workspace_to_disk(&mut self) {
        match self.persistence.save_workspace(&self.workspace) {
            Ok(()) => {
                let path = self.persistence.workspace_file().display().to_string();
                self.timeline
                    .record(TimelineEventKind::WorkspaceSaved { path: path.clone() });
                self.status = format!("workspace saved to {path}");
            }
            Err(error) => {
                self.status = format!("workspace save failed: {error}");
            }
        }
    }

    fn restore_workspace_at_startup(&mut self) {
        match self.persistence.read_workspace() {
            Ok(workspace) => match self
                .prepare_restore_runtimes(&workspace, RuntimeLifecycleTrigger::StartupRestore)
            {
                Ok(runtimes) => {
                    self.replace_workspace_from_disk(workspace, runtimes);
                    let path = self.persistence.workspace_file().display().to_string();
                    self.timeline
                        .record(TimelineEventKind::WorkspaceRestored { path: path.clone() });
                    self.status = format!("workspace restored from {path}");
                    self.preserve_status_on_next_resize = true;
                }
                Err(error) => {
                    self.status = format!("startup restore failed: {error}");
                    self.preserve_status_on_next_resize = true;
                }
            },
            Err(WorkspaceFileError::Io { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                // First run: no saved workspace exists. Show the one-time
                // orientation note and label the state; the shared status-strip
                // control hint adds the generated routes exactly once. Saving a
                // workspace makes this branch unreachable on later launches.
                self.first_run_note = true;
                self.status = "new workspace".to_owned();
                self.preserve_status_on_next_resize = true;
            }
            Err(error) => {
                self.status = format!("startup restore failed: {error}");
                self.preserve_status_on_next_resize = true;
            }
        }
    }

    fn restore_workspace_from_disk(&mut self) {
        match self.persistence.read_workspace() {
            Ok(workspace) => match self
                .prepare_restore_runtimes(&workspace, RuntimeLifecycleTrigger::ExplicitRestore)
            {
                Ok(runtimes) => {
                    self.replace_workspace_from_disk(workspace, runtimes);
                    let path = self.persistence.workspace_file().display().to_string();
                    self.timeline
                        .record(TimelineEventKind::WorkspaceRestored { path: path.clone() });
                    self.status = format!("workspace restored from {path}");
                }
                Err(error) => {
                    self.status = format!("workspace restore failed: {error}");
                }
            },
            Err(error) => {
                self.status = format!("workspace restore failed: {error}");
            }
        }
    }

    fn prepare_restore_runtimes(
        &mut self,
        workspace: &Workspace,
        trigger: RuntimeLifecycleTrigger,
    ) -> Result<PreparedRuntimeRestore, RestoreRuntimeError> {
        let visible_terminals = self.visible_terminal_pane_sizes_for_workspace(workspace);
        let geometry = if self.terminal_size.is_some() {
            RestoreGeometry::Available
        } else {
            RestoreGeometry::Unavailable
        };
        self.runtime.prepare_restore(
            workspace,
            &self.shell_program,
            self.spawn_pty,
            trigger,
            geometry,
            visible_terminals,
        )
    }

    fn replace_workspace_from_disk(
        &mut self,
        mut workspace: Workspace,
        runtimes: PreparedRuntimeRestore,
    ) {
        let outgoing_session_id = self.workspace.active_session().id().clone();
        let report = self
            .runtime
            .commit_restore(&mut workspace, &outgoing_session_id, runtimes);
        debug_assert_eq!(self.runtime.last_lifecycle_report(), &report);
        self.workspace = workspace;
        self.command_context = command_context_for_workspace(&self.workspace);
        self.copy_mode = None;
        self.timeline_view = None;
        self.search_view = None;
        self.session_map = None;
        self.help_view = None;
        self.objective_prompt = None;
        self.frontend_effects.clear();
        self.last_copied = None;
        self.pointer_view = None;
        self.pointer_drag = None;
        self.pointer_forward = None;
        self.context_menu = None;
    }

    fn write_to_focused_terminal(&mut self, bytes: &[u8]) {
        let focused = self.workspace.active_session().focused_pane_id().clone();
        match self.runtime.write_terminal(&focused, bytes) {
            Ok(true) => {
                // Byte-level diagnostics are debug-only: writing them on
                // every keystroke would bury meaningful status (failures,
                // attention) under noise.
                if self.debug_status {
                    self.status = format!("sent {} byte(s) to {focused}", bytes.len());
                }
            }
            Ok(false) => self.status = format!("pane {focused} has no live PTY"),
            Err(error) => {
                self.status = format!("PTY input failed for {focused}: {error}");
            }
        }
    }

    fn run_configured_task(&mut self) {
        let intent = TaskPaneIntent {
            // No recipe: this is an ad-hoc run of the configured default
            // command, and "recipe:" is reserved for real recipe names.
            recipe_id: None,
            command: self.task_command.clone(),
            cwd: Some(self.command_context.project_path.clone()),
        };
        let title = "task".to_owned();
        match self.workspace.apply_action(CoreAction::CreateTaskPane {
            title,
            intent: intent.clone(),
        }) {
            Ok(ActionOutcome::Mutated { focused_pane }) => {
                self.timeline.record(TimelineEventKind::PaneCreated {
                    pane: focused_pane.to_string(),
                    kind: "task".to_owned(),
                });
                self.status = format!("task pane created for {}", intent.command);
                if let Err(error) = self.launch_task_pane(focused_pane, &intent) {
                    self.status = format!("task launch failed: {error}");
                }
            }
            Ok(ActionOutcome::PersistenceRequested(_)) => {
                self.status = "task command unexpectedly requested persistence".to_owned();
            }
            Err(error) => {
                self.status = format!("task pane creation failed: {error}");
            }
        }
        self.mark_redraw();
    }

    fn rerun_focused_task(&mut self) {
        let Some((pane_id, intent)) = self.focused_task_intent() else {
            self.status = "focused pane is not a task pane".to_owned();
            self.mark_redraw();
            return;
        };

        let outcome = self.runtime.launch_task(
            &self.workspace,
            &self.shell_program,
            pane_id.clone(),
            self.visible_task_size(&pane_id),
            self.spawn_pty,
            TaskAttempt::Rerun,
        );
        self.status = match outcome {
            TaskLaunchOutcome::Disabled => {
                format!("task {pane_id} rerun unavailable: PTY spawning is disabled")
            }
            TaskLaunchOutcome::Deferred => {
                format!("task {pane_id} pending rerun: waiting for visible pane size")
            }
            TaskLaunchOutcome::Running => {
                self.timeline.record(TimelineEventKind::TaskStarted {
                    pane: pane_id.to_string(),
                    command: intent.command.clone(),
                });
                format!("task {pane_id} rerunning: {}", intent.command)
            }
            TaskLaunchOutcome::Failed(status) => status,
        };
        self.mark_redraw();
    }

    fn stop_focused_task(&mut self) {
        let Some((pane_id, _)) = self.focused_task_intent() else {
            self.status = "focused pane is not a task pane".to_owned();
            self.mark_redraw();
            return;
        };

        self.status = match self.runtime.stop_task(&pane_id) {
            TaskStopOutcome::StoppedBeforeLaunch => {
                format!("task {pane_id} stopped before launch")
            }
            TaskStopOutcome::NotRunning => format!("task {pane_id} is not running"),
            TaskStopOutcome::Already(status) => format!("task {pane_id} is already {status}"),
            TaskStopOutcome::Stopped => format!("task {pane_id} stopped"),
            TaskStopOutcome::Failed(error) => {
                format!("task stop failed for {pane_id}: {error}")
            }
        };
        self.mark_redraw();
    }

    /// Turn the focused task's known failure into a bounded, durable agent
    /// mandate, then launch it through the same connector and approval seam as
    /// every other agent. Runtime handles and parser state never cross into the
    /// workflow module.
    fn investigate_focused_task_failure(&mut self) {
        let Some((task_pane_id, intent)) = self.focused_task_intent() else {
            self.status = "focused pane is not a task pane".to_owned();
            return;
        };
        let Some(failure) = self.task_failure_status(&task_pane_id) else {
            self.status = "focused task has no known failure".to_owned();
            return;
        };
        let Some((task_title, cwd)) =
            self.workspace
                .active_session()
                .pane(&task_pane_id)
                .map(|pane| {
                    (
                        pane.title().to_owned(),
                        crate::terminal_runtime::resolve_pane_cwd(
                            &self.workspace,
                            pane,
                            intent.cwd.as_ref(),
                        ),
                    )
                })
        else {
            self.status = format!("task pane {task_pane_id} was not found");
            return;
        };
        let handoff = TaskFailureHandoff::new(
            intent.command,
            cwd.clone(),
            failure,
            self.task_output_lines(&task_pane_id),
        );
        let PaneKind::Agent { intent } = handoff.agent_thread_spec().pane_kind() else {
            unreachable!("a task failure handoff always creates agent intent");
        };

        match self.workspace.apply_action(CoreAction::CreateAgentPane {
            title: format!("investigate {task_title}"),
            intent,
            cwd: Some(cwd),
        }) {
            Ok(ActionOutcome::Mutated { focused_pane }) => {
                self.timeline.record(TimelineEventKind::PaneCreated {
                    pane: focused_pane.to_string(),
                    kind: "agent".to_owned(),
                });
                self.status = format!(
                    "agent pane {focused_pane} created to investigate {task_title} ({task_pane_id})"
                );
                self.start_focused_agent();
            }
            Ok(ActionOutcome::PersistenceRequested(_)) => {
                self.status = "failure investigation unexpectedly requested persistence".to_owned();
            }
            Err(error) => {
                self.status = format!("failure investigation pane creation failed: {error}");
            }
        }
        self.mark_redraw();
    }

    pub(crate) fn task_failure_status(&self, pane_id: &PaneId) -> Option<String> {
        self.runtime.task_failure_label(pane_id)
    }

    fn task_output_lines(&self, pane_id: &PaneId) -> Vec<String> {
        self.runtime.task_output_lines(pane_id)
    }

    // --- Agent runtime ---------------------------------------------------------

    fn focused_agent_pane_id(&self) -> Option<PaneId> {
        let pane_id = self.workspace.active_session().focused_pane_id().clone();
        self.workspace
            .active_session()
            .pane(&pane_id)
            .and_then(|pane| match pane.kind() {
                PaneKind::Agent { .. } => Some(pane_id.clone()),
                _ => None,
            })
    }

    fn focused_agent_has_pending_approval(&self) -> bool {
        let focused = self.workspace.active_session().focused_pane_id();
        self.runtime
            .agent_view(focused)
            .is_some_and(|runtime| runtime.pending_approval.is_some())
    }

    fn new_agent_pane(&mut self) {
        let intent = AgentPaneIntent::draft(self.agent_objective.clone());
        let objective = intent.objective.clone();
        let cwd = Some(self.command_context.project_path.clone());
        match self.workspace.apply_action(CoreAction::CreateAgentPane {
            title: "agent".to_owned(),
            intent,
            cwd,
        }) {
            Ok(ActionOutcome::Mutated { focused_pane }) => {
                self.timeline.record(TimelineEventKind::PaneCreated {
                    pane: focused_pane.to_string(),
                    kind: "agent".to_owned(),
                });
                self.status = format!(
                    "agent pane {focused_pane} created: {}",
                    objective_status_summary(&objective)
                );
            }
            Ok(ActionOutcome::PersistenceRequested(_)) => {
                self.status = "agent pane creation unexpectedly requested persistence".to_owned();
            }
            Err(error) => {
                self.status = format!("agent pane creation failed: {error}");
            }
        }
    }

    fn start_focused_agent(&mut self) {
        let pane_id = match self.focused_agent_pane_id() {
            Some(pane_id) => pane_id,
            None if self.agent_pane_ids().is_empty() => {
                // No agent pane anywhere: create one with the configured
                // default objective, then start it.
                self.new_agent_pane();
                match self.focused_agent_pane_id() {
                    Some(pane_id) => pane_id,
                    None => return,
                }
            }
            None => {
                self.status = "focused pane is not an agent pane".to_owned();
                return;
            }
        };

        let Some((objective, cwd)) =
            self.workspace
                .active_session()
                .pane(&pane_id)
                .and_then(|pane| match pane.kind() {
                    PaneKind::Agent { intent } => {
                        Some((intent.objective.clone(), pane.cwd().cloned()))
                    }
                    _ => None,
                })
        else {
            self.status = format!("agent pane {pane_id} was not found");
            return;
        };

        let Some(connector) = self.agent_connector.as_deref() else {
            // A refused launch leaves a durable trace, not just a status line.
            self.timeline.record(TimelineEventKind::AgentLaunchRefused {
                pane: pane_id.to_string(),
                reason: "no agent connector is configured".to_owned(),
            });
            self.status = "no agent connector is configured; set agent_connector to fake or claude"
                .to_owned();
            return;
        };

        let cwd = cwd.unwrap_or_else(|| self.command_context.project_path.clone());
        let mut spec = AgentLaunchSpec::new(objective.clone(), cwd);
        spec.model = self.agent_model.clone();
        let launched = connector.launch(&spec);
        match launched {
            Ok(session) => {
                // Replace the previous runtime only now that the new session
                // exists: shut it down and bump the pane's restart generation
                // (the pane is focused in every path here) so its buffered
                // events can never match again. Bumping before a launch that
                // then fails would leave the old runtime live under a retired
                // generation, making its events overwrite durable truth
                // ([L3-GATE]).
                let replacing = self.runtime.has_agent(&pane_id);
                if replacing {
                    let _ = self.workspace.apply_action(CoreAction::RestartFocused);
                }
                let restart_generation = self.pane_restart_generation(&pane_id);
                self.runtime
                    .replace_agent(pane_id.clone(), restart_generation, session);
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::Running;
                    intent.pending_approvals = 0;
                    intent.pending_approval_ids.clear();
                });
                self.timeline.record(TimelineEventKind::AgentStatus {
                    pane: pane_id.to_string(),
                    status: "running".to_owned(),
                });
                self.status = format!(
                    "agent {pane_id} started: {}",
                    objective_status_summary(&objective)
                );
            }
            Err(error) if self.runtime.has_agent(&pane_id) => {
                // A failed relaunch never touches the previous session: it
                // stays live and authoritative under its unchanged
                // generation, and durable intent keeps reflecting it.
                self.timeline.record(TimelineEventKind::AgentLaunchRefused {
                    pane: pane_id.to_string(),
                    reason: format!("relaunch failed: {error}; previous session still running"),
                });
                self.status = format!(
                    "agent relaunch failed for {pane_id}: {error}; previous session still running"
                );
            }
            Err(error) => {
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::Failed;
                });
                self.timeline.record(TimelineEventKind::AgentLaunchRefused {
                    pane: pane_id.to_string(),
                    reason: error.to_string(),
                });
                self.status = format!("agent launch failed for {pane_id}: {error}");
            }
        }
    }

    fn stop_focused_agent(&mut self) {
        let Some(pane_id) = self.focused_agent_pane_id() else {
            self.status = "focused pane is not an agent pane".to_owned();
            return;
        };
        if !self.runtime.stop_agent(&pane_id) {
            self.status = format!("agent {pane_id} is not running");
            return;
        }
        // An interrupted session has no known outcome; terminal states the
        // session already reported stay as they are.
        self.update_agent_intent(&pane_id, AgentPaneIntent::detach_live_session);
        self.status = format!("agent {pane_id} stopped");
    }

    fn decide_focused_agent_approval(&mut self, approved: bool) {
        let Some(pane_id) = self.focused_agent_pane_id() else {
            self.status = "focused pane is not an agent pane".to_owned();
            return;
        };
        match self.runtime.decide_agent_approval(&pane_id, approved) {
            Ok(request) => {
                self.update_agent_intent(&pane_id, |intent| {
                    intent.pending_approvals = 0;
                    intent.pending_approval_ids.clear();
                    intent.status = AgentStatus::Running;
                    intent.approval_history.push(AgentApprovalRecord {
                        approval_id: request.approval_id.clone(),
                        command: request.command.clone(),
                        approved,
                    });
                });
                let verdict_label = if approved { "approved" } else { "rejected" };
                self.timeline.record(TimelineEventKind::ApprovalDecided {
                    pane: pane_id.to_string(),
                    command: request.command.clone(),
                    verdict: verdict_label.to_owned(),
                    decided_by: "user".to_owned(),
                });
                self.status = format!("{verdict_label} '{}' for {pane_id}", request.command);
            }
            Err(AgentApprovalError::NotRunning) => {
                self.status = format!("agent {pane_id} is not running")
            }
            Err(AgentApprovalError::NoPendingApproval) => {
                self.status = format!("agent {pane_id} has no pending approval")
            }
            Err(AgentApprovalError::DecisionFailed(error)) => {
                self.status = format!("approval decision failed for {pane_id}: {error}")
            }
        }
    }

    fn focus_next_waiting_agent(&mut self) {
        let waiting = {
            let session = self.workspace.active_session();
            let order = session.focus_order();
            let focused_index = order
                .iter()
                .position(|pane_id| pane_id == session.focused_pane_id())
                .unwrap_or(0);
            (1..=order.len())
                .map(|offset| &order[(focused_index + offset) % order.len()])
                .find(|pane_id| self.pane_waiting_for_approval(pane_id))
                .cloned()
        };
        match waiting {
            Some(pane_id) => match self.workspace.apply_action(CoreAction::FocusPane {
                pane_id: pane_id.clone(),
            }) {
                Ok(_) => self.status = format!("focused waiting agent {pane_id}"),
                Err(error) => self.status = format!("focus failed: {error}"),
            },
            None => self.status = "no agent is waiting for approval".to_owned(),
        }
    }

    fn pane_waiting_for_approval(&self, pane_id: &PaneId) -> bool {
        self.workspace
            .active_session()
            .pane(pane_id)
            .is_some_and(|pane| {
                matches!(
                    pane.kind(),
                    PaneKind::Agent { intent } if intent.status == AgentStatus::WaitingForApproval
                )
            })
    }

    fn apply_agent_runtime_event(&mut self, runtime_event: AgentRuntimeEvent) {
        // [L3-GATE] The RuntimeEngine authenticates every event against the
        // current generation and token before durable intent can change.
        if let Some((pane_id, event)) = self.runtime.accept_agent_event(runtime_event) {
            self.apply_agent_event(pane_id, event);
        }
    }

    /// Fold one accepted agent session event into state: the durable subset
    /// (status, summary, changed files, approval count/ids) into the pane's
    /// `AgentPaneIntent`. RuntimeEngine has already authenticated the event and
    /// folded its live-only detail before this method runs.
    fn apply_agent_event(&mut self, pane_id: PaneId, event: AgentSessionEvent) {
        match event {
            AgentSessionEvent::Status(status) => {
                let label = agent_status_label(&status);
                // Record only real transitions (the connector may restate
                // the status the launch path already recorded).
                let changed = self
                    .workspace
                    .active_session()
                    .pane(&pane_id)
                    .is_some_and(|pane| {
                        !matches!(pane.kind(), PaneKind::Agent { intent } if intent.status == status)
                    });
                if changed {
                    self.timeline.record(TimelineEventKind::AgentStatus {
                        pane: pane_id.to_string(),
                        status: label.to_owned(),
                    });
                }
                self.update_agent_intent(&pane_id, |intent| intent.status = status);
                self.status = format!("agent {pane_id} is {label}");
            }
            AgentSessionEvent::Action { .. } => {}
            AgentSessionEvent::Summary(summary) => {
                self.update_agent_intent(&pane_id, |intent| {
                    intent.latest_summary = Some(summary);
                });
            }
            AgentSessionEvent::OutputChunk(_) | AgentSessionEvent::CommandRun { .. } => {}
            AgentSessionEvent::FilesChanged(changes) => {
                self.update_agent_intent(&pane_id, |intent| {
                    for change in changes {
                        if !intent.changed_files.contains(&change.path) {
                            intent.changed_files.push(change.path);
                        }
                    }
                });
            }
            AgentSessionEvent::ApprovalRequested(request) => {
                self.status = format!("agent {pane_id} requests approval: {}", request.command);
                let approval_id = request.approval_id.clone();
                // Timeline fact: what was asked, over what scope, at what
                // assessed risk (durable copies of the request detail).
                let scope = match &request.scope.affected_path {
                    Some(path) => format!("{} -> {}", request.scope.cwd.display(), path.display()),
                    None => request.scope.cwd.display().to_string(),
                };
                self.timeline.record(TimelineEventKind::ApprovalRequested {
                    pane: pane_id.to_string(),
                    command: request.command.clone(),
                    scope,
                    risk: format!("{:?} ({})", request.risk.level, request.risk.basis)
                        .to_lowercase(),
                });
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::WaitingForApproval;
                    intent.pending_approvals = 1;
                    intent.pending_approval_ids = vec![approval_id];
                });
            }
            AgentSessionEvent::Completed { summary } => {
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::Complete;
                    intent.latest_summary = Some(summary);
                });
                self.timeline.record(TimelineEventKind::AgentStatus {
                    pane: pane_id.to_string(),
                    status: "complete".to_owned(),
                });
                self.status = format!("agent {pane_id} completed");
            }
            AgentSessionEvent::Failed { error } => {
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::Failed;
                });
                self.timeline.record(TimelineEventKind::AgentStatus {
                    pane: pane_id.to_string(),
                    status: "failed".to_owned(),
                });
                self.status = format!("agent {pane_id} failed: {error}");
            }
            AgentSessionEvent::Closed => {
                // A session that closed without reporting an outcome has an
                // unknown durable state.
                self.update_agent_intent(&pane_id, AgentPaneIntent::detach_live_session);
            }
        }
    }

    fn update_agent_intent(&mut self, pane_id: &PaneId, update: impl FnOnce(&mut AgentPaneIntent)) {
        if let Some(intent) = self
            .workspace
            .active_session_mut()
            .agent_intent_mut(pane_id)
        {
            update(intent);
        }
    }

    fn agent_pane_ids(&self) -> BTreeSet<PaneId> {
        self.workspace
            .active_session()
            .panes()
            .iter()
            .filter(|(_, pane)| matches!(pane.kind(), PaneKind::Agent { .. }))
            .map(|(pane_id, _)| pane_id.clone())
            .collect()
    }

    /// Runtime registries are intentionally active-session-only and keyed by
    /// pane id. Pane ids repeat across sessions (`pane-1`, ...), so every
    /// active-session switch must retire the old registry contents before
    /// ordinary reconciliation can attach runtimes to the new session. This
    /// is the session boundary for L3; a same-id pane never inherits another
    /// session's process, parser, task state, or agent actor.
    fn retire_runtimes_for_session_switch(&mut self, previous_session_id: &SessionId) {
        let _report = self
            .runtime
            .retire_session(&mut self.workspace, previous_session_id);
        self.copy_mode = None;
        self.pointer_view = None;
        self.context_menu = None;
        self.objective_prompt = None;
    }

    /// The live agent runtime view for a pane, if a session is attached.
    pub(crate) fn agent_runtime_view(&self, pane_id: &PaneId) -> Option<AgentRuntimeView<'_>> {
        self.runtime.agent_view(pane_id)
    }

    #[cfg(test)]
    pub(crate) fn set_agent_connector(&mut self, connector: Box<dyn AgentConnector>) {
        self.agent_connector = Some(connector);
    }

    /// Test-only: retain a task status (the path a real exit or launch
    /// failure writes) without needing a live PTY.
    #[cfg(test)]
    pub(crate) fn set_task_status_for_test(&mut self, pane_id: &PaneId, status: &str) {
        self.runtime
            .set_task_status(pane_id.clone(), status.to_owned());
        let exit = status
            .strip_prefix("failed: exit ")
            .and_then(|code| code.parse().ok())
            .map(|code| mandatum_pty::ChildExitStatus::Exited { code })
            .or_else(|| {
                status
                    .strip_prefix("failed: signal ")
                    .and_then(|signal| signal.parse().ok())
                    .map(|signal| mandatum_pty::ChildExitStatus::Signaled { signal })
            })
            .or_else(|| {
                (status == "failed: unknown exit").then_some(mandatum_pty::ChildExitStatus::Unknown)
            });
        if let Some(exit) = exit {
            self.runtime.record_task_failure(
                pane_id.clone(),
                TaskInvestigationFailure::ProcessExit(exit),
                status.to_owned(),
            );
        } else {
            self.runtime.clear_task_failure(pane_id);
        }
    }

    fn launch_task_pane(
        &mut self,
        pane_id: PaneId,
        intent: &TaskPaneIntent,
    ) -> Result<(), RuntimeReconcileError> {
        let outcome = self.runtime.launch_task(
            &self.workspace,
            &self.shell_program,
            pane_id.clone(),
            self.visible_task_size(&pane_id),
            self.spawn_pty,
            TaskAttempt::Initial,
        );
        self.status = match outcome {
            TaskLaunchOutcome::Disabled => {
                format!("task pane {pane_id} created; PTY spawning is disabled")
            }
            TaskLaunchOutcome::Deferred => {
                format!(
                    "task pane {pane_id} created; pending launch: waiting for visible pane size"
                )
            }
            TaskLaunchOutcome::Running => {
                self.timeline.record(TimelineEventKind::TaskStarted {
                    pane: pane_id.to_string(),
                    command: intent.command.clone(),
                });
                format!("task {pane_id} running: {}", intent.command)
            }
            TaskLaunchOutcome::Failed(status) => status,
        };
        Ok(())
    }

    fn visible_task_size(&self, pane_id: &PaneId) -> Option<PtySize> {
        self.visible_task_pane_sizes()
            .into_iter()
            .find_map(|(candidate, size)| (candidate == *pane_id).then_some(size))
    }

    fn reconcile_runtimes(&mut self) -> Result<(), RuntimeReconcileError> {
        let visible_terminals = self.visible_terminal_pane_sizes();
        let visible_tasks = self.visible_task_pane_sizes();
        let notices = self.runtime.reconcile(
            &mut self.workspace,
            &self.shell_program,
            self.spawn_pty,
            visible_terminals,
            visible_tasks,
        )?;
        for notice in notices {
            match notice {
                RuntimeReconcileNotice::TerminalSpawned(pane_id) => {
                    self.status = format!(
                        "spawned shell for {} · {pane_id}",
                        pane_status_name(&self.workspace, &pane_id)
                    );
                }
                RuntimeReconcileNotice::TerminalRestarted(pane_id) => {
                    if self
                        .copy_mode
                        .as_ref()
                        .is_some_and(|state| state.pane_id == pane_id)
                    {
                        self.copy_mode = None;
                    }
                    if self
                        .pointer_view
                        .as_ref()
                        .is_some_and(|view| view.pane_id == pane_id)
                    {
                        self.pointer_view = None;
                    }
                    self.status = format!("restarted shell for {pane_id}");
                }
                RuntimeReconcileNotice::TaskStarted(pane_id) => {
                    self.timeline.record(TimelineEventKind::TaskStarted {
                        pane: pane_id.to_string(),
                        command: self.task_command_for(&pane_id).unwrap_or_default(),
                    });
                    self.status = format!("task {pane_id} running");
                }
            }
        }
        Ok(())
    }

    fn pane_restart_generation(&self, pane_id: &PaneId) -> u64 {
        self.workspace
            .active_session()
            .pane(pane_id)
            .map(|pane| pane.restart_generation())
            .unwrap_or(0)
    }

    fn visible_terminal_pane_sizes(&self) -> Vec<(PaneId, PtySize)> {
        self.visible_terminal_pane_sizes_for_workspace(&self.workspace)
    }

    fn visible_terminal_pane_sizes_for_workspace(
        &self,
        workspace: &Workspace,
    ) -> Vec<(PaneId, PtySize)> {
        self.visible_pane_sizes_for_workspace(workspace, |kind| {
            matches!(kind, PaneKind::Terminal { .. })
        })
    }

    fn visible_task_pane_sizes(&self) -> Vec<(PaneId, PtySize)> {
        self.visible_pane_sizes_for_workspace(&self.workspace, |kind| {
            matches!(kind, PaneKind::Task { .. })
        })
    }

    fn visible_pane_sizes_for_workspace(
        &self,
        workspace: &Workspace,
        include_kind: impl Fn(&PaneKind) -> bool,
    ) -> Vec<(PaneId, PtySize)> {
        let Some((columns, rows)) = self.terminal_size else {
            return Vec::new();
        };
        let frame = SceneSize::new(columns, rows);
        let session = workspace.active_session();

        session
            .panes()
            .iter()
            .filter_map(|(pane_id, pane)| {
                if !include_kind(pane.kind()) {
                    return None;
                }

                let content_area = pane_content_rect(workspace, frame, pane_id)?;
                let size =
                    PtySize::new(content_area.width.max(1), content_area.height.max(1)).ok()?;
                Some((pane_id.clone(), size))
            })
            .collect()
    }

    #[cfg(test)]
    fn terminal_pane_ids(&self) -> BTreeSet<PaneId> {
        self.terminal_pane_ids_for_workspace(&self.workspace)
    }

    #[cfg(test)]
    fn terminal_pane_ids_for_workspace(&self, workspace: &Workspace) -> BTreeSet<PaneId> {
        workspace
            .active_session()
            .panes()
            .iter()
            .filter(|(_, pane)| matches!(pane.kind(), PaneKind::Terminal { .. }))
            .map(|(pane_id, _)| pane_id.clone())
            .collect()
    }

    /// The durable command string of a task pane, for timeline facts.
    fn task_command_for(&self, pane_id: &PaneId) -> Option<String> {
        self.workspace
            .active_session()
            .pane(pane_id)
            .and_then(|pane| match pane.kind() {
                PaneKind::Task { intent } => Some(intent.command.clone()),
                _ => None,
            })
    }

    fn apply_pty_runtime_event(&mut self, event: PtyRuntimeEvent) {
        let Some(effect) = self.runtime.apply_pty_event(event) else {
            return;
        };
        match effect {
            RuntimePtyEffect::TerminalRead { pane_id, bytes } if self.debug_status => {
                self.status = format!("read {bytes} byte(s) from {pane_id}")
            }
            RuntimePtyEffect::TaskRead { pane_id, bytes } if self.debug_status => {
                self.status = format!("read {bytes} task byte(s) from {pane_id}")
            }
            RuntimePtyEffect::TerminalParserFailed { pane_id, error } => {
                self.status = format!("terminal parser failed for {pane_id}: {error}")
            }
            RuntimePtyEffect::TaskParserFailed { pane_id, error } => {
                self.status = format!("task parser failed for {pane_id}: {error}")
            }
            RuntimePtyEffect::ReaderClosed { pane_id } => {
                self.status = format!("PTY reader closed for {pane_id}")
            }
            RuntimePtyEffect::ReaderFailed { pane_id, error } => {
                self.status = format!("PTY reader failed for {pane_id}: {error}")
            }
            RuntimePtyEffect::TerminalRead { .. } | RuntimePtyEffect::TaskRead { .. } => {}
        }
    }

    /// Heartbeat work: notice exited children. Called from `tick_runtime`
    /// and on the shell's heartbeat cadence rather than per event.
    pub(crate) fn poll_child_exits(&mut self) {
        for effect in self.runtime.poll_child_exits() {
            match effect {
                RuntimeExitEffect::TerminalExited { pane_id, status } => {
                    self.status = format!(
                        "{} exited: {} · {pane_id}",
                        pane_status_name(&self.workspace, &pane_id),
                        exit_status_label(status)
                    );
                }
                RuntimeExitEffect::TerminalWaitFailed { pane_id, error } => {
                    self.status = format!("PTY wait failed for {pane_id}: {error}");
                }
                RuntimeExitEffect::TaskExited { pane_id, status } => {
                    let command = self.task_command_for(&pane_id).unwrap_or_default();
                    self.timeline.record(TimelineEventKind::TaskExited {
                        pane: pane_id.to_string(),
                        command,
                        exit: status.clone(),
                    });
                    self.status = format!(
                        "{} {} · {pane_id}",
                        pane_status_name(&self.workspace, &pane_id),
                        status
                    );
                }
                RuntimeExitEffect::TaskWaitFailed { pane_id, error } => {
                    self.status = format!("task wait failed for {pane_id}: {error}");
                }
            }
        }
    }

    /// The live terminal grid attached to a pane, if any.
    pub(crate) fn terminal_grid(&self, pane_id: &PaneId) -> Option<&TerminalGrid> {
        self.runtime.terminal_grid(pane_id)
    }

    /// The live task runtime view for a pane: its status label plus the
    /// output grid when a runtime is attached. Falls back to the retained
    /// status of a stopped/pending task.
    pub(crate) fn task_view(&self, pane_id: &PaneId) -> Option<(&str, Option<&TerminalGrid>)> {
        self.runtime.task_view(pane_id)
    }

    /// How a pane's grid is being viewed: copy-mode scroll/selection/cursor
    /// for the copy-mode pane, pointer scroll/selection next, following live
    /// output otherwise.
    pub(crate) fn pane_view_state(&self, pane_id: &PaneId) -> PaneViewState {
        match &self.copy_mode {
            Some(state) if &state.pane_id == pane_id => PaneViewState {
                scroll_offset: state.scroll_offset,
                selection: state.selection_span(),
                copy_cursor: Some((state.cursor_row, state.cursor_col)),
            },
            _ => match &self.pointer_view {
                Some(view) if &view.pane_id == pane_id => PaneViewState {
                    scroll_offset: view.scroll_offset,
                    selection: view.ordered_selection(),
                    // Pointer selection shows no block cursor; the selection
                    // itself is the feedback.
                    copy_cursor: None,
                },
                _ => PaneViewState::default(),
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    // --- Pointer routing ---------------------------------------------------

    /// Route one pointer event. The context menu is modal; otherwise events
    /// resolve against the last scene's hit targets, with child mouse
    /// capture honored ahead of workspace behaviors ([L5-GATE]).
    fn handle_pointer(&mut self, pointer: PointerEvent) {
        if self.context_menu.is_some() {
            self.handle_context_menu_pointer(pointer);
            self.mark_redraw();
            return;
        }

        match pointer.kind {
            PointerKind::Down => self.handle_pointer_down(pointer),
            PointerKind::Drag => self.handle_pointer_drag(pointer),
            PointerKind::Up => self.handle_pointer_up(pointer),
            PointerKind::Wheel { .. } => self.handle_pointer_wheel(pointer),
            // No hover behavior outside the menu.
            PointerKind::Move => return,
        }
        self.mark_redraw();
    }

    /// The topmost hit target of the last built scene under a point: the
    /// builder emits targets bottom-up, so the reverse scan wins overlaps.
    fn pointer_target(&self, column: u16, row: u16) -> Option<HitTarget> {
        self.hit_targets
            .iter()
            .rev()
            .find(|target| target.rect.contains(column, row))
            .cloned()
    }

    fn handle_pointer_down(&mut self, pointer: PointerEvent) {
        let target = self.pointer_target(pointer.column, pointer.row);

        // The palette is modal: its rows are clickable, anywhere else closes
        // it, and the press is consumed either way.
        if self.palette.is_some() {
            if let Some(HitTargetKind::PaletteItem(index)) =
                target.as_ref().map(|target| target.kind.clone())
            {
                self.activate_palette_item(index);
            } else {
                self.close_palette();
            }
            return;
        }

        // The visibility overlays are modal the same way: rows activate,
        // click-away dismisses, and the press is consumed.
        if self.objective_prompt.is_some() {
            self.objective_prompt = None;
            self.status = "objective unchanged".to_owned();
            return;
        }
        if self.help_view.is_some() {
            // Help rows are informational, not actionable: any press closes.
            self.close_help();
            return;
        }
        if self.timeline_view.is_some() {
            if let Some(HitTargetKind::TimelineItem(index)) =
                target.as_ref().map(|target| target.kind.clone())
            {
                if let Some(view) = self.timeline_view.as_mut() {
                    view.selected = index;
                }
                self.jump_to_selected_timeline_entry(Some(index));
            } else {
                self.close_timeline();
            }
            return;
        }
        if self.search_view.is_some() {
            if let Some(HitTargetKind::SearchItem(index)) =
                target.as_ref().map(|target| target.kind.clone())
            {
                if let Some(view) = self.search_view.as_mut() {
                    view.selected = index;
                }
                self.activate_search_hit(Some(index));
            } else {
                self.close_search();
            }
            return;
        }
        if self.session_map.is_some() {
            if let Some(HitTargetKind::SessionMapRow(index)) =
                target.as_ref().map(|target| target.kind.clone())
            {
                if let Some(map) = self.session_map.as_mut() {
                    map.selected = index;
                }
                self.activate_session_map_row(Some(index));
            } else {
                self.close_session_map();
            }
            return;
        }

        let Some(target) = target else {
            self.pointer_drag = None;
            return;
        };
        match (target.kind.clone(), pointer.button) {
            // The status strip is the workspace's own front door: clicking
            // it opens the command palette named in its permanent hint.
            (HitTargetKind::StatusStrip, Some(PointerButton::Left)) => self.open_palette(),
            // An attention segment jumps straight to the pane that needs
            // eyes (the keyboard route is Focus next waiting agent).
            (HitTargetKind::AttentionSegment { pane, .. }, Some(PointerButton::Left)) => match pane
            {
                Some(pane_id) => self.focus_pane_for_pointer(&pane_id),
                None => self.status = "this attention item has no single pane".to_owned(),
            },
            (HitTargetKind::PaneBody(pane_id), Some(button)) => {
                // [L5-GATE] The child's grid owns clicks while it tracks the
                // mouse; alt+click stays workspace control.
                if self.try_forward_pointer(&pane_id, target.rect, &pointer) {
                    return;
                }
                match button {
                    PointerButton::Left => self.pointer_down_on_body(pane_id, target.rect, pointer),
                    PointerButton::Right => self.open_context_menu(pane_id, pointer),
                    PointerButton::Middle => {}
                }
            }
            (HitTargetKind::PaneTitle(pane_id), Some(button)) => match button {
                // Pane chrome is workspace surface even when the child
                // captures the mouse inside its own grid.
                PointerButton::Left => self.pointer_down_on_title(pane_id, target.rect, pointer),
                PointerButton::Right => self.open_context_menu(pane_id, pointer),
                PointerButton::Middle => {}
            },
            (HitTargetKind::Separator { split_index, .. }, Some(PointerButton::Left)) => {
                self.begin_split_drag(split_index);
            }
            _ => {}
        }
    }

    /// [L5-GATE] If the child under the pointer requested mouse reporting
    /// and the user is not overriding it (alt, copy mode), the event belongs
    /// to the child: encode it and write it to that pane's PTY. Returns
    /// whether the child consumed the event.
    fn try_forward_pointer(
        &mut self,
        pane_id: &PaneId,
        inner: SceneRect,
        pointer: &PointerEvent,
    ) -> bool {
        if pointer.mods.alt || self.copy_mode.is_some() {
            return false;
        }
        let Some(mode) = self.runtime.terminal_mouse_mode(pane_id) else {
            return false;
        };
        if !mode.wants_mouse() {
            return false;
        }

        let column = pointer
            .column
            .saturating_sub(inner.x)
            .min(inner.width.saturating_sub(1));
        let row = pointer
            .row
            .saturating_sub(inner.y)
            .min(inner.height.saturating_sub(1));
        if let Some(bytes) = encode_mouse_event(mode, pointer, column, row)
            && let Err(error) = self.runtime.write_terminal(pane_id, &bytes)
        {
            self.status = format!("PTY mouse input failed for {pane_id}: {error}");
        }
        // The child that received the press owns the rest of the gesture.
        if pointer.kind == PointerKind::Down {
            self.pointer_forward = Some((pane_id.clone(), inner));
        }
        true
    }

    /// Forward a drag/release to the pane whose press was forwarded, even if
    /// the pointer has left its rect (button-capture semantics).
    fn forward_captured_pointer(
        &mut self,
        pane_id: &PaneId,
        inner: SceneRect,
        pointer: &PointerEvent,
    ) {
        if let Some(mode) = self.runtime.terminal_mouse_mode(pane_id) {
            let column = pointer
                .column
                .saturating_sub(inner.x)
                .min(inner.width.saturating_sub(1));
            let row = pointer
                .row
                .saturating_sub(inner.y)
                .min(inner.height.saturating_sub(1));
            if let Some(bytes) = encode_mouse_event(mode, pointer, column, row)
                && let Err(error) = self.runtime.write_terminal(pane_id, &bytes)
            {
                self.status = format!("PTY mouse input failed for {pane_id}: {error}");
            }
        }
        if pointer.kind == PointerKind::Up {
            self.pointer_forward = None;
        }
    }

    fn pointer_down_on_body(&mut self, pane_id: PaneId, inner: SceneRect, pointer: PointerEvent) {
        self.focus_pane_for_pointer(&pane_id);

        // Double-click with a command modifier toggles zoom without needing
        // the title row.
        if pointer.mods.has_command_modifier()
            && self.take_double_click(&pane_id, PaneClickTarget::Body)
        {
            self.dispatch(CommandId::ZoomPane);
            return;
        }

        // Begin a cell selection on a live terminal grid (the copy-mode
        // selection model, driven by the pointer). Copy mode owns its own
        // pane's viewport.
        if self
            .copy_mode
            .as_ref()
            .is_some_and(|state| state.pane_id == pane_id)
        {
            return;
        }
        let scroll_offset = match &self.pointer_view {
            Some(view) if view.pane_id == pane_id => view.scroll_offset,
            _ => 0,
        };
        let Some(cell) =
            self.cell_under_pointer(&pane_id, inner, scroll_offset, pointer.column, pointer.row)
        else {
            return;
        };
        self.pointer_view = Some(PointerView {
            pane_id: pane_id.clone(),
            scroll_offset,
            selection: Some((cell, cell)),
        });
        self.pointer_drag = Some(PointerDrag::Select { pane_id, inner });
    }

    fn pointer_down_on_title(&mut self, pane_id: PaneId, title: SceneRect, pointer: PointerEvent) {
        self.focus_pane_for_pointer(&pane_id);

        if self.take_double_click(&pane_id, PaneClickTarget::Title) {
            self.pointer_drag = None;
            self.dispatch(CommandId::ZoomPane);
            return;
        }

        // Grab a floating pane by its title to move it.
        let layout = self.workspace.active_session().layout();
        if layout.is_floating(&pane_id) && layout.zoomed().is_none() {
            self.pointer_drag = Some(PointerDrag::MoveFloat {
                pane_id,
                grab_dx: pointer.column.saturating_sub(title.x),
                grab_dy: pointer.row.saturating_sub(title.y),
            });
        }
    }

    fn begin_split_drag(&mut self, split_index: usize) {
        let Some((columns, rows)) = self.terminal_size else {
            return;
        };
        let area = workspace_scene_area(SceneSize::new(columns, rows));
        let Some(separator) = layout_separators(&self.workspace, area)
            .into_iter()
            .find(|separator| separator.split_index == split_index)
        else {
            return;
        };
        self.pointer_drag = Some(PointerDrag::ResizeSplit {
            split_index,
            axis: separator.axis,
            split_area: separator.split_area,
        });
    }

    fn handle_pointer_drag(&mut self, pointer: PointerEvent) {
        // [L5-GATE] A child that captured the press keeps the whole gesture.
        if let Some((pane_id, inner)) = self.pointer_forward.clone() {
            self.forward_captured_pointer(&pane_id, inner, &pointer);
            return;
        }

        match self.pointer_drag.clone() {
            Some(PointerDrag::ResizeSplit {
                split_index,
                axis,
                split_area,
            }) => self.drag_split(split_index, axis, split_area, pointer),
            Some(PointerDrag::MoveFloat {
                pane_id,
                grab_dx,
                grab_dy,
            }) => self.drag_float(pane_id, grab_dx, grab_dy, pointer),
            Some(PointerDrag::Select { pane_id, inner }) => {
                self.drag_selection(pane_id, inner, pointer)
            }
            None => {}
        }
    }

    /// Live drag-resize: every drag event lands as durable layout intent, so
    /// the next frame draws the moved boundary and PTYs re-fit immediately.
    fn drag_split(
        &mut self,
        split_index: usize,
        axis: SplitAxis,
        split_area: SceneRect,
        pointer: PointerEvent,
    ) {
        let Some(percent) =
            split_percent_for_pointer(axis, split_area, pointer.column, pointer.row)
        else {
            return;
        };
        match self.workspace.apply_action(CoreAction::SetSplitRatio {
            split_index,
            first_percent: percent,
        }) {
            Ok(_) => {
                self.status = format!("split resized to {percent}%");
                if let Err(error) = self.reconcile_runtimes() {
                    self.status = error.to_string();
                }
            }
            Err(error) => self.status = format!("split resize failed: {error}"),
        }
    }

    /// Keyboard float movement (the pointer path is `drag_float`): shift the
    /// focused floating pane's durable rect one step, clamped to the same
    /// bounds the drag path enforces.
    fn move_focused_float(&mut self, dx: i32, dy: i32) {
        let session = self.workspace.active_session();
        let pane_id = session.focused_pane_id().clone();
        let Some(rect) = session
            .layout()
            .floating()
            .iter()
            .find(|floating| floating.pane_id == pane_id)
            .map(|floating| floating.rect.clone())
        else {
            self.status = "focused pane is not floating (Float pane first)".to_owned();
            return;
        };
        // The same clamp the drag path applies; without a known frame size
        // there is nothing to clamp against beyond zero.
        let (max_x, max_y) = match self.terminal_size {
            Some((columns, rows)) => {
                let area = workspace_scene_area(SceneSize::new(columns, rows));
                (
                    i32::from(area.width.saturating_sub(2)),
                    i32::from(area.height.saturating_sub(2)),
                )
            }
            None => (i32::from(u16::MAX), i32::from(u16::MAX)),
        };
        let x = (i32::from(rect.x) + dx).clamp(0, max_x) as u16;
        let y = (i32::from(rect.y) + dy).clamp(0, max_y) as u16;
        match self.workspace.apply_action(CoreAction::MoveFloatingPane {
            pane_id: pane_id.clone(),
            x,
            y,
        }) {
            Ok(_) => {
                self.status = format!("moved {pane_id}");
                if let Err(error) = self.reconcile_runtimes() {
                    self.status = error.to_string();
                }
            }
            Err(error) => self.status = format!("move failed: {error}"),
        }
    }

    fn drag_float(&mut self, pane_id: PaneId, grab_dx: u16, grab_dy: u16, pointer: PointerEvent) {
        let Some((columns, rows)) = self.terminal_size else {
            return;
        };
        let area = workspace_scene_area(SceneSize::new(columns, rows));
        if area.is_empty() {
            return;
        }
        let screen_x = pointer.column.saturating_sub(grab_dx).max(area.x);
        let screen_y = pointer.row.saturating_sub(grab_dy).max(area.y);
        let x = (screen_x - area.x).min(area.width.saturating_sub(2));
        let y = (screen_y - area.y).min(area.height.saturating_sub(2));
        match self.workspace.apply_action(CoreAction::MoveFloatingPane {
            pane_id: pane_id.clone(),
            x,
            y,
        }) {
            Ok(_) => {
                self.status = format!("moved {pane_id}");
                if let Err(error) = self.reconcile_runtimes() {
                    self.status = error.to_string();
                }
            }
            Err(error) => self.status = format!("move failed: {error}"),
        }
    }

    fn drag_selection(&mut self, pane_id: PaneId, inner: SceneRect, pointer: PointerEvent) {
        let scroll_offset = match &self.pointer_view {
            Some(view) if view.pane_id == pane_id => view.scroll_offset,
            _ => 0,
        };
        let Some(cell) =
            self.cell_under_pointer(&pane_id, inner, scroll_offset, pointer.column, pointer.row)
        else {
            return;
        };
        if let Some(view) = self.pointer_view.as_mut()
            && view.pane_id == pane_id
            && let Some(selection) = view.selection.as_mut()
        {
            selection.1 = cell;
        }
    }

    fn handle_pointer_up(&mut self, pointer: PointerEvent) {
        // [L5-GATE] Deliver the release to the child that got the press.
        if let Some((pane_id, inner)) = self.pointer_forward.clone() {
            self.forward_captured_pointer(&pane_id, inner, &pointer);
            return;
        }

        match self.pointer_drag.take() {
            Some(PointerDrag::Select { pane_id, .. }) => {
                let mut drop_view = false;
                let mut kept_selection = false;
                if let Some(view) = self.pointer_view.as_mut()
                    && view.pane_id == pane_id
                {
                    match view.selection {
                        // A press without movement is a plain click.
                        Some((anchor, cursor)) if anchor == cursor => view.selection = None,
                        Some(_) => kept_selection = true,
                        None => {}
                    }
                    drop_view = view.scroll_offset == 0 && view.selection.is_none();
                }
                if drop_view {
                    self.pointer_view = None;
                }
                if kept_selection {
                    self.status = "selection ready: Copy Selection copies it".to_owned();
                }
            }
            Some(PointerDrag::ResizeSplit { .. } | PointerDrag::MoveFloat { .. }) | None => {}
        }
    }

    fn handle_pointer_wheel(&mut self, pointer: PointerEvent) {
        let PointerKind::Wheel { dy, .. } = pointer.kind else {
            return;
        };
        // The palette is modal: the wheel moves its selection (the item
        // window follows), so every entry is reachable by mouse.
        if self.palette.is_some() {
            if dy != 0 {
                self.move_palette_selection(isize::from(dy));
            }
            return;
        }
        // Same for the visibility overlays.
        if self.timeline_view.is_some() {
            if dy != 0 {
                self.move_timeline_selection(isize::from(dy));
            }
            return;
        }
        if self.search_view.is_some() {
            if dy != 0 {
                self.move_search_selection(isize::from(dy));
            }
            return;
        }
        if self.session_map.is_some() {
            if dy != 0 {
                self.move_session_map_selection(isize::from(dy));
            }
            return;
        }
        if self.help_view.is_some() {
            if dy != 0 {
                self.move_help_selection(isize::from(dy));
            }
            return;
        }
        if self.objective_prompt.is_some() {
            return;
        }
        let Some(target) = self.pointer_target(pointer.column, pointer.row) else {
            return;
        };
        let HitTargetKind::PaneBody(pane_id) = target.kind.clone() else {
            return;
        };

        // [L5-GATE] The wheel belongs to a mouse-capturing child.
        if self.try_forward_pointer(&pane_id, target.rect, &pointer) {
            return;
        }

        // Only vertical wheel scrolls the workspace viewport.
        if dy == 0 {
            return;
        }

        // In copy mode the wheel moves the copy cursor, which scrolls.
        if self
            .copy_mode
            .as_ref()
            .is_some_and(|state| state.pane_id == pane_id)
        {
            if !self.runtime.has_terminal(&pane_id) {
                return;
            }
            let state = self.copy_mode.as_mut().expect("copy mode present");
            let grid = self
                .runtime
                .terminal_grid(&pane_id)
                .expect("runtime present");
            let step = WHEEL_SCROLL_ROWS * usize::from(dy.unsigned_abs());
            if dy < 0 {
                state.move_up(step, grid);
            } else {
                state.move_down(step, grid);
            }
            return;
        }

        // Plain wheel: viewport scrollback without entering copy mode. The
        // pointer view already renders through the copy-mode windowing math,
        // so this is the same viewing mechanism minus the modal keymap.
        let Some(grid) = self.terminal_grid(&pane_id) else {
            return;
        };
        let view_rows = usize::from(grid.size().rows().min(target.rect.height));
        let max_top = grid.total_rows().saturating_sub(view_rows);
        let step = WHEEL_SCROLL_ROWS * usize::from(dy.unsigned_abs());
        let (current, selection) = match &self.pointer_view {
            Some(view) if view.pane_id == pane_id => (view.scroll_offset, view.selection),
            _ => (0, None),
        };
        let scroll_offset = if dy < 0 {
            (current + step).min(max_top)
        } else {
            current.saturating_sub(step)
        };

        if scroll_offset == 0 && selection.is_none() {
            self.pointer_view = None;
            self.status = "following live output".to_owned();
        } else {
            self.pointer_view = Some(PointerView {
                pane_id,
                scroll_offset,
                selection,
            });
            self.status = format!("scrollback: {scroll_offset} row(s) up");
        }
    }

    fn focus_pane_for_pointer(&mut self, pane_id: &PaneId) {
        if self.workspace.active_session().focused_pane_id() == pane_id {
            return;
        }
        match self.workspace.apply_action(CoreAction::FocusPane {
            pane_id: pane_id.clone(),
        }) {
            Ok(_) => {
                self.status = format!("focused {pane_id}");
                if let Err(error) = self.reconcile_runtimes() {
                    self.status = error.to_string();
                }
            }
            Err(error) => self.status = format!("focus failed: {error}"),
        }
    }

    /// Record a press and report whether it completed a double-click on the
    /// same pane and target within the window.
    fn take_double_click(&mut self, pane_id: &PaneId, target: PaneClickTarget) -> bool {
        let now = Instant::now();
        let double = self.last_pane_click.as_ref().is_some_and(|click| {
            &click.pane_id == pane_id
                && click.target == target
                && now.duration_since(click.at) <= DOUBLE_CLICK_WINDOW
        });
        self.last_pane_click = if double {
            None
        } else {
            Some(PaneClick {
                pane_id: pane_id.clone(),
                target,
                at: now,
            })
        };
        double
    }

    /// The absolute buffer cell under a pointer position inside a pane's
    /// inner rect, mirroring the scene builder's viewport windowing.
    fn cell_under_pointer(
        &self,
        pane_id: &PaneId,
        inner: SceneRect,
        scroll_offset: usize,
        column: u16,
        row: u16,
    ) -> Option<(usize, u16)> {
        let grid = self.terminal_grid(pane_id)?;
        let view_rows = usize::from(grid.size().rows().min(inner.height));
        if view_rows == 0 {
            return None;
        }
        let max_top = grid.total_rows().saturating_sub(view_rows);
        let first_row = max_top.saturating_sub(scroll_offset);
        let relative_row = usize::from(row.saturating_sub(inner.y)).min(view_rows - 1);
        let absolute_row = (first_row + relative_row).min(grid.total_rows().saturating_sub(1));
        let cell_column = column
            .saturating_sub(inner.x)
            .min(grid.size().columns().saturating_sub(1));
        Some((absolute_row, cell_column))
    }

    /// Copy the pointer selection through the copy-mode extraction model and
    /// request that the active frontend update its clipboard.
    fn copy_pointer_selection(&mut self) {
        let Some((pane_id, (anchor, cursor))) = self
            .pointer_view
            .as_ref()
            .and_then(|view| Some((view.pane_id.clone(), view.selection?)))
        else {
            self.status = "nothing is selected to copy".to_owned();
            return;
        };
        let Some(grid) = self.runtime.terminal_grid(&pane_id) else {
            self.status = format!("pane {pane_id} has no live terminal to copy from");
            return;
        };

        let extractor = CopyModeState {
            pane_id: pane_id.clone(),
            scroll_offset: 0,
            cursor_row: cursor.0,
            cursor_col: cursor.1,
            anchor: Some(anchor),
        };
        let text = extractor.selected_text(grid);
        self.frontend_effects
            .push(FrontendEffect::SetClipboard(text.clone()));
        let count = text.chars().count();
        self.last_copied = Some(text);
        let mut drop_view = false;
        if let Some(view) = self.pointer_view.as_mut() {
            view.selection = None;
            drop_view = view.scroll_offset == 0;
        }
        if drop_view {
            self.pointer_view = None;
        }
        self.status = format!("copied {count} char(s) to clipboard");
    }

    /// Click-dispatch for palette rows: same semantics as pressing Enter on
    /// the row (greyed rows surface their reason and keep the palette open).
    fn activate_palette_item(&mut self, index: usize) {
        let rows = self.current_palette_rows();
        let Some(row) = rows.get(index) else {
            return;
        };
        if !row.enabled {
            self.status = format!("{} is unavailable: {}", row.entry.label, row.entry.detail);
            return;
        }
        let command_id = row.command_id;
        self.palette = None;
        self.dispatch(command_id);
    }

    // --- Context menu ------------------------------------------------------

    /// Open the right-click menu for a pane: focus it, then list the
    /// commands relevant to its kind and runtime state. Every row carries
    /// the key chord that runs the same command from the keyboard.
    fn open_context_menu(&mut self, pane_id: PaneId, pointer: PointerEvent) {
        self.focus_pane_for_pointer(&pane_id);
        self.palette = None;
        let items = self.context_menu_items(&pane_id);
        if items.is_empty() {
            return;
        }
        self.context_menu = Some(ContextMenuState {
            items,
            selected: 0,
            anchor: (pointer.column, pointer.row),
        });
        self.status = "menu: up/down choose, Enter run, Esc close".to_owned();
    }

    fn context_menu_items(&self, pane_id: &PaneId) -> Vec<ContextMenuItem> {
        let Some(pane) = self.workspace.active_session().pane(pane_id) else {
            return Vec::new();
        };
        let floating = self
            .workspace
            .active_session()
            .layout()
            .is_floating(pane_id);

        let mut commands: Vec<CommandId> = Vec::new();
        match pane.kind() {
            PaneKind::Terminal { .. } => {
                commands.extend([
                    CommandId::EnterCopyMode,
                    CommandId::CopySelection,
                    CommandId::RestartPane,
                ]);
            }
            PaneKind::Task { .. } => {
                commands.extend([CommandId::RerunTask, CommandId::StopTask]);
                if self.task_failure_status(pane_id).is_some() {
                    commands.push(CommandId::InvestigateTaskFailure);
                }
            }
            PaneKind::Agent { .. } => {
                let live = self.runtime.agent_view(pane_id);
                if live.is_some_and(|runtime| runtime.pending_approval.is_some()) {
                    commands.extend([CommandId::ApproveAgentAction, CommandId::RejectAgentAction]);
                }
                if live.is_some() {
                    commands.push(CommandId::StopAgent);
                } else {
                    commands.push(CommandId::StartAgent);
                }
                commands.push(CommandId::SetAgentObjective);
            }
            PaneKind::StatusLog { .. } => {}
        }
        commands.push(CommandId::NewTerminal);
        // Splits address the tiled tree; a floating pane cannot be split.
        if !floating {
            commands.extend([CommandId::SplitRight, CommandId::SplitDown]);
        }
        // Zoom is one toggle too: name the half that will actually happen.
        let zoomed = self.workspace.active_session().layout().zoomed() == Some(pane_id);
        commands.push(CommandId::ZoomPane);
        // Float/dock is one toggle: offer the half that can actually run.
        commands.push(if floating {
            CommandId::DockPane
        } else {
            CommandId::FloatPane
        });
        commands.push(CommandId::ClosePane);
        // Session-wide, but pane output is what it searches: every pane's
        // menu offers the search door.
        commands.push(CommandId::SearchSession);
        // Help closes the loop for pointer-first users: every menu ends
        // with the door to the full keymap.
        commands.push(CommandId::ShowHelp);

        // "Command palette" leads: the menu is one of the two mouse doors
        // into the palette (the other is the status strip).
        let mut items = vec![ContextMenuItem {
            action: ContextMenuAction::OpenPalette,
            label: "Command palette".to_owned(),
            hint: format_chord(self.keymap.toggle_palette),
        }];
        items.extend(commands.into_iter().filter_map(|command_id| {
            let command = mandatum_commands::command_for_id(command_id)?;
            // State-aware labels: a toggle names the half that will run.
            let label = if command_id == CommandId::ZoomPane && zoomed {
                "Unzoom pane".to_owned()
            } else {
                command.label.to_owned()
            };
            Some(ContextMenuItem {
                action: ContextMenuAction::Command(command_id),
                label,
                hint: self.command_key_hint(command_id),
            })
        }));
        items
    }

    /// The keyboard route to a command, for menu and pane hints: a direct
    /// key where one exists, else its global chord, else its palette letter
    /// spelled as "<palette chord> <letter>".
    pub(crate) fn command_key_hint(&self, command_id: CommandId) -> String {
        if self.focused_agent_has_pending_approval() {
            if command_id == CommandId::ApproveAgentAction {
                return "y".to_owned();
            }
            if command_id == CommandId::RejectAgentAction {
                return "n".to_owned();
            }
        }
        if let Some(chord) = self.keymap.chord_for(command_id) {
            return format_chord(chord);
        }
        // Commands that ride another command's letter through a context
        // substitution show that letter: Dock rides Float's (one toggle key
        // for the pair), Rerun task rides Restart pane's (the task-pane
        // substitution).
        let letter_owner = match command_id {
            CommandId::DockPane => CommandId::FloatPane,
            CommandId::RerunTask => CommandId::RestartPane,
            other => other,
        };
        if let Some(letter) = self.keymap.palette.key_for(letter_owner) {
            return format!("{} {letter}", format_chord(self.keymap.toggle_palette));
        }
        String::new()
    }

    /// The context-menu overlay for the current frame, sized to its rows and
    /// clamped inside the frame.
    pub(crate) fn context_menu_overlay(&self, size: SceneSize) -> Option<ContextMenuOverlay> {
        let menu = self.context_menu.as_ref()?;
        let items: Vec<ContextMenuEntry> = menu
            .items
            .iter()
            .map(|item| ContextMenuEntry::new(item.label.clone(), item.hint.clone()))
            .collect();
        let widest = items
            .iter()
            .map(|entry| entry.label.chars().count() + 2 + entry.chord_hint.chars().count())
            .max()
            .unwrap_or(0) as u16;
        let width = widest.saturating_add(4);
        let height = (items.len() as u16).saturating_add(2);
        let area = context_menu_rect(menu.anchor.0, menu.anchor.1, width, height, size);
        Some(ContextMenuOverlay {
            area,
            items,
            selected: menu.selected,
        })
    }

    fn handle_context_menu_key(&mut self, key: Key) {
        if matches!(self.keymap.chord_action(key), Some(ChordAction::Quit)) {
            self.should_quit = true;
            self.status = "quitting".to_owned();
            return;
        }
        match key.code {
            KeyCode::Escape => self.close_context_menu(),
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(menu) = self.context_menu.as_mut() {
                    menu.selected = menu.selected.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(menu) = self.context_menu.as_mut() {
                    let last = menu.items.len().saturating_sub(1);
                    menu.selected = (menu.selected + 1).min(last);
                }
            }
            KeyCode::Enter => self.run_context_menu_item(None),
            _ => {}
        }
    }

    fn handle_context_menu_pointer(&mut self, pointer: PointerEvent) {
        match pointer.kind {
            // Hover follows the pointer over menu rows.
            PointerKind::Move | PointerKind::Drag => {
                if let Some(HitTargetKind::ContextMenuItem(index)) = self
                    .pointer_target(pointer.column, pointer.row)
                    .map(|target| target.kind)
                    && let Some(menu) = self.context_menu.as_mut()
                {
                    menu.selected = index;
                }
            }
            PointerKind::Down => {
                match self
                    .pointer_target(pointer.column, pointer.row)
                    .map(|target| target.kind)
                {
                    Some(HitTargetKind::ContextMenuItem(index)) => {
                        self.run_context_menu_item(Some(index));
                    }
                    // A press on the menu's own chrome (border, padding) is
                    // not a row and not click-away: swallowing it as a
                    // dismissal would punish a near-miss by one cell.
                    _ if self
                        .context_menu_area()
                        .is_some_and(|area| area.contains(pointer.column, pointer.row)) => {}
                    // Click-away dismisses; the press is consumed.
                    _ => self.close_context_menu(),
                }
            }
            PointerKind::Up | PointerKind::Wheel { .. } => {}
        }
    }

    fn run_context_menu_item(&mut self, index: Option<usize>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let index = index.unwrap_or(menu.selected);
        let Some(item) = menu.items.get(index) else {
            self.status = "menu closed".to_owned();
            return;
        };
        match item.action {
            ContextMenuAction::Command(command_id) => self.dispatch(command_id),
            ContextMenuAction::OpenPalette => self.open_palette(),
        }
    }

    fn close_context_menu(&mut self) {
        self.context_menu = None;
        self.status = "menu closed".to_owned();
    }

    /// The open menu's on-screen rect, for chrome-click detection.
    fn context_menu_area(&self) -> Option<SceneRect> {
        let (columns, rows) = self.terminal_size?;
        self.context_menu_overlay(SceneSize::new(columns, rows))
            .map(|overlay| overlay.area)
    }

    // --- Visibility overlays (timeline, session map, objective prompt) ----

    /// Open the execution timeline: read the durable tail once, newest
    /// first. The other modal surfaces close.
    fn open_timeline(&mut self) {
        self.palette = None;
        self.context_menu = None;
        self.search_view = None;
        self.session_map = None;
        self.help_view = None;
        self.objective_prompt = None;
        let view = TimelineViewState::from_tail(self.timeline.read_tail());
        self.status = format!("timeline: {} event(s)", view.events.len());
        self.timeline_view = Some(view);
    }

    fn close_timeline(&mut self) {
        self.timeline_view = None;
        self.status = "timeline closed".to_owned();
    }

    /// Open session search: snapshot every live pane's scrollback+screen
    /// text plus the timeline tail once, so results stay stable while panes
    /// flood. The other modal surfaces close; copy mode exits because search
    /// owns the keyboard while open.
    fn open_search(&mut self) {
        self.palette = None;
        self.context_menu = None;
        self.timeline_view = None;
        self.session_map = None;
        self.help_view = None;
        self.objective_prompt = None;
        self.copy_mode = None;
        let corpus = self.build_search_corpus();
        let view = SearchViewState::new(corpus);
        self.status = format!(
            "search: snapshot of {} pane source(s) + {} timeline event(s)",
            view.source_count(),
            view.timeline_event_count()
        );
        self.search_view = Some(view);
    }

    fn close_search(&mut self) {
        self.search_view = None;
        self.status = "search closed".to_owned();
    }

    /// Snapshot the searchable text: the active session's live terminal
    /// grids, task output grids, and agent output tails (session pane
    /// order), plus the timeline tail (newest first).
    fn build_search_corpus(&self) -> SearchCorpus {
        let session = self.workspace.active_session();
        let mut sources = Vec::new();
        for (pane_id, pane) in session.panes() {
            match pane.kind() {
                PaneKind::Terminal { .. } => {
                    if let Some(grid) = self.terminal_grid(pane_id) {
                        sources.push(SearchSource::from_grid(
                            pane_id.clone(),
                            pane.title(),
                            SearchSourceKind::Terminal,
                            grid,
                        ));
                    }
                }
                PaneKind::Task { .. } => {
                    if let Some(grid) = self.runtime.task_grid(pane_id) {
                        sources.push(SearchSource::from_grid(
                            pane_id.clone(),
                            pane.title(),
                            SearchSourceKind::Task,
                            grid,
                        ));
                    }
                }
                PaneKind::Agent { .. } => {
                    if let Some(runtime) = self.runtime.agent_view(pane_id) {
                        sources.push(SearchSource::from_lines(
                            pane_id.clone(),
                            pane.title(),
                            SearchSourceKind::Agent,
                            runtime.output_tail.iter(),
                        ));
                    }
                }
                PaneKind::StatusLog { .. } => {}
            }
        }
        let mut timeline = self.timeline.read_tail().events;
        timeline.reverse();
        SearchCorpus { sources, timeline }
    }

    fn handle_search_key(&mut self, key: Key) {
        if matches!(self.keymap.chord_action(key), Some(ChordAction::Quit)) {
            self.should_quit = true;
            self.status = "quitting".to_owned();
            return;
        }
        let ctrl_only = key.mods.control && !key.mods.shift && !key.mods.alt && !key.mods.super_key;
        if key.code == KeyCode::Up || (ctrl_only && key.code == KeyCode::Char('p')) {
            self.move_search_selection(-1);
            return;
        }
        if key.code == KeyCode::Down || (ctrl_only && key.code == KeyCode::Char('n')) {
            self.move_search_selection(1);
            return;
        }
        match key.code {
            KeyCode::Escape => self.close_search(),
            KeyCode::Enter => self.activate_search_hit(None),
            KeyCode::Backspace => {
                if let Some(view) = self.search_view.as_mut() {
                    view.query.pop();
                    view.refresh();
                }
            }
            KeyCode::Char(character)
                if !key.mods.control && !key.mods.alt && !key.mods.super_key =>
            {
                if let Some(view) = self.search_view.as_mut() {
                    view.query.push(character);
                    view.refresh();
                }
            }
            _ => {}
        }
    }

    fn move_search_selection(&mut self, delta: isize) {
        let Some(view) = self.search_view.as_mut() else {
            return;
        };
        let count = view.results.len();
        if count == 0 {
            view.selected = 0;
            return;
        }
        let current = view.selected.min(count - 1) as isize;
        view.selected = (current + delta).clamp(0, count as isize - 1) as usize;
    }

    /// Enter/click on a search result. Pane hits focus the pane; terminal
    /// hits also scroll the viewport to the matched row (the pointer-view
    /// mechanics, so plain typing keeps flowing to the shell — L5). Timeline
    /// hits open the timeline overlay positioned at the matched entry.
    /// `index` overrides the selection for pointer activation.
    fn activate_search_hit(&mut self, index: Option<usize>) {
        let Some(view) = self.search_view.as_ref() else {
            return;
        };
        if view.results.is_empty() {
            self.status = if view.query.trim().is_empty() {
                "type to search session output".to_owned()
            } else {
                format!("no output matches '{}'", view.query.trim())
            };
            return;
        }
        let row = index.unwrap_or(view.selected).min(view.results.len() - 1);
        let hit = view.results[row].clone();

        match hit.target {
            SearchHitTarget::PaneRow { pane_id, row, kind } => {
                if self.workspace.active_session().pane(&pane_id).is_none() {
                    self.status = format!("pane {pane_id} is not in this session");
                    return;
                }
                self.search_view = None;
                self.focus_pane_for_pointer(&pane_id);
                match kind {
                    SearchSourceKind::Terminal => {
                        self.scroll_pane_to_row(&pane_id, row, &hit.text, &hit.match_indices)
                    }
                    // Task output and agent tails render bottom-anchored
                    // without a scrollable viewport; focus is the jump.
                    SearchSourceKind::Task | SearchSourceKind::Agent => {
                        self.status = format!("focused {pane_id} (its output view shows the tail)");
                    }
                }
            }
            SearchHitTarget::Timeline { event } => {
                self.search_view = None;
                self.open_timeline();
                let Some(timeline) = self.timeline_view.as_mut() else {
                    return;
                };
                match timeline
                    .events
                    .iter()
                    .position(|candidate| candidate == &event)
                {
                    Some(position) => {
                        timeline.selected = position;
                        self.status = "timeline opened at the matched event".to_owned();
                    }
                    None => {
                        self.status = "timeline opened; the matched event left the tail".to_owned();
                    }
                }
            }
        }
    }

    /// Scroll a terminal pane's viewport to an absolute buffer row via the
    /// pointer-view mechanics, selecting the matched span so the hit is
    /// visible. The search snapshot may be stale: rows that were pushed out
    /// of the bounded buffer clamp to the oldest retained row, and a row
    /// whose text has moved since the snapshot is named honestly instead of
    /// pretending the jump landed.
    fn scroll_pane_to_row(
        &mut self,
        pane_id: &PaneId,
        row: usize,
        expected_text: &str,
        match_indices: &[usize],
    ) {
        let inner_height = self.terminal_size.and_then(|(columns, rows)| {
            pane_content_rect(&self.workspace, SceneSize::new(columns, rows), pane_id)
                .map(|rect| rect.height)
        });
        let Some(grid) = self.terminal_grid(pane_id) else {
            self.status = format!("focused {pane_id} (no live terminal to scroll)");
            return;
        };
        let total_rows = grid.total_rows();
        let scrollback_len = grid.scrollback_len();
        let columns = grid.size().columns();
        let view_rows = usize::from(grid.size().rows().min(inner_height.unwrap_or(u16::MAX)));
        let scrolled_out = row >= total_rows;
        let target_row = row.min(total_rows.saturating_sub(1));
        let scroll_offset = scroll_offset_for_row(total_rows, view_rows, target_row);
        // Verify the snapshot row still holds the matched text (a flooding
        // pane shifts absolute rows as the scrollback ring evicts).
        let current_text = if target_row < scrollback_len {
            grid.scrollback_row_text(target_row)
        } else {
            grid.row_text((target_row - scrollback_len) as u16)
        };
        let row_intact = !scrolled_out
            && current_text.is_some_and(|text| text.trim_end() == expected_text.trim_end());
        // Select the matched span so the row is visibly marked; the buffer
        // stores one char per cell, so char indices are columns.
        let selection = match (match_indices.first(), match_indices.last()) {
            (Some(&first), Some(&last)) if row_intact && columns > 0 => {
                let clamp = |index: usize| (index.min(usize::from(columns) - 1)) as u16;
                Some(((target_row, clamp(first)), (target_row, clamp(last))))
            }
            _ => None,
        };
        // Keep the pointer-view invariant: offset 0 with no selection means
        // following live output, represented as no view at all.
        self.pointer_view = if scroll_offset == 0 && selection.is_none() {
            None
        } else {
            Some(PointerView {
                pane_id: pane_id.clone(),
                scroll_offset,
                selection,
            })
        };
        self.status = if row_intact {
            format!("{pane_id}: jumped to the matched row")
        } else {
            format!("{pane_id}: output moved since the search snapshot; showing where it was")
        };
    }

    /// Open the session map with the active session's focused pane selected.
    fn open_session_map(&mut self) {
        self.palette = None;
        self.context_menu = None;
        self.timeline_view = None;
        self.search_view = None;
        self.help_view = None;
        self.objective_prompt = None;
        let rows = self.session_map_row_models();
        let focused = self.workspace.active_session().focused_pane_id().clone();
        let active_session = self.workspace.active_session().id().clone();
        let selected = rows
            .iter()
            .position(|model| {
                model.target
                    == SessionMapTarget::Pane {
                        session_id: active_session.clone(),
                        pane_id: focused.clone(),
                    }
            })
            .unwrap_or(0);
        self.session_map = Some(SessionMapState { selected });
        self.status = "session map: up/down choose, Enter focus, Esc close".to_owned();
    }

    fn close_session_map(&mut self) {
        self.session_map = None;
        self.status = "session map closed".to_owned();
    }

    /// Open the help overlay. Content is generated from the command table
    /// and the LIVE keymap (`crate::help`), so rebinds are always reflected.
    /// The other modal surfaces close.
    fn open_help(&mut self) {
        self.palette = None;
        self.context_menu = None;
        self.timeline_view = None;
        self.search_view = None;
        self.session_map = None;
        self.objective_prompt = None;
        self.help_view = Some(HelpViewState::default());
        self.status = "help: type to filter, Esc close".to_owned();
    }

    fn close_help(&mut self) {
        self.help_view = None;
        self.status = "help closed".to_owned();
    }

    fn handle_help_key(&mut self, key: Key) {
        match self.keymap.chord_action(key) {
            Some(ChordAction::Quit) => {
                self.should_quit = true;
                self.status = "quitting".to_owned();
                return;
            }
            // The help chord toggles: pressing it again closes.
            Some(ChordAction::Dispatch(CommandId::ShowHelp)) => {
                self.close_help();
                return;
            }
            _ => {}
        }
        let ctrl_only = key.mods.control && !key.mods.shift && !key.mods.alt && !key.mods.super_key;
        if key.code == KeyCode::Up || (ctrl_only && key.code == KeyCode::Char('p')) {
            self.move_help_selection(-1);
            return;
        }
        if key.code == KeyCode::Down || (ctrl_only && key.code == KeyCode::Char('n')) {
            self.move_help_selection(1);
            return;
        }
        match key.code {
            KeyCode::Escape | KeyCode::Enter => self.close_help(),
            KeyCode::Backspace => {
                if let Some(view) = self.help_view.as_mut() {
                    view.query.pop();
                    view.selected = 0;
                }
            }
            KeyCode::Char(character)
                if !key.mods.control && !key.mods.alt && !key.mods.super_key =>
            {
                if let Some(view) = self.help_view.as_mut() {
                    view.query.push(character);
                    view.selected = 0;
                }
            }
            _ => {}
        }
    }

    fn move_help_selection(&mut self, delta: isize) {
        let count = self
            .help_view
            .as_ref()
            .map(|view| filter_help_rows(&help_rows(&self.keymap), &view.query).len())
            .unwrap_or(0);
        let Some(view) = self.help_view.as_mut() else {
            return;
        };
        if count == 0 {
            view.selected = 0;
            return;
        }
        let current = view.selected.min(count - 1) as isize;
        view.selected = (current + delta).clamp(0, count as isize - 1) as usize;
    }

    /// The help overlay for the current frame, `None` while closed.
    pub(crate) fn help_overlay_scene(&self, size: SceneSize) -> Option<HelpOverlay> {
        let view = self.help_view.as_ref()?;
        let items = filter_help_rows(&help_rows(&self.keymap), &view.query);
        let selected = if items.is_empty() {
            None
        } else {
            Some(view.selected.min(items.len() - 1))
        };
        let area = help_overlay_rect(size);
        let window = palette_item_window(pane_inner_rect(area), items.len(), selected);
        let mut footer = String::new();
        let hidden_above = window.start;
        let hidden_below = items.len().saturating_sub(window.end);
        if hidden_above > 0 || hidden_below > 0 {
            footer.push_str(&format!("↑ {hidden_above} / ↓ {hidden_below} more · "));
        }
        footer.push_str("type to filter · ↑/↓ scroll · esc close");
        Some(HelpOverlay {
            area,
            query: view.query.clone(),
            items,
            selected,
            footer,
        })
    }

    /// The one-time first-run note, `None` once anything has been done (or
    /// when a saved workspace existed at launch). Only composed when no
    /// modal overlay is above it.
    pub(crate) fn welcome_overlay_scene(&self, size: SceneSize) -> Option<WelcomeOverlay> {
        if !self.first_run_note {
            return None;
        }
        let entries = welcome_entries(&self.keymap);
        Some(WelcomeOverlay {
            // Introduction + blank + route rows + blank + dismissal.
            area: welcome_rect(size, entries.len() as u16 + 4),
            introduction: "A workspace for terminals, tasks, and agents.".to_owned(),
            entries,
            dismissal: "Any key or click dismisses this note".to_owned(),
        })
    }

    /// Open the Set-agent-objective prompt for the focused agent pane,
    /// pre-filled with the current durable objective.
    fn open_objective_prompt(&mut self) {
        let Some(pane_id) = self.focused_agent_pane_id() else {
            self.status = "focused pane is not an agent pane".to_owned();
            return;
        };
        let Some(objective) = self
            .workspace
            .active_session()
            .pane(&pane_id)
            .and_then(|pane| match pane.kind() {
                PaneKind::Agent { intent } => Some(intent.objective.clone()),
                _ => None,
            })
        else {
            self.status = format!("agent pane {pane_id} was not found");
            return;
        };
        self.palette = None;
        self.context_menu = None;
        self.timeline_view = None;
        self.search_view = None;
        self.session_map = None;
        self.help_view = None;
        self.objective_prompt = Some(ObjectivePrompt {
            pane_id: pane_id.clone(),
            input: objective,
        });
        self.status = format!("editing objective for {pane_id}");
    }

    fn handle_timeline_key(&mut self, key: Key) {
        if matches!(self.keymap.chord_action(key), Some(ChordAction::Quit)) {
            self.should_quit = true;
            self.status = "quitting".to_owned();
            return;
        }
        let ctrl_only = key.mods.control && !key.mods.shift && !key.mods.alt && !key.mods.super_key;
        if key.code == KeyCode::Up || (ctrl_only && key.code == KeyCode::Char('p')) {
            self.move_timeline_selection(-1);
            return;
        }
        if key.code == KeyCode::Down || (ctrl_only && key.code == KeyCode::Char('n')) {
            self.move_timeline_selection(1);
            return;
        }
        match key.code {
            KeyCode::Escape => self.close_timeline(),
            KeyCode::Enter => self.jump_to_selected_timeline_entry(None),
            KeyCode::Backspace => {
                if let Some(view) = self.timeline_view.as_mut() {
                    view.pop_query(now_ms());
                }
            }
            KeyCode::Char(character)
                if !key.mods.control && !key.mods.alt && !key.mods.super_key =>
            {
                if let Some(view) = self.timeline_view.as_mut() {
                    view.push_query(&character.to_string(), now_ms());
                }
            }
            _ => {}
        }
    }

    fn move_timeline_selection(&mut self, delta: isize) {
        let Some(view) = self.timeline_view.as_mut() else {
            return;
        };
        let count = view.filtered().len();
        if count == 0 {
            view.selected = 0;
            return;
        }
        let current = view.selected.min(count - 1) as isize;
        view.selected = (current + delta).clamp(0, count as isize - 1) as usize;
    }

    /// Enter/click on a timeline row: focus the pane the event names.
    /// `index` overrides the selection for pointer activation.
    fn jump_to_selected_timeline_entry(&mut self, index: Option<usize>) {
        let Some(view) = self.timeline_view.as_ref() else {
            return;
        };
        let filtered = view.filtered().to_vec();
        if filtered.is_empty() {
            self.status = format!("no timeline event matches '{}'", view.query.trim());
            return;
        }
        let row = index.unwrap_or(view.selected).min(filtered.len() - 1);
        let event = &view.events[filtered[row]];
        let Some(pane) = event.kind.pane().map(PaneId::new) else {
            self.status = "this event names no pane to jump to".to_owned();
            return;
        };
        if self.workspace.active_session().pane(&pane).is_none() {
            self.status = format!("pane {pane} is not in this session");
            return;
        }
        match self.workspace.apply_action(CoreAction::FocusPane {
            pane_id: pane.clone(),
        }) {
            Ok(_) => {
                self.timeline_view = None;
                self.status = format!("focused {pane}");
                if let Err(error) = self.reconcile_runtimes() {
                    self.status = error.to_string();
                }
            }
            Err(error) => self.status = format!("focus failed: {error}"),
        }
    }

    fn handle_session_map_key(&mut self, key: Key) {
        if matches!(self.keymap.chord_action(key), Some(ChordAction::Quit)) {
            self.should_quit = true;
            self.status = "quitting".to_owned();
            return;
        }
        let ctrl_only = key.mods.control && !key.mods.shift && !key.mods.alt && !key.mods.super_key;
        match key.code {
            KeyCode::Escape => self.close_session_map(),
            KeyCode::Up => self.move_session_map_selection(-1),
            KeyCode::Down => self.move_session_map_selection(1),
            KeyCode::Char('p') if ctrl_only => self.move_session_map_selection(-1),
            KeyCode::Char('n') if ctrl_only => self.move_session_map_selection(1),
            KeyCode::Enter => self.activate_session_map_row(None),
            _ => {}
        }
    }

    fn move_session_map_selection(&mut self, delta: isize) {
        let count = self.session_map_row_models().len();
        let Some(map) = self.session_map.as_mut() else {
            return;
        };
        if count == 0 {
            map.selected = 0;
            return;
        }
        let current = map.selected.min(count - 1) as isize;
        map.selected = (current + delta).clamp(0, count as isize - 1) as usize;
    }

    /// Enter/click on a session-map row: activate that session (and focus
    /// the pane, for pane rows), then close the map.
    fn activate_session_map_row(&mut self, index: Option<usize>) {
        let rows = self.session_map_row_models();
        let Some(map) = self.session_map.as_ref() else {
            return;
        };
        let Some(row) = rows.get(
            index
                .unwrap_or(map.selected)
                .min(rows.len().saturating_sub(1)),
        ) else {
            return;
        };

        let (session_id, pane_id) = match &row.target {
            SessionMapTarget::Session(session_id) => (session_id.clone(), None),
            SessionMapTarget::Pane {
                session_id,
                pane_id,
            } => (session_id.clone(), Some(pane_id.clone())),
        };

        if self.workspace.active_session().id() != &session_id {
            let previous_session_id = self.workspace.active_session().id().clone();
            if let Err(error) = self.workspace.apply_action(CoreAction::ActivateSession {
                session_id: session_id.clone(),
            }) {
                self.status = format!("session switch failed: {error}");
                return;
            }
            self.retire_runtimes_for_session_switch(&previous_session_id);
            self.command_context = command_context_for_workspace(&self.workspace);
        }
        if let Some(pane_id) = &pane_id
            && let Err(error) = self.workspace.apply_action(CoreAction::FocusPane {
                pane_id: pane_id.clone(),
            })
        {
            self.status = format!("focus failed: {error}");
            return;
        }

        self.session_map = None;
        self.status = match pane_id {
            Some(pane_id) => format!("focused {pane_id} in {session_id}"),
            None => format!("switched to {session_id}"),
        };
        if let Err(error) = self.reconcile_runtimes() {
            self.status = error.to_string();
        }
    }

    fn handle_objective_prompt_key(&mut self, key: Key) {
        if matches!(self.keymap.chord_action(key), Some(ChordAction::Quit)) {
            self.should_quit = true;
            self.status = "quitting".to_owned();
            return;
        }
        match key.code {
            KeyCode::Escape => {
                self.objective_prompt = None;
                self.status = "objective unchanged".to_owned();
            }
            KeyCode::Enter => self.commit_objective_prompt(),
            KeyCode::Backspace => {
                if let Some(prompt) = self.objective_prompt.as_mut() {
                    prompt.input.pop();
                }
            }
            KeyCode::Char(character)
                if !key.mods.control && !key.mods.alt && !key.mods.super_key =>
            {
                if let Some(prompt) = self.objective_prompt.as_mut() {
                    prompt.input.push(character);
                }
            }
            _ => {}
        }
    }

    /// Commit the edited objective into the pane's durable intent; the next
    /// StartAgent/relaunch reads it from there.
    fn commit_objective_prompt(&mut self) {
        let Some(prompt) = self.objective_prompt.take() else {
            return;
        };
        let objective = prompt.input.trim().to_owned();
        if objective.is_empty() {
            self.objective_prompt = Some(prompt);
            self.status = "objective cannot be empty (Esc cancels)".to_owned();
            return;
        }
        let pane_id = prompt.pane_id;
        if self
            .workspace
            .active_session_mut()
            .agent_intent_mut(&pane_id)
            .is_none()
        {
            self.status = format!("agent pane {pane_id} was not found");
            return;
        }
        self.update_agent_intent(&pane_id, |intent| {
            intent.objective = objective.clone();
        });
        self.timeline.record(TimelineEventKind::AgentObjectiveSet {
            pane: pane_id.to_string(),
            objective: objective.clone(),
        });
        self.status = format!("objective set for {pane_id}: {objective}");
    }

    /// The session-map rows for the current workspace, with live one-word
    /// states for the active session's runtimes.
    fn session_map_row_models(&self) -> Vec<SessionMapRowModel> {
        let live = |pane_id: &PaneId| -> Option<String> {
            if let Some(exit_status) = self.runtime.terminal_exit_status(pane_id) {
                return Some(match exit_status {
                    Some(status) => format!("exited: {}", exit_status_label(status)),
                    // A live shell at a prompt is not doing work; "running"
                    // would read as activity. "open" states what is true
                    // without claiming to know whether a command runs.
                    None => "open".to_owned(),
                });
            }
            if let Some((exit_status, status)) = self.runtime.task_live_status(pane_id) {
                return Some(match exit_status {
                    // The retained status label ("failed: exit 3") — the
                    // exact vocabulary the pane body and status line use,
                    // so the same fact never reads two ways.
                    Some(_) => status.to_owned(),
                    None => "running".to_owned(),
                });
            }
            None
        };
        session_map_rows(&self.workspace, &live)
    }

    /// The timeline overlay for the current frame, `None` while closed.
    pub(crate) fn timeline_overlay_scene(&self, size: SceneSize) -> Option<TimelineOverlay> {
        let view = self.timeline_view.as_ref()?;
        Some(timeline_overlay(view, size, now_ms()))
    }

    /// The session-search overlay for the current frame, `None` while
    /// closed.
    pub(crate) fn search_overlay_scene(&self, size: SceneSize) -> Option<SearchOverlay> {
        let view = self.search_view.as_ref()?;
        Some(search_overlay(view, size))
    }

    /// The session-map overlay for the current frame, `None` while closed.
    pub(crate) fn session_map_overlay_scene(&self, size: SceneSize) -> Option<SessionMapOverlay> {
        let map = self.session_map.as_ref()?;
        Some(session_map_overlay(
            &self.session_map_row_models(),
            map.selected,
            size,
        ))
    }

    /// The objective-prompt overlay for the current frame, `None` while
    /// closed.
    pub(crate) fn prompt_overlay_scene(&self, size: SceneSize) -> Option<PromptOverlay> {
        let prompt = self.objective_prompt.as_ref()?;
        Some(PromptOverlay {
            area: prompt_rect(size),
            title: format!(" Set agent objective — {} ", prompt.pane_id),
            input: prompt.input.clone(),
            footer: "enter save · esc cancel".to_owned(),
        })
    }

    // --- Copy mode -------------------------------------------------------------

    fn enter_copy_mode(&mut self) {
        let focused = self.workspace.active_session().focused_pane_id().clone();
        let Some(grid) = self.runtime.terminal_grid(&focused) else {
            self.status = format!("pane {focused} has no live terminal to copy from");
            return;
        };
        self.copy_mode = Some(CopyModeState::enter(focused, grid));
        self.palette = None;
        // Copy mode owns the pane's viewport; a stale pointer selection
        // underneath it would reappear on exit, addressing moved rows.
        self.pointer_view = None;
        self.status = "copy mode: hjkl/arrows move, v select, y/Enter copy, Esc exit".to_owned();
    }

    fn exit_copy_mode(&mut self) {
        self.copy_mode = None;
        self.status = "copy mode closed".to_owned();
    }

    fn handle_copy_mode_key(&mut self, key: Key) {
        let Some(pane_id) = self.copy_mode.as_ref().map(|state| state.pane_id.clone()) else {
            return;
        };
        if !self.runtime.has_terminal(&pane_id) {
            self.copy_mode = None;
            self.status = "copy mode closed: pane is no longer live".to_owned();
            return;
        }

        if matches!(
            self.keymap.chord_action(key),
            Some(crate::keymap::ChordAction::Quit)
        ) {
            self.should_quit = true;
            self.status = "quitting".to_owned();
            return;
        }

        let action = {
            let state = self.copy_mode.as_mut().expect("copy mode present");
            let grid = self
                .runtime
                .terminal_grid(&pane_id)
                .expect("runtime present");
            copy_mode_action(state, grid, key)
        };

        match action {
            CopyModeAction::Continue => {}
            CopyModeAction::Exit => self.exit_copy_mode(),
            CopyModeAction::Copy => self.copy_selection(&pane_id),
        }
    }

    fn copy_selection(&mut self, pane_id: &PaneId) {
        let Some(text) = self.copy_mode.as_ref().and_then(|state| {
            self.runtime
                .terminal_grid(pane_id)
                .map(|grid| state.selected_text(grid))
        }) else {
            return;
        };

        self.frontend_effects
            .push(FrontendEffect::SetClipboard(text.clone()));
        let count = text.chars().count();
        self.last_copied = Some(text);
        self.copy_mode = None;
        self.status = format!("copied {count} char(s) to clipboard");
    }

    fn mark_redraw(&mut self) {
        self.last_redraw = Instant::now();
    }
}

/// Double-click window for pane titles/bodies.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// Rows scrolled per wheel tick over a terminal pane.
const WHEEL_SCROLL_ROWS: usize = 3;

/// Columns one keyboard float-move step covers (rows step by 1: terminal
/// cells are roughly twice as tall as they are wide, so 2:1 keeps a step
/// visually square).
const MOVE_FLOAT_STEP_COLUMNS: u16 = 2;

/// One armed pointer drag gesture, set on a button press over a target.
#[derive(Clone)]
enum PointerDrag {
    ResizeSplit {
        split_index: usize,
        axis: SplitAxis,
        split_area: SceneRect,
    },
    MoveFloat {
        pane_id: PaneId,
        grab_dx: u16,
        grab_dy: u16,
    },
    Select {
        pane_id: PaneId,
        inner: SceneRect,
    },
}

/// Pointer-driven viewport state for one pane: wheel scrollback plus a
/// click-drag selection. This reuses the copy-mode viewing/selection model
/// (absolute buffer coordinates through the same windowing math) without the
/// modal copy-mode keymap, so plain typing keeps flowing to the shell.
struct PointerView {
    pane_id: PaneId,
    /// Rows scrolled up from the live bottom; `0` follows live output.
    scroll_offset: usize,
    /// `(anchor, cursor)` in absolute buffer coordinates, unordered.
    selection: Option<((usize, u16), (usize, u16))>,
}

impl PointerView {
    fn ordered_selection(&self) -> Option<((usize, u16), (usize, u16))> {
        let (anchor, cursor) = self.selection?;
        Some(if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneClickTarget {
    Title,
    Body,
}

/// The previous button press, for double-click detection.
struct PaneClick {
    pane_id: PaneId,
    target: PaneClickTarget,
    at: Instant,
}

/// The open Set-agent-objective prompt: which pane's durable intent it
/// edits, and the live input text. Runtime presentation only.
struct ObjectivePrompt {
    pane_id: PaneId,
    input: String,
}

/// The header label for a configured connector kind.
/// Status copy names panes by their user-facing title, with the id kept as
/// trailing detail for audit ("checks failed: exit 3 · pane-5"): the title
/// is what tells a glance WHICH task failed. Free function so callers
/// holding mutable borrows of runtime registries can still name panes.
fn pane_status_name(workspace: &Workspace, pane_id: &PaneId) -> String {
    workspace
        .active_session()
        .pane(pane_id)
        .map(|pane| pane.title().to_owned())
        .unwrap_or_else(|| pane_id.to_string())
}

fn connector_kind_label(kind: AgentConnectorKind) -> &'static str {
    match kind {
        AgentConnectorKind::Fake => "fake",
        AgentConnectorKind::Claude => "claude",
    }
}

/// The open right-click menu: rows plus the pointer anchor it opened at.
struct ContextMenuState {
    items: Vec<ContextMenuItem>,
    selected: usize,
    anchor: (u16, u16),
}

struct ContextMenuItem {
    action: ContextMenuAction,
    label: String,
    hint: String,
}

/// What a context-menu row does when run: dispatch a command, or open the
/// workspace's own palette (the menu's gateway row, not a command).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ContextMenuAction {
    Command(CommandId),
    OpenPalette,
}

enum CopyModeAction {
    Continue,
    Exit,
    Copy,
}

fn copy_mode_action(state: &mut CopyModeState, grid: &TerminalGrid, key: Key) -> CopyModeAction {
    match key.code {
        KeyCode::Escape | KeyCode::Char('q') => return CopyModeAction::Exit,
        KeyCode::Char('y') | KeyCode::Enter => return CopyModeAction::Copy,
        KeyCode::Char('k') | KeyCode::Up => state.move_up(1, grid),
        KeyCode::Char('j') | KeyCode::Down => state.move_down(1, grid),
        KeyCode::Char('h') | KeyCode::Left => state.move_left(1, grid),
        KeyCode::Char('l') | KeyCode::Right => state.move_right(1, grid),
        KeyCode::PageUp => state.page_up(grid),
        KeyCode::PageDown => state.page_down(grid),
        KeyCode::Char('g') | KeyCode::Home => state.move_to_top(grid),
        KeyCode::Char('G') | KeyCode::End => state.move_to_bottom(grid),
        KeyCode::Char('0') => state.line_start(grid),
        KeyCode::Char('$') => state.line_end(grid),
        KeyCode::Char('v') | KeyCode::Char(' ') => state.set_anchor(),
        KeyCode::Char('c') => state.clear_anchor(),
        _ => {}
    }
    CopyModeAction::Continue
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn status_for_outcome(command_id: CommandId, outcome: ActionOutcome) -> String {
    let label = BUILT_IN_COMMANDS
        .iter()
        .find(|command| command.id == command_id)
        .map(|command| command.label)
        .unwrap_or("Command");

    match outcome {
        ActionOutcome::Mutated { focused_pane } => format!("{label}: focused {focused_pane}"),
        ActionOutcome::PersistenceRequested(request) => {
            format!("unhandled persistence request: {request:?}")
        }
    }
}

fn objective_status_summary(objective: &str) -> String {
    const MAX_CHARS: usize = 96;
    let first_line = objective.lines().next().unwrap_or_default().trim();
    let mut summary: String = first_line.chars().take(MAX_CHARS).collect();
    if first_line.chars().count() > MAX_CHARS {
        summary.push('…');
    }
    summary
}

fn command_context_for_workspace(workspace: &Workspace) -> CommandContext {
    let session = workspace.active_session();
    if let Some(project) = workspace.projects().get(session.project_id()) {
        CommandContext::for_project(project.name().to_owned(), project.path().to_path_buf())
    } else {
        CommandContext::for_project(
            workspace.name().to_owned(),
            std::env::current_dir().ok().unwrap_or_default(),
        )
    }
}

pub(crate) fn agent_status_label(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Draft => "draft",
        AgentStatus::Running => "running",
        AgentStatus::WaitingForApproval => "waiting for approval",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Failed => "failed",
        AgentStatus::Complete => "complete",
        AgentStatus::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests;
