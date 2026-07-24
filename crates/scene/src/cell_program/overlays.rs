use super::{
    CellSelection, Compiler, ProgramCell,
    primitives::{bounded_grapheme, display_width},
};
use crate::{
    ContextMenuOverlay, HelpOverlay, OverlayScene, PaletteOverlay, PromptOverlay,
    SESSION_MAP_FOCUS_GLYPH, SceneCellStyle, SceneColor, SceneRect, SearchOverlay,
    SessionMapOverlay, Theme, TimelineOverlay, WelcomeOverlay, layout,
};

use super::primitives::bordered_inner_rect;
use unicode_segmentation::UnicodeSegmentation;

impl Compiler {
    pub(super) fn paint_overlay(&mut self, overlay: &OverlayScene, theme: &Theme) {
        match overlay {
            OverlayScene::Palette(palette) => self.paint_palette(palette, theme),
            OverlayScene::ContextMenu(menu) => self.paint_context_menu(menu, theme),
            OverlayScene::Timeline(timeline) => self.paint_timeline(timeline, theme),
            OverlayScene::SessionMap(map) => self.paint_session_map(map, theme),
            OverlayScene::Prompt(prompt) => self.paint_prompt(prompt, theme),
            OverlayScene::Search(search) => self.paint_search(search, theme),
            OverlayScene::Help(help) => self.paint_help(help, theme),
            OverlayScene::Welcome(welcome) => self.paint_welcome(welcome, theme),
        }
    }

