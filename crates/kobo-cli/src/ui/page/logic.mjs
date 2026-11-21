// What the page decides, apart from how it draws it.
//
// Everything here is a function of its arguments and nothing else: no
// document, no network, no clock. That is what lets tools/kobo-ui.test.mjs
// import this file under node and hold each decision to an example, and it
// is why app.js, which does touch the document, has nothing in it worth
// testing.

// The command line wraps its sentences at eighty columns and indents what
// continues a list item. A window that is 420 pixels wide wraps wherever it
// needs to, so text shown as the terminal printed it breaks twice: once
// where the terminal broke it and again where the window does. This puts
// the sentences back together and keeps the structure the indentation was
// expressing, so a list is still a list.
//
// Returns blocks in order, each one of:
//   { kind: "heading", text }          a line that introduces what follows
//   { kind: "para", text }             a paragraph
//   { kind: "step", number, text }     a numbered instruction
//   { kind: "item", text }             a bulleted fact
//   { kind: "fact", name, value }      a "Name: value" line under a heading
//   { kind: "error", text }            what the command said as it failed
//   { kind: "code", text }             a command or path on a line of its own
export function reflow(text) {
  const blocks = [];
  let open = null;
  // How long the line before this one was, to tell a line the terminal
  // wrapped from one the command ended on purpose.
  let before = 0;
  const close = () => { open = null; };
  const push = block => { blocks.push(block); open = block; return block; };

  for (const raw of String(text).replaceAll("\r", "").split("\n")) {
    const line = raw.trimEnd();
    const body = line.trim();
    const indent = line.length - line.trimStart().length;
    const previous = before;
    before = line.length;

    // A row of dots is the terminal's way of showing it is waiting. The
    // page has its own.
    if (body === "" || /^\.+$/.test(body)) { close(); continue; }

    const failed = /^kobo: (.*)$/.exec(body);
    if (indent === 0 && failed) { push({ kind: "error", text: failed[1] }); continue; }

    const step = /^(\d+)\.\s+(.*)$/.exec(body);
    if (indent > 0 && step) {
      push({ kind: "step", number: Number(step[1]), text: step[2], indent });
      continue;
    }
    const item = /^[·•]\s+(.*)$/.exec(body);
    if (item) { push({ kind: "item", text: item[1], indent }); continue; }

    // Indented under a list entry, further in than its marker: the entry
    // carrying on.
    if (open && (open.kind === "step" || open.kind === "item") && indent > open.indent) {
      open.text += ` ${body}`;
      continue;
    }
    // An error that ran onto an indented second line.
    if (open && open.kind === "error" && indent > 0) { open.text += ` ${body}`; continue; }

    const last = blocks[blocks.length - 1];
    const under = open && last && (last.kind === "heading" || last.kind === "fact");
    const fact = /^([A-Z][A-Za-z ]{0,24}): (.+)$/.exec(body);
    if (indent > 0 && under && fact) {
      push({ kind: "fact", name: fact[1], value: fact[2] });
      continue;
    }

    // An indented line straight after a sentence that ends in a colon is
    // the command or path that sentence was introducing, and so is the
    // line after that.
    const introduced = open && ((open.kind === "para" && open.text.endsWith(":")) || open.kind === "code");
    if (indent > 0 && introduced) { push({ kind: "code", text: body }); continue; }
    // A line taken for a heading that turns out to introduce plain lines,
    // not a list, was a sentence all along.
    if (indent > 0 && open && open.kind === "heading") {
      open.kind = "para";
      open.text += ":";
      push({ kind: "code", text: body });
      continue;
    }

    // A heading is short, stands alone, and is one phrase. A sentence that
    // ends in a colon after a full stop is prose that happens to end there.
    if (indent === 0 && !open && body.endsWith(":") && body.length <= 60 && !/[.!?] /.test(body)) {
      push({ kind: "heading", text: body.slice(0, -1) });
      continue;
    }

    // A paragraph carries on only where the terminal had to break it. If
    // this line's first word would have fitted on the end of the last one,
    // nothing forced the break, so the command meant a new line: a second
    // statement, or a labelled line such as "sample: ...".
    const wrapped = previous + 1 + body.split(" ")[0].length > WRAP_AT_LEAST;
    if (open && open.kind === "para" && wrapped) { open.text += ` ${body}`; continue; }
    push({ kind: "para", text: body });
  }
  return blocks.map(({ indent, ...block }) => block);
}

