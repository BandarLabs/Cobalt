// The page: it draws what the server says and asks the server to do things.
//
// There is no framework and no build step. Each place is drawn again from
// the page's own small record of what it knows whenever that record changes,
// and everything that decides anything is in logic.mjs, where it is tested.
// Text from the reader, the catalog and the commands is only ever added as
// text, never as markup.

import {
  PLACES,
  absence,
  afterFailure,
  afterwards,
  appFacts,
  appStanding,
  browserStep,
  chosenReader,
  elapsed,
  failure,
  fileSize,
  galleryStep,
  installingNeeds,
  linked,
  pictureDue,
  planStep,
  readerStanding,
  reflow,
  routeFor,
  runningJob,
  sentence,
  storeApps,
  storeShelves,
  tileStanding,
  uploadRefusal,
  webAddress,
} from "/logic.mjs";
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
} from "/path.mjs";

const token = document.querySelector('meta[name="kobo-token"]').content;

const view = {
  place: routeFor(location.hash).place,
  app: routeFor(location.hash).app,
  store: null,
  lost: false,
  misses: 0,
  state: null,
  stateText: "",
  volume: null,
  reader: { plan: null, job: null, error: "", notice: "", adding: false, renaming: null, forgetting: null, corner: null, sought: false, unconnectedSince: null, unplugSince: null },
  apps: { key: "", installed: null, catalog: null, problem: "", loading: false, query: "", job: null, removing: null, said: "", changed: false },
  browser: { list: null, problem: "", busy: "", said: {} },
  send: { info: null, reader: "", job: null, jobFor: "", file: { stage: "idle" }, address: { problem: "", said: "" }, queue: { said: "" } },
};

// Talking to the server.

class Refused extends Error {
  constructor(message, status) {
    super(message);
    this.status = status;
  }
}

// Nothing waits for ever. A question is answered in well under a second
// when the server is there at all, so one that has had no answer in the
// time allowed is treated as unanswered and said to be.
async function answered(path, options, patience) {
  let response;
  try {
    response = await fetch(path, { ...options, signal: AbortSignal.timeout(patience) });
  } catch (error) {
    const waited = error?.name === "TimeoutError";
    throw new Refused(
      waited
        ? `No answer came in ${elapsed(patience)}. The kobo ui command may be busy or may have stopped; try again.`
        : "This window could not reach the kobo ui command behind it.",
      0
    );
  }
  const body = await response.json().catch(() => null);
  if (response.status === 401) {
    lose();
    throw new Refused("", 401);
  }
  if (!response.ok) {
    const refused = new Refused(body?.error || `The request was refused (${response.status}).`, response.status);
    refused.kind = body?.kind ?? "failed";
    throw refused;
  }
  return body ?? {};
}

const ask = (path, patience = 20_000) => answered(path, { cache: "no-store" }, patience);

// Changes are given longer than questions: fetching the catalog is a
// download, and setting up a browser runs a command.
const change = (path, body = {}, patience = 90_000) =>
  answered(
    path,
    {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Kobo-Token": token },
      body: JSON.stringify(body),
    },
    patience
  );

// A file goes up through the older request object because it is the one
// that says how much has gone.
function upload(path, file, progressed) {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest();
    request.open("POST", path);
    request.setRequestHeader("X-Kobo-Token", token);
    request.setRequestHeader("Content-Type", "application/octet-stream");
    request.upload.onprogress = event => progressed(event.loaded, event.total);
    request.onerror = () => reject(new Refused("The file could not be handed over. Check that it is still where it was and try again.", 0));
    request.onload = () => {
      let body = null;
      try { body = JSON.parse(request.responseText); } catch { body = null; }
      if (request.status === 200) resolve(body ?? {});
      else reject(new Refused(body?.error || `The file was refused (${request.status}).`, request.status));
    };
    request.send(file);
  });
}

// Building the document.

function h(tag, props, ...children) {
  const element = document.createElement(tag);
  for (const [name, value] of Object.entries(props ?? {})) {
    if (value == null || value === false) continue;
    if (name === "class") element.className = value;
    else if (name.startsWith("on")) element.addEventListener(name.slice(2), value);
    else if (name in element) element[name] = value;
    else element.setAttribute(name, value === true ? "" : value);
  }
  element.append(...children.flat(Infinity).filter(child => child != null && child !== false));
  return element;
}

const button = (label, onclick, more = {}) => h("button", { type: "button", class: "button", onclick, ...more }, label);
const primary = (label, onclick, more = {}) => button(label, onclick, { class: "button primary", ...more });
const plain = (label, onclick) => h("button", { type: "button", class: "plain", onclick }, label);

// A sentence with any web address in it made a link that opens outside.
const prose = text =>
  linked(text).map(part => (part.href ? h("a", { href: part.href, target: "_blank", rel: "noopener noreferrer" }, part.text) : part.text));

// A path, allowed to wrap after each of its slashes and nowhere stranger.
const pathText = path => path.split("/").flatMap((piece, index) => (index === 0 ? [piece] : ["/", h("wbr"), piece]));

const note = (text, wrong = false) => (text ? h("div", { class: wrong ? "note wrong" : "note" }, h("p", null, prose(text))) : null);

// What a command said, as headings, paragraphs and lists. Takes the text,
// or blocks already taken from it.
function said(text) {
  const out = h("div", { class: "said" });
  let list = null;
  const into = (tag, className) => {
    if (!list || list.dataset.kind !== tag) {
      list = h(tag, { class: className });
      list.dataset.kind = tag;
      out.append(list);
    }
    return list;
  };
  for (const block of typeof text === "string" ? reflow(text) : text) {
    if (block.kind === "step") { into("ol").append(h("li", { value: block.number }, prose(block.text))); continue; }
    if (block.kind === "item") { into("ul").append(h("li", null, prose(block.text))); continue; }
    if (block.kind === "fact") { into("dl", "facts").append(h("dt", null, block.name), h("dd", null, block.value)); continue; }
    list = null;
    if (block.kind === "heading") out.append(h("h3", null, block.text));
    else if (block.kind === "code") out.append(h("pre", null, block.text));
    else out.append(h("p", { class: block.kind === "error" ? "error" : null }, prose(block.text)));
  }
  return out;
}

// Jobs.

// Follows a job to its end, drawing again each time it has said more.
function follow(id, draw, finished) {
  const job = { id, title: "", text: "", next: 0, running: true, ok: null, volume: null, started: Date.now() };
  const look = async () => {
    try {
      const report = await ask(`/api/job?id=${id}&from=${job.next}`);
      Object.assign(job, { title: report.title, text: job.text + report.text, next: report.next, running: report.running, ok: report.ok, volume: report.volume ?? job.volume, setup: report.setup ?? job.setup });
    } catch (error) {
      if (error.status === 404) Object.assign(job, { running: false, ok: false, text: `${job.text}\nkobo: this job is no longer remembered\n` });
    }
    draw();
    if (job.running && !view.lost) setTimeout(look, 700);
    else if (!job.running) finished?.(job);
  };
  look();
  return job;
}

