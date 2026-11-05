import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createContext, runInContext } from "node:vm";
import { createHash, webcrypto } from "node:crypto";

const script = await readFile(new URL("./install.js", import.meta.url), "utf8");
const html = await readFile(new URL("./index.html", import.meta.url), "utf8");
const payload = new TextEncoder().encode("test release archive");
const release = {
  version: "0.3.10", archive: "cobalt-KoboRoot.tgz", bytes: payload.length,
  sha256: createHash("sha256").update(payload).digest("hex"), nickelmenu: "0.6.0"
};
const betaPayload = new TextEncoder().encode("test beta release archive");
const betaRelease = {
  version: "0.3.12", archive: "beta/cobalt-KoboRoot.tgz", bytes: betaPayload.length,
  sha256: createHash("sha256").update(betaPayload).digest("hex"), nickelmenu: "0.6.0"
};

// An application package as the site serves it: magic, format, manifest
// length, a signature, the manifest and the executable. The page splits it
// into the three files the reader expects, so the test needs the real shape.
function bundle(id, manifest = `{"id":"${id}"}`, binary = `binary of ${id}`) {
  const manifestBytes = new TextEncoder().encode(manifest);
  const binaryBytes = new TextEncoder().encode(binary);
  const bytes = new Uint8Array(78 + manifestBytes.length + binaryBytes.length);
  bytes.set(new TextEncoder().encode("COBALTAP"), 0);
  const view = new DataView(bytes.buffer);
  view.setUint16(8, 1);
  view.setUint32(10, manifestBytes.length);
  bytes.fill(0xab, 14, 78);
  bytes.set(manifestBytes, 78);
  bytes.set(binaryBytes, 78 + manifestBytes.length);
  return bytes;
}
const feeds = bundle("rss");
const catalogue = [{
  id: "rss", name: "Feeds", summary: "Follow feeds.", bytes: feeds.length,
  sha256: createHash("sha256").update(feeds).digest("hex")
}];
const profiles = [
  { code: "N249", model: "Kobo Clara HD", firmware: ["4.38.23697"], ready: true },
  { code: "N506", model: "Tolino Shine 5", firmware: ["4.38.23697"], ready: false }
];

// Exercise the shipped script and its registered click handlers without USB
// hardware. Handles retain writes in memory and reject missing entries like
// File System Access does on a fresh reader.
function directory(name, contents = {}) {
  const entries = new Map(Object.entries(contents).map(([key, value]) => [key,
    typeof value === "string" ? {
      kind: "file",
      bytes: new TextEncoder().encode(value),
      async getFile() { return new Blob([this.bytes]); },
      async createWritable() {
        return { write: async bytes => { this.bytes = bytes; }, close: async () => {} };
      }
    } : directory(key, value)
  ]));
  return {
    name, kind: "directory",
    async queryPermission() { return "granted"; },
    async getDirectoryHandle(key, options = {}) {
      if (!entries.has(key) && options.create) entries.set(key, directory(key));
      const entry = entries.get(key);
      if (entry?.kind !== "directory") throw new DOMException(key, "NotFoundError");
      return entry;
    },
    async getFileHandle(key, options = {}) {
      if (!entries.has(key) && options.create) {
        const source = directory("", { [key]: "" });
        entries.set(key, await source.getFileHandle(key));
      }
      const entry = entries.get(key);
      if (entry?.kind !== "file") throw new DOMException(key, "NotFoundError");
      return entry;
    },
    async removeEntry(key) {
      if (!entries.has(key)) throw new DOMException(key, "NotFoundError");
      entries.delete(key);
    },
    has(key) { return entries.has(key); },
    async *entries() { yield* entries; }
  };
}

async function textOf(folder, name) {
  return (await (await folder.getFileHandle(name)).getFile()).text();
}

// The catalogue loads behind the download rather than holding it up, so a
// test that wants the loaded catalogue lets that happen first.
function settled() {
  return new Promise(resolve => setTimeout(resolve, 5));
}

function reader(extra = {}, serial = "N249000000000", firmware = "4.38.23697") {
  return directory("KOBOeReader", { ".kobo": { version: `${serial},4.1.15,${firmware}\n` }, ...extra });
}

