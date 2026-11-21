import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const extension = join(root, "extensions/clip");
const manifest = JSON.parse(readFileSync(join(extension, "manifest.json"), "utf8"));
const background = readFileSync(join(extension, "background.js"), "utf8");
const installer = readFileSync(join(root, "crates/kobo-cli/src/clip_install.rs"), "utf8");

function rustConstant(name) {
  const found = new RegExp(`pub const ${name}: &str = "([^"]+)";`).exec(installer);
  assert.ok(found, `${name} is declared in clip_install.rs`);
  return found[1];
}

test("the extension asks for exactly the five permissions it uses and no sites", () => {
  assert.deepEqual(
    [...manifest.permissions].sort(),
    ["activeTab", "contextMenus", "nativeMessaging", "notifications", "scripting"]
  );
  for (const wider of ["host_permissions", "optional_permissions", "optional_host_permissions", "content_scripts", "externally_connectable", "web_accessible_resources"]) {
    assert.equal(manifest[wider], undefined, `${wider} is not asked for`);
  }
});

test("the manifest is version 3 and runs the same background script in every browser", () => {
  assert.equal(manifest.manifest_version, 3);
  assert.equal(manifest.background.service_worker, "background.js");
  assert.deepEqual(manifest.background.scripts, ["background.js"]);
  assert.match(manifest.version, /^\d+\.\d+\.\d+$/);
  for (const icon of Object.values(manifest.icons)) {
    assert.ok(existsSync(join(extension, icon)), `${icon} is in the extension`);
  }
});

test("the pinned key gives the Chromium ID the host manifest lets in", () => {
  const digest = createHash("sha256").update(Buffer.from(manifest.key, "base64")).digest("hex");
  const id = [...digest.slice(0, 32)].map(hex => String.fromCharCode(97 + parseInt(hex, 16))).join("");
  assert.equal(id, rustConstant("CHROMIUM_ID"));
  assert.equal(manifest.browser_specific_settings.gecko.id, rustConstant("FIREFOX_ID"));
});

test("the extension talks to the host install registers, and to nothing on the network", () => {
  assert.ok(background.includes(`const HOST = "${rustConstant("HOST_NAME")}";`));
  for (const network of ["fetch(", "XMLHttpRequest", "WebSocket", "sendBeacon", "EventSource"]) {
    assert.ok(!background.includes(network), `background.js does not use ${network}`);
  }
  assert.doesNotThrow(() => new vm.Script(background, { filename: "background.js" }));
});

test("the right-click menu offers the page, a link and a selection", () => {
  for (const title of ["Send page to Kobo", "Send link to Kobo", "Send selection to Kobo"]) {
    assert.ok(background.includes(`title: "${title}"`), title);
  }
});

test("the toolbar button has no popup, so one click is the whole gesture", () => {
  assert.equal(manifest.action.default_title, "Send page to Kobo");
  assert.equal(manifest.action.default_popup, undefined);
  for (const icon of Object.values(manifest.action.default_icon)) {
    assert.ok(existsSync(join(extension, icon)), `${icon} is in the extension`);
  }
});

test("the one keyboard shortcut presses the toolbar button, in keys every browser accepts", () => {
  assert.deepEqual(Object.keys(manifest.commands), ["_execute_action"]);
  const keys = manifest.commands._execute_action.suggested_key;
  assert.deepEqual(keys, { default: "Alt+Shift+K" });
  // Chromium wants Ctrl or Alt in a shortcut and Firefox takes the same
  // spelling. No per-platform override, so the README can name one shortcut.
  assert.match(keys.default, /^(Ctrl|Alt)\+(Shift\+)?[A-Z0-9]$/);
  assert.ok(installer.includes(keys.default), "kobo clip install names the same keys");
});

// Runs background.js against a stand-in for the browser and hands back what
// it registered and what it did. `page` is what reading the tab answers;
// `readable: false` is a tab the browser will not let an extension read.
function loaded({ page = null, readable = true } = {}) {
  const listeners = {};
  const listen = name => ({ addListener: listener => { listeners[name] = listener; } });
  const did = { menus: [], scripts: [], sent: [], notices: [] };
  const api = {
    runtime: {
      onInstalled: listen("installed"),
      onStartup: listen("startup"),
      getURL: path => `extension://${path}`,
      connectNative: host => ({
        onMessage: listen("reply"),
        onDisconnect: listen("gone"),
        postMessage: message => did.sent.push({ host, message }),
        disconnect() {}
      })
    },
    contextMenus: {
      removeAll: done => done(),
      create: menu => did.menus.push(menu),
      onClicked: listen("menu")
    },
    action: { onClicked: listen("action") },
    notifications: {
      clear: (id, done) => done(),
      create: (id, notice) => did.notices.push(notice)
    },
    scripting: {
      executeScript: call => {
        did.scripts.push(call);
        return readable ? Promise.resolve([{ result: page }]) : Promise.reject(new Error("not allowed"));
      }
    }
  };
  vm.runInNewContext(background, { chrome: api }, { filename: "background.js" });
  return { listeners, did };
}

// Lets the promise the script is read through settle.
const settled = () => new Promise(resolve => setImmediate(resolve));

// Values made inside the script, as plain data to compare.
const plain = value => JSON.parse(JSON.stringify(value));

const TAB = { id: 7, url: "https://example.org/tides", title: "Tides" };
const PAGE = { url: TAB.url, title: TAB.title, html: "<!DOCTYPE html>\n<html></html>", selection: "" };

test("a click on the toolbar button reads the page in its tab and hands it to kobo", async () => {
  const { listeners, did } = loaded({ page: PAGE });
  listeners.action(TAB);
  await settled();
  assert.equal(did.scripts.length, 1);
  assert.deepEqual(plain(did.scripts[0].target), { tabId: 7 });
  assert.deepEqual(plain(did.scripts[0].args), [false]);
  assert.equal(did.scripts[0].func.name, "capturePage");
  assert.deepEqual(plain(did.sent), [{ host: rustConstant("HOST_NAME"), message: PAGE }]);
  assert.equal(did.notices[0].title, "Sending to your Kobo");
});

test("a page the browser will not let the button read is sent by its address", async () => {
  const { listeners, did } = loaded({ readable: false });
  listeners.action(TAB);
  await settled();
  assert.deepEqual(plain(did.sent), [
    { host: rustConstant("HOST_NAME"), message: { url: TAB.url, title: TAB.title } }
  ]);
});

test("the button on a page that is not a web page says so and sends nothing", async () => {
  for (const tab of [{ id: 3, url: "chrome://extensions", title: "Extensions" }, { id: 4 }, undefined]) {
    const { listeners, did } = loaded({ page: PAGE });
    listeners.action(tab);
    await settled();
    assert.equal(did.scripts.length, 0);
    assert.equal(did.sent.length, 0);
    assert.equal(did.notices.at(-1).message, "Only web pages can be sent.");
  }
});

test("the menu still sends the page, a selection or a link, the link without reading the tab", async () => {
  const { listeners, did } = loaded({ page: PAGE });
  listeners.installed();
  assert.deepEqual(did.menus.map(menu => menu.id), ["send-page", "send-link", "send-selection"]);
  listeners.menu({ menuItemId: "send-selection", pageUrl: TAB.url }, TAB);
  await settled();
  assert.deepEqual(plain(did.scripts[0].args), [true]);
  listeners.menu({ menuItemId: "send-link", linkUrl: "https://example.org/other", linkText: "Other" }, TAB);
  await settled();
  assert.equal(did.scripts.length, 1);
  assert.deepEqual(plain(did.sent[1].message), { url: "https://example.org/other", title: "Other" });
});
