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

// Where the runtime keeps what is installed and what the owner decided, inside
// .adds. `state` and `apps` are two of the folders the runtime carries across
// every update it applies to itself, so a file written into them before the
// first start is still there after it.
const INSTALL_FOLDER = "cobalt";
const APPS_FOLDER = "apps";
const STATE_FOLDER = "state";
// The owner's standing choices about updates, one `name=value` line each, in
// the form the runtime's own Settings application writes. The runtime reads a
// missing file as both switches on and the Stable channel.
const PREFERENCES_FILE = "auto-update";
// The Store records a removal in a file of this name beside where the
// application was, and the runtime takes that record as the removal's durable
// commit: a copy found next to it is finished off, never restored. Written
// here for the same reason, so a removal this page starts is one the reader
// completes rather than undoes.
const UNINSTALLED_SUFFIX = "uninstalled";
// Identities that belong to Cobalt itself. The Store refuses to remove them
// and so does this page: a reader without its launcher has nothing to open
// the Store from.
const RESERVED_APPS = ["books", "cobalt", "kobod", "launcher", "settings", "store", "terminal"];
// Where the site describes each channel's release. The archive each names is
// relative to this page, like the manifest itself.
const MANIFEST = { stable: "manifest.json", beta: "beta/manifest.json" };

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
const installedLede = document.querySelector("#installed-lede");
const installedList = document.querySelector("#installed");
const channelNote = document.querySelector("#channel-note");
const channelChoice = {
  stable: document.querySelector("#channel-stable"),
  beta: document.querySelector("#channel-beta")
};
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
// The reader's standing update choices, read when it is chosen.
let preferences = null;
// Which channel the verified archive was downloaded for. A Beta request the
// site answers with Stable still counts as Beta here: the reader is told to
// follow Beta either way (see fetchRelease).
let archiveChannel = null;

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
function missingCapability() {
  if (typeof window.showDirectoryPicker !== "function") return "picker";
  // The write side is a separate interface and is what the install needs. A
  // browser offering the picker without it would get as far as a chosen reader
  // and a verified download, and fail at the only step that matters.
  const file = window.FileSystemFileHandle;
  if (!file || typeof file.prototype.createWritable !== "function") return "writing";
  const folder = window.FileSystemDirectoryHandle;
  if (!folder || typeof folder.prototype.getDirectoryHandle !== "function") return "folders";
  return null;
}

function refuseUnsupportedBrowser() {
  const missing = missingCapability();
  if (!missing) return false;
  const banner = document.querySelector("#unsupported");
  // Which of the three is missing matters for working out why, and not to
  // somebody who wants to install a reading application: the answer is the same
  // either way. It goes to the console for anyone looking, and the line on the
  // page says what to do instead.
  console.info(`Cobalt: this browser ${
    missing === "writing" ? "opens a drive but cannot write to one"
    : missing === "folders" ? "cannot read folders on a drive"
    : "cannot open a drive"}.`);
  document.querySelector("#unsupported-why").innerHTML =
    "Try Chrome, Edge or Opera on a computer. Or use one of the ways below: " +
    "one command in a terminal, or copy a single file with no terminal at all.";
  banner.hidden = false;
  pickButton.disabled = true;
  // The steps are the whole page and none of them can be followed here. Left
  // as they are they read as the thing to do, and the route that does work sits
  // underneath them looking like an afterthought.
  const steps = document.querySelector("#steps");
  if (steps) steps.setAttribute("aria-disabled", "true");
  // The steps above cannot be followed here, so the ways that can be are
  // opened rather than left folded behind a heading somebody has to think to
  // click.
  document.querySelector("#by-hand").open = true;
  return true;
}

// These reads are optional metadata: a fresh reader has no Cobalt folders,
// and an unavailable device list must not prevent choosing a valid Kobo.
async function readInstalledVersion(handle) {
  try {
    const adds = await handle.getDirectoryHandle(MENU_FOLDER);
    const cobalt = await adds.getDirectoryHandle("cobalt");
    const file = await (await cobalt.getFileHandle("VERSION")).getFile();
    return (await file.text()).trim() || null;
  } catch {
    return null;
  }
}

