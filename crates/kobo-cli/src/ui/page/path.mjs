// The way in: from a Kobo that has never met this computer to one it can
// send things to.
//
// An owner on this path is told one thing at a time, and the page moves on
// by itself when it can see that the thing happened: the cable went in,
// Connect was tapped, the copy finished, the cable came out, the reader
// answered on Wi-Fi. Everything here decides which of those moments it is
// and what to say at it, from what the server reports. Nothing here is the
// command's own wording: the command writes for somebody reading a
// terminal, and this is for somebody who will never open one.

import { elapsed } from "./logic.mjs";

// How long a Kobo may sit plugged in and unconnected before the page stops
// assuming Connect is about to be tapped and says what else it could be.
export const CONNECT_PATIENCE = 20_000;

// What to say while a Kobo is on the cable and has not offered its storage.
// For the first while there is one thing to do. After that there are two
// real causes, and neither can be fixed from a computer.
export function connecting(waitedFor) {
  if (waitedFor < CONNECT_PATIENCE) return { title: "Tap Connect on the Kobo’s screen.", causes: [] };
  return {
    title: "The Kobo is plugged in but has not connected.",
    causes: [
      "The cable may only charge. Try the one that came with the Kobo, or another that carries data.",
      "A new Kobo has to finish its own setup first: its language, Wi-Fi and signing in, on the Kobo itself. No computer can see it until that is done.",
    ],
  };
}

// What installing Cobalt changes, in an owner's words. The exact changes,
// file by file, are the command's own plan and stay behind a disclosure.
export function plainPlan(reader, computer, undo) {
  if (undo) {
    return [
      "Cobalt and the apps installed with it are taken off the Kobo.",
      "What your apps saved is kept on the Kobo in a folder of its own, not deleted.",
      "Your books, and the Kobo’s own reading software, are not touched.",
    ];
  }
  return [
    "Cobalt goes into a folder of its own on the Kobo. Your books and the Kobo’s own reading software are not touched.",
    "A Cobalt entry is added to the Kobo’s menu. That is how you open it.",
    "The Kobo is set up to accept books and articles from this computer over Wi-Fi.",
    reader?.cobalt ? null : "It can all be taken off again from here.",
    computer?.os === "macos"
      ? "After the install, macOS asks whether this may find devices on your local network. Allow it: that is how the Kobo is found on Wi-Fi."
      : null,
  ].filter(Boolean);
}

// The four things an install does, as a list that advances. Each is done,
// being done, waiting on the owner, or still to come.
export function installSteps(stage) {
  const order = ["writing", "eject", "finding", "securing", "ready"];
  // Where an ending that is not a stage of its own leaves the list.
  const at = { not_found: "finding", several: "finding", no_network: "finding", failed: null }[stage] ?? stage;
  const reached = order.indexOf(at);
  const stopped = ["not_found", "several", "no_network"].includes(stage);
  const state = index => {
    if (stage === "failed") return "todo";
    if (reached > index) return "done";
    if (reached < index) return "todo";
    if (stage === "eject") return "yours";
    return stopped ? "stopped" : "doing";
  };
  return [
    { text: "Copy Cobalt to the Kobo and check the copy", state: state(0) },
    { text: "Eject the Kobo", state: state(1) },
    { text: "Find it on Wi-Fi", state: state(2) },
    { text: "Save it on this computer", state: state(3) },
  ];
}

// What an owner does on the Kobo itself once it is unplugged. The corner
// its menu is in depends on its firmware and is only known while it is
// plugged in, so a page opened afterwards says both. `plugged` is for the
// page that has waited a while to see the cable come out and has not: it
// stops waiting and says so as the first step.
export function onTheKobo(menuCorner, plugged = false) {
  const corner = menuCorner ? `at the ${menuCorner}` : "at the bottom right (the top left on an older Kobo)";
  return [
    plugged ? "Unplug it, if it is still plugged in." : null,
    "Wait about a minute while it restarts. It shows Updating, then its home screen.",
    "Join Wi-Fi on it, if it is not on already.",
    `Open the menu ${corner} and choose Cobalt.`,
  ].filter(Boolean);
}

// How long the page waits to see the cable come out before it stops
// waiting. Not every system can say what is on the cable, and one that
// says a Kobo is still there when it is not would otherwise hold an owner
// on "unplug it" for the whole five minutes.
export const UNPLUG_PATIENCE = 30_000;

// What is said under a copy that is still going: the one thing not to do,
// and after a while that it has not stalled.
export function copying(waitedFor) {
  return waitedFor < 120_000 ? "Leave the Kobo plugged in." : "Still copying, which is slow on some Kobos. Leave it plugged in.";
}

