use std::path::PathBuf;

use mandatum_pty::{
    ByteStreamEvent, ChildExit, ChildExitStatus, ChildProcessId, PtyEvent, PtySessionId, PtySize,
    ResizeIntent, RestartIntent, RestartReason, SpawnIntent, SpawnIntentError,
};

fn size() -> PtySize {
    PtySize::new(120, 40).unwrap()
}

#[test]
fn spawn_intent_keeps_process_configuration_as_intent_only() {
    let intent = SpawnIntent::new(PtySessionId::new("session-1"), "/bin/zsh", size())
        .unwrap()
        .with_arguments(["-l"])
        .with_cwd("/tmp/project")
        .with_environment([("TERM", "xterm-256color")]);

    assert_eq!(intent.session_id().as_str(), "session-1");
    assert_eq!(intent.program(), "/bin/zsh");
    assert_eq!(intent.arguments(), &["-l".to_owned()]);
    assert_eq!(intent.cwd(), Some(&PathBuf::from("/tmp/project")));
    assert_eq!(
        intent.environment(),
        &[("TERM".to_owned(), "xterm-256color".to_owned())]
    );
    assert_eq!(intent.size(), size());
}

#[test]
fn spawn_intent_rejects_empty_program() {
    let error = SpawnIntent::new(PtySessionId::new("session-1"), "   ", size()).unwrap_err();

    assert_eq!(error, SpawnIntentError::EmptyProgram);
}

#[test]
fn resize_intent_targets_a_session_and_nonzero_size() {
    assert_eq!(
        PtySize::new(0, 24).unwrap_err(),
        mandatum_pty::PtySizeError {
            columns: 0,
            rows: 24
        }
    );

    let intent = ResizeIntent::new(
        PtySessionId::new("session-1"),
        PtySize::new(80, 24).unwrap(),
    );

    assert_eq!(intent.session_id().as_str(), "session-1");
    assert_eq!(intent.size().columns(), 80);
    assert_eq!(intent.size().rows(), 24);
}

#[test]
fn output_event_preserves_raw_bytes_without_terminal_parser_types() {
    let output = ByteStreamEvent::output(PtySessionId::new("session-1"), b"\x1b[31mred\x1b[0m");
    let event = PtyEvent::Output(output.clone());

    assert_eq!(output.session_id().as_str(), "session-1");
    assert_eq!(output.bytes(), b"\x1b[31mred\x1b[0m");
    assert_eq!(event, PtyEvent::Output(output));
}

#[test]
fn output_event_preserves_invalid_utf8_as_bytes() {
    let output = ByteStreamEvent::output(PtySessionId::new("session-1"), [0xff, b'o', b'k']);

    assert_eq!(output.bytes(), &[0xff, b'o', b'k']);
    assert_eq!(output.into_bytes(), vec![0xff, b'o', b'k']);
}

#[test]
fn child_exit_represents_success_failure_signal_and_unknown() {
    let success = ChildExit::new(
        PtySessionId::new("session-1"),
        Some(ChildProcessId::new(4242)),
        ChildExitStatus::Exited { code: 0 },
    );
    let failure = ChildExit::new(
        PtySessionId::new("session-1"),
        Some(ChildProcessId::new(4243)),
        ChildExitStatus::Exited { code: 2 },
    );
    let signaled = ChildExit::new(
        PtySessionId::new("session-1"),
        None,
        ChildExitStatus::Signaled { signal: 15 },
    );
    let unknown = ChildExit::new(
        PtySessionId::new("session-1"),
        None,
        ChildExitStatus::Unknown,
    );

    assert!(success.succeeded());
    assert_eq!(success.process_id().map(|id| id.get()), Some(4242));
    assert!(!failure.succeeded());
    assert_eq!(signaled.status(), ChildExitStatus::Signaled { signal: 15 });
    assert_eq!(unknown.status(), ChildExitStatus::Unknown);
}

#[test]
fn restart_intent_keeps_reason_without_relaunching_processes() {
    let intent = RestartIntent::new(PtySessionId::new("session-1"), RestartReason::UserRequested);

    assert_eq!(intent.session_id().as_str(), "session-1");
    assert_eq!(intent.reason(), RestartReason::UserRequested);
}

#[test]
fn pty_events_preserve_runtime_order_without_coupling_to_parser() {
    let session_id = PtySessionId::new("session-1");
    let events = [
        PtyEvent::Output(ByteStreamEvent::output(session_id.clone(), b"hello")),
        PtyEvent::ChildExited(ChildExit::new(
            session_id,
            Some(ChildProcessId::new(4242)),
            ChildExitStatus::Exited { code: 0 },
        )),
    ];

    assert!(matches!(events[0], PtyEvent::Output(_)));
    assert!(matches!(events[1], PtyEvent::ChildExited(_)));
}
