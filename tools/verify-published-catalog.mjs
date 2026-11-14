import { verify as verifySignature } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The same key every reader carries, kobo_app_store::PUBLIC_RELEASE_KEY_HEX.
export const PUBLIC_RELEASE_KEY_HEX =
  "bed7511de9fadbcf81fb4efe445b8a073c81a8333f64410c6ded588bbfd4a5de";

const SPKI_ED25519_PREFIX = "302a300506032b6570032100";

const PUBLIC_MANIFEST_FIELDS = [
  "display_name",
  "short_label",
  "summary",
  "version",
  "minimum_cobalt_version",
  "glyph"
];

// Each channel carries two signed catalogs. The one readers from 0.3.34 fetch,
// kobo_app_store::CATALOG_FILE, names every application's own mark. The one
// runtimes up to 0.3.33 fetch, kobo_app_store::EARLIER_CATALOG_FILE, is the
// same catalog with each glyph replaced by one those runtimes draw, because
// they refuse a whole catalog over a glyph name they do not know.
export const CATALOG_FILE = "cobalt-app-catalog-v2.json";
export const EARLIER_CATALOG_FILE = "cobalt-app-catalog.json";
export const FIRST_MARKED_RUNTIME = "0.3.34";

// The glyph names 0.3.33 accepted, kobo_app_store::is_earlier_glyph. Frozen:
// it is what those runtimes shipped with.
export const EARLIER_GLYPHS = Object.freeze([
  "app", "book", "note", "clock", "settings", "folder", "chart", "search", "wifi",
  "battery", "reader", "power", "grid", "circle", "check", "terminal", "chat", "news",
  "rss", "light", "close", "download", "bookmark", "filter", "person", "tag", "globe",
  "refresh", "more", "bluetooth", "key", "magnet", "play", "pause", "rewind30",
  "forward30", "volume-down", "volume-up", "more-vertical", "trash", "previous", "next",
  "plus", "headphones"
]);

function cobaltVersionAtLeast(current, minimum) {
  const parse = value =>
    /^\d+\.\d+\.\d+$/.test(String(value)) ? String(value).split(".").map(Number) : null;
  const left = parse(current);
  const right = parse(minimum);
  if (!left || !right) return false;
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] > right[index];
  }
  return true;
}

// The earlier catalog is correct when it is the catalog, entry for entry,
// differing only in glyphs those runtimes draw, and only on entries they will
// not install: they check a bundle against its entry byte for byte, so a
// changed entry they would install is an application they can never get.
// Which stand-in each mark takes is the Rust publisher's to decide and its
// tests' to check; this is the part a published channel can get wrong.
export function earlierCatalogFailures(earlier, catalog) {
  if (earlier?.format_version !== 1 || !Array.isArray(earlier.entries)) {
    return ["the earlier catalog is not a format_version 1 catalog"];
  }
  if (catalog?.format_version !== 1 || !Array.isArray(catalog.entries)) {
    return ["the catalog is not a format_version 1 catalog"];
  }
  if (earlier.entries.length !== catalog.entries.length) {
    return [
      `the earlier catalog has ${earlier.entries.length} entries and the catalog ${catalog.entries.length}`
    ];
  }
  const failures = [];
  catalog.entries.forEach((entry, index) => {
    const offered = earlier.entries[index];
    const id = entry?.manifest?.id;
    const glyph = offered?.manifest?.glyph;
    if (!EARLIER_GLYPHS.includes(glyph)) {
      failures.push(`${id}: offered to runtimes up to 0.3.33 as ${JSON.stringify(glyph)}, which they refuse`);
    }
    const same = JSON.stringify({ ...offered, manifest: { ...offered?.manifest, glyph: entry?.manifest?.glyph } });
    if (same !== JSON.stringify(entry)) {
      failures.push(`${id}: differs between the two catalogs in more than its glyph`);
      return;
    }
    if (
      glyph !== entry.manifest.glyph &&
      !cobaltVersionAtLeast(entry.manifest.minimum_cobalt_version, FIRST_MARKED_RUNTIME)
    ) {
      failures.push(
        `${id}: wears a stand-in glyph for runtimes that would still install it, which they then refuse`
      );
    }
  });
  return failures;
}

export function catalogSignatureIsValid(
  catalogBytes,
  signatureText,
  publicKeyHex = PUBLIC_RELEASE_KEY_HEX
) {
  if (!/^[0-9a-f]{64}$/.test(publicKeyHex)) return false;
  const trimmed = String(signatureText).trim();
  if (!/^[0-9a-f]{128}$/.test(trimmed)) return false;
  return verifySignature(
    null,
    catalogBytes,
    {
      key: Buffer.from(`${SPKI_ED25519_PREFIX}${publicKeyHex}`, "hex"),
      format: "der",
      type: "spki"
    },
    Buffer.from(trimmed, "hex")
  );
}

