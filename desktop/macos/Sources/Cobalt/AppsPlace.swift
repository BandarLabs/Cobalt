import AppKit
import CobaltKit
import SwiftUI

/// The Apps place: a small store. Every app there is, with a picture of it
/// on a reader's screen, in the groups the project's website lists them in,
/// and a page for each. What is on the reader comes first. It can be read
/// with no reader plugged in; installing is what needs one.
struct AppsPlace: View {
    @EnvironmentObject private var store: Companion

    var body: some View {
        let front = store.storefront
        Group {
            if let id = store.store.open, let entry = front.entry(id) ?? store.storefront(unfiltered: id) {
                AppPage(entry: entry)
            } else {
                Browse(front: front)
            }
        }
        .task {
            if store.store.store == nil { await store.loadStore() } else { store.forgetFailedShots() }
        }
        // What the reader holds and what its catalog offers are read when
        // there is a reader with Cobalt on it, and again when either
        // changes under the place.
        .task(id: store.apps.key) {
            if store.appsReader != nil, store.apps.installed == nil, store.apps.problem == nil { await store.loadApps() }
        }
        .toolbar {
            ToolbarItem(placement: .navigation) {
                if store.store.open != nil {
                    Button {
                        store.closeApp()
                    } label: {
                        Label("All Apps", systemImage: "chevron.backward")
                    }
                    .help("Back to all the apps")
                }
            }
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task {
                        store.forgetFailedShots()
                        await store.loadStore()
                        await store.loadApps()
                    }
                } label: {
                    Label("Check Again", systemImage: "arrow.clockwise")
                }
                .help("Read the reader's apps and fetch the catalog again")
                .disabled(store.apps.loading || store.apps.job?.running == true)
            }
        }
    }
}

extension Companion {
    /// The app a page is open on, found even when what is typed in the
    /// search field would no longer list it: the page stays put.
    func storefront(unfiltered id: String) -> StoreEntry? {
        Storefront.arrange(store: store.store, installed: apps.installed, catalog: apps.catalog, query: "").entry(id)
    }
}

/// What is true of the reader's apps as a whole, above the store: work in
/// hand, what went wrong, and what to do before unplugging.
private struct StoreNotes: View {
    @EnvironmentObject private var store: Companion

    var body: some View {
        let reader = store.appsReader
        if let job = store.apps.job {
            AppsJob(job: job)
        }
        if store.apps.job == nil, !store.apps.said.isEmpty {
            Note(store.apps.said, kind: .done)
        }
        if let refusal = store.apps.refusal {
            Note(refusal.sentence, kind: .caution) {
                Button("Dismiss") { store.dismissAppsNote() }.controlSize(.small)
            }
        }
        if let problem = store.apps.problem {
            Note(problem.sentence, kind: .caution) {
                Button("Try Again") { Task { await store.loadApps() } }.controlSize(.small)
            }
        }
        if let problem = store.store.problem, store.store.store == nil {
            Note(problem.sentence, kind: .caution) {
                Button("Try Again") { Task { await store.loadStore() } }.controlSize(.small)
            }
        }
        if reader?.writable == false {
            Note(Storage.readOnly, kind: .caution)
        }
        if let reader, store.apps.changed, store.apps.job?.running != true {
            Note("Eject the reader before you unplug it.") {
                if store.computer?.ejects == true {
                    Button("Eject Now") { Task { await store.eject(volume: reader.volume, from: .apps) } }
                        .controlSize(.small)
                        .disabled(store.reader.ejecting)
                }
            }
        }
    }
}

private struct Browse: View {
    @EnvironmentObject private var store: Companion
    var front: Storefront

    private var busy: Bool { store.apps.job?.running == true || store.apps.loading || store.apps.removing != nil }

