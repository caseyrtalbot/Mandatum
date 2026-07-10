//! Deterministic scripted connector for tests and demos.
//!
//! A [`FakeConnector`] replays a script of [`FakeStep`]s on a worker thread,
//! exactly like a real connector would stream a live agent. `AwaitApproval`
//! steps block until [`crate::AgentSessionControl::decide`] delivers a
//! verdict, which makes approval flows — including pathological ones
//! (double-decide, decide-after-shutdown, event floods) — fully scriptable.

use std::{
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    approval::{ApprovalDecision, ApprovalVerdict},
    connector::{
        AgentConnector, AgentConnectorError, AgentControlError, AgentSession, AgentSessionControl,
    },
    events::AgentSessionEvent,
    spec::AgentLaunchSpec,
};

/// One step of a [`FakeConnector`] script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeStep {
    /// Emit this event to the session.
    Emit(AgentSessionEvent),
    /// Block until a decision for `approval_id` arrives, then emit the
    /// matching branch. Scripts emit their own
    /// [`AgentSessionEvent::ApprovalRequested`] before this step.
    AwaitApproval {
        approval_id: String,
        then_on_approve: Vec<AgentSessionEvent>,
        then_on_reject: Vec<AgentSessionEvent>,
    },
    /// Pause for this many milliseconds (interruptible by shutdown).
    Sleep(u64),
}

/// Scripted [`AgentConnector`]. Every `launch` replays the same script on a
/// fresh worker thread; the worker always emits [`AgentSessionEvent::Closed`]
/// last and then drops the sender, so the receiver disconnects.
#[derive(Clone, Debug)]
pub struct FakeConnector {
    script: Vec<FakeStep>,
}

impl FakeConnector {
    pub fn new(script: Vec<FakeStep>) -> Self {
        Self { script }
    }
}

impl AgentConnector for FakeConnector {
    fn launch(&self, spec: &AgentLaunchSpec) -> Result<AgentSession, AgentConnectorError> {
        if spec.objective.trim().is_empty() {
            return Err(AgentConnectorError::InvalidSpec {
                message: "objective must not be empty".to_owned(),
            });
        }

        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(SharedState::default());
        let worker_shared = Arc::clone(&shared);
        let script = self.script.clone();
        thread::spawn(move || run_script(script, &tx, &worker_shared));

        Ok(AgentSession {
            events: rx,
            control: Box::new(FakeSessionControl { shared }),
        })
    }

    fn name(&self) -> &str {
        "fake"
    }
}

#[derive(Debug)]
struct SharedState {
    inner: Mutex<ControlState>,
    wake: Condvar,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(ControlState {
                pending_approval: None,
                decision: None,
                shutdown: false,
                interrupted: false,
                alive: true,
            }),
            wake: Condvar::new(),
        }
    }
}

#[derive(Debug)]
struct ControlState {
    /// Approval id the worker is currently blocked on, if any.
    pending_approval: Option<String>,
    /// Verdict queued for the pending approval, not yet consumed.
    decision: Option<ApprovalVerdict>,
    shutdown: bool,
    interrupted: bool,
    alive: bool,
}

impl ControlState {
    fn stop_requested(&self) -> bool {
        self.shutdown || self.interrupted
    }
}

struct FakeSessionControl {
    shared: Arc<SharedState>,
}

impl AgentSessionControl for FakeSessionControl {
    fn decide(&mut self, decision: ApprovalDecision) -> Result<(), AgentControlError> {
        let mut state = self.shared.inner.lock().unwrap();
        if !state.alive {
            return Err(AgentControlError::SessionClosed);
        }
        match &state.pending_approval {
            None => Err(AgentControlError::NoPendingApproval),
            Some(pending) if *pending != decision.approval_id => {
                Err(AgentControlError::UnknownApproval {
                    approval_id: decision.approval_id,
                })
            }
            Some(_) if state.decision.is_some() => Err(AgentControlError::AlreadyDecided {
                approval_id: decision.approval_id,
            }),
            Some(_) => {
                state.decision = Some(decision.verdict);
                self.shared.wake.notify_all();
                Ok(())
            }
        }
    }