// A job as a place shows it: going, done, or failed with its reason.
//
// What it said is led by the steps for the owner once it has got to them,
// with the report of what it did folded away above, so that somebody
// waiting to be told what to do with the reader is told first.
function jobView(job, { done, waiting, extra, after, back, again }) {
  if (!job.running && !job.ok) {
    return [
      h("h2", null, `${job.title} did not finish.`),
      note(sentence(failure(job.text)) || "The command stopped without saying why.", true),
      after && h("p", { class: "reading" }, after),
      h("details", { class: "whole" }, h("summary", null, "Everything it said"), said(job.text)),
      h("div", { class: "actions" }, again && primary("Try again", again), button("Back", back)),
    ];
  }
  const { report, next } = afterwards(reflow(job.text));
  return [
    h("h2", null, job.running ? job.title || "Working" : done),
    job.running && h("div", { class: "working", role: "progressbar", "aria-label": job.title }),
    job.running && h("p", { class: "quiet" }, report.length === 0 ? `${waiting} ` : "", `${elapsed(Date.now() - job.started)} so far.`),
    !job.running && extra,
    said(next),
    report.length > 0 && h("details", { class: "whole" }, h("summary", null, "What was done to the reader"), said(report)),
    !job.running && h("div", { class: "actions" }, primary("Done", back)),
  ];
}

// The Reader place.

const slots = Object.fromEntries(PLACES.map(place => [place, document.querySelector(`#place-${place} [data-body]`)]));
const DONE = {
  "Installing Cobalt": "Cobalt is installed.",
  "Installing Cobalt again": "Cobalt is installed again.",
  "Updating Cobalt": "Cobalt is updated.",
  "Removing Cobalt": "Cobalt is removed.",
};

function drawReader() {
  const local = view.reader;
  const slot = slots.reader;
  if (local.job && isArrival(local.job)) return slot.replaceChildren(...arrivalView(local.job).flat().filter(Boolean));
  if (local.job) {
    const back = () => { local.job = null; local.error = ""; drawReader(); refresh(); };
    // A reader still mounted after a finished job has not seen what was
    // written to it, and pulling the cable now can lose the end of it.
    const reader = view.state?.readers.find(found => found.volume === local.job.volume);
    const extra = reader && h("div", { class: "note" },
      h("p", null, "The reader is still connected. Eject it before you unplug it. ",
        view.state.computer.ejects && plain("Eject it now", () => eject(reader))));
    return slot.replaceChildren(...jobView(local.job, {
      done: DONE[local.job.title] ?? "Done.",
      waiting: "Leave the reader plugged in.",
      extra,
      after: afterFailure(local.job, view.state),
      back,
      again: local.job.again,
    }).filter(Boolean));
  }
  if (!view.state) return slot.replaceChildren(h("p", { class: "quiet" }, "Looking for a reader."));
  const chosen = chosenReader(view.state.readers, view.volume);
  if (chosen.kind === "none") return slot.replaceChildren(...awayView());
  if (chosen.kind === "several") return slot.replaceChildren(...severalReaders(chosen.readers));
  if (local.plan?.volume === chosen.reader.volume) return slot.replaceChildren(...planView(chosen.reader, local.plan).filter(Boolean));
  local.plan = null;
  slot.replaceChildren(...oneReader(chosen).filter(Boolean));
}

function noReader() {
  const missing = absence(view.state);
  const mac = view.state.computer.os === "macos";
  if (missing.kind === "blocked") {
    return [
      h("h2", null, "This computer is not letting Cobalt read the reader."),
      h("p", { class: "reading" }, mac ? "A Kobo is connected, and macOS refused to let this read it." : "A Kobo is connected, and the system refused to let this read it."),
      mac && h("ol", { class: "steps reading" },
        h("li", null, "Open System Settings, then Privacy & Security, then Files and Folders."),
        h("li", null, "Find the terminal app you ran kobo ui from and turn on Removable Volumes."),
        h("li", null, "Unplug the reader, plug it in again and tap Connect.")),
    ].filter(Boolean);
  }
  if (missing.kind === "unconnected") {
    // How long it has been on the cable without connecting decides what is
    // said, so the page is drawn again when that changes.
    const local = view.reader;
    local.unconnectedSince ??= Date.now();
    const waited = Date.now() - local.unconnectedSince;
    if (waited < CONNECT_PATIENCE) setTimeout(drawReader, CONNECT_PATIENCE - waited + 50);
    const advice = connecting(waited);
    return [
      h("h2", null, advice.title),
      advice.causes.length > 0 && h("ul", { class: "reading causes" }, advice.causes.map(cause => h("li", null, cause))),
    ].filter(Boolean);
  }
  view.reader.unconnectedSince = null;
  return [
    note(view.reader.notice),
    h("h2", null, "Plug in your Kobo."),
    h("p", { class: "reading" }, "Use a cable that carries data. Some only charge."),
  ].filter(Boolean);
}

function severalReaders(readers) {
  return [
    h("h2", null, `${readers.length} readers are connected.`),
    h("ul", { class: "entries" }, readers.map(reader =>
      h("li", null,
        h("div", null, h("span", { class: "name" }, reader.model), h("span", { class: "version" }, `firmware ${reader.firmware}`)),
        h("p", { class: "about path" }, pathText(reader.volume)),
        h("div", { class: "does" }, button("Choose", () => { view.volume = reader.volume; view.reader.notice = ""; drawReader(); drawApps(); }))))),
  ];
}

// One reader: its name, what is true of it as a table, and what can be done.
// A sentence appears only when it is the reason something cannot be done,
// or the code's own account of what untested means for this reader.
function oneReader({ reader, among }) {
  const computer = view.state.computer;
  const local = view.reader;
  const standing = readerStanding(reader, computer);
  const cobalt = reader.cobalt ? `${reader.cobalt.cobalt_version}, ${reader.cobalt.channel} channel` : "Not installed";
  const plan = undo => openPlan(reader, undo);
  const locked = reader.writable === false;
  // A reader with Cobalt that this computer never saved is linked by the
  // same install, which is what gives this computer its way in.
  const linking = needsLinking(reader) && standing.repair && !locked;
  return [
    h("h2", null, reader.model),
    h("dl", { class: "facts" },
      h("dt", null, "Firmware"), h("dd", null, reader.firmware),
      h("dt", null, "Cobalt"), h("dd", null, cobalt),
      computer.release && [h("dt", null, "This computer has"), h("dd", null, `Cobalt ${computer.release}`)],
      Number.isFinite(reader.free_bytes) && [h("dt", null, "Free space"), h("dd", null, fileSize(reader.free_bytes))],
      reader.saved_as && [h("dt", null, "Saved as"), h("dd", null, reader.saved_as)]),
    note(standing.reason, !reader.supported || Boolean(reader.cobalt_problem)),
    linking && note("This computer has not been set up to send to this Kobo."),
    standing.primary && !locked && note(sentence(reader.untested)),
    locked && note("This reader's storage is mounted read-only, so nothing can be written to it. Eject it, unplug it, restart the reader and connect it again.", true),
    note(local.error, true),
    h("div", { class: "actions" },
      !locked && standing.primary === "install" && primary("Install Cobalt", () => plan(false)),
      !locked && standing.primary === "update" && primary(`Update to ${computer.release}`, () => plan(false)),
      linking && primary("Link this computer", () => plan(false)),
      !locked && standing.repair && !linking && button("Install again", () => plan(false)),
      !locked && standing.removable && button("Remove Cobalt", () => plan(true)),
      computer.ejects && button("Eject", () => eject(reader))),
    (reader.cobalt || among) && h("p", { class: "after" },
      reader.cobalt && h("a", { href: "#apps" }, "The apps on this reader"),
      among && plain("Choose another reader", () => { view.volume = null; drawReader(); drawApps(); })),
  ];
}

