use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    AgentPaneIntent, Layout, PaneId, PaneKind, PaneSpec, ProjectId, SessionId, SplitDirection,
    TaskPaneIntent, layout::LayoutMutationError,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    id: SessionId,
    project_id: ProjectId,
    name: String,
    panes: BTreeMap<PaneId, PaneSpec>,
    layout: Layout,
    focused_pane: PaneId,
    next_pane_index: u64,
}

impl Session {
    pub fn new(
        id: SessionId,
        project_id: ProjectId,
        name: impl Into<String>,
        project_path: PathBuf,
    ) -> Self {
        let first_pane_id = PaneId::new("pane-1");
        let first_pane = PaneSpec::terminal(first_pane_id.clone(), "terminal", Some(project_path));
        let mut panes = BTreeMap::new();
        panes.insert(first_pane_id.clone(), first_pane);

        Self {
            id,
            project_id,
            name: name.into(),
            panes,
            layout: Layout::new(first_pane_id.clone()),
            focused_pane: first_pane_id,
            next_pane_index: 2,
        }
    }

    pub fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn panes(&self) -> &BTreeMap<PaneId, PaneSpec> {
        &self.panes
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn focused_pane_id(&self) -> &PaneId {
        &self.focused_pane
    }

    pub fn pane(&self, pane_id: &PaneId) -> Option<&PaneSpec> {
        self.panes.get(pane_id)
    }

    pub fn focus_order(&self) -> Vec<PaneId> {
        self.layout.pane_order()
    }

    pub fn split_focused(&mut self, direction: SplitDirection) -> Result<PaneId, SessionError> {
        self.ensure_pane_exists(&self.focused_pane)?;

        let new_pane_id = self.next_pane_id();
        let cwd = self
            .panes
            .get(&self.focused_pane)
            .and_then(|pane| pane.cwd().cloned());
        let title = format!("terminal {}", self.next_pane_index - 1);
        let pane = PaneSpec::terminal(new_pane_id.clone(), title, cwd);

        self.layout
            .split_pane(&self.focused_pane, new_pane_id.clone(), direction)?;
        self.panes.insert(new_pane_id.clone(), pane);
        self.focused_pane = new_pane_id.clone();
        Ok(new_pane_id)
    }

    pub fn add_terminal_pane(&mut self, title: impl Into<String>, cwd: Option<PathBuf>) -> PaneId {
        self.add_floating_pane(title, PaneKind::Terminal { command: None }, cwd)
    }

    pub fn add_task_pane(&mut self, title: impl Into<String>, intent: TaskPaneIntent) -> PaneId {
        let cwd = intent.cwd.clone();
        self.add_floating_pane(title, PaneKind::Task { intent }, cwd)
    }

    pub fn add_agent_pane(
        &mut self,
        title: impl Into<String>,
        intent: AgentPaneIntent,
        cwd: Option<PathBuf>,
    ) -> PaneId {
        self.add_floating_pane(title, PaneKind::Agent { intent }, cwd)
    }

    /// Mutable access to a pane's durable agent intent, when the pane exists
    /// and is an agent pane.
    pub fn agent_intent_mut(&mut self, pane_id: &PaneId) -> Option<&mut AgentPaneIntent> {
        self.panes
            .get_mut(pane_id)
            .and_then(PaneSpec::agent_intent_mut)
    }

    /// Mutable access to every agent intent in this session.
    pub fn agent_intents_mut(&mut self) -> impl Iterator<Item = &mut AgentPaneIntent> {
        self.panes
            .values_mut()
            .filter_map(PaneSpec::agent_intent_mut)
    }

    pub fn add_floating_pane(
        &mut self,
        title: impl Into<String>,
        kind: PaneKind,
        cwd: Option<PathBuf>,
    ) -> PaneId {
        let pane_id = self.next_pane_id();
        let pane = PaneSpec::new(pane_id.clone(), title, kind, cwd);
        self.layout.add_floating(pane_id.clone());
        self.panes.insert(pane_id.clone(), pane);
        self.focused_pane = pane_id.clone();
        pane_id
    }

    pub fn focus_next(&mut self) -> Result<&PaneId, SessionError> {
        self.move_focus(1)
    }

    pub fn focus_previous(&mut self) -> Result<&PaneId, SessionError> {
        self.move_focus(-1)
    }

    pub fn focus_pane(&mut self, pane_id: PaneId) -> Result<&PaneId, SessionError> {
        self.ensure_pane_exists(&pane_id)?;
        self.focused_pane = pane_id;
        Ok(&self.focused_pane)
    }

    pub fn close_focused(&mut self) -> Result<PaneId, SessionError> {
        let closing = self.focused_pane.clone();
        self.ensure_pane_exists(&closing)?;

        let order_before = self.focus_order();
        let closing_index = order_before
            .iter()
            .position(|pane_id| pane_id == &closing)
            .unwrap_or(0);

        self.layout.remove_pane(&closing)?;
        self.panes.remove(&closing);

        let order_after = self.focus_order();
        let next_focus = order_after
            .get(closing_index)
            .or_else(|| order_after.last())
            .ok_or(SessionError::InvalidLayout(
                "closing pane left the session without any focusable panes".to_owned(),
            ))?
            .clone();
        self.focused_pane = next_focus.clone();
        Ok(next_focus)
    }

    pub fn restart_focused(&mut self) -> Result<&PaneId, SessionError> {
        let focused = self.focused_pane.clone();
        let pane = self
            .panes
            .get_mut(&focused)
            .ok_or_else(|| SessionError::PaneNotFound(focused.clone()))?;
        pane.request_restart();
        Ok(&self.focused_pane)
    }

    pub fn rename_focused(&mut self, title: impl Into<String>) -> Result<&PaneId, SessionError> {
        let focused = self.focused_pane.clone();
        let pane = self
            .panes
            .get_mut(&focused)
            .ok_or_else(|| SessionError::PaneNotFound(focused.clone()))?;
        pane.rename(title);
        Ok(&self.focused_pane)
    }

    pub fn toggle_zoom_focused(&mut self) -> Result<&PaneId, SessionError> {
        self.ensure_pane_exists(&self.focused_pane)?;
        self.layout.toggle_zoom(&self.focused_pane);
        Ok(&self.focused_pane)
    }

    pub fn float_focused(&mut self) -> Result<&PaneId, SessionError> {
        self.ensure_pane_exists(&self.focused_pane)?;
        self.layout.float_pane(&self.focused_pane)?;
        Ok(&self.focused_pane)
    }

    pub fn dock_focused(&mut self) -> Result<&PaneId, SessionError> {
        self.ensure_pane_exists(&self.focused_pane)?;
        self.layout.dock_pane(&self.focused_pane)?;
        Ok(&self.focused_pane)
    }

    pub fn resize_focused(&mut self, delta_percent: i8) -> Result<&PaneId, SessionError> {
        self.ensure_pane_exists(&self.focused_pane)?;
        self.layout.resize_pane(&self.focused_pane, delta_percent)?;
        Ok(&self.focused_pane)
    }

    pub fn stack_focused_with_next(&mut self) -> Result<&PaneId, SessionError> {
        self.ensure_pane_exists(&self.focused_pane)?;
        self.layout.stack_with_next(&self.focused_pane)?;
        Ok(&self.focused_pane)
    }

    pub fn set_split_ratio(
        &mut self,
        split_index: usize,
        first_percent: u8,
    ) -> Result<(), SessionError> {
        self.layout.set_split_percent(split_index, first_percent)?;
        Ok(())
    }

    pub fn move_floating_pane(
        &mut self,
        pane_id: &PaneId,
        x: u16,
        y: u16,
    ) -> Result<(), SessionError> {
        self.layout.set_floating_position(pane_id, x, y)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), SessionError> {
        if !self.panes.contains_key(&self.focused_pane) {
            return Err(SessionError::PaneNotFound(self.focused_pane.clone()));
        }

        for pane_id in self.layout.pane_order() {
            if !self.panes.contains_key(&pane_id) {
                return Err(SessionError::InvalidLayout(format!(
                    "layout references missing pane {pane_id}"
                )));
            }
        }

        Ok(())
    }

    fn next_pane_id(&mut self) -> PaneId {
        let pane_id = PaneId::new(format!("pane-{}", self.next_pane_index));
        self.next_pane_index += 1;
        pane_id
    }

    fn move_focus(&mut self, offset: isize) -> Result<&PaneId, SessionError> {
        let order = self.focus_order();
        if order.is_empty() {
            return Err(SessionError::InvalidLayout(
                "session has no focusable panes".to_owned(),
            ));
        }

        let current_index = order
            .iter()
            .position(|pane_id| pane_id == &self.focused_pane)
            .ok_or_else(|| SessionError::PaneNotFound(self.focused_pane.clone()))?;
        let next_index =
            (current_index as isize + offset).rem_euclid(order.len() as isize) as usize;
        self.focused_pane = order[next_index].clone();
        Ok(&self.focused_pane)
    }

    fn ensure_pane_exists(&self, pane_id: &PaneId) -> Result<(), SessionError> {
        if self.panes.contains_key(pane_id) && self.layout.contains_pane(pane_id) {
            Ok(())
        } else {
            Err(SessionError::PaneNotFound(pane_id.clone()))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionError {
    PaneNotFound(PaneId),
    Layout(LayoutMutationError),
    InvalidLayout(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PaneNotFound(pane_id) => write!(formatter, "pane {pane_id} was not found"),
            Self::Layout(error) => write!(formatter, "{error}"),
            Self::InvalidLayout(message) => write!(formatter, "invalid layout: {message}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<LayoutMutationError> for SessionError {
    fn from(error: LayoutMutationError) -> Self {
        Self::Layout(error)
    }
}
