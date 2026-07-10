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
    EnterCopyMode,
    RunTask,
    RerunTask,
    StopTask,
    NewAgentPane,
    StartAgent,
    StopAgent,
    ApproveAgentAction,
    RejectAgentAction,
    FocusNextWaitingAgent,
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
    Task,
    Agent,
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
        id: CommandId::EnterCopyMode,
        label: "Copy Mode",
        category: CommandCategory::Pane,
    },
    Command {
        id: CommandId::RunTask,
        label: "Run Task",
        category: CommandCategory::Task,
    },
    Command {
        id: CommandId::RerunTask,
        label: "Rerun Task",
        category: CommandCategory::Task,
    },
    Command {
        id: CommandId::StopTask,
        label: "Stop Task",
        category: CommandCategory::Task,
    },
    Command {
        id: CommandId::NewAgentPane,
        label: "New Agent Pane",
        category: CommandCategory::Agent,
    },
    Command {
        id: CommandId::StartAgent,
        label: "Start Agent",
        category: CommandCategory::Agent,
    },
    Command {
        id: CommandId::StopAgent,
        label: "Stop Agent",
        category: CommandCategory::Agent,
    },
    Command {
        id: CommandId::ApproveAgentAction,
        label: "Approve Agent Action",
        category: CommandCategory::Agent,
    },
    Command {
        id: CommandId::RejectAgentAction,
        label: "Reject Agent Action",
        category: CommandCategory::Agent,
    },
    Command {
        id: CommandId::FocusNextWaitingAgent,
        label: "Focus Next Waiting Agent",
        category: CommandCategory::Agent,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandTarget {
    Core,
    Runtime(RuntimeCommand),
    RuntimeTask(RuntimeTaskCommand),
    RuntimeAgent(RuntimeAgentCommand),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCommand {
    EnterCopyMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeTaskCommand {
    RunConfiguredTask,
    RerunFocusedTask,
    StopFocusedTask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeAgentCommand {
    NewAgentPane,
    StartFocusedAgent,
    StopFocusedAgent,
    ApproveFocusedAgentAction,
    RejectFocusedAgentAction,
    FocusNextWaitingAgent,
}

impl CommandTarget {
    pub fn is_runtime(self) -> bool {
        !matches!(self, Self::Core)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteKey {
    Character(char),
    Tab,
    BackTab,
    Escape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteInput {
    Close,
    Quit,
    Dispatch(CommandId),
    Noop,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaletteContext {
    pub focused_pane_is_task: bool,
}

impl PaletteContext {
    pub const fn focused_task() -> Self {
        Self {
            focused_pane_is_task: true,
        }
    }
}

pub fn command_target(command_id: CommandId) -> CommandTarget {
    match command_id {
        CommandId::EnterCopyMode => CommandTarget::Runtime(RuntimeCommand::EnterCopyMode),
        CommandId::RunTask => CommandTarget::RuntimeTask(RuntimeTaskCommand::RunConfiguredTask),
        CommandId::RerunTask => CommandTarget::RuntimeTask(RuntimeTaskCommand::RerunFocusedTask),
        CommandId::StopTask => CommandTarget::RuntimeTask(RuntimeTaskCommand::StopFocusedTask),
        CommandId::NewAgentPane => CommandTarget::RuntimeAgent(RuntimeAgentCommand::NewAgentPane),
        CommandId::StartAgent => {
            CommandTarget::RuntimeAgent(RuntimeAgentCommand::StartFocusedAgent)
        }
        CommandId::StopAgent => CommandTarget::RuntimeAgent(RuntimeAgentCommand::StopFocusedAgent),
        CommandId::ApproveAgentAction => {
            CommandTarget::RuntimeAgent(RuntimeAgentCommand::ApproveFocusedAgentAction)
        }
        CommandId::RejectAgentAction => {
            CommandTarget::RuntimeAgent(RuntimeAgentCommand::RejectFocusedAgentAction)
        }
        CommandId::FocusNextWaitingAgent => {
            CommandTarget::RuntimeAgent(RuntimeAgentCommand::FocusNextWaitingAgent)
        }
        _ => CommandTarget::Core,
    }
}

pub fn resolve_palette_key(key: PaletteKey) -> PaletteInput {
    resolve_palette_key_with_context(key, PaletteContext::default())
}

pub fn resolve_palette_key_with_context(key: PaletteKey, context: PaletteContext) -> PaletteInput {
    match key {
        PaletteKey::Escape => PaletteInput::Close,
        PaletteKey::Character('q') => PaletteInput::Quit,
        PaletteKey::Character('n') => PaletteInput::Dispatch(CommandId::NewTerminal),
        PaletteKey::Character('v') => PaletteInput::Dispatch(CommandId::SplitRight),
        PaletteKey::Character('s') => PaletteInput::Dispatch(CommandId::SplitDown),
        PaletteKey::Character('h') | PaletteKey::BackTab => {
            PaletteInput::Dispatch(CommandId::FocusPrevious)
        }
        PaletteKey::Character('l') | PaletteKey::Tab => {
            PaletteInput::Dispatch(CommandId::FocusNext)
        }
        PaletteKey::Character('x') => PaletteInput::Dispatch(CommandId::ClosePane),
        PaletteKey::Character('z') => PaletteInput::Dispatch(CommandId::ZoomPane),
        PaletteKey::Character('f') => PaletteInput::Dispatch(CommandId::FloatPane),
        PaletteKey::Character('t') => PaletteInput::Dispatch(CommandId::StackPanes),
        PaletteKey::Character('r') if context.focused_pane_is_task => {
            PaletteInput::Dispatch(CommandId::RerunTask)
        }
        PaletteKey::Character('r') => PaletteInput::Dispatch(CommandId::RestartPane),
        PaletteKey::Character('c') if context.focused_pane_is_task => {
            PaletteInput::Dispatch(CommandId::StopTask)
        }
        PaletteKey::Character('b') => PaletteInput::Dispatch(CommandId::RunTask),
        PaletteKey::Character('a') => PaletteInput::Dispatch(CommandId::NewAgentPane),
        PaletteKey::Character('g') => PaletteInput::Dispatch(CommandId::StartAgent),
        PaletteKey::Character('k') => PaletteInput::Dispatch(CommandId::StopAgent),
        PaletteKey::Character('y') => PaletteInput::Dispatch(CommandId::ApproveAgentAction),
        PaletteKey::Character('d') => PaletteInput::Dispatch(CommandId::RejectAgentAction),
        PaletteKey::Character('j') => PaletteInput::Dispatch(CommandId::FocusNextWaitingAgent),
        PaletteKey::Character('w') => PaletteInput::Dispatch(CommandId::SaveWorkspace),
        PaletteKey::Character('o') => PaletteInput::Dispatch(CommandId::RestoreWorkspace),
        PaletteKey::Character('[') => PaletteInput::Dispatch(CommandId::EnterCopyMode),
        _ => PaletteInput::Noop,
    }
}

pub fn dispatch_command(
    workspace: &mut Workspace,
    context: &CommandContext,
    command_id: CommandId,
) -> Result<ActionOutcome, CommandError> {
    let action = core_action_for_command(command_id, context)?;
    workspace
        .apply_action(action)
        .map_err(CommandError::Workspace)
}

pub fn core_action_for_command(
    command_id: CommandId,
    context: &CommandContext,
) -> Result<CoreAction, CommandError> {
    command_for_id(command_id).ok_or(CommandError::UnknownCommand(command_id))?;
    if command_target(command_id).is_runtime() {
        return Err(CommandError::NotACoreCommand(command_id));
    }

    Ok(match command_id {
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
        CommandId::EnterCopyMode
        | CommandId::RunTask
        | CommandId::RerunTask
        | CommandId::StopTask
        | CommandId::NewAgentPane
        | CommandId::StartAgent
        | CommandId::StopAgent
        | CommandId::ApproveAgentAction
        | CommandId::RejectAgentAction
        | CommandId::FocusNextWaitingAgent => {
            return Err(CommandError::NotACoreCommand(command_id));
        }
    })
}

#[derive(Debug)]
pub enum CommandError {
    UnknownCommand(CommandId),
    NotACoreCommand(CommandId),
    Workspace(WorkspaceError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(command_id) => write!(formatter, "unknown command {command_id:?}"),
            Self::NotACoreCommand(command_id) => write!(
                formatter,
                "command {command_id:?} is handled by the app runtime, not core"
            ),
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
        assert!(command_ids.contains(&CommandId::RunTask));
        assert!(command_ids.contains(&CommandId::RerunTask));
        assert!(command_ids.contains(&CommandId::StopTask));
        assert!(command_ids.contains(&CommandId::SaveWorkspace));
        assert!(command_ids.contains(&CommandId::RestoreWorkspace));
    }

    #[test]
    fn runtime_commands_are_not_core_commands() {
        assert_eq!(
            command_target(CommandId::EnterCopyMode),
            CommandTarget::Runtime(RuntimeCommand::EnterCopyMode)
        );
        assert_eq!(
            command_target(CommandId::RunTask),
            CommandTarget::RuntimeTask(RuntimeTaskCommand::RunConfiguredTask)
        );
        assert_eq!(
            command_target(CommandId::RerunTask),
            CommandTarget::RuntimeTask(RuntimeTaskCommand::RerunFocusedTask)
        );
        assert_eq!(
            command_target(CommandId::StopTask),
            CommandTarget::RuntimeTask(RuntimeTaskCommand::StopFocusedTask)
        );
        assert_eq!(command_target(CommandId::RestartPane), CommandTarget::Core);
    }

    #[test]
    fn core_action_mapping_rejects_runtime_commands_without_panicking() {
        let context = CommandContext::for_project("w", "/tmp/p");

        let result = core_action_for_command(CommandId::EnterCopyMode, &context);

        assert!(matches!(
            result,
            Err(CommandError::NotACoreCommand(CommandId::EnterCopyMode))
        ));
    }

    #[test]
    fn palette_keys_resolve_to_command_metadata() {
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('[')),
            PaletteInput::Dispatch(CommandId::EnterCopyMode)
        );
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('b')),
            PaletteInput::Dispatch(CommandId::RunTask)
        );
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('r')),
            PaletteInput::Dispatch(CommandId::RestartPane)
        );
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('w')),
            PaletteInput::Dispatch(CommandId::SaveWorkspace)
        );
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('o')),
            PaletteInput::Dispatch(CommandId::RestoreWorkspace)
        );
        assert_eq!(
            resolve_palette_key(PaletteKey::BackTab),
            PaletteInput::Dispatch(CommandId::FocusPrevious)
        );
        assert_eq!(resolve_palette_key(PaletteKey::Escape), PaletteInput::Close);
    }

    #[test]
    fn focused_task_palette_keys_resolve_to_rerun_and_stop() {
        assert_eq!(
            resolve_palette_key_with_context(
                PaletteKey::Character('r'),
                PaletteContext::focused_task(),
            ),
            PaletteInput::Dispatch(CommandId::RerunTask)
        );
        assert_eq!(
            resolve_palette_key_with_context(
                PaletteKey::Character('c'),
                PaletteContext::focused_task(),
            ),
            PaletteInput::Dispatch(CommandId::StopTask)
        );
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('c')),
            PaletteInput::Noop
        );
    }

    #[test]
    fn agent_palette_keys_resolve_to_agent_commands() {
        for (key, command_id) in [
            ('a', CommandId::NewAgentPane),
            ('g', CommandId::StartAgent),
            ('k', CommandId::StopAgent),
            ('y', CommandId::ApproveAgentAction),
            ('d', CommandId::RejectAgentAction),
            ('j', CommandId::FocusNextWaitingAgent),
        ] {
            assert_eq!(
                resolve_palette_key(PaletteKey::Character(key)),
                PaletteInput::Dispatch(command_id)
            );
        }
    }

    #[test]
    fn built_in_agent_commands_are_agent_category_and_runtime_targets() {
        for (command_id, expected_target) in [
            (CommandId::NewAgentPane, RuntimeAgentCommand::NewAgentPane),
            (
                CommandId::StartAgent,
                RuntimeAgentCommand::StartFocusedAgent,
            ),
            (CommandId::StopAgent, RuntimeAgentCommand::StopFocusedAgent),
            (
                CommandId::ApproveAgentAction,
                RuntimeAgentCommand::ApproveFocusedAgentAction,
            ),
            (
                CommandId::RejectAgentAction,
                RuntimeAgentCommand::RejectFocusedAgentAction,
            ),
            (
                CommandId::FocusNextWaitingAgent,
                RuntimeAgentCommand::FocusNextWaitingAgent,
            ),
        ] {
            let command = command_for_id(command_id).unwrap();

            assert_eq!(command.category, CommandCategory::Agent);
            assert_eq!(
                command_target(command_id),
                CommandTarget::RuntimeAgent(expected_target)
            );

            let mut workspace = Workspace::new("w", PathBuf::from("/tmp/p"));
            let context = CommandContext::for_project("w", "/tmp/p");
            let result = dispatch_command(&mut workspace, &context, command_id);

            assert!(matches!(result, Err(CommandError::NotACoreCommand(id)) if id == command_id));
            assert_eq!(workspace.active_session().panes().len(), 1);
        }
    }

    #[test]
    fn dispatch_rejects_runtime_commands_instead_of_mutating_core() {
        let mut workspace = Workspace::new("w", PathBuf::from("/tmp/p"));
        let context = CommandContext::for_project("w", "/tmp/p");

        let result = dispatch_command(&mut workspace, &context, CommandId::EnterCopyMode);

        assert!(matches!(
            result,
            Err(CommandError::NotACoreCommand(CommandId::EnterCopyMode))
        ));
        assert_eq!(workspace.active_session().panes().len(), 1);
    }

    #[test]
    fn built_in_commands_include_copy_mode() {
        assert!(
            BUILT_IN_COMMANDS
                .iter()
                .any(|command| command.id == CommandId::EnterCopyMode)
        );
    }

    #[test]
    fn built_in_task_commands_are_task_category_and_runtime_targets() {
        for (command_id, expected_target) in [
            (CommandId::RunTask, RuntimeTaskCommand::RunConfiguredTask),
            (CommandId::RerunTask, RuntimeTaskCommand::RerunFocusedTask),
            (CommandId::StopTask, RuntimeTaskCommand::StopFocusedTask),
        ] {
            let command = command_for_id(command_id).unwrap();

            assert_eq!(command.category, CommandCategory::Task);
            assert_eq!(
                command_target(command_id),
                CommandTarget::RuntimeTask(expected_target)
            );

            let mut workspace = Workspace::new("w", PathBuf::from("/tmp/p"));
            let context = CommandContext::for_project("w", "/tmp/p");
            let result = dispatch_command(&mut workspace, &context, command_id);

            assert!(matches!(result, Err(CommandError::NotACoreCommand(id)) if id == command_id));
            assert_eq!(workspace.active_session().panes().len(), 1);
        }
    }
}
