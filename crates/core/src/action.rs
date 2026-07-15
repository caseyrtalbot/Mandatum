use std::path::PathBuf;

use crate::{AgentPaneIntent, PaneId, SessionId, TaskPaneIntent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreAction {
    /// Create a fresh session for the active project. The project identity and
    /// path are reused; only session-local panes, layout, and focus start over.
    NewSession,
    OpenProject {
        name: String,
        path: PathBuf,
    },
    /// Make an existing session the active one (the session map's jump
    /// action). Durable intent: which session is active already persists.
    ActivateSession {
        session_id: SessionId,
    },
    NewTerminal {
        title: String,
        cwd: Option<PathBuf>,
    },
    CreateTaskPane {
        title: String,
        intent: TaskPaneIntent,
    },
    CreateAgentPane {
        title: String,
        intent: AgentPaneIntent,
        cwd: Option<PathBuf>,
    },
    SplitRight,
    SplitDown,
    FocusNext,
    FocusPrevious,
    FocusPane {
        pane_id: PaneId,
    },
    CloseFocused,
    RestartFocused,
    RenameFocused {
        title: String,
    },
    ToggleZoomFocused,
    FloatFocused,
    /// Return the focused floating pane to the tiled tree (the inverse of
    /// `FloatFocused`).
    DockFocused,
    StackFocusedWithNext,
    /// Grow (positive) or shrink (negative) the focused tiled pane's share
    /// of its nearest enclosing split, in percentage points (the keyboard
    /// counterpart of `SetSplitRatio` drag-resize).
    ResizeFocused {
        delta_percent: i8,
    },
    /// Set the first-side percentage of the `split_index`-th layout split in
    /// preorder (pointer drag-resize lands here as durable layout intent).
    SetSplitRatio {
        split_index: usize,
        first_percent: u8,
    },
    /// Move a floating pane's top-left corner, in workspace-area coordinates.
    MoveFloatingPane {
        pane_id: PaneId,
        x: u16,
        y: u16,
    },
    SaveWorkspace,
    RestoreWorkspace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    Mutated { focused_pane: PaneId },
    PersistenceRequested(PersistenceRequest),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistenceRequest {
    SaveWorkspace,
    RestoreWorkspace,
}