    var body: some View {
        let query = store.apps.query.trimmingCharacters(in: .whitespacesAndNewlines)
        Pane(width: Picture.grid * 4 + 24 * 3) {
            StoreNotes()
            if store.state != nil, store.appsReader == nil {
                // The one line that says why there is nothing to press.
                Text(without)
                    .foregroundStyle(.secondary)
            }
            if store.appsReader != nil, store.apps.loading {
                // Said above the store and not under it, where it would be
                // a screen's length out of sight.
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(store.apps.installed == nil ? "Reading the apps on the reader." : "Fetching the signed app catalog.")
                        .foregroundStyle(.secondary)
                }
            }
            if store.store.store == nil, store.store.problem == nil, front.isEmpty {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Reading the store.").foregroundStyle(.secondary)
                }
            } else if !query.isEmpty, front.isEmpty {
                Text("No app matches “\(query)”.")
                    .font(Paper.serif(17))
            } else {
                if !front.onReader.isEmpty {
                    shelf("On this reader", front.onReader) {
                        if front.updates.count > 1, query.isEmpty {
                            Button("Update All \(front.updates.count)") { Task { await store.installApps(front.updates) } }
                                .controlSize(.small)
                                .disabled(busy || store.appsReader?.writable == false)
                                .accessibilityLabel("Update all \(front.updates.count) apps")
                        }
                    }
                }
                ForEach(front.sections) { section in
                    shelf(section.title, section.entries) { EmptyView() }
                }
            }
        }
    }

    private var without: String {
        switch store.chosen {
        case .none: return "Installing needs a reader plugged in."
        case .several: return "Installing needs one reader chosen, in Reader."
        case .one: return "Installing needs Cobalt on the reader."
        }
    }

    private func shelf<Trailing: View>(_ title: String, _ entries: [StoreEntry], @ViewBuilder trailing: () -> Trailing) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                Heading(title)
                Spacer()
                trailing()
            }
            // Columns exactly as wide as a screen is drawn, as many as fit.
            LazyVGrid(columns: [GridItem(.adaptive(minimum: Picture.grid, maximum: Picture.grid), spacing: 24, alignment: .top)], alignment: .leading, spacing: 30) {
                ForEach(entries) { entry in
                    Tile(entry: entry)
                }
            }
        }
    }
}

/// One app in the grid: its picture, its name, and the one line of what it
/// does. Pressing it opens its page.
private struct Tile: View {
    @EnvironmentObject private var store: Companion
    var entry: StoreEntry

