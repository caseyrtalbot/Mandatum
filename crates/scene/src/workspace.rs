//! The top-level workspace scene: everything a frontend needs to draw one
//! frame and hit-test pointer input against it.

use mandatum_core::{PaneId, SplitAxis};
use serde::{Deserialize, Serialize};

use crate::geometry::{LogicalPoint, LogicalRect, SceneRect, SceneSize, ViewportMetrics};
use crate::input::TextRange;
use crate::pane::{PaneBadgeKind, PaneScene};
use crate::style::SceneCellStyle;
use crate::theme::UiDensity;

/// One frame of renderable workspace state. `&WorkspaceScene` alone must
/// suffice to paint a frame: the header and status strips carry their own
/// areas and composed text, so no frontend derives chrome content itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceScene {
    pub size: SceneSize,
    pub header: HeaderScene,
    /// Panes in draw order: tiled panes first, floating panes on top.
    pub panes: Vec<PaneScene>,
    pub overlay: Option<OverlayScene>,
    pub status: StatusScene,
    pub focused_pane: PaneId,
    pub hit_targets: Vec<HitTarget>,
    /// Whether the workspace is in copy mode (one pane's surface carries the
    /// copy cursor and selection).
    pub copy_mode: bool,
    /// Active renderer-neutral text-input caret and transient IME preedit.
    /// This is live presentation state and is never durable workspace intent.
    #[serde(default)]
    pub text_input: Option<TextInputScene>,
    /// Native-grade renderer-neutral geometry and semantics. Cell-only
    /// fixtures may leave this at [`ScenePresentation::default`];
    /// app-built frames always populate it from one coherent viewport.
    #[serde(default)]
    pub presentation: ScenePresentation,
}

/// One frame's scene-owned logical geometry and semantic projections.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePresentation {
    pub viewport: Option<ViewportMetrics>,
    pub density: UiDensity,
    pub nodes: Vec<PresentationNode>,
    pub logical_hit_targets: Vec<LogicalHitTarget>,
    pub terminal_viewports: Vec<TerminalViewportMapping>,
    pub transition_targets: Vec<TransitionTarget>,
    pub accessibility_nodes: Vec<AccessibilityNode>,
}

/// A stable item identity supplied by the app, independent of labels,
/// filtering, geometry, and vector position.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SemanticKey(String);

impl SemanticKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// Opaque structural identity for one semantic presentation node.
///
/// Frontends compare this value but use [`PresentationNode::role`] for
/// meaning; the private representation prevents adapters from parsing it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PresentationNodeId(PresentationIdentity);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum PresentationIdentity {
    Workspace(WorkspaceNodePart),
    Pane {
        pane_id: PaneId,
        part: PaneNodePart,
    },
    Overlay {
        kind: OverlayKind,
        part: OverlayNodePart,
    },
    OverlayItem {
        kind: OverlayKind,
        key: SemanticKey,
    },
}

impl PresentationNodeId {
    pub fn workspace(part: WorkspaceNodePart) -> Self {
        Self(PresentationIdentity::Workspace(part))
    }

    pub fn pane(pane_id: PaneId, part: PaneNodePart) -> Self {
        Self(PresentationIdentity::Pane { pane_id, part })
    }

    pub fn overlay(kind: OverlayKind, part: OverlayNodePart) -> Self {
        Self(PresentationIdentity::Overlay { kind, part })
    }

