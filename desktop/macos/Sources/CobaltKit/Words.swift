import Foundation

// The sentences for the states no server can describe, because in each of
// them the server is the thing that is missing, mismatched or kept away.

/// How the app came to be running, which decides a few true sentences: who
/// macOS holds responsible for reading the reader, and whether a terminal
/// is also showing what a job prints.
public enum Origin: Equatable, Sendable {
    /// `kobo ui` in a terminal started the app.
    case terminal
    /// The app was opened by itself and started the kobo command.
    case finder
}

/// Why the window has nothing behind it.
public struct Lost: Error, Equatable, Sendable {
    public enum Reason: Equatable, Sendable {
        /// The server stopped answering.
        case stopped
        /// The server answers and no longer accepts this app's token.
        case refused
        /// There is no kobo command beside the app.
        case missing(path: String)
        /// The kobo command stopped before it had a server to offer.
        case exited(said: String)
        /// The kobo command was started and said nothing in the time given.
        case silent(seconds: Int)
        /// What came back was not something this build understands.
        case garbled
    }

    public var reason: Reason

    public init(_ reason: Reason) {
        self.reason = reason
    }

    public var headline: String {
        switch reason {
        case .stopped: return "The kobo command has stopped."
        case .refused: return "The kobo command no longer accepts this window."
        case .missing: return "Cobalt cannot find the kobo command."
        case .exited: return "The kobo command stopped as soon as it started."
        case .silent(let seconds): return "The kobo command did not answer within \(seconds) seconds."
        case .garbled: return "The kobo command answered in a way this window does not understand."
        }
    }

    /// What goes under the headline: the reason where there is one, or
    /// the one thing to do that the button beneath does not already do.
    /// Where the headline and the button say it all, nothing.
    public var detail: String {
        switch reason {
        case .stopped, .refused, .silent:
            return ""
        case .missing:
            // Where it was looked for is not said. An owner cannot put it
            // there, and the one thing that can is the installer.
            return "Install Cobalt again from https://bandarlabs.github.io/Cobalt/."
        case .exited(let said):
            let reason = Said.failure(said)
            if !reason.isEmpty { return Said.sentence(reason) }
            let last = said.split(separator: "\n").last.map(String.init) ?? ""
            return last.isEmpty ? "It did not say why." : Said.sentence(last)
        case .garbled:
            return "Run kobo update in a terminal, then open Cobalt again."
        }
    }
}

/// The app and the kobo command disagreeing about what they are.
public enum VersionTrouble: Equatable, Sendable {
    /// They do not speak the same contract, so nothing can be trusted to
    /// mean what the app thinks it means and nothing is offered.
    case contract(app: String?, kobo: String)
    /// Same contract, different releases. Things work; it is still worth
    /// saying, because the two are installed together and one was left
    /// behind.
    case release(app: String, kobo: String)

    public var blocks: Bool {
        if case .contract = self { return true }
        return false
    }

    public var headline: String {
        "This window and the kobo command are from different releases."
    }

    public var detail: String {
        switch self {
        case .contract(let app, let kobo):
            return "\(app.map { "This window is Cobalt \($0)" } ?? "This window is a different release") and the kobo command is \(kobo). Run kobo update in a terminal, then open Cobalt again."
        case .release(let app, let kobo):
            return "This window is Cobalt \(app) and the kobo command is \(kobo). Run kobo update in a terminal, then open Cobalt again."
        }
    }
}

/// What to say when macOS is the thing in the way, and where the switch is.
public enum Privacy {
    public struct Advice: Equatable, Sendable {
        public var headline: String
        public var detail: String
        public var button: String
        /// The pane of System Settings the switch is in.
        public var settings: URL
    }

    /// A drive that looks like a reader is mounted and the system refused
    /// to let it be read.
    ///
    /// macOS asks once and remembers the answer, and it holds responsible
    /// whichever app is at the top of the chain that started the command:
    /// Cobalt when Cobalt started it, the terminal when `kobo ui` did. The
    /// switch is listed under that app's name, so the sentence has to name
    /// the right one or it sends the owner to a list their app is not in.
    public static func blocked(volume: String, origin: Origin) -> Advice {
        let who = origin == .finder ? "Cobalt" : "the terminal that started Cobalt"
        let entry = origin == .finder ? "Cobalt" : "that terminal"
        return Advice(
            headline: "macOS is keeping \(who) from reading this reader.",
            detail: "In System Settings, go to Privacy & Security, then Files and Folders, and turn on Removable Volumes for \(entry).",
            button: "Open Privacy & Security",
            settings: URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders")!
        )
    }

