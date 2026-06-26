//! Terminal application runtime for Mandatum.
//!
//! The runtime owns terminal lifecycle, live PTY handles, parser instances, and
//! input orchestration. Product mutations still go through `mandatum-commands`,
//! and drawing still goes through `mandatum-renderer`.

mod clipboard;
mod copy_mode;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mandatum_commands::{
    BUILT_IN_COMMANDS, CommandCategory, CommandContext, CommandError, CommandId, CommandTarget,
    PaletteContext, PaletteInput, PaletteKey, RuntimeCommand, RuntimeTaskCommand, command_target,
    dispatch_command, resolve_palette_key_with_context,
};
use mandatum_core::{
    ActionOutcome, CoreAction, PaneId, PaneKind, PersistenceError, PersistenceRequest,
    TaskPaneIntent, Workspace,
};
use mandatum_pty::{
    ChildExitStatus, NativePtyController, NativePtyError, NativePtyReader, NativePtySession,
    NativePtyWriter, PtyEvent, PtySessionId, PtySize, ResizeIntent, SpawnIntent,
};
use mandatum_renderer::{
    PaletteItem, PaletteView, PaneTaskRuntime, PaneTerminalGrid, RenderState, RuntimePaneViews,
    SelectionPoint, TaskRuntimeView, TerminalGridView, TerminalViewport, pane_content_area,
    render_with_runtime_views,
};
use mandatum_terminal_vt::{TerminalGrid, TerminalParser, TerminalSize};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

use crate::{clipboard::osc52_sequence, copy_mode::CopyModeState};

const POLL_INTERVAL: Duration = Duration::from_millis(40);
const PTY_READ_CHUNK_BYTES: usize = 8192;
const MAX_WORKSPACE_FILE_BYTES: u64 = 1024 * 1024;
static WORKSPACE_FILE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn run() -> Result<(), AppError> {
    run_with_config(AppConfig::from_current_dir()?)
}

pub fn run_with_config(config: AppConfig) -> Result<(), AppError> {
    let mut app = AppState::new(config);
    let mut terminal = TerminalGuard::enter()?;
    let size = terminal.size()?;
    app.handle_terminal_resize(size.width, size.height);

    while !app.should_quit() {
        app.tick_runtime();
        draw(&mut terminal, &app)?;

        if let Some(payload) = app.take_clipboard_payload() {
            write_clipboard_payload(&payload)?;
        }

        if event::poll(POLL_INTERVAL)? {
            app.handle_event(event::read()?);
        }
    }

    app.shutdown();
    draw(&mut terminal, &app)?;
    terminal.restore()?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    pub workspace_name: String,
    pub project_path: PathBuf,
    pub workspace_file: PathBuf,
    pub shell_program: String,
    pub task_command: String,
    pub spawn_pty: bool,
    pub restore_on_startup: bool,
}

impl AppConfig {
    pub fn from_current_dir() -> io::Result<Self> {
        let project_path = std::env::current_dir()?;
        Ok(Self {
            workspace_name: "Mandatum".to_owned(),
            workspace_file: default_workspace_file(&project_path),
            project_path,
            shell_program: default_shell_program(),
            task_command: default_task_command(),
            spawn_pty: true,
            restore_on_startup: true,
        })
    }
}

pub fn default_workspace_file(project_path: &Path) -> PathBuf {
    project_path.join(".mandatum").join("workspace.json")
}

pub struct AppState {
    workspace: Workspace,
    command_context: CommandContext,
    workspace_file: PathBuf,
    shell_program: String,
    task_command: String,
    spawn_pty: bool,
    palette_open: bool,
    should_quit: bool,
    terminal_size: Option<(u16, u16)>,
    status: String,
    preserve_status_on_next_resize: bool,
    last_redraw: Instant,
    terminal_panes: BTreeMap<PaneId, TerminalPaneRuntime>,
    task_panes: BTreeMap<PaneId, TaskPaneRuntime>,
    pending_task_launches: BTreeSet<PaneId>,
    task_statuses: BTreeMap<PaneId, String>,
    copy_mode: Option<CopyModeState>,
    clipboard_payload: Option<Vec<u8>>,
    last_copied: Option<String>,
    runtime_tx: Sender<PtyRuntimeEvent>,
    runtime_rx: Receiver<PtyRuntimeEvent>,
    next_runtime_token: u64,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let command_context =
            CommandContext::for_project(config.workspace_name.clone(), config.project_path.clone());
        let workspace = Workspace::new(config.workspace_name, config.project_path);
        let (runtime_tx, runtime_rx) = mpsc::channel();
        let restore_on_startup = config.restore_on_startup;

