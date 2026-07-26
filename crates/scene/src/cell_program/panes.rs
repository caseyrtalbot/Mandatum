use super::TextPaintScopeKind;
use super::{CellOccupancy, CellSelection, Compiler, ProgramCell};
use crate::{
    ArtifactState, PaneBadgeKind, PaneContent, PaneScene, PresentationTone, SceneCellStyle,
    SceneColor, SceneRect, TerminalSurface, Theme, WorkflowNodePart, WorkflowRow, WorkflowRowRole,
};

use super::primitives::{bordered_inner_rect, bounded_grapheme, display_width, foreground};
use unicode_segmentation::UnicodeSegmentation;

impl Compiler {
    pub(super) fn paint_pane(
        &mut self,
        pane: &PaneScene,
        theme: &Theme,
        raster_layer: Option<u16>,
    ) {
        let chrome_scope = self.begin_text_scope(TextPaintScopeKind::PaneChrome, pane.area);
        // Every pane is opaque in scene order, regardless of layout flags.
        self.paint_rect(pane.area, SceneCellStyle::default());
        let decoration_scope = self.begin_text_scope(TextPaintScopeKind::PaneDecoration, pane.area);
        self.paint_border(pane.area, foreground(theme.pane_border));
        self.set_text_scope(chrome_scope);

        let title_style = if pane.focused {
            SceneCellStyle {
                foreground: theme.focus_title,
                bold: true,
                ..SceneCellStyle::default()
            }
        } else {
            foreground(theme.pane_title)
        };
        let title_area = SceneRect::new(
            pane.area.x.saturating_add(1),
            pane.area.y,
            pane.area.width.saturating_sub(2),
            pane.area.height.min(1),
        );
        let base_title = format!(" {} ", pane.title);
        self.paint_text(title_area, &base_title, title_style);
        let base_width = u16::try_from(display_width(&base_title)).unwrap_or(u16::MAX);
        // First rail cell past the title's painted extent. Cells before it
        // belong to the title in both frontends.
        let title_end = title_area
            .x
            .saturating_add(base_width.min(title_area.width));
        if pane.focused {
            // The literal focus label is terminal-fallback chrome. Painting
            // the same cells under the decoration scope lets native renderers
            // drop the text while keeping cell content byte-identical; native
            // focus is already cued by the focus bar and bold accent title.
            let suffix_area = SceneRect::new(
                title_area
                    .x
                    .saturating_add(base_width.min(title_area.width)),
                title_area.y,
                title_area.width.saturating_sub(base_width),
                title_area.height,
            );
            self.set_text_scope(decoration_scope);
            self.paint_text(suffix_area, "| focused ", title_style);
            self.set_text_scope(chrome_scope);
        }
        for (kind, rect) in pane.badge_rects() {
            // The terminal kind label is terminal-fallback chrome: native
            // renderers suppress its redundant chip, so painting the same
            // cells under the decoration scope drops the glyph text natively
            // while keeping cell content byte-identical. Non-terminal kind
            // and state badges stay chrome-scoped; they carry information
            // and keep their native chips.
            if kind == PaneBadgeKind::Terminal {
                // The title owns its painted extent in both frontends: a
                // natively-dropped badge overwriting occupied title cells
                // would truncate the fallback title while native showed a
                // blank gap. The badge paints only when it sits entirely
                // beyond the title's end and drops when the title needs the
                // space.
                if rect.x < title_end {
                    continue;
                }
                self.set_text_scope(decoration_scope);
                self.paint_text(rect, &format!(" {} ", kind.label()), title_style);
                self.set_text_scope(chrome_scope);
            } else {
                self.paint_text(rect, &format!(" {} ", kind.label()), title_style);
            }
        }

        let inner = bordered_inner_rect(pane.area);
        self.begin_text_scope(TextPaintScopeKind::PaneContent, inner);
        match &pane.content {
            PaneContent::Terminal(surface) => self.paint_surface(inner, 0, surface),
            PaneContent::Task(task) => {
                let rows = pane.workflow_rows();
                for (row, workflow) in rows.iter().enumerate() {
                    self.paint_text_row(
                        inner,
                        row,
                        &fit_line(&workflow.text, inner.width),
                        workflow_row_style(workflow, theme, false),
                    );
                }
                if let Some(output) = &task.output {
                    self.paint_surface(inner, rows.len(), output);
                }
            }
            PaneContent::Agent(agent) => {
                let rows = pane.workflow_rows();
                let lines = rows
                    .iter()
                    .enumerate()
                    .map(|(index, row)| {
                        let first_approval_row = row.part == WorkflowNodePart::Approval
                            && index
                                .checked_sub(1)
                                .and_then(|previous| rows.get(previous))
                                .is_none_or(|previous| previous.part != WorkflowNodePart::Approval);
                        let pulse = first_approval_row
                            && agent
                                .pending_approval
                                .as_ref()
                                .is_some_and(|prompt| prompt.pulse_on);
                        let style = workflow_row_style(row, theme, pulse);
                        (row.text.clone(), style)
                    })
                    .collect::<Vec<_>>();
                for (row, (text, style)) in lines.iter().enumerate() {
                    self.paint_text_row(inner, row, &fit_line(text, inner.width), *style);
                }
            }
            PaneContent::Artifact(artifact) => {
                let rows = pane.workflow_rows();
                for (row, workflow) in rows.iter().enumerate() {
                    self.paint_text_row(
                        inner,
                        row,
                        &fit_line(&workflow.text, inner.width),
                        workflow_row_style(workflow, theme, false),
                    );
                }
                if matches!(artifact.state, ArtifactState::Ready(_))
                    && let Some(raster_layer) = raster_layer
                {
                    self.paint_raster_body(inner, rows.len(), raster_layer);
                }
            }
            PaneContent::Empty(_) => {
                let lines = pane
                    .detail_lines()
                    .into_iter()
                    .map(|text| (text, SceneCellStyle::default()))
                    .collect::<Vec<_>>();
                self.paint_wrapped_lines(inner, &lines);
            }
        }

        if pane.focused && pane.area.width < 4 && !pane.area.is_empty() {
            self.set_text_scope(decoration_scope);
            self.paint_cell(
                pane.area.x,
                pane.area.y,
                ProgramCell::glyph('●', title_style),
            );
        }
    }

