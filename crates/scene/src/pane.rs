//! Pane scenes: identity, chrome flags, and renderable content.

use mandatum_core::{AgentStatus, ArtifactFit, PaneId};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::geometry::SceneRect;
use crate::surface::{RasterSurface, TerminalSurface};
use crate::workspace::PresentationTone;

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
    /// Content-change hint for caching renderers, scoped to [`Self::content`]
    /// alone.
    ///
    /// Contract: within one producing app state, two frames that carry the
    /// same `(id, content_revision)` pair carry an identical `content` value,
    /// so a renderer may reuse work derived purely from that pane's content.
    /// The reverse is not promised — a producer may bump the revision even
    /// though the content happens to be unchanged, and the safe direction is
    /// always a rebuild.
    ///
    /// This is a hint, never a requirement: `&WorkspaceScene` alone still
    /// paints a complete frame, and a renderer that ignores this field must
    /// render identically. It says nothing about the pane's other fields
    /// (`title`, `area`, `focused`, chrome flags) or about theme; caches
    /// keyed on this value must key on those inputs separately.
    #[serde(default)]
    pub content_revision: u64,
    pub content: PaneContent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaneBadgeKind {
    Terminal,
    Task,
    Agent,
    Artifact,
    Status,
    Floating,
    Stacked,
    Zoomed,
    Copy,
    Approval,
}

impl PaneBadgeKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Task => "task",
            Self::Agent => "agent",
            Self::Artifact => "artifact",
            Self::Status => "status",
            Self::Floating => "floating",
            Self::Stacked => "stack",
            Self::Zoomed => "zoom",
            Self::Copy => "copy",
            Self::Approval => "approval",
        }
    }
}

impl PaneScene {
    /// Typed title-rail badges in stable display order. The scene owns this
    /// derivation so native adapters never parse title text or pane content.
    pub fn badge_kinds(&self) -> Vec<PaneBadgeKind> {
        let mut badges = vec![match self.kind {
            PaneSceneKind::Terminal => PaneBadgeKind::Terminal,
            PaneSceneKind::Task => PaneBadgeKind::Task,
            PaneSceneKind::Agent => PaneBadgeKind::Agent,
            PaneSceneKind::Artifact => PaneBadgeKind::Artifact,
            PaneSceneKind::StatusLog => PaneBadgeKind::Status,
        }];
        if self.floating {
            badges.push(PaneBadgeKind::Floating);
        }
        if self.stacked {
            badges.push(PaneBadgeKind::Stacked);
        }
        if self.zoomed {
            badges.push(PaneBadgeKind::Zoomed);
        }
        if matches!(&self.content, PaneContent::Terminal(surface) if surface.in_copy_mode()) {
            badges.push(PaneBadgeKind::Copy);
        }
        if matches!(&self.content, PaneContent::Agent(agent) if agent.pending_approval.is_some()) {
            badges.push(PaneBadgeKind::Approval);
        }
        badges
    }

    /// Visible badge cells in stable display order, right-aligned inside the
    /// pane's title rail. This is shared by CellProgram and logical
    /// presentation so material pills never claim blank terminal cells.
    pub fn badge_rects(&self) -> Vec<(PaneBadgeKind, SceneRect)> {
        let rail = SceneRect::new(
            self.area.x.saturating_add(1),
            self.area.y,
            self.area.width.saturating_sub(2),
            self.area.height.min(1),
        );
        if rail.is_empty() {
            return Vec::new();
        }
        let mut right = rail.right();
        let mut placed = Vec::new();
        for kind in self.badge_kinds().into_iter().rev() {
            let width = u16::try_from(kind.label().len())
                .unwrap_or(u16::MAX)
                .saturating_add(2);
            if width > right.saturating_sub(rail.x) {
                continue;
            }
            let x = right.saturating_sub(width);
            placed.push((kind, SceneRect::new(x, rail.y, width, 1)));
            right = x.saturating_sub(1).max(rail.x);
        }
        placed.reverse();
        placed
    }

