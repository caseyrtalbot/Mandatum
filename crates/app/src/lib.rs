//! Terminal application runtime for Mandatum.
//!
//! The runtime owns terminal lifecycle, live PTY handles, parser instances, and
//! input orchestration. Product mutations still go through `mandatum-commands`,
//! and drawing still goes through `mandatum-renderer`.

mod clipboard;
mod copy_mode;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{self, Write},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
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
    RuntimeCommand, command_target, dispatch_command,
};
use mandatum_core::{ActionOutcome, PaneId, PaneKind, PersistenceRequest, Workspace};
use mandatum_pty::{
    ChildExitStatus, NativePtyController, NativePtyError, NativePtyReader, NativePtySession,
    NativePtyWriter, PtyEvent, PtySessionId, PtySize, ResizeIntent, SpawnIntent,
};
use mandatum_renderer::{
    PaletteItem, PaletteView, PaneTerminalGrid, RenderState, SelectionPoint, TerminalGridView,
    TerminalViewport, pane_content_area, render_with_terminal_grids,
};
use mandatum_terminal_vt::{TerminalAdapterError, TerminalGrid, TerminalParser, TerminalSize};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

use crate::{clipboard::osc52_sequence, copy_mode::CopyModeState};

const POLL_INTERVAL: Duration = Duration::from_millis(40);
const PTY_READ_CHUNK_BYTES: usize = 8192;

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
    pub shell_program: String,
    pub spawn_pty: bool,
}

impl AppConfig {
    pub fn from_current_dir() -> io::Result<Self> {
        Ok(Self {
            workspace_name: "Mandatum".to_owned(),
            project_path: std::env::current_dir()?,
            shell_program: default_shell_program(),
            spawn_pty: true,
        })
    }
}

pub struct AppState {
    workspace: Workspace,
    command_context: CommandContext,
    shell_program: String,
    spawn_pty: bool,
    palette_open: bool,
    should_quit: bool,
    terminal_size: Option<(u16, u16)>,
    status: String,
    last_redraw: Instant,
    terminal_panes: BTreeMap<PaneId, TerminalPaneRuntime>,
    copy_mode: Option<CopyModeState>,
    clipboard_payload: Option<Vec<u8>>,
    last_copied: Option<String>,
    runtime_tx: Sender<PtyRuntimeEvent>,
    runtime_rx: Receiver<PtyRuntimeEvent>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let command_context =
            CommandContext::for_project(config.workspace_name.clone(), config.project_path.clone());
        let workspace = Workspace::new(config.workspace_name, config.project_path);
        let (runtime_tx, runtime_rx) = mpsc::channel();

        Self {
            workspace,
            command_context,
            shell_program: config.shell_program,
            spawn_pty: config.spawn_pty,
            palette_open: false,
            should_quit: false,
            terminal_size: None,
            status: "ready".to_owned(),
            last_redraw: Instant::now(),
            terminal_panes: BTreeMap::new(),
            copy_mode: None,
            clipboard_payload: None,
            last_copied: None,
            runtime_tx,
            runtime_rx,
        }
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

