//! Overlay renderers: palette, context menu, timeline, session map, prompt.
//! Pure scene-to-widget translation; all layout math comes from
//! `mandatum_scene::layout`.

use mandatum_scene::{
    ContextMenuOverlay, PaletteOverlay, PromptOverlay, SessionMapOverlay, Theme, TimelineOverlay,
    layout,
};
use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::{theme_fg, to_rect};

/// Draw the palette overlay: the filter input on the top inner row, the
/// visible slice of entries (matched label chars bold+underlined, greyed
/// entries dimmed, the selection reversed), and the key-hint footer pinned
/// to the bottom inner row. Calm styling: modifiers plus the theme's
/// palette roles, no extra color.
pub(crate) fn render_palette(frame: &mut Frame<'_>, palette: &PaletteOverlay, theme: &Theme) {
    let overlay = to_rect(palette.area);
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Block::default()
            .title(" Command Palette ")
            .borders(Borders::ALL)
            .border_style(theme_fg(theme.palette_border)),
        overlay,
    );

    let inner = layout::pane_inner_rect(palette.area);
    let inner_rect = to_rect(inner);
    if inner_rect.height == 0 || inner_rect.width == 0 {
        return;
    }

    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = Vec::with_capacity(usize::from(inner_rect.height));

    // Filter input line, with a block cursor after the typed text. The
    // empty-input placeholder states the fast-path rule and its escape
    // hatch, because an unlabeled input that runs commands on bare letters
    // would read as a text field and trap the first word typed into it.
    lines.push(input_line(
        &palette.query,
        "letters run their key · shift+letter to search",
        dim,
    ));

    if palette.items.is_empty() {
        lines.push(Line::from(Span::styled(" no matching commands", dim)));
    }
    for index in layout::palette_item_window(inner, palette.items.len(), palette.selected) {
        let item = &palette.items[index];
        let mut spans = vec![Span::raw(" ")];
        for (position, character) in item.label.chars().enumerate() {
            let style = if item.match_indices.contains(&position) {
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default()
            };
            spans.push(Span::styled(character.to_string(), style));
        }
        if let Some(hint) = &item.key_hint {
            spans.push(Span::styled(format!("  {hint}"), dim));
        }
        spans.push(Span::styled(format!("  {}", item.detail), dim));

        let mut line_style = Style::default();
        if !item.enabled {
            line_style = line_style.add_modifier(Modifier::DIM);
        }
        if palette.selected == Some(index) {
            line_style = line_style
                .patch(theme_fg(theme.palette_selection))
                .add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::from(spans).style(line_style));
    }

    render_with_pinned_footer(frame, inner_rect, lines, &palette.footer, dim);
}

/// Draw the execution timeline: the filter input on top, the visible slice
/// of events (glyph, relative time, description; the selection reversed),
/// and the key-hint footer.
pub(crate) fn render_timeline(frame: &mut Frame<'_>, timeline: &TimelineOverlay, theme: &Theme) {
    let overlay = to_rect(timeline.area);
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Block::default()
            .title(" Timeline ")
            .borders(Borders::ALL)
            .border_style(theme_fg(theme.palette_border)),
        overlay,
    );

    let inner = layout::pane_inner_rect(timeline.area);
    let inner_rect = to_rect(inner);
    if inner_rect.height == 0 || inner_rect.width == 0 {
        return;
    }

    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = Vec::with_capacity(usize::from(inner_rect.height));
    lines.push(input_line(
        &timeline.query,
        "type to filter · pane:<id> kind:<family> since:<5m>",
        dim,
    ));

    if timeline.items.is_empty() {
        lines.push(Line::from(Span::styled(" no matching events", dim)));
    }
    for index in layout::palette_item_window(inner, timeline.items.len(), timeline.selected) {
        let item = &timeline.items[index];
        let mut spans = vec![Span::raw(format!(" {} ", item.glyph))];
        spans.push(Span::styled(format!("{:>10}  ", item.when), dim));
        spans.push(Span::raw(item.text.clone()));

        let mut line_style = Style::default();
        if timeline.selected == Some(index) {
            line_style = line_style
                .patch(theme_fg(theme.palette_selection))
                .add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::from(spans).style(line_style));
    }

    render_with_pinned_footer(frame, inner_rect, lines, &timeline.footer, dim);
}

