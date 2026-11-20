import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PLACES,
  absence,
  afterFailure,
  afterwards,
  appObstacle,
  appSections,
  browserStep,
  chosenReader,
  elapsed,
  failure,
  fileSize,
  linked,
  placeFor,
  planStep,
  readerStanding,
  reflow,
  runningJob,
  sentence,
  uploadRefusal,
  webAddress,
} from "../crates/kobo-cli/src/ui/page/logic.mjs";

const page = join(dirname(fileURLToPath(import.meta.url)), "../crates/kobo-cli/src/ui/page");
const markup = readFileSync(join(page, "index.html"), "utf8");
const script = readFileSync(join(page, "app.js"), "utf8");
const style = readFileSync(join(page, "app.css"), "utf8");

// What `kobo setup` prints when it has finished, as the terminal wraps it.
const SETUP_SAID = `found Kobo Clara BW (N365, profile clara-bw-391, device code 391) at /Volumes/KOBOeReader · firmware 4.45.23697

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
`;

test("what a command printed is put back into sentences and keeps its lists", () => {
  const blocks = reflow(SETUP_SAID);
  assert.deepEqual(blocks.slice(0, 6), [
    { kind: "para", text: "found Kobo Clara BW (N365, profile clara-bw-391, device code 391) at /Volumes/KOBOeReader · firmware 4.45.23697" },
    { kind: "heading", text: "Ready to install Cobalt" },
    { kind: "fact", name: "Model", value: "Kobo Clara BW (device code 391, profile clara-bw-391)" },
    { kind: "fact", name: "Firmware", value: "4.45.23697" },
    { kind: "fact", name: "Mount", value: "/Volumes/KOBOeReader" },
    { kind: "fact", name: "Release", value: "Cobalt 0.3.37 (stable)" },
  ]);
  assert.deepEqual(blocks.slice(6, 10), [
    { kind: "heading", text: "Set up /Volumes/KOBOeReader" },
    { kind: "item", text: "212 files installed into .adds/cobalt" },
    { kind: "item", text: "the firmware's SSH server is enabled, and starts at the next restart" },
    { kind: "item", text: "volume ejected" },
  ]);
  assert.deepEqual(blocks.slice(10, 14), [
    { kind: "heading", text: "Next, on the reader" },
    { kind: "step", number: 1, text: "After the host has safely ejected it, disconnect the USB cable." },
    {
      kind: "step",
      number: 2,
      text: "It restarts by itself: ejecting leaves the archive where the firmware looks, so it shows its Updating screen, takes it, and reboots. The SSH server starts with that boot.",
    },
    { kind: "step", number: 3, text: "Join it to Wi-Fi if it is not already." },
  ]);
  assert.equal(
    blocks[14].text,
    "Cobalt itself is started from the Cobalt entry in the reader's own menu, or from .adds/cobalt-launch.sh. Starting it stops the reader and takes the screen; a restart always returns to the stock reader."
  );
});

test("the terminal's row of waiting dots is left out and the lines around it are kept apart", () => {
  const blocks = reflow(SETUP_SAID);
  assert.ok(!blocks.some(block => /^\.+$/.test(block.text ?? "")));
  assert.deepEqual(blocks.slice(-2), [
    { kind: "para", text: "Waiting up to 5 minutes for the reader to come back on 192.168.1.0/24. Ctrl-C to stop." },
    { kind: "para", text: "The reader is back on Wi-Fi at 192.168.1.20." },
  ]);
  assert.deepEqual(reflow("...\n"), []);
});

test("what setup said is divided where it turns to what the owner does next", () => {
  const { report, next } = afterwards(reflow(SETUP_SAID));
  assert.equal(report.length, 10);
  assert.equal(report[report.length - 1].text, "volume ejected");
  assert.deepEqual(next[0], { kind: "heading", text: "Next, on the reader" });
  assert.equal(next[1].kind, "step");
  assert.equal(next[next.length - 1].text, "The reader is back on Wi-Fi at 192.168.1.20.");
  // Nothing is lost or moved: the two parts are the whole, in order.
  assert.deepEqual([...report, ...next], reflow(SETUP_SAID));
  // A command with no such turn is all one part, shown as it came.
  const sent = reflow("preparing Winter.cbz for Panels\nSent Winter.cbz to Panels.\n");
  assert.deepEqual(afterwards(sent), { report: [], next: sent });
});

