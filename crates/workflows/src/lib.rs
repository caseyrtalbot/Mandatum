//! Durable workflow intent helpers.
//!
//! Milestone 1 does not launch tasks or agents. This crate only shapes pane
//! intent that can be handed to `ntw-core` for persistence.

use std::path::PathBuf;

use ntw_core::{AgentPaneIntent, AgentStatus, PaneKind, TaskPaneIntent, TaskStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRecipe {
    pub id: String,
    pub command: String,
    pub cwd: PathBuf,
}

impl TaskRecipe {
    pub fn pane_kind(&self) -> PaneKind {
        PaneKind::Task {
            intent: TaskPaneIntent {
                recipe_id: Some(self.id.clone()),
                command: self.command.clone(),
                cwd: Some(self.cwd.clone()),
                status: TaskStatus::Pending,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThreadSpec {
    pub thread_id: Option<String>,
    pub objective: String,
}

impl AgentThreadSpec {
    pub fn pane_kind(&self) -> PaneKind {
        PaneKind::Agent {
            intent: AgentPaneIntent {
                thread_id: self.thread_id.clone(),
                objective: self.objective.clone(),
                status: AgentStatus::Draft,
                pending_approvals: 0,
                changed_files: Vec::new(),
                latest_summary: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_recipe_creates_durable_pane_intent_only() {
        let recipe = TaskRecipe {
            id: "build".to_owned(),
            command: "cargo build".to_owned(),
            cwd: PathBuf::from("/tmp/project"),
        };

        let kind = recipe.pane_kind();

        assert!(matches!(kind, PaneKind::Task { .. }));
    }

    #[test]
    fn agent_thread_creates_durable_pane_intent_only() {
        let agent = AgentThreadSpec {
            thread_id: Some("thread-1".to_owned()),
            objective: "fix tests".to_owned(),
        };

        let kind = agent.pane_kind();

        assert!(matches!(kind, PaneKind::Agent { .. }));
    }
}
