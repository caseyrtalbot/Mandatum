//! Terminal screen buffer with bounded scrollback.
//!
//! [`TerminalGrid`] owns the visible cell matrix, the cursor, and a bounded ring
//! of rows that have scrolled off the top of the primary screen. The scrollback
//! is terminal-presentation state: it is read-only to the renderer and is never
//! serialized into durable core session state. The mutation primitives are
//! `pub(crate)` so the parser adapters in sibling modules can drive them without
//! exposing screen surgery to the rest of the workspace.

use std::collections::VecDeque;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    CellStyle, GridPosition, TerminalCell, TerminalCellOccupancy, TerminalCursor, TerminalSize,
};

const TAB_WIDTH: u16 = 8;
const MAX_GRAPHEME_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GraphemeWrite {
    Extended { at_edge: bool },
    New { grapheme: String, width: u8 },
}

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
                .filter_map(|cell| match cell.occupancy() {
                    TerminalCellOccupancy::Grapheme(grapheme) => Some(grapheme.as_str()),
                    TerminalCellOccupancy::WideContinuation => None,
                })
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
        self.scrollback.get(index).map(|row| {
            row.iter()
                .filter_map(|cell| match cell.occupancy() {
                    TerminalCellOccupancy::Grapheme(grapheme) => Some(grapheme.as_str()),
                    TerminalCellOccupancy::WideContinuation => None,
                })
                .collect()
        })
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
                .and_then(|row| row.get(usize::from(column)).cloned());
        }
        let screen_row = absolute_row - scrollback_len;
        if screen_row >= usize::from(self.size.rows()) {
            return None;
        }
        self.cell(GridPosition::new(screen_row as u16, column))
            .cloned()
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
                next_cells[next_index] = self.cells[old_index].clone();
            }
        }

        self.size = size;
        self.cells = next_cells;
        for row in 0..size.rows() {
            self.repair_row(row, CellStyle::default());
        }
        self.cursor.position = GridPosition::new(
            self.cursor.row().min(size.rows() - 1),
            self.cursor.column().min(size.columns() - 1),
        );
    }

    // --- Cursor + printing primitives (driven by the parser adapters) ----------

    pub(crate) fn write_printable(&mut self, character: char, wrap_pending: bool) -> GraphemeWrite {
        if let Some(at_edge) = self.extend_previous_grapheme(character, wrap_pending) {
            return GraphemeWrite::Extended { at_edge };
        }

        let mut grapheme = character.to_string();
        let width = grapheme_width(&grapheme);
        if width == 0 {
            grapheme.insert(0, '\u{25cc}');
        }
        GraphemeWrite::New {
            grapheme: grapheme.clone(),
            width: grapheme_width(&grapheme).clamp(1, 2) as u8,
        }
    }

    pub(crate) fn put_grapheme(&mut self, grapheme: String, width: u8, style: CellStyle) -> bool {
        debug_assert!((1..=2).contains(&width));
        let row = self.cursor.row();
        let column = self.cursor.column();
        self.clear_occupied_cell(row, column, style);
        if width == 2 {
            self.clear_occupied_cell(row, column + 1, style);
        }

        let index = self
            .cell_index(GridPosition::new(row, column))
            .expect("cursor stays in grid bounds");
        self.cells[index] = TerminalCell::grapheme(grapheme, style);
        if width == 2 {
            let next = self
                .cell_index(GridPosition::new(row, column + 1))
                .expect("double-width grapheme was wrapped before placement");
            self.cells[next] = TerminalCell::wide_continuation(style);
        }

        let next_column = column.saturating_add(u16::from(width));
        let at_edge = next_column >= self.size.columns();
        self.cursor.position =
            GridPosition::new(row, next_column.min(self.size.columns().saturating_sub(1)));
        at_edge
    }

    fn extend_previous_grapheme(&mut self, character: char, wrap_pending: bool) -> Option<bool> {
        let row = self.cursor.row();
        let cursor_column = self.cursor.column();
        let candidate_column = if wrap_pending {
            cursor_column
        } else {
            cursor_column.checked_sub(1)?
        };
        let lead_column = self.leading_column(row, candidate_column)?;
        let index = self.cell_index(GridPosition::new(row, lead_column))?;
        let TerminalCellOccupancy::Grapheme(current) = self.cells[index].occupancy() else {
            return None;
        };
        let mut candidate = current.clone();
        candidate.push(character);
        if candidate.len() > MAX_GRAPHEME_BYTES || candidate.graphemes(true).count() != 1 {
            return None;
        }

        let old_width = if self
            .cell(GridPosition::new(row, lead_column.saturating_add(1)))
            .is_some_and(|cell| matches!(cell.occupancy(), TerminalCellOccupancy::WideContinuation))
        {
            2u16
        } else {
            1
        };
        let new_width = grapheme_width(&candidate).clamp(1, 2) as u16;
        if new_width == 2 && lead_column + 1 >= self.size.columns() {
            return None;
        }

        let style = self.cells[index].style();
        self.cells[index] = TerminalCell::grapheme(candidate, style);
        if old_width < new_width {
            self.clear_occupied_cell(row, lead_column + 1, style);
            let continuation = self
                .cell_index(GridPosition::new(row, lead_column + 1))
                .expect("extended grapheme continuation is in bounds");
            self.cells[continuation] = TerminalCell::wide_continuation(style);
        } else if old_width > new_width {
            let continuation = self
                .cell_index(GridPosition::new(row, lead_column + 1))
                .expect("old continuation is in bounds");
            self.cells[continuation] = TerminalCell::blank_with_background(style);
        }

        let next_column = lead_column + new_width;
        let at_edge = next_column >= self.size.columns();
        self.cursor.position =
            GridPosition::new(row, next_column.min(self.size.columns().saturating_sub(1)));
        Some(at_edge)
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
        let next_stop = (self.cursor.column() / TAB_WIDTH)
            .saturating_add(1)
            .saturating_mul(TAB_WIDTH);
        let target = next_stop.min(self.size.columns().saturating_sub(1));
        while self.cursor.column() < target {
            self.put_grapheme(" ".to_owned(), 1, style);
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
                self.cells[index] = blank.clone();
            }
        }
        self.repair_row(row, style);
    }

    pub(crate) fn erase_in_display(&mut self, mode: u16, style: CellStyle) {
        let blank = TerminalCell::blank_with_background(style);
        match mode {
            1 => {
                let last = self.cells.len().saturating_sub(1);
                let cursor_index = self.cell_index(self.cursor.position()).unwrap_or(last);
                for cell in &mut self.cells[..=cursor_index.min(last)] {
                    *cell = blank.clone();
                }
            }
            2 => {
                self.cells.fill(blank);
            }
            3 => {
                self.scrollback.clear();
            }
            _ => {
                if let Some(cursor_index) = self.cell_index(self.cursor.position()) {
                    for cell in &mut self.cells[cursor_index..] {
                        *cell = blank.clone();
                    }
                }
            }
        }
        for row in 0..self.size.rows() {
            self.repair_row(row, style);
        }
    }

    pub(crate) fn erase_chars(&mut self, count: u16, style: CellStyle) {
        let row = self.cursor.row();
        let start = self.cursor.column();
        let blank = TerminalCell::blank_with_background(style);
        for column in start..(start.saturating_add(count)).min(self.size.columns()) {
            if let Some(index) = self.cell_index(GridPosition::new(row, column)) {
                self.cells[index] = blank.clone();
            }
        }
        self.repair_row(row, style);
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
                    .cloned()
                    .unwrap_or_else(|| blank.clone()),
                _ => blank.clone(),
            };
            if let Some(index) = self.cell_index(GridPosition::new(row, target)) {
                self.cells[index] = value;
            }
        }
        self.repair_row(row, style);
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
                    .cloned()
                    .unwrap_or_else(|| blank.clone()),
                _ => blank.clone(),
            };
            if let Some(index) = self.cell_index(GridPosition::new(row, column)) {
                self.cells[index] = value;
            }
        }
        self.repair_row(row, style);
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
        let captures_full_screen_scrollback =
            capture_scrollback && top == 0 && bottom == self.size.rows().saturating_sub(1);
        let blank = TerminalCell::blank_with_background(style);
        for _ in 0..count {
            if captures_full_screen_scrollback {
                let evicted = self.row_cells(top);
                self.push_scrollback(evicted);
            }
            for row in top..bottom {
                let source = self.row_cells(row + 1);
                self.write_row(row, &source);
            }
            self.fill_row(bottom, blank.clone());
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
            self.fill_row(top, blank.clone());
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
        self.cells[start..start + columns].clone_from_slice(source);
    }

    fn fill_row(&mut self, row: u16, value: TerminalCell) {
        let columns = self.column_count();
        let start = usize::from(row) * columns;
        for cell in &mut self.cells[start..start + columns] {
            *cell = value.clone();
        }
    }

    fn leading_column(&self, row: u16, column: u16) -> Option<u16> {
        let cell = self.cell(GridPosition::new(row, column))?;
        match cell.occupancy() {
            TerminalCellOccupancy::Grapheme(_) => Some(column),
            TerminalCellOccupancy::WideContinuation => column.checked_sub(1),
        }
    }

    fn clear_occupied_cell(&mut self, row: u16, column: u16, style: CellStyle) {
        if column >= self.size.columns() {
            return;
        }
        let lead = self.leading_column(row, column).unwrap_or(column);
        let blank = TerminalCell::blank_with_background(style);
        if let Some(index) = self.cell_index(GridPosition::new(row, lead)) {
            self.cells[index] = blank.clone();
        }
        let continuation_column = lead.saturating_add(1);
        if let Some(index) = self.cell_index(GridPosition::new(row, continuation_column))
            && matches!(
                self.cells[index].occupancy(),
                TerminalCellOccupancy::WideContinuation
            )
        {
            self.cells[index] = blank;
        }
    }

    fn repair_row(&mut self, row: u16, style: CellStyle) {
        let columns = self.size.columns();
        let blank = TerminalCell::blank_with_background(style);
        for column in 0..columns {
            let index = self
                .cell_index(GridPosition::new(row, column))
                .expect("row repair remains in bounds");
            match self.cells[index].occupancy() {
                TerminalCellOccupancy::WideContinuation => {
                    let valid = column.checked_sub(1).is_some_and(|lead| {
                        self.cell(GridPosition::new(row, lead)).is_some_and(|cell| {
                            matches!(
                                cell.occupancy(),
                                TerminalCellOccupancy::Grapheme(grapheme)
                                    if grapheme_width(grapheme) == 2
                            )
                        })
                    });
                    if !valid {
                        self.cells[index] = blank.clone();
                    }
                }
                TerminalCellOccupancy::Grapheme(grapheme) if grapheme_width(grapheme) == 2 => {
                    if column + 1 >= columns {
                        self.cells[index] = blank.clone();
                    } else {
                        let next = self
                            .cell_index(GridPosition::new(row, column + 1))
                            .expect("wide continuation is in bounds");
                        if !matches!(
                            self.cells[next].occupancy(),
                            TerminalCellOccupancy::WideContinuation
                        ) {
                            self.cells[index] = blank.clone();
                        }
                    }
                }
                _ => {}
            }
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

fn grapheme_width(grapheme: &str) -> usize {
    UnicodeWidthStr::width(grapheme)
}
