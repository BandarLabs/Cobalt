"use strict";

// The reader is written to directly from this page. Nothing is uploaded, and
// nothing outside the two folders below is touched: the release goes into the
// slot the firmware unpacks at startup, and the menu entry goes into a file
// named for Cobalt so that entries somebody else added are left alone.
const SLOT_FOLDER = ".kobo";
const SLOT_FILE = "KoboRoot.tgz";
const MENU_FOLDER = ".adds";
const MENU_SUBFOLDER = "nm";
const MENU_FILE = "cobalt";

// Byte for byte what `kobo setup` writes, and deliberately so. The undo path
// removes its own comment lines by exact string match, so a line reworded to
// mention this page instead would be left behind: the menu_item would go, the
// comments would stay, and the file would survive as a stub that no longer
// says anything true. Saying "written by 'kobo setup'" is less accurate about
// where it came from and more accurate about what can remove it.
const MENU_ENTRY = `# Cobalt. Written by 'kobo setup'; removed by 'kobo setup --undo'.
#
# Starting Cobalt stops the reader and takes over the screen. Restart
# the device to get the reader back.
menu_item :main :Cobalt :cmd_spawn :quiet:/mnt/onboard/.adds/cobalt-launch.sh
`;

const step = {
  connect: document.querySelector("#step-connect"),
  download: document.querySelector("#step-download"),
  apps: document.querySelector("#step-apps"),
  write: document.querySelector("#step-write"),
  finish: document.querySelector("#step-finish")
};
const pickButton = document.querySelector("#pick");
const fetchButton = document.querySelector("#fetch");
const writeButton = document.querySelector("#write");
const pickNote = document.querySelector("#pick-note");
const fetchNote = document.querySelector("#fetch-note");
const writeNote = document.querySelector("#write-note");
const appsNote = document.querySelector("#apps-note");
const search = document.querySelector("#app-search");
const bar = document.querySelector("#bar");
const barFill = bar.querySelector("i");

let drive = null;
let archive = null;
let manifest = null;
let catalogue = [];
let installed = null;
let identity = null;
let untestedAcknowledged = true;
let installedApps = new Set();

function enable(element, on) {
  element.removeAttribute("aria-disabled");
  if (!on) element.setAttribute("aria-disabled", "true");
}

function done(element) {
  element.dataset.state = "done";
}

// Said once, at the top, rather than left for the reader to discover when a
// button does nothing. The directory picker is the part browsers disagree
// about; everything else here is ordinary.
function refuseUnsupportedBrowser() {
  if (typeof window.showDirectoryPicker === "function") return false;
  const banner = document.querySelector("#unsupported");
  document.querySelector("#unsupported-why").innerHTML =
    "Writing to a plugged-in drive is something Chrome, Edge and Opera can do, " +
    "and Firefox and Safari cannot. <strong>Open this page in Chrome</strong> " +
    "to install straight from the browser. Otherwise the ways below install " +
    "exactly the same thing.";
  banner.hidden = false;
  pickButton.disabled = true;
  // The steps above cannot be followed here, so the ways that can be are
  // opened rather than left folded behind a heading somebody has to think to
  // click.
  document.querySelector("#by-hand").open = true;
  return true;
}