    /// The text lines a frontend draws above any embedded output surface.
    ///
    /// Owning these here keeps every frontend's line budget consistent: the
    /// scene builder windows a task's output surface to the space left after
    /// these lines. Terminal content has no detail lines. The pane's id,
    /// kind, and title are deliberately absent — the border chrome already
    /// states them, and repeating them here read as debug output.
    pub fn detail_lines(&self) -> Vec<String> {
        self.workflow_rows()
            .into_iter()
            .map(|row| row.text)
            .collect()
    }

    /// Typed workflow projection used by both the terminal fallback and the
    /// richer native presentation. Renderers style these roles directly;
    /// they never parse labels or string prefixes to rediscover meaning.
    pub fn workflow_rows(&self) -> Vec<WorkflowRow> {
        match &self.content {
            PaneContent::Terminal(_) => Vec::new(),
            PaneContent::Task(task) => task.workflow_rows(),
            PaneContent::Agent(agent) => agent.workflow_rows(),
            PaneContent::Artifact(artifact) => artifact.workflow_rows(),
            PaneContent::Empty(empty) => vec![
                WorkflowRow::new(
                    WorkflowNodePart::Metadata,
                    WorkflowRowRole::Metadata,
                    PresentationTone::Neutral,
                    format!("cwd: {}", bounded_workflow_fragment(&empty.cwd_label)),
                ),
                WorkflowRow::new(
                    WorkflowNodePart::Metadata,
                    WorkflowRowRole::Metadata,
                    PresentationTone::Neutral,
                    format!("restart generation: {}", empty.restart_generation),
                ),
                WorkflowRow::new(
                    WorkflowNodePart::Status,
                    WorkflowRowRole::Status,
                    PresentationTone::Neutral,
                    "no live PTY grid is attached to this pane",
                ),
            ],
        }
    }

    /// Exact terminal-fallback metadata budget above an embedded task console
    /// or artifact canvas. Layout and paint share this one typed projection.
    pub fn terminal_fallback_row_count(&self) -> usize {
        self.workflow_rows().len()
    }

    /// Compact native badge projected over the matching fallback row.
    pub fn workflow_status_badge(&self) -> Option<WorkflowStatusBadge> {
        match &self.content {
            PaneContent::Task(task) => Some(WorkflowStatusBadge {
                row: 0,
                label: task
                    .status_label
                    .as_deref()
                    .map(bounded_workflow_fragment)
                    .unwrap_or_else(|| "unavailable".to_owned()),
                tone: task.status_role.tone(),
            }),
            PaneContent::Agent(agent) => Some(WorkflowStatusBadge {
                row: 1,
                label: format!("status: {}", bounded_workflow_fragment(&agent.status_label)),
                tone: agent_status_tone(&agent.status_role),
            }),
            PaneContent::Artifact(artifact) => match &artifact.state {
                ArtifactState::Loading => Some(WorkflowStatusBadge {
                    row: 4,
                    label: "preview: loading".to_owned(),
                    tone: PresentationTone::Running,
                }),
                ArtifactState::Ready(_) => Some(WorkflowStatusBadge {
                    row: 4,
                    label: "preview: ready".to_owned(),
                    tone: PresentationTone::Complete,
                }),
                ArtifactState::Failed { .. } => None,
            },
            PaneContent::Terminal(_) | PaneContent::Empty(_) => None,
        }
    }
}

/// Stable semantic part for one workflow region inside a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkflowNodePart {
    Heading,
    Status,
    StatusText,
    Metadata,
    Failure,
    Action,
    Summary,
    Approval,
    ChangedFiles,
    Console,
    ArtifactInspector,
    ArtifactState,
    ArtifactCanvas,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStatusBadge {
    pub row: usize,
    pub label: String,
    pub tone: PresentationTone,
}

/// Renderer-neutral visual role for one workflow row or region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowRowRole {
    Heading,
    Status,
    Metadata,
    Callout,
    List,
    Console,
    ArtifactInspector,
}

