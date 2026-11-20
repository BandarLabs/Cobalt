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

// What the Reader place says about one reader and offers to do with it.
//
//   headline   the one sentence that is true of this reader now
//   detail     what follows from it, when something does
//   primary    "install", "update" or null: the one thing to do next
//   repair     whether installing again is offered as a lesser action
//   removable  whether Cobalt can be taken off
export function readerStanding(reader, computer) {
  const release = computer?.release ?? null;
  const noRelease = computer?.release_problem ?? "";
  const on = reader.cobalt?.cobalt_version ?? null;

  if (reader.cobalt_problem) {
    return {
      headline: "The Cobalt on this reader is damaged.",
      detail: reader.cobalt_problem,
      primary: release && reader.supported ? "install" : null,
      repair: false,
      removable: true,
    };
  }
  if (!on) {
    if (!reader.supported) {
      return {
        headline: "Cobalt cannot be installed on this reader.",
        detail: sentence(reader.refusal),
        primary: null,
        repair: false,
        removable: false,
      };
    }
    return release
      ? {
          headline: "Cobalt is not on this reader yet.",
          detail: `This computer has Cobalt ${release} ready to install.`,
          primary: "install",
          repair: false,
          removable: false,
        }
      : {
          headline: "Cobalt is not on this reader yet.",
          detail: noRelease,
          primary: null,
          repair: false,
          removable: false,
        };
  }
  switch (reader.standing) {
    case "behind":
      return {
        headline: `This reader has Cobalt ${on}.`,
        detail: `This computer has ${release}, which is newer.`,
        primary: reader.supported ? "update" : null,
        repair: false,
        removable: true,
      };
    case "ahead":
      return {
        headline: `This reader has Cobalt ${on}.`,
        detail: `That is newer than the ${release} on this computer. Run kobo update in a terminal before changing this reader's Cobalt from here.`,
        primary: null,
        repair: false,
        removable: true,
      };
    case "current":
      return {
        headline: `This reader has Cobalt ${on}.`,
        detail: "That is the release on this computer. The reader also updates itself over Wi-Fi.",
        primary: null,
        repair: reader.supported,
        removable: true,
      };
    default:
      return {
        headline: `This reader has Cobalt ${on}.`,
        detail: noRelease,
        primary: null,
        repair: false,
        removable: true,
      };
  }
}

// The apps as the Apps place lists them: what is on the reader first, then
// what the catalog offers that is not, each narrowed to what was typed in
// the search box.
//
// `installed` is what the reader holds and is always known. `catalog` is
// what was last fetched and may be null, when it has not arrived or could
// not be reached; the reader's own apps are still listed then, with no
// updates offered, because nothing is known about any.
export function appSections(installed, catalog, query) {
  const wanted = String(query ?? "").trim().toLowerCase();
  const matches = app =>
    wanted === "" ||
    [app.title, app.id, app.summary].some(field => String(field ?? "").toLowerCase().includes(wanted));
  const byTitle = (left, right) =>
    left.title.localeCompare(right.title, "en", { sensitivity: "base" }) || left.id.localeCompare(right.id);

  const offered = new Map((catalog ?? []).map(app => [app.id, app]));
  const onReader = (installed ?? []).map(app => {
    const entry = offered.get(app.id);
    return {
      ...app,
      // The catalog's entry knows the version on offer; the reader's own
      // record knows only what it has.
      version: entry ? entry.version : app.version,
      update_available: entry ? Boolean(entry.update_available) : false,
      runs_on_this_reader: entry ? entry.runs_on_this_reader !== false : true,
      minimum_cobalt_version: entry ? entry.minimum_cobalt_version : app.minimum_cobalt_version,
      in_catalog: Boolean(entry),
    };
  });
  const held = new Set(onReader.map(app => app.id));
  const available = (catalog ?? []).filter(app => !held.has(app.id));
  const updates = onReader.filter(app => app.update_available && app.runs_on_this_reader);
  return {
    installed: onReader.filter(matches).sort(byTitle),
    available: available.filter(matches).sort(byTitle),
    updates: updates.map(app => app.id),
    total: onReader.length + available.length,
  };
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
  const named = String(hash ?? "").replace(/^#/, "");
  return PLACES.includes(named) ? named : "reader";
}
