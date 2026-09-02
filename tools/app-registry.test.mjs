import test from "node:test";
import assert from "node:assert/strict";
import { collectRegistry, deriveMinimumCobalt, normalizeContribution } from "./app-registry.mjs";
import { contributionPlan, manifestForBinary } from "./app-contribute.mjs";

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
  assert.equal(app.minimum_cobalt_version, "0.3.1");
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

test("contributors cannot supply derived release plumbing", () => {
  assert.throws(
    () => normalizeContribution(manifest({ package: "other" }), "notes"),
    /package and minimum Cobalt are derived/
  );
  assert.throws(
    () => normalizeContribution(manifest({ minimum_cobalt_version: "0.1.0" }), "notes"),
    /package and minimum Cobalt are derived/
  );
});

test("directory identity and versions fail with actionable errors", () => {
  assert.throws(() => normalizeContribution(manifest(), "reader"), /must match its directory/);
  assert.throws(
    () => normalizeContribution(manifest({ version: "next" }), "notes"),
    /numeric MAJOR.MINOR.PATCH/
  );
});

test("local release manifest binds the derived metadata and exact binary", () => {
  const app = normalizeContribution(manifest(), "notes");
  const release = manifestForBinary(app, Buffer.from("arm binary"));
  assert.equal(release.minimum_cobalt_version, "0.3.1");
  assert.equal(release.binary_bytes, 10);
  assert.match(release.binary_sha256, /^[0-9a-f]{64}$/);
});

test("the effective repository registry includes standalone contributions", () => {
  const registry = collectRegistry();
  assert.ok(registry.apps.length >= 4);
  for (const id of ["arxiv", "morse", "sudoku", "zotero-reader"]) {
    const app = registry.apps.find(candidate => candidate.id === id);
    assert.ok(app, `missing ${id}`);
    assert.equal(app.package, `kobo-${id}`);
    assert.equal(app.minimum_cobalt_version, "0.3.1");
  }
});

test("the one-command plan resolves source manifest package and protocol", () => {
  const plan = contributionPlan("apps/sudoku/cobalt-app.json");
  assert.equal(plan.app.package, "kobo-sudoku");
  assert.equal(plan.protocolVersion, 11);
  assert.equal(plan.cargoPath, "apps/sudoku/Cargo.toml");
});
