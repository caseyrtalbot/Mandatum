//! Command metadata and dispatch boundary.

use std::fmt;
use std::path::PathBuf;

use mandatum_core::{ActionOutcome, CoreAction, Workspace, WorkspaceError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandId {
    OpenProject,
    NewTerminal,
    SplitRight,
    SplitDown,
    FocusNext,
    FocusPrevious,
    ClosePane,
    RestartPane,
    ZoomPane,
    FloatPane,
    StackPanes,
    SaveWorkspace,
    RestoreWorkspace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandCategory {
    Project,
    Pane,
    Layout,
    Persistence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub label: &'static str,
    pub category: CommandCategory,
}

pub const BUILT_IN_COMMANDS: &[Command] = &[
    Command {
        id: CommandId::OpenProject,
        label: "Open Project",
        category: CommandCategory::Project,
    },
    Command {
        id: CommandId::NewTerminal,
        label: "New Terminal",
        category: CommandCategory::Pane,
    },
    Command {
        id: CommandId::SplitRight,
        label: "Split Right",
        category: CommandCategory::Layout,
    },
    Command {
        id: CommandId::SplitDown,
        label: "Split Down",
        category: CommandCategory::Layout,
    },
    Command {
        id: CommandId::FocusNext,
        label: "Focus Next",
        category: CommandCategory::Pane,
    },
    Command {
        id: CommandId::FocusPrevious,
        label: "Focus Previous",
        category: CommandCategory::Pane,
    },
    Command {
        id: CommandId::ClosePane,
        label: "Close Pane",
        category: CommandCategory::Pane,
    },
    Command {
        id: CommandId::RestartPane,
        label: "Restart Pane",
        category: CommandCategory::Pane,
    },
    Command {
        id: CommandId::ZoomPane,
        label: "Zoom Pane",
        category: CommandCategory::Layout,
    },
    Command {
        id: CommandId::FloatPane,
        label: "Float Pane",
        category: CommandCategory::Layout,
    },
    Command {
        id: CommandId::StackPanes,
        label: "Stack Panes",
        category: CommandCategory::Layout,
    },
    Command {
        id: CommandId::SaveWorkspace,
        label: "Save Workspace",
        category: CommandCategory::Persistence,
    },
    Command {
        id: CommandId::RestoreWorkspace,
        label: "Restore Workspace",
        category: CommandCategory::Persistence,
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandContext {
    pub project_name: String,
    pub project_path: PathBuf,
    pub new_terminal_title: String,
    pub new_terminal_cwd: Option<PathBuf>,
}

impl CommandContext {
    pub fn for_project(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            project_name: name.into(),
            project_path: path.clone(),
            new_terminal_title: "terminal".to_owned(),
            new_terminal_cwd: Some(path),
        }
    }
}

pub fn command_for_id(command_id: CommandId) -> Option<&'static Command> {
    BUILT_IN_COMMANDS
        .iter()
        .find(|command| command.id == command_id)
}

pub fn dispatch_command(
    workspace: &mut Workspace,
    context: &CommandContext,
    command_id: CommandId,
) -> Result<ActionOutcome, CommandError> {
    command_for_id(command_id).ok_or(CommandError::UnknownCommand(command_id))?;
    let action = action_for_command(command_id, context);
    workspace
        .apply_action(action)
        .map_err(CommandError::Workspace)
}

pub fn action_for_command(command_id: CommandId, context: &CommandContext) -> CoreAction {
    match command_id {
        CommandId::OpenProject => CoreAction::OpenProject {
            name: context.project_name.clone(),
            path: context.project_path.clone(),
        },
        CommandId::NewTerminal => CoreAction::NewTerminal {
            title: context.new_terminal_title.clone(),
            cwd: context.new_terminal_cwd.clone(),
        },
        CommandId::SplitRight => CoreAction::SplitRight,
        CommandId::SplitDown => CoreAction::SplitDown,
        CommandId::FocusNext => CoreAction::FocusNext,
        CommandId::FocusPrevious => CoreAction::FocusPrevious,
        CommandId::ClosePane => CoreAction::CloseFocused,
        CommandId::RestartPane => CoreAction::RestartFocused,
        CommandId::ZoomPane => CoreAction::ToggleZoomFocused,
        CommandId::FloatPane => CoreAction::FloatFocused,
        CommandId::StackPanes => CoreAction::StackFocusedWithNext,
        CommandId::SaveWorkspace => CoreAction::SaveWorkspace,
        CommandId::RestoreWorkspace => CoreAction::RestoreWorkspace,
    }
}

#[derive(Debug)]
pub enum CommandError {
    UnknownCommand(CommandId),
    Workspace(WorkspaceError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(command_id) => write!(formatter, "unknown command {command_id:?}"),
            Self::Workspace(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CommandError {}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mandatum_core::{ActionOutcome, PersistenceRequest};

    use super::*;

    #[test]
    fn dispatch_invokes_core_actions_without_owning_layout_mutation() {
        let mut workspace = Workspace::new("workspace", PathBuf::from("/tmp/project"));
        let context = CommandContext::for_project("other", "/tmp/other");

        dispatch_command(&mut workspace, &context, CommandId::SplitRight).unwrap();
        dispatch_command(&mut workspace, &context, CommandId::FocusPrevious).unwrap();

        let session = workspace.active_session();
        assert_eq!(session.panes().len(), 2);
        assert_eq!(session.focused_pane_id().as_str(), "pane-1");
    }

    #[test]
    fn persistence_commands_return_requests() {
        let mut workspace = Workspace::new("workspace", PathBuf::from("/tmp/project"));
        let context = CommandContext::for_project("other", "/tmp/other");

        let outcome = dispatch_command(&mut workspace, &context, CommandId::SaveWorkspace).unwrap();

        assert_eq!(
            outcome,
            ActionOutcome::PersistenceRequested(PersistenceRequest::SaveWorkspace)
        );
    }

    #[test]
    fn built_in_commands_include_expected_milestone_one_surface() {
        let command_ids = BUILT_IN_COMMANDS
            .iter()
            .map(|command| command.id)
            .collect::<Vec<_>>();

        assert!(command_ids.contains(&CommandId::OpenProject));
        assert!(command_ids.contains(&CommandId::NewTerminal));
        assert!(command_ids.contains(&CommandId::SplitRight));
        assert!(command_ids.contains(&CommandId::SplitDown));
        assert!(command_ids.contains(&CommandId::StackPanes));
        assert!(command_ids.contains(&CommandId::SaveWorkspace));
        assert!(command_ids.contains(&CommandId::RestoreWorkspace));
    }
}
