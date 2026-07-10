//! The session map: a tree of every session and its panes, with one-word
//! live states, focus markers, and layout badges. Pure model building here;
//! `app_state` owns the open/close/jump wiring.

use mandatum_core::{AgentStatus, PaneId, PaneKind, SessionId, Workspace};
use mandatum_scene::{SceneSize, SessionMapOverlay, SessionMapRow, layout::session_map_rect};

/// Live overlay state while the session map is open. Runtime presentation
/// only; never serialized.
pub(crate) struct SessionMapState {
    pub(crate) selected: usize,
}

/// What a session-map row activates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionMapTarget {
    Session(SessionId),
    Pane {
        session_id: SessionId,
        pane_id: PaneId,
    },
}

/// One resolved row: the scene row plus what Enter/click on it means.
pub(crate) struct SessionMapRowModel {
    pub(crate) target: SessionMapTarget,
    pub(crate) row: SessionMapRow,
}

/// Build the session→pane tree. `live_state` reports the one-word runtime
/// state for panes that have one (only the active session has live
/// runtimes); everything else falls back to durable intent.
pub(crate) fn session_map_rows(
    workspace: &Workspace,
    live_state: &dyn Fn(&PaneId) -> Option<String>,
) -> Vec<SessionMapRowModel> {
    let active_session_id = workspace.active_session().id().clone();
    let mut rows = Vec::new();

    for (session_id, session) in workspace.sessions() {
        let active = *session_id == active_session_id;
        let active_mark = if active { " (active)" } else { "" };
        rows.push(SessionMapRowModel {
            target: SessionMapTarget::Session(session_id.clone()),
            row: SessionMapRow {
                depth: 0,
                glyph: "▸".to_owned(),
                label: format!(
                    "{} · {} · {} pane(s){active_mark}",
                    session_id,
                    session.name(),
                    session.panes().len()
                ),
                state: String::new(),
                focused: false,
                badges: String::new(),
            },
        });

        let layout = session.layout();
        for (pane_id, pane) in session.panes() {
            let state = if active {
                live_state(pane_id).unwrap_or_else(|| durable_pane_state(pane.kind()))
            } else {
                durable_pane_state(pane.kind())
            };
            let mut badges = Vec::new();
            if layout.zoomed() == Some(pane_id) {
                badges.push("zoom");
            }
            if layout.is_floating(pane_id) {
                badges.push("float");
            }
            rows.push(SessionMapRowModel {
                target: SessionMapTarget::Pane {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                },
                row: SessionMapRow {
                    depth: 1,
                    glyph: pane_glyph(pane.kind()).to_owned(),
                    label: format!("{pane_id} {}", pane.title()),
                    state,
                    focused: active && session.focused_pane_id() == pane_id,
                    badges: badges.join(" "),
                },
            });
        }
    }

    rows
}

fn pane_glyph(kind: &PaneKind) -> &'static str {
    match kind {
        PaneKind::Terminal { .. } => "❯",
        PaneKind::Task { .. } => "▶",
        PaneKind::Agent { .. } => "◆",
        PaneKind::StatusLog { .. } => "≡",
    }
}

/// The one-word state durable intent alone supports (no live runtime).
fn durable_pane_state(kind: &PaneKind) -> String {
    match kind {
        PaneKind::Terminal { .. } | PaneKind::StatusLog { .. } | PaneKind::Task { .. } => {
            "idle".to_owned()
        }
        PaneKind::Agent { intent } => agent_state_word(&intent.status).to_owned(),
    }
}

pub(crate) fn agent_state_word(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Draft | AgentStatus::Unknown => "idle",
        AgentStatus::Running => "running",
        AgentStatus::WaitingForApproval => "waiting-approval",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Failed => "failed",
        AgentStatus::Complete => "complete",
    }
}

