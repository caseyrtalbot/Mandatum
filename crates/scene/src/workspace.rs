//! The top-level workspace scene: everything a frontend needs to draw one
//! frame and hit-test pointer input against it.

use mandatum_core::{PaneId, SplitAxis};
use serde::{Deserialize, Serialize};

use crate::geometry::{SceneRect, SceneSize};
use crate::pane::PaneScene;

/// One frame of renderable workspace state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceScene {
    pub size: SceneSize,
    pub header: HeaderScene,
    /// Panes in draw order: tiled panes first, floating panes on top.
    pub panes: Vec<PaneScene>,
    pub overlay: Option<OverlayScene>,
    /// Status line text; a frontend shows "ready" when `None`.
    pub status: Option<String>,
    pub focused_pane: PaneId,
    pub hit_targets: Vec<HitTarget>,
    /// Whether the workspace is in copy mode (one pane's surface carries the
    /// copy cursor and selection).
    pub copy_mode: bool,
}

/// Header strip fields; frontends own the exact formatting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderScene {
    pub session_name: String,
    pub pane_count: usize,
    pub focused_pane: PaneId,
    pub zoomed: bool,
}

/// Modal overlays drawn above the workspace. Open for future overlays
/// (session map, execution timeline).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayScene {
    Palette(PaletteOverlay),
    ContextMenu(ContextMenuOverlay),
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
    /// One palette row, by item index.
    PaletteItem(usize),
    /// One context-menu row, by item index.
    ContextMenuItem(usize),
}
