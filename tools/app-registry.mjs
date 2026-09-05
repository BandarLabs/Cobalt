import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { validatedSetup } from "./app-page-setup.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MAX_VERSION_BYTES = 64;
const MAX_U64 = (1n << 64n) - 1n;
const APP_FIELDS = new Set([
  "id",
  "display_name",
  "short_label",
  "summary",
  "version",
  "release_notes",
  "minimum_cobalt_version",
  "glyph",
  "capabilities",
  "setup"
]);

function object(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value;
}

function versionParts(value, label) {
  if (
    typeof value !== "string" ||
    value.length > MAX_VERSION_BYTES ||
    !/^\d+\.\d+\.\d+$/.test(value)
  ) {
    throw new Error(`${label} must be a numeric MAJOR.MINOR.PATCH version`);
  }
  return value.split(".").map(part => {
    const number = BigInt(part);
    if (number > MAX_U64) {
      throw new Error(`${label} components must fit in an unsigned 64-bit integer`);
    }
    return number;
  });
}

function laterVersion(left, right) {
  const a = versionParts(left, "derived minimum");
  const b = versionParts(right, "capability minimum");
  for (let index = 0; index < a.length; index += 1) {
    if (a[index] !== b[index]) return a[index] > b[index] ? left : right;
  }
  return left;
}

export function currentProtocolVersion(source = readFileSync(
  join(root, "crates/kobo-protocol/src/lib.rs"),
  "utf8"
)) {
  const match = /pub const VERSION: u8 = (\d+);/.exec(source);
  if (!match) throw new Error("could not read kobo-protocol VERSION");
  return Number(match[1]);
}

export function compatibilityPolicy(path = join(root, "tools/protocol-minimums.json")) {
  const policy = object(JSON.parse(readFileSync(path, "utf8")), "compatibility policy");
  if (policy.format_version !== 1) {
    throw new Error("compatibility policy format_version must be 1");
  }
  object(policy.protocols, "compatibility protocol map");
  object(policy.capabilities, "compatibility capability map");
  return policy;
}

export function deriveMinimumCobalt(
  capabilities,
  protocol = currentProtocolVersion(),
  policy = compatibilityPolicy()
) {
  if (!Array.isArray(capabilities) || capabilities.some(value => typeof value !== "string")) {
    throw new Error("capabilities must be an array of strings");
  }
  let minimum = policy.protocols[String(protocol)];
  if (!minimum) {
    throw new Error(
      `protocol ${protocol} has no Cobalt minimum; add it to tools/protocol-minimums.json`
    );
  }
  for (const capability of capabilities) {
    const capabilityMinimum = policy.capabilities[capability];
    if (capabilityMinimum !== undefined) {
      minimum = laterVersion(minimum, capabilityMinimum);
    }
  }
  return minimum;
}

export function normalizeContribution(value, directoryName) {
  const app = object(value, `app manifest ${directoryName}`);
  for (const field of Object.keys(app)) {
    if (!APP_FIELDS.has(field)) {
      throw new Error(
        `unknown field '${field}' in ${directoryName}/cobalt-app.json; package is derived`
      );
    }
  }
  const required = [
    "id",
    "display_name",
    "short_label",
    "summary",
    "version",
    "glyph",
    "capabilities"
  ];
  for (const field of required) {
    if (app[field] === undefined) {
      throw new Error(`${directoryName}/cobalt-app.json is missing '${field}'`);
    }
  }
  if (app.id !== directoryName) {
    throw new Error(
      `${directoryName}/cobalt-app.json id '${app.id}' must match its directory name`
    );
  }
  if (!/^[a-z0-9][a-z0-9-]*$/.test(app.id)) {
    throw new Error(`${app.id}: id must contain only lowercase letters, digits, and hyphens`);
  }
  versionParts(app.version, `${app.id} version`);
  if (
    app.release_notes !== undefined &&
    (typeof app.release_notes !== "string" ||
      app.release_notes.trim().length < 12 ||
      app.release_notes.length > 240)
  ) {
    throw new Error(`${app.id} release_notes must be 12 to 240 meaningful characters`);
  }
  validatedSetup(app);
  const derivedMinimum = deriveMinimumCobalt(app.capabilities);
  const minimum_cobalt_version =
    app.minimum_cobalt_version === undefined
      ? derivedMinimum
      : laterVersion(derivedMinimum, app.minimum_cobalt_version);
  return {
    package: `kobo-${app.id}`,
    id: app.id,
    display_name: app.display_name,
    short_label: app.short_label,
    summary: app.summary,
    version: app.version,
    ...(app.release_notes === undefined ? {} : { release_notes: app.release_notes }),
    minimum_cobalt_version,
    glyph: app.glyph,
    capabilities: app.capabilities,
    ...(app.setup === undefined ? {} : { setup: app.setup })
  };
}

export function collectRegistry({
  basePath = join(root, "apps/catalog.json"),
  sourcePaths = [join(root, "apps"), join(root, "examples")]
} = {}) {
  const base = object(JSON.parse(readFileSync(basePath, "utf8")), "base app registry");
  if (base.format_version !== 1 || !Array.isArray(base.apps)) {
    throw new Error("base app registry must contain format_version 1 and an apps array");
  }
  const apps = [...base.apps];
  for (const sourcePath of sourcePaths) {
    for (const entry of readdirSync(sourcePath, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const manifestPath = join(sourcePath, entry.name, "cobalt-app.json");
      let source;
      try {
        source = readFileSync(manifestPath, "utf8");
      } catch (error) {
        if (error.code === "ENOENT") continue;
        throw error;
      }
      const contribution = normalizeContribution(JSON.parse(source), entry.name);
      const existing = apps.find(
        app => app.id === contribution.id || app.package === contribution.package
      );
      if (existing) {
        if (JSON.stringify(existing) !== JSON.stringify(contribution)) {
          throw new Error(`duplicate app id '${contribution.id}'`);
        }
        continue;
      }
      apps.push(contribution);
    }
  }
  const ids = new Set();
  const packages = new Set();
  for (const app of apps) {
    if (ids.has(app.id)) throw new Error(`duplicate app id '${app.id}'`);
    if (packages.has(app.package)) throw new Error(`duplicate app package '${app.package}'`);
    ids.add(app.id);
    packages.add(app.package);
  }
  apps.sort((left, right) => left.id.localeCompare(right.id));
  return { format_version: 1, apps };
}