        let mut state = Self {
            workspace,
            command_context,
            workspace_file: config.workspace_file,
            shell_program: config.shell_program,
            task_command: config.task_command,
            spawn_pty: config.spawn_pty,
            palette_open: false,
            should_quit: false,
            terminal_size: None,
            status: "ready".to_owned(),
            preserve_status_on_next_resize: false,
            last_redraw: Instant::now(),
            terminal_panes: BTreeMap::new(),
            task_panes: BTreeMap::new(),
            pending_task_launches: BTreeSet::new(),
            task_statuses: BTreeMap::new(),
            copy_mode: None,
            clipboard_payload: None,
            last_copied: None,
            runtime_tx,
            runtime_rx,
            next_runtime_token: 1,
        };

        if restore_on_startup {
            state.restore_workspace_at_startup();
        }

        state
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn palette_open(&self) -> bool {
        self.palette_open
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
        &self.workspace_file
    }

    pub fn live_terminal_count(&self) -> usize {
        self.terminal_panes.len()
    }

    pub fn live_task_count(&self) -> usize {
        self.task_panes.len()
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

    pub fn palette_items(&self) -> Vec<PaletteItem<'static>> {
        BUILT_IN_COMMANDS
            .iter()
            .map(|command| PaletteItem::new(command.label, category_label(command.category)))
            .collect()
    }

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Resize(columns, rows) => self.handle_terminal_resize(columns, rows),
            // Paste only reaches the shell in normal mode; copy mode owns input.
            Event::Paste(text) if self.copy_mode.is_none() => {
                self.write_to_focused_terminal(text.as_bytes())
            }
            _ => {}
        }
    }

    pub fn handle_terminal_resize(&mut self, columns: u16, rows: u16) {
        self.terminal_size = Some((columns, rows));
        // Copy-mode coordinates address a specific grid geometry; a resize
        // reshapes the buffer, so leave copy mode rather than track moved coordinates.
        if self.copy_mode.is_some() {
            self.copy_mode = None;
        }
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

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.copy_mode.is_some() {
            self.handle_copy_mode_key(key);
            self.mark_redraw();
            return;
        }

        match key_to_input_with_palette_context(key, self.palette_open, self.palette_context()) {
            RuntimeInput::Quit => {
                self.should_quit = true;
                self.status = "quitting".to_owned();
            }
            RuntimeInput::TogglePalette => {
                self.palette_open = !self.palette_open;
                self.status = if self.palette_open {
                    "command palette open".to_owned()
                } else {
                    "command palette closed".to_owned()
                };
            }
            RuntimeInput::ClosePalette => {
                if self.palette_open {
                    self.palette_open = false;
                    self.status = "command palette closed".to_owned();
                }
            }
            RuntimeInput::Dispatch(command_id) => {
                if self.palette_open {
                    self.palette_open = false;
                }
                self.dispatch(command_id);
            }
            RuntimeInput::SendToTerminal(bytes) => self.write_to_focused_terminal(&bytes),
            RuntimeInput::Noop => {}
        }
        self.mark_redraw();
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
        PaletteContext {
            focused_pane_is_task: self.focused_pane_is_task(),
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
        }
    }

    fn dispatch_runtime_task_command(&mut self, task_command: RuntimeTaskCommand) {
        match task_command {
            RuntimeTaskCommand::RunConfiguredTask => self.run_configured_task(),
            RuntimeTaskCommand::RerunFocusedTask => self.rerun_focused_task(),
            RuntimeTaskCommand::StopFocusedTask => self.stop_focused_task(),
        }
    }

    pub fn tick_runtime(&mut self) {
        self.drain_runtime_events();
        self.poll_child_exits();
    }

    pub fn shutdown(&mut self) {
        self.shutdown_task_panes();
        self.shutdown_terminal_panes();
        self.status = "terminal sessions stopped".to_owned();
    }

    fn shutdown_terminal_panes(&mut self) {
        for pane in self.terminal_panes.values_mut() {
            pane.shutdown();
        }
        self.terminal_panes.clear();
    }

    fn shutdown_task_panes(&mut self) {
        for pane in self.task_panes.values_mut() {
            pane.shutdown();
        }
        self.task_panes.clear();
        self.pending_task_launches.clear();
        self.task_statuses.clear();
    }

    fn save_workspace_to_disk(&mut self) {
        match write_workspace_file(&self.workspace_file, &self.workspace) {
            Ok(()) => {
                self.status = format!("workspace saved to {}", self.workspace_file.display());
            }
            Err(error) => {
                self.status = format!("workspace save failed: {error}");
            }
        }
    }

    fn restore_workspace_at_startup(&mut self) {
        match read_workspace_file(&self.workspace_file) {
            Ok(workspace) => match self.prepare_restore_runtimes(&workspace) {
                Ok(runtimes) => {
                    self.replace_workspace_from_disk(workspace, runtimes);
                    self.status =
                        format!("workspace restored from {}", self.workspace_file.display());
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
        match read_workspace_file(&self.workspace_file) {
            Ok(workspace) => match self.prepare_restore_runtimes(&workspace) {
                Ok(runtimes) => {
                    self.replace_workspace_from_disk(workspace, runtimes);
                    self.status =
                        format!("workspace restored from {}", self.workspace_file.display());
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
        self.discard_pending_runtime_events();
        self.workspace = workspace;
        self.command_context = command_context_for_workspace(&self.workspace);
        self.copy_mode = None;
        self.clipboard_payload = None;
        self.last_copied = None;
        self.pending_task_launches.clear();
        self.task_statuses.clear();
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
            self.task_statuses.insert(pane_id.clone(), status.clone());
            self.status = format!("task {pane_id} {status}");
            self.mark_redraw();
            return;
        }

        let Some(size) = self.visible_task_size(&pane_id) else {
            if let Some(mut runtime) = self.task_panes.remove(&pane_id) {
                runtime.shutdown();
            }
            let status = "pending rerun: waiting for visible pane size".to_owned();
            self.pending_task_launches.insert(pane_id.clone());
            self.task_statuses.insert(pane_id.clone(), status.clone());
            self.status = format!("task {pane_id} {status}");
            self.mark_redraw();
            return;
        };

        self.pending_task_launches.remove(&pane_id);
        if let Err(source) = self.spawn_task_pane(pane_id.clone(), size) {
            self.task_statuses
                .insert(pane_id.clone(), format!("task rerun failed: {source}"));
            self.status = format!("task rerun failed: {source}");
        } else {
            self.task_statuses.remove(&pane_id);
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

        if self.pending_task_launches.remove(&pane_id) {
            let status = "stopped before launch".to_owned();
            self.task_statuses.insert(pane_id.clone(), status);
            self.status = format!("task {pane_id} stopped before launch");
            self.mark_redraw();
            return;
        }

        let Some(mut task) = self.task_panes.remove(&pane_id) else {
            let status = "not running".to_owned();
            self.task_statuses.insert(pane_id.clone(), status);
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
                self.task_statuses
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
            self.pending_task_launches.insert(pane_id.clone());
            self.task_statuses.insert(pane_id.clone(), status.clone());
            self.status = format!("task pane {pane_id} created; {status}");
            return Ok(());
        };

        if let Err(source) = self.spawn_task_pane(pane_id.clone(), size) {
            self.pending_task_launches.remove(&pane_id);
            self.task_statuses
                .insert(pane_id.clone(), format!("task launch failed: {source}"));
            return Err(ReconcileRuntimeError::Spawn {
                pane_id: pane_id.clone(),
                source,
            });
        }
        self.pending_task_launches.remove(&pane_id);
        self.task_statuses.remove(&pane_id);
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
        self.pending_task_launches
            .retain(|pane_id| task_pane_ids.contains(pane_id));
        self.task_statuses
            .retain(|pane_id, _| task_pane_ids.contains(pane_id));
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
                self.pending_task_launches.contains(pane_id)
                    && !self.task_panes.contains_key(pane_id)
            })
            .collect::<Vec<_>>();
        for (pane_id, size) in pending_visible {
            if let Err(source) = self.spawn_task_pane(pane_id.clone(), size) {
                self.pending_task_launches.remove(&pane_id);
                self.task_statuses
                    .insert(pane_id.clone(), format!("task launch failed: {source}"));
                return Err(ReconcileRuntimeError::Spawn {
                    pane_id: pane_id.clone(),
                    source,
                });
            }
            self.pending_task_launches.remove(&pane_id);
            self.task_statuses.remove(&pane_id);
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
        let area = Rect::new(0, 0, columns, rows);
        let session = workspace.active_session();

        session
            .panes()
            .iter()
            .filter_map(|(pane_id, pane)| {
                if !include_kind(pane.kind()) {
                    return None;
                }

                let content_area = pane_content_area(workspace, area, pane_id)?;
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
        self.task_panes.insert(
            pane_id.clone(),
            TaskPaneRuntime {
                runtime,
                status: "running".to_owned(),
            },
        );
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
        for (pane_id, runtime) in &mut self.terminal_panes {
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

        for (pane_id, task) in &mut self.task_panes {
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

    fn terminal_grid_items(&self) -> Vec<PaneTerminalGrid<'_>> {
        self.terminal_panes
            .iter()
            .map(|(pane_id, runtime)| {
                let viewport = self.viewport_for(pane_id);
                PaneTerminalGrid::with_viewport(pane_id, runtime.parser.grid(), viewport)
            })
            .collect()
    }

    fn task_runtime_items(&self) -> Vec<PaneTaskRuntime<'_>> {
        let mut items = self
            .task_panes
            .iter()
            .map(|(pane_id, task)| {
                PaneTaskRuntime::with_output(
                    pane_id,
                    task.status.as_str(),
                    task.runtime.parser.grid(),
                )
            })
            .collect::<Vec<_>>();
        items.extend(
            self.task_statuses
                .iter()
                .filter(|(pane_id, _)| !self.task_panes.contains_key(*pane_id))
                .map(|(pane_id, status)| PaneTaskRuntime::new(pane_id, status.as_str())),
        );
        items
    }

    fn viewport_for(&self, pane_id: &PaneId) -> TerminalViewport {
        match &self.copy_mode {
            Some(state) if &state.pane_id == pane_id => TerminalViewport {
                scroll_offset: state.scroll_offset,
                selection: state.selection_span().map(|(start, end)| {
                    (
                        SelectionPoint::new(start.0, start.1),
                        SelectionPoint::new(end.0, end.1),
                    )
                }),
                copy_cursor: Some(SelectionPoint::new(state.cursor_row, state.cursor_col)),
            },
            _ => TerminalViewport::live(),
        }
    }

    // --- Copy mode -------------------------------------------------------------

    fn enter_copy_mode(&mut self) {
        let focused = self.workspace.active_session().focused_pane_id().clone();
        let Some(runtime) = self.terminal_panes.get(&focused) else {
            self.status = format!("pane {focused} has no live terminal to copy from");
            return;
        };
        self.copy_mode = Some(CopyModeState::enter(focused, runtime.parser.grid()));
        self.palette_open = false;
        self.status = "copy mode: hjkl/arrows move, v select, y/Enter copy, Esc exit".to_owned();
    }

    fn exit_copy_mode(&mut self) {
        self.copy_mode = None;
        self.status = "copy mode closed".to_owned();
    }

    fn handle_copy_mode_key(&mut self, key: KeyEvent) {
        let Some(pane_id) = self.copy_mode.as_ref().map(|state| state.pane_id.clone()) else {
            return;
        };
        if !self.terminal_panes.contains_key(&pane_id) {
            self.copy_mode = None;
            self.status = "copy mode closed: pane is no longer live".to_owned();
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
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

enum CopyModeAction {
    Continue,
    Exit,
    Copy,
}

fn copy_mode_action(
    state: &mut CopyModeState,
    grid: &TerminalGrid,
    key: KeyEvent,
) -> CopyModeAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return CopyModeAction::Exit,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeInput {
    Quit,
    TogglePalette,
    ClosePalette,
    Dispatch(CommandId),
    SendToTerminal(Vec<u8>),
    Noop,
}

pub fn key_to_input(key: KeyEvent, palette_open: bool) -> RuntimeInput {
    key_to_input_with_palette_context(key, palette_open, PaletteContext::default())
}

pub fn key_to_input_with_palette_context(
    key: KeyEvent,
    palette_open: bool,
    palette_context: PaletteContext,
) -> RuntimeInput {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        return RuntimeInput::Quit;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
        return RuntimeInput::TogglePalette;
    }

    if palette_open {
        return key_to_palette_input(key, palette_context);
    }

    key_to_terminal_input(key)
        .map(RuntimeInput::SendToTerminal)
        .unwrap_or(RuntimeInput::Noop)
}

pub fn key_to_terminal_input(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            control_byte(character).map(|byte| vec![byte])
        }
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::ALT) => {
            let mut bytes = vec![0x1b];
            bytes.extend(character.to_string().as_bytes());
            Some(bytes)
        }
        KeyCode::Char(character) => Some(character.to_string().into_bytes()),
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        _ => None,
    }
}

fn key_to_palette_input(key: KeyEvent, palette_context: PaletteContext) -> RuntimeInput {
    let Some(key) = palette_key_for(key) else {
        return RuntimeInput::Noop;
    };

    match resolve_palette_key_with_context(key, palette_context) {
        PaletteInput::Close => RuntimeInput::ClosePalette,
        PaletteInput::Quit => RuntimeInput::Quit,
        PaletteInput::Dispatch(command_id) => RuntimeInput::Dispatch(command_id),
        PaletteInput::Noop => RuntimeInput::Noop,
    }
}

fn palette_key_for(key: KeyEvent) -> Option<PaletteKey> {
    match key.code {
        KeyCode::Esc => Some(PaletteKey::Escape),
        KeyCode::Tab => Some(PaletteKey::Tab),
        KeyCode::BackTab => Some(PaletteKey::BackTab),
        KeyCode::Char(character) => Some(PaletteKey::Character(character)),
        _ => None,
    }
}

fn write_clipboard_payload(payload: &[u8]) -> io::Result<()> {
    // OSC 52 is processed by the host terminal regardless of the alternate
    // screen, so writing it straight to stdout does not disturb the rendered UI.
    let mut stdout = io::stdout();
    stdout.write_all(payload)?;
    stdout.flush()
}

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();

        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(mut terminal) => {
                if let Err(error) = terminal.clear() {
                    let _ = disable_raw_mode();
                    let _ = execute!(
                        terminal.backend_mut(),
                        DisableBracketedPaste,
                        LeaveAlternateScreen
                    );
                    return Err(error);
                }
                Ok(Self {
                    terminal,
                    restored: false,
                })
            }
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stdout = io::stdout();
                let _ = execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen);
                Err(error)
            }
        }
    }

    pub fn size(&self) -> io::Result<Rect> {
        let size = self.terminal.size()?;
        Ok(Rect::new(0, 0, size.width, size.height))
    }

    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }

        let raw_mode_result = disable_raw_mode();
        let screen_result = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let cursor_result = self.terminal.show_cursor();
        self.restored = true;

        raw_mode_result?;
        screen_result?;
        cursor_result
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct TerminalPaneRuntime {
    parser: TerminalParser,
    controller: NativePtyController,
    writer: NativePtyWriter,
    reader_thread: Option<JoinHandle<()>>,
    size: PtySize,
    restart_generation: u64,
    runtime_token: u64,
    exit_status: Option<ChildExitStatus>,
    error: Option<String>,
}

impl TerminalPaneRuntime {
    fn write_input(&mut self, bytes: &[u8]) -> Result<(), NativePtyError> {
        self.writer.write_input(bytes)
    }

    fn resize(&mut self, size: PtySize) -> Result<(), NativePtyError> {
        if self.size == size {
            return Ok(());
        }

        self.controller.resize(ResizeIntent::new(
            self.controller.session_id().clone(),
            size,
        ))?;
        self.parser.resize(to_terminal_size(size));
        self.size = size;
        Ok(())
    }

    fn shutdown(&mut self) {
        self.writer.close_input();
        let _ = self.controller.kill();
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }

    fn stop(&mut self) -> Result<(), NativePtyError> {
        self.writer.close_input();
        let result = self.controller.kill();
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
        result
    }
}

struct TaskPaneRuntime {
    runtime: TerminalPaneRuntime,
    status: String,
}

impl TaskPaneRuntime {
    fn resize(&mut self, size: PtySize) -> Result<(), NativePtyError> {
        self.runtime.resize(size)
    }

    fn shutdown(&mut self) {
        self.runtime.shutdown();
    }

    fn stop(&mut self) -> Result<(), NativePtyError> {
        self.runtime.stop()
    }
}

struct PendingTerminalPaneRuntime {
    reader: NativePtyReader,
    controller: NativePtyController,
    writer: NativePtyWriter,
    size: PtySize,
    restart_generation: u64,
    runtime_token: u64,
}

impl PendingTerminalPaneRuntime {
    fn activate(self, pane_id: PaneId, tx: Sender<PtyRuntimeEvent>) -> TerminalPaneRuntime {
        let Self {
            reader,
            controller,
            writer,
            size,
            restart_generation,
            runtime_token,
        } = self;
        let reader_thread =
            spawn_reader_thread(pane_id, restart_generation, runtime_token, reader, tx);
        let parser = TerminalParser::new(to_terminal_size(size));

        TerminalPaneRuntime {
            parser,
            controller,
            writer,
            reader_thread: Some(reader_thread),
            size,
            restart_generation,
            runtime_token,
            exit_status: None,
            error: None,
        }
    }

    fn shutdown(&mut self) {
        self.writer.close_input();
        let _ = self.controller.kill();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PtyRuntimeEvent {
    Output {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
        bytes: Vec<u8>,
    },
    ReaderClosed {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
    },
    Error {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
        message: String,
    },
}

#[derive(Debug)]
pub enum AppError {
    Io(io::Error),
    Command(CommandError),
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Command(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<io::Error> for AppError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<CommandError> for AppError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

#[derive(Debug)]
enum TerminalRuntimeError {
    MissingPane(PaneId),
    UnexpectedPaneKind {
        pane_id: PaneId,
        expected: &'static str,
    },
    SpawnIntent(mandatum_pty::SpawnIntentError),
    NativePty(NativePtyError),
}

impl fmt::Display for TerminalRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPane(pane_id) => write!(formatter, "pane {pane_id} was not found"),
            Self::UnexpectedPaneKind { pane_id, expected } => {
                write!(formatter, "pane {pane_id} is not a {expected} pane")
            }
            Self::SpawnIntent(error) => write!(formatter, "{error}"),
            Self::NativePty(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for TerminalRuntimeError {}

impl From<mandatum_pty::SpawnIntentError> for TerminalRuntimeError {
    fn from(error: mandatum_pty::SpawnIntentError) -> Self {
        Self::SpawnIntent(error)
    }
}

impl From<NativePtyError> for TerminalRuntimeError {
    fn from(error: NativePtyError) -> Self {
        Self::NativePty(error)
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

#[derive(Debug)]
enum WorkspaceFileError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    UnsafePath {
        path: PathBuf,
        message: String,
    },
    Persistence {
        path: PathBuf,
        source: PersistenceError,
    },
}

impl fmt::Display for WorkspaceFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::UnsafePath { path, message } => {
                write!(formatter, "{}: {message}", path.display())
            }
            Self::Persistence { path, source } => {
                write!(formatter, "{}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for WorkspaceFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::UnsafePath { .. } => None,
            Self::Persistence { source, .. } => Some(source),
        }
    }
}

fn write_workspace_file(path: &Path, workspace: &Workspace) -> Result<(), WorkspaceFileError> {
    let json = workspace
        .to_json()
        .map_err(|source| WorkspaceFileError::Persistence {
            path: path.to_path_buf(),
            source,
        })?;
    ensure_parent_dir(path)?;
    reject_unsafe_existing_file(path)?;
    let temp_path = workspace_temp_path(path)?;
    let write_result = write_workspace_file_atomically(path, &temp_path, json.as_bytes());
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn write_workspace_file_atomically(
    path: &Path,
    temp_path: &Path,
    contents: &[u8],
) -> Result<(), WorkspaceFileError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|source| WorkspaceFileError::Io {
            path: temp_path.to_path_buf(),
            source,
        })?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|source| WorkspaceFileError::Io {
            path: temp_path.to_path_buf(),
            source,
        })?;
    drop(file);

    fs::rename(temp_path, path).map_err(|source| WorkspaceFileError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn read_workspace_file(path: &Path) -> Result<Workspace, WorkspaceFileError> {
    let metadata = safe_workspace_file_metadata(path)?;
    if metadata.len() > MAX_WORKSPACE_FILE_BYTES {
        return Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: format!(
                "workspace file is too large: {} byte(s), max {MAX_WORKSPACE_FILE_BYTES}",
                metadata.len()
            ),
        });
    }

    let mut file = fs::File::open(path).map_err(|source| WorkspaceFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut json = String::new();
    let mut limited = (&mut file).take(MAX_WORKSPACE_FILE_BYTES + 1);
    limited
        .read_to_string(&mut json)
        .map_err(|source| WorkspaceFileError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if json.len() as u64 > MAX_WORKSPACE_FILE_BYTES {
        return Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: format!(
                "workspace file is too large: more than {MAX_WORKSPACE_FILE_BYTES} byte(s)"
            ),
        });
    }
    Workspace::from_json(&json).map_err(|source| WorkspaceFileError::Persistence {
        path: path.to_path_buf(),
        source,
    })
}

fn ensure_parent_dir(path: &Path) -> Result<(), WorkspaceFileError> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };

    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(WorkspaceFileError::UnsafePath {
            path: parent.to_path_buf(),
            message: "workspace directory must not be a symlink".to_owned(),
        }),
        Ok(metadata) if !metadata.is_dir() => Err(WorkspaceFileError::UnsafePath {
            path: parent.to_path_buf(),
            message: "workspace parent path is not a directory".to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(parent)
            .map_err(|source| WorkspaceFileError::Io {
                path: parent.to_path_buf(),
                source,
            }),
        Err(source) => Err(WorkspaceFileError::Io {
            path: parent.to_path_buf(),
            source,
        }),
    }
}

fn reject_unsafe_existing_file(path: &Path) -> Result<(), WorkspaceFileError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace file must not be a symlink".to_owned(),
        }),
        Ok(metadata) if !metadata.is_file() => Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace path is not a regular file".to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(WorkspaceFileError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn safe_workspace_file_metadata(path: &Path) -> Result<fs::Metadata, WorkspaceFileError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| WorkspaceFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace file must not be a symlink".to_owned(),
        });
    }
    if !metadata.is_file() {
        return Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace path is not a regular file".to_owned(),
        });
    }
    Ok(metadata)
}

