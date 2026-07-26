//! Exact-geometry decomposition of box-drawing and block-element glyphs.
//!
//! Fonts draw these glyphs to their own em box, so shaped output leaves
//! horizontal gaps between rows of `│` and seams along runs of `─`. The
//! renderer intercepts covered codepoints before shaping and emits
//! axis-aligned rectangles sized to the exact snapped cell bounds instead,
//! so strokes tile seamlessly across cells.
//!
//! Covered ranges (the contract is: covered exactly, or left to the font):
//! - U+2500-U+254F: light/heavy solid, dashed, corner, tee, and cross forms.
//! - U+2550-U+256C: double lines and every single/double junction.
//! - U+2574-U+257F: half-line stubs, including mixed-weight transitions.
//! - U+2580-U+2590, U+2594-U+259F: block halves, eighths, and quadrants.
//!
//! Deliberately left to the font (not axis-aligned or not exactly
//! rect-decomposable): U+256D-U+2570 rounded arcs, U+2571-U+2573 diagonals,
//! and U+2591-U+2593 shades.

/// One decomposed stroke rectangle in physical pixels relative to the cell
/// origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoxRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Whether `decompose` covers this character. Kept as an explicit range
/// predicate so run interception can test cheaply; a unit test pins it to
/// `decompose` coverage exactly.
pub(crate) fn is_box_drawing(ch: char) -> bool {
    matches!(ch,
        '\u{2500}'..='\u{256C}'
        | '\u{2574}'..='\u{2590}'
        | '\u{2594}'..='\u{259F}')
}

/// Decompose a covered glyph into non-overlapping stroke rectangles for a
/// cell of `width` x `height` whole physical pixels. Returns `None` for
/// uncovered characters, which continue through font shaping.
pub(crate) fn decompose(ch: char, width: f32, height: f32) -> Option<Vec<BoxRect>> {
    let w = width.max(1.0).round() as u32;
    let h = height.max(1.0).round() as u32;
    let rects = decompose_cell(ch, w, h)?;
    Some(
        rects
            .into_iter()
            .filter(|rect| rect.x1 > rect.x0 && rect.y1 > rect.y0)
            .map(|rect| BoxRect {
                x: rect.x0 as f32,
                y: rect.y0 as f32,
                width: (rect.x1 - rect.x0) as f32,
                height: (rect.y1 - rect.y0) as f32,
            })
            .collect(),
    )
}

/// Half-open integer pixel rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IRect {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl IRect {
    fn new(x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

/// Stroke weight of one arm of a line glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Weight {
    None,
    Light,
    Heavy,
}

use Weight::{Heavy, Light, None as NoArm};

/// Light-stroke thickness for a cell. One eighth of the advance keeps the
/// weight proportional to the grid (2px at the common 16px 2x advance) and
/// never collapses below one pixel.
fn light_thickness(w: u32) -> u32 {
    (w / 8).max(1)
}

fn heavy_thickness(w: u32, h: u32) -> u32 {
    (light_thickness(w) * 2).min(w).min(h)
}

fn thickness(weight: Weight, w: u32, h: u32) -> u32 {
    match weight {
        NoArm => 0,
        Light => light_thickness(w),
        Heavy => heavy_thickness(w, h),
    }
}

/// Centered half-open band of `t` pixels inside `extent`.
fn centered_band(extent: u32, t: u32) -> (u32, u32) {
    let t = t.min(extent);
    let start = (extent - t) / 2;
    (start, start + t)
}

/// Append `rect` minus every rectangle already in `rects`, so the collection
/// stays overlap-free. Emitters may therefore paint strokes generously
/// through junctions: single coverage keeps translucent (dim) glyph colors
/// from double-blending where strokes cross.
fn add_disjoint(rects: &mut Vec<IRect>, rect: IRect) {
    if rect.is_empty() {
        return;
    }
    let mut pieces = vec![rect];
    for hole in rects.iter() {
        let mut next = Vec::new();
        for piece in pieces {
            subtract(piece, *hole, &mut next);
        }
        if next.is_empty() {
            return;
        }
        pieces = next;
    }
    rects.extend(pieces);
}