async function openPlan(reader, undo) {
  const local = view.reader;
  local.error = "";
  try {
    const plan = await ask(`/api/plan?volume=${encodeURIComponent(reader.volume)}${undo ? "&undo=1" : ""}`);
    local.plan = { ...plan, volume: reader.volume };
  } catch (error) {
    local.error = error.message;
  }
  drawReader();
}

function planView(reader, plan) {
  const local = view.reader;
  const release = view.state.computer.release;
  const verb = plan.undo ? "Remove Cobalt" : needsLinking(reader) && reader.standing === "current" ? "Link this computer" : reader.cobalt ? `Update to ${release}` : `Install Cobalt ${release}`;
  const path = plan.undo ? "/api/reader/remove" : "/api/reader/install";
  return [
    h("h2", null, plan.undo ? "What removing Cobalt changes" : `What installing Cobalt ${release} changes`),
    h("p", { class: "quiet" }, reader.model),
    h("ul", { class: "reading causes" }, plainPlan(reader, view.state.computer, plan.undo).map(line => h("li", null, line))),
    note(sentence(plan.untested)),
    h("details", { class: "whole" }, h("summary", null, "Exactly what changes"),
      h("ol", { class: "plan" }, plan.lines.map(line => h("li", null, planStep(line))))),
    note(local.error, true),
    h("div", { class: "actions" }, primary(verb, () => startReader(path, reader.volume)), button("Cancel", () => { local.plan = null; local.error = ""; drawReader(); })),
  ];
}

async function startReader(path, volume) {
  const local = view.reader;
  local.error = "";
  try {
    const { job } = await change(path, { volume });
    local.plan = null;
    local.notice = "";
    local.sought = false;
    local.corner = view.state?.readers.find(reader => reader.volume === volume)?.menu_corner ?? null;
    local.job = follow(job, drawReader, refresh);
    local.job.volume = volume;
    // Known to be an install before its first report says so, so the page
    // goes straight to its steps.
    if (path === "/api/reader/install") local.job.setup = { stage: "writing", notes: [] };
    local.job.again = () => { local.job = null; startReader(path, volume); };
  } catch (error) {
    local.error = error.message;
  }
  drawReader();
}

async function eject(reader) {
  const local = view.reader;
  local.error = "";
  try {
    await change("/api/reader/eject", { volume: reader.volume });
    local.notice = `The ${reader.model} is ejected and can be unplugged.`;
    view.apps.changed = false;
    await refresh();
  } catch (error) {
    local.error = error.message;
  }
  drawReader();
  drawApps();
}

// The way in.
//
// A first reader is walked from the cable to the first thing sent to it,
// one step on the page at a time. Which step it is comes from path.mjs,
// which reads it from what the server reports, so a page closed half way
// opens again on the same step. The command's own account of what it did
// is kept, behind a disclosure, for whoever wants it.

const isArrival = job => Boolean(job.setup) || /^(Installing Cobalt|Installing Cobalt again|Updating Cobalt|Looking for the reader)$/.test(job.title ?? "");

// The four things an install does, each marked with how far it has got.
function advance(stage) {
  const SAID = { done: "done", doing: "in progress", yours: "for you to do", stopped: "stopped here", todo: "" };
  return h("ol", { class: "advance" }, installSteps(stage).map(step =>
    h("li", { class: step.state, "aria-current": step.state === "doing" || step.state === "yours" ? "step" : null },
      h("span", null, step.text, SAID[step.state] && h("span", { class: "unseen" }, `, ${SAID[step.state]}`)))));
}

async function startFind() {
  const local = view.reader;
  local.error = "";
  try {
    const { job } = await change("/api/reader/find", {});
    local.job = follow(job, drawReader, refresh);
    Object.assign(local.job, { title: "Looking for the reader", setup: { stage: "finding", waited: 0, limit: 300, notes: [] } });
  } catch (error) {
    local.error = error.message;
  }
  drawReader();
}

// Ejects the reader an install has finished copying to, then looks for it.
async function ejectThenFind(volume) {
  const local = view.reader;
  local.error = "";
  try {
    await change("/api/reader/eject", { volume });
    await refresh();
    return startFind();
  } catch (error) {
    local.error = error.message;
  }
  drawReader();
}

function arrivalView(job) {
  const local = view.reader;
  const setup = job.setup ?? { stage: "writing" };
  const computer = view.state?.computer ?? {};
  // How long the page has been waiting to see the cable come out. It is
  // drawn again when that wait is up, in case nothing else changes.
  if (setup.stage === "finding") {
    if (!local.unplugSince) setTimeout(drawReader, UNPLUG_PATIENCE + 50);
    local.unplugSince ??= Date.now();
  } else {
    local.unplugSince = null;
  }
  const { scene, title, plugged } = arrivalScene(job, view.state, local.unplugSince ? Date.now() - local.unplugSince : 0);
  const back = () => { local.job = null; local.error = ""; local.adding = false; drawReader(); refresh(); };
  const whole = job.text ? h("details", { class: "whole" }, h("summary", null, "Everything the command said"), said(job.text)) : null;
  const notes = (setup.notes ?? []).map(text => note(text));
  const lookAgain = h("div", { class: "actions" }, primary("Look again", startFind), button("Not now", back));
  const steps = advance(setup.stage ?? "writing");

  if (scene === "failed") {
    return [
      h("h2", null, title),
      note(sentence(failure(job.text)) || "It stopped without saying why.", true),
      afterFailure(job, view.state) && h("p", { class: "reading" }, afterFailure(job, view.state)),
      whole,
      h("div", { class: "actions" }, job.again && primary("Try again", job.again), button("Back", back)),
    ];
  }
  if (scene === "writing") {
    return [
      h("h2", null, title),
      h("div", { class: "working", role: "progressbar", "aria-label": title }),
      steps,
      h("p", { class: "quiet" }, job.title === "Looking for the reader" ? "" : `${copying(Date.now() - job.started)} `, `${elapsed(Date.now() - job.started)} so far.`),
      whole,
    ];
  }
  if (scene === "eject") {
    // Ejected by hand, or by the system's own button: the reader going is
    // the thing to see, and the search starts by itself.
    const mounted = view.state?.readers.some(reader => reader.volume === job.volume);
    if (!mounted && !local.error && !local.sought) { local.sought = true; startFind(); }
    return [
      h("h2", null, title),
      steps,
      note(local.error, true),
      computer.ejects
        ? h("div", { class: "actions" }, primary("Eject", () => ejectThenFind(job.volume)))
        : h("p", { class: "reading" }, "Eject it the way this computer ejects a USB drive. This page carries on when it has gone."),
      whole,
    ];
  }
  if (scene === "unplug") return [h("h2", null, title), steps, whole];
  if (scene === "on_kobo") {
    const search = looking(setup);
    return [
      h("h2", null, title),
      h("ol", { class: "steps reading" }, onTheKobo(local.corner, plugged).map(step => h("li", null, step))),
      h("progress", { max: search.limit, value: search.waited, "aria-label": "Looking for the Kobo on Wi-Fi" }),
      h("p", { class: "quiet" }, `Looking for it on Wi-Fi, ${search.text}.`),
      note(stillLooking(setup)),
      whole,
    ];
  }
  if (scene === "securing") return [h("h2", null, title), h("div", { class: "working", role: "progressbar", "aria-label": "Saving the Kobo" }), steps, whole];
  if (scene === "ready") {
    return [
      h("h2", null, title),
      notes,
      h("ul", { class: "entries next" }, NEXT_THINGS.map(thing =>
        h("li", null, h("div", null, h("a", { class: "name", href: thing.href, onclick: () => { local.job = null; } }, thing.label))))),
      h("div", { class: "actions" }, button("Done", back)),
      whole,
    ].flat();
  }
  if (scene === "not_found") {
    return [
      h("h2", null, title),
      h("ul", { class: "reading causes" }, notFoundCauses(computer).map(cause => h("li", null, cause))),
      note(local.error, true),
      lookAgain,
      whole,
    ];
  }
  const what = scene === "several"
    ? "Give it a minute to settle on one, then look again."
    : "Join the Wi-Fi network the Kobo is on, then look again.";
  return [h("h2", null, title), h("p", { class: "reading" }, what), note(local.error, true), lookAgain, whole];
}

