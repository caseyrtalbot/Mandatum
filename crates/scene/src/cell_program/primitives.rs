use super::{CellSelection, Compiler, ProgramCell};
use crate::{CellOccupancy, SceneCellStyle, SceneColor, SceneRect, Theme, WorkspaceScene};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const MAX_GRAPHEME_BYTES: usize = 256;

impl Compiler {
    pub(super) fn paint_header(&mut self, scene: &WorkspaceScene, theme: &Theme) {
        let header = &scene.header;
        let base = style(theme.header, theme.header_background);
        self.paint_rect(header.area, base);
        self.paint_text(header.area, &header.text, base);

        let attention = SceneCellStyle {
            foreground: theme.attention,
            background: theme.header_background,
            bold: true,
            ..SceneCellStyle::default()
        };
        for segment in &header.attention {
            self.paint_rect(segment.rect, attention);
            self.paint_text(segment.rect, &segment.label, attention);
        }
    }

    pub(super) fn paint_status(&mut self, scene: &WorkspaceScene, theme: &Theme) {
        let status = &scene.status;
        let status_style = foreground(theme.status);
        self.paint_rect(status.area, status_style);
        self.paint_text(status.area, &format!(" {}", status.text), status_style);
    }

    pub(super) fn paint_border(&mut self, area: SceneRect, border_style: SceneCellStyle) {
        let visible = self.clipped_rect(area);
        if visible.is_empty() {
            return;
        }

        // Preserve the glyph semantics of the original rectangle: clipping an
        // off-frame right or bottom edge must not invent a visible corner.
        let right = area.right().saturating_sub(1);
        let bottom = area.bottom().saturating_sub(1);

        if area.y < self.program.size.height {
            for x in visible.x..visible.right() {
                let top = if x == area.x {
                    '┌'
                } else if x == right {
                    '┐'
                } else {
                    '─'
                };
                self.paint_cell(x, area.y, ProgramCell::glyph(top, border_style));
            }
        }

        if bottom != area.y && bottom < self.program.size.height {
            for x in visible.x..visible.right() {
                let bottom_glyph = if x == area.x {
                    '└'
                } else if x == right {
                    '┘'
                } else {
                    '─'
                };
                self.paint_cell(x, bottom, ProgramCell::glyph(bottom_glyph, border_style));
            }
        }

        let vertical_start = visible.y.max(area.y.saturating_add(1));
        let vertical_end = visible.bottom().min(bottom);
        for y in vertical_start..vertical_end {
            self.paint_cell(area.x, y, ProgramCell::glyph('│', border_style));
            if right != area.x && right < self.program.size.width {
                self.paint_cell(right, y, ProgramCell::glyph('│', border_style));
            }
        }
    }

