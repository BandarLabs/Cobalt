// The website is set in the faces the reader draws in, so a page about an app
// and the app's own screen beside it read as one thing. The runtime compiles
// those faces in as TrueType; a browser is better served by WOFF, which is the
// same tables compressed one by one and nothing else. This writes the copies
// the site serves from the fonts the runtime ships, so the two cannot drift.
//
// WOFF 1 rather than WOFF 2 on purpose: its container is a table directory and
// zlib, both of which Node has, so the encoding needs no dependency and can be
// checked by reading it. The faces are already subset, and the difference is a
// few kilobytes each.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = resolve(root, "crates/kobo-text/fonts");
const target = resolve(root, "docs/fonts");
const faces = [
  "Literata-Regular",
  "Literata-Italic",
  "Literata-SemiBold",
  "AtkinsonHyperlegible-Regular",
  "AtkinsonHyperlegible-Bold"
];
const licenses = ["LICENSE-Literata.txt", "LICENSE-AtkinsonHyperlegible.txt"];

const pad = length => (4 - (length % 4)) % 4;

export function woff(sfnt) {
  const flavor = sfnt.readUInt32BE(0);
  const count = sfnt.readUInt16BE(4);
  const tables = [];
  for (let index = 0; index < count; index += 1) {
    const entry = 12 + index * 16;
    const offset = sfnt.readUInt32BE(entry + 8);
    const length = sfnt.readUInt32BE(entry + 12);
    const bytes = sfnt.subarray(offset, offset + length);
    const packed = deflateSync(bytes, { level: 9 });
    tables.push({
      tag: sfnt.subarray(entry, entry + 4),
      checksum: sfnt.readUInt32BE(entry + 4),
      length,
      // A table that does not shrink is stored as it is, which the format
      // signals by the two lengths being equal.
      data: packed.length < length ? packed : bytes
    });
  }
  const header = 44;
  const directory = count * 20;
  let offset = header + directory;
  let sfntSize = 12 + count * 16;
  for (const table of tables) {
    table.offset = offset;
    offset += table.data.length + pad(table.data.length);
    sfntSize += table.length + pad(table.length);
  }
  const out = Buffer.alloc(offset);
  out.write("wOFF", 0, "latin1");
  out.writeUInt32BE(flavor, 4);
  out.writeUInt32BE(offset, 8);
  out.writeUInt16BE(count, 12);
  out.writeUInt32BE(sfntSize, 16);
  out.writeUInt16BE(1, 20);
  tables.forEach((table, index) => {
    const entry = header + index * 20;
    table.tag.copy(out, entry);
    out.writeUInt32BE(table.offset, entry + 4);
    out.writeUInt32BE(table.data.length, entry + 8);
    out.writeUInt32BE(table.length, entry + 12);
    out.writeUInt32BE(table.checksum, entry + 16);
    table.data.copy(out, table.offset);
  });
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  mkdirSync(target, { recursive: true });
  for (const face of faces) {
    const sfnt = readFileSync(resolve(source, `${face}.ttf`));
    const packed = woff(sfnt);
    writeFileSync(resolve(target, `${face}.woff`), packed);
    console.log(`${face}: ${sfnt.length} -> ${packed.length} bytes`);
  }
  for (const license of licenses) {
    writeFileSync(resolve(target, license), readFileSync(resolve(source, license)));
  }
}
