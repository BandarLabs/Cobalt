import Foundation
import Testing
@testable import CobaltKit

/// A store of three apps in two groups, as the server would send it.
let storeJSON = """
{"groups": [{"id": "reading", "title": "Reading and listening", "apps": ["rss", "vault"]},
            {"id": "games", "title": "Games", "apps": ["tide-table"]}],
 "apps": {
   "rss": {"id": "rss", "title": "Feeds", "summary": "Follow sites.", "description": "Follow sites and read their articles as pages, with pictures kept for reading offline.",
           "version": "1.4.0", "release_notes": "Pictures are kept.", "minimum_cobalt_version": "0.3.30", "access": ["Use the network."],
           "needs": [], "setup": [{"text": "Set up the reader once over USB.", "command": "kobo setup", "link": null}],
           "shots": [{"name": "feeds.png", "caption": "The list of sites", "width": 1072, "height": 1448},
                     {"name": "rss/article.png", "caption": "An article", "width": 1072, "height": 1448}]},
   "vault": {"id": "vault", "title": "Vault", "summary": "Keep notes.", "version": "1.1.0", "shots": []},
   "tide-table": {"id": "tide-table", "title": "Tide Table", "summary": "High and low water.", "version": "1.0.0",
                  "shots": [{"name": "tide-table.png", "caption": "A month of tides"}]}
 }}
"""

@Suite("The store")
struct StoreTests {
    let store: Store = decoded(storeJSON)

