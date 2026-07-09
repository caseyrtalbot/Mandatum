use std::{collections::BTreeMap, fmt, sync::mpsc::Sender, thread::JoinHandle};

use mandatum_core::{PaneId, Workspace};
use mandatum_pty::{
    ChildExitStatus, NativePtyController, NativePtyError, NativePtyReader, NativePtySession,
    NativePtyWriter, PtySessionId, PtySize, ResizeIntent, SpawnIntent,
};
use mandatum_terminal_vt::{TerminalParser, TerminalSize};

use crate::process_events::{PtyRuntimeEvent, spawn_reader_thread};

#[derive(Default)]
pub(crate) struct TerminalRuntimeRegistry {
    panes: BTreeMap<PaneId, TerminalPaneRuntime>,
}

impl TerminalRuntimeRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.panes.len()
    }

    pub(crate) fn get(&self, pane_id: &PaneId) -> Option<&TerminalPaneRuntime> {
        self.panes.get(pane_id)
    }

    pub(crate) fn get_mut(&mut self, pane_id: &PaneId) -> Option<&mut TerminalPaneRuntime> {
        self.panes.get_mut(pane_id)
    }

    pub(crate) fn contains_key(&self, pane_id: &PaneId) -> bool {
        self.panes.contains_key(pane_id)
    }

    pub(crate) fn insert(
        &mut self,
        pane_id: PaneId,
        runtime: TerminalPaneRuntime,
    ) -> Option<TerminalPaneRuntime> {
        self.panes.insert(pane_id, runtime)
    }

    pub(crate) fn remove(&mut self, pane_id: &PaneId) -> Option<TerminalPaneRuntime> {
        self.panes.remove(pane_id)
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &PaneId> {
        self.panes.keys()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&PaneId, &TerminalPaneRuntime)> {
        self.panes.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&PaneId, &mut TerminalPaneRuntime)> {
        self.panes.iter_mut()
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut TerminalPaneRuntime> {
        self.panes.values_mut()
    }

    pub(crate) fn clear(&mut self) {
        self.panes.clear();
    }

    pub(crate) fn shutdown_all(&mut self) {
        for pane in self.values_mut() {
            pane.shutdown();
        }
        self.clear();
    }
}

impl FromIterator<(PaneId, TerminalPaneRuntime)> for TerminalRuntimeRegistry {
    fn from_iter<T: IntoIterator<Item = (PaneId, TerminalPaneRuntime)>>(iter: T) -> Self {
        Self {
            panes: BTreeMap::from_iter(iter),
        }
    }
}

pub(crate) struct TerminalPaneRuntime {
    pub(crate) parser: TerminalParser,
    pub(crate) controller: NativePtyController,
    pub(crate) writer: NativePtyWriter,
    pub(crate) reader_thread: Option<JoinHandle<()>>,
    pub(crate) size: PtySize,
    pub(crate) restart_generation: u64,
    pub(crate) runtime_token: u64,
    pub(crate) exit_status: Option<ChildExitStatus>,
    pub(crate) error: Option<String>,
}

impl TerminalPaneRuntime {
    pub(crate) fn write_input(&mut self, bytes: &[u8]) -> Result<(), NativePtyError> {
        self.writer.write_input(bytes)
    }

    pub(crate) fn resize(&mut self, size: PtySize) -> Result<(), NativePtyError> {
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

    pub(crate) fn shutdown(&mut self) {
        self.writer.close_input();
        let _ = self.controller.kill();
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }

    pub(crate) fn stop(&mut self) -> Result<(), NativePtyError> {
        self.writer.close_input();
        let result = self.controller.kill();
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
        result
    }
}

pub(crate) struct PendingTerminalPaneRuntime {
    pub(crate) reader: NativePtyReader,
    pub(crate) controller: NativePtyController,
    pub(crate) writer: NativePtyWriter,
    pub(crate) size: PtySize,
    pub(crate) restart_generation: u64,
    pub(crate) runtime_token: u64,
}

impl PendingTerminalPaneRuntime {
    pub(crate) fn activate(
        self,
        pane_id: PaneId,
        tx: Sender<PtyRuntimeEvent>,
    ) -> TerminalPaneRuntime {
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

    pub(crate) fn shutdown(&mut self) {
        self.writer.close_input();
        let _ = self.controller.kill();
    }
}

#[derive(Debug)]
pub(crate) enum TerminalRuntimeError {
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

pub(crate) fn prepare_terminal_pane_runtime(
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

pub(crate) fn to_terminal_size(size: PtySize) -> TerminalSize {
    TerminalSize::new(size.columns(), size.rows()).expect("PTY sizes are non-zero")
}

pub(crate) fn exit_status_label(status: ChildExitStatus) -> String {
    match status {
        ChildExitStatus::Exited { code } => format!("exit {code}"),
        ChildExitStatus::Signaled { signal } => format!("signal {signal}"),
        ChildExitStatus::Unknown => "unknown".to_owned(),
    }
}
