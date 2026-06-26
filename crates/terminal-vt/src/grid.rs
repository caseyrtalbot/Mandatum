//! Terminal screen buffer with bounded scrollback.
//!
//! [`TerminalGrid`] owns the visible cell matrix, the cursor, and a bounded ring
//! of rows that have scrolled off the top of the primary screen. The scrollback
//! is terminal-presentation state: it is read-only to the renderer and is never
//! serialized into durable core session state. The mutation primitives are
//! `pub(crate)` so the parser adapters in sibling modules can drive them without
//! exposing screen surgery to the rest of the workspace.

use std::collections::VecDeque;

use crate::{CellStyle, GridPosition, TerminalCell, TerminalCursor, TerminalSize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalGrid {
    size: TerminalSize,
    cells: Vec<TerminalCell>,
    cursor: TerminalCursor,
    scrollback: VecDeque<Vec<TerminalCell>>,
    scrollback_limit: usize,
}

impl TerminalGrid {
    pub fn new(size: TerminalSize) -> Self {
        Self::with_scrollback_limit(size, crate::DEFAULT_SCROLLBACK_LIMIT)
    }

    pub fn with_scrollback_limit(size: TerminalSize, scrollback_limit: usize) -> Self {
        let cell_count = usize::from(size.columns()) * usize::from(size.rows());
        Self {
            size,
            cells: vec![TerminalCell::default(); cell_count],
            cursor: TerminalCursor::default(),
            scrollback: VecDeque::new(),
            scrollback_limit,
        }
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn cursor(&self) -> TerminalCursor {
        self.cursor
    }

    pub fn cell(&self, position: GridPosition) -> Option<&TerminalCell> {
        self.cell_index(position)
            .and_then(|index| self.cells.get(index))
    }

    pub fn row_text(&self, row: u16) -> Option<String> {
        if row >= self.size.rows() {
            return None;
        }

        let columns = self.column_count();
        let start = usize::from(row) * columns;
        let end = start + columns;
        Some(
            self.cells[start..end]
                .iter()
                .map(TerminalCell::character)
                .collect(),
        )
    }

    pub fn snapshot(&self) -> Vec<String> {
        (0..self.size.rows())
            .map(|row| self.row_text(row).expect("row must be in grid bounds"))
            .collect()
    }

    // --- Scrollback (read-only presentation history) ---------------------------

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }

    /// Number of rows of history plus visible rows, used to address the combined
    /// scrollback-plus-screen buffer by absolute row index.
    pub fn total_rows(&self) -> usize {
        self.scrollback.len() + usize::from(self.size.rows())
    }

    pub fn scrollback_row(&self, index: usize) -> Option<&[TerminalCell]> {
        self.scrollback.get(index).map(Vec::as_slice)
    }

    pub fn scrollback_row_text(&self, index: usize) -> Option<String> {
        self.scrollback
            .get(index)
            .map(|row| row.iter().map(TerminalCell::character).collect())
    }

    /// Read a cell from the combined scrollback-plus-screen buffer.
    ///
    /// Absolute rows `0..scrollback_len()` index history (oldest first); rows at
    /// and beyond `scrollback_len()` index the visible screen.
    pub fn history_cell(&self, absolute_row: usize, column: u16) -> Option<TerminalCell> {
        let scrollback_len = self.scrollback.len();
        if absolute_row < scrollback_len {
            return self
                .scrollback
                .get(absolute_row)
                .and_then(|row| row.get(usize::from(column)).copied());
        }
        let screen_row = absolute_row - scrollback_len;
        if screen_row >= usize::from(self.size.rows()) {
            return None;
        }
        self.cell(GridPosition::new(screen_row as u16, column))
            .copied()
    }

    pub fn resize(&mut self, size: TerminalSize) {
        let mut next_cells =
            vec![TerminalCell::default(); usize::from(size.columns()) * usize::from(size.rows())];
        let copied_rows = self.size.rows().min(size.rows());
        let copied_columns = self.size.columns().min(size.columns());

        for row in 0..copied_rows {
            for column in 0..copied_columns {
                let old_index = self.cell_index(GridPosition::new(row, column)).unwrap();
                let next_index =
                    usize::from(row) * usize::from(size.columns()) + usize::from(column);
                next_cells[next_index] = self.cells[old_index];
            }
        }

        self.size = size;
        self.cells = next_cells;
        self.cursor.position = GridPosition::new(
            self.cursor.row().min(size.rows() - 1),
            self.cursor.column().min(size.columns() - 1),
        );
    }

    // --- Cursor + printing primitives (driven by the parser adapters) ----------

    pub(crate) fn put_styled(&mut self, character: char, style: CellStyle) {
        if let Some(index) = self.cell_index(self.cursor.position()) {
            self.cells[index] = TerminalCell::styled(character, style);
        }
    }

    pub(crate) fn set_cursor(&mut self, row: u16, column: u16) {
        let row = row.min(self.size.rows().saturating_sub(1));
        let column = column.min(self.size.columns().saturating_sub(1));
        self.cursor.position = GridPosition::new(row, column);
    }

    pub(crate) fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor.visible = visible;
    }

    pub(crate) fn carriage_return(&mut self) {
        self.cursor.position = GridPosition::new(self.cursor.row(), 0);
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor.column() > 0 {
            self.cursor.position = GridPosition::new(self.cursor.row(), self.cursor.column() - 1);
        }
    }

    pub(crate) fn move_cursor_right(&mut self) {
        if self.cursor.column() + 1 < self.size.columns() {
            self.cursor.position = GridPosition::new(self.cursor.row(), self.cursor.column() + 1);
        }
    }

    pub(crate) fn cursor_at_last_column(&self) -> bool {
        self.cursor.column() + 1 >= self.size.columns()
    }

    /// Line feed within `[top, bottom]`: move down one row, scrolling the region
    /// up (and capturing into scrollback when the top of the screen leaves) once
    /// the cursor reaches the bottom of the region.
    pub(crate) fn index(&mut self, top: u16, bottom: u16, capture_scrollback: bool) {
        if self.cursor.row() == bottom {
            self.scroll_up_region(1, top, bottom, capture_scrollback, CellStyle::default());
        } else if self.cursor.row() + 1 < self.size.rows() {
            self.cursor.position = GridPosition::new(self.cursor.row() + 1, self.cursor.column());
        }
    }

    /// Reverse line feed within `[top, bottom]`: move up one row, scrolling the
    /// region down once the cursor reaches the top of the region.
    pub(crate) fn reverse_index(&mut self, top: u16, bottom: u16) {
        if self.cursor.row() == top {
            self.scroll_down_region(1, top, bottom, CellStyle::default());
        } else if self.cursor.row() > 0 {
            self.cursor.position = GridPosition::new(self.cursor.row() - 1, self.cursor.column());
        }
    }

    pub(crate) fn tab(&mut self, style: CellStyle) {
        let next_stop = (self.cursor.column() / 8)
            .saturating_add(1)
            .saturating_mul(8);
        let target = next_stop.min(self.size.columns().saturating_sub(1));
        while self.cursor.column() < target {
            self.put_styled(' ', style);
            self.move_cursor_right();
            if self.cursor_at_last_column() {
                break;
            }
        }
    }

    // --- Erase + edit primitives -----------------------------------------------

    pub(crate) fn erase_in_line(&mut self, mode: u16, style: CellStyle) {
        let row = self.cursor.row();
        let columns = self.size.columns();
        let (start, end) = match mode {
            1 => (0, self.cursor.column() + 1),
            2 => (0, columns),
            _ => (self.cursor.column(), columns),
        };
        let blank = TerminalCell::blank_with_background(style);
        for column in start..end.min(columns) {
            if let Some(index) = self.cell_index(GridPosition::new(row, column)) {
                self.cells[index] = blank;
            }
        }
    }

    pub(crate) fn erase_in_display(&mut self, mode: u16, style: CellStyle) {
        let blank = TerminalCell::blank_with_background(style);
        match mode {
            1 => {
                let last = self.cells.len().saturating_sub(1);
                let cursor_index = self.cell_index(self.cursor.position()).unwrap_or(last);
                for cell in &mut self.cells[..=cursor_index.min(last)] {
                    *cell = blank;
                }
            }
            2 | 3 => {
                self.cells.fill(blank);
                if mode == 3 {
                    self.scrollback.clear();
                }
            }
            _ => {
                if let Some(cursor_index) = self.cell_index(self.cursor.position()) {
                    for cell in &mut self.cells[cursor_index..] {
                        *cell = blank;
                    }
                }
            }
        }
    }

    pub(crate) fn erase_chars(&mut self, count: u16, style: CellStyle) {
        let row = self.cursor.row();
        let start = self.cursor.column();
        let blank = TerminalCell::blank_with_background(style);
        for column in start..(start.saturating_add(count)).min(self.size.columns()) {
            if let Some(index) = self.cell_index(GridPosition::new(row, column)) {
                self.cells[index] = blank;
            }
        }
    }

    pub(crate) fn insert_chars(&mut self, count: u16, style: CellStyle) {
        let row = self.cursor.row();
        let columns = self.size.columns();
        let cursor = self.cursor.column();
        let count = count.min(columns - cursor);
        let blank = TerminalCell::blank_with_background(style);
        let mut column = columns;
        while column > cursor {
            column -= 1;
            let target = column;
            let source = column.checked_sub(count);
            let value = match source {
                Some(source) if source >= cursor => self
                    .cell(GridPosition::new(row, source))
                    .copied()
                    .unwrap_or(blank),
                _ => blank,
            };
            if let Some(index) = self.cell_index(GridPosition::new(row, target)) {
                self.cells[index] = value;
            }
        }
    }

    pub(crate) fn delete_chars(&mut self, count: u16, style: CellStyle) {
        let row = self.cursor.row();
        let columns = self.size.columns();
        let cursor = self.cursor.column();
        let count = count.min(columns - cursor);
        let blank = TerminalCell::blank_with_background(style);
        for column in cursor..columns {
            let source = column.checked_add(count);
            let value = match source {
                Some(source) if source < columns => self
                    .cell(GridPosition::new(row, source))
                    .copied()
                    .unwrap_or(blank),
                _ => blank,
            };
            if let Some(index) = self.cell_index(GridPosition::new(row, column)) {
                self.cells[index] = value;
            }
        }
    }

    pub(crate) fn insert_lines(&mut self, count: u16, top: u16, bottom: u16, style: CellStyle) {
        if self.cursor.row() < top || self.cursor.row() > bottom {
            return;
        }
        self.scroll_down_region(count, self.cursor.row(), bottom, style);
    }

    pub(crate) fn delete_lines(&mut self, count: u16, top: u16, bottom: u16, style: CellStyle) {
        if self.cursor.row() < top || self.cursor.row() > bottom {
            return;
        }
        self.scroll_up_region(count, self.cursor.row(), bottom, false, style);
    }

    pub(crate) fn clear_all(&mut self) {
        self.cells.fill(TerminalCell::default());
        self.cursor = TerminalCursor::default();
    }

    // --- Region scrolling ------------------------------------------------------

    pub(crate) fn scroll_up_region(
        &mut self,
        count: u16,
        top: u16,
        bottom: u16,
        capture_scrollback: bool,
        style: CellStyle,
    ) {
        if top > bottom {
            return;
        }
        let blank = TerminalCell::blank_with_background(style);
        for _ in 0..count {
            if capture_scrollback && top == 0 {
                let evicted = self.row_cells(top);
                self.push_scrollback(evicted);
            }
            for row in top..bottom {
                let source = self.row_cells(row + 1);
                self.write_row(row, &source);
            }
            self.fill_row(bottom, blank);
        }
    }

    pub(crate) fn scroll_down_region(
        &mut self,
        count: u16,
        top: u16,
        bottom: u16,
        style: CellStyle,
    ) {
        if top > bottom {
            return;
        }
        let blank = TerminalCell::blank_with_background(style);
        for _ in 0..count {
            let mut row = bottom;
            while row > top {
                let source = self.row_cells(row - 1);
                self.write_row(row, &source);
                row -= 1;
            }
            self.fill_row(top, blank);
        }
    }

    // --- Internal helpers ------------------------------------------------------

    fn push_scrollback(&mut self, row: Vec<TerminalCell>) {
        if self.scrollback_limit == 0 {
            return;
        }
        self.scrollback.push_back(row);
        while self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
        }
    }

    fn row_cells(&self, row: u16) -> Vec<TerminalCell> {
        let columns = self.column_count();
        let start = usize::from(row) * columns;
        self.cells[start..start + columns].to_vec()
    }

    fn write_row(&mut self, row: u16, source: &[TerminalCell]) {
        let columns = self.column_count();
        let start = usize::from(row) * columns;
        self.cells[start..start + columns].copy_from_slice(source);
    }

    fn fill_row(&mut self, row: u16, value: TerminalCell) {
        let columns = self.column_count();
        let start = usize::from(row) * columns;
        for cell in &mut self.cells[start..start + columns] {
            *cell = value;
        }
    }

    fn cell_index(&self, position: GridPosition) -> Option<usize> {
        if position.row() >= self.size.rows() || position.column() >= self.size.columns() {
            return None;
        }

        Some(usize::from(position.row()) * self.column_count() + usize::from(position.column()))
    }

    fn column_count(&self) -> usize {
        usize::from(self.size.columns())
    }
}