    @Test("The store is read whole, with ids as they are written and what is missing left empty")
    func decodes() {
        #expect(store.groups.map(\.title) == ["Reading and listening", "Games"])
        #expect(Set(store.apps.keys) == ["rss", "vault", "tide-table"])
        #expect(store.apps["rss"]?.setup == [SetupStep(text: "Set up the reader once over USB.", command: "kobo setup", link: nil)])
        #expect(store.apps["rss"]?.shots.map(\.name) == ["feeds.png", "rss/article.png"])
        #expect(store.apps["vault"]?.shots.isEmpty == true)
        #expect(store.apps["vault"]?.description == "")
        // A picture the server gives no size for is taken to be a reader's screen.
        #expect(store.apps["tide-table"]?.shots.first?.width == 1072)
        #expect(store.apps["tide-table"]?.shots.first?.height == 1448)
        // The store says nothing about which Cobalt an app needs; the catalog does.
        #expect(store.apps["rss"]?.needs.isEmpty == true)
        let real: Store = decoded(#"{"groups": [{"id": "other", "title": "Other apps", "apps": ["chat"]}], "apps": {"chat": {"id": "chat", "title": "Chat", "summary": "Ask a question.", "description": "Ask a question.", "version": "1.0.0", "release_notes": null, "access": ["Connects to online services"], "setup": [], "shots": []}}}"#)
        #expect(real.apps["chat"]?.releaseNotes == "")
        #expect(real.apps["chat"]?.minimumCobaltVersion == nil)
        #expect(real.groups.map(\.title) == ["Other apps"])
    }

    @Test("With no reader the whole store is there to read, in its groups, and nothing is on offer")
    func withoutAReader() {
        let front = Storefront.arrange(store: store, installed: nil, catalog: nil, query: "")
        #expect(front.onReader.isEmpty)
        #expect(front.sections.map(\.title) == ["Reading and listening", "Games"])
        #expect(front.sections[0].entries.map(\.id) == ["rss", "vault"])
        #expect(front.entry("rss")?.offered == false)
        #expect(front.total == 3)
        let page = front.entry("rss")
        #expect(StoreWords.obstacle(to: page!, reader: nil, hasCatalog: false) == "Installing needs a reader plugged in.")
        #expect(StoreWords.obstacle(to: page!, reader: reader(), hasCatalog: false) == "Installing needs Cobalt on the reader.")
    }

    @Test("What the reader holds comes first, each app shown once, with the store's pictures and the catalog's facts")
    func joinedById() {
        let installed = [app("rss", "Feeds", #""installed_version": "1.3.0", "version": "1.3.0", "summary": "The reader's own line.""#)]
        let catalog = [
            app("rss", "Feeds", #""installed_version": "1.3.0", "version": "1.4.0", "update_available": true, "summary": "Follow sites and read their articles as pages.", "bytes": 1900412"#),
            app("vault", "Vault", #""version": "1.2.0", "bytes": 700000"#),
            app("atlas", "Atlas", #""summary": "Offline maps.", "bytes": 412000000, "fits": false"#),
        ]
        let front = Storefront.arrange(store: store, installed: installed, catalog: catalog, query: "")
        #expect(front.onReader.map(\.id) == ["rss"])
        #expect(front.updates == ["rss"])
        let feeds = front.onReader[0]
        #expect(feeds.shots.first?.name == "feeds.png")
        #expect(feeds.summary == "Follow sites and read their articles as pages.")
        #expect(feeds.version == "1.4.0")
        #expect(feeds.installedVersion == "1.3.0")
        #expect(feeds.updateAvailable)
        // The groups keep the store's order and leave out what is already above.
        #expect(front.sections.map(\.title) == ["Reading and listening", "Games", "More"])
        #expect(front.sections[0].entries.map(\.id) == ["vault"])
        // The catalog is newer than the kobo command and its version is the one on offer.
        #expect(front.entry("vault")?.version == "1.2.0")
        #expect(front.entry("vault")?.offered == true)
        // Known to the store, not offered to this reader: readable, not installable.
        #expect(front.entry("tide-table")?.offered == false)
        #expect(StoreWords.obstacle(to: front.entry("tide-table")!, reader: readerWithCobalt("0.3.37", "current"), hasCatalog: true) == "The catalog for this reader does not offer it.")
        // Offered by the catalog and unknown to the store: listed apart, with no pictures.
        let atlas = front.sections[2].entries[0]
        #expect(atlas.id == "atlas")
        #expect(atlas.shots.isEmpty)
        #expect(atlas.summary == "Offline maps.")
        #expect(StoreWords.obstacle(to: atlas, reader: readerWithCobalt("0.3.37", "current", #""free_bytes": 310000000"#), hasCatalog: true) == "Needs 412 MB; the reader has 310 MB free.")
        #expect(front.total == 4)
    }

    @Test("An app or an update that needs a newer Cobalt says which, and one that is simply in order says nothing")
    func obstacles() {
        let onReader = readerWithCobalt("0.3.37", "current")
        let catalog = [
            app("rss", "Feeds", #""installed_version": "1.3.0", "version": "2.0.0", "update_available": true, "runs_on_this_reader": false, "minimum_cobalt_version": "0.4.0""#),
            app("vault", "Vault", #""runs_on_this_reader": false, "minimum_cobalt_version": "0.4.0""#),
            app("tide-table", "Tide Table"),
        ]
        let front = Storefront.arrange(store: store, installed: [app("rss", "Feeds", #""installed_version": "1.3.0""#)], catalog: catalog, query: "")
        #expect(StoreWords.obstacle(to: front.entry("rss")!, reader: onReader, hasCatalog: true) == "Version 2.0.0 needs Cobalt 0.4.0 or newer.")
        #expect(StoreWords.obstacle(to: front.entry("vault")!, reader: onReader, hasCatalog: true) == "Needs Cobalt 0.4.0 or newer.")
        #expect(StoreWords.obstacle(to: front.entry("tide-table")!, reader: onReader, hasCatalog: true) == "")
        // An update that cannot run is not counted among the updates.
        #expect(front.updates.isEmpty)
    }

    @Test("The search looks at names and descriptions, and a kobo with no store still lists what the catalog offers")
    func searchingAndFallingBack() {
        let front = Storefront.arrange(store: store, installed: nil, catalog: nil, query: "OFFLINE")
        #expect(front.sections.flatMap(\.entries).map(\.id) == ["rss"])
        #expect(front.total == 3)
        #expect(Storefront.arrange(store: store, installed: nil, catalog: nil, query: "spreadsheet").isEmpty)

        let bare = Storefront.arrange(store: nil, installed: [app("rss", "Feeds", #""installed_version": "1.3.0""#)], catalog: [app("rss", "Feeds", #""installed_version": "1.3.0""#), app("vault", "Vault")], query: "")
        #expect(bare.onReader.map(\.id) == ["rss"])
        #expect(bare.sections.map(\.title) == ["Available"])
        #expect(bare.sections[0].entries.map(\.id) == ["vault"])
    }
}

@MainActor
@Suite("The store and its pictures")
struct StorePicturesTests {
    nonisolated static let png = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3])

    @Test("A picture comes back as its bytes, and one that is not listed or cannot be fetched is refused as that")
    func pictures() async throws {
        let stage = StandIn.stage { asked in
            switch asked.query["name"] {
            case "rss/article.png": return .bytes(200, StorePicturesTests.png, type: "image/png")
            case "feeds.png": return .json(502, refusal("unreachable", "The picture could not be fetched."))
            default: return .json(404, refusal("not_found", "The store lists no picture of that name."))
            }
        }
        defer { stage.close() }
        let client = stage.client()
        #expect(try await client.shot(named: "rss/article.png") == StorePicturesTests.png)
        // The name is asked for exactly as the store lists it.
        #expect(stage.asked[0].path == "/api/shot")
        #expect(stage.asked[0].query == ["name": "rss/article.png"])
        await #expect(throws: Failure.refused(kind: .unreachable, sentence: "The picture could not be fetched.", status: 502)) { _ = try await client.shot(named: "feeds.png") }
        await #expect(throws: Failure.refused(kind: .notFound, sentence: "The store lists no picture of that name.", status: 404)) { _ = try await client.shot(named: "nope.png") }

        let slow = StandIn.stage { _ in .silence }
        defer { slow.close() }
        var patience = Patience()
        patience.picture = 0.3
        await #expect(throws: Failure.timedOut) { _ = try await slow.client(patience: patience).shot(named: "feeds.png") }
    }

    @Test("The store is read with no reader, a picture is fetched once, and one that failed waits for its page to be opened again")
    func fetchedOnceAndRetriedOnOpening() async throws {
        let board = Board(["GET /api/state": .json(200, stateJSON()), "GET /api/store": .json(200, storeJSON), "GET /api/shot": .json(502, refusal("unreachable", "The picture could not be fetched."))])
        let stage = StandIn.stage(board.script)
        defer { stage.close() }
        let store = companion(stage, place: .apps)
        await store.start()
        await store.loadStore()
        #expect(store.storefront.sections.map(\.title) == ["Reading and listening", "Games"])

        await store.loadShot("feeds.png")
        #expect(store.store.shots["feeds.png"] == .failed)
        // Asked for again by a grid that is redrawn, it is not fetched again.
        store.wantShot("feeds.png")
        try await Task.sleep(nanoseconds: 150_000_000)
        #expect(stage.asked.filter { $0.path == "/api/shot" }.count == 1)

        // Opening the app's page is the owner asking, and it is tried once more.
        board["GET /api/shot"] = .bytes(200, StorePicturesTests.png, type: "image/png")
        store.open(app: "rss")
        #expect(store.store.open == "rss")
        #expect(store.store.shots["feeds.png"] == nil)
        store.wantShot("feeds.png")
        for _ in 0..<50 where store.store.shots["feeds.png"] == .loading { try await Task.sleep(nanoseconds: 10_000_000) }
        #expect(store.store.shots["feeds.png"] == .loaded(StorePicturesTests.png))
        // Once had, it is kept for the session.
        store.wantShot("feeds.png")
        await store.loadShot("feeds.png")
        #expect(stage.asked.filter { $0.path == "/api/shot" }.count == 2)
        store.closeApp()
        #expect(store.store.open == nil)

        // Showing the store again asks once more for whatever failed, and for nothing that did not.
        board["GET /api/shot"] = .json(502, refusal("unreachable", "The picture could not be fetched."))
        await store.loadShot("rss/article.png")
        #expect(store.store.shots["rss/article.png"] == .failed)
        store.forgetFailedShots()
        #expect(store.store.shots["rss/article.png"] == nil)
        #expect(store.store.shots["feeds.png"] == .loaded(StorePicturesTests.png))
    }

    @Test("A grid of pictures is asked for a few at a time")
    func aFewAtATime() async throws {
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, stateJSON()), "GET /api/shot": .silence]))
        defer { stage.close() }
        let store = Companion(place: .apps) { .success(stage.client(patience: Patience(everything: 0.6))) }
        await store.start()
        for name in ["a.png", "b.png", "c.png", "d.png", "e.png"] { store.wantShot(name) }
        try await Task.sleep(nanoseconds: 200_000_000)
        #expect(stage.asked.filter { $0.path == "/api/shot" }.count == 3)
        #expect(store.store.shots.values.allSatisfy { $0 == .loading })
        // When those give up, the rest have their turn, and none waits for ever.
        try await Task.sleep(nanoseconds: 1_600_000_000)
        #expect(stage.asked.filter { $0.path == "/api/shot" }.count == 5)
        #expect(store.store.shots.values.allSatisfy { $0 == .failed })
    }

    @Test("A kobo command too old to have a store says so and the place carries on with what the catalog offers")
    func withoutAStore() async {
        let stage = StandIn.stage(StandIn.routes(["GET /api/state": .json(200, stateJSON())]))
        defer { stage.close() }
        let store = companion(stage, place: .apps)
        await store.start()
        await store.loadStore()
        #expect(store.store.store == nil)
        #expect(store.store.problem?.kind == .notFound)
        #expect(store.link == .connected)
    }

    @Test("The arrows step through an app's pictures one at a time and stop at either end")
    func galleryStepping() {
        #expect(StoreWords.galleryStep(from: 0, by: 1, count: 4) == 1)
        #expect(StoreWords.galleryStep(from: 3, by: 1, count: 4) == 3)
        #expect(StoreWords.galleryStep(from: 0, by: -1, count: 4) == 0)
        #expect(StoreWords.galleryStep(from: 2, by: -1, count: 4) == 1)
        // A row of one, or of none, has nowhere to go.
        #expect(StoreWords.galleryStep(from: 0, by: 1, count: 1) == 0)
        #expect(StoreWords.galleryStep(from: 0, by: 1, count: 0) == 0)
    }
}