// The Reader place with nothing plugged in and nothing going on: the way in
// for a first reader, the rest of the way for one that was set up and not
// found, or the readers this computer already knows.
function awayView() {
  const local = view.reader;
  const kind = withoutReader(view.state, local.adding);
  if (kind === "waiting") {
    return [
      note(local.notice),
      h("h2", null, "Finish on the Kobo."),
      h("ol", { class: "steps reading" }, onTheKobo(local.corner).map(step => h("li", null, step))),
      note(local.error, true),
      h("div", { class: "actions" }, primary("Look for it on Wi-Fi", startFind)),
    ].filter(Boolean);
  }
  if (kind === "saved") {
    const act = async (path, body) => {
      local.error = "";
      try {
        await change(path, body);
        Object.assign(local, { renaming: null, forgetting: null });
        await refresh();
      } catch (error) {
        local.error = error.message;
      }
      drawReader();
    };
    return [
      note(local.notice),
      h("h2", null, "Saved on this computer"),
      note(local.error, true),
      h("ul", { class: "entries level" }, view.state.saved.map(reader => {
        if (local.renaming === reader.name) {
          const field = h("input", { class: "field", value: reader.name, maxLength: 40, "aria-label": `A new name for ${reader.name}` });
          return h("li", null,
            h("form", { class: "row", onsubmit: event => { event.preventDefault(); act("/api/readers/rename", { name: reader.name, to: field.value.trim() }); } },
              field, h("button", { type: "submit", class: "button primary" }, "Save")),
            h("div", { class: "does" }, button("Cancel", () => { local.renaming = null; local.error = ""; drawReader(); })));
        }
        const asking = local.forgetting === reader.name;
        return h("li", null,
          h("div", null, h("span", { class: "name" }, reader.name)),
          asking && h("p", { class: "about" }, `Forget “${reader.name}”? Nothing on the Kobo changes.`),
          h("div", { class: "does" }, asking
            ? [button("Forget", () => act("/api/readers/forget", { name: reader.name })), button("Keep", () => { local.forgetting = null; drawReader(); })]
            : [button("Rename", () => { Object.assign(local, { renaming: reader.name, forgetting: null, error: "" }); drawReader(); }),
               button("Forget", () => { Object.assign(local, { forgetting: reader.name, renaming: null, error: "" }); drawReader(); })]));
      })),
      h("div", { class: "actions" }, button("Add another reader", () => { local.adding = true; drawReader(); })),
    ].filter(Boolean);
  }
  return [...noReader(), local.adding && h("p", { class: "after" }, plain("Cancel", () => { local.adding = false; drawReader(); }))].filter(Boolean);
}

// Pictures.
//
// Every picture comes from this server, which fetches it from the published
// site the first time and keeps it. A frame asks for its picture when it
// comes near the window, and three are asked for at a time, so that a store
// of forty apps does not queue forty requests in front of the one that says
// whether a reader was just plugged in. A frame is the reader's shape from
// the moment it is drawn, with or without its picture: empty while it
// waits, and marked with the app's initial when there is no picture to
// show. Nothing spins, because nothing here knows a picture is coming.

const pictures = { known: new Map(), waiting: [], going: 0 };
const shotAddress = name => `/api/shot?name=${encodeURIComponent(name)}`;

const nearWindow = new IntersectionObserver(entries => {
  for (const entry of entries) {
    if (!entry.isIntersecting) continue;
    nearWindow.unobserve(entry.target);
    pictures.waiting.push(entry.target);
  }
  fetchPictures();
}, { rootMargin: "300px" });

// A frame for one picture of an app, or for an app that has none.
//
// A frame on a shelf waits until it is near the window. One on an app's
// page is `eager`: there are a handful, they are what the page is for, and
// the ones further along the row would otherwise be found empty by
// whoever scrolls to them.
function shotFrame(shot, title, tag = "span", eager = false) {
  const frame = h(tag, { class: "shot" }, h("span", { class: "initial", "aria-hidden": "true" }, title.trim().charAt(0).toUpperCase()));
  if (!shot) {
    frame.classList.add("missing");
    return frame;
  }
  frame.dataset.shot = shot.name;
  frame.dataset.alt = shot.caption;
  if (pictures.known.get(shot.name)?.state === "here") settleFrame(frame);
  else if (eager) pictures.waiting.push(frame);
  else nearWindow.observe(frame);
  return frame;
}

// Draws a frame as what is now known of its picture.
function settleFrame(frame) {
  const record = pictures.known.get(frame.dataset.shot);
  if (record?.state === "here" && !frame.querySelector("img")) {
    frame.classList.remove("missing");
    frame.append(h("img", { src: shotAddress(frame.dataset.shot), alt: frame.dataset.alt, decoding: "async", draggable: false }));
  } else if (record?.state === "failed") {
    frame.classList.add("missing");
  }
}

function fetchPictures() {
  while (pictures.going < 3 && pictures.waiting.length > 0) {
    const frame = pictures.waiting.shift();
    if (!frame.isConnected) continue;
    const name = frame.dataset.shot;
    if (!pictureDue(pictures.known.get(name), Date.now())) {
      settleFrame(frame);
      continue;
    }
    pictures.going += 1;
    pictures.known.set(name, { state: "asked", at: Date.now() });
    const picture = new Image();
    const settle = state => {
      pictures.going -= 1;
      pictures.known.set(name, { state, at: Date.now() });
      // Every frame showing this picture, not only the one that asked.
      for (const shown of document.querySelectorAll(".shot[data-shot]")) {
        if (shown.dataset.shot === name) settleFrame(shown);
      }
      notePictures();
      fetchPictures();
    };
    picture.onload = () => settle("here");
    picture.onerror = () => settle("failed");
    picture.src = shotAddress(name);
  }
}

