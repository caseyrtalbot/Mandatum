//! Frontend-neutral ownership seam around the Mandatum state machine.
//!
//! Platform shells translate input and own paint scheduling, but they reach
//! workstation behavior only through this host. The terminal shell keeps the
//! app-owned unified sender without changing the channel as the source of
//! event truth.

use std::{sync::Arc, time::Duration};

use mandatum_commands::CommandId;
use mandatum_scene::{
    SceneSize, Theme, WorkspaceScene,
    input::{InputEvent, Key},
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
    pub theme: Theme,
    pub revision: u64,
}

/// The sole owner of app/runtime state for one frontend run.
pub struct FrontendHost {
    app: AppState,
    frame_revision: u64,
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
            shutdown_complete: false,
        }
    }

    /// Apply one already-neutral platform input synchronously.
    pub fn handle_input(&mut self, input: InputEvent) {
        if !self.shutdown_complete {
            self.app.handle_event(input);
        }
    }

    /// Cancel any platform-owned pointer gesture before geometry changes.
    pub fn cancel_pointer_gesture(&mut self) {
        if !self.shutdown_complete {
            self.app.cancel_pointer_gesture();
        }
    }

    /// Pure pointer motion changes the scene only while a hover-owned overlay
    /// is open; child any-event reporting wakes through the runtime queue.
    pub fn pointer_move_needs_redraw(&self) -> bool {
        !self.shutdown_complete && self.app.pointer_move_needs_redraw()
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

    /// Perform child-exit polling; the active platform shell owns its cadence.
    pub fn heartbeat(&mut self) {
        if !self.shutdown_complete {
            self.app.poll_child_exits();
        }
    }

    /// Build the exact owned scene/theme pair an adapter should paint.
    ///
    /// `AppState::build_scene` also retains this scene's hit targets, so later
    /// pointer input resolves against the most recently requested frame rather
    /// than a speculative rebuild.
    pub fn frame(&mut self, size: SceneSize) -> FrameSnapshot {
        let revision = self
            .frame_revision
            .checked_add(1)
            .expect("frontend frame revision overflowed");
        let scene = self.app.build_scene(size);
        let theme = self.app.theme().clone();
        self.frame_revision = revision;
        FrameSnapshot {
            scene,
            theme,
            revision,
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
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use mandatum_scene::{
        HitTargetKind, SceneSize,
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
        host.handle_input(InputEvent::FocusGained);
        let second = host.frame(FRAME_SIZE);

        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 2);
        assert_eq!(first.scene.size, FRAME_SIZE);
        assert_eq!(first.theme.name, "mandatum-dark");
        assert_eq!(first.scene.panes.len(), 1);
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

        host.copy_selection();
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
