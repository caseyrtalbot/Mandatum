use std::path::PathBuf;

use crate::{PaneId, TaskPaneIntent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreAction {
    OpenProject {
        name: String,
        path: PathBuf,
    },
    NewTerminal {
        title: String,
        cwd: Option<PathBuf>,
    },
    CreateTaskPane {
        title: String,
        intent: TaskPaneIntent,
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
    StackFocusedWithNext,
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
