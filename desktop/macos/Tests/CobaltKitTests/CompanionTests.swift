import Foundation
import Testing
@testable import CobaltKit

/// What a stand-in server says, changeable while a test runs, so a test can
/// unplug a reader or let a job finish between one request and the next.
final class Board: @unchecked Sendable {
    private let lock = NSLock()
    private var replies: [String: [StandIn.Reply]]

    init(_ replies: [String: StandIn.Reply] = [:]) {
        self.replies = replies.mapValues { [$0] }
    }

    /// One reply for a route from now on.
    subscript(route: String) -> StandIn.Reply? {
        get { lock.withLock { replies[route]?.first } }
        set { lock.withLock { replies[route] = newValue.map { [$0] } } }
    }

    /// Replies given in turn, the last one repeating.
    func inTurn(_ route: String, _ sequence: [StandIn.Reply]) {
        lock.withLock { replies[route] = sequence }
    }

    var script: StandIn.Script {
        { asked in
            self.lock.withLock {
                guard var waiting = self.replies[asked.route], let next = waiting.first else {
                    return .json(404, #"{"error": "There is nothing at that address.", "kind": "not_found"}"#)
                }
                if waiting.count > 1 {
                    waiting.removeFirst()
                    self.replies[asked.route] = waiting
                }
                return next
            }
        }
    }
}

/// A clock a test moves by hand.
final class Clock: @unchecked Sendable {
    private let lock = NSLock()
    private var time = Date(timeIntervalSince1970: 1_800_000_000)

    var now: Date { lock.withLock { time } }
    func pass(_ seconds: TimeInterval) { lock.withLock { time += seconds } }
}

func stateJSON(readers: String = "", jobs: String = "", api: String = "1", kobo: String = "0.3.37", release: String = #""0.3.37""#, more: String = "") -> String {
    """
    {"api": \(api), "computer": {"kobo": "\(kobo)", "os": "macos", "release": \(release), "release_problem": \(release == "null" ? #""This copy of the kobo command has nothing to put on a reader.""# : "null"), "ejects": true, "upload_limit": 536870912},
     "blocked": [], "readers": [\(readers)], "jobs": [\(jobs)]\(more.isEmpty ? "" : ", " + more)}
    """
}

func readerJSON(volume: String = "/Volumes/KOBOeReader", model: String = "Kobo Clara BW", standing: String = "absent", cobalt: String? = nil, more: String = "") -> String {
    let on = cobalt.map { #"{"cobalt_version": "\#($0)", "channel": "stable"}"# } ?? "null"
    return #"{"volume": "\#(volume)", "model": "\#(model)", "firmware": "4.45.23697", "supported": true, "standing": "\#(standing)", "cobalt": \#(on), "free_bytes": 5200000000, "writable": true\#(more.isEmpty ? "" : ", " + more)}"#
}

func jobJSON(id: Int = 3, title: String = "Installing Cobalt", running: Bool, ok: String = "null", text: String = "", next: Int = 0, kind: String = "reader", volume: String? = "/Volumes/KOBOeReader") -> String {
    let said = String(decoding: try! JSONSerialization.data(withJSONObject: [text]), as: UTF8.self).dropFirst().dropLast()
    return #"{"id": \#(id), "kind": "\#(kind)", "title": "\#(title)", "running": \#(running), "ok": \#(ok), "code": \#(ok == "false" ? "1" : "null"), "text": \#(said), "next": \#(next), "volume": \#(volume.map { "\"\($0)\"" } ?? "null")}"#
}

@MainActor
func companion(_ stage: StandIn.Stage, appVersion: String? = "0.3.37", origin: Origin = .finder, clock: Clock = Clock(), place: Place = .reader) -> Companion {
    Companion(place: place, appVersion: appVersion, origin: origin, now: { clock.now }) {
        .success(stage.client(patience: Patience(everything: 2)))
    }
}

@MainActor
@Suite("Keeping track of the server")
struct ConnectionTests {
    @Test("A window that finds its server reads the state and shows the reader")
    func connects() async {
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, oneReaderState)]))
        defer { stage.close() }
        let store = companion(stage)
        #expect(store.link == .starting)
        await store.start()
        #expect(store.link == .connected)
        #expect(store.chosen.reader?.model == "Kobo Clara BW")
        #expect(store.versionTrouble == nil)
    }

    @Test("A window that cannot start its server says which way it failed and offers to start again")
    func cannotStart() async {
        let reasons: [Lost.Reason] = [.missing(path: "/Applications/kobo"), .exited(said: "kobo: the port could not be opened"), .silent(seconds: 10), .garbled]
        for reason in reasons {
            let store = Companion { .failure(Lost(reason)) }
            await store.start()
            #expect(store.link == .lost(Lost(reason)))
        }
        #expect(Lost(.missing(path: "/Applications/kobo")).detail.contains("/Applications/kobo"))
        #expect(Lost(.exited(said: "kobo: the port could not be opened")).detail == "The port could not be opened.")
        #expect(Lost(.exited(said: "")).detail == "It did not say why.")
        // Where the headline and the button say everything, nothing is added.
        #expect(Lost(.stopped).detail == "")
        #expect(Lost(.refused).detail == "")
        #expect(Lost(.silent(seconds: 10)).detail == "")
        #expect(Lost(.silent(seconds: 10)).headline == "The kobo command did not answer within 10 seconds.")
    }

    @Test("Starting again after the server has gone connects to the new one")
    func startsAgain() async {
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, oneReaderState)]))
        defer { stage.close() }
        var attempts = 0
        let store = Companion {
            attempts += 1
            return attempts == 1 ? .failure(Lost(.silent(seconds: 10))) : .success(stage.client())
        }
        await store.start()
        #expect(store.link == .lost(Lost(.silent(seconds: 10))))
        await store.start()
        #expect(store.link == .connected)
        #expect(store.state != nil)
    }

    @Test("One missed answer is a busy moment and three in a row is a server that has gone")
    func threeMisses() async {
        let board = Board(["GET /api/state": .json(200, oneReaderState)])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        board["GET /api/state"] = .fail(.cannotConnectToHost)
        await store.refresh()
        await store.refresh()
        #expect(store.link == .connected)
        // An answer in between starts the count again.
        board["GET /api/state"] = .json(200, oneReaderState)
        await store.refresh()
        board["GET /api/state"] = .fail(.cannotConnectToHost)
        await store.refresh()
        await store.refresh()
        #expect(store.link == .connected)
        await store.refresh()
        #expect(store.link == .lost(Lost(.stopped)))
        // What was known is kept, so the window can still say what it was showing.
        #expect(store.state != nil)
    }

    @Test("A server that is slow three times running is treated as gone too, and one that talks nonsense is called that")
    func slowAndGarbled() async {
        let board = Board(["GET /api/state": .json(200, oneReaderState)])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = Companion { .success(stage.client(patience: Patience(everything: 0.2))) }
        await store.start()
        board["GET /api/state"] = .silence
        for _ in 0..<3 { await store.refresh() }
        #expect(store.link == .lost(Lost(.stopped)))

        let nonsense = StandIn.stage { _ in .json(200, "<html>") }
        defer { nonsense.close() }
        let other = companion(nonsense)
        await other.start()
        await other.refresh()
        await other.refresh()
        #expect(other.link == .lost(Lost(.garbled)))
    }

    @Test("A session the server refuses ends the window's usefulness at once, whatever was being asked")
    func refusedSession() async {
        let board = Board(["GET /api/state": .json(200, oneReaderState)])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        board["GET /api/state"] = .json(401, refusal("session", "This session is not accepted."))
        await store.refresh()
        #expect(store.link == .lost(Lost(.refused)))

        let again = companion(stage)
        board["GET /api/state"] = .json(200, oneReaderState)
        await again.start()
        board["GET /api/browsers"] = .json(403, refusal("session", "This session is not accepted."))
        await again.loadBrowsers()
        #expect(again.link == .lost(Lost(.refused)))
    }

    @Test("The kobo command exiting is noticed without waiting for three missed answers")
    func serverExit() async {
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, oneReaderState)]))
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        store.serverStopped(said: "")
        #expect(store.link == .lost(Lost(.stopped)))
    }

    @Test("A server that speaks another contract is not worked with, and one of another release is worked with and remarked on")
    func versions() async {
        let other = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, stateJSON(api: "2", kobo: "0.4.0"))]))
        defer { other.close() }
        let store = companion(other)
        await store.start()
        #expect(store.versionTrouble == .contract(app: "0.3.37", kobo: "0.4.0"))
        #expect(store.versionTrouble?.blocks == true)

        // A server from before the contract had a number is another contract.
        let unnumbered = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, #"{"computer": {"kobo": "0.3.30"}, "readers": []}"#)]))
        defer { unnumbered.close() }
        let old = companion(unnumbered)
        await old.start()
        #expect(old.versionTrouble?.blocks == true)

        let behind = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, stateJSON(kobo: "0.3.36"))]))
        defer { behind.close() }
        let mixed = companion(behind)
        await mixed.start()
        #expect(mixed.versionTrouble == .release(app: "0.3.37", kobo: "0.3.36"))
        #expect(mixed.versionTrouble?.blocks == false)
        #expect(mixed.versionTrouble?.detail == "This window is Cobalt 0.3.37 and the kobo command is 0.3.36. Run kobo update in a terminal, then open Cobalt again.")

        // A build run outside its bundle has no version of its own to compare.
        let bare = companion(behind, appVersion: nil)
        await bare.start()
        #expect(bare.versionTrouble == nil)
    }
}

