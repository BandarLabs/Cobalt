import AppKit
import CobaltKit
import SwiftUI

/// The Reader place: the reader that is plugged in, what stands between it
/// and Cobalt, the plan before anything is written, and the work itself.
struct ReaderPlace: View {
    @EnvironmentObject private var store: Companion
    @EnvironmentObject private var desk: Desk

    var body: some View {
        Group {
            if let job = store.reader.job {
                JobPane(job: job)
            } else if desk.addingReader, store.state != nil {
                AddReader()
            } else if store.state == nil {
                Standstill(headline: "Looking for a reader.", detail: "", working: true) {}
            } else if store.awaitingReader, store.chosen == .none {
                // Cobalt was installed from here and the reader has not
                // been found since. The window was closed in between, and
                // this is where it was.
                Pane {
                    Text("The reader has not been found on Wi-Fi yet.")
                        .font(Paper.serif(24))
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityAddTraits(.isHeader)
                    LookAgain(stage: .notFound)
                }
            } else {
                switch store.chosen {
                case .none:
                    NoReader()
                case .several(let readers):
                    SeveralReaders(readers: readers)
                case .one(let reader, let among):
                    if let open = store.reader.plan, open.volume == reader.volume {
                        PlanPane(reader: reader, plan: open.plan)
                    } else {
                        OneReader(reader: reader, among: !among.isEmpty)
                    }
                }
            }
        }
        .onChange(of: store.chosen.reader?.menuCorner) { corner in
            if let corner { UserDefaults.standard.set(corner, forKey: "menuCorner") }
        }
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                if case .one(_, let among) = store.chosen, !among.isEmpty, store.reader.job == nil {
                    Button {
                        store.chooseAnother()
                    } label: {
                        Label("Choose Another Reader", systemImage: "arrow.left.arrow.right")
                    }
                    .help("Choose another of the readers that are connected")
                }
                if let reader = store.chosen.reader, store.computer?.ejects == true {
                    Button {
                        Task { await store.eject(volume: reader.volume) }
                    } label: {
                        Label("Eject", systemImage: "eject")
                    }
                    .help("Eject the \(reader.model) so it can be unplugged")
                    .disabled(store.reader.ejecting || store.running.contains { $0.kind == .reader })
                }
            }
        }
    }
}

/// What to do when no reader is showing, which depends on why.
private struct NoReader: View {
    @EnvironmentObject private var store: Companion

    var body: some View {
        Pane {
            switch Advice.absence(store.state) {
            case .blocked(let volume):
                let advice = Privacy.blocked(volume: volume, origin: store.origin)
                title("A reader", screen: .blank, present: true, headline: advice.headline)
                Prose(advice.detail)
                Button(advice.button) { NSWorkspace.shared.open(advice.settings) }
            case .unconnected:
                title("A reader", screen: .connect, present: true, headline: "The reader is plugged in but has not connected.")
                Prose("Tap Connect on its screen, or unplug the cable and plug it in again if the question has gone.")
                TimelineView(.periodic(from: .now, by: 2)) { _ in
                    if store.connectStuck {
                        SaidView(blocks: FirstRun.connectCauses.map { .item($0) })
                    }
                }
            case .none, .present:
                title("A reader", screen: .blank, present: false, headline: "No reader is connected.")
                if !store.reader.notice.isEmpty {
                    Note(store.reader.notice, kind: .done)
                }
                Prose("Plug in your Kobo with a cable that carries data and tap Connect on its screen.")
                SavedReaders()
            }
        }
    }

    private func title(_ model: String, screen: ReaderDrawing.Screen, present: Bool, headline: String) -> some View {
        HStack(alignment: .center, spacing: 18) {
            ReaderDrawing(model: model, screen: screen, present: present)
            Text(headline)
                .font(Paper.serif(24))
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
        }
        .padding(.bottom, 2)
    }
}

/// Two readers or more: nothing is done until one is chosen.
private struct SeveralReaders: View {
    @EnvironmentObject private var store: Companion
    var readers: [Reader]

