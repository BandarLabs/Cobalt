import test from "node:test";
import assert from "node:assert/strict";
import { checkEntries } from "./check-app-versions.mjs";

function fixture({ currentVersion = "1.0.0", summary = "Summary" } = {}) {
  const app = {
    package: "kobo-notes",
    id: "notes",
    display_name: "Notes",
    short_label: "Notes",
    summary,
    version: currentVersion,
    minimum_cobalt_version: "0.3.0",
    glyph: "note",
    capabilities: ["network"]
  };
  const previous = {
    format_version: 1,
    id: "notes",
    display_name: "Notes",
    short_label: "Notes",
    summary: "Summary",
    version: "1.0.0",
    minimum_cobalt_version: "0.3.0",
    glyph: "note",
    capabilities: ["network"],
    binary_sha256: "0".repeat(64),
    binary_bytes: 3
  };
  return {
    registry: { format_version: 1, apps: [app] },
    published: { format_version: 1, entries: [{ manifest: previous }] }
  };
}

test("accepts an unchanged app at the published version", () => {
  const values = fixture();
  assert.doesNotThrow(() => checkEntries(values.registry, values.published, new Set()));
});

test("requires a version bump when code or a dependency changes", () => {
  const values = fixture();
  assert.throws(
    () => checkEntries(values.registry, values.published, new Set(["kobo-notes"])),
    /package inputs changed \(release inputs\).*version remains 1\.0\.0/s
  );
});

test("requires a version bump when public metadata changes", () => {
  const values = fixture({ summary: "New summary" });
  assert.throws(
    () => checkEntries(values.registry, values.published, new Set()),
    /package inputs changed \(summary\).*version remains 1\.0\.0/s
  );
});

test("accepts changed content with a new version", () => {
  const values = fixture({ currentVersion: "1.0.1", summary: "New summary" });
  assert.doesNotThrow(() =>
    checkEntries(values.registry, values.published, new Set(["kobo-notes"]))
  );
});
