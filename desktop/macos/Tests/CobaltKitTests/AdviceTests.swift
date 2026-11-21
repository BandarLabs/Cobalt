import Foundation
import Testing
@testable import CobaltKit

/// Builds a value by the same decoding the client uses, so an example in a
/// test is an answer the server could have given.
func decoded<Answer: Decodable>(_ json: String) -> Answer {
    do {
        return try Wire.decode(Answer.self, from: Data(json.utf8))
    } catch {
        fatalError("the example is not an answer the server could give: \(error)")
    }
}

func reader(_ more: String = "", volume: String = "/Volumes/KOBOeReader", model: String = "Kobo Clara BW", supported: Bool = true, standing: String = "absent") -> Reader {
    decoded("""
    {"volume": "\(volume)", "model": "\(model)", "firmware": "4.45.23697", "supported": \(supported), "standing": "\(standing)"\(more.isEmpty ? "" : ", " + more)}
    """)
}

func readerWithCobalt(_ version: String, _ standing: String, _ more: String = "", supported: Bool = true) -> Reader {
    reader("\"cobalt\": {\"cobalt_version\": \"\(version)\", \"channel\": \"stable\"}" + (more.isEmpty ? "" : ", " + more), supported: supported, standing: standing)
}

let withRelease: Computer = decoded(#"{"kobo": "0.3.37", "release": "0.3.37", "release_problem": null}"#)
let withoutRelease: Computer = decoded(#"{"kobo": "0.3.37", "release": null, "release_problem": "This copy of the kobo command has nothing to put on a reader."}"#)

func state(_ body: String) -> ServerState {
    decoded(#"{"api": 1, "computer": {"kobo": "0.3.37", "release": "0.3.37"}, "#  + body + "}")
}

@Suite("What the Reader place says")
struct ReaderAdviceTests {
    let clara = reader()
    let libra = reader(volume: "/Volumes/KOBOeReader 1", model: "Kobo Libra 2")

    @Test("One reader needs no choosing and two are never chosen between for the owner")
    func choosing() {
        #expect(Advice.chosen(among: [], volume: nil) == .none)
        #expect(Advice.chosen(among: [clara], volume: nil) == .one(clara, among: []))
        #expect(Advice.chosen(among: [clara, libra], volume: nil) == .several([clara, libra]))
        #expect(Advice.chosen(among: [clara, libra], volume: libra.volume) == .one(libra, among: [clara, libra]))
        // The chosen one was unplugged: back to asking, not on to the other.
        #expect(Advice.chosen(among: [clara, libra], volume: "/Volumes/Gone") == .several([clara, libra]))
        // A choice made among two is not held against the one that is left.
        #expect(Advice.chosen(among: [clara], volume: libra.volume) == .one(clara, among: []))
    }

    @Test("A reader without Cobalt is offered an install only when there is one to give it")
    func withoutCobalt() {
        #expect(Advice.standing(of: clara, on: withRelease) == ReaderAdvice(reason: "", primary: .install, repair: false, removable: false))
        #expect(Advice.standing(of: clara, on: withoutRelease) == ReaderAdvice(reason: withoutRelease.releaseProblem ?? "", primary: nil, repair: false, removable: false))
    }

    @Test("A reader Cobalt cannot start on is told why and offered nothing")
    func refusedReader() {
        let refused = Advice.standing(
            of: reader(#""refusal": "firmware 5.2.190625 cannot start Cobalt: NickelMenu does not hook the 5.x firmware""#, supported: false),
            on: withRelease
        )
        #expect(refused.reason == "Cobalt cannot be installed on this reader. Firmware 5.2.190625 cannot start Cobalt: NickelMenu does not hook the 5.x firmware.")
        #expect(refused.primary == nil)
        #expect(refused.repair == false)
        #expect(refused.removable == false)
    }

    @Test("A reader with Cobalt is offered an update only when this computer's release is newer")
    func withCobalt() {
        #expect(Advice.standing(of: readerWithCobalt("0.3.30", "behind"), on: withRelease) == ReaderAdvice(reason: "", primary: .update, repair: false, removable: true))
        // In order, nothing is said: the facts already show both releases.
        #expect(Advice.standing(of: readerWithCobalt("0.3.37", "current"), on: withRelease) == ReaderAdvice(reason: "", primary: nil, repair: true, removable: true))

        let ahead = Advice.standing(of: readerWithCobalt("0.4.0", "ahead"), on: withRelease)
        #expect(ahead.primary == nil)
        #expect(ahead.repair == false)
        #expect(ahead.reason.contains("newer than the 0.3.37 on this computer"))
        #expect(ahead.reason.contains("kobo update"))

        let unknown = Advice.standing(of: readerWithCobalt("0.3.30", "unknown"), on: withoutRelease)
        #expect(unknown.primary == nil)
        #expect(unknown.reason == withoutRelease.releaseProblem)
        #expect(unknown.removable)
    }

    @Test("Cobalt can always be taken off, even from a reader it can no longer be put on")
    func alwaysRemovable() {
        let stranded = Advice.standing(of: readerWithCobalt("0.3.30", "behind", #""refusal": "firmware 5.2 cannot start Cobalt""#, supported: false), on: withRelease)
        #expect(stranded.primary == nil)
        #expect(stranded.removable)
        #expect(stranded.reason.hasPrefix("Cobalt cannot be installed on this reader. Firmware 5.2"))
        let damaged = Advice.standing(of: reader(#""cobalt_problem": "The Cobalt on the reader does not say which release it is.""#), on: withRelease)
        #expect(damaged.reason == "The Cobalt on the reader does not say which release it is.")
        #expect(damaged.primary == .install)
        #expect(damaged.removable)
    }

    @Test("A standing this build has not heard of is treated as one it cannot compare")
    func unheardOfStanding() {
        #expect(readerWithCobalt("0.3.30", "sideways").standing == .unknown)
    }

    @Test("No reader showing is told apart: nothing plugged in, Connect not tapped, or the system in the way")
    func absences() {
        #expect(Advice.absence(nil) == .none)
        #expect(Advice.absence(state(#""readers": []"#)) == .none)
        #expect(Advice.absence(state(#""attached": 0, "readers": []"#)) == .none)
        #expect(Advice.absence(state(#""attached": 1, "readers": []"#)) == .unconnected)
        #expect(Advice.absence(state(#""attached": 1, "blocked": ["/Volumes/KOBOeReader"], "readers": []"#)) == .blocked(volume: "/Volumes/KOBOeReader"))
        #expect(Advice.absence(state(#""attached": 2, "readers": [{"volume": "/Volumes/KOBOeReader", "model": "Kobo Clara BW", "supported": true, "standing": "absent"}]"#)) == .present)
    }

    @Test("After a job that did not finish the owner is told what the reader has now")
    func afterFailure() {
        let gone = state(#""readers": []"#)
        #expect(Advice.afterFailure(volume: "/Volumes/KOBOeReader", state: gone).hasPrefix("The reader is no longer connected, so what is on it cannot be read."))
        let bare = state(#""readers": [{"volume": "/Volumes/KOBOeReader", "model": "Kobo Clara BW", "supported": true, "standing": "absent"}]"#)
        #expect(Advice.afterFailure(volume: "/Volumes/KOBOeReader", state: bare) == "The reader is still connected and has no Cobalt on it.")
        let kept = state(#""readers": [{"volume": "/Volumes/KOBOeReader", "model": "Kobo Clara BW", "supported": true, "standing": "behind", "cobalt": {"cobalt_version": "0.3.30", "channel": "stable"}}]"#)
        #expect(Advice.afterFailure(volume: "/Volumes/KOBOeReader", state: kept) == "The reader is still connected and has Cobalt 0.3.30 on it.")
        let damaged = state(#""readers": [{"volume": "/Volumes/KOBOeReader", "model": "Kobo Clara BW", "supported": true, "standing": "unknown", "cobalt_problem": "It does not say which release it is."}]"#)
        #expect(Advice.afterFailure(volume: "/Volumes/KOBOeReader", state: damaged) == "The reader is still connected. The Cobalt on it is damaged; installing again replaces it.")
        // A job that named no reader has nothing to say about one.
        #expect(Advice.afterFailure(volume: nil, state: gone) == "")
    }
}

/// An app as the catalog would describe it, with the fields in `more`
/// laid over the usual ones the way the page's own examples do.
func app(_ id: String, _ title: String, _ more: String = "") -> AppEntry {
    var fields: [String: Any] = [
        "id": id, "title": title, "summary": "The \(title) app.", "version": "1.0.0", "installed_version": NSNull(),
        "update_available": false, "runs_on_this_reader": true, "minimum_cobalt_version": "0.3.0",
    ]
    if !more.isEmpty, let extra = try? JSONSerialization.jsonObject(with: Data("{\(more)}".utf8)) as? [String: Any] {
        fields.merge(extra) { _, new in new }
    }
    do {
        return try Wire.decode(AppEntry.self, from: JSONSerialization.data(withJSONObject: fields))
    } catch {
        fatalError("the example is not an app the server could describe: \(error)")
    }
}

@Suite("What the Apps place lists")
struct ShelfTests {
    @Test("The reader's apps come first, with what the catalog knows about each, then the rest")
    func installedThenAvailable() {
        let installed = [app("rss", "Feeds", #""installed_version": "1.0.0""#), app("almanac", "Almanac", #""installed_version": "1.0.0""#)]
        let catalog = [
            app("almanac", "Almanac", #""version": "1.2.0", "installed_version": "1.0.0", "update_available": true"#),
            app("rss", "Feeds", #""installed_version": "1.0.0""#),
            app("tide-table", "Tide Table"),
            app("backgammon", "Backgammon"),
        ]
        let shelf = Shelf.arrange(installed: installed, catalog: catalog, query: "")
        #expect(shelf.installed.map(\.id) == ["almanac", "rss"])
        #expect(shelf.available.map(\.id) == ["backgammon", "tide-table"])
        #expect(shelf.updates == ["almanac"])
        #expect(shelf.total == 4)
        #expect(shelf.installed[0].app.version == "1.2.0")
        #expect(shelf.installed[0].app.installedVersion == "1.0.0")
        #expect(shelf.installed[0].app.updateAvailable)
        #expect(shelf.installed[1].app.updateAvailable == false)
    }

    @Test("What an app is for is said in the catalog's words where the catalog has them")
    func catalogSummaryIsPreferred() {
        let installed = [app("rss", "Feeds", #""installed_version": "1.0.0", "summary": "The reader's own words for it.""#), app("vault", "Vault", #""installed_version": "1.0.0", "summary": "Only the reader knows this one.""#)]
        let catalog = [app("rss", "Feeds", #""installed_version": "1.0.0", "summary": "The sites you follow, read as pages.""#)]
        let shelf = Shelf.arrange(installed: installed, catalog: catalog, query: "")
        #expect(shelf.installed.first { $0.id == "rss" }?.app.summary == "The sites you follow, read as pages.")
        #expect(shelf.installed.first { $0.id == "vault" }?.app.summary == "Only the reader knows this one.")
        // The search looks at the words that are shown.
        #expect(Shelf.arrange(installed: installed, catalog: catalog, query: "sites").installed.map(\.id) == ["rss"])
    }

    @Test("With no catalog the reader's own apps are still listed and nothing is called an update")
    func withoutCatalog() {
        let installed = [app("rss", "Feeds", #""installed_version": "1.0.0", "update_available": true"#)]
        let shelf = Shelf.arrange(installed: installed, catalog: nil, query: "")
        #expect(shelf.installed.map(\.id) == ["rss"])
        #expect(shelf.installed[0].app.updateAvailable == false)
        #expect(shelf.installed[0].inCatalog == false)
        #expect(shelf.available.isEmpty)
        #expect(shelf.updates.isEmpty)
        #expect(Shelf.arrange(installed: nil, catalog: nil, query: "") == Shelf(installed: [], available: [], updates: [], total: 0))
    }

    @Test("An update the reader's Cobalt is too old for is not counted among the updates")
    func updateTooNew() {
        let installed = [app("almanac", "Almanac", #""installed_version": "1.0.0""#)]
        let catalog = [app("almanac", "Almanac", #""version": "2.0.0", "update_available": true, "runs_on_this_reader": false, "minimum_cobalt_version": "0.4.0""#)]
        let shelf = Shelf.arrange(installed: installed, catalog: catalog, query: "")
        #expect(shelf.updates.isEmpty)
        #expect(shelf.installed[0].app.runsOnThisReader == false)
        #expect(shelf.installed[0].app.minimumCobaltVersion == "0.4.0")
    }

    @Test("The search narrows both lists by name, id or summary and leaves the counts alone")
    func searching() {
        let installed = [app("rss", "Feeds", #""installed_version": "1.0.0""#)]
        let catalog = [app("rss", "Feeds", #""installed_version": "1.0.0""#), app("tide-table", "Tide Table"), app("backgammon", "Backgammon", #""summary": "A board game.""#)]
        #expect(Shelf.arrange(installed: installed, catalog: catalog, query: "TIDE").available.map(\.id) == ["tide-table"])
        #expect(Shelf.arrange(installed: installed, catalog: catalog, query: "rss").installed.map(\.id) == ["rss"])
        #expect(Shelf.arrange(installed: installed, catalog: catalog, query: "board").available.map(\.id) == ["backgammon"])
        let none = Shelf.arrange(installed: installed, catalog: catalog, query: "zzz")
        #expect(none.installed.isEmpty && none.available.isEmpty)
        #expect(none.total == 3)
    }

    @Test("An app that cannot go on this reader as it stands says which of the two reasons it is")
    func obstacles() {
        let clara = reader(#""free_bytes": 1200000"#)
        #expect(Advice.obstacle(to: app("almanac", "Almanac"), on: clara) == "")
        #expect(Advice.obstacle(to: app("almanac", "Almanac", #""runs_on_this_reader": false, "minimum_cobalt_version": "0.4.0""#), on: clara) == "Needs Cobalt 0.4.0 or newer.")
        #expect(Advice.obstacle(to: app("atlas", "Atlas", #""bytes": 1900412, "fits": false"#), on: clara) == "Needs 1.9 MB; the reader has 1.2 MB free.")
        #expect(Advice.obstacle(to: app("atlas", "Atlas", #""bytes": 1900412, "fits": false"#), on: nil) == "Needs 1.9 MB.")
    }
}

@Suite("What the Browser and Send places say")
struct SendingTests {
    @Test("A browser's one step is split into what to do, what to pick and what to know")
    func browserSteps() {
        #expect(Advice.browserStep("Chrome: open chrome://extensions, turn on Developer mode, choose Load unpacked and pick\n  /Users/me/.config/kobo/clip/extension", title: "Chrome") == Advice.BrowserStep(
            instruction: "Open chrome://extensions, turn on Developer mode, choose Load unpacked and pick:",
            paths: ["/Users/me/.config/kobo/clip/extension"],
            notes: []
        ))
        let firefox = Advice.browserStep(
            "Firefox: open about:debugging#/runtime/this-firefox, choose Load Temporary Add-on and pick\n  /home/me/.config/kobo/clip/extension/manifest.json\n  Firefox forgets it when it restarts, so load it again after a restart.",
            title: "Firefox"
        )
        #expect(firefox.paths == ["/home/me/.config/kobo/clip/extension/manifest.json"])
        #expect(firefox.notes == ["Firefox forgets it when it restarts, so load it again after a restart."])
        let safari = Advice.browserStep(
            "Safari: right-click a link and choose Services, then Send to Kobo. For the page you are on, select its address and do the same.\n  A full Safari extension needs a signed Mac app, which this is not.",
            title: "Safari"
        )
        #expect(safari.instruction == "Right-click a link and choose Services, then Send to Kobo. For the page you are on, select its address and do the same.")
        #expect(safari.paths.isEmpty)
        #expect(safari.notes == ["A full Safari extension needs a signed Mac app, which this is not."])
        #expect(Advice.browserStep("Edge: open edge://extensions and pick\n  C:\\Users\\me\\.config\\kobo\\clip\\extension", title: "Edge").paths == ["C:\\Users\\me\\.config\\kobo\\clip\\extension"])
        #expect(Advice.browserStep("", title: "Chrome") == Advice.BrowserStep(instruction: "", paths: [], notes: []))
    }

    @Test("A size reads the way a person says it")
    func sizes() {
        #expect(Advice.fileSize(0) == "0 bytes")
        #expect(Advice.fileSize(999) == "999 bytes")
        #expect(Advice.fileSize(1500) == "1.5 KB")
        #expect(Advice.fileSize(48_300_000) == "48.3 MB")
        #expect(Advice.fileSize(536_870_912) == "537 MB")
        #expect(Advice.fileSize(3_200_000_000) == "3.2 GB")
        #expect(Advice.fileSize(-1) == "")
        #expect(Advice.fileSize(nil) == "")
    }

    @Test("A file that cannot be sent is refused before any of it is, with the reason")
    func uploadRefusals() {
        let limit: Int64 = 536_870_912
        #expect(Advice.uploadRefusal(name: "Winter.cbz", size: 48_300_000, limit: limit) == "")
        #expect(Advice.uploadRefusal(name: "Winter.cbz", size: limit, limit: limit) == "")
        #expect(Advice.uploadRefusal(name: nil, size: 0, limit: limit) == "Choose a file to send.")
        #expect(Advice.uploadRefusal(name: "empty.md", size: 0, limit: limit) == "empty.md is empty, so there is nothing to send.")
        #expect(Advice.uploadRefusal(name: "Everything.cbz", size: limit + 1, limit: limit) == "Everything.cbz is 537 MB. The most this window sends is 537 MB; use kobo send in a terminal for larger files.")
        // A server that names no limit is one the app does not second-guess.
        #expect(Advice.uploadRefusal(name: "Everything.cbz", size: limit + 1, limit: nil) == "")
    }

    @Test("Only something that starts as a web address is sent as one")
    func webAddresses() {
        #expect(Advice.webAddress("  https://example.org/a?b=c  ") == ("https://example.org/a?b=c", ""))
        #expect(Advice.webAddress("HTTP://example.org") == ("HTTP://example.org", ""))
        #expect(Advice.webAddress("") == ("", ""))
        for typed in ["example.org", "file:///etc/passwd", "--out=x", "https://a b", "javascript:alert(1)", "https://"] {
            let (address, problem) = Advice.webAddress(typed)
            #expect(address == "", "\(typed)")
            #expect(problem.contains("starts with http:// or https://"), "\(typed)")
        }
    }

    @Test("A window that opens mid-job picks up the one still running in a place and nothing that has finished")
    func runningJobs() {
        let jobs: [JobSummary] = decoded("""
        [{"id": 1, "kind": "reader", "title": "Installing Cobalt", "running": false, "ok": true},
         {"id": 2, "kind": "send", "title": "Sending Winter.cbz", "running": true, "ok": null},
         {"id": 3, "kind": "reader", "title": "Installing apps", "running": true, "ok": null}]
        """)
        #expect(Advice.runningJob(jobs, kind: .reader)?.id == 3)
        #expect(Advice.runningJob(jobs, kind: .send)?.id == 2)
        #expect(Advice.runningJob(Array(jobs.prefix(1)), kind: .reader) == nil)
        #expect(Advice.runningJob([], kind: .reader) == nil)
    }

    @Test("The window has four places, opens on Reader, and an unknown name is Reader too")
    func places() {
        #expect(Place.allCases.map(\.rawValue) == ["reader", "apps", "browser", "send"])
        #expect(Place(named: "") == .reader)
        #expect(Place(named: "apps") == .apps)
        #expect(Place(named: "send") == .send)
        #expect(Place(named: "settings") == .reader)
        #expect(Place(named: nil) == .reader)
    }
}
