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
