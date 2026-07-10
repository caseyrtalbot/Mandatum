//! Pane chrome and content drawing.

use mandatum_scene::{AgentContent, AgentStatus, PaneContent, PaneScene, Theme};
use ratatui::{
    Frame,
    style::Modifier,
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
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

    match &pane.content {
        PaneContent::Terminal(terminal) => {
            frame.render_widget(
                Paragraph::new(surface_lines(terminal, theme)).block(block),
                area,
            );
        }
        PaneContent::Task(task) => {
            let mut lines = pane
                .detail_lines()
                .into_iter()
                .map(Line::from)
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
/// color with a bold header, everything else is plain text.
fn agent_lines<'a>(pane: &PaneScene, agent: &AgentContent, theme: &Theme) -> Vec<Line<'a>> {
    let approval_style = theme_fg(theme.attention);
    let status_style = theme_fg(agent_status_color(&agent.status_role, theme));
    pane.detail_lines()
        .into_iter()
        .map(|text| {
            if text.starts_with("status: ") {
                Line::styled(text, status_style.add_modifier(Modifier::BOLD))
            } else if agent.pending_approval.is_some() && text.starts_with("approval required: ") {
                Line::styled(text, approval_style.add_modifier(Modifier::BOLD))
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
