//! The top-level workspace scene: everything a frontend needs to draw one
//! frame and hit-test pointer input against it.

use mandatum_core::{PaneId, SplitAxis};
use serde::{Deserialize, Serialize};

use crate::geometry::{SceneRect, SceneSize};
use crate::input::TextRange;
use crate::pane::PaneScene;
use crate::style::SceneCellStyle;

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
/// Dismissed by any action; not modal (input under it behaves normally).
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
    AttentionSegment { index: usize, pane: Option<PaneId> },
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
