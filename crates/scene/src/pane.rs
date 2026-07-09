//! Pane scenes: identity, chrome flags, and renderable content.

use mandatum_core::PaneId;
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
                lines.push(format!("pending approvals: {}", agent.pending_approvals));
                lines.push(format!("changed files: {}", agent.changed_files));
                lines.push(format!(
                    "summary: {}",
                    agent.latest_summary.as_deref().unwrap_or("none")
                ));
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

/// Agent pane content, summarized from the durable agent intent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentContent {
    pub objective: String,
    pub status_label: String,
    pub pending_approvals: u32,
    pub changed_files: usize,
    pub latest_summary: Option<String>,
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
                pending_approvals: 1,
                changed_files: 3,
                latest_summary: None,
            }),
            PaneSceneKind::Agent,
        );
        let lines = agent.detail_lines();
        assert_eq!(lines[0], "pane-1 agent");
        assert!(lines.contains(&"objective: review failing tests".to_owned()));
        assert!(lines.contains(&"pending approvals: 1".to_owned()));
        assert!(lines.contains(&"changed files: 3".to_owned()));
        assert!(lines.contains(&"summary: none".to_owned()));
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