    fn paint_wrapped_lines(&mut self, area: SceneRect, lines: &[(String, SceneCellStyle)]) {
        let mut row = 0usize;
        for (text, line_style) in lines {
            for wrapped in wrap_line(text, area.width) {
                if row >= usize::from(area.height) {
                    return;
                }
                self.paint_text_row(area, row, &wrapped, *line_style);
                row += 1;
            }
        }
    }

    fn paint_surface(&mut self, area: SceneRect, row_offset: usize, surface: &TerminalSurface) {
        for (line, row) in surface.rows.iter().enumerate() {
            let target_row = row_offset.saturating_add(line);
            if target_row >= usize::from(area.height) {
                break;
            }
            let absolute_row = surface.first_row.saturating_add(line);
            for (column, cell) in row.iter().take(usize::from(area.width)).enumerate() {
                let Some(x) = area.x.checked_add(column as u16) else {
                    continue;
                };
                let Some(y) = area.y.checked_add(target_row as u16) else {
                    continue;
                };
                let selected_here = surface.selection_contains(absolute_row, column as u16);
                let cursor_here = surface.cursor_at(absolute_row, column as u16);
                let mut scratch = [0u8; 4];
                match cell.occupancy.grapheme_str(&mut scratch) {
                    Some(source_grapheme) => {
                        let declared_wide = row.get(column + 1).is_some_and(|next| {
                            matches!(next.occupancy, CellOccupancy::WideContinuation)
                        });
                        let (mut grapheme, measured_width) = bounded_grapheme(source_grapheme);
                        let wide = measured_width == 2
                            && declared_wide
                            && column + 1 < usize::from(area.width);
                        if measured_width == 2 && !wide {
                            grapheme = "\u{fffd}".to_owned();
                        }
                        let selected = selected_here
                            || (wide
                                && surface.selection_contains(absolute_row, column as u16 + 1));
                        let cursor = cursor_here
                            || (wide && surface.cursor_at(absolute_row, column as u16 + 1));
                        self.paint_grapheme(
                            x,
                            y,
                            grapheme,
                            if wide { 2 } else { 1 },
                            cell.style,
                            selected.then_some(CellSelection::Terminal),
                            cursor,
                            None,
                        );
                    }
                    None => {
                        // A wide continuation: the leading cell paints this
                        // pair atomically.
                    }
                }
            }
        }
    }

