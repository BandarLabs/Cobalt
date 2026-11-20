import Foundation

/// Where the server is and the secret that lets this app talk to it.
public struct Session: Equatable, Sendable {
    public var address: URL
    public var token: String
    /// The version the kobo command gave when it handed the session over.
    public var kobo: String?

    public init(address: URL, token: String, kobo: String? = nil) {
        self.address = address
        self.token = token
        self.kobo = kobo
    }
}

/// How long each sort of request is given before the app stops waiting.
///
/// Nothing in the window may wait for ever, so every request has one of
/// these. The server answers a question in well under a second; ten is
/// already a server in trouble. The catalog is fetched from the network by
/// the server and is allowed the best part of a minute there, so the app
/// allows a little more than that. A file goes at whatever speed the disk
/// and the loopback allow, so an upload is timed by silence, not by length.
public struct Patience: Equatable, Sendable {
    public var question: TimeInterval = 10
    public var change: TimeInterval = 30
    public var catalog: TimeInterval = 75
    public var uploadSilence: TimeInterval = 60
    /// A picture is fetched from the project's site by the server the
    /// first time it is asked for, so it is allowed longer than a question.
    public var picture: TimeInterval = 30

    public init() {}

    public init(everything: TimeInterval) {
        question = everything
        change = everything
        catalog = everything
        uploadSilence = everything
        picture = everything
    }
}

/// The JSON API of `kobo ui`, one method per route.
///
/// The client holds no state and decides nothing. It sends, and it sorts
/// what comes back into an answer or a `Failure`, so that everything above
/// it deals in those two and never in status codes.
public final class Client: Sendable {
    public let session: Session
    public let patience: Patience
    private let transport: URLSession

    /// - Parameter configuration: replaced in tests and renders by one whose
    ///   protocol classes answer from a script. The live app passes nothing.
    public init(session: Session, patience: Patience = Patience(), configuration: URLSessionConfiguration? = nil) {
        self.session = session
        self.patience = patience
        let configuration = configuration ?? .ephemeral
        // The token is the whole of the app's standing with the server, and
        // the server refuses a bearer request that looks like a browser's.
        // No cookie is ever stored or sent, nothing is cached, and loopback
        // is never sent through a proxy that happens to be configured.
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.httpCookieAcceptPolicy = .never
        configuration.urlCache = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.connectionProxyDictionary = [:]
        configuration.waitsForConnectivity = false
        transport = URLSession(configuration: configuration)
    }

    deinit {
        transport.invalidateAndCancel()
    }

    // The routes.

    public func state() async throws -> ServerState {
        try await ask("/api/state")
    }

    public func plan(volume: String, undo: Bool) async throws -> Plan {
        try await ask("/api/plan", undo ? ["volume": volume, "undo": "1"] : ["volume": volume])
    }

    public func installCobalt(volume: String) async throws -> Int {
        try await (change("/api/reader/install", ["volume": .text(volume)]) as JobStarted).job
    }

    public func removeCobalt(volume: String) async throws -> Int {
        try await (change("/api/reader/remove", ["volume": .text(volume)]) as JobStarted).job
    }

    public func eject(volume: String) async throws {
        let _: Ejected = try await change("/api/reader/eject", ["volume": .text(volume)])
    }

    /// Looks again on Wi-Fi for the reader Cobalt was installed on from
    /// this computer and that has not been saved yet.
    public func findReader() async throws -> Int {
        try await (change("/api/reader/find", [:]) as JobStarted).job
    }

    public func renameReader(_ name: String, to new: String) async throws -> [SavedLink] {
        try await (change("/api/readers/rename", ["name": .text(name), "to": .text(new)]) as SavedAnswer).saved
    }

    public func forgetReader(_ name: String) async throws -> [SavedLink] {
        try await (change("/api/readers/forget", ["name": .text(name)]) as SavedAnswer).saved
    }

    /// What is on the reader. No network is involved.
    public func apps(volume: String) async throws -> [AppEntry] {
        try await (ask("/api/apps", ["volume": volume]) as AppsAnswer).apps
    }