function page(drive = reader(), { devices, pickerError, beta, apps = [], bundles = {} } = {}) {
  function element() {
    return {
      dataset: {}, attributes: {}, handlers: {}, children: [], style: {},
      disabled: false, hidden: false, checked: false, innerHTML: "",
      set textContent(value) {
        this.innerHTML = String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;")
          .replaceAll(">", "&gt;").replaceAll('"', "&quot;");
      },
      get textContent() { return this.innerHTML; },
      setAttribute(name, value) { this.attributes[name] = value; },
      removeAttribute(name) { delete this.attributes[name]; },
      addEventListener(name, handler) { this.handlers[name] = handler; },
      querySelector() { return element(); },
      append(...added) { this.children.push(...added); },
      replaceChildren() { this.children = []; }
    };
  }
  const nodes = new Map([...html.matchAll(/<[^>]*\bid="([^"]+)"[^>]*>/g)].map(([tag, id]) => {
    const node = element();
    node.disabled = /\sdisabled(?:\s|>)/.test(tag);
    node.checked = /\schecked(?:\s|>)/.test(tag);
    if (tag.includes('aria-disabled="true"')) node.attributes["aria-disabled"] = "true";
    return [`#${id}`, node];
  }));
  const consent = element();
  // What the catalogue step has ticked, standing in for the checked inputs the
  // page would otherwise query out of the application list.
  const ticked = [];
  const context = createContext({
    document: {
      querySelector(selector) {
        if (selector === "#untested-ok") {
          return nodes.get("#pick-note").innerHTML.includes('id="untested-ok"') ? consent : null;
        }
        return nodes.get(selector) || null;
      },
      querySelectorAll() { return ticked; },
      createElement: element
    },
    window: {
      async showDirectoryPicker(options) {
        assert.equal(options.mode, "readwrite");
        if (pickerError) throw pickerError;
        return drive;
      },
      FileSystemFileHandle: { prototype: { createWritable() {} } },
      FileSystemDirectoryHandle: { prototype: { getDirectoryHandle() {} } }
    },
    async fetch(url) {
      if (url === "devices.json") return devices ? devices() : Response.json(profiles);
      if (url === "manifest.json") return Response.json(release);
      if (url === release.archive) return new Response(payload);
      if (url === "beta/manifest.json") {
        return beta ? Response.json(betaRelease) : new Response("missing", { status: 404 });
      }
      if (beta && url === betaRelease.archive) return new Response(betaPayload);
      if (url === "apps.json") return Response.json(apps);
      const packaged = url.match(/^apps\/([a-z-]+)\.cobalt-app$/);
      if (packaged && bundles[packaged[1]]) return new Response(bundles[packaged[1]]);
      throw new Error(`Unexpected fetch: ${url}`);
    },
    TextEncoder, TextDecoder, crypto: webcrypto
  });
  runInContext(script, context, { filename: "install.js" });
  return {
    node: selector => nodes.get(selector), consent,
    click: selector => nodes.get(selector).handlers.click(),
    evaluate: expression => runInContext(expression, context),
    // Chooses a channel the way a click on its radio would: the browser
    // unticks the other one before the change event fires.
    choose(channel) {
      nodes.get("#channel-stable").checked = channel === "stable";
      nodes.get("#channel-beta").checked = channel === "beta";
      nodes.get(`#channel-${channel}`).handlers.change();
    },
    tick: id => ticked.push({ value: id }),
    // The installed list's rows, each a name and, where allowed, a Remove.
    rows: () => nodes.get("#installed").children.map(row => ({
      row,
      name: row.children[0]?.innerHTML,
      remove: row.children[1] ?? null
    }))
  };
}

function assertConnected(ui) {
  assert.equal(ui.node("#step-connect").dataset.state, "done");
  assert.equal(ui.node("#step-download").attributes["aria-disabled"], undefined);
  assert.equal(ui.node("#fetch").disabled, false);
}

test("a fresh Clara HD advances from selection through verified download and write", async () => {
  const drive = reader();
  const ui = page(drive);
  await ui.click("#pick");
  assertConnected(ui);
  assert.match(ui.node("#pick-note").innerHTML, /Cobalt is not installed yet/);
  assert.match(ui.node("#pick-note").innerHTML, /Kobo Clara HD, firmware 4.38.23697/);
  await assert.rejects(drive.getDirectoryHandle(".adds"), { name: "NotFoundError" });

  await ui.click("#fetch");
  assert.equal(ui.node("#write").disabled, false);
  assert.match(ui.node("#fetch-note").innerHTML, /0.3.10 verified/);
  await ui.click("#write");
  assert.equal(ui.node("#step-write").dataset.state, "done");
  assert.equal(ui.node("#step-finish").attributes["aria-disabled"], undefined);
  const system = await drive.getDirectoryHandle(".kobo");
  assert.equal(await (await (await system.getFileHandle("KoboRoot.tgz")).getFile()).text(), "test release archive");
  const adds = await drive.getDirectoryHandle(".adds");
  const nm = await adds.getDirectoryHandle("nm");
  assert.match(await (await (await nm.getFileHandle("cobalt")).getFile()).text(), /menu_item :main :Cobalt/);
});

