//! Terminal content surfaces: pre-windowed rows of styled cells plus the
//! viewport state a frontend needs to overlay cursor and selection marks.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::style::SceneCellStyle;

/// One styled cell of terminal content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneCell {
    pub character: char,
    pub style: SceneCellStyle,
}

impl Default for SceneCell {
    fn default() -> Self {
        Self {
            character: ' ',
            style: SceneCellStyle::default(),
        }
    }
}

/// One decoded, renderer-neutral RGBA8 sRGB artifact surface.
///
/// The app owns decoding and bounds enforcement. Shared immutable bytes keep
/// frame snapshots cheap to clone without introducing a decoder or renderer
/// resource into the scene contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RasterSurface {
    pub width: u32,
    pub height: u32,
    pub revision: u64,
    pub rgba8: Arc<[u8]>,
}

/// A point in the combined scrollback-plus-screen buffer, in absolute
/// coordinates: rows `0..scrollback_len` index history, rows at and beyond
/// `scrollback_len` index the live screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct SurfacePosition {
    pub row: usize,
    pub column: u16,
}

impl SurfacePosition {
    pub fn new(row: usize, column: u16) -> Self {
        Self { row, column }
    }
}

/// Renderable terminal content for one pane.
///
/// `rows` hold exactly the cells visible in the pane viewport, top to bottom,
/// already windowed to the pane's inner size — a frontend paints them without
/// knowing the parser or its scrollback storage. Cursor, selection, and
/// copy-cursor coordinates are absolute (see [`SurfacePosition`]); a frontend
/// maps a painted row back to absolute space via `first_row`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSurface {
    /// The visible rows, top to bottom.
    pub rows: Vec<Vec<SceneCell>>,
    /// Absolute index of `rows[0]` in the combined scrollback+screen buffer.
    pub first_row: usize,
    /// Live parser cursor in absolute coordinates; `None` when hidden.
    pub cursor: Option<SurfacePosition>,
    /// Rows scrolled up from the live bottom. `0` follows live output.
    pub scroll_offset: usize,
    /// Rows of history above the live screen.
    pub scrollback_len: usize,
    /// Inclusive selection span, pre-ordered so start <= end in reading order.
    pub selection: Option<(SurfacePosition, SurfacePosition)>,
    /// Copy-mode cursor; `Some` only while the pane is in copy mode.
    pub copy_cursor: Option<SurfacePosition>,
}

impl TerminalSurface {
    /// Whether the viewport follows live output (not scrolled into history).
    pub fn following_live(&self) -> bool {
        self.scroll_offset == 0
    }

    /// Whether the pane is being viewed in copy mode, which frontends mark in
    /// the pane title.
    pub fn in_copy_mode(&self) -> bool {
        self.copy_cursor.is_some() || self.scroll_offset > 0
    }

    /// Whether the given absolute cell falls inside the selection span.
    pub fn selection_contains(&self, row: usize, column: u16) -> bool {
        let Some((start, end)) = self.selection else {
            return false;
        };
        let after_start = row > start.row || (row == start.row && column >= start.column);
        let before_end = row < end.row || (row == end.row && column <= end.column);
        after_start && before_end
    }

    /// Whether a cursor mark is drawn at the given absolute cell: the copy
    /// cursor while in copy mode, otherwise the live cursor when following
    /// live output.
    pub fn cursor_at(&self, row: usize, column: u16) -> bool {
        if self.copy_cursor == Some(SurfacePosition::new(row, column)) {
            return true;
        }
        let show_live = self.scroll_offset == 0 && self.copy_cursor.is_none();
        show_live && self.cursor == Some(SurfacePosition::new(row, column))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface() -> TerminalSurface {
        TerminalSurface {
            rows: vec![vec![SceneCell::default(); 4]; 2],
            first_row: 2,
            cursor: Some(SurfacePosition::new(3, 1)),
            scroll_offset: 0,
            scrollback_len: 2,
            selection: None,
            copy_cursor: None,
        }
    }

    #[test]
    fn live_cursor_is_marked_only_while_following_live_output() {
        let live = surface();
        assert!(live.cursor_at(3, 1));
        assert!(!live.cursor_at(3, 0));

        let scrolled = TerminalSurface {
            scroll_offset: 1,
            ..surface()
        };
        assert!(!scrolled.cursor_at(3, 1));
        assert!(scrolled.in_copy_mode());
    }

    #[test]
    fn copy_cursor_replaces_the_live_cursor_mark() {
        let copying = TerminalSurface {
            copy_cursor: Some(SurfacePosition::new(2, 0)),
            ..surface()
        };
        assert!(copying.in_copy_mode());
        assert!(copying.cursor_at(2, 0));
        assert!(!copying.cursor_at(3, 1), "live cursor hidden in copy mode");
    }

    #[test]
    fn selection_span_is_inclusive_in_reading_order() {
        let selected = TerminalSurface {
            selection: Some((SurfacePosition::new(1, 2), SurfacePosition::new(2, 1))),
            ..surface()
        };
        assert!(selected.selection_contains(1, 2));
        assert!(selected.selection_contains(1, 3));
        assert!(selected.selection_contains(2, 0));
        assert!(selected.selection_contains(2, 1));
        assert!(!selected.selection_contains(1, 1));
        assert!(!selected.selection_contains(2, 2));
    }
}
