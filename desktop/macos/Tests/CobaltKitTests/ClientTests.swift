import Foundation
import Testing
@testable import CobaltKit

/// The state a server with one Clara BW plugged in answers with.
let oneReaderState = """
{"api": 1,
 "computer": {"kobo": "0.3.37", "os": "macos", "release": "0.3.37", "release_problem": null, "ejects": true, "upload_limit": 536870912},
 "attached": 1, "blocked": [],
 "readers": [{"volume": "/Volumes/KOBOeReader", "model": "Kobo Clara BW", "model_code": "N365", "firmware": "4.45.23697",
              "supported": true, "refusal": null, "untested": null, "standing": "current",
              "cobalt": {"cobalt_version": "0.3.37", "channel": "stable"}, "cobalt_problem": null,
              "free_bytes": 5200000000, "writable": true}],
 "jobs": []}
"""

func refusal(_ kind: String, _ sentence: String) -> String {
    #"{"error": "\#(sentence)", "kind": "\#(kind)"}"#
}

@Suite("Asking the server")
struct ClientTests {
    @Test("Every request carries the token as a bearer and nothing a browser would add")
    func requestsCarryTheToken() async throws {
        let stage = StandIn.stage(StandIn.routes([
            "GET /api/state": .json(200, oneReaderState),
            "POST /api/send/queue": .json(200, #"{"job": 10}"#),
        ]))
        defer { stage.close() }
        let client = stage.client()
        _ = try await client.state()
        _ = try await client.sendQueue(reader: nil)
        #expect(stage.asked.count == 2)
        for asked in stage.asked {
            #expect(asked.headers["Authorization"] == "Bearer \(stage.session.token)")
            // The server refuses a bearer request that carries an Origin,
            // and a cookie would be a second way in that nobody meant.
            #expect(asked.headers["Origin"] == nil)
            #expect(asked.headers["Cookie"] == nil)
        }
        #expect(stage.asked[0].method == "GET")
        #expect(stage.asked[0].body.isEmpty)
        // A change with nothing to say still says it as a JSON object.
        #expect(stage.asked[1].method == "POST")
        #expect(stage.asked[1].headers["Content-Type"] == "application/json")
        #expect(String(decoding: stage.asked[1].body, as: UTF8.self) == "{}")
    }

    @Test("A reader's volume with a space in its name is asked about under that exact name")
    func volumesAreNamedExactly() async throws {
        let stage = StandIn.stage(StandIn.routes([
            "GET /api/plan": .json(200, #"{"undo": true, "lines": ["would remove Cobalt"], "release": null, "untested": null}"#),
            "POST /api/apps/install": .json(200, #"{"job": 5}"#),
        ]))
        defer { stage.close() }
        let client = stage.client()
        let plan = try await client.plan(volume: "/Volumes/KOBOeReader 1", undo: true)
        #expect(plan.undo)
        #expect(stage.asked[0].query == ["volume": "/Volumes/KOBOeReader 1", "undo": "1"])
        let job = try await client.installApps(volume: "/Volumes/KOBOeReader 1", ids: ["almanac", "rss"])
        #expect(job == 5)
        let body = try JSONSerialization.jsonObject(with: stage.asked[1].body) as? [String: Any]
        #expect(body?["volume"] as? String == "/Volumes/KOBOeReader 1")
        #expect(body?["ids"] as? [String] == ["almanac", "rss"])
    }

    static let kinds: [(RefusalKind, Int)] = [
        (.session, 401), (.gone, 404), (.busy, 409), (.refused, 409), (.noCobalt, 409), (.readOnly, 409), (.noRoom, 409),
        (.unreachable, 502), (.integrity, 502), (.writeFailed, 500), (.notFound, 404), (.unsupportedFile, 422),
        (.tooLarge, 413), (.badRequest, 400), (.failed, 500),
    ]

    @Test("Each kind of refusal comes back as that kind, with the server's sentence as it wrote it", arguments: kinds)
    func refusalsKeepTheirKind(kind: RefusalKind, status: Int) async {
        let sentence = "The reader at /Volumes/KOBOeReader said no (\(kind.rawValue))."
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(status, refusal(kind.rawValue, sentence))]))
        defer { stage.close() }
        await #expect(throws: Failure.refused(kind: kind, sentence: sentence, status: status)) {
            _ = try await stage.client().state()
        }
    }

    @Test("Every kind the server can name is one the app knows")
    func everyKindIsCovered() {
        #expect(Set(Self.kinds.map(\.0)) == Set(RefusalKind.allCases))
    }

    @Test("A kind this build has not heard of is handled as a plain failure")
    func unknownKindsAreFailures() async {
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(418, refusal("teapot", "The server is a teapot."))]))
        defer { stage.close() }
        await #expect(throws: Failure.refused(kind: .failed, sentence: "The server is a teapot.", status: 418)) {
            _ = try await stage.client().state()
        }
    }

    @Test("A refusal with no kind is a refused session when the status says so, and says something either way")
    func refusalsWithoutAKind() async {
        let stage = StandIn.stage { asked in
            asked.path == "/api/state" ? .json(403, "Forbidden") : .json(500, "{}")
        }
        defer { stage.close() }
        await #expect(throws: Failure.refused(kind: .session, sentence: "The request was refused (403).", status: 403)) {
            _ = try await stage.client().state()
        }
        await #expect(throws: Failure.refused(kind: .failed, sentence: "The request was refused (500).", status: 500)) {
            _ = try await stage.client().browsers()
        }
    }

    @Test("A server that has gone is told apart from one that is slow")
    func goneAndSlowAreDifferent() async {
        let gone = StandIn.stage { _ in .fail(.cannotConnectToHost) }
        defer { gone.close() }
        await #expect(throws: Failure.unreachable) { _ = try await gone.client().state() }

        let dropped = StandIn.stage { _ in .fail(.networkConnectionLost) }
        defer { dropped.close() }
        await #expect(throws: Failure.unreachable) { _ = try await dropped.client().state() }

        let slow = StandIn.stage { _ in .silence }
        defer { slow.close() }
        await #expect(throws: Failure.timedOut) { _ = try await slow.client(patience: Patience(everything: 0.3)).state() }
    }

    @Test("An answer that is not the JSON expected is malformed, not a crash and not an empty state")
    func malformedAnswers() async {
        for body in ["<html>", "", "[]", #"{"api": 1}"#, #"{"api": 1, "computer": {"release": "0.3.37"}}"#] {
            let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, body)]))
            defer { stage.close() }
            await #expect(throws: Failure.malformed, "\(body)") { _ = try await stage.client().state() }
        }
    }

    @Test("Fields a server does not send yet are absent, and the rest of the answer is kept")
    func missingFieldsAreAbsent() async throws {
        let older = #"{"computer": {"kobo": "0.3.36", "release": null, "release_problem": "Nothing to install.", "ejects": true}, "readers": [{"volume": "/Volumes/KOBOeReader", "model": "Kobo Clara BW", "firmware": "4.45.23697", "supported": true, "standing": "absent", "cobalt": null}], "jobs": [{"id": 1, "kind": "reader", "title": "Installing Cobalt", "running": true, "ok": null}]}"#
        let stage = StandIn.stage(StandIn.routes([
            "GET /api/state": .json(200, older),
            "GET /api/apps": .json(200, #"{"apps": [{"id": "rss", "title": "Feeds", "version": "1.0.0"}]}"#),
        ]))
        defer { stage.close() }
        let state = try await stage.client().state()
        #expect(state.api == nil)
        #expect(state.attached == nil)
        #expect(state.blocked.isEmpty)
        #expect(state.computer.uploadLimit == nil)
        #expect(state.readers[0].freeBytes == nil)
        #expect(state.readers[0].writable == nil)
        #expect(state.jobs[0].volume == nil)
        let apps = try await stage.client().apps(volume: "/Volumes/KOBOeReader")
        #expect(apps[0].bytes == nil)
        #expect(apps[0].fits == nil)
        #expect(apps[0].runsOnThisReader)
        #expect(apps[0].summary == "")
    }

    @Test("A file goes up as its bytes under its own name, plus signs and all")
    func uploads() async throws {
        let stage = StandIn.stage(StandIn.routes([
            "POST /api/send/file": .json(200, #"{"upload": 7, "name": "Sonata + notes.pdf", "bytes": 11, "choices": [{"name": "musicstand", "title": "Music Stand", "purpose": "scores", "replaces": null}, {"name": "vault", "title": "Vault", "purpose": "notes", "replaces": null}]}"#),
        ]))
        defer { stage.close() }
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("cobalt-test-\(UUID().uuidString)").appendingPathExtension("pdf")
        try Data("hello there".utf8).write(to: file)
        defer { try? FileManager.default.removeItem(at: file) }
        let got = try await stage.client().upload(file: file, named: "Sonata + notes.pdf") { _, _ in }
        #expect(got.upload == 7)
        #expect(got.choices?.map(\.title) == ["Music Stand", "Vault"])
        #expect(got.app == nil)
        let asked = stage.asked[0]
        #expect(asked.headers["Content-Type"] == "application/octet-stream")
        #expect(asked.query["name"] == "Sonata + notes.pdf")
        #expect(String(decoding: asked.body, as: UTF8.self) == "hello there")
        #expect(asked.headers["Authorization"] == "Bearer \(stage.session.token)")
    }

    @Test("A send says only what the owner decided and leaves the rest to the command")
    func sendBodies() async throws {
        let stage = StandIn.stage { _ in .json(200, #"{"job": 8}"#) }
        defer { stage.close() }
        let client = stage.client()
        _ = try await client.startSend(upload: 7, to: nil, replace: false, reader: "")
        _ = try await client.startSend(upload: 7, to: "panels", replace: true, reader: "kobo")
        _ = try await client.sendAddress("https://example.org/a?b=c", reader: nil)
        let bodies = try stage.asked.map { try JSONSerialization.jsonObject(with: $0.body) as? [String: AnyHashable] }
        #expect(bodies[0] == ["upload": 7])
        #expect(bodies[1] == ["upload": 7, "to": "panels", "replace": true, "reader": "kobo"])
        #expect(bodies[2] == ["url": "https://example.org/a?b=c"])
    }
}

@Suite("Being handed a server")
struct HandoffTests {
    let token = String(repeating: "ab", count: 32)

    @Test("The one line kobo ui writes becomes a session")
    func goodLine() {
        let session = Handoff.session(fromLine: #"{"address":"http://127.0.0.1:53124","token":"\#(token)","kobo":"0.3.37"}"#)
        #expect(session == Session(address: URL(string: "http://127.0.0.1:53124")!, token: token, kobo: "0.3.37"))
    }

    @Test("A line that would send the token anywhere but this computer is not acted on")
    func otherHostsAreRefused() {
        for address in ["http://192.168.1.20:53124", "http://localhost.example.org:53124", "https://127.0.0.1:53124", "http://127.0.0.1", "http://user@127.0.0.1:53124", "http://127.0.0.1:53124/elsewhere", "file:///tmp/x"] {
            #expect(Handoff.session(fromLine: #"{"address":"\#(address)","token":"\#(token)"}"#) == nil, "\(address)")
        }
    }

    @Test("A token that could not sit in a header is not a token")
    func tokensAreChecked() {
        for bad in ["", "abc def", "abc\\r\\nX-Other: 1"] {
            #expect(Handoff.session(fromLine: #"{"address":"http://127.0.0.1:53124","token":"\#(bad)"}"#) == nil, "\(bad)")
        }
        #expect(Handoff.session(fromLine: "not json") == nil)
        #expect(Handoff.session(fromLine: #"{"address":"http://127.0.0.1:53124"}"#) == nil)
    }

    @Test("A server started by hand can be named in the environment")
    func environment() {
        #expect(Handoff.session(environment: ["COBALT_UI_ADDRESS": "http://127.0.0.1:4000/", "COBALT_UI_TOKEN": "abc123"]) == Session(address: URL(string: "http://127.0.0.1:4000")!, token: "abc123"))
        #expect(Handoff.session(environment: ["COBALT_UI_ADDRESS": "http://127.0.0.1:4000"]) == nil)
        #expect(Handoff.session(environment: [:]) == nil)
    }
}

/// Collects what a callback was told, from whatever thread told it.
final class Heard: @unchecked Sendable {
    private let lock = NSLock()
    private var said: [String] = []

    func hear(_ text: String) { lock.withLock { said.append(text) } }
    var all: [String] { lock.withLock { said } }
}

@Suite("Starting the kobo command")
struct ServerProcessTests {
    let shell = URL(fileURLWithPath: "/bin/sh")
    let line = #"{"address":"http://127.0.0.1:53124","token":"\#(String(repeating: "ab", count: 32))","kobo":"0.3.37"}"#

    @Test("A command that says where its server is gives a session, and stops when its input closes")
    func startsAndStops() async throws {
        let heard = Heard()
        // Prints the line, then does what the real command does: waits for
        // its standard input to close.
        let server = try await ServerProcess.start(command: shell, arguments: ["-c", "echo '\(line)'; cat > /dev/null"], patience: 5) { heard.hear($0) }
        #expect(server.session.address == URL(string: "http://127.0.0.1:53124"))
        #expect(server.session.kobo == "0.3.37")
        server.stop()
        try await Task.sleep(nanoseconds: 700_000_000)
        // It was asked to stop, so its stopping is not reported as news.
        #expect(heard.all.isEmpty)
    }

    @Test("A command that is not there is reported as missing, with where it was looked for")
    func missing() async {
        let nowhere = URL(fileURLWithPath: "/nonexistent/cobalt-test/kobo")
        await #expect(throws: ServerProcess.Trouble.missing(path: "/nonexistent/cobalt-test/kobo")) {
            _ = try await ServerProcess.start(command: nowhere)
        }
    }

    @Test("A command that stops before it has a server is reported with what it said")
    func exitsEarly() async {
        await #expect(throws: ServerProcess.Trouble.exited(said: "kobo: the port could not be opened")) {
            _ = try await ServerProcess.start(command: shell, arguments: ["-c", "echo 'kobo: the port could not be opened' >&2; exit 1"], patience: 5)
        }
    }

    @Test("A command that says nothing is given up on when the time allowed has passed")
    func silent() async {
        await #expect(throws: ServerProcess.Trouble.silent(seconds: 1)) {
            _ = try await ServerProcess.start(command: shell, arguments: ["-c", "sleep 30"], patience: 0.6)
        }
    }

    @Test("A command that says something other than a handoff, or names another computer, is not believed")
    func garbled() async {
        await #expect(throws: ServerProcess.Trouble.garbled) {
            _ = try await ServerProcess.start(command: shell, arguments: ["-c", "echo 'Cobalt is open at http://127.0.0.1:5000'; cat > /dev/null"], patience: 5)
        }
        await #expect(throws: ServerProcess.Trouble.garbled) {
            _ = try await ServerProcess.start(command: shell, arguments: ["-c", #"echo '{"address":"http://10.0.0.5:80","token":"abcd"}'; cat > /dev/null"#], patience: 5)
        }
    }

    @Test("A command that stops later, unasked, is reported with the last thing it said")
    func stopsLater() async throws {
        let heard = Heard()
        let server = try await ServerProcess.start(command: shell, arguments: ["-c", "echo '\(line)'; sleep 0.3; echo 'kobo: gave up' >&2; exit 3"], patience: 5) { heard.hear($0) }
        try await Task.sleep(nanoseconds: 1_500_000_000)
        #expect(heard.all == ["kobo: gave up"])
        _ = server
    }

    @Test("The line kobo ui writes to the app it starts is read, and silence is not waited on for ever")
    func ownStandardInput() async throws {
        let pipe = Pipe()
        pipe.fileHandleForWriting.write(Data((line + "\n").utf8))
        try pipe.fileHandleForWriting.close()
        #expect(await Handoff.line(from: pipe.fileHandleForReading, within: 2) == line)

        let quiet = Pipe()
        #expect(await Handoff.line(from: quiet.fileHandleForReading, within: 0.3) == nil)
        _ = quiet.fileHandleForWriting
    }
}
