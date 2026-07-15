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
/// `Theme::default()` is the built-in `mandatum-dark` theme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    /// Title text of the focused pane.
    #[serde(alias = "focus_border")]
    pub focus_title: SceneColor,
    /// Border of every pane; focus emphasis lives on the title.
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
    /// Foreground shared by modal and first-run overlay surfaces.
    pub overlay_foreground: SceneColor,
    /// Background shared by modal and first-run overlay surfaces.
    pub overlay_background: SceneColor,
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
        focus_title: SceneColor::Ansi(12), // bright blue
        pane_border: SceneColor::Ansi(8),  // dark gray
        pane_title: SceneColor::Default,
        header: SceneColor::Ansi(15),           // white
        header_background: SceneColor::Ansi(0), // black
        status: SceneColor::Ansi(7),            // gray
        // Red stays reserved for attention; bright blue marks focus while
        // yellow remains available for waiting states.
        attention: SceneColor::Ansi(1),
        palette_border: SceneColor::Ansi(6),          // cyan
        overlay_foreground: SceneColor::Ansi(7),      // gray
        overlay_background: SceneColor::Indexed(233), // near-black surface
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
        focus_title: SceneColor::Ansi(4), // blue
        pane_border: SceneColor::Ansi(7), // gray
        pane_title: SceneColor::Default,
        header: SceneColor::Ansi(0),                  // black
        header_background: SceneColor::Ansi(7),       // gray
        status: SceneColor::Ansi(8),                  // dark gray
        attention: SceneColor::Ansi(1),               // red
        palette_border: SceneColor::Ansi(4),          // blue
        overlay_foreground: SceneColor::Ansi(0),      // black
        overlay_background: SceneColor::Indexed(254), // pale gray surface
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
        // Focus must be unmistakable without making the entire pane frame
        // loud: bright yellow title text against bright-white calm chrome.
        focus_title: SceneColor::Ansi(11), // bright yellow
        pane_border: SceneColor::Ansi(15),
        pane_title: SceneColor::Ansi(15),
        header: SceneColor::Ansi(15),
        header_background: SceneColor::Ansi(0),
        status: SceneColor::Ansi(15),
        // Bright red, not bright yellow: focus owns yellow here too.
        attention: SceneColor::Ansi(9),
        palette_border: SceneColor::Ansi(15),
        overlay_foreground: SceneColor::Ansi(15), // white
        overlay_background: SceneColor::Ansi(4),  // blue modal surface
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
    fn focused_pane_title_is_distinct_in_every_builtin_theme() {
        // Accessibility: focus visibility must never rely on a modifier
        // alone. Every built-in theme gives the focused title its own color,
        // distinct from calm titles AND from the attention color, so
        // "focused" and "needs attention" stay separate signals.
        for name in Theme::BUILTIN_NAMES {
            let theme = Theme::builtin(name).expect("builtin theme exists");
            assert_ne!(
                theme.focus_title, theme.pane_title,
                "theme {name} must distinguish the focused pane title"
            );
            assert_ne!(
                theme.focus_title, theme.attention,
                "theme {name} must not reuse the attention color for focus"
            );
        }
    }

    #[test]
    fn dark_focus_stays_distinct_from_waiting_and_overlay_chrome() {
        let theme = Theme::default();

        assert_ne!(theme.focus_title, theme.agent_waiting);
        assert_ne!(theme.focus_title, theme.palette_border);
    }

    #[test]
    fn every_overlay_surface_has_an_explicit_background() {
        for name in Theme::BUILTIN_NAMES {
            let theme = Theme::builtin(name).expect("builtin theme exists");
            assert_ne!(theme.overlay_foreground, SceneColor::Default);
            assert_ne!(theme.overlay_background, SceneColor::Default);
            assert_ne!(theme.overlay_background, theme.header_background);
        }
    }
}
