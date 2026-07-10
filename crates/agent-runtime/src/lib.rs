//! Agent runtime contract for Mandatum.
//!
//! This crate owns the shape of the conversation between the workstation and
//! an agent connector: how an agent is launched ([`AgentLaunchSpec`]), what a
//! live session looks like ([`AgentSession`]), what events it streams
//! ([`AgentSessionEvent`]), and how approval-gated actions are decided
//! ([`ApprovalRequest`] / [`ApprovalDecision`]).
//!
//! Architecture laws upheld here:
//!
//! - No async runtime. Connectors run on OS threads and deliver events over
//!   `std::sync::mpsc` channels, mirroring the PTY runtime in the app crate.
//! - Everything in an [`AgentSession`] is **live runtime state**. It is never
//!   serialized; durable agent intent lives in
//!   [`mandatum_core::AgentPaneIntent`] and is updated from these events by
//!   the app layer.
//! - Both [`AgentConnector`] and [`AgentSessionControl`] are object-safe so
//!   the app can hold heterogeneous connectors behind trait objects.
//!
//! [`FakeConnector`] provides a deterministic scripted connector for unit
//! tests and demos, covering happy paths and pathological flows
//! (double-decide, decide-after-shutdown, event floods).

mod approval;
pub mod bridge_protocol;
mod claude;
mod connector;
mod events;
mod fake;
mod spec;

pub use approval::{
    ApprovalDecision, ApprovalRequest, ApprovalScope, ApprovalVerdict, RiskAssessment, RiskLevel,
    assess_command_risk,
};
pub use claude::{ClaudeCliConnector, ClaudeConnectorConfig};
pub use connector::{
    AgentConnector, AgentConnectorError, AgentControlError, AgentSession, AgentSessionControl,
};
pub use events::{AgentSessionEvent, FileChange, FileChangeKind};
pub use fake::{FakeConnector, FakeStep};
pub use spec::{AgentLaunchSpec, ApprovalPolicy, ToolClass};
