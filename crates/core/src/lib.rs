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
pub use pane::{
    AgentApprovalRecord, AgentPaneIntent, AgentStatus, PaneKind, PaneSpec, StatusLogSource,
    TaskPaneIntent,
};
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
    fn activate_session_switches_the_active_session_and_project() {
        let mut workspace = workspace();
        let first_session = workspace.active_session().id().clone();
        workspace
            .apply_action(CoreAction::OpenProject {
                name: "other".to_owned(),
                path: PathBuf::from("/tmp/other"),
            })
            .unwrap();
        let second_session = workspace.active_session().id().clone();
        assert_ne!(first_session, second_session);

        workspace
            .apply_action(CoreAction::ActivateSession {
                session_id: first_session.clone(),
            })
            .unwrap();
        assert_eq!(workspace.active_session().id(), &first_session);
        assert_eq!(
            workspace.active_session().project_id(),
            workspace.active_session().project_id()
        );
        // The active project follows the session.
        let project_id = workspace.active_session().project_id().clone();
        assert!(workspace.projects().contains_key(&project_id));

        // Unknown sessions error; the active session is untouched.
        let missing = workspace.apply_action(CoreAction::ActivateSession {
            session_id: SessionId::new("session-99"),
        });
        assert!(missing.is_err());
        assert_eq!(workspace.active_session().id(), &first_session);
    }

    #[test]
    fn new_session_reuses_the_active_project_without_duplicating_it() {
        let mut workspace = workspace();
        let project_id = workspace.active_session().project_id().clone();
        let project_count = workspace.projects().len();
        let first_session = workspace.active_session().id().clone();

        workspace.apply_action(CoreAction::NewSession).unwrap();

        assert_ne!(workspace.active_session().id(), &first_session);
        assert_eq!(workspace.active_session().project_id(), &project_id);
        assert_eq!(workspace.projects().len(), project_count);
        assert_eq!(
            workspace.active_project_path(),
            PathBuf::from("/tmp/project")
        );
        assert_eq!(workspace.active_session().panes().len(), 1);
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
    fn set_split_ratio_addresses_splits_in_preorder_and_persists() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::SplitDown).unwrap();

        // Preorder: split 0 is the root horizontal split, split 1 the nested
        // vertical split on its second side.
        workspace
            .apply_action(CoreAction::SetSplitRatio {
                split_index: 1,
                first_percent: 30,
            })
            .unwrap();

        let LayoutNode::Split { first, second, .. } = workspace.active_session().layout().root()
        else {
            panic!("root must be a split");
        };
        assert!(matches!(first.as_ref(), LayoutNode::Pane { .. }));
        let LayoutNode::Split { first_percent, .. } = second.as_ref() else {
            panic!("second side must be the nested split");
        };
        assert_eq!(*first_percent, 30);

        // Ratios are durable layout intent: they survive a round-trip.
        let restored = Workspace::from_json(&workspace.to_json().unwrap()).unwrap();
        assert_eq!(
            restored.active_session().layout().root(),
            workspace.active_session().layout().root()
        );

        // Percentages clamp so neither side collapses; unknown splits error.
        workspace
            .apply_action(CoreAction::SetSplitRatio {
                split_index: 0,
                first_percent: 0,
            })
            .unwrap();
        let LayoutNode::Split { first_percent, .. } = workspace.active_session().layout().root()
        else {
            panic!("root must be a split");
        };
        assert_eq!(*first_percent, 1);
        assert!(
            workspace
                .apply_action(CoreAction::SetSplitRatio {
                    split_index: 9,
                    first_percent: 50,
                })
                .is_err()
        );
    }

    #[test]
    fn move_floating_pane_updates_the_durable_rect() {
        let mut workspace = workspace();
        workspace
            .apply_action(CoreAction::NewTerminal {
                title: "scratch".to_owned(),
                cwd: None,
            })
            .unwrap();
        let floating = workspace.active_session().focused_pane_id().clone();

        workspace
            .apply_action(CoreAction::MoveFloatingPane {
                pane_id: floating.clone(),
                x: 13,
                y: 7,
            })
            .unwrap();

        let layout = workspace.active_session().layout();
        let rect = &layout.floating()[0].rect;
        assert_eq!((rect.x, rect.y), (13, 7));
        // Width and height are untouched by a move.
        assert_eq!((rect.width, rect.height), (96, 28));

        // Tiled panes cannot be moved as floats.
        assert!(
            workspace
                .apply_action(CoreAction::MoveFloatingPane {
                    pane_id: PaneId::new("pane-1"),
                    x: 0,
                    y: 0,
                })
                .is_err()
        );
    }

    #[test]
    fn dock_returns_a_floating_pane_to_the_tiled_tree() {
        let mut workspace = workspace();
        workspace
            .apply_action(CoreAction::NewTerminal {
                title: "scratch".to_owned(),
                cwd: None,
            })
            .unwrap();
        let floating = workspace.active_session().focused_pane_id().clone();
        assert!(workspace.active_session().layout().is_floating(&floating));

        // Floating an already-floating pane is an error, never a silent Ok.
        let already = workspace.apply_action(CoreAction::FloatFocused);
        assert!(already.is_err());
        assert!(
            already
                .unwrap_err()
                .to_string()
                .contains("already floating")
        );

        workspace.apply_action(CoreAction::DockFocused).unwrap();
        let layout = workspace.active_session().layout();
        assert!(!layout.is_floating(&floating));
        assert_eq!(
            layout.root(),
            &LayoutNode::Split {
                axis: SplitAxis::Horizontal,
                first_percent: 50,
                first: Box::new(LayoutNode::Pane {
                    pane_id: PaneId::new("pane-1"),
                }),
                second: Box::new(LayoutNode::Pane {
                    pane_id: floating.clone(),
                }),
            }
        );

        // Docking a tiled pane is an error too.
        assert!(workspace.apply_action(CoreAction::DockFocused).is_err());

        // Dock round-trips with float.
        workspace.apply_action(CoreAction::FloatFocused).unwrap();
        assert!(workspace.active_session().layout().is_floating(&floating));
    }

    #[test]
    fn resize_focused_adjusts_the_nearest_enclosing_split() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::SplitDown).unwrap();

        // Focused pane-3 sits on the second side of the nested vertical
        // split: growing it shrinks that split's first side.
        workspace
            .apply_action(CoreAction::ResizeFocused { delta_percent: 5 })
            .unwrap();
        let LayoutNode::Split {
            first_percent: root_percent,
            second,
            ..
        } = workspace.active_session().layout().root()
        else {
            panic!("root must be a split");
        };
        assert_eq!(*root_percent, 50, "outer split is untouched");
        let LayoutNode::Split { first_percent, .. } = second.as_ref() else {
            panic!("second side must be the nested split");
        };
        assert_eq!(*first_percent, 45);

        // Shrinking moves the same boundary back, and clamps at the edges.
        workspace
            .apply_action(CoreAction::ResizeFocused { delta_percent: -5 })
            .unwrap();
        workspace
            .apply_action(CoreAction::ResizeFocused {
                delta_percent: -128,
            })
            .unwrap();
        let LayoutNode::Split { second, .. } = workspace.active_session().layout().root() else {
            panic!("root must be a split");
        };
        let LayoutNode::Split { first_percent, .. } = second.as_ref() else {
            panic!("second side must be the nested split");
        };
        assert_eq!(*first_percent, 99);

        // A floating pane, and a layout with no split, both error.
        workspace
            .apply_action(CoreAction::NewTerminal {
                title: "scratch".to_owned(),
                cwd: None,
            })
            .unwrap();
        assert!(
            workspace
                .apply_action(CoreAction::ResizeFocused { delta_percent: 5 })
                .is_err()
        );
        let mut single = Workspace::new("single", PathBuf::from("/tmp/project"));
        let no_split = single.apply_action(CoreAction::ResizeFocused { delta_percent: 5 });
        assert!(no_split.unwrap_err().to_string().contains("no split"));
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
                    pending_approval_ids: vec!["appr-1".to_owned()],
                    changed_files: vec![PathBuf::from("src/lib.rs")],
                    latest_summary: Some("waiting for approval".to_owned()),
                    approval_history: vec![AgentApprovalRecord {
                        approval_id: "appr-0".to_owned(),
                        command: "cargo test".to_owned(),
                        approved: true,
                    }],
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

    #[test]
    fn detach_live_session_clears_in_flight_claims_but_keeps_outcomes() {
        let mut in_flight = AgentPaneIntent::draft("review failing tests");
        in_flight.status = AgentStatus::WaitingForApproval;
        in_flight.pending_approvals = 1;
        in_flight.pending_approval_ids = vec!["appr-1".to_owned()];
        in_flight.approval_history = vec![AgentApprovalRecord {
            approval_id: "appr-0".to_owned(),
            command: "cargo test".to_owned(),
            approved: true,
        }];

        in_flight.detach_live_session();

        assert_eq!(in_flight.status, AgentStatus::Unknown);
        assert_eq!(in_flight.pending_approvals, 0);
        assert!(in_flight.pending_approval_ids.is_empty());
        assert_eq!(in_flight.approval_history.len(), 1);

        // Terminal states the session already reported stay as they are.
        for terminal in [AgentStatus::Complete, AgentStatus::Failed] {
            let mut done = AgentPaneIntent::draft("done");
            done.status = terminal.clone();
            done.detach_live_session();
            assert_eq!(done.status, terminal);
        }
    }
}
