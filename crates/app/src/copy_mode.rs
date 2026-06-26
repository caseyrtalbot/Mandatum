//! Keyboard copy mode: scrollback navigation plus pane-local text selection.
//!
//! `CopyModeState` is runtime presentation state owned by the app. It addresses
//! the combined scrollback-plus-screen buffer in absolute rows (rows
//! `0..scrollback_len` are history) and tracks a scroll offset, a cursor, and an
//! optional selection anchor. It reads the parser's [`TerminalGrid`] read-only;
//! it never mutates the grid, owns a PTY handle, or touches core state.

use mandatum_core::PaneId;
use mandatum_terminal_vt::TerminalGrid;

pub struct CopyModeState {
    pub pane_id: PaneId,
    pub scroll_offset: usize,
    pub cursor_row: usize,
    pub cursor_col: u16,
    pub anchor: Option<(usize, u16)>,
}

impl CopyModeState {
    /// Enter copy mode at the bottom-left of the live buffer.
    pub fn enter(pane_id: PaneId, grid: &TerminalGrid) -> Self {
        let cursor_row = grid.total_rows().saturating_sub(1);
        let mut state = Self {
            pane_id,
            scroll_offset: 0,
            cursor_row,
            cursor_col: 0,
            anchor: None,
        };
        state.clamp(grid);
        state
    }

    fn view_rows(grid: &TerminalGrid) -> usize {
        usize::from(grid.size().rows())
    }

    fn columns(grid: &TerminalGrid) -> u16 {
        grid.size().columns()
    }

    pub fn move_up(&mut self, count: usize, grid: &TerminalGrid) {
        self.cursor_row = self.cursor_row.saturating_sub(count);
        self.clamp(grid);
    }

    pub fn move_down(&mut self, count: usize, grid: &TerminalGrid) {
        self.cursor_row = self.cursor_row.saturating_add(count);
        self.clamp(grid);
    }

    pub fn move_left(&mut self, count: u16, grid: &TerminalGrid) {
        self.cursor_col = self.cursor_col.saturating_sub(count);
        self.clamp(grid);
    }

    pub fn move_right(&mut self, count: u16, grid: &TerminalGrid) {
        self.cursor_col = self.cursor_col.saturating_add(count);
        self.clamp(grid);
    }

    pub fn page_up(&mut self, grid: &TerminalGrid) {
        self.move_up(Self::view_rows(grid), grid);
    }

    pub fn page_down(&mut self, grid: &TerminalGrid) {
        self.move_down(Self::view_rows(grid), grid);
    }

    pub fn move_to_top(&mut self, grid: &TerminalGrid) {
        self.cursor_row = 0;
        self.clamp(grid);
    }

    pub fn move_to_bottom(&mut self, grid: &TerminalGrid) {
        self.cursor_row = grid.total_rows().saturating_sub(1);
        self.clamp(grid);
    }

    pub fn line_start(&mut self, grid: &TerminalGrid) {
        self.cursor_col = 0;
        self.clamp(grid);
    }

    pub fn line_end(&mut self, grid: &TerminalGrid) {
        self.cursor_col = Self::columns(grid).saturating_sub(1);
        self.clamp(grid);
    }

    pub fn set_anchor(&mut self) {
        self.anchor = Some((self.cursor_row, self.cursor_col));
    }

    pub fn clear_anchor(&mut self) {
        self.anchor = None;
    }

    /// Keep the cursor inside the buffer and scroll so it stays visible.
    pub fn clamp(&mut self, grid: &TerminalGrid) {
        let total = grid.total_rows();
        let view_rows = Self::view_rows(grid);
        self.cursor_row = self.cursor_row.min(total.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(Self::columns(grid).saturating_sub(1));

        let max_top = total.saturating_sub(view_rows);
        let first_visible = max_top.saturating_sub(self.scroll_offset);
        let new_first = if self.cursor_row < first_visible {
            self.cursor_row
        } else if self.cursor_row >= first_visible + view_rows {
            self.cursor_row + 1 - view_rows
        } else {
            first_visible
        };
        self.scroll_offset = max_top.saturating_sub(new_first);
    }

    /// The ordered selection span (start <= end in reading order), if selecting.
    pub fn selection_span(&self) -> Option<((usize, u16), (usize, u16))> {
        let anchor = self.anchor?;
        let cursor = (self.cursor_row, self.cursor_col);
        Some(if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        })
    }