    pub(super) fn paint_rect(&mut self, area: SceneRect, cell_style: SceneCellStyle) {
        let area = self.clipped_rect(area);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                self.paint_cell(x, y, ProgramCell::glyph(' ', cell_style));
            }
        }
    }

    pub(super) fn paint_text(&mut self, area: SceneRect, text: &str, cell_style: SceneCellStyle) {
        let area = self.clipped_rect(area);
        if area.is_empty() {
            return;
        }
        let mut column = 0u16;
        for grapheme in text.graphemes(true) {
            let (grapheme, width) = bounded_grapheme(grapheme);
            if width > usize::from(area.width.saturating_sub(column)) {
                break;
            }
            self.paint_grapheme(
                area.x.saturating_add(column),
                area.y,
                grapheme,
                width as u8,
                cell_style,
                None,
                false,
                None,
            );
            column = column.saturating_add(width as u16);
        }
    }

    pub(super) fn paint_text_row(
        &mut self,
        area: SceneRect,
        row: usize,
        text: &str,
        cell_style: SceneCellStyle,
    ) {
        if row >= usize::from(area.height) {
            return;
        }
        self.paint_text(
            SceneRect::new(area.x, area.y.saturating_add(row as u16), area.width, 1),
            text,
            cell_style,
        );
    }

    pub(super) fn paint_text_row_marked(
        &mut self,
        area: SceneRect,
        row: usize,
        text: &str,
        cell_style: SceneCellStyle,
        selected: bool,
    ) {
        if row >= usize::from(area.height) {
            return;
        }
        let y = area.y.saturating_add(row as u16);
        let visible = self.clipped_rect(SceneRect::new(area.x, y, area.width, 1));
        let mut column = 0u16;
        for grapheme in text.graphemes(true) {
            let (grapheme, width) = bounded_grapheme(grapheme);
            if width > usize::from(visible.width.saturating_sub(column)) {
                break;
            }
            self.paint_grapheme(
                visible.x.saturating_add(column),
                y,
                grapheme,
                width as u8,
                cell_style,
                selected.then_some(CellSelection::Item),
                false,
                None,
            );
            column = column.saturating_add(width as u16);
        }
    }

    fn clipped_rect(&self, area: SceneRect) -> SceneRect {
        let right = area.right().min(self.program.size.width);
        let bottom = area.bottom().min(self.program.size.height);
        if area.x >= right || area.y >= bottom {
            return SceneRect::new(
                area.x.min(self.program.size.width),
                area.y.min(self.program.size.height),
                0,
                0,
            );
        }
        SceneRect::new(area.x, area.y, right - area.x, bottom - area.y)
    }

    pub(super) fn paint_cell(&mut self, x: u16, y: u16, cell: ProgramCell) {
        if x >= self.program.size.width || y >= self.program.size.height {
            return;
        }
        self.remove_cell_span(x, y);
        self.program.cells.insert((y, x), cell);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_grapheme(
        &mut self,
        x: u16,
        y: u16,
        grapheme: String,
        width: u8,
        style: SceneCellStyle,
        selection: Option<CellSelection>,
        cursor: bool,
        raster_layer: Option<u16>,
    ) {
        if x >= self.program.size.width || y >= self.program.size.height {
            return;
        }
        let width = width.clamp(1, 2);
        if width == 2 && x.saturating_add(1) >= self.program.size.width {
            self.paint_cell(
                x,
                y,
                ProgramCell {
                    occupancy: CellOccupancy::Grapheme("\u{fffd}".to_owned()),
                    style,
                    selection,
                    cursor,
                    raster_layer,
                },
            );
            return;
        }

        self.remove_cell_span(x, y);
        if width == 2 {
            self.remove_cell_span(x + 1, y);
        }
        self.program.cells.insert(
            (y, x),
            ProgramCell {
                occupancy: CellOccupancy::Grapheme(grapheme),
                style,
                selection,
                cursor,
                raster_layer,
            },
        );
        if width == 2 {
            self.program.cells.insert(
                (y, x + 1),
                ProgramCell {
                    occupancy: CellOccupancy::WideContinuation,
                    style,
                    selection,
                    cursor,
                    raster_layer,
                },
            );
        }
    }

    fn remove_cell_span(&mut self, x: u16, y: u16) {
        let Some(existing) = self.program.cells.get(&(y, x)) else {
            return;
        };
        match existing.occupancy {
            CellOccupancy::WideContinuation => {
                if let Some(lead) = x.checked_sub(1) {
                    self.program.cells.remove(&(y, lead));
                }
            }
            CellOccupancy::Grapheme(_) => {
                if self
                    .program
                    .cells
                    .get(&(y, x.saturating_add(1)))
                    .is_some_and(|cell| matches!(cell.occupancy, CellOccupancy::WideContinuation))
                {
                    self.program.cells.remove(&(y, x.saturating_add(1)));
                }
            }
        }
        self.program.cells.remove(&(y, x));
    }

    pub(super) fn mark_cursor(&mut self, x: u16, y: u16, style: SceneCellStyle) {
        if x >= self.program.size.width || y >= self.program.size.height {
            return;
        }
        let lead = if self
            .program
            .cells
            .get(&(y, x))
            .is_some_and(|cell| matches!(cell.occupancy, CellOccupancy::WideContinuation))
        {
            x.saturating_sub(1)
        } else {
            x
        };
        let wide = self
            .program
            .cells
            .get(&(y, lead.saturating_add(1)))
            .is_some_and(|cell| matches!(cell.occupancy, CellOccupancy::WideContinuation));
        if let Some(cell) = self.program.cells.get_mut(&(y, lead)) {
            cell.cursor = true;
            cell.style = style;
        } else {
            let mut cursor = ProgramCell::glyph(' ', style);
            cursor.cursor = true;
            self.program.cells.insert((y, lead), cursor);
        }
        if wide && let Some(cell) = self.program.cells.get_mut(&(y, lead + 1)) {
            cell.cursor = true;
            cell.style = style;
        }
    }
}