    var body: some View {
        Pane {
            Text("\(readers.count) readers are connected.")
                .font(Paper.serif(24))
                .accessibilityAddTraits(.isHeader)
            VStack(spacing: 0) {
                ForEach(readers) { reader in
                    Divider().overlay(Paper.rule)
                    HStack(alignment: .center, spacing: 14) {
                        ReaderDrawing(model: reader.model, screen: screen(of: reader), scale: 0.66)
                            .frame(width: 46)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(reader.model)
                                .font(Paper.serif(17))
                            Text(summary(of: reader))
                                .font(.callout)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        Spacer(minLength: 8)
                        Button("Choose") { store.choose(volume: reader.volume) }
                            .accessibilityLabel("Choose the \(reader.model) at \(reader.volume)")
                    }
                    .padding(.vertical, 10)
                }
                Divider().overlay(Paper.rule)
            }
        }
    }

    private func summary(of reader: Reader) -> String {
        let cobalt = reader.cobalt.map { "Cobalt \($0.cobaltVersion)" } ?? "No Cobalt"
        return "\(cobalt) · \(reader.volume)"
    }
}

private func screen(of reader: Reader) -> ReaderDrawing.Screen {
    if let cobalt = reader.cobalt { return .cobalt(cobalt.cobaltVersion) }
    return .page
}

/// One reader: what it is, where it stands, and what can be done with it.
private struct OneReader: View {
    @EnvironmentObject private var store: Companion
    var reader: Reader
    var among: Bool

    var body: some View {
        let advice = Advice.standing(of: reader, on: store.computer)
        let locked = reader.writable == false
        Pane {
            HStack(alignment: .top, spacing: 20) {
                ReaderDrawing(model: reader.model, screen: screen(of: reader))
                VStack(alignment: .leading, spacing: 10) {
                    Text(reader.model)
                        .font(Paper.serif(28))
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityAddTraits(.isHeader)
                    facts
                }
            }
            // A reader in order is its facts and its buttons. A sentence is
            // added only when something cannot be done, and it is the reason.
            if !advice.reason.isEmpty {
                Note(advice.reason, kind: reader.cobaltProblem == nil ? .plain : .caution)
            }
            if linking {
                Note("This computer has not been linked to this reader, so it cannot send to it.")
            }
            if advice.primary != nil, let untested = reader.untested, !untested.isEmpty {
                Note(Said.sentence(untested))
            }
            if locked {
                Note(Storage.readOnly, kind: .caution)
            }
            if let attached = store.state?.attached, attached > (store.state?.readers.count ?? 0) {
                Note("Another Kobo is plugged in and has not connected. Tap Connect on its screen to see it here.")
            }
            if !store.reader.error.isEmpty {
                Note(store.reader.error, kind: .caution)
            }
            HStack(spacing: 10) {
                switch advice.primary {
                case .install:
                    Button("Install Cobalt…") { plan(false) }
                        .leading()
                        .keyboardShortcut(.defaultAction)
                        .disabled(locked)
                case .update:
                    Button("Update to \(store.computer?.release ?? "")…") { plan(false) }
                        .leading()
                        .keyboardShortcut(.defaultAction)
                        .disabled(locked)
                case nil:
                    if linking {
                        Button("Link This Computer…") { plan(false) }
                            .leading()
                            .keyboardShortcut(.defaultAction)
                            .disabled(locked)
                    }
                }
                if advice.repair, !linking {
                    Button("Install Again…") { plan(false) }
                        .disabled(locked)
                }
                if advice.removable {
                    Button("Remove Cobalt…") { plan(true) }
                        .disabled(locked)
                }
                if store.reader.planning {
                    ProgressView()
                        .controlSize(.small)
                        .accessibilityLabel("Reading what would change")
                }
            }
            .disabled(store.reader.planning)
            if reader.cobalt != nil {
                Pointer(title: "Show the apps on this reader") { store.place = .apps }
            }
            SavedReaders()
        }
    }

    private var linking: Bool { store.needsLinking(reader) }