async function readInstalledApps(handle) {
  const present = new Set();
  try {
    const adds = await handle.getDirectoryHandle(MENU_FOLDER);
    const cobalt = await adds.getDirectoryHandle("cobalt");
    const apps = await cobalt.getDirectoryHandle("apps");
    for await (const [name, entry] of apps.entries()) {
      // Only names the runtime would accept as an identity. Staged .next and
      // .prev folders and the record of a removal all carry a dot, and any
      // other folder is not an application and is not offered as one.
      if (entry.kind === "directory" && isAppId(name)) present.add(name);
    }
  } catch { /* nothing installed */ }
  return present;
}

// Read the way the runtime reads it: one switch per line, anything it does
// not recognise ignored, anything missing on. A file that is not there is the
// default, which is the Stable channel.
function parsePreferences(text) {
  const chosen = { cobalt: true, apps: true, channel: "stable" };
  for (const line of text.split("\n")) {
    switch (line.trim()) {
      case "cobalt=off": chosen.cobalt = false; break;
      case "cobalt=on": chosen.cobalt = true; break;
      case "apps=off": chosen.apps = false; break;
      case "apps=on": chosen.apps = true; break;
      case "channel=stable": chosen.channel = "stable"; break;
      case "channel=beta": chosen.channel = "beta"; break;
      default: break;
    }
  }
  return chosen;
}

// Byte for byte what the runtime writes, so a file this page leaves and a
// file Settings on the reader leaves cannot be told apart.
function renderPreferences(chosen) {
  return `cobalt=${chosen.cobalt ? "on" : "off"}\napps=${chosen.apps ? "on" : "off"}\nchannel=${chosen.channel}\n`;
}

async function readPreferences(handle) {
  try {
    const adds = await handle.getDirectoryHandle(MENU_FOLDER);
    const cobalt = await adds.getDirectoryHandle(INSTALL_FOLDER);
    const state = await cobalt.getDirectoryHandle(STATE_FOLDER);
    const file = await (await state.getFileHandle(PREFERENCES_FILE)).getFile();
    return { present: true, ...parsePreferences(await file.text()) };
  } catch {
    return { present: false, ...parsePreferences("") };
  }
}

// The runtime's own rule for an identity: lowercase letters, digits and
// single hyphens, starting with a letter and not ending in a hyphen.
function isAppId(name) {
  return /^[a-z][a-z0-9-]*$/.test(name) && !name.endsWith("-") && !name.includes("--") && name.length <= 64;
}

async function readReaderIdentity(handle) {
  try {
    const system = await handle.getDirectoryHandle(SLOT_FOLDER);
    const text = await (await (await system.getFileHandle("version")).getFile()).text();
    // The serial's first four characters identify the model; field three is
    // the firmware version. Do not send the reader's serial to the server.
    const fields = text.trim().split(",");
    const serial = (fields[0] || "").trim();
    return { model: serial.slice(0, 4), firmware: (fields[2] || "").trim() };
  } catch {
    return null;
  }
}

// NickelMenu is the only way into Cobalt from the reader's own menus, and it
// can only hook the 4.x firmware. On any other generation an installation
// would be written and could never be started, so this is a refusal rather
// than a caution an owner may wave through. The same rule, and the same
// refusal of a version that cannot be read, as kobo_profile's
// launchable_generation and `kobo setup`.
const SUPPORTED_FIRMWARE_GENERATION = 4;

// `major.minor`, or null unless the version also names a build. Written to
// match kobo_profile::firmware_branch exactly: a version file cut short at
// "4.38" or "4.38." is not a branch, and treating it as one would match it to
// a tested profile on the strength of the part that survived.
function firmwareBranch(version) {
  const text = version || "";
  const first = text.indexOf(".");
  if (first < 0) return null;
  const second = text.indexOf(".", first + 1);
  if (second < 0) return null;
  const major = text.slice(0, first);
  const minor = text.slice(first + 1, second);
  const build = text.slice(second + 1);
  if (!major || !minor || !build) return null;
  return `${major}.${minor}`;
}

function launchableGeneration(version) {
  const branch = firmwareBranch(version);
  if (branch === null) return false;
  const major = branch.split(".")[0];
  return /^\+?\d+$/.test(major) && Number(major) === SUPPORTED_FIRMWARE_GENERATION;
}

