//! Live runtime ownership and cross-registry lifecycle policy.
//!
//! `RuntimeEngine` is the app-local Module that owns every live registry, the
//! unified event channel, and runtime-token allocation. The terminal, task,
//! and agent modules remain its low-level Implementations; callers use this
//! Module for replacement transactions, session retirement, shutdown, and
//! identity checks.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    time::Duration,
};

use mandatum_agent_runtime::{
    AgentSession, AgentSessionEvent, ApprovalDecision, ApprovalRequest, ApprovalVerdict,
};
use mandatum_core::{AgentStatus, PaneId, PaneKind, SessionId, Workspace};
use mandatum_pty::{ChildExitStatus, NativePtyError, PtySize};
use mandatum_terminal_vt::{MouseMode, TerminalGrid};

use crate::{
    agent_runtime::{AgentRuntimeEvent, AgentRuntimeRegistry, activate_agent_session},
    events::{AppEvent, AppEventSender, WakeCallback},
    process_events::PtyRuntimeEvent,
    task_runtime::{
        TaskInvestigationFailure, TaskPaneRuntime, TaskRuntimeRegistry, prepare_task_pane_runtime,
        task_status_label,
    },
    terminal_runtime::{
        PendingTerminalPaneRuntime, TerminalRuntimeError, TerminalRuntimeRegistry,
        prepare_terminal_pane_runtime,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeReconcileNotice {
    TerminalSpawned(PaneId),
    TerminalRestarted(PaneId),
    TaskStarted(PaneId),
}

#[derive(Debug)]
pub(crate) enum RuntimeReconcileError {
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

impl fmt::Display for RuntimeReconcileError {
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

impl std::error::Error for RuntimeReconcileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. } | Self::Restart { source, .. } => Some(source),
            Self::Resize { source, .. } => Some(source),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimePtyEffect {
    TerminalRead { pane_id: PaneId, bytes: usize },
    TaskRead { pane_id: PaneId, bytes: usize },
    TerminalParserFailed { pane_id: PaneId, error: String },
    TaskParserFailed { pane_id: PaneId, error: String },
    ReaderClosed { pane_id: PaneId },
    ReaderFailed { pane_id: PaneId, error: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeExitEffect {
    TerminalExited {
        pane_id: PaneId,
        status: ChildExitStatus,
    },
    TerminalWaitFailed {
        pane_id: PaneId,
        error: String,
    },
    TaskExited {
        pane_id: PaneId,
        status: String,
    },
    TaskWaitFailed {
        pane_id: PaneId,
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TaskStopOutcome {
    StoppedBeforeLaunch,
    NotRunning,
    Already(String),
    Stopped,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskAttempt {
    Initial,
    Rerun,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TaskLaunchOutcome {
    Disabled,
    Deferred,
    Running,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentApprovalError {
    NotRunning,
    NoPendingApproval,
    DecisionFailed(String),
}

#[derive(Clone, Copy)]
pub(crate) struct AgentRuntimeView<'a> {
    #[cfg(test)]
    pub(crate) restart_generation: u64,
    #[cfg(test)]
    pub(crate) runtime_token: u64,
    pub(crate) current_action: Option<&'a str>,
    pub(crate) last_error: Option<&'a str>,
    pub(crate) output_tail: &'a VecDeque<String>,
    pub(crate) pending_approval: Option<&'a ApprovalRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeTargetKind {
    Terminal,
    Task,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeLifecycleEpoch(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeLifecycleTrigger {
    StartupRestore,
    ExplicitRestore,
    SessionSwitch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeLifecycleReason {
    SessionSwitched,
    WaitingForGeometry,
    HiddenPane,
    PtySpawningDisabled,
    TaskRequiresExplicitRerun,
    ColdAgentSessionCannotReplay,
    DraftAgentHasNoRuntime,
    CompletedAgentHasNoRuntime,
    AgentRequiresExplicitRelaunch,
    InactiveSession,
    LaunchFailed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeNextAction {
    RerunTask,
    RelaunchAgent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeTarget {
    pub(crate) session_id: SessionId,
    pub(crate) pane_id: PaneId,
    pub(crate) kind: RuntimeTargetKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeDisposition {
    FreshProcessCreated,
    Deferred { reason: RuntimeLifecycleReason },
    Detached { reason: RuntimeLifecycleReason },
    NotReplayed { reason: RuntimeLifecycleReason },
    Failed { reason: RuntimeLifecycleReason },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeLifecycleFact {
    pub(crate) epoch: RuntimeLifecycleEpoch,
    pub(crate) trigger: RuntimeLifecycleTrigger,
    pub(crate) target: RuntimeTarget,
    pub(crate) disposition: RuntimeDisposition,
    pub(crate) next_action: Option<RuntimeNextAction>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RuntimeLifecycleReport {
    pub(crate) epoch: Option<RuntimeLifecycleEpoch>,
    pub(crate) trigger: Option<RuntimeLifecycleTrigger>,
    pub(crate) facts: Vec<RuntimeLifecycleFact>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestoreGeometry {
    Unavailable,
    Available,
}

/// A restore transaction whose PTYs have been spawned but whose reader
/// threads are not active yet. Dropping an uncommitted value rolls back every
/// staged child, so a staging failure cannot disturb the current runtimes or
/// emit committed lifecycle facts.
pub(crate) struct PreparedRuntimeRestore {
    terminals: BTreeMap<PaneId, PendingTerminalPaneRuntime>,
    spawn_pty: bool,
    trigger: RuntimeLifecycleTrigger,
    geometry: RestoreGeometry,
}

impl PreparedRuntimeRestore {
    fn new(spawn_pty: bool, trigger: RuntimeLifecycleTrigger, geometry: RestoreGeometry) -> Self {
        Self {
            terminals: BTreeMap::new(),
            spawn_pty,
            trigger,
            geometry,
        }
    }
}

impl Drop for PreparedRuntimeRestore {
    fn drop(&mut self) {
        for runtime in self.terminals.values_mut() {
            runtime.shutdown();
        }
    }
}

pub(crate) struct RuntimeEngine {
    terminals: TerminalRuntimeRegistry,
    tasks: TaskRuntimeRegistry,
    agents: AgentRuntimeRegistry,
    event_tx: AppEventSender,
    event_rx: Receiver<AppEvent>,
    next_runtime_token: u64,
    next_lifecycle_epoch: u64,
    last_lifecycle_report: RuntimeLifecycleReport,
}

impl RuntimeEngine {
    pub(crate) fn new() -> Self {
        Self::new_with_event_sender(None)
    }

    pub(crate) fn with_wake_callback(wake: WakeCallback) -> Self {
        Self::new_with_event_sender(Some(wake))
    }

    fn new_with_event_sender(wake: Option<WakeCallback>) -> Self {
        let (raw_event_tx, event_rx) = mpsc::channel();
        let event_tx = match wake {
            Some(wake) => AppEventSender::with_shared_wake_callback(raw_event_tx, wake),
            None => AppEventSender::new(raw_event_tx),
        };
        Self {
            terminals: TerminalRuntimeRegistry::new(),
            tasks: TaskRuntimeRegistry::new(),
            agents: AgentRuntimeRegistry::new(),
            event_tx,
            event_rx,
            next_runtime_token: 1,
            next_lifecycle_epoch: 1,
            last_lifecycle_report: RuntimeLifecycleReport::default(),
        }
    }

    pub(crate) fn event_sender(&self) -> AppEventSender {
        self.event_tx.clone()
    }

    pub(crate) fn recv_event_timeout(
        &self,
        timeout: Duration,
    ) -> Result<AppEvent, RecvTimeoutError> {
        self.event_tx.recv_timeout(&self.event_rx, timeout)
    }

    pub(crate) fn try_recv_event(&self) -> Result<AppEvent, TryRecvError> {
        self.event_tx.try_recv(&self.event_rx)
    }

    /// Raw registry inspection is test-only. Production callers use the
    /// product-shaped queries below, so controller, writer, flow-control, and
    /// connector-control Implementations cannot escape this Module.
    #[cfg(test)]
    pub(crate) fn terminals(&self) -> &TerminalRuntimeRegistry {
        &self.terminals
    }

    #[cfg(test)]
    pub(crate) fn tasks(&self) -> &TaskRuntimeRegistry {
        &self.tasks
    }

    pub(crate) fn terminal_count(&self) -> usize {
        self.terminals.len()
    }

    pub(crate) fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub(crate) fn agent_count(&self) -> usize {
        self.agents.len()
    }

    pub(crate) fn has_terminal(&self, pane_id: &PaneId) -> bool {
        self.terminals.contains_key(pane_id)
    }

    pub(crate) fn task_running_or_pending(&self, pane_id: &PaneId) -> bool {
        self.tasks
            .get(pane_id)
            .is_some_and(|task| task.runtime.exit_status.is_none())
            || self.tasks.pending_launches.contains(pane_id)
    }

    pub(crate) fn task_failure_label(&self, pane_id: &PaneId) -> Option<String> {
        self.tasks.failure_label(pane_id)
    }

    pub(crate) fn task_output_lines(&self, pane_id: &PaneId) -> Vec<String> {
        let Some(task) = self.tasks.get(pane_id) else {
            return Vec::new();
        };
        let grid = task.runtime.parser.grid();
        let scrollback = grid.scrollback_len();
        (0..grid.total_rows())
            .filter_map(|row| {
                if row < scrollback {
                    grid.scrollback_row_text(row)
                } else {
                    grid.row_text((row - scrollback) as u16)
                }
            })
            .collect()
    }

    pub(crate) fn task_view(&self, pane_id: &PaneId) -> Option<(&str, Option<&TerminalGrid>)> {
        if let Some(task) = self.tasks.get(pane_id) {
            return Some((task.status.as_str(), Some(task.runtime.parser.grid())));
        }
        self.tasks
            .statuses
            .get(pane_id)
            .map(|status| (status.as_str(), None))
    }

    pub(crate) fn task_grid(&self, pane_id: &PaneId) -> Option<&TerminalGrid> {
        self.tasks
            .get(pane_id)
            .map(|task| task.runtime.parser.grid())
    }

    pub(crate) fn terminal_exit_status(&self, pane_id: &PaneId) -> Option<Option<ChildExitStatus>> {
        self.terminals
            .get(pane_id)
            .map(|runtime| runtime.exit_status)
    }

    pub(crate) fn task_live_status(
        &self,
        pane_id: &PaneId,
    ) -> Option<(Option<ChildExitStatus>, &str)> {
        self.tasks
            .get(pane_id)
            .map(|task| (task.runtime.exit_status, task.status.as_str()))
    }

    pub(crate) fn agent_view(&self, pane_id: &PaneId) -> Option<AgentRuntimeView<'_>> {
        self.agents.get(pane_id).map(|runtime| AgentRuntimeView {
            #[cfg(test)]
            restart_generation: runtime.restart_generation,
            #[cfg(test)]
            runtime_token: runtime.runtime_token,
            current_action: runtime.current_action.as_deref(),
            last_error: runtime.last_error.as_deref(),
            output_tail: &runtime.output_tail,
            pending_approval: runtime.pending_approval.as_ref(),
        })
    }

    pub(crate) fn has_agent(&self, pane_id: &PaneId) -> bool {
        self.agents.get(pane_id).is_some()
    }

    pub(crate) fn terminal_grid(&self, pane_id: &PaneId) -> Option<&TerminalGrid> {
        self.terminals
            .get(pane_id)
            .map(|runtime| runtime.parser.grid())
    }

    pub(crate) fn terminal_mouse_mode(&self, pane_id: &PaneId) -> Option<MouseMode> {
        self.terminals
            .get(pane_id)
            .map(|runtime| runtime.parser.mouse_mode())
    }

    pub(crate) fn write_terminal(
        &mut self,
        pane_id: &PaneId,
        bytes: &[u8],
    ) -> Result<bool, NativePtyError> {
        let Some(runtime) = self.terminals.get_mut(pane_id) else {
            return Ok(false);
        };
        if let Err(error) = runtime.write_input(bytes) {
            runtime.error = Some(error.to_string());
            return Err(error);
        }
        Ok(true)
    }

    pub(crate) fn defer_task(&mut self, pane_id: PaneId, status: String) {
        if let Some(mut runtime) = self.tasks.remove(&pane_id) {
            runtime.shutdown();
        }
        self.tasks.clear_failure(&pane_id);
        self.tasks.pending_launches.insert(pane_id.clone());
        self.tasks.statuses.insert(pane_id, status);
    }

    pub(crate) fn set_task_status(&mut self, pane_id: PaneId, status: String) {
        self.tasks.statuses.insert(pane_id, status);
    }

    pub(crate) fn clear_task_attempt(&mut self, pane_id: &PaneId) {
        self.tasks.pending_launches.remove(pane_id);
        self.tasks.statuses.remove(pane_id);
        self.tasks.clear_failure(pane_id);
    }

    pub(crate) fn record_task_failure(
        &mut self,
        pane_id: PaneId,
        failure: TaskInvestigationFailure,
        status: String,
    ) {
        self.tasks.pending_launches.remove(&pane_id);
        self.tasks.record_failure(pane_id.clone(), failure);
        self.tasks.statuses.insert(pane_id, status);
    }

    #[cfg(test)]
    pub(crate) fn clear_task_failure(&mut self, pane_id: &PaneId) {
        self.tasks.clear_failure(pane_id);
    }

    pub(crate) fn stop_task(&mut self, pane_id: &PaneId) -> TaskStopOutcome {
        if self.tasks.pending_launches.remove(pane_id) {
            self.tasks
                .statuses
                .insert(pane_id.clone(), "stopped before launch".to_owned());
            return TaskStopOutcome::StoppedBeforeLaunch;
        }

        let Some(mut task) = self.tasks.remove(pane_id) else {
            self.tasks
                .statuses
                .insert(pane_id.clone(), "not running".to_owned());
            return TaskStopOutcome::NotRunning;
        };
        if task.runtime.exit_status.is_some() {
            let status = task.status.clone();
            self.tasks.insert(pane_id.clone(), task);
            return TaskStopOutcome::Already(status);
        }
        match task.stop() {
            Ok(()) => {
                self.tasks
                    .statuses
                    .insert(pane_id.clone(), "stopped".to_owned());
                TaskStopOutcome::Stopped
            }
            Err(error) => {
                let message = error.to_string();
                task.runtime.error = Some(message.clone());
                task.status = format!("task stop failed: {message}");
                self.tasks.insert(pane_id.clone(), task);
                TaskStopOutcome::Failed(message)
            }
        }
    }

    pub(crate) fn launch_task(
        &mut self,
        workspace: &Workspace,
        shell_program: &str,
        pane_id: PaneId,
        size: Option<PtySize>,
        spawn_pty: bool,
        attempt: TaskAttempt,
    ) -> TaskLaunchOutcome {
        if !spawn_pty {
            if attempt == TaskAttempt::Rerun {
                self.set_task_status(
                    pane_id,
                    "rerun unavailable: PTY spawning is disabled".to_owned(),
                );
            }
            return TaskLaunchOutcome::Disabled;
        }
        let Some(size) = size else {
            let status = match attempt {
                TaskAttempt::Initial => "pending launch: waiting for visible pane size",
                TaskAttempt::Rerun => "pending rerun: waiting for visible pane size",
            };
            self.defer_task(pane_id, status.to_owned());
            return TaskLaunchOutcome::Deferred;
        };

        self.clear_task_attempt(&pane_id);
        match self.spawn_task(workspace, shell_program, pane_id.clone(), size) {
            Ok(()) => TaskLaunchOutcome::Running,
            Err(source) => {
                let failure = match attempt {
                    TaskAttempt::Initial => TaskInvestigationFailure::Launch(source.to_string()),
                    TaskAttempt::Rerun => TaskInvestigationFailure::Rerun(source.to_string()),
                };
                let status = match attempt {
                    TaskAttempt::Initial => format!("task launch failed: {source}"),
                    TaskAttempt::Rerun => format!("task rerun failed: {source}"),
                };
                self.record_task_failure(pane_id, failure, status.clone());
                TaskLaunchOutcome::Failed(status)
            }
        }
    }

    /// Replace an agent only after its connector has successfully launched.
    /// Returns whether a previous live session was retired.
    pub(crate) fn replace_agent(
        &mut self,
        pane_id: PaneId,
        restart_generation: u64,
        session: AgentSession,
    ) -> bool {
        let replaced = self.stop_agent(&pane_id);
        self.activate_agent(pane_id, restart_generation, session);
        replaced
    }

    pub(crate) fn stop_agent(&mut self, pane_id: &PaneId) -> bool {
        let Some(mut runtime) = self.agents.remove(pane_id) else {
            return false;
        };
        runtime.shutdown();
        true
    }

    pub(crate) fn decide_agent_approval(
        &mut self,
        pane_id: &PaneId,
        approved: bool,
    ) -> Result<ApprovalRequest, AgentApprovalError> {
        let Some(runtime) = self.agents.get_mut(pane_id) else {
            return Err(AgentApprovalError::NotRunning);
        };
        let Some(request) = runtime.pending_approval.clone() else {
            return Err(AgentApprovalError::NoPendingApproval);
        };
        let verdict = if approved {
            ApprovalVerdict::Approved
        } else {
            ApprovalVerdict::Rejected {
                reason: Some("rejected from the workstation".to_owned()),
            }
        };
        runtime
            .control
            .decide(ApprovalDecision {
                approval_id: request.approval_id.clone(),
                verdict,
            })
            .map_err(|error| AgentApprovalError::DecisionFailed(error.to_string()))?;
        runtime.pending_approval = None;
        Ok(request)
    }

    /// Authenticate one connector event and fold its live-only state before
    /// returning the durable event for AppState to apply.
    pub(crate) fn accept_agent_event(
        &mut self,
        runtime_event: AgentRuntimeEvent,
    ) -> Option<(PaneId, AgentSessionEvent)> {
        if !self.agent_identity_matches(&runtime_event) {
            return None;
        }
        let pane_id = runtime_event.pane_id;
        let event = runtime_event.event;
        let runtime = self
            .agents
            .get_mut(&pane_id)
            .expect("an authenticated event has a live agent runtime");
        match &event {
            AgentSessionEvent::Action { description } => {
                runtime.current_action = Some(description.clone());
            }
            AgentSessionEvent::OutputChunk(chunk) => runtime.push_output(chunk),
            AgentSessionEvent::CommandRun { command } => {
                runtime.push_output(&format!("$ {command}"));
                runtime.current_action = Some(format!("ran {command}"));
            }
            AgentSessionEvent::ApprovalRequested(request) => {
                runtime.pending_approval = Some(request.clone());
            }
            AgentSessionEvent::Completed { .. } => runtime.current_action = None,
            AgentSessionEvent::Failed { error } => {
                runtime.current_action = None;
                runtime.last_error = Some(error.clone());
            }
            AgentSessionEvent::Closed => {
                runtime.closed = true;
                runtime.pending_approval = None;
            }
            AgentSessionEvent::Status(_)
            | AgentSessionEvent::Summary(_)
            | AgentSessionEvent::FilesChanged(_) => {}
        }
        Some((pane_id, event))
    }

    /// Reconcile all active-session runtimes in one ordered engine request.
    /// Registry Implementations and replacement ordering never escape this
    /// Module; callers receive renderer-neutral effects only.
    pub(crate) fn reconcile(
        &mut self,
        workspace: &mut Workspace,
        shell_program: &str,
        spawn_pty: bool,
        visible_terminals: Vec<(PaneId, PtySize)>,
        visible_tasks: Vec<(PaneId, PtySize)>,
    ) -> Result<Vec<RuntimeReconcileNotice>, RuntimeReconcileError> {
        let mut notices = Vec::new();
        let session_id = workspace.active_session().id().clone();
        let terminal_ids = workspace
            .active_session()
            .panes()
            .iter()
            .filter(|(_, pane)| matches!(pane.kind(), PaneKind::Terminal { .. }))
            .map(|(pane_id, _)| pane_id.clone())
            .collect::<BTreeSet<_>>();
        let task_ids = workspace
            .active_session()
            .panes()
            .iter()
            .filter(|(_, pane)| matches!(pane.kind(), PaneKind::Task { .. }))
            .map(|(pane_id, _)| pane_id.clone())
            .collect::<BTreeSet<_>>();
        let agent_ids = workspace
            .active_session()
            .panes()
            .iter()
            .filter(|(_, pane)| matches!(pane.kind(), PaneKind::Agent { .. }))
            .map(|(pane_id, _)| pane_id.clone())
            .collect::<BTreeSet<_>>();
        let visible_terminal_ids = visible_terminals
            .iter()
            .map(|(pane_id, _)| pane_id.clone())
            .collect::<BTreeSet<_>>();

        if spawn_pty {
            let removed = self
                .terminals
                .keys()
                .filter(|pane_id| !terminal_ids.contains(*pane_id))
                .cloned()
                .collect::<Vec<_>>();
            for pane_id in removed {
                if let Some(mut runtime) = self.terminals.remove(&pane_id) {
                    runtime.shutdown();
                }
            }

            for (pane_id, size) in visible_terminals {
                let generation = workspace
                    .active_session()
                    .pane(&pane_id)
                    .map_or(0, |pane| pane.restart_generation());
                let needs_restart = self
                    .terminals
                    .get(&pane_id)
                    .is_some_and(|runtime| generation > runtime.restart_generation);
                if needs_restart {
                    if let Some(mut runtime) = self.terminals.remove(&pane_id) {
                        runtime.shutdown();
                    }
                    let pending = self
                        .prepare_terminal(workspace, shell_program, pane_id.clone(), size)
                        .map_err(|source| RuntimeReconcileError::Restart {
                            pane_id: pane_id.clone(),
                            source,
                        })?;
                    self.activate_terminal(pane_id.clone(), pending);
                    notices.push(RuntimeReconcileNotice::TerminalRestarted(pane_id));
                } else if let Some(runtime) = self.terminals.get_mut(&pane_id) {
                    if let Err(source) = runtime.resize(size) {
                        runtime.error = Some(source.to_string());
                        return Err(RuntimeReconcileError::Resize { pane_id, source });
                    }
                } else {
                    let pending = self
                        .prepare_terminal(workspace, shell_program, pane_id.clone(), size)
                        .map_err(|source| RuntimeReconcileError::Spawn {
                            pane_id: pane_id.clone(),
                            source,
                        })?;
                    self.activate_terminal(pane_id.clone(), pending);
                    notices.push(RuntimeReconcileNotice::TerminalSpawned(pane_id));
                }
            }
            self.mark_recovery_geometry_reconciled(&session_id, &visible_terminal_ids);
        }

        let removed_agents = self
            .agents
            .keys()
            .filter(|pane_id| !agent_ids.contains(*pane_id))
            .cloned()
            .collect::<Vec<_>>();
        for pane_id in removed_agents {
            self.stop_agent(&pane_id);
            for session in workspace.sessions_mut() {
                if let Some(intent) = session.agent_intent_mut(&pane_id) {
                    intent.detach_live_session();
                }
            }
        }

        if spawn_pty {
            self.tasks.retain_pane_ids(&task_ids);
            let removed = self
                .tasks
                .keys()
                .filter(|pane_id| !task_ids.contains(*pane_id))
                .cloned()
                .collect::<Vec<_>>();
            for pane_id in removed {
                if let Some(mut runtime) = self.tasks.remove(&pane_id) {
                    runtime.shutdown();
                }
            }

            for (pane_id, size) in &visible_tasks {
                let Some(runtime) = self.tasks.get_mut(pane_id) else {
                    continue;
                };
                if let Err(source) = runtime.resize(*size) {
                    runtime.runtime.error = Some(source.to_string());
                    runtime.status = format!("task resize failed: {source}");
                    return Err(RuntimeReconcileError::Resize {
                        pane_id: pane_id.clone(),
                        source,
                    });
                }
            }

            let pending = visible_tasks
                .into_iter()
                .filter(|(pane_id, _)| {
                    self.tasks.pending_launches.contains(pane_id)
                        && !self.tasks.contains_key(pane_id)
                })
                .collect::<Vec<_>>();
            for (pane_id, size) in pending {
                if let Err(source) =
                    self.spawn_task(workspace, shell_program, pane_id.clone(), size)
                {
                    self.record_task_failure(
                        pane_id.clone(),
                        TaskInvestigationFailure::Launch(source.to_string()),
                        format!("task launch failed: {source}"),
                    );
                    return Err(RuntimeReconcileError::Spawn { pane_id, source });
                }
                self.clear_task_attempt(&pane_id);
                notices.push(RuntimeReconcileNotice::TaskStarted(pane_id));
            }
        }

        Ok(notices)
    }

    /// The latest committed replacement/retirement facts. This is deliberately
    /// renderer-neutral so a future recovery cockpit can consume it without
    /// reopening lifecycle policy in `AppState`.
    pub(crate) fn last_lifecycle_report(&self) -> &RuntimeLifecycleReport {
        &self.last_lifecycle_report
    }

    pub(crate) fn shutdown_all(&mut self) {
        self.agents.shutdown_all();
        self.tasks.shutdown_all();
        self.terminals.shutdown_all();
    }

    /// Retire the previous active session as one committed lifecycle change.
    /// Pane ids are only session-local, so no live entry may survive this call.
    pub(crate) fn retire_session(
        &mut self,
        workspace: &mut Workspace,
        previous_session_id: &SessionId,
    ) -> RuntimeLifecycleReport {
        let epoch = self.allocate_lifecycle_epoch();
        let trigger = RuntimeLifecycleTrigger::SessionSwitch;
        let live_targets = self.live_runtime_targets();
        let live_agent_ids = live_targets
            .iter()
            .filter_map(|(pane_id, kind)| {
                (*kind == RuntimeTargetKind::Agent).then_some(pane_id.clone())
            })
            .collect::<Vec<_>>();
        let facts = live_targets
            .into_iter()
            .map(|(pane_id, kind)| {
                detached_fact(epoch, trigger, previous_session_id, pane_id, kind)
            })
            .collect();
        if let Some(previous_session) = workspace
            .sessions_mut()
            .find(|session| session.id() == previous_session_id)
        {
            for pane_id in &live_agent_ids {
                if let Some(intent) = previous_session.agent_intent_mut(pane_id) {
                    intent.detach_live_session();
                }
            }
        }

        self.shutdown_all();
        let report = RuntimeLifecycleReport {
            epoch: Some(epoch),
            trigger: Some(trigger),
            facts,
        };
        self.last_lifecycle_report = report.clone();
        report
    }

    pub(crate) fn prepare_restore(
        &mut self,
        workspace: &Workspace,
        shell_program: &str,
        spawn_pty: bool,
        trigger: RuntimeLifecycleTrigger,
        geometry: RestoreGeometry,
        visible_terminals: Vec<(PaneId, PtySize)>,
    ) -> Result<PreparedRuntimeRestore, RestoreRuntimeError> {
        let mut prepared = PreparedRuntimeRestore::new(spawn_pty, trigger, geometry);
        if !spawn_pty {
            return Ok(prepared);
        }

        for (pane_id, size) in visible_terminals {
            let runtime_token = self.allocate_runtime_token();
            let runtime = prepare_terminal_pane_runtime(
                workspace,
                shell_program,
                runtime_token,
                pane_id.clone(),
                size,
            )
            .map_err(|source| RestoreRuntimeError {
                pane_id: pane_id.clone(),
                _trigger: trigger,
                _target: Box::new(RuntimeTarget {
                    session_id: workspace.active_session().id().clone(),
                    pane_id: pane_id.clone(),
                    kind: RuntimeTargetKind::Terminal,
                }),
                _disposition: Box::new(RuntimeDisposition::Failed {
                    reason: RuntimeLifecycleReason::LaunchFailed(source.to_string()),
                }),
                source,
            })?;
            prepared.terminals.insert(pane_id, runtime);
        }
        Ok(prepared)
    }

    /// Commit a fully staged restore. This is the only path that emits fresh,
    /// deferred, detached, or not-replayed restore facts.
    pub(crate) fn commit_restore(
        &mut self,
        workspace: &mut Workspace,
        outgoing_session_id: &SessionId,
        mut prepared: PreparedRuntimeRestore,
    ) -> RuntimeLifecycleReport {
        let epoch = self.allocate_lifecycle_epoch();
        let staged_ids = prepared.terminals.keys().cloned().collect::<Vec<_>>();
        let mut facts = self.outgoing_runtime_facts(epoch, prepared.trigger, outgoing_session_id);
        facts.extend(restore_facts(
            epoch,
            prepared.trigger,
            workspace,
            &staged_ids,
            prepared.spawn_pty,
            prepared.geometry,
        ));

        // Preserve the established restore ordering before the replacement is
        // activated, then discard only stale runtime events. Buffered input is
        // retained in channel order.
        self.terminals.shutdown_all();
        self.tasks.shutdown_all();
        self.agents.shutdown_all();
        self.discard_pending_runtime_events();

        for session in workspace.sessions_mut() {
            for intent in session.agent_intents_mut() {
                intent.detach_live_session();
            }
        }

        let staged = std::mem::take(&mut prepared.terminals);
        self.terminals = staged
            .into_iter()
            .map(|(pane_id, runtime)| {
                let active = runtime.activate(pane_id.clone(), self.event_tx.clone());
                (pane_id, active)
            })
            .collect();

        let report = RuntimeLifecycleReport {
            epoch: Some(epoch),
            trigger: Some(prepared.trigger),
            facts,
        };
        self.last_lifecycle_report = report.clone();
        report
    }

    pub(crate) fn discard_pending_runtime_events(&mut self) {
        let mut pending = Vec::new();
        while let Ok(event) = self.try_recv_event() {
            pending.push(event);
        }
        for event in pending {
            if matches!(event, AppEvent::Input(_) | AppEvent::Artifact(_)) {
                let _ = self.event_tx.send(event);
            }
        }
    }

    fn prepare_terminal(
        &mut self,
        workspace: &Workspace,
        shell_program: &str,
        pane_id: PaneId,
        size: PtySize,
    ) -> Result<PendingTerminalPaneRuntime, TerminalRuntimeError> {
        let runtime_token = self.allocate_runtime_token();
        prepare_terminal_pane_runtime(workspace, shell_program, runtime_token, pane_id, size)
    }

    fn prepare_task(
        &mut self,
        workspace: &Workspace,
        shell_program: &str,
        pane_id: PaneId,
        size: PtySize,
    ) -> Result<PendingTerminalPaneRuntime, TerminalRuntimeError> {
        let runtime_token = self.allocate_runtime_token();
        prepare_task_pane_runtime(workspace, shell_program, runtime_token, pane_id, size)
    }

    fn activate_terminal(&mut self, pane_id: PaneId, runtime: PendingTerminalPaneRuntime) {
        let active = runtime.activate(pane_id.clone(), self.event_tx.clone());
        self.terminals.insert(pane_id, active);
    }

    fn activate_task(&mut self, pane_id: PaneId, runtime: PendingTerminalPaneRuntime) {
        let active = runtime.activate(pane_id.clone(), self.event_tx.clone());
        self.tasks.insert(pane_id, TaskPaneRuntime::running(active));
    }

    fn activate_agent(&mut self, pane_id: PaneId, restart_generation: u64, session: AgentSession) {
        let runtime_token = self.allocate_runtime_token();
        let runtime = activate_agent_session(
            pane_id.clone(),
            restart_generation,
            runtime_token,
            session,
            self.event_tx.clone(),
        );
        self.agents.insert(pane_id, runtime);
    }

    fn spawn_task(
        &mut self,
        workspace: &Workspace,
        shell_program: &str,
        pane_id: PaneId,
        size: PtySize,
    ) -> Result<(), TerminalRuntimeError> {
        if let Some(mut runtime) = self.tasks.remove(&pane_id) {
            runtime.shutdown();
        }
        let pending = self.prepare_task(workspace, shell_program, pane_id.clone(), size)?;
        self.activate_task(pane_id, pending);
        Ok(())
    }

    pub(crate) fn apply_pty_event(&mut self, event: PtyRuntimeEvent) -> Option<RuntimePtyEffect> {
        if !self.pty_identity_matches(&event) {
            return None;
        }
        match event {
            PtyRuntimeEvent::Output { pane_id, bytes, .. } => {
                if let Some(runtime) = self.terminals.get_mut(&pane_id) {
                    return Some(match runtime.parser.feed_pty_bytes(&bytes) {
                        Ok(_) => RuntimePtyEffect::TerminalRead {
                            pane_id,
                            bytes: bytes.len(),
                        },
                        Err(error) => {
                            let error = error.to_string();
                            runtime.error = Some(error.clone());
                            RuntimePtyEffect::TerminalParserFailed { pane_id, error }
                        }
                    });
                }
                let task = self
                    .tasks
                    .get_mut(&pane_id)
                    .expect("an authenticated PTY event has a terminal or task runtime");
                Some(match task.runtime.parser.feed_pty_bytes(&bytes) {
                    Ok(_) => RuntimePtyEffect::TaskRead {
                        pane_id,
                        bytes: bytes.len(),
                    },
                    Err(error) => {
                        let error = error.to_string();
                        task.runtime.error = Some(error.clone());
                        task.status = format!("task parser failed: {error}");
                        RuntimePtyEffect::TaskParserFailed { pane_id, error }
                    }
                })
            }
            PtyRuntimeEvent::ReaderClosed { pane_id, .. } => {
                if self
                    .tasks
                    .get(&pane_id)
                    .is_some_and(|task| task.runtime.exit_status.is_some())
                {
                    return None;
                }
                if let Some(task) = self.tasks.get_mut(&pane_id) {
                    task.status = "task reader closed".to_owned();
                }
                Some(RuntimePtyEffect::ReaderClosed { pane_id })
            }
            PtyRuntimeEvent::Error {
                pane_id, message, ..
            } => {
                if let Some(runtime) = self.terminals.get_mut(&pane_id) {
                    runtime.error = Some(message.clone());
                } else if let Some(task) = self.tasks.get_mut(&pane_id) {
                    task.runtime.error = Some(message.clone());
                    task.status = format!("task reader failed: {message}");
                }
                Some(RuntimePtyEffect::ReaderFailed {
                    pane_id,
                    error: message,
                })
            }
        }
    }

    pub(crate) fn poll_child_exits(&mut self) -> Vec<RuntimeExitEffect> {
        let mut effects = Vec::new();
        for (pane_id, runtime) in self.terminals.iter_mut() {
            if runtime.exit_status.is_some() {
                continue;
            }
            match runtime.controller.try_wait() {
                Ok(Some(exit)) => {
                    let status = exit.status();
                    runtime.exit_status = Some(status);
                    effects.push(RuntimeExitEffect::TerminalExited {
                        pane_id: pane_id.clone(),
                        status,
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    let error = error.to_string();
                    runtime.error = Some(error.clone());
                    effects.push(RuntimeExitEffect::TerminalWaitFailed {
                        pane_id: pane_id.clone(),
                        error,
                    });
                }
            }
        }

        let mut failures = Vec::new();
        for (pane_id, task) in self.tasks.iter_mut() {
            if task.runtime.exit_status.is_some() {
                continue;
            }
            match task.runtime.controller.try_wait() {
                Ok(Some(exit)) => {
                    let child_status = exit.status();
                    task.runtime.exit_status = Some(child_status);
                    task.status = task_status_label(child_status);
                    let failure = (child_status != ChildExitStatus::Exited { code: 0 })
                        .then_some(TaskInvestigationFailure::ProcessExit(child_status));
                    failures.push((pane_id.clone(), failure));
                    effects.push(RuntimeExitEffect::TaskExited {
                        pane_id: pane_id.clone(),
                        status: task.status.clone(),
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    let error = error.to_string();
                    task.runtime.error = Some(error.clone());
                    task.status = format!("task wait failed: {error}");
                    effects.push(RuntimeExitEffect::TaskWaitFailed {
                        pane_id: pane_id.clone(),
                        error,
                    });
                }
            }
        }
        for (pane_id, failure) in failures {
            if let Some(failure) = failure {
                self.tasks.record_failure(pane_id, failure);
            } else {
                self.tasks.clear_failure(&pane_id);
            }
        }
        effects
    }

    pub(crate) fn pty_identity_matches(&self, event: &PtyRuntimeEvent) -> bool {
        let (pane_id, restart_generation, runtime_token) = match event {
            PtyRuntimeEvent::Output {
                pane_id,
                restart_generation,
                runtime_token,
                ..
            }
            | PtyRuntimeEvent::ReaderClosed {
                pane_id,
                restart_generation,
                runtime_token,
            }
            | PtyRuntimeEvent::Error {
                pane_id,
                restart_generation,
                runtime_token,
                ..
            } => (pane_id, restart_generation, runtime_token),
        };
        self.terminals.get(pane_id).is_some_and(|runtime| {
            runtime.restart_generation == *restart_generation
                && runtime.runtime_token == *runtime_token
        }) || self.tasks.get(pane_id).is_some_and(|task| {
            task.runtime.restart_generation == *restart_generation
                && task.runtime.runtime_token == *runtime_token
        })
    }

    pub(crate) fn agent_identity_matches(&self, event: &AgentRuntimeEvent) -> bool {
        self.agents.get(&event.pane_id).is_some_and(|runtime| {
            runtime.restart_generation == event.restart_generation
                && runtime.runtime_token == event.runtime_token
        })
    }

    fn allocate_runtime_token(&mut self) -> u64 {
        let token = self.next_runtime_token;
        self.next_runtime_token += 1;
        token
    }

    fn allocate_lifecycle_epoch(&mut self) -> RuntimeLifecycleEpoch {
        let epoch = RuntimeLifecycleEpoch(self.next_lifecycle_epoch);
        self.next_lifecycle_epoch += 1;
        epoch
    }

    fn outgoing_runtime_facts(
        &self,
        epoch: RuntimeLifecycleEpoch,
        trigger: RuntimeLifecycleTrigger,
        session_id: &SessionId,
    ) -> Vec<RuntimeLifecycleFact> {
        self.live_runtime_targets()
            .into_iter()
            .map(|(pane_id, kind)| detached_fact(epoch, trigger, session_id, pane_id, kind))
            .collect()
    }

    fn live_runtime_targets(&self) -> Vec<(PaneId, RuntimeTargetKind)> {
        self.terminals
            .keys()
            .filter(|pane_id| {
                self.terminals
                    .get(pane_id)
                    .is_some_and(|runtime| runtime.exit_status.is_none())
            })
            .cloned()
            .map(|pane_id| (pane_id, RuntimeTargetKind::Terminal))
            .chain(
                self.tasks
                    .keys()
                    .filter(|pane_id| {
                        self.tasks
                            .get(pane_id)
                            .is_some_and(|task| task.runtime.exit_status.is_none())
                    })
                    .cloned()
                    .map(|pane_id| (pane_id, RuntimeTargetKind::Task)),
            )
            .chain(
                self.agents
                    .keys()
                    .filter(|pane_id| {
                        self.agents
                            .get(pane_id)
                            .is_some_and(|runtime| !runtime.closed)
                    })
                    .cloned()
                    .map(|pane_id| (pane_id, RuntimeTargetKind::Agent)),
            )
            .collect()
    }

    fn mark_recovery_geometry_reconciled(
        &mut self,
        session_id: &SessionId,
        visible_terminal_ids: &BTreeSet<PaneId>,
    ) {
        let Some(epoch) = self.last_lifecycle_report.epoch else {
            return;
        };
        let Some(trigger) = self.last_lifecycle_report.trigger else {
            return;
        };
        if !matches!(
            trigger,
            RuntimeLifecycleTrigger::StartupRestore | RuntimeLifecycleTrigger::ExplicitRestore
        ) {
            return;
        }
        for fact in &mut self.last_lifecycle_report.facts {
            if fact.epoch != epoch
                || fact.target.session_id != *session_id
                || fact.target.kind != RuntimeTargetKind::Terminal
            {
                continue;
            }
            let visible = visible_terminal_ids.contains(&fact.target.pane_id);
            fact.disposition = match &fact.disposition {
                RuntimeDisposition::Deferred {
                    reason: RuntimeLifecycleReason::WaitingForGeometry,
                } if visible => RuntimeDisposition::FreshProcessCreated,
                RuntimeDisposition::Deferred {
                    reason: RuntimeLifecycleReason::WaitingForGeometry,
                } => RuntimeDisposition::Deferred {
                    reason: RuntimeLifecycleReason::HiddenPane,
                },
                RuntimeDisposition::Deferred {
                    reason: RuntimeLifecycleReason::HiddenPane,
                } if visible => RuntimeDisposition::FreshProcessCreated,
                _ => continue,
            };
            fact.next_action = None;
        }
    }
}

fn detached_fact(
    epoch: RuntimeLifecycleEpoch,
    trigger: RuntimeLifecycleTrigger,
    session_id: &SessionId,
    pane_id: PaneId,
    kind: RuntimeTargetKind,
) -> RuntimeLifecycleFact {
    RuntimeLifecycleFact {
        epoch,
        trigger,
        target: RuntimeTarget {
            session_id: session_id.clone(),
            pane_id,
            kind,
        },
        disposition: RuntimeDisposition::Detached {
            reason: RuntimeLifecycleReason::SessionSwitched,
        },
        next_action: None,
    }
}

fn restore_facts(
    epoch: RuntimeLifecycleEpoch,
    trigger: RuntimeLifecycleTrigger,
    workspace: &Workspace,
    staged_terminal_ids: &[PaneId],
    spawn_pty: bool,
    geometry: RestoreGeometry,
) -> Vec<RuntimeLifecycleFact> {
    let active_session_id = workspace.active_session().id().clone();
    let mut facts = Vec::new();
    for (session_id, session) in workspace.sessions() {
        for (pane_id, pane) in session.panes() {
            let target = |kind| RuntimeTarget {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                kind,
            };
            let active = session_id == &active_session_id;
            let (kind, disposition, next_action) = match pane.kind() {
                PaneKind::Terminal { .. } if active && staged_terminal_ids.contains(pane_id) => (
                    RuntimeTargetKind::Terminal,
                    RuntimeDisposition::FreshProcessCreated,
                    None,
                ),
                PaneKind::Terminal { .. }
                    if active && spawn_pty && geometry == RestoreGeometry::Unavailable =>
                {
                    (
                        RuntimeTargetKind::Terminal,
                        RuntimeDisposition::Deferred {
                            reason: RuntimeLifecycleReason::WaitingForGeometry,
                        },
                        None,
                    )
                }
                PaneKind::Terminal { .. } if active && spawn_pty => (
                    RuntimeTargetKind::Terminal,
                    RuntimeDisposition::Deferred {
                        reason: RuntimeLifecycleReason::HiddenPane,
                    },
                    None,
                ),
                PaneKind::Terminal { .. } if active => (
                    RuntimeTargetKind::Terminal,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::PtySpawningDisabled,
                    },
                    None,
                ),
                PaneKind::Terminal { .. } => (
                    RuntimeTargetKind::Terminal,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::InactiveSession,
                    },
                    None,
                ),
                PaneKind::Task { .. } if active => (
                    RuntimeTargetKind::Task,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::TaskRequiresExplicitRerun,
                    },
                    Some(RuntimeNextAction::RerunTask),
                ),
                PaneKind::Task { .. } => (
                    RuntimeTargetKind::Task,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::InactiveSession,
                    },
                    None,
                ),
                PaneKind::Agent { .. } if !active => (
                    RuntimeTargetKind::Agent,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::InactiveSession,
                    },
                    None,
                ),
                PaneKind::Agent { intent }
                    if matches!(
                        intent.status,
                        AgentStatus::Running | AgentStatus::WaitingForApproval
                    ) || intent.pending_approvals > 0
                        || !intent.pending_approval_ids.is_empty() =>
                {
                    (
                        RuntimeTargetKind::Agent,
                        RuntimeDisposition::NotReplayed {
                            reason: RuntimeLifecycleReason::ColdAgentSessionCannotReplay,
                        },
                        Some(RuntimeNextAction::RelaunchAgent),
                    )
                }
                PaneKind::Agent { intent } if intent.status == AgentStatus::Draft => (
                    RuntimeTargetKind::Agent,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::DraftAgentHasNoRuntime,
                    },
                    None,
                ),
                PaneKind::Agent { intent } if intent.status == AgentStatus::Complete => (
                    RuntimeTargetKind::Agent,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::CompletedAgentHasNoRuntime,
                    },
                    None,
                ),
                PaneKind::Agent { .. } => (
                    RuntimeTargetKind::Agent,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::AgentRequiresExplicitRelaunch,
                    },
                    Some(RuntimeNextAction::RelaunchAgent),
                ),
                _ => continue,
            };
            facts.push(RuntimeLifecycleFact {
                epoch,
                trigger,
                target: target(kind),
                disposition,
                next_action,
            });
        }
    }
    facts
}

#[derive(Debug)]
pub(crate) struct RestoreRuntimeError {
    pane_id: PaneId,
    _trigger: RuntimeLifecycleTrigger,
    _target: Box<RuntimeTarget>,
    _disposition: Box<RuntimeDisposition>,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mandatum_agent_runtime::{
        AgentConnector, AgentLaunchSpec, AgentSessionEvent, FakeConnector, FakeStep,
    };
    use mandatum_core::{AgentPaneIntent, CoreAction, PaneKind, SessionId, TaskPaneIntent};
    use mandatum_scene::input::InputEvent;

    use super::*;

    #[test]
    fn runtime_tokens_are_monotonic_across_runtime_kinds() {
        let mut engine = RuntimeEngine::new();

        assert_eq!(engine.allocate_runtime_token(), 1);
        assert_eq!(engine.allocate_runtime_token(), 2);
        assert_eq!(engine.allocate_runtime_token(), 3);
    }

    #[test]
    fn runtime_event_discard_preserves_buffered_input_and_artifact_completion() {
        let engine = &mut RuntimeEngine::new();
        let sender = engine.event_sender();
        sender
            .send(AppEvent::Agent(AgentRuntimeEvent {
                pane_id: PaneId::new("pane-stale"),
                restart_generation: 1,
                runtime_token: 7,
                event: AgentSessionEvent::Summary("stale".to_owned()),
            }))
            .unwrap();
        sender
            .send(AppEvent::Artifact(
                crate::artifact_preview::ArtifactLoadEvent::failed_for_test(
                    SessionId::new("session-stale"),
                    PaneId::new("pane-artifact"),
                    11,
                ),
            ))
            .unwrap();
        sender
            .send(AppEvent::Input(InputEvent::FocusGained))
            .unwrap();

        engine.discard_pending_runtime_events();

        assert!(matches!(engine.try_recv_event(), Ok(AppEvent::Artifact(_))));
        assert!(matches!(
            engine.try_recv_event(),
            Ok(AppEvent::Input(InputEvent::FocusGained))
        ));
        assert!(matches!(engine.try_recv_event(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn restore_before_geometry_reports_deferred_and_never_fresh() {
        let project_path = PathBuf::from("/tmp/mandatum-runtime-engine-test");
        let mut workspace = Workspace::new("test", project_path.clone());
        workspace
            .apply_action(CoreAction::CreateTaskPane {
                title: "test".to_owned(),
                intent: TaskPaneIntent {
                    recipe_id: None,
                    command: "cargo test".to_owned(),
                    cwd: Some(project_path.clone()),
                },
            })
            .unwrap();
        let mut agent_intent = AgentPaneIntent::draft("inspect failures");
        agent_intent.status = AgentStatus::Running;
        workspace
            .apply_action(CoreAction::CreateAgentPane {
                title: "agent".to_owned(),
                intent: agent_intent,
                cwd: Some(project_path),
            })
            .unwrap();

        let mut engine = RuntimeEngine::new();
        let prepared = engine
            .prepare_restore(
                &workspace,
                "/bin/sh",
                true,
                RuntimeLifecycleTrigger::StartupRestore,
                RestoreGeometry::Unavailable,
                Vec::new(),
            )
            .unwrap();
        let outgoing_session_id = workspace.active_session().id().clone();
        let report = engine.commit_restore(&mut workspace, &outgoing_session_id, prepared);

        assert_eq!(
            report.trigger,
            Some(RuntimeLifecycleTrigger::StartupRestore)
        );
        let epoch = report.epoch.expect("committed restore must have an epoch");
        assert!(report.facts.iter().any(|fact| {
            fact.epoch == epoch
                && fact.trigger == RuntimeLifecycleTrigger::StartupRestore
                && fact.target.kind == RuntimeTargetKind::Terminal
                && matches!(
                    fact.disposition,
                    RuntimeDisposition::Deferred {
                        reason: RuntimeLifecycleReason::WaitingForGeometry
                    }
                )
        }));
        assert!(report.facts.iter().any(|fact| {
            fact.target.kind == RuntimeTargetKind::Task
                && matches!(
                    fact.disposition,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::TaskRequiresExplicitRerun
                    }
                )
                && fact.next_action == Some(RuntimeNextAction::RerunTask)
        }));
        assert!(report.facts.iter().any(|fact| {
            fact.target.kind == RuntimeTargetKind::Agent
                && matches!(
                    fact.disposition,
                    RuntimeDisposition::NotReplayed {
                        reason: RuntimeLifecycleReason::ColdAgentSessionCannotReplay
                    }
                )
                && fact.next_action == Some(RuntimeNextAction::RelaunchAgent)
        }));
        assert!(!report.facts.iter().any(|fact| {
            fact.target.kind == RuntimeTargetKind::Agent
                && matches!(fact.disposition, RuntimeDisposition::Detached { .. })
        }));
        assert!(
            !report
                .facts
                .iter()
                .any(|fact| matches!(fact.disposition, RuntimeDisposition::FreshProcessCreated))
        );
        assert_eq!(engine.last_lifecycle_report(), &report);

        let detached = workspace
            .active_session()
            .panes()
            .values()
            .find_map(|pane| match pane.kind() {
                PaneKind::Agent { intent } => Some(intent),
                _ => None,
            })
            .unwrap();
        assert_eq!(detached.status, AgentStatus::Unknown);
    }

    #[test]
    fn first_geometry_reconcile_updates_the_restore_epoch_once() {
        let mut workspace = Workspace::new(
            "test",
            PathBuf::from("/tmp/mandatum-runtime-engine-first-size"),
        );
        let pane_id = workspace.active_session().focused_pane_id().clone();
        let outgoing_session_id = workspace.active_session().id().clone();
        let mut engine = RuntimeEngine::new();
        let prepared = engine
            .prepare_restore(
                &workspace,
                "/bin/sh",
                true,
                RuntimeLifecycleTrigger::StartupRestore,
                RestoreGeometry::Unavailable,
                Vec::new(),
            )
            .unwrap();
        let restored = engine.commit_restore(&mut workspace, &outgoing_session_id, prepared);
        let epoch = restored.epoch.expect("restore epoch");
        let fact_count = restored.facts.len();

        let first = engine
            .reconcile(
                &mut workspace,
                "/bin/sh",
                true,
                vec![(pane_id.clone(), PtySize::new(80, 24).unwrap())],
                Vec::new(),
            )
            .unwrap();
        assert_eq!(
            first,
            vec![RuntimeReconcileNotice::TerminalSpawned(pane_id.clone())]
        );
        let report = engine.last_lifecycle_report();
        assert_eq!(report.epoch, Some(epoch));
        assert_eq!(report.facts.len(), fact_count);
        assert!(report.facts.iter().any(|fact| {
            fact.epoch == epoch
                && fact.target.pane_id == pane_id
                && matches!(fact.disposition, RuntimeDisposition::FreshProcessCreated)
        }));

        let repeated = engine
            .reconcile(
                &mut workspace,
                "/bin/sh",
                true,
                vec![(pane_id.clone(), PtySize::new(100, 30).unwrap())],
                Vec::new(),
            )
            .unwrap();
        assert!(repeated.is_empty());
        let report = engine.last_lifecycle_report();
        assert_eq!(report.epoch, Some(epoch));
        assert_eq!(report.facts.len(), fact_count);
        assert_eq!(
            report
                .facts
                .iter()
                .filter(|fact| {
                    fact.target.pane_id == pane_id
                        && matches!(fact.disposition, RuntimeDisposition::FreshProcessCreated)
                })
                .count(),
            1
        );
        engine.shutdown_all();
    }

    #[test]
    fn first_geometry_reconcile_batches_visible_and_hidden_terminal_facts() {
        let mut workspace = Workspace::new(
            "test",
            PathBuf::from("/tmp/mandatum-runtime-engine-first-size-batch"),
        );
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        let terminal_ids = workspace
            .active_session()
            .panes()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let visible_id = terminal_ids[0].clone();
        let hidden_id = terminal_ids[1].clone();
        let outgoing_session_id = workspace.active_session().id().clone();
        let mut engine = RuntimeEngine::new();
        let prepared = engine
            .prepare_restore(
                &workspace,
                "/bin/sh",
                true,
                RuntimeLifecycleTrigger::StartupRestore,
                RestoreGeometry::Unavailable,
                Vec::new(),
            )
            .unwrap();
        let restored = engine.commit_restore(&mut workspace, &outgoing_session_id, prepared);
        let epoch = restored.epoch.expect("restore epoch");
        let fact_count = restored.facts.len();

        engine
            .reconcile(
                &mut workspace,
                "/bin/sh",
                true,
                vec![(visible_id.clone(), PtySize::new(80, 24).unwrap())],
                Vec::new(),
            )
            .unwrap();

        let report = engine.last_lifecycle_report();
        assert_eq!(report.epoch, Some(epoch));
        assert_eq!(report.facts.len(), fact_count);
        let visible = report
            .facts
            .iter()
            .find(|fact| fact.target.pane_id == visible_id)
            .unwrap();
        assert!(matches!(
            visible.disposition,
            RuntimeDisposition::FreshProcessCreated
        ));
        let hidden = report
            .facts
            .iter()
            .find(|fact| fact.target.pane_id == hidden_id)
            .unwrap();
        assert!(matches!(
            hidden.disposition,
            RuntimeDisposition::Deferred {
                reason: RuntimeLifecycleReason::HiddenPane
            }
        ));
        assert!(report.facts.iter().all(|fact| fact.epoch == epoch));

        engine
            .reconcile(
                &mut workspace,
                "/bin/sh",
                true,
                vec![(hidden_id.clone(), PtySize::new(80, 24).unwrap())],
                Vec::new(),
            )
            .unwrap();

        let report = engine.last_lifecycle_report();
        assert_eq!(report.epoch, Some(epoch));
        assert_eq!(report.facts.len(), fact_count);
        assert!(report.facts.iter().any(|fact| {
            fact.target.pane_id == hidden_id
                && matches!(fact.disposition, RuntimeDisposition::FreshProcessCreated)
        }));
        assert_eq!(
            report
                .facts
                .iter()
                .filter(|fact| {
                    fact.target.kind == RuntimeTargetKind::Terminal
                        && matches!(fact.disposition, RuntimeDisposition::FreshProcessCreated)
                })
                .count(),
            2
        );
        engine.shutdown_all();
    }

    #[test]
    fn restore_staging_failure_commits_no_lifecycle_facts() {
        let workspace = Workspace::new("test", PathBuf::from("/tmp"));
        let mut engine = RuntimeEngine::new();
        let pane_id = workspace.active_session().focused_pane_id().clone();
        let size = PtySize::new(80, 24).unwrap();

        let result = engine.prepare_restore(
            &workspace,
            "/definitely/missing/mandatum-shell",
            true,
            RuntimeLifecycleTrigger::ExplicitRestore,
            RestoreGeometry::Available,
            vec![(pane_id, size)],
        );

        assert!(result.is_err());
        assert!(engine.last_lifecycle_report().facts.is_empty());
        assert_eq!(engine.terminal_count(), 0);
    }

    #[test]
    fn restore_with_geometry_distinguishes_hidden_terminal_from_waiting_for_size() {
        let mut workspace = Workspace::new("test", PathBuf::from("/tmp"));
        let outgoing_session_id = workspace.active_session().id().clone();
        let mut engine = RuntimeEngine::new();

        let prepared = engine
            .prepare_restore(
                &workspace,
                "/bin/sh",
                true,
                RuntimeLifecycleTrigger::ExplicitRestore,
                RestoreGeometry::Available,
                Vec::new(),
            )
            .unwrap();
        let report = engine.commit_restore(&mut workspace, &outgoing_session_id, prepared);

        assert!(report.facts.iter().any(|fact| {
            fact.target.kind == RuntimeTargetKind::Terminal
                && matches!(
                    fact.disposition,
                    RuntimeDisposition::Deferred {
                        reason: RuntimeLifecycleReason::HiddenPane
                    }
                )
        }));
        assert!(!report.facts.iter().any(|fact| {
            matches!(
                fact.disposition,
                RuntimeDisposition::Deferred {
                    reason: RuntimeLifecycleReason::WaitingForGeometry
                }
            )
        }));
    }

    #[test]
    fn draft_and_completed_agents_have_no_recovery_action() {
        let project_path = PathBuf::from("/tmp/mandatum-runtime-engine-actions");
        let mut workspace = Workspace::new("test", project_path.clone());
        workspace
            .apply_action(CoreAction::CreateAgentPane {
                title: "draft".to_owned(),
                intent: AgentPaneIntent::draft("draft objective"),
                cwd: Some(project_path.clone()),
            })
            .unwrap();
        let mut complete = AgentPaneIntent::draft("completed objective");
        complete.status = AgentStatus::Complete;
        workspace
            .apply_action(CoreAction::CreateAgentPane {
                title: "complete".to_owned(),
                intent: complete,
                cwd: Some(project_path),
            })
            .unwrap();

        let mut engine = RuntimeEngine::new();
        let prepared = engine
            .prepare_restore(
                &workspace,
                "/bin/sh",
                true,
                RuntimeLifecycleTrigger::ExplicitRestore,
                RestoreGeometry::Unavailable,
                Vec::new(),
            )
            .unwrap();
        let outgoing_session_id = workspace.active_session().id().clone();
        let report = engine.commit_restore(&mut workspace, &outgoing_session_id, prepared);

        let agent_facts = report
            .facts
            .iter()
            .filter(|fact| fact.target.kind == RuntimeTargetKind::Agent)
            .collect::<Vec<_>>();
        assert_eq!(agent_facts.len(), 2);
        assert!(agent_facts.iter().all(|fact| fact.next_action.is_none()));
        assert!(agent_facts.iter().any(|fact| {
            matches!(
                fact.disposition,
                RuntimeDisposition::NotReplayed {
                    reason: RuntimeLifecycleReason::DraftAgentHasNoRuntime
                }
            )
        }));
        assert!(agent_facts.iter().any(|fact| {
            matches!(
                fact.disposition,
                RuntimeDisposition::NotReplayed {
                    reason: RuntimeLifecycleReason::CompletedAgentHasNoRuntime
                }
            )
        }));
    }

    #[test]
    fn inactive_agents_never_claim_a_recovery_action() {
        let project_path = PathBuf::from("/tmp/mandatum-runtime-engine-inactive-agents");
        let mut workspace = Workspace::new("first", project_path.clone());
        let mut running = AgentPaneIntent::draft("running");
        running.status = AgentStatus::Running;
        let mut waiting = AgentPaneIntent::draft("waiting");
        waiting.status = AgentStatus::WaitingForApproval;
        let mut approval_bearing = AgentPaneIntent::draft("approval-bearing");
        approval_bearing.status = AgentStatus::Unknown;
        approval_bearing.pending_approvals = 1;
        approval_bearing.pending_approval_ids = vec!["approval-1".to_owned()];
        for (title, intent) in [
            ("running", running),
            ("waiting", waiting),
            ("approval", approval_bearing),
        ] {
            workspace
                .apply_action(CoreAction::CreateAgentPane {
                    title: title.to_owned(),
                    intent,
                    cwd: Some(project_path.clone()),
                })
                .unwrap();
        }
        workspace
            .apply_action(CoreAction::OpenProject {
                name: "second".to_owned(),
                path: project_path,
            })
            .unwrap();

        let mut engine = RuntimeEngine::new();
        let prepared = engine
            .prepare_restore(
                &workspace,
                "/bin/sh",
                true,
                RuntimeLifecycleTrigger::ExplicitRestore,
                RestoreGeometry::Unavailable,
                Vec::new(),
            )
            .unwrap();
        let outgoing_session_id = workspace.active_session().id().clone();
        let report = engine.commit_restore(&mut workspace, &outgoing_session_id, prepared);
        let inactive_agents = report
            .facts
            .iter()
            .filter(|fact| fact.target.kind == RuntimeTargetKind::Agent)
            .collect::<Vec<_>>();

        assert_eq!(inactive_agents.len(), 3);
        assert!(inactive_agents.iter().all(|fact| {
            matches!(
                fact.disposition,
                RuntimeDisposition::NotReplayed {
                    reason: RuntimeLifecycleReason::InactiveSession
                }
            ) && fact.next_action.is_none()
        }));
    }

    #[test]
    fn explicit_restore_records_outgoing_live_runtime_as_detached() {
        let project_path = PathBuf::from("/tmp/mandatum-runtime-engine-replace");
        let mut outgoing_workspace = Workspace::new("outgoing", project_path.clone());
        let outcome = outgoing_workspace
            .apply_action(CoreAction::CreateAgentPane {
                title: "live agent".to_owned(),
                intent: AgentPaneIntent::draft("remain attributable"),
                cwd: Some(project_path.clone()),
            })
            .unwrap();
        let pane_id = match outcome {
            mandatum_core::ActionOutcome::Mutated { focused_pane } => focused_pane,
            _ => panic!("agent creation must mutate the workspace"),
        };
        let outgoing_session_id = outgoing_workspace.active_session().id().clone();
        let live_session = FakeConnector::new(Vec::new())
            .launch(&AgentLaunchSpec::new("live", project_path.clone()))
            .unwrap();

        let mut engine = RuntimeEngine::new();
        engine.activate_agent(pane_id.clone(), 0, live_session);

        let mut restored_workspace = Workspace::new("restored", project_path);
        let prepared = engine
            .prepare_restore(
                &restored_workspace,
                "/bin/sh",
                true,
                RuntimeLifecycleTrigger::ExplicitRestore,
                RestoreGeometry::Unavailable,
                Vec::new(),
            )
            .unwrap();
        let report = engine.commit_restore(&mut restored_workspace, &outgoing_session_id, prepared);

        assert!(report.facts.iter().any(|fact| {
            fact.trigger == RuntimeLifecycleTrigger::ExplicitRestore
                && fact.target.session_id == outgoing_session_id
                && fact.target.pane_id == pane_id
                && fact.target.kind == RuntimeTargetKind::Agent
                && matches!(
                    fact.disposition,
                    RuntimeDisposition::Detached {
                        reason: RuntimeLifecycleReason::SessionSwitched
                    }
                )
        }));
    }

    #[test]
    fn detached_facts_include_only_live_runtime_entries() {
        let project_path = PathBuf::from("/tmp/mandatum-runtime-engine-live-facts");
        let mut workspace = Workspace::new("test", project_path.clone());
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        let terminal_ids = workspace
            .active_session()
            .panes()
            .keys()
            .cloned()
            .collect::<Vec<_>>();

        let mut task_ids = Vec::new();
        for title in ["live task", "exited task"] {
            let outcome = workspace
                .apply_action(CoreAction::CreateTaskPane {
                    title: title.to_owned(),
                    intent: TaskPaneIntent {
                        recipe_id: None,
                        command: "sleep 30".to_owned(),
                        cwd: Some(project_path.clone()),
                    },
                })
                .unwrap();
            let mandatum_core::ActionOutcome::Mutated { focused_pane } = outcome else {
                panic!("task creation must mutate the workspace");
            };
            task_ids.push(focused_pane);
        }

        let mut agent_ids = Vec::new();
        for title in ["live agent", "closed agent"] {
            let outcome = workspace
                .apply_action(CoreAction::CreateAgentPane {
                    title: title.to_owned(),
                    intent: AgentPaneIntent::draft(title),
                    cwd: Some(project_path.clone()),
                })
                .unwrap();
            let mandatum_core::ActionOutcome::Mutated { focused_pane } = outcome else {
                panic!("agent creation must mutate the workspace");
            };
            agent_ids.push(focused_pane);
        }

        let session_id = workspace.active_session().id().clone();
        let size = PtySize::new(80, 24).unwrap();
        let mut engine = RuntimeEngine::new();
        let prepared = engine
            .prepare_restore(
                &workspace,
                "/bin/sh",
                true,
                RuntimeLifecycleTrigger::ExplicitRestore,
                RestoreGeometry::Available,
                terminal_ids
                    .iter()
                    .cloned()
                    .map(|pane_id| (pane_id, size))
                    .collect(),
            )
            .unwrap();
        engine.commit_restore(&mut workspace, &session_id, prepared);
        for pane_id in &task_ids {
            let pending = engine
                .prepare_task(&workspace, "/bin/sh", pane_id.clone(), size)
                .unwrap();
            engine.activate_task(pane_id.clone(), pending);
        }
        for pane_id in &agent_ids {
            let session = FakeConnector::new(Vec::new())
                .launch(&AgentLaunchSpec::new("test", project_path.clone()))
                .unwrap();
            engine.activate_agent(pane_id.clone(), 0, session);
        }

        let live_terminal = terminal_ids[0].clone();
        let exited_terminal = terminal_ids[1].clone();
        let live_task = task_ids[0].clone();
        let exited_task = task_ids[1].clone();
        let live_agent = agent_ids[0].clone();
        let closed_agent = agent_ids[1].clone();
        engine
            .terminals
            .get_mut(&exited_terminal)
            .unwrap()
            .exit_status = Some(ChildExitStatus::Exited { code: 0 });
        engine
            .tasks
            .get_mut(&exited_task)
            .unwrap()
            .runtime
            .exit_status = Some(ChildExitStatus::Exited { code: 0 });
        engine.agents.get_mut(&closed_agent).unwrap().closed = true;

        let assert_live_only = |facts: &[RuntimeLifecycleFact]| {
            let ids = facts
                .iter()
                .map(|fact| fact.target.pane_id.clone())
                .collect::<BTreeSet<_>>();
            assert!(ids.contains(&live_terminal));
            assert!(ids.contains(&live_task));
            assert!(ids.contains(&live_agent));
            assert!(!ids.contains(&exited_terminal));
            assert!(!ids.contains(&exited_task));
            assert!(!ids.contains(&closed_agent));
        };

        let outgoing = engine.outgoing_runtime_facts(
            RuntimeLifecycleEpoch(999),
            RuntimeLifecycleTrigger::ExplicitRestore,
            &session_id,
        );
        assert_live_only(&outgoing);
        let retired = engine.retire_session(&mut workspace, &session_id);
        assert_live_only(&retired.facts);
    }

    #[test]
    fn session_retirement_detaches_agents_and_records_the_transition() {
        let project_path = PathBuf::from("/tmp/mandatum-runtime-engine-retire");
        let mut workspace = Workspace::new("test", project_path.clone());
        let mut intent = AgentPaneIntent::draft("wait for shutdown");
        intent.status = AgentStatus::Running;
        let outcome = workspace
            .apply_action(CoreAction::CreateAgentPane {
                title: "agent".to_owned(),
                intent,
                cwd: Some(project_path.clone()),
            })
            .unwrap();
        let pane_id = match outcome {
            mandatum_core::ActionOutcome::Mutated { focused_pane } => focused_pane,
            _ => panic!("agent creation must mutate the workspace"),
        };
        let session_id = workspace.active_session().id().clone();
        let session = FakeConnector::new(vec![FakeStep::AwaitApproval {
            approval_id: "never".to_owned(),
            then_on_approve: Vec::new(),
            then_on_reject: Vec::new(),
        }])
        .launch(&AgentLaunchSpec::new("wait", project_path))
        .unwrap();

        let mut engine = RuntimeEngine::new();
        engine.activate_agent(pane_id.clone(), 0, session);
        let report = engine.retire_session(&mut workspace, &session_id);

        assert_eq!(engine.agent_count(), 0);
        assert_eq!(report.trigger, Some(RuntimeLifecycleTrigger::SessionSwitch));
        assert!(report.facts.iter().any(|fact| {
            fact.target.session_id == session_id
                && fact.target.pane_id == pane_id
                && fact.target.kind == RuntimeTargetKind::Agent
                && matches!(
                    fact.disposition,
                    RuntimeDisposition::Detached {
                        reason: RuntimeLifecycleReason::SessionSwitched
                    }
                )
        }));
        let PaneKind::Agent { intent } = workspace.active_session().pane(&pane_id).unwrap().kind()
        else {
            panic!("created pane must remain an agent pane")
        };
        assert_eq!(intent.status, AgentStatus::Unknown);
    }
}
