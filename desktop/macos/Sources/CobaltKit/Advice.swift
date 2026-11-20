import Foundation

// What the places say, apart from how they draw it.
//
// Each function here is a function of its arguments and nothing else, and
// each has a twin in the browser page's logic.mjs that is held to the same
// examples. The two front ends are read by the same people on different
// computers and must not disagree about what is true of a reader.

/// Which reader the Reader and Apps places are about.
public enum Chosen: Equatable, Sendable {
    case none
    /// `among` holds the others when there was a choice to make, so the
    /// place can offer to choose again.
    case one(Reader, among: [Reader])
    case several([Reader])

    public var reader: Reader? {
        if case .one(let reader, _) = self { return reader }
        return nil
    }
}

/// Why no reader is showing, when none is.
public enum Absence: Equatable, Sendable {
    /// At least one reader is mounted, so nothing is absent.
    case present
    /// A drive that looks like a reader is mounted and the system will not
    /// let the kobo command read it.
    case blocked(volume: String)
    /// A Kobo is on the USB bus with no drive, so Connect has not been
    /// tapped on its screen.
    case unconnected
    /// Nothing is plugged in, as far as can be told.
    case none
}

/// What the Reader place says about one reader and offers to do with it.
public struct ReaderAdvice: Equatable, Sendable {
    public enum Primary: Equatable, Sendable { case install, update }

    /// The one sentence that is true of this reader now.
    public var headline: String
    /// What follows from it, when something does.
    public var detail: String
    /// The one thing to do next, when there is one.
    public var primary: Primary?
    /// Whether installing again is offered as a lesser action.
    public var repair: Bool
    /// Whether Cobalt can be taken off.
    public var removable: Bool
}

public enum Advice {
    /// One reader needs no choosing. With several, the one already chosen
    /// stays chosen for as long as it is plugged in, and nothing is chosen
    /// for the owner: installing onto the wrong one of two readers is not a
    /// mistake to make on their behalf.
    public static func chosen(among readers: [Reader], volume: String?) -> Chosen {
        guard let first = readers.first else { return .none }
        if readers.count == 1 { return .one(first, among: []) }
        if let chosen = readers.first(where: { $0.volume == volume }) { return .one(chosen, among: readers) }
        return .several(readers)
    }

    /// "No reader" covers three different things, and the advice for one is
    /// wrong for the others: telling somebody to plug in a reader that is
    /// plugged in and waiting for a tap, or one the system is keeping this
    /// program from reading, sends them to the cable when the answer is on
    /// the reader's screen or in System Settings.
    public static func absence(_ state: ServerState?) -> Absence {
        let mounted = state?.readers.count ?? 0
        if mounted > 0 { return .present }
        if let volume = state?.blocked.first { return .blocked(volume: volume) }
        if let attached = state?.attached, attached > mounted { return .unconnected }
        return .none
    }

    public static func standing(of reader: Reader, on computer: Computer?) -> ReaderAdvice {
        let release = computer?.release
        let noRelease = computer?.releaseProblem ?? ""
        let on = reader.cobalt?.cobaltVersion

        if let problem = reader.cobaltProblem, !problem.isEmpty {
            return ReaderAdvice(
                headline: "The Cobalt on this reader is damaged.",
                detail: problem,
                primary: release != nil && reader.supported ? .install : nil,
                repair: false,
                removable: true
            )
        }
        guard let on else {
            if !reader.supported {
                return ReaderAdvice(
                    headline: "Cobalt cannot be installed on this reader.",
                    detail: Said.sentence(reader.refusal),
                    primary: nil,
                    repair: false,
                    removable: false
                )
            }
            if let release {
                return ReaderAdvice(
                    headline: "Cobalt is not on this reader yet.",
                    detail: "This computer has Cobalt \(release) ready to install.",
                    primary: .install,
                    repair: false,
                    removable: false
                )
            }
            return ReaderAdvice(headline: "Cobalt is not on this reader yet.", detail: noRelease, primary: nil, repair: false, removable: false)
        }
        switch reader.standing {
        case .behind:
            return ReaderAdvice(
                headline: "This reader has Cobalt \(on).",
                detail: "This computer has \(release ?? ""), which is newer.",
                primary: reader.supported ? .update : nil,
                repair: false,
                removable: true
            )
        case .ahead:
            return ReaderAdvice(
                headline: "This reader has Cobalt \(on).",
                detail: "That is newer than the \(release ?? "") on this computer. Run kobo update in a terminal before changing this reader's Cobalt from here.",
                primary: nil,
                repair: false,
                removable: true
            )
        case .current:
            return ReaderAdvice(
                headline: "This reader has Cobalt \(on).",
                detail: "That is the release on this computer. The reader also updates itself over Wi-Fi.",
                primary: nil,
                repair: reader.supported,
                removable: true
            )
        case .absent, .unknown:
            return ReaderAdvice(headline: "This reader has Cobalt \(on).", detail: noRelease, primary: nil, repair: false, removable: true)
        }
    }

