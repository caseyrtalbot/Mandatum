use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    ActionOutcome, CoreAction, PersistenceError, PersistenceRequest, ProjectId, Session,
    SessionError, SessionId, SplitDirection, WorkspaceId, deserialize_workspace,
    serialize_workspace,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    id: WorkspaceId,
    name: String,
    projects: BTreeMap<ProjectId, Project>,
    sessions: BTreeMap<SessionId, Session>,
    active_project_id: ProjectId,
    active_session_id: SessionId,
    next_project_index: u64,
    next_session_index: u64,
}

impl Workspace {
    pub fn new(name: impl Into<String>, project_path: PathBuf) -> Self {
        let workspace_id = WorkspaceId::new("workspace-1");
        let project_id = ProjectId::new("project-1");
        let session_id = SessionId::new("session-1");
        let project_name = path_name(&project_path).unwrap_or_else(|| "project".to_owned());

        let project = Project::new(
            project_id.clone(),
            project_name.clone(),
            project_path.clone(),
        );
        let session = Session::new(
            session_id.clone(),
            project_id.clone(),
            project_name,
            project_path,
        );

        let mut projects = BTreeMap::new();
        projects.insert(project_id.clone(), project);
        let mut sessions = BTreeMap::new();
        sessions.insert(session_id.clone(), session);

        Self {
            id: workspace_id,
            name: name.into(),
            projects,
            sessions,
            active_project_id: project_id,
            active_session_id: session_id,
            next_project_index: 2,
            next_session_index: 2,
        }
    }

    pub fn id(&self) -> &WorkspaceId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn projects(&self) -> &BTreeMap<ProjectId, Project> {
        &self.projects
    }

    pub fn sessions(&self) -> &BTreeMap<SessionId, Session> {
        &self.sessions
    }

    /// The active project's directory: the default working directory for
    /// panes whose intent does not name one.
    pub fn active_project_path(&self) -> &Path {
        self.projects
            .get(&self.active_project_id)
            .map(Project::path)
            .expect("active project should be validated by workspace constructors")
    }

    pub fn active_session(&self) -> &Session {
        self.sessions
            .get(&self.active_session_id)
            .expect("active session should be validated by workspace constructors")
    }

    pub fn active_session_mut(&mut self) -> &mut Session {
        self.sessions
            .get_mut(&self.active_session_id)
            .expect("active session should be validated by workspace constructors")
    }

    /// Mutable access to every session, active or not. Panes outside the
    /// active session keep durable state (agent intents) that the runtime
    /// layer must be able to reconcile.
    pub fn sessions_mut(&mut self) -> impl Iterator<Item = &mut Session> {
        self.sessions.values_mut()
    }

