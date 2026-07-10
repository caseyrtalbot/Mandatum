use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use mandatum_agent_runtime::{
    AgentConnector, AgentLaunchSpec, AgentSessionEvent, ApprovalDecision, ApprovalVerdict,
};
use mandatum_commands::{
    BUILT_IN_COMMANDS, CommandContext, CommandId, CommandTarget, PaletteContext, PaletteInput,
    PaletteKey, RuntimeAgentCommand, RuntimeCommand, RuntimeTaskCommand, command_target,
    dispatch_command, resolve_palette_key_with_bindings,
};
use mandatum_core::{
    ActionOutcome, AgentApprovalRecord, AgentPaneIntent, AgentStatus, CoreAction, LayoutNode,
    PaneId, PaneKind, PersistenceRequest, SplitAxis, TaskPaneIntent, Workspace,
};
use mandatum_pty::{NativePtyError, PtySize};
use mandatum_scene::{
    ContextMenuEntry, ContextMenuOverlay, HitTarget, HitTargetKind, PaletteOverlay, PaneSceneKind,
    SceneRect, SceneSize, Theme, WorkspaceScene,
    input::{InputEvent, Key, KeyCode, PointerButton, PointerEvent, PointerKind},
    layout::{
        context_menu_rect, layout_separators, palette_overlay_rect, pane_content_rect,
        workspace_scene_area,
    },
};
use mandatum_terminal_vt::TerminalGrid;

use crate::{
    agent_runtime::{
        AgentPaneRuntime, AgentRuntimeEvent, AgentRuntimeRegistry, activate_agent_session,
        connector_for_kind,
    },
    app_shell::AppConfig,
    clipboard::osc52_sequence,
    config::{load_config, project_config_file},
    copy_mode::CopyModeState,
    input::{RuntimeInput, key_to_input_with_keymap},
    keymap::{ChordAction, Keymap, format_chord},
    palette::{PaletteRow, PaletteState, PaletteWorkspaceView, palette_footer, palette_rows},
    persistence::{PersistenceCoordinator, WorkspaceFileError},
    pointer::{encode_mouse_event, split_percent_for_pointer},
    process_events::PtyRuntimeEvent,
    scene_builder::PaneViewState,
    task_runtime::{
        TaskPaneRuntime, TaskRuntimeRegistry, prepare_task_pane_runtime, task_status_label,
    },
    terminal_runtime::{
        PendingTerminalPaneRuntime, TerminalRuntimeError, TerminalRuntimeRegistry,
        exit_status_label, prepare_terminal_pane_runtime,
    },
};