test("a command named on a line of its own after a colon is kept as a command", () => {
  assert.deepEqual(
    reflow("It could not be saved on this computer. Save it with:\n  kobo pair\nThen try again.\n"),
    [
      { kind: "para", text: "It could not be saved on this computer. Save it with:" },
      { kind: "code", text: "kobo pair" },
      { kind: "para", text: "Then try again." },
    ]
  );
});

test("the reason a command failed is its last error line, without the command's name", () => {
  const said = "preparing Winter.cbz for Panels\nkobo: No reader is saved yet.\n  Run kobo setup with the reader plugged in.\n";
  assert.equal(failure(said), "No reader is saved yet. Run kobo setup with the reader plugged in.");
  assert.equal(failure("first\nkobo: one\nmore\nkobo: two\n"), "two");
  assert.equal(failure("Sent Winter.cbz to Panels.\n"), "");
  // Only at the start of a line is it the command speaking.
  assert.equal(failure("  kobo: indented is part of something else\n"), "");
});

test("a line the terminal wrapped is joined and a line the command ended is not", () => {
  // The first break is where the next word would not fit in eighty columns.
  // The second comes after a short line, so nothing forced it.
  // These are setup's own closing lines, as it wraps them.
  const said =
    "exact Cobalt NickelMenu entry. It moves owner folders to\n" +
    ".adds/cobalt.recovery.N first and leaves .adds/cobalt.unusable[.N] quarantines\n" +
    "for inspection, so recoverable data is never silently deleted.\n" +
    "sample: Welcome from Cobalt.txt is in the library\n";
  assert.deepEqual(reflow(said), [
    { kind: "para", text: "exact Cobalt NickelMenu entry. It moves owner folders to .adds/cobalt.recovery.N first and leaves .adds/cobalt.unusable[.N] quarantines for inspection, so recoverable data is never silently deleted." },
    { kind: "para", text: "sample: Welcome from Cobalt.txt is in the library" },
  ]);
  assert.deepEqual(reflow("The reader is back on Wi-Fi at 192.168.1.20.\nSaved as kobo.\r\n\nthree\n"), [
    { kind: "para", text: "The reader is back on Wi-Fi at 192.168.1.20." },
    { kind: "para", text: "Saved as kobo." },
    { kind: "para", text: "three" },
  ]);
  assert.deepEqual(reflow(""), []);
  // A long sentence that happens to end in a colon is not a heading.
  const long = `${"word ".repeat(14)}ends here:`;
  assert.equal(reflow(long)[0].kind, "para");
  // Nor is a "Name: value" line with no heading above it a fact.
  assert.equal(reflow("  Model: Clara\n")[0].kind, "para");
});

test("a sentence that introduces paths is a sentence, and each path keeps its own line", () => {
  assert.deepEqual(
    reflow("Owner data was preserved for manual backup or deletion in:\n  .adds/cobalt.recovery.1\n  .adds/cobalt.recovery.2\n\nA key this tool staged is taken back.\n"),
    [
      { kind: "para", text: "Owner data was preserved for manual backup or deletion in:" },
      { kind: "code", text: ".adds/cobalt.recovery.1" },
      { kind: "code", text: ".adds/cobalt.recovery.2" },
      { kind: "para", text: "A key this tool staged is taken back." },
    ]
  );
});

test("a line of the plan loses its would and nothing else", () => {
  assert.equal(planStep("would install Cobalt into /Volumes/KOBOeReader/.adds/cobalt"), "Install Cobalt into /Volumes/KOBOeReader/.adds/cobalt");
  assert.equal(planStep("nothing outside the book partition"), "Nothing outside the book partition");
  assert.equal(planStep("would eject, then wait; it would not guess"), "Eject, then wait; it would not guess");
  assert.equal(planStep(""), "");
});

