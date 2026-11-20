import AppKit
import CobaltKit
import SwiftUI
import UniformTypeIdentifiers

/// Everything in the window: the four places down the side and the one
/// that is open beside them.
struct RootView: View {
    @EnvironmentObject private var store: Companion
    @EnvironmentObject private var desk: Desk
    @State private var dropping = false

    var body: some View {
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 170, ideal: 190, max: 250)
        } detail: {
            detail
                .navigationTitle(store.place.title)
        }
        // The whole window takes a dropped file or link, wherever in it the
        // owner lets go, because nobody aims at a box when the window is
        // the obvious target.
        .onDrop(of: [.fileURL, .url], isTargeted: $dropping) { providers in
            Drops.take(providers, into: store)
        }
        .overlay {
            if dropping {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(Paper.accent, lineWidth: 3)
                    .padding(3)
                    .allowsHitTesting(false)
            }
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch store.link {
        case .starting:
            Standstill(headline: "Starting the kobo command.", detail: "", working: true) {}
        case .lost(let lost):
            Standstill(headline: lost.headline, detail: lost.detail) {
                Button("Start Again") { Task { await store.start() } }
                    .leading()
                    .keyboardShortcut(.defaultAction)
            }
        case .connected:
            if let trouble = store.versionTrouble, trouble.blocks {
                Standstill(headline: trouble.headline, detail: trouble.detail) {}
            } else {
                // Set into the top of the place rather than stacked above
                // it, so the place still reaches up under the toolbar the
                // way every other pane does and scrolls beneath the banner.
                place.safeAreaInset(edge: .top, spacing: 0) {
                    if let trouble = store.versionTrouble {
                        VersionBanner(trouble: trouble)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var place: some View {
        switch store.place {
        case .reader:
            ReaderPlace()
        case .apps:
            AppsPlace()
                .searchable(
                    text: Binding(get: { store.apps.query }, set: { store.search($0) }),
                    placement: .toolbar,
                    prompt: "Name or what it is for"
                )
        case .browser:
            BrowserPlace()
        case .send:
            SendPlace()
        }
    }
}

/// The app and the kobo command being of different releases, said across
/// the top of whichever place is open, because it is true of all of them.
private struct VersionBanner: View {
    var trouble: VersionTrouble

    var body: some View {
        // The sentence is left to wrap as the width allows and is not
        // pinned to its ideal height. Outside a scroll view, a pinned
        // paragraph reports the height it would have at no width at all,
        // and the split view above it stops laying anything out.
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: "exclamationmark.triangle")
                .font(.body.weight(.medium))
                .accessibilityHidden(true)
            Text(trouble.detail)
                .font(.callout)
                .lineLimit(4)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Paper.inset, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .frame(maxWidth: Paper.column)
        .padding(.horizontal, 28)
        .padding(.top, 14)
        .padding(.bottom, 2)
        .frame(maxWidth: .infinity)
        .background(Paper.ground)
        .accessibilityElement(children: .combine)
    }
}

private struct Sidebar: View {
    @EnvironmentObject private var store: Companion

    var body: some View {
        List(selection: Binding<Place?>(get: { store.place }, set: { if let place = $0 { store.place = place } })) {
            ForEach(Place.allCases) { place in
                Label(place.title, systemImage: symbol(for: place))
                    .badge(badge(for: place))
                    // The symbols are drawn in the label's own colour and
                    // not the accent: four blue pictures down the side are
                    // more colour than the rest of the window has in it.
                    .listItemTint(.monochrome)
                    .tag(place)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(alignment: .leading, spacing: 1) {
                // Plain label colours, not the sidebar's own vibrant ones:
                // two short lines of small text read more surely that way
                // over whatever the glass happens to be in front of.
                Text(status.0)
                    .font(.caption)
                    .foregroundStyle(Color(nsColor: .labelColor))
                    .lineLimit(1)
                Text(status.1)
                    .font(.caption2)
                    .foregroundStyle(Color(nsColor: .secondaryLabelColor))
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .accessibilityElement(children: .combine)
        }
    }

    private func symbol(for place: Place) -> String {
        switch place {
        case .reader: return "book.closed"
        case .apps: return "square.grid.2x2"
        case .browser: return "globe"
        case .send: return "paperplane"
        }
    }

    /// A number beside a place when something there is waiting on the
    /// owner: apps that can be updated, articles that have not gone.
    private func badge(for place: Place) -> Int {
        switch place {
        case .apps: return store.shelf.updates.count
        case .send: return store.send.info?.waiting.count ?? 0
        case .reader, .browser: return 0
        }
    }

    /// The reader, in two short lines, from wherever in the window the
    /// owner is.
    private var status: (String, String) {
        guard store.link == .connected, let state = store.state else { return ("Not running", "") }
        switch store.chosen {
        case .none:
            return ("No reader connected", "kobo \(state.computer.kobo)")
        case .several(let readers):
            return ("\(readers.count) readers connected", "None chosen")
        case .one(let reader, _):
            return (reader.model, reader.cobalt.map { "Cobalt \($0.cobaltVersion)" } ?? "No Cobalt")
        }
    }
}

/// Sorting what was dropped on the window or the Dock icon into the two
/// things the reader can be sent.
@MainActor
enum Drops {
    static func take(_ providers: [NSItemProvider], into store: Companion) -> Bool {
        guard let provider = providers.first else { return false }
        let type = provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) ? UTType.fileURL : UTType.url
        guard provider.hasItemConformingToTypeIdentifier(type.identifier) else { return false }
        _ = provider.loadObject(ofClass: URL.self) { url, _ in
            guard let url else { return }
            Task { @MainActor in take(url, into: store) }
        }
        return true
    }

    static func take(_ url: URL, into store: Companion) {
        guard store.link == .connected else { return }
        store.place = .send
        Task {
            if url.isFileURL {
                await store.offer(file: url)
            } else {
                await store.send(address: url.absoluteString)
            }
        }
    }
}

@MainActor
enum MainWindow {
    /// Wide enough for two of the store's screens side by side and one of
    /// an app's at full size, with the sidebar open.
    static let smallest = NSSize(width: 820, height: 520)
    /// What the window opens at the first time: three screens across.
    static let usual = NSSize(width: 1100, height: 760)

    /// Builds the window. The app shows the one it gets; the render mode
    /// draws the one it gets to a file. They are the same window.
    static func make(store: Companion, desk: Desk) -> NSWindow {
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: usual),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "Cobalt"
        window.toolbarStyle = .unified
        window.tabbingMode = .disallowed
        window.isReleasedWhenClosed = false
        let root = RootView()
            .environmentObject(store)
            .environmentObject(desk)
        let host = NSHostingController(
            rootView: desk.rendering ? AnyView(root.environment(\.controlActiveState, .key)) : AnyView(root)
        )
        // The window's size is the owner's. Left to itself the hosting
        // controller resizes the window to whatever the content would
        // ideally like, which for a long sentence is one very long line.
        host.sizingOptions = []
        window.contentViewController = host
        window.minSize = smallest
        window.setContentSize(usual)
        return window
    }
}
