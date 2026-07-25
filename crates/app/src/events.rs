//! The app's unified event channel.
//!
//! Every wake source — frontend input, PTY reader threads, agent forwarder
//! threads — sends through one app-owned ingress. Input has a priority lane;
//! its marker shares the runtime lane so the main loop still has exactly one
//! blocking wait and can never miss a wake. This prevents a bounded PTY flood
//! from putting terminal parsing ahead of newly arrived user input.

use std::{
    cell::Cell,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    time::Duration,
};

use mandatum_scene::input::InputEvent;

use crate::{
    agent_runtime::AgentRuntimeEvent,
    artifact_preview::ArtifactLoadEvent,
    process_events::{PtyFlowCredit, PtyRuntimeEvent},
};

/// One event on the app's unified channel.
///
/// `Input` is sent by the frontend's input thread (already translated to
/// neutral `mandatum_scene::input` values, so this type stays
/// frontend-neutral). `Pty` and `Agent` are sent by runtime worker threads.
/// A PTY output event carries the flow credit reserved for its bytes; the
/// credit's drop — on apply, discard, or channel teardown — returns that
/// capacity to the reader thread's backpressure gate.
#[derive(Debug)]
pub(crate) enum AppEvent {
    Input(InputEvent),
    Pty(PtyRuntimeEvent, Option<PtyFlowCredit>),
    Agent(AgentRuntimeEvent),
    Artifact(ArtifactLoadEvent),
}

pub(crate) type WakeCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Debug)]
enum QueuedEvent {
    App(AppEvent),
    InputWake,
}

pub(crate) struct AppEventReceiver {
    events: Receiver<QueuedEvent>,
    inputs: Receiver<InputEvent>,
    claimed_input_wakes: Cell<usize>,
}

#[derive(Debug)]
pub(crate) struct AppEventSendError;

#[derive(Default)]
struct WakeState {
    queued_events: usize,
    wake_pending: bool,
}

/// App-owned send side of the unified event channel.
///
/// The channel remains event truth. The optional callback only tells a
/// frontend that the queue changed from empty to non-empty. Sender and
/// receiver bookkeeping share one small lock so draining the last event and
/// enqueueing the next event cannot clear each other's wake.
#[derive(Clone)]
pub(crate) struct AppEventSender {
    tx: Sender<QueuedEvent>,
    input_tx: Sender<InputEvent>,
    wake: Option<WakeCallback>,
    state: Arc<Mutex<WakeState>>,
}

impl AppEventSender {
    pub(crate) fn channel() -> (Self, AppEventReceiver) {
        Self::with_optional_wake_callback(None)
    }

    #[cfg(test)]
    pub(crate) fn channel_with_wake_callback(
        wake: impl Fn() + Send + Sync + 'static,
    ) -> (Self, AppEventReceiver) {
        Self::with_optional_wake_callback(Some(Arc::new(wake)))
    }

    pub(crate) fn channel_with_shared_wake_callback(
        wake: WakeCallback,
    ) -> (Self, AppEventReceiver) {
        Self::with_optional_wake_callback(Some(wake))
    }

    fn with_optional_wake_callback(wake: Option<WakeCallback>) -> (Self, AppEventReceiver) {
        let (tx, events) = mpsc::channel();
        let (input_tx, inputs) = mpsc::channel();
        (
            Self {
                tx,
                input_tx,
                wake,
                state: Arc::new(Mutex::new(WakeState::default())),
            },
            AppEventReceiver {
                events,
                inputs,
                claimed_input_wakes: Cell::new(0),
            },
        )
    }

    pub(crate) fn send(&self, event: AppEvent) -> Result<(), AppEventSendError> {
        let should_wake = {
            let mut state = self.state.lock().expect("app event wake state lock");
            let queued = match event {
                AppEvent::Input(input) => {
                    self.input_tx.send(input).map_err(|_| AppEventSendError)?;
                    QueuedEvent::InputWake
                }
                event => QueuedEvent::App(event),
            };
            self.tx.send(queued).map_err(|_| AppEventSendError)?;
            state.queued_events = state
                .queued_events
                .checked_add(1)
                .expect("app event queue count overflowed");
            if state.wake_pending {
                false
            } else {
                state.wake_pending = true;
                true
            }
        };

        if should_wake && let Some(wake) = &self.wake {
            wake();
        }
        Ok(())
    }

