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
const profiles = [{ code: "N249", model: "Kobo Clara HD", firmware: ["4.38.23697"], ready: true }];

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
    async *entries() { yield* entries; }
  };
}

function reader(extra = {}, serial = "N249000000000") {
  return directory("KOBOeReader", { ".kobo": { version: `${serial},4.1.15,4.38.23697\n` }, ...extra });
}

function page(drive = reader(), { devices, pickerError } = {}) {
  function element() {
    return {
      dataset: {}, attributes: {}, handlers: {}, children: [], style: {},
      disabled: false, hidden: false, innerHTML: "",
      set textContent(value) {
        this.innerHTML = String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;")
          .replaceAll(">", "&gt;").replaceAll('"', "&quot;");
      },
      setAttribute(name, value) { this.attributes[name] = value; },
      removeAttribute(name) { delete this.attributes[name]; },
      addEventListener(name, handler) { this.handlers[name] = handler; },
      querySelector() { return element(); },
      append(child) { this.children.push(child); }
    };
  }
  const nodes = new Map([...html.matchAll(/<[^>]*\bid="([^"]+)"[^>]*>/g)].map(([tag, id]) => {
    const node = element();
    node.disabled = /\sdisabled(?:\s|>)/.test(tag);
    if (tag.includes('aria-disabled="true"')) node.attributes["aria-disabled"] = "true";
    return [`#${id}`, node];
  }));
  const consent = element();
  const context = createContext({
    document: {
      querySelector(selector) {
        if (selector === "#untested-ok") {
          return nodes.get("#pick-note").innerHTML.includes('id="untested-ok"') ? consent : null;
        }
        return nodes.get(selector) || null;
      },
      querySelectorAll() { return []; },
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
      if (url === "apps.json") return Response.json([]);
      throw new Error(`Unexpected fetch: ${url}`);
    },
    TextEncoder, TextDecoder, crypto: webcrypto
  });
  runInContext(script, context, { filename: "install.js" });
  return {
    node: selector => nodes.get(selector), consent,
    click: selector => nodes.get(selector).handlers.click(),
    evaluate: expression => runInContext(expression, context)
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
