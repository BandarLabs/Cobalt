import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  CONNECT_PATIENCE,
  NEXT_THINGS,
  UNPLUG_PATIENCE,
  arrivalScene,
  connecting,
  copying,
  installSteps,
  looking,
  needsLinking,
  notFoundCauses,
  onTheKobo,
  plainPlan,
  stillLooking,
  withoutReader,
} from "../crates/kobo-cli/src/ui/page/path.mjs";

const page = join(dirname(fileURLToPath(import.meta.url)), "../crates/kobo-cli/src/ui/page");
const source = readFileSync(join(page, "path.mjs"), "utf8");

const job = (stage, more = {}) => ({ title: "Installing Cobalt", running: true, ok: null, setup: { stage, waited: 0, limit: 300, saved_as: null, secured: null, notes: [] }, ...more });
const ended = (stage, setup = {}) => job(stage, { running: false, ok: true, setup: { stage, waited: 0, limit: 300, saved_as: null, secured: null, notes: [], ...setup } });

test("a Kobo on the cable is asked for Connect first, and only later for what else it could be", () => {
  assert.deepEqual(connecting(0), { title: "Tap Connect on the Kobo’s screen.", causes: [] });
  assert.deepEqual(connecting(CONNECT_PATIENCE - 1).causes, []);
  const late = connecting(CONNECT_PATIENCE);
  assert.equal(late.title, "The Kobo is plugged in but has not connected.");
  assert.equal(late.causes.length, 2);
  assert.match(late.causes[0], /cable/);
  // The Kobo's own first-time setup is said plainly and not worked around.
  assert.match(late.causes[1], /finish its own setup first/);
  assert.match(late.causes[1], /No computer can see it until that is done/);
});

test("an install is four things that advance, and the one the owner does is marked as theirs", () => {
  const states = stage => installSteps(stage).map(step => step.state);
  assert.deepEqual(installSteps("writing").map(step => step.text), [
    "Copy Cobalt to the Kobo and check the copy",
    "Eject the Kobo",
    "Find it on Wi-Fi",
    "Save it on this computer",
  ]);
  assert.deepEqual(states("writing"), ["doing", "todo", "todo", "todo"]);
  assert.deepEqual(states("eject"), ["done", "yours", "todo", "todo"]);
  assert.deepEqual(states("finding"), ["done", "done", "doing", "todo"]);
  assert.deepEqual(states("securing"), ["done", "done", "done", "doing"]);
  assert.deepEqual(states("ready"), ["done", "done", "done", "done"]);
  // A search that ended without finding it stopped at the search.
  for (const stage of ["not_found", "several", "no_network"]) {
    assert.deepEqual(states(stage), ["done", "done", "stopped", "todo"], stage);
  }
  assert.deepEqual(states("failed"), ["todo", "todo", "todo", "todo"]);
});

test("each moment of an install is one screen with one heading", () => {
  const scene = (work, state = { readers: [], attached: 0 }) => arrivalScene(work, state);
  assert.deepEqual(scene(job("writing")), { scene: "writing", title: "Installing Cobalt", again: false });
  assert.equal(copying(0), "Leave the Kobo plugged in.");
  assert.match(copying(120_000), /^Still copying/);
  assert.deepEqual(scene(job("writing", { title: "Updating Cobalt" })).title, "Updating Cobalt");
  // A job the server says nothing structured about is still an install.
  assert.equal(scene({ title: "Installing Cobalt", running: true, ok: null }).scene, "writing");
  assert.deepEqual(scene(ended("eject")), { scene: "eject", title: "Eject the Kobo.", again: false });
  assert.equal(scene(job("securing")).scene, "securing");
  assert.deepEqual(scene(ended("ready", { saved_as: "study" })), { scene: "ready", title: "“study” is ready.", again: false });
  assert.equal(scene(ended("ready")).title, "Cobalt is installed.");
  for (const stage of ["not_found", "several", "no_network"]) {
    const shown = scene(ended(stage));
    assert.deepEqual([shown.scene, shown.again], [stage, true]);
  }
  // A command that failed is a failure whatever stage it had reached.
  const failed = scene(job("finding", { running: false, ok: false }));
  assert.deepEqual(failed, { scene: "failed", title: "Installing Cobalt did not finish.", again: false });
});

test("while the Kobo is being looked for, the cable coming out is what moves the page on", () => {
  const work = job("finding");
  assert.equal(arrivalScene(work, { readers: [], attached: 1 }).scene, "unplug");
  assert.equal(arrivalScene(work, { readers: [{}], attached: null }).scene, "unplug");
  assert.equal(arrivalScene(work, { readers: [], attached: 0 }).scene, "on_kobo");
  // A system that cannot say what is on the cable does not keep an owner
  // on "unplug it" for ever.
  assert.equal(arrivalScene(work, { readers: [], attached: null }).scene, "on_kobo");
  assert.equal(arrivalScene(work, null).scene, "on_kobo");
  // Nor does one that says a Kobo is still there when it has gone: after a
  // while the page moves on, and says to unplug it as the first step.
  const stuck = arrivalScene(work, { readers: [], attached: 1 }, UNPLUG_PATIENCE);
  assert.deepEqual([stuck.scene, stuck.plugged], ["on_kobo", true]);
  assert.equal(onTheKobo("bottom right", stuck.plugged)[0], "Unplug it, if it is still plugged in.");
  assert.equal(onTheKobo("bottom right", stuck.plugged).length, 4);
});