// Says once, where the pictures would be, that they could not be fetched:
// in the store when none has arrived and one has failed, which is a
// computer that is offline, and on an app's page when one of its own did.
function notePictures() {
  const states = [...pictures.known.values()].map(record => record.state);
  const none = states.includes("failed") && !states.includes("here");
  for (const line of document.querySelectorAll("[data-pictures]")) {
    const names = line.dataset.pictures.split(" ").filter(Boolean);
    line.hidden = names.length === 0 ? !none : !names.some(name => pictures.known.get(name)?.state === "failed");
  }
}

const pictureNote = names => h("p", { class: "quiet", hidden: true, "data-pictures": names.join(" ") }, "The pictures could not be fetched.");

// One picture at a time, at its full size, for reading what is on the
// screen in it. The arrow keys move through an app's pictures and stop at
// each end. What it is and the way out of it stay at the top while the
// picture scrolls under them, because a reader's screen at full size is
// taller than most windows.
const closer = { shots: [], title: "", index: 0 };
closer.frame = h("div", { class: "full" });
closer.scroll = h("div", { class: "scroll" }, closer.frame);
closer.caption = h("p", { class: "caption" });
closer.count = h("span", { class: "quiet count" });
closer.back = button("Previous", () => lookCloser(closer.index - 1));
closer.on = button("Next", () => lookCloser(closer.index + 1));
closer.dialog = h("dialog", { class: "closer" },
  h("div", { class: "head" },
    closer.caption,
    h("div", { class: "actions" }, closer.back, closer.count, closer.on, button("Close", () => closer.dialog.close()))),
  closer.scroll);
closer.dialog.addEventListener("keydown", event => {
  if (event.key === "ArrowLeft") lookCloser(closer.index - 1);
  if (event.key === "ArrowRight") lookCloser(closer.index + 1);
});
// A click outside the picture closes it, as a click on the dimmed page
// behind any sheet does.
closer.dialog.addEventListener("click", event => { if (event.target === closer.dialog) closer.dialog.close(); });
document.body.append(closer.dialog);

// How wide a picture is when each of its pixels is one of the screen's.
const fullWidth = shot => Math.round((shot.width || 1072) / (window.devicePixelRatio || 1));
// How wide an app's page draws its pictures: half the reader's panel, which
// is every pixel of it on a dense screen, and a quarter in a narrow window.
const shownWidth = () => (matchMedia("(max-width: 760px)").matches ? 268 : 536);

function lookCloser(index) {
  closer.index = galleryStep(index, 0, closer.shots.length);
  const shot = closer.shots[closer.index];
  closer.dialog.setAttribute("aria-label", `${closer.title}, picture ${closer.index + 1} of ${closer.shots.length}`);
  closer.frame.style.width = `${fullWidth(shot)}px`;
  closer.frame.replaceChildren(shotFrame(shot, closer.title, "span", true));
  closer.scroll.scrollTop = 0;
  fetchPictures();
  closer.caption.textContent = shot.caption;
  closer.count.textContent = `${closer.index + 1} of ${closer.shots.length}`;
  closer.back.disabled = closer.index === 0;
  closer.on.disabled = closer.index === closer.shots.length - 1;
  closer.back.hidden = closer.on.hidden = closer.count.hidden = closer.shots.length < 2;
  if (!closer.dialog.open) closer.dialog.showModal();
}

// The Apps place.
//
// A store. With no reader it is still all there to read, from what the
// command was built with; a reader adds what it holds and what the signed
// catalog offers it today. The shelves are for looking and each app's page
// is where anything is done to it.

const appsTitle = document.getElementById("title-apps");
const appsBack = h("p", { class: "back", hidden: true }, h("a", { href: "#apps" }, "All apps"));
appsTitle.before(appsBack);

const search = h("input", {
  class: "field",
  type: "search",
  id: "find-app",
  placeholder: "Name or what it is for",
  autocomplete: "off",
  oninput: () => { view.apps.query = search.value; drawShelves(); },
});
const appsHead = h("div");
const appsTools = h("div", { class: "toolbar" }, h("div", { class: "grow" }, h("label", { class: "over", htmlFor: "find-app" }, "Find an app"), search));
const appsBody = h("div");
slots.apps.append(appsHead, appsTools, appsBody);

// The reader the Apps place is about, when it is one with Cobalt on it.
function appsReader() {
  const chosen = view.state ? chosenReader(view.state.readers, view.volume) : { kind: "none" };
  return chosen.kind === "one" && chosen.reader.cobalt ? chosen.reader : null;
}

function drawApps() {
  const local = view.apps;
  const reader = appsReader();
  const key = reader ? `${reader.volume} ${reader.cobalt.cobalt_version}` : "";
  if (key !== local.key) {
    Object.assign(local, { key, installed: null, catalog: null, problem: "", removing: null, said: "" });
    if (reader && view.place === "apps") loadApps();
  }
  const chosen = view.state ? chosenReader(view.state.readers, view.volume) : { kind: "none" };
  const busy = Boolean(local.job?.running);
  const needs = reader ? "" : installingNeeds(chosen, view.state);
  // Which reader this is about is said over the shelves. On an app's page
  // it is said only when more than one is connected, which is the one time
  // the button under the app's name could be about the wrong one.
  const naming = reader && (!view.app || view.state.readers.length > 1);
  appsHead.replaceChildren(...[
    naming && h("p", { class: "quiet" }, `${reader.model}, Cobalt ${reader.cobalt.cobalt_version}, ${reader.cobalt.channel} catalog.`),
    needs && h("p", { class: "quiet" }, needs),
    !view.app && pictureNote([]),
    local.job && h("div", { class: "strip" },
      h("p", null, h("strong", null, busy ? local.job.title : local.job.ok ? "Finished." : "Not everything was installed.")),
      busy && h("div", { class: "working", role: "progressbar", "aria-label": local.job.title }),
      said(local.job.text),
      !busy && h("div", { class: "actions" }, button("Dismiss", () => { local.job = null; drawApps(); }))),
    !local.job && local.said && note(local.said),
    local.problem && h("div", { class: "note wrong" }, h("p", null, prose(local.problem)), h("p", null, plain("Try again", loadApps))),
    local.changed && !busy && reader && h("div", { class: "note" },
      h("p", null, "Eject the reader before you unplug it. ",
        view.state.computer.ejects && plain("Eject it now", () => eject(reader)))),
    reader && !local.catalog && !local.problem && h("p", { class: "quiet" }, local.installed === null ? "Reading the apps on the reader." : "Fetching the signed app catalog."),
  ].filter(Boolean));
  appsBack.hidden = !view.app;
  appsTools.hidden = Boolean(view.app) || !view.store;
  if (view.app) drawAppPage();
  else drawShelves();
}

