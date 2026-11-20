// Draws the icon of Cobalt.app.
//
//   swift desktop/macos/Icon.swift OUT.iconset
//
// Writes the ten pictures an icon is made of into OUT.iconset, which
// `iconutil` turns into the one file a bundle carries. The build script
// runs both, so the icon is made from this file on every build and no
// picture is kept in the repository.
//
// The icon is the tile from the project's mark: Co, cobalt, with its atomic
// number above it, in the mark's own square lettering and its own blue. The
// rectangles below are that tile's, copied from assets/cobalt-logo.svg, in
// the units the mark is drawn in. The mark has the tile white with blue
// letters; the icon is the other way round, which is the half of the mark
// that says "balt", because a white plate with pale blue on it was looked
// at beside this one and was faint at every size a Dock shows.

import CoreGraphics
import Foundation
import ImageIO

/// The number, then the letters, each as x, y, width, height from the top
/// left of the mark's tile.
let number: [(Int, Int, Int, Int)] = [
    (16, 16, 9, 3), (22, 19, 3, 3), (16, 22, 9, 3), (16, 25, 3, 3), (16, 28, 9, 3),
    (28, 16, 9, 3), (34, 19, 3, 3), (34, 22, 3, 3), (34, 25, 3, 3), (34, 28, 3, 3),
]
let letters: [(Int, Int, Int, Int)] = [
    (24, 39, 24, 8), (16, 47, 8, 8), (48, 47, 8, 8), (16, 55, 8, 8), (16, 63, 8, 8),
    (16, 71, 8, 8), (16, 79, 8, 8), (48, 79, 8, 8), (24, 87, 24, 8),
    (72, 55, 16, 8), (64, 63, 8, 8), (88, 63, 8, 8), (64, 71, 8, 8), (88, 71, 8, 8),
    (64, 79, 8, 8), (88, 79, 8, 8), (72, 87, 16, 8),
]

let space = CGColorSpace(name: CGColorSpace.sRGB)!
let cobalt = CGColor(colorSpace: space, components: [0x66 / 255.0, 0x99 / 255.0, 0xDD / 255.0, 1])!
let paper = CGColor(colorSpace: space, components: [1, 1, 1, 1])!

/// One picture of the icon, `side` pixels square.
///
/// The plate is the shape and size every Mac icon has had since macOS 11:
/// 824 of 1024 across, with the corner that goes with it and the slight
/// shadow under it that the others in a Dock have.
///
/// From 128 pixels up it is the whole tile. Below that the number would be
/// a smear two pixels high, so the small pictures are the letters alone,
/// each square of them a whole number of pixels so that nothing is blurred.
func picture(_ side: Int) -> CGImage {
    let context = CGContext(
        data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
        space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    // Drawn from the top left, as the mark is.
    context.translateBy(x: 0, y: CGFloat(side))
    context.scaleBy(x: 1, y: -1)
    let whole = CGFloat(side)
    let scale = whole / 1024

    let small = side < 128
    // The small pictures are laid out in whole pixels: the plate first,
    // then the letters inside it.
    let inset = small ? (whole * 100 / 1024).rounded() : 100 * scale
    let plate = CGRect(x: inset, y: inset, width: whole - 2 * inset, height: whole - 2 * inset)
    let corner = plate.width * 185.4 / 824
    let shape = CGPath(roundedRect: plate, cornerWidth: corner, cornerHeight: corner, transform: nil)

    context.saveGState()
    context.setShadow(
        offset: CGSize(width: 0, height: -10 * scale), blur: 20 * scale,
        color: CGColor(colorSpace: space, components: [0, 0, 0, 0.28])
    )
    context.addPath(shape)
    context.setFillColor(cobalt)
    context.fillPath()
    context.restoreGState()

    context.setFillColor(paper)
    if small {
        // Ten squares across and seven down, each `square` pixels.
        let square = max(1, (plate.width / 13).rounded(.down))
        let left = plate.minX + ((plate.width - 10 * square) / 2).rounded()
        let top = plate.minY + ((plate.height - 7 * square) / 2).rounded()
        context.setShouldAntialias(false)
        context.addRects(letters.map { x, y, width, height in
            CGRect(
                x: left + CGFloat(x - 16) / 8 * square, y: top + CGFloat(y - 39) / 8 * square,
                width: CGFloat(width) / 8 * square, height: CGFloat(height) / 8 * square
            )
        })
    } else {
        // Seven pixels to a unit at full size puts the tile's 80 by 79
        // units of lettering in the middle of the plate with the margin
        // the mark gives it.
        let unit = 7 * scale
        let left = plate.midX - 40 * unit
        let top = plate.midY - 39.5 * unit
        context.addRects((number + letters).map { x, y, width, height in
            CGRect(
                x: left + CGFloat(x - 16) * unit, y: top + CGFloat(y - 16) * unit,
                width: CGFloat(width) * unit, height: CGFloat(height) * unit
            )
        })
    }
    // Filled as one shape, so two squares that touch leave no hairline
    // between them at the sizes where their edge falls inside a pixel.
    context.fillPath()
    return context.makeImage()!
}

guard CommandLine.arguments.count == 2 else {
    FileHandle.standardError.write(Data("usage: swift Icon.swift OUT.iconset\n".utf8))
    exit(2)
}
let folder = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)

// The names are the ones iconutil looks for, and nothing else may be in
// the folder.
for (points, scale) in [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)] {
    let name = "icon_\(points)x\(points)" + (scale == 2 ? "@2x" : "") + ".png"
    let file = folder.appendingPathComponent(name)
    guard let written = CGImageDestinationCreateWithURL(file as CFURL, "public.png" as CFString, 1, nil) else {
        FileHandle.standardError.write(Data("could not write \(file.path)\n".utf8))
        exit(1)
    }
    CGImageDestinationAddImage(written, picture(points * scale), nil)
    guard CGImageDestinationFinalize(written) else {
        FileHandle.standardError.write(Data("could not write \(file.path)\n".utf8))
        exit(1)
    }
}