    /// The signed catalog, each entry marked with what the reader has.
    public func catalog(volume: String) async throws -> [AppEntry] {
        try await (change("/api/apps/catalog", ["volume": .text(volume)], within: patience.catalog) as AppsAnswer).apps
    }

    public func installApps(volume: String, ids: [String]) async throws -> Int {
        try await (change("/api/apps/install", ["volume": .text(volume), "ids": .list(ids)]) as JobStarted).job
    }

    public func removeApp(volume: String, id: String) async throws -> AppRemoved {
        try await change("/api/apps/remove", ["volume": .text(volume), "id": .text(id)])
    }

    /// Every app the kobo command knows of. No reader and no network.
    public func store() async throws -> Store {
        try await ask("/api/store")
    }

    /// One picture, as the bytes of a PNG.
    public func shot(named name: String) async throws -> Data {
        let request = try request("/api/shot", ["name": name], method: "GET", within: patience.picture)
        do {
            let (data, response) = try await transport.data(for: request)
            return try Self.accepted(data, response)
        } catch let failure as Failure {
            throw failure
        } catch {
            throw Self.failure(from: error)
        }
    }

    public func browsers() async throws -> [Browser] {
        try await (ask("/api/browsers") as BrowsersAnswer).browsers
    }

    public func setUpBrowser(_ id: String) async throws -> BrowsersAnswer {
        try await change("/api/browsers/setup", ["browser": .text(id)])
    }

    public func removeBrowser(_ id: String) async throws -> BrowsersAnswer {
        try await change("/api/browsers/remove", ["browser": .text(id)])
    }

    public func sendInfo() async throws -> SendInfo {
        try await ask("/api/send")
    }

    /// Hands a file to the server, which holds it until told where it goes.
    ///
    /// - Parameter progressed: called with the bytes gone and the bytes in
    ///   all, on no particular thread.
    public func upload(file: URL, named name: String, progressed: @escaping @Sendable (Int64, Int64) -> Void) async throws -> Upload {
        var request = try request("/api/send/file", ["name": name], method: "POST", within: patience.uploadSilence)
        request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        let watcher = UploadWatcher(progressed)
        do {
            let (data, response) = try await transport.upload(for: request, fromFile: file, delegate: watcher)
            return try Self.answer(data, response)
        } catch let failure as Failure {
            throw failure
        } catch {
            throw Self.failure(from: error)
        }
    }

    public func startSend(upload: Int, to app: String?, replace: Bool, reader: String?) async throws -> Int {
        var body: [String: Field] = ["upload": .number(upload)]
        if let app { body["to"] = .text(app) }
        if replace { body["replace"] = .flag(true) }
        if let reader, !reader.isEmpty { body["reader"] = .text(reader) }
        return try await (change("/api/send/start", body) as JobStarted).job
    }

    public func sendAddress(_ address: String, reader: String?) async throws -> Int {
        var body: [String: Field] = ["url": .text(address)]
        if let reader, !reader.isEmpty { body["reader"] = .text(reader) }
        return try await (change("/api/send/address", body) as JobStarted).job
    }

    public func sendQueue(reader: String?) async throws -> Int {
        var body: [String: Field] = [:]
        if let reader, !reader.isEmpty { body["reader"] = .text(reader) }
        return try await (change("/api/send/queue", body) as JobStarted).job
    }

    public func job(_ id: Int, from offset: Int) async throws -> JobReport {
        try await ask("/api/job", ["id": String(id), "from": String(offset)])
    }

    // Sending and sorting.

    /// A value in a request body. The bodies are small and flat, and
    /// writing them out by kind keeps them checkable where they are built.
    enum Field: Encodable {
        case text(String)
        case number(Int)
        case flag(Bool)
        case list([String])

        func encode(to encoder: Encoder) throws {
            var container = encoder.singleValueContainer()
            switch self {
            case .text(let value): try container.encode(value)
            case .number(let value): try container.encode(value)
            case .flag(let value): try container.encode(value)
            case .list(let value): try container.encode(value)
            }
        }
    }

