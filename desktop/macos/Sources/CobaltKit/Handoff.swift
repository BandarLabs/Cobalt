import Foundation

/// How the app is told where its server is.
///
/// The token is the whole of what stands between the server and every other
/// program on the computer, so it is never put where another program could
/// read it: not on a command line, which any process can list, and not in a
/// file. It arrives as one line of JSON down a pipe, from the kobo command
/// to the app, in whichever direction started the other.
public enum Handoff {
    private struct Line: Decodable {
        var address: String
        var token: String
        var kobo: String?
    }

    /// The session a handoff line describes, or nothing when it is not one
    /// this app should act on.
    ///
    /// The address must be this computer and nothing else. A line that named
    /// another host would have the app send its token there with every
    /// request, so it is refused rather than trusted, whoever wrote it.
    public static func session(fromLine line: String) -> Session? {
        guard let said = try? JSONDecoder().decode(Line.self, from: Data(line.utf8)) else { return nil }
        return session(address: said.address, token: said.token, kobo: said.kobo)
    }

    /// For development: the address and token of a server started by hand.
    public static func session(environment: [String: String]) -> Session? {
        guard let address = environment["COBALT_UI_ADDRESS"], let token = environment["COBALT_UI_TOKEN"] else { return nil }
        return session(address: address, token: token, kobo: nil)
    }

    private static func session(address: String, token: String, kobo: String?) -> Session? {
        guard let url = URL(string: address), url.scheme == "http", url.host == "127.0.0.1", url.port != nil,
              url.path.isEmpty || url.path == "/", url.query == nil, url.user == nil
        else { return nil }
        // The token goes into a header as it is, so anything that could end
        // the header or start another is not a token.
        guard !token.isEmpty, token.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }) else { return nil }
        guard var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return nil }
        parts.path = ""
        guard let bare = parts.url else { return nil }
        return Session(address: bare, token: token, kobo: kobo)
    }

    /// Reads the one line `kobo ui` writes to the app it starts.
    ///
    /// Bounded, because a parent that set the variable and then wrote
    /// nothing would otherwise leave the app waiting on its own standard
    /// input for ever with no window to say so.
    public static func line(from handle: FileHandle, within patience: TimeInterval) async -> String? {
        let wait = LineWait()
        handle.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                wait.ended(after: 0)
            } else {
                wait.heard(data)
            }
        }
        defer { handle.readabilityHandler = nil }
        switch await wait.outcome(within: patience) {
        case .line(let line): return line
        case .ended, .silent: return nil
        }
    }
}

/// Waits for the first whole line from a pipe, the end of the pipe, or the
/// end of patience, whichever comes first, and answers exactly once.
final class LineWait: @unchecked Sendable {
    enum Outcome: Sendable { case line(String), ended, silent }

    private let lock = NSLock()
    private var buffer = Data()
    private var settled: Outcome?
    private var continuation: CheckedContinuation<Outcome, Never>?

    func heard(_ data: Data) {
        lock.lock()
        guard settled == nil else { lock.unlock(); return }
        buffer.append(data)
        guard let newline = buffer.firstIndex(of: 0x0A) else { lock.unlock(); return }
        let line = String(decoding: buffer[buffer.startIndex..<newline], as: UTF8.self)
        lock.unlock()
        settle(.line(line))
    }

    /// The pipe closed. A short grace lets a last line that was written
    /// just before the close be read first.
    func ended(after grace: TimeInterval) {
        if grace <= 0 {
            settle(.ended)
        } else {
            DispatchQueue.global().asyncAfter(deadline: .now() + grace) { self.settle(.ended) }
        }
    }

    func outcome(within patience: TimeInterval) async -> Outcome {
        DispatchQueue.global().asyncAfter(deadline: .now() + patience) { self.settle(.silent) }
        return await withCheckedContinuation { continuation in
            lock.lock()
            if let settled {
                lock.unlock()
                continuation.resume(returning: settled)
            } else {
                self.continuation = continuation
                lock.unlock()
            }
        }
    }

    private func settle(_ outcome: Outcome) {
        lock.lock()
        guard settled == nil else { lock.unlock(); return }
        settled = outcome
        let waiting = continuation
        continuation = nil
        lock.unlock()
        waiting?.resume(returning: outcome)
    }
}

/// The kobo command, started by the app and kept for as long as the app
/// lives.
///
/// `kobo ui --for-app` serves until its standard input closes. The app holds
/// the other end of that pipe and never writes to it, so the server stops
/// when the app does, however the app goes: quit, crash or kill all close
/// the pipe, and none of them needs the app to have done anything first.
public final class ServerProcess: @unchecked Sendable {
    public enum Trouble: Error, Equatable, Sendable {
        /// There is no kobo command where one was expected.
        case missing(path: String)
        /// It ran and stopped before saying where its server was.
        case exited(said: String)
        /// It is running and has said nothing in the time allowed.
        case silent(seconds: Int)
        /// It said something that is not a handoff.
        case garbled
    }

