import Foundation
import Testing
@testable import CobaltKit

// The examples here are the browser page's own, from tools/kobo-ui.test.mjs,
// so that the same input gives the same words in both front ends. An example
// changed in one belongs in the other.

/// What `kobo setup` prints when it has finished, as the terminal wraps it.
let setupSaid = """
found Kobo Clara BW (N365, profile clara-bw-391, device code 391) at /Volumes/KOBOeReader · firmware 4.45.23697

Ready to install Cobalt:
  Model: Kobo Clara BW (device code 391, profile clara-bw-391)
  Firmware: 4.45.23697
  Mount: /Volumes/KOBOeReader
  Release: Cobalt 0.3.37 (stable)

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

Cobalt itself is started from the Cobalt entry in the reader's own menu, or
from .adds/cobalt-launch.sh. Starting it stops the reader and takes the
screen; a restart always returns to the stock reader.

Waiting up to 5 minutes for the reader to come back on 192.168.1.0/24. Ctrl-C to stop.
.....
The reader is back on Wi-Fi at 192.168.1.20.

"""

@Suite("What a command printed")
struct SaidTests {
    @Test("What a command printed is put back into sentences and keeps its lists")
    func reflowKeepsStructure() {
        let blocks = Said.reflow(setupSaid)
        #expect(Array(blocks[0..<6]) == [
            .paragraph("found Kobo Clara BW (N365, profile clara-bw-391, device code 391) at /Volumes/KOBOeReader · firmware 4.45.23697"),
            .heading("Ready to install Cobalt"),
            .fact(name: "Model", value: "Kobo Clara BW (device code 391, profile clara-bw-391)"),
            .fact(name: "Firmware", value: "4.45.23697"),
            .fact(name: "Mount", value: "/Volumes/KOBOeReader"),
            .fact(name: "Release", value: "Cobalt 0.3.37 (stable)"),
        ])
        #expect(Array(blocks[6..<10]) == [
            .heading("Set up /Volumes/KOBOeReader"),
            .item("212 files installed into .adds/cobalt"),
            .item("the firmware's SSH server is enabled, and starts at the next restart"),
            .item("volume ejected"),
        ])
        #expect(Array(blocks[10..<14]) == [
            .heading("Next, on the reader"),
            .step(1, "After the host has safely ejected it, disconnect the USB cable."),
            .step(2, "It restarts by itself: ejecting leaves the archive where the firmware looks, so it shows its Updating screen, takes it, and reboots. The SSH server starts with that boot."),
            .step(3, "Join it to Wi-Fi if it is not already."),
        ])
        #expect(blocks[14].text == "Cobalt itself is started from the Cobalt entry in the reader's own menu, or from .adds/cobalt-launch.sh. Starting it stops the reader and takes the screen; a restart always returns to the stock reader.")
    }

    @Test("The terminal's row of waiting dots is left out and the lines around it are kept apart")
    func dotsAreLeftOut() {
        let blocks = Said.reflow(setupSaid)
        #expect(!blocks.contains { $0.text.allSatisfy { $0 == "." } })
        #expect(Array(blocks.suffix(2)) == [
            .paragraph("Waiting up to 5 minutes for the reader to come back on 192.168.1.0/24. Ctrl-C to stop."),
            .paragraph("The reader is back on Wi-Fi at 192.168.1.20."),
        ])
        #expect(Said.reflow("...\n").isEmpty)
    }

    @Test("What setup said is divided where it turns to what the owner does next")
    func afterwardsDividesAtNext() {
        let all = Said.reflow(setupSaid)
        let (report, next) = Said.afterwards(all)
        #expect(report.count == 10)
        #expect(report.last?.text == "volume ejected")
        #expect(next.first == .heading("Next, on the reader"))
        if case .step = next[1] {} else { Issue.record("the steps come straight after their heading") }
        #expect(next.last?.text == "The reader is back on Wi-Fi at 192.168.1.20.")
        // Nothing is lost or moved: the two parts are the whole, in order.
        #expect(report + next == all)
        // A command with no such turn is all one part, shown as it came.
        let sent = Said.reflow("preparing Winter.cbz for Panels\nSent Winter.cbz to Panels.\n")
        let whole = Said.afterwards(sent)
        #expect(whole.report.isEmpty)
        #expect(whole.next == sent)
        // A heading that merely starts with the same letters is not the turn.
        #expect(Said.afterwards([.heading("Nextcloud folders")]).report.isEmpty)
        #expect(Said.afterwards([.paragraph("x"), .heading("Next")]).report == [.paragraph("x")])
    }

    @Test("A command named on a line of its own after a colon is kept as a command")
    func commandsAreKept() {
        #expect(Said.reflow("It could not be saved on this computer. Save it with:\n  kobo pair\nThen try again.\n") == [
            .paragraph("It could not be saved on this computer. Save it with:"),
            .code("kobo pair"),
            .paragraph("Then try again."),
        ])
    }

    @Test("The reason a command failed is its last error line, without the command's name")
    func failureIsTheLastError() {
        let said = "preparing Winter.cbz for Panels\nkobo: No reader is saved yet.\n  Run kobo setup with the reader plugged in.\n"
        #expect(Said.failure(said) == "No reader is saved yet. Run kobo setup with the reader plugged in.")
        #expect(Said.failure("first\nkobo: one\nmore\nkobo: two\n") == "two")
        #expect(Said.failure("Sent Winter.cbz to Panels.\n") == "")
        // Only at the start of a line is it the command speaking.
        #expect(Said.failure("  kobo: indented is part of something else\n") == "")
    }

    @Test("A line the terminal wrapped is joined and a line the command ended is not")
    func wrappedLinesAreJoined() {
        // The first break is where the next word would not fit in eighty
        // columns. The second comes after a short line, so nothing forced
        // it. These are setup's own closing lines, as it wraps them.
        let said =
            "exact Cobalt NickelMenu entry. It moves owner folders to\n" +
            ".adds/cobalt.recovery.N first and leaves .adds/cobalt.unusable[.N] quarantines\n" +
            "for inspection, so recoverable data is never silently deleted.\n" +
            "sample: Welcome from Cobalt.txt is in the library\n"
        #expect(Said.reflow(said) == [
            .paragraph("exact Cobalt NickelMenu entry. It moves owner folders to .adds/cobalt.recovery.N first and leaves .adds/cobalt.unusable[.N] quarantines for inspection, so recoverable data is never silently deleted."),
            .paragraph("sample: Welcome from Cobalt.txt is in the library"),
        ])
        #expect(Said.reflow("The reader is back on Wi-Fi at 192.168.1.20.\nSaved as kobo.\r\n\nthree\n") == [
            .paragraph("The reader is back on Wi-Fi at 192.168.1.20."),
            .paragraph("Saved as kobo."),
            .paragraph("three"),
        ])
        #expect(Said.reflow("").isEmpty)
        // A long sentence that happens to end in a colon is not a heading.
        let long = String(repeating: "word ", count: 14) + "ends here:"
        #expect(Said.reflow(long) == [.paragraph(long)])
        // Nor is a "Name: value" line with no heading above it a fact.
        #expect(Said.reflow("  Model: Clara\n") == [.paragraph("Model: Clara")])
    }

    @Test("A sentence that introduces paths is a sentence, and each path keeps its own line")
    func pathsKeepTheirLines() {
        #expect(Said.reflow("Owner data was preserved for manual backup or deletion in:\n  .adds/cobalt.recovery.1\n  .adds/cobalt.recovery.2\n\nA key this tool staged is taken back.\n") == [
            .paragraph("Owner data was preserved for manual backup or deletion in:"),
            .code(".adds/cobalt.recovery.1"),
            .code(".adds/cobalt.recovery.2"),
            .paragraph("A key this tool staged is taken back."),
        ])
    }

    @Test("A line of the plan loses its would and nothing else")
    func planStepDropsWould() {
        #expect(Said.planStep("would install Cobalt into /Volumes/KOBOeReader/.adds/cobalt") == "Install Cobalt into /Volumes/KOBOeReader/.adds/cobalt")
        #expect(Said.planStep("nothing outside the book partition") == "Nothing outside the book partition")
        #expect(Said.planStep("would eject, then wait; it would not guess") == "Eject, then wait; it would not guess")
        #expect(Said.planStep("") == "")
        #expect(Said.planStep(nil) == "")
    }

    @Test("A web address inside a sentence becomes a link and the sentence keeps its full stop")
    func addressesBecomeLinks() {
        #expect(Said.linked("Install it from https://bandarlabs.github.io/Cobalt/ and open this window again.") == [
            Said.Part(text: "Install it from "),
            Said.Part(text: "bandarlabs.github.io/Cobalt", link: URL(string: "https://bandarlabs.github.io/Cobalt/")),
            Said.Part(text: " and open this window again."),
        ])
        #expect(Said.linked("The browser installer does the same job: https://bandarlabs.github.io/Cobalt/install/.") == [
            Said.Part(text: "The browser installer does the same job: "),
            Said.Part(text: "bandarlabs.github.io/Cobalt/install", link: URL(string: "https://bandarlabs.github.io/Cobalt/install/")),
            Said.Part(text: "."),
        ])
        #expect(Said.linked("No address here.") == [Said.Part(text: "No address here.")])
        // The scheme named in a sentence is not an address.
        let advice = "Enter a web address that starts with http:// or https://."
        #expect(Said.linked(advice) == [Said.Part(text: advice)])
        // Nothing but https is ever made into something to click.
        #expect(Said.linked("see http://example.org or javascript:alert(1)") == [Said.Part(text: "see http://example.org or javascript:alert(1)")])
        #expect(Said.linked("").isEmpty)
        #expect(Said.linked(nil).isEmpty)
    }

    @Test("A reason written to follow a colon is given a capital and a full stop")
    func reasonsBecomeSentences() {
        #expect(Said.sentence("firmware 5.2 cannot start Cobalt: NickelMenu does not hook it") == "Firmware 5.2 cannot start Cobalt: NickelMenu does not hook it.")
        #expect(Said.sentence("Already a sentence.") == "Already a sentence.")
        #expect(Said.sentence("  ") == "")
        #expect(Said.sentence(nil) == "")
    }

    @Test("A wait is counted in seconds until it is long enough to count in minutes")
    func elapsedReadsNaturally() {
        #expect(Said.elapsed(0) == "0 s")
        #expect(Said.elapsed(59.9) == "59 s")
        #expect(Said.elapsed(60) == "1 min")
        #expect(Said.elapsed(125) == "2 min 5 s")
        #expect(Said.elapsed(-4) == "0 s")
    }
}
