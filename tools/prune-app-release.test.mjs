import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { assetsToPrune, packageNames } from "./prune-app-release.mjs";

const now = Date.parse("2026-11-28T00:00:00Z");
const daysAgo = days => new Date(now - days * 24 * 60 * 60 * 1000).toISOString();
const asset = (name, days, id = 1) => ({ id, name, created_at: daysAgo(days) });

test("only a package no catalog names, and two weeks old, is let go", () => {
  const assets = [
    asset("chess-aaa.cobalt-app", 40),
    asset("chess-bbb.cobalt-app", 40),
    asset("chess-ccc.cobalt-app", 3),
    asset("notes-ddd.cobalt-app", 90)
  ];
  const pruned = assetsToPrune(assets, ["chess-bbb.cobalt-app", "notes-ddd.cobalt-app"], now);
  assert.deepEqual(pruned.map(item => item.name), ["chess-aaa.cobalt-app"]);
});

test("catalogs, signatures and provenance are never let go", () => {
  const assets = [
    "cobalt-app-catalog.json",
    "cobalt-app-catalog.json.sig",
    "cobalt-app-catalog-v2.json",
    "cobalt-app-catalog-v2.json.sig",
    "cobalt-app-catalog-provenance.json",
    "cobalt-app-catalog-33902359628-1.json"
  ].map(name => asset(name, 400));
  assert.deepEqual(assetsToPrune(assets, [], now), []);
});

test("the newest ten transaction markers stay, and so does a young one", () => {
  const markers = Array.from({ length: 14 }, (_, index) =>
    asset(`cobalt-app-catalog-provenance-${1000 + index}-1.json`, 40 - index)
  );
  markers.push(asset("cobalt-app-catalog-provenance-999-1.json", 2));
  const pruned = assetsToPrune(markers, [], now).map(item => item.name);
  // Fifteen markers: the ten newest stay, and of the five older ones every
  // one is past the grace period.
  assert.deepEqual(pruned, [
    "cobalt-app-catalog-provenance-1004-1.json",
    "cobalt-app-catalog-provenance-1003-1.json",
    "cobalt-app-catalog-provenance-1002-1.json",
    "cobalt-app-catalog-provenance-1001-1.json",
    "cobalt-app-catalog-provenance-1000-1.json"
  ]);
});

test("a catalog's packages are named by the last part of their address", () => {
  assert.deepEqual(
    packageNames({
      entries: [
        { package_url: "https://github.com/o/r/releases/download/app-catalog-beta/chess-aaa.cobalt-app" }
      ]
    }),
    ["chess-aaa.cobalt-app"]
  );
  assert.throws(() => packageNames({ entries: [{}] }), /names no package/);
  assert.throws(() => packageNames({}), /entries array/);
});

test("the publish job prunes before it uploads, keeping both catalogs' packages", () => {
  const workflow = readFileSync(".github/workflows/apps.yml", "utf8");
  const prune = workflow.indexOf("node tools/prune-app-release.mjs");
  const upload = workflow.indexOf("for package in dist/apps/*.cobalt-app; do");
  assert.ok(prune > 0 && upload > prune, "pruning must come before the first upload");
  const call = workflow.slice(prune, workflow.indexOf("\n", workflow.indexOf("> ", prune)));
  for (const catalog of [
    "published-app-catalog.json",
    "dist/apps/cobalt-app-catalog-v2.json",
    "dist/apps/cobalt-app-catalog.json"
  ]) {
    assert.ok(call.includes(catalog), `${catalog} is not kept`);
  }
});
