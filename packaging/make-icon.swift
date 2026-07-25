// Renders the Mandatum app icon master (1024x1024 PNG).
// Invoked by make-icon.sh; the committed artifact is Mandatum.icns.
import AppKit
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

let canvas = 1024
// macOS icon grid: the tile occupies 824x824 centered in the 1024 canvas.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let cornerRadius: CGFloat = 186

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
        green: CGFloat((hex >> 8) & 0xFF) / 255,
        blue: CGFloat(hex & 0xFF) / 255,
        alpha: alpha
    )
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!
guard let ctx = CGContext(
    data: nil, width: canvas, height: canvas,
    bitsPerComponent: 8, bytesPerRow: 0, space: space,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
) else { fatalError("could not create drawing context") }
ctx.interpolationQuality = .high
ctx.setAllowsAntialiasing(true)

let tilePath = CGPath(
    roundedRect: tile, cornerWidth: cornerRadius, cornerHeight: cornerRadius,
    transform: nil
)

// Soft tile shadow so the icon sits naturally on light backgrounds.
ctx.saveGState()
ctx.setShadow(
    offset: CGSize(width: 0, height: -14), blur: 36,
    color: color(0x000000, 0.35)
)
ctx.addPath(tilePath)
ctx.setFillColor(color(0x11141F))
ctx.fillPath()
ctx.restoreGState()

// Everything else clips to the tile.
ctx.saveGState()
ctx.addPath(tilePath)
ctx.clip()

// Background: deep graphite-blue vertical gradient.
let background = CGGradient(
    colorsSpace: space,
    colors: [color(0x232A3D), color(0x0C0E16)] as CFArray,
    locations: [0, 1]
)!
ctx.drawLinearGradient(
    background,
    start: CGPoint(x: 512, y: tile.maxY),
    end: CGPoint(x: 512, y: tile.minY),
    options: []
)

// Faint radial glow behind the glyph.
let glow = CGGradient(
    colorsSpace: space,
    colors: [color(0x4C6FFF, 0.32), color(0x4C6FFF, 0.0)] as CFArray,
    locations: [0, 1]
)!
ctx.drawRadialGradient(
    glow,
    startCenter: CGPoint(x: 512, y: 560), startRadius: 0,
    endCenter: CGPoint(x: 512, y: 560), endRadius: 470,
    options: []
)

// Glyph: a prompt chevron and cursor, filled with a GPU gradient.
let chevron = CGMutablePath()
chevron.move(to: CGPoint(x: 330, y: 668))
chevron.addLine(to: CGPoint(x: 522, y: 512))
chevron.addLine(to: CGPoint(x: 330, y: 356))
let chevronStroke = chevron.copy(
    strokingWithWidth: 94, lineCap: .round, lineJoin: .round, miterLimit: 10
)

let cursor = CGPath(
    roundedRect: CGRect(x: 570, y: 309, width: 152, height: 50),
    cornerWidth: 25, cornerHeight: 25, transform: nil
)

let glyph = CGMutablePath()
glyph.addPath(chevronStroke)
glyph.addPath(cursor)

// Glow pass under the glyph.
ctx.saveGState()
ctx.setShadow(
    offset: .zero, blur: 60, color: color(0x6FB4FF, 0.55)
)
ctx.addPath(glyph)
ctx.setFillColor(color(0x6FB4FF))
ctx.fillPath()
ctx.restoreGState()

// Gradient fill pass.
ctx.saveGState()
ctx.addPath(glyph)
ctx.clip()
let accent = CGGradient(
    colorsSpace: space,
    colors: [color(0x59D7FF), color(0x8E7BFF)] as CFArray,
    locations: [0, 1]
)!
ctx.drawLinearGradient(
    accent,
    start: CGPoint(x: 300, y: 700),
    end: CGPoint(x: 760, y: 300),
    options: [.drawsBeforeStartLocation, .drawsAfterEndLocation]
)
ctx.restoreGState()

// Hairline top highlight for depth.
ctx.addPath(
    CGPath(
        roundedRect: tile.insetBy(dx: 3, dy: 3),
        cornerWidth: cornerRadius - 3, cornerHeight: cornerRadius - 3,
        transform: nil
    )
)
ctx.setStrokeColor(color(0xFFFFFF, 0.07))
ctx.setLineWidth(6)
ctx.strokePath()

ctx.restoreGState()

guard let image = ctx.makeImage() else { fatalError("could not render icon") }
let outputURL = URL(fileURLWithPath: CommandLine.arguments[1])
guard let destination = CGImageDestinationCreateWithURL(
    outputURL as CFURL, UTType.png.identifier as CFString, 1, nil
) else { fatalError("could not open \(outputURL.path)") }
CGImageDestinationAddImage(destination, image, nil)
guard CGImageDestinationFinalize(destination) else {
    fatalError("could not write \(outputURL.path)")
}
