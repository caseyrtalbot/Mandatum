//! Renderer-neutral effects requested by workstation state.
//!
//! Product state describes the platform action and leaves its concrete
//! encoding to the active frontend. The terminal shell maps clipboard text to
//! OSC 52; a native shell can use its platform clipboard without changing
//! `AppState`.

/// A platform action requested by the shared workstation state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontendEffect {
    /// Replace the platform clipboard with the supplied text.
    SetClipboard(String),
}