test("existing version and apps are read, excluding staging directories and files", async () => {
  const ui = page(reader({ ".adds": { cobalt: {
    VERSION: " 0.3.9\n", apps: { rss: {}, "rss.next": {}, "rss.prev": {}, "notes.txt": "" }
  } } }));
  await ui.click("#pick");
  assertConnected(ui);
  assert.match(ui.node("#pick-note").innerHTML, /Cobalt 0.3.9 installed/);
  assert.deepEqual(Array.from(ui.evaluate("installedApps")), ["rss"]);
  await ui.click("#fetch");
  assert.match(ui.node("#fetch-note").innerHTML, /This reader has 0.3.9/);
});

for (const [name, devices] of [
  ["HTTP error", () => new Response("missing", { status: 404 })],
  ["network error", () => { throw new Error("offline"); }],
  ["invalid JSON", () => new Response("invalid JSON")]
]) {
  test(`device profile ${name} does not block drive selection`, async () => {
    const ui = page(reader(), { devices });
    await ui.click("#pick");
    assertConnected(ui);
  });
}

test("an untested model still requires consent before writing", async () => {
  const ui = page(reader({}, "N999000000000"));
  await ui.click("#pick");
  assertConnected(ui);
  assert.match(ui.node("#pick-note").innerHTML, /has not been tested/);
  await ui.click("#fetch");
  assert.equal(ui.node("#write").disabled, true);
  ui.consent.checked = true;
  ui.consent.handlers.change();
  assert.equal(ui.node("#write").disabled, false);
});

test("an ordinary drive is refused", async () => {
  const ui = page(directory("USB"));
  await ui.click("#pick");
  assert.equal(ui.node("#fetch").disabled, true);
  assert.match(ui.node("#pick-note").innerHTML, /it is not a Kobo/);
});

test("picker cancellation leaves download disabled", async () => {
  const ui = page(reader(), { pickerError: new DOMException("cancelled", "AbortError") });
  await ui.click("#pick");
  assert.equal(ui.node("#fetch").disabled, true);
  assert.equal(ui.node("#pick-note").innerHTML, "");
});

test("denied write permission leaves download disabled", async () => {
  const drive = reader();
  drive.queryPermission = async () => "denied";
  drive.requestPermission = async () => "denied";
  const ui = page(drive);
  await ui.click("#pick");
  assert.equal(ui.node("#fetch").disabled, true);
  assert.match(ui.node("#pick-note").innerHTML, /Writing was not allowed/);
});

for (const firmware of ["5.1.184318", "5.0.0", "3.19.5761", "", "unknown", "4.38", "4.38.", "4..23697", ".38.23697"]) {
  test(`firmware ${JSON.stringify(firmware)} is refused before anything is offered`, async () => {
    const drive = reader({}, "N249000000000", firmware);
    const ui = page(drive);
    await ui.click("#pick");
    assert.notEqual(ui.node("#step-connect").dataset.state, "done");
    assert.equal(ui.node("#fetch").disabled, true);
    assert.equal(ui.node("#write").disabled, true);
    assert.match(ui.node("#pick-note").innerHTML, /can only be started on 4\.x firmware/);
    assert.match(ui.node("#pick-note").innerHTML, /Nothing was written/);
    assert.doesNotMatch(ui.node("#pick-note").innerHTML, /untested-ok/);
    assert.equal(ui.evaluate("drive"), null);
    await assert.rejects(drive.getDirectoryHandle(".adds"), { name: "NotFoundError" });
  });
}

test("a refused reader disarms one chosen earlier in the same visit", async () => {
  let drive = reader();
  const ui = page({
    get name() { return drive.name; },
    queryPermission: () => drive.queryPermission(),
    getDirectoryHandle: (...args) => drive.getDirectoryHandle(...args),
    getFileHandle: (...args) => drive.getFileHandle(...args),
    entries: () => drive.entries()
  });
  await ui.click("#pick");
  assertConnected(ui);
  drive = reader({}, "N249000000000", "5.1.184318");
  await ui.click("#pick");
  assert.equal(ui.evaluate("drive"), null);
  assert.equal(ui.node("#fetch").disabled, true);
  assert.equal(ui.node("#step-download").attributes["aria-disabled"], "true");
});

