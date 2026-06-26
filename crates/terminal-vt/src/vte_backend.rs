//! Hardened local VT parser backend.
//!
//! [`VteTerminalAdapter`] is the default [`crate::TerminalAdapter`]. It drives a
//! [`TerminalState`] (a `vte::Perform`) over the pure-Rust `vte` escape-sequence
//! tokenizer, so the fiddly tokenization (UTF-8, CSI parameters, intermediate
//! bytes, OSC/DCS strings) is handled by a battle-tested state machine while the
//! grid semantics (SGR styling, cursor motion, erase/edit, scroll region,
//! alternate screen) live here. The rest of the workspace only sees the
//! [`crate::TerminalAdapter`] trait, so no caller names this backend.

use vte::{Params, Parser, Perform};

use crate::{
    CellStyle, Color, TerminalAdapter, TerminalAdapterError, TerminalCapabilities, TerminalGrid,
    TerminalSize, TerminalUpdate,
};

pub struct VteTerminalAdapter {
    parser: Parser,
    state: TerminalState,
}

impl VteTerminalAdapter {
    pub fn new(size: TerminalSize) -> Self {
        Self::with_scrollback_limit(size, crate::DEFAULT_SCROLLBACK_LIMIT)
    }

    pub fn with_scrollback_limit(size: TerminalSize, scrollback_limit: usize) -> Self {
        Self {
            parser: Parser::new(),
            state: TerminalState::new(size, scrollback_limit),
        }
    }
}

impl TerminalAdapter for VteTerminalAdapter {
    fn capabilities(&self) -> TerminalCapabilities {
        self.state.capabilities
    }

    fn size(&self) -> TerminalSize {
        self.state.active_grid().size()
    }

    fn grid(&self) -> &TerminalGrid {
        self.state.active_grid()
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<TerminalUpdate, TerminalAdapterError> {
        self.state.dirty = false;
        // Borrows two disjoint fields of `self`; the `vte` parser and the grid
        // state never alias.
        self.parser.advance(&mut self.state, bytes);
        Ok(TerminalUpdate {
            screen_changed: self.state.dirty,
            cursor: self.state.active_grid().cursor(),
        })
    }

    fn resize(&mut self, size: TerminalSize) {
        self.state.resize(size);
    }
}

#[derive(Clone, Copy)]
struct SavedCursor {
    row: u16,
    column: u16,
    pen: CellStyle,
    wrap_pending: bool,
}

struct TerminalState {
    primary: TerminalGrid,
    alternate: Option<TerminalGrid>,
    using_alternate: bool,
    pen: CellStyle,
    saved_cursor: Option<SavedCursor>,
    alt_return_cursor: Option<SavedCursor>,
    scroll_top: u16,
    scroll_bottom: u16,
    wrap_pending: bool,
    dirty: bool,
    capabilities: TerminalCapabilities,
}

impl TerminalState {
    fn new(size: TerminalSize, scrollback_limit: usize) -> Self {
        Self {
            primary: TerminalGrid::with_scrollback_limit(size, scrollback_limit),
            alternate: None,
            using_alternate: false,
            pen: CellStyle::default(),
            saved_cursor: None,
            alt_return_cursor: None,
            scroll_top: 0,
            scroll_bottom: size.rows() - 1,
            wrap_pending: false,
            dirty: false,
            capabilities: TerminalCapabilities {
                true_color: true,
                mouse_reporting: false,
                alternate_screen: true,
            },
        }
    }

    fn active_grid(&self) -> &TerminalGrid {
        if self.using_alternate {
            self.alternate.as_ref().expect("alternate grid present")
        } else {
            &self.primary
        }
    }

    fn active_grid_mut(&mut self) -> &mut TerminalGrid {
        if self.using_alternate {
            self.alternate.as_mut().expect("alternate grid present")
        } else {
            &mut self.primary
        }
    }

    fn resize(&mut self, size: TerminalSize) {
        self.primary.resize(size);
        if let Some(alt) = self.alternate.as_mut() {
            alt.resize(size);
        }
        self.scroll_top = 0;
        self.scroll_bottom = size.rows() - 1;
        self.wrap_pending = false;
    }

    // --- Cursor + line movement ------------------------------------------------

    fn set_cursor(&mut self, row: u16, column: u16) {
        self.wrap_pending = false;
        self.dirty = true;
        self.active_grid_mut().set_cursor(row, column);
    }

    fn set_cursor_column(&mut self, column: u16) {
        let row = self.active_grid().cursor().row();
        self.set_cursor(row, column);
    }

