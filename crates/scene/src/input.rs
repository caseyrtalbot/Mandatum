//! Neutral input contract: the events every frontend emits toward the app.
//!
//! The app consumes these types exclusively (`app_state::handle_event`);
//! each frontend owns the translation from its platform event types into
//! these values at its boundary. See docs/decisions.md ("Neutral Input
//! Wiring Landed At The Frontend Boundary").

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

impl Key {
    pub const fn new(code: KeyCode, mods: Modifiers) -> Self {
        Self { code, mods }
    }

    /// A key press with no modifiers held.
    pub const fn plain(code: KeyCode) -> Self {
        Self::new(code, Modifiers::NONE)
    }

    /// A control chord on a character key.
    pub const fn ctrl(character: char) -> Self {
        Self::new(KeyCode::Char(character), Modifiers::CTRL)
    }
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

    pub const CTRL: Self = Self {
        control: true,
        ..Self::NONE
    };

    pub const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };

    /// No modifier held at all.
    pub const fn is_empty(self) -> bool {
        !self.shift && !self.control && !self.alt && !self.super_key
    }

    /// At least one explicit workspace-control modifier (control, alt or
    /// super) is held. Shift alone is ordinary typing, not a chord.
    pub const fn has_command_modifier(self) -> bool {
        self.control || self.alt || self.super_key
    }
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
