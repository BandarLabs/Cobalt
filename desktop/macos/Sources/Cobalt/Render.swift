import AppKit
import CobaltKit
import SwiftUI

/// Drawing every state of the window to a file, so each can be looked at
/// without arranging for it to happen.
///
/// `Cobalt --render-states DIR` builds the same window the app shows, over
/// the same record, fed by the same client and decoding; only the server is
/// a script. The window is never put on screen. Its views are asked to draw
/// themselves into a bitmap, which needs no permission to record the screen
/// and no one at the keyboard.
///
/// What that cannot show is whatever the window server composes rather than
/// the views: on recent systems the sidebar's rows and its glass, which are
/// drawn through filters only the compositor applies. That part of each
/// image is the window's plain background with nothing on it. It is left
/// that way rather than painted in, because a picture of a sidebar that the
/// app did not draw would be evidence of nothing.
@MainActor
enum Renderer {
    struct Size {
        var name: String
        var width: CGFloat
        var height: CGFloat
    }

    static let sizes = [Size(name: "narrow", width: 700, height: 520), Size(name: "wide", width: 1120, height: 760)]

    static func run(into directory: URL, only: String?, scale: CGFloat) async -> Int32 {
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        } catch {
            FileHandle.standardError.write(Data("cannot create \(directory.path): \(error.localizedDescription)\n".utf8))
            return 1
        }
        var failed = 0
        var written = 0
        for scenario in Scenarios.all where only.map({ scenario.name.contains($0) }) ?? true {
            for dark in [false, true] {
                let appearance = dark ? "dark" : "light"
                do {
                    if scenario.question != nil {
                        let staged = await scenario.stage()
                        defer { staged.close() }
                        guard let alert = scenario.question?(staged.store) else { throw Problem.nothingToDraw }
                        alert.window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                        alert.layout()
                        let file = directory.appendingPathComponent("\(scenario.name)-sheet-\(appearance).png")
                        try await capture(alert.window, scale: scale).write(to: file)
                        written += 1
                        continue
                    }
                    for size in sizes {
                        let staged = await scenario.stage()
                        defer { staged.close() }
                        let window = MainWindow.make(store: staged.store, desk: staged.desk)
                        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                        window.setContentSize(NSSize(width: size.width, height: size.height))
                        let file = directory.appendingPathComponent("\(scenario.name)-\(size.name)-\(appearance).png")
                        try await capture(window, scale: scale).write(to: file)
                        window.contentViewController = nil
                        written += 1
                    }
                } catch {
                    failed += 1
                    FileHandle.standardError.write(Data("\(scenario.name) (\(appearance)) did not render: \(error)\n".utf8))
                }
            }
        }
        print("\(written) images in \(directory.path)\(failed == 0 ? "" : ", \(failed) states failed")")
        return failed == 0 && written > 0 ? 0 : 1
    }

    enum Problem: Error { case noFrame, noBitmap, blank, nothingToDraw }

    static func capture(_ window: NSWindow, scale: CGFloat) async throws -> Data {
        // The views lay themselves out and their tasks run over a few turns
        // of the run loop. A scripted server answers at once, so a few
        // short waits are enough for the window to reach the state it will
        // stay in.
        for _ in 0..<4 {
            window.layoutIfNeeded()
            window.displayIfNeeded()
            try await Task.sleep(nanoseconds: 70_000_000)
        }
        guard let frame = window.contentView?.superview else { throw Problem.noFrame }
        let bounds = frame.bounds
        func bitmap() -> NSBitmapImageRep? {
            let bitmap = NSBitmapImageRep(
                bitmapDataPlanes: nil,
                pixelsWide: Int((bounds.width * scale).rounded()),
                pixelsHigh: Int((bounds.height * scale).rounded()),
                bitsPerSample: 8,
                samplesPerPixel: 4,
                hasAlpha: true,
                isPlanar: false,
                colorSpaceName: .deviceRGB,
                bytesPerRow: 0,
                bitsPerPixel: 0
            )
            bitmap?.size = bounds.size
            return bitmap
        }
        guard let drawn = bitmap(), let whole = bitmap() else { throw Problem.noBitmap }
        frame.cacheDisplay(in: bounds, to: drawn)

        // Where no view drew, the bitmap is clear. On screen that is where
        // the window's own background shows, so that is what is put there.
        guard let context = NSGraphicsContext(bitmapImageRep: whole) else { throw Problem.noBitmap }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        (window.appearance ?? NSApp.effectiveAppearance).performAsCurrentDrawingAppearance {
            NSColor.windowBackgroundColor.setFill()
            NSRect(origin: .zero, size: bounds.size).fill()
        }
        drawn.draw(in: NSRect(origin: .zero, size: bounds.size), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: false, hints: nil)
        NSGraphicsContext.restoreGraphicsState()

        guard let png = whole.representation(using: .png, properties: [:]) else { throw Problem.noBitmap }
        // A window whose content never drew comes out as one flat colour. A
        // file is still written by then, and a state that silently rendered
        // as nothing is the failure this mode exists to catch.
        guard hasDetail(drawn) else { throw Problem.blank }
        return png
    }

    /// Whether more than one colour was drawn, sampled sparsely.
    private static func hasDetail(_ bitmap: NSBitmapImageRep) -> Bool {
        var seen = Set<UInt32>()
        let stepX = max(1, bitmap.pixelsWide / 60)
        let stepY = max(1, bitmap.pixelsHigh / 60)
        for y in stride(from: 0, to: bitmap.pixelsHigh, by: stepY) {
            for x in stride(from: 0, to: bitmap.pixelsWide, by: stepX) {
                guard let colour = bitmap.colorAt(x: x, y: y) else { continue }
                let key = UInt32(colour.redComponent * 255) << 16 | UInt32(colour.greenComponent * 255) << 8 | UInt32(colour.blueComponent * 255)
                seen.insert(key)
                if seen.count > 3 { return true }
            }
        }
        return false
    }

    /// Puts one state on screen as a real window, for looking at what the
    /// compositor adds. Not used by the render; a person runs it.
    static func show(_ name: String, narrow: Bool, dark: Bool) async -> Bool {
        guard let scenario = Scenarios.all.first(where: { $0.name == name }) else { return false }
        let staged = await scenario.stage(rendering: false)
        let size = narrow ? sizes[0] : sizes[1]
        if let alert = scenario.question?(staged.store) {
            alert.window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
            alert.layout()
            alert.window.center()
            alert.window.orderFrontRegardless()
            print("window \(alert.window.windowNumber)")
        } else {
            let window = MainWindow.make(store: staged.store, desk: staged.desk)
            window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
            window.setContentSize(NSSize(width: size.width, height: size.height))
            window.center()
            window.orderFrontRegardless()
            shown.append(window)
            print("window \(window.windowNumber)")
        }
        kept.append(staged)
        return true
    }

    private static var shown: [NSWindow] = []
    private static var kept: [Scenarios.Staged] = []
}