// The narrowest the commands wrap their own prose. Their sentences are
// broken by hand so that the next word would have passed about 76 columns;
// a line that stops with room to spare for it stopped because it was
// finished.
const WRAP_AT_LEAST = 72;

// What a setup said, divided at the point where it turns from what it did
// to what the owner does next.
//
// Setup reports everything it changed, at length, and then gives the steps
// for the reader. The report is worth keeping and the steps are what
// somebody is waiting to be told, so the page leads with the steps and folds
// the report away. Nothing is reordered within either part. Output with no
// such turn, which is every command but an install, is all one part.
export function afterwards(blocks) {
  const turn = blocks.findIndex(block => block.kind === "heading" && /^Next\b/.test(block.text));
  return turn < 0 ? { report: [], next: blocks } : { report: blocks.slice(0, turn), next: blocks.slice(turn) };
}

// The line a failed command ended on, which is the reason it failed, or
// nothing when it never said one.
export function failure(text) {
  const reasons = reflow(text).filter(block => block.kind === "error");
  return reasons.length > 0 ? reasons[reasons.length - 1].text : "";
}

// Why a job failed, as the owner is told it. For the failures it knows, the
// server says it in the window's own words, with no file or command in it.
// For the rest it is the sentence the command stopped on.
export function whyFailed(job) {
  const told = String(job?.reason ?? "").trim();
  return told !== "" ? told : sentence(failure(job?.text ?? ""));
}

// A sentence as it should begin and end on a page. The commands write some
// of their reasons to follow a colon, in lower case and unpunctuated.
export function sentence(text) {
  const trimmed = String(text ?? "").trim();
  if (trimmed === "") return "";
  const started = trimmed[0].toUpperCase() + trimmed.slice(1);
  return /[.!?:]$/.test(started) ? started : `${started}.`;
}

// One line of the plan as an instruction.
//
// The dry run writes each change as "would do this", which reads well down
// a terminal and badly as a list under a heading that already says these
// are the things that would happen. The word is dropped and nothing else is
// touched: the plan is setup's, and the page is not where it gets reworded.
export function planStep(line) {
  const text = String(line ?? "").trim().replace(/^would /, "");
  return text === "" ? "" : text[0].toUpperCase() + text.slice(1);
}

