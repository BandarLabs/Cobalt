import test from "node:test";
import assert from "node:assert/strict";
import { collectRegistry, deriveMinimumCobalt, normalizeContribution } from "./app-registry.mjs";
import {
  contributionPlan,
  manifestForBinary,
  validatePublishedBaseline
} from "./app-contribute.mjs";

function manifest(overrides = {}) {
  return {
    id: "notes",
    display_name: "Notes",
    short_label: "Notes",
    summary: "A small public notes example.",
    version: "1.2.3",
    glyph: "note",
    capabilities: ["network"],
    ...overrides
  };
}

test("a concise contribution derives package and minimum Cobalt", () => {
  const app = normalizeContribution(manifest(), "notes");
  assert.equal(app.package, "kobo-notes");
  assert.equal(app.minimum_cobalt_version, "0.3.2");
  assert.equal(app.version, "1.2.3");
});

test("protocol and capability policy derive the strictest minimum", () => {
  const policy = {
    format_version: 1,
    protocols: { "11": "0.3.1" },
    capabilities: { "future-service": "0.4.2" }
  };
  assert.equal(deriveMinimumCobalt(["network", "future-service"], 11, policy), "0.4.2");
  assert.throws(
    () => deriveMinimumCobalt([], 12, policy),
    /protocol 12 has no Cobalt minimum/
  );
});

test("contributors cannot supply a package name", () => {
  assert.throws(
    () => normalizeContribution(manifest({ package: "other" }), "notes"),
    /package is derived/
  );
});

test("an explicit minimum Cobalt version can only raise the derived floor", () => {
  assert.equal(
    normalizeContribution(manifest({ minimum_cobalt_version: "0.4.0" }), "notes")
      .minimum_cobalt_version,
    "0.4.0"
  );
  assert.equal(
    normalizeContribution(manifest({ minimum_cobalt_version: "0.1.0" }), "notes")
      .minimum_cobalt_version,
    "0.3.1"
  );
});

test("directory identity and versions fail with actionable errors", () => {
  assert.throws(() => normalizeContribution(manifest(), "reader"), /must match its directory/);
  assert.throws(
    () => normalizeContribution(manifest({ version: "next" }), "notes"),
    /numeric MAJOR.MINOR.PATCH/
  );
  assert.throws(
    () =>
      normalizeContribution(
        manifest({ minimum_cobalt_version: "18446744073709551616.0.0" }),
        "notes"
      ),
    /unsigned 64-bit integer/
  );
  assert.throws(
    () =>
      normalizeContribution(
        manifest({ minimum_cobalt_version: `${"1".repeat(63)}.0.0` }),
        "notes"
      ),
    /numeric MAJOR.MINOR.PATCH/
  );
});

test("local release manifest binds the derived metadata and exact binary", () => {
  const app = normalizeContribution(manifest(), "notes");
  const release = manifestForBinary(app, Buffer.from("arm binary"));
  assert.equal(release.minimum_cobalt_version, "0.3.2");
  assert.equal(release.binary_bytes, 10);
  assert.match(release.binary_sha256, /^[0-9a-f]{64}$/);
});

test("the contributor baseline binds provenance to the exact catalog bytes", () => {
  const catalog = Buffer.from('{"format_version":1,"entries":[]}');
  const provenance = {
    format_version: 1,
    channel: "app-catalog-beta",
    source_sha: "1".repeat(40),
    catalog_sha256: "a262b0b9f57b924819fe73d16926f6e0895b50c66792a235d67870b8669f0bf7"
  };
  assert.doesNotThrow(() => validatePublishedBaseline(catalog, provenance));
  assert.throws(
    () => validatePublishedBaseline(Buffer.from("{}"), provenance),
    /do not form one verified baseline/
  );
});

test("the effective repository registry includes standalone contributions", () => {
  const registry = collectRegistry();
  assert.ok(registry.apps.length >= 4);
  for (const id of ["arxiv", "morse", "sudoku", "zotero-reader"]) {
    const app = registry.apps.find(candidate => candidate.id === id);
    assert.ok(app, `missing ${id}`);
    assert.equal(app.package, `kobo-${id}`);
    assert.equal(app.minimum_cobalt_version, "0.3.2");
  }
  assert.equal(
    registry.apps.find(candidate => candidate.id === "chat")?.minimum_cobalt_version,
    "0.3.4"
  );
});

test("the one-command plan resolves source manifest package and protocol", () => {
  const plan = contributionPlan("apps/sudoku/cobalt-app.json");
  assert.equal(plan.app.package, "kobo-sudoku");
  assert.equal(plan.protocolVersion, 12);
  assert.equal(plan.cargoPath, "apps/sudoku/Cargo.toml");
});