    pub fn overlay_item(kind: OverlayKind, key: SemanticKey) -> Self {
        Self(PresentationIdentity::OverlayItem { kind, key })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkspaceNodePart {
    Surface,
    Header,
    Status,
    Separator {
        split_index: usize,
        axis: PresentationAxis,
    },
    Attention {
        pane: Option<PaneId>,
        kind: AttentionKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PresentationAxis {
    Horizontal,
    Vertical,
}

impl From<SplitAxis> for PresentationAxis {
    fn from(value: SplitAxis) -> Self {
        match value {
            SplitAxis::Horizontal => Self::Horizontal,
            SplitAxis::Vertical => Self::Vertical,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaneNodePart {
    Surface,
    Title,
    Badge(PaneBadgeKind),
    FocusIndicator,
    Body,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OverlayNodePart {
    Surface,
    Input,
    Footer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OverlayKind {
    Palette,
    ContextMenu,
    Timeline,
    SessionMap,
    Prompt,
    Search,
    Help,
    Welcome,
}

impl OverlayScene {
    pub const fn kind(&self) -> OverlayKind {
        match self {
            Self::Palette(_) => OverlayKind::Palette,
            Self::ContextMenu(_) => OverlayKind::ContextMenu,
            Self::Timeline(_) => OverlayKind::Timeline,
            Self::SessionMap(_) => OverlayKind::SessionMap,
            Self::Prompt(_) => OverlayKind::Prompt,
            Self::Search(_) => OverlayKind::Search,
            Self::Help(_) => OverlayKind::Help,
            Self::Welcome(_) => OverlayKind::Welcome,
        }
    }
}

/// What a semantic node means without requiring a renderer to inspect its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresentationNodeRole {
    Workspace,
    Header,
    Status,
    Pane,
    PaneTitle,
    PaneBadge(PaneBadgeKind),
    FocusIndicator,
    PaneBody,
    TerminalOutput,
    TaskOutput,
    Overlay,
    TextInput,
    Item,
    Separator,
    Attention,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresentationTone {
    #[default]
    Neutral,
    Focus,
    Running,
    Waiting,
    Failure,
    Complete,
    AgentIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttentionKind {
    ApprovalWaiting,
    TaskFailed,
    AgentBlockedOrFailed,
}

/// The cell region that communicates the same meaning in `CellProgram`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalProjection {
    CellRegions(Vec<SceneRect>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationNode {
    pub id: PresentationNodeId,
    pub parent: Option<PresentationNodeId>,
    pub role: PresentationNodeRole,
    pub state: PresentationNodeState,
    pub logical_rect: LogicalRect,
    pub cell_rect: Option<SceneRect>,
    pub terminal_projection: TerminalProjection,
}

/// Renderer-visible semantic state; frontends never infer these facts from
/// labels, colors, or node identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationNodeState {
    pub focused: bool,
    pub selected: bool,
    pub disabled: bool,
    pub attention: bool,
    pub floating: bool,
    pub hovered: bool,
    pub dragging: bool,
    pub tone: PresentationTone,
}

/// Logical-pixel twin of one existing cell hit target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalHitTarget {
    pub node_id: PresentationNodeId,
    pub logical_rect: LogicalRect,
    pub kind: HitTargetKind,
}

/// Exact child-grid mapping for one visible terminal/task output surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalViewportMapping {
    pub node_id: PresentationNodeId,
    pub pane_id: PaneId,
    pub pty_size: SceneSize,
    pub visible_cell_rect: SceneRect,
    pub logical_rect: LogicalRect,
    pub first_visible_surface_row: usize,
}

impl TerminalViewportMapping {
    pub fn logical_point_to_child_cell(
        &self,
        viewport: ViewportMetrics,
        point: LogicalPoint,
    ) -> Option<(u16, u16)> {
        viewport.logical_point_to_cell(self.visible_cell_rect, point)
    }
}

/// A typed property the native adapter may animate for a stable node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionProperty {
    Geometry,
    Opacity,
    Scale,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionTarget {
    pub node_id: PresentationNodeId,
    pub property: TransitionProperty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessibilityRole {
    Workspace,
    Group,
    Header,
    Status,
    Pane,
    Terminal,
    Dialog,
    TextField,
    List,
    ListItem,
    Button,
    Separator,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibilityState {
    pub focused: bool,
    pub selected: bool,
    pub disabled: bool,
    pub busy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessibilityActionKind {
    Focus,
    Activate,
    SetText,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibilityNode {
    pub id: PresentationNodeId,
    pub parent: Option<PresentationNodeId>,
    pub role: AccessibilityRole,
    pub label: String,
    pub value: Option<String>,
    pub state: AccessibilityState,
    pub logical_rect: LogicalRect,
    pub supported_actions: Vec<AccessibilityActionKind>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextInputScene {
    /// One-row region beginning at the active caret and extending to the
    /// surface's right edge.
    pub area: SceneRect,
    pub kind: TextInputKind,
    pub preedit: Option<PreeditScene>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextInputKind {
    Terminal { style: SceneCellStyle },
    Overlay,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreeditScene {
    pub text: String,
    pub cursor: Option<TextRange>,
}

/// The attention strip at the top of the frame. Never blank: when something
/// needs attention `text` leads with the workspace name and the
/// [`AttentionSegment`]s follow at their resolved rects; when calm, `text`
/// is the full session-facts line and `attention` is empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderScene {
    pub area: SceneRect,
    pub workspace_name: String,
    /// Active project label for platform chrome. Kept distinct from session
    /// identity so native adapters never parse composed header text.
    #[serde(default)]
    pub project_name: String,
    pub session_name: String,
    pub pane_count: usize,
    pub focused_pane: PaneId,
    pub zoomed: bool,
    /// Agent connector kind label for the calm strip ("fake" / "claude" /
    /// "none").
    pub connector_label: String,
    /// Pre-composed base text a frontend paints verbatim at `area.x`.
    pub text: String,
    /// Attention segments with resolved rects inside `area`, drawn after
    /// `text` in the theme's attention style. Empty when nothing needs
    /// attention.
    pub attention: Vec<AttentionSegment>,
}

/// One clickable attention segment in the header strip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionSegment {
    pub kind: AttentionKind,
    pub tone: PresentationTone,
    /// Where the segment's label is drawn (and hit-tested).
    pub rect: SceneRect,
    /// e.g. "1 approval · pane-3" or "2 tasks failed · pane-2".
    pub label: String,
    /// The pane a click jumps to, when the condition has one.
    pub pane: Option<PaneId>,
}

/// The status strip at the bottom of the frame: composed text plus its area.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusScene {
    pub area: SceneRect,
    pub text: String,
}

/// Modal overlays drawn above the workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayScene {
    Palette(PaletteOverlay),
    ContextMenu(ContextMenuOverlay),
    Timeline(TimelineOverlay),
    SessionMap(SessionMapOverlay),
    Prompt(PromptOverlay),
    Search(SearchOverlay),
    Help(HelpOverlay),
    Welcome(WelcomeOverlay),
}

/// The help overlay: the live keymap grouped by category, palette fast
/// paths, mouse gestures, and the glyph legends — generated from the command
/// table and keymap, filterable with the palette input pattern.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpOverlay {
    pub area: SceneRect,
    /// The live filter text the user has typed.
    pub query: String,
    /// Rows matching the query (section headings plus their entries).
    pub items: Vec<HelpEntry>,
    /// Highlighted row (scroll anchor); `None` only when `items` is empty.
    pub selected: Option<usize>,
    /// Footer hint line naming the overlay's own keys.
    pub footer: String,
}

/// One help row: a section heading, or a "label + keys" line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpEntry {
    /// `true` renders the row emphasized as a section heading.
    pub heading: bool,
    pub label: String,
    /// The current key route(s), from the live keymap; empty when none.
    pub keys: String,
}

/// The one-time first-run note: a short orientation card shown only when no
/// saved workspace exists. Its semantic rows let every frontend distinguish
/// keys, descriptions, and dismissal guidance without parsing whitespace.
/// Bare Escape is a one-shot consumed dismissal; every other dismissal action
/// continues to its normal route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WelcomeOverlay {
    pub area: SceneRect,
    pub introduction: String,
    pub entries: Vec<WelcomeEntry>,
    pub dismissal: String,
}

/// One first-run route: the live key gesture and the behavior it opens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WelcomeEntry {
    pub keys: String,
    pub description: String,
}

/// The marker frontends draw in front of the focused pane's session-map row.
/// Named here (with the row glyphs it accompanies) so legends and renderers
/// share one source and cannot drift.
pub const SESSION_MAP_FOCUS_GLYPH: &str = "●";

/// The session-search overlay: a filter input on top, matched output lines
/// grouped by source (pane or timeline, most recent first) below it, and a
/// key-hint footer. Plain text search over a snapshot — never embeddings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchOverlay {
    pub area: SceneRect,
    /// The live search text the user has typed.
    pub query: String,
    /// Matches for the query, grouped by source, capped by the engine.
    pub items: Vec<SearchEntry>,
    /// Stable semantic identity parallel to `items`.
    #[serde(default)]
    pub item_keys: Vec<SemanticKey>,
    /// Highlighted entry; `None` only when `items` is empty.
    pub selected: Option<usize>,
    /// Matches beyond the display cap ("+N more" honesty).
    pub overflow: usize,
    /// Footer hint line naming the overlay's own keys.
    pub footer: String,
}

/// One matched line in the search overlay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchEntry {
    /// Source label ("shell · pane-1 (terminal)" or "timeline"). Consecutive
    /// rows share a source; frontends may dim or elide repeats.
    pub source: String,
    /// The matched line, trailing whitespace trimmed.
    pub text: String,
    /// Char indices into `text` matched by the query, for highlighting.
    pub match_indices: Vec<usize>,
    /// The pane Enter jumps to; `None` for timeline hits (Enter opens the
    /// timeline overlay at the entry instead).
    pub pane: Option<PaneId>,
}

/// The execution-timeline overlay: a filter input on top, the filtered
/// durable events below it (newest first), and a key-hint footer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineOverlay {
    pub area: SceneRect,
    /// The live filter text the user has typed.
    pub query: String,
    /// Entries matching the query, newest first.
    pub items: Vec<TimelineEntry>,
    /// Stable semantic identity parallel to `items`.
    #[serde(default)]
    pub item_keys: Vec<SemanticKey>,
    /// Highlighted entry; `None` only when `items` is empty.
    pub selected: Option<usize>,
    /// Malformed log lines skipped while reading (never a crash).
    pub skipped_malformed: usize,
    /// Footer hint line naming the overlay's own keys.
    pub footer: String,
}

/// One rendered timeline event row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEntry {
    /// Kind glyph ("▶", "✓", "✗", "?", …).
    pub glyph: String,
    /// Relative timestamp ("2m ago").
    pub when: String,
    /// Human description of the durable fact.
    pub text: String,
    /// The pane Enter jumps to, when the event names one.
    pub pane: Option<PaneId>,
}

/// The session-map overlay: a tree of sessions and their panes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMapOverlay {
    pub area: SceneRect,
    pub rows: Vec<SessionMapRow>,
    /// Stable semantic identity parallel to `rows`.
    #[serde(default)]
    pub item_keys: Vec<SemanticKey>,
    /// Highlighted row.
    pub selected: usize,
    /// Footer hint line naming the overlay's own keys.
    pub footer: String,
}

/// One session-map row: a session heading (depth 0) or a pane (depth 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMapRow {
    /// Tree depth: 0 for sessions, 1 for panes.
    pub depth: u8,
    /// Kind glyph for panes; session marker for sessions.
    pub glyph: String,
    pub label: String,
    /// One-word live state ("running", "exited:1", "waiting-approval", …);
    /// empty for session rows.
    pub state: String,
    /// Focus marker: the focused pane of the active session.
    pub focused: bool,
    /// Layout badges ("zoom", "float"), space-joined; empty when none.
    pub badges: String,
}

/// A one-line text-input overlay (Set agent objective), reusing the palette
/// input pattern: a bordered box with a title, the editable text, a cursor,
/// and a key-hint footer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptOverlay {
    pub area: SceneRect,
    pub title: String,
    /// The editable input text.
    pub input: String,
    /// Footer hint line naming the overlay's own keys.
    pub footer: String,
}

