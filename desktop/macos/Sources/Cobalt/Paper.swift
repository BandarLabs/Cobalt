import AppKit
import SwiftUI

/// The few colours and faces that are Cobalt's own.
///
/// Everything else in the window is the system's: its controls, its
/// symbols, its text colours. What is added is what an e-reader suggests,
/// which is paper to read from, a book face for what is read, and one blue.
enum Paper {
    /// The ground everything in the detail pane sits on: warm paper by day
    /// and true dark by night, the same pair the browser page uses, so the
    /// two front ends are recognisably one thing.
    static let ground = Color(nsColor: dynamic(light: 0xF6F4ED, dark: 0x171613))

    /// A panel a shade off the ground, for a note or a command's words.
    static let inset = Color(nsColor: NSColor(name: nil) { appearance in
        isDark(appearance) ? NSColor.white.withAlphaComponent(0.06) : NSColor.black.withAlphaComponent(0.045)
    })

    static let rule = Color(nsColor: NSColor(name: nil) { appearance in
        isDark(appearance) ? NSColor.white.withAlphaComponent(0.14) : NSColor.black.withAlphaComponent(0.12)
    })

    /// The project's blue, darkened until it carries white text on paper
    /// and held back at night so a button does not glare.
    static let accent = Color(nsColor: dynamic(light: 0x2B5597, dark: 0x4A78C2))

    /// The same blue as it reads when it is text and not a fill.
    static let link = Color(nsColor: dynamic(light: 0x2B5597, dark: 0x8FB3EA))

    /// The book face, for the reader's name and for anything that is read
    /// rather than pressed.
    static func serif(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .serif)
    }

    /// The widest a column of reading runs before the lines get too long
    /// to follow back.
    static let column: CGFloat = 620

    private static func isDark(_ appearance: NSAppearance) -> Bool {
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    }

    private static func dynamic(light: Int, dark: Int) -> NSColor {
        NSColor(name: nil) { appearance in
            let hex = isDark(appearance) ? dark : light
            return NSColor(
                srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: 1
            )
        }
    }
}

/// What the window keeps for itself: things that are about the window and
/// not about any reader, so they have no place in the record the server
/// fills in.
@MainActor
final class Desk: ObservableObject {
    /// Raised to send the keyboard to the web address field, from the menu.
    @Published var addressFocus = 0
    /// True while the window is being drawn to a file. A window that is not
    /// on screen is never the key window, and the system draws a button in
    /// a window that is not key as if it were switched off.
    var rendering = false
}