    var body: some View {
        Button {
            store.open(app: entry.id)
        } label: {
            VStack(alignment: .leading, spacing: 10) {
                Picture(shot: entry.shots.first, title: entry.title, width: Picture.grid)
                VStack(alignment: .leading, spacing: 3) {
                    Text(entry.title)
                        .font(Paper.serif(19))
                        .lineLimit(1)
                    if !entry.summary.isEmpty {
                        // Whole, however long: the row grows to the
                        // tallest of its tiles and nothing ends in dots.
                        Text(entry.summary)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .multilineTextAlignment(.leading)
                    }
                    if let fact {
                        Text(fact)
                            .font(.caption)
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                            .padding(.top, 1)
                    }
                }
            }
            .frame(width: Picture.grid, alignment: .topLeading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel([entry.title, entry.summary, fact ?? ""].filter { !$0.isEmpty }.joined(separator: ". "))
        .accessibilityAddTraits(.isButton)
    }

    /// The fact worth a line in the grid: what the reader has, and what it
    /// could have.
    private var fact: String? {
        guard let installed = entry.installedVersion else { return nil }
        if entry.updateAvailable, entry.runsOnThisReader { return "\(installed), \(entry.version) available" }
        return installed
    }
}

/// A picture of an app on a reader's screen, framed as one.
///
/// The frame is always the reader's proportions and always there, so a
/// picture that has not arrived, could not be had or does not exist leaves
/// the same shape with the app's name in it, and nothing around it moves
/// when the picture comes.
struct Picture: View {
    /// The two widths a screen is drawn at. A reader's panel is 1072
    /// pixels across, so these are a half and a quarter of it on an
    /// ordinary display and the whole and a half on a dense one: a whole
    /// fraction either way, which is what keeps a hairline on the reader a
    /// hairline here.
    static let page: CGFloat = 536
    static let grid: CGFloat = 268

    @EnvironmentObject private var store: Companion
    @Environment(\.displayScale) private var density
    var shot: Shot?
    var title: String
    var width: CGFloat

    /// The frame is a reader's screen whatever the picture's own shape. A
    /// picture of another shape sits inside it on white, the way it would
    /// on the reader, so a row of frames stays a row.
    private let proportions: CGFloat = 1072.0 / 1448.0

    /// Whether the picture has yet to be asked for. It becomes true again
    /// when a failed picture is forgotten, which is what asks for it anew.
    private var unasked: Bool {
        guard let shot else { return false }
        return store.store.shots[shot.name] == nil
    }

    var body: some View {
        ZStack {
            Rectangle().fill(Color.white.opacity(image == nil ? 0 : 1))
            if let image {
                Image(nsImage: image)
                    .resizable()
                    // Smoothed only when it is being shrunk. Pixel for
                    // pixel, smoothing would soften what is already exact.
                    .interpolation(image.size.width > width * density ? .high : .none)
                    .aspectRatio(contentMode: .fit)
            } else {
                Text(title)
                    .font(Paper.serif(width > Picture.grid ? 22 : 15))
                    .foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
                    .padding(12)
            }
        }
        // Whole points both ways, so the frame never lands on half a pixel.
        .frame(width: width, height: (width / proportions).rounded())
        .overlay(Rectangle().strokeBorder(Paper.rule, lineWidth: 1))
        .task(id: "\(shot?.name ?? "")|\(unasked)") {
            if let shot { store.wantShot(shot.name) }
        }
        .accessibilityElement()
        .accessibilityLabel(shot?.caption.isEmpty == false ? shot?.caption ?? title : "\(title), no picture")
        .accessibilityAddTraits(.isImage)
    }

    private var image: NSImage? {
        guard let shot, case .loaded(let data) = store.store.shots[shot.name] else { return nil }
        return Pictures.image(named: shot.name, from: data)
    }
}

/// Pictures decoded once and kept for the session.
@MainActor
enum Pictures {
    private static var decoded: [String: NSImage] = [:]

    static func image(named name: String, from data: Data) -> NSImage? {
        if let image = decoded[name] { return image }
        // Bytes that are not a picture are treated as no picture, which the
        // frame already has a look for.
        guard let image = NSImage(data: data), image.isValid, image.size.width > 0 else { return nil }
        decoded[name] = image
        return image
    }
}

/// The page for one app.
private struct AppPage: View {
    @EnvironmentObject private var store: Companion
    var entry: StoreEntry

    private var busy: Bool { store.apps.job?.running == true || store.apps.loading || store.apps.removing != nil }

    var body: some View {
        let reader = store.appsReader
        let obstacle = StoreWords.obstacle(to: entry, reader: store.chosen.reader, hasCatalog: store.apps.catalog != nil)
        let locked = reader?.writable == false
        Pane(width: Picture.page * 2 + 24) {
            // Wide enough for a screen and a column of reading beside it,
            // the screen leads and the words sit next to it, so what the
            // app does is read without scrolling past a picture. Narrower,
            // the words come first and the screens follow in a row.
            ViewThatFits(in: .horizontal) {
                VStack(alignment: .leading, spacing: 24) {
                    HStack(alignment: .top, spacing: 28) {
                        if let first = entry.shots.first {
                            shown(first)
                        }
                        VStack(alignment: .leading, spacing: 18) {
                            words(obstacle: obstacle, reader: reader, locked: locked)
                        }
                        .frame(minWidth: 280, idealWidth: 280, maxWidth: 460, alignment: .leading)
                    }
                    if entry.shots.count > 1 {
                        // A row that scrolls sideways would like to be as
                        // wide as everything in it. Asked what it needs, it
                        // is made to say very little, or this layout would
                        // never be judged to fit.
                        Gallery(entry: entry, shots: Array(entry.shots.dropFirst()))
                            .frame(idealWidth: 300, maxWidth: .infinity, alignment: .leading)
                    }
                }
                VStack(alignment: .leading, spacing: 18) {
                    heading
                    StoreNotes()
                    actions(obstacle: obstacle, reader: reader, locked: locked)
                    if !entry.shots.isEmpty {
                        Gallery(entry: entry, shots: entry.shots)
                            .padding(.vertical, 6)
                    }
                    about
                }
            }
        }
    }

    private func shown(_ shot: Shot) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Picture(shot: shot, title: entry.title, width: Picture.page)
            if !shot.caption.isEmpty {
                Text(shot.caption)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(width: Picture.page, alignment: .leading)
                    .accessibilityHidden(true)
            }
        }
    }