fn workspace_temp_path(path: &Path) -> Result<PathBuf, WorkspaceFileError> {
    let parent = path
        .parent()
        .ok_or_else(|| WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace path has no parent directory".to_owned(),
        })?;
    let file_name = path
        .file_name()
        .ok_or_else(|| WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace path has no file name".to_owned(),
        })?;
    let file_name = file_name.to_string_lossy();
    let counter = WORKSPACE_FILE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id())))
}

fn prepare_terminal_pane_runtime(
    workspace: &Workspace,
    shell_program: &str,
    runtime_token: u64,
    pane_id: PaneId,
    size: PtySize,
) -> Result<PendingTerminalPaneRuntime, TerminalRuntimeError> {
    let session = workspace.active_session();
    let pane = session
        .pane(&pane_id)
        .ok_or_else(|| TerminalRuntimeError::MissingPane(pane_id.clone()))?;
    let session_id = PtySessionId::new(pane_id.as_str().to_owned());
    let restart_generation = pane.restart_generation();
    let mut intent = SpawnIntent::new(session_id, shell_program.to_owned(), size)?;
    if let Some(cwd) = pane.cwd() {
        intent = intent.with_cwd(cwd.clone());
    }
    // The hardened parser handles real VT output, so advertise a capable
    // terminal. The rest of the environment (PATH, HOME, prompt) is inherited.
    intent = intent.with_environment([("TERM", "xterm-256color")]);

    let session = NativePtySession::spawn(intent)?;
    let parts = session.into_split()?;

    Ok(PendingTerminalPaneRuntime {
        reader: parts.reader,
        controller: parts.controller,
        writer: parts.writer,
        size,
        restart_generation,
        runtime_token,
    })
}

