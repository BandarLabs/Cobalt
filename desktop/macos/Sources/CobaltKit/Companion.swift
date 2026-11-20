import Combine
import Foundation

/// Whether the window has a server behind it.
public enum Link: Equatable, Sendable {
    case starting
    case connected
    case lost(Lost)
}

/// A plan the owner has opened and not yet agreed to or dismissed.
public struct OpenPlan: Equatable, Sendable {
    public var volume: String
    public var plan: Plan
}

public struct ReaderDesk: Equatable, Sendable {
    public var plan: OpenPlan?
    /// A plan has been asked for and has not arrived.
    public var planning = false
    public var job: FollowedJob?
    /// Why the last thing asked for here did not happen.
    public var error = ""
    /// Something that went well and is worth a line: a reader ejected.
    public var notice = ""
    public var ejecting = false
}

public struct AppsDesk: Equatable, Sendable {
    /// The reader and the Cobalt on it that the lists below describe. When
    /// either changes the lists are for something else and are dropped.
    public var key = ""
    public var installed: [AppEntry]?
    public var catalog: [AppEntry]?
    /// Why the reader's apps or the catalog could not be read.
    public var problem: Failure?
    /// Why an install or a removal was not started.
    public var refusal: Failure?
    public var loading = false
    public var query = ""
    public var job: FollowedJob?
    public var said = ""
    /// Something has been written to the reader from here, so it wants
    /// ejecting before the cable comes out.
    public var changed = false
    /// The app being removed at this moment.
    public var removing: String?
}

public struct BrowserDesk: Equatable, Sendable {
    public var list: [Browser]?
    public var problem: Failure?
    /// The browser being set up or removed at this moment.
    public var busy: String?
    /// What there is to say about a browser after a change to it.
    public var said: [String: String] = [:]
}

/// How far a file has got on its way to the reader.
public enum FileStage: Equatable, Sendable {
    case idle(problem: String)
    case uploading(name: String, size: Int64, sent: Int64)
    /// The server has the file and the owner has something to decide: which
    /// app it goes to, or whether to replace what that app is holding.
    case asking(Upload, to: String?)
}

public enum SendPart: Equatable, Sendable { case file, address, queue }

public struct SendDesk: Equatable, Sendable {
    public var info: SendInfo?
    public var infoProblem: Failure?
    /// The saved reader chosen to send to. Empty for the one the command
    /// uses when none is named.
    public var reader = ""
    public var file: FileStage = .idle(problem: "")
    public var job: FollowedJob?
    public var jobFor: SendPart?
    public var addressProblem = ""
    public var queueProblem = ""
}

/// The window's record of what it knows and what the owner is in the
/// middle of.
///
/// Every place draws from this and asks it to do things; nothing else talks
/// to the server. Each method that asks the server something does it once
/// and returns when the record reflects the answer, and the timers that call
/// them live in the app. That is what lets a test walk the record through a
/// failure one request at a time, and lets the render mode stop it in any
/// state and draw it.
@MainActor
public final class Companion: ObservableObject {
    /// Finds or starts a server. Called when the window opens and again
    /// whenever the owner asks to start again.
    public typealias Connect = @MainActor () async -> Result<Client, Lost>

    @Published public private(set) var link: Link = .starting
    @Published public private(set) var state: ServerState?
    @Published public var place: Place
    @Published public private(set) var chosenVolume: String?
    @Published public private(set) var reader = ReaderDesk()
    @Published public private(set) var apps = AppsDesk()
    @Published public private(set) var browser = BrowserDesk()
    @Published public private(set) var send = SendDesk()

    /// The app's own version, when it has one to compare. A build run
    /// outside its bundle has none and compares nothing.
    public let appVersion: String?
    public let origin: Origin
    /// Whether a terminal is also printing what jobs say. True while the
    /// server is the `kobo ui` that started the app, and false once the app
    /// has had to start a server of its own, whose output only it reads.
    public var jobsEchoInTerminal: Bool

