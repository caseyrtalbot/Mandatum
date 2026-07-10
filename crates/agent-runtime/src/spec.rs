use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Everything a connector needs to launch an agent session.
///
/// This is intent, not runtime state: it can be built from a durable
/// [`mandatum_core::AgentPaneIntent`] plus workspace context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLaunchSpec {
    /// What the agent is being asked to accomplish.
    pub objective: String,
    /// Working directory the agent operates in.
    pub cwd: PathBuf,
    /// Optional model hint; connectors may ignore it.
    pub model: Option<String>,
    /// Maximum number of agent turns before the backend stops the run.
    /// `None` lets the connector pick its own default.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Which tool classes require user approval before executing.
    pub approval_policy: ApprovalPolicy,
}

impl AgentLaunchSpec {
    /// Spec with the default approval policy (shell commands gate, reads
    /// auto-allowed) and no model hint.
    pub fn new(objective: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            objective: objective.into(),
            cwd: cwd.into(),
            model: None,
            max_turns: None,
            approval_policy: ApprovalPolicy::default(),
        }
    }
}

/// Classes of agent tool use that an approval policy can gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    /// Shell/process execution.
    ShellCommand,
    /// File creation or modification.
    FileWrite,
    /// File or directory reads.
    FileRead,
}

/// Which tool classes require an [`crate::ApprovalRequest`] round-trip
/// before the connector lets the agent proceed.
///
/// The default gates shell commands only; reads are auto-allowed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    /// Tool classes that must pause for a user decision.
    pub gated_classes: Vec<ToolClass>,
}

impl ApprovalPolicy {
    /// Whether the given tool class requires approval under this policy.
    pub fn gates(&self, class: ToolClass) -> bool {
        self.gated_classes.contains(&class)
    }
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            gated_classes: vec![ToolClass::ShellCommand],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_gates_shell_commands_and_auto_allows_reads() {
        let policy = ApprovalPolicy::default();
        assert!(policy.gates(ToolClass::ShellCommand));
        assert!(!policy.gates(ToolClass::FileRead));
        assert!(!policy.gates(ToolClass::FileWrite));
    }
}
