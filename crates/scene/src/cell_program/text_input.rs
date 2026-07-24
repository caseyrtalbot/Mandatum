use super::{Compiler, primitives::display_width};
use crate::{SceneCellStyle, TextInputKind, TextInputScene, Theme};

impl Compiler {
    pub(super) fn paint_text_input(&mut self, input: &TextInputScene, theme: &Theme) {
        let Some(preedit) = &input.preedit else {
            return;
        };
        if input.area.is_empty() || preedit.text.is_empty() {
            return;
        }

        let mut style = match input.kind {
            TextInputKind::Terminal { style } => style,
            TextInputKind::Overlay => SceneCellStyle {
                foreground: theme.overlay_foreground,
                background: theme.overlay_background,
                ..SceneCellStyle::default()
            },
        };
        if matches!(input.kind, TextInputKind::Overlay) {
            // Empty overlay inputs paint placeholder copy first. Preedit owns
            // the active input row, so clear that copy before drawing the
            // transient composition.
            self.paint_rect(input.area, style);
        }
        style.underline = true;
        self.paint_text(input.area, &preedit.text, style);

        let cursor_end = preedit
            .cursor
            .filter(|range| range.is_valid_for(&preedit.text))
            .map_or(preedit.text.len(), |range| range.end);
        let cursor_column = display_width(&preedit.text[..cursor_end])
            .min(usize::from(input.area.width.saturating_sub(1)));
        self.mark_cursor(
            input.area.x.saturating_add(cursor_column as u16),
            input.area.y,
            style,
        );
    }
}
