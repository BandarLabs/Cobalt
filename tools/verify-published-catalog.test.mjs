import test from "node:test";
import assert from "node:assert/strict";
import { generateKeyPairSync, sign } from "node:crypto";
import { readFileSync } from "node:fs";
import {
  CATALOG_FILE,
  EARLIER_CATALOG_FILE,
  EARLIER_GLYPHS,
  catalogSignatureIsValid,
  earlierCatalogFailures,
  publishedCatalogFailures
} from "./verify-published-catalog.mjs";

function signer() {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const raw = publicKey.export({ format: "der", type: "spki" }).subarray(12);
  return {
    publicKeyHex: raw.toString("hex"),
    sign: bytes => `${sign(null, bytes, privateKey).toString("hex")}\n`
  };
}

test("only the exact bytes the key signed verify", () => {
  const key = signer();
  const catalog = Buffer.from('{"format_version":1,"entries":[]}\n');
  const signature = key.sign(catalog);

  assert.equal(catalogSignatureIsValid(catalog, signature, key.publicKeyHex), true);
  assert.equal(
    catalogSignatureIsValid(Buffer.from('{"format_version":1,"entries":[ ]}\n'), signature, key.publicKeyHex),
    false
  );
  assert.equal(
    catalogSignatureIsValid(catalog, key.sign(Buffer.from("something else")), key.publicKeyHex),
    false
  );
});

test("a malformed signature or key is never treated as valid", () => {
  const key = signer();
  const catalog = Buffer.from("catalog");
  assert.equal(catalogSignatureIsValid(catalog, "", key.publicKeyHex), false);
  assert.equal(catalogSignatureIsValid(catalog, "not hex", key.publicKeyHex), false);
  assert.equal(catalogSignatureIsValid(catalog, "ab".repeat(63), key.publicKeyHex), false);
  assert.equal(catalogSignatureIsValid(catalog, key.sign(catalog), "short"), false);
});

function published({ version = "1.0.0", assetPresent = true } = {}) {
  const sha = "a".repeat(64);
  const registry = {
    format_version: 1,
    apps: [
      {
        package: "kobo-notes",
        id: "notes",
        display_name: "Notes",
        short_label: "Notes",
        summary: "Summary",
        version: "1.0.0",
        minimum_cobalt_version: "0.3.1",
        glyph: "note",
        capabilities: ["network"]
      }
    ]
  };
  const catalog = {
    format_version: 1,
    entries: [
      {
        manifest: {
          format_version: 1,
          id: "notes",
          display_name: "Notes",
          short_label: "Notes",
          summary: "Summary",
          version,
          minimum_cobalt_version: "0.3.1",
          glyph: "note",
          capabilities: ["network"],
          binary_sha256: "0".repeat(64),
          binary_bytes: 3
        },
        package_url: `https://example.test/download/app-catalog-beta/notes-${sha}.cobalt-app`,
        package_sha256: sha,
        package_bytes: 4
      }
    ]
  };
  return {
    registry,
    catalog,
    assets: assetPresent ? [`notes-${sha}.cobalt-app`, "cobalt-app-catalog.json"] : ["cobalt-app-catalog.json"]
  };
}

test("a channel that matches this tree and carries its bundles passes", () => {
  const values = published();
  assert.deepEqual(
    publishedCatalogFailures(values.catalog, values.registry, values.assets),
    []
  );
});

test("a catalog offering a bundle the release does not carry fails", () => {
  const values = published({ assetPresent: false });
  assert.deepEqual(publishedCatalogFailures(values.catalog, values.registry, values.assets), [
    `notes: published catalog offers notes-${"a".repeat(64)}.cobalt-app, which the release does not carry`
  ]);
});

test("a published version that is not the registered one fails", () => {
  const values = published({ version: "0.9.0" });
  assert.deepEqual(publishedCatalogFailures(values.catalog, values.registry, values.assets), [
    'notes: published version "0.9.0" is not the registered "1.0.0"'
  ]);
});