    fn set_cursor_row(&mut self, row: u16) {
        let column = self.active_grid().cursor().column();
        self.set_cursor(row, column);
    }

    fn cursor_up(&mut self, count: u16) {
        let cursor = self.active_grid().cursor();
        self.set_cursor(cursor.row().saturating_sub(count), cursor.column());
    }

    fn cursor_down(&mut self, count: u16) {
        let cursor = self.active_grid().cursor();
        self.set_cursor(cursor.row().saturating_add(count), cursor.column());
    }

    fn cursor_forward(&mut self, count: u16) {
        let cursor = self.active_grid().cursor();
        self.set_cursor(cursor.row(), cursor.column().saturating_add(count));
    }

    fn cursor_back(&mut self, count: u16) {
        let cursor = self.active_grid().cursor();
        self.set_cursor(cursor.row(), cursor.column().saturating_sub(count));
    }

    fn cursor_next_line(&mut self, count: u16) {
        let cursor = self.active_grid().cursor();
        self.set_cursor(cursor.row().saturating_add(count), 0);
    }

    fn cursor_prev_line(&mut self, count: u16) {
        let cursor = self.active_grid().cursor();
        self.set_cursor(cursor.row().saturating_sub(count), 0);
    }

    fn carriage_return(&mut self) {
        self.wrap_pending = false;
        self.dirty = true;
        self.active_grid_mut().carriage_return();
    }

    fn line_feed(&mut self) {
        self.dirty = true;
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let capture = !self.using_alternate;
        self.active_grid_mut().index(top, bottom, capture);
    }

    fn reverse_index(&mut self) {
        self.dirty = true;
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        self.active_grid_mut().reverse_index(top, bottom);
    }

    // --- Save / restore / reset ------------------------------------------------

    fn save_cursor(&mut self) {
        let cursor = self.active_grid().cursor();
        self.saved_cursor = Some(SavedCursor {
            row: cursor.row(),
            column: cursor.column(),
            pen: self.pen,
            wrap_pending: self.wrap_pending,
        });
    }

    fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor {
            self.pen = saved.pen;
            self.wrap_pending = saved.wrap_pending;
            self.dirty = true;
            self.active_grid_mut().set_cursor(saved.row, saved.column);
        }
    }

    fn reset(&mut self) {
        self.leave_alternate();
        self.pen = CellStyle::default();
        self.saved_cursor = None;
        self.alt_return_cursor = None;
        self.scroll_top = 0;
        self.scroll_bottom = self.primary.size().rows() - 1;
        self.wrap_pending = false;
        self.dirty = true;
        self.primary.erase_in_display(2, CellStyle::default());
        self.primary.erase_in_display(3, CellStyle::default());
        self.primary.set_cursor(0, 0);
    }

    fn enter_alternate(&mut self) {
        if self.using_alternate {
            return;
        }
        let cursor = self.primary.cursor();
        self.alt_return_cursor = Some(SavedCursor {
            row: cursor.row(),
            column: cursor.column(),
            pen: self.pen,
            wrap_pending: self.wrap_pending,
        });
        let size = self.primary.size();
        self.alternate = Some(TerminalGrid::with_scrollback_limit(size, 0));
        self.using_alternate = true;
        self.scroll_top = 0;
        self.scroll_bottom = size.rows() - 1;
        self.wrap_pending = false;
        self.dirty = true;
    }

    fn leave_alternate(&mut self) {
        if !self.using_alternate {
            return;
        }
        self.using_alternate = false;
        self.alternate = None;
        self.scroll_top = 0;
        self.scroll_bottom = self.primary.size().rows() - 1;
        if let Some(saved) = self.alt_return_cursor.take() {
            self.pen = saved.pen;
            self.wrap_pending = saved.wrap_pending;
            self.primary.set_cursor(saved.row, saved.column);
        }
        self.dirty = true;
    }

    // --- Modes -----------------------------------------------------------------

    fn set_mode(&mut self, params: &Params, private: bool, enabled: bool) {
        if !private {
            return;
        }
        for param in params.iter() {
            match param.first().copied().unwrap_or(0) {
                25 => {
                    self.dirty = true;
                    self.active_grid_mut().set_cursor_visible(enabled);
                }
                47 | 1047 | 1049 => {
                    if enabled {
                        self.enter_alternate();
                    } else {
                        self.leave_alternate();
                    }
                }
                _ => {}
            }
        }
    }

    // --- SGR styling -----------------------------------------------------------

