import AppKit
import CobaltKit
import SwiftUI

/// The steps of a job as a numbered list that advances: what is behind,
/// what is happening, what is still to come.
struct StageList: View {
    var stages: [JobScene.Stage]
    /// False when the step the list has reached is the owner's to take. A
    /// spinner there would say the computer was doing something.
    var working = true

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            ForEach(Array(stages.enumerated()), id: \.offset) { index, stage in
                HStack(alignment: .center, spacing: 8) {
                    Text("\(index + 1).")
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .frame(width: 22, alignment: .trailing)
                    Text(stage.title)
                        .foregroundStyle(stage.state == .waiting ? .secondary : .primary)
                    switch stage.state {
                    case .done:
                        Image(systemName: "checkmark")
                            .font(.callout.weight(.semibold))
                            .foregroundStyle(.secondary)
                    case .current:
                        if working { ProgressView().controlSize(.small) }
                    case .waiting:
                        EmptyView()
                    }
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Step \(index + 1), \(stage.title), \(stage.state == .done ? "done" : stage.state == .current ? "in progress" : "not started")")
            }
        }
    }
}

/// A first install from the moment Cobalt starts being copied to the
/// moment the reader has been found on Wi-Fi and saved. One thing is on
/// screen at a time, and it changes when the server says the last thing
/// has happened; the owner is never left with something to type.
struct SetupPane: View {
    @EnvironmentObject private var store: Companion
    var job: FollowedJob
    var scene: JobScene
    var setup: SetupProgress

    private var mounted: Reader? {
        store.state?.readers.first { $0.volume == job.volume }
    }

    private var heading: String {
        switch setup.stage {
        case .eject: return "Cobalt is on the reader."
        case .finding, .securing: return "Finding the reader"
        case .ready: return readyHeading
        case .notFound: return "The reader was not found on Wi-Fi."
        case .several: return "More than one reader answered."
        case .noNetwork: return "This computer is not on a network."
        default: return scene.heading
        }
    }

    /// A reader the owner named is called by that name. One saved under
    /// the name every first reader gets is "kobo", which is no name at all
    /// at the head of a sentence.
    private var readyHeading: String {
        guard let name = setup.savedAs, name.lowercased() != "kobo" else {
            return "Your Kobo is ready."
        }
        return "\(name) is ready."
    }

    var body: some View {
        Pane {
            Text(heading)
                .font(Paper.serif(28))
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            if setup.stage != .ready, !scene.stages.isEmpty {
                StageList(stages: scene.stages, working: job.running)
            }
            // What did not go as planned and did not stop it, in the
            // server's own sentences, before anything is offered.
            ForEach(setup.notes, id: \.self) { note in
                Note(note)
            }
            switch setup.stage {
            case .eject:
                eject
            case .finding:
                finding
            case .ready:
                ready
            case .notFound, .several, .noNetwork:
                LookAgain(stage: setup.stage)
            default:
                Text(scene.elapsed)
                    .font(.callout)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                if !scene.waiting.isEmpty { Prose(scene.waiting) }
                if !scene.pulled.isEmpty { Note(scene.pulled, kind: .caution) }
                if !scene.quiet.isEmpty { Note(scene.quiet) }
            }
            if !store.reader.error.isEmpty, setup.stage == .eject {
                Note(store.reader.error, kind: .caution)
            }
            if !scene.said.isEmpty {
                Folded(title: "Details") { SaidView(blocks: scene.said) }
            }
        }
    }

    /// Copied, and the reader will not restart into it until it has been
    /// ejected and unplugged.
    @ViewBuilder
    private var eject: some View {
        if let reader = mounted {
            if store.computer?.ejects == true {
                Prose("Eject the reader, then unplug it.")
                Button("Eject") { Task { await store.ejectThenLook(volume: reader.volume) } }
                    .leading()
                    .keyboardShortcut(.defaultAction)
                    .disabled(store.reader.ejecting)
            } else {
                Prose("Eject the reader in the Finder, then unplug it.")
            }
        } else {
            Button("Look for the Reader") { Task { await store.lookAgain() } }
                .leading()
                .keyboardShortcut(.defaultAction)
        }
    }

    @ViewBuilder
    private var finding: some View {
        VStack(alignment: .leading, spacing: 6) {
            ProgressView(value: Double(min(setup.waited, setup.limit)), total: Double(max(setup.limit, 1)))
                .tint(Paper.accent)
                .accessibilityLabel("Looking for the reader")
            Text("\(Said.elapsed(TimeInterval(setup.waited))) of \(Said.elapsed(TimeInterval(setup.limit)))")
                .font(.callout)
                .monospacedDigit()
                .foregroundStyle(.secondary)
        }
        VStack(alignment: .leading, spacing: 8) {
            Heading("On the reader")
            SaidView(blocks: FirstRun.onTheReader(menuCorner: corner).enumerated().map { .step($0.offset + 1, $0.element) })
        }
        if setup.waited * 2 >= setup.limit {
            // Half the time has gone. What is most often missing is said
            // while there is still time to do it.
            Note("It has not answered yet. It has to be awake, on the same Wi-Fi as this computer, and showing Cobalt.")
        }
    }

