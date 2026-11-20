import Foundation

// What the server says, as it says it.
//
// Every type here decodes a field the server may not send as absent rather
// than failing. The app and the kobo command are installed together, but a
// window can outlive an update of the command behind it, and an answer with
// one field missing is still mostly an answer. What cannot be done without is
// required, and its absence is a malformed reply, which the client reports.

/// Everything `GET /api/state` answers with: this computer, the readers
/// plugged into it, and the work in hand.
public struct ServerState: Decodable, Equatable, Sendable {
    /// The contract number. The app speaks 1 and says so when the server
    /// speaks anything else.
    public var api: Int?
    public var computer: Computer
    /// Kobo devices on the USB bus, mounted or not. Nil where the system
    /// cannot tell.
    public var attached: Int?
    /// Drives that look like readers and that the system would not let the
    /// server read.
    public var blocked: [String]
    public var readers: [Reader]
    public var jobs: [JobSummary]
    /// The readers this computer has been linked to, by the name each was
    /// saved under. Nil from a server that does not say, which is not the
    /// same as none.
    public var saved: [SavedLink]?
    /// Cobalt was put on a reader from this computer and that reader has
    /// not yet been found on Wi-Fi and saved.
    public var awaiting: Bool

    enum CodingKeys: String, CodingKey { case api, computer, attached, blocked, readers, jobs, saved, awaiting }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        api = try container.decodeIfPresent(Int.self, forKey: .api)
        computer = try container.decode(Computer.self, forKey: .computer)
        attached = try container.decodeIfPresent(Int.self, forKey: .attached)
        blocked = try container.decodeIfPresent([String].self, forKey: .blocked) ?? []
        readers = try container.decodeIfPresent([Reader].self, forKey: .readers) ?? []
        jobs = try container.decodeIfPresent([JobSummary].self, forKey: .jobs) ?? []
        saved = try container.decodeIfPresent([SavedLink].self, forKey: .saved)
        awaiting = try container.decodeIfPresent(Bool.self, forKey: .awaiting) ?? false
    }
}

/// A reader this computer can reach by name over Wi-Fi.
public struct SavedLink: Decodable, Equatable, Sendable, Identifiable {
    public var name: String
    /// Where it is mounted, when it is the one plugged in now.
    public var volume: String?
    public var id: String { name }
}

public struct SavedAnswer: Decodable, Equatable, Sendable {
    public var saved: [SavedLink]
}

/// How far a first install has got, as the server sees it.
public enum SetupStage: String, Sendable {
    case writing, eject, finding, securing, ready
    case notFound = "not_found"
    case several
    case noNetwork = "no_network"
    case failed
    /// A stage this build has not heard of. The job is still followed and
    /// what it says is still shown; only the list of steps is left out.
    case unknown
}

public struct SetupProgress: Decodable, Equatable, Sendable {
    public var stage: SetupStage
    /// Seconds spent looking for the reader on Wi-Fi, and how many it is
    /// given.
    public var waited: Int
    public var limit: Int
    public var savedAs: String?
    /// Whether managing the reader was confirmed to be this computer's
    /// alone. Nil until it is known either way.
    public var secured: Bool?
    /// Things that did not go as planned and did not stop it, in sentences
    /// written for the owner.
    public var notes: [String]

    enum CodingKeys: String, CodingKey { case stage, waited, limit, savedAs, secured, notes }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        stage = SetupStage(rawValue: try container.decodeIfPresent(String.self, forKey: .stage) ?? "") ?? .unknown
        waited = try container.decodeIfPresent(Int.self, forKey: .waited) ?? 0
        limit = try container.decodeIfPresent(Int.self, forKey: .limit) ?? 300
        savedAs = try container.decodeIfPresent(String.self, forKey: .savedAs)
        secured = try container.decodeIfPresent(Bool.self, forKey: .secured)
        notes = try container.decodeIfPresent([String].self, forKey: .notes) ?? []
    }
}

public struct Computer: Decodable, Equatable, Sendable {
    /// The version of the kobo command that is answering.
    public var kobo: String
    public var os: String?
    /// The Cobalt release this computer would install, when it has one.
    public var release: String?
    /// Why it has none, in the command's words.
    public var releaseProblem: String?
    /// Whether this system can eject a reader for the owner.
    public var ejects: Bool
    public var uploadLimit: Int64?
    /// Whether this computer is on a network a reader could be found on.
    public var network: Bool?

