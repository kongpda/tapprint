import AppKit
import Foundation

/// Draws the TapPrint app icon at every size an `.icns` needs.
/// The mark is an ink tile with one cream slip and three rules. Small sizes
/// keep the slip and drop the rules so the shape still reads.

let sizes: [(Int, String)] = [
    (16, "icon_16x16.png"),
    (32, "icon_16x16@2x.png"),
    (32, "icon_32x32.png"),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_128x128@2x.png"),
    (256, "icon_256x256.png"),
    (512, "icon_256x256@2x.png"),
    (512, "icon_512x512.png"),
    (1024, "icon_512x512@2x.png"),
]

let arguments = CommandLine.arguments
guard arguments.count == 2 else {
    fputs("usage: draw-macos-icon.swift <AppIcon.iconset>\n", stderr)
    exit(1)
}

let directory = URL(fileURLWithPath: arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

let ink = NSColor(srgbRed: 28.0 / 255.0, green: 25.0 / 255.0, blue: 21.0 / 255.0, alpha: 1)
let paper = NSColor(srgbRed: 1, green: 252.0 / 255.0, blue: 247.0 / 255.0, alpha: 1)

for (size, name) in sizes {
    guard let png = draw(size: size) else {
        fputs("could not draw \(name)\n", stderr)
        exit(1)
    }
    try png.write(to: directory.appendingPathComponent(name))
}

func draw(size: Int) -> Data? {
    guard
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: size,
            pixelsHigh: size,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        )
    else {
        return nil
    }
    rep.size = NSSize(width: size, height: size)
    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    guard let context = NSGraphicsContext(bitmapImageRep: rep) else {
        return nil
    }
    NSGraphicsContext.current = context
    context.imageInterpolation = .high

    let canvas = CGFloat(size)
    let tile = NSBezierPath(
        roundedRect: NSRect(x: 0, y: 0, width: canvas, height: canvas),
        xRadius: canvas * 0.2237,
        yRadius: canvas * 0.2237
    )
    ink.setFill()
    tile.fill()

    let slip = NSRect(
        x: canvas * 0.29,
        y: canvas * 0.18,
        width: canvas * 0.42,
        height: canvas * 0.64
    )
    paper.setFill()
    NSBezierPath(
        roundedRect: slip,
        xRadius: max(1, canvas * 0.03),
        yRadius: max(1, canvas * 0.03)
    ).fill()

    if size >= 32 {
        ink.setFill()
        let rules = 3
        let gap = slip.height * 0.07
        let ruleHeight = max(1, slip.height * 0.045)
        var top = slip.maxY - slip.height * 0.22
        for index in 0..<rules {
            let width = index == rules - 1 ? slip.width * 0.38 : slip.width * 0.62
            NSBezierPath(
                roundedRect: NSRect(x: slip.minX + slip.width * 0.16, y: top, width: width, height: ruleHeight),
                xRadius: ruleHeight / 2,
                yRadius: ruleHeight / 2
            ).fill()
            top -= ruleHeight + gap
        }
    }

    return rep.representation(using: .png, properties: [:])
}