// A sentence split around the web addresses in it, so the page can make
// each one a link and leave the rest as text:
//   [{ text }, { href, text }, { text }]
// Only https addresses with a host after the scheme, so that a sentence
// which merely mentions "https://" is left alone, and the full stop that
// ends the sentence is not taken for part of the address.
export function linked(text) {
  const parts = [];
  let rest = String(text ?? "");
  for (;;) {
    const found = /https:\/\/[A-Za-z0-9][^\s<>"')]*/.exec(rest);
    if (!found) break;
    const address = found[0].replace(/[.,;:]+$/, "");
    if (found.index > 0) parts.push({ text: rest.slice(0, found.index) });
    parts.push({ href: address, text: address.replace(/^https:\/\//, "").replace(/\/$/, "") });
    rest = rest.slice(found.index + address.length);
  }
  if (rest !== "") parts.push({ text: rest });
  return parts;
}

// Which reader the Reader and Apps places are about.
//
// One reader needs no choosing. With several, the one already chosen stays
// chosen for as long as it is plugged in, and nothing is chosen for the
// owner: installing onto the wrong one of two readers is not a mistake to
// make on their behalf.
export function chosenReader(readers, chosenVolume) {
  if (!Array.isArray(readers) || readers.length === 0) return { kind: "none" };
  if (readers.length === 1) return { kind: "one", reader: readers[0] };
  const chosen = readers.find(reader => reader.volume === chosenVolume);
  return chosen ? { kind: "one", reader: chosen, among: readers } : { kind: "several", readers };
}

// Why no reader is showing, when none is.
//
// "No reader" used to cover three different things, and the advice for one
// is wrong for the others: telling somebody to plug in a reader that is
// plugged in and waiting for a tap, or one the system is keeping this
// program from reading, sends them to the cable when the answer is on the
// reader's screen or in System Settings.
//
//   "blocked"      a drive that looks like a reader is mounted and the
//                  system will not let the kobo command read it
//   "unconnected"  a Kobo is on the USB bus with no drive, so Connect has
//                  not been tapped on its screen
//   "none"         nothing is plugged in, as far as can be told
export function absence(state) {
  const mounted = state?.readers?.length ?? 0;
  if (mounted > 0) return { kind: "present" };
  const blocked = state?.blocked ?? [];
  if (blocked.length > 0) return { kind: "blocked", volume: blocked[0] };
  if (Number.isInteger(state?.attached) && state.attached > mounted) return { kind: "unconnected" };
  return { kind: "none" };
}

// What is on the reader after a job that did not finish, said so that the
// owner knows where they stand before deciding what to do next.
//
// A failed install does not leave a half-installed Cobalt: setup writes
// beside the old copy and swaps, so the reader has what it had or what was
// being installed. Which of the two is read from the reader, not assumed.
// A reader that is no longer mounted cannot be read, and that is said too,
// because pulling the cable is the commonest reason a job fails.
export function afterFailure(job, state) {
  if (!job?.volume) return "";
  const reader = (state?.readers ?? []).find(found => found.volume === job.volume);
  if (!reader) {
    return "The reader is no longer connected, so what is on it cannot be read. Plug it in again and tap Connect; this window then says what it has.";
  }
  if (reader.cobalt_problem) return "The reader is still connected. The Cobalt on it is damaged; installing again replaces it.";
  if (reader.cobalt) return `The reader is still connected and has Cobalt ${reader.cobalt.cobalt_version} on it.`;
  return "The reader is still connected and has no Cobalt on it.";
}

// How long something has been going, to the precision anybody wants while
// waiting for it.
export function elapsed(milliseconds) {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  return seconds % 60 === 0 ? `${minutes} min` : `${minutes} min ${seconds % 60} s`;
}

// Why an app that is offered cannot be installed on this reader as it
// stands, or nothing when it can.
export function appObstacle(app, reader) {
  if (app.runs_on_this_reader === false) return `Needs Cobalt ${app.minimum_cobalt_version} or newer.`;
  if (app.fits === false) {
    const free = Number.isFinite(reader?.free_bytes) ? `; the reader has ${fileSize(reader.free_bytes)} free` : "";
    return `Needs ${fileSize(app.bytes)}${free}.`;
  }
  return "";
}

// What the Reader place offers to do with one reader, and the reason when
// the thing an owner would expect is not on offer.
//
// What the reader has and what this computer has are facts and are shown as
// facts, in the table. Nothing here restates them. `reason` is only ever
// why something cannot be done: Cobalt cannot go on this reader, there is
// nothing here to install, the reader is ahead of this computer, what is on
// it is damaged. When everything is in order it is empty.
//
//   reason     why the expected action is missing, or ""
//   primary    "install", "update" or null: the one thing to do next
//   repair     whether installing again is offered as a lesser action
//   removable  whether Cobalt can be taken off
export function readerStanding(reader, computer) {
  const release = computer?.release ?? null;
  const noRelease = computer?.release_problem ?? "";
  const on = reader.cobalt?.cobalt_version ?? null;
  const refused = reader.supported ? "" : `Cobalt cannot be installed on this reader. ${sentence(reader.refusal)}`;

  if (reader.cobalt_problem) {
    return { reason: reader.cobalt_problem, primary: release && reader.supported ? "install" : null, repair: false, removable: true };
  }
  if (!on) {
    if (refused) return { reason: refused, primary: null, repair: false, removable: false };
    return release
      ? { reason: "", primary: "install", repair: false, removable: false }
      : { reason: noRelease, primary: null, repair: false, removable: false };
  }
  switch (reader.standing) {
    case "behind":
      return { reason: refused, primary: refused ? null : "update", repair: false, removable: true };
    case "ahead":
      return {
        reason: `The Cobalt on this reader is newer than the ${release} on this computer. Run kobo update in a terminal before changing it from here.`,
        primary: null,
        repair: false,
        removable: true,
      };
    case "current":
      return { reason: "", primary: null, repair: !refused, removable: true };
    default:
      return { reason: noRelease, primary: null, repair: false, removable: true };
  }
}

// Every app the window knows of, by id, with what each source says of it
// laid together.
//
// Three things know about apps and none knows everything. `store` is built
// into the command: what each app is, its pictures, its group. It is always
// there and can be a release behind. `installed` is what this reader holds.
// `catalog` is what is published for this reader today: the version on
// offer, its size, whether this reader can run it. Either of the last two
// may be null, when no reader is chosen or the answer has not come.
//
// Where two disagree the newer source wins: the catalog's version and one
// line over the store's, since the store was written when the command was
// built, and the store's over the reader's own, which for a built-in copy
// is the runtime's stand-in.
export function storeApps(store, installed, catalog) {
  const known = store?.apps ?? {};
  const held = new Map((installed ?? []).map(app => [app.id, app]));
  const offered = new Map((catalog ?? []).map(app => [app.id, app]));
  const apps = new Map();
  for (const id of new Set([...Object.keys(known), ...held.keys(), ...offered.keys()])) {
    const shown = known[id] ?? null;
    const here = held.get(id) ?? null;
    const offer = offered.get(id) ?? null;
    const summary = offer?.summary || shown?.summary || here?.summary || "";
    apps.set(id, {
      id,
      title: offer?.title || shown?.title || here?.title || id,
      summary,
      description: shown?.description || summary,
      shots: shown?.shots ?? [],
      // The note is about the version the store was built with, so it is
      // dropped when the catalog has moved on to another.
      release_notes: shown && (!offer || offer.version === shown.version) ? shown.release_notes ?? null : null,
      access: shown ? shown.access ?? [] : null,
      setup: shown?.setup ?? [],
      version: offer?.version ?? shown?.version ?? here?.version ?? "",
      installed_version: here ? here.installed_version ?? here.version ?? "" : null,
      on_reader: Boolean(here),
      offered: Boolean(offer),
      update: Boolean(here && offer?.update_available),
      runs: offer ? offer.runs_on_this_reader !== false : true,
      minimum: offer?.minimum_cobalt_version ?? null,
      bytes: Number.isFinite(offer?.bytes) ? offer.bytes : null,
      fits: offer?.fits ?? null,
    });
  }
  return apps;
}

const byTitle = (left, right) =>
  left.title.localeCompare(right.title, "en", { sensitivity: "base" }) || left.id.localeCompare(right.id);

// Whether an app is one the words typed are looking for: every word has to
// be somewhere in its name, its id or what it says about itself.
function sought(app, query) {
  const words = String(query ?? "").toLowerCase().split(/\s+/).filter(Boolean);
  const text = [app.title, app.id, app.summary, app.description].join("\n").toLowerCase();
  return words.every(word => text.includes(word));
}

// The store as the Apps place lays it out: what is on the reader first,
// then every app under the group the website lists it in, each narrowed to
// what was typed in the search box.
//
// An app on the reader is in its group as well. The first shelf is for
// looking after what is there, the groups are for looking around, and an
// owner looking through Games should see the one they already have. An app
// the catalog offers and the store has never heard of, because it was
// published after this command was built, goes in a last group so that it
// can still be installed.
//
//   onReader  the reader's apps, or null when no reader's apps are known
//   updates   ids of the ones with an update this reader can take
//   groups    { id, title, apps } for each group with something to show
//   found     how many apps are shown at all
export function storeShelves(store, installed, catalog, query) {
  const apps = storeApps(store, installed, catalog);
  const wanted = app => sought(app, query);
  const grouped = new Set();
  const groups = (store?.groups ?? []).map(group => ({
    id: group.id,
    title: group.title,
    apps: group.apps.map(id => { grouped.add(id); return apps.get(id); }).filter(Boolean),
  }));
  const strays = [...apps.values()].filter(app => app.offered && !grouped.has(app.id));
  if (strays.length > 0) {
    const other = groups.find(group => group.id === "other");
    if (other) other.apps.push(...strays);
    else groups.push({ id: "other", title: "Other apps", apps: strays });
  }
  for (const group of groups) group.apps = group.apps.filter(wanted).sort(byTitle);

  const held = [...apps.values()].filter(app => app.on_reader);
  const onReader = installed ? held.filter(wanted).sort(byTitle) : null;
  const shown = groups.filter(group => group.apps.length > 0);
  return {
    onReader,
    updates: held.filter(app => app.update && app.runs).sort(byTitle).map(app => app.id),
    groups: shown,
    found: new Set([...(onReader ?? []), ...shown.flatMap(group => group.apps)].map(app => app.id)).size,
  };
}

// The few words under an app's name in a shelf that are about this reader,
// or nothing. A fact the owner needs before opening the app's page: that it
// is already here, that there is a newer one, or why it cannot go on.
export function tileStanding(app) {
  if (app.on_reader) {
    if (app.update && app.runs) return `Update to ${app.version}`;
    if (app.update) return `${app.version} needs Cobalt ${app.minimum}`;
    return "On this reader";
  }
  if (!app.offered) return "";
  if (!app.runs) return `Needs Cobalt ${app.minimum}`;
  if (app.fits === false) return `Needs ${fileSize(app.bytes)} free`;
  return "";
}

// Why nothing can be installed from the store as things stand, in the one
// line the store says it in, or nothing when a reader is ready.
export function installingNeeds(chosen, state) {
  if (!state) return "";
  if (chosen.kind === "none") return "Connect a reader to install apps.";
  if (chosen.kind === "several") return "Choose one of the connected readers in Reader to install apps.";
  if (!chosen.reader.cobalt) return "Install Cobalt on this reader, in Reader, to install apps.";
  return "";
}

// What an app's page offers to do with it on this reader, and the reason
// when the thing an owner would expect is not on offer.
//
//   primary    "install", "update" or null
//   removable  whether it is on the reader to be removed
//   reason     why there is no primary action, or ""
//
// `reader` is the chosen reader with Cobalt on it, or null. `catalogKnown`
// is whether the catalog has arrived: until it has, an app that is not on
// the reader gives no reason, because none is known yet.
export function appStanding(app, reader, catalogKnown) {
  if (!reader) return { primary: null, removable: false, reason: "" };
  const has = reader.cobalt?.cobalt_version ? ` This reader has ${reader.cobalt.cobalt_version}.` : "";
  if (app.on_reader) {
    if (app.update && app.runs) return { primary: "update", removable: true, reason: "" };
    if (app.update) {
      return { primary: null, removable: true, reason: `Version ${app.version} needs Cobalt ${app.minimum} or newer.${has}` };
    }
    return { primary: null, removable: true, reason: "" };
  }
  if (!app.offered) {
    return { primary: null, removable: false, reason: catalogKnown ? "The catalog for this reader does not offer this app." : "" };
  }
  if (!app.runs) return { primary: null, removable: false, reason: `Needs Cobalt ${app.minimum} or newer.${has}` };
  if (app.fits === false) {
    const free = Number.isFinite(reader.free_bytes) ? ` The reader has ${fileSize(reader.free_bytes)} free.` : "";
    return { primary: null, removable: false, reason: `Needs ${fileSize(app.bytes)}.${free}` };
  }
  return { primary: "install", removable: false, reason: "" };
}

// What is true of an app, as the name and value pairs its page sets in a
// table. Only what is known is listed: a size and a minimum come from the
// catalog, so a store read with no reader has neither.
export function appFacts(app) {
  return [
    app.version && ["Version", app.version],
    app.on_reader && ["On this reader", app.installed_version || "Installed"],
    app.bytes !== null && ["Size", fileSize(app.bytes)],
    app.minimum && ["Needs", `Cobalt ${app.minimum} or newer`],
  ].filter(Boolean);
}

// A picture's place in the queue of ones being fetched: whether to ask for
// it now. One that failed is asked for again only after the server has
// stopped resting from the failure, so that an owner who opens a page of
// empty frames, gets back on the network and opens it again sees pictures,
// and a page that redraws itself does not ask in a loop.
export function pictureDue(record, now, rest = 30_000) {
  if (!record) return true;
  if (record.state === "failed") return now - record.at >= rest;
  return false;
}

// Which picture a step from one in a gallery lands on. It stops at the
// ends: a gallery of four that wrapped from the last to the first would
// leave somebody pressing an arrow unsure whether they had seen them all.
export function galleryStep(index, by, count) {
  return Math.min(Math.max(index + by, 0), Math.max(count - 1, 0));
}

// The one step a browser still needs, taken apart for display.
//
// The command writes it as "Chrome: do this and pick" followed by indented
// lines: the folder to pick, and sometimes a caution. The browser's name is
// already the row's heading, a folder is something to copy, and a caution is
// a sentence, so each is handed back as what it is.
export function browserStep(step, title) {
  const lines = String(step ?? "").split("\n").map(line => line.trim()).filter(Boolean);
  if (lines.length === 0) return { instruction: "", paths: [], notes: [] };
  const prefix = `${title}: `;
  const first = lines[0].startsWith(prefix) ? lines[0].slice(prefix.length) : lines[0];
  const paths = [];
  const notes = [];
  for (const line of lines.slice(1)) {
    if (/^(\/|~\/|[A-Za-z]:\\)/.test(line)) paths.push(line);
    else notes.push(sentence(line));
  }
  // An instruction that goes on to name a folder ends in "and pick"; what
  // it picks is shown underneath, so the sentence is closed with a colon.
  const instruction = paths.length > 0 ? `${sentence(first).replace(/\.$/, "")}:` : sentence(first);
  return { instruction, paths, notes };
}

// A size as a person reads it.
export function fileSize(bytes) {
  const size = Number(bytes);
  if (!Number.isFinite(size) || size < 0) return "";
  if (size < 1000) return `${size} bytes`;
  const units = ["KB", "MB", "GB"];
  let value = size / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) { value /= 1000; unit += 1; }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

// Why a file cannot be handed over, or nothing when it can. Asked before
// the bytes are sent, because the server refuses an oversized file by
// closing the connection and a browser reports that as a network failure
// with no reason attached.
export function uploadRefusal(file, limit) {
  if (!file) return "Choose a file to send.";
  if (file.size === 0) return `${file.name} is empty, so there is nothing to send.`;
  if (Number.isFinite(limit) && file.size > limit) {
    return `${file.name} is ${fileSize(file.size)}. The most this window sends is ${fileSize(limit)}; use kobo send in a terminal for larger files.`;
  }
  return "";
}

// Whether what was typed can be sent as a web address, and the address to
// send. The server checks again; this is so the owner is told before a
// request is made, in the field they typed in.
export function webAddress(typed) {
  const address = String(typed ?? "").trim();
  if (address === "") return { address: "", problem: "" };
  if (!/^https?:\/\/\S+$/i.test(address)) {
    return { address: "", problem: "Enter a web address that starts with http:// or https://." };
  }
  return { address, problem: "" };
}

// The job to show in a place when the page is opened or reloaded: the one
// of that kind still running, or failing that nothing. A finished job is
// not brought back, since what it did is already reflected in everything
// else the page shows.
export function runningJob(jobs, kind) {
  return (jobs ?? []).find(job => job.kind === kind && job.running) ?? null;
}

// The four places, in order, and the one an address names.
export const PLACES = ["reader", "apps", "browser", "send"];

export function placeFor(hash) {
  return routeFor(hash).place;
}

// The place an address names and, under Apps, the one app whose page it
// is. An app is named by its id, and anything that could not be an id is
// read as no app at all, never looked up.
export function routeFor(hash) {
  const [named, rest] = String(hash ?? "").replace(/^#/, "").split("/", 2);
  const place = PLACES.includes(named) ? named : "reader";
  const app = place === "apps" && /^[a-z0-9][a-z0-9-]{0,31}$/.test(rest ?? "") ? rest : null;
  return { place, app };
}
