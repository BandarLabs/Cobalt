import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { compatibilityPolicy, deriveMinimumCobalt } from "./app-registry.mjs";
import {
  isFilmingScript,
  isDocumentation,
  storeCatalogChanges,
  storeWatchDirectories,
  unpublishedStoreChangeReport
} from "./store-catalog-changes.mjs";

const MANIFEST_FIELDS = [
  "id",
  "display_name",
  "short_label",
  "summary",
  "minimum_cobalt_version",
  "glyph"
];

// Cargo metadata grows with every workspace package. Bound it well above the
// current workspace while still refusing an unexpectedly large child output.
export const COMMAND_MAX_BUFFER = 8 * 1024 * 1024;

// Store packages are built from the current SDK and therefore speak its exact
// wire protocol. A new protocol must add its first compatible Cobalt release
// here before the catalog can be published.
const PROTOCOL_MINIMUMS = new Map(
  Object.entries(compatibilityPolicy().protocols).map(([protocol, version]) => [
    Number(protocol),
    version
  ])
);

function readJson(path, label) {
  try {
    const value = JSON.parse(readFileSync(path, "utf8"));
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error("must be a JSON object");
    }
    return value;
  } catch (error) {
    throw new Error(`read ${label} ${path}: ${error.message}`);
  }
}

function normalizedCapabilities(value, label) {
  if (!Array.isArray(value) || value.some(capability => typeof capability !== "string")) {
    throw new Error(`${label} capabilities must be an array of strings`);
  }
  return [...value].sort();
}

function changedManifestFields(app, previous) {
  const changed = [];
  for (const field of [
    ...MANIFEST_FIELDS.filter(field => field !== "minimum_cobalt_version"),
    "version"
  ]) {
    if (app[field] !== previous[field]) changed.push(field);
  }
  if (
    app.minimum_cobalt_version !== previous.minimum_cobalt_version &&
    app.minimum_cobalt_version !== deriveMinimumCobalt(app.capabilities)
  ) {
    changed.push("minimum_cobalt_version");
  }
  const currentCapabilities = normalizedCapabilities(app.capabilities, app.id);
  const previousCapabilities = normalizedCapabilities(previous.capabilities, app.id);
  if (JSON.stringify(currentCapabilities) !== JSON.stringify(previousCapabilities)) {
    changed.push("capabilities");
  }
  return changed;
}

function numericVersion(value) {
  if (typeof value !== "string") return null;
  const parts = value.split(".");
  if (parts.some(part => part.length === 0 || !/^\d+$/.test(part))) return null;
  const numbers = parts.map(BigInt);
  return numbers.some(part => part > 18446744073709551615n) ? null : numbers;
}

function numericVersionIsNewer(candidate, installed) {
  const left = numericVersion(candidate);
  const right = numericVersion(installed);
  if (!left || !right) return false;
  const width = Math.max(left.length, right.length);
  while (left.length < width) left.push(0n);
  while (right.length < width) right.push(0n);
  for (let index = 0; index < width; index += 1) {
    if (left[index] !== right[index]) return left[index] > right[index];
  }
  return false;
}

// Select only packages whose binary or signed public manifest can differ.
// Unchanged raw binaries are reused from the previous successful run; the
// current trusted tool verifies them again before signing the new catalog.
export function packagesToBuild(registry, published, affectedPackages) {
  if (!Array.isArray(registry.apps) || !Array.isArray(published.entries)) {
    throw new Error("registry apps and published catalog entries must be arrays");
  }
  const previousById = new Map(
    published.entries.map(entry => [entry?.manifest?.id, entry?.manifest])
  );
  return registry.apps
    .filter(app => {
      const previous = previousById.get(app.id);
      return (
        !previous ||
        affectedPackages.has(app.package) ||
        changedManifestFields(app, previous).length > 0
      );
    })
    .map(app => app.package);
}