// One app in a shelf: its picture, its name, one line, and a few words
// about this reader when there is something to say. The whole of it is a
// link to the app's page.
function tile(app, standing) {
  return h("li", { class: "tile" },
    h("a", { href: `#apps/${app.id}` },
      shotFrame(app.shots[0], app.title),
      h("span", { class: "name" }, app.title),
      h("span", { class: "about" }, app.summary),
      standing && h("span", { class: "standing" }, standing)));
}

function drawShelves() {
  const local = view.apps;
  if (view.app) return;
  appsTitle.textContent = "Apps";
  if (!view.store) return appsBody.replaceChildren();
  const reader = appsReader();
  const shelves = storeShelves(view.store, reader ? local.installed : null, reader ? local.catalog : null, local.query);
  const typed = local.query.trim();
  const busy = Boolean(local.job?.running) || local.loading;
  if (typed !== "" && shelves.found === 0) {
    return appsBody.replaceChildren(h("p", { class: "reading" }, `No app matches “${typed}”.`));
  }
  const held = shelves.onReader;
  appsBody.replaceChildren(...[
    typed === "" && h("nav", { class: "jump", "aria-label": "Kinds of app" },
      shelves.groups.map(group => plain(group.title, () => document.getElementById(`group-${group.id}`)?.scrollIntoView({ block: "start" })))),
    held && !(typed !== "" && held.length === 0) && h("section", { class: "part" },
      h("h2", null, "On this reader"),
      shelves.updates.length > 1 && typed === "" && h("p", null, button(`Update all ${shelves.updates.length}`, () => installApps(reader, shelves.updates), { disabled: busy })),
      held.length > 0
        ? h("ul", { class: "shelf" }, held.map(app => tile(app, app.update ? tileStanding(app) : app.installed_version)))
        : h("p", { class: "quiet" }, "No apps are on this reader yet; open one below to install it.")),
    shelves.groups.map(group => h("section", { class: "part", id: `group-${group.id}` },
      h("h2", null, group.title),
      h("ul", { class: "shelf" }, group.apps.map(app => tile(app, tileStanding(app)))))),
  ].flat().filter(Boolean));
  notePictures();
}

// One app's page: its pictures, what it says about itself, what is true of
// it, and the one or two things that can be done with it on this reader.
function drawAppPage() {
  const local = view.apps;
  if (!view.store) return appsBody.replaceChildren();
  const reader = appsReader();
  const app = storeApps(view.store, reader ? local.installed : null, reader ? local.catalog : null).get(view.app);
  if (!app) {
    appsTitle.textContent = "Apps";
    return appsBody.replaceChildren(h("p", { class: "reading" }, `There is no app with the id “${view.app}”.`));
  }
  appsTitle.textContent = app.title;
  document.title = `Cobalt – ${app.title}`;
  const standing = appStanding(app, reader, Boolean(local.catalog));
  const busy = Boolean(local.job?.running) || local.loading;
  const asking = local.removing === app.id;
  const facts = appFacts(app);
  const look = index => { Object.assign(closer, { shots: app.shots, title: app.title }); lookCloser(index); };
  // A picture is something to press only where pressing it shows more: on
  // a screen dense enough to show every pixel already, it is just a picture.
  const closerShowsMore = app.shots.some(shot => fullWidth(shot) > shownWidth());

  appsBody.replaceChildren(...[
    asking
      ? h("div", { class: "note" },
          h("p", null, `Remove ${app.title} from this reader? Its data is kept.`),
          h("div", { class: "actions" },
            button("Remove", () => removeApp(reader, app), { disabled: busy }),
            button("Keep", () => { local.removing = null; drawApps(); })))
      : (standing.primary || standing.removable) && h("div", { class: "actions first" },
          standing.primary === "install" && primary("Install", () => installApps(reader, [app.id]), { disabled: busy }),
          standing.primary === "update" && primary(`Update to ${app.version}`, () => installApps(reader, [app.id]), { disabled: busy }),
          standing.removable && button("Remove", () => { local.removing = app.id; drawApps(); }, { disabled: busy })),
    standing.reason && h("p", { class: "quiet" }, standing.reason),
    // An app with no pictures has no row of them: an empty frame the size
    // of a screen would be the largest thing on its page.
    app.shots.length > 0
      && h("div", { class: "gallery", role: "group", "aria-label": `Pictures of ${app.title}`, tabIndex: closerShowsMore ? -1 : 0 },
          app.shots.map((shot, index) => {
            if (!closerShowsMore) return h("figure", null, shotFrame(shot, app.title, "span", true), h("figcaption", null, shot.caption));
            const frame = shotFrame(shot, app.title, "button", true);
            frame.type = "button";
            frame.setAttribute("aria-label", `${shot.caption}. Picture ${index + 1} of ${app.shots.length}, show at full size`);
            frame.addEventListener("click", () => look(index));
            return h("figure", null, frame, h("figcaption", { "aria-hidden": "true" }, shot.caption));
          })),
    pictureNote(app.shots.map(shot => shot.name)),
    h("p", { class: "reading about-app" }, app.description),
    facts.length > 0 && h("dl", { class: "facts" }, facts.map(([name, value]) => [h("dt", null, name), h("dd", null, value)])),
    app.release_notes && [h("h3", null, `New in ${app.version}`), h("p", { class: "reading" }, app.release_notes)],
    app.access && [
      h("h3", null, "What it can use"),
      app.access.length > 0
        ? h("ul", { class: "reading uses" }, app.access.map(phrase => h("li", null, phrase)))
        : h("p", { class: "reading" }, "Nothing beyond its own storage."),
    ],
    app.setup.length > 0 && [
      h("h3", null, "Setup"),
      h("ol", { class: "steps reading" }, app.setup.map(step => h("li", null,
        prose(step.text),
        step.link && h("p", null, h("a", { href: step.link.url, target: "_blank", rel: "noopener noreferrer" }, step.link.label)),
        step.command && h("p", { class: "row command" }, h("code", null, step.command), copyButton(step.command))))),
    ],
  ].flat(Infinity).filter(Boolean));
  fetchPictures();
  notePictures();
}

async function loadStore() {
  try {
    view.store = await ask("/api/store");
  } catch {
    // Without the store the place still lists nothing wrong; the next
    // look at the state says whether the server has gone.
    return;
  }
  drawApps();
}

async function loadApps() {
  const local = view.apps;
  const reader = appsReader();
  if (!reader || local.loading || local.job?.running) return;
  const key = local.key;
  local.loading = true;
  local.problem = "";
  drawApps();
  try {
    const volume = encodeURIComponent(reader.volume);
    const here = await ask(`/api/apps?volume=${volume}`);
    if (local.key === key) { local.installed = here.apps; drawApps(); }
    const published = await change("/api/apps/catalog", { volume: reader.volume }, 120_000);
    if (local.key === key) local.catalog = published.apps;
  } catch (error) {
    if (local.key === key) local.problem = error.message;
  }
  local.loading = false;
  drawApps();
}

async function installApps(reader, ids) {
  const local = view.apps;
  local.said = "";
  try {
    const { job } = await change("/api/apps/install", { volume: reader.volume, ids });
    local.job = follow(job, drawApps, () => { local.changed = true; loadApps(); });
  } catch (error) {
    local.problem = error.message;
  }
  drawApps();
}