    /// Extract the selected text as a stream selection across rows. With no
    /// anchor, copies the cursor's current line.
    pub fn selected_text(&self, grid: &TerminalGrid) -> String {
        let columns = Self::columns(grid);
        let last_column = columns.saturating_sub(1);
        let ((start_row, start_col), (end_row, end_col)) = self
            .selection_span()
            .unwrap_or(((self.cursor_row, 0), (self.cursor_row, last_column)));

        let mut lines = Vec::new();
        for row in start_row..=end_row {
            let (from, to) = if start_row == end_row {
                (start_col, end_col)
            } else if row == start_row {
                (start_col, last_column)
            } else if row == end_row {
                (0, end_col)
            } else {
                (0, last_column)
            };

            let mut line = String::new();
            for column in from..=to {
                let character = grid
                    .history_cell(row, column)
                    .map(|cell| cell.character())
                    .unwrap_or(' ');
                line.push(character);
            }
            lines.push(line.trim_end().to_owned());
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandatum_terminal_vt::{TerminalParser, TerminalSize};

    fn grid_with(lines: &[&str], columns: u16, rows: u16) -> TerminalParser {
        let mut parser = TerminalParser::new(TerminalSize::new(columns, rows).unwrap());
        let joined = lines.join("\r\n");
        parser.feed_pty_bytes(joined.as_bytes()).unwrap();
        parser
    }

    #[test]
    fn enter_places_cursor_at_buffer_bottom() {
        let parser = grid_with(&["alpha", "bravo"], 8, 2);
        let state = CopyModeState::enter(PaneId::new("pane-1"), parser.grid());
        assert_eq!(state.cursor_row, parser.grid().total_rows() - 1);
        assert_eq!(state.scroll_offset, 0);
        assert!(state.anchor.is_none());
    }

    #[test]
    fn selecting_a_single_line_extracts_its_text() {
        let parser = grid_with(&["hello world"], 16, 1);
        let mut state = CopyModeState::enter(PaneId::new("pane-1"), parser.grid());
        state.line_start(parser.grid());
        state.set_anchor();
        state.move_right(4, parser.grid()); // cursor over "hello"[..=4]
        assert_eq!(state.selected_text(parser.grid()), "hello");
    }

    #[test]
    fn selection_spans_multiple_rows_including_scrollback() {
        // 8x2 grid: feeding four lines pushes the first two into scrollback.
        let parser = grid_with(&["one", "two", "three", "four"], 8, 2);
        let grid = parser.grid();
        assert!(grid.scrollback_len() >= 2);

        let mut state = CopyModeState::enter(PaneId::new("pane-1"), grid);
        state.move_to_top(grid); // absolute row 0 = "one"
        state.line_start(grid);
        state.set_anchor();
        state.move_to_bottom(grid);
        state.line_end(grid);
        let text = state.selected_text(grid);
        assert_eq!(text, "one\ntwo\nthree\nfour");
    }

    #[test]
    fn scrolling_up_keeps_cursor_visible() {
        let parser = grid_with(&["one", "two", "three", "four"], 8, 2);
        let grid = parser.grid();
        let mut state = CopyModeState::enter(PaneId::new("pane-1"), grid);
        state.move_to_top(grid);
        // The top row is at the maximum scroll offset.
        let max_top = grid.total_rows() - usize::from(grid.size().rows());
        assert_eq!(state.scroll_offset, max_top);
        assert_eq!(state.cursor_row, 0);
    }

    #[test]
    fn no_anchor_copies_current_line() {
        let parser = grid_with(&["first", "second"], 8, 2);
        let grid = parser.grid();
        let mut state = CopyModeState::enter(PaneId::new("pane-1"), grid);
        state.move_to_top(grid); // absolute row 0 = "first"
        assert_eq!(state.selected_text(grid), "first");
    }
}
