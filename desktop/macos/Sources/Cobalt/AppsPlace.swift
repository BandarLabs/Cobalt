import AppKit
import CobaltKit
import SwiftUI

/// The Apps place: what is on the reader, what the signed catalog offers,
/// and installing, updating and removing.
struct AppsPlace: View {
    @EnvironmentObject private var store: Companion

    var body: some View {
        Group {
            if let reader = store.appsReader {
                AppList(reader: reader)
                    // Read when the place is opened on a reader, and again
                    // when the reader or its Cobalt changes under it.
                    .task(id: store.apps.key) {
                        if store.apps.installed == nil, store.apps.problem == nil { await store.loadApps() }
                    }
            } else {
                noReader
            }
        }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await store.loadApps() }
                } label: {
                    Label("Check Again", systemImage: "arrow.clockwise")
                }
                .help("Read the reader's apps and fetch the catalog again")
                .disabled(store.appsReader == nil || store.apps.loading || store.apps.job?.running == true)
            }
        }
    }

    @ViewBuilder
    private var noReader: some View {
        if store.state == nil {
            Standstill(headline: "Looking for a reader.", detail: "", working: true) {}
        } else {
            switch store.chosen {
            case .none:
                Standstill(headline: "No reader is connected.", detail: "Plug in your Kobo and tap Connect on its screen.") {}
            case .several:
                Standstill(headline: "More than one reader is connected.", detail: "Choose one in Reader.") {
                    Button("Go to Reader") { store.place = .reader }
                }
            case .one:
                Standstill(headline: "This reader does not have Cobalt yet.", detail: "Install it in Reader.") {
                    Button("Go to Reader") { store.place = .reader }
                }
            }
        }
    }
}

private struct AppList: View {
    @EnvironmentObject private var store: Companion
    var reader: Reader

    private var busy: Bool { store.apps.job?.running == true || store.apps.loading || store.apps.removing != nil }
    private var locked: Bool { reader.writable == false }

    var body: some View {
        let shelf = store.shelf
        let query = store.apps.query.trimmingCharacters(in: .whitespacesAndNewlines)
        // The same column of paper as the other places, with the apps as
        // ruled rows down it. A table would bring a selection that means
        // nothing here and a background of its own under the toolbar.
        Pane {
            notes
            if store.apps.installed == nil {
                if store.apps.problem == nil {
                    waiting("Reading the apps on the reader.")
                }
            } else if !query.isEmpty, shelf.installed.isEmpty, shelf.available.isEmpty {
                Text("No app matches “\(query)”.")
                    .font(Paper.serif(17))
            } else {
                section("On this reader") {
                    if shelf.updates.count > 1, query.isEmpty {
                        Button("Update All") { Task { await store.installApps(shelf.updates) } }
                            .controlSize(.small)
                            .disabled(busy || locked)
                            .accessibilityLabel("Update all \(shelf.updates.count) apps")
                    }
                } rows: {
                    if shelf.installed.isEmpty {
                        quiet(query.isEmpty ? "No apps yet." : "None of them match.")
                    }
                    ForEach(shelf.installed) { listed in
                        InstalledRow(listed: listed, busy: busy || locked)
                        Divider().overlay(Paper.rule)
                    }
                }
                if store.apps.catalog != nil {
                    section("Available") {
                        EmptyView()
                    } rows: {
                        if shelf.available.isEmpty {
                            quiet(query.isEmpty ? "Everything published is on this reader." : "None of them match.")
                        }
                        ForEach(shelf.available) { listed in
                            AvailableRow(listed: listed, reader: reader, busy: busy || locked)
                            Divider().overlay(Paper.rule)
                        }
                    }
                } else if store.apps.problem == nil {
                    waiting("Fetching the signed app catalog.")
                }
            }
        }
    }