// A published channel is correct when it offers exactly the apps this tree
// registers, at the manifests this tree describes, and every bundle it points a
// reader at is actually on the release.
export function publishedCatalogFailures(catalog, registry, assetNames) {
  const failures = [];
  if (catalog?.format_version !== 1 || !Array.isArray(catalog.entries)) {
    return ["published catalog is not a format_version 1 catalog"];
  }
  if (!Array.isArray(registry?.apps)) return ["app registry has no app array"];

  const assets = new Set(assetNames);
  const entriesById = new Map();
  for (const entry of catalog.entries) {
    const id = entry?.manifest?.id;
    if (typeof id !== "string") return ["published catalog entry has no identity"];
    if (entriesById.has(id)) failures.push(`${id}: published twice in one catalog`);
    entriesById.set(id, entry);
  }

  for (const app of registry.apps) {
    const entry = entriesById.get(app.id);
    if (!entry) {
      failures.push(`${app.id}: registered here but absent from the published catalog`);
      continue;
    }
    entriesById.delete(app.id);
    for (const field of PUBLIC_MANIFEST_FIELDS) {
      if (entry.manifest[field] !== app[field]) {
        failures.push(
          `${app.id}: published ${field} ${JSON.stringify(entry.manifest[field])} is not the registered ${JSON.stringify(app[field])}`
        );
      }
    }
    const published = [...(entry.manifest.capabilities || [])].sort();
    const registered = [...(app.capabilities || [])].sort();
    if (JSON.stringify(published) !== JSON.stringify(registered)) {
      failures.push(`${app.id}: published capabilities are not the registered ones`);
    }
    if (!/^[0-9a-f]{64}$/.test(entry.package_sha256 || "")) {
      failures.push(`${app.id}: published entry has no package checksum`);
      continue;
    }
    const asset = `${app.id}-${entry.package_sha256}.cobalt-app`;
    if (!entry.package_url?.endsWith(`/${asset}`)) {
      failures.push(`${app.id}: published package URL does not name ${asset}`);
    }
    if (!assets.has(asset)) {
      failures.push(`${app.id}: published catalog offers ${asset}, which the release does not carry`);
    }
  }

  for (const id of entriesById.keys()) {
    failures.push(`${id}: published to readers but no longer registered here`);
  }
  return failures;
}

function argumentsFrom(argv) {
  const required = ["--catalog", "--signature", "--registry", "--release"];
  const earlier = ["--earlier-catalog", "--earlier-signature"];
  const usage =
    "usage: node tools/verify-published-catalog.mjs --catalog PATH --signature PATH --registry PATH --release PATH [--earlier-catalog PATH --earlier-signature PATH]";
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (![...required, ...earlier].includes(flag) || !value || values.has(flag)) {
      throw new Error(usage);
    }
    values.set(flag, value);
  }
  if (!required.every(flag => values.has(flag))) throw new Error(usage);
  if (values.has(earlier[0]) !== values.has(earlier[1])) throw new Error(usage);
  return values;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const values = argumentsFrom(process.argv.slice(2));
    const catalogBytes = readFileSync(values.get("--catalog"));
    if (
      !catalogSignatureIsValid(catalogBytes, readFileSync(values.get("--signature"), "utf8"))
    ) {
      throw new Error(
        "the published catalog signature does not cover the published catalog; readers are refusing this channel"
      );
    }
    const release = JSON.parse(readFileSync(values.get("--release"), "utf8"));
    const failures = publishedCatalogFailures(
      JSON.parse(catalogBytes.toString("utf8")),
      JSON.parse(readFileSync(values.get("--registry"), "utf8")),
      (release.assets || []).map(asset => asset.name)
    );
    if (failures.length > 0) {
      throw new Error(
        `the published catalog is not what this tree describes:\n${failures.map(failure => `  ${failure}`).join("\n")}`
      );
    }
    if (values.has("--earlier-catalog")) {
      const earlierBytes = readFileSync(values.get("--earlier-catalog"));
      if (
        !catalogSignatureIsValid(
          earlierBytes,
          readFileSync(values.get("--earlier-signature"), "utf8")
        )
      ) {
        throw new Error(
          "the earlier catalog signature does not cover the earlier catalog; runtimes up to 0.3.33 are refusing this channel"
        );
      }
      const earlierFailures = earlierCatalogFailures(
        JSON.parse(earlierBytes.toString("utf8")),
        JSON.parse(catalogBytes.toString("utf8"))
      );
      if (earlierFailures.length > 0) {
        throw new Error(
          `the earlier catalog is not the catalog for runtimes up to 0.3.33:\n${earlierFailures.map(failure => `  ${failure}`).join("\n")}`
        );
      }
    }
    console.log(
      `The published catalog is signed, complete and matches this tree: ${JSON.parse(catalogBytes.toString("utf8")).entries.length} apps.`
    );
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
