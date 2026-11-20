import AppKit
import CobaltKit
import Combine
import SwiftUI

/// Finding a server for the window, in whichever direction it was started.
@MainActor
final class Launch {
    /// Who started whom. Read once: it decides which program macOS holds
    /// responsible for the reader's drive, and that does not change when
    /// the server is started again.
    let origin: Origin
    var whenServerStops: (@MainActor (String) -> Void)?
    /// Whether the server in use is the one a terminal started.
    private(set) var servedByTerminal = false

    private var server: ServerProcess?
    private var firstAttempt = true
    private let environment = ProcessInfo.processInfo.environment

    init() {
        origin = environment["COBALT_UI_HANDOFF"] == "stdin" ? .terminal : .finder
    }

    func connect() async -> Result<Client, Lost> {
        let first = firstAttempt
        firstAttempt = false
        servedByTerminal = false
        if first {
            // Started by `kobo ui`: the session is one line on standard
            // input. This can only be tried once, because the line is gone
            // once read and the command that wrote it is the one being
            // replaced if the owner starts again.
            if origin == .terminal,
               let line = await Handoff.line(from: .standardInput, within: 5),
               let session = Handoff.session(fromLine: line) {
                servedByTerminal = true
                return .success(Client(session: session))
            }
            if let session = Handoff.session(environment: environment) {
                return .success(Client(session: session))
            }
        }
        server?.stop()
        server = nil
        // The kobo command is installed in the same folder as the app. A
        // development build has no such neighbour and may name one.
        let command = environment["COBALT_UI_KOBO"].map { URL(fileURLWithPath: $0) }
            ?? Bundle.main.bundleURL.deletingLastPathComponent().appendingPathComponent("kobo")
        do {
            let started = try await ServerProcess.start(command: command) { [weak self] said in
                Task { @MainActor in self?.whenServerStops?(said) }
            }
            server = started
            return .success(Client(session: started.session))
        } catch let trouble as ServerProcess.Trouble {
            switch trouble {
            case .missing(let path): return .failure(Lost(.missing(path: path)))
            case .exited(let said): return .failure(Lost(.exited(said: said)))
            case .silent(let seconds): return .failure(Lost(.silent(seconds: seconds)))
            case .garbled: return .failure(Lost(.garbled))
            }
        } catch {
            return .failure(Lost(.stopped))
        }
    }

    func stop() {
        server?.stop()
    }
}

/// The questions the app asks in a sheet, built here so the render mode
/// can draw exactly what the owner is shown.
@MainActor
enum Questions {
    static func alert(for question: QuitQuestion) -> NSAlert {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = question.title
        alert.informativeText = question.detail
        alert.addButton(withTitle: question.stay)
        alert.addButton(withTitle: question.leave)
        return alert
    }

    static func removal(of app: AppEntry) -> NSAlert {
        let alert = NSAlert()
        alert.messageText = "Remove \(app.title) from this reader?"
        alert.informativeText = "Its data is kept, so installing it again picks up where it left off."
        alert.addButton(withTitle: "Remove")
        alert.addButton(withTitle: "Cancel")
        alert.buttons.first?.hasDestructiveAction = true
        return alert
    }

    static func askToRemove(_ app: AppEntry, store: Companion) {
        let alert = removal(of: app)
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else { return }
        alert.beginSheetModal(for: window) { response in
            guard response == .alertFirstButtonReturn else { return }
            Task { await store.removeApp(app) }
        }
    }

