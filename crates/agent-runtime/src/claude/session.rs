//! Live-session machinery for the Claude CLI connector: shared control
//! state, the approval-socket listener thread, the stdout/stderr pump
//! threads, and the [`AgentSessionControl`] implementation.
//!
//! Everything here is runtime state — threads, channels, a child process —
//! and is never serialized. Threads and `std::sync::mpsc` only, mirroring
//! the PTY runtime.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdout, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::Sender,
    },
    thread::JoinHandle,
    time::Duration,
};

use crate::{
    approval::{
        ApprovalDecision, ApprovalRequest, ApprovalScope, ApprovalVerdict, RiskAssessment,
        RiskLevel, assess_command_risk,
    },
    bridge_protocol::{BridgeApprovalRequest, BridgeVerdict},
    connector::{AgentControlError, AgentSessionControl},
    events::AgentSessionEvent,
};

use super::parser::StreamParser;

/// Poll interval for the non-blocking accept loop and shutdown checks.
const LISTENER_POLL: Duration = Duration::from_millis(25);
/// How long the listener waits for a connected bridge to send its request.
const BRIDGE_READ_TIMEOUT: Duration = Duration::from_secs(5);

static APPROVAL_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Control state shared between the session control handle and the worker
/// threads. Semantics mirror `FakeConnector`: one pending approval at a
/// time, decisions consumed exactly once.
#[derive(Debug)]
pub(crate) struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
}

#[derive(Debug)]
struct Inner {
    /// Approval id the listener is currently blocked on, if any.
    pending: Option<String>,
    /// Verdict queued for the pending approval, not yet consumed.
    decision: Option<ApprovalVerdict>,
    shutdown: bool,
    alive: bool,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                pending: None,
                decision: None,
                shutdown: false,
                alive: true,
            }),
            wake: Condvar::new(),
        }
    }

    /// True once the session is over for any reason: an app-driven shutdown
    /// or the child dying on its own (stdout pump saw EOF). Approval
    /// machinery must fail closed on both, not just on shutdown.
    fn session_over(&self) -> bool {
        let state = self.inner.lock().unwrap();
        state.shutdown || !state.alive
    }
}

/// [`AgentSessionControl`] for a live Claude CLI child process.
pub(crate) struct ClaudeSessionControl {
    shared: Arc<Shared>,
    child: Arc<Mutex<Option<Child>>>,
    child_pid: u32,
    threads: Vec<JoinHandle<()>>,
    socket_path: PathBuf,
    torn_down: bool,
}

impl ClaudeSessionControl {
    pub(crate) fn new(
        shared: Arc<Shared>,
        child: Arc<Mutex<Option<Child>>>,
        child_pid: u32,
        threads: Vec<JoinHandle<()>>,
        socket_path: PathBuf,
    ) -> Self {
        Self {
            shared,
            child,
            child_pid,
            threads,
            socket_path,
            torn_down: false,
        }
    }
}

impl AgentSessionControl for ClaudeSessionControl {
    fn decide(&mut self, decision: ApprovalDecision) -> Result<(), AgentControlError> {
        let mut state = self.shared.inner.lock().unwrap();
        if !state.alive {
            return Err(AgentControlError::SessionClosed);
        }
        match &state.pending {
            None => Err(AgentControlError::NoPendingApproval),
            Some(pending) if *pending != decision.approval_id => {
                Err(AgentControlError::UnknownApproval {
                    approval_id: decision.approval_id,
                })
            }
            Some(_) if state.decision.is_some() => Err(AgentControlError::AlreadyDecided {
                approval_id: decision.approval_id,
            }),
            Some(_) => {
                state.decision = Some(decision.verdict);
                self.shared.wake.notify_all();
                Ok(())
            }
        }
    }

    fn interrupt(&mut self) -> Result<(), AgentControlError> {
        if !self.shared.inner.lock().unwrap().alive {
            return Err(AgentControlError::SessionClosed);
        }
        signal_process_group(self.child_pid, "INT");
        Ok(())
    }

    fn shutdown(&mut self) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        {
            let mut state = self.shared.inner.lock().unwrap();
            state.shutdown = true;
            self.shared.wake.notify_all();
        }
        // Kill the whole process group (claude spawns its own children), then
        // the direct child as a fallback if the group signal failed.
        if !signal_process_group(self.child_pid, "KILL")
            && let Some(child) = self.child.lock().unwrap().as_mut()
        {
            let _ = child.kill();
        }
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }

    fn is_alive(&self) -> bool {
        self.shared.inner.lock().unwrap().alive
    }
}

