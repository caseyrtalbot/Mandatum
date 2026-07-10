//! Semantic theme roles resolved to neutral colors.
//!
//! The scene stays color-semantic: panes carry roles (focused, waiting for
//! approval, agent status) and the theme maps each role to a [`SceneColor`].
//! Frontend adapters translate those neutral colors into their own paint
//! types; no frontend color type appears here (L1).

use serde::{Deserialize, Serialize};

use crate::style::SceneColor;

/// Named color for every themable role in the workspace chrome.
///
/// `Theme::default()` is the built-in `mandatum-dark` theme, which matches
/// the pre-theme hardcoded palette exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    /// Border of the focused pane.
    pub focus_border: SceneColor,
    /// Border of unfocused panes.
    pub pane_border: SceneColor,
    /// Pane title text.
    pub pane_title: SceneColor,
    /// Header strip foreground.
    pub header: SceneColor,
    /// Header strip background.
    pub header_background: SceneColor,
    /// Status line foreground.
    pub status: SceneColor,
    /// Attention/approval emphasis (pending agent approvals).
    pub attention: SceneColor,
    /// Palette overlay border.
    pub palette_border: SceneColor,
    /// Highlighted palette row. `Default` keeps the reverse-video highlight.
    pub palette_selection: SceneColor,
    /// Copy-mode selection background. `Default` keeps reverse-video.
    pub selection_highlight: SceneColor,
    pub agent_running: SceneColor,
    pub agent_waiting: SceneColor,
    pub agent_failed: SceneColor,
    pub agent_complete: SceneColor,
    /// Draft/blocked/unknown agent states.
    pub agent_idle: SceneColor,
}

impl Default for Theme {
    fn default() -> Self {
        mandatum_dark()
    }
}

impl Theme {
    /// A built-in theme by name, if one exists.
    pub fn builtin(name: &str) -> Option<Self> {
        match name {
            "mandatum-dark" => Some(mandatum_dark()),
            "mandatum-light" => Some(mandatum_light()),
            "mandatum-high-contrast" => Some(mandatum_high_contrast()),
            _ => None,
        }
    }

    /// The names of every built-in theme, for error messages.
    pub const BUILTIN_NAMES: &'static [&'static str] =
        &["mandatum-dark", "mandatum-light", "mandatum-high-contrast"];
}

fn mandatum_dark() -> Theme {
    Theme {
        name: "mandatum-dark".to_owned(),
        focus_border: SceneColor::Ansi(3), // yellow
        pane_border: SceneColor::Ansi(8),  // dark gray
        pane_title: SceneColor::Default,
        header: SceneColor::Ansi(15),           // white
        header_background: SceneColor::Ansi(0), // black
        status: SceneColor::Ansi(7),            // gray
        // Red, not yellow: focus is yellow in this theme, and "focused" and
        // "needs attention" must never share a color.
        attention: SceneColor::Ansi(1),
        palette_border: SceneColor::Ansi(6), // cyan
        palette_selection: SceneColor::Default,
        selection_highlight: SceneColor::Default,
        agent_running: SceneColor::Ansi(2),  // green
        agent_waiting: SceneColor::Ansi(3),  // yellow
        agent_failed: SceneColor::Ansi(1),   // red
        agent_complete: SceneColor::Ansi(6), // cyan
        agent_idle: SceneColor::Default,
    }
}

fn mandatum_light() -> Theme {
    Theme {
        name: "mandatum-light".to_owned(),
        focus_border: SceneColor::Ansi(4), // blue
        pane_border: SceneColor::Ansi(7),  // gray
        pane_title: SceneColor::Default,
        header: SceneColor::Ansi(0),            // black
        header_background: SceneColor::Ansi(7), // gray
        status: SceneColor::Ansi(8),            // dark gray
        attention: SceneColor::Ansi(1),         // red
        palette_border: SceneColor::Ansi(4),    // blue
        palette_selection: SceneColor::Default,
        selection_highlight: SceneColor::Default,
        agent_running: SceneColor::Ansi(2),  // green
        agent_waiting: SceneColor::Ansi(5),  // magenta
        agent_failed: SceneColor::Ansi(1),   // red
        agent_complete: SceneColor::Ansi(4), // blue
        agent_idle: SceneColor::Default,
    }
}

fn mandatum_high_contrast() -> Theme {
    Theme {
        name: "mandatum-high-contrast".to_owned(),
        // Focus must be unmistakable: bright yellow against the bright-white
        // unfocused borders (the same focus-is-yellow convention as
        // mandatum-dark), not white-on-white.
        focus_border: SceneColor::Ansi(11), // bright yellow
        pane_border: SceneColor::Ansi(15),
        pane_title: SceneColor::Ansi(15),
        header: SceneColor::Ansi(15),
        header_background: SceneColor::Ansi(0),
        status: SceneColor::Ansi(15),
        // Bright red, not bright yellow: focus owns yellow here too.
        attention: SceneColor::Ansi(9),
        palette_border: SceneColor::Ansi(15),
        palette_selection: SceneColor::Default,
        selection_highlight: SceneColor::Default,
        agent_running: SceneColor::Ansi(10),  // bright green
        agent_waiting: SceneColor::Ansi(11),  // bright yellow
        agent_failed: SceneColor::Ansi(9),    // bright red
        agent_complete: SceneColor::Ansi(14), // bright cyan
        agent_idle: SceneColor::Ansi(15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_is_mandatum_dark() {
        assert_eq!(Theme::default().name, "mandatum-dark");
        assert_eq!(Theme::default(), Theme::builtin("mandatum-dark").unwrap());
    }

    #[test]
    fn every_builtin_name_resolves_and_unknown_names_do_not() {
        for name in Theme::BUILTIN_NAMES {
            let theme = Theme::builtin(name).expect("builtin theme exists");
            assert_eq!(&theme.name, name);
        }
        assert!(Theme::builtin("solarized").is_none());
    }

    #[test]
    fn focused_pane_border_is_distinct_in_every_builtin_theme() {
        // Accessibility: focus visibility must never rely on a modifier
        // alone. Every built-in theme gives the focused border its own
        // color, distinct from unfocused borders AND from the attention
        // color, so "focused" and "needs attention" stay separate signals.
        for name in Theme::BUILTIN_NAMES {
            let theme = Theme::builtin(name).expect("builtin theme exists");
            assert_ne!(
                theme.focus_border, theme.pane_border,
                "theme {name} must distinguish the focused pane border"
            );
            assert_ne!(
                theme.focus_border, theme.attention,
                "theme {name} must not reuse the attention color for focus"
            );
        }
    }
}
