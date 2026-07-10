use std::{
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
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
use mandatum_scene::{SceneSize, Theme};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

use crate::{
    app_state::AppState,
    config::{load_config, project_config_file, user_config_file},
    events::AppEvent,
    frontend::translate_event,
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
    let mut app = AppState::new(config);
    let mut terminal = TerminalGuard::enter()?;
    let size = terminal.size()?;
    app.handle_terminal_resize(size.width, size.height);

    let input_thread = InputThread::spawn(app.event_sender());
    let result = event_loop(&mut terminal, &mut app);
    // Stop the input thread before restoring the host terminal so it cannot
    // consume keystrokes that belong to the parent shell after exit.
    input_thread.stop_and_join();
    terminal.restore()?;
    result
}

/// The event-driven main loop: draw, then block until input or runtime
/// activity arrives (or the heartbeat elapses). No fixed-interval polling —
/// a keystroke wakes the loop the moment the input thread forwards it.
fn event_loop(terminal: &mut TerminalGuard, app: &mut AppState) -> Result<(), AppError> {
    let mut last_child_poll = Instant::now();
    app.tick_runtime();

    while !app.should_quit() {
        draw(terminal, app)?;
        let last_draw = Instant::now();

        if let Some(payload) = app.take_clipboard_payload() {
            write_clipboard_payload(&payload)?;
        }

        if app.wait_event(HEARTBEAT) {
            // Burst drain for drag/flood responsiveness, then coalesce
            // further arrivals until the redraw window opens so a flood
            // repaints at the cap instead of per event. Blocking between
            // arrivals keeps this idle-free (no busy spin).
            app.drain_events();
            loop {
                let elapsed = last_draw.elapsed();
                if app.should_quit() || elapsed >= MIN_REDRAW_INTERVAL {
                    break;
                }
                if !app.wait_event(MIN_REDRAW_INTERVAL - elapsed) {
                    break;
                }
                app.drain_events();
            }
        }

        // Child exits surface on the heartbeat cadence, not per event.
        if last_child_poll.elapsed() >= HEARTBEAT {
            app.poll_child_exits();
            last_child_poll = Instant::now();
        }
    }

    app.shutdown();
    draw(terminal, app)?;
    Ok(())
}

/// Dedicated crossterm input reader. Lives in the frontend layer: it
/// translates every event to neutral `mandatum_scene::input` values before
/// forwarding, so nothing past this thread names crossterm ([L1-GATE]).
struct InputThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl InputThread {
    fn spawn(tx: Sender<AppEvent>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match event::poll(INPUT_STOP_CHECK) {
                    Ok(false) => {}
                    Ok(true) => match event::read() {
                        Ok(raw) => {
                            if let Some(input) = translate_event(raw)
                                && tx.send(AppEvent::Input(input)).is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => break,
                    },
                    Err(_) => break,
                }
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop_and_join(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
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

/// Which agent connector backend the app launches sessions through.
///
/// `Claude` is the product default; tests wire `Fake` everywhere so no test
/// touches a network or a live agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentConnectorKind {
    Fake,
    Claude,
}

impl AppConfig {
    pub fn from_current_dir() -> io::Result<Self> {
        let project_path = std::env::current_dir()?;
        let user_config_file = user_config_file();
        let loaded = load_config(
            user_config_file.as_deref(),
            &project_config_file(&project_path),
        );
        Ok(Self {
            workspace_name: "Mandatum".to_owned(),
            workspace_file: default_workspace_file(&project_path),
            project_path,
            shell_program: loaded.shell_program.unwrap_or_else(default_shell_program),
            task_command: loaded.task_command.unwrap_or_else(default_task_command),
            agent_connector: loaded.agent_connector.unwrap_or(AgentConnectorKind::Claude),
            agent_objective: default_agent_objective(),
            agent_model: loaded.agent_model.or_else(default_agent_model),
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

fn draw(terminal: &mut TerminalGuard, app: &mut AppState) -> io::Result<()> {
    terminal.terminal.draw(|frame| {
        let area = frame.area();
        // `build_scene` retains the frame's hit targets, so pointer events
        // resolve against exactly what is on screen.
        let scene = app.build_scene(SceneSize::new(area.width, area.height));
        render(frame, &scene, app.theme());
    })?;
    Ok(())
}

fn write_clipboard_payload(payload: &[u8]) -> io::Result<()> {
    // OSC 52 is processed by the host terminal regardless of the alternate
    // screen, so writing it straight to stdout does not disturb the rendered UI.
    let mut stdout = io::stdout();
    stdout.write_all(payload)?;
    stdout.flush()
}

fn default_shell_program() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
}

fn default_task_command() -> String {
    "cargo test".to_owned()
}

fn default_agent_objective() -> String {
    "summarize the state of this project and propose the next step".to_owned()
}

/// Model hint from `MANDATUM_AGENT_MODEL`, read once here so env access
/// stays at the config boundary (mirrors `SHELL` for `shell_program`).
fn default_agent_model() -> Option<String> {
    std::env::var("MANDATUM_AGENT_MODEL")
        .ok()
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty())
}
