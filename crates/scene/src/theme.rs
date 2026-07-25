//! Semantic theme roles resolved to neutral colors.
//!
//! The scene stays color-semantic: panes carry roles (focused, waiting for
//! approval, agent status) and the theme maps each role to a [`SceneColor`].
//! Frontend adapters translate those neutral colors into their own paint
//! types; no frontend color type appears here (L1).

use serde::{Deserialize, Serialize};

use crate::style::SceneColor;

/// Native workspace density. This is presentation policy, not terminal-cell
/// geometry: the maintained terminal projection remains unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiDensity {
    #[default]
    Compact,
    Comfortable,
}

/// Direct RGB colors used when the native pixel surface materializes terminal
/// defaults and the 16 named ANSI colors.
///
/// Keeping these values separate from [`SceneColor`] prevents a terminal
/// palette entry from recursively referring to `Default` or another ANSI
/// slot. The fixed-size array also makes the complete ANSI range part of the
/// type contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPalette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub ansi: [[u8; 3]; 16],
}

impl Default for TerminalPalette {
    fn default() -> Self {
        Self {
            foreground: [220, 220, 224],
            background: [18, 18, 22],
            ansi: [
                [0, 0, 0],
                [205, 49, 49],
                [13, 188, 121],
                [229, 229, 16],
                [36, 114, 200],
                [188, 63, 188],
                [17, 168, 205],
                [229, 229, 229],
                [128, 128, 128],
                [241, 76, 76],
                [35, 209, 139],
                [245, 245, 67],
                [59, 142, 234],
                [214, 112, 214],
                [41, 184, 219],
                [255, 255, 255],
            ],
        }
    }
}

/// Direct RGBA color used by native UI materials.
///
/// Unlike [`SceneColor`], this type cannot refer to terminal defaults or ANSI
/// slots. Native chrome therefore keeps a stable identity when a child
/// terminal palette changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl UiColor {
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::rgba(red, green, blue, u8::MAX)
    }

    pub const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub const fn to_array(self) -> [u8; 4] {
        [self.red, self.green, self.blue, self.alpha]
    }
}

/// Semantic colors for app-owned native surfaces and state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPalette {
    pub canvas: UiColor,
    pub pane_surface: UiColor,
    pub chrome_surface: UiColor,
    pub overlay_surface: UiColor,
    pub border_subtle: UiColor,
    pub border_strong: UiColor,
    pub text_primary: UiColor,
    pub text_secondary: UiColor,
    pub text_muted: UiColor,
    pub focus: UiColor,
    pub running: UiColor,
    pub waiting: UiColor,
    pub failure: UiColor,
    pub complete: UiColor,
    pub agent_identity: UiColor,
    pub selection_fill: UiColor,
    pub modal_scrim: UiColor,
}

impl Default for UiPalette {
    fn default() -> Self {
        flagship_dark_palette()
    }
}

/// The only provisioned native UI font faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiFontFace {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

/// One native UI text metric. Point size uses 1/64-point fixed precision;
/// line height is measured in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTextStyle {
    pub point_size_x64: u16,
    pub line_height: u16,
    pub face: UiFontFace,
}

impl UiTextStyle {
    pub const fn new(point_size_x64: u16, line_height: u16, face: UiFontFace) -> Self {
        Self {
            point_size_x64,
            line_height,
            face,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiTypographyTokens {
    pub terminal: UiTextStyle,
    pub title: UiTextStyle,
    pub pane_title: UiTextStyle,
    pub pane_title_focused: UiTextStyle,
    pub body: UiTextStyle,
    pub metadata: UiTextStyle,
    pub key: UiTextStyle,
}

impl Default for UiTypographyTokens {
    fn default() -> Self {
        Self {
            terminal: UiTextStyle::new(13 * 64, 17, UiFontFace::Regular),
            title: UiTextStyle::new(13 * 64, 18, UiFontFace::Bold),
            pane_title: UiTextStyle::new(12 * 64, 17, UiFontFace::Regular),
            pane_title_focused: UiTextStyle::new(12 * 64, 17, UiFontFace::Bold),
            body: UiTextStyle::new(12 * 64 + 32, 18, UiFontFace::Regular),
            metadata: UiTextStyle::new(11 * 64, 15, UiFontFace::Regular),
            key: UiTextStyle::new(11 * 64, 15, UiFontFace::Regular),
        }
    }
}

/// Spacing and hit-target dimensions in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSpacingTokens {
    pub space_1: u16,
    pub space_2: u16,
    pub space_3: u16,
    pub space_4: u16,
    pub space_6: u16,
    pub pane_title_inset_x: u16,
    pub pane_title_inset_y: u16,
    pub workflow_section_gap: u16,
    pub overlay_outer_padding: u16,
    pub overlay_row_padding_x: u16,
    pub overlay_row_padding_y: u16,
    pub tiled_separator: u16,
    pub separator_target: u16,
    pub min_control_width: u16,
    pub min_control_height: u16,
    pub viewport_edge_margin: u16,
}

impl Default for UiSpacingTokens {
    fn default() -> Self {
        Self {
            space_1: 4,
            space_2: 8,
            space_3: 12,
            space_4: 16,
            space_6: 24,
            pane_title_inset_x: 8,
            pane_title_inset_y: 4,
            workflow_section_gap: 12,
            overlay_outer_padding: 16,
            overlay_row_padding_x: 8,
            overlay_row_padding_y: 6,
            tiled_separator: 1,
            separator_target: 6,
            min_control_width: 28,
            min_control_height: 28,
            viewport_edge_margin: 24,
        }
    }
}

/// Corner radii in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiRadiusTokens {
    pub tiled: u16,
    pub floating: u16,
    pub overlay: u16,
    pub context_menu: u16,
}

impl Default for UiRadiusTokens {
    fn default() -> Self {
        Self {
            tiled: 0,
            floating: 10,
            overlay: 10,
            context_menu: 8,
        }
    }
}

/// One renderer-neutral shadow in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiShadow {
    pub offset_x: i16,
    pub offset_y: i16,
    pub blur_radius: u16,
    pub color: UiColor,
}