@MainActor
@Suite("The Reader place")
struct ReaderPlaceTests {
    let two = stateJSON(readers: readerJSON() + ", " + readerJSON(volume: "/Volumes/KOBOeReader 1", model: "Kobo Libra 2"))

    @Test("With two readers nothing is chosen until the owner chooses, and unplugging the chosen one asks again")
    func twoReaders() async {
        let board = Board(["GET /api/state": .json(200, two)])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        #expect(store.chosen.reader == nil)
        store.choose(volume: "/Volumes/KOBOeReader 1")
        #expect(store.chosen.reader?.model == "Kobo Libra 2")
        board["GET /api/state"] = .json(200, stateJSON(readers: readerJSON()))
        await store.refresh()
        // One reader left needs no choosing.
        #expect(store.chosen.reader?.model == "Kobo Clara BW")
        #expect(store.chosenVolume == nil)
    }

    @Test("The plan is shown before anything is written and agreeing to it starts the job it described")
    func planThenInstall() async {
        let board = Board([
            "GET /api/state": .json(200, stateJSON(readers: readerJSON())),
            "GET /api/plan": .json(200, #"{"undo": false, "lines": ["would install Cobalt into /Volumes/KOBOeReader/.adds/cobalt", "would eject the reader"], "release": "0.3.37", "untested": null}"#),
            "POST /api/reader/install": .json(200, #"{"job": 3}"#),
        ])
        board.inTurn("GET /api/job", [
            .json(200, jobJSON(running: true, text: "found Kobo Clara BW\n", next: 20)),
            .json(200, jobJSON(running: true, text: "\nNext, on the reader:\n\n  1. Unplug it.\n", next: 60)),
            .json(200, jobJSON(running: false, ok: "true", text: "The reader is back on Wi-Fi.\n", next: 90)),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        await store.openPlan(undo: false)
        #expect(store.reader.plan?.plan.lines.count == 2)
        #expect(!stage.asked.contains { $0.method == "POST" })

        await store.agreeToPlan()
        #expect(store.reader.plan == nil)
        #expect(store.reader.job?.id == 3)
        #expect(store.reader.job?.text == "found Kobo Clara BW\n")
        #expect(store.reader.job?.again == .install(volume: "/Volumes/KOBOeReader"))
        await store.tick()
        #expect(stage.asked.last?.query == ["id": "3", "from": "20"])
        await store.tick()
        #expect(store.reader.job?.running == false)
        #expect(store.reader.job?.text == "found Kobo Clara BW\n\nNext, on the reader:\n\n  1. Unplug it.\nThe reader is back on Wi-Fi.\n")
        let scene = JobScene.scene(for: store.reader.job!, state: store.state, now: Date())
        #expect(scene.phase == .done)
        #expect(scene.heading == "Cobalt is installed.")
        #expect(scene.next.first == .heading("Next, on the reader"))
        #expect(scene.report == [.paragraph("found Kobo Clara BW")])
        // A finished job is not asked about again.
        let asked = stage.asked.count
        await store.tick()
        #expect(stage.asked.count == asked)
    }

    @Test("A plan the server refuses is not shown, and its reason is")
    func refusedPlan() async {
        let reasons: [(String, Int, String)] = [
            ("refused", 409, "firmware 5.2.190625 cannot start Cobalt: NickelMenu does not hook the 5.x firmware"),
            ("gone", 404, "That reader is no longer connected."),
            ("read_only", 409, "The reader's storage is mounted read-only."),
        ]
        for (kind, status, sentence) in reasons {
            let stage = StandIn.stage(StandIn.routes([
                "GET /api/state": .json(200, stateJSON(readers: readerJSON())),
                "GET /api/plan": .json(status, refusal(kind, sentence)),
            ]))
            defer { stage.close() }
            let store = companion(stage)
            await store.start()
            await store.openPlan(undo: false)
            #expect(store.reader.plan == nil)
            #expect(store.reader.error == sentence)
            #expect(store.link == .connected)
        }
    }

    @Test("An install refused because another job is running keeps the plan open and says which job")
    func busy() async {
        let stage = StandIn.stage(StandIn.routes([
            "GET /api/state": .json(200, stateJSON(readers: readerJSON())),
            "GET /api/plan": .json(200, #"{"undo": false, "lines": ["would install Cobalt"], "release": "0.3.37"}"#),
            "POST /api/reader/install": .json(409, refusal("busy", "Installing apps is still running. Wait for it to finish.")),
        ]))
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        await store.openPlan(undo: false)
        await store.agreeToPlan()
        #expect(store.reader.job == nil)
        #expect(store.reader.plan != nil)
        #expect(store.reader.error == "Installing apps is still running. Wait for it to finish.")
    }

    @Test("An install that fails part way shows the command's own reason and what the reader has now")
    func failedInstall() async {
        let board = Board([
            "GET /api/state": .json(200, stateJSON(readers: readerJSON(standing: "behind", cobalt: "0.3.30"))),
            "GET /api/plan": .json(200, #"{"undo": false, "lines": ["would install Cobalt"], "release": "0.3.37"}"#),
            "POST /api/reader/install": .json(200, #"{"job": 4}"#),
            "GET /api/job": .json(200, jobJSON(id: 4, title: "Updating Cobalt", running: false, ok: "false", text: "found Kobo Clara BW\nkobo: cobalt-0.3.37-KoboRoot.tgz is truncated: manifest says 27699108 bytes, found 9000000\n", next: 110)),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        await store.openPlan(undo: false)
        await store.agreeToPlan()
        let scene = JobScene.scene(for: store.reader.job!, state: store.state, now: Date())
        #expect(scene.phase == .failed)
        #expect(scene.heading == "Updating Cobalt did not finish.")
        #expect(scene.reason == "Cobalt-0.3.37-KoboRoot.tgz is truncated: manifest says 27699108 bytes, found 9000000.")
        #expect(scene.aftermath == "The reader is still connected and has Cobalt 0.3.30 on it.")

        // Trying again runs the same thing without going back through the plan.
        board["GET /api/job"] = .json(200, jobJSON(id: 5, title: "Updating Cobalt", running: true, next: 0))
        board["POST /api/reader/install"] = .json(200, #"{"job": 5}"#)
        await store.tryReaderJobAgain()
        #expect(store.reader.job?.id == 5)
        #expect(store.reader.job?.running == true)
    }

    @Test("A command that stopped without a word is said to have, and a job the server has forgotten is not followed for ever")
    func silentAndForgotten() async {
        let board = Board([
            "GET /api/state": .json(200, stateJSON(readers: readerJSON(), jobs: #"{"id": 9, "kind": "reader", "title": "Removing Cobalt", "running": true, "ok": null, "volume": "/Volumes/KOBOeReader"}"#)),
            "GET /api/job": .json(404, refusal("not_found", "There is no job 9.")),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        // The job was running when the window opened, so it is picked up.
        #expect(store.reader.job?.id == 9)
        await store.tick()
        #expect(store.reader.job?.running == false)
        let scene = JobScene.scene(for: store.reader.job!, state: store.state, now: Date())
        #expect(scene.phase == .failed)
        #expect(scene.reason == "This job is no longer remembered.")

        var wordless = FollowedJob(id: 1, kind: .reader, title: "Removing Cobalt", volume: nil, began: Date())
        wordless.running = false
        wordless.ok = false
        #expect(JobScene.scene(for: wordless, state: nil, now: Date()).reason == "The command stopped without saying why.")
    }

    @Test("A reader pulled out under a running job is remarked on, and one the install ejected itself is not")
    func pulledMidway() async {
        let board = Board([
            "GET /api/state": .json(200, stateJSON(readers: readerJSON())),
            "GET /api/plan": .json(200, #"{"undo": false, "lines": ["would install Cobalt"], "release": "0.3.37"}"#),
            "POST /api/reader/install": .json(200, #"{"job": 3}"#),
        ])
        board.inTurn("GET /api/job", [
            .json(200, jobJSON(running: true, text: "found Kobo Clara BW\n", next: 20)),
            .json(200, jobJSON(running: true, text: "\nSet up /Volumes/KOBOeReader:\n  · volume ejected\n\nNext, on the reader:\n\n  1. Unplug it.\n", next: 90)),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let clock = Clock()
        let store = companion(stage, clock: clock)
        await store.start()
        await store.openPlan(undo: false)
        await store.agreeToPlan()
        #expect(JobScene.scene(for: store.reader.job!, state: store.state, now: clock.now).pulled == "")

        board["GET /api/state"] = .json(200, stateJSON())
        await store.refresh()
        let pulled = JobScene.scene(for: store.reader.job!, state: store.state, now: clock.now)
        #expect(pulled.phase == .running)
        #expect(pulled.pulled.hasPrefix("The reader was disconnected while this was running."))

        await store.tick()
        let ejected = JobScene.scene(for: store.reader.job!, state: store.state, now: clock.now)
        #expect(ejected.pulled == "")
        #expect(ejected.waiting == "")
        #expect(ejected.next.first == .heading("Next, on the reader"))
        #expect(ejected.turned)
    }

    @Test("A job that has printed nothing for two minutes says so, and after ten says what can be done")
    func quietJobs() async {
        let board = Board([
            "GET /api/state": .json(200, stateJSON(readers: readerJSON(), jobs: #"{"id": 3, "kind": "reader", "title": "Installing Cobalt", "running": true, "ok": null, "volume": "/Volumes/KOBOeReader"}"#)),
            "GET /api/job": .json(200, jobJSON(running: true, text: "", next: 0)),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let clock = Clock()
        let store = companion(stage, clock: clock)
        await store.start()
        clock.pass(45)
        await store.tick()
        var scene = JobScene.scene(for: store.reader.job!, state: store.state, now: clock.now)
        #expect(scene.quiet == "")
        #expect(scene.elapsed == "45 s")
        #expect(scene.waiting == "Leave the reader plugged in.")
        clock.pass(80)
        scene = JobScene.scene(for: store.reader.job!, state: store.state, now: clock.now)
        #expect(scene.quiet == "Nothing new for 2 min 5 s. It is still running.")
        clock.pass(600)
        scene = JobScene.scene(for: store.reader.job!, state: store.state, now: clock.now)
        #expect(scene.quiet.contains("longer than any step should take"))
        #expect(scene.quiet.contains("Activity Monitor"))
    }

    @Test("Ejecting says the reader can be unplugged, and a reader that will not eject says why")
    func ejecting() async {
        let board = Board([
            "GET /api/state": .json(200, stateJSON(readers: readerJSON())),
            "POST /api/reader/eject": .json(500, refusal("failed", "The reader could not be ejected: a file on it is open in Preview.")),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage)
        await store.start()
        await store.eject(volume: "/Volumes/KOBOeReader")
        #expect(store.reader.error == "The reader could not be ejected: a file on it is open in Preview.")
        #expect(store.reader.notice == "")

        board["POST /api/reader/eject"] = .json(200, #"{"ejected": true}"#)
        board["GET /api/state"] = .json(200, stateJSON())
        await store.eject(volume: "/Volumes/KOBOeReader")
        #expect(store.reader.notice == "The Kobo Clara BW is ejected and can be unplugged.")
        #expect(store.reader.error == "")
        #expect(store.chosen == .none)
    }

    @Test("Closing the window on a running job asks first and says truthfully what carries on")
    func quitting() async {
        let board = Board([
            "GET /api/state": .json(200, stateJSON(readers: readerJSON(), jobs: #"{"id": 3, "kind": "reader", "title": "Installing Cobalt", "running": true, "ok": null, "volume": "/Volumes/KOBOeReader"}"#)),
            "GET /api/job": .json(200, jobJSON(running: true, next: 0)),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let alone = companion(stage, origin: .finder)
        #expect(alone.quitQuestion == nil)
        await alone.start()
        #expect(alone.quitQuestion?.title == "Installing Cobalt is still running.")
        #expect(alone.quitQuestion?.detail == "It finishes by itself if you quit. Leave the reader plugged in until it has. The steps for the reader appear only in this window.")

        let fromTerminal = companion(stage, origin: .terminal)
        await fromTerminal.start()
        #expect(fromTerminal.quitQuestion?.detail.hasSuffix("will also be in the terminal that started Cobalt.") == true)

        // A removal has no steps afterwards to miss.
        board["GET /api/state"] = .json(200, stateJSON(readers: readerJSON(), jobs: #"{"id": 4, "kind": "reader", "title": "Removing Cobalt", "running": true, "ok": null}"#))
        let removing = companion(stage)
        await removing.start()
        #expect(removing.quitQuestion?.detail == "It finishes by itself if you quit. Leave the reader plugged in until it has.")
    }
}

@MainActor
@Suite("The Apps place")
struct AppsPlaceTests {
    let withCobalt = stateJSON(readers: readerJSON(standing: "current", cobalt: "0.3.37"))
    let installed = #"{"apps": [{"id": "rss", "title": "Feeds", "summary": "Reads feeds.", "version": "1.0.0", "installed_version": "1.0.0"}]}"#
    let catalog = #"{"apps": [{"id": "rss", "title": "Feeds", "summary": "Reads feeds.", "version": "1.1.0", "installed_version": "1.0.0", "update_available": true, "runs_on_this_reader": true}, {"id": "almanac", "title": "Almanac", "summary": "Sun and moon.", "version": "1.0.0", "runs_on_this_reader": true, "bytes": 1900412, "fits": true}]}"#

    @Test("The reader's own apps are listed as soon as they are read, and the catalog is added when it arrives")
    func loads() async {
        let stage = StandIn.stage(StandIn.routes([
            "GET /api/state": .json(200, withCobalt),
            "GET /api/apps": .json(200, installed),
            "POST /api/apps/catalog": .json(200, catalog),
        ]))
        defer { stage.close() }
        let store = companion(stage, place: .apps)
        await store.start()
        #expect(store.apps.installed == nil)
        await store.loadApps()
        #expect(store.shelf.installed.map(\.id) == ["rss"])
        #expect(store.shelf.available.map(\.id) == ["almanac"])
        #expect(store.shelf.updates == ["rss"])
        #expect(store.apps.loading == false)
    }

    @Test("A catalog that cannot be reached, or fails its signature, leaves the reader's apps listed and says which it was")
    func catalogTroubles() async {
        let troubles: [(StandIn.Reply, Failure)] = [
            (.json(502, refusal("unreachable", "The app catalog could not be reached: no route to host.")), .refused(kind: .unreachable, sentence: "The app catalog could not be reached: no route to host.", status: 502)),
            (.json(502, refusal("integrity", "The app catalog's signature does not match. Nothing was changed.")), .refused(kind: .integrity, sentence: "The app catalog's signature does not match. Nothing was changed.", status: 502)),
            (.silence, .timedOut),
        ]
        for (reply, failure) in troubles {
            let stage = StandIn.stage(StandIn.routes([
                "GET /api/state": .json(200, withCobalt),
                "GET /api/apps": .json(200, installed),
                "POST /api/apps/catalog": reply,
            ]))
            defer { stage.close() }
            let store = Companion(place: .apps) { .success(stage.client(patience: Patience(everything: 0.3))) }
            await store.start()
            await store.loadApps()
            #expect(store.apps.problem == failure)
            #expect(store.shelf.installed.map(\.id) == ["rss"])
            #expect(store.shelf.updates.isEmpty)
            #expect(store.apps.catalog == nil)
            #expect(store.apps.loading == false)
        }
    }

    @Test("A reader without Cobalt has no apps to list and none are asked for")
    func noCobalt() async {
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, stateJSON(readers: readerJSON()))]))
        defer { stage.close() }
        let store = companion(stage, place: .apps)
        await store.start()
        await store.loadApps()
        #expect(store.appsReader == nil)
        #expect(stage.asked.map(\.path) == ["/api/state"])
    }

    @Test("An install that is refused for want of room, or because the card is read-only, says so and starts nothing")
    func refusedInstalls() async {
        let refusals = [
            ("no_room", "Almanac needs 1.9 MB and the reader has 1.2 MB free."),
            ("read_only", "The reader's storage is mounted read-only, so nothing can be installed."),
            ("busy", "Installing Cobalt is still running."),
            ("no_cobalt", "This reader has no Cobalt to install apps into."),
        ]
        for (kind, sentence) in refusals {
            let stage = StandIn.stage(StandIn.routes([
                "GET /api/state": .json(200, withCobalt),
                "GET /api/apps": .json(200, installed),
                "POST /api/apps/catalog": .json(200, catalog),
                "POST /api/apps/install": .json(409, refusal(kind, sentence)),
            ]))
            defer { stage.close() }
            let store = companion(stage, place: .apps)
            await store.start()
            await store.loadApps()
            await store.installApps(["almanac"])
            #expect(store.apps.job == nil)
            #expect(store.apps.refusal?.sentence == sentence)
            #expect(store.apps.refusal?.kind == RefusalKind(rawValue: kind))
        }
    }

    @Test("An install is followed to its end, the lists are read again, and the reader is marked as wanting an eject")
    func installs() async {
        let board = Board([
            "GET /api/state": .json(200, withCobalt),
            "GET /api/apps": .json(200, installed),
            "POST /api/apps/catalog": .json(200, catalog),
            "POST /api/apps/install": .json(200, #"{"job": 5}"#),
        ])
        board.inTurn("GET /api/job", [
            .json(200, jobJSON(id: 5, title: "Installing an app", running: true, text: "Almanac: fetching\n", next: 18)),
            .json(200, jobJSON(id: 5, title: "Installing an app", running: false, ok: "true", text: "Almanac 1.0.0 installed.\n", next: 43)),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage, place: .apps)
        await store.start()
        await store.loadApps()
        await store.installApps(["almanac"])
        #expect(store.apps.job?.running == true)
        #expect(store.apps.changed == false)
        let before = stage.asked.filter { $0.path == "/api/apps" }.count
        await store.tick()
        #expect(store.apps.job?.ok == true)
        #expect(store.apps.changed)
        #expect(stage.asked.filter { $0.path == "/api/apps" }.count == before + 1)
    }

    @Test("Removing an app says its data is kept, and removing one that is no longer there says that")
    func removes() async {
        let board = Board([
            "GET /api/state": .json(200, withCobalt),
            "GET /api/apps": .json(200, installed),
            "POST /api/apps/catalog": .json(200, catalog),
            "POST /api/apps/remove": .json(200, #"{"apps": [], "removed": {"id": "rss", "title": "Feeds"}}"#),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage, place: .apps)
        await store.start()
        await store.loadApps()
        let feeds = store.shelf.installed[0].app
        board["GET /api/apps"] = .json(200, #"{"apps": []}"#)
        await store.removeApp(id: feeds.id, title: feeds.title)
        #expect(store.apps.said == "Feeds is removed. Its data is kept.")
        #expect(store.apps.changed)
        #expect(store.shelf.installed.isEmpty)
        #expect(store.apps.removing == nil)

        board["POST /api/apps/remove"] = .json(404, refusal("not_found", "Feeds is not installed on this reader."))
        await store.removeApp(id: feeds.id, title: feeds.title)
        #expect(store.apps.refusal?.kind == .notFound)
        #expect(store.apps.refusal?.sentence == "Feeds is not installed on this reader.")
    }

    @Test("The lists belong to one reader and one Cobalt, and are dropped when either changes")
    func listsFollowTheReader() async {
        let board = Board([
            "GET /api/state": .json(200, withCobalt),
            "GET /api/apps": .json(200, installed),
            "POST /api/apps/catalog": .json(200, catalog),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage, place: .apps)
        await store.start()
        store.search("feed")
        await store.loadApps()
        #expect(store.apps.installed != nil)
        board["GET /api/state"] = .json(200, stateJSON())
        await store.refresh()
        #expect(store.apps.installed == nil)
        #expect(store.apps.catalog == nil)
        #expect(store.apps.key == "")
        // What was typed in the search field is the owner's and is kept.
        #expect(store.apps.query == "feed")
    }
}

@MainActor
@Suite("The Browser and Send places")
struct SendPlaceTests {
    let base: [String: StandIn.Reply] = [
        "GET /api/state": .json(200, oneReaderState),
        "GET /api/send": .json(200, #"{"readers": [{"name": "kobo", "paired": true}], "waiting": [{"title": "Tides", "site": "example.org"}]}"#),
    ]

    func file(_ name: String, bytes: Int) throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("cobalt-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appendingPathComponent(name)
        try Data(repeating: 0x41, count: bytes).write(to: url)
        return url
    }

    @Test("Browsers are listed with what each still needs, and removing one says what is left to do by hand")
    func browsers() async {
        let board = Board([
            "GET /api/state": .json(200, oneReaderState),
            "GET /api/browsers": .json(200, #"{"browsers": [{"id": "chrome", "title": "Chrome", "set_up": false, "step": ""}]}"#),
            "POST /api/browsers/setup": .json(200, #"{"browsers": [{"id": "chrome", "title": "Chrome", "set_up": true, "step": "Chrome: open chrome://extensions and pick\n  /Users/me/.config/kobo/clip/extension"}], "said": "Registered."}"#),
            "POST /api/browsers/remove": .json(200, #"{"browsers": [{"id": "chrome", "title": "Chrome", "set_up": false, "step": ""}], "said": "Removed."}"#),
        ])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage, place: .browser)
        await store.start()
        await store.loadBrowsers()
        let chrome = store.browser.list![0]
        await store.change(browser: chrome, setUp: true)
        #expect(store.browser.list?[0].setUp == true)
        #expect(store.browser.busy == nil)
        // What was just asked for is said once, to the owner who asked.
        #expect(store.browser.said["chrome"] == "Send to Kobo adds Send page, Send link and Send selection to the right-click menu.")
        await store.change(browser: chrome, setUp: false)
        #expect(store.browser.said["chrome"] == "Remove the extension on Chrome’s own extensions page too.")

        board["POST /api/browsers/setup"] = .json(500, refusal("failed", "Chrome's folder could not be written to."))
        await store.change(browser: chrome, setUp: true)
        #expect(store.browser.refusal?.sentence == "Chrome's folder could not be written to.")
        // Reading the list again leaves the reason where the owner can read it.
        await store.loadBrowsers()
        #expect(store.browser.refusal?.sentence == "Chrome's folder could not be written to.")
        #expect(store.browser.problem == nil)
    }

    @Test("A file too large or empty is turned away before any of it is sent")
    func refusedLocally() async throws {
        let small = #"{"api": 1, "computer": {"kobo": "0.3.37", "upload_limit": 10}, "readers": []}"#
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, small)]))
        defer { stage.close() }
        let store = companion(stage, place: .send)
        await store.start()
        await store.offer(file: try file("Everything.cbz", bytes: 11))
        #expect(store.send.file == .idle(problem: "Everything.cbz is 11 bytes. The most this window sends is 10 bytes; use kobo send in a terminal for larger files."))
        await store.offer(file: try file("empty.md", bytes: 0))
        #expect(store.send.file == .idle(problem: "empty.md is empty, so there is nothing to send."))
        await store.offer(file: FileManager.default.temporaryDirectory)
        if case .idle(let problem) = store.send.file { #expect(problem.hasSuffix("is a folder. Send the files in it one at a time.")) } else { Issue.record("a folder is turned away") }
        await store.offer(file: URL(fileURLWithPath: "/nonexistent/cobalt-test/Gone.pdf"))
        #expect(store.send.file == .idle(problem: "Gone.pdf could not be read. Check that it is still where it was."))
        #expect(stage.asked.map(\.path) == ["/api/state"])
    }

    @Test("A file the reader has no app for, or the server finds too large, is refused in the server's words")
    func refusedByServer() async throws {
        for (kind, status, sentence) in [("unsupported_file", 422, "No app on the reader takes .xyz files."), ("too_large", 413, "That file is larger than this window sends.")] {
            var routes = base
            routes["POST /api/send/file"] = .json(status, refusal(kind, sentence))
            let stage = StandIn.stage(StandIn.routes(routes))
            defer { stage.close() }
            let store = companion(stage, place: .send)
            await store.start()
            await store.offer(file: try file("Thing.xyz", bytes: 5))
            #expect(store.send.file == .idle(problem: sentence))
            #expect(store.send.job == nil)
        }
    }

    @Test("A file one app takes is sent without a question, and what the command says is shown")
    func plainFile() async throws {
        var routes = base
        routes["POST /api/send/file"] = .json(200, #"{"upload": 7, "name": "Winter.cbz", "bytes": 5, "app": {"name": "panels", "title": "Panels", "purpose": "comics", "replaces": null}}"#)
        routes["POST /api/send/start"] = .json(200, #"{"job": 8}"#)
        routes["GET /api/job"] = .json(200, jobJSON(id: 8, title: "Sending Winter.cbz", running: false, ok: "true", text: "preparing Winter.cbz for Panels\nSent Winter.cbz to Panels on kobo.\n", next: 66, kind: "send", volume: nil))
        let stage = StandIn.stage(StandIn.routes(routes))
        defer { stage.close() }
        let store = companion(stage, place: .send)
        await store.start()
        await store.offer(file: try file("Winter.cbz", bytes: 5))
        #expect(store.send.jobFor == .file)
        #expect(store.send.job?.ok == true)
        #expect(store.send.file == .idle(problem: ""))
        let start = stage.asked.first { $0.path == "/api/send/start" }!
        #expect(try JSONSerialization.jsonObject(with: start.body) as? [String: Int] == ["upload": 7])
        // The queue is read again once a send has finished.
        #expect(stage.asked.last?.path == "/api/send")
    }

    @Test("A file more than one app could take waits for the owner to choose, and one that replaces something waits to be told it may")
    func questions() async throws {
        var routes = base
        routes["POST /api/send/file"] = .json(200, #"{"upload": 7, "name": "Sonata.pdf", "bytes": 5, "choices": [{"name": "musicstand", "title": "Music Stand", "purpose": "scores", "replaces": null}, {"name": "frame", "title": "Frame", "purpose": "a photo", "replaces": "photo"}]}"#)
        routes["POST /api/send/start"] = .json(200, #"{"job": 8}"#)
        routes["GET /api/job"] = .json(200, jobJSON(id: 8, title: "Sending Sonata.pdf", running: true, kind: "send", volume: nil))
        let stage = StandIn.stage(StandIn.routes(routes))
        defer { stage.close() }
        let store = companion(stage, place: .send)
        await store.start()
        await store.offer(file: try file("Sonata.pdf", bytes: 5))
        guard case .asking(let upload, let to) = store.send.file else { Issue.record("the owner is asked"); return }
        #expect(upload.choices?.count == 2)
        #expect(to == nil)
        // Nothing is sent until something is chosen.
        await store.confirmFile()
        #expect(!stage.asked.contains { $0.path == "/api/send/start" })
        store.choose(app: "frame")
        await store.confirmFile()
        let start = stage.asked.first { $0.path == "/api/send/start" }!
        let body = try JSONSerialization.jsonObject(with: start.body) as? [String: AnyHashable]
        #expect(body == ["upload": 7, "to": "frame", "replace": true])
        #expect(store.send.job?.running == true)

        // While that is going, another file is turned away and the first is left alone.
        await store.offer(file: try file("Other.pdf", bytes: 5))
        #expect(store.send.file == .idle(problem: "Something is still being sent. Other.pdf can go when it has finished."))
        #expect(store.send.job?.id == 8)
    }

    @Test("What was typed is checked before it is sent as a web address")
    func addresses() async {
        var routes = base
        routes["POST /api/send/address"] = .json(200, #"{"job": 9}"#)
        routes["GET /api/job"] = .json(200, jobJSON(id: 9, title: "Sending an article", running: false, ok: "true", text: "Waiting for your Kobo: Tides. Your Kobo did not answer. It may be asleep or away from Wi-Fi. It goes over with kobo clip sync, with Sync on the reader's Articles shelf, or with the next clip that finds the reader awake.\n", next: 200, kind: "send", volume: nil))
        let stage = StandIn.stage(StandIn.routes(routes))
        defer { stage.close() }
        let store = companion(stage, place: .send)
        await store.start()
        #expect(await store.send(address: "example.org") == false)
        #expect(store.send.addressProblem == "Enter a web address that starts with http:// or https://.")
        #expect(!stage.asked.contains { $0.method == "POST" })
        #expect(await store.send(address: " https://example.org/tides "))
        #expect(store.send.addressProblem == "")
        #expect(store.send.jobFor == .address)
        // A reader that is asleep is not a failure: the article waits.
        #expect(store.send.job?.ok == true)
        #expect(store.send.job?.text.hasPrefix("Waiting for your Kobo: Tides.") == true)
    }

    @Test("Sending what is waiting with no reader saved fails with the command's reason under the queue")
    func queue() async {
        var routes = base
        routes["POST /api/send/queue"] = .json(200, #"{"job": 10}"#)
        routes["GET /api/job"] = .json(200, jobJSON(id: 10, title: "Sending waiting articles", running: false, ok: "false", text: "kobo: 1 article still waiting for kobo. Your Kobo did not answer. It may be asleep or away from Wi-Fi.\n", next: 99, kind: "send", volume: nil))
        let stage = StandIn.stage(StandIn.routes(routes))
        defer { stage.close() }
        let store = companion(stage, place: .send)
        await store.start()
        await store.loadSend()
        #expect(store.send.info?.waiting.map(\.title) == ["Tides"])
        await store.sendQueue()
        #expect(store.send.jobFor == .queue)
        #expect(store.send.job?.failed == true)
        #expect(Said.failure(store.send.job!.text).hasPrefix("1 article still waiting for kobo."))
    }

    @Test("A window opened while things are being sent picks each up in the part of the place it belongs to")
    func resumes() async {
        let titles: [(String, SendPart)] = [("Sending Winter.cbz", .file), ("Sending an article", .address), ("Sending waiting articles", .queue)]
        for (title, part) in titles {
            let stage = StandIn.stage(StandIn.routes([
                "GET /api/state": .json(200, stateJSON(jobs: #"{"id": 2, "kind": "send", "title": "\#(title)", "running": true, "ok": null}, {"id": 3, "kind": "reader", "title": "Installing apps", "running": true, "ok": null, "volume": "/Volumes/KOBOeReader"}"#)),
            ]))
            defer { stage.close() }
            let store = companion(stage)
            await store.start()
            #expect(store.send.jobFor == part)
            #expect(store.send.job?.id == 2)
            // An app install shares the reader's queue and is shown in Apps.
            #expect(store.apps.job?.id == 3)
            #expect(store.reader.job == nil)
        }
    }

    @Test("A file part way up is the one thing quitting really loses, and the question says so")
    func quittingMidUpload() async throws {
        var routes = base
        routes["POST /api/send/file"] = .silence
        let stage = StandIn.stage(StandIn.routes(routes))
        defer { stage.close() }
        let store = Companion(place: .send) { .success(stage.client(patience: Patience(everything: 1))) }
        await store.start()
        let url = try file("Winter.cbz", bytes: 5)
        let sending = Task { await store.offer(file: url) }
        for _ in 0..<50 where store.quitQuestion == nil { try await Task.sleep(nanoseconds: 10_000_000) }
        #expect(store.quitQuestion?.title == "Winter.cbz has not all been handed over.")
        #expect(store.quitQuestion?.detail == "Quitting now stops it and nothing is sent.")
        await sending.value
        // The upload timed out, which is said where the file was offered.
        #expect(store.send.file == .idle(problem: "The kobo command did not answer in time."))
        #expect(store.quitQuestion == nil)
    }

    @Test("macOS keeping the reader or the network away is put into words that name the right switch")
    func privacy() {
        let own = Privacy.blocked(volume: "/Volumes/KOBOeReader", origin: .finder)
        #expect(own.headline == "macOS is keeping Cobalt from reading this reader.")
        #expect(own.detail == "In System Settings, go to Privacy & Security, then Files and Folders, and turn on Removable Volumes for Cobalt.")
        let terminal = Privacy.blocked(volume: "/Volumes/KOBOeReader", origin: .terminal)
        #expect(terminal.detail.contains("turn on Removable Volumes for that terminal"))
        #expect(Privacy.looksKeptOffNetwork("kobo: could not reach 192.168.1.20: No route to host (os error 65)"))
        #expect(!Privacy.looksKeptOffNetwork("kobo: No reader is saved yet."))
        #expect(Privacy.localNetwork(origin: .finder).detail.contains("Local Network"))
    }
}

@Suite("Where an install has got to")
struct InstallStageTests {
    func job(_ title: String, said: String) -> FollowedJob {
        var job = FollowedJob(id: 3, kind: .reader, title: title, volume: "/Volumes/KOBOeReader", began: Date())
        job.text = said
        return job
    }

    func states(_ said: String, title: String = "Installing Cobalt") -> [JobScene.Stage.State] {
        JobScene.scene(for: job(title, said: said), state: nil, now: Date()).stages.map(\.state)
    }

    @Test("An install is four things that happen in order, and the list shows which one is happening")
    func advances() {
        #expect(states("") == [.current, .waiting, .waiting, .waiting])
        let ready = String(setupSaid[..<setupSaid.range(of: "Set up /Volumes")!.lowerBound])
        #expect(states(ready) == [.done, .current, .waiting, .waiting])
        let written = String(setupSaid[..<setupSaid.range(of: "  · volume ejected")!.lowerBound])
        #expect(states(written) == [.done, .done, .current, .waiting])
        let ejected = String(setupSaid[..<setupSaid.range(of: "The reader is back")!.lowerBound])
        #expect(states(ejected) == [.done, .done, .done, .current])
        #expect(states(setupSaid) == [.done, .done, .done, .done])
        #expect(JobScene.scene(for: job("Updating Cobalt", said: ""), state: nil, now: Date()).stages.map(\.title) == [
            "Reading the reader", "Writing Cobalt to the reader", "Ejecting the reader", "Waiting for the reader on Wi-Fi",
        ])
    }

    @Test("A job that is not an install has no such list, and everything any job said is kept to be opened")
    func onlyInstalls() {
        #expect(states("anything", title: "Removing Cobalt").isEmpty)
        #expect(states("anything", title: "Installing apps").isEmpty)
        let scene = JobScene.scene(for: job("Installing Cobalt", said: setupSaid), state: nil, now: Date())
        #expect(scene.said == Said.reflow(setupSaid))
        #expect(scene.turned)
        #expect(JobScene.scene(for: job("Installing Cobalt", said: "found Kobo Clara BW\n"), state: nil, now: Date()).turned == false)
    }
}