    private var facts: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 14, verticalSpacing: 4) {
            if let firmware = reader.firmware, !firmware.isEmpty {
                fact("Firmware", firmware)
            }
            fact("Cobalt", cobalt)
            fact("This computer has", store.computer?.release.map { "Cobalt \($0)" } ?? "No release")
            if let free = reader.freeBytes {
                fact("Free space", Advice.fileSize(free))
            }
            fact("Connected at", reader.volume)
        }
        .font(.callout)
    }

    private var cobalt: String {
        if let problem = reader.cobaltProblem, !problem.isEmpty { return "Damaged" }
        guard let cobalt = reader.cobalt else { return "Not installed" }
        return "\(cobalt.cobaltVersion)\(cobalt.channel.map { ", \($0) channel" } ?? "")"
    }

    private func fact(_ name: String, _ value: String) -> some View {
        GridRow {
            Text(name)
                .foregroundStyle(.secondary)
                .gridColumnAlignment(.leading)
            Text(value)
                .monospacedDigit()
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        }
        .accessibilityElement(children: .combine)
    }

    private func plan(_ undo: Bool) {
        Task { await store.openPlan(undo: undo) }
    }
}

/// The dry run, shown before anything is written, with the one button that
/// writes it.
private struct PlanPane: View {
    @EnvironmentObject private var store: Companion
    var reader: Reader
    var plan: Plan
    @State private var starting = false

    private var release: String { plan.release ?? store.computer?.release ?? "" }

    private var linking: Bool { !plan.undo && store.needsLinking(reader) }

    private var title: String {
        plan.undo ? "What removing Cobalt changes" : linking ? "What linking this computer changes" : "What installing Cobalt \(release) changes"
    }

    /// The button says what will happen in the words the reader's standing
    /// was described in, so installing the same release again is not called
    /// an update.
    private var verb: String {
        if plan.undo { return "Remove Cobalt" }
        if linking { return "Link This Computer" }
        guard reader.cobalt != nil else { return "Install Cobalt \(release)" }
        return reader.standing == .current ? "Install Again" : "Update to \(release)"
    }

    var body: some View {
        Pane {
            VStack(alignment: .leading, spacing: 6) {
                Text(title)
                    .font(Paper.serif(24))
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)
                Text("On the \(reader.model) at \(reader.volume).")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            // What it comes to, in the owner's words. The dry run's own
            // lines name files and settings, and are there for whoever
            // wants them and not in the way of whoever does not.
            SaidView(blocks: FirstRun.plan(undo: plan.undo).map { .item($0) })
            if !plan.undo {
                Note(FirstRun.localNetworkAhead)
            }
            Folded(title: "Exactly what changes") {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(plan.lines.enumerated()), id: \.offset) { index, line in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text("\(index + 1).")
                                .monospacedDigit()
                                .foregroundStyle(.secondary)
                                .frame(width: 22, alignment: .trailing)
                            Text(Said.planStep(line))
                                .fixedSize(horizontal: false, vertical: true)
                                .textSelection(.enabled)
                        }
                    }
                }
            }
            if let untested = plan.untested, !untested.isEmpty {
                Note(Said.sentence(untested))
            }
            if !store.reader.error.isEmpty {
                Note(store.reader.error, kind: .caution)
            }
            HStack(spacing: 10) {
                Spacer()
                if starting {
                    ProgressView().controlSize(.small)
                }
                Button("Cancel", role: .cancel) { store.closePlan() }
                    .keyboardShortcut(.cancelAction)
                Button(verb) {
                    starting = true
                    Task {
                        await store.agreeToPlan()
                        starting = false
                    }
                }
                .leading()
                .keyboardShortcut(.defaultAction)
                .disabled(starting)
            }
            .padding(.top, 4)
        }
    }
}

/// A reader job: going, done, or failed with its reason.
///
/// What it said is led by the steps for the owner once it has got to them,
/// with the report of what it did folded away beneath, so that somebody
/// waiting to be told what to do with the reader is told first.
private struct JobPane: View {
    @EnvironmentObject private var store: Companion
    var job: FollowedJob