    /// Make an existing session (and its project) active. Unknown sessions
    /// error; nothing is created.
    pub fn activate_session(&mut self, session_id: &SessionId) -> Result<(), WorkspaceError> {
        let Some(session) = self.sessions.get(session_id) else {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "session {session_id} was not found"
            )));
        };
        self.active_project_id = session.project_id().clone();
        self.active_session_id = session_id.clone();
        Ok(())
    }

    pub fn open_project(&mut self, name: String, path: PathBuf) -> SessionId {
        let project_id = self.next_project_id();
        let session_id = self.next_session_id();
        let project = Project::new(project_id.clone(), name.clone(), path.clone());
        let session = Session::new(session_id.clone(), project_id.clone(), name, path);

        self.projects.insert(project_id.clone(), project);
        self.sessions.insert(session_id.clone(), session);
        self.active_project_id = project_id;
        self.active_session_id = session_id.clone();
        session_id
    }

    pub fn apply_action(&mut self, action: CoreAction) -> Result<ActionOutcome, WorkspaceError> {
        match action {
            CoreAction::OpenProject { name, path } => {
                self.open_project(name, path);
                Ok(self.mutated_outcome())
            }
            CoreAction::ActivateSession { session_id } => {
                self.activate_session(&session_id)?;
                Ok(self.mutated_outcome())
            }
            CoreAction::NewTerminal { title, cwd } => {
                self.active_session_mut().add_terminal_pane(title, cwd);
                Ok(self.mutated_outcome())
            }
            CoreAction::CreateTaskPane { title, intent } => {
                self.active_session_mut().add_task_pane(title, intent);
                Ok(self.mutated_outcome())
            }
            CoreAction::CreateAgentPane { title, intent, cwd } => {
                self.active_session_mut().add_agent_pane(title, intent, cwd);
                Ok(self.mutated_outcome())
            }
            CoreAction::SplitRight => {
                self.active_session_mut()
                    .split_focused(SplitDirection::Right)?;
                Ok(self.mutated_outcome())
            }
            CoreAction::SplitDown => {
                self.active_session_mut()
                    .split_focused(SplitDirection::Down)?;
                Ok(self.mutated_outcome())
            }
            CoreAction::FocusNext => {
                self.active_session_mut().focus_next()?;
                Ok(self.mutated_outcome())
            }
            CoreAction::FocusPrevious => {
                self.active_session_mut().focus_previous()?;
                Ok(self.mutated_outcome())
            }
            CoreAction::FocusPane { pane_id } => {
                self.active_session_mut().focus_pane(pane_id)?;
                Ok(self.mutated_outcome())
            }
            CoreAction::CloseFocused => {
                self.active_session_mut().close_focused()?;
                Ok(self.mutated_outcome())
            }
            CoreAction::RestartFocused => {
                self.active_session_mut().restart_focused()?;
                Ok(self.mutated_outcome())
            }
            CoreAction::RenameFocused { title } => {
                self.active_session_mut().rename_focused(title)?;
                Ok(self.mutated_outcome())
            }
            CoreAction::ToggleZoomFocused => {
                self.active_session_mut().toggle_zoom_focused()?;
                Ok(self.mutated_outcome())
            }
            CoreAction::FloatFocused => {
                self.active_session_mut().float_focused()?;
                Ok(self.mutated_outcome())
            }
            CoreAction::DockFocused => {
                self.active_session_mut().dock_focused()?;
                Ok(self.mutated_outcome())
            }
            CoreAction::ResizeFocused { delta_percent } => {
                self.active_session_mut().resize_focused(delta_percent)?;
                Ok(self.mutated_outcome())
            }
            CoreAction::StackFocusedWithNext => {
                self.active_session_mut().stack_focused_with_next()?;
                Ok(self.mutated_outcome())
            }
            CoreAction::SetSplitRatio {
                split_index,
                first_percent,
            } => {
                self.active_session_mut()
                    .set_split_ratio(split_index, first_percent)?;
                Ok(self.mutated_outcome())
            }
            CoreAction::MoveFloatingPane { pane_id, x, y } => {
                self.active_session_mut()
                    .move_floating_pane(&pane_id, x, y)?;
                Ok(self.mutated_outcome())
            }
            CoreAction::SaveWorkspace => Ok(ActionOutcome::PersistenceRequested(
                PersistenceRequest::SaveWorkspace,
            )),
            CoreAction::RestoreWorkspace => Ok(ActionOutcome::PersistenceRequested(
                PersistenceRequest::RestoreWorkspace,
            )),
        }
    }

    pub fn to_json(&self) -> Result<String, PersistenceError> {
        serialize_workspace(self)
    }

    pub fn from_json(input: &str) -> Result<Self, PersistenceError> {
        deserialize_workspace(input)
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if !self.projects.contains_key(&self.active_project_id) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "active project {} is missing",
                self.active_project_id
            )));
        }

        if !self.sessions.contains_key(&self.active_session_id) {
            return Err(WorkspaceError::InvalidWorkspace(format!(
                "active session {} is missing",
                self.active_session_id
            )));
        }

        for session in self.sessions.values() {
            if !self.projects.contains_key(session.project_id()) {
                return Err(WorkspaceError::InvalidWorkspace(format!(
                    "session {} references missing project {}",
                    session.id(),
                    session.project_id()
                )));
            }
            session.validate()?;
        }

        Ok(())
    }

    fn mutated_outcome(&self) -> ActionOutcome {
        ActionOutcome::Mutated {
            focused_pane: self.active_session().focused_pane_id().clone(),
        }
    }

    fn next_project_id(&mut self) -> ProjectId {
        let id = ProjectId::new(format!("project-{}", self.next_project_index));
        self.next_project_index += 1;
        id
    }

    fn next_session_id(&mut self) -> SessionId {
        let id = SessionId::new(format!("session-{}", self.next_session_index));
        self.next_session_index += 1;
        id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    id: ProjectId,
    name: String,
    path: PathBuf,
}

impl Project {
    pub fn new(id: ProjectId, name: impl Into<String>, path: PathBuf) -> Self {
        Self {
            id,
            name: name.into(),
            path,
        }
    }

    pub fn id(&self) -> &ProjectId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    Session(SessionError),
    InvalidWorkspace(String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(error) => write!(formatter, "{error}"),
            Self::InvalidWorkspace(message) => write!(formatter, "invalid workspace: {message}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<SessionError> for WorkspaceError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

fn path_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
}