test("a web address inside a sentence becomes a link and the sentence keeps its full stop", () => {
  assert.deepEqual(linked("Install it from https://bandarlabs.github.io/Cobalt/ and open this window again."), [
    { text: "Install it from " },
    { href: "https://bandarlabs.github.io/Cobalt/", text: "bandarlabs.github.io/Cobalt" },
    { text: " and open this window again." },
  ]);
  assert.deepEqual(linked("The browser installer does the same job: https://bandarlabs.github.io/Cobalt/install/."), [
    { text: "The browser installer does the same job: " },
    { href: "https://bandarlabs.github.io/Cobalt/install/", text: "bandarlabs.github.io/Cobalt/install" },
    { text: "." },
  ]);
  assert.deepEqual(linked("No address here."), [{ text: "No address here." }]);
  // The scheme named in a sentence is not an address.
  const advice = "Enter a web address that starts with http:// or https://.";
  assert.deepEqual(linked(advice), [{ text: advice }]);
  // Nothing but https is ever made into something to click.
  assert.deepEqual(linked("see http://example.org or javascript:alert(1)"), [{ text: "see http://example.org or javascript:alert(1)" }]);
  assert.deepEqual(linked(""), []);
});

test("a reason written to follow a colon is given a capital and a full stop", () => {
  assert.equal(
    sentence("firmware 5.2 cannot start Cobalt: NickelMenu does not hook it"),
    "Firmware 5.2 cannot start Cobalt: NickelMenu does not hook it."
  );
  assert.equal(sentence("Already a sentence."), "Already a sentence.");
  assert.equal(sentence("  "), "");
  assert.equal(sentence(null), "");
});

const clara = { volume: "/Volumes/KOBOeReader", model: "Kobo Clara BW", supported: true, standing: "absent", cobalt: null };
const libra = { volume: "/Volumes/KOBOeReader 1", model: "Kobo Libra 2", supported: true, standing: "absent", cobalt: null };

test("one reader needs no choosing and two are never chosen between for the owner", () => {
  assert.deepEqual(chosenReader([], null), { kind: "none" });
  assert.deepEqual(chosenReader(undefined, null), { kind: "none" });
  assert.deepEqual(chosenReader([clara], null), { kind: "one", reader: clara });
  assert.deepEqual(chosenReader([clara, libra], null), { kind: "several", readers: [clara, libra] });
  assert.deepEqual(chosenReader([clara, libra], libra.volume), { kind: "one", reader: libra, among: [clara, libra] });
  // The chosen one was unplugged: back to asking, not on to the other.
  assert.deepEqual(chosenReader([clara, libra], "/Volumes/Gone"), { kind: "several", readers: [clara, libra] });
  // A choice made among two is not held against the one that is left.
  assert.deepEqual(chosenReader([clara], libra.volume), { kind: "one", reader: clara });
});

test("no reader showing is told apart: nothing plugged in, Connect not tapped, or kept from us", () => {
  assert.deepEqual(absence({ readers: [], attached: 0, blocked: [] }), { kind: "none" });
  assert.deepEqual(absence({ readers: [], attached: null, blocked: [] }), { kind: "none" });
  assert.deepEqual(absence({ readers: [] }), { kind: "none" });
  assert.deepEqual(absence(null), { kind: "none" });
  assert.deepEqual(absence({ readers: [], attached: 1, blocked: [] }), { kind: "unconnected" });
  assert.deepEqual(absence({ readers: [], attached: 1, blocked: ["/Volumes/KOBOeReader"] }), { kind: "blocked", volume: "/Volumes/KOBOeReader" });
  // A reader that is mounted is a reader, whatever else is on the bus.
  assert.deepEqual(absence({ readers: [clara], attached: 2, blocked: [] }), { kind: "present" });
});

