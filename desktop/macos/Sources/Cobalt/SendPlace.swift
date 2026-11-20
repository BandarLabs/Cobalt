import AppKit
import CobaltKit
import SwiftUI
import UniformTypeIdentifiers

/// The Browser place: Send to Kobo in each browser found, and the one step
/// each still needs.
struct BrowserPlace: View {
    @EnvironmentObject private var store: Companion

    var body: some View {
        Pane {
            if let problem = store.browser.problem {
                Note(problem.sentence, kind: .caution)
            }
            if let refusal = store.browser.refusal {
                Note(refusal.sentence, kind: .caution)
            }
            if let list = store.browser.list {
                if list.isEmpty {
                    Note("Install Chrome, Edge, Brave, Chromium or Firefox to use Send to Kobo.")
                } else {
                    VStack(spacing: 0) {
                        ForEach(list) { browser in
                            Divider().overlay(Paper.rule)
                            BrowserRow(browser: browser)
                        }
                        Divider().overlay(Paper.rule)
                    }
                }
            } else if store.browser.problem == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Looking for browsers.").foregroundStyle(.secondary)
                }
            }
        }
        .task { await store.loadBrowsers() }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await store.loadBrowsers() }
                } label: {
                    Label("Check Again", systemImage: "arrow.clockwise")
                }
                .help("Look for browsers again")
            }
        }
    }
}

private struct BrowserRow: View {
    @EnvironmentObject private var store: Companion
    var browser: Browser
    @State private var copied = false

    var body: some View {
        let step = Advice.browserStep(browser.step, title: browser.title)
        let said = store.browser.said[browser.id] ?? ""
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(browser.title)
                    .font(Paper.serif(17))
                Text(browser.setUp ? "Set up" : "Not set up")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                if store.browser.busy == browser.id {
                    ProgressView().controlSize(.small)
                }
                Button(browser.setUp ? "Remove" : "Set Up") {
                    Task { await store.change(browser: browser, setUp: !browser.setUp) }
                }
                .disabled(store.browser.busy != nil)
                // What is being set up is said here, where it is set up,
                // and nowhere above the list.
                .help(browser.setUp ? "Take Send to Kobo out of \(browser.title)" : "Add Send to Kobo to \(browser.title)’s right-click menu: send this page, this link or the selection to the reader")
                .accessibilityLabel(browser.setUp ? "Remove Send to Kobo from \(browser.title)" : "Set up Send to Kobo in \(browser.title)")
            }
            if browser.setUp, !step.instruction.isEmpty {
                Prose(step.paths.isEmpty ? step.instruction : "One step is left, in \(browser.title) itself. \(step.instruction)")
                ForEach(step.paths, id: \.self) { path in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(path)
                            .font(.system(.callout, design: .monospaced))
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 8)
                        Button(copied ? "Copied" : "Copy") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(path, forType: .string)
                            copied = true
                            Task {
                                try? await Task.sleep(nanoseconds: 2_000_000_000)
                                copied = false
                            }
                        }
                        .controlSize(.small)
                        .accessibilityLabel("Copy the folder’s path")
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(Paper.inset, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                }
                ForEach(step.notes, id: \.self) { note in
                    Text(note)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            if !said.isEmpty {
                Text(said)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 11)
    }
}

/// The Send place: a file, a web article, and what is waiting for the
/// reader to wake.
struct SendPlace: View {
    @EnvironmentObject private var store: Companion
    @EnvironmentObject private var desk: Desk
    @State private var address = ""
    @FocusState private var addressFocused: Bool

    private var busy: Bool { store.send.job?.running == true }