test("the steps on the Kobo name the corner its menu is in when that is known", () => {
  const known = onTheKobo("bottom right");
  assert.equal(known.length, 3);
  assert.match(known[0], /about a minute/);
  assert.match(known[1], /Wi-Fi/);
  assert.equal(known[2], "Open the menu at the bottom right and choose Cobalt.");
  assert.equal(onTheKobo("top left")[2], "Open the menu at the top left and choose Cobalt.");
  assert.equal(onTheKobo(null)[2], "Open the menu at the bottom right (the top left on an older Kobo) and choose Cobalt.");
});

test("a Kobo that was not found is given its likely causes and never a command", () => {
  const anywhere = notFoundCauses({ os: "linux" });
  assert.equal(anywhere.length, 3);
  const mac = notFoundCauses({ os: "macos" });
  assert.equal(mac.length, 4);
  assert.match(mac[3], /System Settings.*Privacy & Security.*Local Network/);
  for (const cause of mac) assert.ok(!/kobo pair|kobo setup|ssh/i.test(cause), cause);
});

test("the search is shown against its limit", () => {
  assert.deepEqual(looking({ waited: 80, limit: 300 }), { waited: 80, limit: 300, text: "1 min 20 s of 5 min" });
  assert.deepEqual(looking({ waited: 999, limit: 300 }).waited, 300);
  assert.deepEqual(looking(null), { waited: 0, limit: 300, text: "0 s of 5 min" });
  // Half way through it says what has to be true for the Kobo to answer.
  assert.equal(stillLooking({ waited: 140, limit: 300 }), "");
  assert.match(stillLooking({ waited: 150, limit: 300 }), /^Not found yet\. The Kobo has to be awake/);
});

test("the plan is said in an owner's words, and says what macOS will ask before it asks", () => {
  const fresh = plainPlan({ cobalt: null }, { os: "linux" }, false);
  assert.equal(fresh.length, 4);
  assert.match(fresh[0], /Your books/);
  assert.match(fresh.at(-1), /taken off again/);
  const mac = plainPlan({ cobalt: null }, { os: "macos" }, false);
  assert.match(mac.at(-1), /macOS asks whether this may find devices on your local network/);
  // An update does not repeat that it can be removed.
  assert.ok(!plainPlan({ cobalt: { cobalt_version: "0.3.30" } }, { os: "linux" }, false).some(line => /taken off again/.test(line)));
  const undo = plainPlan({ cobalt: {} }, { os: "macos" }, true);
  assert.equal(undo.length, 3);
  assert.match(undo[1], /not deleted/);
});

test("with no reader plugged in the place is the way in, the wait, or the readers already known", () => {
  assert.equal(withoutReader(null, false), "first");
  assert.equal(withoutReader({ saved: [], awaiting: false }, false), "first");
  assert.equal(withoutReader({ saved: [{ name: "kobo" }], awaiting: false }, false), "saved");
  // Adding another reader is the same way in.
  assert.equal(withoutReader({ saved: [{ name: "kobo" }], awaiting: false }, true), "first");
  // A reader still to be found comes before anything else.
  assert.equal(withoutReader({ saved: [{ name: "kobo" }], awaiting: true }, false), "waiting");
  assert.equal(withoutReader({ saved: [], awaiting: true }, true), "waiting");
});

test("a reader with Cobalt that this computer never saved still has to be linked", () => {
  assert.equal(needsLinking({ cobalt: { cobalt_version: "0.3.37" }, saved_as: null }), true);
  assert.equal(needsLinking({ cobalt: { cobalt_version: "0.3.37" }, saved_as: "kobo" }), false);
  assert.equal(needsLinking({ cobalt: null, saved_as: null }), false);
  assert.equal(needsLinking(null), false);
});

test("nothing on the way in is the command's wording or something to type", () => {
  // Paths, script names, the menu add-on's name and the remote login's
  // name belong behind a disclosure, in the command's own words.
  const said = source.split("\n").filter(line => !line.trim().startsWith("//")).join("\n");
  for (const word of [".adds", "KoboRoot", "NickelMenu", "SSH", "ssh", "kobo pair", "kobo setup", "rcS", "sshd"]) {
    assert.ok(!said.includes(word), `path.mjs says ${word}`);
  }
  for (const banned of ["—", "!", "Simply", "simply", "Just ", "just "]) {
    assert.ok(!said.replaceAll("!==", "").replaceAll("!job", "").replaceAll("!state", "").replaceAll("!reader", "").replaceAll("!adding", "").includes(banned), `path.mjs uses ${banned}`);
  }
  assert.deepEqual(NEXT_THINGS.map(thing => thing.href), ["#apps", "#browser", "#send"]);
});

test("the way in decides and never asks: it reaches nothing but the page's other module", () => {
  for (const reach of ["fetch(", "XMLHttpRequest", "WebSocket", "EventSource", "sendBeacon", "import(", "document.", "localStorage"]) {
    assert.ok(!source.includes(reach), `path.mjs uses ${reach}`);
  }
  assert.deepEqual([...source.matchAll(/from "([^"]+)"/g)].map(found => found[1]), ["./logic.mjs"]);
  // The page loads it from its own server, by a path and not an address.
  const script = readFileSync(join(page, "app.js"), "utf8");
  assert.ok(script.includes('} from "/path.mjs";'));
});