    private func section<Trailing: View, Rows: View>(_ title: String, @ViewBuilder trailing: () -> Trailing, @ViewBuilder rows: () -> Rows) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Heading(title)
                Spacer()
                trailing()
            }
            .padding(.bottom, 8)
            Divider().overlay(Paper.rule)
            rows()
        }
    }

    @ViewBuilder
    private var notes: some View {
        if locked {
            Note(Storage.readOnly, kind: .caution)
        }
        if let job = store.apps.job {
            AppsJob(job: job)
        }
        if store.apps.job == nil, !store.apps.said.isEmpty {
            Note(store.apps.said, kind: .done)
        }
        if let refusal = store.apps.refusal {
            Note(refusal.sentence, kind: .caution) {
                Button("Dismiss") { store.dismissAppsNote() }
                    .controlSize(.small)
            }
        }
        if let problem = store.apps.problem {
            Note(problem.sentence, kind: .caution) {
                Button("Try Again") { Task { await store.loadApps() } }
                    .controlSize(.small)
            }
        }
        if store.apps.changed, store.apps.job?.running != true {
            Note("Eject the reader before you unplug it.") {
                if store.computer?.ejects == true {
                    Button("Eject Now") { Task { await store.eject(volume: reader.volume, from: .apps) } }
                        .controlSize(.small)
                        .disabled(store.reader.ejecting)
                }
            }
        }
    }

    private func waiting(_ text: String) -> some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(text).foregroundStyle(.secondary)
        }
    }

    private func quiet(_ text: String) -> some View {
        Text(text)
            .foregroundStyle(.secondary)
            .padding(.vertical, 10)
    }
}

/// An app install as the list shows it above itself: going, finished, or
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
                    .accessibilityLabel(job.title)
                if store.state != nil, !(store.state?.readers.contains { $0.volume == job.volume } ?? true) {
                    Text("The reader was disconnected while this was running. Wait for it to stop; it says how far it got.")
                        .fixedSize(horizontal: false, vertical: true)
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

private struct AppTitle: View {
    var title: String
    var version: String
    var summary: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(title)
                    .font(Paper.serif(16))
                Text(version)
                    .font(.caption)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            if !summary.isEmpty {
                Text(summary)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

private struct InstalledRow: View {
    @EnvironmentObject private var store: Companion
    var listed: ListedApp
    var busy: Bool

    var body: some View {
        let app = listed.app
        let held = app.updateAvailable && !app.runsOnThisReader
        HStack(alignment: .center, spacing: 12) {
            AppTitle(
                title: app.title,
                version: app.installedVersion ?? app.version,
                summary: held ? "Version \(app.version) needs Cobalt \(app.minimumCobaltVersion ?? "") or newer." : app.summary
            )
            Spacer(minLength: 8)
            if store.apps.removing == app.id {
                ProgressView().controlSize(.small)
            }
            if app.updateAvailable, app.runsOnThisReader {
                Button("Update to \(app.version)") { Task { await store.installApps([app.id]) } }
                    .disabled(busy)
                    .accessibilityLabel("Update \(app.title) to \(app.version)")
            }
            Button("Remove…") { Questions.askToRemove(app, store: store) }
                .disabled(busy)
                .accessibilityLabel("Remove \(app.title)")
        }
        .controlSize(.small)
        .padding(.vertical, 9)
        .contentShape(Rectangle())
        .contextMenu {
            if app.updateAvailable, app.runsOnThisReader {
                Button("Update to \(app.version)") { Task { await store.installApps([app.id]) } }.disabled(busy)
            }
            Button("Remove…") { Questions.askToRemove(app, store: store) }.disabled(busy)
        }
    }
}

private struct AvailableRow: View {
    @EnvironmentObject private var store: Companion
    var listed: ListedApp
    var reader: Reader
    var busy: Bool

    var body: some View {
        let app = listed.app
        let obstacle = Advice.obstacle(to: app, on: reader)
        HStack(alignment: .center, spacing: 12) {
            AppTitle(title: app.title, version: app.version, summary: app.summary)
            Spacer(minLength: 8)
            if obstacle.isEmpty {
                if let bytes = app.bytes {
                    Text(Advice.fileSize(bytes))
                        .font(.caption)
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                Button("Install") { Task { await store.installApps([app.id]) } }
                    .disabled(busy)
                    .accessibilityLabel("Install \(app.title)")
            } else {
                Text(obstacle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.trailing)
                    .frame(maxWidth: 150, alignment: .trailing)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .controlSize(.small)
        .padding(.vertical, 9)
    }
}