test("after a job fails the owner is told what the reader has, or that it cannot be read", () => {
  const job = { volume: clara.volume, running: false, ok: false };
  assert.match(afterFailure(job, { readers: [] }), /no longer connected/);
  assert.equal(afterFailure(job, { readers: [clara] }), "The reader is still connected and has no Cobalt on it.");
  assert.equal(
    afterFailure(job, { readers: [{ ...clara, cobalt: { cobalt_version: "0.3.30", channel: "stable" } }] }),
    "The reader is still connected and has Cobalt 0.3.30 on it."
  );
  assert.match(afterFailure(job, { readers: [{ ...clara, cobalt_problem: "does not say which release it is" }] }), /damaged/);
  // A job that was not about a reader says nothing about one.
  assert.equal(afterFailure({ volume: null }, { readers: [clara] }), "");
});

test("a wait is counted in seconds and then in minutes", () => {
  assert.equal(elapsed(0), "0 s");
  assert.equal(elapsed(59_900), "59 s");
  assert.equal(elapsed(60_000), "1 min");
  assert.equal(elapsed(200_000), "3 min 20 s");
  assert.equal(elapsed(-5), "0 s");
});

test("an app that cannot go on this reader says what stands in the way", () => {
  const reader = { free_bytes: 400_000 };
  assert.equal(appObstacle({ runs_on_this_reader: true, fits: true, bytes: 1 }, reader), "");
  assert.equal(appObstacle({ runs_on_this_reader: true, fits: null, bytes: null }, reader), "");
  assert.equal(appObstacle({ runs_on_this_reader: false, minimum_cobalt_version: "0.4.0", fits: false, bytes: 9 }, reader), "Needs Cobalt 0.4.0 or newer.");
  assert.equal(appObstacle({ runs_on_this_reader: true, fits: false, bytes: 1_900_412 }, reader), "Needs 1.9 MB; the reader has 400 KB free.");
  assert.equal(appObstacle({ runs_on_this_reader: true, fits: false, bytes: 1_900_412 }, {}), "Needs 1.9 MB.");
});

const computer = { release: "0.3.37", release_problem: null };
const without = { release: null, release_problem: "This copy of the kobo command has nothing to put on a reader." };
const withCobalt = (version, standing, more = {}) => ({
  ...clara,
  standing,
  cobalt: { cobalt_version: version, channel: "stable" },
  ...more,
});

test("a reader without Cobalt is offered an install only when there is one to give it", () => {
  assert.deepEqual(readerStanding(clara, computer), { reason: "", primary: "install", repair: false, removable: false });
  assert.deepEqual(readerStanding(clara, without), { reason: without.release_problem, primary: null, repair: false, removable: false });
});

test("a reader Cobalt cannot start on is told why and offered nothing", () => {
  const refused = readerStanding(
    { ...clara, supported: false, refusal: "firmware 5.2.190625 cannot start Cobalt: NickelMenu does not hook the 5.x firmware" },
    computer
  );
  assert.equal(
    refused.reason,
    "Cobalt cannot be installed on this reader. Firmware 5.2.190625 cannot start Cobalt: NickelMenu does not hook the 5.x firmware."
  );
  assert.equal(refused.primary, null);
  assert.equal(refused.repair, false);
  assert.equal(refused.removable, false);
});

test("a reader with Cobalt is offered an update only when this computer's release is newer", () => {
  assert.deepEqual(readerStanding(withCobalt("0.3.30", "behind"), computer), { reason: "", primary: "update", repair: false, removable: true });
  // In order, nothing is said: the table already shows both releases.
  assert.deepEqual(readerStanding(withCobalt("0.3.37", "current"), computer), { reason: "", primary: null, repair: true, removable: true });

  const ahead = readerStanding(withCobalt("0.4.0", "ahead"), computer);
  assert.equal(ahead.primary, null);
  assert.equal(ahead.repair, false);
  assert.match(ahead.reason, /newer than the 0\.3\.37 on this computer/);
  assert.match(ahead.reason, /kobo update/);

  const unknown = readerStanding(withCobalt("0.3.30", "unknown"), without);
  assert.equal(unknown.primary, null);
  assert.equal(unknown.reason, without.release_problem);
  assert.equal(unknown.removable, true);
});

