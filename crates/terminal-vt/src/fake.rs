//! Minimal fixture parser backend.
//!
//! `FakeTerminalAdapter` predates the hardened [`crate::VteTerminalAdapter`] and
//! is retained so renderer-independent fixtures can exercise grid behavior
//! (printable text, carriage returns, wrapping, scroll, tab, and clear) without
//! depending on a full VT state machine. It treats `\n` as a newline (carriage
//! return plus line feed) and `\x0c` as clear; it does not interpret CSI escape
//! sequences. New runtime work should use the default backend.

use crate::{
    CellStyle, TerminalAdapter, TerminalAdapterError, TerminalCapabilities, TerminalGrid,
    TerminalSize, TerminalUpdate,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeTerminalAdapter {
    grid: TerminalGrid,
    capabilities: TerminalCapabilities,
    wrap_pending: bool,
    pending_utf8: Vec<u8>,
}

impl FakeTerminalAdapter {
    pub fn new(size: TerminalSize) -> Self {
        Self {
            grid: TerminalGrid::new(size),
            capabilities: TerminalCapabilities::default(),
            wrap_pending: false,
            pending_utf8: Vec::new(),
        }
    }

    fn decode_input(&mut self, bytes: &[u8]) -> Result<Option<String>, TerminalAdapterError> {
        self.pending_utf8.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending_utf8) {
            Ok(input) => {
                let input = input.to_owned();
                self.pending_utf8.clear();
                Ok(Some(input))
            }
            Err(error) if error.error_len().is_none() => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to == 0 {
                    Ok(None)
                } else {
                    let input = std::str::from_utf8(&self.pending_utf8[..valid_up_to])
                        .unwrap()
                        .to_owned();
                    self.pending_utf8.drain(..valid_up_to);
                    Ok(Some(input))
                }
            }
            Err(error) => {
                let message = error.to_string();
                self.pending_utf8.clear();
                Err(TerminalAdapterError::InvalidUtf8 { message })
            }
        }
    }

    /// Newline: carriage return plus line feed. Returns whether the screen
    /// scrolled, which counts as a screen change.
    fn newline(&mut self) -> bool {
        let scrolled = self.grid.cursor().row() + 1 >= self.grid.size().rows();
        self.grid.carriage_return();
        self.grid.index(0, self.grid.size().rows() - 1, true);
        scrolled
    }

    fn apply_printable(&mut self, character: char) {
        if self.wrap_pending {
            self.newline();
            self.wrap_pending = false;
        }

        self.grid.put_styled(character, CellStyle::default());
        if self.grid.cursor_at_last_column() {
            self.wrap_pending = true;
        } else {
            self.grid.move_cursor_right();
        }
    }

    fn apply_tab(&mut self) {
        let next_tab_stop = (self.grid.cursor().column() / 8)
            .saturating_add(1)
            .saturating_mul(8);
        let target_column = next_tab_stop.min(self.grid.size().columns());

        while self.grid.cursor().column() < target_column {
            self.apply_printable(' ');
            if self.wrap_pending {
                break;
            }
        }
    }

    fn apply_input(&mut self, input: &str) -> TerminalUpdate {
        let mut screen_changed = false;

        for character in input.chars() {
            match character {
                '\n' => {
                    screen_changed |= self.newline();
                    self.wrap_pending = false;
                }
                '\r' => {
                    self.grid.carriage_return();
                    self.wrap_pending = false;
                }
                '\u{0008}' => {
                    self.grid.backspace();
                    self.wrap_pending = false;
                }
                '\u{000c}' => {
                    self.grid.clear_all();
                    self.wrap_pending = false;
                    screen_changed = true;
                }
                '\t' => {
                    self.apply_tab();
                    screen_changed = true;
                }
                character if character.is_control() => {}
                character => {
                    self.apply_printable(character);
                    screen_changed = true;
                }
            }
        }

        TerminalUpdate {
            screen_changed,
            cursor: self.grid.cursor(),
        }
    }
}

impl TerminalAdapter for FakeTerminalAdapter {
    fn capabilities(&self) -> TerminalCapabilities {
        self.capabilities
    }

    fn size(&self) -> TerminalSize {
        self.grid.size()
    }

    fn grid(&self) -> &TerminalGrid {
        &self.grid
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<TerminalUpdate, TerminalAdapterError> {
        let Some(input) = self.decode_input(bytes)? else {
            return Ok(TerminalUpdate {
                screen_changed: false,
                cursor: self.grid.cursor(),
            });
        };

        Ok(self.apply_input(&input))
    }

    fn resize(&mut self, size: TerminalSize) {
        self.grid.resize(size);
        if self.grid.cursor().column() + 1 < self.grid.size().columns() {
            self.wrap_pending = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GridPosition, TerminalCell, TerminalCursor};

    #[test]
    fn grid_resize_preserves_top_left_overlap_and_clamps_cursor() {
        let size = TerminalSize::new(4, 2).unwrap();
        let mut adapter = FakeTerminalAdapter::new(size);

        adapter.feed(b"abc\ndef").unwrap();
        adapter.resize(TerminalSize::new(2, 1).unwrap());

        assert_eq!(adapter.grid().snapshot(), vec!["ab"]);
        assert_eq!(
            adapter.grid().cursor(),
            TerminalCursor::new(GridPosition::new(0, 1))
        );
    }

    #[test]
    fn fake_parser_buffers_split_utf8_across_feed_calls() {
        let mut adapter = FakeTerminalAdapter::new(TerminalSize::new(4, 2).unwrap());

        let first_update = adapter.feed(&[0xe2]).unwrap();
        let second_update = adapter.feed(&[0x82, 0xac]).unwrap();

        assert!(!first_update.screen_changed);
        assert!(second_update.screen_changed);
        assert_eq!(
            adapter
                .grid()
                .cell(GridPosition::new(0, 0))
                .map(TerminalCell::character),
            Some('\u{20ac}')
        );
    }
}
