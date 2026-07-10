//! Command metadata and dispatch boundary.

pub mod fuzzy;

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
    CopySelection,
    RunTask,
    RerunTask,
    StopTask,
    NewAgentPane,
    StartAgent,
    StopAgent,
    ApproveAgentAction,
    RejectAgentAction,
    FocusNextWaitingAgent,
    SetAgentObjective,
    ShowTimeline,
    ShowSessionMap,
    SearchSession,
    ZoomPane,
    FloatPane,
    DockPane,
    StackPanes,
    GrowPane,
    ShrinkPane,
    MoveFloatLeft,
    MoveFloatRight,
    MoveFloatUp,
    MoveFloatDown,
    ShowHelp,
    SaveWorkspace,
    RestoreWorkspace,
    ReloadConfig,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandCategory {
    Project,
    Pane,
    Task,
    Agent,
    Layout,
    Persistence,
    Config,
    App,
}

/// How many percentage points one Grow/Shrink step moves the focused pane's
/// nearest enclosing split boundary.
pub const RESIZE_STEP_PERCENT: i8 = 5;

/// One built-in command. This table is the single source of keymap defaults:
/// `name` is the stable kebab-case key a config file uses to rebind the
/// command, and `palette_key` is its default single-letter palette binding
/// (`None` for commands with no default letter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub label: &'static str,
    pub name: &'static str,
    pub category: CommandCategory,
    pub palette_key: Option<char>,
}