/// The right-click context menu overlay: a bordered list of the commands
/// relevant to the pane under the pointer, keyboard-navigable and clickable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuOverlay {
    pub area: SceneRect,
    pub items: Vec<ContextMenuEntry>,
    /// Stable semantic identity parallel to `items`.
    #[serde(default)]
    pub item_keys: Vec<SemanticKey>,
    /// Highlighted row.
    pub selected: usize,
}

/// One context-menu row: a command label plus the key chord that runs the
/// same command from the keyboard (rendered right-aligned).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuEntry {
    pub label: String,
    pub chord_hint: String,
}

impl ContextMenuEntry {
    pub fn new(label: impl Into<String>, chord_hint: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            chord_hint: chord_hint.into(),
        }
    }
}

/// The command palette overlay: a fuzzy-filter input line on top, the
/// filtered entries below it, and a key-hint footer on the bottom row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteOverlay {
    pub area: SceneRect,
    /// The live filter text the user has typed.
    pub query: String,
    /// Entries matching the query, best match first.
    pub items: Vec<PaletteEntry>,
    /// Stable semantic identity parallel to `items`.
    #[serde(default)]
    pub item_keys: Vec<SemanticKey>,
    /// Highlighted item; `None` only when `items` is empty.
    pub selected: Option<usize>,
    /// Footer hint line naming the palette's own keys.
    pub footer: String,
}