    @ViewBuilder
    private func words(obstacle: String, reader: Reader?, locked: Bool) -> some View {
        heading
        StoreNotes()
        actions(obstacle: obstacle, reader: reader, locked: locked)
        about
    }

    private var heading: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(entry.title)
                .font(Paper.serif(34))
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            if !entry.summary.isEmpty {
                Prose(entry.summary)
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func actions(obstacle: String, reader: Reader?, locked: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            if reader != nil, obstacle.isEmpty {
                if entry.isInstalled, entry.updateAvailable {
                    Button("Update to \(entry.version)") { Task { await store.installApps([entry.id]) } }
                        .leading()
                        .disabled(busy || locked)
                } else if !entry.isInstalled, entry.offered {
                    Button("Install") { Task { await store.installApps([entry.id]) } }
                        .leading()
                        .disabled(busy || locked)
                }
            }
            if entry.isInstalled, reader != nil {
                Button("Remove…") { Questions.askToRemove(id: entry.id, title: entry.title, store: store) }
                    .disabled(busy || locked)
            }
            if store.apps.removing == entry.id {
                ProgressView().controlSize(.small)
            }
            Text(facts)
                .font(.callout)
                .monospacedDigit()
                .foregroundStyle(.secondary)
        }
        if !obstacle.isEmpty {
            Note(obstacle)
                .frame(maxWidth: Paper.column)
        }
    }

    /// What the app does, what it needs and asks for, what is new, and how
    /// to set it up: everything that is read and not pressed.
    @ViewBuilder
    private var about: some View {
        if !entry.description.isEmpty, entry.description != entry.summary {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(entry.description.components(separatedBy: "\n\n").enumerated()), id: \.offset) { _, paragraph in
                    Prose(paragraph.replacingOccurrences(of: "\n", with: " "), font: Paper.serif(16))
                }
            }
            .frame(maxWidth: Paper.column, alignment: .leading)
        }
        if !entry.needs.isEmpty || entry.minimumCobaltVersion != nil {
            listed("Needs", (entry.minimumCobaltVersion.map { ["Cobalt \($0) or newer"] } ?? []) + entry.needs)
        }
        if !entry.access.isEmpty {
            listed("Access", entry.access)
        }
        if !entry.releaseNotes.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Heading("New in \(entry.version)")
                Prose(entry.releaseNotes, font: Paper.serif(15))
            }
        }
        if !entry.setup.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Heading("Setup")
                ForEach(Array(entry.setup.enumerated()), id: \.offset) { index, step in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("\(index + 1).")
                            .font(Paper.serif(15))
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                            .frame(width: 22, alignment: .trailing)
                        VStack(alignment: .leading, spacing: 5) {
                            Prose(step.text, font: Paper.serif(15))
                            if let command = step.command, !command.isEmpty {
                                Text(command)
                                    .font(.system(.callout, design: .monospaced))
                                    .textSelection(.enabled)
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 4)
                                    .background(Paper.inset, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                            }
                            if let link = step.link, let url = URL(string: link.url), url.scheme == "https" {
                                Link(link.label, destination: url)
                                    .tint(Paper.link)
                            }
                        }
                    }
                }
            }
        }
    }

    /// Version and size: what there is to know before pressing anything.
    private var facts: String {
        var parts: [String] = []
        if let installed = entry.installedVersion {
            parts.append(entry.updateAvailable ? "\(installed) on this reader" : "Version \(installed)")
        } else if !entry.version.isEmpty {
            parts.append("Version \(entry.version)")
        }
        if let bytes = entry.bytes { parts.append(Advice.fileSize(bytes)) }
        return parts.joined(separator: " · ")
    }

    private func listed(_ title: String, _ lines: [String]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Heading(title)
            ForEach(lines, id: \.self) { line in
                Prose(line)
            }
        }
    }
}

