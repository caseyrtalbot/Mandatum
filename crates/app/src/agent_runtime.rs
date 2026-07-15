//! Live agent session registry and event forwarding.
//!
//! Mirrors the PTY runtime discipline exactly (`process_events.rs` /
//! `task_runtime.rs`): one forwarder thread per live agent session pumps
//! [`AgentSessionEvent`]s into the app event channel wrapped with the pane's
//! `(restart_generation, runtime_token)` identity, and `app_state` applies an
//! event only if the pane's *current* generation and token match — events
//! from a replaced runtime are dropped (the [L3-GATE] path).
//!
//! Everything held here is **live runtime state** (control handles, threads,
//! current action, output tail, full approval detail). None of it is ever
//! serialized; the durable subset of agent state lives in
//! `mandatum_core::AgentPaneIntent`.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::mpsc::{Receiver, Sender},
    thread::{self, JoinHandle},
};

use mandatum_agent_runtime::{
    AgentConnector, AgentSession, AgentSessionControl, AgentSessionEvent, ApprovalRequest,
    ApprovalScope, ClaudeCliConnector, FakeConnector, FakeStep, FileChange, RiskAssessment,
    RiskLevel,
};
use mandatum_core::{AgentStatus, PaneId};

use crate::events::AppEvent;

/// How many trailing output lines the live view retains per agent pane.
pub(crate) const AGENT_OUTPUT_TAIL_LINES: usize = 200;

/// An agent session event stamped with the runtime identity it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentRuntimeEvent {
    pub(crate) pane_id: PaneId,
    pub(crate) restart_generation: u64,
    pub(crate) runtime_token: u64,
    pub(crate) event: AgentSessionEvent,
}

/// Forward every event from a live agent session into the app event channel,
/// stamped with the launching runtime's identity. Exits when the session's
/// sender drops (after `Closed`) or the app channel disconnects.
pub(crate) fn spawn_agent_forwarder_thread(
    pane_id: PaneId,
    restart_generation: u64,
    runtime_token: u64,
    events: Receiver<AgentSessionEvent>,
    tx: Sender<AppEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(event) = events.recv() {
            if tx
                .send(AppEvent::Agent(AgentRuntimeEvent {
                    pane_id: pane_id.clone(),
                    restart_generation,
                    runtime_token,
                    event,
                }))
                .is_err()
            {
                break;
            }
        }
    })
}

#[derive(Default)]
pub(crate) struct AgentRuntimeRegistry {
    runtimes: BTreeMap<PaneId, AgentPaneRuntime>,
}

impl AgentRuntimeRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.runtimes.len()
    }

    pub(crate) fn get(&self, pane_id: &PaneId) -> Option<&AgentPaneRuntime> {
        self.runtimes.get(pane_id)
    }

    pub(crate) fn get_mut(&mut self, pane_id: &PaneId) -> Option<&mut AgentPaneRuntime> {
        self.runtimes.get_mut(pane_id)
    }

    pub(crate) fn insert(
        &mut self,
        pane_id: PaneId,
        runtime: AgentPaneRuntime,
    ) -> Option<AgentPaneRuntime> {
        self.runtimes.insert(pane_id, runtime)
    }

    pub(crate) fn remove(&mut self, pane_id: &PaneId) -> Option<AgentPaneRuntime> {
        self.runtimes.remove(pane_id)
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &PaneId> {
        self.runtimes.keys()
    }

    pub(crate) fn shutdown_all(&mut self) {
        for runtime in self.runtimes.values_mut() {
            runtime.shutdown();
        }
        self.runtimes.clear();
    }
}

/// Live runtime state for one agent pane. Never serialized.
pub(crate) struct AgentPaneRuntime {
    pub(crate) control: Box<dyn AgentSessionControl>,
    pub(crate) forwarder_thread: Option<JoinHandle<()>>,
    pub(crate) restart_generation: u64,
    pub(crate) runtime_token: u64,
    /// What the agent is doing right now.
    pub(crate) current_action: Option<String>,
    /// Why the session failed (its `Failed` event's reason), kept so the
    /// pane can state the failure persistently, not just in the transient
    /// status line.
    pub(crate) last_error: Option<String>,
    /// Trailing output lines, capped at [`AGENT_OUTPUT_TAIL_LINES`].
    pub(crate) output_tail: VecDeque<String>,
    /// Full detail of the approval awaiting a decision.
    pub(crate) pending_approval: Option<ApprovalRequest>,
    /// Whether the session has emitted `Closed`.
    pub(crate) closed: bool,
}

