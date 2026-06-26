use std::collections::BTreeMap;

use mandatum_terminal_vt::{
    FakeTerminalAdapter, GridPosition, TerminalAdapter, TerminalAdapterError, TerminalGrid,
    TerminalParser, TerminalSize,
};

fn fixture(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < input.len() {
        if input[index] != b'\\' {
            if index + 1 == input.len() && input[index] == b'\n' {
                break;
            }
            output.push(input[index]);
            index += 1;
            continue;
        }

        let Some(escaped) = input.get(index + 1) else {
            output.push(input[index]);
            break;
        };
        match escaped {
            b'n' => output.push(b'\n'),
            b'r' => output.push(b'\r'),
            b't' => output.push(b'\t'),
            b'b' => output.push(0x08),
            b'f' => output.push(0x0c),
            escaped => {
                output.push(b'\\');
                output.push(*escaped);
            }
        }
        index += 2;
    }
    output
}

fn trimmed_grid_rows(grid: &TerminalGrid) -> Vec<String> {
    grid.snapshot()
        .into_iter()
        .map(|row| row.trim_end().to_owned())
        .collect()
}

fn trimmed_rows(adapter: &FakeTerminalAdapter) -> Vec<String> {
    trimmed_grid_rows(adapter.grid())
}

#[test]
fn fixture_stream_populates_grid_rows() {
    let mut adapter = FakeTerminalAdapter::new(TerminalSize::new(20, 4).unwrap());

    let update = adapter
        .feed(&fixture(include_bytes!("fixtures/basic-output.txt")))
        .unwrap();

    assert!(update.screen_changed);
    assert_eq!(trimmed_rows(&adapter), vec!["cargo test", "ok", "", ""]);
    assert_eq!(adapter.grid().cursor().position(), GridPosition::new(2, 0));
}

#[test]
fn fixture_carriage_return_overwrites_progress_line() {
    let mut adapter = FakeTerminalAdapter::new(TerminalSize::new(12, 2).unwrap());

    adapter
        .feed(&fixture(include_bytes!("fixtures/carriage-return.txt")))
        .unwrap();

    assert_eq!(trimmed_rows(&adapter), vec!["build 100%", ""]);
    assert_eq!(
        adapter
            .grid()
            .cell(GridPosition::new(0, 8))
            .map(|cell| cell.character()),
        Some('0')
    );
}

#[test]
fn fixture_newlines_scroll_when_stream_exceeds_grid_height() {
    let mut adapter = FakeTerminalAdapter::new(TerminalSize::new(8, 3).unwrap());

    adapter
        .feed(&fixture(include_bytes!("fixtures/scrolling-output.txt")))
        .unwrap();

    assert_eq!(trimmed_rows(&adapter), vec!["two", "three", "four"]);
}

#[test]
fn fixture_wrapping_defers_scroll_until_next_printable_character() {
    let mut adapter = FakeTerminalAdapter::new(TerminalSize::new(4, 2).unwrap());

    adapter
        .feed(&fixture(include_bytes!("fixtures/wrapping-output.txt")))
        .unwrap();

    assert_eq!(trimmed_rows(&adapter), vec!["abcd", "efgh"]);
    assert_eq!(adapter.grid().cursor().position(), GridPosition::new(1, 3));
}

#[test]
fn fixture_backspace_tab_and_clear_controls_are_supported() {
    let mut adapter = FakeTerminalAdapter::new(TerminalSize::new(10, 2).unwrap());

    adapter
        .feed(&fixture(include_bytes!("fixtures/control-output.txt")))
        .unwrap();

    assert_eq!(trimmed_rows(&adapter), vec!["A       B", ""]);
    assert_eq!(adapter.grid().cursor().position(), GridPosition::new(0, 9));
}

#[test]
fn fake_parser_reports_invalid_utf8() {
    let mut adapter = FakeTerminalAdapter::new(TerminalSize::new(4, 2).unwrap());

    let error = adapter.feed(&[0xff]).unwrap_err();

    assert!(matches!(error, TerminalAdapterError::InvalidUtf8 { .. }));
}

#[test]
fn terminal_parser_can_be_owned_per_pane_and_feed_pty_bytes() {
    let mut panes = BTreeMap::new();
    panes.insert(
        "left",
        TerminalParser::new(TerminalSize::new(8, 2).unwrap()),
    );
    panes.insert(
        "right",
        TerminalParser::new(TerminalSize::new(8, 2).unwrap()),
    );

    let left_update = panes
        .get_mut("left")
        .unwrap()
        .feed_pty_bytes(b"left")
        .unwrap();
    let right_update = panes
        .get_mut("right")
        .unwrap()
        .feed_pty_bytes(b"right")
        .unwrap();

    assert!(left_update.screen_changed);
    assert!(right_update.screen_changed);
    assert_eq!(
        trimmed_grid_rows(panes.get("left").unwrap().grid()),
        vec!["left", ""]
    );
    assert_eq!(
        trimmed_grid_rows(panes.get("right").unwrap().grid()),
        vec!["right", ""]
    );
}

#[test]
fn terminal_parser_delegates_resize_and_custom_adapter_feed() {
    let adapter = FakeTerminalAdapter::new(TerminalSize::new(6, 2).unwrap());
    let mut parser = TerminalParser::with_adapter(adapter);

    parser.feed_pty_bytes(b"abcdef").unwrap();
    parser.resize(TerminalSize::new(3, 1).unwrap());

    assert_eq!(parser.size(), TerminalSize::new(3, 1).unwrap());
    assert_eq!(trimmed_grid_rows(parser.grid()), vec!["abc"]);
}
