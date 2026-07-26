//! Frontend-neutral ownership seam around the Mandatum state machine.
//!
//! Platform shells translate input and own paint scheduling, but they reach
//! workstation behavior only through this host. The terminal shell keeps the
//! app-owned unified sender without changing the channel as the source of
//! event truth.

use std::{sync::Arc, time::Duration};

use mandatum_commands::CommandId;
use mandatum_scene::{
    LogicalPoint, SceneSize, Theme, ViewportMetrics, WorkspaceScene,
    input::{InputEvent, Key, PointerEvent},
};

use crate::{
    app_shell::AppConfig,
    app_state::AppState,
    events::{AppEventSender, WakeCallback},
    frontend_effect::FrontendEffect,
};

/// One owned paint input from the shared workstation state machine.
///
/// `revision` identifies snapshot production order, not semantic dirtiness:
/// every call to [`FrontendHost::frame`] advances it, even when the scene is
/// unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub scene: WorkspaceScene,
    /// Shared theme snapshot: consecutive frames with an unchanged theme hand
    /// out the same allocation, so per-frame theme cost is one `Arc` bump.
    pub theme: Arc<Theme>,
    pub revision: u64,
    /// Monotonic app-owned scene dirtiness at snapshot construction time.
    /// Unlike `revision`, this does not advance merely because a frame was
    /// requested.
    pub scene_generation: u64,
}

/// The sole owner of app/runtime state for one frontend run.
pub struct FrontendHost {
    app: AppState,
    frame_revision: u64,
    /// Retained theme `Arc` handed to snapshots; refreshed only when the
    /// app's theme value actually changes (config reload).
    theme_snapshot: Option<Arc<Theme>>,
    shutdown_complete: bool,
}

impl FrontendHost {
    pub fn new(config: AppConfig) -> Self {
        Self::with_optional_wake_callback(config, None)
    }

    /// Build a host whose asynchronous producers notify a platform event loop
    /// when the unified queue changes from empty to non-empty.
    pub fn new_with_wake_callback(
        config: AppConfig,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_optional_wake_callback(config, Some(Arc::new(wake)))
    }

    fn with_optional_wake_callback(config: AppConfig, wake: Option<WakeCallback>) -> Self {
        Self {
            app: AppState::new_with_frontend_wake(config, wake),
            frame_revision: 0,
            theme_snapshot: None,
            shutdown_complete: false,
        }
    }

    /// Apply one already-neutral platform input synchronously.
    pub fn handle_input(&mut self, input: InputEvent) {
        if !self.shutdown_complete {
            self.app.handle_event(input);
        }
    }

    /// Apply one native pointer event using the prior frame's logical-pixel
    /// hit targets while retaining its cell coordinates for terminal input.
    pub fn handle_pointer_at_logical(
        &mut self,
        pointer: PointerEvent,
        logical_position: LogicalPoint,
    ) {
        if !self.shutdown_complete {
            self.app
                .handle_pointer_at_logical(pointer, logical_position);
        }
    }

    /// Cancel any platform-owned pointer gesture before geometry changes.
    pub fn cancel_pointer_gesture(&mut self) {
        if !self.shutdown_complete {
            self.app.cancel_pointer_gesture();
        }
    }

    /// Whether the current presentation state makes pure pointer motion
    /// redraw-sensitive. The native shell compares this before and after
    /// routing motion so separator enter and leave both repaint. The tuple is
    /// `(continuous_redraw, hovered_separator_identity, overlay_row_identity)`.
    pub fn pointer_move_redraw_state(&self) -> (bool, Option<usize>, Option<usize>) {
        if self.shutdown_complete {
            (false, None, None)
        } else {
            (
                self.app.pointer_move_needs_redraw(),
                self.app.hovered_separator(),
                self.app.hovered_overlay_row(),
            )
        }
    }

    /// Notify the shared state that the platform cursor left the client area.
    /// Returns whether transient presentation state changed.
    pub fn pointer_left(&mut self) -> bool {
        !self.shutdown_complete && self.app.pointer_left()
    }

    /// Reject hit targets from a frame the platform could not present.
    pub fn suspend_scene_interaction(&mut self) {
        if !self.shutdown_complete {
            self.app.suspend_scene_interaction();
        }
    }