test("Cobalt can always be taken off, even from a reader it can no longer be put on", () => {
  const stranded = readerStanding(withCobalt("0.3.30", "behind", { supported: false, refusal: "firmware 5.2 cannot start Cobalt" }), computer);
  assert.equal(stranded.primary, null);
  assert.equal(stranded.removable, true);
  assert.match(stranded.reason, /^Cobalt cannot be installed on this reader\. Firmware 5\.2/);
  const damaged = readerStanding({ ...clara, cobalt_problem: "The Cobalt on the reader does not say which release it is." }, computer);
  assert.equal(damaged.reason, "The Cobalt on the reader does not say which release it is.");
  assert.equal(damaged.primary, "install");
  assert.equal(damaged.removable, true);
});

const app = (id, title, more = {}) => ({
  id,
  title,
  summary: `The ${title} app.`,
  version: "1.0.0",
  installed_version: null,
  update_available: false,
  runs_on_this_reader: true,
  minimum_cobalt_version: "0.3.0",
  ...more,
});

test("the reader's apps come first, with what the catalog knows about each, then the rest", () => {
  const installed = [app("rss", "Feeds", { installed_version: "1.0.0" }), app("almanac", "Almanac", { installed_version: "1.0.0" })];
  const catalog = [
    app("almanac", "Almanac", { version: "1.2.0", installed_version: "1.0.0", update_available: true }),
    app("rss", "Feeds", { installed_version: "1.0.0" }),
    app("tide-table", "Tide Table"),
    app("backgammon", "Backgammon"),
  ];
  const sections = appSections(installed, catalog, "");
  assert.deepEqual(sections.installed.map(entry => entry.id), ["almanac", "rss"]);
  assert.deepEqual(sections.available.map(entry => entry.id), ["backgammon", "tide-table"]);
  assert.deepEqual(sections.updates, ["almanac"]);
  assert.equal(sections.total, 4);
  assert.equal(sections.installed[0].version, "1.2.0");
  // What the app does is said in the catalog's words where it is in the
  // catalog, and in the reader's own where it is not.
  const blurbed = appSections([app("rss", "Feeds", { installed_version: "1.0.0", summary: "The runtime's line." })], [app("rss", "Feeds", { summary: "Follow feeds." })], "");
  assert.equal(blurbed.installed[0].summary, "Follow feeds.");
  assert.equal(appSections([app("rss", "Feeds", { summary: "The runtime's line." })], null, "").installed[0].summary, "The runtime's line.");
  assert.equal(sections.installed[0].installed_version, "1.0.0");
  assert.equal(sections.installed[0].update_available, true);
  assert.equal(sections.installed[1].update_available, false);
});

test("with no catalog the reader's own apps are still listed and nothing is called an update", () => {
  const installed = [app("rss", "Feeds", { installed_version: "1.0.0", update_available: true })];
  for (const catalog of [null, undefined]) {
    const sections = appSections(installed, catalog, "");
    assert.deepEqual(sections.installed.map(entry => entry.id), ["rss"]);
    assert.equal(sections.installed[0].update_available, false);
    assert.equal(sections.installed[0].in_catalog, false);
    assert.deepEqual(sections.available, []);
    assert.deepEqual(sections.updates, []);
  }
  assert.deepEqual(appSections(null, null, ""), { installed: [], available: [], updates: [], total: 0 });
});

test("an update the reader's Cobalt is too old for is not counted among the updates", () => {
  const installed = [app("almanac", "Almanac", { installed_version: "1.0.0" })];
  const catalog = [app("almanac", "Almanac", { version: "2.0.0", update_available: true, runs_on_this_reader: false, minimum_cobalt_version: "0.4.0" })];
  const sections = appSections(installed, catalog, "");
  assert.deepEqual(sections.updates, []);
  assert.equal(sections.installed[0].runs_on_this_reader, false);
  assert.equal(sections.installed[0].minimum_cobalt_version, "0.4.0");
});

