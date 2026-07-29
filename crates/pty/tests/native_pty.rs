#![cfg(unix)]

use std::{
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

use mandatum_pty::{
    ChildExitStatus, NativePtyError, NativePtySession, PtyEvent, PtySessionId, PtySize,
    ResizeIntent, SpawnIntent, WRITE_QUEUE_CAPACITY_BYTES,
};

/// Apple's `openpty` routes through `ptsname`'s static buffer, so concurrent
/// in-process calls can corrupt each other and fail with a garbage errno.
/// The product spawns panes serially on one thread and never hits this; the
/// test harness runs every test at once, so spawns serialize here instead.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn spawn_serialized(intent: SpawnIntent) -> Result<NativePtySession, NativePtyError> {
    let _guard = SPAWN_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    NativePtySession::spawn(intent)
}

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
    let mut session = spawn_serialized(shell_intent(
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
    let mut session = spawn_serialized(shell_intent(
        "native-input",
        "stty -echo; IFS= read line; printf 'reply:%s' \"$line\"",
    ))
    .unwrap();

    session.write_input(b"sample\n").unwrap();

    let output = read_until_contains(&mut session, b"reply:sample");

    assert!(contains_bytes(&output, b"reply:sample"));
    assert_eq!(
        session.wait().unwrap().status(),
        ChildExitStatus::Exited { code: 0 }
    );
}

#[test]
fn native_pty_reports_child_exit_status() {
    let mut session = spawn_serialized(shell_intent("native-exit", "exit 7")).unwrap();

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

    let error = match spawn_serialized(intent) {
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
fn native_pty_rejects_missing_cwd_instead_of_home_fallback() {
    // portable-pty would silently run the child in `$HOME` here; the spawn
    // boundary must reject the missing directory instead.
    let intent =
        shell_intent("native-missing-cwd", "pwd").with_cwd("/definitely/not/a/real/directory");

    let error = match spawn_serialized(intent) {
        Ok(_) => panic!("spawn should fail"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        NativePtyError::CwdNotFound {
            session_id: PtySessionId::new("native-missing-cwd"),
            cwd: PathBuf::from("/definitely/not/a/real/directory"),
        }
    );
}

#[test]
fn native_pty_resizes_matching_session_only() {
    let mut session = spawn_serialized(shell_intent("native-resize", "sleep 5")).unwrap();
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
    let mut session = spawn_serialized(shell_intent("native-closed-input", "sleep 5")).unwrap();

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

#[test]
fn native_pty_split_supports_concurrent_read_write_and_control() {
    let session = spawn_serialized(shell_intent(
        "native-split",
        "stty -echo; IFS= read line; printf 'split:%s' \"$line\"",
    ))
    .unwrap();
    let mut parts = session.into_split().unwrap();

    assert_eq!(parts.reader.session_id().as_str(), "native-split");
    assert_eq!(parts.writer.session_id().as_str(), "native-split");
    assert_eq!(parts.controller.session_id().as_str(), "native-split");
    assert!(parts.controller.process_id().is_some());

    parts.writer.write_input(b"sample\n").unwrap();

    let mut output = Vec::new();
    for _ in 0..8 {
        let Some(event) = parts.reader.read_event(1024).unwrap() else {
            break;
        };
        let PtyEvent::Output(chunk) = event else {
            panic!("expected output event");
        };
        output.extend(chunk.into_bytes());
        if contains_bytes(&output, b"split:sample") {
            break;
        }
    }

    assert!(contains_bytes(&output, b"split:sample"));
    assert_eq!(
        parts.controller.wait().unwrap().status(),
        ChildExitStatus::Exited { code: 0 }
    );
}

// The split writer feeds a dedicated writer thread, so pasting far more than
// the kernel tty buffer into a child that never reads must enqueue and return
// instead of blocking the caller.
#[test]
fn native_pty_split_writer_enqueues_without_blocking_when_child_never_reads() {
    let session = spawn_serialized(shell_intent("native-writer-nonblocking", "sleep 5")).unwrap();
    let mut parts = session.into_split().unwrap();
    let chunk = vec![b'x'; 64 * 1024];

    let started = Instant::now();
    for _ in 0..8 {
        parts.writer.write_input(&chunk).unwrap();
    }

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "write_input blocked for {:?} behind a full tty buffer",
        started.elapsed()
    );

    parts.controller.kill().unwrap();
    let _ = parts.controller.wait().unwrap();
}

#[test]
fn native_pty_split_writer_rejects_writes_past_queue_capacity() {
    let session = spawn_serialized(shell_intent("native-writer-overflow", "sleep 5")).unwrap();
    let mut parts = session.into_split().unwrap();

    // One oversized chunk overflows even an empty queue, so the rejection is
    // deterministic regardless of how much the writer thread has drained.
    let oversized = vec![b'x'; WRITE_QUEUE_CAPACITY_BYTES + 1];
    match parts.writer.write_input(&oversized) {
        Err(NativePtyError::WriteQueueFull {
            session_id,
            queued_bytes: _,
            rejected_bytes,
        }) => {
            assert_eq!(session_id.as_str(), "native-writer-overflow");
            assert_eq!(rejected_bytes, WRITE_QUEUE_CAPACITY_BYTES + 1);
        }
        other => panic!("expected WriteQueueFull, got {other:?}"),
    }

    parts.controller.kill().unwrap();
    let _ = parts.controller.wait().unwrap();
}

// Closing the writer must not drop queued bytes: the writer thread drains
// what was accepted before it exits.
#[test]
fn native_pty_split_writer_drains_queued_input_after_close() {
    let session = spawn_serialized(shell_intent(
        "native-writer-drain",
        "stty -echo; IFS= read line; printf 'drained:%s' \"$line\"",
    ))
    .unwrap();
    let mut parts = session.into_split().unwrap();

    parts.writer.write_input(b"sample\n").unwrap();
    parts.writer.close_input();
    assert_eq!(
        parts.writer.write_input(b"ignored").unwrap_err(),
        NativePtyError::InputClosed {
            session_id: PtySessionId::new("native-writer-drain"),
        }
    );

    let mut output = Vec::new();
    for _ in 0..8 {
        let Some(event) = parts.reader.read_event(1024).unwrap() else {
            break;
        };
        let PtyEvent::Output(chunk) = event else {
            panic!("expected output event");
        };
        output.extend(chunk.into_bytes());
        if contains_bytes(&output, b"drained:sample") {
            break;
        }
    }

    assert!(contains_bytes(&output, b"drained:sample"));
    assert_eq!(
        parts.controller.wait().unwrap().status(),
        ChildExitStatus::Exited { code: 0 }
    );
}

// Dropping the writer while its thread is blocked mid-write must return
// immediately: the thread is detached and exits on its own once the child
// dies, so shutdown never waits on a wedged write.
#[test]
fn native_pty_split_writer_drop_returns_while_writes_are_blocked() {
    let session = spawn_serialized(shell_intent("native-writer-shutdown", "sleep 5")).unwrap();
    let mut parts = session.into_split().unwrap();
    let chunk = vec![b'x'; 64 * 1024];
    for _ in 0..8 {
        parts.writer.write_input(&chunk).unwrap();
    }

    let started = Instant::now();
    drop(parts.writer);

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "dropping the writer blocked for {:?}",
        started.elapsed()
    );

    parts.controller.kill().unwrap();
    let _ = parts.controller.wait().unwrap();
}

#[test]
fn native_pty_split_controller_resizes_matching_session_only() {
    let session = spawn_serialized(shell_intent("native-split-resize", "sleep 5")).unwrap();
    let mut parts = session.into_split().unwrap();
    let new_size = PtySize::new(90, 20).unwrap();

    let mismatch = parts
        .controller
        .resize(ResizeIntent::new(PtySessionId::new("wrong"), new_size))
        .unwrap_err();
    assert_eq!(
        mismatch,
        NativePtyError::SessionMismatch {
            expected: PtySessionId::new("native-split-resize"),
            actual: PtySessionId::new("wrong"),
        }
    );

    parts
        .controller
        .resize(ResizeIntent::new(
            PtySessionId::new("native-split-resize"),
            new_size,
        ))
        .unwrap();
    assert_eq!(parts.controller.current_size().unwrap(), new_size);

    parts.controller.kill().unwrap();
    let _ = parts.controller.wait().unwrap();
}
