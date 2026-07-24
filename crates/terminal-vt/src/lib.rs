//! Terminal parser adapter boundary.
//!
//! `terminal-vt` owns terminal parser adapters and hides the concrete parser
//! choice behind [`TerminalAdapter`]. The default backend is a local parser
//! built on the pure-Rust [`vte`] tokenizer; a [`FakeTerminalAdapter`] remains
//! for renderer-independent fixtures.
//!
//! `libghostty-vt` has been evaluated as a future optional backend, but this
//! crate intentionally has no Ghostty, Zig, CMake, or FFI dependency. The only
//! external dependency is the pure-Rust `vte` escape-sequence state machine.

mod fake;
mod grid;
mod vte_backend;

use std::fmt;

pub use fake::FakeTerminalAdapter;
pub use grid::TerminalGrid;

/// Bounded number of scrolled-off rows retained per terminal grid.
pub const DEFAULT_SCROLLBACK_LIMIT: usize = 2000;

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

/// Renderer-neutral terminal color.
///
/// `Indexed(0..=15)` are the standard plus bright ANSI colors, `Indexed(16..=255)`
/// the 256-color palette, and `Rgb` is direct-color. `Default` means "use the
/// surface default", which the renderer maps to its reset color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Per-cell styling carried from the parser to the renderer.
///
/// All fields default to "off"/`Color::Default`, so `CellStyle::default()` is a
/// plain, unstyled cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub foreground: Color,
    pub background: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

/// What occupies one terminal grid cell.
///
/// A grapheme is stored once in its leading cell. A double-width grapheme
/// reserves the following cell with `WideContinuation`, so erasure,
/// selection, cursor placement, and both frontends agree on the same columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalCellOccupancy {
    Grapheme(String),
    WideContinuation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    occupancy: TerminalCellOccupancy,
    style: CellStyle,
}

impl TerminalCell {
    pub fn blank() -> Self {
        Self {
            occupancy: TerminalCellOccupancy::Grapheme(" ".to_owned()),
            style: CellStyle::default(),
        }
    }

    pub fn styled(character: char, style: CellStyle) -> Self {
        Self::grapheme(character.to_string(), style)
    }

    pub fn grapheme(grapheme: String, style: CellStyle) -> Self {
        Self {
            occupancy: TerminalCellOccupancy::Grapheme(grapheme),
            style,
        }
    }

    pub fn wide_continuation(style: CellStyle) -> Self {
        Self {
            occupancy: TerminalCellOccupancy::WideContinuation,
            style,
        }
    }

    /// A blank cell that keeps the given background, used when erasing regions so
    /// a painted background color survives a clear.
    pub fn blank_with_background(style: CellStyle) -> Self {
        Self {
            occupancy: TerminalCellOccupancy::Grapheme(" ".to_owned()),
            style: CellStyle {
                background: style.background,
                ..CellStyle::default()
            },
        }
    }

    /// Compatibility scalar for callers that only need plain snapshots.
    /// Grapheme-aware callers should use [`Self::occupancy`].
    pub fn character(&self) -> char {
        match &self.occupancy {
            TerminalCellOccupancy::Grapheme(grapheme) => grapheme.chars().next().unwrap_or(' '),
            TerminalCellOccupancy::WideContinuation => ' ',
        }
    }

    pub fn occupancy(&self) -> &TerminalCellOccupancy {
        &self.occupancy
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCapabilities {
    pub true_color: bool,
    pub mouse_reporting: bool,
    pub alternate_screen: bool,
}

/// How much mouse tracking the child application requested (DECSET 9/1000/
/// 1002/1003). Ordered by granularity so the highest requested mode wins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum MouseTracking {
    /// The child did not ask for the mouse; the workspace owns pointer input.
    #[default]
    Off,
    /// Press/release reporting (DECSET 9 or 1000).
    Normal,
    /// Press/release plus motion while a button is held (DECSET 1002).
    ButtonEvent,
    /// All motion (DECSET 1003).
    AnyEvent,
}

/// The child's live mouse-reporting request: tracking granularity plus the
/// wire encoding it expects. [L5-GATE] While `tracking` is not `Off`, the
/// workspace must forward pointer events to the child instead of acting on
/// them, unless the user invokes explicit workspace-level control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseMode {
    pub tracking: MouseTracking,
    /// `true` once the child enabled SGR encoding (DECSET 1006).
    pub sgr: bool,
}

impl MouseMode {
    /// Whether the child asked for pointer events at all.
    pub fn wants_mouse(&self) -> bool {
        self.tracking != MouseTracking::Off
    }
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

    /// The child's current mouse-reporting request. Backends that do not
    /// interpret DECSET mode changes must report the default (`Off`), which
    /// keeps pointer input with the workspace (L5).
    fn mouse_mode(&self) -> MouseMode {
        MouseMode::default()
    }
}

/// Owns one terminal parser backend per pane and hides the concrete choice.
///
/// [`TerminalParser::new`] selects the hardened default backend so the app and
/// renderer never name a parser implementation.
pub struct TerminalParser {
    adapter: Box<dyn TerminalAdapter>,
}

impl TerminalParser {
    pub fn new(size: TerminalSize) -> Self {
        Self::with_adapter(vte_backend::VteTerminalAdapter::new(size))
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

    /// The child's current mouse-reporting request (L5 pointer routing).
    pub fn mouse_mode(&self) -> MouseMode {
        self.adapter.mouse_mode()
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

    fn mouse_mode(&self) -> MouseMode {
        self.mouse_mode()
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
    fn default_parser_uses_hardened_vt_backend() {
        let parser = TerminalParser::new(TerminalSize::new(80, 24).unwrap());
        // The hardened backend models true color and alternate screen support.
        assert!(parser.capabilities().true_color);
        assert!(parser.capabilities().alternate_screen);
    }

    #[test]
    fn cell_style_default_is_unstyled() {
        let style = CellStyle::default();
        assert_eq!(style.foreground, Color::Default);
        assert_eq!(style.background, Color::Default);
        assert!(!style.bold);
        assert!(!style.inverse);
    }
}