// Forgets the reader chosen so far and everything that would write to it.
// Every refusal ends here: a drive chosen earlier in this visit must not stay
// armed behind a message about a different one.
function disarm() {
  drive = null;
  untestedAcknowledged = false;
  // The list of applications belongs to the reader that was just forgotten;
  // a Remove left on screen would act on nothing.
  installedApps = new Set();
  renderInstalled();
  delete step.connect.dataset.state;
  enable(step.download, false);
  fetchButton.disabled = true;
  writeButton.disabled = true;
}

async function readTestedDevices() {
  try {
    const response = await fetch("devices.json", { cache: "no-store" });
    if (!response.ok) return [];
    return await response.json();
  } catch {
    return [];
  }
}

async function chooseDrive() {
  let handle;
  try {
    handle = await window.showDirectoryPicker({ id: "kobo", mode: "readwrite" });
  } catch (error) {
    // Dismissing the picker is a choice, not a fault, and should not be
    // reported as one.
    if (error && error.name === "AbortError") return;
    // Whatever went wrong here, this browser has just failed at the first step,
    // so the routes that do not depend on it are opened rather than left folded
    // away. A button that does nothing and says nothing is the worst of the
    // outcomes available.
    pickNote.innerHTML =
      `<span class="bad">The drive could not be opened: ${escapeText(error.message || error.name || "no reason given")}. ` +
      "The other ways to install, below, do not need this.</span>";
    document.querySelector("#by-hand").open = true;
    return;
  }

  // A different reader is being chosen. Nothing may be written until it has
  // been read and, where it asks for one, agreed to, so Write goes dark now
  // rather than when the checks below finish.
  writeButton.disabled = true;
  untestedAcknowledged = false;

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
    disarm();
    return;
  }

  if (typeof handle.queryPermission === "function") {
    const already = await handle.queryPermission({ mode: "readwrite" });
    const granted = already === "granted" ||
      (typeof handle.requestPermission === "function" &&
        (await handle.requestPermission({ mode: "readwrite" })) === "granted");
    if (!granted) {
      pickNote.innerHTML = '<span class="bad">Writing was not allowed, so nothing was installed.</span>';
      disarm();
      return;
    }
  }

  // Checked before anything else is read or offered, and before the drive is
  // kept: nothing after this point should be reachable for a reader that
  // could never start what would be written to it.
  const reading = await readReaderIdentity(handle);
  if (!reading || !launchableGeneration(reading.firmware)) {
    const found = reading && reading.firmware ? `firmware ${escapeText(reading.firmware)}` : "a firmware version that could not be read";
    pickNote.innerHTML =
      `<span class="bad">This Kobo has ${found}. Cobalt can only be started on ${SUPPORTED_FIRMWARE_GENERATION}.x firmware: ` +
      "NickelMenu, the way into Cobalt from the Kobo's menu, does not work on newer firmware, so an installation " +
      'here could never be opened. Nothing was written. <a href="https://github.com/BandarLabs/Cobalt/blob/main/docs/DEVICES.md">Supported devices</a></span>';
    disarm();
    return;
  }

  drive = handle;
  // Read before anything is offered, so somebody can see whether this is a
  // first install, an update, or the version they already have, rather than
  // finding out from what the reader does afterwards.
  installed = await readInstalledVersion(handle);
  installedApps = await readInstalledApps(handle);
  preferences = await readPreferences(handle);
  identity = reading;
  // The reader's own choice is shown as chosen. Somebody reinstalling a reader
  // that follows Beta should find Beta selected, not be moved back to Stable
  // by a default they never looked at.
  selectChannel(preferences.channel);
  renderInstalled();
  const where = `Reader found on <strong>${escapeText(handle.name)}</strong>`;
  const said = installed
    ? `${where}, with Cobalt ${escapeText(installed)} installed${preferences.channel === "beta" ? " on the Beta channel" : ""}.`
    : `${where}. Cobalt is not installed yet.`;

  // The reviewed models come from the profiles the runtime enforces, so this
  // says the same thing the project says. It is a note, not a gate: an untested
  // reader may well work, and somebody who wants to try theirs should be able
  // to. Nothing is said when the model cannot be read, because a warning about
  // a question that was never answered is just noise.
  let caution = "";
  const tested = await readTestedDevices();
  if (identity && identity.model && tested.length > 0) {
    // A profile names a model and the firmware branches it was measured on,
    // and a model can have several. Only a ready profile on this reader's
    // branch counts as tested; the same model on another branch, or a profile
    // still awaiting review, is the same kind of risk as an unknown model.
    const branch = firmwareBranch(identity.firmware);
    const sameModel = tested.filter(device => device.code === identity.model);
    const match = sameModel.find(device => device.ready !== false &&
      (device.firmware || []).some(version => firmwareBranch(version) === branch));
    const consentBox = '<label class="consent"><input type="checkbox" id="untested-ok"> Install anyway on this untested Kobo</label>';
    if (match) {
      caution = `<span class="note">${escapeText(match.model)}, firmware ${escapeText(identity.firmware)}. This model and firmware have been tested.</span>`;
    } else if (sameModel.length > 0) {
      const known = [...new Set(sameModel.filter(device => device.ready !== false)
        .flatMap(device => device.firmware || []).map(firmwareBranch).filter(Boolean))];
      caution =
        `<span class="note">${escapeText(sameModel[0].model)}, firmware ${escapeText(identity.firmware)}. ` +
        (known.length > 0
          ? `This model has been tested on firmware ${escapeText(known.map(b => b + ".x").join(", "))}, not on this one. It may work.`
          : "This model has not been fully tested yet. It may work.") +
        "</span>" + consentBox;
    } else {
      caution =
        `<span class="note">Model ${escapeText(identity.model)}, firmware ${escapeText(identity.firmware || "unknown")}. ` +
        "This model has not been tested with Cobalt. It may work and it may not, " +
        `and the ones that have been tested are: ${escapeText([...new Set(tested.filter(d => d.ready !== false).map(d => d.model))].join(", "))}.</span>` +
        consentBox;
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
  // A release downloaded for an earlier reader is still good for this one;
  // whether it may be written is this reader's consent, decided above.
  writeButton.disabled = !untestedAcknowledged || archive === null;
}

function chosenChannel() {
  return channelChoice.beta && channelChoice.beta.checked ? "beta" : "stable";
}

function selectChannel(channel) {
  if (!channelChoice.stable || !channelChoice.beta) return;
  channelChoice.stable.checked = channel === "stable";
  channelChoice.beta.checked = channel === "beta";
  describeChannel();
}

// Said beside the choice, so what Write will do to the reader's standing
// preference is read here rather than discovered later in Settings.
function describeChannel() {
  if (!channelNote) return;
  const channel = chosenChannel();
  const current = preferences ? preferences.channel : "stable";
  let said = channel === "beta"
    ? "Beta gets new versions sooner. The reader keeps updating from Beta over Wi-Fi afterwards."
    : "Stable gets tested versions.";
  if (installed && current !== channel) {
    said += channel === "beta"
      ? " This reader follows Stable now and will follow Beta."
      : " This reader follows Beta now and will follow Stable.";
  }
  channelNote.textContent = said;
  // A release verified for one channel is not the other's. Changing the
  // choice after a download puts the download back, rather than writing the
  // Stable archive to a reader that is about to be told to follow Beta.
  if (archive !== null && archiveChannel !== channel) {
    archive = null;
    archiveChannel = null;
    delete step.download.dataset.state;
    enable(step.write, false);
    writeButton.disabled = true;
    fetchButton.disabled = false;
    fetchNote.textContent = `Download Cobalt again for the ${channel === "beta" ? "Beta" : "Stable"} channel.`;
  }
}

// What is on the reader now, by name where the catalogue knows one. Drawn
// again once the catalogue arrives, because the folder names are identities
// and the names are what somebody recognises.
function renderInstalled() {
  if (!installedList) return;
  const ids = [...installedApps].sort();
  installedList.replaceChildren();
  installedList.hidden = ids.length === 0;
  if (installedLede) installedLede.hidden = ids.length === 0;
  for (const id of ids) {
    const row = document.createElement("li");
    const name = document.createElement("span");
    const known = catalogue.find(app => app.id === id);
    name.textContent = known ? known.name : id;
    row.append(name);
    if (!RESERVED_APPS.includes(id)) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "btn small";
      button.textContent = "Remove";
      // Two clicks, the second naming what goes, rather than a dialog. A
      // removal keeps the application's data and the Store can put the
      // application back, so a second look is proportionate and a modal is
      // not.
      button.addEventListener("click", async () => {
        if (button.dataset.armed !== "true") {
          button.dataset.armed = "true";
          button.textContent = `Remove ${name.textContent}`;
          return;
        }
        await removeApp(id, row, name.textContent, button);
      });
      row.append(button);
    }
    installedList.append(row);
  }
}

