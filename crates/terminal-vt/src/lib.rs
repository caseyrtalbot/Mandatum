//! Terminal parser adapter boundary.
//!
//! The first Milestone 2 seam uses a fake parser so renderer-independent tests
//! can exercise terminal grid behavior before binding a real VT parser.
//!
//! `libghostty-vt` has been evaluated as a future optional backend, but this
//! crate intentionally has no Ghostty, Zig, CMake, or FFI dependency yet.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    columns: u16,
    rows: u16,
}

impl TerminalSize {
    pub fn new(columns: u16, rows: u16) -> Result<Self, TerminalSizeError> {
        if columns == 0 || rows == 0 {
            return Err(TerminalSizeError { columns, rows });
        }

        Ok(Self { columns, rows })
    }

    pub fn columns(&self) -> u16 {
        self.columns
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSizeError {
    pub columns: u16,
    pub rows: u16,
}

impl fmt::Display for TerminalSizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "terminal size must be non-zero, got {}x{}",
            self.columns, self.rows
        )
    }
}

impl std::error::Error for TerminalSizeError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridPosition {
    row: u16,
    column: u16,
}

impl GridPosition {
    pub fn new(row: u16, column: u16) -> Self {
        Self { row, column }
    }

    pub fn row(&self) -> u16 {
        self.row
    }