async function chooseDrive() {
  let handle;
  try {
    handle = await window.showDirectoryPicker({ id: "kobo", mode: "readwrite" });
  } catch (error) {
    // Dismissing the picker is a choice, not a fault, and should not be
    // reported as one.
    if (error && error.name === "AbortError") return;
    pickNote.innerHTML = `<span class="bad">The drive could not be opened: ${escapeText(error.message)}</span>`;
    return;
  }

  // A Kobo has a .kobo folder holding a version file. Checking for it means an
  // ordinary USB stick chosen by mistake is refused here rather than written
  // to and left with a KoboRoot.tgz on it.
  try {
    const system = await handle.getDirectoryHandle(SLOT_FOLDER);
    await system.getFileHandle("version");
  } catch {
    pickNote.innerHTML =
      '<span class="bad">That drive has no <code>.kobo/version</code>, so it is not a Kobo. ' +
      "Choose the drive named KOBOeReader.</span>";
    return;
  }

  if ((await handle.queryPermission({ mode: "readwrite" })) !== "granted" &&
      (await handle.requestPermission({ mode: "readwrite" })) !== "granted") {
    pickNote.innerHTML = '<span class="bad">Writing was not allowed, so nothing was installed.</span>';
    return;
  }

  drive = handle;
  // Read before anything is offered, so somebody can see whether this is a
  // first install, an update, or the version they already have, rather than
  // finding out from what the reader does afterwards.
  installed = await readInstalledVersion(handle);
  installedApps = await readInstalledApps(handle);
  identity = await readReaderIdentity(handle);
  const where = `Reader found on <strong>${escapeText(handle.name)}</strong>`;
  const said = installed
    ? `${where}, with Cobalt ${escapeText(installed)} installed.`
    : `${where}. Cobalt is not installed yet.`;

  // The reviewed models come from the profiles the runtime enforces, so this
  // says the same thing the project says. It is a note, not a gate: an untested
  // reader may well work, and somebody who wants to try theirs should be able
  // to. Nothing is said when the model cannot be read, because a warning about
  // a question that was never answered is just noise.
  let caution = "";
  const tested = await readTestedDevices();
  if (identity && identity.model && tested.length > 0) {
    const match = tested.find(device => device.code === identity.model);
    if (match) {
      caution = `<span class="note">${escapeText(match.model)}, firmware ${escapeText(identity.firmware || "unknown")}. This model has been tested.</span>`;
    } else {
      caution =
        `<span class="note">Model ${escapeText(identity.model)}, firmware ${escapeText(identity.firmware || "unknown")}. ` +
        "This model has not been tested with Cobalt. It may work and it may not, " +
        `and the ones that have been tested are: ${escapeText([...new Set(tested.map(d => d.model))].join(", "))}.</span>` +
        '<label class="consent"><input type="checkbox" id="untested-ok"> Install anyway on this untested model</label>';
    }
  }
  pickNote.innerHTML = `<span class="ok">${said}</span>${caution}`;
  const consent = document.querySelector("#untested-ok");
  if (consent) {
    // The reader is chosen at step one and written at step three, so the
    // agreement has to survive the steps in between rather than gate a button
    // that is not on screen yet.
    untestedAcknowledged = false;
    consent.addEventListener("change", () => {
      untestedAcknowledged = consent.checked;
      writeButton.disabled = !consent.checked || archive === null;
    });
  } else {
    untestedAcknowledged = true;
  }

  done(step.connect);
  enable(step.download, true);
  fetchButton.disabled = false;
}

async function fetchRelease() {
  fetchButton.disabled = true;
  fetchNote.textContent = "Checking what is available\u2026";
  try {
    // Checked before it is parsed. A 404 here answers with a page, and parsing
    // that as JSON reports a syntax error to somebody who wanted to install a
    // reading application.
    const described = await fetch("manifest.json", { cache: "no-store" });
    if (!described.ok) {
      throw new Error(`could not check what is available (${described.status})`);
    }
    manifest = await described.json();
    bar.hidden = false;
    const response = await fetch(manifest.archive, { cache: "no-store" });
    if (!response.ok) throw new Error(`Cobalt could not be downloaded (${response.status})`);

    // Read in pieces so the bar moves. A reader on a slow connection is
    // downloading about eighteen megabytes and deserves to see it happening.
    const total = manifest.bytes;
    const reader = response.body.getReader();
    const pieces = [];
    let received = 0;
    for (;;) {
      const { done: finished, value } = await reader.read();
      if (finished) break;
      pieces.push(value);
      received += value.length;
      barFill.style.width = `${Math.min(100, (received / total) * 100)}%`;
      fetchNote.textContent = `${(received / 1048576).toFixed(1)} of ${(total / 1048576).toFixed(1)} MB`;
    }
    const bytes = new Uint8Array(received);
    let at = 0;
    for (const piece of pieces) { bytes.set(piece, at); at += piece.length; }

    // Checked before it is offered for writing, not after. A download that is
    // not what was published is discarded here and never reaches the reader.
    const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
      .map(byte => byte.toString(16).padStart(2, "0")).join("");
    if (digest !== manifest.sha256) {
      throw new Error("the file that arrived is not the one that was published, so it was thrown away");
    }

    archive = bytes;
    const change = !installed
      ? `Cobalt ${escapeText(manifest.version)} verified.`
      : installed === manifest.version
        ? `Cobalt ${escapeText(manifest.version)} verified. This reader already has that version, so this reinstalls it.`
        : `Cobalt ${escapeText(manifest.version)} verified. This reader has ${escapeText(installed)}.`;
    fetchNote.innerHTML = `<span class="ok">${change}</span>`;
    done(step.download);
    enable(step.apps, true);
    enable(step.write, true);
    writeButton.disabled = !untestedAcknowledged;
    loadCatalogue();
  } catch (error) {
    archive = null;
    fetchNote.innerHTML = `<span class="bad">${escapeText(error.message)}</span>`;
    fetchButton.disabled = false;
  }
}

