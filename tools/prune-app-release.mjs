import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// What a catalog release no longer needs, so the next publication fits.
//
// A GitHub release holds at most 1000 files, and the beta app channel never
// let go of one: every package a publication replaced stayed beside the new
// one, and every run left a transaction marker. It reached 1000 on the run
// that published thirteen app fixes, and the upload of the fourteenth file
// was refused, so nothing could be published at all.
//
// A package is let go only when neither the catalog readers have now nor
// the catalog about to replace it names it, and only once it has been on
// the release for two weeks. Those two weeks are for a stable promotion of a
// catalog that has since been replaced, and for a reader still holding one,
// both of which download a package by the name that catalog gave it.
// Catalogs, signatures and provenance are never touched. Transaction markers
// are only needed for the most recent publications, so the newest ten stay.

export const GRACE_DAYS = 14;
export const KEPT_TRANSACTIONS = 10;
const DAY_MS = 24 * 60 * 60 * 1000;
const TRANSACTION = /^cobalt-app-catalog-provenance-\d+-\d+\.json$/;

export function packageNames(catalog) {
  if (!catalog || !Array.isArray(catalog.entries)) {
    throw new Error("a catalog to keep must have an entries array");
  }
  return catalog.entries.map(entry => {
    const url = entry?.package_url;
    if (typeof url !== "string" || !url.includes("/")) {
      throw new Error("a catalog entry names no package");
    }
    return decodeURIComponent(url.slice(url.lastIndexOf("/") + 1));
  });
}

export function assetsToPrune(
  assets,
  keep,
  now,
  graceDays = GRACE_DAYS,
  keptTransactions = KEPT_TRANSACTIONS
) {
  if (!Array.isArray(assets)) throw new Error("release assets must be an array");
  const kept = new Set(keep);
  const age = asset => now - Date.parse(asset.created_at);
  const old = asset => Number.isFinite(age(asset)) && age(asset) >= graceDays * DAY_MS;
  const packages = assets.filter(
    asset => asset.name.endsWith(".cobalt-app") && !kept.has(asset.name) && old(asset)
  );
  const transactions = assets
    .filter(asset => TRANSACTION.test(asset.name))
    .sort((left, right) => Date.parse(right.created_at) - Date.parse(left.created_at))
    .slice(keptTransactions)
    .filter(old);
  return [...packages, ...transactions];
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  // node tools/prune-app-release.mjs RELEASE_JSON CATALOG... prints the id
  // and name of each asset to delete, one per line.
  const [releasePath, ...catalogs] = process.argv.slice(2);
  if (!releasePath || catalogs.length === 0) {
    console.error("usage: node tools/prune-app-release.mjs RELEASE_JSON CATALOG [CATALOG ...]");
    process.exit(2);
  }
  const release = JSON.parse(readFileSync(releasePath, "utf8"));
  const keep = catalogs.flatMap(path => packageNames(JSON.parse(readFileSync(path, "utf8"))));
  for (const asset of assetsToPrune(release.assets, keep, Date.now())) {
    if (!Number.isSafeInteger(asset.id)) throw new Error(`asset ${asset.name} has no id`);
    console.log(`${asset.id}\t${asset.name}`);
  }
}
