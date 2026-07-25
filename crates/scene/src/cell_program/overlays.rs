use super::{
    CellSelection, Compiler, ProgramCell, TextPaintScopeKind,
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
        let area = match overlay {
            OverlayScene::Palette(overlay) => overlay.area,
            OverlayScene::ContextMenu(overlay) => overlay.area,
            OverlayScene::Timeline(overlay) => overlay.area,
            OverlayScene::SessionMap(overlay) => overlay.area,
            OverlayScene::Prompt(overlay) => overlay.area,
            OverlayScene::Search(overlay) => overlay.area,
            OverlayScene::Help(overlay) => overlay.area,
            OverlayScene::Welcome(overlay) => overlay.area,
        };
        self.begin_text_scope(TextPaintScopeKind::Overlay, area);
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
        if inner.is_empty() {
            return;
        }
        let window = layout::context_menu_item_window(inner, menu.items.len(), Some(menu.selected));
        for (row, index) in window.enumerate() {
            let item = &menu.items[index];
            let Some(block) = layout::context_menu_item_rect(inner, row) else {
                continue;
            };
            let selected = menu.selected == index;
            let line_style = selected_item_style(surface, selected, theme);
            let label = format!(" {}", item.label);
            let hint_width =
                display_width(&item.chord_hint).min(usize::from(inner.width.saturating_sub(2)));
            let label_width = usize::from(inner.width)
                .saturating_sub(hint_width.saturating_add(2))
                .max(1);
            let y = block.y;
            let label_area = SceneRect::new(inner.x, y, label_width as u16, 1);
            let mut column = 0usize;
            for grapheme in label.graphemes(true) {
                self.paint_overlay_grapheme(
                    label_area,
                    &mut column,
                    y,
                    grapheme,
                    line_style,
                    selected,
                );
            }
            if !item.chord_hint.is_empty() {
                let hint_area = SceneRect::new(
                    inner.right().saturating_sub(hint_width as u16),
                    y,
                    hint_width as u16,
                    1,
                );
                let mut hint_column = 0usize;
                for grapheme in item.chord_hint.graphemes(true) {
                    self.paint_overlay_grapheme(
                        hint_area,
                        &mut hint_column,
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
    }

    fn paint_timeline(&mut self, timeline: &TimelineOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(timeline.area, Some(" Timeline "), theme);
        if inner.is_empty() {
            return;
        }
        self.paint_input(
            layout::filtered_overlay_input_rect(inner),
            &timeline.query,
            "type to filter · pane:<id> kind:<family> since:<5m>",
            surface,
        );
        if let (true, Some(empty)) = (
            timeline.items.is_empty(),
            layout::palette_item_rect(inner, 0),
        ) {
            self.paint_text_row(
                empty,
                0,
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
            let Some(block) = layout::palette_item_rect(inner, row) else {
                continue;
            };
            let item = &timeline.items[index];
            let selected = timeline.selected == Some(index);
            let line_style = selected_item_style(surface, selected, theme);
            let y = block.y;
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
        if let Some(footer) = layout::filtered_overlay_footer_rect(inner) {
            self.paint_overlay_footer(footer, &timeline.footer, surface);
        }
    }

    fn paint_search(&mut self, search: &SearchOverlay, theme: &Theme) {
        let (inner, surface) =
            self.paint_overlay_shell(search.area, Some(" Search Session Output "), theme);
        if inner.is_empty() {
            return;
        }
        self.paint_input(
            layout::filtered_overlay_input_rect(inner),
            &search.query,
            "type to search output · pane:<title> kind:<terminal|task|agent|timeline>",
            surface,
        );
        if let (true, Some(empty)) = (search.items.is_empty(), layout::palette_item_rect(inner, 0))
        {
            let calm = if search.query.trim().is_empty() {
                " searching this session's pane output and timeline (snapshot)"
            } else {
                " no matches"
            };
            self.paint_text_row(
                empty,
                0,
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
            let Some(block) = layout::palette_item_rect(inner, row) else {
                continue;
            };
            let item = &search.items[index];
            let source = if previous_source == Some(item.source.as_str()) {
                " ".repeat(display_width(&item.source))
            } else {
                item.source.clone()
            };
            previous_source = Some(item.source.as_str());
            let selected = search.selected == Some(index);
            let line_style = selected_item_style(surface, selected, theme);
            let y = block.y;
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
        if let Some(footer) = layout::filtered_overlay_footer_rect(inner) {
            self.paint_overlay_footer(footer, &search.footer, surface);
        }
    }

    fn paint_session_map(&mut self, map: &SessionMapOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(map.area, Some(" Sessions "), theme);
        for (row, index) in
            layout::session_map_item_window(inner, map.rows.len(), Some(map.selected)).enumerate()
        {
            let Some(block) = layout::session_map_item_rect(inner, row) else {
                continue;
            };
            let item = &map.rows[index];
            let selected = map.selected == index;
            let line_style = selected_item_style(surface, selected, theme);
            let y = block.y;
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
        if let Some(footer) = layout::footer_only_overlay_footer_rect(inner) {
            self.paint_overlay_footer(footer, &map.footer, surface);
        }
    }

    fn paint_prompt(&mut self, prompt: &PromptOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(prompt.area, Some(&prompt.title), theme);
        if inner.is_empty() {
            return;
        }
        let input = layout::filtered_overlay_input_rect(inner);
        self.paint_text_row(input, 0, "> ", surface);
        self.paint_text(
            SceneRect::new(
                input.x.saturating_add(2),
                input.y,
                input.width.saturating_sub(2),
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
            input.x.saturating_add(cursor_column as u16),
            input.y,
            cursor,
        );
        if let Some(footer) = layout::filtered_overlay_footer_rect(inner) {
            self.paint_overlay_footer(footer, &prompt.footer, surface);
        }
    }

    fn paint_help(&mut self, help: &HelpOverlay, theme: &Theme) {
        let (inner, surface) = self.paint_overlay_shell(help.area, Some(" Help "), theme);
        if inner.is_empty() {
            return;
        }
        self.paint_input(
            layout::filtered_overlay_input_rect(inner),
            &help.query,
            "type to filter the keymap",
            surface,
        );
        if let (true, Some(empty)) = (help.items.is_empty(), layout::palette_item_rect(inner, 0)) {
            self.paint_text_row(
                empty,
                0,
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
            let Some(block) = layout::palette_item_rect(inner, row) else {
                continue;
            };
            let item = &help.items[index];
            let selected = help.selected == Some(index);
            let line_style = selected_item_style(surface, selected, theme);
            let y = block.y;
            let label = if item.heading {
                format!(" {}", item.label)
            } else {
                format!("   {}", item.label)
            };
            let key_width =
                display_width(&item.keys).min(usize::from(inner.width.saturating_sub(2)));
            let label_width = usize::from(inner.width)
                .saturating_sub(key_width.saturating_add(2))
                .max(1);
            let label_area = SceneRect::new(inner.x, y, label_width as u16, 1);
            let mut column = 0usize;
            for grapheme in label.graphemes(true) {
                self.paint_overlay_grapheme(
                    label_area,
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
                let key_area = SceneRect::new(
                    inner.right().saturating_sub(key_width as u16),
                    y,
                    key_width as u16,
                    1,
                );
                let mut key_column = 0usize;
                for grapheme in item.keys.graphemes(true) {
                    self.paint_overlay_grapheme(
                        key_area,
                        &mut key_column,
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
        if let Some(footer) = layout::filtered_overlay_footer_rect(inner) {
            self.paint_overlay_footer(footer, &help.footer, surface);
        }
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
        let surface_scope = self.begin_text_scope(TextPaintScopeKind::Overlay, area);
        self.paint_rect(area, surface);
        self.begin_text_scope(TextPaintScopeKind::OverlayDecoration, area);
        self.paint_border(area, style(theme.palette_border, theme.overlay_background));
        self.set_text_scope(surface_scope);
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

    fn paint_overlay_footer(&mut self, area: SceneRect, footer: &str, surface: SceneCellStyle) {
        if area.is_empty() {
            return;
        }
        self.paint_text_row(
            area,
            0,
            &format!(" {footer}"),
            SceneCellStyle {
                dim: true,
                ..surface
            },
        );
    }

    fn paint_palette(&mut self, palette: &PaletteOverlay, theme: &Theme) {
        let (inner, surface) =
            self.paint_overlay_shell(palette.area, Some(" Command Palette "), theme);
        if inner.is_empty() {
            return;
        }

        self.paint_input(
            layout::filtered_overlay_input_rect(inner),
            &palette.query,
            "letters run their key · shift+letter to search",
            surface,
        );

        if let (true, Some(empty)) = (
            palette.items.is_empty(),
            layout::palette_item_rect(inner, 0),
        ) {
            self.paint_text_row(
                empty,
                0,
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
            let Some(block) = layout::palette_item_rect(inner, row) else {
                continue;
            };
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

            let y = block.y;
            let hint_width = item
                .key_hint
                .as_deref()
                .map(display_width)
                .unwrap_or(0)
                .min(usize::from(inner.width.saturating_sub(2)));
            let left_width = usize::from(inner.width)
                .saturating_sub(hint_width.saturating_add(2))
                .max(1);
            let left_area = SceneRect::new(inner.x, y, left_width as u16, 1);
            let mut column = 0usize;
            self.paint_overlay_grapheme(left_area, &mut column, y, " ", line_style, selected);
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
                self.paint_overlay_grapheme(
                    left_area,
                    &mut column,
                    y,
                    grapheme,
                    cell_style,
                    selected,
                );
                scalar_position += scalar_len;
            }
            for grapheme in format!("  {}", item.detail).graphemes(true) {
                let cell_style = SceneCellStyle {
                    dim: true,
                    ..line_style
                };
                self.paint_overlay_grapheme(
                    left_area,
                    &mut column,
                    y,
                    grapheme,
                    cell_style,
                    selected,
                );
            }
            if let Some(hint) = &item.key_hint {
                let hint_area = SceneRect::new(
                    inner.right().saturating_sub(hint_width as u16),
                    y,
                    hint_width as u16,
                    1,
                );
                let mut hint_column = 0usize;
                for grapheme in hint.graphemes(true) {
                    self.paint_overlay_grapheme(
                        hint_area,
                        &mut hint_column,
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

        if let Some(footer) = layout::filtered_overlay_footer_rect(inner) {
            self.paint_text_row_marked(
                footer,
                0,
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