export function releaseNeeded(registry, published, affectedPackages) {
  if (packagesToBuild(registry, published, affectedPackages).length > 0) return true;
  const currentIds = new Set(registry.apps.map(app => app.id));
  return published.entries.some(entry => !currentIds.has(entry?.manifest?.id));
}

export function changedRegistryPackages(previousRegistry, currentRegistry) {
  if (!Array.isArray(previousRegistry?.apps) || !Array.isArray(currentRegistry?.apps)) {
    throw new Error("previous and current app registries must contain app arrays");
  }
  const previousById = new Map();
  for (const app of previousRegistry.apps) {
    if (typeof app?.id !== "string" || typeof app?.package !== "string") {
      throw new Error("previous app registry has no valid identity or package name");
    }
    if (previousById.has(app.id)) throw new Error(`previous app registry repeats ${app.id}`);
    previousById.set(app.id, app.package);
  }
  return new Set(
    currentRegistry.apps
      .filter(app => {
        if (typeof app?.id !== "string" || typeof app?.package !== "string") {
          throw new Error("current app registry has no valid identity or package name");
        }
        const previousPackage = previousById.get(app.id);
        return previousPackage !== undefined && previousPackage !== app.package;
      })
      .map(app => app.package)
  );
}

export function checkEntries(registry, published, affectedPackages, requireReleaseNotes = true) {
  if (!Array.isArray(registry.apps) || !Array.isArray(published.entries)) {
    throw new Error("registry apps and published catalog entries must be arrays");
  }

  const previousById = new Map();
  for (const entry of published.entries) {
    const manifest = entry?.manifest;
    if (!manifest || typeof manifest.id !== "string") {
      throw new Error("published catalog entry has no valid manifest identity");
    }
    previousById.set(manifest.id, manifest);
  }

  const failures = [];
  for (const app of registry.apps) {
    if (!app || typeof app.id !== "string" || typeof app.package !== "string") {
      throw new Error("registry app has no valid identity or package name");
    }
    const previous = previousById.get(app.id);
    if (!previous) {
      if (!meaningfulReleaseNotes(app.release_notes)) {
        failures.push(
          `${app.id}: a new Store app needs release_notes describing the initial user-visible value`
        );
      }
      continue;
    }
    if (typeof app.version !== "string" || typeof previous.version !== "string") {
      throw new Error(`${app.id} has no valid version`);
    }
    const changed = changedManifestFields(app, previous);
    if (affectedPackages.has(app.package)) changed.unshift("release inputs");

    if (changed.length > 0) {
      if (!numericVersionIsNewer(app.version, previous.version)) {
        failures.push(
          `${app.id}: package inputs changed (${changed.join(", ")}) but version ` +
            `${app.version} is not newer than ${previous.version}`
        );
      } else if (requireReleaseNotes && !meaningfulReleaseNotes(app.release_notes)) {
        failures.push(
          `${app.id}: version ${app.version} needs meaningful release_notes because ${changed.join(", ")} changed`
        );
      }
    }
  }

  if (failures.length > 0) {
    throw new Error(
      `${failures.join("\n")}\nUpdate only the affected app's version and release_notes, then rerun the contributor check.`
    );
  }
}

export function meaningfulReleaseNotes(value) {
  if (typeof value !== "string") return false;
  const note = value.trim();
  if (note.length < 12 || note.length > 240) return false;
  return !new Set(["update", "updated", "changes", "bug fixes", "misc fixes"]).has(
    note.toLowerCase().replace(/[.!]$/, "")
  );
}

function versionParts(value) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(value);
  if (!match) throw new Error(`invalid Cobalt version ${value}`);
  return match.slice(1).map(Number);
}

function versionIsOlder(value, minimum) {
  const left = versionParts(value);
  const right = versionParts(minimum);
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) return left[index] < right[index];
  }
  return false;
}

