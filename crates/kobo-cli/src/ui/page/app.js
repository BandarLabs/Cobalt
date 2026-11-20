// The page: it draws what the server says and asks the server to do things.
//
// There is no framework and no build step. Each place is drawn again from
// the page's own small record of what it knows whenever that record changes,
// and everything that decides anything is in logic.mjs, where it is tested.
// Text from the reader, the catalog and the commands is only ever added as
// text, never as markup.

import {
  PLACES,
  afterwards,
  appSections,
  browserStep,
  chosenReader,
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
} from "/logic.mjs";

const token = document.querySelector('meta[name="kobo-token"]').content;

const view = {
  place: placeFor(location.hash),
  lost: false,
  misses: 0,
  state: null,
  stateText: "",
  volume: null,
  reader: { plan: null, job: null, error: "", notice: "" },
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

async function answered(request) {
  let response;
  try {
    response = await request;
  } catch {
    throw new Refused("This window could not reach the kobo ui command behind it.", 0);
  }
  const body = await response.json().catch(() => null);
  if (response.status === 401) {
    lose();
    throw new Refused("", 401);
  }
  if (!response.ok) throw new Refused(body?.error || `The request was refused (${response.status}).`, response.status);
  return body ?? {};
}

const ask = path => answered(fetch(path, { cache: "no-store" }));

const change = (path, body = {}) =>
  answered(
    fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Kobo-Token": token },
      body: JSON.stringify(body),
    })
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
  const job = { id, title: "", text: "", next: 0, running: true, ok: null };
  const look = async () => {
    try {
      const report = await ask(`/api/job?id=${id}&from=${job.next}`);
      Object.assign(job, { title: report.title, text: job.text + report.text, next: report.next, running: report.running, ok: report.ok });
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
function jobView(job, { done, waiting, extra, back, again }) {
  if (!job.running && !job.ok) {
    return [
      h("h2", null, `${job.title} did not finish.`),
      note(sentence(failure(job.text)) || "The command stopped without saying why.", true),
      h("details", { class: "whole" }, h("summary", null, "Everything it said"), said(job.text)),
      h("div", { class: "actions" }, again && primary("Try again", again), button("Back", back)),
    ];
  }
  const { report, next } = afterwards(reflow(job.text));
  return [
    h("h2", null, job.running ? job.title || "Working" : done),
    job.running && h("div", { class: "working", role: "progressbar", "aria-label": job.title }),
    job.running && report.length === 0 && h("p", { class: "quiet" }, waiting),
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
  if (local.job) {
    const back = () => { local.job = null; local.error = ""; drawReader(); refresh(); };
    // A reader still mounted after a finished job has not seen what was
    // written to it, and pulling the cable now can lose the end of it.
    const reader = view.state?.readers.find(found => found.volume === local.job.volume);
    const extra = reader && h("div", { class: "note" },
      h("p", null, "The reader is still connected. Eject it before you unplug it, so everything written is on it. ",
        view.state.computer.ejects && plain("Eject it now", () => eject(reader))));
    return slot.replaceChildren(...jobView(local.job, {
      done: DONE[local.job.title] ?? "Done.",
      waiting: "This takes a minute or two. Leave the reader plugged in until the steps for it appear here.",
      extra,
      back,
      again: local.job.again,
    }).filter(Boolean));
  }
  if (!view.state) return slot.replaceChildren(h("p", { class: "quiet" }, "Looking for a reader."));
  const chosen = chosenReader(view.state.readers, view.volume);
  if (chosen.kind === "none") return slot.replaceChildren(...noReader());
  if (chosen.kind === "several") return slot.replaceChildren(...severalReaders(chosen.readers));
  if (local.plan?.volume === chosen.reader.volume) return slot.replaceChildren(...planView(chosen.reader, local.plan).filter(Boolean));
  local.plan = null;
  slot.replaceChildren(...oneReader(chosen).filter(Boolean));
}

function noReader() {
  return [
    note(view.reader.notice),
    h("h2", null, "No reader is connected."),
    h("p", { class: "reading" }, "Plug your Kobo into this computer and it appears here by itself."),
    h("ol", { class: "steps reading" },
      h("li", null, "Use a cable that carries data, plugged straight into the computer and not through a hub. A cable that only charges looks the same."),
      h("li", null, "The reader asks whether to connect. Tap Connect. It stays hidden from the computer until you do.")),
    h("p", { class: "quiet small" }, "Looking for a reader."),
  ].filter(Boolean);
}

function severalReaders(readers) {
  return [
    h("h2", null, `${readers.length} readers are connected.`),
    h("p", { class: "reading" }, "Choose the one to work on. Nothing is done to a reader you did not choose."),
    h("ul", { class: "entries" }, readers.map(reader =>
      h("li", null,
        h("div", null, h("span", { class: "name" }, reader.model), h("span", { class: "version" }, `firmware ${reader.firmware}`)),
        h("p", { class: "about path" }, pathText(reader.volume)),
        h("div", { class: "does" }, button("Choose", () => { view.volume = reader.volume; view.reader.notice = ""; drawReader(); drawApps(); }))))),
  ];
}

function oneReader({ reader, among }) {
  const computer = view.state.computer;
  const local = view.reader;
  const standing = readerStanding(reader, computer);
  const cobalt = reader.cobalt ? `${reader.cobalt.cobalt_version}, ${reader.cobalt.channel} channel` : "Not installed";
  const plan = undo => openPlan(reader, undo);
  return [
    h("h2", null, reader.model),
    h("dl", { class: "facts" },
      h("dt", null, "Firmware"), h("dd", null, reader.firmware),
      h("dt", null, "Cobalt"), h("dd", null, cobalt),
      h("dt", null, "Connected at"), h("dd", { class: "path" }, pathText(reader.volume))),
    h("p", { class: "lead" }, standing.headline),
    standing.detail && h("p", { class: "reading" }, prose(standing.detail)),
    standing.primary && note(sentence(reader.untested)),
    note(local.error, true),
    h("div", { class: "actions" },
      standing.primary === "install" && primary("Install Cobalt", () => plan(false)),
      standing.primary === "update" && primary(`Update to ${computer.release}`, () => plan(false)),
      standing.repair && button("Install again", () => plan(false)),
      standing.removable && button("Remove Cobalt", () => plan(true)),
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
  const verb = plan.undo ? "Remove Cobalt" : reader.cobalt ? `Update to ${release}` : `Install Cobalt ${release}`;
  const path = plan.undo ? "/api/reader/remove" : "/api/reader/install";
  return [
    h("h2", null, plan.undo ? "What removing Cobalt changes" : `What installing Cobalt ${release} changes`),
    h("p", { class: "quiet" }, `On the ${reader.model} at `, h("span", { class: "path" }, pathText(reader.volume)), ". Nothing is written until you agree below."),
    h("ol", { class: "plan" }, plan.lines.map(line => h("li", null, planStep(line)))),
    note(sentence(plan.untested)),
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
    local.job = follow(job, drawReader, refresh);
    local.job.volume = volume;
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

// The Apps place.

const search = h("input", {
  class: "field",
  type: "search",
  id: "find-app",
  placeholder: "Name or what it is for",
  autocomplete: "off",
  oninput: () => { view.apps.query = search.value; drawAppLists(); },
});
const appsHead = h("div");
const appsTools = h("div", { class: "toolbar" }, h("div", { class: "grow" }, h("label", { class: "over", htmlFor: "find-app" }, "Find an app"), search));
const appsLists = h("div");
slots.apps.append(appsHead, appsTools, appsLists);

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
  appsTools.hidden = !reader || local.installed === null;
  if (!reader) {
    appsLists.replaceChildren();
    const chosen = view.state ? chosenReader(view.state.readers, view.volume) : { kind: "none" };
    const why =
      !view.state ? "Looking for a reader."
      : chosen.kind === "none" ? "No reader is connected. Plug one in and tap Connect on its screen, and its apps are listed here."
      : chosen.kind === "several" ? "More than one reader is connected. Choose one in Reader first."
      : "This reader does not have Cobalt yet. Install it in Reader, and its apps are listed here.";
    return appsHead.replaceChildren(h("p", { class: "reading" }, why), view.state && h("p", null, h("a", { href: "#reader" }, "Go to Reader")) || "");
  }
  const busy = Boolean(local.job?.running);
  appsHead.replaceChildren(...[
    h("p", { class: "quiet" }, `${reader.model}, Cobalt ${reader.cobalt.cobalt_version}, ${reader.cobalt.channel} catalog.`),
    local.job && h("div", { class: "strip" },
      h("p", null, h("strong", null, busy ? local.job.title : local.job.ok ? "Finished." : "Not everything was installed.")),
      busy && h("div", { class: "working", role: "progressbar", "aria-label": local.job.title }),
      said(local.job.text),
      !busy && h("div", { class: "actions" }, button("Dismiss", () => { local.job = null; drawApps(); }))),
    !local.job && local.said && note(local.said),
    local.problem && h("div", { class: "note wrong" }, h("p", null, prose(local.problem)), h("p", null, plain("Try again", loadApps))),
    local.changed && !busy && h("div", { class: "note" },
      h("p", null, "Eject the reader before you unplug it, so the change is fully written. ",
        view.state.computer.ejects && plain("Eject it now", () => eject(reader)))),
  ].filter(Boolean));
  drawAppLists();
}

function drawAppLists() {
  const local = view.apps;
  const reader = appsReader();
  if (!reader) return;
  if (local.installed === null) return appsLists.replaceChildren(h("p", { class: "quiet" }, local.problem ? "" : "Reading the apps on the reader."));
  const sections = appSections(local.installed, local.catalog, local.query);
  const busy = Boolean(local.job?.running) || local.loading;
  const install = ids => installApps(reader, ids);

  const onReader = sections.installed.map(app => {
    const asking = local.removing === app.id;
    return h("li", null,
      h("div", null, h("span", { class: "name" }, app.title), h("span", { class: "version" }, app.installed_version ?? app.version)),
      h("p", { class: "about" }, asking ? `Remove ${app.title}? Its data is kept, so installing it again picks up where it left off.`
        : app.update_available && !app.runs_on_this_reader ? `Version ${app.version} needs Cobalt ${app.minimum_cobalt_version} or newer.` : app.summary),
      h("div", { class: "does" }, asking
        ? [button("Remove", () => removeApp(reader, app), { disabled: busy }), button("Keep", () => { local.removing = null; drawAppLists(); })]
        : [app.update_available && app.runs_on_this_reader && button(`Update to ${app.version}`, () => install([app.id]), { disabled: busy }),
           button("Remove", () => { local.removing = app.id; drawAppLists(); }, { disabled: busy })]));
  });
  const offered = sections.available.map(app =>
    h("li", null,
      h("div", null, h("span", { class: "name" }, app.title), h("span", { class: "version" }, app.version)),
      h("p", { class: "about" }, app.summary),
      h("div", { class: "does" }, app.runs_on_this_reader
        ? button("Install", () => install([app.id]), { disabled: busy })
        : h("span", { class: "quiet small" }, `Needs Cobalt ${app.minimum_cobalt_version}`))));

  const nothing = local.query.trim() !== "" && onReader.length + offered.length === 0;
  appsLists.replaceChildren(...[
    nothing && h("p", { class: "reading" }, `No app matches “${local.query.trim()}”.`),
    !nothing && h("div", { class: "part" },
      h("h3", null, "On this reader"),
      sections.updates.length > 1 && local.query.trim() === "" && h("p", null, button(`Update all ${sections.updates.length}`, () => install(sections.updates), { disabled: busy })),
      onReader.length > 0 ? h("ul", { class: "entries" }, onReader)
        : h("p", { class: "quiet" }, local.query ? "None of them match." : "No apps yet. The ones below can be installed.")),
    !nothing && local.catalog && h("div", { class: "part" },
      h("h3", null, "Available"),
      offered.length > 0 ? h("ul", { class: "entries" }, offered) : h("p", { class: "quiet" }, local.query ? "None of them match." : "Everything published is on this reader.")),
    !local.catalog && !local.problem && h("p", { class: "quiet" }, "Fetching the signed app catalog."),
  ].filter(Boolean));
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
    const published = await change("/api/apps/catalog", { volume: reader.volume });
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
    local.said = `${app.title} is removed. Its data is kept.`;
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
      !browser.set_up && local.said[browser.id] && h("p", { class: "about wide" }, local.said[browser.id]),
      h("div", { class: "does" }, browser.set_up
        ? button("Remove", () => act("remove"), { disabled: local.busy !== "" })
        : button("Set up", () => act("setup"), { disabled: local.busy !== "" })));
  });
  slots.browser.replaceChildren(...[
    h("p", { class: "reading" }, "Send to Kobo adds three things to your browser’s right-click menu: send this page, send this link, and send the selection. Each arrives on the reader as a book, without the menus and advertisements."),
    note(local.problem, true),
    local.list === null && h("p", { class: "quiet" }, "Looking for browsers."),
    local.list?.length === 0 && h("p", { class: "reading" }, "No browser that Send to Kobo works with was found on this computer. It works with Chrome, Edge, Brave, Chromium and Firefox, and with Safari on a Mac."),
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
    local.said[browser.id] = verb === "remove" ? `Send to Kobo is gone from ${browser.title}. Remove the extension on ${browser.title}’s own extensions page too.` : "";
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
    local.info && saved.length === 0 && note("No reader is saved on this computer yet. One is saved when it comes back on Wi-Fi after Cobalt is installed from Reader. Until then a web article waits here, and a file has nowhere to go."),
    saved.length === 1 && h("p", { class: "quiet" }, `These go over Wi-Fi to the reader saved as “${saved[0].name}”.`),
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
    waiting.length === 0 && h("p", { class: "quiet" }, "Nothing is waiting. An article sent while the reader is asleep waits here and goes when the reader is next awake."),
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
    h("p", null, button("Choose a file", () => picker.click(), { disabled: busy }), " or drag one onto this box."),
    h("p", { class: "quiet small" }, "Photos go to Frame, PDF scores to Music Stand, Markdown notes to Vault, OPML lists to Feeds, CBZ comics to Panels, story files to Parser and Anki decks to Flashcards."));
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

function show(place) {
  view.place = place;
  for (const name of [...PLACES, "lost"]) document.getElementById(`place-${name}`).hidden = view.lost ? name !== "lost" : name !== place;
  for (const link of document.querySelectorAll(".places a")) {
    if (!view.lost && link.dataset.place === place) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  if (view.lost) return;
  document.title = `Cobalt – ${place[0].toUpperCase()}${place.slice(1)}`;
  if (place === "apps") { drawApps(); if (appsReader() && view.apps.installed === null) loadApps(); }
  if (place === "browser") loadBrowsers();
  if (place === "send") loadSend();
}

function lose() {
  if (view.lost) return;
  view.lost = true;
  show(view.place);
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
  document.getElementById("colophon").replaceChildren(h("span", null, `kobo ${state.computer.kobo}`), h("span", null, "For this computer only."));
  if (first) resume(state.jobs);
  drawReader();
  drawApps();
}

// Picks up jobs that were already running when the page was opened.
function resume(jobs) {
  const reader = runningJob(jobs, "reader");
  if (reader && /app/.test(reader.title)) view.apps.job = follow(reader.id, drawApps, () => { view.apps.changed = true; loadApps(); });
  else if (reader) view.reader.job = follow(reader.id, drawReader, refresh);
  const sending = runningJob(jobs, "send");
  if (sending) {
    view.send.jobFor = /article/.test(sending.title) ? (/waiting/.test(sending.title) ? "queue" : "address") : "file";
    view.send.job = follow(sending.id, drawSend, loadSend);
  }
}

addEventListener("hashchange", () => show(placeFor(location.hash)));
// Tells the server the window is closing, so it can stop without waiting
// out its idle limit. Sent so that it outlives the page it is sent from.
addEventListener("pagehide", () => {
  fetch("/api/goodbye", { method: "POST", keepalive: true, headers: { "Content-Type": "application/json", "X-Kobo-Token": token }, body: "{}" }).catch(() => {});
});

drawReader();
drawBrowser();
drawSend();
show(view.place);
refresh();
setInterval(refresh, 2000);
