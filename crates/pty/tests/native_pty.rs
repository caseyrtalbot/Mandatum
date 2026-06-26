#![cfg(unix)]

use mandatum_pty::{
    ChildExitStatus, NativePtyError, NativePtySession, PtyEvent, PtySessionId, PtySize,
    ResizeIntent, SpawnIntent,
};

fn size() -> PtySize {
    PtySize::new(80, 24).unwrap()
}

fn shell_intent(session_id: &str, script: &str) -> SpawnIntent {
    SpawnIntent::new(PtySessionId::new(session_id), "/bin/sh", size())
        .unwrap()
        .with_arguments(["-lc", script])
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn read_until_contains(session: &mut NativePtySession, needle: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();

    for _ in 0..8 {
        let Some(event) = session.read_event(1024).unwrap() else {
            break;
        };
        let PtyEvent::Output(chunk) = event else {
            panic!("expected output event");
        };
        output.extend(chunk.into_bytes());
        if contains_bytes(&output, needle) {
            return output;
        }
    }

    panic!(
        "expected output to contain {:?}, got {:?}",
        String::from_utf8_lossy(needle),
        output
    );
}

#[test]
fn native_pty_spawns_command_and_preserves_raw_output_bytes() {
    let mut session = NativePtySession::spawn(shell_intent(
        "native-output",
        "printf 'hello'; printf '\\377'",
    ))
    .unwrap();

    assert_eq!(session.session_id().as_str(), "native-output");
    assert!(session.process_id().is_some());

    let output = read_until_contains(&mut session, &[0xff]);

    assert!(output.starts_with(b"hello"));
    assert!(output.contains(&0xff));

    let exit_event = session.wait_event().unwrap();
    let PtyEvent::ChildExited(exit) = exit_event else {
        panic!("expected child exit event");
    };
    assert_eq!(exit.status(), ChildExitStatus::Exited { code: 0 });
    assert!(exit.succeeded());
}

#[test]
fn native_pty_writes_input_bytes_to_child() {
    let mut session = NativePtySession::spawn(shell_intent(
        "native-input",
        "stty -echo; IFS= read line; printf 'reply:%s' \"$line\"",
    ))
    .unwrap();

    session.write_input(b"casey\n").unwrap();

    let output = read_until_contains(&mut session, b"reply:casey");

    assert!(contains_bytes(&output, b"reply:casey"));
    assert_eq!(
        session.wait().unwrap().status(),
        ChildExitStatus::Exited { code: 0 }
    );
}

#[test]
fn native_pty_reports_child_exit_status() {
    let mut session = NativePtySession::spawn(shell_intent("native-exit", "exit 7")).unwrap();

    let exit = session.wait().unwrap();

    assert_eq!(exit.session_id().as_str(), "native-exit");
    assert_eq!(exit.process_id(), session.process_id());
    assert_eq!(exit.status(), ChildExitStatus::Exited { code: 7 });
    assert!(!exit.succeeded());
}

#[test]
fn native_pty_rejects_spawn_failure_without_runtime_session() {
    let intent = SpawnIntent::new(
        PtySessionId::new("native-spawn-failure"),
        "/definitely/not/a/real/command",
        size(),
    )
    .unwrap();

    let error = match NativePtySession::spawn(intent) {
        Ok(_) => panic!("spawn should fail"),
        Err(error) => error,
    };

    match error {
        NativePtyError::SpawnFailed {
            session_id,
            message,
        } => {
            assert_eq!(session_id.as_str(), "native-spawn-failure");
            assert!(!message.is_empty());
        }
        other => panic!("expected spawn failure, got {other}"),
    }
}

#[test]
fn native_pty_resizes_matching_session_only() {
    let mut session = NativePtySession::spawn(shell_intent("native-resize", "sleep 5")).unwrap();
    let new_size = PtySize::new(100, 30).unwrap();

    let mismatch = session
        .resize(ResizeIntent::new(
            PtySessionId::new("wrong-session"),
            new_size,
        ))
        .unwrap_err();
    assert_eq!(
        mismatch,
        NativePtyError::SessionMismatch {
            expected: PtySessionId::new("native-resize"),
            actual: PtySessionId::new("wrong-session"),
        }
    );

    session
        .resize(ResizeIntent::new(
            PtySessionId::new("native-resize"),
            new_size,
        ))
        .unwrap();
    assert_eq!(session.current_size().unwrap(), new_size);

    session.kill().unwrap();
    let exit = session.wait().unwrap();
    assert_eq!(exit.session_id().as_str(), "native-resize");
    assert!(!exit.succeeded());
}

#[test]
fn native_pty_closed_input_rejects_later_writes() {
    let mut session =
        NativePtySession::spawn(shell_intent("native-closed-input", "sleep 5")).unwrap();

    session.close_input();

    assert_eq!(
        session.write_input(b"ignored").unwrap_err(),
        NativePtyError::InputClosed {
            session_id: PtySessionId::new("native-closed-input"),
        }
    );

    session.kill().unwrap();
    let _ = session.wait().unwrap();
}