impl UiShadow {
    pub const fn new(offset_x: i16, offset_y: i16, blur_radius: u16, color: UiColor) -> Self {
        Self {
            offset_x,
            offset_y,
            blur_radius,
            color,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiElevationTokens {
    pub raised: [UiShadow; 2],
}

impl Default for UiElevationTokens {
    fn default() -> Self {
        Self {
            raised: [
                UiShadow::new(0, 18, 48, UiColor::rgba(0, 0, 0, 115)),
                UiShadow::new(0, 2, 8, UiColor::rgba(0, 0, 0, 71)),
            ],
        }
    }
}

/// Unit opacity represented as thousandths to retain deterministic equality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct UiOpacity(u16);

impl UiOpacity {
    pub const fn new(thousandths: u16) -> Option<Self> {
        if thousandths <= 1_000 {
            Some(Self(thousandths))
        } else {
            None
        }
    }

    const fn checked(thousandths: u16) -> Self {
        assert!(thousandths <= 1_000);
        Self(thousandths)
    }

    pub const fn thousandths(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for UiOpacity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u16::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            serde::de::Error::custom(format!("opacity {value} exceeds 1000 thousandths"))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiOpacityTokens {
    pub overlay_enter_start: UiOpacity,
    pub overlay_enter_end: UiOpacity,
}

impl Default for UiOpacityTokens {
    fn default() -> Self {
        Self {
            overlay_enter_start: UiOpacity::checked(985),
            overlay_enter_end: UiOpacity::checked(1_000),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSelectionTokens {
    pub leading_indicator_width: u16,
}

impl Default for UiSelectionTokens {
    fn default() -> Self {
        Self {
            leading_indicator_width: 2,
        }
    }
}

/// CSS-like cubic-bezier control points represented as thousandths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiCubicBezier {
    pub x1_thousandths: i16,
    pub y1_thousandths: i16,
    pub x2_thousandths: i16,
    pub y2_thousandths: i16,
}

impl UiCubicBezier {
    pub const fn new(
        x1_thousandths: i16,
        y1_thousandths: i16,
        x2_thousandths: i16,
        y2_thousandths: i16,
    ) -> Self {
        Self {
            x1_thousandths,
            y1_thousandths,
            x2_thousandths,
            y2_thousandths,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiMotionToken {
    pub duration_ms: u16,
    pub easing: Option<UiCubicBezier>,
}

impl UiMotionToken {
    pub const fn new(duration_ms: u16, easing: UiCubicBezier) -> Self {
        Self {
            duration_ms,
            easing: Some(easing),
        }
    }

    pub const fn direct() -> Self {
        Self {
            duration_ms: 0,
            easing: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiMotionTokens {
    pub press_hover: UiMotionToken,
    pub focus_selection: UiMotionToken,
    pub overlay_enter: UiMotionToken,
    pub overlay_exit: UiMotionToken,
    pub pane_change: UiMotionToken,
    pub direct: UiMotionToken,
}

impl Default for UiMotionTokens {
    fn default() -> Self {
        let standard = UiCubicBezier::new(200, 0, 0, 1_000);
        Self {
            press_hover: UiMotionToken::new(80, standard),
            focus_selection: UiMotionToken::new(120, standard),
            overlay_enter: UiMotionToken::new(180, UiCubicBezier::new(160, 1_000, 300, 1_000)),
            overlay_exit: UiMotionToken::new(110, UiCubicBezier::new(400, 0, 1_000, 1_000)),
            pane_change: UiMotionToken::new(140, standard),
            direct: UiMotionToken::direct(),
        }
    }
}

/// Complete renderer-neutral native UI token set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiTokens {
    pub palette: UiPalette,
    pub typography: UiTypographyTokens,
    pub spacing: UiSpacingTokens,
    pub radii: UiRadiusTokens,
    pub elevation: UiElevationTokens,
    pub opacity: UiOpacityTokens,
    pub selection: UiSelectionTokens,
    pub motion: UiMotionTokens,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiContrastKind {
    NormalText,
    StateCue,
}

/// One fully resolved foreground/background pair used by app-owned native UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedUiContrastPair {
    pub name: &'static str,
    pub kind: UiContrastKind,
    pub foreground: UiColor,
    pub background: UiColor,
    pub minimum_ratio_milli: u32,
    pub actual_ratio_milli: u32,
}

impl ResolvedUiContrastPair {
    pub const fn passes(&self) -> bool {
        self.actual_ratio_milli >= self.minimum_ratio_milli
    }
}

/// Named color for every themable role in the workspace chrome.
///
/// `Theme::default()` is the built-in `mandatum-dark` theme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    /// Native materialization of terminal defaults and ANSI colors. Cell
    /// programs retain their abstract [`SceneColor`] values.
    pub terminal_palette: TerminalPalette,
    /// Native UI values. These direct colors and metrics never resolve
    /// through [`TerminalPalette`].
    pub ui: UiTokens,
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
            "mandatum-dark" | "dark" => Some(mandatum_dark()),
            "mandatum-light" | "light" => Some(mandatum_light()),
            "mandatum-high-contrast" | "high-contrast" | "high_contrast" => {
                Some(mandatum_high_contrast())
            }
            _ => None,
        }
    }

    /// The names of every built-in theme, for error messages.
    pub const BUILTIN_NAMES: &'static [&'static str] =
        &["mandatum-dark", "mandatum-light", "mandatum-high-contrast"];

    /// Resolve the actual app-owned foreground/background pairs native UI
    /// paints. Child-terminal ANSI combinations are intentionally absent.
    pub fn resolved_ui_contrast(&self) -> Vec<ResolvedUiContrastPair> {
        let palette = self.ui.palette;
        let normal_text_minimum = if self.name == "mandatum-high-contrast" {
            7_000
        } else {
            4_500
        };
        let mut report = Vec::new();
        let mut add = |name, kind, foreground, background, minimum_ratio_milli| {
            report.push(ResolvedUiContrastPair {
                name,
                kind,
                foreground,
                background,
                minimum_ratio_milli,
                actual_ratio_milli: contrast_ratio_milli(foreground, background),
            });
        };

        for (name, foreground) in [
            ("primary text", palette.text_primary),
            ("secondary text", palette.text_secondary),
            ("muted text", palette.text_muted),
        ] {
            for (surface, background) in [
                ("canvas", palette.canvas),
                ("pane", palette.pane_surface),
                ("chrome", palette.chrome_surface),
                ("overlay", palette.overlay_surface),
            ] {
                let pair_name = match (name, surface) {
                    ("primary text", "canvas") => "primary text on canvas",
                    ("primary text", "pane") => "primary text on pane",
                    ("primary text", "chrome") => "primary text on chrome",
                    ("primary text", "overlay") => "primary text on overlay",
                    ("secondary text", "canvas") => "secondary text on canvas",
                    ("secondary text", "pane") => "secondary text on pane",
                    ("secondary text", "chrome") => "secondary text on chrome",
                    ("secondary text", "overlay") => "secondary text on overlay",
                    ("muted text", "canvas") => "muted text on canvas",
                    ("muted text", "pane") => "muted text on pane",
                    ("muted text", "chrome") => "muted text on chrome",
                    ("muted text", "overlay") => "muted text on overlay",
                    _ => unreachable!(),
                };
                add(
                    pair_name,
                    UiContrastKind::NormalText,
                    foreground,
                    background,
                    normal_text_minimum,
                );
            }
        }

        for (name, foreground) in [
            ("running", palette.running),
            ("waiting", palette.waiting),
            ("failure", palette.failure),
            ("complete", palette.complete),
            ("agent identity", palette.agent_identity),
        ] {
            for (surface, background) in [
                ("pane", palette.pane_surface),
                ("chrome", palette.chrome_surface),
                ("overlay", palette.overlay_surface),
            ] {
                let pair_name = match (name, surface) {
                    ("running", "pane") => "running text on pane",
                    ("running", "chrome") => "running text on chrome",
                    ("running", "overlay") => "running text on overlay",
                    ("waiting", "pane") => "waiting text on pane",
                    ("waiting", "chrome") => "waiting text on chrome",
                    ("waiting", "overlay") => "waiting text on overlay",
                    ("failure", "pane") => "failure text on pane",
                    ("failure", "chrome") => "failure text on chrome",
                    ("failure", "overlay") => "failure text on overlay",
                    ("complete", "pane") => "complete text on pane",
                    ("complete", "chrome") => "complete text on chrome",
                    ("complete", "overlay") => "complete text on overlay",
                    ("agent identity", "pane") => "agent identity text on pane",
                    ("agent identity", "chrome") => "agent identity text on chrome",
                    ("agent identity", "overlay") => "agent identity text on overlay",
                    _ => unreachable!(),
                };
                add(
                    pair_name,
                    UiContrastKind::NormalText,
                    foreground,
                    background,
                    normal_text_minimum,
                );
            }
        }

        for (name, foreground) in [
            ("focus", palette.focus),
            ("strong boundary", palette.border_strong),
        ] {
            for (surface, background) in [
                ("canvas", palette.canvas),
                ("pane", palette.pane_surface),
                ("chrome", palette.chrome_surface),
                ("overlay", palette.overlay_surface),
            ] {
                let pair_name = match (name, surface) {
                    ("focus", "canvas") => "focus cue on canvas",
                    ("focus", "pane") => "focus cue on pane",
                    ("focus", "chrome") => "focus cue on chrome",
                    ("focus", "overlay") => "focus cue on overlay",
                    ("strong boundary", "canvas") => "strong boundary on canvas",
                    ("strong boundary", "pane") => "strong boundary on pane",
                    ("strong boundary", "chrome") => "strong boundary on chrome",
                    ("strong boundary", "overlay") => "strong boundary on overlay",
                    _ => unreachable!(),
                };
                add(
                    pair_name,
                    UiContrastKind::StateCue,
                    foreground,
                    background,
                    3_000,
                );
            }
        }

        report
    }

    pub fn ui_contrast_violations(&self) -> Vec<ResolvedUiContrastPair> {
        self.resolved_ui_contrast()
            .into_iter()
            .filter(|pair| !pair.passes())
            .collect()
    }
}

fn mandatum_dark() -> Theme {
    Theme {
        name: "mandatum-dark".to_owned(),
        terminal_palette: TerminalPalette::default(),
        ui: UiTokens::default(),
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
        terminal_palette: light_terminal_palette(),
        ui: UiTokens {
            palette: light_palette(),
            ..UiTokens::default()
        },
        focus_title: SceneColor::Ansi(4), // blue
        pane_border: SceneColor::Ansi(7), // gray
        pane_title: SceneColor::Default,
        header: SceneColor::Ansi(0), // near-black
        header_background: SceneColor::Rgb(0xe9, 0xed, 0xf2),
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
        terminal_palette: high_contrast_terminal_palette(),
        ui: UiTokens {
            palette: high_contrast_palette(),
            ..UiTokens::default()
        },
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
        overlay_background: SceneColor::Rgb(0x00, 0x20, 0x60),
        palette_selection: SceneColor::Default,
        selection_highlight: SceneColor::Default,
        agent_running: SceneColor::Ansi(10),  // bright green
        agent_waiting: SceneColor::Ansi(11),  // bright yellow
        agent_failed: SceneColor::Ansi(9),    // bright red
        agent_complete: SceneColor::Ansi(14), // bright cyan
        agent_idle: SceneColor::Ansi(15),
    }
}

const fn light_terminal_palette() -> TerminalPalette {
    TerminalPalette {
        foreground: [0x1b, 0x24, 0x30],
        background: [0xff, 0xff, 0xff],
        ansi: [
            [0x1b, 0x24, 0x30],
            [0xb4, 0x23, 0x3b],
            [0x17, 0x66, 0x3c],
            [0x7a, 0x4b, 0x00],
            [0x1d, 0x5f, 0xd1],
            [0x67, 0x42, 0xa0],
            [0x00, 0x6b, 0x70],
            [0x80, 0x8b, 0x9c],
            [0x46, 0x54, 0x67],
            [0xcf, 0x33, 0x4c],
            [0x1e, 0x7a, 0x4b],
            [0x8f, 0x5b, 0x00],
            [0x32, 0x69, 0xd2],
            [0x7c, 0x52, 0xb7],
            [0x00, 0x77, 0x7c],
            [0x6e, 0x77, 0x85],
        ],
    }
}

const fn high_contrast_terminal_palette() -> TerminalPalette {
    TerminalPalette {
        foreground: [0xff, 0xff, 0xff],
        background: [0x00, 0x00, 0x00],
        ansi: [
            [0x00, 0x00, 0x00],
            [0xff, 0x6b, 0x6b],
            [0x5c, 0xff, 0x9d],
            [0xff, 0xff, 0x00],
            [0x78, 0xa9, 0xff],
            [0xff, 0x9f, 0xff],
            [0x66, 0xff, 0xff],
            [0xe6, 0xe6, 0xe6],
            [0x80, 0x80, 0x80],
            [0xff, 0x6b, 0x6b],
            [0x5c, 0xff, 0x9d],
            [0xff, 0xff, 0x00],
            [0x78, 0xa9, 0xff],
            [0xff, 0x9f, 0xff],
            [0x66, 0xff, 0xff],
            [0xff, 0xff, 0xff],
        ],
    }
}

const fn flagship_dark_palette() -> UiPalette {
    UiPalette {
        canvas: UiColor::rgb(0x0b, 0x0d, 0x10),
        pane_surface: UiColor::rgb(0x10, 0x14, 0x1a),
        chrome_surface: UiColor::rgb(0x17, 0x1b, 0x23),
        overlay_surface: UiColor::rgb(0x1c, 0x21, 0x2b),
        border_subtle: UiColor::rgb(0x2a, 0x31, 0x3d),
        border_strong: UiColor::rgb(0x5e, 0x6c, 0x84),
        text_primary: UiColor::rgb(0xe7, 0xea, 0xf0),
        text_secondary: UiColor::rgb(0xaa, 0xb2, 0xc0),
        text_muted: UiColor::rgb(0x8d, 0x97, 0xa8),
        focus: UiColor::rgb(0x78, 0xa9, 0xff),
        running: UiColor::rgb(0x72, 0xd6, 0xa0),
        waiting: UiColor::rgb(0xf2, 0xc6, 0x6d),
        failure: UiColor::rgb(0xff, 0x7a, 0x8a),
        complete: UiColor::rgb(0x73, 0xd8, 0xd4),
        agent_identity: UiColor::rgb(0xc0, 0xa7, 0xf2),
        selection_fill: UiColor::rgba(0x78, 0xa9, 0xff, 36),
        modal_scrim: UiColor::rgba(0x05, 0x07, 0x0a, 140),
    }
}

const fn light_palette() -> UiPalette {
    UiPalette {
        canvas: UiColor::rgb(0xf4, 0xf6, 0xf8),
        pane_surface: UiColor::rgb(0xff, 0xff, 0xff),
        chrome_surface: UiColor::rgb(0xe9, 0xed, 0xf2),
        overlay_surface: UiColor::rgb(0xff, 0xff, 0xff),
        border_subtle: UiColor::rgb(0xd1, 0xd7, 0xe0),
        border_strong: UiColor::rgb(0x52, 0x60, 0x79),
        text_primary: UiColor::rgb(0x1b, 0x24, 0x30),
        text_secondary: UiColor::rgb(0x46, 0x54, 0x67),
        text_muted: UiColor::rgb(0x5e, 0x6b, 0x7d),
        focus: UiColor::rgb(0x1d, 0x5f, 0xd1),
        running: UiColor::rgb(0x17, 0x66, 0x3c),
        waiting: UiColor::rgb(0x7a, 0x4b, 0x00),
        failure: UiColor::rgb(0xb4, 0x23, 0x3b),
        complete: UiColor::rgb(0x00, 0x6b, 0x70),
        agent_identity: UiColor::rgb(0x67, 0x42, 0xa0),
        selection_fill: UiColor::rgba(0x1d, 0x5f, 0xd1, 31),
        modal_scrim: UiColor::rgba(0x0b, 0x0d, 0x10, 82),
    }
}

const fn high_contrast_palette() -> UiPalette {
    UiPalette {
        canvas: UiColor::rgb(0x00, 0x00, 0x00),
        pane_surface: UiColor::rgb(0x00, 0x00, 0x00),
        chrome_surface: UiColor::rgb(0x00, 0x00, 0x00),
        overlay_surface: UiColor::rgb(0x00, 0x00, 0x00),
        border_subtle: UiColor::rgb(0xe6, 0xe6, 0xe6),
        border_strong: UiColor::rgb(0xff, 0xff, 0xff),
        text_primary: UiColor::rgb(0xff, 0xff, 0xff),
        text_secondary: UiColor::rgb(0xff, 0xff, 0xff),
        text_muted: UiColor::rgb(0xe6, 0xe6, 0xe6),
        focus: UiColor::rgb(0xff, 0xff, 0x00),
        running: UiColor::rgb(0x5c, 0xff, 0x9d),
        waiting: UiColor::rgb(0xff, 0xff, 0x00),
        failure: UiColor::rgb(0xff, 0x6b, 0x6b),
        complete: UiColor::rgb(0x66, 0xff, 0xff),
        agent_identity: UiColor::rgb(0xff, 0x9f, 0xff),
        selection_fill: UiColor::rgba(0xff, 0xff, 0x00, 64),
        modal_scrim: UiColor::rgba(0x00, 0x00, 0x00, 191),
    }
}

fn contrast_ratio_milli(foreground: UiColor, background: UiColor) -> u32 {
    let alpha = f64::from(foreground.alpha) / 255.0;
    let blended = UiColor::rgb(
        blend_channel(foreground.red, background.red, alpha),
        blend_channel(foreground.green, background.green, alpha),
        blend_channel(foreground.blue, background.blue, alpha),
    );
    let foreground_luminance = relative_luminance(blended);
    let background_luminance = relative_luminance(background);
    let lighter = foreground_luminance.max(background_luminance);
    let darker = foreground_luminance.min(background_luminance);
    (((lighter + 0.05) / (darker + 0.05)) * 1_000.0).floor() as u32
}

fn blend_channel(foreground: u8, background: u8, alpha: f64) -> u8 {
    (f64::from(foreground) * alpha + f64::from(background) * (1.0 - alpha)).round() as u8
}

fn relative_luminance(color: UiColor) -> f64 {
    fn linear(channel: u8) -> f64 {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(color.red) + 0.7152 * linear(color.green) + 0.0722 * linear(color.blue)
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
    fn dark_ui_palette_matches_the_flagship_native_contract() {
        let colors = Theme::default().ui.palette;

        assert_eq!(colors.canvas, UiColor::rgb(0x0b, 0x0d, 0x10));
        assert_eq!(colors.pane_surface, UiColor::rgb(0x10, 0x14, 0x1a));
        assert_eq!(colors.chrome_surface, UiColor::rgb(0x17, 0x1b, 0x23));
        assert_eq!(colors.overlay_surface, UiColor::rgb(0x1c, 0x21, 0x2b));
        assert_eq!(colors.border_subtle, UiColor::rgb(0x2a, 0x31, 0x3d));
        assert_eq!(colors.border_strong, UiColor::rgb(0x5e, 0x6c, 0x84));
        assert_eq!(colors.text_primary, UiColor::rgb(0xe7, 0xea, 0xf0));
        assert_eq!(colors.text_secondary, UiColor::rgb(0xaa, 0xb2, 0xc0));
        assert_eq!(colors.text_muted, UiColor::rgb(0x8d, 0x97, 0xa8));
        assert_eq!(colors.focus, UiColor::rgb(0x78, 0xa9, 0xff));
        assert_eq!(colors.running, UiColor::rgb(0x72, 0xd6, 0xa0));
        assert_eq!(colors.waiting, UiColor::rgb(0xf2, 0xc6, 0x6d));
        assert_eq!(colors.failure, UiColor::rgb(0xff, 0x7a, 0x8a));
        assert_eq!(colors.complete, UiColor::rgb(0x73, 0xd8, 0xd4));
        assert_eq!(colors.agent_identity, UiColor::rgb(0xc0, 0xa7, 0xf2));
        assert_eq!(colors.selection_fill, UiColor::rgba(0x78, 0xa9, 0xff, 36));
        assert_eq!(colors.modal_scrim, UiColor::rgba(0x05, 0x07, 0x0a, 140));
    }

    #[test]
    fn ui_foundation_tokens_match_the_logical_pixel_and_motion_contract() {
        let ui = Theme::default().ui;

        assert_eq!(
            ui.typography.terminal,
            UiTextStyle::new(13 * 64, 17, UiFontFace::Regular)
        );
        assert_eq!(
            ui.typography.title,
            UiTextStyle::new(13 * 64, 18, UiFontFace::Bold)
        );
        assert_eq!(
            ui.typography.pane_title,
            UiTextStyle::new(12 * 64, 17, UiFontFace::Regular)
        );
        assert_eq!(
            ui.typography.pane_title_focused,
            UiTextStyle::new(12 * 64, 17, UiFontFace::Bold)
        );
        assert_eq!(
            ui.typography.body,
            UiTextStyle::new(12 * 64 + 32, 18, UiFontFace::Regular)
        );
        assert_eq!(
            ui.typography.metadata,
            UiTextStyle::new(11 * 64, 15, UiFontFace::Regular)
        );
        assert_eq!(ui.typography.key, ui.typography.metadata);

        assert_eq!(
            [
                ui.spacing.space_1,
                ui.spacing.space_2,
                ui.spacing.space_3,
                ui.spacing.space_4,
                ui.spacing.space_6,
            ],
            [4, 8, 12, 16, 24]
        );
        assert_eq!(ui.spacing.pane_title_inset_x, 8);
        assert_eq!(ui.spacing.pane_title_inset_y, 4);
        assert_eq!(ui.spacing.workflow_section_gap, 12);
        assert_eq!(ui.spacing.overlay_outer_padding, 16);
        assert_eq!(ui.spacing.overlay_row_padding_x, 8);
        assert_eq!(ui.spacing.overlay_row_padding_y, 6);
        assert_eq!(ui.spacing.tiled_separator, 1);
        assert_eq!(ui.spacing.separator_target, 6);
        assert_eq!(ui.spacing.min_control_width, 28);
        assert_eq!(ui.spacing.min_control_height, 28);
        assert_eq!(ui.spacing.viewport_edge_margin, 24);

        assert_eq!(ui.radii.tiled, 0);
        assert_eq!(ui.radii.floating, 10);
        assert_eq!(ui.radii.overlay, 10);
        assert_eq!(ui.radii.context_menu, 8);
        assert_eq!(
            ui.elevation.raised,
            [
                UiShadow::new(0, 18, 48, UiColor::rgba(0, 0, 0, 115)),
                UiShadow::new(0, 2, 8, UiColor::rgba(0, 0, 0, 71)),
            ]
        );
        assert_eq!(ui.opacity.overlay_enter_start.thousandths(), 985);
        assert_eq!(ui.opacity.overlay_enter_end.thousandths(), 1_000);
        assert_eq!(ui.selection.leading_indicator_width, 2);

        assert_eq!(ui.motion.press_hover.duration_ms, 80);
        assert_eq!(ui.motion.focus_selection.duration_ms, 120);
        assert_eq!(ui.motion.overlay_enter.duration_ms, 180);
        assert_eq!(ui.motion.overlay_exit.duration_ms, 110);
        assert_eq!(ui.motion.pane_change.duration_ms, 140);
        assert_eq!(ui.motion.direct.duration_ms, 0);
        assert_eq!(ui.motion.direct.easing, None);
        assert_eq!(
            ui.motion.overlay_enter.easing,
            Some(UiCubicBezier::new(160, 1_000, 300, 1_000))
        );
    }

    #[test]
    fn builtins_resolve_every_app_owned_native_contrast_pair() {
        for name in Theme::BUILTIN_NAMES {
            let theme = Theme::builtin(name).unwrap();
            let report = theme.resolved_ui_contrast();
            assert!(!report.is_empty());
            assert!(
                theme.ui_contrast_violations().is_empty(),
                "{name}: {:?}",
                theme.ui_contrast_violations()
            );
            assert!(report.iter().all(ResolvedUiContrastPair::passes));
        }

        let dark = Theme::default();
        let muted_overlay = dark
            .resolved_ui_contrast()
            .into_iter()
            .find(|pair| pair.name == "muted text on overlay")
            .unwrap();
        assert_eq!(muted_overlay.minimum_ratio_milli, 4_500);
        assert!(muted_overlay.actual_ratio_milli >= 4_500);

        let high_contrast = Theme::builtin("mandatum-high-contrast").unwrap();
        assert!(
            high_contrast
                .resolved_ui_contrast()
                .iter()
                .filter(|pair| pair.kind == UiContrastKind::NormalText)
                .all(|pair| pair.minimum_ratio_milli == 7_000)
        );
    }

    #[test]
    fn light_and_high_contrast_terminal_projection_meet_app_owned_contrast_contract() {
        for name in ["mandatum-light", "mandatum-high-contrast"] {
            let theme = Theme::builtin(name).unwrap();
            let text_minimum = if name == "mandatum-high-contrast" {
                7_000
            } else {
                4_500
            };
            let terminal_background =
                resolve_scene_color(&theme, SceneColor::Default, PaletteDefault::Background);
            let header_background =
                resolve_scene_color(&theme, theme.header_background, PaletteDefault::Background);
            let overlay_background =
                resolve_scene_color(&theme, theme.overlay_background, PaletteDefault::Background);

            for (pair_name, foreground, background) in [
                (
                    "terminal default text",
                    SceneColor::Default,
                    terminal_background,
                ),
                ("pane title", theme.pane_title, terminal_background),
                ("focused pane title", theme.focus_title, terminal_background),
                ("header", theme.header, header_background),
                ("header attention", theme.attention, header_background),
                ("status", theme.status, terminal_background),
                ("overlay text", theme.overlay_foreground, overlay_background),
                ("running state", theme.agent_running, terminal_background),
                ("waiting state", theme.agent_waiting, terminal_background),
                ("failure state", theme.agent_failed, terminal_background),
                ("complete state", theme.agent_complete, terminal_background),
                ("idle state", theme.agent_idle, terminal_background),
            ] {
                assert_scene_contrast(&theme, pair_name, foreground, background, text_minimum);
            }

            for (pair_name, foreground, background) in [
                ("pane border", theme.pane_border, terminal_background),
                ("palette border", theme.palette_border, overlay_background),
            ] {
                assert_scene_contrast(&theme, pair_name, foreground, background, 3_000);
            }
        }
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
    fn concise_builtin_aliases_resolve_to_canonical_theme_names() {
        for (alias, canonical) in [
            ("dark", "mandatum-dark"),
            ("light", "mandatum-light"),
            ("high-contrast", "mandatum-high-contrast"),
            ("high_contrast", "mandatum-high-contrast"),
        ] {
            assert_eq!(Theme::builtin(alias).unwrap().name, canonical);
        }
    }

    #[test]
    fn builtins_own_theme_specific_terminal_palettes_without_changing_dark() {
        let expected = TerminalPalette::default();
        assert_eq!(expected.foreground, [220, 220, 224]);
        assert_eq!(expected.background, [18, 18, 22]);
        assert_eq!(
            expected.ansi,
            [
                [0, 0, 0],
                [205, 49, 49],
                [13, 188, 121],
                [229, 229, 16],
                [36, 114, 200],
                [188, 63, 188],
                [17, 168, 205],
                [229, 229, 229],
                [128, 128, 128],
                [241, 76, 76],
                [35, 209, 139],
                [245, 245, 67],
                [59, 142, 234],
                [214, 112, 214],
                [41, 184, 219],
                [255, 255, 255],
            ]
        );
        let dark = Theme::builtin("mandatum-dark").unwrap().terminal_palette;
        let light_theme = Theme::builtin("mandatum-light").unwrap();
        let light = light_theme.terminal_palette;
        let high_contrast_theme = Theme::builtin("mandatum-high-contrast").unwrap();
        let high_contrast = high_contrast_theme.terminal_palette;

        assert_eq!(dark, expected);
        assert_ne!(light, dark);
        assert_ne!(high_contrast, dark);
        assert_ne!(high_contrast, light);

        let light_ui = light_theme.ui.palette;
        assert_eq!(light.foreground, rgb(light_ui.text_primary));
        assert_eq!(light.background, rgb(light_ui.pane_surface));
        assert_eq!(light.ansi[1], rgb(light_ui.failure));
        assert_eq!(light.ansi[2], rgb(light_ui.running));
        assert_eq!(light.ansi[3], rgb(light_ui.waiting));
        assert_eq!(light.ansi[4], rgb(light_ui.focus));
        assert_eq!(light.ansi[5], rgb(light_ui.agent_identity));
        assert_eq!(light.ansi[6], rgb(light_ui.complete));

        let high_contrast_ui = high_contrast_theme.ui.palette;
        assert_eq!(high_contrast.foreground, rgb(high_contrast_ui.text_primary));
        assert_eq!(high_contrast.background, rgb(high_contrast_ui.pane_surface));
        assert_eq!(high_contrast.ansi[9], rgb(high_contrast_ui.failure));
        assert_eq!(high_contrast.ansi[10], rgb(high_contrast_ui.running));
        assert_eq!(high_contrast.ansi[11], rgb(high_contrast_ui.waiting));
        assert_eq!(high_contrast.ansi[11], rgb(high_contrast_ui.focus));
        assert_eq!(high_contrast.ansi[13], rgb(high_contrast_ui.agent_identity));
        assert_eq!(high_contrast.ansi[14], rgb(high_contrast_ui.complete));
    }

    #[test]
    fn light_terminal_bright_white_remains_legible_on_the_default_background() {
        let theme = Theme::builtin("mandatum-light").unwrap();
        let background = theme.terminal_palette.background;

        assert_ne!(theme.terminal_palette.ansi[15], background);
        assert_scene_contrast(
            &theme,
            "ANSI bright white",
            SceneColor::Ansi(15),
            background,
            4_500,
        );
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

    #[derive(Clone, Copy)]
    enum PaletteDefault {
        Foreground,
        Background,
    }

    fn assert_scene_contrast(
        theme: &Theme,
        pair_name: &str,
        foreground: SceneColor,
        background: [u8; 3],
        minimum_ratio_milli: u32,
    ) {
        let foreground = resolve_scene_color(theme, foreground, PaletteDefault::Foreground);
        let actual_ratio_milli = contrast_ratio_milli(ui_color(foreground), ui_color(background));
        assert!(
            actual_ratio_milli >= minimum_ratio_milli,
            "{} {pair_name}: expected at least {:.1}:1, got {:.3}:1",
            theme.name,
            minimum_ratio_milli as f64 / 1_000.0,
            actual_ratio_milli as f64 / 1_000.0
        );
    }

    fn resolve_scene_color(theme: &Theme, color: SceneColor, default: PaletteDefault) -> [u8; 3] {
        match color {
            SceneColor::Default => match default {
                PaletteDefault::Foreground => theme.terminal_palette.foreground,
                PaletteDefault::Background => theme.terminal_palette.background,
            },
            SceneColor::Ansi(index @ 0..=15) => theme.terminal_palette.ansi[index as usize],
            SceneColor::Ansi(index) | SceneColor::Indexed(index) => indexed_color(index),
            SceneColor::Rgb(red, green, blue) => [red, green, blue],
        }
    }

    fn indexed_color(index: u8) -> [u8; 3] {
        match index {
            0..=15 => TerminalPalette::default().ansi[index as usize],
            16..=231 => {
                let cube = index - 16;
                let component = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
                [
                    component(cube / 36),
                    component((cube % 36) / 6),
                    component(cube % 6),
                ]
            }
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                [gray, gray, gray]
            }
        }
    }

    const fn ui_color(rgb: [u8; 3]) -> UiColor {
        UiColor::rgb(rgb[0], rgb[1], rgb[2])
    }

    const fn rgb(color: UiColor) -> [u8; 3] {
        [color.red, color.green, color.blue]
    }
}