    fn apply_sgr(&mut self, params: &Params) {
        // Iterate over parameter GROUPS (not a flattened stream). Each group is a
        // `&[u16]` of colon-separated subparameters. The extended-color
        // introducers `38`/`48` come in two forms: the ISO 8613-6 colon form
        // carries the whole color spec inside one group's subparameters
        // (`38:2:cs:r:g:b`, where `cs` may be an empty colorspace placeholder),
        // while the legacy semicolon form spreads it across following groups
        // (`38;2;r;g;b`). Flattening would conflate the two, so we respect group
        // boundaries.
        let groups: Vec<&[u16]> = params.iter().collect();
        if groups.is_empty() {
            self.pen = CellStyle::default();
            return;
        }

        let mut index = 0;
        while index < groups.len() {
            let group = groups[index];
            match group.first().copied().unwrap_or(0) {
                code @ (38 | 48) => {
                    let foreground = code == 38;
                    if group.len() >= 2 {
                        // Colon form: the color spec is this group's subparameters.
                        if let Some(color) = parse_colon_color(group) {
                            self.set_sgr_color(foreground, color);
                        }
                        index += 1;
                    } else {
                        // Semicolon form: the spec continues in following groups.
                        index += self.apply_semicolon_color(&groups, index, foreground);
                    }
                }
                code => {
                    self.apply_sgr_code(code);
                    index += 1;
                }
            }
        }
    }

    fn set_sgr_color(&mut self, foreground: bool, color: Color) {
        if foreground {
            self.pen.foreground = color;
        } else {
            self.pen.background = color;
        }
    }

    /// Consume a semicolon-form `38`/`48` introducer that spreads its color spec
    /// across the following groups. Returns the number of groups consumed (always
    /// at least 1 so the SGR loop makes progress).
    fn apply_semicolon_color(
        &mut self,
        groups: &[&[u16]],
        start: usize,
        foreground: bool,
    ) -> usize {
        let value_at = |offset: usize| groups.get(start + offset).and_then(|g| g.first().copied());
        match value_at(1) {
            Some(5) => match value_at(2) {
                Some(index) => {
                    self.set_sgr_color(foreground, Color::Indexed(index as u8));
                    3
                }
                None => 2,
            },
            Some(2) => match (value_at(2), value_at(3), value_at(4)) {
                (Some(r), Some(g), Some(b)) => {
                    self.set_sgr_color(foreground, Color::Rgb(r as u8, g as u8, b as u8));
                    5
                }
                _ => (groups.len() - start).max(1),
            },
            _ => 1,
        }
    }

    fn apply_sgr_code(&mut self, code: u16) {
        match code {
            0 => self.pen = CellStyle::default(),
            1 => self.pen.bold = true,
            2 => self.pen.dim = true,
            3 => self.pen.italic = true,
            4 => self.pen.underline = true,
            7 => self.pen.inverse = true,
            8 => self.pen.hidden = true,
            9 => self.pen.strikethrough = true,
            21 | 22 => {
                self.pen.bold = false;
                self.pen.dim = false;
            }
            23 => self.pen.italic = false,
            24 => self.pen.underline = false,
            27 => self.pen.inverse = false,
            28 => self.pen.hidden = false,
            29 => self.pen.strikethrough = false,
            30..=37 => self.pen.foreground = Color::Indexed((code - 30) as u8),
            39 => self.pen.foreground = Color::Default,
            40..=47 => self.pen.background = Color::Indexed((code - 40) as u8),
            49 => self.pen.background = Color::Default,
            90..=97 => self.pen.foreground = Color::Indexed((code - 90 + 8) as u8),
            100..=107 => self.pen.background = Color::Indexed((code - 100 + 8) as u8),
            _ => {}
        }
    }

    fn dispatch_cursor_csi(&mut self, params: &Params, action: char) {
        match action {
            'H' | 'f' => {
                let row = pos_param(params, 0);
                let column = pos_param(params, 1);
                self.set_cursor(row, column);
            }
            'A' => self.cursor_up(count_param(params, 0)),
            'B' | 'e' => self.cursor_down(count_param(params, 0)),
            'C' | 'a' => self.cursor_forward(count_param(params, 0)),
            'D' => self.cursor_back(count_param(params, 0)),
            'E' => self.cursor_next_line(count_param(params, 0)),
            'F' => self.cursor_prev_line(count_param(params, 0)),
            'G' | '`' => self.set_cursor_column(pos_param(params, 0)),
            'd' => self.set_cursor_row(pos_param(params, 0)),
            _ => {}
        }
    }