test("the search narrows both lists by name, id or summary and leaves the counts alone", () => {
  const installed = [app("rss", "Feeds", { installed_version: "1.0.0" })];
  const catalog = [app("rss", "Feeds", { installed_version: "1.0.0" }), app("tide-table", "Tide Table"), app("backgammon", "Backgammon", { summary: "A board game." })];
  assert.deepEqual(appSections(installed, catalog, "TIDE").available.map(entry => entry.id), ["tide-table"]);
  assert.deepEqual(appSections(installed, catalog, "rss").installed.map(entry => entry.id), ["rss"]);
  assert.deepEqual(appSections(installed, catalog, "board").available.map(entry => entry.id), ["backgammon"]);
  const none = appSections(installed, catalog, "zzz");
  assert.deepEqual([none.installed, none.available], [[], []]);
  assert.equal(none.total, 3);
});

test("a browser's one step is split into what to do, what to pick and what to know", () => {
  assert.deepEqual(
    browserStep("Chrome: open chrome://extensions, turn on Developer mode, choose Load unpacked and pick\n  /Users/me/.config/kobo/clip/extension", "Chrome"),
    {
      instruction: "Open chrome://extensions, turn on Developer mode, choose Load unpacked and pick:",
      paths: ["/Users/me/.config/kobo/clip/extension"],
      notes: [],
    }
  );
  const firefox = browserStep(
    "Firefox: open about:debugging#/runtime/this-firefox, choose Load Temporary Add-on and pick\n  /home/me/.config/kobo/clip/extension/manifest.json\n  Firefox forgets it when it restarts, so load it again after a restart.",
    "Firefox"
  );
  assert.deepEqual(firefox.paths, ["/home/me/.config/kobo/clip/extension/manifest.json"]);
  assert.deepEqual(firefox.notes, ["Firefox forgets it when it restarts, so load it again after a restart."]);
  const safari = browserStep(
    "Safari: right-click a link and choose Services, then Send to Kobo. For the page you are on, select its address and do the same.\n  A full Safari extension needs a signed Mac app, which this is not.",
    "Safari"
  );
  assert.equal(safari.instruction, "Right-click a link and choose Services, then Send to Kobo. For the page you are on, select its address and do the same.");
  assert.deepEqual(safari.paths, []);
  assert.deepEqual(safari.notes, ["A full Safari extension needs a signed Mac app, which this is not."]);
  assert.deepEqual(browserStep("Edge: open edge://extensions and pick\n  C:\\Users\\me\\.config\\kobo\\clip\\extension", "Edge").paths, [
    "C:\\Users\\me\\.config\\kobo\\clip\\extension",
  ]);
  assert.deepEqual(browserStep("", "Chrome"), { instruction: "", paths: [], notes: [] });
});

test("a size reads the way a person says it", () => {
  assert.equal(fileSize(0), "0 bytes");
  assert.equal(fileSize(999), "999 bytes");
  assert.equal(fileSize(1500), "1.5 KB");
  assert.equal(fileSize(48_300_000), "48.3 MB");
  assert.equal(fileSize(536_870_912), "537 MB");
  assert.equal(fileSize(3_200_000_000), "3.2 GB");
  assert.equal(fileSize(-1), "");
  assert.equal(fileSize("many"), "");
});

test("a file that cannot be sent is refused before any of it is, with the reason", () => {
  const limit = 536_870_912;
  assert.equal(uploadRefusal({ name: "Winter.cbz", size: 48_300_000 }, limit), "");
  assert.equal(uploadRefusal({ name: "Winter.cbz", size: limit }, limit), "");
  assert.equal(uploadRefusal(null, limit), "Choose a file to send.");
  assert.equal(uploadRefusal({ name: "empty.md", size: 0 }, limit), "empty.md is empty, so there is nothing to send.");
  assert.equal(
    uploadRefusal({ name: "Everything.cbz", size: limit + 1 }, limit),
    "Everything.cbz is 537 MB. The most this window sends is 537 MB; use kobo send in a terminal for larger files."
  );
});

