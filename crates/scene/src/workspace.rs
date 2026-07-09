//! The top-level workspace scene: everything a frontend needs to draw one
//! frame and hit-test pointer input against it.

use mandatum_core::PaneId;
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
}

/// The command palette overlay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteOverlay {
    pub area: SceneRect,
    pub items: Vec<PaletteEntry>,
    /// Highlighted item; `None` until palette selection navigation exists.
    pub selected: Option<usize>,
}

/// One palette row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteEntry {
    pub label: String,
    pub detail: String,
}

impl PaletteEntry {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
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
///
/// Split-separator targets are deliberately absent: the percentage-split
/// layout leaves no dedicated separator cells (pane borders butt together),
/// so drag-to-resize targets land with the pointer outcome (Outcome 5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HitTargetKind {
    /// The pane's inner content area.
    PaneBody(PaneId),
    /// The pane's top border row, where the title is drawn.
    PaneTitle(PaneId),
    /// The status strip at the bottom of the frame.
    StatusStrip,
    /// One palette row, by item index.
    PaletteItem(usize),
}