async function removeApp(id, row, name, button) {
  button.disabled = true;
  try {
    const adds = await drive.getDirectoryHandle(MENU_FOLDER);
    const cobalt = await adds.getDirectoryHandle(INSTALL_FOLDER);
    const apps = await cobalt.getDirectoryHandle(APPS_FOLDER);
    // The record first, then the folders, which is the order that fails
    // safely. The runtime finishes a removal it finds recorded and half done.
    // One it finds unrecorded it undoes: an application whose folder is gone
    // while its `.prev` copy from the last update remains looks like an
    // interrupted update, and the copy is put back. Stopping after the record
    // alone leaves a whole application beside a record the runtime clears at
    // its next look, which is nothing having happened.
    await writeFile(apps, `${id}.${UNINSTALLED_SUFFIX}`, new TextEncoder().encode("committed\n"));
    for (const entry of [id, `${id}.prev`, `${id}.next`, `${id}.removed`]) {
      try { await apps.removeEntry(entry, { recursive: true }); } catch { /* absent */ }
    }
    installedApps.delete(id);
    row.className = "gone";
    row.replaceChildren();
    row.textContent = `${name} removed. Its data stays on the reader.`;
    markCatalogueRow(id);
  } catch (error) {
    button.disabled = false;
    delete button.dataset.armed;
    button.textContent = "Remove";
    const why = document.createElement("span");
    why.className = "bad";
    why.textContent = `${name} could not be removed: ${error.message || error.name || "no reason given"}.`;
    row.append(why);
  }
}

