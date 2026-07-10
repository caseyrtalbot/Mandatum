//! Pane scenes: identity, chrome flags, and renderable content.

use mandatum_core::{AgentStatus, PaneId};
use serde::{Deserialize, Serialize};

use crate::geometry::SceneRect;
use crate::surface::TerminalSurface;

/// One pane ready to draw: durable identity plus resolved geometry, chrome
/// flags, and content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneScene {
    pub id: PaneId,
    pub title: String,
    pub kind: PaneSceneKind,
    pub area: SceneRect,
    pub focused: bool,
    pub floating: bool,
    pub stacked: bool,
    pub zoomed: bool,
    pub content: PaneContent,
}

impl PaneScene {
    /// The text lines a frontend draws above any embedded output surface.
    ///
    /// Owning these here keeps every frontend's line budget consistent: the
    /// scene builder windows a task's output surface to the space left after
    /// these lines. Terminal content has no detail lines.
    pub fn detail_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("{} {}", self.id, self.kind.label()),
            format!("title: {}", self.title),
        ];
        match &self.content {
            PaneContent::Terminal(_) => return Vec::new(),
            PaneContent::Task(task) => {
                lines.push(format!("command: {}", task.command));
                lines.push(format!("cwd: {}", task.cwd_label));
                lines.push(format!("recipe: {}", task.recipe_label));
                match &task.status_label {
                    Some(status) => {
                        lines.push(format!("runtime status: {status}"));
                        if task.output.is_some() {
                            lines.push("output:".to_owned());
                        } else {
                            lines.push("output: no live grid attached".to_owned());
                        }
                    }
                    None => {
                        lines.push("runtime status: unavailable".to_owned());
                        lines.push("output: no live runtime attached".to_owned());
                    }
                }
            }
            PaneContent::Agent(agent) => {
                lines.push(format!("objective: {}", agent.objective));
                lines.push(format!("status: {}", agent.status_label));
                lines.push(format!(
                    "action: {}",
                    agent.current_action.as_deref().unwrap_or("idle")
                ));
                lines.push(format!(
                    "summary: {}",
                    agent.latest_summary.as_deref().unwrap_or("none")
                ));
                match &agent.pending_approval {
                    Some(prompt) => {
                        lines.push(format!("approval required: {}", prompt.command));
                        match &prompt.affected_path {
                            Some(path) => {
                                lines.push(format!("scope: {} -> {}", prompt.cwd, path));
                            }
                            None => lines.push(format!("scope: {}", prompt.cwd)),
                        }
                        lines.push(format!(
                            "risk: {} ({})",
                            prompt.risk_label, prompt.risk_basis
                        ));
                        lines.push(format!("keys: {}", prompt.key_hint));
                    }
                    None => {
                        lines.push(format!("pending approvals: {}", agent.pending_approvals));
                    }
                }
                if agent.changed_files.is_empty() {
                    lines.push("changed files: none".to_owned());
                } else {
                    lines.push(format!("changed files ({}):", agent.changed_file_count));
                    for path in &agent.changed_files {
                        lines.push(format!("  {path}"));
                    }
                }
                if !agent.output_tail.is_empty() {
                    lines.push("output:".to_owned());
                    for line in &agent.output_tail {
                        lines.push(format!("  {line}"));
                    }
                }
            }
            PaneContent::Empty(empty) => {
                lines.push(format!("cwd: {}", empty.cwd_label));
                lines.push(format!("restart generation: {}", empty.restart_generation));
                lines.push("no live PTY grid is attached to this pane".to_owned());
            }
        }
        lines
    }
}

/// The durable pane kind, re-expressed for frontends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneSceneKind {
    Terminal,
    Task,
    Agent,
    StatusLog,
}

impl PaneSceneKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Task => "task",
            Self::Agent => "agent",
            Self::StatusLog => "status",
        }
    }
}

/// What a pane displays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneContent {
    Terminal(TerminalSurface),
    Task(TaskContent),
    Agent(AgentContent),
    Empty(EmptyContent),
}

