//! Validated native interface typography metrics.

use mandatum_scene::{LogicalRect, Theme, UiFontFace, UiTextStyle};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeFontFace {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl From<UiFontFace> for NativeFontFace {
    fn from(face: UiFontFace) -> Self {
        match face {
            UiFontFace::Regular => Self::Regular,
            UiFontFace::Bold => Self::Bold,
            UiFontFace::Italic => Self::Italic,
            UiFontFace::BoldItalic => Self::BoldItalic,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NativeTextMetricRole {
    Terminal,
    Title,
    PaneTitle,
    PaneTitleFocused,
    Body,
    Metadata,
    Key,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeTextStyle {
    pub point_size_x64: u16,
    pub line_height_units: u64,
    pub face: NativeFontFace,
}

impl From<UiTextStyle> for NativeTextStyle {
    fn from(style: UiTextStyle) -> Self {
        Self {
            point_size_x64: style.point_size_x64,
            line_height_units: u64::from(style.line_height) * 64,
            face: style.face.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeTextMetricIdentity {
    pub generation: u64,
    pub role: NativeTextMetricRole,
    pub style: NativeTextStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeTextMetricSet {
    generation: u64,
    terminal: NativeTextStyle,
    title: NativeTextStyle,
    pane_title: NativeTextStyle,
    pane_title_focused: NativeTextStyle,
    body: NativeTextStyle,
    metadata: NativeTextStyle,
    key: NativeTextStyle,
}

impl NativeTextMetricSet {
    pub fn from_theme(theme: &Theme, generation: u64) -> Self {
        let typography = theme.ui.typography;
        Self {
            generation,
            terminal: typography.terminal.into(),
            title: typography.title.into(),
            pane_title: typography.pane_title.into(),
            pane_title_focused: typography.pane_title_focused.into(),
            body: typography.body.into(),
            metadata: typography.metadata.into(),
            key: typography.key.into(),
        }
    }

    pub fn identity(self, role: NativeTextMetricRole) -> NativeTextMetricIdentity {
        NativeTextMetricIdentity {
            generation: self.generation,
            role,
            style: self.style(role),
        }
    }

    pub fn style(self, role: NativeTextMetricRole) -> NativeTextStyle {
        match role {
            NativeTextMetricRole::Terminal => self.terminal,
            NativeTextMetricRole::Title => self.title,
            NativeTextMetricRole::PaneTitle => self.pane_title,
            NativeTextMetricRole::PaneTitleFocused => self.pane_title_focused,
            NativeTextMetricRole::Body => self.body,
            NativeTextMetricRole::Metadata => self.metadata,
            NativeTextMetricRole::Key => self.key,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeTextMetricError {
    ZeroFontSize,
    InvalidLineHeight,
    InvalidAscent,
    BaselineOutsideClip,
    GeometryOverflow,
}

/// One fixed-point interface-text metric profile.
///
/// Values use the scene contract's 1/64-logical-pixel unit. Keeping retained
/// identity integral avoids repeated float rounding across frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeTextMetrics {
    pub font_size_units: u64,
    pub line_height_units: u64,
    pub ascent_units: u64,
}

impl NativeTextMetrics {
    pub fn from_units(
        font_size_units: u64,
        line_height_units: u64,
        ascent_units: u64,
    ) -> Result<Self, NativeTextMetricError> {
        if font_size_units == 0 {
            return Err(NativeTextMetricError::ZeroFontSize);
        }
        if line_height_units < font_size_units {
            return Err(NativeTextMetricError::InvalidLineHeight);
        }
        if ascent_units == 0 || ascent_units > line_height_units {
            return Err(NativeTextMetricError::InvalidAscent);
        }
        Ok(Self {
            font_size_units,
            line_height_units,
            ascent_units,
        })
    }

    /// Place this line box inside `clip`, optionally sharing a baseline with
    /// an already placed metric profile.
    pub fn align_baseline(
        self,
        clip: LogicalRect,
        shared_baseline_y_units: Option<i64>,
    ) -> Result<NativeTextPlacement, NativeTextMetricError> {
        if self.line_height_units > clip.size.height_units() {
            return Err(NativeTextMetricError::BaselineOutsideClip);
        }
        let clip_top = clip.origin.y_units();
        let top = if let Some(baseline) = shared_baseline_y_units {
            baseline
                .checked_sub_unsigned(self.ascent_units)
                .ok_or(NativeTextMetricError::GeometryOverflow)?
        } else {
            let inset = (clip.size.height_units() - self.line_height_units) / 2;
            clip_top
                .checked_add_unsigned(inset)
                .ok_or(NativeTextMetricError::GeometryOverflow)?
        };
        let bottom = top
            .checked_add_unsigned(self.line_height_units)
            .ok_or(NativeTextMetricError::GeometryOverflow)?;
        if top < clip_top || bottom > clip.bottom_units() {
            return Err(NativeTextMetricError::BaselineOutsideClip);
        }
        let baseline_y_units = top
            .checked_add_unsigned(self.ascent_units)
            .ok_or(NativeTextMetricError::GeometryOverflow)?;
        Ok(NativeTextPlacement {
            logical_rect: LogicalRect::from_units(
                clip.origin.x_units(),
                top,
                clip.size.width_units(),
                self.line_height_units,
            ),
            baseline_y_units,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeTextPlacement {
    pub logical_rect: LogicalRect,
    pub baseline_y_units: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandatum_scene::LogicalRect;

    #[test]
    fn mixed_metrics_align_to_one_baseline_without_escaping_the_clip() {
        let clip = LogicalRect::from_units(640, 1_280, 12_800, 2_560);
        let body = NativeTextMetrics::from_units(13 * 64, 18 * 64, 14 * 64).unwrap();
        let caption = NativeTextMetrics::from_units(11 * 64, 14 * 64, 11 * 64).unwrap();

        let body_placement = body.align_baseline(clip, None).unwrap();
        let caption_placement = caption
            .align_baseline(clip, Some(body_placement.baseline_y_units))
            .unwrap();

        assert_eq!(
            body_placement.baseline_y_units,
            caption_placement.baseline_y_units
        );
        assert!(body_placement.logical_rect.bottom_units() <= clip.bottom_units());
        assert!(caption_placement.logical_rect.bottom_units() <= clip.bottom_units());
    }

    #[test]
    fn metric_profiles_and_shared_baselines_fail_closed() {
        assert_eq!(
            NativeTextMetrics::from_units(0, 18 * 64, 14 * 64),
            Err(NativeTextMetricError::ZeroFontSize)
        );
        assert_eq!(
            NativeTextMetrics::from_units(14 * 64, 13 * 64, 11 * 64),
            Err(NativeTextMetricError::InvalidLineHeight)
        );
        assert_eq!(
            NativeTextMetrics::from_units(11 * 64, 14 * 64, 15 * 64),
            Err(NativeTextMetricError::InvalidAscent)
        );

        let clip = LogicalRect::from_units(0, 0, 6_400, 14 * 64);
        let caption = NativeTextMetrics::from_units(11 * 64, 14 * 64, 11 * 64).unwrap();
        assert_eq!(
            caption.align_baseline(clip, Some(10 * 64)),
            Err(NativeTextMetricError::BaselineOutsideClip)
        );
    }

    #[test]
    fn theme_metric_identity_includes_generation_role_size_height_and_face() {
        let theme = mandatum_scene::Theme::default();
        let first = NativeTextMetricSet::from_theme(&theme, 41);
        let next = NativeTextMetricSet::from_theme(&theme, 42);

        assert_ne!(
            first.identity(NativeTextMetricRole::Body),
            next.identity(NativeTextMetricRole::Body)
        );
        assert_ne!(
            first.identity(NativeTextMetricRole::Body),
            first.identity(NativeTextMetricRole::Metadata)
        );
        assert_eq!(
            first.identity(NativeTextMetricRole::Metadata).style,
            first.identity(NativeTextMetricRole::Key).style
        );
        assert_ne!(
            first.identity(NativeTextMetricRole::Metadata).role,
            first.identity(NativeTextMetricRole::Key).role,
            "equal values in distinct semantic slots remain distinct cache identities"
        );
    }
}