// The catalogue marks what is already on the reader; after a removal that
// mark is no longer true for this application.
function markCatalogueRow(id) {
  const row = document.querySelector(`#applist [data-id="${id}"] em`);
  if (!row) return;
  const app = catalogue.find(entry => entry.id === id);
  if (app) row.textContent = `${(app.bytes / 1048576).toFixed(1)} MB`;
}

async function fetchRelease() {
  fetchButton.disabled = true;
  fetchNote.textContent = "Checking what is available\u2026";
  const channel = chosenChannel();
  try {
    // Checked before it is parsed. A 404 here answers with a page, and parsing
    // that as JSON reports a syntax error to somebody who wanted to install a
    // reading application.
    let described = await fetch(MANIFEST[channel], { cache: "no-store" });
    let served = channel;
    // The site carries a Beta release only once one has been published since
    // the site learnt to offer it. Until then the Stable release is written,
    // and the reader, told to follow Beta, moves itself to the newest Beta the
    // first time it checks for updates over Wi-Fi. That is also what Settings
    // on the reader does when Beta is chosen there.
    if (!described.ok && channel === "beta" && described.status === 404) {
      described = await fetch(MANIFEST.stable, { cache: "no-store" });
      served = "stable";
    }
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

    // The choice may have moved while the bytes were arriving. What arrived
    // was asked for under the channel chosen then, and offering it for the
    // channel chosen now would write one channel's release to a reader told
    // to follow the other. It is let go, and Write stays dark.
    if (chosenChannel() !== channel) {
      archive = null;
      archiveChannel = null;
      bar.hidden = true;
      writeButton.disabled = true;
      fetchButton.disabled = false;
      fetchNote.textContent =
        `The channel was changed while Cobalt was downloading. Download again for the ${chosenChannel() === "beta" ? "Beta" : "Stable"} channel.`;
      return;
    }

    archive = bytes;
    archiveChannel = channel;
    const label = served === "beta" ? `Cobalt ${escapeText(manifest.version)} (Beta)` : `Cobalt ${escapeText(manifest.version)}`;
    const change = !installed
      ? `${label} verified.`
      : installed === manifest.version
        ? `${label} verified. This reader already has that version, so this reinstalls it.`
        : `${label} verified. This reader has ${escapeText(installed)}.`;
    const stood = served !== channel
      ? " No Beta release is published here yet, so this is the Stable one; the reader moves to Beta when it next updates over Wi-Fi."
      : "";
    fetchNote.innerHTML = `<span class="ok">${change}${stood}</span>`;
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

// The radios are dark while a write runs. The choice is settled by what is
// being written, and a change made then is one the write could not honour.
function setChannelChoiceEnabled(on) {
  for (const input of Object.values(channelChoice)) {
    if (input) input.disabled = !on;
  }
}

async function writeToReader() {
  // The button is the usual guard; this is the one that holds if the button
  // and the state ever disagree.
  if (drive === null || archive === null || !untestedAcknowledged) return;
  // Taken now and used throughout, the cleanup included. The page's own state
  // can move while the write runs: a channel change puts the download back,
  // and a write that read `archive` after that found nothing to measure the
  // slot against, and took a complete archive off the reader as a partial
  // one. The channel recorded is the one the archive was downloaded for, so
  // the two cannot disagree either.
  const reader = drive;
  const bytes = archive;
  const described = manifest;
  const channel = archiveChannel;
  writeButton.disabled = true;
  setChannelChoiceEnabled(false);
  try {
    writeNote.textContent = "Writing the release…";
    const system = await reader.getDirectoryHandle(SLOT_FOLDER, { create: true });

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
    await target.write(bytes);
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
    if (written.length !== bytes.length) {
      throw new Error(
        `only ${written.length.toLocaleString()} of ${bytes.length.toLocaleString()} bytes ` +
        "were written, so the reader was left alone. Check there is room on the drive and try again."
      );
    }
    const writtenDigest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", written))]
      .map(byte => byte.toString(16).padStart(2, "0")).join("");
    if (writtenDigest !== described.sha256) {
      throw new Error("what ended up on the reader is not what was downloaded, so it was taken off again.");
    }

    writeNote.textContent = "Adding the menu entry…";
    const adds = await reader.getDirectoryHandle(MENU_FOLDER, { create: true });
    const nm = await adds.getDirectoryHandle(MENU_SUBFOLDER, { create: true });
    const entry = await nm.getFileHandle(MENU_FILE, { create: true });
    const entryTarget = await entry.createWritable();
    await entryTarget.write(new TextEncoder().encode(MENU_ENTRY));
    await entryTarget.close();

    writeNote.textContent = "Recording the update channel…";
    await writeChannel(reader, adds, channel);

    const chosen = chosenApps();
    if (chosen.length > 0) {
      const cobalt = await adds.getDirectoryHandle("cobalt", { create: true });
      const appsFolder = await cobalt.getDirectoryHandle("apps", { create: true });
      for (const [index, app] of chosen.entries()) {
        writeNote.textContent = `Writing ${app.name} (${index + 1} of ${chosen.length})…`;
        await writeApp(appsFolder, app);
      }
      renderInstalled();
    }

    const tail = (chosen.length > 0 ? ` with ${chosen.length} application${chosen.length === 1 ? "" : "s"}` : "") +
      (channel === "beta" ? ", following Beta" : "");
    writeNote.innerHTML =
      `<span class="ok">Written${escapeText(tail)}. Eject the drive, unplug the cable and restart the reader.</span>`;
    done(step.write);
    enable(step.finish, true);
  } catch (error) {
    // A partial archive in the slot is worse than none: the firmware would try
    // it at the next start and fail on its own, with nobody to say why.
    try {
      const system = await reader.getDirectoryHandle(SLOT_FOLDER);
      const slot = await system.getFileHandle(SLOT_FILE);
      const partial = await slot.getFile();
      if (partial.size !== bytes.length) {
        await system.removeEntry(SLOT_FILE);
      }
    } catch { /* nothing was written, or it is complete and stays */ }
    writeNote.innerHTML = `<span class="bad">${escapeText(error.message)}</span>`;
    writeButton.disabled = false;
  } finally {
    setChannelChoiceEnabled(true);
  }
}

// The list is whatever the Store was offering when the site was built, not a
// list kept by hand here, so an application added to the catalogue appears
// without anyone editing this page.
//
// Asked for once. Every verified download used to ask again, and each answer
// put a second copy of every row under the first, so an application ticked in
// both copies was written twice. A load that failed may be asked for again,
// since it left no rows behind.
let catalogueRequested = false;

async function loadCatalogue() {
  if (catalogueRequested) return;
  catalogueRequested = true;
  try {
    catalogue = await (await fetch("apps.json", { cache: "no-store" })).json();
  } catch {
    catalogueRequested = false;
    appsNote.textContent =
      "The application catalogue could not be read. Cobalt can still be installed, " +
      "and applications can be added later from the Store on the reader.";
    return;
  }
  const list = document.querySelector("#applist");
  for (const app of catalogue) {
    const row = document.createElement("label");
    row.className = "app";
    row.dataset.id = app.id;
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
  renderInstalled();
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
  // A removal recorded earlier, here or by the Store, is over once the three
  // files agree again. The runtime would reach the same conclusion at its
  // next look and clear the record itself; clearing it here means the
  // reader's first start has nothing to decide. Last, because until the
  // signature is down the record is still telling the truth.
  try { await appsFolder.removeEntry(`${app.id}.${UNINSTALLED_SUFFIX}`); } catch { /* none */ }
  installedApps.add(app.id);
}

// Records the chosen channel the way Settings on the reader would, and only
// when it changes something. To the runtime no file means Stable, so a reader
// kept on Stable is left without one, and a reader's other two switches are
// carried over rather than reset.
async function writeChannel(reader, adds, channel) {
  const current = await readPreferences(reader);
  if (current.channel === channel) return;
  const cobalt = await adds.getDirectoryHandle(INSTALL_FOLDER, { create: true });
  const state = await cobalt.getDirectoryHandle(STATE_FOLDER, { create: true });
  const chosen = { cobalt: current.cobalt, apps: current.apps, channel };
  await writeFile(state, PREFERENCES_FILE, new TextEncoder().encode(renderPreferences(chosen)));
  preferences = { present: true, ...chosen };
}

async function writeFile(folder, name, bytes) {
  const handle = await folder.getFileHandle(name, { create: true });
  const target = await handle.createWritable();
  await target.write(bytes);
  await target.close();
}

// The by-hand route shows the same archive and the same menu entry the steps
// above would write, read from the same manifest and the same constant, so the
// two cannot drift into installing different things.
async function describeManualRoute() {
  const entry = document.querySelector("#by-hand-entry");
  if (entry) entry.textContent = MENU_ENTRY;
  try {
    const described = await fetch("manifest.json", { cache: "no-store" });
    if (!described.ok) return;
    const facts = await described.json();
    const link = document.querySelector("#by-hand-archive");
    if (link) link.setAttribute("href", facts.archive);
    const size = document.querySelector("#by-hand-size");
    if (size) {
      size.textContent =
        `Cobalt ${facts.version} with NickelMenu ${facts.nickelmenu}, ${(facts.bytes / 1048576).toFixed(1)} MB`;
    }
  } catch {
    // The steps above report a manifest that cannot be read; the link still
    // points at the archive.
  }
}

function escapeText(value) {
  const holder = document.createElement("span");
  holder.textContent = String(value);
  return holder.innerHTML;
}

if (refuseUnsupportedBrowser()) {
  // The only route left, so it is opened rather than left to be discovered.
  document.querySelector("#by-hand").open = true;
} else {
  pickButton.addEventListener("click", chooseDrive);
  fetchButton.addEventListener("click", fetchRelease);
  writeButton.addEventListener("click", writeToReader);
  for (const input of Object.values(channelChoice)) {
    if (input) input.addEventListener("change", describeChannel);
  }
  describeChannel();
}

// After the wiring, and its failure is its own. This filled in the by-hand
// section and was called first; when it referred to something that no longer
// existed it threw, the wiring below it never ran, and every button on the page
// did nothing in every browser.
describeManualRoute();
