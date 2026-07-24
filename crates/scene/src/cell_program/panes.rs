use super::{CellOccupancy, CellSelection, Compiler, ProgramCell};
use crate::{
    AgentStatus, ArtifactState, PaneContent, PaneScene, SceneCellStyle, SceneColor, SceneRect,
    TerminalSurface, Theme,
};

use super::primitives::{bordered_inner_rect, foreground};

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
                self.paint_cell(
                    x,
                    y,
                    ProgramCell {
                        occupancy: CellOccupancy::Glyph(cell.character),
                        style: cell.style,
                        selection: surface
                            .selection_contains(absolute_row, column as u16)
                            .then_some(CellSelection::Terminal),
                        cursor: surface.cursor_at(absolute_row, column as u16),
                        raster_layer: None,
                    },
                );
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
    let characters = text.chars().collect::<Vec<_>>();
    if characters.len() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let tail_len = (width - 1) / 2;
    let head_len = width - 1 - tail_len;
    let mut fitted = characters[..head_len].iter().collect::<String>();
    fitted.push('…');
    fitted.extend(&characters[characters.len() - tail_len..]);
    fitted
}

fn wrap_line(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    if text.chars().count() <= width {
        return vec![text.to_owned()];
    }

    let mut rows = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let separator = usize::from(!current.is_empty());
        if current.chars().count() + separator + word.chars().count() <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            continue;
        }
        if !current.is_empty() {
            rows.push(std::mem::take(&mut current));
        }
        let mut remaining = word.chars().collect::<Vec<_>>();
        while remaining.len() > width {
            rows.push(remaining.drain(..width).collect());
        }
        current.extend(remaining);
    }
    if !current.is_empty() {
        rows.push(current);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}