/// Task pane content: durable intent labels plus the live runtime view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContent {
    pub command: String,
    pub cwd_label: String,
    pub recipe_label: String,
    /// Live runtime status; `None` when no runtime view exists for the pane.
    pub status_label: Option<String>,
    pub output: Option<TerminalSurface>,
}

/// Agent pane content: the durable intent summary plus the live session
/// surface (current action, pending approval detail, output tail). Live
/// fields are empty/`None` when no runtime is attached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentContent {
    pub objective: String,
    pub status_label: String,
    /// Semantic status role, so frontends can theme the status line without
    /// parsing the label text.
    pub status_role: AgentStatus,
    pub pending_approvals: u32,
    /// Total changed files reported so far.
    pub changed_file_count: usize,
    /// The most recent changed files (the builder caps this list, ~10).
    pub changed_files: Vec<String>,
    pub latest_summary: Option<String>,
    /// What the agent is doing right now (live only).
    pub current_action: Option<String>,
    /// Full detail of the approval awaiting a decision (live only).
    pub pending_approval: Option<AgentApprovalPrompt>,
    /// Trailing raw output lines (live only; the builder caps the tail).
    pub output_tail: Vec<String>,
}

/// A gated action awaiting a user verdict, re-expressed for frontends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentApprovalPrompt {
    /// The verbatim command the agent wants to run.
    pub command: String,
    /// Working directory the command would run in.
    pub cwd: String,
    /// Path the action is expected to affect, when known.
    pub affected_path: Option<String>,
    /// Risk band label ("low" / "medium" / "high").
    pub risk_label: String,
    /// Which pattern produced the band.
    pub risk_basis: String,
    /// The decision keys frontends should surface ("y approve / n reject").
    pub key_hint: String,
}