    fn dispatch_edit_csi(&mut self, params: &Params, action: char) {
        let pen = self.pen;
        self.dirty = true;
        match action {
            'J' => self
                .active_grid_mut()
                .erase_in_display(mode_param(params, 0), pen),
            'K' => self
                .active_grid_mut()
                .erase_in_line(mode_param(params, 0), pen),
            'L' => {
                let (top, bottom) = (self.scroll_top, self.scroll_bottom);
                self.active_grid_mut()
                    .insert_lines(count_param(params, 0), top, bottom, pen);
            }
            'M' => {
                let (top, bottom) = (self.scroll_top, self.scroll_bottom);
                self.active_grid_mut()
                    .delete_lines(count_param(params, 0), top, bottom, pen);
            }
            '@' => self
                .active_grid_mut()
                .insert_chars(count_param(params, 0), pen),
            'P' => self
                .active_grid_mut()
                .delete_chars(count_param(params, 0), pen),
            'X' => self
                .active_grid_mut()
                .erase_chars(count_param(params, 0), pen),
            _ => {}
        }
    }

    fn dispatch_scroll_csi(&mut self, params: &Params, action: char) {
        let pen = self.pen;
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        self.dirty = true;
        match action {
            'S' => {
                let capture = !self.using_alternate;
                self.active_grid_mut().scroll_up_region(
                    count_param(params, 0),
                    top,
                    bottom,
                    capture,
                    pen,
                );
            }
            'T' => {
                self.active_grid_mut()
                    .scroll_down_region(count_param(params, 0), top, bottom, pen)
            }
            _ => {}
        }
    }
}