impl AgentPaneRuntime {
    pub(crate) fn push_output(&mut self, chunk: &str) {
        for line in chunk.lines() {
            if self.output_tail.len() == AGENT_OUTPUT_TAIL_LINES {
                self.output_tail.pop_front();
            }
            self.output_tail.push_back(line.to_owned());
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.control.shutdown();
        if let Some(handle) = self.forwarder_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Activate a launched session for a pane: spawn its forwarder thread and
/// build the live registry entry.
pub(crate) fn activate_agent_session(
    pane_id: PaneId,
    restart_generation: u64,
    runtime_token: u64,
    session: AgentSession,
    tx: Sender<AppEvent>,
) -> AgentPaneRuntime {
    let AgentSession { events, control } = session;
    let forwarder_thread =
        spawn_agent_forwarder_thread(pane_id, restart_generation, runtime_token, events, tx);
    AgentPaneRuntime {
        control,
        forwarder_thread: Some(forwarder_thread),
        restart_generation,
        runtime_token,
        current_action: None,
        last_error: None,
        output_tail: VecDeque::new(),
        pending_approval: None,
        closed: false,
    }
}

/// Build the connector configured in [`crate::AppConfig`], if it is
/// available in this build.
pub(crate) fn connector_for_kind(
    kind: crate::config::AgentConnectorKind,
) -> Option<Box<dyn AgentConnector>> {
    match kind {
        crate::config::AgentConnectorKind::Fake => {
            Some(Box::new(FakeConnector::new(default_fake_script())))
        }
        crate::config::AgentConnectorKind::Claude => Some(Box::new(ClaudeCliConnector::default())),
    }
}

/// The default script the fake connector replays when selected via config
/// (tests inject their own scripts). It walks the full agent loop a
/// stranger should see: run, work, request an approval, wait for the
/// verdict, then finish — approve completes with a changed file, reject
/// fails.
fn default_fake_script() -> Vec<FakeStep> {
    use std::path::PathBuf;

    vec![
        FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
        FakeStep::Emit(AgentSessionEvent::Action {
            description: "surveying the project".to_owned(),
        }),
        FakeStep::Emit(AgentSessionEvent::OutputChunk(
            "reading the failing check output".to_owned(),
        )),
        FakeStep::Emit(AgentSessionEvent::CommandRun {
            command: "cat .flip".to_owned(),
        }),
        FakeStep::Emit(AgentSessionEvent::Summary(
            "found the flaky marker file; want to remove it".to_owned(),
        )),
        FakeStep::Emit(AgentSessionEvent::ApprovalRequested(ApprovalRequest {
            approval_id: "fake-appr-1".to_owned(),
            command: "rm .flip".to_owned(),
            scope: ApprovalScope {
                cwd: PathBuf::from("."),
                affected_path: Some(PathBuf::from(".flip")),
            },
            risk: RiskAssessment {
                level: RiskLevel::Medium,
                basis: "removes files (rm)".to_owned(),
            },
        })),
        FakeStep::AwaitApproval {
            approval_id: "fake-appr-1".to_owned(),
            then_on_approve: vec![
                AgentSessionEvent::CommandRun {
                    command: "rm .flip".to_owned(),
                },
                AgentSessionEvent::FilesChanged(vec![FileChange {
                    path: PathBuf::from(".flip"),
                    change_kind: mandatum_agent_runtime::FileChangeKind::Deleted,
                }]),
                AgentSessionEvent::Completed {
                    summary: "removed the flaky marker; checks should pass".to_owned(),
                },
            ],
            then_on_reject: vec![AgentSessionEvent::Failed {
                error: "the gated command was rejected".to_owned(),
            }],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConnectorKind;

    /// Every connector kind AppConfig can select must resolve to a live
    /// connector: a kind that maps to None makes Start Agent unlaunchable
    /// for that configuration (the product default is Claude).
    #[test]
    fn every_configured_connector_kind_is_wired() {
        assert!(connector_for_kind(AgentConnectorKind::Fake).is_some());
        assert!(connector_for_kind(AgentConnectorKind::Claude).is_some());
    }
}
