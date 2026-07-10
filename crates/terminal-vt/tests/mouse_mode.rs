// [L4-GATE] TerminalAdapter conformance: mouse-mode exposure. Every backend
// must report the child's mouse-reporting request through `mouse_mode()` so
// the workspace can honor child mouse capture (L5). A backend that does not
// interpret DECSET mode changes must report `Off`, which keeps pointer
// input with the workspace.

use mandatum_terminal_vt::{
    FakeTerminalAdapter, MouseMode, MouseTracking, TerminalAdapter, TerminalParser, TerminalSize,
};

fn vte(columns: u16, rows: u16) -> TerminalParser {
    TerminalParser::new(TerminalSize::new(columns, rows).unwrap())
}

#[test]
fn vte_backend_tracks_decset_mouse_modes() {
    let mut parser = vte(20, 4);
    assert_eq!(parser.mouse_mode(), MouseMode::default());
    assert!(!parser.capabilities().mouse_reporting);

    // DECSET 1000: press/release tracking.
    parser.feed_pty_bytes(b"\x1b[?1000h").unwrap();
    assert_eq!(parser.mouse_mode().tracking, MouseTracking::Normal);
    assert!(!parser.mouse_mode().sgr);
    assert!(parser.capabilities().mouse_reporting);

    // DECSET 1006: SGR encoding rides on top of the tracking mode.
    parser.feed_pty_bytes(b"\x1b[?1006h").unwrap();
    assert_eq!(
        parser.mouse_mode(),
        MouseMode {
            tracking: MouseTracking::Normal,
            sgr: true,
        }
    );

    // Higher-granularity requests win; disabling one falls back.
    parser.feed_pty_bytes(b"\x1b[?1002h\x1b[?1003h").unwrap();
    assert_eq!(parser.mouse_mode().tracking, MouseTracking::AnyEvent);
    parser.feed_pty_bytes(b"\x1b[?1003l").unwrap();
    assert_eq!(parser.mouse_mode().tracking, MouseTracking::ButtonEvent);
    parser.feed_pty_bytes(b"\x1b[?1002l").unwrap();
    assert_eq!(parser.mouse_mode().tracking, MouseTracking::Normal);

    // Releasing the last tracking mode returns the pointer to the workspace.
    parser.feed_pty_bytes(b"\x1b[?1000l\x1b[?1006l").unwrap();
    assert_eq!(parser.mouse_mode(), MouseMode::default());
    assert!(!parser.capabilities().mouse_reporting);
}

#[test]
fn vte_backend_maps_x10_tracking_and_clears_modes_on_reset() {
    let mut parser = vte(20, 4);

    // DECSET 9 (X10 press-only) maps to Normal tracking.
    parser.feed_pty_bytes(b"\x1b[?9h").unwrap();
    assert_eq!(parser.mouse_mode().tracking, MouseTracking::Normal);

    // RIS (full reset) drops every mouse request.
    parser.feed_pty_bytes(b"\x1b[?1003h\x1b[?1006h").unwrap();
    parser.feed_pty_bytes(b"\x1bc").unwrap();
    assert_eq!(parser.mouse_mode(), MouseMode::default());
}

#[test]
fn fake_backend_reports_no_mouse_request_even_when_fed_decset_bytes() {
    // The fixture backend does not interpret CSI sequences, so it must hold
    // the conformance default: Off, pointer stays with the workspace.
    let mut adapter = FakeTerminalAdapter::new(TerminalSize::new(20, 4).unwrap());
    adapter.feed(b"\x1b[?1000h\x1b[?1006h").unwrap();
    assert_eq!(adapter.mouse_mode(), MouseMode::default());
    assert!(!adapter.capabilities().mouse_reporting);
}

#[test]
fn terminal_parser_delegates_mouse_mode_to_its_adapter() {
    let mut parser =
        TerminalParser::with_adapter(FakeTerminalAdapter::new(TerminalSize::new(8, 2).unwrap()));
    parser.feed_pty_bytes(b"\x1b[?1000h").unwrap();
    assert_eq!(parser.mouse_mode(), MouseMode::default());
}