fn prepare_task_pane_runtime(
    workspace: &Workspace,
    shell_program: &str,
    runtime_token: u64,
    pane_id: PaneId,
    size: PtySize,
) -> Result<PendingTerminalPaneRuntime, TerminalRuntimeError> {
    let session = workspace.active_session();
    let pane = session
        .pane(&pane_id)
        .ok_or_else(|| TerminalRuntimeError::MissingPane(pane_id.clone()))?;
    let PaneKind::Task { intent } = pane.kind() else {
        return Err(TerminalRuntimeError::UnexpectedPaneKind {
            pane_id,
            expected: "task",
        });
    };

    let session_id = PtySessionId::new(pane.id().as_str().to_owned());
    let restart_generation = pane.restart_generation();
    let mut spawn_intent = SpawnIntent::new(session_id, shell_program.to_owned(), size)?
        .with_arguments(["-c", intent.command.as_str()]);
    if let Some(cwd) = intent.cwd.as_ref().or_else(|| pane.cwd()) {
        spawn_intent = spawn_intent.with_cwd(cwd.clone());
    }
    spawn_intent = spawn_intent.with_environment([("TERM", "xterm-256color")]);

    let session = NativePtySession::spawn(spawn_intent)?;
    let parts = session.into_split()?;

    Ok(PendingTerminalPaneRuntime {
        reader: parts.reader,
        controller: parts.controller,
        writer: parts.writer,
        size,
        restart_generation,
        runtime_token,
    })
}