export function checkProtocolMinimums(
  registry,
  protocolVersion,
  baselines = PROTOCOL_MINIMUMS,
  affectedPackages = null
) {
  if (!Array.isArray(registry.apps)) throw new Error("registry apps must be an array");
  const minimum = baselines.get(protocolVersion);
  if (!minimum) {
    throw new Error(
      `protocol ${protocolVersion} has no minimum Cobalt release; add it to PROTOCOL_MINIMUMS`
    );
  }
  const failures = registry.apps
    .filter(app => !affectedPackages || affectedPackages.has(app.package))
    .filter(app => versionIsOlder(app.minimum_cobalt_version, minimum))
    .map(
      app =>
        `${app.id}: minimum Cobalt ${app.minimum_cobalt_version} is older than protocol ${protocolVersion}, first supported by ${minimum}`
    );
  if (failures.length > 0) throw new Error(failures.join("\n"));
}

export function checkBuildPackages(
  registry,
  published,
  packages,
  protocolVersion,
  baselines = PROTOCOL_MINIMUMS
) {
  if (!Array.isArray(registry.apps)) throw new Error("registry apps must be an array");
  if (!Array.isArray(packages) || packages.some(package_ => typeof package_ !== "string")) {
    throw new Error("build packages must be an array of strings");
  }
  const selected = new Set(packages);
  if (selected.size !== packages.length) {
    throw new Error("build packages must not contain duplicates");
  }
  const registered = new Set(registry.apps.map(app => app?.package));
  for (const package_ of selected) {
    if (!registered.has(package_)) throw new Error(`unknown build package ${package_}`);
  }
  checkProtocolMinimums(registry, protocolVersion, baselines, selected);
  checkEntries(registry, published, selected, false);
}

function currentProtocolVersion() {
  const source = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), "../crates/kobo-protocol/src/lib.rs"),
    "utf8"
  );
  const match = /pub const VERSION: u8 = (\d+);/.exec(source);
  if (!match) throw new Error("read the current protocol version");
  return Number(match[1]);
}

function command(name, arguments_) {
  try {
    return execFileSync(name, arguments_, {
      encoding: "utf8",
      maxBuffer: COMMAND_MAX_BUFFER
    }).trim();
  } catch (error) {
    throw new Error(`${name} ${arguments_.join(" ")} failed: ${error.message}`);
  }
}

function optionalCommand(name, arguments_) {
  try {
    return execFileSync(name, arguments_, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] })
      .trim();
  } catch {
    return null;
  }
}

function isInside(path, directory) {
  return path === directory || path.startsWith(`${directory}/`);
}


export function isContributionManifest(path, directory) {
  return path === `${directory}/cobalt-app.json`;
}
export { isFilmingScript, isDocumentation } from "./store-catalog-changes.mjs";

// The `version` entry of `[package]`, in any of its spellings, replaced by a
// placeholder so two manifests can be compared with it out of the way. Only
// `[package]` is touched: a `version` inside a dependency table is a different
// thing.
function normalizePackageVersion(source) {
  const lines = source.split("\n");
  const start = lines.findIndex(line => line.trim() === "[package]");
  if (start < 0) return source;
  for (let index = start + 1; index < lines.length; index += 1) {
    const line = lines[index].trim();
    if (line.startsWith("[")) break;
    if (
      /^version(\.workspace\s*=\s*true|\s*=\s*"[^"]*"|\s*=\s*\{\s*workspace\s*=\s*true\s*\})$/.test(
        line
      )
    ) {
      lines[index] = 'version = "<package-version>"';
      return lines.join("\n");
    }
  }
  return source;
}

// A package manifest that changed only in its version lines: its own under
// `[package]`, in whichever spelling, and the versions written beside path
// dependencies. Neither changes what the package compiles to, so the gate
// treats such an edit as it already treats the workspace version moving.
export function manifestOnlyChangesVersions(previousSource, currentSource) {
  return manifestOnlyChangesPathDependencyVersions(
    normalizePackageVersion(previousSource),
    normalizePackageVersion(currentSource)
  );
}