    fn paint_context_menu(&mut self, menu: &ContextMenuOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(menu.area, None, theme);
        for (index, item) in menu
            .items
            .iter()
            .take(usize::from(inner.height))
            .enumerate()
        {
            let selected = menu.selected == index;
            let line_style = selected_item_style(surface, selected, theme);
            let label = format!(" {}", item.label);
            let label_width = display_width(&label);
            let hint_width = display_width(&item.chord_hint) + 1;
            let padding = usize::from(inner.width)
                .saturating_sub(label_width + hint_width)
                .max(1);
            let y = inner.y.saturating_add(index as u16);
            let mut column = 0usize;
            let leading = format!("{label}{}", " ".repeat(padding));
            for grapheme in leading.graphemes(true) {
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, line_style, selected);
            }
            for grapheme in item.chord_hint.graphemes(true) {
                self.paint_overlay_grapheme(
                    inner,
                    &mut column,
                    y,
                    grapheme,
                    SceneCellStyle {
                        dim: true,
                        ..line_style
                    },
                    selected,
                );
            }
        }
    }

    fn paint_timeline(&mut self, timeline: &TimelineOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(timeline.area, Some(" Timeline "), theme);
        if inner.is_empty() {
            return;
        }
        self.paint_input(
            inner,
            &timeline.query,
            "type to filter · pane:<id> kind:<family> since:<5m>",
            surface,
        );
        if timeline.items.is_empty() && inner.height > 1 {
            self.paint_text_row(
                inner,
                1,
                " no matching events",
                SceneCellStyle {
                    dim: true,
                    ..surface
                },
            );
        }
        for (row, index) in
            layout::palette_item_window(inner, timeline.items.len(), timeline.selected).enumerate()
        {
            let item = &timeline.items[index];
            let selected = timeline.selected == Some(index);
            let line_style = selected_item_style(surface, selected, theme);
            let y = inner.y.saturating_add(1).saturating_add(row as u16);
            let mut column = 0usize;
            for grapheme in format!(" {} ", item.glyph).graphemes(true) {
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, line_style, selected);
            }
            for grapheme in format!("{:>10}  ", item.when).graphemes(true) {
                self.paint_overlay_grapheme(
                    inner,
                    &mut column,
                    y,
                    grapheme,
                    SceneCellStyle {
                        dim: true,
                        ..line_style
                    },
                    selected,
                );
            }
            for grapheme in item.text.graphemes(true) {
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, line_style, selected);
            }
        }
        self.paint_overlay_footer(inner, &timeline.footer, surface);
    }

    fn paint_search(&mut self, search: &SearchOverlay, theme: &Theme) {
        let (inner, surface) =
            self.paint_overlay_shell(search.area, Some(" Search Session Output "), theme);
        if inner.is_empty() {
            return;
        }
        self.paint_input(
            inner,
            &search.query,
            "type to search output · pane:<title> kind:<terminal|task|agent|timeline>",
            surface,
        );
        if search.items.is_empty() && inner.height > 1 {
            let calm = if search.query.trim().is_empty() {
                " searching this session's pane output and timeline (snapshot)"
            } else {
                " no matches"
            };
            self.paint_text_row(
                inner,
                1,
                calm,
                SceneCellStyle {
                    dim: true,
                    ..surface
                },
            );
        }

        let mut previous_source: Option<&str> = None;
        for (row, index) in
            layout::palette_item_window(inner, search.items.len(), search.selected).enumerate()
        {
            let item = &search.items[index];
            let source = if previous_source == Some(item.source.as_str()) {
                " ".repeat(display_width(&item.source))
            } else {
                item.source.clone()
            };
            previous_source = Some(item.source.as_str());
            let selected = search.selected == Some(index);
            let line_style = selected_item_style(surface, selected, theme);
            let y = inner.y.saturating_add(1).saturating_add(row as u16);
            let mut column = 0usize;
            for grapheme in format!(" {source}  ").graphemes(true) {
                self.paint_overlay_grapheme(
                    inner,
                    &mut column,
                    y,
                    grapheme,
                    SceneCellStyle {
                        dim: true,
                        ..line_style
                    },
                    selected,
                );
            }
            let mut scalar_position = 0usize;
            for grapheme in item.text.graphemes(true) {
                let mut cell_style = line_style;
                let scalar_len = grapheme.chars().count();
                if item.match_indices.iter().any(|index| {
                    (*index >= scalar_position) && (*index < scalar_position + scalar_len)
                }) {
                    cell_style.bold = true;
                    cell_style.underline = true;
                }
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, cell_style, selected);
                scalar_position += scalar_len;
            }
        }
        self.paint_overlay_footer(inner, &search.footer, surface);
    }

    fn paint_session_map(&mut self, map: &SessionMapOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(map.area, Some(" Sessions "), theme);
        for (row, index) in
            layout::session_map_item_window(inner, map.rows.len(), Some(map.selected)).enumerate()
        {
            let item = &map.rows[index];
            let selected = map.selected == index;
            let line_style = selected_item_style(surface, selected, theme);
            let y = inner.y.saturating_add(row as u16);
            let marker = if item.focused {
                SESSION_MAP_FOCUS_GLYPH
            } else {
                " "
            };
            let mut column = 0usize;
            for grapheme in format!(
                "{marker}{}{} {}",
                "  ".repeat(usize::from(item.depth)),
                item.glyph,
                item.label
            )
            .graphemes(true)
            {
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, line_style, selected);
            }
            if !item.state.is_empty() {
                for grapheme in format!("  {}", item.state).graphemes(true) {
                    self.paint_overlay_grapheme(
                        inner,
                        &mut column,
                        y,
                        grapheme,
                        SceneCellStyle {
                            dim: true,
                            ..line_style
                        },
                        selected,
                    );
                }
            }
            if !item.badges.is_empty() {
                for grapheme in format!("  [{}]", item.badges).graphemes(true) {
                    self.paint_overlay_grapheme(
                        inner,
                        &mut column,
                        y,
                        grapheme,
                        SceneCellStyle {
                            dim: true,
                            ..line_style
                        },
                        selected,
                    );
                }
            }
        }
        self.paint_overlay_footer(inner, &map.footer, surface);
    }

    fn paint_prompt(&mut self, prompt: &PromptOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(prompt.area, Some(&prompt.title), theme);
        if inner.is_empty() {
            return;
        }
        self.paint_text_row(inner, 0, "> ", surface);
        self.paint_text(
            SceneRect::new(
                inner.x.saturating_add(2),
                inner.y,
                inner.width.saturating_sub(2),
                1,
            ),
            &prompt.input,
            surface,
        );
        let cursor_column = 2usize
            .saturating_add(display_width(&prompt.input))
            .min(usize::from(inner.width.saturating_sub(1)));
        let mut cursor = ProgramCell::glyph(' ', surface);
        cursor.cursor = true;
        self.paint_cell(
            inner.x.saturating_add(cursor_column as u16),
            inner.y,
            cursor,
        );
        self.paint_overlay_footer(inner, &prompt.footer, surface);
    }

    fn paint_help(&mut self, help: &HelpOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(help.area, Some(" Help "), theme);
        if inner.is_empty() {
            return;
        }
        self.paint_input(inner, &help.query, "type to filter the keymap", surface);
        if help.items.is_empty() && inner.height > 1 {
            self.paint_text_row(
                inner,
                1,
                " no matching entries",
                SceneCellStyle {
                    dim: true,
                    ..surface
                },
            );
        }
        for (row, index) in
            layout::palette_item_window(inner, help.items.len(), help.selected).enumerate()
        {
            let item = &help.items[index];
            let selected = help.selected == Some(index);
            let line_style = selected_item_style(surface, selected, theme);
            let y = inner.y.saturating_add(1).saturating_add(row as u16);
            let label = if item.heading {
                format!(" {}", item.label)
            } else {
                format!("   {}", item.label)
            };
            let mut column = 0usize;
            for grapheme in label.graphemes(true) {
                self.paint_overlay_grapheme(
                    inner,
                    &mut column,
                    y,
                    grapheme,
                    SceneCellStyle {
                        bold: item.heading,
                        ..line_style
                    },
                    selected,
                );
            }
            if !item.keys.is_empty() {
                for grapheme in format!("  {}", item.keys).graphemes(true) {
                    self.paint_overlay_grapheme(
                        inner,
                        &mut column,
                        y,
                        grapheme,
                        SceneCellStyle {
                            dim: true,
                            ..line_style
                        },
                        selected,
                    );
                }
            }
        }
        self.paint_overlay_footer(inner, &help.footer, surface);
    }

    fn paint_welcome(&mut self, welcome: &WelcomeOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(welcome.area, Some(" Mandatum "), theme);
        if inner.is_empty() {
            return;
        }
        self.paint_text_row(
            inner,
            0,
            &welcome.introduction,
            SceneCellStyle {
                bold: true,
                ..surface
            },
        );
        let key_width = welcome
            .entries
            .iter()
            .map(|entry| display_width(&entry.keys))
            .max()
            .unwrap_or(0);
        for (index, entry) in welcome.entries.iter().enumerate() {
            let row = index.saturating_add(2);
            if row >= usize::from(inner.height) {
                break;
            }
            let y = inner.y.saturating_add(row as u16);
            let mut column = 0usize;
            for grapheme in "  ".graphemes(true) {
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, surface, false);
            }
            let padding = key_width.saturating_sub(display_width(&entry.keys));
            for grapheme in format!("{}{}", entry.keys, " ".repeat(padding)).graphemes(true) {
                self.paint_overlay_grapheme(
                    inner,
                    &mut column,
                    y,
                    grapheme,
                    SceneCellStyle {
                        foreground: theme.palette_border,
                        bold: true,
                        ..surface
                    },
                    false,
                );
            }
            for grapheme in format!("  {}", entry.description).graphemes(true) {
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, surface, false);
            }
        }
        let dismissal_row = welcome.entries.len().saturating_add(3);
        if dismissal_row < usize::from(inner.height) {
            self.paint_text_row(
                inner,
                dismissal_row,
                &welcome.dismissal,
                SceneCellStyle {
                    dim: true,
                    ..surface
                },
            );
        }
    }

    fn paint_overlay_shell(
        &mut self,
        area: SceneRect,
        title: Option<&str>,
        theme: &Theme,
    ) -> (SceneRect, SceneCellStyle) {
        let surface = style(theme.overlay_foreground, theme.overlay_background);
        self.paint_rect(area, surface);
        self.paint_border(area, style(theme.palette_border, theme.overlay_background));
        if let Some(title) = title {
            self.paint_text(
                SceneRect::new(
                    area.x.saturating_add(1),
                    area.y,
                    area.width.saturating_sub(2),
                    area.height.min(1),
                ),
                title,
                surface,
            );
        }
        (bordered_inner_rect(area), surface)
    }

    fn paint_input(
        &mut self,
        inner: SceneRect,
        query: &str,
        placeholder: &str,
        surface: SceneCellStyle,
    ) {
        self.paint_text_row(inner, 0, "> ", surface);
        let input_area = SceneRect::new(
            inner.x.saturating_add(2),
            inner.y,
            inner.width.saturating_sub(2),
            1,
        );
        if query.is_empty() {
            self.paint_text(
                input_area,
                placeholder,
                SceneCellStyle {
                    dim: true,
                    ..surface
                },
            );
            return;
        }
        self.paint_text(input_area, query, surface);
        let cursor_column = 2usize
            .saturating_add(display_width(query))
            .min(usize::from(inner.width.saturating_sub(1)));
        let mut cursor = ProgramCell::glyph(' ', surface);
        cursor.cursor = true;
        self.paint_cell(
            inner.x.saturating_add(cursor_column as u16),
            inner.y,
            cursor,
        );
    }

    fn paint_overlay_footer(&mut self, inner: SceneRect, footer: &str, surface: SceneCellStyle) {
        if inner.height <= 1 {
            return;
        }
        self.paint_text_row(
            inner,
            usize::from(inner.height.saturating_sub(1)),
            &format!(" {footer}"),
            SceneCellStyle {
                dim: true,
                ..surface
            },
        );
    }

    fn paint_palette(&mut self, palette: &PaletteOverlay, theme: &Theme) {
        let surface = style(theme.overlay_foreground, theme.overlay_background);
        let border = style(theme.palette_border, theme.overlay_background);
        self.paint_rect(palette.area, surface);
        self.paint_border(palette.area, border);
        self.paint_text(
            SceneRect::new(
                palette.area.x.saturating_add(1),
                palette.area.y,
                palette.area.width.saturating_sub(2),
                palette.area.height.min(1),
            ),
            " Command Palette ",
            surface,
        );

        let inner = bordered_inner_rect(palette.area);
        if inner.is_empty() {
            return;
        }

        self.paint_text_row(inner, 0, "> ", surface);
        if palette.query.is_empty() {
            self.paint_text(
                SceneRect::new(
                    inner.x.saturating_add(2),
                    inner.y,
                    inner.width.saturating_sub(2),
                    1,
                ),
                "letters run their key · shift+letter to search",
                SceneCellStyle {
                    dim: true,
                    ..surface
                },
            );
        } else {
            self.paint_text(
                SceneRect::new(
                    inner.x.saturating_add(2),
                    inner.y,
                    inner.width.saturating_sub(2),
                    1,
                ),
                &palette.query,
                surface,
            );
            let cursor_column = 2usize
                .saturating_add(display_width(&palette.query))
                .min(usize::from(inner.width.saturating_sub(1)));
            let mut cursor = ProgramCell::glyph(' ', surface);
            cursor.cursor = true;
            self.paint_cell(
                inner.x.saturating_add(cursor_column as u16),
                inner.y,
                cursor,
            );
        }

        if palette.items.is_empty() && inner.height > 1 {
            self.paint_text_row(
                inner,
                1,
                " no matching commands",
                SceneCellStyle {
                    dim: true,
                    ..surface
                },
            );
        }

        for (row, index) in
            layout::palette_item_window(inner, palette.items.len(), palette.selected).enumerate()
        {
            let item = &palette.items[index];
            let selected = palette.selected == Some(index);
            let mut line_style = surface;
            line_style.dim = !item.enabled;
            if selected {
                if theme.palette_selection != SceneColor::Default {
                    line_style.foreground = theme.palette_selection;
                }
                line_style.inverse = true;
            }

            let y = inner.y.saturating_add(1).saturating_add(row as u16);
            let mut column = 0usize;
            self.paint_overlay_grapheme(inner, &mut column, y, " ", line_style, selected);
            let mut scalar_position = 0usize;
            for grapheme in item.label.graphemes(true) {
                let mut cell_style = line_style;
                let scalar_len = grapheme.chars().count();
                if item.match_indices.iter().any(|index| {
                    (*index >= scalar_position) && (*index < scalar_position + scalar_len)
                }) {
                    cell_style.bold = true;
                    cell_style.underline = true;
                }
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, cell_style, selected);
                scalar_position += scalar_len;
            }
            if let Some(hint) = &item.key_hint {
                for grapheme in format!("  {hint}").graphemes(true) {
                    let cell_style = SceneCellStyle {
                        dim: true,
                        ..line_style
                    };
                    self.paint_overlay_grapheme(
                        inner,
                        &mut column,
                        y,
                        grapheme,
                        cell_style,
                        selected,
                    );
                }
            }
            for grapheme in format!("  {}", item.detail).graphemes(true) {
                let cell_style = SceneCellStyle {
                    dim: true,
                    ..line_style
                };
                self.paint_overlay_grapheme(inner, &mut column, y, grapheme, cell_style, selected);
            }
        }

        if inner.height > 1 {
            self.paint_text_row_marked(
                inner,
                usize::from(inner.height.saturating_sub(1)),
                &format!(" {}", palette.footer),
                SceneCellStyle {
                    dim: true,
                    ..surface
                },
                false,
            );
        }
    }

    fn paint_overlay_grapheme(
        &mut self,
        area: SceneRect,
        column: &mut usize,
        y: u16,
        grapheme: &str,
        cell_style: SceneCellStyle,
        selected: bool,
    ) {
        let (grapheme, width) = bounded_grapheme(grapheme);
        if width <= usize::from(area.width).saturating_sub(*column) {
            self.paint_grapheme(
                area.x.saturating_add(*column as u16),
                y,
                grapheme,
                width as u8,
                cell_style,
                selected.then_some(CellSelection::Item),
                false,
                None,
            );
        }
        *column = column.saturating_add(width);
    }
}

fn style(foreground: SceneColor, background: SceneColor) -> SceneCellStyle {
    SceneCellStyle {
        foreground,
        background,
        ..SceneCellStyle::default()
    }
}

fn selected_item_style(
    mut surface: SceneCellStyle,
    selected: bool,
    theme: &Theme,
) -> SceneCellStyle {
    if selected {
        if theme.palette_selection != SceneColor::Default {
            surface.foreground = theme.palette_selection;
        }
        surface.inverse = true;
    }
    surface
}