/// One complete terminal-fallback row with typed native meaning.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRow {
    pub part: WorkflowNodePart,
    pub role: WorkflowRowRole,
    pub tone: PresentationTone,
    pub text: String,
}

impl WorkflowRow {
    fn new(
        part: WorkflowNodePart,
        role: WorkflowRowRole,
        tone: PresentationTone,
        text: impl Into<String>,
    ) -> Self {
        Self {
            part,
            role,
            tone,
            text: bounded_workflow_text(text.into()),
        }
    }
}

const MAX_WORKFLOW_ROW_GRAPHEMES: usize = 1_024;

fn bounded_workflow_text(text: String) -> String {
    let mut graphemes = text.graphemes(true);
    let bounded = graphemes
        .by_ref()
        .take(MAX_WORKFLOW_ROW_GRAPHEMES)
        .collect::<String>();
    if graphemes.next().is_some() {
        format!("{bounded} … [truncated]")
    } else {
        bounded
    }
}

fn bounded_workflow_fragment(text: &str) -> String {
    bounded_workflow_text(
        text.graphemes(true)
            .take(MAX_WORKFLOW_ROW_GRAPHEMES + 1)
            .collect(),
    )
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

impl ArtifactContent {
    fn workflow_rows(&self) -> Vec<WorkflowRow> {
        let (tone, state, dimensions, revision) = match &self.state {
            ArtifactState::Loading => (
                PresentationTone::Running,
                "loading".to_owned(),
                "unavailable".to_owned(),
                "pending".to_owned(),
            ),
            ArtifactState::Ready(surface) => (
                PresentationTone::Complete,
                "ready".to_owned(),
                format!("{}x{} RGBA8 sRGB", surface.width, surface.height),
                surface.revision.to_string(),
            ),
            ArtifactState::Failed { message } => (
                PresentationTone::Failure,
                format!("failed · {}", bounded_workflow_fragment(message)),
                "unavailable".to_owned(),
                "unavailable".to_owned(),
            ),
        };
        vec![
            WorkflowRow::new(
                WorkflowNodePart::ArtifactInspector,
                WorkflowRowRole::ArtifactInspector,
                PresentationTone::Neutral,
                format!("source: {}", bounded_workflow_fragment(&self.source_label)),
            ),
            WorkflowRow::new(
                WorkflowNodePart::ArtifactInspector,
                WorkflowRowRole::ArtifactInspector,
                PresentationTone::Neutral,
                format!("alt: {}", bounded_workflow_fragment(&self.alt_text)),
            ),
            WorkflowRow::new(
                WorkflowNodePart::ArtifactInspector,
                WorkflowRowRole::ArtifactInspector,
                PresentationTone::Neutral,
                format!("dimensions: {dimensions}"),
            ),
            WorkflowRow::new(
                WorkflowNodePart::ArtifactInspector,
                WorkflowRowRole::ArtifactInspector,
                PresentationTone::Neutral,
                format!("revision: {revision}"),
            ),
            WorkflowRow::new(
                WorkflowNodePart::ArtifactState,
                if tone == PresentationTone::Failure {
                    WorkflowRowRole::Callout
                } else {
                    WorkflowRowRole::Metadata
                },
                tone,
                format!("preview: {state}"),
            ),
        ]
    }
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
    /// Typed status used by presentation adapters. The label remains the
    /// exact user-facing terminal fallback.
    pub status_role: TaskStatusRole,
    /// The keyboard route to Rerun task (composed from the live keymap),
    /// shown on failed tasks next to the right-click route.
    pub rerun_hint: Option<String>,
    pub output: Option<TerminalSurface>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatusRole {
    Detached,
    Waiting,
    Running,
    Succeeded,
    Diagnostic,
    Failed,
}

impl TaskStatusRole {
    pub const fn tone(self) -> PresentationTone {
        match self {
            Self::Detached => PresentationTone::Neutral,
            Self::Waiting | Self::Diagnostic => PresentationTone::Waiting,
            Self::Running => PresentationTone::Running,
            Self::Succeeded => PresentationTone::Complete,
            Self::Failed => PresentationTone::Failure,
        }
    }
}

impl TaskContent {
    fn workflow_rows(&self) -> Vec<WorkflowRow> {
        let status = self.status_label.as_deref().unwrap_or("unavailable");
        let mut rows = vec![
            WorkflowRow::new(
                WorkflowNodePart::Heading,
                WorkflowRowRole::Heading,
                self.status_role.tone(),
                format!(
                    "{} · {}",
                    bounded_workflow_fragment(status),
                    bounded_workflow_fragment(&self.command)
                ),
            ),
            WorkflowRow::new(
                WorkflowNodePart::Metadata,
                WorkflowRowRole::Metadata,
                PresentationTone::Neutral,
                format!("cwd: {}", bounded_workflow_fragment(&self.cwd_label)),
            ),
        ];
        if let Some(recipe) = &self.recipe_label {
            rows.push(WorkflowRow::new(
                WorkflowNodePart::Metadata,
                WorkflowRowRole::Metadata,
                PresentationTone::Neutral,
                format!("recipe: {}", bounded_workflow_fragment(recipe)),
            ));
        }
        if self.status_role == TaskStatusRole::Failed {
            let route = self
                .rerun_hint
                .as_deref()
                .filter(|hint| !hint.is_empty())
                .map(|hint| format!("{} · right-click menu", bounded_workflow_fragment(hint)))
                .unwrap_or_else(|| "right-click menu".to_owned());
            rows.push(WorkflowRow::new(
                WorkflowNodePart::Failure,
                WorkflowRowRole::Callout,
                PresentationTone::Failure,
                format!(
                    "failure: {} · rerun: {route}",
                    bounded_workflow_fragment(status)
                ),
            ));
        }
        rows.push(WorkflowRow::new(
            WorkflowNodePart::Console,
            WorkflowRowRole::Console,
            PresentationTone::Neutral,
            if self.output.is_some() {
                "output:".to_owned()
            } else if self.status_label.is_some() {
                "output: no live grid attached".to_owned()
            } else {
                "output: no live runtime attached".to_owned()
            },
        ));
        rows
    }
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

impl AgentContent {
    fn workflow_rows(&self) -> Vec<WorkflowRow> {
        let status_tone = agent_status_tone(&self.status_role);
        let mut rows = vec![
            WorkflowRow::new(
                WorkflowNodePart::Heading,
                WorkflowRowRole::Heading,
                PresentationTone::AgentIdentity,
                format!("objective: {}", bounded_workflow_fragment(&self.objective)),
            ),
            WorkflowRow::new(
                WorkflowNodePart::StatusText,
                WorkflowRowRole::Metadata,
                status_tone,
                format!("status: {}", bounded_workflow_fragment(&self.status_label)),
            ),
        ];
        if self.status_role == AgentStatus::Failed {
            if let Some(error) = &self.last_error {
                rows.push(WorkflowRow::new(
                    WorkflowNodePart::Failure,
                    WorkflowRowRole::Callout,
                    PresentationTone::Failure,
                    format!("error: {}", bounded_workflow_fragment(error)),
                ));
            }
            if let Some(hint) = self
                .relaunch_hint
                .as_deref()
                .filter(|hint| !hint.is_empty())
            {
                rows.push(WorkflowRow::new(
                    WorkflowNodePart::Failure,
                    WorkflowRowRole::Callout,
                    PresentationTone::Failure,
                    format!(
                        "relaunch: {} · right-click menu",
                        bounded_workflow_fragment(hint)
                    ),
                ));
            }
        }
        rows.extend([
            WorkflowRow::new(
                WorkflowNodePart::Action,
                WorkflowRowRole::Metadata,
                PresentationTone::Neutral,
                format!(
                    "action: {}",
                    bounded_workflow_fragment(self.current_action.as_deref().unwrap_or("idle"))
                ),
            ),
            WorkflowRow::new(
                WorkflowNodePart::Summary,
                WorkflowRowRole::Metadata,
                PresentationTone::Neutral,
                format!(
                    "summary: {}",
                    bounded_workflow_fragment(self.latest_summary.as_deref().unwrap_or("none"))
                ),
            ),
        ]);
        match &self.pending_approval {
            Some(prompt) => {
                rows.push(WorkflowRow::new(
                    WorkflowNodePart::Approval,
                    WorkflowRowRole::Callout,
                    PresentationTone::Waiting,
                    format!(
                        "approval required: {}",
                        bounded_workflow_fragment(&prompt.command)
                    ),
                ));
                rows.push(WorkflowRow::new(
                    WorkflowNodePart::Approval,
                    WorkflowRowRole::Callout,
                    PresentationTone::Waiting,
                    match &prompt.affected_path {
                        Some(path) => format!(
                            "scope: {} -> {}",
                            bounded_workflow_fragment(&prompt.cwd),
                            bounded_workflow_fragment(path)
                        ),
                        None => format!("scope: {}", bounded_workflow_fragment(&prompt.cwd)),
                    },
                ));
                rows.push(WorkflowRow::new(
                    WorkflowNodePart::Approval,
                    WorkflowRowRole::Callout,
                    PresentationTone::Waiting,
                    format!(
                        "risk: {} ({})",
                        bounded_workflow_fragment(&prompt.risk_label),
                        bounded_workflow_fragment(&prompt.risk_basis)
                    ),
                ));
                rows.push(WorkflowRow::new(
                    WorkflowNodePart::Approval,
                    WorkflowRowRole::Callout,
                    PresentationTone::Waiting,
                    format!("keys: {}", bounded_workflow_fragment(&prompt.key_hint)),
                ));
            }
            None => rows.push(WorkflowRow::new(
                WorkflowNodePart::Approval,
                WorkflowRowRole::Metadata,
                PresentationTone::Neutral,
                format!("pending approvals: {}", self.pending_approvals),
            )),
        }
        if self.changed_files.is_empty() {
            rows.push(WorkflowRow::new(
                WorkflowNodePart::ChangedFiles,
                WorkflowRowRole::List,
                PresentationTone::Neutral,
                "changed files: none",
            ));
        } else {
            rows.push(WorkflowRow::new(
                WorkflowNodePart::ChangedFiles,
                WorkflowRowRole::List,
                PresentationTone::Neutral,
                format!("changed files ({}):", self.changed_file_count),
            ));
            rows.extend(self.changed_files.iter().map(|path| {
                WorkflowRow::new(
                    WorkflowNodePart::ChangedFiles,
                    WorkflowRowRole::List,
                    PresentationTone::Neutral,
                    format!("  {}", bounded_workflow_fragment(path)),
                )
            }));
        }
        if !self.output_tail.is_empty() {
            rows.push(WorkflowRow::new(
                WorkflowNodePart::Console,
                WorkflowRowRole::Console,
                PresentationTone::Neutral,
                "output:",
            ));
            const MAX_OUTPUT_ROWS: usize = 64;
            let skipped = self.output_tail.len().saturating_sub(MAX_OUTPUT_ROWS);
            if skipped > 0 {
                rows.push(WorkflowRow::new(
                    WorkflowNodePart::Console,
                    WorkflowRowRole::Console,
                    PresentationTone::Neutral,
                    format!("  … {skipped} earlier output lines omitted"),
                ));
            }
            for line in self.output_tail.iter().skip(skipped) {
                rows.push(WorkflowRow::new(
                    WorkflowNodePart::Console,
                    WorkflowRowRole::Console,
                    PresentationTone::Neutral,
                    format!("  {}", bounded_workflow_fragment(line)),
                ));
            }
        }
        rows
    }
}

fn agent_status_tone(status: &AgentStatus) -> PresentationTone {
    match status {
        AgentStatus::Running => PresentationTone::Running,
        AgentStatus::WaitingForApproval | AgentStatus::Blocked => PresentationTone::Waiting,
        AgentStatus::Failed => PresentationTone::Failure,
        AgentStatus::Complete => PresentationTone::Complete,
        AgentStatus::Draft | AgentStatus::Unknown => PresentationTone::Neutral,
    }
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
    /// Waiting approvals keep this true as a static non-motion emphasis.
    /// Native presentation may additionally receive one typed
    /// `ApprovalArrival` transition target when the request first appears.
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
            content_revision: 0,
            content,
        }
    }

    fn task_content(status_label: Option<String>, output: Option<TerminalSurface>) -> TaskContent {
        let status_role = match status_label.as_deref() {
            Some(status) if status.starts_with("failed") => TaskStatusRole::Failed,
            Some(status) if status.starts_with("succeeded") => TaskStatusRole::Succeeded,
            Some(_) => TaskStatusRole::Running,
            None => TaskStatusRole::Detached,
        };
        TaskContent {
            command: "cargo test".to_owned(),
            cwd_label: "/tmp/project".to_owned(),
            recipe_label: Some("test".to_owned()),
            status_label,
            status_role,
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
                "failed: exit 101 · cargo test",
                "cwd: /tmp/project",
                "recipe: test",
                "failure: failed: exit 101 · rerun: ctrl+p r · right-click menu",
                "output:",
            ]
        );
    }

    #[test]
    fn pane_badges_are_typed_and_derived_in_stable_display_order() {
        let terminal = TerminalSurface {
            copy_cursor: Some(crate::SurfacePosition::new(0, 0)),
            ..TerminalSurface::default()
        };
        let mut pane = pane(PaneContent::Terminal(terminal), PaneSceneKind::Terminal);
        pane.floating = true;
        pane.stacked = true;
        pane.zoomed = true;

        assert_eq!(
            pane.badge_kinds(),
            vec![
                PaneBadgeKind::Terminal,
                PaneBadgeKind::Floating,
                PaneBadgeKind::Stacked,
                PaneBadgeKind::Zoomed,
                PaneBadgeKind::Copy,
            ]
        );
        assert_eq!(PaneBadgeKind::Floating.label(), "floating");
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
        assert_eq!(lines[0], "running · cargo test");
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
        assert!(lines.contains(&"failed: exit 3 · cargo test".to_owned()));
        assert!(
            lines.contains(
                &"failure: failed: exit 3 · rerun: ctrl+p r · right-click menu".to_owned()
            )
        );

        // Without a composed key hint the right-click route still shows.
        let mut content = task_content(Some("failed: exit 3".to_owned()), None);
        content.rerun_hint = None;
        let lines = pane(PaneContent::Task(content), PaneSceneKind::Task).detail_lines();
        assert!(lines.contains(&"failure: failed: exit 3 · rerun: right-click menu".to_owned()));

        // A healthy task never shows the rerun row.
        let running = pane(
            PaneContent::Task(task_content(Some("running".to_owned()), None)),
            PaneSceneKind::Task,
        );
        assert!(
            !running
                .detail_lines()
                .iter()
                .any(|line| line.starts_with("failure:"))
        );
    }

    #[test]
    fn task_detail_line_count_is_stable_across_runtime_attachment() {
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
        assert_eq!(with_output.detail_lines().len(), 4);
        assert_eq!(without_output.detail_lines().len(), 4);
        assert_eq!(unavailable.detail_lines().len(), 4);
        assert!(
            unavailable
                .detail_lines()
                .contains(&"unavailable · cargo test".to_owned())
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

    #[test]
    fn output_tail_preserves_raw_lines_without_prompt_inference() {
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
                "  $ rm .flip",
                "  removed",
                "  $ true",
            ]
        );
    }

    // The revision is a pure hint layered onto the scene contract: scenes
    // captured before the field existed still deserialize (revision 0), and
    // the value round-trips so consumers on the far side of a serialization
    // boundary see the producer's revision, not a re-derived one.
    #[test]
    fn content_revision_round_trips_and_defaults_to_zero_for_old_scenes() {
        let mut scene = pane(
            PaneContent::Terminal(TerminalSurface::default()),
            PaneSceneKind::Terminal,
        );
        scene.content_revision = 7;
        let json = serde_json::to_value(&scene).unwrap();
        assert_eq!(json["content_revision"], 7);
        let round_tripped: PaneScene = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(round_tripped, scene);

        let mut without_field = json;
        without_field
            .as_object_mut()
            .unwrap()
            .remove("content_revision")
            .expect("field present before removal");
        let legacy: PaneScene = serde_json::from_value(without_field).unwrap();
        assert_eq!(legacy.content_revision, 0);
        assert_eq!(legacy.content, scene.content);
    }

    #[test]
    fn terminal_content_has_no_detail_lines() {
        let terminal = pane(
            PaneContent::Terminal(TerminalSurface::default()),
            PaneSceneKind::Terminal,
        );
        assert!(terminal.detail_lines().is_empty());
    }

    #[test]
    fn typed_workflow_rows_are_bounded_and_never_infer_failure_from_text() {
        let mut content = task_content(Some("running".to_owned()), None);
        content.command = format!("printf failed:{}", "x".repeat(2_000));
        let pane = pane(PaneContent::Task(content), PaneSceneKind::Task);
        let rows = pane.workflow_rows();

        assert_eq!(rows[0].part, WorkflowNodePart::Heading);
        assert_eq!(rows[0].role, WorkflowRowRole::Heading);
        assert_eq!(
            pane.workflow_status_badge().unwrap().tone,
            PresentationTone::Running
        );
        assert!(
            rows.iter().all(|row| row.part != WorkflowNodePart::Failure),
            "command text cannot manufacture failure presentation"
        );
        assert!(rows[0].text.ends_with(" … [truncated]"));
        assert!(
            rows[0].text.graphemes(true).count() <= MAX_WORKFLOW_ROW_GRAPHEMES + 15,
            "bounded rows may add only the explicit truncation marker"
        );
        assert_eq!(
            pane.detail_lines(),
            rows.into_iter().map(|row| row.text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn agent_projection_caps_rows_and_bounds_every_external_fragment() {
        let long = "界".repeat(2_000);
        let mut content = agent_content(AgentStatus::WaitingForApproval, &long);
        content.objective = long.clone();
        content.current_action = Some(long.clone());
        content.latest_summary = Some(long.clone());
        content.changed_file_count = 1;
        content.changed_files = vec![long.clone()];
        content.pending_approval = Some(AgentApprovalPrompt {
            command: long.clone(),
            cwd: long.clone(),
            affected_path: Some(long.clone()),
            risk_label: long.clone(),
            risk_basis: long.clone(),
            key_hint: long.clone(),
            pulse_on: false,
        });
        content.output_tail = (0..100).map(|index| format!("{index}: {long}")).collect();
        let rows = pane(PaneContent::Agent(content), PaneSceneKind::Agent).workflow_rows();

        assert!(
            rows.iter()
                .all(|row| { row.text.graphemes(true).count() <= MAX_WORKFLOW_ROW_GRAPHEMES + 15 })
        );
        assert!(rows.iter().any(|row| row.text.ends_with(" … [truncated]")));
        let console = rows
            .iter()
            .filter(|row| row.role == WorkflowRowRole::Console)
            .collect::<Vec<_>>();
        assert_eq!(console.len(), 66);
        assert_eq!(console[1].text, "  … 36 earlier output lines omitted");
        assert!(console.last().unwrap().text.starts_with("  99: "));
        assert!(console.last().unwrap().text.ends_with(" … [truncated]"));
    }
}
