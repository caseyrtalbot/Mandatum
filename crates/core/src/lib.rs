//! Renderer-neutral workspace/session/layout domain for Mandatum.

mod action;
mod ids;
mod layout;
mod pane;
mod persistence;
mod session;
mod workspace;

pub use action::{ActionOutcome, CoreAction, PersistenceRequest};
pub use ids::{PaneId, ProjectId, SessionId, WorkspaceId};
pub use layout::{FloatingPane, FloatingRect, Layout, LayoutNode, SplitAxis, SplitDirection};
pub use pane::{AgentPaneIntent, AgentStatus, PaneKind, PaneSpec, StatusLogSource, TaskPaneIntent};
pub use persistence::{
    PersistedWorkspace, PersistenceError, SESSION_SCHEMA_VERSION, deserialize_workspace,
    serialize_workspace,
};
pub use session::{Session, SessionError};
pub use workspace::{Project, Workspace, WorkspaceError};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn workspace() -> Workspace {
        Workspace::new("Mandatum", PathBuf::from("/tmp/project"))
    }

    #[test]
    fn creates_workspace_with_default_project_session_and_pane() {
        let workspace = workspace();

        assert_eq!(workspace.name(), "Mandatum");
        assert_eq!(workspace.projects().len(), 1);
        assert_eq!(workspace.sessions().len(), 1);

        let session = workspace.active_session();
        assert_eq!(session.panes().len(), 1);
        assert_eq!(session.focused_pane_id().as_str(), "pane-1");
        assert_eq!(
            session.layout().root(),
            &LayoutNode::Pane {
                pane_id: PaneId::new("pane-1"),
            }
        );
    }

    #[test]
    fn split_right_and_down_build_deterministic_layout_tree() {
        let mut workspace = workspace();

        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::SplitDown).unwrap();

        let session = workspace.active_session();
        assert_eq!(
            session.layout().root(),
            &LayoutNode::Split {
                axis: SplitAxis::Horizontal,
                first_percent: 50,
                first: Box::new(LayoutNode::Pane {
                    pane_id: PaneId::new("pane-1"),
                }),
                second: Box::new(LayoutNode::Split {
                    axis: SplitAxis::Vertical,
                    first_percent: 50,
                    first: Box::new(LayoutNode::Pane {
                        pane_id: PaneId::new("pane-2"),
                    }),
                    second: Box::new(LayoutNode::Pane {
                        pane_id: PaneId::new("pane-3"),
                    }),
                }),
            }
        );
        assert_eq!(session.focused_pane_id().as_str(), "pane-3");
    }

    #[test]
    fn stack_and_floating_representations_serialize_as_durable_intent() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace
            .apply_action(CoreAction::StackFocusedWithNext)
            .unwrap();

        let intent = TaskPaneIntent {
            recipe_id: Some("build".to_owned()),
            command: "cargo build".to_owned(),
            cwd: Some(PathBuf::from("/tmp/project")),
        };
        workspace
            .apply_action(CoreAction::CreateTaskPane {
                title: "task".to_owned(),
                intent: intent.clone(),
            })
            .unwrap();

        let session = workspace.active_session();
        let floating = session.focused_pane_id();

        assert_eq!(floating.as_str(), "pane-3");
        assert_eq!(
            session.pane(floating).map(PaneSpec::kind),
            Some(&PaneKind::Task {
                intent: intent.clone(),
            })
        );

        let serialized = workspace.to_json().unwrap();
        assert!(serialized.contains(r#""type": "stack""#));
        assert!(serialized.contains(r#""floating""#));
        assert!(serialized.contains(r#""command": "cargo build""#));
    }

    #[test]
    fn focus_next_previous_and_close_are_deterministic() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::SplitDown).unwrap();

        workspace.apply_action(CoreAction::FocusPrevious).unwrap();
        assert_eq!(
            workspace.active_session().focused_pane_id().as_str(),
            "pane-2"
        );

        workspace.apply_action(CoreAction::FocusNext).unwrap();
        assert_eq!(
            workspace.active_session().focused_pane_id().as_str(),
            "pane-3"
        );

        workspace.apply_action(CoreAction::CloseFocused).unwrap();
        let session = workspace.active_session();
        assert_eq!(session.focused_pane_id().as_str(), "pane-2");
        assert_eq!(
            session.focus_order(),
            vec![PaneId::new("pane-1"), PaneId::new("pane-2")]
        );
    }

    #[test]
    fn zoom_preserves_underlying_layout_intent() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();

        let before = workspace.active_session().layout().root().clone();
        workspace
            .apply_action(CoreAction::ToggleZoomFocused)
            .unwrap();

        let session = workspace.active_session();
        assert_eq!(session.layout().zoomed(), Some(session.focused_pane_id()));
        assert_eq!(session.layout().root(), &before);
    }

    #[test]
    fn restart_and_rename_are_pane_intent_only() {
        let mut workspace = workspace();

        workspace
            .apply_action(CoreAction::RenameFocused {
                title: "editor".to_owned(),
            })
            .unwrap();
        workspace.apply_action(CoreAction::RestartFocused).unwrap();

        let pane = workspace
            .active_session()
            .pane(workspace.active_session().focused_pane_id())
            .unwrap();
        assert_eq!(pane.title(), "editor");
        assert_eq!(pane.restart_generation(), 1);
    }

    #[test]
    fn invalid_corrupt_and_unsupported_session_data_returns_structured_errors() {
        let corrupt = Workspace::from_json("{ not-json }").unwrap_err();
        assert!(matches!(corrupt, PersistenceError::CorruptJson { .. }));

        let unsupported =
            Workspace::from_json(r#"{"schema_version":99,"workspace":{}}"#).unwrap_err();
        assert_eq!(
            unsupported,
            PersistenceError::UnsupportedSchema {
                found: 99,
                supported: SESSION_SCHEMA_VERSION,
            }
        );

        let invalid = Workspace::from_json(r#"{"schema_version":1,"workspace":{}}"#).unwrap_err();
        assert!(matches!(invalid, PersistenceError::InvalidSession { .. }));
    }

    #[test]
    fn task_and_agent_panes_persist_without_runtime_handles() {
        let mut workspace = workspace();
        let session = workspace.active_session_mut();

        session.add_floating_pane(
            "tests",
            PaneKind::Task {
                intent: TaskPaneIntent {
                    recipe_id: Some("test".to_owned()),
                    command: "cargo test".to_owned(),
                    cwd: Some(PathBuf::from("/tmp/project")),
                },
            },
            Some(PathBuf::from("/tmp/project")),
        );
        session.add_floating_pane(
            "agent",
            PaneKind::Agent {
                intent: AgentPaneIntent {
                    thread_id: Some("thread-1".to_owned()),
                    objective: "review failing tests".to_owned(),
                    status: AgentStatus::Blocked,
                    pending_approvals: 1,
                    changed_files: vec![PathBuf::from("src/lib.rs")],
                    latest_summary: Some("waiting for approval".to_owned()),
                },
            },
            Some(PathBuf::from("/tmp/project")),
        );

        let serialized = workspace.to_json().unwrap();
        assert!(serialized.contains(r#""type": "task""#));
        assert!(serialized.contains(r#""type": "agent""#));
        assert!(!serialized.contains("process_id"));
        assert!(!serialized.contains("runtime_token"));
        assert!(!serialized.contains("reader_thread"));
        assert!(!serialized.contains("JoinHandle"));
        assert!(!serialized.contains("pty"));
        assert!(!serialized.contains("parser"));
        assert!(!serialized.contains("renderer"));
        assert!(!serialized.contains("scrollback"));
        assert!(!serialized.contains(r#""status": "running""#));
    }
}
