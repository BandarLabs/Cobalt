import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Everything whose content reaches a reader through the signed Store catalog.
//
// `apps/` is taken whole: it holds the registry and every Store-only package.
// Registered packages that still live under `examples/` are watched by their
// workspace directory, because those binaries are the same ones the catalog
// signs. Device-only examples (launcher, settings, store, terminal, hello)
// are not in the catalog and are not watched here.
//
// `crates/`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain*` and `.cargo/` are
// deliberately absent. Those are device-package inputs. A platform release
// carries them; demanding a catalog republish for them is what turned a
// Nickel-supplicant fix into sixteen Store version bumps and a red publish
// job. The next Store publication of an actually edited app compiles against
// whatever the platform then is.

export function affectsStoreCatalog(path, directories) {
  if (typeof path !== "string" || !Array.isArray(directories)) return false;
  const normalized = path.trim().split("\\").join("/");
  if (normalized.length === 0) return false;
  return directories.some(
    directory => normalized === directory || normalized.startsWith(`${directory}/`)
  );
}

export function storeCatalogChanges(paths, directories) {
  return [...new Set(paths.filter(path => affectsStoreCatalog(path, directories)))].sort();
}

export function storeWatchDirectories(packageDirectories, registeredPackages) {
  const directories = new Set(["apps"]);
  for (const name of registeredPackages) {
    const directory = packageDirectories.get(name);
    if (!directory) {
      throw new Error(`${name} is in the Store catalog but names no workspace member`);
    }
    directories.add(directory);
  }
  return [...directories].sort();
}

export function workspacePackageDirectories(rootManifest, readMemberManifest) {
  const match = /^members\s*=\s*\[([\s\S]*?)^\]/m.exec(rootManifest);
  if (!match) throw new Error("read the workspace members from Cargo.toml");
  const directories = new Map();
  for (const member of [...match[1].matchAll(/"([^"]+)"/g)].map(entry => entry[1])) {
    const name = /^name\s*=\s*"([^"]+)"$/m.exec(readMemberManifest(member))?.[1];
    if (name) directories.set(name, member);
  }
  return directories;
}

export function registeredStorePackages(registry) {
  if (!Array.isArray(registry?.apps)) {
    throw new Error("app registry has no app array");
  }
  return registry.apps.map(app => {
    if (typeof app?.package !== "string" || app.package.length === 0) {
      throw new Error("registry app has no package name");
    }
    return app.package;
  });
}

// The run that finds these changes is the one that decided there was nothing
// to publish, so the report has to name the edit that makes it publish.
export function unpublishedStoreChangeReport(channel, changes) {
  return [
    `${channel} is already published, but Store catalog inputs changed and this run is not publishing.`,
    "These paths differ between the published catalog and this commit:",
    ...changes.map(path => `  ${path}`),
    "",
    "No reader receives them while the catalog stays at the previous publication.",
    "Bump each affected app to a strictly newer numeric version in apps/catalog.json,",
    "push again, and Publish apps will sign a new beta catalog."
  ].join("\n");
}

function argumentsFrom(argv) {
  const allowed = ["--channel", "--root"];
  const usage =
    "usage: node tools/store-catalog-changes.mjs --channel TAG [--root PATH] < changed-paths";
  const values = new Map([["--root", "."]]);
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowed.includes(flag) || !value) throw new Error(usage);
    values.set(flag, value);
  }
  if (!values.has("--channel")) throw new Error(usage);
  return values;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const values = argumentsFrom(process.argv.slice(2));
    const root = values.get("--root");
    const channel = values.get("--channel");
    const registry = JSON.parse(readFileSync(join(root, "apps/catalog.json"), "utf8"));
    const directories = storeWatchDirectories(
      workspacePackageDirectories(readFileSync(join(root, "Cargo.toml"), "utf8"), member =>
        readFileSync(join(root, member, "Cargo.toml"), "utf8")
      ),
      registeredStorePackages(registry)
    );
    const paths = readFileSync(0, "utf8").split("\n").filter(Boolean);
    const changes = storeCatalogChanges(paths, directories);
    if (changes.length > 0) {
      console.error(unpublishedStoreChangeReport(channel, changes));
      process.exitCode = 1;
    } else {
      console.log(
        `No Store catalog input changed since ${channel}; ${paths.length} changed path(s) reach readers without a catalog publication.`
      );
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
