//! Frontend-neutral cell styling.
//!
//! These mirror what the terminal engine's cell style expresses without
//! importing the parser crate (L4): the app converts engine cells into scene
//! cells, and every frontend maps scene styles to its own paint types.

use serde::{Deserialize, Serialize};

/// A renderer-neutral terminal color.
///
/// `Default` means "use the surface default", which a frontend maps to its
/// reset color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SceneColor {
    #[default]
    Default,
    /// One of the 16 standard ANSI colors (`0..=7` normal, `8..=15` bright).
    Ansi(u8),
    /// A 256-color palette index beyond the ANSI range (`16..=255`).
    Indexed(u8),
    /// Direct color.
    Rgb(u8, u8, u8),
}

/// Per-cell styling. All fields default to "off"/[`SceneColor::Default`], so
/// `SceneCellStyle::default()` is a plain, unstyled cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneCellStyle {
    pub foreground: SceneColor,
    pub background: SceneColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}