    private let connect: Connect
    private let now: @Sendable () -> Date
    private var client: Client?
    private var misses = 0
    private var lastMiss: Failure?

    public init(
        place: Place = .reader,
        appVersion: String? = nil,
        origin: Origin = .finder,
        now: @escaping @Sendable () -> Date = { Date() },
        connect: @escaping Connect
    ) {
        self.place = place
        self.appVersion = appVersion
        self.origin = origin
        self.jobsEchoInTerminal = origin == .terminal
        self.now = now
        self.connect = connect
    }

    // What the places read.

    public var computer: Computer? { state?.computer }

    public var chosen: Chosen {
        Advice.chosen(among: state?.readers ?? [], volume: chosenVolume)
    }

    /// The reader the Apps place is about, when it is one with Cobalt on it.
    public var appsReader: Reader? {
        guard let reader = chosen.reader, reader.cobalt != nil else { return nil }
        return reader
    }

    public var shelf: Shelf {
        Shelf.arrange(installed: apps.installed, catalog: apps.catalog, query: apps.query)
    }

    public var versionTrouble: VersionTrouble? {
        guard let state else { return nil }
        if state.api != 1 { return .contract(app: appVersion, kobo: state.computer.kobo) }
        if let appVersion, appVersion != state.computer.kobo { return .release(app: appVersion, kobo: state.computer.kobo) }
        return nil
    }

    public var clock: Date { now() }

    /// The jobs still running, which is what closing the window would walk
    /// away from.
    public var running: [FollowedJob] {
        [reader.job, apps.job, send.job].compactMap { $0 }.filter(\.running)
    }

    /// What to ask before the window closes, or nothing when closing costs
    /// nothing.
    ///
    /// A job belongs to the server, which finishes it whether or not the
    /// window is there, so the honest question is not "stop the install?"
    /// but "stop watching it?", and what would be missed. A file part way
    /// up is the one thing that really is lost, because the app is what is
    /// sending it.
    public var quitQuestion: QuitQuestion? {
        if case .uploading(let name, _, _) = send.file {
            return QuitQuestion(
                title: "\(name) has not all been handed over.",
                detail: "Quitting now stops it and nothing is sent. The file itself is not touched.",
                stay: "Keep Open",
                leave: "Quit"
            )
        }
        guard let job = running.first else { return nil }
        var detail = "It finishes by itself if you quit."
        if job.kind == .reader {
            detail += " Leave the reader plugged in until it has."
            if job.title.contains("Cobalt"), !job.title.hasPrefix("Removing") {
                detail += jobsEchoInTerminal
                    ? " The steps to take on the reader afterwards will also be in the terminal that started Cobalt."
                    : " The steps to take on the reader afterwards appear only in this window, so quitting means not seeing them."
            }
        }
        return QuitQuestion(title: "\(job.title) is still running.", detail: detail, stay: "Keep Open", leave: "Quit")
    }

    // Starting, and noticing the server has gone.

    public func start() async {
        link = .starting
        client = nil
        misses = 0
        switch await connect() {
        case .success(let client):
            self.client = client
            link = .connected
            await refresh()
        case .failure(let lost):
            link = .lost(lost)
        }
    }

    /// Called by the app when the kobo command it started has exited.
    public func serverStopped(said: String) {
        guard link == .connected else { return }
        lose(said.isEmpty ? Lost(.stopped) : Lost(.exited(said: said)))
    }

    /// Reads the state once. The app calls this every two seconds.
    public func refresh() async {
        guard link == .connected, let client else { return }
        let fresh: ServerState
        do {
            fresh = try await client.state()
        } catch {
            let failure = heard(error)
            // One missed answer is a busy moment. Three in a row is a
            // server that has gone.
            guard link == .connected else { return }
            misses += 1
            lastMiss = failure
            if misses >= 3 { lose(Lost(lastMiss == .malformed ? .garbled : .stopped)) }
            return
        }
        misses = 0
        let first = state == nil
        // Published only when something changed, so a place that is being
        // read or typed into is not redrawn under the owner every two
        // seconds.
        if fresh != state { state = fresh }
        if !fresh.readers.isEmpty, !reader.notice.isEmpty { reader.notice = "" }
        if first { resume(fresh.jobs) }
        settle()
    }

