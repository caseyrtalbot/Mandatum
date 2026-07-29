use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use mandatum_core::{PaneId, PaneSpec, Workspace};
use mandatum_pty::{
    ChildExitStatus, NativePtyController, NativePtyError, NativePtyReader, NativePtySession,
    NativePtyWriter, PtySessionId, PtySize, ResizeIntent, SpawnIntent,
};
use mandatum_terminal_vt::{TerminalParser, TerminalSize};

use crate::{
    events::AppEventSender,
    process_events::{PtyFlowControl, spawn_reader_thread},
};

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
    /// The reader thread's backpressure gate; `stop()` before joining so a
    /// reader blocked on a full gate cannot deadlock shutdown.
    pub(crate) flow: Arc<PtyFlowControl>,
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
        let _ = self.stop();
    }

    pub(crate) fn stop(&mut self) -> Result<(), NativePtyError> {
        self.writer.close_input();
        let result = self.controller.kill();
        self.flow.stop();
        if let Some(handle) = self.reader_thread.take() {
            join_reader_with_deadline(handle);
        }
        reap_child_briefly(&mut self.controller);
        result
    }
}

/// How long shutdown waits for the reader thread before detaching it.
const READER_JOIN_DEADLINE: Duration = Duration::from_millis(250);

/// How long shutdown polls `try_wait` for a killed child before giving up.
const CHILD_REAP_DEADLINE: Duration = Duration::from_millis(100);

/// Poll interval for both bounded shutdown waits.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Join the reader thread, detaching it at the deadline. An unbounded join
/// hangs pane close whenever the reader's blocking read never returns — e.g.
/// a SIGHUP-immune grandchild keeping the slave fd alive. The reader exits at
/// EOF on its own once the fd chain finally closes, so detaching leaks
/// nothing.
fn join_reader_with_deadline(handle: JoinHandle<()>) {
    let deadline = Instant::now() + READER_JOIN_DEADLINE;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(SHUTDOWN_POLL_INTERVAL);
    }
    let _ = handle.join();
}

/// Reap a killed child so it does not linger as a zombie: portable-pty's
/// `kill` delivers the signal but never waits, and kill paths remove the
/// runtime from the registry before `poll_child_exits` can `try_wait` it.
/// Signal delivery is asynchronous, so poll briefly and give up on a child
/// that has not died yet rather than block the UI thread.
fn reap_child_briefly(controller: &mut NativePtyController) {
    let deadline = Instant::now() + CHILD_REAP_DEADLINE;
    loop {
        match controller.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return;
                }
                thread::sleep(SHUTDOWN_POLL_INTERVAL);
            }
        }
    }
}

pub(crate) struct PendingTerminalPaneRuntime {
    pub(crate) reader: NativePtyReader,
    pub(crate) controller: NativePtyController,
    pub(crate) writer: NativePtyWriter,
    pub(crate) size: PtySize,
    pub(crate) restart_generation: u64,
    pub(crate) runtime_token: u64,
    /// Set when the pane's own cwd was gone and the shell opened elsewhere.
    pub(crate) cwd_fallback: Option<CwdFallback>,
}

/// A pane that opened somewhere other than its resolved cwd, because that
/// directory no longer exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CwdFallback {
    pub(crate) requested: PathBuf,
    pub(crate) used: PathBuf,
}