async function writeToReader() {
  writeButton.disabled = true;
  try {
    writeNote.textContent = "Writing the release…";
    const system = await drive.getDirectoryHandle(SLOT_FOLDER, { create: true });

    // The firmware reads exactly one archive here and deletes it afterwards.
    // Anything already in the slot belongs to another mod's install that has
    // not been through a restart yet, and replacing it would cancel it
    // silently.
    let occupied = true;
    try { await system.getFileHandle(SLOT_FILE); } catch { occupied = false; }
    if (occupied) {
      throw new Error(
        "something is already waiting to be installed at .kobo/KoboRoot.tgz. " +
        "Restart the reader to let it finish, then come back."
      );
    }

    const slot = await system.getFileHandle(SLOT_FILE, { create: true });
    const target = await slot.createWritable();
    await target.write(archive);
    await target.close();

    // Read back what was written. There is no way for a page to eject a drive
    // -- no web API offers it -- so the operating system may still be holding
    // these bytes in a cache that only ejecting flushes. This does not prove
    // they reached the card, and nothing a page can do would. It does prove
    // the file is the right length and the right bytes as the system reports
    // it, which is what catches a write that ran out of room half way and
    // would otherwise be found by the reader, at the next start, as an update
    // that failed for no stated reason.
    writeNote.textContent = "Checking what was written…";
    const written = new Uint8Array(await (await slot.getFile()).arrayBuffer());
    if (written.length !== archive.length) {
      throw new Error(
        `only ${written.length.toLocaleString()} of ${archive.length.toLocaleString()} bytes ` +
        "were written, so the reader was left alone. Check there is room on the drive and try again."
      );
    }
    const writtenDigest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", written))]
      .map(byte => byte.toString(16).padStart(2, "0")).join("");
    if (writtenDigest !== manifest.sha256) {
      throw new Error("what ended up on the reader is not what was downloaded, so it was taken off again.");
    }

    writeNote.textContent = "Adding the menu entry…";
    const adds = await drive.getDirectoryHandle(MENU_FOLDER, { create: true });
    const nm = await adds.getDirectoryHandle(MENU_SUBFOLDER, { create: true });
    const entry = await nm.getFileHandle(MENU_FILE, { create: true });
    const entryTarget = await entry.createWritable();
    await entryTarget.write(new TextEncoder().encode(MENU_ENTRY));
    await entryTarget.close();

    const chosen = chosenApps();
    if (chosen.length > 0) {
      const cobalt = await adds.getDirectoryHandle("cobalt", { create: true });
      const appsFolder = await cobalt.getDirectoryHandle("apps", { create: true });
      for (const [index, app] of chosen.entries()) {
        writeNote.textContent = `Writing ${app.name} (${index + 1} of ${chosen.length})…`;
        await writeApp(appsFolder, app);
      }
    }

    const tail = chosen.length > 0 ? ` with ${chosen.length} application${chosen.length === 1 ? "" : "s"}` : "";
    writeNote.innerHTML =
      `<span class="ok">Written${escapeText(tail)}. Eject the drive, unplug the cable and restart the reader.</span>`;
    done(step.write);
    enable(step.finish, true);
  } catch (error) {
    // A partial archive in the slot is worse than none: the firmware would try
    // it at the next start and fail on its own, with nobody to say why.
    try {
      const system = await drive.getDirectoryHandle(SLOT_FOLDER);
      const slot = await system.getFileHandle(SLOT_FILE);
      const partial = await slot.getFile();
      if (partial.size !== (archive ? archive.length : -1)) {
        await system.removeEntry(SLOT_FILE);
      }
    } catch { /* nothing was written, or it is complete and stays */ }
    writeNote.innerHTML = `<span class="bad">${escapeText(error.message)}</span>`;
    writeButton.disabled = false;
  }
}

// The list is whatever the Store was offering when the site was built, not a
// list kept by hand here, so an application added to the catalogue appears
// without anyone editing this page.
async function loadCatalogue() {
  try {
    catalogue = await (await fetch("apps.json", { cache: "no-store" })).json();
  } catch {
    appsNote.textContent =
      "The application catalogue could not be read. Cobalt can still be installed, " +
      "and applications can be added later from the Store on the reader.";
    return;
  }
  const list = document.querySelector("#applist");
  for (const app of catalogue) {
    const row = document.createElement("label");
    row.className = "app";
    row.dataset.search = `${app.name} ${app.summary} ${app.id}`.toLowerCase();
    const already = installedApps.has(app.id);
    // The name links to the application's own page, which is where somebody
    // deciding whether they want it can read more than a one-line summary. It
    // opens in its own tab: this page holds a chosen reader and a verified
    // download, and navigating away would lose both.
    row.innerHTML =
      `<input type="checkbox" value="${escapeText(app.id)}">` +
      `<div><a class="app-name" href="../apps/${escapeText(app.id)}/" target="_blank" rel="noopener">${escapeText(app.name)}</a>` +
      `<span>${escapeText(app.summary)}</span></div>` +
      `<em>${already ? "on the reader · " : ""}${(app.bytes / 1048576).toFixed(1)} MB</em>`;
    row.querySelector("input").addEventListener("change", countChosen);
    row.querySelector(".app-name").addEventListener("click", event => event.stopPropagation());
    list.append(row);
  }
  search.disabled = false;
  search.addEventListener("input", () => {
    const needle = search.value.trim().toLowerCase();
    for (const row of list.children) {
      row.hidden = needle !== "" && !row.dataset.search.includes(needle);
    }
  });
  countChosen();
}