test("a tested model on an untested firmware branch asks for consent", async () => {
  const ui = page(reader({}, "N249000000000", "4.41.23145"));
  await ui.click("#pick");
  assertConnected(ui);
  assert.match(ui.node("#pick-note").innerHTML, /tested on firmware 4\.38\.x, not on this one/);
  await ui.click("#fetch");
  assert.equal(ui.node("#write").disabled, true);
  ui.consent.checked = true;
  ui.consent.handlers.change();
  assert.equal(ui.node("#write").disabled, false);
});

test("a profile still awaiting review is not called tested", async () => {
  const ui = page(reader({}, "N506000000000"));
  await ui.click("#pick");
  assertConnected(ui);
  assert.match(ui.node("#pick-note").innerHTML, /not been fully tested/);
  await ui.click("#fetch");
  assert.equal(ui.node("#write").disabled, true);
});

test("the untested-model message lists only ready models", async () => {
  const ui = page(reader({}, "N999000000000"));
  await ui.click("#pick");
  assert.match(ui.node("#pick-note").innerHTML, /tested are: Kobo Clara HD\./);
  assert.doesNotMatch(ui.node("#pick-note").innerHTML, /Tolino/);
});

// A drive whose contents can be swapped between picks, so one visit can choose
// two readers in turn the way somebody with two Kobos would.
function swappable(first) {
  const current = { drive: first };
  return {
    current,
    handle: {
      get name() { return current.drive.name; },
      queryPermission: () => current.drive.queryPermission(),
      getDirectoryHandle: (...args) => current.drive.getDirectoryHandle(...args),
      getFileHandle: (...args) => current.drive.getFileHandle(...args),
      entries: () => current.drive.entries()
    }
  };
}

for (const [name, serial, firmware] of [
  ["an untested firmware branch", "N249000000000", "4.41.23145"],
  ["a profile awaiting review", "N506000000000", "4.38.23697"],
  ["an unknown model", "N999000000000", "4.38.23697"]
]) {
  test(`choosing ${name} after a download disarms Write until consent`, async () => {
    const tested = reader();
    const swap = swappable(tested);
    const ui = page(swap.handle);
    await ui.click("#pick");
    await ui.click("#fetch");
    assert.equal(ui.node("#write").disabled, false);

    const untested = reader({}, serial, firmware);
    swap.current.drive = untested;
    await ui.click("#pick");
    assertConnected(ui);
    assert.match(ui.node("#pick-note").innerHTML, /untested-ok/);
    assert.equal(ui.node("#write").disabled, true);

    // Even a click that reached the handler writes nothing without consent.
    await ui.click("#write");
    await assert.rejects(untested.getDirectoryHandle(".adds"), { name: "NotFoundError" });
    const system = await untested.getDirectoryHandle(".kobo");
    await assert.rejects(system.getFileHandle("KoboRoot.tgz"), { name: "NotFoundError" });

    ui.consent.checked = true;
    ui.consent.handlers.change();
    assert.equal(ui.node("#write").disabled, false);
  });
}

test("choosing a tested reader after a download keeps the release ready to write", async () => {
  const swap = swappable(reader({}, "N999000000000"));
  const ui = page(swap.handle);
  await ui.click("#pick");
  await ui.click("#fetch");
  assert.equal(ui.node("#write").disabled, true);
  swap.current.drive = reader();
  await ui.click("#pick");
  assert.doesNotMatch(ui.node("#pick-note").innerHTML, /untested-ok/);
  assert.equal(ui.node("#write").disabled, false);
});

for (const [name, drive] of [
  ["an ordinary drive", directory("USB")],
  ["a reader that refuses writing", Object.assign(reader(), {
    queryPermission: async () => "denied", requestPermission: async () => "denied"
  })]
]) {
  test(`${name} chosen after a download disarms the reader before it`, async () => {
    const swap = swappable(reader());
    const ui = page(swap.handle);
    await ui.click("#pick");
    await ui.click("#fetch");
    swap.current.drive = drive;
    await ui.click("#pick");
    assert.equal(ui.evaluate("drive"), null);
    assert.equal(ui.node("#write").disabled, true);
    assert.equal(ui.node("#fetch").disabled, true);
  });
}