impl PendingTerminalPaneRuntime {
    pub(crate) fn activate(self, pane_id: PaneId, tx: AppEventSender) -> TerminalPaneRuntime {
        let Self {
            reader,
            controller,
            writer,
            size,
            restart_generation,
            runtime_token,
            // Already reported by the caller that staged this runtime.
            cwd_fallback: _,
        } = self;
        let flow = PtyFlowControl::new();
        let reader_thread = spawn_reader_thread(
            pane_id,
            restart_generation,
            runtime_token,
            reader,
            tx,
            Arc::clone(&flow),
        );
        let parser = TerminalParser::new(to_terminal_size(size));

        TerminalPaneRuntime {
            parser,
            controller,
            writer,
            reader_thread: Some(reader_thread),
            flow,
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
        reap_child_briefly(&mut self.controller);
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
    let requested_cwd = resolve_pane_cwd(workspace, pane, None);
    let mut intent = SpawnIntent::new(session_id, shell_program.to_owned(), size)?;
    intent = intent.with_cwd(requested_cwd.clone());
    // The hardened parser handles real VT output, so advertise a capable
    // terminal. The rest of the environment (PATH, HOME, prompt) is inherited.
    intent = intent.with_environment([("TERM", "xterm-256color")]);

    let (session, cwd_fallback) = spawn_with_cwd_fallback(intent, workspace, &requested_cwd)?;
    let parts = session.into_split()?;

    Ok(PendingTerminalPaneRuntime {
        reader: parts.reader,
        controller: parts.controller,
        writer: parts.writer,
        size,
        restart_generation,
        runtime_token,
        cwd_fallback,
    })
}

/// Spawn a terminal pane, degrading a dead cwd to a live one.
///
/// A pane's durable cwd outlives the directory itself (renames, deletes), and
/// one such pane must not take the reconcile pass down with it: reopen this
/// pane alone in the project directory, or `$HOME` when that is gone too.
/// Every other spawn failure still propagates.
fn spawn_with_cwd_fallback(
    intent: SpawnIntent,
    workspace: &Workspace,
    requested_cwd: &Path,
) -> Result<(NativePtySession, Option<CwdFallback>), TerminalRuntimeError> {
    let error = match NativePtySession::spawn(intent.clone()) {
        Ok(session) => return Ok((session, None)),
        Err(error) => error,
    };
    if !matches!(error, NativePtyError::CwdNotFound { .. }) {
        return Err(error.into());
    }
    let Some(fallback) = fallback_cwd(workspace) else {
        return Err(error.into());
    };

    let session = NativePtySession::spawn(intent.with_cwd(fallback.clone()))?;
    Ok((
        session,
        Some(CwdFallback {
            requested: requested_cwd.to_owned(),
            used: fallback,
        }),
    ))
}

/// The directory a pane with a dead cwd opens in instead: the active
/// project when it still exists, else the user's home. `None` means there is
/// nothing live to fall back to and the original failure stands.
fn fallback_cwd(workspace: &Workspace) -> Option<PathBuf> {
    let project_path = workspace.active_project_path();
    if project_path.is_dir() {
        return Some(project_path.to_owned());
    }

    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
}

/// The working directory a pane's process actually runs in: explicit intent
/// first, then the pane's durable cwd, then the active project's directory.
/// The resolved directory may be stale (durable state outlives renames), so
/// the spawn boundary rejects a missing one rather than letting portable-pty
/// silently substitute the user's `$HOME`.
pub(crate) fn resolve_pane_cwd(
    workspace: &Workspace,
    pane: &PaneSpec,
    intent_cwd: Option<&PathBuf>,
) -> PathBuf {
    intent_cwd
        .or_else(|| pane.cwd())
        .cloned()
        .unwrap_or_else(|| workspace.active_project_path().to_owned())
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use mandatum_pty::{NativePtySession, SpawnIntent};

    use super::*;

    fn spawn_pending(script: &str) -> PendingTerminalPaneRuntime {
        let size = PtySize::new(80, 24).expect("non-zero size");
        let intent = SpawnIntent::new(PtySessionId::new("pane-test"), "/bin/sh", size)
            .expect("valid intent")
            .with_arguments(["-c", script]);
        let session = NativePtySession::spawn(intent).expect("spawn test shell");
        let parts = session.into_split().expect("split session");

        PendingTerminalPaneRuntime {
            reader: parts.reader,
            controller: parts.controller,
            writer: parts.writer,
            size,
            restart_generation: 0,
            runtime_token: 0,
            cwd_fallback: None,
        }
    }

    /// Block until the pane's reader has forwarded output containing `needle`,
    /// so a test cannot race the shell script it is synchronizing with.
    fn wait_for_output(tx: &AppEventSender, rx: &crate::events::AppEventReceiver, needle: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "never saw {:?} in PTY output ({:?})",
                String::from_utf8_lossy(needle),
                String::from_utf8_lossy(&seen)
            );
            let Ok(event) = tx.recv_timeout(rx, Duration::from_millis(100)) else {
                continue;
            };
            if let crate::events::AppEvent::Pty(
                crate::process_events::PtyRuntimeEvent::Output { bytes, .. },
                _,
            ) = event
            {
                seen.extend(bytes);
                if seen.windows(needle.len()).any(|window| window == needle) {
                    return;
                }
            }
        }
    }

    // portable-pty's `kill` signals but never reaps, so without an explicit
    // wait in shutdown a SIGHUP-immune child killed on pane close lingers as
    // a zombie for the app's lifetime.
    #[test]
    fn shutdown_reaps_a_killed_sighup_immune_child() {
        let (tx, rx) = AppEventSender::channel();
        let pending = spawn_pending("trap '' HUP; printf ready; sleep 30");
        let mut runtime = pending.activate(PaneId::new("pane-reap"), tx.clone());
        let pid = runtime
            .controller
            .process_id()
            .expect("spawned child has a pid")
            .get();
        // Wait for the marker so the kill cannot race the shell's trap.
        wait_for_output(&tx, &rx, b"ready");

        runtime.shutdown();

        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("run ps");
        let stat = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stat.trim_start().starts_with('Z'),
            "child {pid} was left as a zombie (ps stat {stat:?})"
        );
    }

    // The shutdown join must be bounded: a reader whose blocking read never
    // returns (no EOF while something keeps the slave fd alive) is detached
    // at the deadline instead of hanging the UI thread.
    #[test]
    fn join_reader_with_deadline_detaches_a_reader_that_never_finishes() {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            let _ = release_rx.recv();
        });

        let started = Instant::now();
        join_reader_with_deadline(handle);

        assert!(
            started.elapsed() < READER_JOIN_DEADLINE + Duration::from_secs(1),
            "detach took {:?}; the join must be bounded",
            started.elapsed()
        );
        // Release the detached thread so it exits on its own.
        drop(release_tx);
    }

    #[test]
    fn join_reader_with_deadline_joins_a_finished_reader() {
        let handle = thread::spawn(|| {});
        // A finished thread joins immediately; only a stuck one costs the
        // deadline. No timing assertion needed — completion is the check.
        join_reader_with_deadline(handle);
    }
}