function chosenApps() {
  return [...document.querySelectorAll("#applist input:checked")]
    .map(box => catalogue.find(app => app.id === box.value));
}

function countChosen() {
  const chosen = chosenApps();
  if (chosen.length === 0) {
    appsNote.textContent = `${catalogue.length} available. None chosen; Cobalt will be installed on its own.`;
    return;
  }
  const megabytes = chosen.reduce((total, app) => total + app.bytes, 0) / 1048576;
  appsNote.textContent =
    `${chosen.length} chosen, ${megabytes.toFixed(1)} MB to download alongside Cobalt.`;
}

// magic(8) | version u16 | manifest length u32 | signature(64) | manifest | binary.
// Split rather than unpacked: there is no archive here, and the three files the
// reader expects are these three pieces written out.
function splitBundle(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const magic = new TextDecoder().decode(bytes.subarray(0, 8));
  if (magic !== "COBALTAP") throw new Error("that is not an application package");
  if (view.getUint16(8) !== 1) throw new Error("the package is a format this page does not know");
  const manifestLength = view.getUint32(10);
  const signature = bytes.subarray(14, 78);
  const manifest = bytes.subarray(78, 78 + manifestLength);
  const binary = bytes.subarray(78 + manifestLength);
  if (binary.length === 0) throw new Error("the package carries no application");
  return { signature, manifest, binary };
}

async function writeApp(appsFolder, app) {
  const response = await fetch(`apps/${app.id}.cobalt-app`, { cache: "no-store" });
  if (!response.ok) throw new Error(`${app.name} could not be downloaded (${response.status})`);
  const bytes = new Uint8Array(await response.arrayBuffer());

  // Same rule as the platform archive: checked before anything is written, and
  // discarded rather than installed if it does not match.
  const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
    .map(byte => byte.toString(16).padStart(2, "0")).join("");
  if (digest !== app.sha256) throw new Error(`${app.name} did not match its published digest`);

  const { signature, manifest, binary } = splitBundle(bytes);
  const folder = await appsFolder.getDirectoryHandle(app.id, { create: true });
  const bin = await folder.getDirectoryHandle("bin", { create: true });

  // The runtime stages an application beside the one it replaces and swaps the
  // two, so an interruption leaves the old one whole. A page has no rename to
  // do that with, so the order is chosen instead to make every point at which
  // this can stop a refusal rather than a half-installed application:
  //
  //   the executable first -- on its own it no longer matches the manifest
  //   still beside it, and the runtime checks that
  //   then the manifest -- now signed by a signature that is still the old one
  //   the signature last -- which is the only point where the three agree
  //
  // Stop anywhere before the end and the reader declines to start it, which is
  // recoverable by installing again. Writing the signature first would leave
  // the opposite: an application that verifies and is not the one that was
  // signed.
  await writeFile(bin, `kobo-${app.id}`, binary);
  // The manifest bytes are written exactly as they were signed; re-encoding
  // them would leave a signature that no longer covers what is on disk.
  await writeFile(folder, "manifest.json", manifest);
  await writeFile(folder, "manifest.json.sig",
    new TextEncoder().encode([...signature].map(b => b.toString(16).padStart(2, "0")).join("") + "\n"));
}

async function writeFile(folder, name, bytes) {
  const handle = await folder.getFileHandle(name, { create: true });
  const target = await handle.createWritable();
  await target.write(bytes);
  await target.close();
}

function escapeText(value) {
  const holder = document.createElement("span");
  holder.textContent = String(value);
  return holder.innerHTML;
}

describeManualRoute();

if (refuseUnsupportedBrowser()) {
  // The only route left, so it is opened rather than left to be discovered.
  document.querySelector("#by-hand").open = true;
} else {
  pickButton.addEventListener("click", chooseDrive);
  fetchButton.addEventListener("click", fetchRelease);
  writeButton.addEventListener("click", writeToReader);
}
