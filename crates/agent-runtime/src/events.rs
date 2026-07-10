use std::path::PathBuf;

use mandatum_core::AgentStatus;
use serde::{Deserialize, Serialize};

use crate::approval::ApprovalRequest;

/// How a file changed during an agent session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
}

/// One file the agent touched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// Path, relative to the session cwd when the connector can express it.
    pub path: PathBuf,
    /// What happened to it.
    pub change_kind: FileChangeKind,
}

/// Everything a connector can tell the workstation about a live session.
///
/// These are **runtime events**: the app layer folds the durable subset
/// (status, summary, changed files, approval counts) into
/// [`mandatum_core::AgentPaneIntent`]; the events themselves are never
/// persisted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionEvent {
    /// The agent's lifecycle status changed.
    Status(AgentStatus),
    /// The agent took a discrete action worth surfacing (tool use, step).
    Action { description: String },
    /// The agent produced or updated a progress summary.
    Summary(String),
    /// Raw output text from the agent stream.
    OutputChunk(String),
    /// The agent ran a command (already-approved or auto-allowed).
    CommandRun { command: String },
    /// Files changed since the last report.
    FilesChanged(Vec<FileChange>),
    /// A gated action needs a user verdict before it can run.
    ApprovalRequested(ApprovalRequest),
    /// The session finished successfully.
    Completed { summary: String },
    /// The session finished unsuccessfully.
    Failed { error: String },
    /// Terminal event: no further events will arrive on this session.
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{ApprovalScope, RiskAssessment, RiskLevel};

    #[test]
    fn events_round_trip_through_json() {
        let events = vec![
            AgentSessionEvent::Status(AgentStatus::Running),
            AgentSessionEvent::Action {
                description: "reading crates/core/src/pane.rs".to_owned(),
            },
            AgentSessionEvent::Summary("exploring the pane model".to_owned()),
            AgentSessionEvent::OutputChunk("…".to_owned()),
            AgentSessionEvent::CommandRun {
                command: "cargo check".to_owned(),
            },
            AgentSessionEvent::FilesChanged(vec![FileChange {
                path: PathBuf::from("src/lib.rs"),
                change_kind: FileChangeKind::Modified,
            }]),
            AgentSessionEvent::ApprovalRequested(ApprovalRequest {
                approval_id: "appr-1".to_owned(),
                command: "rm -rf target".to_owned(),
                scope: ApprovalScope {
                    cwd: PathBuf::from("/tmp/project"),
                    affected_path: Some(PathBuf::from("target")),
                },
                risk: RiskAssessment {
                    level: RiskLevel::High,
                    basis: "removes files (rm)".to_owned(),
                },
            }),
            AgentSessionEvent::Completed {
                summary: "done".to_owned(),
            },
            AgentSessionEvent::Failed {
                error: "boom".to_owned(),
            },
            AgentSessionEvent::Closed,
        ];

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: AgentSessionEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, event);
        }
    }
}