// What is said once the search has used half its time: what has to be true
// of the Kobo for it to answer, since one of those is what is missing.
export function stillLooking(setup) {
  const search = looking(setup);
  return search.waited * 2 >= search.limit
    ? "Not found yet. The Kobo has to be awake, on the same Wi-Fi network as this computer, and showing Cobalt."
    : "";
}

// Why a Kobo that was set up has not been found on Wi-Fi, most likely
// first, each with what to do about it.
export function notFoundCauses(computer) {
  return [
    "The Kobo is asleep. Press its power button, and open Cobalt from its menu.",
    "The Kobo and this computer are on different Wi-Fi networks. Put them on the same one.",
    "The network keeps devices apart, as guest networks do. Use your main network.",
    computer?.os === "macos"
      ? "This Mac is not allowing the search. In System Settings, open Privacy & Security, then Local Network, and turn it on for Cobalt, or for the terminal you ran kobo ui from."
      : null,
  ].filter(Boolean);
}

// How far the search has got, for showing against its limit.
export function looking(setup) {
  const limit = Number(setup?.limit) || 300;
  const waited = Math.min(Math.max(Number(setup?.waited) || 0, 0), limit);
  return { waited, limit, text: `${elapsed(waited * 1000)} of ${elapsed(limit * 1000)}` };
}

// The moment an install or a search is at, as the Reader place shows it.
//
//   scene   which screen: "writing", "eject", "unplug", "on_kobo",
//           "securing", "ready", "not_found", "several", "no_network" or
//           "failed"
//   title   its heading
//   again   whether looking again is what is offered
//
// The server's stage "finding" is two moments here. Until the cable comes
// out the reader cannot restart, so the one thing to say is to unplug it;
// only then are the steps on the Kobo itself worth reading. Where the
// system cannot say what is on the cable, both are shown together.
export function arrivalScene(job, state, unplugWaited = 0) {
  const setup = job?.setup ?? null;
  const stage = !job ? "" : !job.running && job.ok === false ? "failed" : setup?.stage ?? "writing";
  const name = setup?.saved_as ? `“${setup.saved_as}”` : "The Kobo";
  switch (stage) {
    case "eject":
      return { scene: "eject", title: "Eject the Kobo.", again: false };
    case "finding": {
      const plugged = (state?.readers?.length ?? 0) > 0 || (state?.attached ?? 0) > 0;
      if (plugged && unplugWaited < UNPLUG_PATIENCE) return { scene: "unplug", title: "Unplug the Kobo.", again: false, plugged };
      return { scene: "on_kobo", title: "Now, on the Kobo.", again: false, plugged };
    }
    case "securing":
      return { scene: "securing", title: "Found it.", again: false };
    case "ready":
      return { scene: "ready", title: setup?.saved_as ? `${name} is ready.` : "Cobalt is installed.", again: false };
    case "not_found":
      return { scene: "not_found", title: "The Kobo was not found on Wi-Fi.", again: true };
    case "several":
      return { scene: "several", title: "The Kobo answered from two places at once.", again: true };
    case "no_network":
      return { scene: "no_network", title: "This computer is not on a network.", again: true };
    case "failed":
      return { scene: "failed", title: `${job.title} did not finish.`, again: false };
    default:
      return { scene: "writing", title: job?.title || "Installing Cobalt", again: false };
  }
}

// The three things worth doing with a reader that has just been set up,
// each a place in this window.
export const NEXT_THINGS = [
  { href: "#apps", label: "Add apps" },
  { href: "#browser", label: "Add Send to Kobo to your browser" },
  { href: "#send", label: "Send a book or an article" },
];

// What the Reader place is showing when no install is going on: the way in
// for a first reader, the wait for one that was set up and has not been
// found, or the readers this computer already knows.
//
//   "first"     nothing saved and nothing waiting: ask for the cable
//   "waiting"   a reader was set up from here and is still to be found
//   "saved"     readers are saved; list them, and offer to add another
//
// A Kobo on the cable comes before either of the others. It has not
// connected, or the system is keeping it from this page, and what to do
// about that is what its owner is looking at the page for.
export function withoutReader(state, adding) {
  if (!state) return "first";
  if ((state.attached ?? 0) > 0 || (state.blocked ?? []).length > 0) return "first";
  if (state.awaiting) return "waiting";
  if ((state.saved ?? []).length > 0 && !adding) return "saved";
  return "first";
}

// Whether a reader that is plugged in still has to be linked to this
// computer: it has Cobalt, and this computer has never saved it, because
// it was set up from another or the search for it was never finished.
export function needsLinking(reader) {
  return Boolean(reader?.cobalt) && !reader.saved_as;
}
