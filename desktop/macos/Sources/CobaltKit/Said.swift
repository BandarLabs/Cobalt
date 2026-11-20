import Foundation

/// One piece of what a command printed, once its sentences are whole again.
public enum Block: Equatable, Sendable {
    /// A line that introduces what follows.
    case heading(String)
    case paragraph(String)
    /// A numbered instruction, keeping the number the command gave it.
    case step(Int, String)
    case item(String)
    /// A "Name: value" line under a heading.
    case fact(name: String, value: String)
    /// What the command said as it failed, without its own name in front.
    case error(String)
    /// A command or a path on a line of its own.
    case code(String)

    /// The words in the block, whichever kind it is.
    public var text: String {
        switch self {
        case .heading(let text), .paragraph(let text), .item(let text), .error(let text), .code(let text):
            return text
        case .step(_, let text):
            return text
        case .fact(_, let value):
            return value
        }
    }
}

/// Reading what the commands print.
///
/// The browser page makes the same decisions in logic.mjs, and the two are
/// held to the same examples so that one owner never reads two versions of
/// what a command said. A change here belongs there too.
public enum Said {
    /// The narrowest the commands wrap their own prose. Their sentences are
    /// broken by hand so that the next word would have passed about 76
    /// columns; a line that stops with room to spare for it stopped because
    /// it was finished.
    static let wrapAtLeast = 72

    private enum Kind { case heading, paragraph, step, item, fact, error, code }

    private struct Draft {
        var kind: Kind
        var text: String
        var number = 0
        var name = ""
        var indent = 0

        var block: Block {
            switch kind {
            case .heading: return .heading(text)
            case .paragraph: return .paragraph(text)
            case .step: return .step(number, text)
            case .item: return .item(text)
            case .fact: return .fact(name: name, value: text)
            case .error: return .error(text)
            case .code: return .code(text)
            }
        }
    }

