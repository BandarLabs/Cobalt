import AppKit
import CobaltKit
import SwiftUI

/// A place's content: one column of reading on paper, scrolling when it is
/// longer than the window.
struct Pane<Content: View>: View {
    /// How wide the column may run. Reading wants it narrow; a grid of
    /// pictures can use more.
    var width: CGFloat = Paper.column
    @ViewBuilder var content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                content
            }
            .frame(maxWidth: width, alignment: .leading)
            .padding(.horizontal, 28)
            .padding(.top, 22)
            .padding(.bottom, 32)
            // Every column starts at the same left edge whatever its width,
            // so nothing moves sideways when the owner changes place.
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Paper.ground)
    }
}

/// A state with nothing in it but what is true and the one thing to do.
/// It starts where every other place starts, at the top of the column, so
/// the window does not rearrange itself when a reader appears.
struct Standstill<Actions: View>: View {
    var headline: String
    var detail: String
    var working = false
    @ViewBuilder var actions: Actions

    var body: some View {
        Pane {
            VStack(alignment: .leading, spacing: 10) {
                Text(headline)
                    .font(Paper.serif(24))
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)
                if !detail.isEmpty {
                    Prose(detail)
                        .foregroundStyle(.secondary)
                }
            }
            if working {
                ProgressView()
                    .controlSize(.small)
            }
            HStack(spacing: 10) { actions }
        }
    }
}

/// A sentence with any web address in it made a link that opens in the
/// owner's browser.
struct Prose: View {
    var text: String
    var font: Font

    init(_ text: String, font: Font = .body) {
        self.text = text
        self.font = font
    }

    var body: some View {
        Text(Self.attributed(text))
            .font(font)
            .tint(Paper.link)
            .lineSpacing(2)
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
    }

    static func attributed(_ text: String) -> AttributedString {
        var whole = AttributedString()
        for part in Said.linked(text) {
            var piece = AttributedString(part.text)
            if let link = part.link {
                piece.link = link
                piece.underlineStyle = .single
            }
            whole.append(piece)
        }
        return whole
    }
}

/// A line set apart from the page: something to know, or something that
/// went wrong, with at most one thing to do about it.
struct Note<Action: View>: View {
    enum Kind { case plain, caution, done }

    var kind: Kind
    var text: String
    var detail: String
    @ViewBuilder var action: Action

    init(_ text: String, detail: String = "", kind: Kind = .plain, @ViewBuilder action: () -> Action) {
        self.kind = kind
        self.text = text
        self.detail = detail
        self.action = action()
    }

    private var symbol: String {
        switch kind {
        case .plain: return "info.circle"
        case .caution: return "exclamationmark.triangle"
        case .done: return "checkmark.circle"
        }
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: symbol)
                .font(.body.weight(.medium))
                .foregroundStyle(kind == .caution ? .primary : .secondary)
                .frame(width: 18)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) {
                Prose(text, font: kind == .caution ? .body.weight(.medium) : .body)
                if !detail.isEmpty {
                    Prose(detail)
                        .foregroundStyle(.secondary)
                }
                action
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(Paper.inset, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .accessibilityElement(children: .combine)
    }
}

extension Note where Action == EmptyView {
    init(_ text: String, detail: String = "", kind: Kind = .plain) {
        self.init(text, detail: detail, kind: kind) { EmptyView() }
    }
}

/// What a command said, as headings, paragraphs, steps, facts and the odd
/// command line. The words are the command's and are set as reading.
struct SaidView: View {
    var blocks: [Block]

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { index, block in
                row(block)
                    .padding(.top, index > 0 && Self.endsAList(blocks[index - 1], before: block) ? 7 : 0)
            }
        }
    }

    /// Whether one block closes a list that the next does not carry on, so
    /// the eye is given the gap a new paragraph would have had on paper.
    private static func endsAList(_ previous: Block, before next: Block) -> Bool {
        func listed(_ block: Block) -> Bool {
            switch block {
            case .step, .item, .fact, .code: return true
            default: return false
            }
        }
        return listed(previous) && !listed(next)
    }

    @ViewBuilder
    private func row(_ block: Block) -> some View {
        switch block {
        case .heading(let text):
            Text(text)
                .font(.headline)
                .padding(.top, 4)
                .accessibilityAddTraits(.isHeader)
        case .paragraph(let text):
            Prose(text, font: Paper.serif(15))
        case .step(let number, let text):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("\(number).")
                    .font(Paper.serif(15))
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                    .frame(width: 22, alignment: .trailing)
                Prose(text, font: Paper.serif(15))
            }
        case .item(let text):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("•")
                    .font(Paper.serif(15))
                    .foregroundStyle(.secondary)
                    .frame(width: 22, alignment: .trailing)
                    .accessibilityHidden(true)
                Prose(text, font: Paper.serif(15))
            }
        case .fact(let name, let value):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(name)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(width: 76, alignment: .leading)
                Text(value)
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            .accessibilityElement(children: .combine)
        case .error(let text):
            Prose(Said.sentence(text), font: Paper.serif(15, .semibold))
        case .code(let text):
            Text(text)
                .font(.system(.callout, design: .monospaced))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Paper.inset, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
        }
    }
}