    public let session: Session
    private let process: Process
    private let input: Pipe
    private let complaints: Complaints
    private let lock = NSLock()
    private var stopping = false

    private init(session: Session, process: Process, input: Pipe, complaints: Complaints) {
        self.session = session
        self.process = process
        self.input = input
        self.complaints = complaints
    }

    /// Starts the command and waits for it to say where its server is.
    ///
    /// - Parameter whenItStops: called once if the command exits later
    ///   without having been asked to, with whatever it last said.
    public static func start(
        command: URL,
        arguments: [String] = ["ui", "--for-app"],
        patience: TimeInterval = 10,
        whenItStops: @escaping @Sendable (String) -> Void = { _ in }
    ) async throws -> ServerProcess {
        guard FileManager.default.isExecutableFile(atPath: command.path) else { throw Trouble.missing(path: command.path) }
        let process = Process()
        let input = Pipe()
        let output = Pipe()
        let errors = Pipe()
        process.executableURL = command
        process.arguments = arguments
        process.standardInput = input
        process.standardOutput = output
        process.standardError = errors

        let wait = LineWait()
        let complaints = Complaints()
        output.fileHandleForReading.readabilityHandler = { handle in
            // Read on after the first line too. A pipe nobody drains fills,
            // and a command blocked on a full pipe looks exactly like one
            // that has hung.
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil } else { wait.heard(data) }
        }
        errors.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil } else { complaints.heard(data) }
        }
        let started = Started()
        process.terminationHandler = { _ in
            wait.ended(after: 0.1)
            started.stopped(complaints: complaints, tell: whenItStops)
        }
        do {
            try process.run()
        } catch {
            throw Trouble.missing(path: command.path)
        }

        func abandon() {
            output.fileHandleForReading.readabilityHandler = nil
            errors.fileHandleForReading.readabilityHandler = nil
            try? input.fileHandleForWriting.close()
            if process.isRunning { process.terminate() }
        }

        switch await wait.outcome(within: patience) {
        case .line(let line):
            guard let session = Handoff.session(fromLine: line) else {
                started.abandoned()
                abandon()
                throw Trouble.garbled
            }
            let server = ServerProcess(session: session, process: process, input: input, complaints: complaints)
            started.became(server)
            return server
        case .ended:
            started.abandoned()
            abandon()
            throw Trouble.exited(said: complaints.said)
        case .silent:
            started.abandoned()
            abandon()
            throw Trouble.silent(seconds: Int(patience.rounded()))
        }
    }

    /// Lets the server go: closes the pipe it is waiting on. It finishes
    /// any job in hand first, which is its own rule and not this one's.
    public func stop() {
        lock.withLock { stopping = true }
        try? input.fileHandleForWriting.close()
    }

    fileprivate var wasAskedToStop: Bool {
        lock.withLock { stopping }
    }

    /// What the command has written to its error stream, most recent last
    /// and bounded, so a command that complains for an hour costs nothing.
    final class Complaints: @unchecked Sendable {
        private let lock = NSLock()
        private var kept = Data()

        func heard(_ data: Data) {
            lock.withLock {
                kept.append(data)
                if kept.count > 4096 { kept.removeFirst(kept.count - 4096) }
            }
        }

        var said: String {
            lock.withLock { String(decoding: kept, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines) }
        }
    }

    /// Whether the command's exit is news. It is not while the app is still
    /// waiting for the handoff, which reports the exit itself, and it is not
    /// when the app asked for it.
    private final class Started: @unchecked Sendable {
        private let lock = NSLock()
        private var server: ServerProcess?
        private var over = false
        private var exited = false

        func became(_ server: ServerProcess) {
            lock.withLock { self.server = server }
        }

        func abandoned() {
            lock.withLock { over = true }
        }

        func stopped(complaints: Complaints, tell: @escaping @Sendable (String) -> Void) {
            // The handoff may be a moment from arriving when the exit is
            // seen, so the news waits long enough for the two to be put in
            // order.
            DispatchQueue.global().asyncAfter(deadline: .now() + 0.3) {
                let server = self.lock.withLock { () -> ServerProcess? in
                    guard !self.over, !self.exited else { return nil }
                    self.exited = true
                    return self.server
                }
                guard let server, !server.wasAskedToStop else { return }
                tell(complaints.said)
            }
        }
    }
}
