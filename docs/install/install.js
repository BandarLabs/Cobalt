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

// Written by 'kobo setup' too, and removed by 'kobo setup --undo'. Keeping the
// same wording means the command-line tool recognises and can remove what this
// page writes.
const MENU_ENTRY = `# Cobalt. Written by the browser installer; removed by 'kobo setup --undo'.
#
# Starting Cobalt stops the reader and takes over the screen. Restart
# the device to get the reader back.
menu_item :main :Cobalt :cmd_spawn :quiet:/mnt/onboard/.adds/cobalt-launch.sh
`;

const step = {
  connect: document.querySelector("#step-connect"),
  download: document.querySelector("#step-download"),
  write: document.querySelector("#step-write"),
  finish: document.querySelector("#step-finish")
};
const pickButton = document.querySelector("#pick");
const fetchButton = document.querySelector("#fetch");
const writeButton = document.querySelector("#write");
const pickNote = document.querySelector("#pick-note");
const fetchNote = document.querySelector("#fetch-note");
const writeNote = document.querySelector("#write-note");
const bar = document.querySelector("#bar");
const barFill = bar.querySelector("i");

let drive = null;
let archive = null;
let manifest = null;

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
  document.querySelector("#unsupported-why").textContent =
    "Writing to a connected drive needs the File System Access API, which " +
    "Chrome, Edge and Opera have and Firefox and Safari do not. Open this " +
    "page in one of those, or install from a terminal with the one-line " +
    "command on the home page.";
  banner.hidden = false;
  pickButton.disabled = true;
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
    pickNote.innerHTML = '<span class="bad">Writing was not permitted, so nothing can be installed.</span>';
    return;
  }

  drive = handle;
  pickNote.innerHTML = `<span class="ok">Reader found on <strong>${escapeText(handle.name)}</strong>.</span>`;
  done(step.connect);
  enable(step.download, true);
  fetchButton.disabled = false;
}

async function fetchRelease() {
  fetchButton.disabled = true;
  fetchNote.textContent = "Reading the release description…";
  try {
    manifest = await (await fetch("manifest.json", { cache: "no-store" })).json();
    bar.hidden = false;
    const response = await fetch(manifest.archive, { cache: "no-store" });
    if (!response.ok) throw new Error(`the release could not be downloaded (${response.status})`);

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
      throw new Error("the download did not match its published digest, so it was discarded");
    }

    archive = bytes;
    fetchNote.innerHTML = `<span class="ok">Cobalt ${escapeText(manifest.version)} verified.</span>`;
    showFacts(digest);
    done(step.download);
    enable(step.write, true);
    writeButton.disabled = false;
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

    writeNote.textContent = "Adding the menu entry…";
    const adds = await drive.getDirectoryHandle(MENU_FOLDER, { create: true });
    const nm = await adds.getDirectoryHandle(MENU_SUBFOLDER, { create: true });
    const entry = await nm.getFileHandle(MENU_FILE, { create: true });
    const entryTarget = await entry.createWritable();
    await entryTarget.write(new TextEncoder().encode(MENU_ENTRY));
    await entryTarget.close();

    writeNote.innerHTML =
      '<span class="ok">Written. Eject the drive, unplug the cable and restart the reader.</span>';
    done(step.write);
    enable(step.finish, true);
  } catch (error) {
    writeNote.innerHTML = `<span class="bad">${escapeText(error.message)}</span>`;
    writeButton.disabled = false;
  }
}

function showFacts(digest) {
  document.querySelector("#fact-version").textContent = manifest.version;
  document.querySelector("#fact-digest").textContent = digest;
  document.querySelector("#fact-bytes").textContent = manifest.bytes.toLocaleString();
  document.querySelector("#fact-nm").textContent = manifest.nickelmenu;
  document.querySelector("#facts").hidden = false;
}

function escapeText(value) {
  const holder = document.createElement("span");
  holder.textContent = String(value);
  return holder.innerHTML;
}

if (!refuseUnsupportedBrowser()) {
  pickButton.addEventListener("click", chooseDrive);
  fetchButton.addEventListener("click", fetchRelease);
  writeButton.addEventListener("click", writeToReader);
}
