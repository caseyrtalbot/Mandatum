use super::{CellSelection, Compiler, ProgramCell};
use crate::{SceneCellStyle, SceneColor, SceneRect, Theme, WorkspaceScene};

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
        for (column, character) in text.chars().take(usize::from(area.width)).enumerate() {
            self.paint_cell(
                area.x.saturating_add(column as u16),
                area.y,
                ProgramCell::glyph(character, cell_style),
            );
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
        for (column, character) in text.chars().take(usize::from(visible.width)).enumerate() {
            let mut cell = ProgramCell::glyph(character, cell_style);
            cell.selection = selected.then_some(CellSelection::Item);
            self.paint_cell(visible.x.saturating_add(column as u16), y, cell);
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
        self.program.cells.insert((y, x), cell);
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