    pub(crate) fn try_recv(&self, rx: &AppEventReceiver) -> Result<AppEvent, TryRecvError> {
        let mut state = self.state.lock().expect("app event wake state lock");
        if let Ok(input) = rx.inputs.try_recv() {
            Self::finish_receive(&mut state);
            rx.claimed_input_wakes.set(
                rx.claimed_input_wakes
                    .get()
                    .checked_add(1)
                    .expect("claimed input wake count overflowed"),
            );
            return Ok(AppEvent::Input(input));
        }
        loop {
            match rx.events.try_recv() {
                Ok(QueuedEvent::App(event)) => {
                    Self::finish_receive(&mut state);
                    return Ok(event);
                }
                Ok(QueuedEvent::InputWake) => {
                    if rx.claimed_input_wakes.get() > 0 {
                        rx.claimed_input_wakes.set(rx.claimed_input_wakes.get() - 1);
                        continue;
                    }
                    Self::finish_receive(&mut state);
                    if let Ok(input) = rx.inputs.try_recv() {
                        return Ok(AppEvent::Input(input));
                    }
                }
                Err(TryRecvError::Empty) => {
                    debug_assert_eq!(state.queued_events, 0);
                    state.wake_pending = false;
                    return Err(TryRecvError::Empty);
                }
                Err(TryRecvError::Disconnected) => {
                    state.queued_events = 0;
                    state.wake_pending = false;
                    return Err(TryRecvError::Disconnected);
                }
            }
        }
    }

