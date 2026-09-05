import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { compatibilityPolicy, deriveMinimumCobalt } from "./app-registry.mjs";
import {
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
const COMPATIBLE_RELEASE_PATHS = new Set([
  "Cargo.lock",
  "crates/kobo-abi/src/lib.rs",
  "crates/kobo-net/src/lib.rs",
  "crates/kobo-net/src/lines.rs",
  "crates/kobo-net/tests/fixtures/localhost-ca.der",
  "crates/kobo-net/tests/fixtures/localhost-cert.der",
  "crates/kobo-net/tests/fixtures/localhost-key.der",
  "crates/kobo-net/tests/lichess_stream_mock.rs",
  "crates/kobo-policy/src/credentials.rs",
  "crates/kobo-policy/src/services.rs",
  "crates/kobo-policy/src/tasks.rs",
  "crates/kobo-protocol/src/lib.rs",
  "crates/kobo-sdk/Cargo.toml",
  "crates/kobo-sdk/src/credentials.rs",
  "crates/kobo-sdk/src/keyboard.rs",
  "crates/kobo-sdk/src/lib.rs",
  "crates/kobo-sdk/src/terminal.rs",
  "crates/kobo-text/src/lib.rs",
  "crates/kobo-ui/Cargo.toml",
  "crates/kobo-ui/src/lib.rs",
  "crates/kobo-ui/src/vector.rs",
  "crates/kobo-ui/src/vector/tabler.rs",
  "examples/gutenbird/Cargo.toml",
  "examples/gutenbird/src/main.rs"
]);

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

export function checkEntries(registry, published, affectedPackages) {
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
      } else if (!meaningfulReleaseNotes(app.release_notes)) {
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
  checkEntries(registry, published, selected);
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

// A drive script is the host-side route used to film an application. It is
// never compiled into the signed bundle, so adding or editing one must not
// look like a Store package change. That mistake is what turned a simulator
// recording script into a forced version bump of every example that grew one.
//
// Matching only drive.txt and drive.kobo let it happen again as soon as an
// application needed more than one route: a shelf of thirty-six applications
// grew drive.sh, drive-states.kobo, drive-empty.kobo, and a drive/ directory
// of scenes, and every one of those counted as a release input. So the whole
// family beside the package is named here, and only beside the package —
// src/drive.txt is source and a sibling directory is another package.
export function isFilmingScript(path, packageDirectory) {
  if (!path.startsWith(`${packageDirectory}/`)) return false;
  const beside = path.slice(packageDirectory.length + 1);
  return beside === "drive" || beside.startsWith("drive/") || /^drive[-.][^/]*$/.test(beside);
}

// Returns the dependency edges capable of changing a release artifact.
//
// Cargo includes dev dependencies in the resolved metadata graph even when
// they are used only to compile tests. Following those edges made an
// unrelated test fixture change look like an app binary change and forced
// contributors to bump and republish unaffected apps. Normal and build
// dependencies still count, including an edge used as both dev and normal.
export function releaseDependencyIds(node) {
  return node.deps
    .filter(dependency => {
      const kinds = dependency.dep_kinds;
      return (
        !Array.isArray(kinds) ||
        kinds.length === 0 ||
        kinds.some(dependencyKind => dependencyKind.kind !== "dev")
      );
    })
    .map(dependency => dependency.pkg);
}

function workspaceMembers(source) {
  const match = /(^members\s*=\s*\[)([\s\S]*?)(^\])/m.exec(source);
  if (!match) return null;
  return {
    entries: [...match[2].matchAll(/"([^"]+)"/g)].map(entry => entry[1]),
    remainder:
      `${source.slice(0, match.index)}${match[1]}\n${match[3]}` +
      source.slice(match.index + match[0].length).trimEnd()
  };
}

function normalizeWorkspaceVersion(source) {
  const lines = source.split("\n");
  const start = lines.findIndex(line => line.trim() === "[workspace.package]");
  if (start < 0) return null;
  const end = lines.findIndex((line, index) => index > start && line.trim().startsWith("["));
  const stop = end < 0 ? lines.length : end;
  const versions = [];
  for (let index = start + 1; index < stop; index += 1) {
    if (/^version\s*=\s*"[^"]+"\s*$/.test(lines[index])) versions.push(index);
  }
  if (versions.length !== 1) return null;
  lines[versions[0]] = 'version = "<workspace-version>"';
  return lines.join("\n");
}

// Workspace member additions and the shared package version do not alter an
// existing Store app by themselves. Resolver, profile, dependency, or lint
// changes remain conservative global release inputs.
export function manifestOnlyChangesWorkspaceMembershipOrVersion(
  previousSource,
  currentSource
) {
  const previous = workspaceMembers(previousSource);
  const current = workspaceMembers(currentSource);
  if (!previous || !current) return false;

  const previousMembers = new Set(previous.entries);
  const currentMembers = new Set(current.entries);
  const currentGlobs = [...currentMembers]
    .filter(member => member.endsWith("/*"))
    .map(member => member.slice(0, -1));
  if (
    ![...previousMembers].every(
      member =>
        currentMembers.has(member) ||
        currentGlobs.some(prefix => member.startsWith(prefix))
    )
  ) {
    return false;
  }

  const previousRemainder = normalizeWorkspaceVersion(previous.remainder);
  const currentRemainder = normalizeWorkspaceVersion(current.remainder);
  return (
    previousRemainder !== null &&
    currentRemainder !== null &&
    previousRemainder === currentRemainder
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

export function compatibleChangePaths(
  manifest,
  protocolVersion,
  changedPaths,
  baseBlobOf,
  currentBlobOf
) {
  if (
    manifest?.format_version !== 1 ||
    !Array.isArray(manifest.changes) ||
    manifest.changes.some(
      change =>
        !Number.isInteger(change?.protocol_version) ||
        typeof change?.reason !== "string" ||
        change.reason.length === 0 ||
        !Array.isArray(change?.files)
    )
  ) {
    throw new Error("invalid app release compatible-change manifest");
  }

  const changed = new Set(changedPaths);
  const compatible = new Set();
  for (const change of manifest.changes) {
    if (change.protocol_version !== protocolVersion) continue;
    for (const file of change.files) {
      if (
        typeof file?.path !== "string" ||
        !COMPATIBLE_RELEASE_PATHS.has(file.path) ||
        (file?.base_blob !== null && !/^[0-9a-f]{40}$/.test(file?.base_blob)) ||
        !/^[0-9a-f]{40}$/.test(file?.compatible_blob)
      ) {
        throw new Error("invalid app release compatible-change file");
      }
      if (
        changed.has(file.path) &&
        baseBlobOf(file.path) === file.base_blob &&
        currentBlobOf(file.path) === file.compatible_blob
      ) {
        compatible.add(file.path);
      }
    }
  }
  return compatible;
}

function lockfileParts(source) {
  const marker = "[[package]]";
  const firstPackage = source.indexOf(marker);
  if (firstPackage < 0) return null;
  return {
    preamble: source.slice(0, firstPackage),
    packages: source
      .slice(firstPackage)
      .split(/(?=^\[\[package\]\]$)/m)
      .filter(Boolean)
      .map(packageBlock => packageBlock.trimEnd())
  };
}

function lockPackage(packageBlock) {
  const name = /^name = "([^"]+)"$/m.exec(packageBlock)?.[1];
  const version = /^version = "([^"]+)"$/m.exec(packageBlock)?.[1];
  if (!name || !version) {
    throw new Error("Cargo.lock package block has no valid name and version");
  }
  const source = /^source = "([^"]+)"$/m.exec(packageBlock)?.[1];
  return {
    name,
    version: source ? version : "<workspace-version>",
    source: source || "",
    canonical: source
      ? packageBlock
      : packageBlock.replace(/^version = "[^"]+"$/m, 'version = "<workspace-version>"')
  };
}