    /// Asks each running job what it has said since last time. The app
    /// calls this about every 700 milliseconds.
    public func tick() async {
        for slot in Slot.allCases where job(in: slot)?.running == true {
            await poll(slot)
        }
    }

    // Reader.

    public func choose(volume: String) {
        chosenVolume = volume
        reader.notice = ""
        reader.error = ""
        settle()
    }

    public func chooseAnother() {
        chosenVolume = nil
        reader.error = ""
        settle()
    }

    /// Asks what an install or a removal would change, and shows it.
    public func openPlan(undo: Bool) async {
        guard let client, let chosen = chosen.reader, !reader.planning else { return }
        reader.error = ""
        reader.planning = true
        defer { reader.planning = false }
        do {
            let plan = try await client.plan(volume: chosen.volume, undo: undo)
            reader.plan = OpenPlan(volume: chosen.volume, plan: plan)
        } catch {
            reader.error = heard(error).sentence
        }
    }

    public func closePlan() {
        reader.plan = nil
        reader.error = ""
    }

    /// Starts what the open plan describes.
    public func agreeToPlan() async {
        guard let open = reader.plan else { return }
        await startReaderJob(open.plan.undo ? .remove(volume: open.volume) : .install(volume: open.volume))
    }

    /// Runs a failed reader job again, as it was.
    public func tryReaderJobAgain() async {
        guard let again = reader.job?.again else { return }
        reader.job = nil
        await startReaderJob(again)
    }

    public func dismissReaderJob() async {
        reader.job = nil
        reader.error = ""
        await refresh()
    }

    public func eject(volume: String, from place: Place = .reader) async {
        guard let client, !reader.ejecting else { return }
        let model = state?.readers.first { $0.volume == volume }?.model ?? "reader"
        reader.error = ""
        apps.refusal = nil
        reader.ejecting = true
        defer { reader.ejecting = false }
        do {
            try await client.eject(volume: volume)
            reader.notice = "The \(model) is ejected and can be unplugged."
            apps.changed = false
            await refresh()
        } catch {
            let failure = heard(error)
            if place == .apps { apps.refusal = failure } else { reader.error = failure.sentence }
        }
    }

    // Apps.

    public func search(_ query: String) {
        if apps.query != query { apps.query = query }
    }

    /// Reads what is on the reader, then fetches the catalog. The first is
    /// shown as soon as it is known: it needs no network and the second can
    /// take the best part of a minute.
    public func loadApps() async {
        guard let client, let reader = appsReader, !apps.loading, apps.job?.running != true else { return }
        let key = apps.key
        apps.loading = true
        apps.problem = nil
        defer { if apps.key == key { apps.loading = false } }
        do {
            let here = try await client.apps(volume: reader.volume)
            guard apps.key == key else { return }
            apps.installed = here
            let published = try await client.catalog(volume: reader.volume)
            guard apps.key == key else { return }
            apps.catalog = published
        } catch {
            let failure = heard(error)
            if apps.key == key { apps.problem = failure }
        }
    }

    public func installApps(_ ids: [String]) async {
        guard let client, let reader = appsReader, !ids.isEmpty else { return }
        apps.said = ""
        apps.refusal = nil
        do {
            let id = try await client.installApps(volume: reader.volume, ids: ids)
            apps.job = FollowedJob(
                id: id,
                kind: .reader,
                title: ids.count == 1 ? "Installing an app" : "Installing apps",
                volume: reader.volume,
                began: now(),
                again: .apps(volume: reader.volume, ids: ids)
            )
            await poll(.apps)
        } catch {
            apps.refusal = heard(error)
        }
    }