/// Build the overlay scene for the current rows and selection.
pub(crate) fn session_map_overlay(
    rows: &[SessionMapRowModel],
    selected: usize,
    size: SceneSize,
) -> SessionMapOverlay {
    let selected = selected.min(rows.len().saturating_sub(1));
    SessionMapOverlay {
        area: session_map_rect(size),
        rows: rows.iter().map(|model| model.row.clone()).collect(),
        selected,
        footer: "↑/↓ move · enter focus · esc close".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mandatum_core::{AgentPaneIntent, CoreAction, TaskPaneIntent};

    use super::*;

    #[test]
    fn rows_tree_sessions_panes_states_focus_and_badges() {
        let mut workspace = Workspace::new("Mandatum", PathBuf::from("/tmp/project"));
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace
            .apply_action(CoreAction::CreateAgentPane {
                title: "agent".to_owned(),
                intent: {
                    let mut intent = AgentPaneIntent::draft("review");
                    intent.status = AgentStatus::WaitingForApproval;
                    intent
                },
                cwd: None,
            })
            .unwrap();
        workspace
            .apply_action(CoreAction::OpenProject {
                name: "other".to_owned(),
                path: PathBuf::from("/tmp/other"),
            })
            .unwrap();
        workspace
            .apply_action(CoreAction::CreateTaskPane {
                title: "checks".to_owned(),
                intent: TaskPaneIntent {
                    recipe_id: None,
                    command: "cargo test".to_owned(),
                    cwd: None,
                },
            })
            .unwrap();

        // Session 2 is active; pane-1 of session 2 reports a live state.
        let live =
            |pane_id: &PaneId| (pane_id == &PaneId::new("pane-1")).then(|| "running".to_owned());
        let rows = session_map_rows(&workspace, &live);

        // Tree shape: session-1 heading, its 3 panes, session-2 heading,
        // its 2 panes.
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0].row.depth, 0);
        assert!(rows[0].row.label.contains("session-1"));
        assert!(!rows[0].row.label.contains("(active)"));
        assert_eq!(rows[4].row.depth, 0);
        assert!(rows[4].row.label.contains("session-2"));
        assert!(rows[4].row.label.contains("(active)"));

        // Inactive-session panes fall back to durable state; the agent's
        // durable status shows through.
        let agent_row = rows
            .iter()
            .find(|model| model.row.label.contains("agent"))
            .unwrap();
        assert_eq!(agent_row.row.state, "waiting-approval");
        assert_eq!(agent_row.row.glyph, "◆");
        assert_eq!(agent_row.row.badges, "float");
        assert!(!agent_row.row.focused);

        // The active session's live states and focus marker.
        let live_terminal = &rows[5];
        assert_eq!(live_terminal.row.state, "running");
        assert!(!live_terminal.row.focused);
        let task_row = &rows[6];
        assert_eq!(task_row.row.glyph, "▶");
        assert_eq!(task_row.row.state, "idle");
        assert!(task_row.row.focused, "the focused pane carries the marker");
        assert_eq!(
            task_row.target,
            SessionMapTarget::Pane {
                session_id: SessionId::new("session-2"),
                pane_id: PaneId::new("pane-2"),
            }
        );
    }

    #[test]
    fn zoom_badge_follows_the_layout() {
        let mut workspace = Workspace::new("Mandatum", PathBuf::from("/tmp/project"));
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace
            .apply_action(CoreAction::ToggleZoomFocused)
            .unwrap();

        let rows = session_map_rows(&workspace, &|_| None);
        let zoomed = rows
            .iter()
            .find(|model| model.row.label.starts_with("pane-2"))
            .unwrap();
        assert_eq!(zoomed.row.badges, "zoom");
    }

    #[test]
    fn overlay_clamps_selection_and_names_its_keys() {
        let workspace = Workspace::new("Mandatum", PathBuf::from("/tmp/project"));
        let rows = session_map_rows(&workspace, &|_| None);
        let overlay = session_map_overlay(&rows, 99, SceneSize::new(100, 30));
        assert_eq!(overlay.selected, rows.len() - 1);
        assert_eq!(overlay.rows.len(), rows.len());
        assert!(overlay.footer.contains("enter focus"));
    }
}