export function manifestOnlyChangesPathDependencyVersions(previousSource, currentSource) {
  const normalize = source =>
    source
      .split("\n")
      .map(line => {
        if (!line.includes("{") || !/\bpath\s*=/.test(line) || !/\bversion\s*=/.test(line)) {
          return line;
        }
        return line.replace(/\bversion\s*=\s*"[^"]+"/, 'version = "<workspace-version>"');
      })
      .join("\n");
  return normalize(previousSource).trimEnd() === normalize(currentSource).trimEnd();
}

export function releaseDiffArguments(baseRevision) {
  return [
    "diff",
    "--name-only",
    "--diff-filter=ACDMRT",
    `${baseRevision}...HEAD`
  ];
}

// The apps a change since the published catalog asks for a new version of.
//
// An app's release inputs are its own: the files in its package directory,
// less the README, its screenshots and its filming scripts, and less a
// manifest edit that only moves version numbers. Nothing outside that
// directory counts.
//
// The platform under the apps used to count as well. A lockfile entry that
// moved, a toolchain pin, or a workspace manifest edit asked every app that
// compiled it for a new version, and so did a publication that could not
// find the previous run's binaries and rebuilt everything. Each was a
// change that was not the app's own. A hand-kept list of reviewed file
// versions excused some of them, and had to be pinned again every time beta
// moved.
//
// The published build keeps working instead. The runtime reads every
// protocol back to the oldest one a published app speaks, so an app built
// against an older platform runs on a newer one, the way a phone app built
// for an older system does. Apps that are not asked for a version keep their
// published package byte for byte, minimum Cobalt version included (see
// kept_apps.rs in kobo-cli), and an app that does publish a version is built
// from the platform as it then is.
export function appsWithOwnChanges(changedPaths, packageDirectories, registeredPackages, isQuietManifest) {
  const affected = new Set();
  const quietManifests = new Set();
  for (const packageName of registeredPackages) {
    const directory = packageDirectories.get(packageName);
    if (!directory) {
      throw new Error(`${packageName} is in the Store catalog but names no workspace member`);
    }
    const ownChanges = changedPaths.filter(
      path =>
        isInside(path, directory) &&
        !isFilmingScript(path, directory) &&
        !isDocumentation(path, directory)
    );
    if (ownChanges.length === 0) continue;
    const manifest = `${directory}/Cargo.toml`;
    if (ownChanges.every(path => path === manifest) && isQuietManifest(manifest)) {
      quietManifests.add(manifest);
      continue;
    }
    affected.add(packageName);
  }
  return { affected, quietManifests };
}

export function analyzeAppReleaseInputs(baseRevision, registry) {
  const metadata = JSON.parse(
    command("cargo", ["metadata", "--format-version", "1", "--locked", "--no-deps"])
  );
  const workspaceRoot = resolve(metadata.workspace_root);
  const changedPaths = command("git", releaseDiffArguments(baseRevision))
    .split("\n")
    .filter(Boolean)
    .map(path => path.split(sep).join("/"));

  const registeredPackages = registry.apps.map(app => app.package);
  const workspaceMembers = new Set(metadata.workspace_members);
  const packageDirectories = new Map(
    metadata.packages
      .filter(package_ => workspaceMembers.has(package_.id))
      .map(package_ => [
        package_.name,
        relative(workspaceRoot, dirname(package_.manifest_path)).split(sep).join("/")
      ])
  );
  const storeChanges = storeCatalogChanges(
    changedPaths,
    storeWatchDirectories(packageDirectories, registeredPackages)
  );
  const { affected, quietManifests } = appsWithOwnChanges(
    changedPaths,
    packageDirectories,
    registeredPackages,
    manifest => {
      const previous = optionalCommand("git", ["show", `${baseRevision}:${manifest}`]);
      return (
        previous !== null &&
        manifestOnlyChangesVersions(previous, readFileSync(resolve(workspaceRoot, manifest), "utf8"))
      );
    }
  );
  // A package renamed in the registry is a different binary under the same
  // app, whatever its directory says.
  const previousRegistry = JSON.parse(
    command("git", ["show", `${baseRevision}:apps/catalog.json`])
  );
  for (const packageName of changedRegistryPackages(previousRegistry, registry)) {
    affected.add(packageName);
  }
  return {
    affected,
    storeChanges: storeChanges.filter(path => !quietManifests.has(path))
  };
}