    private func ask<Answer: Decodable>(_ path: String, _ query: [String: String] = [:]) async throws -> Answer {
        try await send(request(path, query, method: "GET", within: patience.question))
    }

    private func change<Answer: Decodable>(_ path: String, _ body: [String: Field], within limit: TimeInterval? = nil) async throws -> Answer {
        var request = try request(path, [:], method: "POST", within: limit ?? patience.change)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        // Every change carries a JSON object, even an empty one: the server
        // reads the body of a POST before it decides anything.
        request.httpBody = try JSONEncoder().encode(body)
        return try await send(request)
    }

    private func request(_ path: String, _ query: [String: String], method: String, within limit: TimeInterval) throws -> URLRequest {
        guard var parts = URLComponents(url: session.address, resolvingAgainstBaseURL: false) else { throw Failure.malformed }
        parts.path = path
        if !query.isEmpty {
            // Written out rather than left to URLComponents, which leaves a
            // plus sign alone. A server reading the query as a form would
            // take that for a space, and file names have plus signs in them.
            parts.percentEncodedQuery = query.sorted { $0.key < $1.key }
                .map { "\(Self.escaped($0.key))=\(Self.escaped($0.value))" }
                .joined(separator: "&")
        }
        guard let url = parts.url else { throw Failure.malformed }
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: limit)
        request.httpMethod = method
        request.setValue("Bearer \(session.token)", forHTTPHeaderField: "Authorization")
        request.httpShouldHandleCookies = false
        return request
    }

    private func send<Answer: Decodable>(_ request: URLRequest) async throws -> Answer {
        do {
            let (data, response) = try await transport.data(for: request)
            return try Self.answer(data, response)
        } catch let failure as Failure {
            throw failure
        } catch {
            throw Self.failure(from: error)
        }
    }

    private static func answer<Answer: Decodable>(_ data: Data, _ response: URLResponse) throws -> Answer {
        do {
            return try JSONDecoder.wire.decode(Answer.self, from: accepted(data, response))
        } catch let failure as Failure {
            throw failure
        } catch {
            throw Failure.malformed
        }
    }

    /// The body of an answer the server said yes with, or the refusal it
    /// said no with.
    private static func accepted(_ data: Data, _ response: URLResponse) throws -> Data {
        guard let response = response as? HTTPURLResponse else { throw Failure.malformed }
        guard (200..<300).contains(response.statusCode) else {
            let body = try? JSONDecoder().decode(RefusalBody.self, from: data)
            let sentence = body?.error?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            throw Failure.refused(
                kind: RefusalKind(named: body?.kind, status: response.statusCode),
                sentence: sentence.isEmpty ? "The request was refused (\(response.statusCode))." : sentence,
                status: response.statusCode
            )
        }
        return data
    }

    /// Sorts the system's many ways of saying a request did not arrive into
    /// the two that matter here: it took too long, or nobody is there.
    private static func failure(from error: Error) -> Failure {
        guard let error = error as? URLError else { return .unreachable }
        switch error.code {
        case .timedOut:
            return .timedOut
        case .badServerResponse, .cannotParseResponse, .cannotDecodeContentData, .cannotDecodeRawData:
            return .malformed
        default:
            return .unreachable
        }
    }

    /// The characters a query value may keep, which are the ones the page's
    /// own encoding keeps, so both front ends name a file the same way.
    private static let unreserved = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()")

    private static func escaped(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: unreserved) ?? ""
    }
}

/// Reports how much of a file has gone. One per upload, so that the
/// closure it holds goes away with the task.
private final class UploadWatcher: NSObject, URLSessionTaskDelegate, Sendable {
    private let progressed: @Sendable (Int64, Int64) -> Void

    init(_ progressed: @escaping @Sendable (Int64, Int64) -> Void) {
        self.progressed = progressed
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didSendBodyData bytesSent: Int64, totalBytesSent: Int64, totalBytesExpectedToSend: Int64) {
        progressed(totalBytesSent, totalBytesExpectedToSend)
    }
}
