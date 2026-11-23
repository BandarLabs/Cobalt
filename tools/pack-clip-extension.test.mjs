import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateRawSync } from "node:zlib";
import { crc32, pack, shippedFiles, storeManifest, zip } from "./pack-clip-extension.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const extension = join(root, "extensions/clip");
const script = join(root, "tools/pack-clip-extension.mjs");
const manifest = JSON.parse(readFileSync(join(extension, "manifest.json"), "utf8"));
const installer = readFileSync(join(root, "crates/kobo-cli/src/clip_install.rs"), "utf8");

function scratch() {
  return mkdtempSync(join(tmpdir(), "pack-clip-extension-"));
}

// Reads a zip back the way a store's own unpacker would: from the directory
// at its end, checking each entry's length and checksum on the way.
function unzipped(archive) {
  const end = archive.length - 22;
  assert.equal(archive.toString("latin1", end, end + 4), "PK\x05\x06", "the archive ends with its directory");
  const count = archive.readUInt16LE(end + 10);
  let at = archive.readUInt32LE(end + 16);
  const files = new Map();
  for (let index = 0; index < count; index += 1) {
    assert.equal(archive.toString("latin1", at, at + 4), "PK\x01\x02");
    const method = archive.readUInt16LE(at + 10);
    const date = archive.readUInt16LE(at + 14);
    const checksum = archive.readUInt32LE(at + 16);
    const packed = archive.readUInt32LE(at + 20);
    const size = archive.readUInt32LE(at + 24);
    const nameLength = archive.readUInt16LE(at + 28);
    const local = archive.readUInt32LE(at + 42);
    const name = archive.toString("utf8", at + 46, at + 46 + nameLength);
    assert.equal(archive.toString("latin1", local, local + 4), "PK\x03\x04", `${name} is where the directory says`);
    const start = local + 30 + archive.readUInt16LE(local + 26) + archive.readUInt16LE(local + 28);
    const body = archive.subarray(start, start + packed);
    const bytes = method === 8 ? inflateRawSync(body) : body;
    assert.equal(bytes.length, size, `${name} is the length it says`);
    assert.equal(crc32(bytes), checksum, `${name} matches its checksum`);
    files.set(name, { bytes, date });
    at += 46 + nameLength + archive.readUInt16LE(at + 30) + archive.readUInt16LE(at + 32);
  }
  return files;
}

function packed(family) {
  const out = scratch();
  const made = pack(out).find(zipped => zipped.family === family);
  return unzipped(readFileSync(made.path));
}

function manifestIn(files) {
  return JSON.parse(files.get("manifest.json").bytes.toString("utf8"));
}

test("each store gets one zip, named for its browser family and the extension's version", () => {
  const out = join(scratch(), "not", "made", "yet");
  const made = pack(out);
  assert.deepEqual(
    made.map(zipped => zipped.family),
    ["chromium", "firefox"]
  );
  assert.deepEqual(readdirSync(out).sort(), [
    `send-to-kobo-${manifest.version}-chromium.zip`,
    `send-to-kobo-${manifest.version}-firefox.zip`
  ]);
  for (const { path } of made) assert.ok(existsSync(path));
});

test("the Chromium zip's manifest has no key, which that store rejects, and is otherwise the manifest", () => {
  assert.ok(manifest.key, "the unpacked extension pins its ID with a key");
  const stored = manifestIn(packed("chromium"));
  assert.equal(stored.key, undefined);
  assert.equal(stored.browser_specific_settings, undefined);
  const { key, browser_specific_settings: gecko, ...shared } = manifest;
  assert.deepEqual(stored, shared);
  assert.deepEqual(stored.permissions, manifest.permissions);
});

test("the Firefox zip keeps the ID Firefox knows the extension by", () => {
  const stored = manifestIn(packed("firefox"));
  assert.deepEqual(stored.browser_specific_settings, manifest.browser_specific_settings);
  assert.ok(stored.browser_specific_settings.gecko.id);
  assert.equal(stored.key, undefined);
  const { key, ...kept } = manifest;
  assert.deepEqual(stored, kept);
});

test("the manifest in the source tree is not changed by packing it", () => {
  const before = JSON.stringify(manifest);
  storeManifest(manifest, "chromium");
  storeManifest(manifest, "firefox");
  assert.equal(JSON.stringify(manifest), before);
  assert.throws(() => storeManifest(manifest, "safari"), /not a browser family/);
});

test("each zip holds exactly the files kobo clip install copies out, byte for byte", () => {
  const copied = [...installer.matchAll(/include_bytes!\("\.\.\/\.\.\/\.\.\/extensions\/clip\/([^"]+)"\)/g)]
    .map(found => found[1])
    .sort();
  assert.ok(copied.length >= 6, "the installer's list was found");
  assert.deepEqual(shippedFiles(manifest), copied);
  for (const family of ["chromium", "firefox"]) {
    const files = packed(family);
    assert.deepEqual([...files.keys()], copied);
    for (const name of copied.filter(name => name !== "manifest.json")) {
      assert.ok(files.get(name).bytes.equals(readFileSync(join(extension, name))), `${family}: ${name}`);
    }
  }
});

test("the same tree packs to the same bytes, with no clock in them", () => {
  const first = pack(scratch());
  const second = pack(scratch());
  for (const [index, { path }] of first.entries()) {
    assert.ok(readFileSync(path).equals(readFileSync(second[index].path)), path);
  }
  // 1 January 1980 in the format's packed date, for every entry.
  for (const { date } of packed("chromium").values()) assert.equal(date, 33);
});

test("an archive of text is compressed, read back whole and listed in name order", () => {
  const text = Buffer.from("the tide comes in twice a day\n".repeat(200), "utf8");
  const archive = zip([
    { name: "b/text.txt", bytes: text },
    { name: "a.bin", bytes: Buffer.from([0, 1, 2, 3]) },
    { name: "empty", bytes: Buffer.alloc(0) }
  ]);
  assert.ok(archive.length < text.length);
  const files = unzipped(archive);
  assert.deepEqual([...files.keys()], ["a.bin", "b/text.txt", "empty"]);
  assert.ok(files.get("b/text.txt").bytes.equals(text));
  assert.equal(files.get("empty").bytes.length, 0);
  assert.equal(crc32(Buffer.from("123456789")), 0xcbf43926);
});

test("run as a command it writes both zips and says where, and without --out it writes nothing", () => {
  const out = scratch();
  const run = spawnSync(process.execPath, [script, "--out", out], { encoding: "utf8" });
  assert.equal(run.status, 0, run.stderr);
  assert.deepEqual(
    run.stdout.trim().split("\n"),
    ["chromium", "firefox"].map(family => `${family}: ${join(out, `send-to-kobo-${manifest.version}-${family}.zip`)}`)
  );
  const bare = spawnSync(process.execPath, [script], { encoding: "utf8", cwd: scratch() });
  assert.equal(bare.status, 2);
  assert.equal(bare.stderr.trim(), "usage: node tools/pack-clip-extension.mjs --out DIR");
  assert.equal(bare.stdout, "");
});