async function removeApp(reader, app) {
  const local = view.apps;
  local.removing = null;
  try {
    const left = await change("/api/apps/remove", { volume: reader.volume, id: app.id });
    local.installed = left.apps;
    local.said = `${app.title} is removed.`;
    local.changed = true;
    loadApps();
  } catch (error) {
    local.problem = error.message;
  }
  drawApps();
}

// The Browser place.

function drawBrowser() {
  const local = view.browser;
  const rows = (local.list ?? []).map(browser => {
    const step = browserStep(browser.step, browser.title);
    const act = verb => setBrowser(browser, verb);
    return h("li", { class: browser.set_up ? "tall" : null },
      h("div", null, h("span", { class: "name" }, browser.title), h("span", { class: "state" }, browser.set_up ? "Set up" : "Not set up")),
      browser.set_up && h("div", { class: "wide step" },
        h("p", null, step.paths.length > 0 ? `One step is left, in ${browser.title} itself. ${step.instruction}` : step.instruction),
        step.paths.map(path => h("p", { class: "row" }, h("span", { class: "path" }, pathText(path)), copyButton(path))),
        step.notes.map(text => h("p", { class: "quiet" }, text))),
      local.said[browser.id] && h("p", { class: "about wide" }, local.said[browser.id]),
      h("div", { class: "does" }, browser.set_up
        ? button("Remove", () => act("remove"), { disabled: local.busy !== "" })
        : button("Set up", () => act("setup"), { disabled: local.busy !== "" })));
  });
  slots.browser.replaceChildren(...[
    note(local.problem, true),
    local.list === null && h("p", { class: "quiet" }, "Looking for browsers."),
    local.list?.length === 0 && h("p", { class: "reading" }, "Install Chrome, Edge, Brave, Chromium or Firefox to use Send to Kobo."),
    rows.length > 0 && h("ul", { class: "entries" }, rows),
  ].filter(Boolean));
}

function copyButton(text) {
  const copy = button("Copy", async () => {
    try {
      await navigator.clipboard.writeText(text);
      copy.textContent = "Copied";
    } catch {
      copy.textContent = "Select it to copy";
    }
    setTimeout(() => { copy.textContent = "Copy"; }, 2000);
  });
  return copy;
}

async function loadBrowsers() {
  try {
    view.browser.list = (await ask("/api/browsers")).browsers;
  } catch (error) {
    view.browser.problem = error.message;
  }
  drawBrowser();
}

async function setBrowser(browser, verb) {
  const local = view.browser;
  Object.assign(local, { busy: browser.id, problem: "" });
  drawBrowser();
  try {
    const done = await change(`/api/browsers/${verb}`, { browser: browser.id });
    local.list = done.browsers;
    // What Send to Kobo is, said once, to the owner who has just asked for
    // it; and after a removal, the one thing that is left for them to do.
    local.said[browser.id] = verb === "remove"
      ? `Remove the extension on ${browser.title}’s own extensions page too.`
      : "Send to Kobo adds Send page, Send link and Send selection to the right-click menu.";
  } catch (error) {
    local.problem = error.message;
  }
  local.busy = "";
  drawBrowser();
}

// The Send place.

const picker = h("input", { type: "file", hidden: true, onchange: () => { if (picker.files[0]) receive(picker.files[0]); picker.value = ""; } });
const address = h("input", { class: "field", type: "url", id: "web-address", placeholder: "https://", autocomplete: "off", spellcheck: false });
const sendTo = h("div", { class: "part" });
const fileArea = h("div");
const addressSaid = h("div");
const queueArea = h("div");
slots.send.append(
  sendTo,
  h("div", { class: "part" }, h("h2", null, "A file"), picker, fileArea),
  h("div", { class: "part" },
    h("h2", null, "A web article"),
    h("form", { noValidate: true, onsubmit: event => { event.preventDefault(); sendAddress(); } },
      h("label", { class: "over", htmlFor: "web-address" }, "The page’s address"),
      h("div", { class: "row" }, address, h("button", { type: "submit", class: "button primary", id: "send-address" }, "Send"))),
    addressSaid),
  h("div", { class: "part" }, h("h2", null, "Waiting for the reader"), queueArea));

function drawSend() {
  const local = view.send;
  const busy = Boolean(local.job?.running);
  const saved = local.info?.readers ?? [];
  sendTo.replaceChildren(...[
    !local.info && h("p", { class: "quiet" }, "Looking for saved readers."),
    local.info && saved.length === 0 && note("No reader is saved on this computer yet, so a file cannot be sent and an article waits. Installing Cobalt from Reader saves one."),
    saved.length === 1 && h("p", { class: "quiet" }, `To “${saved[0].name}”, over Wi-Fi.`),
    saved.length > 1 && h("p", null,
      h("label", { class: "over", htmlFor: "send-to" }, "Send to"),
      h("select", { class: "field", id: "send-to", onchange: event => { local.reader = event.target.value; } },
        h("option", { value: "", selected: local.reader === "" }, "The reader kobo setup saved"),
        saved.map(reader => h("option", { value: reader.name, selected: local.reader === reader.name }, reader.name)))),
  ].filter(Boolean));

  document.getElementById("send-address").disabled = busy;
  const mine = part => (local.jobFor === part ? local.job : null);
  fileArea.replaceChildren(...fileView(local.file, mine("file"), busy).filter(Boolean));
  addressSaid.replaceChildren(...[note(local.address.problem, true), sendStatus(mine("address"))].filter(Boolean));

  const waiting = local.info?.waiting ?? [];
  queueArea.replaceChildren(...[
    waiting.length === 0 && h("p", { class: "quiet" }, "Nothing is waiting."),
    waiting.length > 0 && h("ul", { class: "entries" }, waiting.map(article =>
      h("li", null, h("div", null, h("span", { class: "name" }, article.title)), article.site && h("p", { class: "about" }, article.site)))),
    waiting.length > 0 && h("div", { class: "actions" }, button("Send them now", () => startSend("queue", "/api/send/queue", {}), { disabled: busy })),
    sendStatus(mine("queue")),
  ].filter(Boolean));
}

// How a send is going, or how it went, under the form that started it.
function sendStatus(job) {
  if (!job) return null;
  if (job.running) return h("div", null, h("div", { class: "working", role: "progressbar", "aria-label": job.title }), said(job.text));
  if (job.ok) return said(job.text);
  return h("div", null, note(sentence(failure(job.text)) || "The command stopped without saying why.", true),
    h("details", { class: "whole" }, h("summary", null, "Everything it said"), said(job.text)));
}