pub const BUILT_IN_COMMANDS: &[Command] = &[
    Command {
        id: CommandId::OpenProject,
        label: "Open project",
        name: "open-project",
        category: CommandCategory::Project,
        palette_key: None,
    },
    Command {
        id: CommandId::NewTerminal,
        label: "New terminal",
        name: "new-terminal",
        category: CommandCategory::Pane,
        palette_key: Some('n'),
    },
    Command {
        id: CommandId::SplitRight,
        label: "Split pane right",
        name: "split-right",
        category: CommandCategory::Layout,
        palette_key: Some('v'),
    },
    Command {
        id: CommandId::SplitDown,
        label: "Split pane down",
        name: "split-down",
        category: CommandCategory::Layout,
        palette_key: Some('s'),
    },
    Command {
        id: CommandId::FocusNext,
        label: "Focus next pane",
        name: "focus-next",
        category: CommandCategory::Pane,
        palette_key: Some('l'),
    },
    Command {
        id: CommandId::FocusPrevious,
        label: "Focus previous pane",
        name: "focus-previous",
        category: CommandCategory::Pane,
        palette_key: Some('h'),
    },
    Command {
        id: CommandId::ClosePane,
        label: "Close pane",
        name: "close-pane",
        category: CommandCategory::Pane,
        palette_key: Some('x'),
    },
    Command {
        id: CommandId::RestartPane,
        label: "Restart pane",
        name: "restart-pane",
        category: CommandCategory::Pane,
        palette_key: Some('r'),
    },
    Command {
        id: CommandId::EnterCopyMode,
        label: "Enter copy mode",
        name: "copy-mode",
        category: CommandCategory::Pane,
        palette_key: Some('['),
    },
    Command {
        id: CommandId::CopySelection,
        label: "Copy selection",
        name: "copy-selection",
        category: CommandCategory::Pane,
        palette_key: Some('u'),
    },
    Command {
        id: CommandId::RunTask,
        label: "Run task",
        name: "run-task",
        category: CommandCategory::Task,
        palette_key: Some('b'),
    },
    Command {
        id: CommandId::RerunTask,
        label: "Rerun task",
        name: "rerun-task",
        category: CommandCategory::Task,
        // Rides the restart-pane letter when a task pane is focused.
        palette_key: None,
    },
    Command {
        id: CommandId::StopTask,
        label: "Stop task",
        name: "stop-task",
        category: CommandCategory::Task,
        palette_key: Some('c'),
    },
    Command {
        id: CommandId::NewAgentPane,
        label: "New agent pane",
        name: "new-agent-pane",
        category: CommandCategory::Agent,
        palette_key: Some('a'),
    },
    Command {
        id: CommandId::StartAgent,
        label: "Start agent",
        name: "start-agent",
        category: CommandCategory::Agent,
        palette_key: Some('g'),
    },
    Command {
        id: CommandId::StopAgent,
        label: "Stop agent",
        name: "stop-agent",
        category: CommandCategory::Agent,
        palette_key: Some('k'),
    },
    Command {
        id: CommandId::ApproveAgentAction,
        label: "Approve agent action",
        name: "approve-agent-action",
        category: CommandCategory::Agent,
        palette_key: Some('y'),
    },
    Command {
        id: CommandId::RejectAgentAction,
        label: "Reject agent action",
        name: "reject-agent-action",
        category: CommandCategory::Agent,
        palette_key: Some('d'),
    },
    Command {
        id: CommandId::FocusNextWaitingAgent,
        label: "Focus next waiting agent",
        name: "focus-next-waiting-agent",
        category: CommandCategory::Agent,
        palette_key: Some('j'),
    },
    Command {
        id: CommandId::SetAgentObjective,
        label: "Set agent objective",
        name: "set-agent-objective",
        category: CommandCategory::Agent,
        palette_key: Some('p'),
    },
    Command {
        id: CommandId::ShowTimeline,
        label: "Show timeline",
        name: "show-timeline",
        category: CommandCategory::App,
        // '/' — the timeline is the searchable history surface.
        palette_key: Some('/'),
    },
    Command {
        id: CommandId::ShowSessionMap,
        label: "Show session map",
        name: "show-session-map",
        category: CommandCategory::App,
        palette_key: Some('m'),
    },
    Command {
        id: CommandId::SearchSession,
        // Honest naming: exact/fuzzy text search over output, not embeddings.
        label: "Search session output",
        name: "search-session",
        category: CommandCategory::App,
        // Deliberately unbound: 'i' is the last free letter, and binding it
        // would leave no bare key that seeds the palette filter. The routes
        // are the ctrl+shift+f chord, the fuzzy palette, and the pane menu.
        palette_key: None,
    },
    Command {
        id: CommandId::ZoomPane,
        label: "Zoom pane",
        name: "zoom-pane",
        category: CommandCategory::Layout,
        palette_key: Some('z'),
    },
    Command {
        id: CommandId::FloatPane,
        label: "Float pane",
        name: "float-pane",
        category: CommandCategory::Layout,
        palette_key: Some('f'),
    },
    Command {
        id: CommandId::DockPane,
        label: "Dock pane",
        name: "dock-pane",
        category: CommandCategory::Layout,
        // Rides the float-pane letter when the focused pane is floating.
        palette_key: None,
    },
    Command {
        id: CommandId::StackPanes,
        label: "Stack panes",
        name: "stack-panes",
        category: CommandCategory::Layout,
        palette_key: Some('t'),
    },
    Command {
        id: CommandId::GrowPane,
        label: "Grow pane",
        name: "grow-pane",
        category: CommandCategory::Layout,
        palette_key: Some('+'),
    },
    Command {
        id: CommandId::ShrinkPane,
        label: "Shrink pane",
        name: "shrink-pane",
        category: CommandCategory::Layout,
        palette_key: Some('-'),
    },
    // The keyboard path for what pointer users do by dragging a float's
    // title (accessibility: every pointer behavior has a keyboard route).
    // No default letters: they are reachable by typing in the palette, and
    // users who live in floats can bind chords.
    Command {
        id: CommandId::MoveFloatLeft,
        label: "Move float left",
        name: "move-float-left",
        category: CommandCategory::Layout,
        palette_key: None,
    },
    Command {
        id: CommandId::MoveFloatRight,
        label: "Move float right",
        name: "move-float-right",
        category: CommandCategory::Layout,
        palette_key: None,
    },
    Command {
        id: CommandId::MoveFloatUp,
        label: "Move float up",
        name: "move-float-up",
        category: CommandCategory::Layout,
        palette_key: None,
    },
    Command {
        id: CommandId::MoveFloatDown,
        label: "Move float down",
        name: "move-float-down",
        category: CommandCategory::Layout,
        palette_key: None,
    },
    Command {
        id: CommandId::ShowHelp,
        label: "Help",
        name: "help",
        category: CommandCategory::App,
        palette_key: Some('?'),
    },
    Command {
        id: CommandId::SaveWorkspace,
        label: "Save workspace",
        name: "save-workspace",
        category: CommandCategory::Persistence,
        palette_key: Some('w'),
    },
    Command {
        id: CommandId::RestoreWorkspace,
        label: "Restore workspace",
        name: "restore-workspace",
        category: CommandCategory::Persistence,
        palette_key: Some('o'),
    },
    Command {
        id: CommandId::ReloadConfig,
        label: "Reload config",
        name: "reload-config",
        category: CommandCategory::Config,
        palette_key: Some('e'),
    },
    Command {
        id: CommandId::Quit,
        label: "Quit Mandatum",
        name: "quit",
        category: CommandCategory::App,
        palette_key: Some('q'),
    },
];

