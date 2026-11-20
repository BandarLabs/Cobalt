import Foundation

/// A server that is not there: answers come from a script.
///
/// The tests need a server that refuses in every way the real one can, and
/// the render mode needs one that holds each awkward state still while it is
/// drawn. Both are given the real client with this in place of the network,
/// so everything from the request the client builds to the decoding of the
/// reply is the shipped code. It is in the library and not in the tests
/// because the render mode is part of the app.
public final class StandIn: URLProtocol, @unchecked Sendable {
    /// One request, as the server would have seen it.
    public struct Asked: Sendable {
        public var method: String
        public var path: String
        public var query: [String: String]
        public var headers: [String: String]
        public var body: Data

        /// "GET /api/state", which is how a script names what it answers.
        public var route: String { "\(method) \(path)" }
    }

    public enum Reply: Sendable {
        case json(Int, String)
        /// The connection fails the way the system reports it.
        case fail(URLError.Code)
        /// Nothing comes back at all, which is how a wait is held open for
        /// a timeout to end or a render to draw.
        case silence
    }

    public typealias Script = @Sendable (Asked) -> Reply

    /// One stand-in server: its address, what it was asked, and a way to
    /// change what it says part way through.
    public final class Stage: @unchecked Sendable {
        public let session: Session
        private let lock = NSLock()
        private var script: Script
        private var heard: [Asked] = []

        fileprivate init(session: Session, script: @escaping Script) {
            self.session = session
            self.script = script
        }

        /// A session configuration whose requests come here and go nowhere
        /// else.
        public var configuration: URLSessionConfiguration {
            let configuration = URLSessionConfiguration.ephemeral
            configuration.protocolClasses = [StandIn.self]
            return configuration
        }

        public func client(patience: Patience = Patience()) -> Client {
            Client(session: session, patience: patience, configuration: configuration)
        }

        /// Everything asked so far, in order.
        public var asked: [Asked] {
            lock.withLock { heard }
        }

        public func say(_ script: @escaping Script) {
            lock.withLock { self.script = script }
        }

        fileprivate func answer(_ asked: Asked) -> Reply {
            let script = lock.withLock {
                heard.append(asked)
                return self.script
            }
            return script(asked)
        }

        public func close() {
            StandIn.forget(port: session.address.port ?? 0)
        }
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var stages: [Int: Stage] = [:]
    nonisolated(unsafe) private static var nextPort = 41_000

    /// Opens a stand-in server on a port of its own, so tests that run side
    /// by side never answer each other's requests.
    public static func stage(token: String = String(repeating: "5a", count: 32), _ script: @escaping Script) -> Stage {
        lock.withLock {
            nextPort += 1
            let address = URL(string: "http://127.0.0.1:\(nextPort)")!
            let stage = Stage(session: Session(address: address, token: token, kobo: nil), script: script)
            stages[nextPort] = stage
            return stage
        }
    }

    /// A script that answers each route with one reply and anything else
    /// the way the server answers a route it does not have.
    public static func routes(_ replies: [String: Reply]) -> Script {
        { asked in
            replies[asked.route] ?? .json(404, #"{"error": "There is nothing at that address.", "kind": "not_found"}"#)
        }
    }

    private static func forget(port: Int) {
        lock.withLock { stages[port] = nil }
    }

    private static func stage(for request: URLRequest) -> Stage? {
        guard let url = request.url, url.host == "127.0.0.1", let port = url.port else { return nil }
        return lock.withLock { stages[port] }
    }

    override public class func canInit(with request: URLRequest) -> Bool {
        stage(for: request) != nil
    }

    override public class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override public func startLoading() {
        guard let stage = Self.stage(for: request), let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        var query: [String: String] = [:]
        for item in URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? [] {
            query[item.name] = item.value ?? ""
        }
        let asked = Asked(
            method: request.httpMethod ?? "GET",
            path: url.path,
            query: query,
            headers: request.allHTTPHeaderFields ?? [:],
            body: Self.body(of: request)
        )
        switch stage.answer(asked) {
        case .json(let status, let text):
            let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: Data(text.utf8))
            client?.urlProtocolDidFinishLoading(self)
        case .fail(let code):
            client?.urlProtocol(self, didFailWithError: URLError(code))
        case .silence:
            break
        }
    }

    override public func stopLoading() {}

    /// A request's body, whichever of the two ways the system carries it.
    /// An upload arrives as a stream and nothing else.
    private static func body(of request: URLRequest) -> Data {
        if let body = request.httpBody { return body }
        guard let stream = request.httpBodyStream else { return Data() }
        stream.open()
        defer { stream.close() }
        var body = Data()
        var buffer = [UInt8](repeating: 0, count: 16_384)
        while stream.hasBytesAvailable {
            let read = stream.read(&buffer, maxLength: buffer.count)
            if read <= 0 { break }
            body.append(buffer, count: read)
        }
        return body
    }
}
