//! Pane chrome and content drawing.

use mandatum_scene::{AgentContent, PaneContent, PaneScene};
use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{surface::surface_lines, to_rect};

pub(crate) fn render_pane(frame: &mut Frame<'_>, pane: &PaneScene) {
    let border_style = if pane.focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(pane_title(pane));
    let area = to_rect(pane.area);

    match &pane.content {
        PaneContent::Terminal(terminal) => {
            frame.render_widget(Paragraph::new(surface_lines(terminal)).block(block), area);
        }
        PaneContent::Task(task) => {
            let mut lines = pane
                .detail_lines()
                .into_iter()
                .map(Line::from)
                .collect::<Vec<_>>();
            if let Some(output) = &task.output {
                lines.extend(surface_lines(output));
            }
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        PaneContent::Agent(agent) => {
            let lines = agent_lines(pane, agent);
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
/// bold, the approval block is yellow with a bold header, everything else is
/// plain text.
fn agent_lines<'a>(pane: &PaneScene, agent: &AgentContent) -> Vec<Line<'a>> {
    let approval_style = Style::default().fg(Color::Yellow);
    pane.detail_lines()
        .into_iter()
        .map(|text| {
            if text.starts_with("status: ") {
                Line::styled(text, Style::default().add_modifier(Modifier::BOLD))
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