    var body: some View {
        // The clock is read through TimelineView so the count of seconds
        // moves while nothing else in the record is changing.
        TimelineView(.periodic(from: .now, by: 1)) { _ in
            let scene = JobScene.scene(for: job, state: store.state, now: store.clock)
            if let setup = job.setup, setup.stage != .failed, setup.stage != .unknown, !job.failed || setup.stage != .writing {
                SetupPane(job: job, scene: scene, setup: setup)
            } else {
              Pane {
                Text(scene.heading)
                    .font(Paper.serif(24))
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)
                switch scene.phase {
                case .running: running(scene)
                case .done: done(scene)
                case .failed: failed(scene)
                }
              }
            }
        }
    }

    @ViewBuilder
    private func running(_ scene: JobScene) -> some View {
        if scene.stages.isEmpty {
            ProgressView()
                .progressViewStyle(.linear)
                .tint(Paper.accent)
                .accessibilityLabel(scene.heading)
        } else {
            // Which of four things is happening, not a log of them.
            StageList(stages: scene.stages)
        }
        Text(scene.elapsed)
            .font(.callout)
            .monospacedDigit()
            .foregroundStyle(.secondary)
            .accessibilityLabel("Running for \(scene.elapsed)")
        if !scene.waiting.isEmpty {
            Prose(scene.waiting)
        }
        if !scene.pulled.isEmpty {
            Note(scene.pulled, kind: .caution)
        }
        if !scene.quiet.isEmpty {
            Note(scene.quiet)
        }
        if scene.turned, !scene.stages.isEmpty {
            onTheReader
        }
        if !scene.said.isEmpty {
            Folded(title: "Everything it has said") { SaidView(blocks: scene.said) }
        }
    }

    /// What to do with the reader after an install, in the owner's words.
    /// A kobo command too old to report its stages still prints its own
    /// account of these steps; that stays behind the disclosure with the
    /// rest of what it said.
    private var onTheReader: some View {
        VStack(alignment: .leading, spacing: 8) {
            Heading("On the reader")
            SaidView(blocks: FirstRun.onTheReader(menuCorner: store.state?.readers.first { $0.volume == job.volume }?.menuCorner ?? store.lastMenuCorner).enumerated().map { .step($0.offset + 1, $0.element) })
        }
    }

    @ViewBuilder
    private func done(_ scene: JobScene) -> some View {
        if let reader = scene.connected {
            // A reader still mounted after a finished job has not seen what
            // was written to it, and pulling the cable now can lose the end
            // of it.
            Note("The reader is still connected. Eject it before you unplug it.") {
                if store.computer?.ejects == true {
                    Button("Eject Now") { Task { await store.eject(volume: reader.volume) } }
                        .disabled(store.reader.ejecting)
                }
            }
        }
        if !store.reader.notice.isEmpty {
            Note(store.reader.notice, kind: .done)
        }
        if !store.reader.error.isEmpty {
            Note(store.reader.error, kind: .caution)
        }
        if !scene.stages.isEmpty {
            onTheReader
        }
        if !scene.said.isEmpty {
            Folded(title: "Everything it said") { SaidView(blocks: scene.said) }
        }
        HStack {
            Spacer()
            Button("Done") { Task { await store.dismissReaderJob() } }
                .leading()
                .keyboardShortcut(.defaultAction)
        }
    }

    @ViewBuilder
    private func failed(_ scene: JobScene) -> some View {
        Note(scene.reason, detail: scene.aftermath, kind: .caution)
        Folded(title: "Everything it said") { SaidView(blocks: scene.next) }
        HStack(spacing: 10) {
            Spacer()
            Button("Back") { Task { await store.dismissReaderJob() } }
                .keyboardShortcut(.cancelAction)
            if job.again != nil {
                Button("Try Again") { Task { await store.tryReaderJobAgain() } }
                    .leading()
                    .keyboardShortcut(.defaultAction)
            }
        }
    }
}