test("installed applications are listed by identity until the catalogue names them, and reserved ones get no Remove", async () => {
  const ui = page(reader({ ".adds": { cobalt: {
    VERSION: "0.3.9\n", apps: { rss: {}, launcher: {}, "rss.prev": {} }
  } } }), { apps: catalogue });
  await ui.click("#pick");
  assert.equal(ui.node("#installed").hidden, false);
  assert.deepEqual(ui.rows().map(({ name, remove }) => [name, remove !== null]),
    [["launcher", false], ["rss", true]]);
  assert.equal(ui.rows()[1].remove.innerHTML, "Remove");

  await ui.click("#fetch");
  await settled();
  assert.deepEqual(ui.rows().map(({ name }) => name), ["launcher", "Feeds"]);
});

test("a reader with nothing installed shows no list", async () => {
  const ui = page(reader());
  await ui.click("#pick");
  assert.equal(ui.node("#installed").hidden, true);
  assert.equal(ui.node("#installed-lede").hidden, true);
});

test("removing an application records the removal before deleting its folder and staged copies, and keeps its data", async () => {
  const drive = reader({ ".adds": { cobalt: {
    VERSION: "0.3.9\n",
    apps: { rss: { "manifest.json": "{}", bin: { "kobo-rss": "x" } }, "rss.prev": { "manifest.json": "{}" }, sudoku: {} },
    data: { rss: { "feeds.opml": "kept" } }
  } } });
  const ui = page(drive);
  await ui.click("#pick");
  const { remove } = ui.rows().find(({ name }) => name === "rss");

  // The first click only arms the button and names what would go.
  await remove.handlers.click();
  assert.equal(remove.innerHTML, "Remove rss");
  const apps = await (await (await drive.getDirectoryHandle(".adds")).getDirectoryHandle("cobalt")).getDirectoryHandle("apps");
  assert.equal(apps.has("rss"), true);

  await remove.handlers.click();
  assert.equal(apps.has("rss"), false);
  assert.equal(apps.has("rss.prev"), false);
  assert.equal(apps.has("sudoku"), true);
  assert.equal(await textOf(apps, "rss.uninstalled"), "committed\n");
  const data = await (await (await drive.getDirectoryHandle(".adds")).getDirectoryHandle("cobalt")).getDirectoryHandle("data");
  assert.equal(await textOf(await data.getDirectoryHandle("rss"), "feeds.opml"), "kept");
  assert.deepEqual(Array.from(ui.evaluate("installedApps")), ["sudoku"]);
  assert.match(ui.rows()[0].row.innerHTML, /rss removed\. Its data stays on the reader\./);
});

test("installing an application clears a removal recorded for it", async () => {
  const drive = reader({ ".adds": { cobalt: { VERSION: "0.3.9\n", apps: { "rss.uninstalled": "committed\n" } } } });
  const ui = page(drive, { apps: catalogue, bundles: { rss: feeds } });
  await ui.click("#pick");
  await ui.click("#fetch");
  ui.tick("rss");
  await ui.click("#write");
  assert.equal(ui.node("#step-write").dataset.state, "done");
  const apps = await (await (await drive.getDirectoryHandle(".adds")).getDirectoryHandle("cobalt")).getDirectoryHandle("apps");
  assert.equal(apps.has("rss.uninstalled"), false);
  const rss = await apps.getDirectoryHandle("rss");
  assert.equal(await textOf(rss, "manifest.json"), '{"id":"rss"}');
  assert.equal(await textOf(await rss.getDirectoryHandle("bin"), "kobo-rss"), "binary of rss");
  assert.match(await textOf(rss, "manifest.json.sig"), /^(ab){64}\n$/);
});

test("choosing Beta downloads the Beta release and records the channel with both switches on", async () => {
  const drive = reader();
  const ui = page(drive, { beta: true });
  await ui.click("#pick");
  assert.equal(ui.node("#channel-stable").checked, true);
  ui.choose("beta");
  assert.match(ui.node("#channel-note").innerHTML, /keeps updating from Beta/);
  await ui.click("#fetch");
  assert.match(ui.node("#fetch-note").innerHTML, /Cobalt 0\.3\.12 \(Beta\) verified/);
  await ui.click("#write");
  assert.equal(ui.node("#step-write").dataset.state, "done");
  assert.match(ui.node("#write-note").innerHTML, /following Beta/);
  const system = await drive.getDirectoryHandle(".kobo");
  assert.equal(await textOf(system, "KoboRoot.tgz"), "test beta release archive");
  const cobalt = await (await drive.getDirectoryHandle(".adds")).getDirectoryHandle("cobalt");
  assert.equal(await textOf(await cobalt.getDirectoryHandle("state"), "auto-update"), "cobalt=on\napps=on\nchannel=beta\n");
});

