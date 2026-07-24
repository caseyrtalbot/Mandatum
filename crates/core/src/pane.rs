use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::PaneId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSpec {
    id: PaneId,
    title: String,
    kind: PaneKind,
    cwd: Option<PathBuf>,
    restart_generation: u64,
}

impl PaneSpec {
    pub fn new(id: PaneId, title: impl Into<String>, kind: PaneKind, cwd: Option<PathBuf>) -> Self {
        Self {
            id,
            title: title.into(),
            kind,
            cwd,
            restart_generation: 0,
        }
    }

    pub fn terminal(id: PaneId, title: impl Into<String>, cwd: Option<PathBuf>) -> Self {
        Self::new(id, title, PaneKind::Terminal { command: None }, cwd)
    }

    pub fn id(&self) -> &PaneId {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn kind(&self) -> &PaneKind {
        &self.kind
    }

    pub fn cwd(&self) -> Option<&PathBuf> {
        self.cwd.as_ref()
    }

    pub fn restart_generation(&self) -> u64 {
        self.restart_generation
    }

    pub fn rename(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn request_restart(&mut self) {
        self.restart_generation += 1;
    }

    /// Mutable access to the durable agent intent, when this is an agent pane.
    pub fn agent_intent_mut(&mut self) -> Option<&mut AgentPaneIntent> {
        match &mut self.kind {
            PaneKind::Agent { intent } => Some(intent),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PaneKind {
    Terminal { command: Option<String> },
    Task { intent: TaskPaneIntent },
    Agent { intent: AgentPaneIntent },
    Artifact { intent: ArtifactPaneIntent },
    StatusLog { source: StatusLogSource },
}

/// Durable intent for one project-local artifact preview.
///
/// The source remains project-relative intent. Decoded pixels, file handles,
/// and decoder state belong to the app's live artifact loader.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPaneIntent {
    pub source: PathBuf,
    pub title: String,
    pub alt_text: String,
    pub fit: ArtifactFit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFit {
    Contain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPaneIntent {
    pub recipe_id: Option<String>,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPaneIntent {
    pub thread_id: Option<String>,
    pub objective: String,
    pub status: AgentStatus,
    pub pending_approvals: u32,
    /// Ids of approvals currently awaiting a decision. Durable so a restart
    /// can show *which* approval was pending; the full request detail
    /// (command, scope, risk) is live runtime state and is never persisted.
    #[serde(default)]
    pub pending_approval_ids: Vec<String>,
    pub changed_files: Vec<PathBuf>,
    pub latest_summary: Option<String>,
    /// Decided approvals, oldest first, so past decisions remain visible
    /// after a restart.
    #[serde(default)]
    pub approval_history: Vec<AgentApprovalRecord>,
}

impl AgentPaneIntent {
    /// Fold "the live session is gone" into the durable intent: in-flight
    /// claims (running, waiting for approval, pending approval ids) are
    /// properties of a live session and must never outlive it. Terminal
    /// states the session already reported stay as they are.
    pub fn detach_live_session(&mut self) {
        if matches!(
            self.status,
            AgentStatus::Running | AgentStatus::WaitingForApproval
        ) {
            self.status = AgentStatus::Unknown;
        }
        self.pending_approvals = 0;
        self.pending_approval_ids.clear();
    }

    /// A fresh draft intent: an objective and nothing else yet.
    pub fn draft(objective: impl Into<String>) -> Self {
        Self {
            thread_id: None,
            objective: objective.into(),
            status: AgentStatus::Draft,
            pending_approvals: 0,
            pending_approval_ids: Vec::new(),
            changed_files: Vec::new(),
            latest_summary: None,
            approval_history: Vec::new(),
        }
    }
}

/// One decided approval, kept in the durable agent intent as execution
/// history. Holds only durable facts (id, verbatim command, verdict) —
/// scope and risk detail belong to the live request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentApprovalRecord {
    pub approval_id: String,
    pub command: String,
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Draft,
    Running,
    WaitingForApproval,
    Blocked,
    Failed,
    Complete,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLogSource {
    Workspace,
    Project,
    Tasks,
    Agents,
}