/// Push `piece` minus `hole` (up to four strips) onto `out`.
fn subtract(piece: IRect, hole: IRect, out: &mut Vec<IRect>) {
    let ix0 = piece.x0.max(hole.x0);
    let iy0 = piece.y0.max(hole.y0);
    let ix1 = piece.x1.min(hole.x1);
    let iy1 = piece.y1.min(hole.y1);
    if ix0 >= ix1 || iy0 >= iy1 {
        out.push(piece);
        return;
    }
    for strip in [
        IRect::new(piece.x0, piece.y0, piece.x1, iy0),
        IRect::new(piece.x0, iy1, piece.x1, piece.y1),
        IRect::new(piece.x0, iy0, ix0, iy1),
        IRect::new(ix1, iy0, piece.x1, iy1),
    ] {
        if !strip.is_empty() {
            out.push(strip);
        }
    }
}

fn decompose_cell(ch: char, w: u32, h: u32) -> Option<Vec<IRect>> {
    if let Some(arms) = line_arms(ch) {
        return Some(arm_rects(arms, w, h));
    }
    if let Some((horizontal, weight, dashes)) = dashed_line(ch) {
        return Some(dash_rects(horizontal, weight, dashes, w, h));
    }
    if let Some(arms) = double_arms(ch) {
        return Some(double_rects(arms, w, h));
    }
    block_rects(ch, w, h)
}

/// `(up, down, left, right)` arm weights for the light/heavy solid set.
fn line_arms(ch: char) -> Option<(Weight, Weight, Weight, Weight)> {
    Some(match ch {
        '─' => (NoArm, NoArm, Light, Light),
        '━' => (NoArm, NoArm, Heavy, Heavy),
        '│' => (Light, Light, NoArm, NoArm),
        '┃' => (Heavy, Heavy, NoArm, NoArm),
        '┌' => (NoArm, Light, NoArm, Light),
        '┍' => (NoArm, Light, NoArm, Heavy),
        '┎' => (NoArm, Heavy, NoArm, Light),
        '┏' => (NoArm, Heavy, NoArm, Heavy),
        '┐' => (NoArm, Light, Light, NoArm),
        '┑' => (NoArm, Light, Heavy, NoArm),
        '┒' => (NoArm, Heavy, Light, NoArm),
        '┓' => (NoArm, Heavy, Heavy, NoArm),
        '└' => (Light, NoArm, NoArm, Light),
        '┕' => (Light, NoArm, NoArm, Heavy),
        '┖' => (Heavy, NoArm, NoArm, Light),
        '┗' => (Heavy, NoArm, NoArm, Heavy),
        '┘' => (Light, NoArm, Light, NoArm),
        '┙' => (Light, NoArm, Heavy, NoArm),
        '┚' => (Heavy, NoArm, Light, NoArm),
        '┛' => (Heavy, NoArm, Heavy, NoArm),
        '├' => (Light, Light, NoArm, Light),
        '┝' => (Light, Light, NoArm, Heavy),
        '┞' => (Heavy, Light, NoArm, Light),
        '┟' => (Light, Heavy, NoArm, Light),
        '┠' => (Heavy, Heavy, NoArm, Light),
        '┡' => (Heavy, Light, NoArm, Heavy),
        '┢' => (Light, Heavy, NoArm, Heavy),
        '┣' => (Heavy, Heavy, NoArm, Heavy),
        '┤' => (Light, Light, Light, NoArm),
        '┥' => (Light, Light, Heavy, NoArm),
        '┦' => (Heavy, Light, Light, NoArm),
        '┧' => (Light, Heavy, Light, NoArm),
        '┨' => (Heavy, Heavy, Light, NoArm),
        '┩' => (Heavy, Light, Heavy, NoArm),
        '┪' => (Light, Heavy, Heavy, NoArm),
        '┫' => (Heavy, Heavy, Heavy, NoArm),
        '┬' => (NoArm, Light, Light, Light),
        '┭' => (NoArm, Light, Heavy, Light),
        '┮' => (NoArm, Light, Light, Heavy),
        '┯' => (NoArm, Light, Heavy, Heavy),
        '┰' => (NoArm, Heavy, Light, Light),
        '┱' => (NoArm, Heavy, Heavy, Light),
        '┲' => (NoArm, Heavy, Light, Heavy),
        '┳' => (NoArm, Heavy, Heavy, Heavy),
        '┴' => (Light, NoArm, Light, Light),
        '┵' => (Light, NoArm, Heavy, Light),
        '┶' => (Light, NoArm, Light, Heavy),
        '┷' => (Light, NoArm, Heavy, Heavy),
        '┸' => (Heavy, NoArm, Light, Light),
        '┹' => (Heavy, NoArm, Heavy, Light),
        '┺' => (Heavy, NoArm, Light, Heavy),
        '┻' => (Heavy, NoArm, Heavy, Heavy),
        '┼' => (Light, Light, Light, Light),
        '┽' => (Light, Light, Heavy, Light),
        '┾' => (Light, Light, Light, Heavy),
        '┿' => (Light, Light, Heavy, Heavy),
        '╀' => (Heavy, Light, Light, Light),
        '╁' => (Light, Heavy, Light, Light),
        '╂' => (Heavy, Heavy, Light, Light),
        '╃' => (Heavy, Light, Heavy, Light),
        '╄' => (Heavy, Light, Light, Heavy),
        '╅' => (Light, Heavy, Heavy, Light),
        '╆' => (Light, Heavy, Light, Heavy),
        '╇' => (Heavy, Light, Heavy, Heavy),
        '╈' => (Light, Heavy, Heavy, Heavy),
        '╉' => (Heavy, Heavy, Heavy, Light),
        '╊' => (Heavy, Heavy, Light, Heavy),
        '╋' => (Heavy, Heavy, Heavy, Heavy),
        '╴' => (NoArm, NoArm, Light, NoArm),
        '╵' => (Light, NoArm, NoArm, NoArm),
        '╶' => (NoArm, NoArm, NoArm, Light),
        '╷' => (NoArm, Light, NoArm, NoArm),
        '╸' => (NoArm, NoArm, Heavy, NoArm),
        '╹' => (Heavy, NoArm, NoArm, NoArm),
        '╺' => (NoArm, NoArm, NoArm, Heavy),
        '╻' => (NoArm, Heavy, NoArm, NoArm),
        '╼' => (NoArm, NoArm, Light, Heavy),
        '╽' => (Light, Heavy, NoArm, NoArm),
        '╾' => (NoArm, NoArm, Heavy, Light),
        '╿' => (Heavy, Light, NoArm, NoArm),
        _ => return None,
    })
}