    pub fn column(&self) -> u16 {
        self.column
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursor {
    position: GridPosition,
    visible: bool,
}

impl TerminalCursor {
    pub fn new(position: GridPosition) -> Self {
        Self {
            position,
            visible: true,
        }
    }

    pub fn position(&self) -> GridPosition {
        self.position
    }

    pub fn row(&self) -> u16 {
        self.position.row()
    }

    pub fn column(&self) -> u16 {
        self.position.column()
    }

    pub fn visible(&self) -> bool {
        self.visible
    }
}

impl Default for TerminalCursor {
    fn default() -> Self {
        Self::new(GridPosition::new(0, 0))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub bold: bool,
    pub inverse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    character: char,
    style: CellStyle,
}

impl TerminalCell {
    pub fn blank() -> Self {
        Self {
            character: ' ',
            style: CellStyle::default(),
        }
    }

    pub fn character(&self) -> char {
        self.character
    }

    pub fn style(&self) -> CellStyle {
        self.style
    }
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self::blank()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalGrid {
    size: TerminalSize,
    cells: Vec<TerminalCell>,
    cursor: TerminalCursor,
}

impl TerminalGrid {
    pub fn new(size: TerminalSize) -> Self {
        let cell_count = usize::from(size.columns()) * usize::from(size.rows());
        Self {
            size,
            cells: vec![TerminalCell::default(); cell_count],
            cursor: TerminalCursor::default(),
        }
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn cursor(&self) -> TerminalCursor {
        self.cursor
    }

    pub fn cell(&self, position: GridPosition) -> Option<&TerminalCell> {
        self.index(position).and_then(|index| self.cells.get(index))
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

    pub fn resize(&mut self, size: TerminalSize) {
        let mut next_cells =
            vec![TerminalCell::default(); usize::from(size.columns()) * usize::from(size.rows())];
        let copied_rows = self.size.rows().min(size.rows());
        let copied_columns = self.size.columns().min(size.columns());

        for row in 0..copied_rows {
            for column in 0..copied_columns {
                let old_index = self.index(GridPosition::new(row, column)).unwrap();
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

    fn put_character(&mut self, character: char) {
        let index = self
            .index(self.cursor.position())
            .expect("cursor must stay in grid bounds");
        self.cells[index] = TerminalCell {
            character,
            style: CellStyle::default(),
        };
    }

    fn line_feed(&mut self) -> bool {
        if self.cursor.row() + 1 >= self.size.rows() {
            self.scroll_up();
            self.cursor.position = GridPosition::new(self.size.rows() - 1, 0);
            true
        } else {
            self.cursor.position = GridPosition::new(self.cursor.row() + 1, 0);
            false
        }
    }

    fn carriage_return(&mut self) {
        self.cursor.position = GridPosition::new(self.cursor.row(), 0);
    }

    fn backspace(&mut self) {
        if self.cursor.column() > 0 {
            self.cursor.position = GridPosition::new(self.cursor.row(), self.cursor.column() - 1);
        }
    }

    fn move_cursor_right(&mut self) {
        self.cursor.position = GridPosition::new(self.cursor.row(), self.cursor.column() + 1);
    }

    fn clear(&mut self) {
        self.cells.fill(TerminalCell::default());
        self.cursor = TerminalCursor::default();
    }

    fn scroll_up(&mut self) {
        let columns = self.column_count();
        if self.size.rows() > 1 {
            self.cells.copy_within(columns.., 0);
        }

        let last_row_start = columns * (usize::from(self.size.rows()) - 1);
        for cell in &mut self.cells[last_row_start..] {
            *cell = TerminalCell::default();
        }
    }

    fn index(&self, position: GridPosition) -> Option<usize> {
        if position.row() >= self.size.rows() || position.column() >= self.size.columns() {
            return None;
        }

        Some(usize::from(position.row()) * self.column_count() + usize::from(position.column()))
    }

    fn column_count(&self) -> usize {
        usize::from(self.size.columns())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCapabilities {
    pub true_color: bool,
    pub mouse_reporting: bool,
    pub alternate_screen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalUpdate {
    pub screen_changed: bool,
    pub cursor: TerminalCursor,
}

pub trait TerminalAdapter {
    fn capabilities(&self) -> TerminalCapabilities;
    fn size(&self) -> TerminalSize;
    fn grid(&self) -> &TerminalGrid;
    fn feed(&mut self, bytes: &[u8]) -> Result<TerminalUpdate, TerminalAdapterError>;
    fn resize(&mut self, size: TerminalSize);
}

pub struct TerminalParser {
    adapter: Box<dyn TerminalAdapter>,
}

impl TerminalParser {
    pub fn new(size: TerminalSize) -> Self {
        Self::with_adapter(FakeTerminalAdapter::new(size))
    }

    pub fn with_adapter(adapter: impl TerminalAdapter + 'static) -> Self {
        Self {
            adapter: Box::new(adapter),
        }
    }

    pub fn capabilities(&self) -> TerminalCapabilities {
        self.adapter.capabilities()
    }

    pub fn size(&self) -> TerminalSize {
        self.adapter.size()
    }

    pub fn grid(&self) -> &TerminalGrid {
        self.adapter.grid()
    }

    pub fn feed_pty_bytes(&mut self, bytes: &[u8]) -> Result<TerminalUpdate, TerminalAdapterError> {
        self.adapter.feed(bytes)
    }

    pub fn resize(&mut self, size: TerminalSize) {
        self.adapter.resize(size);
    }
}

impl TerminalAdapter for TerminalParser {
    fn capabilities(&self) -> TerminalCapabilities {
        self.capabilities()
    }

    fn size(&self) -> TerminalSize {
        self.size()
    }

    fn grid(&self) -> &TerminalGrid {
        self.grid()
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<TerminalUpdate, TerminalAdapterError> {
        self.feed_pty_bytes(bytes)
    }

    fn resize(&mut self, size: TerminalSize) {
        self.resize(size);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalAdapterError {
    InvalidUtf8 { message: String },
}

impl fmt::Display for TerminalAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 { message } => write!(formatter, "invalid UTF-8 input: {message}"),
        }
    }
}

impl std::error::Error for TerminalAdapterError {}

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

    fn apply_printable(&mut self, character: char) {
        if self.wrap_pending {
            self.grid.line_feed();
            self.wrap_pending = false;
        }

        self.grid.put_character(character);
        if self.grid.cursor().column() + 1 >= self.grid.size().columns() {
            self.wrap_pending = true;
        } else {
            self.grid.move_cursor_right();
        }
    }

    fn apply_tab(&mut self) {
        let next_tab_stop = ((self.grid.cursor().column() / 8) + 1) * 8;
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
                    screen_changed |= self.grid.line_feed();
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
                    self.grid.clear();
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

    #[test]
    fn rejects_zero_sized_terminal_grid() {
        assert_eq!(
            TerminalSize::new(0, 24),
            Err(TerminalSizeError {
                columns: 0,
                rows: 24,
            })
        );
        assert_eq!(
            TerminalSize::new(80, 0),
            Err(TerminalSizeError {
                columns: 80,
                rows: 0,
            })
        );
    }

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
