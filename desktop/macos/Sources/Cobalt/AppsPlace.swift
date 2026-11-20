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
        .task { if store.store.store == nil { await store.loadStore() } }
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
        Pane(width: 760) {
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
                            Button("Update All") { Task { await store.installApps(front.updates) } }
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
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 132, maximum: 176), spacing: 20, alignment: .top)], alignment: .leading, spacing: 22) {
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
            VStack(alignment: .leading, spacing: 8) {
                Picture(shot: entry.shots.first, title: entry.title)
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.title)
                        .font(Paper.serif(15))
                        .lineLimit(1)
                    if !entry.summary.isEmpty {
                        Text(entry.summary)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .lineLimit(3)
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
            .frame(maxWidth: .infinity, alignment: .topLeading)
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
    @EnvironmentObject private var store: Companion
    var shot: Shot?
    var title: String

    private var proportions: CGFloat {
        guard let shot, shot.height > 0 else { return 1072.0 / 1448.0 }
        return CGFloat(shot.width) / CGFloat(shot.height)
    }

    var body: some View {
        ZStack {
            Rectangle().fill(Color.white.opacity(image == nil ? 0 : 1))
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .interpolation(.high)
                    .aspectRatio(contentMode: .fit)
            } else {
                Text(title)
                    .font(Paper.serif(12))
                    .foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
                    .padding(8)
            }
        }
        .aspectRatio(proportions, contentMode: .fit)
        // Never drawn larger than it is, which on a screen of twice the
        // density is half its pixels in points.
        .frame(maxWidth: shot.map { CGFloat($0.width) / 2 } ?? 536)
        .overlay(Rectangle().strokeBorder(Paper.rule, lineWidth: 1))
        .task(id: shot?.name) {
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
        Pane(width: 680) {
            VStack(alignment: .leading, spacing: 6) {
                Text(entry.title)
                    .font(Paper.serif(28))
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)
                if !entry.summary.isEmpty {
                    Prose(entry.summary)
                        .foregroundStyle(.secondary)
                }
            }
            StoreNotes()
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
            }
            if !entry.shots.isEmpty {
                Gallery(entry: entry)
            }
            if !entry.description.isEmpty, entry.description != entry.summary {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(entry.description.components(separatedBy: "\n\n").enumerated()), id: \.offset) { _, paragraph in
                        Prose(paragraph.replacingOccurrences(of: "\n", with: " "), font: Paper.serif(15))
                    }
                }
            }
            if !entry.needs.isEmpty {
                listed("Needs", entry.needs)
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
private struct Gallery: View {
    var entry: StoreEntry

    var body: some View {
        ScrollView(.horizontal) {
            HStack(alignment: .top, spacing: 16) {
                ForEach(entry.shots) { shot in
                    VStack(alignment: .leading, spacing: 6) {
                        Picture(shot: shot, title: entry.title)
                            .frame(width: 230)
                        if !shot.caption.isEmpty {
                            Text(shot.caption)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .frame(width: 230, alignment: .leading)
                                .lineLimit(3)
                                .accessibilityHidden(true)
                        }
                    }
                    .focusable()
                }
            }
            .padding(.bottom, 10)
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