    /// Report whether a neutral key is explicit workspace control.
    ///
    /// Platform shells consult this before applying native clipboard
    /// conventions so configured command chords retain first refusal.
    pub fn handles_workspace_key(&self, key: Key) -> bool {
        !self.shutdown_complete && self.app.key_is_workspace_chord(key)
    }

    /// Request the product's renderer-neutral selection-copy behavior.
    ///
    /// Native shells call this only after a platform copy shortcut loses the
    /// configurable workspace-chord preflight. Clipboard delivery still
    /// returns through the ordinary FIFO [`FrontendEffect`] queue.
    pub fn copy_selection(&mut self) {
        if !self.shutdown_complete {
            self.app.dispatch(CommandId::CopySelection);
        }
    }

    /// Surface a recoverable native-shell integration failure in the shared
    /// status strip rather than silently dropping the user's action.
    pub fn report_platform_error(&mut self, message: impl Into<String>) {
        if !self.shutdown_complete {
            self.app.report_platform_error(message.into());
        }
    }

    /// Declare the frontend's live font: the resolved family and size on
    /// screen plus the families the appearance overlay can offer. Frontends
    /// that render their own text call this at startup and after every font
    /// apply attempt; a terminal frontend never does, which hides the
    /// overlay's font rows.
    pub fn set_font_facts(&mut self, family: String, size: f32, families: Vec<String>) {
        if !self.shutdown_complete {
            self.app.set_font_facts(family, size, families);
        }
    }

    /// Block until one unified input/runtime event is applied or the timeout
    /// expires. A shut-down host never blocks or applies queued work.
    pub fn wait_event(&mut self, timeout: Duration) -> bool {
        !self.shutdown_complete && self.app.wait_event(timeout)
    }

    /// Apply at most the app's bounded per-call event budget without blocking.
    pub fn drain_runtime(&mut self) -> usize {
        if self.shutdown_complete {
            0
        } else {
            self.app.drain_events()
        }
    }

    /// Apply at most `budget` buffered events without blocking.
    ///
    /// Platform shells with tighter event-loop deadlines can choose a smaller
    /// slice while preserving the app's existing hard maximum and FIFO event
    /// truth. A zero budget is a no-op.
    pub fn drain_runtime_bounded(&mut self, budget: usize) -> usize {
        if self.shutdown_complete {
            0
        } else {
            self.app.drain_events_bounded(budget)
        }
    }

    /// Perform child-exit polling; the active platform shell owns its cadence.
    pub fn heartbeat(&mut self) -> bool {
        if self.shutdown_complete {
            return false;
        }
        let before = self.app.scene_generation();
        self.app.poll_child_exits();
        self.app.scene_generation() != before
    }

    /// Monotonic scene-dirtiness generation for event-loop redraw decisions.
    /// Platform shells may compare it before and after input, runtime drains,
    /// or heartbeat work without manufacturing a speculative frame.
    pub fn scene_generation(&self) -> u64 {
        self.app.scene_generation()
    }

    /// Build the exact owned scene/theme pair an adapter should paint.
    ///
    /// `AppState::build_scene` also retains this scene's hit targets, so later
    /// pointer input resolves against the most recently requested frame rather
    /// than a speculative rebuild.
    pub fn frame(&mut self, size: SceneSize) -> FrameSnapshot {
        self.frame_with_viewport(ViewportMetrics::from_scene_size(size))
    }

    /// Build a frame from one coherent logical/physical viewport snapshot.
    pub fn frame_with_viewport(&mut self, viewport: ViewportMetrics) -> FrameSnapshot {
        let revision = self
            .frame_revision
            .checked_add(1)
            .expect("frontend frame revision overflowed");
        let scene = self.app.build_scene_with_viewport(viewport);
        let theme = match &self.theme_snapshot {
            Some(cached) if **cached == *self.app.theme() => Arc::clone(cached),
            _ => {
                let fresh = Arc::new(self.app.theme().clone());
                self.theme_snapshot = Some(Arc::clone(&fresh));
                fresh
            }
        };
        let scene_generation = self.app.scene_generation();
        self.frame_revision = revision;
        FrameSnapshot {
            scene,
            theme,
            revision,
            scene_generation,
        }
    }

    /// Take all pending platform effects in request order, exactly once.
    pub fn take_effects(&mut self) -> Vec<FrontendEffect> {
        self.app.take_frontend_effects()
    }

    pub fn should_quit(&self) -> bool {
        self.app.should_quit()
    }