    private var corner: String? {
        store.state?.readers.first?.menuCorner ?? store.lastMenuCorner
    }

    @ViewBuilder
    private var ready: some View {
        if setup.secured == false {
            Note("This computer could not confirm that it is the only one allowed to manage the reader.", kind: .caution)
        }
        VStack(alignment: .leading, spacing: 10) {
            Heading("Next")
            Button("Add Apps") { leave(for: .apps) }
            Button("Add Send to Kobo to a Browser") { leave(for: .browser) }
            Button("Send an Article or a Book") { leave(for: .send) }
        }
        Button("Done") { Task { await store.dismissReaderJob() } }
            .leading()
            .keyboardShortcut(.defaultAction)
    }

    private func leave(for place: Place) {
        Task {
            await store.dismissReaderJob()
            store.place = place
        }
    }
}

/// Why the reader was not found and the one thing to do about it.
struct LookAgain: View {
    @EnvironmentObject private var store: Companion
    var stage: SetupStage

    var body: some View {
        let causes = FirstRun.notFoundCauses(stage)
        // One thing to do is a sentence; several possible causes are a list.
        if causes.count == 1 {
            Prose(causes[0])
        } else {
            SaidView(blocks: causes.map { .item($0) })
        }
        if stage == .notFound {
            Button("Open Local Network Settings") { NSWorkspace.shared.open(Privacy.localNetwork(origin: store.origin).settings) }
        }
        if !store.reader.error.isEmpty {
            Note(store.reader.error, kind: .caution)
        }
        HStack {
            Spacer()
            Button("Look Again") { Task { await store.lookAgain() } }
                .leading()
                .keyboardShortcut(.defaultAction)
        }
    }
}

/// The readers this computer is linked to, each by the name it sends to.
struct SavedReaders: View {
    @EnvironmentObject private var store: Companion
    @EnvironmentObject private var desk: Desk

    var body: some View {
        if !store.saved.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                Heading("Saved readers")
                    .padding(.bottom, 8)
                ForEach(store.saved) { link in
                    Divider().overlay(Paper.rule)
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Text(link.name)
                            .font(Paper.serif(17))
                        if link.volume != nil {
                            Text("Connected")
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 8)
                        Button("Rename…") { Questions.askToRename(link.name, store: store) }
                            .accessibilityLabel("Rename \(link.name)")
                        Button("Forget…") { Questions.askToForget(link.name, store: store) }
                            .accessibilityLabel("Forget \(link.name)")
                    }
                    .controlSize(.small)
                    .padding(.vertical, 9)
                }
                Divider().overlay(Paper.rule)
                Button("Add Another Reader") { desk.addingReader = true }
                    .padding(.top, 12)
            }
            .padding(.top, 10)
        }
    }
}

/// The first step of the path, shown to an owner who has asked to add a
/// reader while another is plugged in or saved. It gives way by itself
/// when a new reader appears.
struct AddReader: View {
    @EnvironmentObject private var store: Companion
    @EnvironmentObject private var desk: Desk

    var body: some View {
        Pane {
            HStack(alignment: .center, spacing: 18) {
                ReaderDrawing(model: "A reader", screen: .blank, present: false)
                Text("Add another reader")
                    .font(Paper.serif(24))
                    .accessibilityAddTraits(.isHeader)
            }
            Prose("Plug in the Kobo with a cable that carries data and tap Connect on its screen.")
            Button("Back") { desk.addingReader = false }
                .keyboardShortcut(.cancelAction)
        }
        .onChange(of: store.state?.readers.count ?? 0) { _ in desk.addingReader = false }
    }
}

extension Companion {
    /// Where the menu is on the reader last seen plugged in, kept for the
    /// steps shown after it has been unplugged.
    var lastMenuCorner: String? {
        UserDefaults.standard.string(forKey: "menuCorner")
    }
}

extension Questions {
    static func renaming(_ name: String) -> (NSAlert, NSTextField) {
        let alert = NSAlert()
        alert.messageText = "Rename “\(name)”"
        alert.addButton(withTitle: "Rename")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(string: name)
        field.frame = NSRect(x: 0, y: 0, width: 228, height: 22)
        field.setAccessibilityLabel("New name")
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        return (alert, field)
    }

    static func forgetting(_ name: String) -> NSAlert {
        let alert = NSAlert()
        alert.messageText = "Forget “\(name)”?"
        alert.informativeText = "Cobalt stays on the reader. This computer stops sending to it."
        alert.addButton(withTitle: "Forget")
        alert.addButton(withTitle: "Cancel")
        alert.buttons.first?.hasDestructiveAction = true
        return alert
    }

    static func askToRename(_ name: String, store: Companion) {
        let (alert, field) = renaming(name)
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else { return }
        alert.beginSheetModal(for: window) { response in
            let new = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard response == .alertFirstButtonReturn, !new.isEmpty, new != name else { return }
            Task { await store.rename(saved: name, to: new) }
        }
    }

    static func askToForget(_ name: String, store: Companion) {
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else { return }
        forgetting(name).beginSheetModal(for: window) { response in
            guard response == .alertFirstButtonReturn else { return }
            Task { await store.forget(saved: name) }
        }
    }
}