impl Drop for ClaudeSessionControl {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Best-effort `SIG<signal>` to the child's process group (the child is its
/// own group leader via `process_group(0)`).
fn signal_process_group(pid: u32, signal: &str) -> bool {
    if pid == 0 {
        // pid 0 addresses the caller's own process group; never signal it.
        return false;
    }
    std::process::Command::new("kill")
        .args(["-s", signal, "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Approval-socket listener: accepts one bridge connection at a time,
/// surfaces it as [`AgentSessionEvent::ApprovalRequested`], blocks until the
/// user decides (or the session shuts down), and writes the verdict back.
pub(crate) fn run_listener(
    listener: UnixListener,
    shared: &Shared,
    tx: &Sender<AgentSessionEvent>,
    session_cwd: &Path,
) {
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    loop {
        if shared.session_over() {
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => handle_bridge_connection(stream, shared, tx, session_cwd),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(LISTENER_POLL);
            }
            Err(_) => return,
        }
    }
}

fn handle_bridge_connection(
    stream: UnixStream,
    shared: &Shared,
    tx: &Sender<AgentSessionEvent>,
    session_cwd: &Path,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(BRIDGE_READ_TIMEOUT));
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return; // Bridge fails closed on its side.
    }
    let Ok(bridge_request) = serde_json::from_str::<BridgeApprovalRequest>(line.trim()) else {
        reply(
            reader.into_inner(),
            false,
            "Mandatum could not parse the approval request",
        );
        return;
    };

    let request = approval_request_from_bridge(&bridge_request, session_cwd);
    // Register the pending approval BEFORE emitting the event: a consumer
    // may call decide() the instant it sees ApprovalRequested.
    {
        let mut state = shared.inner.lock().unwrap();
        state.pending = Some(request.approval_id.clone());
        state.decision = None;
    }
    if tx
        .send(AgentSessionEvent::ApprovalRequested(request))
        .is_err()
    {
        shared.inner.lock().unwrap().pending = None;
        reply(
            reader.into_inner(),
            false,
            "Mandatum is no longer listening for approvals",
        );
        return;
    }

    let verdict = wait_for_decision(shared);
    let (allow, reason) = match verdict {
        Some(ApprovalVerdict::Approved) => (true, String::new()),
        Some(ApprovalVerdict::Rejected { reason }) => {
            let reason = reason
                .map(|why| format!("Mandatum rejected this command: {why}"))
                .unwrap_or_else(|| "Mandatum rejected this command".to_owned());
            (false, reason)
        }
        None => (false, "Mandatum session ended".to_owned()),
    };
    reply(reader.into_inner(), allow, &reason);
}

/// Park until a decision for the registered pending approval is queued or
/// the session is over (shutdown requested, or the child died — the stdout
/// pump flips `alive` and wakes us). Returns `None` when the session is
/// over, which the caller turns into a deny: a connected bridge must never
/// outlive the session it is gating for.
fn wait_for_decision(shared: &Shared) -> Option<ApprovalVerdict> {
    let mut state = shared.inner.lock().unwrap();
    loop {
        if state.shutdown || !state.alive {
            state.pending = None;
            return None;
        }
        if let Some(verdict) = state.decision.take() {
            state.pending = None;
            return Some(verdict);
        }
        state = shared.wake.wait_timeout(state, LISTENER_POLL).unwrap().0;
    }
}

fn reply(mut stream: UnixStream, allow: bool, reason: &str) {
    let verdict = BridgeVerdict {
        allow,
        reason: if reason.is_empty() {
            None
        } else {
            Some(reason.to_owned())
        },
    };
    if let Ok(mut line) = serde_json::to_string(&verdict) {
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
        let _ = stream.flush();
    }
}

/// Build the user-facing approval request (risk heuristic lives here).
fn approval_request_from_bridge(
    bridge: &BridgeApprovalRequest,
    session_cwd: &Path,
) -> ApprovalRequest {
    let affected_path = ["file_path", "notebook_path"]
        .iter()
        .find_map(|key| bridge.tool_input.get(key).and_then(|v| v.as_str()))
        .map(PathBuf::from);
    let (command, risk) = if bridge.tool_name == "Bash" {
        let command = bridge
            .tool_input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let risk = assess_command_risk(&command);
        (command, risk)
    } else if let Some(path) = &affected_path {
        (
            format!("{}: {}", bridge.tool_name, path.display()),
            RiskAssessment {
                level: RiskLevel::Medium,
                basis: format!("file-writing tool ({})", bridge.tool_name),
            },
        )
    } else {
        (
            bridge.tool_name.clone(),
            RiskAssessment {
                level: RiskLevel::Low,
                basis: "no known destructive pattern".to_owned(),
            },
        )
    };
    ApprovalRequest {
        approval_id: bridge.tool_use_id.clone().unwrap_or_else(|| {
            format!(
                "mandatum-approval-{}",
                APPROVAL_COUNTER.fetch_add(1, Ordering::Relaxed)
            )
        }),
        command,
        scope: ApprovalScope {
            cwd: bridge
                .cwd
                .clone()
                .unwrap_or_else(|| session_cwd.to_path_buf()),
            affected_path,
        },
        risk,
    }
}

/// Stdout pump: parses the stream-json lines into events, reaps the child at
/// EOF, and emits the terminal [`AgentSessionEvent::Closed`].
pub(crate) fn run_stdout_pump(
    stdout: ChildStdout,
    tx: &Sender<AgentSessionEvent>,
    shared: &Shared,
    child: &Mutex<Option<Child>>,
) {
    let mut parser = StreamParser::new();
    let mut saw_terminal = false;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        for event in parser.parse_line(&line) {
            saw_terminal |= matches!(
                event,
                AgentSessionEvent::Completed { .. } | AgentSessionEvent::Failed { .. }
            );
            if tx.send(event).is_err() {
                break;
            }
        }
    }

