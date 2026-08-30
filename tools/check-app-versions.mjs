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

function command(name, arguments_) {
  try {
    return execFileSync(name, arguments_, { encoding: "utf8" }).trim();
  } catch (error) {
    throw new Error(`${name} ${arguments_.join(" ")} failed: ${error.message}`);
  }
}

function isInside(path, directory) {
  return path === directory || path.startsWith(`${directory}/`);
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
    metadata.resolve.nodes.map(node => [node.id, node.deps.map(dependency => dependency.pkg)])
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
    checkEntries(registry, published, affected);
    console.log("Every changed app package has a new version.");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
