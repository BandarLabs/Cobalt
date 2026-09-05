import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { collectRegistry, currentProtocolVersion, deriveMinimumCobalt, normalizeContribution } from "./app-registry.mjs";
import {
  contributionPlan,
  manifestForBinary,
  validatePublishedBaseline
} from "./app-contribute.mjs";

const CURRENT_MINIMUM = deriveMinimumCobalt([]);
const CURRENT_PROTOCOL = currentProtocolVersion();

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
  assert.equal(app.minimum_cobalt_version, CURRENT_MINIMUM);
  assert.equal(app.version, "1.2.3");
});

test("protocol and capability policy derive the strictest minimum", () => {
  const policy = {
    format_version: 1,
    protocols: { "11": "0.3.1", "12": "0.3.5" },
    capabilities: { "future-service": "0.4.2" }
  };
  assert.equal(deriveMinimumCobalt(["network", "future-service"], 11, policy), "0.4.2");
  assert.throws(
    () => deriveMinimumCobalt([], 13, policy),
    /protocol 13 has no Cobalt minimum/
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
    CURRENT_MINIMUM
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
  assert.equal(release.minimum_cobalt_version, CURRENT_MINIMUM);
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
    assert.equal(app.minimum_cobalt_version, CURRENT_MINIMUM);
  }
  assert.ok(
    registry.apps.find(candidate => candidate.id === "chat")?.minimum_cobalt_version >=
      CURRENT_MINIMUM
  );
});

test("the one-command plan resolves source manifest package and protocol", () => {
  const plan = contributionPlan("apps/sudoku/cobalt-app.json");
  assert.equal(plan.app.package, "kobo-sudoku");
  assert.equal(plan.protocolVersion, CURRENT_PROTOCOL);
  assert.equal(plan.cargoPath, "apps/sudoku/Cargo.toml");
});

// Every app moved to a manifest beside its source, which left apps/catalog.json
// an empty base. A check handed that file instead of the collected registry
// still exits zero, having validated nothing, so no workflow may name it.
test("no workflow checks the empty base catalog instead of the collected registry", () => {
  const base = JSON.parse(readFileSync("apps/catalog.json", "utf8"));
  const collected = collectRegistry();
  assert.ok(
    collected.apps.length > base.apps.length,
    "the collected registry must carry more than the base catalog"
  );

  // Not just `--registry`: the publish job read the file inline with
  // readFileSync, emitted no packages, copied nothing, and failed on an empty
  // upload. Any mention outside a comment is the same mistake.
  for (const name of readdirSync(".github/workflows")) {
    const workflow = readFileSync(`.github/workflows/${name}`, "utf8");
    for (const line of workflow.split("\n")) {
      if (line.trimStart().startsWith("#")) continue;
      assert.ok(
        !line.includes("apps/catalog.json"),
        `${name} reads the empty base catalog instead of the collected registry: ${line.trim()}`
      );
    }
  }
});