function packageIdentity(name, version, source = "") {
  return JSON.stringify([name, version, source]);
}

function lockPackagesByIdentity(source) {
  const parts = lockfileParts(source);
  if (!parts) throw new Error("Cargo.lock has no package blocks");
  const packages = new Map();
  for (const packageBlock of parts.packages) {
    const parsed = lockPackage(packageBlock);
    const identity = packageIdentity(parsed.name, parsed.version, parsed.source);
    const value = packages.get(identity) || {
      name: parsed.name,
      blocks: []
    };
    const blocks = value.blocks;
    blocks.push(parsed.canonical);
    packages.set(identity, value);
  }
  for (const value of packages.values()) value.blocks.sort();
  return { preamble: parts.preamble, packages };
}

// Compare exact package identities rather than crate names, so two resolved
// versions of one crate keep their distinct consumer sets. Local workspace or
// path package version-only edits are normalized because the shared workspace
// release number does not by itself change Store app code.
export function changedLockPackageIdentities(previousSource, currentSource) {
  const previous = lockPackagesByIdentity(previousSource);
  const current = lockPackagesByIdentity(currentSource);
  if (previous.preamble !== current.preamble) {
    throw new Error("Cargo.lock preamble changed; cannot isolate affected Store apps");
  }

  const names = new Set([...previous.packages.keys(), ...current.packages.keys()]);
  return new Set(
    [...names].filter(identity => {
      const previousBlocks = previous.packages.get(identity)?.blocks || [];
      const currentBlocks = current.packages.get(identity)?.blocks || [];
      return JSON.stringify(previousBlocks) !== JSON.stringify(currentBlocks);
    })
  );
}