/// Something worth keeping that nobody needs to read unless they want to:
/// the full report under the steps, or everything a failed command said.
struct Folded<Content: View>: View {
    var title: String
    @State private var open = false
    @ViewBuilder var content: Content

    var body: some View {
        DisclosureGroup(isExpanded: $open) {
            content
                .padding(.top, 8)
                .padding(.leading, 2)
                .frame(maxWidth: .infinity, alignment: .leading)
        } label: {
            Text(title)
                .font(.callout)
                .foregroundStyle(.secondary)
        }
    }
}

/// A heading inside a place.
struct Heading: View {
    var text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .font(.headline)
            .accessibilityAddTraits(.isHeader)
    }
}

/// The reader, drawn as plainly as it can be and still be that reader.
///
/// Not an illustration of a product: a few lines that say which of two
/// shapes it is, whether a cable is in it, and what is on its screen as far
/// as this computer knows. A reader that is not there is drawn in dashes.
struct ReaderDrawing: View {
    enum Screen: Equatable {
        case blank
        /// The stock reader, showing a page.
        case page
        case cobalt(String)
        /// The question the reader asks when it is plugged in.
        case connect
    }

    var model: String
    var screen: Screen
    var present = true
    var scale: CGFloat = 1

    /// The readers with page-turn buttons have a wide grip down one side
    /// and are nearly square. The rest are a plain slab.
    private var hasGrip: Bool {
        ["Libra", "Sage", "Forma", "Aura One", "Oasis"].contains { model.contains($0) }
    }

    var body: some View {
        let width: CGFloat = (hasGrip ? 68 : 58) * scale
        let height: CGFloat = 80 * scale
        let cable: CGFloat = 12 * scale
        VStack(spacing: 0) {
            ZStack(alignment: .topLeading) {
                RoundedRectangle(cornerRadius: 6 * scale, style: .continuous)
                    .strokeBorder(style: StrokeStyle(lineWidth: 1.25, dash: present ? [] : [3, 3]))
                RoundedRectangle(cornerRadius: 1.5 * scale)
                    .strokeBorder(style: StrokeStyle(lineWidth: 0.75, dash: present ? [] : [2, 2]))
                    .frame(width: 46 * scale, height: 62 * scale)
                    .overlay { face }
                    .offset(x: 6 * scale, y: 7 * scale)
                if hasGrip {
                    VStack(spacing: 5 * scale) {
                        Capsule().frame(width: 2 * scale, height: 11 * scale)
                        Capsule().frame(width: 2 * scale, height: 11 * scale)
                    }
                    .opacity(present ? 0.8 : 0.35)
                    .offset(x: 58 * scale, y: 25 * scale)
                }
            }
            .frame(width: width, height: height)
            // The cable is only there when the reader is.
            Rectangle()
                .frame(width: 1.25, height: cable)
                .opacity(present ? 1 : 0)
        }
        .foregroundStyle(.primary.opacity(present ? 0.85 : 0.4))
        .accessibilityHidden(true)
    }

    @ViewBuilder
    private var face: some View {
        switch screen {
        case .blank:
            EmptyView()
        case .page:
            VStack(alignment: .leading, spacing: 3.5 * scale) {
                ForEach(0..<7, id: \.self) { line in
                    Capsule()
                        .frame(width: (line == 6 ? 18 : line == 2 ? 30 : 34) * scale, height: 1)
                }
            }
            .opacity(0.45)
        case .cobalt(let version):
            VStack(spacing: 1 * scale) {
                Text("Cobalt")
                    .font(Paper.serif(max(9.5 * scale, 7.5), .medium))
                // Drawn small, the version would be a smudge, and the row
                // it sits in says it in words.
                if scale >= 0.9 {
                    Text(version)
                        .font(.system(size: 7 * scale))
                        .monospacedDigit()
                        .opacity(0.7)
                }
            }
        case .connect:
            VStack(spacing: 4 * scale) {
                Capsule().frame(width: 26 * scale, height: 1).opacity(0.45)
                Capsule().frame(width: 20 * scale, height: 1).opacity(0.45)
                HStack(spacing: 3 * scale) {
                    RoundedRectangle(cornerRadius: 1.5 * scale)
                        .strokeBorder(lineWidth: 0.75)
                        .frame(width: 15 * scale, height: 8 * scale)
                        .opacity(0.5)
                    RoundedRectangle(cornerRadius: 1.5 * scale)
                        .frame(width: 15 * scale, height: 8 * scale)
                }
                .padding(.top, 2 * scale)
            }
        }
    }
}

/// Something to press that reads as part of a sentence: a way to another
/// place, in the one blue.
struct Pointer: View {
    var title: String
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .foregroundStyle(Paper.link)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(.isLink)
    }
}

extension View {
    /// The one filled button in a place.
    func leading() -> some View {
        buttonStyle(.borderedProminent)
            .tint(Paper.accent)
    }
}