    // Reap the child; never hold the lock across wait() callers of kill.
    let reaped = child.lock().unwrap().take();
    let exit = reaped.map(|mut child| child.wait());

    let was_shutdown = {
        let mut state = shared.inner.lock().unwrap();
        state.alive = false;
        state.pending = None;
        shared.wake.notify_all();
        state.shutdown
    };
    if !saw_terminal && !was_shutdown {
        let status = match exit {
            Some(Ok(status)) => format!("{status}"),
            _ => "unknown exit status".to_owned(),
        };
        let _ = tx.send(AgentSessionEvent::Failed {
            error: format!("claude exited without a result event ({status})"),
        });
    }
    let _ = tx.send(AgentSessionEvent::Closed);
}

/// Stderr pump: surfaces diagnostics as prefixed output chunks so the child
/// never blocks on a full pipe.
pub(crate) fn run_stderr_pump(stderr: ChildStderr, tx: &Sender<AgentSessionEvent>) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if tx
            .send(AgentSessionEvent::OutputChunk(format!("[stderr] {line}")))
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn temp_socket(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mandatum-listener-{tag}-{}-{}.sock",
            std::process::id(),
            APPROVAL_COUNTER.fetch_add(1, Ordering::Relaxed),
        ))
    }

    /// Full bridge round-trip against the real listener thread, no child
    /// process: a scripted bridge client sends a request, the test approves
    /// it through control-equivalent state, and the client reads the verdict.
    #[test]
    fn bridge_round_trip_approve_and_reject() {
        for (verdict, expect_allow, expect_reason_contains) in [
            (ApprovalVerdict::Approved, true, None),
            (
                ApprovalVerdict::Rejected {
                    reason: Some("out of mandate".to_owned()),
                },
                false,
                Some("Mandatum rejected this command: out of mandate"),
            ),
        ] {
            let socket_path = temp_socket("roundtrip");
            let _ = std::fs::remove_file(&socket_path);
            let listener = UnixListener::bind(&socket_path).unwrap();
            let shared = Arc::new(Shared::new());
            let (tx, rx) = mpsc::channel();
            let listener_shared = Arc::clone(&shared);
            let listener_handle = std::thread::spawn(move || {
                run_listener(listener, &listener_shared, &tx, Path::new("/tmp/project"));
            });

            let mut stream = UnixStream::connect(&socket_path).unwrap();
            let request = BridgeApprovalRequest {
                tool_name: "Bash".to_owned(),
                tool_input: serde_json::json!({"command": "echo MANDATUM_LIVE_OK"}),
                cwd: Some(PathBuf::from("/tmp/project")),
                tool_use_id: Some("toolu_test".to_owned()),
            };
            let mut line = serde_json::to_string(&request).unwrap();
            line.push('\n');
            stream.write_all(line.as_bytes()).unwrap();

            let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let AgentSessionEvent::ApprovalRequested(approval) = event else {
                panic!("expected ApprovalRequested, got {event:?}");
            };
            assert_eq!(approval.approval_id, "toolu_test");
            assert_eq!(approval.command, "echo MANDATUM_LIVE_OK");
            assert_eq!(approval.risk.level, RiskLevel::Low);

            {
                let mut state = shared.inner.lock().unwrap();
                assert_eq!(state.pending.as_deref(), Some("toolu_test"));
                state.decision = Some(verdict);
                shared.wake.notify_all();
            }

            let mut reply_line = String::new();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            BufReader::new(&stream).read_line(&mut reply_line).unwrap();
            let reply: BridgeVerdict = serde_json::from_str(reply_line.trim()).unwrap();
            assert_eq!(reply.allow, expect_allow);
            match expect_reason_contains {
                Some(text) => assert!(reply.reason.unwrap().contains(text)),
                None => assert_eq!(reply.reason, None),
            }

            shared.inner.lock().unwrap().shutdown = true;
            listener_handle.join().unwrap();
            let _ = std::fs::remove_file(&socket_path);
        }
    }

    #[test]
    fn listener_shutdown_denies_a_waiting_bridge() {
        let socket_path = temp_socket("shutdown");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let shared = Arc::new(Shared::new());
        let (tx, rx) = mpsc::channel();
        let listener_shared = Arc::clone(&shared);
        let handle = std::thread::spawn(move || {
            run_listener(listener, &listener_shared, &tx, Path::new("/tmp"));
        });

        let mut stream = UnixStream::connect(&socket_path).unwrap();
        let request = BridgeApprovalRequest {
            tool_name: "Bash".to_owned(),
            tool_input: serde_json::json!({"command": "ls"}),
            cwd: None,
            tool_use_id: None,
        };
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        stream.write_all(line.as_bytes()).unwrap();
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(event, AgentSessionEvent::ApprovalRequested(_)));

        shared.inner.lock().unwrap().shutdown = true;
        shared.wake.notify_all();

        let mut reply_line = String::new();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        BufReader::new(&stream).read_line(&mut reply_line).unwrap();
        let reply: BridgeVerdict = serde_json::from_str(reply_line.trim()).unwrap();
        assert!(!reply.allow, "shutdown must fail closed");
        assert!(reply.reason.unwrap().contains("Mandatum"));
        handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    /// Child death without an app-driven shutdown — exactly what
    /// `run_stdout_pump` does at EOF: `alive = false`, pending cleared,
    /// wake, `shutdown` untouched — must deny a connected bridge
    /// immediately and let the listener thread exit on its own.
    #[test]
    fn child_death_denies_a_waiting_bridge_without_shutdown() {
        let socket_path = temp_socket("child-death");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let shared = Arc::new(Shared::new());
        let (tx, rx) = mpsc::channel();
        let listener_shared = Arc::clone(&shared);
        let handle = std::thread::spawn(move || {
            run_listener(listener, &listener_shared, &tx, Path::new("/tmp"));
        });

        let mut stream = UnixStream::connect(&socket_path).unwrap();
        let request = BridgeApprovalRequest {
            tool_name: "Bash".to_owned(),
            tool_input: serde_json::json!({"command": "ls"}),
            cwd: None,
            tool_use_id: None,
        };
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        stream.write_all(line.as_bytes()).unwrap();
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(event, AgentSessionEvent::ApprovalRequested(_)));

        {
            let mut state = shared.inner.lock().unwrap();
            state.alive = false;
            state.pending = None;
        }
        shared.wake.notify_all();

        let mut reply_line = String::new();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        BufReader::new(&stream).read_line(&mut reply_line).unwrap();
        let reply: BridgeVerdict = serde_json::from_str(reply_line.trim()).unwrap();
        assert!(!reply.allow, "child death must fail closed");
        assert!(reply.reason.unwrap().contains("Mandatum"));
        // No shutdown flag was ever set: the listener exits on `alive`.
        handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn malformed_bridge_request_is_denied() {
        let socket_path = temp_socket("malformed");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let shared = Arc::new(Shared::new());
        let (tx, _rx) = mpsc::channel();
        let listener_shared = Arc::clone(&shared);
        let handle = std::thread::spawn(move || {
            run_listener(listener, &listener_shared, &tx, Path::new("/tmp"));
        });

        let mut stream = UnixStream::connect(&socket_path).unwrap();
        stream.write_all(b"this is not json\n").unwrap();
        let mut reply_line = String::new();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        BufReader::new(&stream).read_line(&mut reply_line).unwrap();
        let reply: BridgeVerdict = serde_json::from_str(reply_line.trim()).unwrap();
        assert!(!reply.allow);

        shared.inner.lock().unwrap().shutdown = true;
        handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn file_write_bridge_requests_band_medium_with_affected_path() {
        let request = approval_request_from_bridge(
            &BridgeApprovalRequest {
                tool_name: "Write".to_owned(),
                tool_input: serde_json::json!({"file_path": "/tmp/project/out.txt"}),
                cwd: None,
                tool_use_id: None,
            },
            Path::new("/tmp/project"),
        );
        assert_eq!(request.risk.level, RiskLevel::Medium);
        assert_eq!(
            request.scope.affected_path.as_deref(),
            Some(Path::new("/tmp/project/out.txt"))
        );
        assert_eq!(request.scope.cwd, Path::new("/tmp/project"));
        assert!(request.approval_id.starts_with("mandatum-approval-"));
    }
}