    fn interrupt(&mut self) -> Result<(), AgentControlError> {
        let mut state = self.shared.inner.lock().unwrap();
        if !state.alive {
            return Err(AgentControlError::SessionClosed);
        }
        state.interrupted = true;
        self.shared.wake.notify_all();
        Ok(())
    }

    fn shutdown(&mut self) {
        let mut state = self.shared.inner.lock().unwrap();
        state.shutdown = true;
        self.shared.wake.notify_all();
    }

    fn is_alive(&self) -> bool {
        self.shared.inner.lock().unwrap().alive
    }
}

fn run_script(script: Vec<FakeStep>, tx: &Sender<AgentSessionEvent>, shared: &SharedState) {
    'script: for step in script {
        if shared.inner.lock().unwrap().stop_requested() {
            break;
        }
        match step {
            FakeStep::Emit(event) => {
                if tx.send(event).is_err() {
                    break 'script;
                }
            }
            FakeStep::Sleep(ms) => {
                let deadline = Instant::now() + Duration::from_millis(ms);
                let mut state = shared.inner.lock().unwrap();
                loop {
                    if state.stop_requested() {
                        break 'script;
                    }
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    let (next, _) = shared.wake.wait_timeout(state, deadline - now).unwrap();
                    state = next;
                }
            }
            FakeStep::AwaitApproval {
                approval_id,
                then_on_approve,
                then_on_reject,
            } => {
                let verdict = {
                    let mut state = shared.inner.lock().unwrap();
                    state.pending_approval = Some(approval_id);
                    state.decision = None;
                    loop {
                        if state.stop_requested() {
                            state.pending_approval = None;
                            break 'script;
                        }
                        if let Some(verdict) = state.decision.take() {
                            state.pending_approval = None;
                            break verdict;
                        }
                        state = shared.wake.wait(state).unwrap();
                    }
                };
                let branch = match verdict {
                    ApprovalVerdict::Approved => then_on_approve,
                    ApprovalVerdict::Rejected { .. } => then_on_reject,
                };
                for event in branch {
                    if tx.send(event).is_err() {
                        break 'script;
                    }
                }
            }
        }
    }

    // Mark dead *before* emitting Closed so that once a consumer observes
    // Closed, is_alive() is already false.
    {
        let mut state = shared.inner.lock().unwrap();
        state.alive = false;
        state.pending_approval = None;
    }
    let _ = tx.send(AgentSessionEvent::Closed);
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use mandatum_core::AgentStatus;

    use super::*;
    use crate::{
        approval::{ApprovalRequest, ApprovalScope, RiskAssessment, RiskLevel},
        events::{FileChange, FileChangeKind},
    };

    const RECV_TIMEOUT: Duration = Duration::from_secs(5);

    fn spec() -> AgentLaunchSpec {
        AgentLaunchSpec::new("fix the failing test", "/tmp/project")
    }

    fn approval_request(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            approval_id: id.to_owned(),
            command: "rm -rf target".to_owned(),
            scope: ApprovalScope {
                cwd: PathBuf::from("/tmp/project"),
                affected_path: Some(PathBuf::from("target")),
            },
            risk: RiskAssessment {
                level: RiskLevel::High,
                basis: "removes files (rm)".to_owned(),
            },
        }
    }

    fn recv(session: &AgentSession) -> AgentSessionEvent {
        session.events.recv_timeout(RECV_TIMEOUT).unwrap()
    }

    fn approve(id: &str) -> ApprovalDecision {
        ApprovalDecision {
            approval_id: id.to_owned(),
            verdict: ApprovalVerdict::Approved,
        }
    }

    /// Wait until the worker parks on the AwaitApproval step, so decide()
    /// has a pending approval to address.
    fn wait_for_pending(session: &mut AgentSession) {
        let deadline = Instant::now() + RECV_TIMEOUT;
        loop {
            match session.control.decide(ApprovalDecision {
                approval_id: "definitely-not-a-real-id".to_owned(),
                verdict: ApprovalVerdict::Approved,
            }) {
                Err(AgentControlError::UnknownApproval { .. }) => return,
                Err(AgentControlError::NoPendingApproval) if Instant::now() < deadline => {
                    thread::yield_now();
                }
                other => panic!("unexpected control result while waiting: {other:?}"),
            }
        }
    }

    #[test]
    fn happy_path_script_streams_events_in_order_then_closes() {
        let script = vec![
            FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
            FakeStep::Emit(AgentSessionEvent::Action {
                description: "reading tests".to_owned(),
            }),
            FakeStep::Emit(AgentSessionEvent::CommandRun {
                command: "cargo test".to_owned(),
            }),
            FakeStep::Emit(AgentSessionEvent::FilesChanged(vec![FileChange {
                path: PathBuf::from("src/lib.rs"),
                change_kind: FileChangeKind::Modified,
            }])),
            FakeStep::Emit(AgentSessionEvent::Summary("patched the test".to_owned())),
            FakeStep::Emit(AgentSessionEvent::Completed {
                summary: "test passes".to_owned(),
            }),
        ];
        let session = FakeConnector::new(script.clone()).launch(&spec()).unwrap();

        for step in script {
            let FakeStep::Emit(expected) = step else {
                unreachable!()
            };
            assert_eq!(recv(&session), expected);
        }
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
        // After Closed the sender is dropped: the channel disconnects.
        assert!(session.events.recv_timeout(RECV_TIMEOUT).is_err());
    }

    #[test]
    fn approve_path_emits_the_approve_branch() {
        let connector = FakeConnector::new(vec![
            FakeStep::Emit(AgentSessionEvent::ApprovalRequested(approval_request(
                "appr-1",
            ))),
            FakeStep::AwaitApproval {
                approval_id: "appr-1".to_owned(),
                then_on_approve: vec![AgentSessionEvent::CommandRun {
                    command: "rm -rf target".to_owned(),
                }],
                then_on_reject: vec![AgentSessionEvent::Failed {
                    error: "user rejected".to_owned(),
                }],
            },
        ]);
        let mut session = connector.launch(&spec()).unwrap();

        assert_eq!(
            recv(&session),
            AgentSessionEvent::ApprovalRequested(approval_request("appr-1"))
        );
        wait_for_pending(&mut session);
        session.control.decide(approve("appr-1")).unwrap();
        assert_eq!(
            recv(&session),
            AgentSessionEvent::CommandRun {
                command: "rm -rf target".to_owned(),
            }
        );
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
    }

    #[test]
    fn reject_path_emits_the_reject_branch() {
        let connector = FakeConnector::new(vec![FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![AgentSessionEvent::CommandRun {
                command: "rm -rf target".to_owned(),
            }],
            then_on_reject: vec![AgentSessionEvent::Failed {
                error: "user rejected".to_owned(),
            }],
        }]);
        let mut session = connector.launch(&spec()).unwrap();

        wait_for_pending(&mut session);
        session
            .control
            .decide(ApprovalDecision {
                approval_id: "appr-1".to_owned(),
                verdict: ApprovalVerdict::Rejected {
                    reason: Some("too risky".to_owned()),
                },
            })
            .unwrap();
        assert_eq!(
            recv(&session),
            AgentSessionEvent::Failed {
                error: "user rejected".to_owned(),
            }
        );
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
    }

    #[test]
    fn decide_with_wrong_approval_id_errors_and_leaves_the_approval_pending() {
        let connector = FakeConnector::new(vec![FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        }]);
        let mut session = connector.launch(&spec()).unwrap();

        wait_for_pending(&mut session);
        assert_eq!(
            session.control.decide(approve("appr-9")),
            Err(AgentControlError::UnknownApproval {
                approval_id: "appr-9".to_owned(),
            })
        );
        // The right id still works afterwards.
        session.control.decide(approve("appr-1")).unwrap();
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
    }

    #[test]
    fn double_decide_on_the_same_approval_errors() {
        let connector = FakeConnector::new(vec![
            FakeStep::AwaitApproval {
                approval_id: "appr-1".to_owned(),
                then_on_approve: vec![],
                then_on_reject: vec![],
            },
            // Keep the worker busy after the decision so the second decide
            // hits either AlreadyDecided (still parked) or NoPendingApproval
            // (already moved on) — never a silent success.
            FakeStep::Sleep(50),
        ]);
        let mut session = connector.launch(&spec()).unwrap();

        wait_for_pending(&mut session);
        session.control.decide(approve("appr-1")).unwrap();
        let second = session.control.decide(approve("appr-1"));
        assert!(
            matches!(
                second,
                Err(AgentControlError::AlreadyDecided { .. })
                    | Err(AgentControlError::NoPendingApproval)
                    | Err(AgentControlError::SessionClosed)
            ),
            "second decide must error, got {second:?}"
        );
    }

    #[test]
    fn decide_without_pending_approval_errors() {
        let connector = FakeConnector::new(vec![FakeStep::Sleep(5_000)]);
        let mut session = connector.launch(&spec()).unwrap();

        assert_eq!(
            session.control.decide(approve("appr-1")),
            Err(AgentControlError::NoPendingApproval)
        );
        session.control.shutdown();
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
    }

    #[test]
    fn shutdown_mid_script_closes_the_receiver_and_kills_is_alive() {
        let connector = FakeConnector::new(vec![
            FakeStep::Emit(AgentSessionEvent::Status(AgentStatus::Running)),
            FakeStep::AwaitApproval {
                approval_id: "appr-1".to_owned(),
                then_on_approve: vec![AgentSessionEvent::Completed {
                    summary: "never reached".to_owned(),
                }],
                then_on_reject: vec![],
            },
        ]);
        let mut session = connector.launch(&spec()).unwrap();

        assert_eq!(
            recv(&session),
            AgentSessionEvent::Status(AgentStatus::Running)
        );
        assert!(session.control.is_alive());
        session.control.shutdown();

        // The remaining script is abandoned: Closed is the next and last event.
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
        assert!(!session.control.is_alive());
        assert!(session.events.recv_timeout(RECV_TIMEOUT).is_err());
    }

    #[test]
    fn decide_after_shutdown_errors_with_session_closed() {
        let connector = FakeConnector::new(vec![FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        }]);
        let mut session = connector.launch(&spec()).unwrap();

        wait_for_pending(&mut session);
        session.control.shutdown();
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
        assert_eq!(
            session.control.decide(approve("appr-1")),
            Err(AgentControlError::SessionClosed)
        );
    }

    #[test]
    fn interrupt_abandons_the_script_and_closes() {
        let connector = FakeConnector::new(vec![FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        }]);
        let mut session = connector.launch(&spec()).unwrap();

        wait_for_pending(&mut session);
        session.control.interrupt().unwrap();
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
        assert!(!session.control.is_alive());
        assert_eq!(
            session.control.interrupt(),
            Err(AgentControlError::SessionClosed)
        );
    }

    #[test]
    fn is_alive_is_true_while_running_and_false_after_completion() {
        let connector = FakeConnector::new(vec![FakeStep::AwaitApproval {
            approval_id: "appr-1".to_owned(),
            then_on_approve: vec![],
            then_on_reject: vec![],
        }]);
        let mut session = connector.launch(&spec()).unwrap();

        wait_for_pending(&mut session);
        assert!(session.control.is_alive());
        session.control.decide(approve("appr-1")).unwrap();

        // alive flips before Closed is emitted, so this ordering is exact.
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
        assert!(!session.control.is_alive());
    }

    #[test]
    fn event_flood_delivers_every_event_in_order() {
        const FLOOD: usize = 10_000;
        let script: Vec<FakeStep> = (0..FLOOD)
            .map(|i| FakeStep::Emit(AgentSessionEvent::OutputChunk(format!("chunk-{i}"))))
            .collect();
        let session = FakeConnector::new(script).launch(&spec()).unwrap();

        for i in 0..FLOOD {
            assert_eq!(
                recv(&session),
                AgentSessionEvent::OutputChunk(format!("chunk-{i}"))
            );
        }
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
    }

    #[test]
    fn empty_objective_is_rejected_at_launch() {
        let connector = FakeConnector::new(vec![]);
        let result = connector.launch(&AgentLaunchSpec::new("   ", "/tmp/project"));
        assert!(matches!(
            result,
            Err(AgentConnectorError::InvalidSpec { .. })
        ));
        assert_eq!(connector.name(), "fake");
    }

    #[test]
    fn connector_and_control_are_object_safe() {
        let connector: Box<dyn AgentConnector> = Box::new(FakeConnector::new(vec![]));
        let session = connector.launch(&spec()).unwrap();
        let _control: &dyn AgentSessionControl = session.control.as_ref();
        assert_eq!(recv(&session), AgentSessionEvent::Closed);
    }
}
