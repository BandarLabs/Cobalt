// The Cobalt window on a Mac.
//
// `kobo ui` serves one page to this computer and needs somewhere to show it.
// A browser tab works and is what every other system gets, but a tab is a
// guest in somebody else's window: it has an address bar that invites the
// address to be copied, it survives the command that was serving it, and
// it is one of forty. This is the smallest program that gives the page a
// window of its own. It draws nothing itself. Everything in the window is
// the page, and everything the page does is done by the `kobo` command that
// started this program and is waiting for it to exit.
//
// One file, the system's own frameworks, and no state kept on disk, so that
// there is nothing here to keep in step with the page and nothing left
// behind when the window closes.

import AppKit
import WebKit

// The one address this window shows: the loopback server that launched it.
// Anything else it is ever asked to show goes to the default browser, so the
// window cannot be walked off to another site and left looking like Cobalt.
struct Home {
    let launch: URL
    let port: Int

    init?(_ argument: String?) {
        guard let argument, let url = URL(string: argument),
              url.scheme == "http", url.host == "127.0.0.1", let port = url.port
        else { return nil }
        self.launch = url
        self.port = port
    }

    func holds(_ url: URL?) -> Bool {
        guard let url else { return false }
        return url.scheme == "http" && url.host == "127.0.0.1" && url.port == port
    }
}

final class Window: NSObject, NSApplicationDelegate, NSWindowDelegate, WKNavigationDelegate, WKUIDelegate {
    private let home: Home
    private var window: NSWindow!
    private var page: WKWebView!

    init(home: Home) {
        self.home = home
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let settings = WKWebViewConfiguration()
        // The session is a cookie that must not outlive the command that
        // issued it. A store that is never written to disk guarantees that
        // without relying on the cookie's own lifetime.
        settings.websiteDataStore = .nonPersistent()
        page = WKWebView(frame: .zero, configuration: settings)
        page.navigationDelegate = self
        page.uiDelegate = self
        // The ground the page paints itself on, shown for the moment before
        // it has, so the window does not open white and then turn to paper.
        let paper = NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
                ? NSColor(srgbRed: 0x17 / 255, green: 0x16 / 255, blue: 0x13 / 255, alpha: 1)
                : NSColor(srgbRed: 0xF6 / 255, green: 0xF4 / 255, blue: 0xED / 255, alpha: 1)
        }
        page.underPageBackgroundColor = paper

        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1040, height: 760),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Cobalt"
        window.backgroundColor = paper
        // The page is laid out down to this width and no narrower.
        window.minSize = NSSize(width: 420, height: 480)
        window.contentView = page
        window.delegate = self
        window.isReleasedWhenClosed = false
        window.center()
        // Where the owner last put the window is where it opens next time.
        window.setFrameAutosaveName("Cobalt")
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        page.load(URLRequest(url: home.launch))
    }

    // Closing the window is quitting. There is nothing to come back to: the
    // command that serves the page stops when this program exits, and a
    // Dock icon with no window would be a program that does nothing.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func webView(
        _ webView: WKWebView,
        decidePolicyFor action: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        if home.holds(action.request.url) {
            decisionHandler(.allow)
            return
        }
        decisionHandler(.cancel)
        open(action.request.url)
    }

    // A link that asks for a new window, which is how the page marks every
    // link that leaves it.
    func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for action: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        open(action.request.url)
        return nil
    }

    // Only web addresses are handed on. A page cannot use this window to
    // open a file, start another program, or run a script in the browser.
    private func open(_ url: URL?) {
        guard let url, url.scheme == "https" || url.scheme == "http", !home.holds(url) else { return }
        NSWorkspace.shared.open(url)
    }

    // The page's file chooser. A web view has no chooser of its own on a
    // Mac, and without this the button that asks for a file does nothing.
    func webView(
        _ webView: WKWebView,
        runOpenPanelWith parameters: WKOpenPanelParameters,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping ([URL]?) -> Void
    ) {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = parameters.allowsMultipleSelection
        panel.beginSheetModal(for: window) { answer in
            completionHandler(answer == .OK ? panel.urls : nil)
        }
    }

    // The page failing to load means the command behind it has gone. The
    // page says so itself once it is up; before that there is no page, so
    // the window says it.
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        let alert = NSAlert()
        alert.messageText = "Cobalt could not open its page."
        alert.informativeText = "The kobo ui command that opened this window is no longer running. Run kobo ui in a terminal to open it again."
        alert.addButton(withTitle: "Quit")
        alert.beginSheetModal(for: window) { _ in NSApp.terminate(nil) }
    }
}

// The menu bar every Mac program is expected to have. Edit matters most:
// without it Copy and Paste have no key equivalents and do not work in the
// page's fields at all.
func menuBar() -> NSMenu {
    let bar = NSMenu()
    func menu(_ title: String, _ items: [NSMenuItem]) -> NSMenu {
        let menu = NSMenu(title: title)
        items.forEach(menu.addItem)
        let holder = NSMenuItem()
        holder.submenu = menu
        bar.addItem(holder)
        return menu
    }
    func item(_ title: String, _ action: Selector?, _ key: String, _ modifiers: NSEvent.ModifierFlags = .command) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.keyEquivalentModifierMask = modifiers
        return item
    }

    _ = menu("Cobalt", [
        item("About Cobalt", #selector(NSApplication.orderFrontStandardAboutPanel(_:)), ""),
        .separator(),
        item("Hide Cobalt", #selector(NSApplication.hide(_:)), "h"),
        item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option]),
        item("Show All", #selector(NSApplication.unhideAllApplications(_:)), ""),
        .separator(),
        item("Quit Cobalt", #selector(NSApplication.terminate(_:)), "q"),
    ])
    _ = menu("Edit", [
        item("Undo", Selector(("undo:")), "z"),
        item("Redo", Selector(("redo:")), "z", [.command, .shift]),
        .separator(),
        item("Cut", #selector(NSText.cut(_:)), "x"),
        item("Copy", #selector(NSText.copy(_:)), "c"),
        item("Paste", #selector(NSText.paste(_:)), "v"),
        item("Select All", #selector(NSText.selectAll(_:)), "a"),
    ])
    NSApp.windowsMenu = menu("Window", [
        item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"),
        item("Zoom", #selector(NSWindow.performZoom(_:)), ""),
        .separator(),
        item("Bring All to Front", #selector(NSApplication.arrangeInFront(_:)), ""),
    ])
    return bar
}

guard let home = Home(CommandLine.arguments.dropFirst().first) else {
    // Started by hand, from the Finder or the Dock, there is no page to
    // show: the page exists only while `kobo ui` is serving it.
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    let alert = NSAlert()
    alert.messageText = "Cobalt opens from the kobo command."
    alert.informativeText = "Run kobo ui in a terminal. It starts this window and closes with it."
    alert.addButton(withTitle: "Quit")
    app.activate(ignoringOtherApps: true)
    alert.runModal()
    exit(64)
}

let app = NSApplication.shared
let delegate = Window(home: home)
app.delegate = delegate
app.setActivationPolicy(.regular)
app.mainMenu = menuBar()
app.run()