    public func removeApp(_ app: AppEntry) async {
        guard let client, let reader = appsReader, apps.removing == nil else { return }
        apps.refusal = nil
        apps.said = ""
        apps.removing = app.id
        do {
            let left = try await client.removeApp(volume: reader.volume, id: app.id)
            apps.removing = nil
            apps.installed = left.apps
            apps.said = "\(app.title) is removed. Its data is kept."
            apps.changed = true
            await loadApps()
        } catch {
            apps.removing = nil
            apps.refusal = heard(error)
        }
    }

    public func dismissAppsJob() {
        apps.job = nil
    }

    public func dismissAppsNote() {
        apps.said = ""
        apps.refusal = nil
    }

    // Browser.

    public func loadBrowsers() async {
        guard let client else { return }
        do {
            let found = try await client.browsers()
            if browser.list != found { browser.list = found }
            browser.problem = nil
        } catch {
            browser.problem = heard(error)
        }
    }

    public func change(browser changed: Browser, setUp: Bool) async {
        guard let client, browser.busy == nil else { return }
        browser.busy = changed.id
        browser.problem = nil
        defer { browser.busy = nil }
        do {
            let done = try await (setUp ? client.setUpBrowser(changed.id) : client.removeBrowser(changed.id))
            browser.list = done.browsers
            // Taking the registration away does not take the extension out
            // of the browser, which only the browser can do, so the owner
            // is told the half that is left.
            browser.said[changed.id] = setUp ? "" : "Send to Kobo is gone from \(changed.title). Remove the extension on \(changed.title)’s own extensions page too."
        } catch {
            browser.problem = heard(error)
        }
    }

    // Send.

    public func loadSend() async {
        guard let client else { return }
        do {
            let info = try await client.sendInfo()
            if send.info != info { send.info = info }
            send.infoProblem = nil
            if !info.readers.contains(where: { $0.name == send.reader }) { send.reader = "" }
        } catch {
            let failure = heard(error)
            if send.info == nil { send.infoProblem = failure }
        }
    }

    public func sendTo(reader name: String) {
        send.reader = name
    }

    /// Takes a file the owner chose or dropped, and gets it as far as it
    /// can go without a decision from them.
    public func offer(file: URL) async {
        guard let client else { return }
        let name = file.lastPathComponent
        if send.job?.running == true || isUploading {
            send.file = sendingElse(problem: "Something is still being sent. \(name) can go when it has finished.")
            return
        }
        let values = try? file.resourceValues(forKeys: [.fileSizeKey, .isDirectoryKey])
        if values?.isDirectory == true {
            send.file = .idle(problem: "\(name) is a folder. Send the files in it one at a time.")
            return
        }
        guard let size = values?.fileSize.map(Int64.init) else {
            send.file = .idle(problem: "\(name) could not be read. Check that it is still where it was.")
            return
        }
        let refusal = Advice.uploadRefusal(name: name, size: size, limit: computer?.uploadLimit)
        if !refusal.isEmpty {
            send.file = .idle(problem: refusal)
            return
        }
        if send.jobFor == .file {
            send.job = nil
            send.jobFor = nil
        }
        send.file = .uploading(name: name, size: size, sent: 0)
        do {
            let got = try await client.upload(file: file, named: name) { [weak self] sent, _ in
                Task { @MainActor in self?.uploaded(sent, of: name) }
            }
            let question = !(got.choices ?? []).isEmpty || got.app?.replaces != nil
            if question {
                send.file = .asking(got, to: nil)
            } else {
                await startSend(.file, title: "Sending \(got.name)") { client in
                    try await client.startSend(upload: got.upload, to: nil, replace: false, reader: self.send.reader)
                }
            }
        } catch {
            send.file = .idle(problem: heard(error).sentence)
        }
    }

