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

mod geometry;
pub mod input;
pub mod layout;
mod pane;
mod style;
mod surface;
mod workspace;

/// Durable pane identity, shared with `mandatum-core` so frontends need only
/// this crate.
pub use mandatum_core::PaneId;

pub use geometry::{SceneRect, SceneSize};
pub use pane::{
    AgentApprovalPrompt, AgentContent, EmptyContent, PaneContent, PaneScene, PaneSceneKind,
    TaskContent,
};
pub use style::{SceneCellStyle, SceneColor};
pub use surface::{SceneCell, SurfacePosition, TerminalSurface};
pub use workspace::{
    HeaderScene, HitTarget, HitTargetKind, OverlayScene, PaletteEntry, PaletteOverlay,
    WorkspaceScene,
};