/// An app's pictures, side by side, each with what it shows written under
/// it. Nothing moves unless the owner moves it.
///
/// The row is one stop for the keyboard, and the left and right arrows step
/// through it a picture at a time, stopping at the ends: a row that went
/// round from the last picture to the first would leave somebody pressing an
/// arrow unsure whether they had seen them all. A click on the row gives it
/// the keys, as a click on a list does.
private struct Gallery: View {
    var entry: StoreEntry
    var shots: [Shot]
    /// The picture the arrows last went to.
    @State private var at = 0
    @FocusState private var hasKeys: Bool
    @Environment(\.accessibilityReduceMotion) private var still

    var body: some View {
        ScrollViewReader { row in
            ScrollView(.horizontal) {
                HStack(alignment: .top, spacing: 24) {
                    ForEach(shots) { shot in
                        VStack(alignment: .leading, spacing: 8) {
                            Picture(shot: shot, title: entry.title, width: Picture.page)
                            if !shot.caption.isEmpty {
                                Text(shot.caption)
                                    .font(.callout)
                                    .foregroundStyle(.secondary)
                                    .frame(width: Picture.page, alignment: .leading)
                                    .lineLimit(2)
                                    .accessibilityHidden(true)
                            }
                        }
                        .id(shot.id)
                    }
                }
                .padding(.bottom, 10)
            }
            .focusable()
            .focused($hasKeys)
            .simultaneousGesture(TapGesture().onEnded { hasKeys = true })
            .onMoveCommand { direction in
                let by: Int
                switch direction {
                case .left: by = -1
                case .right: by = 1
                default: return
                }
                let next = StoreWords.galleryStep(from: at, by: by, count: shots.count)
                guard next != at, shots.indices.contains(next) else { return }
                at = next
                withAnimation(still ? nil : .easeOut(duration: 0.2)) {
                    row.scrollTo(shots[next].id, anchor: .leading)
                }
            }
            // Another app's page has another row, which starts at its start.
            .onChange(of: entry.id) { _ in at = 0 }
        }
        .accessibilityLabel("Pictures of \(entry.title)")
    }
}

/// An app install as the store shows it above itself: going, finished, or
/// finished with something left undone.
private struct AppsJob: View {
    @EnvironmentObject private var store: Companion
    var job: FollowedJob

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(job.running ? job.title : job.ok == true ? "Finished." : "Not everything was installed.")
                .font(.headline)
            if job.running {
                ProgressView()
                    .progressViewStyle(.linear)
                    .tint(Paper.accent)
                    .accessibilityLabel(job.title)
                if store.state != nil, !(store.state?.readers.contains { $0.volume == job.volume } ?? true) {
                    Text("The reader was disconnected while this was running. Wait for it to stop; it says how far it got.")
                }
            }
            SaidView(blocks: Said.reflow(job.text))
            if !job.running {
                Button("Dismiss") { store.dismissAppsJob() }
                    .controlSize(.small)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Paper.inset, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}
