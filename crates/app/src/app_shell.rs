use std::{
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mandatum_commands::CommandError;
use mandatum_renderer::render;
use mandatum_scene::{SceneSize, Theme, input::InputEvent};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

use crate::{
    clipboard::osc52_sequence,
    config::{
        AgentConnectorKind, default_task_command, effective_runtime_settings, load_config,
        project_config_file, user_config_file,
    },
    events::{AppEvent, AppEventSender},
    frontend::translate_event,
    frontend_effect::FrontendEffect,
    frontend_host::FrontendHost,
    keymap::Keymap,
};

/// Heartbeat when nothing arrives: child-exit polling and clock-driven UI.
/// It is not an input latency floor — input wakes the loop immediately.
const HEARTBEAT: Duration = Duration::from_millis(250);
/// Redraw cap (~120 fps): under an event flood the loop keeps absorbing
/// events and repaints at most once per interval instead of once per event.
const MIN_REDRAW_INTERVAL: Duration = Duration::from_millis(8);
/// How often the input thread checks its stop flag. `event::poll` returns
/// the instant an event arrives, so this bounds only shutdown latency.
const INPUT_STOP_CHECK: Duration = Duration::from_millis(100);

pub fn run() -> Result<(), AppError> {
    run_with_config(AppConfig::from_current_dir()?)
}

pub fn run_with_config(config: AppConfig) -> Result<(), AppError> {
    let mut host = FrontendHost::new(config);
    let mut terminal = TerminalGuard::enter()?;
    let size = terminal.size()?;
    host.handle_input(InputEvent::Resize(SceneSize::new(size.width, size.height)));

    let input_thread = InputThread::spawn(host.event_sender());
    let result = event_loop(&mut terminal, &mut host, &input_thread);
    finalize_run(
        result,
        || {
            host.shutdown();
        },
        || input_thread.stop_and_join(),
        || terminal.restore(),
    )
}

/// Complete the terminal lifecycle in one testable ordering seam. The normal
/// quit path already shut runtimes down inside `event_loop`; an early error did
/// not, so it must close live work first. The reader always stops before host
/// restoration, and a restoration failure never hides the primary error.
fn finalize_run(
    result: Result<(), AppError>,
    shutdown_runtimes: impl FnOnce(),
    stop_input: impl FnOnce(),
    restore_terminal: impl FnOnce() -> io::Result<()>,
) -> Result<(), AppError> {
    if result.is_err() {
        shutdown_runtimes();
    }
    stop_input();
    let restore_result = restore_terminal().map_err(AppError::Io);

    match result {
        Err(error) => {
            let _ = restore_result;
            Err(error)
        }
        Ok(()) => restore_result,
    }
}

/// The event-driven main loop: draw, then block until input or runtime
/// activity arrives (or the heartbeat elapses). No fixed-interval polling —
/// a keystroke wakes the loop the moment the input thread forwards it.
fn event_loop(
    terminal: &mut TerminalGuard,
    host: &mut FrontendHost,
    input_thread: &InputThread,
) -> Result<(), AppError> {
    let mut last_child_poll = Instant::now();
    host.drain_runtime();
    host.heartbeat();

    while !host.should_quit() {
        input_thread.propagate_outcome()?;
        draw(terminal, host)?;
        let last_draw = Instant::now();

        let frontend_effects = host.take_effects();
        if !frontend_effects.is_empty() {
            let mut stdout = io::stdout();
            for effect in frontend_effects {
                write_frontend_effect(&mut stdout, effect)?;
            }
        }

        if host.wait_event(HEARTBEAT) {
            // Burst drain for drag/flood responsiveness, then coalesce
            // further arrivals until the redraw window opens so a flood
            // repaints at the cap instead of per event. Blocking between
            // arrivals keeps this idle-free (no busy spin).
            host.drain_runtime();
            loop {
                let elapsed = last_draw.elapsed();
                if host.should_quit() || elapsed >= MIN_REDRAW_INTERVAL {
                    break;
                }
                if !host.wait_event(MIN_REDRAW_INTERVAL - elapsed) {
                    break;
                }
                host.drain_runtime();
            }
        }

        // Child exits surface on the heartbeat cadence, not per event.
        if last_child_poll.elapsed() >= HEARTBEAT {
            host.heartbeat();
            last_child_poll = Instant::now();
        }
        input_thread.propagate_outcome()?;
    }

    host.shutdown();
    draw(terminal, host)?;
    Ok(())
}

/// Dedicated crossterm input reader. Lives in the frontend layer: it
/// translates every event to neutral `mandatum_scene::input` values before
/// forwarding, so nothing past this thread names crossterm ([L1-GATE]).
struct InputThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    outcome_rx: Receiver<InputThreadOutcome>,
}

#[derive(Debug)]
enum InputThreadOutcome {
    Stopped,
    Failed {
        operation: &'static str,
        source: io::Error,
    },
}