    pub(crate) fn recv_timeout(
        &self,
        rx: &AppEventReceiver,
        timeout: Duration,
    ) -> Result<AppEvent, RecvTimeoutError> {
        match self.try_recv(rx) {
            Ok(event) => Ok(event),
            Err(TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
            Err(TryRecvError::Empty) => {
                let queued = rx.events.recv_timeout(timeout)?;
                let mut state = self.state.lock().expect("app event wake state lock");
                Self::finish_receive(&mut state);
                match queued {
                    QueuedEvent::App(event) => Ok(event),
                    QueuedEvent::InputWake => {
                        rx.inputs.recv_timeout(Duration::ZERO).map(AppEvent::Input)
                    }
                }
            }
        }
    }

    /// Re-fire the frontend wake when the queue is still non-empty.
    ///
    /// `send` wakes only on the empty→non-empty transition, so a consumer that
    /// stops draining before the queue empties — a drain cut short by its
    /// budget or its deadline — must re-arm the wake itself or the remainder
    /// strands until the frontend's next heartbeat.
    pub(crate) fn rearm_wake(&self) {
        let queued = {
            let state = self.state.lock().expect("app event wake state lock");
            state.queued_events > 0
        };
        if queued && let Some(wake) = &self.wake {
            wake();
        }
    }

    fn finish_receive(state: &mut WakeState) {
        state.queued_events = state
            .queued_events
            .checked_sub(1)
            .expect("received an app event that bypassed AppEventSender");
        if state.queued_events == 0 {
            state.wake_pending = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use mandatum_agent_runtime::AgentSessionEvent;
    use mandatum_core::PaneId;
    use mandatum_scene::{SceneSize, input::InputEvent};

    use super::*;
    use crate::{
        agent_runtime::spawn_agent_forwarder_thread,
        process_events::{PtyFlowControl, spawn_reader_thread},
    };

    fn counting_sender() -> (AppEventSender, AppEventReceiver, Arc<AtomicUsize>) {
        let wake_count = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&wake_count);
        let (sender, rx) = AppEventSender::channel_with_wake_callback(move || {
            callback_count.fetch_add(1, Ordering::SeqCst);
        });
        (sender, rx, wake_count)
    }

    #[test]
    fn enqueued_input_wakes_the_frontend_and_remains_channel_truth() {
        let (sender, rx, wake_count) = counting_sender();

        sender
            .send(AppEvent::Input(InputEvent::FocusGained))
            .unwrap();

        assert_eq!(wake_count.load(Ordering::SeqCst), 1);
        assert!(matches!(
            sender.recv_timeout(&rx, Duration::ZERO),
            Ok(AppEvent::Input(InputEvent::FocusGained))
        ));
    }

    #[test]
    fn queued_burst_coalesces_wakes_without_dropping_events() {
        let (sender, rx, wake_count) = counting_sender();

        for width in 1..=64 {
            sender
                .send(AppEvent::Input(InputEvent::Resize(SceneSize::new(
                    width, 1,
                ))))
                .unwrap();
        }

        assert_eq!(wake_count.load(Ordering::SeqCst), 1);
        for expected_width in 1..=64 {
            assert!(matches!(
                sender.recv_timeout(&rx, Duration::ZERO),
                Ok(AppEvent::Input(InputEvent::Resize(size)))
                    if size == SceneSize::new(expected_width, 1)
            ));
        }

        sender
            .send(AppEvent::Input(InputEvent::FocusGained))
            .unwrap();
        assert_eq!(wake_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn event_racing_with_pending_wake_clear_is_never_stranded() {
        const EVENT_COUNT: u16 = 4_096;

        let (wake_tx, wake_rx) = mpsc::channel();
        let (sender, event_rx) = AppEventSender::channel_with_wake_callback(move || {
            wake_tx.send(()).expect("fake frontend is listening");
        });
        let producer = {
            let sender = sender.clone();
            thread::spawn(move || {
                for width in 1..=EVENT_COUNT {
                    sender
                        .send(AppEvent::Input(InputEvent::Resize(SceneSize::new(
                            width, 1,
                        ))))
                        .unwrap();
                    thread::yield_now();
                }
            })
        };

        let mut received = 0;
        while received < usize::from(EVENT_COUNT) {
            wake_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("a queued event lost its frontend wake");
            loop {
                match sender.try_recv(&event_rx) {
                    Ok(AppEvent::Input(InputEvent::Resize(_))) => {
                        received += 1;
                        thread::yield_now();
                    }
                    Ok(other) => panic!("unexpected event: {other:?}"),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        panic!("event channel disconnected")
                    }
                }
            }
        }

        producer.join().unwrap();
        assert_eq!(received, usize::from(EVENT_COUNT));
    }

    #[test]
    fn input_overtakes_a_runtime_backlog_without_dropping_runtime_events() {
        let (sender, rx, _) = counting_sender();
        sender
            .send(AppEvent::Pty(
                PtyRuntimeEvent::ReaderClosed {
                    pane_id: PaneId::new("flooded-pane"),
                    restart_generation: 0,
                    runtime_token: 1,
                },
                None,
            ))
            .unwrap();
        sender
            .send(AppEvent::Input(InputEvent::FocusGained))
            .unwrap();

        assert!(matches!(
            sender.try_recv(&rx),
            Ok(AppEvent::Input(InputEvent::FocusGained))
        ));
        assert!(matches!(
            sender.try_recv(&rx),
            Ok(AppEvent::Pty(PtyRuntimeEvent::ReaderClosed { .. }, None))
        ));
        assert!(matches!(sender.try_recv(&rx), Err(TryRecvError::Empty)));
    }

    #[cfg(unix)]
    #[test]
    fn pty_and_agent_producers_share_the_wake_aware_sender() {
        use mandatum_pty::{NativePtySession, PtySessionId, PtySize, SpawnIntent};

        let (sender, rx, wake_count) = counting_sender();
        let intent = SpawnIntent::new(
            PtySessionId::new("wake-aware-pty"),
            "/bin/sh",
            PtySize::new(80, 24).unwrap(),
        )
        .unwrap()
        .with_arguments(["-c", "printf 'pty-wake'"]);
        let parts = NativePtySession::spawn(intent)
            .unwrap()
            .into_split()
            .unwrap();
        let pty_thread = spawn_reader_thread(
            PaneId::new("pty-pane"),
            0,
            1,
            parts.reader,
            sender.clone(),
            PtyFlowControl::new(),
        );
        pty_thread.join().unwrap();

        let mut saw_pty = false;
        loop {
            match sender.try_recv(&rx) {
                Ok(AppEvent::Pty(_, _)) => saw_pty = true,
                Ok(other) => panic!("unexpected PTY-path event: {other:?}"),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("event channel disconnected")
                }
            }
        }
        assert!(saw_pty);
        let wakes_after_pty = wake_count.load(Ordering::SeqCst);
        assert!(wakes_after_pty >= 1);

        let (agent_tx, agent_rx) = mpsc::channel();
        let agent_thread =
            spawn_agent_forwarder_thread(PaneId::new("agent-pane"), 0, 2, agent_rx, sender.clone());
        agent_tx
            .send(AgentSessionEvent::Summary("agent wake".to_owned()))
            .unwrap();
        drop(agent_tx);
        agent_thread.join().unwrap();

        assert!(wake_count.load(Ordering::SeqCst) > wakes_after_pty);
        assert!(matches!(
            sender.recv_timeout(&rx, Duration::ZERO),
            Ok(AppEvent::Agent(AgentRuntimeEvent { pane_id, .. }))
                if pane_id == PaneId::new("agent-pane")
        ));
    }
}