    /// Whether a command's output reads like the local network being
    /// denied. macOS gives a program it is keeping off the network the same
    /// error as a network with no path to the address, so this is a
    /// likelihood and is worded as one.
    public static func looksKeptOffNetwork(_ said: String) -> Bool {
        said.contains("No route to host") || said.contains("os error 65")
    }

    public static func localNetwork(origin: Origin) -> Advice {
        let who = origin == .finder ? "Cobalt" : "the terminal that started Cobalt"
        return Advice(
            headline: "macOS may be keeping \(who) off the local network.",
            detail: "In System Settings, go to Privacy & Security, then Local Network, and turn it on for \(who). If it is already on, the reader is on a different network from this computer.",
            button: "Open Local Network Settings",
            settings: URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")!
        )
    }
}

/// The words of the first install, written for someone who has never used
/// Cobalt and will not open a terminal. Nothing here is lifted from what
/// the command prints: its paths and the names of the parts it installs are
/// for the disclosure beside these, not for the page.
public enum FirstRun {
    /// What an install or a removal does, in a few lines.
    public static func plan(undo: Bool) -> [String] {
        undo
            ? [
                "Cobalt and its entry in the reader's menu are taken off.",
                "The settings Cobalt changed are put back.",
                "Your books are left as they are, and so is anything you made in Cobalt's apps.",
            ]
            : [
                "Cobalt is copied onto the reader's storage, beside your books.",
                "An entry named Cobalt is added to the reader's own menu.",
                "This computer, and no other, is allowed to manage the reader over Wi-Fi.",
                "The reader is set to stay awake and on Wi-Fi for longer, so it can be reached.",
                "Your books and the reader's own software are left as they are.",
            ]
    }

    /// Said before an install on a Mac, so the system's question is not a
    /// surprise and is answered the way that lets the reader be found.
    public static let localNetworkAhead = "After the install, macOS asks whether Cobalt may find devices on your local network. Allow it: that is how the reader is found."

    /// Why a Kobo that is plugged in still offers no storage after Connect
    /// has had time to be tapped. There are two real causes and both are
    /// said, because the owner is the only one who can tell which.
    public static let connectCauses = [
        "The cable may only charge. Use the one that came with the reader, plugged straight into the computer.",
        "A new Kobo has to finish its own setup on its screen first: language, Wi-Fi and signing in. No computer can see it until that is done.",
    ]

    /// What the owner does with the reader once Cobalt is on it.
    public static func onTheReader(menuCorner: String?) -> [String] {
        [
            "Unplug the reader. It restarts and shows Updating for about a minute.",
            "Join it to Wi-Fi if it is not on it already.",
            "Open the menu at the \(menuCorner ?? "bottom right") of its screen and choose Cobalt.",
        ]
    }

    /// Why the reader was not found, most likely first.
    public static func notFoundCauses(_ stage: SetupStage) -> [String] {
        switch stage {
        case .several:
            return ["Put the others to sleep and look again."]
        case .noNetwork:
            return ["Join the Wi-Fi the reader is on and look again."]
        default:
            return [
                "The reader may be asleep. Wake it and choose Cobalt from its menu.",
                "It may be on a different Wi-Fi network from this computer, or on a guest network that keeps devices apart.",
                "macOS may not have allowed Cobalt onto the local network. Allow it in System Settings, under Privacy & Security, Local Network.",
            ]
        }
    }
}

/// What is asked before the window closes on work that is not finished.
public struct QuitQuestion: Equatable, Sendable {
    public var title: String
    public var detail: String
    public var stay: String
    public var leave: String
}

/// What to say about a reader whose storage will not take a write.
public enum Storage {
    /// The server refuses a write to such a card with its own sentence.
    /// This is for before anything is tried, so the owner is not offered a
    /// button that can only fail.
    /// The remedy is the server's own, word for word, so the owner reads
    /// one piece of advice whichever of the two says it.
    public static let readOnly = "This reader's storage is mounted read-only, so nothing can be written to it. Eject it, unplug it, restart the reader and connect it again."
}