    pub fn live_terminal_count(&self) -> usize {
        self.terminal_panes.len()
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
        self.reconcile_terminal_runtimes();
        self.status = format!("terminal resized to {columns}x{rows}");
        self.mark_redraw();
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.copy_mode.is_some() {
            self.handle_copy_mode_key(key);
            self.mark_redraw();
            return;
        }

        match key_to_input(key, self.palette_open) {
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
        if let CommandTarget::Runtime(runtime_command) = command_target(command_id) {
            self.dispatch_runtime_command(runtime_command);
            return;
        }

        match dispatch_command(&mut self.workspace, &self.command_context, command_id) {
            Ok(outcome) => {
                self.status = status_for_outcome(command_id, outcome);
                self.reconcile_terminal_runtimes();
            }
            Err(error) => {
                self.status = format!("command failed: {error}");
            }
        }
    }

    fn dispatch_runtime_command(&mut self, runtime_command: RuntimeCommand) {
        match runtime_command {
            RuntimeCommand::EnterCopyMode => self.enter_copy_mode(),
        }
    }

    pub fn tick_runtime(&mut self) {
        self.drain_runtime_events();
        self.poll_child_exits();
    }

    pub fn shutdown(&mut self) {
        for pane in self.terminal_panes.values_mut() {
            pane.shutdown();
        }
        self.terminal_panes.clear();
        self.status = "terminal sessions stopped".to_owned();
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

    fn reconcile_terminal_runtimes(&mut self) {
        if !self.spawn_pty {
            return;
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
                self.restart_terminal_pane(pane_id, size);
            } else if let Some(runtime) = self.terminal_panes.get_mut(&pane_id) {
                if let Err(error) = runtime.resize(size) {
                    runtime.error = Some(error.to_string());
                    self.status = format!("PTY resize failed for {pane_id}: {error}");
                }
            } else if let Err(error) = self.spawn_terminal_pane(pane_id.clone(), size) {
                self.status = format!("PTY spawn failed for {pane_id}: {error}");
            }
        }
    }

    /// Tear down a pane's live PTY/parser/runtime and launch a fresh one for the
    /// same `PaneId`. Core layout intent (the durable `PaneId` and its restart
    /// generation) is preserved; no runtime handles are serialized.
    fn restart_terminal_pane(&mut self, pane_id: PaneId, size: PtySize) {
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
        match self.spawn_terminal_pane(pane_id.clone(), size) {
            Ok(()) => self.status = format!("restarted shell for {pane_id}"),
            Err(error) => self.status = format!("PTY restart failed for {pane_id}: {error}"),
        }
    }

    fn pane_restart_generation(&self, pane_id: &PaneId) -> u64 {
        self.workspace
            .active_session()
            .pane(pane_id)
            .map(|pane| pane.restart_generation())
            .unwrap_or(0)
    }

    fn visible_terminal_pane_sizes(&self) -> Vec<(PaneId, PtySize)> {
        let Some((columns, rows)) = self.terminal_size else {
            return Vec::new();
        };
        let area = Rect::new(0, 0, columns, rows);
        let session = self.workspace.active_session();

        session
            .panes()
            .iter()
            .filter_map(|(pane_id, pane)| {
                if !matches!(pane.kind(), PaneKind::Terminal { .. }) {
                    return None;
                }

                let content_area = pane_content_area(&self.workspace, area, pane_id)?;
                let size =
                    PtySize::new(content_area.width.max(1), content_area.height.max(1)).ok()?;
                Some((pane_id.clone(), size))
            })
            .collect()
    }

    fn terminal_pane_ids(&self) -> BTreeSet<PaneId> {
        self.workspace
            .active_session()
            .panes()
            .iter()
            .filter(|(_, pane)| matches!(pane.kind(), PaneKind::Terminal { .. }))
            .map(|(pane_id, _)| pane_id.clone())
            .collect()
    }

    fn spawn_terminal_pane(
        &mut self,
        pane_id: PaneId,
        size: PtySize,
    ) -> Result<(), TerminalRuntimeError> {
        let session = self.workspace.active_session();
        let pane = session
            .pane(&pane_id)
            .ok_or_else(|| TerminalRuntimeError::MissingPane(pane_id.clone()))?;
        let session_id = PtySessionId::new(pane_id.as_str().to_owned());
        let restart_generation = pane.restart_generation();
        let mut intent = SpawnIntent::new(session_id.clone(), self.shell_program.clone(), size)?;
        if let Some(cwd) = pane.cwd() {
            intent = intent.with_cwd(cwd.clone());
        }
        // The hardened parser handles real VT output, so advertise a capable
        // terminal. The rest of the environment (PATH, HOME, prompt) is inherited.
        intent = intent.with_environment([("TERM", "xterm-256color")]);

        let session = NativePtySession::spawn(intent)?;
        let parts = session.into_split()?;
        let reader_thread =
            spawn_reader_thread(pane_id.clone(), parts.reader, self.runtime_tx.clone());
        let parser = TerminalParser::new(to_terminal_size(size));

        self.terminal_panes.insert(
            pane_id.clone(),
            TerminalPaneRuntime {
                parser,
                controller: parts.controller,
                writer: parts.writer,
                reader_thread: Some(reader_thread),
                size,
                restart_generation,
                exit_status: None,
                error: None,
            },
        );
        self.status = format!("spawned shell for {pane_id}");
        Ok(())
    }

    fn drain_runtime_events(&mut self) {
        while let Ok(event) = self.runtime_rx.try_recv() {
            match event {
                PtyRuntimeEvent::Output { pane_id, bytes } => {
                    let Some(runtime) = self.terminal_panes.get_mut(&pane_id) else {
                        continue;
                    };
                    match runtime.parser.feed_pty_bytes(&bytes) {
                        Ok(_) => {
                            self.status = format!("read {} byte(s) from {pane_id}", bytes.len());
                        }
                        Err(error) => {
                            runtime.error = Some(error.to_string());
                            self.status = format!("terminal parser failed for {pane_id}: {error}");
                        }
                    }
                }
                PtyRuntimeEvent::ReaderClosed { pane_id } => {
                    self.status = format!("PTY reader closed for {pane_id}");
                }
                PtyRuntimeEvent::Error { pane_id, message } => {
                    if let Some(runtime) = self.terminal_panes.get_mut(&pane_id) {
                        runtime.error = Some(message.clone());
                    }
                    self.status = format!("PTY reader failed for {pane_id}: {message}");
                }
            }
        }
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
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        return RuntimeInput::Quit;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
        return RuntimeInput::TogglePalette;
    }

    if palette_open {
        return key_to_palette_input(key);
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

fn key_to_palette_input(key: KeyEvent) -> RuntimeInput {
    match key.code {
        KeyCode::Esc => RuntimeInput::ClosePalette,
        KeyCode::Char('q') => RuntimeInput::Quit,
        KeyCode::Char('n') => RuntimeInput::Dispatch(CommandId::NewTerminal),
        KeyCode::Char('v') => RuntimeInput::Dispatch(CommandId::SplitRight),
        KeyCode::Char('s') => RuntimeInput::Dispatch(CommandId::SplitDown),
        KeyCode::Char('h') => RuntimeInput::Dispatch(CommandId::FocusPrevious),
        KeyCode::BackTab => RuntimeInput::Dispatch(CommandId::FocusPrevious),
        KeyCode::Char('l') | KeyCode::Tab => RuntimeInput::Dispatch(CommandId::FocusNext),
        KeyCode::Char('x') => RuntimeInput::Dispatch(CommandId::ClosePane),
        KeyCode::Char('z') => RuntimeInput::Dispatch(CommandId::ZoomPane),
        KeyCode::Char('f') => RuntimeInput::Dispatch(CommandId::FloatPane),
        KeyCode::Char('t') => RuntimeInput::Dispatch(CommandId::StackPanes),
        KeyCode::Char('r') => RuntimeInput::Dispatch(CommandId::RestartPane),
        KeyCode::Char('[') => RuntimeInput::Dispatch(CommandId::EnterCopyMode),
        _ => RuntimeInput::Noop,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PtyRuntimeEvent {
    Output { pane_id: PaneId, bytes: Vec<u8> },
    ReaderClosed { pane_id: PaneId },
    Error { pane_id: PaneId, message: String },
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
    SpawnIntent(mandatum_pty::SpawnIntentError),
    NativePty(NativePtyError),
}

impl fmt::Display for TerminalRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPane(pane_id) => write!(formatter, "pane {pane_id} was not found"),
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

impl From<TerminalAdapterError> for TerminalRuntimeError {
    fn from(error: TerminalAdapterError) -> Self {
        Self::NativePty(NativePtyError::ReadFailed {
            session_id: PtySessionId::new("terminal-parser"),
            message: error.to_string(),
        })
    }
}

fn draw(terminal: &mut TerminalGuard, app: &AppState) -> io::Result<()> {
    let palette_items = app.palette_items();
    let terminal_grid_items = app.terminal_grid_items();
    terminal.terminal.draw(|frame| {
        render_with_terminal_grids(
            frame,
            RenderState {
                workspace: app.workspace(),
                palette: PaletteView {
                    open: app.palette_open(),
                    items: &palette_items,
                },
                status: Some(app.status()),
            },
            TerminalGridView::new(&terminal_grid_items),
        );
    })?;
    Ok(())
}

fn spawn_reader_thread(
    pane_id: PaneId,
    mut reader: NativePtyReader,
    tx: Sender<PtyRuntimeEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match reader.read_event(PTY_READ_CHUNK_BYTES) {
                Ok(Some(PtyEvent::Output(output))) => {
                    let _ = tx.send(PtyRuntimeEvent::Output {
                        pane_id: pane_id.clone(),
                        bytes: output.into_bytes(),
                    });
                }
                Ok(Some(PtyEvent::ChildExited(_))) | Ok(Some(PtyEvent::BackpressureChanged(_))) => {
                }
                Ok(None) => {
                    let _ = tx.send(PtyRuntimeEvent::ReaderClosed { pane_id });
                    break;
                }
                Err(error) => {
                    let _ = tx.send(PtyRuntimeEvent::Error {
                        pane_id,
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
        ActionOutcome::PersistenceRequested(PersistenceRequest::SaveWorkspace) => {
            "save requested".to_owned()
        }
        ActionOutcome::PersistenceRequested(PersistenceRequest::RestoreWorkspace) => {
            "restore requested".to_owned()
        }
    }
}

fn category_label(category: CommandCategory) -> &'static str {
    match category {
        CommandCategory::Project => "project",
        CommandCategory::Pane => "pane",
        CommandCategory::Layout => "layout",
        CommandCategory::Persistence => "persistence",
    }
}

fn default_shell_program() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new(AppConfig {
            workspace_name: "Mandatum".to_owned(),
            project_path: PathBuf::from("/tmp/mandatum"),
            shell_program: "/bin/sh".to_owned(),
            spawn_pty: false,
        })
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL)
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
            shell_program: "/bin/sh".to_owned(),
            spawn_pty: true,
        })
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
}