#[cfg(test)]
use crate::{
    input::key_to_input,
    persistence::{MAX_WORKSPACE_FILE_BYTES, ensure_parent_dir, write_workspace_file},
};

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
    terminal_panes: TerminalRuntimeRegistry,
    task_panes: TaskRuntimeRegistry,
    agent_panes: AgentRuntimeRegistry,
    agent_connector: Option<Box<dyn AgentConnector>>,
    agent_objective: String,
    agent_model: Option<String>,
    keymap: Keymap,
    theme: Theme,
    reduced_motion: bool,
    user_config_file: Option<PathBuf>,
    copy_mode: Option<CopyModeState>,
    clipboard_payload: Option<Vec<u8>>,
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
    runtime_tx: Sender<PtyRuntimeEvent>,
    runtime_rx: Receiver<PtyRuntimeEvent>,
    agent_tx: Sender<AgentRuntimeEvent>,
    agent_rx: Receiver<AgentRuntimeEvent>,
    next_runtime_token: u64,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let command_context =
            CommandContext::for_project(config.workspace_name.clone(), config.project_path.clone());
        let workspace = Workspace::new(config.workspace_name, config.project_path);
        let (runtime_tx, runtime_rx) = mpsc::channel();
        let (agent_tx, agent_rx) = mpsc::channel();
        let restore_on_startup = config.restore_on_startup;
        let config_warnings = config.config_warnings;

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
            terminal_panes: TerminalRuntimeRegistry::new(),
            task_panes: TaskRuntimeRegistry::new(),
            agent_panes: AgentRuntimeRegistry::new(),
            agent_connector: connector_for_kind(config.agent_connector),
            agent_objective: config.agent_objective,
            agent_model: config.agent_model,
            keymap: config.keymap,
            theme: config.theme,
            reduced_motion: config.reduced_motion,
            user_config_file: config.user_config_file,
            copy_mode: None,
            clipboard_payload: None,
            last_copied: None,
            hit_targets: Vec::new(),
            pointer_drag: None,
            pointer_forward: None,
            pointer_view: None,
            context_menu: None,
            last_pane_click: None,
            runtime_tx,
            runtime_rx,
            agent_tx,
            agent_rx,
            next_runtime_token: 1,
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
        self.terminal_panes.len()
    }

    pub fn live_task_count(&self) -> usize {
        self.task_panes.len()
    }

    pub fn live_agent_count(&self) -> usize {
        self.agent_panes.len()
    }

    pub fn copy_mode_active(&self) -> bool {
        self.copy_mode.is_some()
    }

    /// The text most recently copied via copy mode, for verification and tests.
    pub fn last_copied(&self) -> Option<&str> {
        self.last_copied.as_deref()
    }

    /// Take the pending OSC 52 clipboard payload, if any, so the run loop can
    /// write it to the host terminal. Clears it so it is written once.
    pub fn take_clipboard_payload(&mut self) -> Option<Vec<u8>> {
        self.clipboard_payload.take()
    }

    /// The active theme, resolved from config, for the frontend adapter.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The permanent status-strip hint naming the workspace's entry points,
    /// from the live keymap. A stranger's first breadcrumb: the palette
    /// chord and the right-click menu are always written on screen.
    pub(crate) fn control_hint(&self) -> String {
        format!(
            "{} commands · right-click menu",
            format_chord(self.keymap.toggle_palette)
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
        let agent_runtime = self.agent_panes.get(&focused_id);
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
            focused_task_running: self
                .task_panes
                .get(&focused_id)
                .is_some_and(|task| task.runtime.exit_status.is_none())
                || self.task_panes.pending_launches.contains(&focused_id),
            focused_has_live_terminal: self.terminal_panes.get(&focused_id).is_some(),
            focused_is_floating,
            // Any tiled pane sits inside a split exactly when the tiled root
            // is one.
            focused_in_tiled_split: !focused_is_floating
                && matches!(session.layout().root(), LayoutNode::Split { .. }),
            pane_count: session.panes().len(),
        }
    }

    pub fn handle_event(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key(key) => self.handle_key(key),
            InputEvent::Resize(size) => self.handle_terminal_resize(size.width, size.height),
            // Paste only reaches the shell in normal mode; copy mode and the
            // context menu own input while open.
            InputEvent::Paste(text) if self.copy_mode.is_none() && self.context_menu.is_none() => {
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

        match dispatch_command(&mut self.workspace, &self.command_context, command_id) {
            Ok(outcome) => {
                self.handle_command_outcome(command_id, outcome);
            }
            Err(error) => {
                self.status = format!("command failed: {error}");
            }
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
        self.keymap = loaded.keymap;
        self.theme = loaded.theme;
        self.reduced_motion = loaded.reduced_motion;
        if let Some(shell_program) = loaded.shell_program {
            self.shell_program = shell_program;
        }
        if let Some(task_command) = loaded.task_command {
            self.task_command = task_command;
        }
        if let Some(kind) = loaded.agent_connector {
            self.agent_connector = connector_for_kind(kind);
        }
        if let Some(model) = loaded.agent_model {
            self.agent_model = Some(model);
        }
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
        }
        self.mark_redraw();
    }

    pub fn tick_runtime(&mut self) {
        self.drain_runtime_events();
        self.drain_agent_events();
        self.poll_child_exits();
    }

    pub fn shutdown(&mut self) {
        self.shutdown_agent_panes();
        self.shutdown_task_panes();
        self.shutdown_terminal_panes();
        self.status = "terminal sessions stopped".to_owned();
    }

    fn shutdown_agent_panes(&mut self) {
        self.agent_panes.shutdown_all();
    }

    fn shutdown_terminal_panes(&mut self) {
        self.terminal_panes.shutdown_all();
    }

    fn shutdown_task_panes(&mut self) {
        self.task_panes.shutdown_all();
    }

    fn save_workspace_to_disk(&mut self) {
        match self.persistence.save_workspace(&self.workspace) {
            Ok(()) => {
                self.status = format!(
                    "workspace saved to {}",
                    self.persistence.workspace_file().display()
                );
            }
            Err(error) => {
                self.status = format!("workspace save failed: {error}");
            }
        }
    }

    fn restore_workspace_at_startup(&mut self) {
        match self.persistence.read_workspace() {
            Ok(workspace) => match self.prepare_restore_runtimes(&workspace) {
                Ok(runtimes) => {
                    self.replace_workspace_from_disk(workspace, runtimes);
                    self.status = format!(
                        "workspace restored from {}",
                        self.persistence.workspace_file().display()
                    );
                    self.preserve_status_on_next_resize = true;
                }
                Err(error) => {
                    self.status = format!("startup restore failed: {error}");
                    self.preserve_status_on_next_resize = true;
                }
            },
            Err(WorkspaceFileError::Io { source, .. })
                if source.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                self.status = format!("startup restore failed: {error}");
                self.preserve_status_on_next_resize = true;
            }
        }
    }

    fn restore_workspace_from_disk(&mut self) {
        match self.persistence.read_workspace() {
            Ok(workspace) => match self.prepare_restore_runtimes(&workspace) {
                Ok(runtimes) => {
                    self.replace_workspace_from_disk(workspace, runtimes);
                    self.status = format!(
                        "workspace restored from {}",
                        self.persistence.workspace_file().display()
                    );
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
    ) -> Result<BTreeMap<PaneId, PendingTerminalPaneRuntime>, RestoreRuntimeError> {
        if !self.spawn_pty {
            return Ok(BTreeMap::new());
        }

        let mut runtimes = BTreeMap::new();
        for (pane_id, size) in self.visible_terminal_pane_sizes_for_workspace(workspace) {
            let runtime_token = self.next_runtime_token();
            match prepare_terminal_pane_runtime(
                workspace,
                &self.shell_program,
                runtime_token,
                pane_id.clone(),
                size,
            ) {
                Ok(runtime) => {
                    runtimes.insert(pane_id, runtime);
                }
                Err(error) => {
                    for runtime in runtimes.values_mut() {
                        runtime.shutdown();
                    }
                    return Err(RestoreRuntimeError {
                        pane_id,
                        source: error,
                    });
                }
            }
        }

        Ok(runtimes)
    }

    fn replace_workspace_from_disk(
        &mut self,
        workspace: Workspace,
        runtimes: BTreeMap<PaneId, PendingTerminalPaneRuntime>,
    ) {
        self.shutdown_terminal_panes();
        self.shutdown_task_panes();
        self.shutdown_agent_panes();
        self.discard_pending_runtime_events();
        self.workspace = workspace;
        // [L3-GATE] A loaded workspace has no live agent sessions, so durable
        // intents must not keep session-scoped claims from the run that saved
        // them: running/waiting statuses and pending approval ids name a live
        // session (and connector-scoped approval ids) that no longer exists.
        for session in self.workspace.sessions_mut() {
            for intent in session.agent_intents_mut() {
                intent.detach_live_session();
            }
        }
        self.command_context = command_context_for_workspace(&self.workspace);
        self.copy_mode = None;
        self.clipboard_payload = None;
        self.last_copied = None;
        self.pointer_view = None;
        self.pointer_drag = None;
        self.pointer_forward = None;
        self.context_menu = None;
        self.task_panes.pending_launches.clear();
        self.task_panes.statuses.clear();
        self.terminal_panes = runtimes
            .into_iter()
            .map(|(pane_id, runtime)| {
                let active = runtime.activate(pane_id.clone(), self.runtime_tx.clone());
                (pane_id, active)
            })
            .collect();
    }

    fn discard_pending_runtime_events(&mut self) {
        while self.runtime_rx.try_recv().is_ok() {}
        while self.agent_rx.try_recv().is_ok() {}
    }

    fn write_to_focused_terminal(&mut self, bytes: &[u8]) {
        let focused = self.workspace.active_session().focused_pane_id().clone();
        let Some(runtime) = self.terminal_panes.get_mut(&focused) else {
            self.status = format!("pane {focused} has no live PTY");
            return;
        };

        match runtime.write_input(bytes) {
            Ok(()) => {
                self.status = format!("sent {} byte(s) to {focused}", bytes.len());
            }
            Err(error) => {
                runtime.error = Some(error.to_string());
                self.status = format!("PTY input failed for {focused}: {error}");
            }
        }
    }

    fn run_configured_task(&mut self) {
        let intent = TaskPaneIntent {
            recipe_id: Some("configured".to_owned()),
            command: self.task_command.clone(),
            cwd: Some(self.command_context.project_path.clone()),
        };
        let title = "task".to_owned();
        match self.workspace.apply_action(CoreAction::CreateTaskPane {
            title,
            intent: intent.clone(),
        }) {
            Ok(ActionOutcome::Mutated { focused_pane }) => {
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

        if !self.spawn_pty {
            let status = "rerun unavailable: PTY spawning is disabled".to_owned();
            self.task_panes
                .statuses
                .insert(pane_id.clone(), status.clone());
            self.status = format!("task {pane_id} {status}");
            self.mark_redraw();
            return;
        }

        let Some(size) = self.visible_task_size(&pane_id) else {
            if let Some(mut runtime) = self.task_panes.remove(&pane_id) {
                runtime.shutdown();
            }
            let status = "pending rerun: waiting for visible pane size".to_owned();
            self.task_panes.pending_launches.insert(pane_id.clone());
            self.task_panes
                .statuses
                .insert(pane_id.clone(), status.clone());
            self.status = format!("task {pane_id} {status}");
            self.mark_redraw();
            return;
        };

        self.task_panes.pending_launches.remove(&pane_id);
        if let Err(source) = self.spawn_task_pane(pane_id.clone(), size) {
            self.task_panes
                .statuses
                .insert(pane_id.clone(), format!("task rerun failed: {source}"));
            self.status = format!("task rerun failed: {source}");
        } else {
            self.task_panes.statuses.remove(&pane_id);
            self.status = format!("task {pane_id} rerunning: {}", intent.command);
        }
        self.mark_redraw();
    }

    fn stop_focused_task(&mut self) {
        let Some((pane_id, _)) = self.focused_task_intent() else {
            self.status = "focused pane is not a task pane".to_owned();
            self.mark_redraw();
            return;
        };

        if self.task_panes.pending_launches.remove(&pane_id) {
            let status = "stopped before launch".to_owned();
            self.task_panes.statuses.insert(pane_id.clone(), status);
            self.status = format!("task {pane_id} stopped before launch");
            self.mark_redraw();
            return;
        }

        let Some(mut task) = self.task_panes.remove(&pane_id) else {
            let status = "not running".to_owned();
            self.task_panes.statuses.insert(pane_id.clone(), status);
            self.status = format!("task {pane_id} is not running");
            self.mark_redraw();
            return;
        };

        if task.runtime.exit_status.is_some() {
            let status = task.status.clone();
            self.task_panes.insert(pane_id.clone(), task);
            self.status = format!("task {pane_id} is already {status}");
            self.mark_redraw();
            return;
        }

        match task.stop() {
            Ok(()) => {
                self.task_panes
                    .statuses
                    .insert(pane_id.clone(), "stopped".to_owned());
                self.status = format!("task {pane_id} stopped");
            }
            Err(error) => {
                task.runtime.error = Some(error.to_string());
                task.status = format!("task stop failed: {error}");
                self.task_panes.insert(pane_id.clone(), task);
                self.status = format!("task stop failed for {pane_id}: {error}");
            }
        }
        self.mark_redraw();
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
        self.agent_panes
            .get(focused)
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
                self.status = format!("agent pane {focused_pane} created: {objective}");
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
                if let Some(mut old) = self.agent_panes.remove(&pane_id) {
                    old.shutdown();
                    let _ = self.workspace.apply_action(CoreAction::RestartFocused);
                }
                let restart_generation = self.pane_restart_generation(&pane_id);
                let runtime_token = self.next_runtime_token();
                let runtime = activate_agent_session(
                    pane_id.clone(),
                    restart_generation,
                    runtime_token,
                    session,
                    self.agent_tx.clone(),
                );
                self.agent_panes.insert(pane_id.clone(), runtime);
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::Running;
                    intent.pending_approvals = 0;
                    intent.pending_approval_ids.clear();
                });
                self.status = format!("agent {pane_id} started: {objective}");
            }
            Err(error) if self.agent_panes.get(&pane_id).is_some() => {
                // A failed relaunch never touches the previous session: it
                // stays live and authoritative under its unchanged
                // generation, and durable intent keeps reflecting it.
                self.status = format!(
                    "agent relaunch failed for {pane_id}: {error}; previous session still running"
                );
            }
            Err(error) => {
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::Failed;
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
        let Some(mut runtime) = self.agent_panes.remove(&pane_id) else {
            self.status = format!("agent {pane_id} is not running");
            return;
        };
        runtime.shutdown();
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
        let Some(runtime) = self.agent_panes.get_mut(&pane_id) else {
            self.status = format!("agent {pane_id} is not running");
            return;
        };
        let Some(request) = runtime.pending_approval.clone() else {
            self.status = format!("agent {pane_id} has no pending approval");
            return;
        };

        let verdict = if approved {
            ApprovalVerdict::Approved
        } else {
            ApprovalVerdict::Rejected {
                reason: Some("rejected from the workstation".to_owned()),
            }
        };
        match runtime.control.decide(ApprovalDecision {
            approval_id: request.approval_id.clone(),
            verdict,
        }) {
            Ok(()) => {
                runtime.pending_approval = None;
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
                self.status = format!("{verdict_label} '{}' for {pane_id}", request.command);
            }
            Err(error) => {
                self.status = format!("approval decision failed for {pane_id}: {error}");
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

    fn drain_agent_events(&mut self) {
        while let Ok(runtime_event) = self.agent_rx.try_recv() {
            let AgentRuntimeEvent {
                pane_id,
                restart_generation,
                runtime_token,
                event,
            } = runtime_event;
            // [L3-GATE] Events from a replaced agent runtime are rejected:
            // apply an event only if the pane's current generation and token
            // match the stamp the forwarder recorded at launch.
            let current = self.agent_panes.get(&pane_id).is_some_and(|runtime| {
                runtime.restart_generation == restart_generation
                    && runtime.runtime_token == runtime_token
            });
            if !current {
                continue;
            }
            self.apply_agent_event(pane_id, event);
        }
    }

    /// Fold one accepted agent session event into state: the durable subset
    /// (status, summary, changed files, approval count/ids) into the pane's
    /// `AgentPaneIntent`; live-only detail (current action, output tail, full
    /// approval request) into the runtime registry.
    fn apply_agent_event(&mut self, pane_id: PaneId, event: AgentSessionEvent) {
        match event {
            AgentSessionEvent::Status(status) => {
                let label = agent_status_label(&status);
                self.update_agent_intent(&pane_id, |intent| intent.status = status);
                self.status = format!("agent {pane_id} is {label}");
            }
            AgentSessionEvent::Action { description } => {
                if let Some(runtime) = self.agent_panes.get_mut(&pane_id) {
                    runtime.current_action = Some(description);
                }
            }
            AgentSessionEvent::Summary(summary) => {
                self.update_agent_intent(&pane_id, |intent| {
                    intent.latest_summary = Some(summary);
                });
            }
            AgentSessionEvent::OutputChunk(chunk) => {
                if let Some(runtime) = self.agent_panes.get_mut(&pane_id) {
                    runtime.push_output(&chunk);
                }
            }
            AgentSessionEvent::CommandRun { command } => {
                if let Some(runtime) = self.agent_panes.get_mut(&pane_id) {
                    runtime.push_output(&format!("$ {command}"));
                    runtime.current_action = Some(format!("ran {command}"));
                }
            }
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
                if let Some(runtime) = self.agent_panes.get_mut(&pane_id) {
                    runtime.pending_approval = Some(request);
                }
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::WaitingForApproval;
                    intent.pending_approvals = 1;
                    intent.pending_approval_ids = vec![approval_id];
                });
            }
            AgentSessionEvent::Completed { summary } => {
                if let Some(runtime) = self.agent_panes.get_mut(&pane_id) {
                    runtime.current_action = None;
                }
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::Complete;
                    intent.latest_summary = Some(summary);
                });
                self.status = format!("agent {pane_id} completed");
            }
            AgentSessionEvent::Failed { error } => {
                if let Some(runtime) = self.agent_panes.get_mut(&pane_id) {
                    runtime.current_action = None;
                }
                self.update_agent_intent(&pane_id, |intent| {
                    intent.status = AgentStatus::Failed;
                });
                self.status = format!("agent {pane_id} failed: {error}");
            }
            AgentSessionEvent::Closed => {
                if let Some(runtime) = self.agent_panes.get_mut(&pane_id) {
                    runtime.closed = true;
                    runtime.pending_approval = None;
                }
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

    /// Shut down live agent sessions whose pane is no longer in the active
    /// session, and fold the stop semantics into the pane's durable intent
    /// wherever it still lives. OpenProject switches sessions without
    /// closing panes, so the killed session's pane may survive in an
    /// inactive session; leaving it claiming running/waiting would persist a
    /// live-session claim with no live session behind it ([L3-GATE]).
    fn reconcile_agent_runtimes(&mut self) {
        let agent_pane_ids = self.agent_pane_ids();
        let removed_runtime_ids = self
            .agent_panes
            .keys()
            .filter(|pane_id| !agent_pane_ids.contains(*pane_id))
            .cloned()
            .collect::<Vec<_>>();
        for pane_id in removed_runtime_ids {
            if let Some(mut runtime) = self.agent_panes.remove(&pane_id) {
                runtime.shutdown();
            }
            // The registry is keyed by pane id alone, so the removed runtime
            // was the only live session any same-id pane could point at:
            // detaching every match is safe.
            for session in self.workspace.sessions_mut() {
                if let Some(intent) = session.agent_intent_mut(&pane_id) {
                    intent.detach_live_session();
                }
            }
        }
    }

    /// The live agent runtime view for a pane, if a session is attached.
    pub(crate) fn agent_runtime_view(&self, pane_id: &PaneId) -> Option<&AgentPaneRuntime> {
        self.agent_panes.get(pane_id)
    }

    #[cfg(test)]
    pub(crate) fn set_agent_connector(&mut self, connector: Box<dyn AgentConnector>) {
        self.agent_connector = Some(connector);
    }

    fn launch_task_pane(
        &mut self,
        pane_id: PaneId,
        intent: &TaskPaneIntent,
    ) -> Result<(), ReconcileRuntimeError> {
        if !self.spawn_pty {
            self.status = format!("task pane {pane_id} created; PTY spawning is disabled");
            return Ok(());
        }

        let Some(size) = self
            .visible_task_pane_sizes()
            .into_iter()
            .find_map(|(candidate, size)| (candidate == pane_id).then_some(size))
        else {
            let status = "pending launch: waiting for visible pane size".to_owned();
            self.task_panes.pending_launches.insert(pane_id.clone());
            self.task_panes
                .statuses
                .insert(pane_id.clone(), status.clone());
            self.status = format!("task pane {pane_id} created; {status}");
            return Ok(());
        };

        if let Err(source) = self.spawn_task_pane(pane_id.clone(), size) {
            self.task_panes.pending_launches.remove(&pane_id);
            self.task_panes
                .statuses
                .insert(pane_id.clone(), format!("task launch failed: {source}"));
            return Err(ReconcileRuntimeError::Spawn {
                pane_id: pane_id.clone(),
                source,
            });
        }
        self.task_panes.pending_launches.remove(&pane_id);
        self.task_panes.statuses.remove(&pane_id);
        self.status = format!("task {pane_id} running: {}", intent.command);
        Ok(())
    }

    fn visible_task_size(&self, pane_id: &PaneId) -> Option<PtySize> {
        self.visible_task_pane_sizes()
            .into_iter()
            .find_map(|(candidate, size)| (candidate == *pane_id).then_some(size))
    }

    fn reconcile_runtimes(&mut self) -> Result<(), ReconcileRuntimeError> {
        self.reconcile_terminal_runtimes()?;
        self.reconcile_agent_runtimes();
        self.reconcile_task_runtimes()
    }

    fn reconcile_terminal_runtimes(&mut self) -> Result<(), ReconcileRuntimeError> {
        if !self.spawn_pty {
            return Ok(());
        }

        let desired = self.visible_terminal_pane_sizes();
        let terminal_pane_ids = self.terminal_pane_ids();

        let removed_runtime_ids = self
            .terminal_panes
            .keys()
            .filter(|pane_id| !terminal_pane_ids.contains(*pane_id))
            .cloned()
            .collect::<Vec<_>>();
        for pane_id in removed_runtime_ids {
            if let Some(mut runtime) = self.terminal_panes.remove(&pane_id) {
                runtime.shutdown();
            }
        }

        for (pane_id, size) in desired {
            let core_generation = self.pane_restart_generation(&pane_id);
            let needs_restart = self
                .terminal_panes
                .get(&pane_id)
                .is_some_and(|runtime| core_generation > runtime.restart_generation);

            if needs_restart {
                self.restart_terminal_pane(pane_id, size)?;
            } else if let Some(runtime) = self.terminal_panes.get_mut(&pane_id) {
                if let Err(error) = runtime.resize(size) {
                    runtime.error = Some(error.to_string());
                    return Err(ReconcileRuntimeError::Resize {
                        pane_id,
                        source: error,
                    });
                }
            } else if let Err(error) = self.spawn_terminal_pane(pane_id.clone(), size) {
                return Err(ReconcileRuntimeError::Spawn {
                    pane_id,
                    source: error,
                });
            }
        }

        Ok(())
    }

    fn reconcile_task_runtimes(&mut self) -> Result<(), ReconcileRuntimeError> {
        if !self.spawn_pty {
            return Ok(());
        }

        let task_pane_ids = self.task_pane_ids();
        self.task_panes.retain_pane_ids(&task_pane_ids);
        let removed_runtime_ids = self
            .task_panes
            .keys()
            .filter(|pane_id| !task_pane_ids.contains(*pane_id))
            .cloned()
            .collect::<Vec<_>>();
        for pane_id in removed_runtime_ids {
            if let Some(mut runtime) = self.task_panes.remove(&pane_id) {
                runtime.shutdown();
            }
        }

        let visible_task_sizes = self.visible_task_pane_sizes();
        for (pane_id, size) in &visible_task_sizes {
            let Some(runtime) = self.task_panes.get_mut(pane_id) else {
                continue;
            };
            if let Err(error) = runtime.resize(*size) {
                runtime.runtime.error = Some(error.to_string());
                runtime.status = format!("task resize failed: {error}");
                return Err(ReconcileRuntimeError::Resize {
                    pane_id: pane_id.clone(),
                    source: error,
                });
            }
        }

        let pending_visible = visible_task_sizes
            .into_iter()
            .filter(|(pane_id, _)| {
                self.task_panes.pending_launches.contains(pane_id)
                    && !self.task_panes.contains_key(pane_id)
            })
            .collect::<Vec<_>>();
        for (pane_id, size) in pending_visible {
            if let Err(source) = self.spawn_task_pane(pane_id.clone(), size) {
                self.task_panes.pending_launches.remove(&pane_id);
                self.task_panes
                    .statuses
                    .insert(pane_id.clone(), format!("task launch failed: {source}"));
                return Err(ReconcileRuntimeError::Spawn {
                    pane_id: pane_id.clone(),
                    source,
                });
            }
            self.task_panes.pending_launches.remove(&pane_id);
            self.task_panes.statuses.remove(&pane_id);
            self.status = format!("task {pane_id} running");
        }

        Ok(())
    }

    /// Tear down a pane's live PTY/parser/runtime and launch a fresh one for the
    /// same `PaneId`. Core layout intent (the durable `PaneId` and its restart
    /// generation) is preserved; no runtime handles are serialized.
    fn restart_terminal_pane(
        &mut self,
        pane_id: PaneId,
        size: PtySize,
    ) -> Result<(), ReconcileRuntimeError> {
        if let Some(mut runtime) = self.terminal_panes.remove(&pane_id) {
            runtime.shutdown();
        }
        // A restart invalidates copy-mode coordinates for that pane.
        if self
            .copy_mode
            .as_ref()
            .is_some_and(|state| state.pane_id == pane_id)
        {
            self.copy_mode = None;
        }
        // Pointer selection/scroll addresses the replaced grid too.
        if self
            .pointer_view
            .as_ref()
            .is_some_and(|view| view.pane_id == pane_id)
        {
            self.pointer_view = None;
        }
        self.spawn_terminal_pane(pane_id.clone(), size)
            .map_err(|source| ReconcileRuntimeError::Restart {
                pane_id: pane_id.clone(),
                source,
            })?;
        self.status = format!("restarted shell for {pane_id}");
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

    fn terminal_pane_ids(&self) -> BTreeSet<PaneId> {
        self.terminal_pane_ids_for_workspace(&self.workspace)
    }

    fn terminal_pane_ids_for_workspace(&self, workspace: &Workspace) -> BTreeSet<PaneId> {
        workspace
            .active_session()
            .panes()
            .iter()
            .filter(|(_, pane)| matches!(pane.kind(), PaneKind::Terminal { .. }))
            .map(|(pane_id, _)| pane_id.clone())
            .collect()
    }

    fn task_pane_ids(&self) -> BTreeSet<PaneId> {
        self.workspace
            .active_session()
            .panes()
            .iter()
            .filter(|(_, pane)| matches!(pane.kind(), PaneKind::Task { .. }))
            .map(|(pane_id, _)| pane_id.clone())
            .collect()
    }

    fn next_runtime_token(&mut self) -> u64 {
        let token = self.next_runtime_token;
        self.next_runtime_token += 1;
        token
    }

    fn spawn_terminal_pane(
        &mut self,
        pane_id: PaneId,
        size: PtySize,
    ) -> Result<(), TerminalRuntimeError> {
        let runtime_token = self.next_runtime_token();
        let runtime = prepare_terminal_pane_runtime(
            &self.workspace,
            &self.shell_program,
            runtime_token,
            pane_id.clone(),
            size,
        )?
        .activate(pane_id.clone(), self.runtime_tx.clone());
        self.terminal_panes.insert(pane_id.clone(), runtime);
        self.status = format!("spawned shell for {pane_id}");
        Ok(())
    }

    fn spawn_task_pane(
        &mut self,
        pane_id: PaneId,
        size: PtySize,
    ) -> Result<(), TerminalRuntimeError> {
        if let Some(mut runtime) = self.task_panes.remove(&pane_id) {
            runtime.shutdown();
        }

        let runtime_token = self.next_runtime_token();
        let runtime = prepare_task_pane_runtime(
            &self.workspace,
            &self.shell_program,
            runtime_token,
            pane_id.clone(),
            size,
        )?
        .activate(pane_id.clone(), self.runtime_tx.clone());
        self.task_panes
            .insert(pane_id.clone(), TaskPaneRuntime::running(runtime));
        Ok(())
    }

    fn drain_runtime_events(&mut self) {
        while let Ok(event) = self.runtime_rx.try_recv() {
            match event {
                PtyRuntimeEvent::Output {
                    pane_id,
                    restart_generation,
                    runtime_token,
                    bytes,
                } => {
                    if let Some(runtime) = self.terminal_panes.get_mut(&pane_id) {
                        if runtime.restart_generation != restart_generation
                            || runtime.runtime_token != runtime_token
                        {
                            continue;
                        }
                        match runtime.parser.feed_pty_bytes(&bytes) {
                            Ok(_) => {
                                self.status =
                                    format!("read {} byte(s) from {pane_id}", bytes.len());
                            }
                            Err(error) => {
                                runtime.error = Some(error.to_string());
                                self.status =
                                    format!("terminal parser failed for {pane_id}: {error}");
                            }
                        }
                    } else if let Some(task) = self.task_panes.get_mut(&pane_id) {
                        if task.runtime.restart_generation != restart_generation
                            || task.runtime.runtime_token != runtime_token
                        {
                            continue;
                        }
                        match task.runtime.parser.feed_pty_bytes(&bytes) {
                            Ok(_) => {
                                self.status =
                                    format!("read {} task byte(s) from {pane_id}", bytes.len());
                            }
                            Err(error) => {
                                task.runtime.error = Some(error.to_string());
                                task.status = format!("task parser failed: {error}");
                                self.status = format!("task parser failed for {pane_id}: {error}");
                            }
                        }
                    }
                }
                PtyRuntimeEvent::ReaderClosed {
                    pane_id,
                    restart_generation,
                    runtime_token,
                } => {
                    if !self.runtime_generation_matches(&pane_id, restart_generation, runtime_token)
                    {
                        continue;
                    }
                    if let Some(task) = self.task_panes.get_mut(&pane_id) {
                        if task.runtime.exit_status.is_some() {
                            continue;
                        }
                        task.status = "task reader closed".to_owned();
                    }
                    self.status = format!("PTY reader closed for {pane_id}");
                }
                PtyRuntimeEvent::Error {
                    pane_id,
                    restart_generation,
                    runtime_token,
                    message,
                } => {
                    if !self.runtime_generation_matches(&pane_id, restart_generation, runtime_token)
                    {
                        continue;
                    }
                    if let Some(runtime) = self.terminal_panes.get_mut(&pane_id) {
                        runtime.error = Some(message.clone());
                    } else if let Some(task) = self.task_panes.get_mut(&pane_id) {
                        task.runtime.error = Some(message.clone());
                        task.status = format!("task reader failed: {message}");
                    }
                    self.status = format!("PTY reader failed for {pane_id}: {message}");
                }
            }
        }
    }

    fn runtime_generation_matches(
        &self,
        pane_id: &PaneId,
        restart_generation: u64,
        runtime_token: u64,
    ) -> bool {
        self.terminal_panes.get(pane_id).is_some_and(|runtime| {
            runtime.restart_generation == restart_generation
                && runtime.runtime_token == runtime_token
        }) || self.task_panes.get(pane_id).is_some_and(|task| {
            task.runtime.restart_generation == restart_generation
                && task.runtime.runtime_token == runtime_token
        })
    }

    fn poll_child_exits(&mut self) {
        for (pane_id, runtime) in self.terminal_panes.iter_mut() {
            if runtime.exit_status.is_some() {
                continue;
            }

            match runtime.controller.try_wait() {
                Ok(Some(exit)) => {
                    runtime.exit_status = Some(exit.status());
                    self.status =
                        format!("PTY {pane_id} exited: {}", exit_status_label(exit.status()));
                }
                Ok(None) => {}
                Err(error) => {
                    runtime.error = Some(error.to_string());
                    self.status = format!("PTY wait failed for {pane_id}: {error}");
                }
            }
        }

        for (pane_id, task) in self.task_panes.iter_mut() {
            if task.runtime.exit_status.is_some() {
                continue;
            }

            match task.runtime.controller.try_wait() {
                Ok(Some(exit)) => {
                    let status = exit.status();
                    task.runtime.exit_status = Some(status);
                    task.status = task_status_label(status);
                    self.status = format!("task {pane_id} {}", task.status);
                }
                Ok(None) => {}
                Err(error) => {
                    task.runtime.error = Some(error.to_string());
                    task.status = format!("task wait failed: {error}");
                    self.status = format!("task wait failed for {pane_id}: {error}");
                }
            }
        }
    }

    /// The live terminal grid attached to a pane, if any.
    pub(crate) fn terminal_grid(&self, pane_id: &PaneId) -> Option<&TerminalGrid> {
        self.terminal_panes
            .get(pane_id)
            .map(|runtime| runtime.parser.grid())
    }

    /// The live task runtime view for a pane: its status label plus the
    /// output grid when a runtime is attached. Falls back to the retained
    /// status of a stopped/pending task.
    pub(crate) fn task_view(&self, pane_id: &PaneId) -> Option<(&str, Option<&TerminalGrid>)> {
        if let Some(task) = self.task_panes.get(pane_id) {
            return Some((task.status.as_str(), Some(task.runtime.parser.grid())));
        }
        self.task_panes
            .statuses
            .get(pane_id)
            .map(|status| (status.as_str(), None))
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

        let Some(target) = target else {
            self.pointer_drag = None;
            return;
        };
        match (target.kind.clone(), pointer.button) {
            // The status strip is the workspace's own front door: clicking
            // it opens the command palette named in its permanent hint.
            (HitTargetKind::StatusStrip, Some(PointerButton::Left)) => self.open_palette(),
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
        let Some(runtime) = self.terminal_panes.get_mut(pane_id) else {
            return false;
        };
        let mode = runtime.parser.mouse_mode();
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
            && let Err(error) = runtime.write_input(&bytes)
        {
            runtime.error = Some(error.to_string());
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
        if let Some(runtime) = self.terminal_panes.get_mut(pane_id) {
            let mode = runtime.parser.mouse_mode();
            let column = pointer
                .column
                .saturating_sub(inner.x)
                .min(inner.width.saturating_sub(1));
            let row = pointer
                .row
                .saturating_sub(inner.y)
                .min(inner.height.saturating_sub(1));
            if let Some(bytes) = encode_mouse_event(mode, pointer, column, row)
                && let Err(error) = runtime.write_input(&bytes)
            {
                runtime.error = Some(error.to_string());
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
            if !self.terminal_panes.contains_key(&pane_id) {
                return;
            }
            let state = self.copy_mode.as_mut().expect("copy mode present");
            let grid = self
                .terminal_panes
                .get(&pane_id)
                .expect("runtime present")
                .parser
                .grid();
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
    /// the OSC 52 clipboard path.
    fn copy_pointer_selection(&mut self) {
        let Some((pane_id, (anchor, cursor))) = self
            .pointer_view
            .as_ref()
            .and_then(|view| Some((view.pane_id.clone(), view.selection?)))
        else {
            self.status = "nothing is selected to copy".to_owned();
            return;
        };
        let Some(runtime) = self.terminal_panes.get(&pane_id) else {
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
        let text = extractor.selected_text(runtime.parser.grid());
        self.clipboard_payload = Some(osc52_sequence(&text));
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
            }
            PaneKind::Agent { .. } => {
                let live = self.agent_panes.get(pane_id);
                if live.is_some_and(|runtime| runtime.pending_approval.is_some()) {
                    commands.extend([CommandId::ApproveAgentAction, CommandId::RejectAgentAction]);
                }
                if live.is_some() {
                    commands.push(CommandId::StopAgent);
                } else {
                    commands.push(CommandId::StartAgent);
                }
            }
            PaneKind::StatusLog { .. } => {}
        }
        commands.push(CommandId::NewTerminal);
        // Splits address the tiled tree; a floating pane cannot be split.
        if !floating {
            commands.extend([CommandId::SplitRight, CommandId::SplitDown]);
        }
        commands.push(CommandId::ZoomPane);
        // Float/dock is one toggle: offer the half that can actually run.
        commands.push(if floating {
            CommandId::DockPane
        } else {
            CommandId::FloatPane
        });
        commands.push(CommandId::ClosePane);

        // "Command palette" leads: the menu is one of the two mouse doors
        // into the palette (the other is the status strip).
        let mut items = vec![ContextMenuItem {
            action: ContextMenuAction::OpenPalette,
            label: "Command palette".to_owned(),
            hint: format_chord(self.keymap.toggle_palette),
        }];
        items.extend(commands.into_iter().filter_map(|command_id| {
            let command = mandatum_commands::command_for_id(command_id)?;
            Some(ContextMenuItem {
                action: ContextMenuAction::Command(command_id),
                label: command.label.to_owned(),
                hint: self.command_key_hint(command_id),
            })
        }));
        items
    }

    /// The keyboard route to a command, for menu hints: a direct key where
    /// one exists, else its global chord, else its palette letter spelled as
    /// "<palette chord> <letter>".
    fn command_key_hint(&self, command_id: CommandId) -> String {
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
        // Dock rides the float letter (one toggle key for the pair).
        let letter_owner = if command_id == CommandId::DockPane {
            CommandId::FloatPane
        } else {
            command_id
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

    // --- Copy mode -------------------------------------------------------------

    fn enter_copy_mode(&mut self) {
        let focused = self.workspace.active_session().focused_pane_id().clone();
        let Some(runtime) = self.terminal_panes.get(&focused) else {
            self.status = format!("pane {focused} has no live terminal to copy from");
            return;
        };
        self.copy_mode = Some(CopyModeState::enter(focused, runtime.parser.grid()));
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
        if !self.terminal_panes.contains_key(&pane_id) {
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
                .terminal_panes
                .get(&pane_id)
                .expect("runtime present")
                .parser
                .grid();
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
            self.terminal_panes
                .get(pane_id)
                .map(|runtime| state.selected_text(runtime.parser.grid()))
        }) else {
            return;
        };

        self.clipboard_payload = Some(osc52_sequence(&text));
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

#[derive(Debug)]
enum ReconcileRuntimeError {
    Spawn {
        pane_id: PaneId,
        source: TerminalRuntimeError,
    },
    Restart {
        pane_id: PaneId,
        source: TerminalRuntimeError,
    },
    Resize {
        pane_id: PaneId,
        source: NativePtyError,
    },
}

impl fmt::Display for ReconcileRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { pane_id, source } => {
                write!(formatter, "PTY spawn failed for {pane_id}: {source}")
            }
            Self::Restart { pane_id, source } => {
                write!(formatter, "PTY restart failed for {pane_id}: {source}")
            }
            Self::Resize { pane_id, source } => {
                write!(formatter, "PTY resize failed for {pane_id}: {source}")
            }
        }
    }
}

impl std::error::Error for ReconcileRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. } | Self::Restart { source, .. } => Some(source),
            Self::Resize { source, .. } => Some(source),
        }
    }
}

#[derive(Debug)]
struct RestoreRuntimeError {
    pane_id: PaneId,
    source: TerminalRuntimeError,
}

impl fmt::Display for RestoreRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "PTY spawn failed for {}: {}",
            self.pane_id, self.source
        )
    }
}

impl std::error::Error for RestoreRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
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
mod tests {
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
    fn test_config() -> AppConfig {
        AppConfig {
            project_path: PathBuf::from("/tmp/mandatum"),
            workspace_file: PathBuf::from("/tmp/mandatum/.mandatum/workspace.json"),
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

        // 'c' on a focused task pane means Stop Task.
        state.handle_key(ctrl('p'));
        state.handle_key(key(KeyCode::Char('c')));
        assert!(!state.palette_open());
        assert!(
            state.status().contains("stopped before launch")
                || state.status().contains("not running"),
            "{}",
            state.status()
        );
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

        state.handle_key(ctrl('p'));
        state.handle_key(key(KeyCode::Char('x')));

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

    fn pump_runtime_until(
        state: &mut AppState,
        mut predicate: impl FnMut(&AppState) -> bool,
    ) -> bool {
        for _ in 0..300 {
            state.tick_runtime();
            if predicate(state) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
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
        state.handle_event(InputEvent::Paste("echo SELECT_ME\r".to_owned()));
        // Wait for the output line (exactly the marker), not the echoed
        // command line (which contains it).
        let printed = pump_runtime_until(&mut state, |state| {
            state.terminal_panes.get(&pane_id).is_some_and(|runtime| {
                runtime
                    .parser
                    .grid()
                    .snapshot()
                    .iter()
                    .any(|line| line.trim_end() == "SELECT_ME")
            })
        });
        assert!(printed, "marker output never reached the grid");
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
            .find(|(_, line)| line.trim_end() == "SELECT_ME")
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
        state.build_scene(POINTER_FRAME);

        // Drag a selection, then plain-click: the selection clears.
        send_pointer(&mut state, left(PointerKind::Down, 5, 5));
        send_pointer(&mut state, left(PointerKind::Drag, 12, 5));
        send_pointer(&mut state, left(PointerKind::Up, 12, 5));
        assert!(state.pane_view_state(&pane_id).selection.is_some());
        send_pointer(&mut state, left(PointerKind::Down, 5, 6));
        send_pointer(&mut state, left(PointerKind::Up, 5, 6));
        assert!(state.pane_view_state(&pane_id).selection.is_none());

        // Selection is not a mode: keys still flow to the child (L5).
        send_pointer(&mut state, left(PointerKind::Down, 5, 5));
        send_pointer(&mut state, left(PointerKind::Drag, 12, 5));
        send_pointer(&mut state, left(PointerKind::Up, 12, 5));
        state.handle_key(key(KeyCode::Char('w')));
        assert!(state.status().contains("sent 1 byte(s)"));

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
            .runtime_tx
            .send(PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation: 0,
                runtime_token: 0,
                bytes: b"OLD_READER_OUTPUT".to_vec(),
            })
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
            .runtime_tx
            .send(PtyRuntimeEvent::ReaderClosed {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
            })
            .unwrap();
        state
            .runtime_tx
            .send(PtyRuntimeEvent::Error {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
                message: "STALE_TERMINAL_READER_ERROR".to_owned(),
            })
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
            .runtime_tx
            .send(PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
                bytes: b"OLD_TASK_OUTPUT".to_vec(),
            })
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
            .runtime_tx
            .send(PtyRuntimeEvent::Output {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
                bytes: b"OLD_HIDDEN_RERUN_OUTPUT".to_vec(),
            })
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
            .runtime_tx
            .send(PtyRuntimeEvent::Error {
                pane_id: pane_id.clone(),
                restart_generation,
                runtime_token,
                message: "late reader error".to_owned(),
            })
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
            .runtime_tx
            .send(PtyRuntimeEvent::ReaderClosed {
                pane_id: pane_id.clone(),
                restart_generation: task.runtime.restart_generation,
                runtime_token: task.runtime.runtime_token,
            })
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
            .agent_tx
            .send(crate::agent_runtime::AgentRuntimeEvent {
                pane_id: pane_id.clone(),
                restart_generation: before_generation,
                runtime_token: before_token,
                event: AgentSessionEvent::Summary("STALE_AGENT_SUMMARY".to_owned()),
            })
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
}
