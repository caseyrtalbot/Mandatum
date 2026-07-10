use std::{
    fmt,
    sync::{Arc, Condvar, Mutex, mpsc::Sender},
    thread::{self, JoinHandle},
};

use mandatum_core::PaneId;
use mandatum_pty::{NativePtyReader, PtyEvent};

use crate::events::AppEvent;

const PTY_READ_CHUNK_BYTES: usize = 8192;

/// The most PTY output one pane may have in flight (read but not yet applied)
/// before its reader thread blocks instead of sending more. While the reader
/// is blocked the child's writes back up in the kernel pipe, so a flooding
/// `yes`/`cat` blocks in the OS instead of ballooning the app heap.
pub(crate) const MAX_IN_FLIGHT_BYTES: usize = 256 * 1024;

/// Per-reader flow control between a PTY reader thread and the main loop.
///
/// The reader acquires a [`PtyFlowCredit`] for each chunk before sending it
/// on the event channel; the credit releases its bytes when it is dropped —
/// after the chunk is applied, or when a queued event is discarded — so no
/// path can leak capacity. `stop` aborts a blocked acquire so shutdown can
/// always join the reader thread.
pub(crate) struct PtyFlowControl {
    state: Mutex<PtyFlowState>,
    changed: Condvar,
}

struct PtyFlowState {
    in_flight_bytes: usize,
    stopped: bool,
}

impl PtyFlowControl {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PtyFlowState {
                in_flight_bytes: 0,
                stopped: false,
            }),
            changed: Condvar::new(),
        })
    }

    /// Block until `bytes` fit under the in-flight cap, then reserve them.
    /// Returns `None` once [`PtyFlowControl::stop`] has been called.
    pub(crate) fn acquire(self: &Arc<Self>, bytes: usize) -> Option<PtyFlowCredit> {
        let mut state = self.state.lock().expect("PTY flow lock");
        loop {
            if state.stopped {
                return None;
            }
            // An empty gate always admits one chunk, so an oversized chunk
            // can never wedge the reader.
            if state.in_flight_bytes == 0 || state.in_flight_bytes + bytes <= MAX_IN_FLIGHT_BYTES {
                state.in_flight_bytes += bytes;
                return Some(PtyFlowCredit {
                    flow: Arc::clone(self),
                    bytes,
                });
            }
            state = self.changed.wait(state).expect("PTY flow lock");
        }
    }

    /// Abort any blocked `acquire` and refuse future ones. Called before
    /// joining the reader thread so shutdown cannot deadlock on a full gate.
    pub(crate) fn stop(&self) {
        let mut state = self.state.lock().expect("PTY flow lock");
        state.stopped = true;
        self.changed.notify_all();
    }

    fn release(&self, bytes: usize) {
        let mut state = self.state.lock().expect("PTY flow lock");
        state.in_flight_bytes = state.in_flight_bytes.saturating_sub(bytes);
        self.changed.notify_all();
    }

    /// Bytes currently in flight (sent but not yet consumed). Test-facing.
    #[cfg(test)]
    pub(crate) fn in_flight_bytes(&self) -> usize {
        self.state.lock().expect("PTY flow lock").in_flight_bytes
    }
}

impl fmt::Debug for PtyFlowControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PtyFlowControl")
    }
}

/// A reservation of in-flight bytes, released on drop. Travels with the
/// output event so every consumption path — applied, stale-rejected,
/// discarded, or dropped with the channel — returns its capacity.
#[derive(Debug)]
pub(crate) struct PtyFlowCredit {
    flow: Arc<PtyFlowControl>,
    bytes: usize,
}

impl Drop for PtyFlowCredit {
    fn drop(&mut self) {
        self.flow.release(self.bytes);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PtyRuntimeEvent {
    Output {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
        bytes: Vec<u8>,
    },
    ReaderClosed {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
    },
    Error {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
        message: String,
    },
}

pub(crate) fn spawn_reader_thread(
    pane_id: PaneId,
    restart_generation: u64,
    runtime_token: u64,
    mut reader: NativePtyReader,
    tx: Sender<AppEvent>,
    flow: Arc<PtyFlowControl>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match reader.read_event(PTY_READ_CHUNK_BYTES) {
                Ok(Some(PtyEvent::Output(output))) => {
                    let bytes = output.into_bytes();
                    // Backpressure: block here (leaving the child blocked in
                    // the OS pipe) rather than queue unbounded chunks. A
                    // `None` credit means shutdown is joining this thread.
                    let Some(credit) = flow.acquire(bytes.len()) else {
                        break;
                    };
                    let event = PtyRuntimeEvent::Output {
                        pane_id: pane_id.clone(),
                        restart_generation,
                        runtime_token,
                        bytes,
                    };
                    if tx.send(AppEvent::Pty(event, Some(credit))).is_err() {
                        break;
                    }
                }
                Ok(Some(PtyEvent::ChildExited(_))) | Ok(Some(PtyEvent::BackpressureChanged(_))) => {
                }
                Ok(None) => {
                    let _ = tx.send(AppEvent::Pty(
                        PtyRuntimeEvent::ReaderClosed {
                            pane_id,
                            restart_generation,
                            runtime_token,
                        },
                        None,
                    ));
                    break;
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::Pty(
                        PtyRuntimeEvent::Error {
                            pane_id,
                            restart_generation,
                            runtime_token,
                            message: error.to_string(),
                        },
                        None,
                    ));
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn flow_credits_release_capacity_on_drop() {
        let flow = PtyFlowControl::new();
        let credit = flow.acquire(1000).expect("gate is open");
        assert_eq!(flow.in_flight_bytes(), 1000);
        drop(credit);
        assert_eq!(flow.in_flight_bytes(), 0);
    }

    #[test]
    fn acquire_blocks_at_capacity_until_a_credit_releases() {
        let flow = PtyFlowControl::new();
        let held = flow.acquire(MAX_IN_FLIGHT_BYTES).expect("gate is open");

        let producer = {
            let flow = Arc::clone(&flow);
            thread::spawn(move || {
                let started = Instant::now();
                let credit = flow.acquire(1).expect("released capacity admits us");
                drop(credit);
                started.elapsed()
            })
        };
        // Give the producer time to reach the blocked wait, then release.
        thread::sleep(Duration::from_millis(50));
        drop(held);
        let blocked_for = producer.join().expect("producer thread");
        assert!(
            blocked_for >= Duration::from_millis(30),
            "acquire returned in {blocked_for:?}; it should have blocked until release"
        );
        assert_eq!(flow.in_flight_bytes(), 0);
    }

    #[test]
    fn stop_aborts_a_blocked_acquire_so_shutdown_can_join() {
        let flow = PtyFlowControl::new();
        let _held = flow.acquire(MAX_IN_FLIGHT_BYTES).expect("gate is open");

        let producer = {
            let flow = Arc::clone(&flow);
            thread::spawn(move || flow.acquire(1).is_none())
        };
        thread::sleep(Duration::from_millis(20));
        flow.stop();
        assert!(
            producer.join().expect("producer thread"),
            "a stopped gate must abort the blocked acquire"
        );
        assert!(flow.acquire(1).is_none(), "a stopped gate stays closed");
    }
}
