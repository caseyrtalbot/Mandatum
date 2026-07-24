use super::{CellOccupancy, CellSelection, Compiler, ProgramCell};
use crate::{
    AgentStatus, ArtifactState, PaneContent, PaneScene, SceneCellStyle, SceneColor, SceneRect,
    TerminalSurface, Theme,
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
        // Every pane is opaque in scene order, regardless of layout flags.
        self.paint_rect(pane.area, SceneCellStyle::default());
        self.paint_border(pane.area, foreground(theme.pane_border));

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
        self.paint_text(title_area, &pane_title(pane), title_style);

        let inner = bordered_inner_rect(pane.area);
        match &pane.content {
            PaneContent::Terminal(surface) => self.paint_surface(inner, 0, surface),
            PaneContent::Task(task) => {
                let lines = pane.detail_lines();
                for (row, text) in lines.iter().enumerate() {
                    let line_style =
                        if text.starts_with("runtime status:") && text.contains("failed") {
                            SceneCellStyle {
                                foreground: theme.attention,
                                bold: true,
                                ..SceneCellStyle::default()
                            }
                        } else {
                            SceneCellStyle::default()
                        };
                    self.paint_text_row(inner, row, &fit_line(text, inner.width), line_style);
                }
                if let Some(output) = &task.output {
                    self.paint_surface(inner, lines.len(), output);
                }
            }
            PaneContent::Agent(agent) => {
                let approval = foreground(theme.attention);
                let approval_header = SceneCellStyle {
                    bold: agent
                        .pending_approval
                        .as_ref()
                        .is_some_and(|prompt| prompt.pulse_on),
                    ..approval
                };
                let status = SceneCellStyle {
                    foreground: agent_status_color(&agent.status_role, theme),
                    bold: true,
                    ..SceneCellStyle::default()
                };
                let lines = pane
                    .detail_lines()
                    .into_iter()
                    .map(|text| {
                        let line_style = if text.starts_with("status: ") {
                            status
                        } else if text.starts_with("error: ") || text.starts_with("relaunch: ") {
                            foreground(theme.agent_failed)
                        } else if agent.pending_approval.is_some()
                            && text.starts_with("approval required: ")
                        {
                            approval_header
                        } else if agent.pending_approval.is_some()
                            && (text.starts_with("scope: ")
                                || text.starts_with("risk: ")
                                || text.starts_with("keys: "))
                        {
                            approval
                        } else {
                            SceneCellStyle::default()
                        };
                        (text, line_style)
                    })
                    .collect::<Vec<_>>();
                self.paint_wrapped_lines(inner, &lines);
            }
            PaneContent::Artifact(artifact) => {
                let lines = pane.detail_lines();
                for (row, text) in lines.iter().enumerate() {
                    let line_style = if matches!(artifact.state, ArtifactState::Failed { .. })
                        && text.starts_with("preview: failed")
                    {
                        foreground(theme.attention)
                    } else {
                        SceneCellStyle::default()
                    };
                    self.paint_text_row(inner, row, &fit_line(text, inner.width), line_style);
                }
                if matches!(artifact.state, ArtifactState::Ready(_))
                    && let Some(raster_layer) = raster_layer
                {
                    self.paint_raster_body(inner, lines.len(), raster_layer);
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
                match &cell.occupancy {
                    CellOccupancy::Grapheme(grapheme) => {
                        let declared_wide = row.get(column + 1).is_some_and(|next| {
                            matches!(next.occupancy, CellOccupancy::WideContinuation)
                        });
                        let (mut grapheme, measured_width) = bounded_grapheme(grapheme);
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
                    CellOccupancy::WideContinuation => {
                        // The leading cell paints this pair atomically.
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

fn agent_status_color(status: &AgentStatus, theme: &Theme) -> SceneColor {
    match status {
        AgentStatus::Running => theme.agent_running,
        AgentStatus::WaitingForApproval => theme.agent_waiting,
        AgentStatus::Failed => theme.agent_failed,
        AgentStatus::Complete => theme.agent_complete,
        AgentStatus::Draft | AgentStatus::Blocked | AgentStatus::Unknown => theme.agent_idle,
    }
}

fn pane_title(pane: &PaneScene) -> String {
    let mut parts = vec![pane.title.clone()];
    if pane.focused {
        parts.push("focused".to_owned());
    }
    if pane.floating {
        parts.push("floating".to_owned());
    }
    if pane.stacked {
        parts.push("stack".to_owned());
    }
    if pane.zoomed {
        parts.push("zoom".to_owned());
    }
    if let PaneContent::Terminal(terminal) = &pane.content
        && terminal.in_copy_mode()
    {
        parts.push("copy".to_owned());
    }
    if let PaneContent::Agent(agent) = &pane.content
        && agent.pending_approval.is_some()
    {
        parts.push("approval".to_owned());
    }
    format!(" {} ", parts.join(" | "))
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