/// The command a config file names with this stable kebab-case key, if any.
pub fn command_id_for_name(name: &str) -> Option<CommandId> {
    BUILT_IN_COMMANDS
        .iter()
        .find(|command| command.name == name)
        .map(|command| command.id)
}

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
    CopySelection,
    ReloadConfig,
    ShowTimeline,
    ShowSessionMap,
    SearchSession,
    ShowHelp,
    // Runtime, not core: the target position derives from the pane's current
    // floating rect and the live frame size, which only the app knows.
    MoveFloatLeft,
    MoveFloatRight,
    MoveFloatUp,
    MoveFloatDown,
    Quit,
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
    SetFocusedAgentObjective,
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
    pub focused_pane_is_floating: bool,
}

impl PaletteContext {
    pub const fn focused_task() -> Self {
        Self {
            focused_pane_is_task: true,
            focused_pane_is_floating: false,
        }
    }
}

pub fn command_target(command_id: CommandId) -> CommandTarget {
    match command_id {
        CommandId::EnterCopyMode => CommandTarget::Runtime(RuntimeCommand::EnterCopyMode),
        CommandId::CopySelection => CommandTarget::Runtime(RuntimeCommand::CopySelection),
        CommandId::ReloadConfig => CommandTarget::Runtime(RuntimeCommand::ReloadConfig),
        CommandId::ShowTimeline => CommandTarget::Runtime(RuntimeCommand::ShowTimeline),
        CommandId::ShowSessionMap => CommandTarget::Runtime(RuntimeCommand::ShowSessionMap),
        CommandId::SearchSession => CommandTarget::Runtime(RuntimeCommand::SearchSession),
        CommandId::ShowHelp => CommandTarget::Runtime(RuntimeCommand::ShowHelp),
        CommandId::MoveFloatLeft => CommandTarget::Runtime(RuntimeCommand::MoveFloatLeft),
        CommandId::MoveFloatRight => CommandTarget::Runtime(RuntimeCommand::MoveFloatRight),
        CommandId::MoveFloatUp => CommandTarget::Runtime(RuntimeCommand::MoveFloatUp),
        CommandId::MoveFloatDown => CommandTarget::Runtime(RuntimeCommand::MoveFloatDown),
        CommandId::Quit => CommandTarget::Runtime(RuntimeCommand::Quit),
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
        CommandId::SetAgentObjective => {
            CommandTarget::RuntimeAgent(RuntimeAgentCommand::SetFocusedAgentObjective)
        }
        _ => CommandTarget::Core,
    }
}

/// Remappable single-letter palette bindings.
///
/// Defaults come from the `palette_key` column of [`BUILT_IN_COMMANDS`], so
/// the default keymap is defined in exactly one place, as data — including
/// `q`, which is simply the [`CommandId::Quit`] binding. The structural
/// palette keys (Escape closes, Tab/BackTab cycle focus) are not letter
/// bindings and stay fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteBindings {
    bindings: Vec<(char, CommandId)>,
}

impl Default for PaletteBindings {
    fn default() -> Self {
        Self {
            bindings: BUILT_IN_COMMANDS
                .iter()
                .filter_map(|command| command.palette_key.map(|key| (key, command.id)))
                .collect(),
        }
    }
}

impl PaletteBindings {
    /// The command bound to a palette letter, if any.
    pub fn resolve_char(&self, key: char) -> Option<CommandId> {
        self.bindings
            .iter()
            .find(|(bound, _)| *bound == key)
            .map(|(_, command_id)| *command_id)
    }

    /// The palette letter bound to a command, if any.
    pub fn key_for(&self, command_id: CommandId) -> Option<char> {
        self.bindings
            .iter()
            .find(|(_, bound)| *bound == command_id)
            .map(|(key, _)| *key)
    }

