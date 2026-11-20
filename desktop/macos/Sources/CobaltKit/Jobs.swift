import Foundation

/// How a failed job can be started again without going back through the
/// steps that led to it.
public enum Again: Equatable, Sendable {
    case install(volume: String)
    case remove(volume: String)
    case apps(volume: String, ids: [String])
}

/// A job the window is following: what the server last said about it, and
/// what the window has kept from every time it asked.
public struct FollowedJob: Equatable, Identifiable, Sendable {
    public var id: Int
    public var kind: JobKind
    public var title: String
    /// Everything it has printed, joined from each poll's share.
    public var text = ""
    /// The offset to ask from next.
    public var next = 0
    public var running = true
    public var ok: Bool?
    public var code: Int?
    public var volume: String?
    public var began: Date
    /// When it last printed anything, which is what "still going" is
    /// measured from.
    public var lastHeard: Date
    public var again: Again?

    public init(id: Int, kind: JobKind, title: String, volume: String?, began: Date, again: Again? = nil) {
        self.id = id
        self.kind = kind
        self.title = title
        self.volume = volume
        self.began = began
        self.lastHeard = began
        self.again = again
    }

    public var failed: Bool { !running && ok != true }
}

/// A reader job as the Reader place shows it: going, done, or failed with
/// its reason and where that leaves the reader.
public struct JobScene: Equatable, Sendable {
    public enum Phase: Equatable, Sendable { case running, done, failed }

    public var phase: Phase
    public var heading: String
    /// Why it failed, as a sentence.
    public var reason = ""
    /// What the reader has on it after the failure.
    public var aftermath = ""
    /// What to expect, shown until the job has something to show.
    public var waiting = ""
    /// Said when the job has printed nothing for longer than is usual.
    public var quiet = ""
    /// Said when the reader went away under a job that was not done with it.
    public var pulled = ""
    /// The part the owner acts on: the steps for the reader, or all of it
    /// when the command had no such turn.
    public var next: [Block] = []
    /// What was done to the reader, folded away under the steps.
    public var report: [Block] = []
    public var elapsed = ""
    /// Where an install has got to, as a short list that advances. Empty
    /// for any job that is not an install.
    public var stages: [Stage] = []
    /// Everything the command has said, for whoever wants all of it.
    public var said: [Block] = []
    /// Whether the command has turned to the steps for the reader.
    public var turned = false

    public struct Stage: Equatable, Sendable {
        public enum State: Equatable, Sendable { case done, current, waiting }
        public var title: String
        public var state: State
    }

    /// The four things an install does, and the line setup prints when
    /// each is behind it. An install is a minute or two of output that
    /// nobody waiting needs to read; which of four things is happening is
    /// what they want to know. The lines are setup's own headings, so a
    /// release that rewords one leaves a stage showing as not yet done
    /// and loses nothing, since everything it said is still there to open.
    private static let installStages: [(title: String, done: String)] = [
        ("Reading the reader", "Ready to install Cobalt"),
        ("Writing Cobalt to the reader", "\nSet up "),
        ("Ejecting the reader", "volume ejected"),
        ("Waiting for the reader on Wi-Fi", "The reader is back on Wi-Fi"),
    ]

    static func stages(of job: FollowedJob) -> [Stage] {
        guard job.title.hasPrefix("Installing Cobalt") || job.title.hasPrefix("Updating Cobalt") else { return [] }
        var reached = true
        return installStages.map { stage in
            let done = job.text.contains(stage.done)
            defer { if !done { reached = false } }
            return Stage(title: stage.title, state: done ? .done : reached ? .current : .waiting)
        }
    }
    /// The reader the job wrote to, when it is still mounted and so still
    /// needs ejecting before the cable comes out.
    public var connected: Reader?

    /// How long a job may print nothing before the window says so, and how
    /// long before it says something is wrong. Setup's own longest wait is
    /// five minutes for the reader to come back on Wi-Fi, and it prints
    /// while it waits.
    static let quietAfter: TimeInterval = 120
    static let stuckAfter: TimeInterval = 600

    private static let done = [
        "Installing Cobalt": "Cobalt is installed.",
        "Installing Cobalt again": "Cobalt is installed again.",
        "Updating Cobalt": "Cobalt is updated.",
        "Removing Cobalt": "Cobalt is removed.",
    ]

    private static func isTurn(_ block: Block?) -> Bool {
        guard case .heading(let text)? = block else { return false }
        return text.hasPrefix("Next")
    }

    public static func scene(for job: FollowedJob, state: ServerState?, now: Date) -> JobScene {
        let reader = state?.readers.first { $0.volume == job.volume }
        if job.failed {
            var scene = JobScene(phase: .failed, heading: "\(job.title.isEmpty ? "The work" : job.title) did not finish.")
            let reason = Said.sentence(Said.failure(job.text))
            scene.reason = reason.isEmpty ? "The command stopped without saying why." : reason
            scene.aftermath = Advice.afterFailure(volume: job.volume, state: state)
            scene.next = Said.reflow(job.text)
            return scene
        }
        let all = Said.reflow(job.text)
        let (report, next) = Said.afterwards(all)
        if !job.running {
            var scene = JobScene(phase: .done, heading: done[job.title] ?? "Done.")
            scene.next = next
            scene.report = report
            scene.connected = reader
            return scene
        }
        var scene = JobScene(phase: .running, heading: job.title.isEmpty ? "Working" : job.title)
        scene.next = next
        scene.report = report
        scene.said = all
        scene.turned = !report.isEmpty || isTurn(next.first)
        scene.stages = stages(of: job)
        scene.elapsed = Said.elapsed(now.timeIntervalSince(job.began))
        // Until the command turns to the steps for the reader there is
        // nothing for the owner to do but leave it alone, so that is said.
        if report.isEmpty {
            scene.waiting = "Leave the reader plugged in."
        }
        let silence = now.timeIntervalSince(job.lastHeard)
        if silence >= stuckAfter {
            scene.quiet = "Nothing new for \(Said.elapsed(silence)), which is longer than any step should take. Quitting Cobalt does not stop it. To stop it, end the kobo process in Activity Monitor, then look at the reader here before trying again."
        } else if silence >= quietAfter {
            scene.quiet = "Nothing new for \(Said.elapsed(silence)). It is still running."
        }
        // An install ejects the reader itself near the end and then waits
        // for it on Wi-Fi, so a reader that has gone is only news when the
        // job had not got that far.
        if job.volume != nil, state != nil, reader == nil, report.isEmpty, !job.text.contains("ejected") {
            scene.pulled = "The reader was disconnected while this was running. Wait for the command to stop; it says how far it got."
        }
        return scene
    }
}