    enum CodingKeys: String, CodingKey { case kobo, os, release, releaseProblem, ejects, uploadLimit, network }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        kobo = try container.decode(String.self, forKey: .kobo)
        os = try container.decodeIfPresent(String.self, forKey: .os)
        release = try container.decodeIfPresent(String.self, forKey: .release)
        releaseProblem = try container.decodeIfPresent(String.self, forKey: .releaseProblem)
        ejects = try container.decodeIfPresent(Bool.self, forKey: .ejects) ?? false
        uploadLimit = try container.decodeIfPresent(Int64.self, forKey: .uploadLimit)
        network = try container.decodeIfPresent(Bool.self, forKey: .network)
    }
}

/// How the Cobalt on a reader compares with the release this computer has.
public enum Standing: String, Sendable {
    case absent, current, behind, ahead, unknown
}

public struct InstalledCobalt: Decodable, Equatable, Sendable {
    public var cobaltVersion: String
    public var channel: String?
}

public struct Reader: Decodable, Equatable, Sendable, Identifiable {
    /// Where the reader's storage is mounted, which is also how the server
    /// is told which reader is meant.
    public var volume: String
    public var model: String
    public var modelCode: String?
    public var firmware: String?
    public var supported: Bool
    /// Why Cobalt cannot be installed, written to follow a colon.
    public var refusal: String?
    public var untested: String?
    public var standing: Standing
    public var cobalt: InstalledCobalt?
    public var cobaltProblem: String?
    public var freeBytes: Int64?
    /// False when the system mounted the storage read-only. Nil when the
    /// server could not tell.
    public var writable: Bool?
    /// The name this computer saved the reader under, when it has.
    public var savedAs: String?
    /// Where on this model's screen its menu is: "bottom right" or
    /// "top left".
    public var menuCorner: String?

    public var id: String { volume }

    enum CodingKeys: String, CodingKey {
        case volume, model, modelCode, firmware, supported, refusal, untested, standing, cobalt, cobaltProblem, freeBytes, writable, savedAs, menuCorner
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        volume = try container.decode(String.self, forKey: .volume)
        model = try container.decodeIfPresent(String.self, forKey: .model) ?? "Kobo"
        modelCode = try container.decodeIfPresent(String.self, forKey: .modelCode)
        firmware = try container.decodeIfPresent(String.self, forKey: .firmware)
        supported = try container.decodeIfPresent(Bool.self, forKey: .supported) ?? false
        refusal = try container.decodeIfPresent(String.self, forKey: .refusal)
        untested = try container.decodeIfPresent(String.self, forKey: .untested)
        // A standing this build has not heard of is one it cannot act on,
        // which is what "unknown" already means.
        let named = try container.decodeIfPresent(String.self, forKey: .standing) ?? ""
        standing = Standing(rawValue: named) ?? .unknown
        cobalt = try container.decodeIfPresent(InstalledCobalt.self, forKey: .cobalt)
        cobaltProblem = try container.decodeIfPresent(String.self, forKey: .cobaltProblem)
        freeBytes = try container.decodeIfPresent(Int64.self, forKey: .freeBytes)
        writable = try container.decodeIfPresent(Bool.self, forKey: .writable)
        savedAs = try container.decodeIfPresent(String.self, forKey: .savedAs)
        menuCorner = try container.decodeIfPresent(String.self, forKey: .menuCorner)
    }
}

/// The two queues the server runs work in. One job of each kind at a time.
public enum JobKind: String, Sendable {
    case reader, send, other

    init(named: String?) {
        self = JobKind(rawValue: named ?? "") ?? .other
    }
}

public struct JobSummary: Decodable, Equatable, Sendable, Identifiable {
    public var id: Int
    public var kind: JobKind
    public var title: String
    public var running: Bool
    public var ok: Bool?
    public var volume: String?
    public var setup: SetupProgress?

    enum CodingKeys: String, CodingKey { case id, kind, title, running, ok, volume, setup }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(Int.self, forKey: .id)
        kind = JobKind(named: try container.decodeIfPresent(String.self, forKey: .kind))
        title = try container.decodeIfPresent(String.self, forKey: .title) ?? ""
        running = try container.decodeIfPresent(Bool.self, forKey: .running) ?? false
        ok = try container.decodeIfPresent(Bool.self, forKey: .ok)
        volume = try container.decodeIfPresent(String.self, forKey: .volume)
        setup = try container.decodeIfPresent(SetupProgress.self, forKey: .setup)
    }
}

/// What a job has said since the offset it was asked from.
public struct JobReport: Decodable, Equatable, Sendable {
    public var id: Int
    public var kind: JobKind
    public var title: String
    public var running: Bool
    public var ok: Bool?
    public var code: Int?
    public var text: String
    public var next: Int
    public var volume: String?
    public var setup: SetupProgress?

