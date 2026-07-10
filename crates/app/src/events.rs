//! The app's unified event channel.
//!
//! Every wake source — frontend input, PTY reader threads, agent forwarder
//! threads — sends into one `std::sync::mpsc` channel, so the main loop has
//! exactly one blocking wait and can never miss a wake. This is what lets the
//! shell block on event arrival instead of polling on a fixed interval.

use mandatum_scene::input::InputEvent;

use crate::{
    agent_runtime::AgentRuntimeEvent,
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
}