    /// What is on the reader after a job that did not finish, said so that
    /// the owner knows where they stand before deciding what to do next.
    ///
    /// A failed install does not leave a half-installed Cobalt: setup writes
    /// beside the old copy and swaps, so the reader has what it had or what
    /// was being installed. Which of the two is read from the reader, not
    /// assumed. A reader that is no longer mounted cannot be read, and that
    /// is said too, because pulling the cable is the commonest reason a job
    /// fails.
    public static func afterFailure(volume: String?, state: ServerState?) -> String {
        guard let volume else { return "" }
        guard let reader = state?.readers.first(where: { $0.volume == volume }) else {
            return "The reader is no longer connected, so what is on it cannot be read. Plug it in again and tap Connect; this window then says what it has."
        }
        if let problem = reader.cobaltProblem, !problem.isEmpty {
            return "The reader is still connected. The Cobalt on it is damaged; installing again replaces it."
        }
        if let cobalt = reader.cobalt {
            return "The reader is still connected and has Cobalt \(cobalt.cobaltVersion) on it."
        }
        return "The reader is still connected and has no Cobalt on it."
    }

    /// Why an app that is offered cannot be installed on this reader as it
    /// stands, or nothing when it can.
    public static func obstacle(to app: AppEntry, on reader: Reader?) -> String {
        if !app.runsOnThisReader { return "Needs Cobalt \(app.minimumCobaltVersion ?? "") or newer." }
        if app.fits == false {
            let free = reader?.freeBytes.map { "; the reader has \(fileSize($0)) free" } ?? ""
            return "Needs \(fileSize(app.bytes))\(free)."
        }
        return ""
    }

    /// A size as a person reads it.
    public static func fileSize(_ bytes: Int64?) -> String {
        guard let bytes, bytes >= 0 else { return "" }
        if bytes < 1000 { return "\(bytes) bytes" }
        let units = ["KB", "MB", "GB"]
        var value = Double(bytes) / 1000
        var unit = 0
        while value >= 1000, unit < units.count - 1 {
            value /= 1000
            unit += 1
        }
        let shown = value >= 100 ? String(Int(value.rounded(.toNearestOrAwayFromZero))) : String(format: "%.1f", value)
        return "\(shown) \(units[unit])"
    }

    /// Why a file cannot be handed over, or nothing when it can. Asked
    /// before the bytes are sent, because the server refuses an oversized
    /// file by closing the connection, and that arrives as a broken
    /// connection with no reason attached.
    public static func uploadRefusal(name: String?, size: Int64, limit: Int64?) -> String {
        guard let name else { return "Choose a file to send." }
        if size == 0 { return "\(name) is empty, so there is nothing to send." }
        if let limit, size > limit {
            return "\(name) is \(fileSize(size)). The most this window sends is \(fileSize(limit)); use kobo send in a terminal for larger files."
        }
        return ""
    }

    /// Whether what was typed can be sent as a web address, and the address
    /// to send. The server checks again; this is so the owner is told before
    /// a request is made, in the field they typed in.
    public static func webAddress(_ typed: String?) -> (address: String, problem: String) {
        let address = (typed ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if address.isEmpty { return ("", "") }
        let lowered = address.lowercased()
        let scheme = lowered.hasPrefix("https://") ? 8 : lowered.hasPrefix("http://") ? 7 : 0
        let rest = address.dropFirst(scheme)
        if scheme == 0 || rest.isEmpty || address.contains(where: \.isWhitespace) {
            return ("", "Enter a web address that starts with http:// or https://.")
        }
        return (address, "")
    }

    /// The one step a browser still needs, taken apart for display.
    public struct BrowserStep: Equatable, Sendable {
        public var instruction: String
        public var paths: [String]
        public var notes: [String]
    }

    /// The command writes the step as "Chrome: do this and pick" followed by
    /// indented lines: the folder to pick, and sometimes a caution. The
    /// browser's name is already the row's heading, a folder is something to
    /// copy, and a caution is a sentence, so each is handed back as what it
    /// is.
    public static func browserStep(_ step: String?, title: String) -> BrowserStep {
        let lines = (step ?? "").split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        guard let head = lines.first else { return BrowserStep(instruction: "", paths: [], notes: []) }
        let prefix = "\(title): "
        let first = head.hasPrefix(prefix) ? String(head.dropFirst(prefix.count)) : head
        var paths: [String] = []
        var notes: [String] = []
        for line in lines.dropFirst() {
            if isPath(line) { paths.append(line) } else { notes.append(Said.sentence(line)) }
        }
        // An instruction that goes on to name a folder ends in "and pick";
        // what it picks is shown underneath, so the sentence is closed with
        // a colon.
        var instruction = Said.sentence(first)
        if !paths.isEmpty {
            if instruction.hasSuffix(".") { instruction.removeLast() }
            instruction += ":"
        }
        return BrowserStep(instruction: instruction, paths: paths, notes: notes)
    }

    /// The job to show in a place when the window opens: the one of that
    /// kind still running, or failing that nothing. A finished job is not
    /// brought back, since what it did is already reflected in everything
    /// else the window shows.
    public static func runningJob(_ jobs: [JobSummary], kind: JobKind) -> JobSummary? {
        jobs.first { $0.kind == kind && $0.running }
    }

    private static func isPath(_ line: String) -> Bool {
        if line.hasPrefix("/") || line.hasPrefix("~/") { return true }
        let start = Array(line.prefix(3))
        return start.count == 3 && start[0].isASCII && start[0].isLetter && start[1] == ":" && start[2] == "\\"
    }
}

/// The four places, in the order the sidebar lists them.
public enum Place: String, CaseIterable, Identifiable, Sendable {
    case reader, apps, browser, send

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .reader: return "Reader"
        case .apps: return "Apps"
        case .browser: return "Browser"
        case .send: return "Send"
        }
    }

    /// The place a saved name stands for. Anything unrecognised is Reader,
    /// which is where the window opens.
    public init(named: String?) {
        self = Place(rawValue: named ?? "") ?? .reader
    }
}