fn draw(terminal: &mut TerminalGuard, app: &AppState) -> io::Result<()> {
    let palette_items = app.palette_items();
    let terminal_grid_items = app.terminal_grid_items();
    let task_runtime_items = app.task_runtime_items();
    terminal.terminal.draw(|frame| {
        render_with_runtime_views(
            frame,
            RenderState {
                workspace: app.workspace(),
                palette: PaletteView {
                    open: app.palette_open(),
                    items: &palette_items,
                },
                status: Some(app.status()),
            },
            RuntimePaneViews::new(
                TerminalGridView::new(&terminal_grid_items),
                TaskRuntimeView::new(&task_runtime_items),
            ),
        );
    })?;
    Ok(())
}

fn spawn_reader_thread(
    pane_id: PaneId,
    restart_generation: u64,
    runtime_token: u64,
    mut reader: NativePtyReader,
    tx: Sender<PtyRuntimeEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match reader.read_event(PTY_READ_CHUNK_BYTES) {
                Ok(Some(PtyEvent::Output(output))) => {
                    let _ = tx.send(PtyRuntimeEvent::Output {
                        pane_id: pane_id.clone(),
                        restart_generation,
                        runtime_token,
                        bytes: output.into_bytes(),
                    });
                }
                Ok(Some(PtyEvent::ChildExited(_))) | Ok(Some(PtyEvent::BackpressureChanged(_))) => {
                }
                Ok(None) => {
                    let _ = tx.send(PtyRuntimeEvent::ReaderClosed {
                        pane_id,
                        restart_generation,
                        runtime_token,
                    });
                    break;
                }
                Err(error) => {
                    let _ = tx.send(PtyRuntimeEvent::Error {
                        pane_id,
                        restart_generation,
                        runtime_token,
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
    })
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

fn category_label(category: CommandCategory) -> &'static str {
    match category {
        CommandCategory::Project => "project",
        CommandCategory::Pane => "pane",
        CommandCategory::Task => "task",
        CommandCategory::Layout => "layout",
        CommandCategory::Persistence => "persistence",
    }
}

fn default_shell_program() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
}

fn default_task_command() -> String {
    "cargo test".to_owned()
}

fn to_terminal_size(size: PtySize) -> TerminalSize {
    TerminalSize::new(size.columns(), size.rows()).expect("PTY sizes are non-zero")
}

fn control_byte(character: char) -> Option<u8> {
    let lower = character.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        Some((lower as u8) - b'a' + 1)
    } else if character == '[' {
        Some(0x1b)
    } else {
        None
    }
}

