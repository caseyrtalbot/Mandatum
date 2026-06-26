//! End-to-end smoke tests: a real shell process drives the hardened parser.
//!
//! These exercise the `pty` -> `terminal-vt` integration with a live `/bin/sh`
//! child emitting real VT output, standing in for the interactive `cargo run`
//! smoke where an automated terminal is not available. They assert that common
//! shell output renders as text without leaking raw escape sequences, that
//! command output round-trips, and that moderate output is captured into bounded
//! scrollback without hanging.

use mandatum_pty::{NativePtySession, PtyEvent, PtySessionId, PtySize, SpawnIntent};
use mandatum_terminal_vt::{TerminalParser, TerminalSize};

/// Run `script` under `/bin/sh -c`, feed all its PTY output through the default
/// hardened parser, and return the parser for inspection.
fn run_shell(script: &str, columns: u16, rows: u16) -> TerminalParser {
    let size = PtySize::new(columns, rows).expect("non-zero pty size");
    let intent = SpawnIntent::new(PtySessionId::new("smoke"), "/bin/sh", size)
        .expect("spawn intent")
        .with_arguments(["-c", script])
        .with_environment([("TERM", "xterm-256color")]);
    let mut session = NativePtySession::spawn(intent).expect("spawn shell");
    let mut parser = TerminalParser::new(TerminalSize::new(columns, rows).expect("terminal size"));

    loop {
        match session.read_event(8192) {
            Ok(Some(PtyEvent::Output(output))) => {
                parser
                    .feed_pty_bytes(&output.into_bytes())
                    .expect("parser feed");
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let _ = session.wait();
    parser
}

fn visible_text(parser: &TerminalParser) -> String {
    parser
        .grid()
        .snapshot()
        .into_iter()
        .map(|row| row.trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn real_shell_color_output_does_not_leak_escapes() {
    let parser = run_shell(
        "printf '\\033[31;1mRED\\033[0m\\n'; printf 'plain text\\n'",
        24,
        6,
    );
    let text = visible_text(&parser);

    assert!(
        text.contains("RED"),
        "expected styled text content: {text:?}"
    );
    assert!(text.contains("plain text"), "expected plain line: {text:?}");
    assert!(
        !text.contains('\u{1b}'),
        "raw ESC leaked into the grid: {text:?}"
    );
    assert!(
        !text.contains("[31") && !text.contains("[0m"),
        "raw SGR sequence leaked as text: {text:?}"
    );
}

#[test]
fn real_shell_echo_round_trips() {
    let parser = run_shell("echo M4_COMPLETE_SMOKE", 40, 4);
    let text = visible_text(&parser);
    assert!(
        text.contains("M4_COMPLETE_SMOKE"),
        "echo did not round-trip: {text:?}"
    );
}

#[test]
fn real_shell_cursor_addressing_does_not_leak() {
    // Clear screen, home the cursor, and address a position, then print.
    let parser = run_shell("printf '\\033[2J\\033[3;5HXY\\033[1;1Htop'", 20, 6);
    let text = visible_text(&parser);
    assert!(
        text.contains("top"),
        "cursor-addressed text missing: {text:?}"
    );
    assert!(text.contains("XY"), "positioned text missing: {text:?}");
    assert!(!text.contains('\u{1b}'), "raw ESC leaked: {text:?}");
    assert!(
        !text.contains("[2J") && !text.contains("[3;5H"),
        "CSI leaked: {text:?}"
    );
}

#[test]
fn moderate_output_is_captured_into_bounded_scrollback() {
    // A pure POSIX-sh counting loop (no dependency on `seq`) printing 1..=200.
    let parser = run_shell(
        "i=1; while [ \"$i\" -le 200 ]; do echo \"$i\"; i=$((i+1)); done",
        16,
        5,
    );
    let grid = parser.grid();

    // The run completed (did not hang) and the latest line is visible.
    let visible = visible_text(&parser);
    assert!(
        visible.contains("200"),
        "tail of output not visible: {visible:?}"
    );

    // Earlier lines scrolled into bounded scrollback rather than being lost.
    assert!(
        grid.scrollback_len() >= 100,
        "expected substantial scrollback, got {}",
        grid.scrollback_len()
    );
    assert!(grid.scrollback_len() <= grid.scrollback_limit());

    // The very first line is recoverable from history.
    let first_history = grid.scrollback_row_text(0).unwrap_or_default();
    assert!(
        first_history.trim_end() == "1",
        "expected first history row to be '1', got {first_history:?}"
    );
}