/// `(horizontal, weight, dash_count)` for the dashed line forms.
fn dashed_line(ch: char) -> Option<(bool, Weight, u32)> {
    Some(match ch {
        '┄' => (true, Light, 3),
        '┅' => (true, Heavy, 3),
        '┆' => (false, Light, 3),
        '┇' => (false, Heavy, 3),
        '┈' => (true, Light, 4),
        '┉' => (true, Heavy, 4),
        '┊' => (false, Light, 4),
        '┋' => (false, Heavy, 4),
        '╌' => (true, Light, 2),
        '╍' => (true, Heavy, 2),
        '╎' => (false, Light, 2),
        '╏' => (false, Heavy, 2),
        _ => return None,
    })
}

/// Solid light/heavy composition from per-arm weights.
fn arm_rects(arms: (Weight, Weight, Weight, Weight), w: u32, h: u32) -> Vec<IRect> {
    let (up, down, left, right) = arms;
    let vertical_max = up.max(down);
    let horizontal_max = left.max(right);
    let band_x = |weight| centered_band(w, thickness(weight, w, h));
    let band_y = |weight| centered_band(h, thickness(weight, w, h));
    let mut rects = Vec::new();

    if vertical_max != NoArm && horizontal_max != NoArm {
        // Junction square sized to the heavier stroke of each axis; arms
        // stop at its edges so nothing overlaps.
        let (jx0, jx1) = band_x(vertical_max);
        let (jy0, jy1) = band_y(horizontal_max);
        add_disjoint(&mut rects, IRect::new(jx0, jy0, jx1, jy1));
        if up != NoArm {
            let (x0, x1) = band_x(up);
            add_disjoint(&mut rects, IRect::new(x0, 0, x1, jy0));
        }
        if down != NoArm {
            let (x0, x1) = band_x(down);
            add_disjoint(&mut rects, IRect::new(x0, jy1, x1, h));
        }
        if left != NoArm {
            let (y0, y1) = band_y(left);
            add_disjoint(&mut rects, IRect::new(0, y0, jx0, y1));
        }
        if right != NoArm {
            let (y0, y1) = band_y(right);
            add_disjoint(&mut rects, IRect::new(jx1, y0, w, y1));
        }
    } else if vertical_max != NoArm {
        match (up, down) {
            (u, d) if u == d => {
                let (x0, x1) = band_x(u);
                add_disjoint(&mut rects, IRect::new(x0, 0, x1, h));
            }
            (u, NoArm) => {
                // A stub extends through the far edge of where a junction
                // square would sit, so it meets partner stubs seamlessly.
                let (x0, x1) = band_x(u);
                let (_, jy1) = band_y(u);
                add_disjoint(&mut rects, IRect::new(x0, 0, x1, jy1));
            }
            (NoArm, d) => {
                let (x0, x1) = band_x(d);
                let (jy0, _) = band_y(d);
                add_disjoint(&mut rects, IRect::new(x0, jy0, x1, h));
            }
            (u, d) => {
                // Mixed-weight transition: halves meet at the midline.
                let mid = h / 2;
                let (ux0, ux1) = band_x(u);
                let (dx0, dx1) = band_x(d);
                add_disjoint(&mut rects, IRect::new(ux0, 0, ux1, mid));
                add_disjoint(&mut rects, IRect::new(dx0, mid, dx1, h));
            }
        }
    } else if horizontal_max != NoArm {
        match (left, right) {
            (l, r) if l == r => {
                let (y0, y1) = band_y(l);
                add_disjoint(&mut rects, IRect::new(0, y0, w, y1));
            }
            (l, NoArm) => {
                let (y0, y1) = band_y(l);
                let (_, jx1) = band_x(l);
                add_disjoint(&mut rects, IRect::new(0, y0, jx1, y1));
            }
            (NoArm, r) => {
                let (y0, y1) = band_y(r);
                let (jx0, _) = band_x(r);
                add_disjoint(&mut rects, IRect::new(jx0, y0, w, y1));
            }
            (l, r) => {
                let mid = w / 2;
                let (ly0, ly1) = band_y(l);
                let (ry0, ry1) = band_y(r);
                add_disjoint(&mut rects, IRect::new(0, ly0, mid, ly1));
                add_disjoint(&mut rects, IRect::new(mid, ry0, w, ry1));
            }
        }
    }
    rects
}

