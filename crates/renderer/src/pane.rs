//! Pane chrome and content drawing.

use mandatum_scene::{AgentContent, AgentStatus, PaneContent, PaneScene, Theme};
use ratatui::{
    Frame,
    style::Modifier,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::{surface::surface_lines, theme_fg, to_rect};

pub(crate) fn render_pane(frame: &mut Frame<'_>, pane: &PaneScene, theme: &Theme) {
    let border_style = if pane.focused {
        theme_fg(theme.focus_border).add_modifier(Modifier::BOLD)
    } else {
        theme_fg(theme.pane_border)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(pane_title(pane))
        .title_style(theme_fg(theme.pane_title));
    let area = to_rect(pane.area);

    // A floating pane owns every cell of its rect: without the clear,
    // underlying pane borders and text bleed through rows the float's
    // content does not paint (metadata rows shorter than the rect).
    if pane.floating {
        frame.render_widget(Clear, area);
    }

    match &pane.content {
        PaneContent::Terminal(terminal) => {
            frame.render_widget(
                Paragraph::new(surface_lines(terminal, theme)).block(block),
                area,
            );
        }
        PaneContent::Task(task) => {
            // Detail lines clip at the inner width (no wrap: the output
            // surface below owns the remaining rows), so they truncate with
            // a visible ellipsis that keeps the load-bearing tail — the
            // exit code lives at the end of the status line.
            let inner_width = pane.area.width.saturating_sub(2);
            let failed_style = theme_fg(theme.attention).add_modifier(Modifier::BOLD);
            let mut lines = pane
                .detail_lines()
                .into_iter()
                .map(|text| {
                    let fitted = fit_line(&text, inner_width);
                    if text.starts_with("runtime status:") && text.contains("failed") {
                        Line::styled(fitted, failed_style)
                    } else {
                        Line::from(fitted)
                    }
                })
                .collect::<Vec<_>>();
            if let Some(output) = &task.output {
                lines.extend(surface_lines(output, theme));
            }
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        PaneContent::Agent(agent) => {
            let lines = agent_lines(pane, agent, theme);
            frame.render_widget(
                Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
                area,
            );
        }
        PaneContent::Empty(_) => {
            let lines = pane
                .detail_lines()
                .into_iter()
                .map(Line::from)
                .collect::<Vec<_>>();
            frame.render_widget(
                Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
                area,
            );
        }
    }
}

/// Agent pane lines: the scene's detail lines with the approval block set
/// apart when the agent is waiting. Calm emphasis only — the status line is
/// bold in the agent-status color, the approval block uses the attention
/// color (its header bold only while the scene's ~1 Hz pulse is on — the
/// product's single piece of motion), a failed pane's error row takes the
/// failure color, everything else is plain text.
fn agent_lines<'a>(pane: &PaneScene, agent: &AgentContent, theme: &Theme) -> Vec<Line<'a>> {
    let approval_style = theme_fg(theme.attention);
    let header_style = if agent
        .pending_approval
        .as_ref()
        .is_some_and(|prompt| prompt.pulse_on)
    {
        approval_style.add_modifier(Modifier::BOLD)
    } else {
        approval_style
    };
    let status_style = theme_fg(agent_status_color(&agent.status_role, theme));
    pane.detail_lines()
        .into_iter()
        .map(|text| {
            if text.starts_with("status: ") {
                Line::styled(text, status_style.add_modifier(Modifier::BOLD))
            } else if text.starts_with("error: ") || text.starts_with("relaunch: ") {
                Line::styled(text, theme_fg(theme.agent_failed))
            } else if agent.pending_approval.is_some() && text.starts_with("approval required: ") {
                Line::styled(text, header_style)
            } else if agent.pending_approval.is_some()
                && (text.starts_with("scope: ")
                    || text.starts_with("risk: ")
                    || text.starts_with("keys: "))
            {
                Line::styled(text, approval_style)
            } else {
                Line::from(text)
            }
        })
        .collect()
}

/// Fit one text line into `width` columns. Lines that fit pass through;
/// longer lines truncate around a visible `…` that keeps both the leading
/// label and the trailing tokens — the tail is load-bearing ("failed:
/// exit 3" must never lose the "exit 3").
fn fit_line(text: &str, width: u16) -> String {
    let width = usize::from(width);
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    // Keep roughly the trailing half (the tail carries exit codes and file
    // names); the head keeps whatever the ellipsis leaves.
    let tail_len = (width - 1) / 2;
    let head_len = width - 1 - tail_len;
    let mut fitted: String = characters[..head_len].iter().collect();
    fitted.push('…');
    fitted.extend(&characters[characters.len() - tail_len..]);
    fitted
}

fn agent_status_color(status: &AgentStatus, theme: &Theme) -> mandatum_scene::SceneColor {
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

#[cfg(test)]
mod tests {
    use super::fit_line;

    // Truncation must keep the load-bearing tail: "failed: exit 3" losing
    // its exit code was the stranger-test finding this guards against.
    #[test]
    fn fit_line_truncates_with_ellipsis_and_keeps_exit_codes() {
        let line = "runtime status: failed: exit 3";
        assert_eq!(fit_line(line, 40), line, "fitting lines pass through");

        let fitted = fit_line(line, 24);
        assert_eq!(fitted.chars().count(), 24);
        assert!(fitted.contains('…'), "{fitted:?}");
        assert!(fitted.ends_with("exit 3"), "{fitted:?}");
        assert!(fitted.starts_with("runtime"), "{fitted:?}");

        let fitted = fit_line("command: sh ./very/long/path/flaky-check.sh --verbose", 20);
        assert_eq!(fitted.chars().count(), 20);
        assert!(fitted.ends_with("--verbose"), "{fitted:?}");
        assert!(fitted.starts_with("command:"), "{fitted:?}");

        // Degenerate widths never panic and never lie about content.
        assert_eq!(fit_line(line, 1), "…");
        assert_eq!(fit_line(line, 0), "");
    }
}