/// Draw the session map: sessions with their panes indented beneath them,
/// each pane carrying its glyph, one-word state, focus marker, and badges.
pub(crate) fn render_session_map(frame: &mut Frame<'_>, map: &SessionMapOverlay, theme: &Theme) {
    let overlay = to_rect(map.area);
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Block::default()
            .title(" Sessions ")
            .borders(Borders::ALL)
            .border_style(theme_fg(theme.palette_border)),
        overlay,
    );

    let inner = layout::pane_inner_rect(map.area);
    let inner_rect = to_rect(inner);
    if inner_rect.height == 0 || inner_rect.width == 0 {
        return;
    }

    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = Vec::with_capacity(usize::from(inner_rect.height));
    for index in layout::session_map_item_window(inner, map.rows.len(), Some(map.selected)) {
        let row = &map.rows[index];
        let marker = if row.focused { "●" } else { " " };
        let indent = "  ".repeat(usize::from(row.depth));
        let mut spans = vec![Span::raw(format!(
            "{marker}{indent}{} {}",
            row.glyph, row.label
        ))];
        if !row.state.is_empty() {
            spans.push(Span::styled(format!("  {}", row.state), dim));
        }
        if !row.badges.is_empty() {
            spans.push(Span::styled(format!("  [{}]", row.badges), dim));
        }

        let mut line_style = Style::default();
        if map.selected == index {
            line_style = line_style
                .patch(theme_fg(theme.palette_selection))
                .add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::from(spans).style(line_style));
    }

    render_with_pinned_footer(frame, inner_rect, lines, &map.footer, dim);
}

/// Draw the one-line text prompt (Set agent objective): a titled box with
/// the editable input and a cursor, plus the key-hint footer.
pub(crate) fn render_prompt(frame: &mut Frame<'_>, prompt: &PromptOverlay, theme: &Theme) {
    let overlay = to_rect(prompt.area);
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Block::default()
            .title(prompt.title.clone())
            .borders(Borders::ALL)
            .border_style(theme_fg(theme.palette_border)),
        overlay,
    );

    let inner = layout::pane_inner_rect(prompt.area);
    let inner_rect = to_rect(inner);
    if inner_rect.height == 0 || inner_rect.width == 0 {
        return;
    }

    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut input = vec![Span::raw("> "), Span::raw(prompt.input.clone())];
    input.push(Span::styled(
        " ",
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    let lines = vec![Line::from(input)];

    render_with_pinned_footer(frame, inner_rect, lines, &prompt.footer, dim);
}

/// Draw the right-click context menu: a calm bordered list, the selected
/// row reversed, each row's key-chord hint right-aligned and dimmed.
pub(crate) fn render_context_menu(frame: &mut Frame<'_>, menu: &ContextMenuOverlay, theme: &Theme) {
    let overlay = to_rect(menu.area);
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme_fg(theme.palette_border)),
        overlay,
    );

    let inner = layout::pane_inner_rect(menu.area);
    let inner_rect = to_rect(inner);
    if inner_rect.height == 0 || inner_rect.width == 0 {
        return;
    }

    let dim = Style::default().add_modifier(Modifier::DIM);
    let width = usize::from(inner_rect.width);
    let mut lines = Vec::with_capacity(usize::from(inner_rect.height));
    for (index, item) in menu
        .items
        .iter()
        .take(usize::from(inner_rect.height))
        .enumerate()
    {
        // " label", padding, then the chord hint ending one cell short of
        // the right edge.
        let label_width = item.label.chars().count() + 1;
        let hint_width = item.chord_hint.chars().count() + 1;
        let padding = width.saturating_sub(label_width + hint_width).max(1);
        let mut spans = vec![Span::raw(format!(" {}", item.label))];
        spans.push(Span::raw(" ".repeat(padding)));
        if !item.chord_hint.is_empty() {
            spans.push(Span::styled(item.chord_hint.clone(), dim));
        }

        let mut line_style = Style::default();
        if menu.selected == index {
            line_style = line_style
                .patch(theme_fg(theme.palette_selection))
                .add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::from(spans).style(line_style));
    }

    frame.render_widget(Paragraph::new(Text::from(lines)), inner_rect);
}

/// The shared "> input" line with a block cursor, or a dim placeholder while
/// empty (the palette input pattern every text-input overlay reuses).
fn input_line(query: &str, placeholder: &'static str, dim: Style) -> Line<'static> {
    let mut input = vec![Span::raw("> ")];
    if query.is_empty() {
        input.push(Span::styled(placeholder, dim));
    } else {
        input.push(Span::raw(query.to_owned()));
        input.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    Line::from(input)
}

/// Truncate/pad `lines` so the footer lands on the bottom inner row.
fn render_with_pinned_footer(
    frame: &mut Frame<'_>,
    inner_rect: ratatui::layout::Rect,
    mut lines: Vec<Line<'_>>,
    footer: &str,
    dim: Style,
) {
    let footer_row = usize::from(inner_rect.height).saturating_sub(1);
    lines.truncate(footer_row.max(1));
    while lines.len() < footer_row {
        lines.push(Line::default());
    }
    if footer_row > 0 {
        lines.push(Line::from(Span::styled(format!(" {footer}"), dim)));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner_rect);
}