/// A pane with no live content surface attached (a terminal pane before its
/// PTY exists, or a status-log pane).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyContent {
    pub cwd_label: String,
    pub restart_generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(content: PaneContent, kind: PaneSceneKind) -> PaneScene {
        PaneScene {
            id: PaneId::new("pane-1"),
            title: "tests".to_owned(),
            kind,
            area: SceneRect::new(0, 0, 40, 12),
            focused: false,
            floating: false,
            stacked: false,
            zoomed: false,
            content,
        }
    }

    fn task_content(status_label: Option<String>, output: Option<TerminalSurface>) -> TaskContent {
        TaskContent {
            command: "cargo test".to_owned(),
            cwd_label: "/tmp/project".to_owned(),
            recipe_label: "test".to_owned(),
            status_label,
            output,
        }
    }

    #[test]
    fn task_detail_lines_carry_intent_and_runtime_status() {
        let pane = pane(
            PaneContent::Task(task_content(
                Some("failed: exit 101".to_owned()),
                Some(TerminalSurface::default()),
            )),
            PaneSceneKind::Task,
        );
        let lines = pane.detail_lines();
        assert_eq!(
            lines,
            vec![
                "pane-1 task",
                "title: tests",
                "command: cargo test",
                "cwd: /tmp/project",
                "recipe: test",
                "runtime status: failed: exit 101",
                "output:",
            ]
        );
    }

    #[test]
    fn task_detail_line_count_is_stable_across_runtime_states() {
        // The scene builder windows a task's output surface to the space left
        // after these lines, so the count must not depend on whether the
        // output surface is attached yet.
        let with_output = pane(
            PaneContent::Task(task_content(
                Some("running".to_owned()),
                Some(TerminalSurface::default()),
            )),
            PaneSceneKind::Task,
        );
        let without_output = pane(
            PaneContent::Task(task_content(Some("running".to_owned()), None)),
            PaneSceneKind::Task,
        );
        let unavailable = pane(
            PaneContent::Task(task_content(None, None)),
            PaneSceneKind::Task,
        );
        assert_eq!(with_output.detail_lines().len(), 7);
        assert_eq!(without_output.detail_lines().len(), 7);
        assert_eq!(unavailable.detail_lines().len(), 7);
        assert!(
            unavailable
                .detail_lines()
                .contains(&"runtime status: unavailable".to_owned())
        );
        assert!(
            unavailable
                .detail_lines()
                .contains(&"output: no live runtime attached".to_owned())
        );
    }

    #[test]
    fn empty_and_agent_detail_lines_describe_the_pane() {
        let empty = pane(
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp/project".to_owned(),
                restart_generation: 2,
            }),
            PaneSceneKind::Terminal,
        );
        let lines = empty.detail_lines();
        assert_eq!(lines[0], "pane-1 terminal");
        assert!(lines.contains(&"restart generation: 2".to_owned()));
        assert!(lines.contains(&"no live PTY grid is attached to this pane".to_owned()));

        let agent = pane(
            PaneContent::Agent(AgentContent {
                objective: "review failing tests".to_owned(),
                status_label: "blocked".to_owned(),
                status_role: AgentStatus::Blocked,
                pending_approvals: 1,
                changed_file_count: 0,
                changed_files: Vec::new(),
                latest_summary: None,
                current_action: None,
                pending_approval: None,
                output_tail: Vec::new(),
            }),
            PaneSceneKind::Agent,
        );
        let lines = agent.detail_lines();
        assert_eq!(lines[0], "pane-1 agent");
        assert!(lines.contains(&"objective: review failing tests".to_owned()));
        assert!(lines.contains(&"status: blocked".to_owned()));
        assert!(lines.contains(&"action: idle".to_owned()));
        assert!(lines.contains(&"pending approvals: 1".to_owned()));
        assert!(lines.contains(&"changed files: none".to_owned()));
        assert!(lines.contains(&"summary: none".to_owned()));
    }

    #[test]
    fn waiting_agent_detail_lines_carry_the_approval_block_and_live_surface() {
        let agent = pane(
            PaneContent::Agent(AgentContent {
                objective: "fix the failing test".to_owned(),
                status_label: "waiting for approval".to_owned(),
                status_role: AgentStatus::WaitingForApproval,
                pending_approvals: 1,
                changed_file_count: 12,
                changed_files: vec!["src/lib.rs".to_owned(), "src/x.rs".to_owned()],
                latest_summary: Some("patched the test".to_owned()),
                current_action: Some("running cargo test".to_owned()),
                pending_approval: Some(AgentApprovalPrompt {
                    command: "rm -rf target".to_owned(),
                    cwd: "/tmp/project".to_owned(),
                    affected_path: Some("target".to_owned()),
                    risk_label: "high".to_owned(),
                    risk_basis: "removes files (rm)".to_owned(),
                    key_hint: "y approve / n reject".to_owned(),
                }),
                output_tail: vec!["$ cargo test".to_owned(), "1 test failed".to_owned()],
            }),
            PaneSceneKind::Agent,
        );
        let lines = agent.detail_lines();
        assert!(lines.contains(&"status: waiting for approval".to_owned()));
        assert!(lines.contains(&"action: running cargo test".to_owned()));
        assert!(lines.contains(&"approval required: rm -rf target".to_owned()));
        assert!(lines.contains(&"scope: /tmp/project -> target".to_owned()));
        assert!(lines.contains(&"risk: high (removes files (rm))".to_owned()));
        assert!(lines.contains(&"keys: y approve / n reject".to_owned()));
        assert!(lines.contains(&"changed files (12):".to_owned()));
        assert!(lines.contains(&"  src/lib.rs".to_owned()));
        assert!(lines.contains(&"output:".to_owned()));
        assert!(lines.contains(&"  1 test failed".to_owned()));
        // No stale "pending approvals" counter next to the full block.
        assert!(!lines.contains(&"pending approvals: 1".to_owned()));
    }

    #[test]
    fn terminal_content_has_no_detail_lines() {
        let terminal = pane(
            PaneContent::Terminal(TerminalSurface::default()),
            PaneSceneKind::Terminal,
        );
        assert!(terminal.detail_lines().is_empty());
    }
}