/// Evenly slotted dash segments with a per-slot gap.
fn dash_rects(horizontal: bool, weight: Weight, dashes: u32, w: u32, h: u32) -> Vec<IRect> {
    let extent = if horizontal { w } else { h };
    let t = thickness(weight, w, h);
    let (b0, b1) = if horizontal {
        centered_band(h, t)
    } else {
        centered_band(w, t)
    };
    let mut rects = Vec::new();
    for i in 0..dashes {
        let slot0 = i * extent / dashes;
        let slot1 = (i + 1) * extent / dashes;
        let gap = ((slot1 - slot0) / 3).max(1).min(slot1 - slot0);
        let d0 = slot0 + gap / 2;
        let d1 = slot1 - gap.div_ceil(2);
        let rect = if horizontal {
            IRect::new(d0, b0, d1, b1)
        } else {
            IRect::new(b0, d0, b1, d1)
        };
        add_disjoint(&mut rects, rect);
    }
    rects
}

/// Arm presence for the double-line set. Within U+2550-U+256C the two arms
/// of one axis always share a weight, so each axis is described by
/// `(first_arm, second_arm, double)` — `(up, down, _)` and `(left, right, _)`.
struct DoubleArms {
    up: bool,
    down: bool,
    vertical_double: bool,
    left: bool,
    right: bool,
    horizontal_double: bool,
}