    /// Picks one of the apps a held file could go to.
    public func choose(app name: String) {
        guard case .asking(let upload, _) = send.file else { return }
        send.file = .asking(upload, to: name)
    }

    /// Sends the held file where the owner has now said it goes.
    public func confirmFile() async {
        guard case .asking(let upload, let to) = send.file else { return }
        let app: SendApp?
        if let choices = upload.choices, !choices.isEmpty {
            app = choices.first { $0.name == to }
        } else {
            app = upload.app
        }
        guard let app else { return }
        await startSend(.file, title: "Sending \(upload.name)") { client in
            try await client.startSend(upload: upload.upload, to: app.name, replace: app.replaces != nil, reader: self.send.reader)
        }
    }

    public func cancelFile() {
        send.file = .idle(problem: "")
        if send.jobFor == .file, send.job?.running != true {
            send.job = nil
            send.jobFor = nil
        }
    }

    /// Sends a web address as an article.
    ///
    /// - Returns: whether it was taken, so the field can be cleared.
    @discardableResult
    public func send(address typed: String) async -> Bool {
        let (address, problem) = Advice.webAddress(typed)
        send.addressProblem = problem
        guard !address.isEmpty else { return false }
        if send.job?.running == true {
            send.addressProblem = "Something is still being sent. Send this when it has finished."
            return false
        }
        return await startSend(.address, title: "Sending an article") { client in
            try await client.sendAddress(address, reader: self.send.reader)
        }
    }

    public func sendQueue() async {
        await startSend(.queue, title: "Sending waiting articles") { client in
            try await client.sendQueue(reader: self.send.reader)
        }
    }

    public func dismissSendJob() {
        guard send.job?.running != true else { return }
        send.job = nil
        send.jobFor = nil
    }

    // The working parts.

    private enum Slot: CaseIterable { case reader, apps, send }

    private var isUploading: Bool {
        if case .uploading = send.file { return true }
        return false
    }

    private func sendingElse(problem: String) -> FileStage {
        // A file already on its way keeps its place; the one that was just
        // dropped is the one turned away.
        if case .uploading = send.file { return send.file }
        if case .asking = send.file { return send.file }
        return .idle(problem: problem)
    }

    /// Records how much of a file has gone. The client calls this as the
    /// bytes leave; the render mode calls it to hold an upload part way.
    public func uploaded(_ sent: Int64, of name: String) {
        guard case .uploading(let current, let size, _) = send.file, current == name else { return }
        send.file = .uploading(name: current, size: size, sent: min(sent, size))
    }

    private func job(in slot: Slot) -> FollowedJob? {
        switch slot {
        case .reader: return reader.job
        case .apps: return apps.job
        case .send: return send.job
        }
    }

    private func set(_ job: FollowedJob, in slot: Slot) {
        switch slot {
        case .reader: reader.job = job
        case .apps: apps.job = job
        case .send: send.job = job
        }
    }

    private func poll(_ slot: Slot) async {
        guard let client, let asked = job(in: slot), asked.running else { return }
        do {
            let report = try await client.job(asked.id, from: asked.next)
            // The owner may have dismissed the job, or started another,
            // while the question was out.
            guard var job = job(in: slot), job.id == asked.id, job.next == asked.next else { return }
            if !report.title.isEmpty { job.title = report.title }
            if !report.text.isEmpty {
                job.text += report.text
                job.lastHeard = now()
            }
            job.next = report.next
            job.running = report.running
            job.ok = report.ok
            job.code = report.code
            job.volume = report.volume ?? job.volume
            set(job, in: slot)
            if !job.running { await finished(slot) }
        } catch {
            let failure = heard(error)
            guard failure.kind == .notFound, var job = job(in: slot), job.id == asked.id else { return }
            // The server keeps only so many finished jobs. One it has
            // forgotten cannot be followed, and is shown as stopped with
            // that as the reason rather than left running for ever.
            job.running = false
            job.ok = false
            job.text += "\nkobo: this job is no longer remembered\n"
            set(job, in: slot)
        }
    }