    static func chooseFile(store: Companion) {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.message = "Choose a file to send to the reader."
        panel.prompt = "Send"
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else { return }
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            Drops.take(url, into: store)
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate, NSMenuItemValidation {
    private let desk = Desk()
    private let launch = Launch()
    private var store: Companion!
    private var window: NSWindow!
    private var watching: Set<AnyCancellable> = []
    private var polling: [Task<Void, Never>] = []
    /// Files and links handed over before there was a server to take them.
    private var waiting: [URL] = []
    /// Set once the owner has said to go ahead and close on running work,
    /// so the question is asked once and not again on the way out.
    private var agreedToLeave = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        let launch = launch
        // A build run outside its bundle has no version of its own, and
        // then nothing is compared.
        let version = Bundle.main.bundleURL.pathExtension == "app"
            ? Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
            : nil
        store = Companion(
            place: Place(named: UserDefaults.standard.string(forKey: "place")),
            appVersion: version,
            origin: launch.origin
        ) { await launch.connect() }
        launch.whenServerStops = { [weak self] said in self?.store.serverStopped(said: said) }

        NSApp.mainMenu = Menus.build(for: self)
        window = MainWindow.make(store: store, desk: desk)
        window.delegate = self
        window.center()
        // After the first run the window comes back where it was left.
        window.setFrameAutosaveName("Cobalt")
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        store.$place.dropFirst().sink { place in
            UserDefaults.standard.set(place.rawValue, forKey: "place")
        }.store(in: &watching)

        polling.append(Task { [store, weak self] in
            await store?.start()
            self?.store.jobsEchoInTerminal = launch.servedByTerminal
            self?.deliverWaiting()
            // The state is read every two seconds. A place that shows
            // something only the server knows is brought up to date on the
            // same beat, so an article that went while the window was open
            // leaves the queue without the owner asking.
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                await store?.refresh()
                if store?.link == .connected, store?.place == .send { await store?.loadSend() }
            }
        })
        polling.append(Task { [store] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 700_000_000)
                await store?.tick()
            }
        })
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool { true }

    /// A file or a link dropped on the Dock icon, or opened with Cobalt.
    func application(_ application: NSApplication, open urls: [URL]) {
        waiting.append(contentsOf: urls)
        deliverWaiting()
    }

    private func deliverWaiting() {
        guard let store, store.link == .connected else { return }
        // One thing is sent at a time, so of several dropped at once the
        // first is taken and the rest are named as not sent by the place.
        for url in waiting { Drops.take(url, into: store) }
        waiting.removeAll()
    }

    // Leaving while something is running.

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard !agreedToLeave, let question = store.quitQuestion else { return true }
        Questions.alert(for: question).beginSheetModal(for: sender) { [weak self] response in
            guard response == .alertSecondButtonReturn else { return }
            self?.agreedToLeave = true
            sender.close()
        }
        return false
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !agreedToLeave, let question = store?.quitQuestion, let window, window.isVisible else { return .terminateNow }
        Questions.alert(for: question).beginSheetModal(for: window) { [weak self] response in
            let leave = response == .alertSecondButtonReturn
            self?.agreedToLeave = leave
            sender.reply(toApplicationShouldTerminate: leave)
        }
        return .terminateLater
    }

    func applicationWillTerminate(_ notification: Notification) {
        polling.forEach { $0.cancel() }
        launch.stop()
    }

    // What the menus do.

    @objc func showPlace(_ sender: NSMenuItem) {
        guard let place = Place.allCases.first(where: { $0.rawValue == sender.representedObject as? String }) else { return }
        store.place = place
        window.makeKeyAndOrderFront(nil)
    }

    @objc func sendFile(_ sender: Any?) {
        store.place = .send
        Questions.chooseFile(store: store)
    }

    @objc func sendWebAddress(_ sender: Any?) {
        store.place = .send
        // The field does not exist until the place has been drawn.
        DispatchQueue.main.async { self.desk.addressFocus += 1 }
    }

    @objc func findApp(_ sender: Any?) {
        store.place = .apps
        DispatchQueue.main.async {
            let search = self.window.toolbar?.items.compactMap { $0 as? NSSearchToolbarItem }.first
            search?.beginSearchInteraction()
        }
    }

    @objc func checkAgain(_ sender: Any?) {
        Task {
            await store.refresh()
            switch store.place {
            case .reader: break
            case .apps: await store.loadApps()
            case .browser: await store.loadBrowsers()
            case .send: await store.loadSend()
            }
        }
    }

    @objc func installCobalt(_ sender: Any?) {
        store.place = .reader
        Task { await store.openPlan(undo: false) }
    }

    @objc func removeCobalt(_ sender: Any?) {
        store.place = .reader
        Task { await store.openPlan(undo: true) }
    }

    @objc func ejectReader(_ sender: Any?) {
        guard let reader = store.chosen.reader else { return }
        Task { await store.eject(volume: reader.volume) }
    }

    @objc func chooseAnotherReader(_ sender: Any?) {
        store.place = .reader
        store.chooseAnother()
    }

    @objc func openHelp(_ sender: Any?) {
        if let url = URL(string: "https://bandarlabs.github.io/Cobalt/") { NSWorkspace.shared.open(url) }
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        guard let store else { return false }
        let working = store.link == .connected && store.versionTrouble?.blocks != true
        let reader = store.chosen.reader
        let readerBusy = store.running.contains { $0.kind == .reader } || store.reader.plan != nil
        let advice = reader.map { Advice.standing(of: $0, on: store.computer) }
        switch item.action {
        case #selector(showPlace(_:)):
            item.state = (item.representedObject as? String) == store.place.rawValue ? .on : .off
            return true
        case #selector(sendFile(_:)), #selector(sendWebAddress(_:)):
            return working && store.send.job?.running != true
        case #selector(findApp(_:)):
            return working && store.appsReader != nil
        case #selector(checkAgain(_:)):
            return working
        case #selector(installCobalt(_:)):
            item.title = advice?.primary == .update ? "Update Cobalt…" : advice?.repair == true ? "Install Cobalt Again…" : "Install Cobalt…"
            return working && !readerBusy && reader?.writable != false && (advice?.primary != nil || advice?.repair == true)
        case #selector(removeCobalt(_:)):
            return working && !readerBusy && reader?.writable != false && advice?.removable == true
        case #selector(ejectReader(_:)):
            return working && reader != nil && store.computer?.ejects == true && !store.running.contains { $0.kind == .reader }
        case #selector(chooseAnotherReader(_:)):
            if case .one(_, let among) = store.chosen { return working && !among.isEmpty && !readerBusy }
            return false
        default:
            return true
        }
    }
}