export function releaseLockPackageIdentities(
  previousSource,
  currentSource,
  compatiblePaths
) {
  if (!(compatiblePaths instanceof Set)) {
    throw new Error("compatible release paths must be a set");
  }
  return compatiblePaths.has("Cargo.lock")
    ? new Set()
    : changedLockPackageIdentities(previousSource, currentSource);
}

export function registeredConsumers(
  metadata,
  registeredPackages,
  changedPackageIdentities,
  strictUnknown = true
) {
  const packagesByIdentity = new Map();
  const packageIdsByName = new Map();
  for (const package_ of metadata.packages) {
    const identity = packageIdentity(
      package_.name,
      package_.source ? package_.version : "<workspace-version>",
      package_.source || ""
    );
    const ids = packagesByIdentity.get(identity) || [];
    ids.push(package_.id);
    packagesByIdentity.set(identity, ids);
    const namedIds = packageIdsByName.get(package_.name) || [];
    namedIds.push(package_.id);
    packageIdsByName.set(package_.name, namedIds);
  }

  const changedIds = new Set();
  for (const identity of changedPackageIdentities) {
    const ids = packagesByIdentity.get(identity);
    if (!ids) {
      const [name] = JSON.parse(identity);
      const possibleReplacements = packageIdsByName.get(name);
      if (!possibleReplacements) {
        if (!strictUnknown) continue;
        throw new Error(
          `Cargo.lock changed package ${name}, but current cargo metadata cannot identify its consumers`
        );
      }
      for (const id of possibleReplacements) changedIds.add(id);
      continue;
    }
    for (const id of ids) changedIds.add(id);
  }

  const dependencies = new Map(
    metadata.resolve.nodes.map(node => [node.id, releaseDependencyIds(node)])
  );
  function dependsOnChanged(id, seen = new Set()) {
    if (changedIds.has(id)) return true;
    if (seen.has(id)) return false;
    const next = new Set(seen);
    next.add(id);
    return (dependencies.get(id) || []).some(dependency => dependsOnChanged(dependency, next));
  }

  const registered = new Set(registeredPackages);
  return new Set(
    metadata.packages
      .filter(package_ => registered.has(package_.name) && dependsOnChanged(package_.id))
      .map(package_ => package_.name)
  );
}

export function releaseDiffArguments(baseRevision) {
  return [
    "diff",
    "--name-only",
    "--diff-filter=ACDMRT",
    `${baseRevision}...HEAD`
  ];
}

// Kept as a small compatibility helper for callers that only need to know
// whether an edit consists entirely of unrelated package additions.
export function lockfileOnlyAddsPackages(previousSource, currentSource) {
  const previous = lockfileParts(previousSource);
  const current = lockfileParts(currentSource);
  if (!previous || !current || previous.preamble !== current.preamble) return false;
  if (current.packages.length <= previous.packages.length) return false;

  const available = new Map();
  for (const packageBlock of current.packages) {
    available.set(packageBlock, (available.get(packageBlock) || 0) + 1);
  }
  for (const packageBlock of previous.packages) {
    const count = available.get(packageBlock) || 0;
    if (count === 0) return false;
    available.set(packageBlock, count - 1);
  }
  return true;
}

// Given an explicit change list, decide whether the catalog must move.
// CI and documentation stay quiet. Workspace packages and build inputs still
// reach the Cargo dependency walk because Store binaries statically link them.
export function storeImpactOfChangedPaths(changedPaths, packageDirectories, registeredPackages) {
  const storeDirectories = storeWatchDirectories(packageDirectories, registeredPackages);
  const storeChanges = storeCatalogChanges(changedPaths, storeDirectories).filter(path => {
    const directory = path.split("/").slice(0, -1).join("/");
    return !isFilmingScript(path, directory);
  });
  const registered = new Set(registeredPackages);
  const sharedPackageChanged = [...packageDirectories].some(
    ([packageName, directory]) =>
      !registered.has(packageName) &&
      changedPaths.some(path => isInside(path, directory) && !isFilmingScript(path, directory))
  );
  const globalBuildInputChanged = changedPaths.some(
    path =>
      path === "Cargo.toml" ||
      path === "Cargo.lock" ||
      path === "rust-toolchain" ||
      path === "rust-toolchain.toml" ||
      path.startsWith(".cargo/")
  );
  const catalogQuiet =
    storeChanges.length === 0 && !sharedPackageChanged && !globalBuildInputChanged;
  return {
    storeChanges,
    catalogQuiet,
    affected: catalogQuiet ? new Set() : null
  };
}

