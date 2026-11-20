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
        case .stopped: return "The kobo command behind this window has stopped."
        case .refused: return "The kobo command no longer accepts this window."
        case .missing: return "Cobalt cannot find the kobo command."
        case .exited: return "The kobo command stopped as soon as it started."
        case .silent(let seconds): return "The kobo command did not answer within \(seconds) seconds."
        case .garbled: return "The kobo command answered in a way this window does not understand."
        }
    }

    public var detail: String {
        switch reason {
        case .stopped:
            return "Anything that had finished is still done. Start it again to carry on from here."
        case .refused:
            return "Each window is given a key when it opens, and this one's is no longer good. Start again to get a new one."
        case .missing(let path):
            return "It is expected beside this app, at \(path). Install Cobalt again from https://bandarlabs.github.io/Cobalt/ so the two are together, or run kobo ui in a terminal."
        case .exited(let said):
            let reason = Said.failure(said)
            if !reason.isEmpty { return Said.sentence(reason) }
            let last = said.split(separator: "\n").last.map(String.init) ?? ""
            return last.isEmpty ? "It did not say why. Run kobo ui in a terminal to see what it prints." : Said.sentence(last)
        case .silent:
            return "It was started and has said nothing. Start again; if this keeps happening, run kobo ui in a terminal to see what it prints."
        case .garbled:
            return "The two are probably from different releases. Run kobo update in a terminal, then open Cobalt again."
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
            return "\(app.map { "Cobalt \($0)" } ?? "This window") cannot work with kobo \(kobo): they no longer ask and answer in the same way. Run kobo update in a terminal, then open Cobalt again."
        case .release(let app, let kobo):
            return "This window is Cobalt \(app) and the kobo command is \(kobo). They are installed together and are meant to match. Run kobo update in a terminal, then open Cobalt again."
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
            detail: "A drive is connected at \(volume), and macOS has not allowed it to be opened. In System Settings, go to Privacy & Security, then Files and Folders, and turn on Removable Volumes for \(entry). This window carries on by itself once it can read the drive.",
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
            detail: "Files and articles reach the reader over Wi-Fi, and \"No route to host\" is what macOS reports when it has not allowed that. In System Settings, go to Privacy & Security, then Local Network, and turn it on for \(who). If it is already on, the reader is on a different network from this computer.",
            button: "Open Local Network Settings",
            settings: URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")!
        )
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
    public static let readOnly = "This reader's storage is mounted read-only, so nothing can be installed on it or removed from it. Eject it, unplug it, restart the reader and connect it again. If it comes back read-only, its storage needs repairing from the reader's own settings."
}
