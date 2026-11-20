#!/usr/bin/env node
// Packs the Send to Kobo extension for the two browser stores.
//
// The folder in extensions/clip is what `kobo clip install` copies out for
// loading unpacked, and one manifest serves every browser there. A store
// wants a zip, and each wants the manifest a little different:
//
// - The Chromium store assigns the extension its ID and rejects an upload
//   whose manifest carries a `key`, which is how the unpacked copy pins its
//   own. So the Chromium zip leaves `key` out. It also leaves out
//   `browser_specific_settings`, which only Firefox reads and Chromium
//   warns about.
// - Firefox knows the extension by the ID in `browser_specific_settings`,
//   so its zip keeps that, and leaves out `key`, which only Chromium reads.
//
// Nothing else differs, and nothing is uploaded from here: the zips are
// written to a folder and the submitting is the owner's to do. See
// PUBLISHING.md.
//
//   node tools/pack-clip-extension.mjs --out DIR
//
// The zip is written here rather than by a `zip` command so that the same
// tree gives the same bytes on every machine with the same Node: entries in
// name order, one fixed timestamp, no owner or permission fields.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateRawSync } from "node:zlib";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const EXTENSION = join(root, "extensions/clip");

export const FAMILIES = ["chromium", "firefox"];

const USAGE = "usage: node tools/pack-clip-extension.mjs --out DIR";

// The manifest a store is given. A copy: the source manifest is not touched.
export function storeManifest(manifest, family) {
  const { key, browser_specific_settings: gecko, ...shared } = manifest;
  if (family === "chromium") return shared;
  if (family === "firefox") return { ...shared, browser_specific_settings: gecko };
  throw new Error(`${family} is not a browser family this packs for`);
}

// Every file the manifest refers to, which is the whole extension: a file
// nothing names is one the browser never loads, and a README does not
// belong in a store upload.
export function shippedFiles(manifest) {
  const named = new Set(["manifest.json"]);
  for (const icon of Object.values(manifest.icons ?? {})) named.add(icon);
  for (const icon of Object.values(manifest.action?.default_icon ?? {})) named.add(icon);
  if (manifest.background?.service_worker) named.add(manifest.background.service_worker);
  for (const script of manifest.background?.scripts ?? []) named.add(script);
  return [...named].sort();
}

const CRC_TABLE = Array.from({ length: 256 }, (_, index) => {
  let value = index;
  for (let bit = 0; bit < 8; bit += 1) value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  return value >>> 0;
});

export function crc32(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes) value = CRC_TABLE[(value ^ byte) & 0xff] ^ (value >>> 8);
  return (value ^ 0xffffffff) >>> 0;
}

// Midnight on 1 January 1980, the earliest date the format can say, as its
// packed date. The time beside it is zero.
const ZIP_DATE = (0 << 9) | (1 << 5) | 1;

// A zip archive of `entries`, each `{ name, bytes }`, in name order.
export function zip(entries) {
  const sorted = [...entries].sort((left, right) => (left.name < right.name ? -1 : 1));
  const locals = [];
  const central = [];
  let offset = 0;
  for (const { name, bytes } of sorted) {
    const path = Buffer.from(name, "utf8");
    const packed = deflateRawSync(bytes, { level: 9 });
    // A picture is already compressed, and deflating it again only adds.
    const stored = packed.length >= bytes.length;
    const body = stored ? bytes : packed;
    const fields = Buffer.alloc(26);
    fields.writeUInt16LE(20, 0); // readable by anything that knows deflate
    fields.writeUInt16LE(0x0800, 2); // names are UTF-8
    fields.writeUInt16LE(stored ? 0 : 8, 4);
    fields.writeUInt16LE(0, 6);
    fields.writeUInt16LE(ZIP_DATE, 8);
    fields.writeUInt32LE(crc32(bytes), 10);
    fields.writeUInt32LE(body.length, 14);
    fields.writeUInt32LE(bytes.length, 18);
    fields.writeUInt16LE(path.length, 22);
    fields.writeUInt16LE(0, 24);

    const local = Buffer.concat([Buffer.from("PK\x03\x04", "latin1"), fields, path, body]);
    const made = Buffer.alloc(2);
    made.writeUInt16LE(20, 0);
    const rest = Buffer.alloc(14);
    rest.writeUInt32LE(offset, 10); // no comment, disk 0, no attributes, then where the entry starts
    central.push(Buffer.concat([Buffer.from("PK\x01\x02", "latin1"), made, fields, rest, path]));
    locals.push(local);
    offset += local.length;
  }
  const directory = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.write("PK\x05\x06", 0, "latin1");
  end.writeUInt16LE(sorted.length, 8);
  end.writeUInt16LE(sorted.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

// The entries of one family's zip, from the extension in `source`.
export function entriesFor(family, source = EXTENSION) {
  const manifest = JSON.parse(readFileSync(join(source, "manifest.json"), "utf8"));
  return shippedFiles(manifest).map(name => ({
    name,
    bytes:
      name === "manifest.json"
        ? Buffer.from(`${JSON.stringify(storeManifest(manifest, family), null, 2)}\n`, "utf8")
        : readFileSync(join(source, name))
  }));
}

// Writes one zip per family into `out`, making the folder if it is not
// there, and answers where each went.
export function pack(out, source = EXTENSION) {
  const { version } = JSON.parse(readFileSync(join(source, "manifest.json"), "utf8"));
  mkdirSync(out, { recursive: true });
  return FAMILIES.map(family => {
    const path = join(out, `send-to-kobo-${version}-${family}.zip`);
    writeFileSync(path, zip(entriesFor(family, source)));
    return { family, path };
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [flag, out, ...more] = process.argv.slice(2);
  if (flag !== "--out" || !out || more.length > 0) {
    console.error(USAGE);
    process.exitCode = 2;
  } else {
    for (const { family, path } of pack(resolve(out))) console.log(`${family}: ${path}`);
  }
}