export function analyzeAppReleaseInputs(baseRevision, registry, strictUnknown = true) {
  const metadata = JSON.parse(command("cargo", ["metadata", "--format-version", "1", "--locked"]));
  const workspaceRoot = resolve(metadata.workspace_root);
  const changedPaths = command("git", releaseDiffArguments(baseRevision))
    .split("\n")
    .filter(Boolean)
    .map(path => path.split(sep).join("/"));

  const registeredPackages = registry.apps.map(app => app.package);
  const registered = new Set(registeredPackages);
  const workspaceMembers = new Set(metadata.workspace_members);
  const workspacePackages = metadata.packages.filter(package_ => workspaceMembers.has(package_.id));
  const packageDirectories = new Map(
    workspacePackages.map(package_ => [
      package_.name,
      relative(workspaceRoot, dirname(package_.manifest_path)).split(sep).join("/")
    ])
  );
  const impact = storeImpactOfChangedPaths(changedPaths, packageDirectories, registeredPackages);
  const storeChanges = impact.storeChanges;
  if (impact.catalogQuiet) {
    return { affected: impact.affected, storeChanges };
  }

  const compatibleManifest = readJson(
    resolve(
      dirname(fileURLToPath(import.meta.url)),
      "app-release-compatible-changes.json"
    ),
    "app release compatible changes"
  );
  const compatiblePaths = compatibleChangePaths(
    compatibleManifest,
    currentProtocolVersion(),
    changedPaths,
    path => optionalCommand("git", ["rev-parse", `${baseRevision}:${path}`]),
    path => command("git", ["hash-object", path])
  );

  const unconditionalGlobalInputs = new Set(["rust-toolchain", "rust-toolchain.toml"]);
  const changesGlobalInputs =
    changedPaths.some(path => unconditionalGlobalInputs.has(path) || path.startsWith(".cargo/")) ||
    (changedPaths.includes("Cargo.toml") &&
      !manifestOnlyChangesWorkspaceMembershipOrVersion(
        command("git", ["show", `${baseRevision}:Cargo.toml`]),
        readFileSync(resolve(workspaceRoot, "Cargo.toml"), "utf8")
      ));
  if (changesGlobalInputs) {
    return { affected: new Set(registeredPackages), storeChanges };
  }

  const changedIdentities = new Set();
  const previousRegistry = JSON.parse(
    command("git", ["show", `${baseRevision}:apps/catalog.json`])
  );
  for (const packageName of changedRegistryPackages(previousRegistry, registry)) {
    const candidates = workspacePackages.filter(package_ => package_.name === packageName);
    if (candidates.length !== 1) {
      throw new Error(
        `app registry package ${packageName} does not name exactly one workspace package`
      );
    }
    const [package_] = candidates;
    changedIdentities.add(
      packageIdentity(package_.name, "<workspace-version>", "")
    );
  }
  for (const package_ of workspacePackages) {
    const directory = dirname(package_.manifest_path);
    const relativeDirectory = relative(workspaceRoot, directory).split(sep).join("/");
    const packageChanges = changedPaths.filter(
      path =>
        isInside(path, relativeDirectory) &&
        !isContributionManifest(path, relativeDirectory) &&
        !compatiblePaths.has(path) &&
        !isFilmingScript(path, relativeDirectory)
    );
    if (packageChanges.length === 0) continue;
    const relativeManifest = relative(workspaceRoot, package_.manifest_path).split(sep).join("/");
    const previousManifest = optionalCommand("git", [
      "show",
      `${baseRevision}:${relativeManifest}`
    ]);
    if (
      previousManifest !== null &&
      packageChanges.every(path => path === relativeManifest) &&
      manifestOnlyChangesPathDependencyVersions(
        previousManifest,
        readFileSync(package_.manifest_path, "utf8")
      )
    ) {
      continue;
    }
    changedIdentities.add(
      packageIdentity(
        package_.name,
        package_.source ? package_.version : "<workspace-version>",
        package_.source || ""
      )
    );
  }

  if (changedPaths.includes("Cargo.lock")) {
    const lockChanges = releaseLockPackageIdentities(
      command("git", ["show", `${baseRevision}:Cargo.lock`]),
      readFileSync(resolve(workspaceRoot, "Cargo.lock"), "utf8"),
      compatiblePaths
    );
    for (const identity of lockChanges) changedIdentities.add(identity);
  }

  return {
    affected: registeredConsumers(metadata, registeredPackages, changedIdentities, strictUnknown),
    storeChanges
  };
}

export function affectedWorkspacePackages(baseRevision, registry, strictUnknown = true) {
  return analyzeAppReleaseInputs(baseRevision, registry, strictUnknown).affected;
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