/// The true content area inside a one-cell border.
///
/// The general layout helper intentionally preserves a minimum PTY size, but
/// the compiler must not paint content through borders when a pane is only one
/// or two cells wide or tall.
pub(super) fn bordered_inner_rect(area: SceneRect) -> SceneRect {
    SceneRect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

pub(super) fn foreground(color: SceneColor) -> SceneCellStyle {
    SceneCellStyle {
        foreground: color,
        ..SceneCellStyle::default()
    }
}

fn style(foreground: SceneColor, background: SceneColor) -> SceneCellStyle {
    SceneCellStyle {
        foreground,
        background,
        ..SceneCellStyle::default()
    }
}

pub fn display_width(text: &str) -> usize {
    text.graphemes(true)
        .map(|grapheme| bounded_grapheme(grapheme).1)
        .sum()
}

/// Convert a Unicode-scalar range into the complete display-column span of
/// every grapheme it touches.
pub fn scalar_range_to_columns(
    text: &str,
    scalar_start: usize,
    scalar_end: usize,
) -> (usize, usize) {
    if scalar_start >= scalar_end {
        let prefix = text.chars().take(scalar_start).collect::<String>();
        let column = display_width(&prefix);
        return (column, column);
    }

    let mut scalar = 0usize;
    let mut column = 0usize;
    let mut start_column = None;
    let mut end_column = None;
    for grapheme in text.graphemes(true) {
        let scalar_len = grapheme.chars().count();
        let next_scalar = scalar.saturating_add(scalar_len);
        let width = bounded_grapheme(grapheme).1;
        let next_column = column.saturating_add(width);
        if scalar_start < next_scalar && scalar_end > scalar {
            start_column.get_or_insert(column);
            end_column = Some(next_column);
        }
        scalar = next_scalar;
        column = next_column;
    }
    let end = end_column.unwrap_or(column);
    (start_column.unwrap_or(end), end)
}

pub(super) fn bounded_grapheme(grapheme: &str) -> (String, usize) {
    if grapheme.is_empty()
        || grapheme.len() > MAX_GRAPHEME_BYTES
        || grapheme.graphemes(true).count() != 1
    {
        return ("\u{fffd}".to_owned(), 1);
    }
    let width = UnicodeWidthStr::width(grapheme);
    if width == 0 {
        (format!("\u{25cc}{grapheme}"), 1)
    } else if width > 2 {
        ("\u{fffd}".to_owned(), 1)
    } else {
        (grapheme.to_owned(), width)
    }
}

#[cfg(test)]
mod tests {
    use super::scalar_range_to_columns;

    #[test]
    fn scalar_ranges_snap_to_complete_grapheme_columns() {
        let text = "e\u{301}界X";
        assert_eq!(scalar_range_to_columns(text, 1, 2), (0, 1));
        assert_eq!(scalar_range_to_columns(text, 2, 3), (1, 3));
        assert_eq!(scalar_range_to_columns(text, 3, 4), (3, 4));
    }
}