impl InputThread {
    fn spawn(tx: AppEventSender) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let (outcome_tx, outcome_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let outcome = loop {
                if stop_flag.load(Ordering::Relaxed) {
                    break InputThreadOutcome::Stopped;
                }
                match event::poll(INPUT_STOP_CHECK) {
                    Ok(false) => {}
                    Ok(true) => match event::read() {
                        Ok(raw) => {
                            if let Some(input) = translate_event(raw)
                                && tx.send(AppEvent::Input(input)).is_err()
                            {
                                break InputThreadOutcome::Stopped;
                            }
                        }
                        Err(source) => {
                            break InputThreadOutcome::Failed {
                                operation: "read",
                                source,
                            };
                        }
                    },
                    Err(source) => {
                        break InputThreadOutcome::Failed {
                            operation: "poll",
                            source,
                        };
                    }
                }
            };
            let _ = outcome_tx.send(outcome);
        });
        Self {
            stop,
            handle: Some(handle),
            outcome_rx,
        }
    }

    fn propagate_outcome(&self) -> Result<(), AppError> {
        propagate_input_thread_outcome(self.outcome_rx.try_recv())
    }

    fn stop_and_join(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn propagate_input_thread_outcome(
    outcome: Result<InputThreadOutcome, TryRecvError>,
) -> Result<(), AppError> {
    match outcome {
        Ok(InputThreadOutcome::Stopped) | Err(TryRecvError::Empty) => Ok(()),
        Ok(InputThreadOutcome::Failed { operation, source }) => {
            Err(AppError::FrontendInput { operation, source })
        }
        Err(TryRecvError::Disconnected) => Err(AppError::FrontendInput {
            operation: "thread",
            source: io::Error::new(
                io::ErrorKind::BrokenPipe,
                "input reader stopped without reporting an outcome",
            ),
        }),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    pub workspace_name: String,
    pub project_path: PathBuf,
    pub workspace_file: PathBuf,
    pub shell_program: String,
    pub task_command: String,
    pub agent_connector: AgentConnectorKind,
    pub agent_objective: String,
    /// Optional model hint passed through to agent launches (`None` lets the
    /// connector use its account default).
    pub agent_model: Option<String>,
    pub spawn_pty: bool,
    pub restore_on_startup: bool,
    pub keymap: Keymap,
    pub theme: Theme,
    pub reduced_motion: bool,
    /// Surface byte-level PTY diagnostics in the status line (`[ui]
    /// debug_status`). Off by default: diagnostics are noise that would
    /// overwrite meaningful status on every read.
    pub debug_status: bool,
    /// Validation problems from config loading, surfaced as a startup
    /// status line. A broken config never prevents launch.
    pub config_warnings: Vec<String>,
    /// The user-level config file consulted by Reload Config; `None` skips
    /// the user layer (tests).
    pub user_config_file: Option<PathBuf>,
}

impl Default for AppConfig {
    /// Test-friendly baseline: fake connector, no PTY spawning, no restore,
    /// default keymap and theme. The product path is `from_current_dir`.
    fn default() -> Self {
        Self {
            workspace_name: "Mandatum".to_owned(),
            project_path: PathBuf::new(),
            workspace_file: PathBuf::new(),
            shell_program: "/bin/sh".to_owned(),
            task_command: default_task_command(),
            agent_connector: AgentConnectorKind::Fake,
            agent_objective: default_agent_objective(),
            agent_model: None,
            spawn_pty: false,
            restore_on_startup: false,
            keymap: Keymap::default(),
            theme: Theme::default(),
            reduced_motion: false,
            debug_status: false,
            config_warnings: Vec::new(),
            user_config_file: None,
        }
    }
}

impl AppConfig {
    pub fn from_current_dir() -> io::Result<Self> {
        let project_path = std::env::current_dir()?;
        let user_config_file = user_config_file();
        let loaded = load_config(
            user_config_file.as_deref(),
            &project_config_file(&project_path),
        );
        let runtime = effective_runtime_settings(&loaded);
        Ok(Self {
            workspace_name: "Mandatum".to_owned(),
            workspace_file: default_workspace_file(&project_path),
            project_path,
            shell_program: runtime.shell_program,
            task_command: runtime.task_command,
            agent_connector: runtime.agent_connector,
            agent_objective: default_agent_objective(),
            agent_model: runtime.agent_model,
            spawn_pty: true,
            restore_on_startup: true,
            keymap: loaded.keymap,
            theme: loaded.theme,
            reduced_motion: loaded.reduced_motion,
            debug_status: loaded.debug_status,
            config_warnings: loaded.warnings,
            user_config_file,
        })
    }
}

pub fn default_workspace_file(project_path: &Path) -> PathBuf {
    project_path.join(".mandatum").join("workspace.json")
}

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();

        // Host-level mouse capture only makes pointer events visible to the
        // workspace; children that request the mouse get them forwarded as
        // PTY bytes instead of workspace handling (L5, `app_state` routing).
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(mut terminal) => {
                if let Err(error) = terminal.clear() {
                    let _ = disable_raw_mode();
                    let _ = execute!(
                        terminal.backend_mut(),
                        DisableMouseCapture,
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
                let _ = execute!(
                    stdout,
                    DisableMouseCapture,
                    DisableBracketedPaste,
                    LeaveAlternateScreen
                );
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
            DisableMouseCapture,
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

#[derive(Debug)]
pub enum AppError {
    Io(io::Error),
    Command(CommandError),
    FrontendInput {
        operation: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Command(error) => write!(formatter, "{error}"),
            Self::FrontendInput { operation, source } => {
                write!(formatter, "frontend input {operation} failed: {source}")
            }
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) | Self::FrontendInput { source: error, .. } => Some(error),
            Self::Command(error) => Some(error),
        }
    }
}

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

fn draw(terminal: &mut TerminalGuard, host: &mut FrontendHost) -> io::Result<()> {
    terminal.terminal.draw(|frame| {
        let area = frame.area();
        // `FrontendHost::frame` retains this snapshot's hit targets, so
        // pointer events resolve against exactly what is painted here.
        let snapshot = host.frame(SceneSize::new(area.width, area.height));
        render(frame, &snapshot.scene, &snapshot.theme);
    })?;
    Ok(())
}

fn write_frontend_effect(writer: &mut dyn Write, effect: FrontendEffect) -> io::Result<()> {
    match effect {
        FrontendEffect::SetClipboard(text) => {
            // OSC 52 is processed by the host terminal regardless of the
            // alternate screen, so writing it straight to stdout does not
            // disturb the rendered UI.
            writer.write_all(&osc52_sequence(&text))?;
        }
    }
    writer.flush()
}

fn default_agent_objective() -> String {
    "summarize the state of this project and propose the next step".to_owned()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn terminal_frontend_encodes_clipboard_effect_as_osc52() {
        let mut payload = Vec::new();
        write_frontend_effect(&mut payload, FrontendEffect::SetClipboard("hi".to_owned())).unwrap();

        assert!(payload.starts_with(b"\x1b]52;c;"));
        assert_eq!(payload.last(), Some(&0x07));
        assert!(String::from_utf8(payload).unwrap().contains("aGk="));
    }

    #[test]
    fn input_failure_shuts_runtimes_stops_reader_restores_and_keeps_primary_error() {
        let order = RefCell::new(Vec::new());
        let result = finalize_run(
            Err(AppError::FrontendInput {
                operation: "read",
                source: io::Error::other("primary input failure"),
            }),
            || order.borrow_mut().push("shutdown runtimes"),
            || order.borrow_mut().push("stop input"),
            || {
                order.borrow_mut().push("restore terminal");
                Err(io::Error::other("secondary restore failure"))
            },
        );

        assert_eq!(
            *order.borrow(),
            ["shutdown runtimes", "stop input", "restore terminal"]
        );
        assert!(matches!(
            result,
            Err(AppError::FrontendInput {
                operation: "read",
                source,
            }) if source.to_string() == "primary input failure"
        ));
    }

    #[test]
    fn normal_completion_stops_input_then_reports_restore_failure() {
        let order = RefCell::new(Vec::new());
        let result = finalize_run(
            Ok(()),
            || order.borrow_mut().push("unexpected shutdown"),
            || order.borrow_mut().push("stop input"),
            || {
                order.borrow_mut().push("restore terminal");
                Err(io::Error::other("restore failure"))
            },
        );

        assert_eq!(*order.borrow(), ["stop input", "restore terminal"]);
        assert!(
            matches!(result, Err(AppError::Io(source)) if source.to_string() == "restore failure")
        );
    }

    #[test]
    fn input_poll_failure_becomes_a_structured_app_error() {
        let error = propagate_input_thread_outcome(Ok(InputThreadOutcome::Failed {
            operation: "poll",
            source: io::Error::other("poll broke"),
        }))
        .expect_err("a failed input poll must stop the app");

        assert!(matches!(
            error,
            AppError::FrontendInput {
                operation: "poll",
                ..
            }
        ));
        assert_eq!(error.to_string(), "frontend input poll failed: poll broke");
    }

    #[test]
    fn input_read_failure_becomes_a_structured_app_error() {
        let error = propagate_input_thread_outcome(Ok(InputThreadOutcome::Failed {
            operation: "read",
            source: io::Error::other("read broke"),
        }))
        .expect_err("a failed input read must stop the app");

        assert!(matches!(
            error,
            AppError::FrontendInput {
                operation: "read",
                ..
            }
        ));
    }

    #[test]
    fn normal_input_stop_and_a_running_reader_are_not_failures() {
        propagate_input_thread_outcome(Ok(InputThreadOutcome::Stopped))
            .expect("a requested stop is normal shutdown");
        propagate_input_thread_outcome(Err(TryRecvError::Empty))
            .expect("no outcome means the reader is still running");
    }

    #[test]
    fn disappearing_input_reader_is_a_structured_failure() {
        let error = propagate_input_thread_outcome(Err(TryRecvError::Disconnected))
            .expect_err("an unexplained reader exit must stop the app");

        assert!(matches!(
            error,
            AppError::FrontendInput {
                operation: "thread",
                ..
            }
        ));
    }
}
