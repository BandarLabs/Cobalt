import AppKit
import CobaltKit

// Cobalt, the Mac app for a Kobo running Cobalt.
//
// Started with no arguments it is the app. The two arguments below are for
// whoever is working on it: one draws every state of the window to files,
// and the other puts one of those states on screen. Neither is in any menu.

let arguments = Array(CommandLine.arguments.dropFirst())

func value(after flag: String) -> String? {
    guard let index = arguments.firstIndex(of: flag), index + 1 < arguments.count else { return nil }
    return arguments[index + 1]
}

let application = NSApplication.shared

if let directory = value(after: "--render-states") {
    // An accessory takes no Dock icon and never comes to the front, which
    // is right for something that only writes files.
    application.setActivationPolicy(.accessory)
    let scale = value(after: "--scale").flatMap(Double.init).map { CGFloat($0) } ?? 2
    let only = value(after: "--only")
    // A folder of the project's published screenshots, to draw the store
    // with real pictures in it. Without one, plain stand-ins are drawn.
    Scenarios.pictures = value(after: "--pictures").map { URL(fileURLWithPath: $0, isDirectory: true) }
    Task { @MainActor in
        let status = await Renderer.run(into: URL(fileURLWithPath: directory), only: only, scale: scale)
        exit(status)
    }
    application.run()
} else if let name = value(after: "--show-state") {
    application.setActivationPolicy(.accessory)
    Scenarios.pictures = value(after: "--pictures").map { URL(fileURLWithPath: $0, isDirectory: true) }
    Task { @MainActor in
        let shown = await Renderer.show(name, narrow: arguments.contains("--narrow"), dark: arguments.contains("--dark"))
        if !shown {
            FileHandle.standardError.write(Data("there is no state named \(name)\n".utf8))
            exit(2)
        }
    }
    application.run()
} else {
    let delegate = AppDelegate()
    application.delegate = delegate
    application.setActivationPolicy(.regular)
    application.run()
}