/// The menu bar. Built by hand because the app is not a SwiftUI scene, and
/// kept to what a Mac program is expected to have plus the few things this
/// one does.
@MainActor
enum Menus {
    static func build(for delegate: AppDelegate) -> NSMenu {
        let bar = NSMenu()

        let app = menu("Cobalt", in: bar)
        app.addItem(withTitle: "About Cobalt", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        app.addItem(.separator())
        let services = NSMenuItem(title: "Services", action: nil, keyEquivalent: "")
        services.submenu = NSMenu(title: "Services")
        NSApp.servicesMenu = services.submenu
        app.addItem(services)
        app.addItem(.separator())
        app.addItem(withTitle: "Hide Cobalt", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        app.addItem(item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option]))
        app.addItem(withTitle: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
        app.addItem(.separator())
        app.addItem(withTitle: "Quit Cobalt", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")

        let file = menu("File", in: bar)
        file.addItem(item("Send File…", #selector(AppDelegate.sendFile(_:)), "o", target: delegate))
        file.addItem(item("Send Web Address…", #selector(AppDelegate.sendWebAddress(_:)), "l", target: delegate))
        file.addItem(.separator())
        file.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")

        let edit = menu("Edit", in: bar)
        edit.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        edit.addItem(item("Redo", Selector(("redo:")), "z", [.command, .shift]))
        edit.addItem(.separator())
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "Delete", action: #selector(NSText.delete(_:)), keyEquivalent: "")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        edit.addItem(.separator())
        edit.addItem(item("Find an App…", #selector(AppDelegate.findApp(_:)), "f", target: delegate))

        let view = menu("View", in: bar)
        for (index, place) in Place.allCases.enumerated() {
            let entry = item(place.title, #selector(AppDelegate.showPlace(_:)), "\(index + 1)", target: delegate)
            entry.representedObject = place.rawValue
            view.addItem(entry)
        }
        view.addItem(.separator())
        view.addItem(item("Check Again", #selector(AppDelegate.checkAgain(_:)), "r", target: delegate))
        view.addItem(.separator())
        view.addItem(item("Show Sidebar", #selector(NSSplitViewController.toggleSidebar(_:)), "s", [.command, .control]))
        view.addItem(item("Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control]))

        let reader = menu("Reader", in: bar)
        reader.addItem(item("Install Cobalt…", #selector(AppDelegate.installCobalt(_:)), "", target: delegate))
        reader.addItem(item("Remove Cobalt…", #selector(AppDelegate.removeCobalt(_:)), "", target: delegate))
        reader.addItem(.separator())
        reader.addItem(item("Eject", #selector(AppDelegate.ejectReader(_:)), "e", target: delegate))
        reader.addItem(item("Choose Another Reader", #selector(AppDelegate.chooseAnotherReader(_:)), "", target: delegate))

        let window = menu("Window", in: bar)
        window.addItem(withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m")
        window.addItem(withTitle: "Zoom", action: #selector(NSWindow.performZoom(_:)), keyEquivalent: "")
        window.addItem(.separator())
        window.addItem(withTitle: "Bring All to Front", action: #selector(NSApplication.arrangeInFront(_:)), keyEquivalent: "")
        NSApp.windowsMenu = window

        let help = menu("Help", in: bar)
        help.addItem(item("Cobalt on the Web", #selector(AppDelegate.openHelp(_:)), "", target: delegate))
        NSApp.helpMenu = help

        return bar
    }

    private static func menu(_ title: String, in bar: NSMenu) -> NSMenu {
        let entry = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        let menu = NSMenu(title: title)
        entry.submenu = menu
        bar.addItem(entry)
        return menu
    }

    private static func item(_ title: String, _ action: Selector, _ key: String, _ modifiers: NSEvent.ModifierFlags = .command, target: AnyObject? = nil) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.keyEquivalentModifierMask = modifiers
        item.target = target
        return item
    }
}
