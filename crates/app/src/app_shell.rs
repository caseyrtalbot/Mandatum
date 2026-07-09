use std::{
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mandatum_commands::CommandError;
use mandatum_renderer::{
    PaletteView, RenderState, RuntimePaneViews, TaskRuntimeView, TerminalGridView,
    render_with_runtime_views,
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

use crate::app_state::AppState;

const POLL_INTERVAL: Duration = Duration::from_millis(40);

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
