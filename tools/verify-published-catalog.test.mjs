import test from "node:test";
import assert from "node:assert/strict";
import { generateKeyPairSync, sign } from "node:crypto";
import {
  catalogSignatureIsValid,
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