fn exit_status_label(status: ChildExitStatus) -> String {
    match status {
        ChildExitStatus::Exited { code } => format!("exit {code}"),
        ChildExitStatus::Signaled { signal } => format!("signal {signal}"),
        ChildExitStatus::Unknown => "unknown".to_owned(),
    }
}

fn task_status_label(status: ChildExitStatus) -> String {
    match status {
        ChildExitStatus::Exited { code: 0 } => "succeeded: exit 0".to_owned(),
        ChildExitStatus::Exited { code } => format!("failed: exit {code}"),
        ChildExitStatus::Signaled { signal } => format!("failed: signal {signal}"),
        ChildExitStatus::Unknown => "failed: unknown exit".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use mandatum_core::CoreAction;

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn state() -> AppState {
        AppState::new(AppConfig {
            workspace_name: "Mandatum".to_owned(),
            project_path: PathBuf::from("/tmp/mandatum"),
            workspace_file: PathBuf::from("/tmp/mandatum/.mandatum/workspace.json"),
            shell_program: "/bin/sh".to_owned(),
            task_command: "printf TASK_OK".to_owned(),
            spawn_pty: false,
            restore_on_startup: false,
        })
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL)
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
                workspace_name: "Mandatum".to_owned(),
                project_path,
                workspace_file: self.workspace_file(),
                shell_program: "/bin/sh".to_owned(),
                task_command: "printf TASK_OK".to_owned(),
                spawn_pty,
                restore_on_startup,
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
        assert_eq!(key_to_input(ctrl('q'), false), RuntimeInput::Quit);
        assert_eq!(key_to_input(ctrl('p'), false), RuntimeInput::TogglePalette);
        assert_eq!(
            key_to_input(key(KeyCode::Char('v')), true),
            RuntimeInput::Dispatch(CommandId::SplitRight)
        );
        assert_eq!(
            key_to_input(key(KeyCode::Tab), true),
            RuntimeInput::Dispatch(CommandId::FocusNext)
        );
        assert_eq!(
            key_to_input(key(KeyCode::Char('r')), true),
            RuntimeInput::Dispatch(CommandId::RestartPane)
        );
        assert_eq!(
            key_to_input_with_palette_context(
                key(KeyCode::Char('r')),
                true,
                PaletteContext::focused_task(),
            ),
            RuntimeInput::Dispatch(CommandId::RerunTask)
        );
        assert_eq!(
            key_to_input_with_palette_context(
                key(KeyCode::Char('c')),
                true,
                PaletteContext::focused_task(),
            ),
            RuntimeInput::Dispatch(CommandId::StopTask)
        );
    }

    #[test]
    fn normal_keys_are_terminal_input_when_palette_is_closed() {
        assert_eq!(
            key_to_input(key(KeyCode::Char('q')), false),
            RuntimeInput::SendToTerminal(b"q".to_vec())
        );
        assert_eq!(
            key_to_input(key(KeyCode::Enter), false),
            RuntimeInput::SendToTerminal(b"\r".to_vec())
        );
        assert_eq!(
            key_to_input(ctrl('c'), false),
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
        assert!(state.status().contains("Focus Previous"));
    }

    #[test]
    fn palette_opens_and_closes_without_mutating_layout() {
        let mut state = state();

        state.handle_key(ctrl('p'));
        assert!(state.palette_open());
        assert_eq!(state.workspace().active_session().panes().len(), 1);

        state.handle_key(key(KeyCode::Esc));
        assert!(!state.palette_open());
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

        state.handle_event(Event::Resize(100, 35));

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
            shell_program: "/bin/sh".to_owned(),
            task_command: "printf TASK_OK".to_owned(),
            spawn_pty: false,
            restore_on_startup: false,
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

        state.handle_event(Event::Resize(100, 35));
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
            workspace_name: "Mandatum".to_owned(),
            project_path: PathBuf::from("/tmp/mandatum"),
            workspace_file: PathBuf::from("/tmp/mandatum/.mandatum/workspace.json"),
            shell_program: "/bin/sh".to_owned(),
            task_command: "printf TASK_OK".to_owned(),
            spawn_pty: true,
            restore_on_startup: false,
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
        assert!(state.pending_task_launches.contains(&pane_id));
        assert_eq!(
            state.task_statuses.get(&pane_id).map(String::as_str),
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
        assert!(!state.pending_task_launches.contains(&pane_id));
        assert!(!state.task_statuses.contains_key(&pane_id));

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
                .task_statuses
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
        assert!(state.pending_task_launches.contains(&pane_id));
        assert_eq!(
            state.task_statuses.get(&pane_id).map(String::as_str),
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
            state.task_statuses.get(&pane_id).map(String::as_str),
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
        assert!(!state.pending_task_launches.contains(&pane_id));
        assert!(!state.task_statuses.contains_key(&pane_id));
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
        assert!(!state.pending_task_launches.contains(&pane_id));

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
            state.task_statuses.get(&pane_id).map(String::as_str),
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
            state.task_statuses.get(&pane_id).map(String::as_str),
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
        assert!(state.pending_task_launches.contains(&pane_id));

        state.dispatch(CommandId::StopTask);

        assert!(!state.pending_task_launches.contains(&pane_id));
        assert_eq!(
            state.task_statuses.get(&pane_id).map(String::as_str),
            Some("stopped before launch")
        );

        state.dispatch(CommandId::ZoomPane);
        for _ in 0..30 {
            state.tick_runtime();
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(state.live_task_count(), 0);
        assert_eq!(
            state.task_statuses.get(&pane_id).map(String::as_str),
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
}