fn double_arms(ch: char) -> Option<DoubleArms> {
    // (up, down, vertical_double, left, right, horizontal_double)
    let spec = match ch {
        '═' => (false, false, false, true, true, true),
        '║' => (true, true, true, false, false, false),
        '╒' => (false, true, false, false, true, true),
        '╓' => (false, true, true, false, true, false),
        '╔' => (false, true, true, false, true, true),
        '╕' => (false, true, false, true, false, true),
        '╖' => (false, true, true, true, false, false),
        '╗' => (false, true, true, true, false, true),
        '╘' => (true, false, false, false, true, true),
        '╙' => (true, false, true, false, true, false),
        '╚' => (true, false, true, false, true, true),
        '╛' => (true, false, false, true, false, true),
        '╜' => (true, false, true, true, false, false),
        '╝' => (true, false, true, true, false, true),
        '╞' => (true, true, false, false, true, true),
        '╟' => (true, true, true, false, true, false),
        '╠' => (true, true, true, false, true, true),
        '╡' => (true, true, false, true, false, true),
        '╢' => (true, true, true, true, false, false),
        '╣' => (true, true, true, true, false, true),
        '╤' => (false, true, false, true, true, true),
        '╥' => (false, true, true, true, true, false),
        '╦' => (false, true, true, true, true, true),
        '╧' => (true, false, false, true, true, true),
        '╨' => (true, false, true, true, true, false),
        '╩' => (true, false, true, true, true, true),
        '╪' => (true, true, false, true, true, true),
        '╫' => (true, true, true, true, true, false),
        '╬' => (true, true, true, true, true, true),
        _ => return None,
    };
    Some(DoubleArms {
        up: spec.0,
        down: spec.1,
        vertical_double: spec.2,
        left: spec.3,
        right: spec.4,
        horizontal_double: spec.5,
    })
}

/// Double-line composition. Double strokes are two light rails separated by
/// a light gap; single strokes in this set are light. A double rail breaks
/// only where perpendicular double arms attach on its side; single strokes
/// cross straight through.
fn double_rects(arms: DoubleArms, w: u32, h: u32) -> Vec<IRect> {
    let t = light_thickness(w);
    // Vertical rail bands (A = left rail, B = right rail) and the single
    // band; horizontal counterparts (C = top, D = bottom).
    let (vs0, _) = centered_band(w, 3 * t);
    let (a0, a1) = (vs0, (vs0 + t).min(w));
    let (b0, b1) = ((vs0 + 2 * t).min(w), (vs0 + 3 * t).min(w));
    let (sv0, sv1) = centered_band(w, t);
    let (hs0, _) = centered_band(h, 3 * t);
    let (c0, c1) = (hs0, (hs0 + t).min(h));
    let (d0, d1) = ((hs0 + 2 * t).min(h), (hs0 + 3 * t).min(h));
    let (sh0, sh1) = centered_band(h, t);

    let has_vertical = arms.up || arms.down;
    let has_horizontal = arms.left || arms.right;
    // Obstruction band of the perpendicular stroke, for rail extents at
    // outer corners.
    let horizontal_band = if arms.horizontal_double {
        (c0, d1)
    } else {
        (sh0, sh1)
    };
    let mut rects = Vec::new();

    if has_vertical {
        if arms.vertical_double {
            for (x0, x1, is_left_rail) in [(a0, a1, true), (b0, b1, false)] {
                // A rail breaks (or hangs from the near rail) only where
                // perpendicular double arms attach on its own side; single
                // perpendicular strokes cross straight through.
                let attaches = arms.horizontal_double
                    && ((is_left_rail && arms.left) || (!is_left_rail && arms.right));
                if attaches && arms.up && arms.down {
                    add_disjoint(&mut rects, IRect::new(x0, 0, x1, c1));
                    add_disjoint(&mut rects, IRect::new(x0, d0, x1, h));
                    continue;
                }
                let (y_top, y_bottom) = if !has_horizontal || (arms.up && arms.down) {
                    (0, h)
                } else if arms.down {
                    // Attached rails hang from the near horizontal rail;
                    // outer corner rails start at the stroke's far edge.
                    (if attaches { d0 } else { horizontal_band.0 }, h)
                } else {
                    (0, if attaches { c1 } else { horizontal_band.1 })
                };
                add_disjoint(&mut rects, IRect::new(x0, y_top, x1, y_bottom));
            }
        } else {
            // Every single-vertical glyph in this set has double horizontal
            // arms. A tee connects at the near rail and keeps the rail gap
            // clear; a corner spans the whole stroke band.
            let tee = arms.left && arms.right;
            let (y_top, y_bottom) = if (arms.up && arms.down) || !has_horizontal {
                (0, h)
            } else if arms.down {
                (if tee { d0 } else { c0 }, h)
            } else {
                (0, if tee { c1 } else { d1 })
            };
            add_disjoint(&mut rects, IRect::new(sv0, y_top, sv1, y_bottom));
        }
    }
    if has_horizontal {
        if arms.horizontal_double {
            for (y0, y1, is_top_rail) in [(c0, c1, true), (d0, d1, false)] {
                let attaches = arms.vertical_double
                    && ((is_top_rail && arms.up) || (!is_top_rail && arms.down));
                if attaches && arms.left && arms.right {
                    add_disjoint(&mut rects, IRect::new(0, y0, a1, y1));
                    add_disjoint(&mut rects, IRect::new(b0, y0, w, y1));
                    continue;
                }
                let vertical_band = if arms.vertical_double {
                    (a0, b1)
                } else {
                    (sv0, sv1)
                };
                let (x_left, x_right) = if !has_vertical || (arms.left && arms.right) {
                    (0, w)
                } else if arms.right {
                    (if attaches { b0 } else { vertical_band.0 }, w)
                } else {
                    (0, if attaches { a1 } else { vertical_band.1 })
                };
                add_disjoint(&mut rects, IRect::new(x_left, y0, x_right, y1));
            }
        } else {
            // Every single-horizontal glyph in this set has double vertical
            // arms: tees connect at the near rail, corners span the band.
            let tee = arms.up && arms.down;
            let (x_left, x_right) = if (arms.left && arms.right) || !has_vertical {
                (0, w)
            } else if arms.right {
                (if tee { b1 } else { a0 }, w)
            } else {
                (0, if tee { a0 } else { b1 })
            };
            add_disjoint(&mut rects, IRect::new(x_left, sh0, x_right, sh1));
        }
    }
    rects
}

