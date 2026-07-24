//! Mandatum's frontend contract.
//!
//! This crate defines the renderer-neutral scene model every frontend
//! consumes and the neutral input events every frontend emits. Product
//! behavior lives behind this boundary; frontends translate scenes into
//! pixels or cells and translate platform events into [`input`] values.
//! The [`layout`] module owns all pane-rect computation, so no frontend
//! computes layout.
//!
//! No frontend, parser, process, or async-runtime type may appear here
//! (Constitution L1/L2/L4; enforced by `ci/conformance.sh`).

pub mod cell_program;
mod geometry;
pub mod input;
pub mod layout;
mod pane;
mod style;
mod surface;
mod theme;
mod workspace;

/// Durable pane identity, agent status, and split-axis orientation, shared
/// with `mandatum-core` so frontends need only this crate.
pub use mandatum_core::{AgentStatus, ArtifactFit, PaneId, SplitAxis};

pub use cell_program::{
    CellOccupancy, CellProgram, CellSelection, ProgramCell, compile_cell_program,
};
pub use geometry::{SceneRect, SceneSize};
pub use pane::{
    AgentApprovalPrompt, AgentContent, ArtifactContent, ArtifactState, EmptyContent, PaneContent,
    PaneScene, PaneSceneKind, TaskContent,
};
pub use style::{SceneCellStyle, SceneColor};
pub use surface::{RasterSurface, SceneCell, SurfacePosition, TerminalSurface};
pub use theme::Theme;
pub use workspace::{
    AttentionSegment, ContextMenuEntry, ContextMenuOverlay, HeaderScene, HelpEntry, HelpOverlay,
    HitTarget, HitTargetKind, OverlayScene, PaletteEntry, PaletteOverlay, PromptOverlay,
    SESSION_MAP_FOCUS_GLYPH, SearchEntry, SearchOverlay, SessionMapOverlay, SessionMapRow,
    StatusScene, TimelineEntry, TimelineOverlay, WelcomeEntry, WelcomeOverlay, WorkspaceScene,
};
