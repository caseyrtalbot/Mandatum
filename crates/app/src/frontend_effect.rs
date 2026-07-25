//! Renderer-neutral effects requested by workstation state.
//!
//! Product state describes the platform action and leaves its concrete
//! encoding to the active frontend. The terminal shell maps clipboard text to
//! OSC 52; a native shell can use its platform clipboard without changing
//! `AppState`.

/// A platform action requested by the shared workstation state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum FrontendEffect {
    /// Replace the platform clipboard with the supplied text.
    SetClipboard(String),
    /// Apply this font to the frontend's text surface. Only the native
    /// frontend renders its own text; a terminal frontend inherits the host
    /// terminal's font and ignores this.
    ApplyFont { family: String, size: f32 },
}
