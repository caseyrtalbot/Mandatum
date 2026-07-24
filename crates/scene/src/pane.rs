//! Pane scenes: identity, chrome flags, and renderable content.

use mandatum_core::{AgentStatus, ArtifactFit, PaneId};
use serde::{Deserialize, Serialize};

use crate::geometry::SceneRect;
use crate::surface::{RasterSurface, TerminalSurface};

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
    /// these lines. Terminal content has no detail lines. The pane's id,
    /// kind, and title are deliberately absent — the border chrome already
    /// states them, and repeating them here read as debug output.
    pub fn detail_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match &self.content {
            PaneContent::Terminal(_) => return Vec::new(),
            PaneContent::Task(task) => {
                lines.push(format!("command: {}", task.command));
                lines.push(format!("cwd: {}", task.cwd_label));
                // "recipe:" is reserved for a real recipe name; ad-hoc runs
                // simply omit the row.
                if let Some(recipe) = &task.recipe_label {
                    lines.push(format!("recipe: {recipe}"));
                }
                match &task.status_label {
                    Some(status) => {
                        lines.push(format!("runtime status: {status}"));
                        // A failed task states its way back on the same
                        // rows that state the failure: the failing command
                        // is already above, the exit status is on the
                        // status line, and this line names the rerun route.
                        if status.contains("failed") {
                            let hint = task
                                .rerun_hint
                                .as_deref()
                                .filter(|hint| !hint.is_empty())
                                .map(|hint| format!("rerun: {hint} · right-click menu"))
                                .unwrap_or_else(|| "rerun: right-click menu".to_owned());
                            lines.push(hint);
                        }
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
                // A failed agent keeps its failure reason and the way back
                // on screen, not just in a transient status line.
                if agent.status_role == AgentStatus::Failed {
                    if let Some(error) = &agent.last_error {
                        lines.push(format!("error: {error}"));
                    }
                    if let Some(hint) = agent
                        .relaunch_hint
                        .as_deref()
                        .filter(|hint| !hint.is_empty())
                    {
                        lines.push(format!("relaunch: {hint} · right-click menu"));
                    }
                }
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
                    for (index, line) in agent.output_tail.iter().enumerate() {
                        lines.push(format!("  {line}"));
                        // A command with nothing after it says so, instead
                        // of leaving a bare "$ cmd" that reads as pending.
                        let next_is_output = agent
                            .output_tail
                            .get(index + 1)
                            .is_some_and(|next| !next.starts_with("$ "));
                        if line.starts_with("$ ") && !next_is_output {
                            lines.push("    (no output)".to_owned());
                        }
                    }
                }
            }
            PaneContent::Artifact(artifact) => {
                lines.push(format!("source: {}", artifact.source_label));
                lines.push(format!("alt: {}", artifact.alt_text));
                match &artifact.state {
                    ArtifactState::Loading => lines.push("preview: loading".to_owned()),
                    ArtifactState::Ready(surface) => lines.push(format!(
                        "preview: ready · {}x{} RGBA8 sRGB",
                        surface.width, surface.height
                    )),
                    ArtifactState::Failed { message } => {
                        lines.push(format!("preview: failed · {message}"));
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
    Artifact,
    StatusLog,
}

impl PaneSceneKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Task => "task",
            Self::Agent => "agent",
            Self::Artifact => "artifact",
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
    Artifact(ArtifactContent),
    Empty(EmptyContent),
}

/// Artifact pane labels plus its app-owned live load state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactContent {
    pub source_label: String,
    pub alt_text: String,
    pub fit: ArtifactFit,
    pub state: ArtifactState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactState {
    Loading,
    Ready(RasterSurface),
    Failed { message: String },
}

/// Task pane content: durable intent labels plus the live runtime view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContent {
    pub command: String,
    /// The resolved working directory the command runs in (intent, pane, or
    /// the project directory) — never an internal "unset".
    pub cwd_label: String,
    /// A real recipe name when the task came from one; `None` for ad-hoc
    /// runs (no row is drawn).
    pub recipe_label: Option<String>,
    /// Live runtime status; `None` when no runtime view exists for the pane.
    pub status_label: Option<String>,
    /// The keyboard route to Rerun task (composed from the live keymap),
    /// shown on failed tasks next to the right-click route.
    pub rerun_hint: Option<String>,
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
    /// Why the session failed, from its `Failed` event (live only). Shown
    /// persistently on a failed pane, not just in the status line.
    pub last_error: Option<String>,
    /// The keyboard route to Start agent (composed from the live keymap),
    /// shown on failed panes as the relaunch affordance.
    pub relaunch_hint: Option<String>,
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
    /// Whether the approval header draws emphasized this frame. The app
    /// alternates it at ~1 Hz off the heartbeat clock (steady `true` under
    /// reduced motion), giving waiting-approval panes one calm pulse; it is
    /// the only motion in the product.
    pub pulse_on: bool,
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
            recipe_label: Some("test".to_owned()),
            status_label,
            rerun_hint: Some("ctrl+p r".to_owned()),
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
                "command: cargo test",
                "cwd: /tmp/project",
                "recipe: test",
                "runtime status: failed: exit 101",
                "rerun: ctrl+p r · right-click menu",
                "output:",
            ]
        );
    }

    // The border chrome already names the pane and its title; the body rows
    // never repeat them, and "recipe:" only appears for a named recipe.
    #[test]
    fn detail_lines_carry_no_pane_id_title_or_adhoc_recipe_rows() {
        let mut content = task_content(Some("running".to_owned()), None);
        content.recipe_label = None;
        let lines = pane(PaneContent::Task(content), PaneSceneKind::Task).detail_lines();
        assert!(
            !lines.iter().any(|line| line.contains("pane-1")
                || line.starts_with("title:")
                || line.starts_with("recipe:")),
            "{lines:?}"
        );
        assert_eq!(lines[0], "command: cargo test");
    }

    // A failed task states its way back: the failing command, the exit
    // status, and the rerun affordance all sit in the metadata rows.
    #[test]
    fn failed_task_detail_lines_carry_the_rerun_affordance() {
        let failed = pane(
            PaneContent::Task(task_content(Some("failed: exit 3".to_owned()), None)),
            PaneSceneKind::Task,
        );
        let lines = failed.detail_lines();
        assert!(lines.contains(&"command: cargo test".to_owned()));
        assert!(lines.contains(&"runtime status: failed: exit 3".to_owned()));
        assert!(lines.contains(&"rerun: ctrl+p r · right-click menu".to_owned()));

        // Without a composed key hint the right-click route still shows.
        let mut content = task_content(Some("failed: exit 3".to_owned()), None);
        content.rerun_hint = None;
        let lines = pane(PaneContent::Task(content), PaneSceneKind::Task).detail_lines();
        assert!(lines.contains(&"rerun: right-click menu".to_owned()));

        // A healthy task never shows the rerun row.
        let running = pane(
            PaneContent::Task(task_content(Some("running".to_owned()), None)),
            PaneSceneKind::Task,
        );
        assert!(
            !running
                .detail_lines()
                .iter()
                .any(|line| line.starts_with("rerun:"))
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
        assert_eq!(with_output.detail_lines().len(), 5);
        assert_eq!(without_output.detail_lines().len(), 5);
        assert_eq!(unavailable.detail_lines().len(), 5);
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
        assert_eq!(lines[0], "cwd: /tmp/project");
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
                last_error: None,
                relaunch_hint: None,
                pending_approval: None,
                output_tail: Vec::new(),
            }),
            PaneSceneKind::Agent,
        );
        let lines = agent.detail_lines();
        assert_eq!(lines[0], "objective: review failing tests");
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
                last_error: None,
                relaunch_hint: None,
                pending_approval: Some(AgentApprovalPrompt {
                    command: "rm -rf target".to_owned(),
                    cwd: "/tmp/project".to_owned(),
                    affected_path: Some("target".to_owned()),
                    risk_label: "high".to_owned(),
                    risk_basis: "removes files (rm)".to_owned(),
                    key_hint: "y approve / n reject".to_owned(),
                    pulse_on: true,
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

    fn agent_content(status_role: AgentStatus, status_label: &str) -> AgentContent {
        AgentContent {
            objective: "fix the failing test".to_owned(),
            status_label: status_label.to_owned(),
            status_role,
            pending_approvals: 0,
            changed_file_count: 0,
            changed_files: Vec::new(),
            latest_summary: None,
            current_action: None,
            last_error: None,
            relaunch_hint: None,
            pending_approval: None,
            output_tail: Vec::new(),
        }
    }

    // A failed agent keeps its failure reason and relaunch route on the
    // pane, not just in the transient status line.
    #[test]
    fn failed_agent_detail_lines_carry_the_error_and_relaunch_affordance() {
        let mut content = agent_content(AgentStatus::Failed, "failed");
        content.last_error = Some("the gated command was rejected".to_owned());
        content.relaunch_hint = Some("ctrl+p g".to_owned());
        let lines = pane(PaneContent::Agent(content), PaneSceneKind::Agent).detail_lines();
        assert!(lines.contains(&"status: failed".to_owned()));
        assert!(lines.contains(&"error: the gated command was rejected".to_owned()));
        assert!(lines.contains(&"relaunch: ctrl+p g · right-click menu".to_owned()));

        // A healthy agent shows neither row.
        let mut content = agent_content(AgentStatus::Running, "running");
        content.last_error = Some("stale".to_owned());
        content.relaunch_hint = Some("ctrl+p g".to_owned());
        let lines = pane(PaneContent::Agent(content), PaneSceneKind::Agent).detail_lines();
        assert!(!lines.iter().any(|line| line.starts_with("error:")));
        assert!(!lines.iter().any(|line| line.starts_with("relaunch:")));
    }

    // A bare "$ cmd" with nothing after it reads as pending; the tail says
    // "(no output)" explicitly for commands that produced none.
    #[test]
    fn output_tail_marks_commands_without_output() {
        let mut content = agent_content(AgentStatus::Complete, "complete");
        content.output_tail = vec![
            "$ cat .flip".to_owned(),
            "$ rm .flip".to_owned(),
            "removed".to_owned(),
            "$ true".to_owned(),
        ];
        let lines = pane(PaneContent::Agent(content), PaneSceneKind::Agent).detail_lines();
        let output_start = lines.iter().position(|line| line == "output:").unwrap();
        assert_eq!(
            &lines[output_start..],
            &[
                "output:",
                "  $ cat .flip",
                "    (no output)",
                "  $ rm .flip",
                "  removed",
                "  $ true",
                "    (no output)",
            ]
        );
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
