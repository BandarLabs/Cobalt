import AppKit
import CobaltKit
import SwiftUI

/// Every state the window can be in, each with the answers a server would
/// have to give for the window to get there.
///
/// A scenario is a script for the stand-in server and, where the state is
/// reached by doing something, the same calls the owner's clicks make. What
/// is drawn has therefore been through the client, the decoding and the
/// record exactly as it would be live. The command output quoted here is
/// what the commands really print.
@MainActor
enum Scenarios {
    @MainActor
    struct Scenario {
        var name: String
        var place: Place = .reader
        var routes: [String: StandIn.Reply] = [:]
        var appVersion: String? = "0.3.37"
        var origin: Origin = .finder
        var shots: Shots = .all
        /// Set when the window never gets a server, and why.
        var lost: Lost?
        /// The connection is still being made when the state is drawn.
        var starting = false
        /// What the owner did to get here.
        var act: (@MainActor (Companion, Desk) async -> Void)?
        /// The same, for a state that needs the server to change its mind
        /// part way: a reader that goes once it has been ejected.
        var actOnStage: (@MainActor (Companion, StandIn.Stage) async -> Void)?
        /// The sheet this state is, when it is a question and not a place.
        var question: (@MainActor (Companion) -> NSAlert?)?

        func stage(rendering: Bool = true) async -> Staged {
            let answers = StandIn.routes(routes)
            let shots = shots
            let stage = StandIn.stage { asked in
                guard asked.path == "/api/shot" else { return answers(asked) }
                let name = asked.query["name"] ?? ""
                switch shots {
                case .none:
                    return .json(502, #"{"error": "The picture could not be fetched: this computer does not seem to be online.", "kind": "unreachable"}"#)
                case .failing(let names) where names.contains(name):
                    return .json(502, #"{"error": "The picture could not be fetched.", "kind": "unreachable"}"#)
                case .all, .failing:
                    return .bytes(200, Scenarios.picture(named: name), type: "image/png")
                }
            }
            let desk = Desk()
            desk.rendering = rendering
            let lost = lost
            let starting = starting
            // The clock stands still, so a count of seconds reads the same
            // in every image of a state.
            Scenarios.drift = 0
            let store = Companion(place: place, appVersion: appVersion, origin: origin, now: { Scenarios.noon.addingTimeInterval(Scenarios.drift) }) {
                if starting {
                    try? await Task.sleep(nanoseconds: 3_600_000_000_000)
                }
                if let lost { return .failure(lost) }
                return .success(stage.client(patience: Patience(everything: 3600)))
            }
            // A state in which the first answer never comes cannot be
            // waited for, so it is started and left to get as far as it
            // gets.
            var unanswered = starting
            if case .silence = routes["GET /api/state"] { unanswered = true }
            if unanswered {
                Task { await store.start() }
                try? await Task.sleep(nanoseconds: 250_000_000)
            } else {
                await store.start()
            }
            await act?(store, desk)
            await actOnStage?(store, stage)
            return Staged(store: store, desk: desk, stage: stage)
        }
    }

    @MainActor
    struct Staged {
        var store: Companion
        var desk: Desk
        var stage: StandIn.Stage

        func close() { stage.close() }
    }

    nonisolated static let noon = Date(timeIntervalSince1970: 1_795_176_000)
    /// How long after noon it is, for the states that are about time
    /// having passed. One state is staged at a time.
    nonisolated(unsafe) static var drift: TimeInterval = 0

    /// Starts something that will not finish, and gives the record a moment
    /// to reach the state the unfinished thing leaves it in.
    private static func midway(_ work: @escaping @MainActor () async -> Void) async {
        Task { await work() }
        try? await Task.sleep(nanoseconds: 250_000_000)
    }

    // The answers.

    private static func json(_ value: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }

    private static func ok(_ value: Any) -> StandIn.Reply { .json(200, json(value)) }

    private static func refuse(_ status: Int, _ kind: String, _ sentence: String) -> StandIn.Reply {
        .json(status, json(["error": sentence, "kind": kind]))
    }

    private static let clara = "/Volumes/KOBOeReader"
    private static let libra = "/Volumes/KOBOeReader 1"
    private static let noRelease = "This copy of the kobo command was not installed with a Cobalt release, so it has nothing to put on a reader. Install it from https://bandarlabs.github.io/Cobalt/ and open this window again."

    private static func reader(
        volume: String = clara,
        model: String = "Kobo Clara BW",
        firmware: String = "4.45.23697",
        standing: String = "absent",
        cobalt: String? = nil,
        refusal: String? = nil,
        untested: String? = nil,
        problem: String? = nil,
        free: Int = 5_200_000_000,
        writable: Bool = true,
        savedAs: String? = nil
    ) -> [String: Any] {
        [
            "volume": volume, "model": model, "model_code": "N365", "firmware": firmware,
            "supported": refusal == nil, "refusal": refusal ?? NSNull(), "untested": untested ?? NSNull(),
            "standing": standing,
            "cobalt": cobalt.map { ["cobalt_version": $0, "channel": "stable"] as [String: Any] } ?? NSNull(),
            "cobalt_problem": problem ?? NSNull(), "free_bytes": free, "writable": writable,
            "saved_as": savedAs ?? NSNull(), "menu_corner": "bottom right",
        ]
    }

    private static func state(
        _ readers: [[String: Any]] = [],
        jobs: [[String: Any]] = [],
        attached: Int? = nil,
        blocked: [String] = [],
        release: String? = "0.3.37",
        api: Int = 1,
        kobo: String = "0.3.37",
        saved: [[String: Any]]? = nil,
        awaiting: Bool = false,
        ejects: Bool = true
    ) -> StandIn.Reply {
        var answer: [String: Any] = ["awaiting": awaiting]
        // A server that keeps no list of linked readers sends none, and
        // most states here are drawn as that server would have them.
        if let saved { answer["saved"] = saved }
        return ok(answer.merging([
            "api": api,
            "computer": [
                "kobo": kobo, "os": "macos", "release": release ?? NSNull(),
                "release_problem": release == nil ? noRelease : NSNull(), "ejects": ejects, "upload_limit": 536_870_912, "network": true,
            ] as [String: Any],
            "attached": attached ?? readers.count, "blocked": blocked, "readers": readers, "jobs": jobs,
        ]) { old, _ in old })
    }

    /// How far a first install has got, as the server reports it.
    private static func setup(_ stage: String, waited: Int = 0, savedAs: String? = nil, secured: Bool? = nil, notes: [String] = []) -> [String: Any] {
        ["stage": stage, "waited": waited, "limit": 300, "saved_as": savedAs ?? NSNull(), "secured": secured ?? NSNull(), "notes": notes]
    }

    /// A first install caught at one stage: found running when the window
    /// opened, which is also what resuming after the window was closed is.
    private static func path(_ name: String, _ stage: String, running: Bool, ok succeeded: Bool? = nil, readers: [[String: Any]] = [], waited: Int = 0, savedAs: String? = nil, secured: Bool? = nil, notes: [String] = [], ejects: Bool = true, text: String = setUp, after seconds: TimeInterval = 70) -> Scenario {
        let progress = setup(stage, waited: waited, savedAs: savedAs, secured: secured, notes: notes)
        var summary = Scenarios.running(3, "Installing Cobalt")
        summary["setup"] = progress
        var report: [String: Any] = [
            "id": 3, "kind": "reader", "title": "Installing Cobalt", "running": running, "ok": succeeded ?? NSNull(), "code": NSNull(),
            "text": text, "next": text.utf8.count, "volume": clara, "setup": progress,
        ]
        if !running, succeeded == nil { report["ok"] = true }
        return Scenario(
            name: name,
            routes: ["GET /api/state": state(readers, jobs: [summary], saved: [], ejects: ejects), "GET /api/job": ok(report)],
            act: { store, _ in
                await store.tick()
                drift = seconds
            }
        )
    }

    private static func job(_ id: Int, _ title: String, running: Bool, ok succeeded: Bool? = nil, text: String = "", kind: String = "reader", volume: String? = clara) -> StandIn.Reply {
        ok([
            "id": id, "kind": kind, "title": title, "running": running, "ok": succeeded ?? NSNull(),
            "code": succeeded == false ? 1 : NSNull(), "text": text, "next": text.utf8.count, "volume": volume ?? NSNull(),
        ])
    }

    private static func running(_ id: Int, _ title: String, kind: String = "reader", volume: String? = clara) -> [String: Any] {
        ["id": id, "kind": kind, "title": title, "running": true, "ok": NSNull(), "volume": volume ?? NSNull()]
    }

    private static let installPlan = ok([
        "undo": false, "release": "0.3.37", "untested": NSNull(),
        "lines": [
            "would install Cobalt into /Volumes/KOBOeReader/.adds/cobalt",
            "would enable the firmware's root SSH server by renaming .kobo/ssh-disabled",
            "would set DeveloperSettings/ForceWifiOn=true, PowerOptions/AutoSleepMinutes=90",
            "would stage this computer's public key for root, and close password login",
            "would add a Cobalt entry to the reader's own menu",
            "would copy Welcome from Cobalt.txt into the library",
            "would eject, then wait up to 5 minutes for the reader to come back on Wi-Fi",
            "nothing outside the book partition",
        ],
    ])

    private static let removePlan = ok([
        "undo": true, "release": NSNull(), "untested": NSNull(),
        "lines": [
            "would remove managed Cobalt trees under /Volumes/KOBOeReader/.adds/cobalt after moving any owner folders into a free .adds/cobalt.recovery.N directory",
            "would remove /Volumes/KOBOeReader/.adds/cobalt-launch.sh and the exact Cobalt line from .adds/nm/cobalt and .adds/nm/menu",
            "would leave any .adds/cobalt.unusable[.N] quarantine for explicit inspection",
            "would disable the firmware's SSH server by renaming .kobo/ssh-enabled back",
            "would clear DeveloperSettings/ForceWifiOn, PowerOptions/AutoSleepMinutes",
            "nothing else on the reader is touched",
        ],
    ])

    private static let found = "found Kobo Clara BW (N365, profile clara-bw-391, device code 391) at /Volumes/KOBOeReader · firmware 4.45.23697\n\nReady to install Cobalt:\n  Model: Kobo Clara BW (device code 391, profile clara-bw-391)\n  Firmware: 4.45.23697\n  Mount: /Volumes/KOBOeReader\n  Release: Cobalt 0.3.37 (stable)\n"

    private static let setUp = found + """

    Set up /Volumes/KOBOeReader:
      · 212 files installed into .adds/cobalt
      · the firmware's SSH server is enabled, and starts at the next
        restart
      · volume ejected

    Next, on the reader:

      1. After the host has safely ejected it, disconnect the USB cable.
      2. It restarts by itself: ejecting leaves the archive where the firmware
         looks, so it shows its Updating screen, takes it, and reboots. The SSH
         server starts with that boot.
      3. Join it to Wi-Fi if it is not already.
      4. Open the bottom-right Kobo menu and choose Cobalt.

    Cobalt itself is started from the Cobalt entry in the reader's own menu, or
    from .adds/cobalt-launch.sh. Starting it stops the reader and takes the
    screen; a restart always returns to the stock reader.

    """

    private static let waitingForWifi = setUp + "\nWaiting up to 5 minutes for the reader to come back on 192.168.1.0/24. Ctrl-C to stop.\n....."

    private static let installed = setUp + "\nWaiting up to 5 minutes for the reader to come back on 192.168.1.0/24. Ctrl-C to stop.\n.....\nThe reader is back on Wi-Fi at 192.168.1.20.\nSaved as kobo.\n"

    private static let removed = """
    found Kobo Clara BW (N365, profile clara-bw-391, device code 391) at /Volumes/KOBOeReader · firmware 4.45.23697

    Undone on /Volumes/KOBOeReader:
      · Cobalt removed
      · SSH disabled again (takes effect at the next restart)
      · settings restored: DeveloperSettings/ForceWifiOn,
        PowerOptions/AutoSleepMinutes
      · volume left mounted

    Owner data was preserved for manual backup or deletion in:
      .adds/cobalt.recovery.0

    A key this tool staged is taken back with the archive it was in. One the
    reader has already extracted stays in authorized_keys under root's home
    directory, which is on the root filesystem, and USB does not reach it.

    """

    private static let truncated = found + "kobo: /Users/owner/.local/share/kobo/release/cobalt-0.3.37-KoboRoot.tgz is truncated: manifest says 27699108 bytes, found 9000000\n"

    private static func entry(_ id: String, _ title: String, _ summary: String, _ version: String, installed: String? = nil, update: Bool = false, runs: Bool = true, minimum: String = "0.3.30", bytes: Int = 1_900_412, fits: Bool = true) -> [String: Any] {
        [
            "id": id, "title": title, "label": title, "summary": summary, "version": version,
            "installed_version": installed ?? NSNull(), "update_available": update, "minimum_cobalt_version": minimum,
            "runs_on_this_reader": runs, "capabilities": [] as [String], "bytes": bytes, "fits": fits,
        ]
    }

    /// One app as the store describes it. The names of the pictures are the
    /// ones the project's site publishes, so a render given that folder
    /// shows the real screens.
    private static func known(_ id: String, _ title: String, _ summary: String, _ version: String = "1.0.0", shots: [(String, String)] = [], description: String? = nil, access: [String] = [], notes: String = "", setup: [[String: Any]] = []) -> [String: Any] {
        [
            "id": id, "title": title, "summary": summary, "description": description ?? summary,
            "version": version, "release_notes": notes.isEmpty ? NSNull() : notes, "access": access, "setup": setup,
            "shots": shots.map { ["name": $0.0, "caption": $0.1, "width": 1072, "height": 1448] as [String: Any] },
        ]
    }

    private static let summaries: [String: String] = [
        "rss": "Follow sites and read their articles as pages.",
        "panels": "Read comics from CBZ files, a page or a panel at a time.",
        "vault": "Keep Markdown notes on the reader.",
        "backgammon": "Play complete solo or pass-and-play backgammon on one Kobo.",
        "crossword": "Solve a crossword with the on-screen keyboard.",
        "sudoku": "Solve sudoku puzzles with pencil marks.",
        "nonograms": "Solve picture logic puzzles.",
        "lichess": "Play chess against the computer or other people.",
        "morse": "Learn Morse code by ear and by eye.",
        "habits": "Tick off the things you mean to do every day.",
        "todo": "Keep a list of things to do.",
        "atlas": "Carry offline maps of the places you choose.",
        "homepanel": "See and switch the things in your home.",
        "syncthing": "Keep a folder on the reader in step with your computer.",
        "terminal": "Run a shell on the reader.",
        "gallery": "See every interface component on real hardware.",
        "tide-table": "See high and low water for a harbour, a month ahead.",
    ]

    private static let longDescription = """
    Solve a full-size crossword on the reader. The grid fills the top of the screen and the clue for the square you are on sits under it; tap a square to move, tap it again to turn from across to down.

    Letters go in with an on-screen keyboard that takes the bottom third of the screen and leaves the grid where it was, so nothing jumps when you start typing. A wrong letter is not marked until you ask, and asking is a choice in the menu and not a button beside the grid, so it cannot be pressed by accident.

    Puzzles are plain files in the format most free crosswords are published in. Send one from your computer and it appears in the list; the reader keeps your place in each, and a finished puzzle stays finished. Nothing is fetched from the network, and the app works the same on a train as at home.

    A puzzle with a rebus square, a circled square or a shaded one is drawn as its setter meant it. Diagramless puzzles are not supported, and a puzzle larger than twenty-three squares a side is refused with a sentence that says so, because the squares would be too small to tap.
    """

    private static let storeAnswer = ok([
        "groups": [
            ["id": "reading", "title": "Reading and listening", "apps": ["rss", "panels", "vault"]],
            ["id": "games", "title": "Games", "apps": ["backgammon", "lichess", "crossword", "morse", "sudoku", "nonograms"]],
            ["id": "everyday", "title": "Everyday tools", "apps": ["habits", "todo", "atlas"]],
            ["id": "home-and-computer", "title": "Home and computer", "apps": ["homepanel", "syncthing"]],
            ["id": "developers", "title": "For developers", "apps": ["terminal", "gallery"]],
        ] as [[String: Any]],
        "apps": [
            "rss": known("rss", "Feeds", summaries["rss"]!, "1.4.0", shots: [("feeds.png", "The list of sites, with how many articles are unread")], access: ["Connects to online services"], notes: "Articles keep their pictures when they are saved for reading offline."),
            "panels": known("panels", "Panels", summaries["panels"]!, "1.0.0", shots: [("panels.png", "A page of a comic, whole"), ("panels/library.png", "The comics on the reader"), ("panels/reader.png", "One panel filling the screen")]),
            "vault": known("vault", "Vault", summaries["vault"]!, "1.1.0", shots: [("vault.png", "A note open for reading")]),
            "backgammon": known("backgammon", "Backgammon", summaries["backgammon"]!, "1.0.1", shots: [("backgammon.png", "Backgammon in the middle of a game against the computer"), ("backgammon/backgammon-opening.png", "A new game against the computer"), ("backgammon/backgammon-match.png", "The score of a match to five points")], description: "Play complete backgammon on one Kobo, solo against the computer or passing the reader between two people. The doubling cube, the Crawford rule and matches to a number of points are all there, and a game left half way is where you left it.", notes: "Against the computer, the cube is now offered when it should be."),
            "crossword": known("crossword", "Crossword", summaries["crossword"]!, "2.1.0", shots: [("crossword.png", "A puzzle part way through"), ("crossword/crossword-play.png", "The keyboard up, with the clue above it"), ("crossword/completed.png", "A finished puzzle")], description: longDescription, notes: "Rebus squares take more than one letter."),
            "lichess": known("lichess", "Chess", summaries["lichess"]!, "1.2.0", shots: [("lichess.png", "A game against the computer, with white to move"), ("lichess/computer-selection.png", "Playing the computer offline, with the legal moves for the selected bishop marked"), ("lichess/computer-large-text.png", "The same game at the largest text size"), ("lichess/challenge.png", "An incoming challenge from another player"), ("lichess/candidate.png", "A rated rapid game found and ready to open")], description: "Play chess on the reader. Against the computer it needs no network. With an account it finds rated games and takes challenges from other people, and a game you leave is where you left it.", access: ["Connects to online services", "Can keep the reader awake while in use"], notes: "The legal moves for a selected piece are marked on the board."),
            "morse": known("morse", "Morse", summaries["morse"]!, shots: [("morse.png", "A letter sounded and shown, with the last ten answers")]),
            "sudoku": known("sudoku", "Sudoku", summaries["sudoku"]!, shots: [("sudoku.png", "A puzzle with pencil marks")]),
            "nonograms": known("nonograms", "Nonograms", summaries["nonograms"]!, shots: [("nonograms.png", "A fifteen by fifteen puzzle")]),
            "habits": known("habits", "Habits", summaries["habits"]!, shots: [("habits.png", "A week of habits")]),
            "todo": known("todo", "To Do", summaries["todo"]!, shots: [("todo.png", "A list with two things done")]),
            "atlas": known("atlas", "Atlas", summaries["atlas"]!, "0.8.0"),
            "homepanel": known("homepanel", "Home Panel", summaries["homepanel"]!, shots: [("homepanel.png", "The lights and the heating in one room")], access: ["Connects to services on your network", "Can keep the reader awake while in use"], setup: [
                ["text": "Set up the reader once over USB.", "command": "kobo setup", "link": NSNull()],
                ["text": "Give the reader the address of your server and a token for it.", "command": "kobo secret set homepanel", "link": ["label": "Home Panel README", "url": "https://bandarlabs.github.io/Cobalt/apps/homepanel/"]],
            ]),
            "syncthing": known("syncthing", "Syncthing", summaries["syncthing"]!, shots: [("syncthing.png", "A folder in step, and when it last changed")], access: ["Connects to services on your network", "Reads and writes the reader's library"]),
            "terminal": known("terminal", "Terminal", summaries["terminal"]!, shots: [("terminal.png", "A shell on the reader")]),
            "gallery": known("gallery", "Components", summaries["gallery"]!, shots: [("components.png", "Buttons, lists and type at their real size")]),
        ] as [String: Any],
    ])

    private static let onReader = ok(["apps": [
        entry("backgammon", "Backgammon", summaries["backgammon"]!, "1.0.0", installed: "1.0.0", bytes: 640_300),
        entry("rss", "Feeds", summaries["rss"]!, "1.3.0", installed: "1.3.0"),
        entry("panels", "Panels", summaries["panels"]!, "0.9.2", installed: "0.9.2"),
        entry("vault", "Vault", summaries["vault"]!, "1.1.0", installed: "1.1.0"),
    ]])

    private static let published = ok(["apps": [
        entry("backgammon", "Backgammon", summaries["backgammon"]!, "1.0.1", installed: "1.0.0", update: true, bytes: 640_300),
        entry("rss", "Feeds", summaries["rss"]!, "1.4.0", installed: "1.3.0", update: true),
        entry("panels", "Panels", summaries["panels"]!, "1.0.0", installed: "0.9.2", update: true, runs: false, minimum: "0.4.0"),
        entry("vault", "Vault", summaries["vault"]!, "1.1.0", installed: "1.1.0"),
        entry("crossword", "Crossword", summaries["crossword"]!, "2.1.0", bytes: 2_310_000),
        entry("lichess", "Chess", summaries["lichess"]!, "1.2.0", bytes: 3_100_000),
        entry("morse", "Morse", summaries["morse"]!, "1.0.0", bytes: 540_000),
        entry("sudoku", "Sudoku", summaries["sudoku"]!, "1.0.0", bytes: 880_000),
        entry("nonograms", "Nonograms", summaries["nonograms"]!, "1.0.0", bytes: 910_000),
        entry("habits", "Habits", summaries["habits"]!, "1.0.0", bytes: 720_000),
        entry("todo", "To Do", summaries["todo"]!, "1.0.0", bytes: 700_000),
        entry("atlas", "Atlas", summaries["atlas"]!, "0.8.0", bytes: 412_000_000, fits: false),
        entry("homepanel", "Home Panel", summaries["homepanel"]!, "1.0.0", bytes: 1_400_000),
        entry("syncthing", "Syncthing", summaries["syncthing"]!, "1.0.0", bytes: 9_800_000),
        entry("terminal", "Terminal", summaries["terminal"]!, "1.0.0", bytes: 1_100_000),
        entry("gallery", "Components", summaries["gallery"]!, "1.0.0", bytes: 1_911_388),
        entry("tide-table", "Tide Table", summaries["tide-table"]!, "1.0.0", runs: false, minimum: "0.4.0", bytes: 388_000),
    ]])

    private static let withCobalt = state([reader(standing: "current", cobalt: "0.3.37")])
    private static let appsBase: [String: StandIn.Reply] = [
        "GET /api/state": state([reader(standing: "current", cobalt: "0.3.37", free: 310_000_000)]),
        "GET /api/store": storeAnswer, "GET /api/apps": onReader, "POST /api/apps/catalog": published,
    ]

    /// A folder of real pictures to answer with, when the render is given
    /// one. Without it each picture is a plain drawing of a screen.
    nonisolated(unsafe) static var pictures: URL?
    nonisolated(unsafe) private static var drawn: [String: Data] = [:]
    private nonisolated static let drawing = NSLock()

    /// The bytes for a picture: the published one when there is a folder of
    /// them and it is there, and otherwise a stand-in drawn here, so the
    /// render needs nothing but itself.
    ///
    /// Called from whichever thread the stand-in server answers on, so the
    /// store of pictures already made is kept under a lock.
    nonisolated static func picture(named name: String) -> Data {
        drawing.lock()
        defer { drawing.unlock() }
        if let found = drawn[name] { return found }
        var data = pictures.flatMap { try? Data(contentsOf: $0.appendingPathComponent(name)) }
        if data == nil {
            let size = NSSize(width: 1072, height: 1448)
            if let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1072, pixelsHigh: 1448, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
               let context = NSGraphicsContext(bitmapImageRep: bitmap) {
                NSGraphicsContext.saveGraphicsState()
                NSGraphicsContext.current = context
                NSColor.white.setFill()
                NSRect(origin: .zero, size: size).fill()
                let title = (name as NSString).lastPathComponent.replacingOccurrences(of: ".png", with: "")
                NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: 54, weight: .semibold), .foregroundColor: NSColor.black]).draw(at: NSPoint(x: 72, y: 1300))
                NSColor(white: 0.25, alpha: 1).setFill()
                NSRect(x: 72, y: 1268, width: 928, height: 3).fill()
                NSColor(white: 0.55, alpha: 1).setFill()
                for row in 0..<16 {
                    NSRect(x: 72, y: 1180 - CGFloat(row) * 64, width: row % 4 == 3 ? 520 : 928, height: 14).fill()
                }
                NSGraphicsContext.restoreGraphicsState()
                data = bitmap.representation(using: .png, properties: [:])
            }
        }
        let bytes = data ?? Data()
        drawn[name] = bytes
        return bytes
    }

    /// Which pictures the stand-in server can supply.
    enum Shots {
        case all
        /// The computer is offline and nothing is cached.
        case none
        case failing(Set<String>)
    }

    private static let chromeStep = "Chrome: open chrome://extensions, turn on Developer mode, choose Load unpacked and pick\n  /Users/owner/.config/kobo/clip/extension"
    private static let safariStep = "Safari: right-click a link and choose Services, then Send to Kobo. For the page you are on, select its address and do the same.\n  A full Safari extension needs a signed Mac app, which this is not."

    private static func browser(_ id: String, _ title: String, setUp: Bool, step: String = "") -> [String: Any] {
        ["id": id, "title": title, "set_up": setUp, "step": step]
    }

    private static let sendReady = ok([
        "readers": [["name": "kobo", "paired": true] as [String: Any]],
        "waiting": [
            ["title": "The Tides of the Severn Estuary", "site": "example.org"],
            ["title": "How a Lighthouse Lens Is Ground", "site": "journal.example.net"],
        ] as [[String: Any]],
    ])
    private static let sendBase: [String: StandIn.Reply] = ["GET /api/state": withCobalt, "GET /api/send": sendReady]

    /// A file of the given size that takes no room on disk, for the states
    /// that are about how large a file is.
    private static func file(_ name: String, bytes: UInt64) -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("cobalt-render", isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appendingPathComponent(name)
        FileManager.default.createFile(atPath: url.path, contents: nil)
        if let handle = try? FileHandle(forWritingTo: url) {
            try? handle.truncate(atOffset: bytes)
            try? handle.close()
        }
        return url
    }

    private static func with(_ base: [String: StandIn.Reply], _ more: [String: StandIn.Reply]) -> [String: StandIn.Reply] {
        base.merging(more) { _, new in new }
    }

    // The states.

    static let all: [Scenario] = reader + work + firstRun + apps + browserAndSend + window

    private static let linked: [[String: Any]] = [["name": "kobo", "volume": clara], ["name": "study", "volume": NSNull()]]

    private static let firstRun: [Scenario] = [
        Scenario(name: "path-tap-connect-stuck", routes: ["GET /api/state": state(attached: 1, saved: [])], act: { _, _ in drift = 25 }),
        path("path-writing", "writing", running: true, readers: [reader()], text: found, after: 48),
        path("path-eject", "eject", running: false, readers: [reader(standing: "current", cobalt: "0.3.37")]),
        path("path-eject-by-hand", "eject", running: false, readers: [reader(standing: "current", cobalt: "0.3.37")], ejects: false),
        path("path-finding", "finding", running: true, waited: 40),
        path("path-finding-long", "finding", running: true, waited: 215),
        path("path-securing", "securing", running: true, waited: 96),
        path("path-ready", "ready", running: false, savedAs: "kobo", secured: true),
        path("path-ready-with-notes", "ready", running: false, savedAs: "kobo", secured: false, notes: ["The welcome note could not be added to the library."]),
        path("path-not-found", "not_found", running: false, ok: false, waited: 300),
        path("path-several", "several", running: false, ok: false),
        path("path-no-network", "no_network", running: false, ok: false),
        Scenario(name: "path-awaiting", routes: ["GET /api/state": state(saved: [], awaiting: true)]),
        Scenario(name: "path-look-again-refused", routes: ["GET /api/state": state(saved: [], awaiting: true), "POST /api/reader/find": refuse(409, "busy", "Installing apps is still running. Wait for it to finish, then look again.")], act: { store, _ in await store.lookAgain() }),
        Scenario(name: "path-link", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37")], saved: [])]),
        Scenario(name: "path-link-plan", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37")], saved: []), "GET /api/plan": installPlan], act: { store, _ in await store.openPlan(undo: false) }),
        Scenario(name: "reader-none-with-saved", routes: ["GET /api/state": state(saved: [["name": "kobo", "volume": NSNull()], ["name": "study", "volume": NSNull()]])]),
        Scenario(name: "reader-current-with-saved", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37", savedAs: "kobo")], saved: linked)]),
        Scenario(name: "reader-rename-refused", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37", savedAs: "kobo")], saved: linked), "POST /api/readers/rename": refuse(409, "refused", "Another reader is already saved as “study”.")], act: { store, _ in await store.rename(saved: "kobo", to: "study") }),
        Scenario(name: "path-add-another", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37", savedAs: "kobo")], saved: linked)], act: { _, desk in desk.addingReader = true }),
        Scenario(name: "reader-rename-too-long", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37", savedAs: "kobo")], saved: linked)], act: { store, _ in await store.rename(saved: "kobo", to: String(repeating: "the reader in the study ", count: 3)) }),
        Scenario(name: "ask-rename", routes: ["GET /api/state": withCobalt], question: { _ in Questions.renaming("kobo").0 }),
        Scenario(name: "ask-forget", routes: ["GET /api/state": withCobalt], question: { _ in Questions.forgetting("kobo") }),
    ]

    private static let reader: [Scenario] = [
        Scenario(name: "reader-looking", routes: ["GET /api/state": .silence]),
        Scenario(name: "reader-none", routes: ["GET /api/state": state()]),
        Scenario(name: "reader-not-connected", routes: ["GET /api/state": state(attached: 1)]),
        Scenario(name: "reader-blocked", routes: ["GET /api/state": state(attached: 1, blocked: [clara])]),
        Scenario(name: "reader-blocked-from-terminal", routes: ["GET /api/state": state(attached: 1, blocked: [clara])], origin: .terminal),
        Scenario(name: "reader-two", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37"), reader(volume: libra, model: "Kobo Libra 2", firmware: "4.38.23171")])]),
        Scenario(name: "reader-chosen-of-two", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37"), reader(volume: libra, model: "Kobo Libra 2", firmware: "4.38.23171")])], act: { store, _ in store.choose(volume: libra) }),
        Scenario(name: "reader-refused", routes: ["GET /api/state": state([reader(model: "Kobo Clara Colour", firmware: "5.2.190625", refusal: "firmware 5.2.190625 cannot start Cobalt: NickelMenu is the only way in from the reader's own menus and it does not hook the 5.x firmware, so an installation here could never be launched")])]),
        Scenario(name: "reader-untested", routes: ["GET /api/state": state([reader(model: "Kobo Elipsa 2E", untested: "untested model. Cobalt has not been tested on this Kobo, so the screen may not draw correctly. Before its first screen it asks for two taps on a dot, so that taps land where they are aimed.")])]),
        Scenario(name: "reader-fresh", routes: ["GET /api/state": state([reader()])]),
        Scenario(name: "reader-behind", routes: ["GET /api/state": state([reader(standing: "behind", cobalt: "0.3.30")])]),
        Scenario(name: "reader-current", routes: ["GET /api/state": withCobalt]),
        Scenario(name: "reader-ahead", routes: ["GET /api/state": state([reader(standing: "ahead", cobalt: "0.4.0")])]),
        Scenario(name: "reader-no-release", routes: ["GET /api/state": state([reader()], release: nil)]),
        Scenario(name: "reader-no-release-with-cobalt", routes: ["GET /api/state": state([reader(standing: "unknown", cobalt: "0.3.30")], release: nil)]),
        Scenario(name: "reader-damaged", routes: ["GET /api/state": state([reader(standing: "unknown", problem: "The Cobalt on the reader does not say which release it is: .adds/cobalt/version is missing.")])]),
        Scenario(name: "reader-read-only", routes: ["GET /api/state": state([reader(standing: "behind", cobalt: "0.3.30", writable: false)])]),
        Scenario(name: "reader-second-not-connected", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37")], attached: 2)]),
        Scenario(name: "reader-ejected", routes: ["GET /api/state": withCobalt], actOnStage: { store, stage in
            stage.say(StandIn.routes(["GET /api/state": state(), "POST /api/reader/eject": ok(["ejected": true])]))
            await store.eject(volume: clara)
        }),
        Scenario(name: "reader-eject-failed", routes: ["GET /api/state": withCobalt, "POST /api/reader/eject": refuse(500, "failed", "The reader could not be ejected: diskutil eject /Volumes/KOBOeReader failed, because a file on it is open in Preview.")], act: { store, _ in await store.eject(volume: clara) }),
        Scenario(name: "reader-plan-install", routes: ["GET /api/state": state([reader()]), "GET /api/plan": installPlan], act: { store, _ in await store.openPlan(undo: false) }),
        Scenario(name: "reader-plan-remove", routes: ["GET /api/state": withCobalt, "GET /api/plan": removePlan], act: { store, _ in await store.openPlan(undo: true) }),
        Scenario(name: "reader-plan-busy", routes: ["GET /api/state": state([reader()]), "GET /api/plan": installPlan, "POST /api/reader/install": refuse(409, "busy", "Installing apps is still running on this reader. Wait for it to finish, then try again.")], act: { store, _ in
            await store.openPlan(undo: false)
            await store.agreeToPlan()
        }),
        Scenario(name: "reader-plan-refused", routes: ["GET /api/state": state([reader()]), "GET /api/plan": refuse(409, "refused", "The Cobalt release on this computer was refused: its manifest signature does not match.")], act: { store, _ in await store.openPlan(undo: false) }),
    ]

    private static func installing(_ name: String, text: String, after seconds: TimeInterval = 0, ok succeeded: Bool? = nil, title: String = "Installing Cobalt", plan: StandIn.Reply = installPlan, undo: Bool = false) -> Scenario {
        Scenario(
            name: name,
            routes: [
                "GET /api/state": state([reader()]),
                "GET /api/plan": plan,
                "POST /api/reader/install": ok(["job": 3]),
                "POST /api/reader/remove": ok(["job": 3]),
                "GET /api/job": job(3, title, running: succeeded == nil, ok: succeeded, text: text),
            ],
            act: { store, _ in
                await store.openPlan(undo: undo)
                await store.agreeToPlan()
                drift = seconds
            }
        )
    }

    private static let work: [Scenario] = [
        installing("reader-installing", text: found, after: 48),
        installing("reader-installing-steps", text: waitingForWifi, after: 95),
        installing("reader-installed", text: installed, ok: true),
        installing("reader-removed", text: removed, ok: true, title: "Removing Cobalt", plan: removePlan, undo: true),
        installing("reader-failed", text: truncated, ok: false),
        Scenario(
            name: "reader-installing-quiet",
            routes: ["GET /api/state": state([reader()], jobs: [running(3, "Installing Cobalt")]), "GET /api/job": job(3, "Installing Cobalt", running: true, text: found)],
            act: { store, _ in
                // The job has printed nothing since the window found it.
                await store.tick()
                drift = 195
            }
        ),
        Scenario(
            name: "reader-pulled",
            routes: ["GET /api/state": state([], jobs: [running(3, "Installing Cobalt")]), "GET /api/job": job(3, "Installing Cobalt", running: true, text: found)],
            act: { store, _ in
                await store.tick()
                drift = 62
            }
        ),
        Scenario(
            name: "reader-failed-gone",
            routes: [
                "GET /api/state": state([], jobs: [running(3, "Updating Cobalt")]),
                "GET /api/job": job(3, "Updating Cobalt", running: false, ok: false, text: found + "kobo: could not write /Volumes/KOBOeReader/.adds/cobalt.staging/bin/kobod: No such file or directory (os error 2)\n"),
            ],
            act: { store, _ in await store.tick() }
        ),
    ]

    private static func loaded(_ more: (@MainActor (Companion) async -> Void)? = nil) -> @MainActor (Companion, Desk) async -> Void {
        { store, _ in
            await store.loadStore()
            await store.loadApps()
            // The pictures the grid opens on, fetched before it is drawn so
            // the image is of the place as it settles and not mid-arrival.
            for app in store.store.store?.apps.values.sorted(by: { $0.id < $1.id }) ?? [] {
                if let first = app.shots.first { await store.loadShot(first.name) }
            }
            await more?(store)
        }
    }

    private static func page(_ id: String, _ more: (@MainActor (Companion) async -> Void)? = nil) -> @MainActor (Companion, Desk) async -> Void {
        loaded { store in
            for shot in store.store.store?.apps[id]?.shots ?? [] { await store.loadShot(shot.name) }
            store.open(app: id)
            await more?(store)
        }
    }

    private static let apps: [Scenario] = [
        Scenario(name: "store-browse", place: .apps, routes: appsBase, act: loaded()),
        Scenario(name: "store-no-reader", place: .apps, routes: ["GET /api/state": state(), "GET /api/store": storeAnswer], act: loaded()),
        Scenario(name: "store-no-cobalt", place: .apps, routes: ["GET /api/state": state([reader()]), "GET /api/store": storeAnswer], act: loaded()),
        Scenario(name: "store-two-readers", place: .apps, routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37"), reader(volume: libra, model: "Kobo Libra 2")]), "GET /api/store": storeAnswer], act: loaded()),
        Scenario(name: "store-reading", place: .apps, routes: with(appsBase, ["GET /api/apps": .silence]), act: { store, _ in
            await store.loadStore()
            await midway { await store.loadApps() }
        }),
        Scenario(name: "store-fetching", place: .apps, routes: with(appsBase, ["POST /api/apps/catalog": .silence]), act: { store, _ in
            await store.loadStore()
            await midway { await store.loadApps() }
        }),
        Scenario(name: "store-offline", place: .apps, routes: with(appsBase, ["POST /api/apps/catalog": refuse(502, "unreachable", "The app catalog could not be reached: this computer does not seem to be online.")]), shots: .none, act: loaded()),
        Scenario(name: "store-picture-fails", place: .apps, routes: appsBase, shots: .failing(["crossword.png", "habits.png"]), act: loaded()),
        Scenario(name: "store-catalog-integrity", place: .apps, routes: with(appsBase, ["POST /api/apps/catalog": refuse(502, "integrity", "The app catalog was fetched and its signature does not match the key Cobalt was built with, so it was not used.")]), act: loaded()),
        Scenario(name: "store-reader-unreadable", place: .apps, routes: with(appsBase, ["GET /api/apps": refuse(500, "failed", "The apps on the reader could not be read: .adds/cobalt/apps is not a folder.")]), act: loaded()),
        Scenario(name: "store-without-store", place: .apps, routes: with(appsBase, ["GET /api/store": refuse(404, "not_found", "There is nothing at that address.")]), act: loaded()),
        Scenario(name: "store-search", place: .apps, routes: appsBase, act: loaded { store in store.search("puzzle") }),
        Scenario(name: "store-no-match", place: .apps, routes: appsBase, act: loaded { store in store.search("spreadsheet") }),
        Scenario(name: "store-installing", place: .apps, routes: with(appsBase, ["POST /api/apps/install": ok(["job": 5]), "GET /api/job": job(5, "Installing apps", running: true, text: "Backgammon 1.0.1: installed\nFeeds 1.4.0: fetching\n")]), act: loaded { store in await store.installApps(["backgammon", "rss"]) }),
        Scenario(name: "store-installed", place: .apps, routes: with(appsBase, ["POST /api/apps/install": ok(["job": 5]), "GET /api/job": job(5, "Installing an app", running: false, ok: true, text: "Crossword 2.1.0: installed\n")]), act: loaded { store in await store.installApps(["crossword"]) }),
        Scenario(name: "store-install-incomplete", place: .apps, routes: with(appsBase, ["POST /api/apps/install": ok(["job": 5]), "GET /api/job": job(5, "Installing apps", running: false, ok: false, text: "Backgammon 1.0.1: installed\nkobo: Feeds 1.4.0 was not installed: its package does not match the digest in the catalog. Nothing of it was kept.\n")]), act: loaded { store in await store.installApps(["backgammon", "rss"]) }),
        Scenario(name: "store-no-room", place: .apps, routes: with(appsBase, ["POST /api/apps/install": refuse(409, "no_room", "Syncthing needs 9.8 MB and the reader has 1.1 MB free. Remove something from the reader and try again.")]), act: loaded { store in await store.installApps(["syncthing"]) }),
        Scenario(name: "store-read-only", place: .apps, routes: with(appsBase, ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37", writable: false)])]), act: loaded()),
        Scenario(name: "store-removed", place: .apps, routes: with(appsBase, ["POST /api/apps/remove": ok(["apps": [] as [Any], "removed": ["id": "vault", "title": "Vault"]])]), act: loaded { store in await store.removeApp(id: "vault", title: "Vault") }),
        Scenario(name: "store-app-update", place: .apps, routes: appsBase, act: page("backgammon")),
        Scenario(name: "store-app-long", place: .apps, routes: appsBase, act: page("crossword")),
        Scenario(name: "store-app-many-pictures", place: .apps, routes: appsBase, act: page("lichess")),
        Scenario(name: "store-app-one-picture", place: .apps, routes: appsBase, act: page("morse")),
        Scenario(name: "store-app-setup", place: .apps, routes: appsBase, act: page("homepanel")),
        Scenario(name: "store-app-no-shots", place: .apps, routes: appsBase, act: page("atlas")),
        Scenario(name: "store-app-needs-newer", place: .apps, routes: appsBase, act: page("tide-table")),
        Scenario(name: "store-app-update-needs-newer", place: .apps, routes: appsBase, act: page("panels")),
        Scenario(name: "store-app-no-reader", place: .apps, routes: ["GET /api/state": state(), "GET /api/store": storeAnswer], act: page("backgammon")),
        Scenario(name: "store-app-picture-fails", place: .apps, routes: appsBase, shots: .failing(["crossword/crossword-play.png"]), act: page("crossword")),
        Scenario(name: "store-app-installing", place: .apps, routes: with(appsBase, ["POST /api/apps/install": ok(["job": 5]), "GET /api/job": job(5, "Installing an app", running: true, text: "Crossword 2.1.0: fetching\n")]), act: page("crossword") { store in await store.installApps(["crossword"]) }),
        Scenario(name: "apps-remove-ask", place: .apps, routes: appsBase, act: loaded(), question: { _ in Questions.removal(of: "Vault") }),
    ]

    private static let browserAndSend: [Scenario] = [
        Scenario(name: "browser-none", place: .browser, routes: ["GET /api/state": withCobalt, "GET /api/browsers": ok(["browsers": [] as [Any]])], act: { store, _ in await store.loadBrowsers() }),
        Scenario(name: "browser-list", place: .browser, routes: ["GET /api/state": withCobalt, "GET /api/browsers": ok(["browsers": [browser("chrome", "Chrome", setUp: true, step: chromeStep), browser("firefox", "Firefox", setUp: false), browser("safari", "Safari", setUp: true, step: safariStep)]])], act: { store, _ in await store.loadBrowsers() }),
        Scenario(name: "browser-removed", place: .browser, routes: ["GET /api/state": withCobalt, "GET /api/browsers": ok(["browsers": [browser("chrome", "Chrome", setUp: false), browser("safari", "Safari", setUp: false)]]), "POST /api/browsers/remove": ok(["browsers": [browser("chrome", "Chrome", setUp: false), browser("safari", "Safari", setUp: false)], "said": "Removed."])], act: { store, _ in
            await store.loadBrowsers()
            if let chrome = store.browser.list?.first { await store.change(browser: chrome, setUp: false) }
        }),
        Scenario(name: "browser-failed", place: .browser, routes: ["GET /api/state": withCobalt, "GET /api/browsers": ok(["browsers": [browser("chrome", "Chrome", setUp: false), browser("safari", "Safari", setUp: false)]]), "POST /api/browsers/setup": refuse(500, "failed", "Chrome could not be set up: /Users/owner/Library/Application Support/Google/Chrome/NativeMessagingHosts could not be written to (Permission denied).")], act: { store, _ in
            await store.loadBrowsers()
            if let chrome = store.browser.list?.first { await store.change(browser: chrome, setUp: true) }
        }),
        Scenario(name: "send-empty", place: .send, routes: ["GET /api/state": withCobalt, "GET /api/send": ok(["readers": [] as [Any], "waiting": [] as [Any]])], act: { store, _ in await store.loadSend() }),
        Scenario(name: "send-ready", place: .send, routes: sendBase, act: { store, _ in await store.loadSend() }),
        Scenario(name: "send-two-readers", place: .send, routes: with(sendBase, ["GET /api/send": ok(["readers": [["name": "kobo"], ["name": "libra"]], "waiting": [] as [Any]])]), act: { store, _ in await store.loadSend() }),
        Scenario(name: "send-uploading", place: .send, routes: with(sendBase, ["POST /api/send/file": .silence]), act: { store, _ in
            await store.loadSend()
            await midway { await store.offer(file: file("Winter.cbz", bytes: 48_300_000)) }
            store.uploaded(19_800_000, of: "Winter.cbz")
        }),
        Scenario(name: "send-choice", place: .send, routes: with(sendBase, ["POST /api/send/file": ok(["upload": 7, "name": "Sonata in G.pdf", "bytes": 4096, "choices": [["name": "musicstand", "title": "Music Stand", "purpose": "scores to play from"], ["name": "vault", "title": "Vault", "purpose": "papers to keep"]]])]), act: { store, _ in
            await store.loadSend()
            await store.offer(file: file("Sonata in G.pdf", bytes: 4096))
            store.choose(app: "musicstand")
        }),
        Scenario(name: "send-replace", place: .send, routes: with(sendBase, ["POST /api/send/file": ok(["upload": 7, "name": "Harbour at dusk.jpg", "bytes": 4096, "app": ["name": "frame", "title": "Frame", "purpose": "a photo on the sleep screen", "replaces": "photo"]])]), act: { store, _ in
            await store.loadSend()
            await store.offer(file: file("Harbour at dusk.jpg", bytes: 4096))
        }),
        sending("send-sending", running: true, text: "preparing Winter.cbz for Panels\n"),
        sending("send-sent", running: false, ok: true, text: "preparing Winter.cbz for Panels\nSent Winter.cbz to Panels on kobo.\n"),
        sending("send-failed", running: false, ok: false, text: "preparing Winter.cbz for Panels\nkobo: No reader saved. Run kobo setup with the reader plugged in, or kobo pair.\n"),
        sending("send-network-denied", running: false, ok: false, text: "preparing Winter.cbz for Panels\nkobo: could not reach the reader at 192.168.1.20: No route to host (os error 65)\n"),
        Scenario(name: "send-unsupported", place: .send, routes: with(sendBase, ["POST /api/send/file": refuse(422, "unsupported_file", "No app on the reader takes a .numbers file, so Budget.numbers has nowhere to go.")]), act: { store, _ in
            await store.loadSend()
            await store.offer(file: file("Budget.numbers", bytes: 4096))
        }),
        Scenario(name: "send-too-large", place: .send, routes: sendBase, act: { store, _ in
            await store.loadSend()
            await store.offer(file: file("Everything.cbz", bytes: 1_900_000_000))
        }),
        Scenario(name: "send-address-problem", place: .send, routes: sendBase, act: { store, _ in
            await store.loadSend()
            await store.send(address: "example.org/tides")
        }),
        Scenario(name: "send-article-waits", place: .send, routes: with(sendBase, ["POST /api/send/address": ok(["job": 9]), "GET /api/job": job(9, "Sending an article", running: false, ok: true, text: "Waiting for your Kobo: The Tides of the Severn Estuary. Your Kobo did not answer. It may be asleep or away from Wi-Fi. It goes over with kobo clip sync, with Sync on the reader's Articles shelf, or with the next clip that finds the reader awake.\n", kind: "send", volume: nil)]), act: { store, _ in
            await store.loadSend()
            await store.send(address: "https://example.org/tides")
        }),
        Scenario(name: "send-queue-still-waiting", place: .send, routes: with(sendBase, ["POST /api/send/queue": ok(["job": 10]), "GET /api/job": job(10, "Sending waiting articles", running: false, ok: false, text: "kobo: 2 articles still waiting for kobo. Your Kobo did not answer. It may be asleep or away from Wi-Fi.\n", kind: "send", volume: nil)]), act: { store, _ in
            await store.loadSend()
            await store.sendQueue()
        }),
    ]

    private static func sending(_ name: String, running: Bool, ok succeeded: Bool? = nil, text: String) -> Scenario {
        Scenario(name: name, place: .send, routes: with(sendBase, [
            "POST /api/send/file": ok(["upload": 7, "name": "Winter.cbz", "bytes": 4096, "app": ["name": "panels", "title": "Panels", "purpose": "comics"]]),
            "POST /api/send/start": ok(["job": 8]),
            "GET /api/job": job(8, "Sending Winter.cbz", running: running, ok: succeeded, text: text, kind: "send", volume: nil),
        ]), act: { store, _ in
            await store.loadSend()
            await store.offer(file: file("Winter.cbz", bytes: 4096))
        })
    }

    private static let window: [Scenario] = [
        Scenario(name: "starting", starting: true),
        Scenario(name: "lost-stopped", routes: ["GET /api/state": withCobalt], act: { store, _ in store.serverStopped(said: "") }),
        Scenario(name: "lost-refused", routes: ["GET /api/state": refuse(401, "session", "This session is not accepted.")]),
        Scenario(name: "lost-missing", lost: Lost(.missing(path: "/Users/owner/.local/bin/kobo"))),
        Scenario(name: "lost-exited", lost: Lost(.exited(said: "kobo: 127.0.0.1 could not be listened on: Operation not permitted (os error 1)"))),
        Scenario(name: "lost-silent", lost: Lost(.silent(seconds: 10))),
        Scenario(name: "lost-garbled", lost: Lost(.garbled)),
        Scenario(name: "version-contract", routes: ["GET /api/state": state([reader()], api: 2, kobo: "0.4.0")]),
        Scenario(name: "version-release", routes: ["GET /api/state": state([reader(standing: "current", cobalt: "0.3.37")], kobo: "0.3.36")]),
        Scenario(name: "ask-quit-install", routes: ["GET /api/state": state([reader()], jobs: [running(3, "Installing Cobalt")])], question: { store in store.quitQuestion.map(Questions.alert) }),
        Scenario(name: "ask-quit-install-from-terminal", routes: ["GET /api/state": state([reader()], jobs: [running(3, "Installing Cobalt")])], origin: .terminal, question: { store in store.quitQuestion.map(Questions.alert) }),
        Scenario(name: "ask-quit-send", routes: ["GET /api/state": state([reader()], jobs: [running(8, "Sending Winter.cbz", kind: "send", volume: nil)])], question: { store in store.quitQuestion.map(Questions.alert) }),
        Scenario(name: "ask-quit-upload", place: .send, routes: with(sendBase, ["POST /api/send/file": .silence]), act: { store, _ in
            await midway { await store.offer(file: file("Winter.cbz", bytes: 48_300_000)) }
        }, question: { store in store.quitQuestion.map(Questions.alert) }),
    ]
}