    var body: some View {
        Pane {
            destination
            VStack(alignment: .leading, spacing: 10) {
                Heading("A file")
                FilePart()
            }
            Divider().overlay(Paper.rule)
            VStack(alignment: .leading, spacing: 10) {
                Heading("A web article")
                HStack(spacing: 8) {
                    TextField("The page’s address", text: $address, prompt: Text("https://"))
                        .textFieldStyle(.roundedBorder)
                        .focused($addressFocused)
                        .onSubmit(sendAddress)
                        .accessibilityLabel("The page’s address")
                    Button("Send", action: sendAddress)
                        .disabled(busy || address.trimmingCharacters(in: .whitespaces).isEmpty)
                        .accessibilityLabel("Send the article")
                }
                if !store.send.addressProblem.isEmpty {
                    Note(store.send.addressProblem, kind: .caution)
                }
                if store.send.jobFor == .address, let job = store.send.job {
                    SendStatus(job: job)
                }
            }
            Divider().overlay(Paper.rule)
            VStack(alignment: .leading, spacing: 10) {
                Heading("Waiting for the reader")
                queue
            }
        }
        .task { await store.loadSend() }
        .onChange(of: desk.addressFocus) { _ in addressFocused = true }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Questions.chooseFile(store: store)
                } label: {
                    Label("Send a File", systemImage: "doc.badge.plus")
                }
                .help("Choose a file to send to the reader")
                .disabled(busy)
            }
        }
    }

    @ViewBuilder
    private var destination: some View {
        if let info = store.send.info {
            if info.readers.isEmpty {
                Note("No reader is saved on this computer yet, so a file cannot be sent and an article waits. Installing Cobalt from Reader saves one.")
            } else if info.readers.count == 1 {
                Text("To “\(info.readers[0].name)”, over Wi-Fi.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                HStack(spacing: 8) {
                    Text("Send to")
                    Picker("Send to", selection: Binding(get: { store.send.reader }, set: { store.sendTo(reader: $0) })) {
                        Text("The reader kobo setup saved").tag("")
                        ForEach(info.readers) { reader in
                            Text(reader.name).tag(reader.name)
                        }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
            }
        } else if let problem = store.send.infoProblem {
            Note(problem.sentence, kind: .caution) {
                Button("Try Again") { Task { await store.loadSend() } }.controlSize(.small)
            }
        } else {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Looking for saved readers.").foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private var queue: some View {
        let waiting = store.send.info?.waiting ?? []
        if waiting.isEmpty {
            Text("Nothing is waiting.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(waiting.enumerated()), id: \.offset) { _, article in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(article.title)
                            .font(Paper.serif(16))
                            .fixedSize(horizontal: false, vertical: true)
                        if let site = article.site, !site.isEmpty {
                            Text(site)
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .padding(.vertical, 6)
                    .accessibilityElement(children: .combine)
                }
            }
            Button(waiting.count == 1 ? "Send It Now" : "Send Them Now") { Task { await store.sendQueue() } }
                .disabled(busy)
        }
        if !store.send.queueProblem.isEmpty {
            Note(store.send.queueProblem, kind: .caution)
        }
        if store.send.jobFor == .queue, let job = store.send.job {
            SendStatus(job: job)
        }
    }

    private func sendAddress() {
        let typed = address
        Task {
            if await store.send(address: typed) { address = "" }
        }
    }
}

/// How far a file has got, and whatever it is waiting on the owner for.
private struct FilePart: View {
    @EnvironmentObject private var store: Companion

    private var busy: Bool { store.send.job?.running == true }

    var body: some View {
        if store.send.jobFor == .file, let job = store.send.job {
            Text(job.running ? job.title : job.ok == true ? "Sent." : "\(job.title) did not finish.")
                .font(Paper.serif(17))
            SendStatus(job: job)
            if !job.running {
                Button("Send Another File") { store.cancelFile() }
            }
        } else {
            switch store.send.file {
            case .idle(let problem):
                if !problem.isEmpty {
                    Note(problem, kind: .caution)
                }
                HStack(spacing: 10) {
                    Button("Choose a File…") { Questions.chooseFile(store: store) }
                        .disabled(busy)
                    Text("or drag one onto this window.")
                        .foregroundStyle(.secondary)
                }
            case .uploading(let name, let size, let sent):
                Text("Handing over \(name), \(Advice.fileSize(size)).")
                ProgressView(value: Double(sent), total: Double(max(size, 1)))
                    .tint(Paper.accent)
                    .accessibilityLabel("Handing over \(name)")
            case .asking(let upload, let to):
                ask(upload, to: to)
            }
        }
    }

    @ViewBuilder
    private func ask(_ upload: Upload, to: String?) -> some View {
        if let choices = upload.choices, !choices.isEmpty {
            let chosen = choices.first { $0.name == to }
            Text("\(upload.name) could go to more than one app.")
                .font(Paper.serif(17))
                .fixedSize(horizontal: false, vertical: true)
            Picker("Send it to", selection: Binding(get: { to ?? "" }, set: { store.choose(app: $0) })) {
                ForEach(choices) { app in
                    Text(app.purpose.map { "\(app.title), for \($0)" } ?? app.title).tag(app.name)
                }
            }
            .pickerStyle(.radioGroup)
            .labelsHidden()
            .tint(Paper.accent)
            if let chosen, let replaces = chosen.replaces {
                Text("\(chosen.title) keeps one \(replaces) at a time, so this replaces the \(replaces) on the reader.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 10) {
                Spacer()
                Button("Cancel", role: .cancel) { store.cancelFile() }
                Button(chosen?.replaces == nil ? "Send" : "Replace and Send") { Task { await store.confirmFile() } }
                    .leading()
                    .disabled(chosen == nil || busy)
            }
        } else if let app = upload.app {
            let replaces = app.replaces ?? "file"
            Text("\(app.title) keeps one \(replaces) at a time, so sending \(upload.name) replaces the \(replaces) on the reader.")
                .font(Paper.serif(17))
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 10) {
                Spacer()
                Button("Cancel", role: .cancel) { store.cancelFile() }
                Button("Replace and Send") { Task { await store.confirmFile() } }
                    .leading()
                    .disabled(busy)
            }
        }
    }
}

/// How a send is going, or how it went, under the part that started it.
private struct SendStatus: View {
    @EnvironmentObject private var store: Companion
    var job: FollowedJob

    var body: some View {
        if job.running {
            ProgressView()
                .progressViewStyle(.linear)
                .accessibilityLabel(job.title)
            SaidView(blocks: Said.reflow(job.text))
        } else if job.ok == true {
            SaidView(blocks: Said.reflow(job.text))
        } else {
            let reason = Said.sentence(Said.failure(job.text))
            Note(reason.isEmpty ? "The command stopped without saying why." : reason, kind: .caution)
            if Privacy.looksKeptOffNetwork(job.text) {
                let advice = Privacy.localNetwork(origin: store.origin)
                Note(advice.headline, detail: advice.detail) {
                    Button(advice.button) { NSWorkspace.shared.open(advice.settings) }
                        .controlSize(.small)
                }
            }
            Folded(title: "Everything it said") { SaidView(blocks: Said.reflow(job.text)) }
        }
    }
}
