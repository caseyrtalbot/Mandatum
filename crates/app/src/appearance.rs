//! The appearance overlay: adjustment semantics and scene assembly.
//!
//! Rows operate on the live [`Theme`]: the base theme cycles through the
//! built-ins, and the terminal background adjusts by HSL channel. Every
//! adjustment is a plain mutation of app state; the overlay scene is rebuilt
//! from the same state each frame, so the controls can never disagree with
//! what the workspace shows.

use mandatum_scene::color::{Hsl, hsl_to_rgb, rgb_to_hsl};
use mandatum_scene::{
    AppearanceControl, AppearanceOverlay, AppearanceRow, SceneSize, Theme, layout,
};

/// One Left/Right hue step, chosen so a full lap is 30 keypresses.
const HUE_STEP_DEGREES: u16 = 12;
/// One Left/Right saturation or lightness step (thousandths).
const PERCENT_STEP_THOUSANDTHS: u16 = 50;
/// Gradient stops per bar; painters spread these across the bar's cells.
const BAR_STOPS: usize = 24;

/// The open appearance overlay's transient view state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AppearanceViewState {
    pub(crate) selected: usize,
}

/// The adjustable rows, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppearanceField {
    ThemeName,
    BackgroundHue,
    BackgroundSaturation,
    BackgroundLightness,
}

pub(crate) const APPEARANCE_FIELDS: &[AppearanceField] = &[
    AppearanceField::ThemeName,
    AppearanceField::BackgroundHue,
    AppearanceField::BackgroundSaturation,
    AppearanceField::BackgroundLightness,
];

/// Apply one Left/Right adjustment to the live theme and describe the result
/// for the status line.
pub(crate) fn adjust_theme(theme: &mut Theme, field: AppearanceField, direction: i8) -> String {
    match field {
        AppearanceField::ThemeName => {
            let names = Theme::BUILTIN_NAMES;
            let current = names
                .iter()
                .position(|name| *name == theme.name)
                .unwrap_or(0);
            let next = if direction >= 0 {
                (current + 1) % names.len()
            } else {
                (current + names.len() - 1) % names.len()
            };
            // Selecting a base theme is a complete snapshot, exactly like
            // `[theme] name` in config: earlier per-role adjustments reset.
            if let Some(selected) = Theme::builtin(names[next]) {
                *theme = selected;
            }
            format!("theme {}", theme.name)
        }
        AppearanceField::BackgroundHue => {
            let mut hsl = background_hsl(theme);
            hsl.hue_degrees = if direction >= 0 {
                (hsl.hue_degrees + HUE_STEP_DEGREES) % 360
            } else {
                (hsl.hue_degrees + 360 - HUE_STEP_DEGREES) % 360
            };
            set_background(theme, hsl)
        }
        AppearanceField::BackgroundSaturation => {
            let mut hsl = background_hsl(theme);
            hsl.saturation_thousandths = stepped(
                hsl.saturation_thousandths,
                direction,
                PERCENT_STEP_THOUSANDTHS,
            );
            set_background(theme, hsl)
        }
        AppearanceField::BackgroundLightness => {
            let mut hsl = background_hsl(theme);
            hsl.lightness_thousandths = stepped(
                hsl.lightness_thousandths,
                direction,
                PERCENT_STEP_THOUSANDTHS,
            );
            set_background(theme, hsl)
        }
    }
}

fn stepped(value: u16, direction: i8, step: u16) -> u16 {
    if direction >= 0 {
        value.saturating_add(step).min(1000)
    } else {
        value.saturating_sub(step)
    }
}

fn background_hsl(theme: &Theme) -> Hsl {
    rgb_to_hsl(theme.terminal_palette.background)
}