impl Perform for TerminalState {
    fn print(&mut self, character: char) {
        self.dirty = true;
        if self.wrap_pending {
            self.wrap_pending = false;
            self.carriage_return();
            self.line_feed();
        }
        let pen = self.pen;
        let at_last = {
            let grid = self.active_grid_mut();
            grid.put_styled(character, pen);
            let at_last = grid.cursor_at_last_column();
            if !at_last {
                grid.move_cursor_right();
            }
            at_last
        };
        self.wrap_pending = at_last;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => {
                self.wrap_pending = false;
                self.dirty = true;
                self.active_grid_mut().backspace();
            }
            0x09 => {
                self.wrap_pending = false;
                self.dirty = true;
                let pen = self.pen;
                self.active_grid_mut().tab(pen);
            }
            0x0a..=0x0c => {
                self.wrap_pending = false;
                self.line_feed();
            }
            0x0d => self.carriage_return(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        let private = intermediates.first() == Some(&b'?');

        match action {
            'm' => self.apply_sgr(params),
            'H' | 'f' | 'A' | 'B' | 'e' | 'C' | 'a' | 'D' | 'E' | 'F' | 'G' | '`' | 'd' => {
                self.dispatch_cursor_csi(params, action)
            }
            'J' | 'K' | 'L' | 'M' | '@' | 'P' | 'X' => self.dispatch_edit_csi(params, action),
            'S' | 'T' => self.dispatch_scroll_csi(params, action),
            'r' => self.set_scroll_region(params),
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            'h' => self.set_mode(params, private, true),
            'l' => self.set_mode(params, private, false),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore || !intermediates.is_empty() {
            // Charset designations (`ESC ( B`, etc.) carry intermediates and are
            // intentionally ignored.
            return;
        }
        match byte {
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.line_feed(),
            b'E' => {
                self.carriage_return();
                self.line_feed();
            }
            b'M' => self.reverse_index(),
            b'c' => self.reset(),
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
}

impl TerminalState {
    fn set_scroll_region(&mut self, params: &Params) {
        let rows = self.active_grid().size().rows();
        let top = pos_param(params, 0);
        let bottom = nth_raw(params, 1)
            .filter(|value| *value != 0)
            .map(|value| value - 1)
            .unwrap_or(rows - 1);
        if top < bottom && bottom < rows {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = rows - 1;
        }
        self.set_cursor(self.scroll_top, 0);
    }
}

/// Parse an ISO 8613-6 colon-form extended color from one parameter group whose
/// subparameters are `[38|48, mode, ...]`. Handles `38:5:n`, `38:2:r:g:b`
/// (no colorspace), and `38:2:cs:r:g:b` (with a colorspace placeholder).
fn parse_colon_color(group: &[u16]) -> Option<Color> {
    match group.get(1).copied()? {
        5 => group.get(2).map(|&index| Color::Indexed(index as u8)),
        2 => {
            // 6+ subparameters include a colorspace id at index 2 to skip; a
            // 5-subparameter group puts r/g/b directly after the mode.
            let base = if group.len() >= 6 { 3 } else { 2 };
            match (group.get(base), group.get(base + 1), group.get(base + 2)) {
                (Some(&r), Some(&g), Some(&b)) => Some(Color::Rgb(r as u8, g as u8, b as u8)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn nth_raw(params: &Params, index: usize) -> Option<u16> {
    params
        .iter()
        .nth(index)
        .and_then(|param| param.first().copied())
}

/// Count-style parameter: missing or zero means 1 (e.g. cursor-move counts).
fn count_param(params: &Params, index: usize) -> u16 {
    nth_raw(params, index)
        .filter(|value| *value != 0)
        .unwrap_or(1)
}

/// 1-based position parameter converted to a 0-based row/column.
fn pos_param(params: &Params, index: usize) -> u16 {
    count_param(params, index) - 1
}

/// Mode-style parameter: missing means 0 (e.g. erase modes).
fn mode_param(params: &Params, index: usize) -> u16 {
    nth_raw(params, index).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GridPosition, TerminalAdapter};

    fn adapter(columns: u16, rows: u16) -> VteTerminalAdapter {
        VteTerminalAdapter::new(TerminalSize::new(columns, rows).unwrap())
    }

    fn trimmed(adapter: &VteTerminalAdapter) -> Vec<String> {
        adapter
            .grid()
            .snapshot()
            .into_iter()
            .map(|row| row.trim_end().to_owned())
            .collect()
    }

    #[test]
    fn plain_text_and_crlf_render_without_escapes() {
        let mut adapter = adapter(20, 3);
        adapter.feed(b"cargo test\r\nok").unwrap();
        assert_eq!(trimmed(&adapter), vec!["cargo test", "ok", ""]);
    }

    #[test]
    fn sgr_color_does_not_leak_as_text_and_sets_style() {
        let mut adapter = adapter(10, 1);
        adapter.feed(b"\x1b[31;1mRED\x1b[0m").unwrap();
        assert_eq!(trimmed(&adapter), vec!["RED"]);
        let cell = adapter.grid().cell(GridPosition::new(0, 0)).unwrap();
        assert_eq!(cell.style().foreground, Color::Indexed(1));
        assert!(cell.style().bold);
    }

    #[test]
    fn truecolor_sgr_is_parsed() {
        let mut adapter = adapter(4, 1);
        adapter.feed(b"\x1b[38;2;10;20;30mX").unwrap();
        let cell = adapter.grid().cell(GridPosition::new(0, 0)).unwrap();
        assert_eq!(cell.style().foreground, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn colon_form_extended_colors_parse_without_pen_corruption() {
        // Colon RGB with an empty colorspace placeholder (`38:2::r:g:b`).
        let mut rgb_placeholder = adapter(4, 1);
        rgb_placeholder.feed(b"\x1b[38:2::10:20:30mX").unwrap();
        let style = rgb_placeholder
            .grid()
            .cell(GridPosition::new(0, 0))
            .unwrap()
            .style();
        assert_eq!(style.foreground, Color::Rgb(10, 20, 30));
        // No spurious SGR code leaked into the pen.
        assert_eq!(style.background, Color::Default);
        assert!(!style.bold && !style.italic && !style.underline && !style.inverse);

        // Colon RGB without a colorspace field (`48:2:r:g:b`).
        let mut rgb_bare = adapter(4, 1);
        rgb_bare.feed(b"\x1b[48:2:1:2:3mY").unwrap();
        assert_eq!(
            rgb_bare
                .grid()
                .cell(GridPosition::new(0, 0))
                .unwrap()
                .style()
                .background,
            Color::Rgb(1, 2, 3)
        );

        // Colon indexed (`38:5:n`).
        let mut indexed = adapter(4, 1);
        indexed.feed(b"\x1b[38:5:42mZ").unwrap();
        assert_eq!(
            indexed
                .grid()
                .cell(GridPosition::new(0, 0))
                .unwrap()
                .style()
                .foreground,
            Color::Indexed(42)
        );
    }

    #[test]
    fn tab_at_extreme_width_does_not_panic() {
        // A near-maximum width grid must not overflow the tab-stop computation.
        let mut adapter = VteTerminalAdapter::new(TerminalSize::new(65535, 1).unwrap());
        adapter.feed(b"\x1b[1;65535H\tX").unwrap();
        // Reaching here without a panic is the assertion; the grid stays valid.
        assert_eq!(adapter.grid().size().columns(), 65535);
    }

    #[test]
    fn cursor_addressing_and_erase_display_clears_screen() {
        let mut adapter = adapter(10, 3);
        adapter.feed(b"line1\r\nline2\r\nline3").unwrap();
        // Home, then erase entire display.
        adapter.feed(b"\x1b[H\x1b[2J").unwrap();
        assert_eq!(trimmed(&adapter), vec!["", "", ""]);
        assert_eq!(adapter.grid().cursor().position(), GridPosition::new(0, 0));
    }

    #[test]
    fn erase_saved_lines_preserves_visible_screen() {
        let mut adapter = adapter(8, 2);
        adapter.feed(b"one\r\ntwo\r\nthree").unwrap();
        assert!(adapter.grid().scrollback_len() > 0);

        adapter.feed(b"\x1b[3J").unwrap();

        assert_eq!(adapter.grid().scrollback_len(), 0);
        assert!(
            trimmed(&adapter).iter().any(|line| line.contains("three")),
            "ED3 must not blank the active screen"
        );
    }

    #[test]
    fn carriage_return_redraw_overwrites_line() {
        let mut adapter = adapter(12, 1);
        adapter.feed(b"progress 10%\rprogress 99%").unwrap();
        assert_eq!(trimmed(&adapter), vec!["progress 99%"]);
    }

    #[test]
    fn scrolling_captures_bounded_scrollback() {
        let mut adapter =
            VteTerminalAdapter::with_scrollback_limit(TerminalSize::new(8, 2).unwrap(), 10);
        adapter.feed(b"one\r\ntwo\r\nthree\r\nfour").unwrap();
        // Two visible rows; "one" and "two" scrolled into history.
        assert_eq!(trimmed(&adapter), vec!["three", "four"]);
        assert_eq!(adapter.grid().scrollback_len(), 2);
        assert_eq!(
            adapter
                .grid()
                .scrollback_row_text(0)
                .map(|r| r.trim_end().to_owned()),
            Some("one".to_owned())
        );
    }

    #[test]
    fn scrollback_is_bounded_by_limit() {
        let mut adapter =
            VteTerminalAdapter::with_scrollback_limit(TerminalSize::new(4, 1).unwrap(), 3);
        for _ in 0..20 {
            adapter.feed(b"x\r\n").unwrap();
        }
        assert!(adapter.grid().scrollback_len() <= 3);
    }

    #[test]
    fn alternate_screen_does_not_pollute_primary_or_scrollback() {
        let mut adapter = adapter(8, 2);
        adapter.feed(b"main\r\n").unwrap();
        let scrollback_before = adapter.grid().scrollback_len();
        adapter.feed(b"\x1b[?1049h").unwrap();
        adapter.feed(b"ALT\r\nALT2\r\nALT3").unwrap();
        adapter.feed(b"\x1b[?1049l").unwrap();
        // Back on the primary screen, alt content is gone and scrollback did not
        // grow from alt-screen scrolling.
        assert_eq!(trimmed(&adapter), vec!["main", ""]);
        assert_eq!(adapter.grid().scrollback_len(), scrollback_before);
    }

    #[test]
    fn partial_scroll_regions_do_not_enter_scrollback() {
        let mut adapter = adapter(8, 3);
        adapter.feed(b"top\r\nmid\r\nbot").unwrap();

        adapter.feed(b"\x1b[1;2r\x1b[2;1H\n").unwrap();

        assert_eq!(
            adapter.grid().scrollback_len(),
            0,
            "top-origin partial scroll regions are screen edits, not scrollback"
        );
    }

    #[test]
    fn erase_in_line_to_end_clears_tail() {
        let mut adapter = adapter(10, 1);
        adapter.feed(b"abcdefghij").unwrap();
        adapter.feed(b"\x1b[1G\x1b[3C\x1b[K").unwrap(); // col1, forward 3 -> col4, erase to end
        assert_eq!(trimmed(&adapter), vec!["abc"]);
    }

    #[test]
    fn unsupported_escapes_do_not_render_as_text() {
        let mut adapter = adapter(12, 1);
        // OSC title set + DCS-ish sequence should be swallowed, not printed.
        adapter.feed(b"\x1b]0;my title\x07hi").unwrap();
        assert_eq!(trimmed(&adapter), vec!["hi"]);
    }
}