test("only something that starts as a web address is sent as one", () => {
  assert.deepEqual(webAddress("  https://example.org/a?b=c  "), { address: "https://example.org/a?b=c", problem: "" });
  assert.deepEqual(webAddress("HTTP://example.org"), { address: "HTTP://example.org", problem: "" });
  assert.deepEqual(webAddress(""), { address: "", problem: "" });
  for (const typed of ["example.org", "file:///etc/passwd", "--out=x", "https://a b", "javascript:alert(1)", "https://"]) {
    const { address, problem } = webAddress(typed);
    assert.equal(address, "", typed);
    assert.match(problem, /starts with http:\/\/ or https:\/\//, typed);
  }
});

test("a reloaded page picks up the job still running in a place and nothing that has finished", () => {
  const jobs = [
    { id: 1, kind: "reader", title: "Installing Cobalt", running: false, ok: true },
    { id: 2, kind: "send", title: "Sending Winter.cbz", running: true, ok: null },
    { id: 3, kind: "reader", title: "Installing apps", running: true, ok: null },
  ];
  assert.equal(runningJob(jobs, "reader").id, 3);
  assert.equal(runningJob(jobs, "send").id, 2);
  assert.equal(runningJob(jobs.slice(0, 1), "reader"), null);
  assert.equal(runningJob(undefined, "reader"), null);
});

test("the page has four places, opens on Reader, and an unknown address is Reader too", () => {
  assert.deepEqual(PLACES, ["reader", "apps", "browser", "send"]);
  assert.equal(placeFor(""), "reader");
  assert.equal(placeFor("#apps"), "apps");
  assert.equal(placeFor("#send"), "send");
  assert.equal(placeFor("#settings"), "reader");
  assert.equal(placeFor(undefined), "reader");
  for (const place of PLACES) {
    assert.ok(markup.includes(`href="#${place}"`), `the page links to ${place}`);
    assert.ok(markup.includes(`id="place-${place}"`), `the page has a section for ${place}`);
  }
});

test("the page never writes text it was given into the document as markup", () => {
  // Everything shown comes from the reader, the catalog or a command's
  // output. All of it goes in as text nodes.
  for (const sink of ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "eval(", "new Function"]) {
    assert.ok(!script.includes(sink), `app.js uses ${sink}`);
  }
});

test("every request the page makes is to its own server and carries the session token when it changes something", () => {
  // The three ways the page asks for anything, and what each is given: a
  // path under /api/ written where it is used, or one passed down from a
  // caller that wrote it the same way.
  const named = [...script.matchAll(/\b(?:ask|change|upload|startSend|startReader)\(\s*(?:"[a-z]+",\s*)?["'`]([^"'`]+)/g)].map(found => found[1]);
  assert.ok(named.length >= 12, `found ${named.length} requests`);
  for (const target of named) assert.ok(target.startsWith("/api/"), target);
  // Nothing else in the script reaches the network: every fetch and every
  // opened request is one of those three, or the goodbye.
  const reached = [...script.matchAll(/\bfetch\(\s*([^,)]+)/g)].map(found => found[1]);
  assert.deepEqual(reached.sort(), ['"/api/goodbye"', "path"]);
  // And every one of them gives up after a stated time instead of waiting.
  assert.match(script, /fetch\(path, \{ \.\.\.options, signal: AbortSignal\.timeout\(patience\) \}\)/);
  assert.deepEqual([...script.matchAll(/\.open\(([^)]*)\)/g)].map(found => found[1]), ['"POST", path']);
  for (const other of ["WebSocket", "EventSource", "sendBeacon", "importScripts", "import("]) {
    assert.ok(!script.includes(other), `app.js uses ${other}`);
  }
  // Both ways of changing something send the token, and so does the goodbye.
  assert.equal(script.match(/"X-Kobo-Token"/g).length, 3);
  assert.match(markup, /<meta name="kobo-token" content="\{\{request-token\}\}">/);
});

test("the page is styled for paper and for night, and for a narrow window", () => {
  assert.match(style, /prefers-color-scheme:\s*dark/);
  assert.match(style, /prefers-reduced-motion:\s*reduce/);
  assert.match(style, /:focus-visible/);
  for (const banned of ["linear-gradient", "radial-gradient", "backdrop-filter", "box-shadow", "@import", "url("]) {
    assert.ok(!style.includes(banned), `app.css uses ${banned}`);
  }
  assert.match(style, /-apple-system/);
  assert.match(style, /serif/);
});