fn set_background(theme: &mut Theme, hsl: Hsl) -> String {
    let rgb = hsl_to_rgb(hsl);
    theme.terminal_palette.background = rgb;
    format!("background #{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// Assemble the appearance overlay scene from the live theme.
pub(crate) fn appearance_overlay_scene(
    theme: &Theme,
    selected: usize,
    size: SceneSize,
) -> AppearanceOverlay {
    let hsl = background_hsl(theme);
    let rows = APPEARANCE_FIELDS
        .iter()
        .map(|field| appearance_row(theme, hsl, *field))
        .collect::<Vec<_>>();
    let row_count = rows.len() as u16;
    AppearanceOverlay {
        area: layout::appearance_rect(size, row_count),
        rows,
        selected: selected.min(APPEARANCE_FIELDS.len().saturating_sub(1)),
        footer: "←/→ adjust · ↑/↓ select · Esc close".to_owned(),
    }
}

fn appearance_row(theme: &Theme, background: Hsl, field: AppearanceField) -> AppearanceRow {
    match field {
        AppearanceField::ThemeName => AppearanceRow {
            label: "Theme".to_owned(),
            control: AppearanceControl::Cycle {
                value: theme.name.clone(),
            },
        },
        AppearanceField::BackgroundHue => AppearanceRow {
            label: "Background hue".to_owned(),
            control: bar(
                (0..BAR_STOPS).map(|stop| Hsl {
                    hue_degrees: (stop * 359 / (BAR_STOPS - 1)) as u16,
                    ..background
                }),
                u32::from(background.hue_degrees) * 1000 / 359,
                theme,
            ),
        },
        AppearanceField::BackgroundSaturation => AppearanceRow {
            label: "Background saturation".to_owned(),
            control: bar(
                (0..BAR_STOPS).map(|stop| Hsl {
                    saturation_thousandths: (stop * 1000 / (BAR_STOPS - 1)) as u16,
                    ..background
                }),
                u32::from(background.saturation_thousandths),
                theme,
            ),
        },
        AppearanceField::BackgroundLightness => AppearanceRow {
            label: "Background lightness".to_owned(),
            control: bar(
                (0..BAR_STOPS).map(|stop| Hsl {
                    lightness_thousandths: (stop * 1000 / (BAR_STOPS - 1)) as u16,
                    ..background
                }),
                u32::from(background.lightness_thousandths),
                theme,
            ),
        },
    }
}

/// A channel bar previews the exact colors the adjustment would produce given
/// the other channels' current values — truthful, never idealized.
fn bar(
    stops: impl Iterator<Item = Hsl>,
    position_thousandths: u32,
    theme: &Theme,
) -> AppearanceControl {
    AppearanceControl::Bar {
        stops: stops.map(hsl_to_rgb).collect(),
        position_thousandths: position_thousandths.min(1000) as u16,
        swatch: theme.terminal_palette.background,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_row_cycles_through_every_builtin_and_wraps() {
        let mut theme = Theme::default();
        let mut seen = vec![theme.name.clone()];
        for _ in 0..Theme::BUILTIN_NAMES.len() {
            adjust_theme(&mut theme, AppearanceField::ThemeName, 1);
            seen.push(theme.name.clone());
        }
        assert_eq!(seen.first(), seen.last(), "a full lap returns home");
        for name in Theme::BUILTIN_NAMES {
            assert!(seen.iter().any(|s| s == name), "missing builtin {name}");
        }
        adjust_theme(&mut theme, AppearanceField::ThemeName, -1);
        assert_eq!(
            theme.name,
            *Theme::BUILTIN_NAMES.last().unwrap(),
            "left from the first builtin wraps to the last"
        );
    }

    #[test]
    fn background_channels_adjust_clamp_and_report_the_hex_value() {
        let mut theme = Theme::default();
        // Raise saturation so hue changes are visible in RGB.
        for _ in 0..8 {
            adjust_theme(&mut theme, AppearanceField::BackgroundSaturation, 1);
        }
        let before = theme.terminal_palette.background;
        let status = adjust_theme(&mut theme, AppearanceField::BackgroundHue, 1);
        assert_ne!(theme.terminal_palette.background, before);
        assert!(status.starts_with("background #"), "{status}");

        // Lightness pegs at the extremes without panicking.
        for _ in 0..40 {
            adjust_theme(&mut theme, AppearanceField::BackgroundLightness, 1);
        }
        assert_eq!(theme.terminal_palette.background, [255, 255, 255]);
        for _ in 0..40 {
            adjust_theme(&mut theme, AppearanceField::BackgroundLightness, -1);
        }
        assert_eq!(theme.terminal_palette.background, [0, 0, 0]);
    }

    #[test]
    fn overlay_scene_rows_match_the_field_order_and_carry_truthful_swatches() {
        let theme = Theme::default();
        let scene = appearance_overlay_scene(&theme, 2, SceneSize::new(120, 40));
        assert_eq!(scene.rows.len(), APPEARANCE_FIELDS.len());
        assert_eq!(scene.selected, 2);
        assert!(matches!(
            scene.rows[0].control,
            AppearanceControl::Cycle { .. }
        ));
        for row in &scene.rows[1..] {
            let AppearanceControl::Bar { stops, swatch, .. } = &row.control else {
                panic!("background rows are bars");
            };
            assert_eq!(stops.len(), BAR_STOPS);
            assert_eq!(*swatch, theme.terminal_palette.background);
        }
    }

    #[test]
    fn selection_clamps_to_the_last_row() {
        let scene = appearance_overlay_scene(&Theme::default(), 99, SceneSize::new(80, 24));
        assert_eq!(scene.selected, APPEARANCE_FIELDS.len() - 1);
    }
}
