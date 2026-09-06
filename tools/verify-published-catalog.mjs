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
  const allowed = ["--catalog", "--signature", "--registry", "--release"];
  const usage =
    "usage: node tools/verify-published-catalog.mjs --catalog PATH --signature PATH --registry PATH --release PATH";
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowed.includes(flag) || !value) throw new Error(usage);
    values.set(flag, value);
  }
  if (values.size !== allowed.length) throw new Error(usage);
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
    console.log(
      `The published catalog is signed, complete and matches this tree: ${JSON.parse(catalogBytes.toString("utf8")).entries.length} apps.`
    );
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