export function affectedWorkspacePackages(baseRevision, registry) {
  return analyzeAppReleaseInputs(baseRevision, registry).affected;
}

const DIFF_MODES = new Set(["--list-packages", "--publish-needed"]);
const USAGE = [
  "usage: node tools/check-app-versions.mjs [--list-packages|--publish-needed]",
  "         --registry PATH --published-catalog PATH --base GIT_REVISION [--package NAME]",
  "   or: node tools/check-app-versions.mjs --validate-packages",
  "         --registry PATH --published-catalog PATH --packages JSON_ARRAY"
].join("\n");

// --validate-packages judges a package list the caller already settled on, so it
// takes that list instead of deriving one from a base revision. The publish
// workflow reaches it after it may have expanded an incremental selection back
// to every package, at which point there is no single base the list came from.
function argumentsFrom(argv) {
  const mode = DIFF_MODES.has(argv[0]) || argv[0] === "--validate-packages" ? argv[0] : null;
  if (mode) argv = argv.slice(1);
  const validating = mode === "--validate-packages";
  const required = validating
    ? ["--registry", "--published-catalog", "--packages"]
    : ["--registry", "--published-catalog", "--base"];
  const allowed = validating ? required : [...required, "--package"];
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowed.includes(flag) || !value) throw new Error(USAGE);
    values.set(flag, value);
  }
  if (!required.every(flag => values.has(flag))) throw new Error(USAGE);
  return { values, mode };
}

function requestedPackages(raw) {
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("--packages must be a JSON array of package names");
  }
  if (!Array.isArray(parsed) || parsed.some(name => typeof name !== "string")) {
    throw new Error("--packages must be a JSON array of package names");
  }
  return parsed;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const { values, mode } = argumentsFrom(process.argv.slice(2));
    const registry = readJson(resolve(values.get("--registry")), "app registry");
    const published = readJson(resolve(values.get("--published-catalog")), "published catalog");
    if (mode === "--validate-packages") {
      checkBuildPackages(
        registry,
        published,
        requestedPackages(values.get("--packages")),
        currentProtocolVersion()
      );
      console.log("Every selected app package is registered and carries a new version.");
    } else {
      const selectedPackage = values.get("--package");
      if (selectedPackage !== undefined) {
        registry.apps = registry.apps.filter(app => app.package === selectedPackage);
        if (registry.apps.length !== 1) {
          throw new Error(`package ${selectedPackage} does not name exactly one registered app`);
        }
      }
      const { affected, storeChanges } = analyzeAppReleaseInputs(
        values.get("--base"),
        registry,
        selectedPackage === undefined
      );
      checkProtocolMinimums(registry, currentProtocolVersion(), PROTOCOL_MINIMUMS, affected);
      if (mode === "--list-packages") {
        console.log(JSON.stringify(packagesToBuild(registry, published, affected)));
      } else if (mode === "--publish-needed") {
        console.log(releaseNeeded(registry, published, affected) ? "true" : "false");
      } else {
        checkEntries(registry, published, affected);
        if (storeChanges.length > 0 && !releaseNeeded(registry, published, affected)) {
          throw new Error(unpublishedStoreChangeReport("the app catalog", storeChanges));
        }
        console.log("Every changed app package has a new version.");
      }
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