test("a reader kept on Stable gets no preferences file, since to the runtime none means Stable", async () => {
  const drive = reader();
  const ui = page(drive);
  await ui.click("#pick");
  await ui.click("#fetch");
  await ui.click("#write");
  assert.equal(ui.node("#step-write").dataset.state, "done");
  const cobalt = await (await drive.getDirectoryHandle(".adds")).getDirectoryHandle("cobalt", { create: true });
  assert.equal(cobalt.has("state"), false);
});

test("a reader that follows Beta has Beta chosen for it, and keeps its other switches when moved to Stable", async () => {
  const drive = reader({ ".adds": { cobalt: {
    VERSION: "0.3.9\n", state: { "auto-update": "cobalt=on\napps=off\nchannel=beta\n" }
  } } });
  const ui = page(drive, { beta: true });
  await ui.click("#pick");
  assert.equal(ui.node("#channel-beta").checked, true);
  assert.match(ui.node("#pick-note").innerHTML, /installed on the Beta channel/);

  ui.choose("stable");
  assert.match(ui.node("#channel-note").innerHTML, /follows Beta now and will follow Stable/);
  await ui.click("#fetch");
  await ui.click("#write");
  assert.equal(ui.node("#step-write").dataset.state, "done");
  const cobalt = await (await drive.getDirectoryHandle(".adds")).getDirectoryHandle("cobalt");
  assert.equal(await textOf(await cobalt.getDirectoryHandle("state"), "auto-update"), "cobalt=on\napps=off\nchannel=stable\n");
});

test("a reader that follows Beta and stays on it is not rewritten", async () => {
  const drive = reader({ ".adds": { cobalt: {
    VERSION: "0.3.9\n", state: { "auto-update": "cobalt=off\napps=on\nchannel=beta\n" }
  } } });
  const ui = page(drive, { beta: true });
  await ui.click("#pick");
  await ui.click("#fetch");
  await ui.click("#write");
  const cobalt = await (await drive.getDirectoryHandle(".adds")).getDirectoryHandle("cobalt");
  assert.equal(await textOf(await cobalt.getDirectoryHandle("state"), "auto-update"), "cobalt=off\napps=on\nchannel=beta\n");
});

test("changing the channel after a download puts the download back until it is repeated", async () => {
  const drive = reader();
  const ui = page(drive, { beta: true });
  await ui.click("#pick");
  await ui.click("#fetch");
  assert.equal(ui.node("#write").disabled, false);

  ui.choose("beta");
  assert.equal(ui.evaluate("archive"), null);
  assert.equal(ui.node("#write").disabled, true);
  assert.equal(ui.node("#fetch").disabled, false);
  assert.equal(ui.node("#step-download").dataset.state, undefined);
  assert.match(ui.node("#fetch-note").innerHTML, /Download Cobalt again for the Beta channel/);

  // A click that reached the handler anyway writes nothing.
  await ui.click("#write");
  const system = await drive.getDirectoryHandle(".kobo");
  await assert.rejects(system.getFileHandle("KoboRoot.tgz"), { name: "NotFoundError" });

  await ui.click("#fetch");
  assert.equal(ui.node("#write").disabled, false);
  await ui.click("#write");
  assert.equal(await textOf(system, "KoboRoot.tgz"), "test beta release archive");
});

test("Beta falls back to the Stable release when the site has none, and still records Beta", async () => {
  const drive = reader();
  const ui = page(drive);
  await ui.click("#pick");
  ui.choose("beta");
  await ui.click("#fetch");
  assert.match(ui.node("#fetch-note").innerHTML, /Cobalt 0\.3\.10 verified\. No Beta release is published here yet/);
  await ui.click("#write");
  assert.equal(ui.node("#step-write").dataset.state, "done");
  const system = await drive.getDirectoryHandle(".kobo");
  assert.equal(await textOf(system, "KoboRoot.tgz"), "test release archive");
  const cobalt = await (await drive.getDirectoryHandle(".adds")).getDirectoryHandle("cobalt");
  assert.equal(await textOf(await cobalt.getDirectoryHandle("state"), "auto-update"), "cobalt=on\napps=on\nchannel=beta\n");
});