    /// Puts hard-wrapped output back into paragraphs and lists.
    ///
    /// The command line wraps its sentences at eighty columns and indents
    /// what continues a list item. A window wraps wherever it needs to, so
    /// text shown as the terminal printed it breaks twice: once where the
    /// terminal broke it and again where the window does. This joins the
    /// sentences and keeps the structure the indentation was expressing, so
    /// a list is still a list.
    public static func reflow(_ text: String) -> [Block] {
        var drafts: [Draft] = []
        // Whether the last block is still being added to. A blank line ends
        // it, which is the only thing that does.
        var open = false
        // How long the line before this one was, to tell a line the terminal
        // wrapped from one the command ended on purpose.
        var before = 0

        func push(_ draft: Draft) {
            drafts.append(draft)
            open = true
        }

        for raw in text.replacingOccurrences(of: "\r", with: "").split(separator: "\n", omittingEmptySubsequences: false) {
            let line = trimmedEnd(raw)
            let body = line.trimmingCharacters(in: .whitespaces)
            let indent = line.utf16.count - trimmedStart(line).utf16.count
            let previous = before
            before = line.utf16.count

            // A row of dots is the terminal's way of showing it is waiting.
            // The window has its own.
            if body.isEmpty || body.allSatisfy({ $0 == "." }) {
                open = false
                continue
            }

            if indent == 0, body.hasPrefix("kobo: ") {
                push(Draft(kind: .error, text: String(body.dropFirst(6))))
                continue
            }
            if indent > 0, let (number, rest) = numbered(body) {
                push(Draft(kind: .step, text: rest, number: number, indent: indent))
                continue
            }
            if let rest = bulleted(body) {
                push(Draft(kind: .item, text: rest, indent: indent))
                continue
            }

            if open, let last = drafts.indices.last {
                // Indented under a list entry, further in than its marker:
                // the entry carrying on.
                if drafts[last].kind == .step || drafts[last].kind == .item, indent > drafts[last].indent {
                    drafts[last].text += " \(body)"
                    continue
                }
                // An error that ran onto an indented second line.
                if drafts[last].kind == .error, indent > 0 {
                    drafts[last].text += " \(body)"
                    continue
                }
                if indent > 0 {
                    if drafts[last].kind == .heading || drafts[last].kind == .fact, let (name, value) = labelled(body) {
                        push(Draft(kind: .fact, text: value, name: name))
                        continue
                    }
                    // An indented line straight after a sentence that ends in
                    // a colon is the command or path that sentence was
                    // introducing, and so is the line after that.
                    let introduced = (drafts[last].kind == .paragraph && drafts[last].text.hasSuffix(":")) || drafts[last].kind == .code
                    if introduced {
                        push(Draft(kind: .code, text: body))
                        continue
                    }
                    // A line taken for a heading that turns out to introduce
                    // plain lines, not a list, was a sentence all along.
                    if drafts[last].kind == .heading {
                        drafts[last].kind = .paragraph
                        drafts[last].text += ":"
                        push(Draft(kind: .code, text: body))
                        continue
                    }
                }
            }

            // A heading is short, stands alone, and is one phrase. A sentence
            // that ends in a colon after a full stop is prose that happens to
            // end there.
            if indent == 0, !open, body.hasSuffix(":"), body.utf16.count <= 60, !endsASentenceInside(body) {
                push(Draft(kind: .heading, text: String(body.dropLast())))
                continue
            }

            // A paragraph carries on only where the terminal had to break it.
            // If this line's first word would have fitted on the end of the
            // last one, nothing forced the break, so the command meant a new
            // line: a second statement, or a labelled line such as
            // "sample: ...".
            let firstWord = body.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: false).first.map { $0.utf16.count } ?? 0
            let wrapped = previous + 1 + firstWord > wrapAtLeast
            if open, wrapped, let last = drafts.indices.last, drafts[last].kind == .paragraph {
                drafts[last].text += " \(body)"
                continue
            }
            push(Draft(kind: .paragraph, text: body))
        }
        return drafts.map(\.block)
    }

    /// What a setup said, divided at the point where it turns from what it
    /// did to what the owner does next.
    ///
    /// Setup reports everything it changed, at length, and then gives the
    /// steps for the reader. The report is worth keeping and the steps are
    /// what somebody is waiting to be told, so the window leads with the
    /// steps and folds the report away. Nothing is reordered within either
    /// part. Output with no such turn, which is every command but an install,
    /// is all one part.
    public static func afterwards(_ blocks: [Block]) -> (report: [Block], next: [Block]) {
        let turn = blocks.firstIndex { block in
            guard case .heading(let text) = block else { return false }
            return text == "Next" || (text.hasPrefix("Next") && !isWordCharacter(text[text.index(text.startIndex, offsetBy: 4)]))
        }
        guard let turn else { return ([], blocks) }
        return (Array(blocks[..<turn]), Array(blocks[turn...]))
    }

    /// The line a failed command ended on, which is the reason it failed, or
    /// nothing when it never said one.
    public static func failure(_ text: String) -> String {
        for block in reflow(text).reversed() {
            if case .error(let reason) = block { return reason }
        }
        return ""
    }

    /// A sentence as it should begin and end in a window. The commands write
    /// some of their reasons to follow a colon, in lower case and
    /// unpunctuated.
    public static func sentence(_ text: String?) -> String {
        let trimmed = (text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first = trimmed.first else { return "" }
        let started = first.uppercased() + trimmed.dropFirst()
        guard let last = started.last else { return "" }
        return ".!?:".contains(last) ? started : "\(started)."
    }

    /// One line of the plan as an instruction.
    ///
    /// The dry run writes each change as "would do this", which reads well
    /// down a terminal and badly as a list under a heading that already says
    /// these are the things that would happen. The word is dropped and
    /// nothing else is touched: the plan is setup's, and the window is not
    /// where it gets reworded.
    public static func planStep(_ line: String?) -> String {
        var text = (line ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasPrefix("would ") { text.removeFirst(6) }
        guard let first = text.first else { return "" }
        return first.uppercased() + text.dropFirst()
    }

    /// A stretch of a sentence: plain words, or a web address to open.
    public struct Part: Equatable, Sendable {
        public var text: String
        public var link: URL?

        public init(text: String, link: URL? = nil) {
            self.text = text
            self.link = link
        }
    }

    /// A sentence split around the web addresses in it, so each can be made
    /// a link and the rest left as text.
    ///
    /// Only https addresses with a host after the scheme, so that a sentence
    /// which merely mentions "https://" is left alone, and the full stop
    /// that ends the sentence is not taken for part of the address.
    public static func linked(_ text: String?) -> [Part] {
        var parts: [Part] = []
        var plain = ""
        var rest = Substring(text ?? "")
        let scheme = "https://"
        while let found = rest.range(of: scheme) {
            let afterScheme = rest[found.upperBound...]
            let end = afterScheme.firstIndex { $0.isWhitespace || "<>\"')".contains($0) } ?? rest.endIndex
            var address = rest[found.lowerBound..<end]
            while let last = address.last, ".,;:".contains(last) { address.removeLast() }
            guard let first = afterScheme.first, first.isASCII, first.isLetter || first.isNumber,
                  let link = URL(string: String(address))
            else {
                plain += rest[..<found.upperBound]
                rest = afterScheme
                continue
            }
            plain += rest[..<found.lowerBound]
            if !plain.isEmpty {
                parts.append(Part(text: plain))
                plain = ""
            }
            var shown = address.dropFirst(scheme.count)
            if shown.hasSuffix("/") { shown.removeLast() }
            parts.append(Part(text: String(shown), link: link))
            rest = rest[address.endIndex...]
        }
        plain += rest
        if !plain.isEmpty { parts.append(Part(text: plain)) }
        return parts
    }

    /// How long something has been going, to the precision anybody wants
    /// while waiting for it.
    public static func elapsed(_ seconds: TimeInterval) -> String {
        let whole = max(0, Int(seconds.rounded(.down)))
        if whole < 60 { return "\(whole) s" }
        let minutes = whole / 60
        return whole % 60 == 0 ? "\(minutes) min" : "\(minutes) min \(whole % 60) s"
    }

    // The small readers the rules above are written in.

    private static func trimmedEnd(_ line: Substring) -> String {
        var line = line
        while let last = line.last, last.isWhitespace { line.removeLast() }
        return String(line)
    }

    private static func trimmedStart(_ line: String) -> Substring {
        line.drop { $0.isWhitespace }
    }

    /// "3. Join it to Wi-Fi" as its number and its words.
    private static func numbered(_ body: String) -> (Int, String)? {
        let digits = body.prefix { $0.isASCII && $0.isNumber }
        guard !digits.isEmpty, let number = Int(digits) else { return nil }
        let rest = body[digits.endIndex...]
        guard rest.first == "." else { return nil }
        let afterStop = rest.dropFirst()
        guard let gap = afterStop.first, gap.isWhitespace else { return nil }
        return (number, String(afterStop.drop { $0.isWhitespace }))
    }

    private static func bulleted(_ body: String) -> String? {
        guard let mark = body.first, mark == "·" || mark == "•" else { return nil }
        let rest = body.dropFirst()
        guard let gap = rest.first, gap.isWhitespace else { return nil }
        return String(rest.drop { $0.isWhitespace })
    }

    /// "Firmware: 4.45" as its name and value, when the name is a short
    /// capitalised label and not the start of a sentence with a colon in it.
    private static func labelled(_ body: String) -> (String, String)? {
        guard let colon = body.range(of: ": ") else { return nil }
        let name = body[..<colon.lowerBound]
        let value = body[colon.upperBound...]
        guard let first = name.first, first.isASCII, first.isUppercase, name.count <= 25, !value.isEmpty else { return nil }
        guard name.dropFirst().allSatisfy({ $0.isASCII && ($0.isLetter || $0 == " ") }) else { return nil }
        return (String(name), String(value))
    }

    private static func endsASentenceInside(_ body: String) -> Bool {
        body.contains(". ") || body.contains("! ") || body.contains("? ")
    }

    private static func isWordCharacter(_ character: Character) -> Bool {
        character.isASCII && (character.isLetter || character.isNumber || character == "_")
    }
}
