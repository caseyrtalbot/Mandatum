//! Terminal surface drawing: neutral scene cells to ratatui spans.

use mandatum_scene::{SceneCellStyle, SceneColor, TerminalSurface};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Paint a surface's visible rows, overlaying selection and cursor marks as
/// reversed cells.
pub(crate) fn surface_lines(surface: &TerminalSurface) -> Vec<Line<'static>> {
    surface
        .rows
        .iter()
        .enumerate()
        .map(|(line, row)| {
            let absolute_row = surface.first_row + line;
            let spans = row
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    let column = column as u16;
                    let mut style = cell_style(cell.style);

                    if surface.selection_contains(absolute_row, column) {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    if surface.cursor_at(absolute_row, column) {
                        style = style.add_modifier(Modifier::REVERSED);
                    }

                    Span::styled(cell.character.to_string(), style)
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

fn cell_style(style: SceneCellStyle) -> Style {
    let mut cell_style = Style::default();
    if style.foreground != SceneColor::Default {
        cell_style = cell_style.fg(map_color(style.foreground));
    }
    if style.background != SceneColor::Default {
        cell_style = cell_style.bg(map_color(style.background));
    }
    if style.bold {
        cell_style = cell_style.add_modifier(Modifier::BOLD);
    }
    if style.dim {
        cell_style = cell_style.add_modifier(Modifier::DIM);
    }
    if style.italic {
        cell_style = cell_style.add_modifier(Modifier::ITALIC);
    }
    if style.underline {
        cell_style = cell_style.add_modifier(Modifier::UNDERLINED);
    }
    if style.inverse {
        cell_style = cell_style.add_modifier(Modifier::REVERSED);
    }
    if style.hidden {
        cell_style = cell_style.add_modifier(Modifier::HIDDEN);
    }
    if style.strikethrough {
        cell_style = cell_style.add_modifier(Modifier::CROSSED_OUT);
    }
    cell_style
}

fn map_color(color: SceneColor) -> Color {
    match color {
        SceneColor::Default => Color::Reset,
        SceneColor::Ansi(index) | SceneColor::Indexed(index) => Color::Indexed(index),
        SceneColor::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}
