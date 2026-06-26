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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PaneKind {
    Terminal { command: Option<String> },
    Task { intent: TaskPaneIntent },
    Agent { intent: AgentPaneIntent },
    StatusLog { source: StatusLogSource },
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
    pub changed_files: Vec<PathBuf>,
    pub latest_summary: Option<String>,
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