/// Fractional block extent for level `k` of 8, by nearest-integer rounding
/// (the old `div_ceil` duplicated adjacent levels at odd extents, e.g. 9px
/// gave ▍ and ▌ four pixels each, so progress bars stalled and jumped).
///
/// Monotonicity: consecutive levels differ by `floor((k*e + 4)/8)` steps of
/// at least `floor(e/8)`, so for `extent >= 8` the eight levels are strictly
/// increasing and bracket the `extent / 2` half-block level
/// (`round(3e/8) < floor(e/2) < round(5e/8)` for `e >= 8`). Below eight
/// pixels duplicates are unavoidable and the ladder degrades gracefully,
/// staying within `1..=extent` and never regressing. The sweep test pins
/// both properties.
fn block_level(extent: u32, k: u32) -> u32 {
    ((k * extent + 4) / 8).clamp(1, extent)
}

/// Block-element halves, eighths, and quadrants.
fn block_rects(ch: char, w: u32, h: u32) -> Option<Vec<IRect>> {
    let eighth_w = |k: u32| block_level(w, k);
    let eighth_h = |k: u32| block_level(h, k);
    let xm = w / 2;
    let ym = h / 2;
    let quadrants = |ul: bool, ur: bool, ll: bool, lr: bool| {
        let mut rects = Vec::new();
        for (present, rect) in [
            (ul, IRect::new(0, 0, xm, ym)),
            (ur, IRect::new(xm, 0, w, ym)),
            (ll, IRect::new(0, ym, xm, h)),
            (lr, IRect::new(xm, ym, w, h)),
        ] {
            if present {
                add_disjoint(&mut rects, rect);
            }
        }
        rects
    };
    Some(match ch {
        '▀' => vec![IRect::new(0, 0, w, ym)],
        '▁' => vec![IRect::new(0, h - eighth_h(1), w, h)],
        '▂' => vec![IRect::new(0, h - eighth_h(2), w, h)],
        '▃' => vec![IRect::new(0, h - eighth_h(3), w, h)],
        '▄' => vec![IRect::new(0, ym, w, h)],
        '▅' => vec![IRect::new(0, h - eighth_h(5), w, h)],
        '▆' => vec![IRect::new(0, h - eighth_h(6), w, h)],
        '▇' => vec![IRect::new(0, h - eighth_h(7), w, h)],
        '█' => vec![IRect::new(0, 0, w, h)],
        '▉' => vec![IRect::new(0, 0, eighth_w(7), h)],
        '▊' => vec![IRect::new(0, 0, eighth_w(6), h)],
        '▋' => vec![IRect::new(0, 0, eighth_w(5), h)],
        '▌' => vec![IRect::new(0, 0, xm, h)],
        '▍' => vec![IRect::new(0, 0, eighth_w(3), h)],
        '▎' => vec![IRect::new(0, 0, eighth_w(2), h)],
        '▏' => vec![IRect::new(0, 0, eighth_w(1), h)],
        '▐' => vec![IRect::new(xm, 0, w, h)],
        '▔' => vec![IRect::new(0, 0, w, eighth_h(1))],
        '▕' => vec![IRect::new(w - eighth_w(1), 0, w, h)],
        '▖' => quadrants(false, false, true, false),
        '▗' => quadrants(false, false, false, true),
        '▘' => quadrants(true, false, false, false),
        '▙' => quadrants(true, false, true, true),
        '▚' => quadrants(true, false, false, true),
        '▛' => quadrants(true, true, true, false),
        '▜' => quadrants(true, true, false, true),
        '▝' => quadrants(false, true, false, false),
        '▞' => quadrants(false, true, true, false),
        '▟' => quadrants(false, true, true, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered_chars() -> impl Iterator<Item = char> {
        (0x2500u32..=0x25FF).filter_map(char::from_u32)
    }

    fn overlap(a: IRect, b: IRect) -> bool {
        a.x0.max(b.x0) < a.x1.min(b.x1) && a.y0.max(b.y0) < a.y1.min(b.y1)
    }

    #[test]
    fn coverage_predicate_matches_decomposition_exactly() {
        for ch in covered_chars() {
            assert_eq!(
                is_box_drawing(ch),
                decompose(ch, 16.0, 34.0).is_some(),
                "coverage disagreement at U+{:04X}",
                ch as u32
            );
        }
        assert!(!is_box_drawing('╭'), "arcs stay with the font");
        assert!(!is_box_drawing('╲'), "diagonals stay with the font");
        assert!(!is_box_drawing('▒'), "shades stay with the font");
        assert!(!is_box_drawing('X'));
        assert!(!is_box_drawing('⠋'), "Braille keeps its bundled face");
    }

    #[test]
    fn decompositions_stay_inside_the_cell_and_never_overlap() {
        for (w, h) in [(8.0, 17.0), (16.0, 34.0), (13.0, 29.0), (1.0, 1.0)] {
            for ch in covered_chars() {
                let Some(rects) = decompose(ch, w, h) else {
                    continue;
                };
                if w >= 8.0 {
                    assert!(
                        !rects.is_empty(),
                        "U+{:04X} decomposed to nothing",
                        ch as u32
                    );
                }
                let irects = rects
                    .iter()
                    .map(|r| {
                        IRect::new(
                            r.x as u32,
                            r.y as u32,
                            (r.x + r.width) as u32,
                            (r.y + r.height) as u32,
                        )
                    })
                    .collect::<Vec<_>>();
                for rect in &irects {
                    assert!(
                        !rect.is_empty() && rect.x1 <= w as u32 && rect.y1 <= h as u32,
                        "U+{:04X} rect escapes the {w}x{h} cell: {rect:?}",
                        ch as u32
                    );
                }
                for (i, a) in irects.iter().enumerate() {
                    for b in &irects[i + 1..] {
                        assert!(
                            !overlap(*a, *b),
                            "U+{:04X} rects overlap: {a:?} vs {b:?}",
                            ch as u32
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn straight_lines_span_the_full_cell_for_seamless_tiling() {
        // 16x34 cell, light thickness 16/8 = 2.
        let horizontal = decompose('─', 16.0, 34.0).unwrap();
        assert_eq!(
            horizontal,
            vec![BoxRect {
                x: 0.0,
                y: 16.0,
                width: 16.0,
                height: 2.0
            }]
        );
        let vertical = decompose('│', 16.0, 34.0).unwrap();
        assert_eq!(
            vertical,
            vec![BoxRect {
                x: 7.0,
                y: 0.0,
                width: 2.0,
                height: 34.0
            }]
        );
        // Every solid line/junction glyph with an up arm reaches y=0, with a
        // down arm reaches the bottom, and similarly for left/right, so
        // adjacent cells connect without seams.
        for ch in covered_chars() {
            let Some((up, down, left, right)) = line_arms(ch) else {
                continue;
            };
            let rects = decompose(ch, 16.0, 34.0).unwrap();
            if up != NoArm {
                assert!(rects.iter().any(|r| r.y == 0.0), "U+{:04X}", ch as u32);
            }
            if down != NoArm {
                assert!(
                    rects.iter().any(|r| r.y + r.height == 34.0),
                    "U+{:04X}",
                    ch as u32
                );
            }
            if left != NoArm {
                assert!(rects.iter().any(|r| r.x == 0.0), "U+{:04X}", ch as u32);
            }
            if right != NoArm {
                assert!(
                    rects.iter().any(|r| r.x + r.width == 16.0),
                    "U+{:04X}",
                    ch as u32
                );
            }
        }
    }

    #[test]
    fn fractional_block_levels_are_strictly_monotonic_at_every_width_from_eight() {
        // The eight left-block levels ▏▎▍▌▋▊▉█ must each add at least one
        // pixel — odd widths included — or progress bars stall and jump.
        let left_blocks = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
        for w in 8..=33u32 {
            let widths = left_blocks
                .iter()
                .map(|ch| {
                    let rects = decompose(*ch, w as f32, 34.0).unwrap();
                    assert_eq!(rects.len(), 1, "U+{:04X} at width {w}", *ch as u32);
                    assert_eq!(rects[0].x, 0.0);
                    rects[0].width as u32
                })
                .collect::<Vec<_>>();
            for pair in widths.windows(2) {
                assert!(
                    pair[1] > pair[0],
                    "duplicate block level at width {w}: {widths:?}"
                );
            }
            assert_eq!(*widths.last().unwrap(), w, "█ fills the cell");
        }
        // The reported duplicate: at 9px, ▍ and ▌ both rounded to 4px.
        assert_eq!(decompose('▍', 9.0, 34.0).unwrap()[0].width, 3.0);
        assert_eq!(decompose('▌', 9.0, 34.0).unwrap()[0].width, 4.0);
        // Same ladder vertically: ▁▂▃▄▅▆▇█ heights are strictly monotonic.
        let lower_blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        for h in 8..=35u32 {
            let heights = lower_blocks
                .iter()
                .map(|ch| decompose(*ch, 16.0, h as f32).unwrap()[0].height as u32)
                .collect::<Vec<_>>();
            for pair in heights.windows(2) {
                assert!(
                    pair[1] > pair[0],
                    "duplicate block level at height {h}: {heights:?}"
                );
            }
        }
        // Below eight pixels the ladder degrades gracefully: levels stay
        // within the cell and never regress. (Width 1 is excluded: the
        // half-block collapses to an empty decomposition there.)
        for w in 2..8u32 {
            let widths = left_blocks
                .iter()
                .map(|ch| decompose(*ch, w as f32, 34.0).unwrap()[0].width as u32)
                .collect::<Vec<_>>();
            for pair in widths.windows(2) {
                assert!(
                    pair[1] >= pair[0],
                    "regressing level at width {w}: {widths:?}"
                );
            }
            assert!(widths.iter().all(|width| (1..=w).contains(width)));
        }
    }

    #[test]
    fn double_lines_leave_the_gap_clear_and_blocks_tile_exactly() {
        // ║ at 16x34: rails 2px wide around a 2px gap, full height.
        let rails = decompose('║', 16.0, 34.0).unwrap();
        assert_eq!(rails.len(), 2);
        assert!(rails.iter().all(|r| r.y == 0.0 && r.height == 34.0));
        let mut xs = rails.iter().map(|r| (r.x, r.width)).collect::<Vec<_>>();
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert_eq!(xs, vec![(5.0, 2.0), (9.0, 2.0)]);
        // ╬ keeps its hollow center: no rect covers the cell midpoint.
        let cross = decompose('╬', 16.0, 34.0).unwrap();
        assert!(cross.iter().all(|r| {
            !(r.x <= 7.5 && 7.5 < r.x + r.width && r.y <= 17.5 && 17.5 < r.y + r.height)
        }));
        // Complementary halves reassemble the full cell.
        let upper = decompose('▀', 16.0, 34.0).unwrap();
        let lower = decompose('▄', 16.0, 34.0).unwrap();
        assert_eq!(upper[0].height + lower[0].height, 34.0);
        assert_eq!(upper[0].y + upper[0].height, lower[0].y);
        let left = decompose('▌', 16.0, 34.0).unwrap();
        let right = decompose('▐', 16.0, 34.0).unwrap();
        assert_eq!(left[0].width + right[0].width, 16.0);
        assert_eq!(left[0].x + left[0].width, right[0].x);
    }
}