/// One palette row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteEntry {
    /// Verb-first human label ("Split pane right").
    pub label: String,
    /// Context detail; for a disabled entry, the reason it is unavailable.
    pub detail: String,
    /// The entry's current key(s) from the live keymap: its palette letter
    /// and/or global chord, `None` when unbound.
    pub key_hint: Option<String>,
    /// Char indices into `label` matched by the query, for highlighting.
    pub match_indices: Vec<usize>,
    /// `false` renders the entry greyed; `detail` carries the reason.
    pub enabled: bool,
}

impl PaletteEntry {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            key_hint: None,
            match_indices: Vec::new(),
            enabled: true,
        }
    }
}

/// A rectangle pointer input can land on, tagged with what it means.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HitTarget {
    pub rect: SceneRect,
    pub kind: HitTargetKind,
}

/// What a hit target resolves to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HitTargetKind {
    /// The pane's inner content area.
    PaneBody(PaneId),
    /// The pane's top border row, where the title is drawn.
    PaneTitle(PaneId),
    /// A draggable split boundary (the two adjacent border columns/rows).
    /// `split_index` is the preorder index of the split in the layout tree,
    /// matching `mandatum_core::Layout::set_split_percent`.
    Separator { split_index: usize, axis: SplitAxis },
    /// The status strip at the bottom of the frame.
    StatusStrip,
    /// One header attention segment, by index into `HeaderScene::attention`,
    /// carrying the pane a click jumps to (self-contained for hit testing).
    AttentionSegment {
        index: usize,
        pane: Option<PaneId>,
        kind: AttentionKind,
    },
    /// One palette row, by item index.
    PaletteItem(usize),
    /// One context-menu row, by item index.
    ContextMenuItem(usize),
    /// One timeline row, by index into the overlay's filtered items.
    TimelineItem(usize),
    /// One session-map row, by row index.
    SessionMapRow(usize),
    /// One search-result row, by index into the overlay's items.
    SearchItem(usize),
}