    enum CodingKeys: String, CodingKey { case id, kind, title, running, ok, code, text, next, volume, setup }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(Int.self, forKey: .id)
        kind = JobKind(named: try container.decodeIfPresent(String.self, forKey: .kind))
        title = try container.decodeIfPresent(String.self, forKey: .title) ?? ""
        running = try container.decode(Bool.self, forKey: .running)
        ok = try container.decodeIfPresent(Bool.self, forKey: .ok)
        code = try container.decodeIfPresent(Int.self, forKey: .code)
        text = try container.decodeIfPresent(String.self, forKey: .text) ?? ""
        next = try container.decode(Int.self, forKey: .next)
        volume = try container.decodeIfPresent(String.self, forKey: .volume)
        setup = try container.decodeIfPresent(SetupProgress.self, forKey: .setup)
    }
}

/// The dry run: what an install or a removal would change, before it does.
public struct Plan: Decodable, Equatable, Sendable {
    public var undo: Bool
    public var lines: [String]
    public var release: String?
    public var untested: String?

    enum CodingKeys: String, CodingKey { case undo, lines, release, untested }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        undo = try container.decodeIfPresent(Bool.self, forKey: .undo) ?? false
        lines = try container.decode([String].self, forKey: .lines)
        release = try container.decodeIfPresent(String.self, forKey: .release)
        untested = try container.decodeIfPresent(String.self, forKey: .untested)
    }
}

public struct JobStarted: Decodable, Equatable, Sendable {
    public var job: Int
}

public struct Ejected: Decodable, Equatable, Sendable {
    public var ejected: Bool
}

public struct AppEntry: Decodable, Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var label: String?
    public var summary: String
    /// The version on offer, or for an app only the reader knows about, the
    /// version it has.
    public var version: String
    public var installedVersion: String?
    public var updateAvailable: Bool
    public var minimumCobaltVersion: String?
    public var runsOnThisReader: Bool
    public var capabilities: [String]
    public var bytes: Int64?
    public var fits: Bool?

    enum CodingKeys: String, CodingKey {
        case id, title, label, summary, version, installedVersion, updateAvailable, minimumCobaltVersion, runsOnThisReader, capabilities, bytes, fits
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        title = try container.decodeIfPresent(String.self, forKey: .title) ?? id
        label = try container.decodeIfPresent(String.self, forKey: .label)
        summary = try container.decodeIfPresent(String.self, forKey: .summary) ?? ""
        version = try container.decodeIfPresent(String.self, forKey: .version) ?? ""
        installedVersion = try container.decodeIfPresent(String.self, forKey: .installedVersion)
        updateAvailable = try container.decodeIfPresent(Bool.self, forKey: .updateAvailable) ?? false
        minimumCobaltVersion = try container.decodeIfPresent(String.self, forKey: .minimumCobaltVersion)
        runsOnThisReader = try container.decodeIfPresent(Bool.self, forKey: .runsOnThisReader) ?? true
        capabilities = try container.decodeIfPresent([String].self, forKey: .capabilities) ?? []
        bytes = try container.decodeIfPresent(Int64.self, forKey: .bytes)
        fits = try container.decodeIfPresent(Bool.self, forKey: .fits)
    }
}

public struct AppsAnswer: Decodable, Equatable, Sendable {
    public var apps: [AppEntry]
}

public struct RemovedApp: Decodable, Equatable, Sendable {
    public var id: String
    public var title: String?
}

public struct AppRemoved: Decodable, Equatable, Sendable {
    public var apps: [AppEntry]
    public var removed: RemovedApp?
}

public struct Browser: Decodable, Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var setUp: Bool
    /// The one thing left to do in the browser itself, as the command
    /// prints it. Empty when nothing is left.
    public var step: String

    enum CodingKeys: String, CodingKey { case id, title, setUp, step }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        title = try container.decodeIfPresent(String.self, forKey: .title) ?? id
        setUp = try container.decodeIfPresent(Bool.self, forKey: .setUp) ?? false
        step = try container.decodeIfPresent(String.self, forKey: .step) ?? ""
    }
}

public struct BrowsersAnswer: Decodable, Equatable, Sendable {
    public var browsers: [Browser]
    public var said: String?
}

public struct SavedReader: Decodable, Equatable, Sendable, Identifiable {
    public var name: String
    public var paired: Bool?
    public var id: String { name }
}

public struct WaitingArticle: Decodable, Equatable, Sendable {
    public var title: String
    public var site: String?
}

public struct SendInfo: Decodable, Equatable, Sendable {
    public var readers: [SavedReader]
    public var waiting: [WaitingArticle]

    enum CodingKeys: String, CodingKey { case readers, waiting }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        readers = try container.decodeIfPresent([SavedReader].self, forKey: .readers) ?? []
        waiting = try container.decodeIfPresent([WaitingArticle].self, forKey: .waiting) ?? []
    }
}

