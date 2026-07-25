#!/usr/bin/env python3
"""Regenerate the bundled Mandatum Braille fallback font.

The bundled JetBrains Mono has no Braille glyphs (U+2800-U+28FF), and
neither does any monospace font on stock macOS, so CLI spinner frames
fell through to Apple Braille -- whose 0.692em advance fails cell
admission and forces the anchored-decomposition render path. This font
is generated from scratch (no third-party outlines, no license text to
carry): each glyph is the dot pattern of `codepoint - 0x2800` drawn on
the standard 2x4 Braille grid, with metrics copied from JetBrains Mono
Regular (1000 UPM, 600 advance, 1020/-300 typo ascender/descender) so
runs shape at exactly one cell per glyph.

Requires fontTools (`pip install fonttools`). The output TTF is
committed; rerun only when changing the design:

    python3 packaging/make-braille-font.py
"""

import math
from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

UPM = 1000
ADVANCE = 600
ASCENDER = 1020
DESCENDER = -300

# Dot grid: two columns, four rows, centered in the 600-unit cell and
# spread across the ink band so 8-dot patterns fill the cell the way
# terminal emulators draw them.
COLUMN_X = (165, 435)
ROW_Y = (750, 450, 150, -150)
DOT_RADIUS = 70

# U+2800+N: bit k set => dot k+1 raised. Dots 1,2,3,7 are the left
# column top-to-bottom; dots 4,5,6,8 the right column.
DOT_POSITIONS = {
    0: (0, 0),  # dot 1
    1: (0, 1),  # dot 2
    2: (0, 2),  # dot 3
    3: (1, 0),  # dot 4
    4: (1, 1),  # dot 5
    5: (1, 2),  # dot 6
    6: (0, 3),  # dot 7
    7: (1, 3),  # dot 8
}

FAMILY = "Mandatum Braille"
STYLE = "Regular"
VERSION = "1.000"


def draw_dot(pen: TTGlyphPen, cx: int, cy: int, radius: int) -> None:
    """One filled circle as eight off-curve quadratic points (TrueType
    implies the on-curve midpoints, which land on the circle)."""
    control_radius = radius / math.cos(math.pi / 8)
    points = []
    for step in range(8):
        angle = math.tau * (step + 0.5) / 8
        points.append((
            round(cx + control_radius * math.cos(angle)),
            round(cy + control_radius * math.sin(angle)),
        ))
    pen.moveTo(points[0])
    for point in points[1:]:
        pen.qCurveTo(point)
    pen.qCurveTo(points[0])
    pen.closePath()


def braille_glyph(bits: int):
    pen = TTGlyphPen(None)
    for bit, (column, row) in DOT_POSITIONS.items():
        if bits & (1 << bit):
            draw_dot(pen, COLUMN_X[column], ROW_Y[row], DOT_RADIUS)
    return pen.glyph()


def main() -> None:
    glyph_order = [".notdef"] + [f"braille{bits:02X}" for bits in range(256)]
    builder = FontBuilder(UPM, isTTF=True)
    builder.setupGlyphOrder(glyph_order)
    builder.setupCharacterMap(
        {0x2800 + bits: f"braille{bits:02X}" for bits in range(256)}
    )
    builder.setupGlyf(
        {".notdef": TTGlyphPen(None).glyph()}
        | {f"braille{bits:02X}": braille_glyph(bits) for bits in range(256)}
    )
    metrics = {}
    glyf = builder.font["glyf"]
    for name in glyph_order:
        bounds = glyf[name].xMin if glyf[name].numberOfContours else 0
        metrics[name] = (ADVANCE, bounds)
    builder.setupHorizontalMetrics(metrics)
    builder.setupHorizontalHeader(ascent=ASCENDER, descent=DESCENDER)
    builder.setupNameTable(
        {
            "familyName": FAMILY,
            "styleName": STYLE,
            "uniqueFontIdentifier": f"{VERSION};MNDT;MandatumBraille-Regular",
            "fullName": f"{FAMILY} {STYLE}",
            "psName": "MandatumBraille-Regular",
            "version": f"Version {VERSION}",
        }
    )
    builder.setupOS2(
        sTypoAscender=ASCENDER,
        sTypoDescender=DESCENDER,
        sTypoLineGap=0,
        usWinAscent=ASCENDER,
        usWinDescent=-DESCENDER,
        achVendID="MNDT",
    )
    # isFixedPitch is what fontdb keys monospace classification on.
    builder.setupPost(isFixedPitch=1)

    output = (
        Path(__file__).resolve().parent.parent
        / "crates/native-renderer/assets/fonts/mandatum-braille/MandatumBraille-Regular.ttf"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    builder.save(str(output))
    print(f"wrote {output} ({output.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