    fn paint_raster_body(&mut self, area: SceneRect, row_offset: usize, raster_layer: u16) {
        let start = area
            .y
            .saturating_add(u16::try_from(row_offset).unwrap_or(u16::MAX));
        for y in start..area.bottom() {
            for x in area.x..area.right() {
                let mut cell = ProgramCell::glyph(' ', SceneCellStyle::default());
                cell.raster_layer = Some(raster_layer);
                self.paint_cell(x, y, cell);
            }
        }
    }
}

fn workflow_row_style(row: &WorkflowRow, theme: &Theme, pulse: bool) -> SceneCellStyle {
    let foreground = match row.tone {
        PresentationTone::Neutral => SceneColor::Default,
        PresentationTone::Focus => theme.focus_title,
        PresentationTone::Running => theme.agent_running,
        PresentationTone::Waiting => theme.agent_waiting,
        PresentationTone::Failure => theme.agent_failed,
        PresentationTone::Complete => theme.agent_complete,
        PresentationTone::AgentIdentity => theme.pane_title,
    };
    SceneCellStyle {
        foreground,
        bold: pulse
            || matches!(row.role, WorkflowRowRole::Heading | WorkflowRowRole::Status)
            || row.part == WorkflowNodePart::StatusText
            || (row.role == WorkflowRowRole::Callout && row.tone == PresentationTone::Failure),
        ..SceneCellStyle::default()
    }
}

fn fit_line(text: &str, width: u16) -> String {
    let width = usize::from(width);
    if display_width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let graphemes = text
        .graphemes(true)
        .map(bounded_grapheme)
        .collect::<Vec<_>>();
    let tail_width = (width - 1) / 2;
    let head_width = width - 1 - tail_width;
    let mut fitted = String::new();
    let mut used = 0usize;
    for (grapheme, grapheme_width) in &graphemes {
        if used.saturating_add(*grapheme_width) > head_width {
            break;
        }
        fitted.push_str(grapheme);
        used += grapheme_width;
    }
    fitted.push('…');
    let mut tail = Vec::new();
    let mut used = 0usize;
    for (grapheme, grapheme_width) in graphemes.iter().rev() {
        if used.saturating_add(*grapheme_width) > tail_width {
            break;
        }
        tail.push(grapheme);
        used += grapheme_width;
    }
    for grapheme in tail.into_iter().rev() {
        fitted.push_str(grapheme);
    }
    fitted
}

fn wrap_line(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    if display_width(text) <= width {
        return vec![text.to_owned()];
    }

    let mut rows = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let separator = usize::from(!current.is_empty());
        if display_width(&current) + separator + display_width(word) <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            continue;
        }
        if !current.is_empty() {
            rows.push(std::mem::take(&mut current));
        }
        let segments = wrap_word(word, width);
        let last = segments.len().saturating_sub(1);
        for (index, segment) in segments.into_iter().enumerate() {
            if index < last {
                rows.push(segment);
            } else {
                current = segment;
            }
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

fn wrap_word(word: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for grapheme in word.graphemes(true) {
        let (mut grapheme, mut grapheme_width) = bounded_grapheme(grapheme);
        if grapheme_width > width {
            grapheme = "\u{fffd}".to_owned();
            grapheme_width = 1;
        }
        if current_width > 0 && current_width.saturating_add(grapheme_width) > width {
            rows.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push_str(&grapheme);
        current_width += grapheme_width;
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{fit_line, wrap_line};
    use crate::cell_program::display_width;

    #[test]
    fn fitting_and_wrapping_never_split_graphemes_or_exceed_columns() {
        let fitted = fit_line("ab界e\u{301}👩\u{200d}💻xyz", 7);
        assert!(display_width(&fitted) <= 7);
        assert!(!fitted.starts_with('\u{301}'));

        let rows = wrap_line("界界界 e\u{301}e\u{301} 👩\u{200d}💻x", 3);
        assert!(rows.iter().all(|row| display_width(row) <= 3));
        assert!(rows.iter().any(|row| row.contains("e\u{301}")));
        assert!(rows.iter().any(|row| row.contains("👩\u{200d}💻")));
    }
}