test("apps missing from or left behind in the channel are both reported", () => {
  const values = published();
  values.registry.apps.push({ ...values.registry.apps[0], id: "reader", package: "kobo-reader" });
  assert.deepEqual(publishedCatalogFailures(values.catalog, values.registry, values.assets), [
    "reader: registered here but absent from the published catalog"
  ]);

  const removed = published();
  removed.registry.apps = [];
  assert.deepEqual(publishedCatalogFailures(removed.catalog, removed.registry, removed.assets), [
    "notes: published to readers but no longer registered here"
  ]);
});

test("capability order is not a difference but capability content is", () => {
  const values = published();
  values.registry.apps[0].capabilities = ["audio", "network"];
  values.catalog.entries[0].manifest.capabilities = ["network", "audio"];
  assert.deepEqual(publishedCatalogFailures(values.catalog, values.registry, values.assets), []);

  values.catalog.entries[0].manifest.capabilities = ["network"];
  assert.deepEqual(publishedCatalogFailures(values.catalog, values.registry, values.assets), [
    "notes: published capabilities are not the registered ones"
  ]);
});

function marked(id, glyph, minimum) {
  return {
    manifest: {
      format_version: 1,
      id,
      display_name: id,
      short_label: id,
      summary: `The ${id} application.`,
      version: "1.0.0",
      minimum_cobalt_version: minimum,
      glyph,
      capabilities: [],
      binary_sha256: "a".repeat(64),
      binary_bytes: 1
    },
    package_url: `https://example.test/${id}.cobalt-app`,
    package_sha256: "b".repeat(64),
    package_bytes: 2
  };
}

const offeredAs = (entry, glyph) => ({ ...entry, manifest: { ...entry.manifest, glyph } });

test("the earlier catalog is the catalog with stand-in glyphs on entries older runtimes cannot install", () => {
  const brief = marked("brief", "clock", "0.1.9");
  const backgammon = marked("backgammon", "dice", "0.3.34");
  const catalog = { format_version: 1, entries: [brief, backgammon] };
  const earlier = { format_version: 1, entries: [brief, offeredAs(backgammon, "circle")] };
  assert.deepEqual(earlierCatalogFailures(earlier, catalog), []);

  // A glyph 0.3.33 refuses, left in the file it reads, costs it the Store.
  const leaked = { format_version: 1, entries: [brief, backgammon] };
  assert.match(earlierCatalogFailures(leaked, catalog)[0], /backgammon: offered .* "dice"/);

  // Anything but the glyph differing is a different catalog.
  const moved = {
    format_version: 1,
    entries: [brief, { ...offeredAs(backgammon, "circle"), package_bytes: 3 }]
  };
  assert.match(earlierCatalogFailures(moved, catalog)[0], /more than its glyph/);

  // An older runtime would install this one and then refuse the bundle.
  const early = marked("backgammon", "dice", "0.3.33");
  assert.match(
    earlierCatalogFailures(
      { format_version: 1, entries: [brief, offeredAs(early, "circle")] },
      { format_version: 1, entries: [brief, early] }
    )[0],
    /would still install it/
  );
  assert.match(
    earlierCatalogFailures({ format_version: 1, entries: [brief] }, catalog)[0],
    /1 entries and the catalog 2/
  );
});

test("the tools name the catalogs and the glyphs 0.3.33 draws as the Rust publisher does", () => {
  const library = readFileSync(new URL("../crates/kobo-app-store/src/lib.rs", import.meta.url), "utf8");
  assert.match(library, new RegExp(`CATALOG_FILE: &str = "${CATALOG_FILE}"`));
  assert.match(library, new RegExp(`EARLIER_CATALOG_FILE: &str = "${EARLIER_CATALOG_FILE}"`));
  const model = readFileSync(new URL("../crates/kobo-app-store/src/model.rs", import.meta.url), "utf8");
  const body = model.slice(model.indexOf("pub fn is_earlier_glyph"));
  const list = body.slice(0, body.indexOf("\n}\n"));
  const names = [...list.matchAll(/"([a-z0-9-]+)"/g)].map(match => match[1]);
  assert.deepEqual([...names].sort(), [...EARLIER_GLYPHS].sort());
});