    private func finished(_ slot: Slot) async {
        switch slot {
        case .reader:
            await refresh()
        case .apps:
            apps.changed = true
            await loadApps()
        case .send:
            await loadSend()
        }
    }

    private func startReaderJob(_ again: Again) async {
        guard let client else { return }
        reader.error = ""
        do {
            let id: Int
            let title: String
            let volume: String
            switch again {
            case .install(let target):
                id = try await client.installCobalt(volume: target)
                volume = target
                let reader = state?.readers.first { $0.volume == target }
                title = reader?.cobalt == nil ? "Installing Cobalt" : reader?.standing == .current ? "Installing Cobalt again" : "Updating Cobalt"
            case .remove(let target):
                id = try await client.removeCobalt(volume: target)
                volume = target
                title = "Removing Cobalt"
            case .apps:
                return
            }
            reader.plan = nil
            reader.notice = ""
            reader.job = FollowedJob(id: id, kind: .reader, title: title, volume: volume, began: now(), again: again)
            await poll(.reader)
        } catch {
            reader.error = heard(error).sentence
        }
    }

    @discardableResult
    private func startSend(_ part: SendPart, title: String, _ start: (Client) async throws -> Int) async -> Bool {
        guard let client else { return false }
        do {
            let id = try await start(client)
            send.jobFor = part
            send.job = FollowedJob(id: id, kind: .send, title: title, volume: nil, began: now())
            send.queueProblem = ""
            if part == .file { send.file = .idle(problem: "") }
            await poll(.send)
            return true
        } catch {
            let sentence = heard(error).sentence
            switch part {
            case .address: send.addressProblem = sentence
            case .file: send.file = .idle(problem: sentence)
            case .queue: send.queueProblem = sentence
            }
            return false
        }
    }

    /// Picks up jobs that were already running when the window opened.
    private func resume(_ jobs: [JobSummary]) {
        if let running = Advice.runningJob(jobs, kind: .reader) {
            let job = FollowedJob(id: running.id, kind: .reader, title: running.title, volume: running.volume, began: now())
            // The two sorts of reader job share a queue and are shown in
            // different places. The title is the only thing that tells
            // them apart from here.
            if running.title.contains("app") { apps.job = job } else { reader.job = job }
        }
        if let running = Advice.runningJob(jobs, kind: .send) {
            send.jobFor = running.title.contains("article") ? (running.title.contains("waiting") ? .queue : .address) : .file
            send.job = FollowedJob(id: running.id, kind: .send, title: running.title, volume: nil, began: now())
        }
    }

    /// Brings what the owner had chosen back into line with what is
    /// plugged in now.
    private func settle() {
        let readers = state?.readers ?? []
        if let volume = chosenVolume, !readers.contains(where: { $0.volume == volume }) { chosenVolume = nil }
        if let plan = reader.plan, chosen.reader?.volume != plan.volume { reader.plan = nil }
        let key = appsReader.map { "\($0.volume) \($0.cobalt?.cobaltVersion ?? "")" } ?? ""
        if key != apps.key {
            apps = AppsDesk(key: key, query: apps.query, job: apps.job, changed: apps.changed && !key.isEmpty)
        }
    }

    /// Sorts an error into a `Failure` and acts on the two kinds that say
    /// something about more than the request they came back from.
    private func heard(_ error: Error) -> Failure {
        let failure = error as? Failure ?? .unreachable
        if failure.endsTheSession {
            lose(Lost(.refused))
        } else if failure.kind == .gone {
            // The reader named has been unplugged. The list of readers is
            // out of date by up to two seconds; this brings it up to date
            // now so the place stops offering what was just refused.
            Task { await self.refresh() }
        }
        return failure
    }

    private func lose(_ lost: Lost) {
        guard link == .connected else { return }
        link = .lost(lost)
        client = nil
    }
}
