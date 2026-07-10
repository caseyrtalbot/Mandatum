use std::{error::Error, fmt, sync::mpsc::Receiver};

use crate::{approval::ApprovalDecision, events::AgentSessionEvent, spec::AgentLaunchSpec};

/// A backend that can launch agent sessions.
///
/// Object-safe: the app holds connectors as `Box<dyn AgentConnector>` /
/// `&dyn AgentConnector` and treats every backend identically.
pub trait AgentConnector {
    /// Launch a session for the given spec.
    ///
    /// On success the connector has spawned whatever threads it needs and the
    /// returned [`AgentSession`] streams events until
    /// [`AgentSessionEvent::Closed`].
    fn launch(&self, spec: &AgentLaunchSpec) -> Result<AgentSession, AgentConnectorError>;

    /// Stable connector name, for display and diagnostics.
    fn name(&self) -> &str;
}

/// A live agent session: an event stream plus a control handle.
///
/// This is **runtime state** in the sense of the durable-intent law: it holds
/// a channel receiver and a trait object, is deliberately not serializable,
/// and must never be written into workspace persistence. Durable agent state
/// lives in [`mandatum_core::AgentPaneIntent`].
pub struct AgentSession {
    /// Events from the connector's worker thread. Disconnects after
    /// [`AgentSessionEvent::Closed`].
    pub events: Receiver<AgentSessionEvent>,
    /// Control surface for decisions and lifecycle.
    pub control: Box<dyn AgentSessionControl>,
}

/// Control surface for a live session. Object-safe and `Send` so the app can
/// own it on whichever thread drains the events.
pub trait AgentSessionControl: Send {
    /// Deliver a verdict for the pending approval request.
    fn decide(&mut self, decision: ApprovalDecision) -> Result<(), AgentControlError>;

    /// Ask the agent to stop its current work. The session still emits
    /// terminal events ([`AgentSessionEvent::Closed`] at minimum).
    fn interrupt(&mut self) -> Result<(), AgentControlError>;

    /// Tear the session down. Idempotent; safe to call at any time.
    fn shutdown(&mut self);

    /// Whether the connector's worker is still running.
    fn is_alive(&self) -> bool;
}

/// Failure to launch a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentConnectorError {
    /// The spec is unusable (e.g. empty objective).
    InvalidSpec { message: String },
    /// The backend failed to start.
    LaunchFailed { message: String },
}

impl fmt::Display for AgentConnectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec { message } => write!(f, "invalid launch spec: {message}"),
            Self::LaunchFailed { message } => write!(f, "agent launch failed: {message}"),
        }
    }
}

impl Error for AgentConnectorError {}

/// Failure of a control operation on a live session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentControlError {
    /// No approval is currently pending.
    NoPendingApproval,
    /// A decision arrived for an id that is not the pending approval.
    UnknownApproval { approval_id: String },
    /// The pending approval already has a decision queued.
    AlreadyDecided { approval_id: String },
    /// The session's worker has already exited.
    SessionClosed,
}

impl fmt::Display for AgentControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPendingApproval => write!(f, "no approval is pending"),
            Self::UnknownApproval { approval_id } => {
                write!(f, "no pending approval with id {approval_id:?}")
            }
            Self::AlreadyDecided { approval_id } => {
                write!(f, "approval {approval_id:?} already has a decision")
            }
            Self::SessionClosed => write!(f, "the agent session is closed"),
        }
    }
}

impl Error for AgentControlError {}
