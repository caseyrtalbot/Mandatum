//! Direct-color conversion shared by appearance controls and their painter.
//!
//! One implementation serves both sides of the boundary: the app computes
//! adjusted channel values and gradient stops here, and the cell-program
//! painter resolves the same stops into cell backgrounds, so a bar can never
//! disagree with the color it produces.

/// A color as hue (degrees, 0..360), saturation and lightness (thousandths,
/// 0..=1000). Integer thousandths keep round-trips deterministic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hsl {
    pub hue_degrees: u16,
    pub saturation_thousandths: u16,
    pub lightness_thousandths: u16,
}

/// Convert direct RGB to HSL. Achromatic inputs report hue 0.
pub fn rgb_to_hsl(rgb: [u8; 3]) -> Hsl {
    let red = f64::from(rgb[0]) / 255.0;
    let green = f64::from(rgb[1]) / 255.0;
    let blue = f64::from(rgb[2]) / 255.0;
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let lightness = (maximum + minimum) / 2.0;
    let delta = maximum - minimum;

    let (hue, saturation) = if delta <= f64::EPSILON {
        (0.0, 0.0)
    } else {
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if maximum == red {
            60.0 * (((green - blue) / delta).rem_euclid(6.0))
        } else if maximum == green {
            60.0 * ((blue - red) / delta + 2.0)
        } else {
            60.0 * ((red - green) / delta + 4.0)
        };
        (hue, saturation)
    };

    Hsl {
        hue_degrees: (hue.round() as u16).min(359),
        saturation_thousandths: ((saturation * 1000.0).round() as u16).min(1000),
        lightness_thousandths: ((lightness * 1000.0).round() as u16).min(1000),
    }
}

/// Convert HSL back to direct RGB.
pub fn hsl_to_rgb(hsl: Hsl) -> [u8; 3] {
    let hue = f64::from(hsl.hue_degrees % 360);
    let saturation = f64::from(hsl.saturation_thousandths.min(1000)) / 1000.0;
    let lightness = f64::from(hsl.lightness_thousandths.min(1000)) / 1000.0;

    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let secondary = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let base = lightness - chroma / 2.0;
    let (red, green, blue) = match hue as u16 {
        0..=59 => (chroma, secondary, 0.0),
        60..=119 => (secondary, chroma, 0.0),
        120..=179 => (0.0, chroma, secondary),
        180..=239 => (0.0, secondary, chroma),
        240..=299 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    [
        ((red + base) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((green + base) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((blue + base) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// Pick the readable marker color for a bar cell: white on dark, black on
/// light, by relative luminance of the underlying stop.
pub fn marker_on(rgb: [u8; 3]) -> [u8; 3] {
    let luminance =
        0.2126 * f64::from(rgb[0]) + 0.7152 * f64::from(rgb[1]) + 0.0722 * f64::from(rgb[2]);
    if luminance > 128.0 {
        [0, 0, 0]
    } else {
        [255, 255, 255]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trips_primaries_and_grays_exactly() {
        for rgb in [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [0, 255, 255],
            [255, 0, 255],
            [0, 0, 0],
            [255, 255, 255],
            [128, 128, 128],
        ] {
            assert_eq!(hsl_to_rgb(rgb_to_hsl(rgb)), rgb, "round trip for {rgb:?}");
        }
    }

    #[test]
    fn hsl_round_trip_stays_within_quantization_error_everywhere() {
        // Sample the cube; thousandths quantization may move a channel by a
        // hair, never more than 2/255.
        for red in (0..=255).step_by(15) {
            for green in (0..=255).step_by(15) {
                for blue in (0..=255).step_by(15) {
                    let rgb = [red as u8, green as u8, blue as u8];
                    let back = hsl_to_rgb(rgb_to_hsl(rgb));
                    for channel in 0..3 {
                        let error = i16::from(rgb[channel]).abs_diff(i16::from(back[channel]));
                        assert!(error <= 2, "{rgb:?} -> {back:?} channel {channel}");
                    }
                }
            }
        }
    }

    #[test]
    fn achromatic_colors_report_zero_hue_and_saturation() {
        let gray = rgb_to_hsl([18, 18, 22]);
        assert!(gray.saturation_thousandths < 120, "{gray:?}");
        let pure = rgb_to_hsl([40, 40, 40]);
        assert_eq!(pure.hue_degrees, 0);
        assert_eq!(pure.saturation_thousandths, 0);
    }

    #[test]
    fn marker_contrast_flips_on_light_backgrounds() {
        assert_eq!(marker_on([0, 0, 0]), [255, 255, 255]);
        assert_eq!(marker_on([250, 250, 250]), [0, 0, 0]);
    }
}