/// One of the reader's apps as somewhere a file could go.
public struct SendApp: Decodable, Equatable, Sendable, Identifiable {
    public var name: String
    public var title: String
    public var purpose: String?
    /// What the app holds one of at a time, when sending replaces it.
    public var replaces: String?
    public var id: String { name }
}

/// A file the server has taken and is holding until it is told where to
/// send it.
public struct Upload: Decodable, Equatable, Sendable {
    public var upload: Int
    public var name: String
    public var bytes: Int64?
    public var app: SendApp?
    /// More than one app could take the file, so the owner chooses.
    public var choices: [SendApp]?
}

/// A picture of an app on the reader's screen.
public struct Shot: Decodable, Equatable, Sendable, Identifiable {
    /// What the server knows the picture by, and what it is asked for as.
    public var name: String
    public var caption: String
    public var width: Int
    public var height: Int

    public var id: String { name }

    enum CodingKeys: String, CodingKey { case name, caption, width, height }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        name = try container.decode(String.self, forKey: .name)
        caption = try container.decodeIfPresent(String.self, forKey: .caption) ?? ""
        // A reader's screen, which is what nearly every picture is of, when
        // the server does not say.
        width = try container.decodeIfPresent(Int.self, forKey: .width) ?? 1072
        height = try container.decodeIfPresent(Int.self, forKey: .height) ?? 1448
    }
}

public struct StoreLink: Decodable, Equatable, Sendable {
    public var label: String
    public var url: String
}

/// One thing to do before an app is of use, as `kobo apps setup` puts it.
public struct SetupStep: Decodable, Equatable, Sendable {
    public var text: String
    public var command: String?
    public var link: StoreLink?
}

/// An app as the store describes it, whatever reader is or is not plugged
/// in.
public struct StoreApp: Decodable, Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var summary: String
    public var needs: [String]
    public var description: String
    public var version: String
    public var releaseNotes: String
    public var minimumCobaltVersion: String?
    public var access: [String]
    public var setup: [SetupStep]
    public var shots: [Shot]

    enum CodingKeys: String, CodingKey {
        case id, title, summary, needs, description, version, releaseNotes, minimumCobaltVersion, access, setup, shots
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        title = try container.decodeIfPresent(String.self, forKey: .title) ?? id
        summary = try container.decodeIfPresent(String.self, forKey: .summary) ?? ""
        needs = try container.decodeIfPresent([String].self, forKey: .needs) ?? []
        description = try container.decodeIfPresent(String.self, forKey: .description) ?? ""
        version = try container.decodeIfPresent(String.self, forKey: .version) ?? ""
        releaseNotes = try container.decodeIfPresent(String.self, forKey: .releaseNotes) ?? ""
        minimumCobaltVersion = try container.decodeIfPresent(String.self, forKey: .minimumCobaltVersion)
        access = try container.decodeIfPresent([String].self, forKey: .access) ?? []
        setup = try container.decodeIfPresent([SetupStep].self, forKey: .setup) ?? []
        shots = try container.decodeIfPresent([Shot].self, forKey: .shots) ?? []
    }
}

public struct StoreGroup: Decodable, Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var apps: [String]
}

/// The whole store: every app the kobo command was built knowing, in the
/// groups the project's website lists them in. It needs no reader and no
/// network.
public struct Store: Decodable, Equatable, Sendable {
    public var groups: [StoreGroup]
    public var apps: [String: StoreApp]

    enum CodingKeys: String, CodingKey { case groups, apps }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        groups = try container.decodeIfPresent([StoreGroup].self, forKey: .groups) ?? []
        // The apps are keyed by id, and ids have hyphens in them that the
        // snake-case reading of keys would otherwise leave alone or mangle,
        // so the keys are read as they are written.
        let plain = try container.nestedContainer(keyedBy: AnyKey.self, forKey: .apps)
        var apps: [String: StoreApp] = [:]
        for key in plain.allKeys {
            let app = try plain.decode(StoreApp.self, forKey: key)
            apps[app.id] = app
        }
        self.apps = apps
    }

    private struct AnyKey: CodingKey {
        var stringValue: String
        var intValue: Int? { nil }
        init?(stringValue: String) { self.stringValue = stringValue }
        init?(intValue: Int) { nil }
    }
}

extension JSONDecoder {
    /// The decoder every answer goes through, in the app, the tests and the
    /// renders alike.
    static var wire: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }
}

public enum Wire {
    /// Decodes a reply the way the client does. Exposed for fixtures, so
    /// that a state drawn from one has been through the shipped decoding.
    public static func decode<Answer: Decodable>(_ type: Answer.Type, from data: Data) throws -> Answer {
        try JSONDecoder.wire.decode(type, from: data)
    }
}
