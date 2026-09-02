import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const apps = readFileSync(".github/workflows/apps.yml", "utf8");
const promotion = readFileSync(".github/workflows/promote-beta-apps.yml", "utf8");
const ci = readFileSync(".github/workflows/ci.yml", "utf8");

test("pull request and build automation use generated contributor manifests", () => {
  assert.match(ci, /collect-app-registry\.mjs --out generated-app-registry\.json/);
  assert.match(apps, /app-list --registry generated-app-registry\.json/);
  assert.doesNotMatch(apps, /--registry apps\/catalog\.json/);
  assert.match(ci, /else\n            target_branch=beta/);
});

test("only the final Beta publication job can write repository contents", () => {
  assert.match(apps, /permissions:\n  contents: read\n  actions: read/);
  assert.match(
    apps,
    /publish:[\s\S]*?permissions:\n      contents: write\n      actions: read/
  );
  const buildJob = apps.slice(apps.indexOf("  build-apps:"), apps.indexOf("  publish:"));
  assert.doesNotMatch(buildJob, /contents: write/);
});

test("Stable promotion is environment-approved without manual crypto inputs", () => {
  assert.match(promotion, /environment: app-store-stable/);
  assert.match(promotion, /tested_commit:/);
  assert.doesNotMatch(promotion, /expected_catalog_sha256:/);
  assert.doesNotMatch(promotion, /confirmation:/);
  assert.match(promotion, /permissions:\n  contents: read\n  actions: read/);
  assert.match(promotion, /promote:[\s\S]*?permissions:\n      contents: write/);
  assert.match(promotion, /digests\.size !== 1/);
});

test("Beta and Stable retain immutable transaction assets for rollback", () => {
  for (const workflow of [apps, promotion]) {
    assert.match(workflow, /cobalt-app-catalog-\$\{GITHUB_RUN_ID\}-\$\{GITHUB_RUN_ATTEMPT\}\.json/);
    assert.match(
      workflow,
      /cobalt-app-catalog-provenance-\$\{GITHUB_RUN_ID\}-\$\{GITHUB_RUN_ATTEMPT\}\.json/
    );
  }
  assert.match(promotion, /Download the exact beta app packages/);
  assert.match(promotion, /cmp "\$package"/);
  assert.match(apps, /archive_name="cobalt-app-catalog-\$\{archive_run\}-\$\{archive_attempt\}\.json"/);
  assert.match(apps, /api_download_asset published-app-catalog\.json "\$archive_id"/);
  assert.match(apps, /archive_signature="\$\{archive_catalog\}\.sig"/);
  assert.match(
    apps,
    /archive_provenance="cobalt-app-catalog-provenance-\$\{archive_run\}-\$\{archive_attempt\}\.json"/
  );
});