function fileView(file, job, busy) {
  const again = () => { view.send.file = { stage: "idle" }; if (!view.send.job?.running) { view.send.job = null; } drawSend(); };
  if (job) return [h("p", null, h("strong", null, job.running ? job.title : job.ok ? "Sent." : `${job.title} did not finish.`)), sendStatus(job), !job.running && h("div", { class: "actions" }, button("Send another file", again))];
  if (file.stage === "uploading") {
    return [h("p", null, `Handing over ${file.name}, ${fileSize(file.size)}.`), h("progress", { max: file.size, value: file.sent })];
  }
  if (file.stage === "asking") {
    const go = more => startSend("file", "/api/send/start", { upload: file.upload, ...more });
    if (file.choices) {
      return [
        h("p", { class: "reading" }, `${file.name} could go to more than one app.`),
        h("ul", { class: "choices" }, file.choices.map(app =>
          h("li", null, h("label", null,
            h("input", { type: "radio", name: "send-app", value: app.name, checked: file.to === app.name, onchange: () => { file.to = app.name; drawSend(); } }),
            h("span", null, h("strong", null, app.title), `, for ${app.purpose}`))))),
        h("div", { class: "actions" }, primary("Send", () => go({ to: file.to }), { disabled: !file.to || busy }), button("Cancel", again)),
      ];
    }
    return [
      h("p", { class: "reading" }, `${file.app.title} keeps one ${file.app.replaces} at a time, so sending ${file.name} replaces the ${file.app.replaces} on the reader.`),
      h("div", { class: "actions" }, primary("Replace it and send", () => go({ to: file.app.name, replace: true }), { disabled: busy }), button("Cancel", again)),
    ];
  }
  const zone = h("div", { class: "drop" },
    note(file.problem, true),
    h("p", null, button("Choose a file", () => picker.click(), { disabled: busy }), " or drag one onto this box."));
  zone.addEventListener("dragover", event => { event.preventDefault(); zone.classList.add("over"); });
  zone.addEventListener("dragleave", () => zone.classList.remove("over"));
  zone.addEventListener("drop", event => { event.preventDefault(); zone.classList.remove("over"); if (event.dataTransfer.files[0] && !busy) receive(event.dataTransfer.files[0]); });
  return [zone];
}

async function receive(chosen) {
  const local = view.send;
  const refusal = uploadRefusal(chosen, view.state?.computer.upload_limit);
  if (refusal) { local.file = { stage: "idle", problem: refusal }; return drawSend(); }
  if (local.jobFor === "file") { local.job = null; local.jobFor = ""; }
  local.file = { stage: "uploading", name: chosen.name, size: chosen.size, sent: 0 };
  drawSend();
  try {
    const got = await upload(`/api/send/file?name=${encodeURIComponent(chosen.name)}`, chosen, sent => { local.file.sent = sent; drawSend(); });
    const question = got.choices || got.app?.replaces;
    local.file = { stage: "asking", name: got.name, upload: got.upload, app: got.app, choices: got.choices, to: null };
    if (!question) return startSend("file", "/api/send/start", { upload: got.upload });
  } catch (error) {
    local.file = { stage: "idle", problem: error.message };
  }
  drawSend();
}

function sendAddress() {
  const local = view.send;
  const { address: wanted, problem } = webAddress(address.value);
  local.address.problem = problem;
  if (!wanted) return drawSend();
  startSend("address", "/api/send/address", { url: wanted }).then(started => { if (started) address.value = ""; });
}

async function startSend(part, path, body) {
  const local = view.send;
  try {
    const { job } = await change(path, { ...body, reader: local.reader });
    local.jobFor = part;
    local.job = follow(job, drawSend, loadSend);
    if (part === "file") local.file = { stage: "idle" };
    drawSend();
    return true;
  } catch (error) {
    if (part === "address") local.address.problem = error.message;
    else if (part === "file") local.file = { stage: "idle", problem: error.message };
    else { local.jobFor = "queue"; local.job = { title: "Sending waiting articles", running: false, ok: false, text: `kobo: ${error.message}\n` }; }
    drawSend();
    return false;
  }
}

async function loadSend() {
  try {
    view.send.info = await ask("/api/send");
  } catch {
    view.send.info ??= { readers: [], waiting: [] };
  }
  drawSend();
}

// The whole page.

function show({ place, app }) {
  // Coming back to the shelves from an app's page lands where the owner
  // was looking, and an app's page opens at its top.
  const leavingShelves = view.place === "apps" && !view.app;
  if (leavingShelves) view.shelvesAt = scrollY;
  const returning = place === "apps" && !app && view.app;
  view.place = place;
  view.app = app;
  document.querySelector("main").dataset.place = place;
  for (const name of [...PLACES, "lost"]) document.getElementById(`place-${name}`).hidden = view.lost ? name !== "lost" : name !== place;
  for (const link of document.querySelectorAll(".places a")) {
    if (!view.lost && link.dataset.place === place) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  if (view.lost) return;
  document.title = `Cobalt – ${place[0].toUpperCase()}${place.slice(1)}`;
  if (place === "apps") {
    drawApps();
    if (appsReader() && view.apps.installed === null) loadApps();
    scrollTo(0, returning ? view.shelvesAt ?? 0 : app ? 0 : scrollY);
  }
  if (place === "browser") loadBrowsers();
  if (place === "send") loadSend();
}

function lose() {
  if (view.lost) return;
  view.lost = true;
  show(view);
}

// Reads the state, and draws again only when something in it changed, so a
// page that is being read or typed into is not redrawn under the owner.
async function refresh() {
  if (view.lost) return;
  let state;
  try {
    state = await ask("/api/state");
    view.misses = 0;
  } catch {
    // One missed answer is a busy moment. Three in a row is a server that
    // has gone.
    view.misses += 1;
    if (view.misses >= 3) lose();
    return;
  }
  const text = JSON.stringify(state);
  if (text === view.stateText) return;
  const first = view.state === null;
  view.state = state;
  view.stateText = text;
  if (state.readers.length > 0) view.reader.notice = "";
  document.getElementById("colophon").textContent = `kobo ${state.computer.kobo}`;
  if (first) resume(state.jobs);
  drawReader();
  drawApps();
}

// Picks up jobs that were already running when the page was opened.
function resume(jobs) {
  const reader = runningJob(jobs, "reader");
  if (reader && /app/.test(reader.title)) view.apps.job = follow(reader.id, drawApps, () => { view.apps.changed = true; loadApps(); });
  else if (reader) {
    view.reader.job = follow(reader.id, drawReader, refresh);
    Object.assign(view.reader.job, { title: reader.title, volume: reader.volume, setup: reader.setup });
  }
  const sending = runningJob(jobs, "send");
  if (sending) {
    view.send.jobFor = /article/.test(sending.title) ? (/waiting/.test(sending.title) ? "queue" : "address") : "file";
    view.send.job = follow(sending.id, drawSend, loadSend);
  }
}

addEventListener("hashchange", () => show(routeFor(location.hash)));
// Closing the window does not stop what is running: the command behind it
// finishes the job before it exits. The browser's own question is still
// asked, because an owner who closes the tab mid-install should do it
// knowing the reader is being written to.
addEventListener("beforeunload", event => {
  if ([view.reader.job, view.apps.job, view.send.job].some(job => job?.running)) event.preventDefault();
});
// Tells the server the window is closing, so it can stop without waiting
// out its idle limit. Sent so that it outlives the page it is sent from.
addEventListener("pagehide", () => {
  fetch("/api/goodbye", { method: "POST", keepalive: true, headers: { "Content-Type": "application/json", "X-Kobo-Token": token }, body: "{}" }).catch(() => {});
});

drawReader();
drawBrowser();
drawSend();
show(view);
loadStore();
refresh();
setInterval(refresh, 2000);