    /// Move a command onto a letter. The command's previous letter is
    /// released; if another command held the letter it is displaced (the
    /// later binding wins) and returned so callers can surface a warning.
    pub fn rebind(&mut self, command_id: CommandId, key: char) -> Option<CommandId> {
        self.bindings.retain(|(_, bound)| *bound != command_id);
        let displaced = self.resolve_char(key);
        self.bindings.retain(|(bound, _)| *bound != key);
        self.bindings.push((key, command_id));
        displaced
    }
}

pub fn resolve_palette_key(key: PaletteKey) -> PaletteInput {
    resolve_palette_key_with_context(key, PaletteContext::default())
}

pub fn resolve_palette_key_with_context(key: PaletteKey, context: PaletteContext) -> PaletteInput {
    resolve_palette_key_with_bindings(key, context, &PaletteBindings::default())
}

pub fn resolve_palette_key_with_bindings(
    key: PaletteKey,
    context: PaletteContext,
    bindings: &PaletteBindings,
) -> PaletteInput {
    let character = match key {
        PaletteKey::Escape => return PaletteInput::Close,
        PaletteKey::Tab => return PaletteInput::Dispatch(CommandId::FocusNext),
        PaletteKey::BackTab => return PaletteInput::Dispatch(CommandId::FocusPrevious),
        PaletteKey::Character(character) => character,
    };

    match bindings.resolve_char(character) {
        // Context substitution: on a focused task pane the restart letter
        // means "rerun this task", and the stop-task letter is only
        // meaningful there. On a floating pane the float letter means
        // "dock this pane" (float/dock is one toggle key).
        Some(CommandId::RestartPane) if context.focused_pane_is_task => {
            PaletteInput::Dispatch(CommandId::RerunTask)
        }
        Some(CommandId::StopTask) if !context.focused_pane_is_task => PaletteInput::Noop,
        Some(CommandId::FloatPane) if context.focused_pane_is_floating => {
            PaletteInput::Dispatch(CommandId::DockPane)
        }
        Some(CommandId::Quit) => PaletteInput::Quit,
        Some(command_id) => PaletteInput::Dispatch(command_id),
        None => PaletteInput::Noop,
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
        CommandId::DockPane => CoreAction::DockFocused,
        CommandId::StackPanes => CoreAction::StackFocusedWithNext,
        CommandId::GrowPane => CoreAction::ResizeFocused {
            delta_percent: RESIZE_STEP_PERCENT,
        },
        CommandId::ShrinkPane => CoreAction::ResizeFocused {
            delta_percent: -RESIZE_STEP_PERCENT,
        },
        CommandId::SaveWorkspace => CoreAction::SaveWorkspace,
        CommandId::RestoreWorkspace => CoreAction::RestoreWorkspace,
        CommandId::EnterCopyMode
        | CommandId::CopySelection
        | CommandId::ReloadConfig
        | CommandId::ShowTimeline
        | CommandId::ShowSessionMap
        | CommandId::SearchSession
        | CommandId::ShowHelp
        | CommandId::MoveFloatLeft
        | CommandId::MoveFloatRight
        | CommandId::MoveFloatUp
        | CommandId::MoveFloatDown
        | CommandId::Quit
        | CommandId::RunTask
        | CommandId::RerunTask
        | CommandId::StopTask
        | CommandId::NewAgentPane
        | CommandId::StartAgent
        | CommandId::StopAgent
        | CommandId::ApproveAgentAction
        | CommandId::RejectAgentAction
        | CommandId::FocusNextWaitingAgent
        | CommandId::SetAgentObjective => {
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
    fn command_names_and_default_palette_keys_are_unique() {
        let mut names = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        for command in BUILT_IN_COMMANDS {
            assert!(
                names.insert(command.name),
                "duplicate name {}",
                command.name
            );
            assert_eq!(command_id_for_name(command.name), Some(command.id));
            if let Some(key) = command.palette_key {
                assert!(keys.insert(key), "duplicate palette key {key}");
                // The quit letter is owned by the Quit command itself, so it
                // is searchable, rebindable, and shown like every other key.
                assert!(
                    key != 'q' || command.id == CommandId::Quit,
                    "q belongs to the quit command"
                );
            }
        }
        assert_eq!(command_id_for_name("not-a-command"), None);
    }

    #[test]
    fn quit_is_a_listed_runtime_command_and_q_still_quits_the_palette() {
        let quit = command_for_id(CommandId::Quit).unwrap();
        assert_eq!(quit.label, "Quit Mandatum");
        assert_eq!(quit.palette_key, Some('q'));
        assert_eq!(
            command_target(CommandId::Quit),
            CommandTarget::Runtime(RuntimeCommand::Quit)
        );
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('q')),
            PaletteInput::Quit
        );
    }

    #[test]
    fn dock_grow_and_shrink_map_to_core_layout_actions() {
        let context = CommandContext::for_project("w", "/tmp/p");
        assert_eq!(
            core_action_for_command(CommandId::DockPane, &context).unwrap(),
            CoreAction::DockFocused
        );
        assert_eq!(
            core_action_for_command(CommandId::GrowPane, &context).unwrap(),
            CoreAction::ResizeFocused {
                delta_percent: RESIZE_STEP_PERCENT,
            }
        );
        assert_eq!(
            core_action_for_command(CommandId::ShrinkPane, &context).unwrap(),
            CoreAction::ResizeFocused {
                delta_percent: -RESIZE_STEP_PERCENT,
            }
        );
    }

    #[test]
    fn float_letter_docks_when_the_focused_pane_is_floating() {
        let floating = PaletteContext {
            focused_pane_is_floating: true,
            ..PaletteContext::default()
        };
        assert_eq!(
            resolve_palette_key_with_context(PaletteKey::Character('f'), floating),
            PaletteInput::Dispatch(CommandId::DockPane)
        );
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('f')),
            PaletteInput::Dispatch(CommandId::FloatPane)
        );
    }

    #[test]
    fn palette_rebind_moves_the_command_and_displaces_the_previous_owner() {
        let mut bindings = PaletteBindings::default();

        // Move SplitRight onto NewTerminal's letter: later binding wins.
        let displaced = bindings.rebind(CommandId::SplitRight, 'n');
        assert_eq!(displaced, Some(CommandId::NewTerminal));
        assert_eq!(bindings.resolve_char('n'), Some(CommandId::SplitRight));
        // SplitRight's old letter is released, not left dangling.
        assert_eq!(bindings.resolve_char('v'), None);
        assert_eq!(bindings.key_for(CommandId::NewTerminal), None);

        // An overridden 'q' dispatches instead of quitting.
        bindings.rebind(CommandId::ZoomPane, 'q');
        assert_eq!(
            resolve_palette_key_with_bindings(
                PaletteKey::Character('q'),
                PaletteContext::default(),
                &bindings,
            ),
            PaletteInput::Dispatch(CommandId::ZoomPane)
        );
    }

    #[test]
    fn help_and_float_moves_are_runtime_commands() {
        assert_eq!(
            command_target(CommandId::ShowHelp),
            CommandTarget::Runtime(RuntimeCommand::ShowHelp)
        );
        assert_eq!(command_id_for_name("help"), Some(CommandId::ShowHelp));
        assert_eq!(
            resolve_palette_key(PaletteKey::Character('?')),
            PaletteInput::Dispatch(CommandId::ShowHelp)
        );
        for (command_id, expected) in [
            (CommandId::MoveFloatLeft, RuntimeCommand::MoveFloatLeft),
            (CommandId::MoveFloatRight, RuntimeCommand::MoveFloatRight),
            (CommandId::MoveFloatUp, RuntimeCommand::MoveFloatUp),
            (CommandId::MoveFloatDown, RuntimeCommand::MoveFloatDown),
        ] {
            assert_eq!(command_target(command_id), CommandTarget::Runtime(expected));
            let mut workspace = Workspace::new("w", PathBuf::from("/tmp/p"));
            let context = CommandContext::for_project("w", "/tmp/p");
            let result = dispatch_command(&mut workspace, &context, command_id);
            assert!(matches!(result, Err(CommandError::NotACoreCommand(id)) if id == command_id));
        }
    }

    #[test]
    fn reload_config_is_a_runtime_command() {
        assert_eq!(
            command_target(CommandId::ReloadConfig),
            CommandTarget::Runtime(RuntimeCommand::ReloadConfig)
        );
        assert_eq!(
            command_id_for_name("reload-config"),
            Some(CommandId::ReloadConfig)
        );
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
