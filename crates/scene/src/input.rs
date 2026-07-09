//! Neutral input contract: the events every frontend emits toward the app.
//!
//! TYPES ONLY for now. The app still consumes crossterm events directly
//! (`app_state::handle_event`); rewiring input through these types is
//! deferred to the pointer outcome, which forces the translation layer
//! anyway. See docs/decisions.md ("Scene Output Contract Adopted; Neutral
//! Input Wiring Deferred To The Pointer Outcome").

use serde::{Deserialize, Serialize};

use crate::geometry::SceneSize;

/// One platform-neutral input event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    Key(Key),
    Pointer(PointerEvent),
    Paste(String),
    Resize(SceneSize),
    FocusGained,
    FocusLost,
}

/// A key press with modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Modifiers,
}

/// Neutral key identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyCode {
    Char(char),
    Enter,
    Escape,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Function(u8),
}

/// Modifier keys held during an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// Command on macOS, Windows key elsewhere.
    pub super_key: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        control: false,
        alt: false,
        super_key: false,
    };
}

/// A pointer event in cell coordinates, ready to hit-test against
/// [`crate::HitTarget`] rects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerEvent {
    pub kind: PointerKind,
    /// The button involved; `None` for pure motion and wheel events.
    pub button: Option<PointerButton>,
    /// Cell column of the event.
    pub column: u16,
    /// Cell row of the event.
    pub row: u16,
    pub mods: Modifiers,
}

/// What the pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerKind {
    Down,
    Up,
    Move,
    Drag,
    /// Scroll wheel movement in cells; positive `dy` scrolls down.
    Wheel {
        dx: i16,
        dy: i16,
    },
}

/// Which pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}
