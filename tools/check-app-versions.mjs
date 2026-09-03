import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

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
const PROTOCOL_MINIMUMS = new Map([
  [10, "0.2.4"],
  [11, "0.3.1"],
  [12, "0.3.2"]
]);

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
    if (!previous) continue;
    if (typeof app.version !== "string" || typeof previous.version !== "string") {
      throw new Error(`${app.id} has no valid version`);
    }
    if (app.version !== previous.version) continue;

    const changed = [];
    if (affectedPackages.has(app.package)) changed.push("release inputs");
    for (const field of MANIFEST_FIELDS) {
      if (app[field] !== previous[field]) changed.push(field);
    }
    const currentCapabilities = normalizedCapabilities(app.capabilities, app.id);
    const previousCapabilities = normalizedCapabilities(previous.capabilities, app.id);
    if (JSON.stringify(currentCapabilities) !== JSON.stringify(previousCapabilities)) {
      changed.push("capabilities");
    }

    if (changed.length > 0) {
      failures.push(
        `${app.id}: package inputs changed (${changed.join(", ")}) but version remains ${app.version}`
      );
    }
  }

  if (failures.length > 0) {
    throw new Error(`${failures.join("\n")}\nBump each affected version in the app registry.`);
  }
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

export function checkProtocolMinimums(registry, protocolVersion, baselines = PROTOCOL_MINIMUMS) {
  if (!Array.isArray(registry.apps)) throw new Error("registry apps must be an array");
  const minimum = baselines.get(protocolVersion);
  if (!minimum) {
    throw new Error(
      `protocol ${protocolVersion} has no minimum Cobalt release; add it to PROTOCOL_MINIMUMS`
    );
  }
  const failures = registry.apps
    .filter(app => versionIsOlder(app.minimum_cobalt_version, minimum))
    .map(
      app =>
        `${app.id}: minimum Cobalt ${app.minimum_cobalt_version} is older than protocol ${protocolVersion}, first supported by ${minimum}`
    );
  if (failures.length > 0) throw new Error(failures.join("\n"));
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

function isInside(path, directory) {
  return path === directory || path.startsWith(`${directory}/`);
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

export function affectedWorkspacePackages(baseRevision) {
  const metadata = JSON.parse(command("cargo", ["metadata", "--format-version", "1", "--locked"]));
  const workspaceRoot = resolve(metadata.workspace_root);
  const changedPaths = command("git", [
    "diff",
    "--name-only",
    "--diff-filter=ACMRT",
    `${baseRevision}...HEAD`
  ])
    .split("\n")
    .filter(Boolean)
    .map(path => path.split(sep).join("/"));

  const globalInputs = new Set(["Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml"]);
  if (changedPaths.some(path => globalInputs.has(path) || path.startsWith(".cargo/"))) {
    return new Set(metadata.packages.map(package_ => package_.name));
  }

  const workspaceMembers = new Set(metadata.workspace_members);
  const workspacePackages = metadata.packages.filter(package_ => workspaceMembers.has(package_.id));
  const changedIds = new Set();
  for (const package_ of workspacePackages) {
    const directory = dirname(package_.manifest_path);
    const relativeDirectory = relative(workspaceRoot, directory).split(sep).join("/");
    if (changedPaths.some(path => isInside(path, relativeDirectory))) changedIds.add(package_.id);
  }

  const dependencies = new Map(
    metadata.resolve.nodes.map(node => [node.id, releaseDependencyIds(node)])
  );
  function dependsOnChanged(id, seen = new Set()) {
    if (changedIds.has(id)) return true;
    if (seen.has(id)) return false;
    seen.add(id);
    return (dependencies.get(id) || []).some(dependency => dependsOnChanged(dependency, seen));
  }

  return new Set(
    workspacePackages.filter(package_ => dependsOnChanged(package_.id)).map(package_ => package_.name)
  );
}

function argumentsFrom(argv) {
  const allowed = ["--registry", "--published-catalog", "--base"];
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowed.includes(flag) || !value) {
      throw new Error(
        "usage: node tools/check-app-versions.mjs --registry PATH --published-catalog PATH --base GIT_REVISION"
      );
    }
    values.set(flag, value);
  }
  if (values.size !== allowed.length) {
    throw new Error(
      "usage: node tools/check-app-versions.mjs --registry PATH --published-catalog PATH --base GIT_REVISION"
    );
  }
  return values;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const values = argumentsFrom(process.argv.slice(2));
    const registry = readJson(resolve(values.get("--registry")), "app registry");
    const published = readJson(resolve(values.get("--published-catalog")), "published catalog");
    const affected = affectedWorkspacePackages(values.get("--base"));
    checkProtocolMinimums(registry, currentProtocolVersion());
    checkEntries(registry, published, affected);
    console.log("Every changed app package has a new version.");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
