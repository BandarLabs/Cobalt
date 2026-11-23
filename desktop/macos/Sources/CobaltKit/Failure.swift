import Foundation

/// The word the server puts beside a refusal, saying what kind it was.
///
/// The sentence that comes with it is for the owner and is shown as it is.
/// The word is for the app, which does different things after different
/// refusals: a session that is not accepted ends the window's usefulness, a
/// reader that has gone means the list of readers is stale, and so on.
public enum RefusalKind: String, Sendable, CaseIterable {
    case session
    case gone
    case busy
    case refused
    case noCobalt = "no_cobalt"
    case readOnly = "read_only"
    case noRoom = "no_room"
    case unreachable
    case integrity
    case writeFailed = "write_failed"
    case notFound = "not_found"
    case unsupportedFile = "unsupported_file"
    case tooLarge = "too_large"
    case badRequest = "bad_request"
    case failed

    /// A newer server may name a kind this build has not heard of. Whatever
    /// it means, the request did not happen and the sentence says why, which
    /// is all "failed" promises.
    init(named: String?, status: Int) {
        if let named, let known = RefusalKind(rawValue: named) {
            self = known
        } else if named == nil, status == 401 || status == 403 {
            self = .session
        } else {
            self = .failed
        }
    }
}

/// Every way a request can come back without an answer.
public enum Failure: Error, Equatable, Sendable {
    /// The server answered, and the answer was no.
    case refused(kind: RefusalKind, sentence: String, status: Int)
    /// Nothing is listening at the address: the kobo command has stopped.
    case unreachable
    /// Something is listening and did not answer in the time allowed.
    case timedOut
    /// An answer arrived that is not the JSON this build expects.
    case malformed

    public var kind: RefusalKind? {
        if case .refused(let kind, _, _) = self { return kind }
        return nil
    }

    /// Whether the session itself is over, as opposed to one request.
    public var endsTheSession: Bool { kind == .session }

    /// What to show the owner. A refusal carries the server's own sentence;
    /// the rest never reached anything that could write one.
    public var sentence: String {
        switch self {
        case .refused(_, let sentence, _):
            return sentence
        case .unreachable:
            return "This window could not reach the kobo command behind it."
        case .timedOut:
            return "The kobo command did not answer in time."
        case .malformed:
            return "The kobo command answered with something this window does not understand."
        }
    }
}

/// The body of a refusal, as the server writes it.
struct RefusalBody: Decodable {
    var error: String?
    var kind: String?
}
