"use strict";

// Removing is the install in reverse: the payload a release wrote, and the menu
// entry named for Cobalt. Everything else on the reader was put there by its
// owner, and none of it is a release's to take away.
const SLOT_FOLDER = ".kobo";
const ADDS_FOLDER = ".adds";
const MENU_SUBFOLDER = "nm";
const MENU_FILE = "cobalt";
const PAYLOAD_ENTRIES = ["bin", "licenses", "start.sh", "README.txt", "LICENSE", "THIRD-PARTY.md", "VERSION"];
const OWNER_ENTRIES = ["secrets", "trust", "state", "data", "apps", "store"];
const STALE_FOLDERS = ["cobalt.prev", "cobalt.next", "cobalt.previous"];

const pickButton = document.querySelector("#remove-pick");
const goButton = document.querySelector("#remove-go");
const note = document.querySelector("#remove-note");
const keeping = document.querySelector("#remove-keeping");
const result = document.querySelector("#remove-result");
const stepGo = document.querySelector("#step-go");
const stepDone = document.querySelector("#step-done");
const stepPick = document.querySelector("#step-pick");

let drive = null;

function enable(element, on) {
  element.removeAttribute("aria-disabled");
  if (!on) element.setAttribute("aria-disabled", "true");
}

function escapeText(value) {
  const holder = document.createElement("span");
  holder.textContent = String(value);
  return holder.innerHTML;
}

function refuseUnsupportedBrowser() {
  if (typeof window.showDirectoryPicker === "function") return false;
  document.querySelector("#unsupported-why").textContent =
    "Reading a plugged-in drive is something Chrome, Edge and Opera can do and " +
    "Firefox and Safari cannot. Open this page in one of those, or use one of " +
    "the other ways below. They remove exactly the same things.";
  document.querySelector("#unsupported").hidden = false;
  pickButton.disabled = true;
  document.querySelector("#by-hand").open = true;
  return true;
}

async function installedVersion(adds) {
  try {
    const cobalt = await adds.getDirectoryHandle("cobalt");
    return (await (await (await cobalt.getFileHandle("VERSION")).getFile()).text()).trim() || null;
  } catch {
    return null;
  }
}

async function chooseDrive() {
  let handle;
  try {
    handle = await window.showDirectoryPicker({ id: "kobo", mode: "readwrite" });
  } catch (error) {
    if (error && error.name === "AbortError") return;
    note.innerHTML = `<span class="bad">${escapeText(error.message)}</span>`;
    return;
  }

  try {
    const system = await handle.getDirectoryHandle(SLOT_FOLDER);
    await system.getFileHandle("version");
  } catch {
    note.innerHTML = '<span class="bad">That drive is not a Kobo. Choose the one called KOBOeReader.</span>';
    return;
  }

  let adds;
  let install;
  try {
    adds = await handle.getDirectoryHandle(ADDS_FOLDER);
    install = await adds.getDirectoryHandle("cobalt");
  } catch {
    note.innerHTML = '<span class="ok">Cobalt is not on this reader, so there is nothing to remove.</span>';
    return;
  }

  // The folder outlives a removal, because the owner's data is in it. So
  // finding the folder does not mean finding Cobalt, and offering to remove it
  // again would be offering to take away something that already went.
  const version = await installedVersion(adds);
  const kept = [];
  for (const entry of OWNER_ENTRIES) {
    try { await install.getDirectoryHandle(entry); kept.push(entry); } catch { /* absent */ }
  }
  if (!version) {
    note.innerHTML = kept.length > 0
      ? '<span class="ok">Cobalt is already off this reader. What is still here is yours: ' +
        `${escapeText(kept.join(", "))}. Installing again picks it up where it is.</span>`
      : '<span class="ok">Cobalt is already off this reader.</span>';
    return;
  }

  drive = handle;
  note.innerHTML = `<span class="ok">Cobalt ${escapeText(version)} is on <strong>${escapeText(handle.name)}</strong>.</span>`;
  // Named before anything is removed, so what survives is something the owner
  // read beforehand rather than something they were told afterwards.
  keeping.textContent = kept.length > 0
    ? `Keeping: ${kept.join(", ")}.`
    : "There is no data on this reader to keep.";
  stepPick.dataset.state = "done";
  enable(stepGo, true);
  goButton.disabled = false;
}

async function removeCobalt() {
  goButton.disabled = true;
  try {
    const adds = await drive.getDirectoryHandle(ADDS_FOLDER);
    const install = await adds.getDirectoryHandle("cobalt");
    // Entry by entry, never the folder itself: removing .adds/cobalt would take
    // the owner's data with it, which is the one thing this must not do.
    for (const entry of PAYLOAD_ENTRIES) {
      try { await install.removeEntry(entry, { recursive: true }); } catch { /* absent */ }
    }
    try { await adds.removeEntry("cobalt-launch.sh"); } catch { /* absent */ }
    for (const folder of STALE_FOLDERS) {
      try { await adds.removeEntry(folder, { recursive: true }); } catch { /* absent */ }
    }
    // Ours by name, which is the reason it has one of its own. A menu somebody
    // wrote by hand is .adds/nm/menu and is not read, let alone written, and
    // NickelMenu itself stays because other mods are using it.
    try {
      const nm = await adds.getDirectoryHandle(MENU_SUBFOLDER);
      await nm.removeEntry(MENU_FILE);
    } catch { /* absent */ }

    result.innerHTML = '<span class="ok">Cobalt is off the reader and your data is still there.</span>';
    stepGo.dataset.state = "done";
    enable(stepDone, true);
  } catch (error) {
    result.innerHTML = `<span class="bad">${escapeText(error.message)}</span>`;
    goButton.disabled = false;
  }
}

if (!refuseUnsupportedBrowser()) {
  pickButton.addEventListener("click", chooseDrive);
  goButton.addEventListener("click", removeCobalt);
}
