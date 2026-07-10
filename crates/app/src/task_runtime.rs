use std::collections::{BTreeMap, BTreeSet};

use mandatum_core::{PaneId, PaneKind, Workspace};
use mandatum_pty::{
    ChildExitStatus, NativePtyError, NativePtySession, PtySessionId, PtySize, SpawnIntent,
};

use crate::terminal_runtime::{
    PendingTerminalPaneRuntime, TerminalPaneRuntime, TerminalRuntimeError,
};

#[derive(Default)]
pub(crate) struct TaskRuntimeRegistry {
    runtimes: BTreeMap<PaneId, TaskPaneRuntime>,
    pub(crate) pending_launches: BTreeSet<PaneId>,
    pub(crate) statuses: BTreeMap<PaneId, String>,
}

impl TaskRuntimeRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.runtimes.len()
    }

    pub(crate) fn get(&self, pane_id: &PaneId) -> Option<&TaskPaneRuntime> {
        self.runtimes.get(pane_id)
    }

    pub(crate) fn get_mut(&mut self, pane_id: &PaneId) -> Option<&mut TaskPaneRuntime> {
        self.runtimes.get_mut(pane_id)
    }

    pub(crate) fn contains_key(&self, pane_id: &PaneId) -> bool {
        self.runtimes.contains_key(pane_id)
    }

    pub(crate) fn insert(
        &mut self,
        pane_id: PaneId,
        runtime: TaskPaneRuntime,
    ) -> Option<TaskPaneRuntime> {
        self.runtimes.insert(pane_id, runtime)
    }

    pub(crate) fn remove(&mut self, pane_id: &PaneId) -> Option<TaskPaneRuntime> {
        self.runtimes.remove(pane_id)
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &PaneId> {
        self.runtimes.keys()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&PaneId, &mut TaskPaneRuntime)> {
        self.runtimes.iter_mut()
    }

    pub(crate) fn shutdown_all(&mut self) {
        for pane in self.runtimes.values_mut() {
            pane.shutdown();
        }
        self.runtimes.clear();
        self.pending_launches.clear();
        self.statuses.clear();
    }

    pub(crate) fn retain_pane_ids(&mut self, pane_ids: &BTreeSet<PaneId>) {
        self.pending_launches
            .retain(|pane_id| pane_ids.contains(pane_id));
        self.statuses
            .retain(|pane_id, _| pane_ids.contains(pane_id));
    }
}

pub(crate) struct TaskPaneRuntime {
    pub(crate) runtime: TerminalPaneRuntime,
    pub(crate) status: String,
}

impl TaskPaneRuntime {
    pub(crate) fn running(runtime: TerminalPaneRuntime) -> Self {
        Self {
            runtime,
            status: "running".to_owned(),
        }
    }

    pub(crate) fn resize(&mut self, size: PtySize) -> Result<(), NativePtyError> {
        self.runtime.resize(size)
    }

    pub(crate) fn shutdown(&mut self) {
        self.runtime.shutdown();
    }

    pub(crate) fn stop(&mut self) -> Result<(), NativePtyError> {
        self.runtime.stop()
    }
}

pub(crate) fn prepare_task_pane_runtime(
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
    // Always resolved (intent -> pane -> project): an unset cwd would fall
    // back to `$HOME` inside portable-pty and quietly run the user's task
    // command in the wrong directory.
    spawn_intent = spawn_intent.with_cwd(crate::terminal_runtime::resolve_pane_cwd(
        workspace,
        pane,
        intent.cwd.as_ref(),
    ));
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

pub(crate) fn task_status_label(status: ChildExitStatus) -> String {
    match status {
        ChildExitStatus::Exited { code: 0 } => "succeeded: exit 0".to_owned(),
        ChildExitStatus::Exited { code } => format!("failed: exit {code}"),
        ChildExitStatus::Signaled { signal } => format!("failed: signal {signal}"),
        ChildExitStatus::Unknown => "failed: unknown exit".to_owned(),
    }
}