    /// Shut live work down once. Returns whether this call performed shutdown.
    pub fn shutdown(&mut self) -> bool {
        if self.shutdown_complete {
            return false;
        }
        self.shutdown_complete = true;
        self.app.shutdown();
        true
    }

    /// Terminal input uses the same app-owned sender as PTY and agent workers.
    pub(crate) fn event_sender(&self) -> AppEventSender {
        self.app.event_sender()
    }
}

impl Drop for FrontendHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use mandatum_agent_runtime::{
        AgentSessionEvent, ApprovalRequest, ApprovalScope, FakeConnector, FakeStep, RiskAssessment,
        RiskLevel,
    };
    use mandatum_scene::{
        HitTargetKind, SceneSize, TransitionRole,
        input::{InputEvent, Key, KeyCode, Modifiers, PointerButton, PointerEvent, PointerKind},
    };

    use super::*;
    use crate::{AppConfig, events::AppEvent, frontend_effect::FrontendEffect};

    const FRAME_SIZE: SceneSize = SceneSize {
        width: 100,
        height: 30,
    };

    #[test]
    fn frame_snapshot_is_owned_and_revisions_follow_snapshot_order() {
        let mut host = FrontendHost::new(AppConfig::default());

        let first = host.frame(FRAME_SIZE);
        let first_generation = host.scene_generation();
        host.handle_input(InputEvent::FocusGained);
        let second = host.frame(FRAME_SIZE);

        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 2);
        assert_eq!(first.scene_generation, first_generation);
        assert_eq!(second.scene_generation, host.scene_generation());
        assert!(second.scene_generation > first.scene_generation);
        assert_eq!(first.scene.size, FRAME_SIZE);
        assert_eq!(first.theme.name, "mandatum-dark");
        assert_eq!(first.scene.panes.len(), 1);
    }

    /// P3(e) coverage proof: an unchanged theme is shared across snapshots as
    /// one `Arc` allocation, and the shared value always equals the app's
    /// live theme (the equality gate in `frame_with_viewport` is the only
    /// path that may reuse the retained `Arc`).
    #[test]
    fn snapshots_share_one_theme_allocation_while_the_theme_is_unchanged() {
        let mut host = FrontendHost::new(AppConfig::default());

        let first = host.frame(FRAME_SIZE);
        host.handle_input(InputEvent::FocusGained);
        let second = host.frame(FRAME_SIZE);

        assert!(
            Arc::ptr_eq(&first.theme, &second.theme),
            "an unchanged theme must not be recloned per frame"
        );
        assert_eq!(*first.theme, *host.app.theme());
    }

    #[test]
    fn scene_generation_changes_for_dirty_work_but_not_an_idle_heartbeat() {
        let mut host = FrontendHost::new(AppConfig::default());
        let initial = host.scene_generation();

        assert!(!host.heartbeat());
        assert_eq!(host.scene_generation(), initial);

        host.handle_input(InputEvent::FocusGained);
        let after_input = host.scene_generation();
        assert!(after_input > initial);

        let sender = host.event_sender();
        sender
            .send(AppEvent::Input(InputEvent::FocusGained))
            .unwrap();
        assert_eq!(host.drain_runtime_bounded(1), 1);
        assert!(host.scene_generation() > after_input);

        host.shutdown();
        let shutdown_generation = host.scene_generation();
        assert!(!host.heartbeat());
        assert_eq!(host.scene_generation(), shutdown_generation);
    }

    #[test]
    fn approval_arrival_survives_frame_polling_until_resolution() {
        let request = ApprovalRequest {
            approval_id: "frame-poll-approval".to_owned(),
            command: "rm -rf target".to_owned(),
            scope: ApprovalScope {
                cwd: PathBuf::from("/tmp/project"),
                affected_path: Some(PathBuf::from("target")),
            },
            risk: RiskAssessment {
                level: RiskLevel::High,
                basis: "removes files (rm)".to_owned(),
            },
        };
        let next_request = ApprovalRequest {
            approval_id: "frame-poll-approval-2".to_owned(),
            command: "touch next".to_owned(),
            scope: ApprovalScope {
                cwd: PathBuf::from("/tmp/project"),
                affected_path: Some(PathBuf::from("next")),
            },
            risk: RiskAssessment {
                level: RiskLevel::High,
                basis: "writes a file".to_owned(),
            },
        };
        let mut host = FrontendHost::new(AppConfig::default());
        host.app
            .set_agent_connector(Box::new(FakeConnector::new(vec![
                FakeStep::Emit(AgentSessionEvent::ApprovalRequested(request)),
                FakeStep::AwaitApproval {
                    approval_id: "frame-poll-approval".to_owned(),
                    then_on_approve: vec![AgentSessionEvent::ApprovalRequested(next_request)],
                    then_on_reject: vec![],
                },
                FakeStep::AwaitApproval {
                    approval_id: "frame-poll-approval-2".to_owned(),
                    then_on_approve: vec![],
                    then_on_reject: vec![],
                },
            ])));
        host.app.dispatch(CommandId::StartAgent);
        let pane_id = host
            .app
            .workspace()
            .active_session()
            .focused_pane_id()
            .clone();

        let mut arrival = None;
        for _ in 0..300 {
            host.drain_runtime();
            let frame = host.frame(FRAME_SIZE);
            if let Some(target) = frame
                .scene
                .presentation
                .transition_targets
                .iter()
                .find(|target| target.role == TransitionRole::ApprovalArrival)
            {
                arrival = Some(target.clone());
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let arrival = arrival.expect("approval arrival must reach a polled host frame");
        assert!(arrival.sequence > 0);

        for _ in 0..3 {
            let polled = host.frame(FRAME_SIZE);
            let matching = polled
                .scene
                .presentation
                .transition_targets
                .iter()
                .filter(|target| **target == arrival)
                .count();
            assert_eq!(
                matching, 1,
                "speculative frame polling must retain one stable arrival eligibility"
            );
        }

        for _ in 0..300 {
            host.app.dispatch(CommandId::ApproveAgentAction);
            if host.app.status().starts_with("approved") {
                break;
            }
            host.drain_runtime();
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            host.app.status().starts_with("approved"),
            "approval decision was never applied: {}",
            host.app.status()
        );
        for _ in 0..300 {
            host.drain_runtime();
            if host
                .app
                .approval_arrival_sequence(&pane_id)
                .is_some_and(|sequence| sequence != arrival.sequence)
            {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let next = host.frame(FRAME_SIZE);
        let next_arrival = next
            .scene
            .presentation
            .transition_targets
            .iter()
            .find(|target| target.role == TransitionRole::ApprovalArrival)
            .expect("second approval must remain eligible without an absent frame");
        assert_eq!(next_arrival.node_id, arrival.node_id);
        assert_eq!(next_arrival.role, arrival.role);
        assert_eq!(next_arrival.property, arrival.property);
        assert_ne!(
            next_arrival.sequence, arrival.sequence,
            "same-pane consecutive arrivals require distinct event identity"
        );

        for _ in 0..300 {
            host.app.dispatch(CommandId::ApproveAgentAction);
            if host.app.approval_arrival_sequence(&pane_id).is_none() {
                break;
            }
            host.drain_runtime();
            thread::sleep(Duration::from_millis(2));
        }
        let resolved = host.frame(FRAME_SIZE);
        assert!(
            !resolved
                .scene
                .presentation
                .transition_targets
                .iter()
                .any(|target| target.role == TransitionRole::ApprovalArrival),
            "resolution removes eligibility so a later approval may enter once"
        );
        host.shutdown();
    }

    #[test]
    fn frontend_effects_preserve_fifo_order_and_drain_once_through_host() {
        let mut host = FrontendHost::new(AppConfig::default());
        host.app
            .stage_frontend_effect_for_test(FrontendEffect::SetClipboard("first".to_owned()));
        host.app
            .stage_frontend_effect_for_test(FrontendEffect::SetClipboard("second".to_owned()));

        assert_eq!(
            host.take_effects(),
            vec![
                FrontendEffect::SetClipboard("first".to_owned()),
                FrontendEffect::SetClipboard("second".to_owned()),
            ]
        );
        assert!(host.take_effects().is_empty());
    }

    #[test]
    fn configured_wake_callback_and_blocking_wait_share_unified_input() {
        let wake_count = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&wake_count);
        let mut host = FrontendHost::new_with_wake_callback(AppConfig::default(), move || {
            callback_count.fetch_add(1, Ordering::SeqCst);
        });
        let sender = host.event_sender();
        let input = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            sender
                .send(AppEvent::Input(InputEvent::Key(Key::ctrl('q'))))
                .unwrap();
        });

        assert!(host.wait_event(Duration::from_secs(1)));
        assert!(host.should_quit());
        input.join().unwrap();
        assert_eq!(wake_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runtime_drain_is_bounded_per_call() {
        let mut host = FrontendHost::new(AppConfig::default());
        let sender = host.event_sender();
        for _ in 0..256 {
            sender
                .send(AppEvent::Input(InputEvent::FocusGained))
                .unwrap();
        }
        sender
            .send(AppEvent::Input(InputEvent::Key(Key::ctrl('q'))))
            .unwrap();

        assert_eq!(host.drain_runtime(), 256);
        assert!(!host.should_quit());
        assert!(host.wait_event(Duration::ZERO));
        assert!(host.should_quit());
    }

    #[test]
    fn platform_shell_can_choose_a_smaller_runtime_drain_slice() {
        let mut host = FrontendHost::new(AppConfig::default());
        let sender = host.event_sender();
        for _ in 0..32 {
            sender
                .send(AppEvent::Input(InputEvent::FocusGained))
                .unwrap();
        }

        assert_eq!(host.drain_runtime_bounded(0), 0);
        assert_eq!(host.drain_runtime_bounded(16), 16);
        assert_eq!(host.drain_runtime_bounded(16), 16);
        assert_eq!(host.drain_runtime_bounded(16), 0);
    }

    #[test]
    fn pointer_input_uses_hit_targets_from_the_exact_prior_snapshot() {
        let mut host = FrontendHost::new(AppConfig::default());
        let first = host.frame(FRAME_SIZE);
        assert!(first.scene.hit_targets.iter().any(|target| {
            matches!(&target.kind, HitTargetKind::PaneBody(pane_id) if pane_id.as_str() == "pane-1")
                && target.rect.contains(75, 5)
        }));

        host.handle_input(InputEvent::Key(Key::ctrl('p')));
        host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char('v'))));
        assert_eq!(
            host.app
                .workspace()
                .active_session()
                .focused_pane_id()
                .as_str(),
            "pane-2"
        );

        host.handle_input(InputEvent::Pointer(PointerEvent {
            kind: PointerKind::Down,
            button: Some(PointerButton::Left),
            column: 75,
            row: 5,
            mods: Modifiers::NONE,
        }));
        let next = host.frame(FRAME_SIZE);

        assert_eq!(next.scene.panes.len(), 2);
        assert_eq!(next.scene.focused_pane.as_str(), "pane-1");
    }

    #[test]
    fn shutdown_is_idempotent() {
        let mut host = FrontendHost::new(AppConfig::default());

        assert!(host.shutdown());
        assert!(!host.shutdown());
        assert!(host.shutdown_complete);
    }

    #[test]
    fn workspace_key_query_honors_configured_chords_and_shutdown() {
        let mut config = AppConfig::default();
        let super_v = Key::new(
            KeyCode::Char('v'),
            Modifiers {
                super_key: true,
                ..Modifiers::NONE
            },
        );
        config
            .keymap
            .bind_chord(mandatum_commands::CommandId::ShowHelp, super_v);
        let mut host = FrontendHost::new(config);

        assert!(host.handles_workspace_key(super_v));
        assert!(!host.handles_workspace_key(Key::plain(KeyCode::Char('v'))));
        host.shutdown();
        assert!(!host.handles_workspace_key(super_v));
    }

    #[test]
    fn native_copy_request_stays_behind_the_effect_boundary() {
        let mut host = FrontendHost::new(AppConfig::default());

        let before = host.scene_generation();
        host.copy_selection();
        assert!(
            host.scene_generation() > before,
            "even the nothing-selected exit rewrites the status line, which is \
             scene-visible: without a generation bump the skip guard hides it"
        );
        assert!(host.take_effects().is_empty());
        let frame = host.frame(FRAME_SIZE);
        assert!(frame.scene.status.text.contains("nothing is selected"));

        host.shutdown();
        host.copy_selection();
        assert!(host.take_effects().is_empty());
    }

    #[test]
    fn platform_errors_are_visible_and_ignored_after_shutdown() {
        let mut host = FrontendHost::new(AppConfig::default());
        host.report_platform_error("clipboard write failed");
        assert!(
            host.frame(FRAME_SIZE)
                .scene
                .status
                .text
                .contains("clipboard write failed")
        );

        host.shutdown();
        host.report_platform_error("late failure");
        assert!(
            !host
                .frame(FRAME_SIZE)
                .scene
                .status
                .text
                .contains("late failure")
        );
    }
}
