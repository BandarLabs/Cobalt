import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { collectRegistry } from "./app-registry.mjs";

// Everything whose content reaches a reader through the signed Store catalog.
//
// `apps/` holds the registry and every Store-only package. Host drive scripts
// and package README files use the same exclusions as the release checker.
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

// Prose beside a package is not in the package. A published entry is built
// from cobalt-app.json and the compiled binary, and neither the registry nor
// the app-page generator reads a README, so no byte anybody downloads can
// change because a paragraph did.
//
// Counting it is the same mistake the drive scripts above were rescued from.
// Correcting a sentence in apps/syncthing/README.md that had gone stale --
// it still told readers to export an environment variable for a packaging
// step that no longer exists -- was refused as an unreleased change to the
// Syncthing application, and the remedy on offer was to publish a new version
// of it to every reader who has it in order to fix a paragraph none of them
// download.
//
// Only prose directly beside the package: a nested path may be a screenshot
// the app page does publish, and src/notes.md is source.
export function isDocumentation(path, packageDirectory) {
  if (!path.startsWith(`${packageDirectory}/`)) return false;
  const beside = path.slice(packageDirectory.length + 1);
  return !beside.includes("/") && beside.endsWith(".md");
}

export function affectsStoreCatalog(path, directories) {
  if (typeof path !== "string" || !Array.isArray(directories)) return false;
  const normalized = path.trim().split("\\").join("/");
  if (normalized.length === 0) return false;
  // "apps" is the broad catch-all, not a package root. Applying an exclusion
  // there would hide an actual app named drive or a new registry-level file.
  const packageDirectories = directories.filter(directory => directory !== "apps");
  if (packageDirectories.some(directory =>
    isFilmingScript(normalized, directory) || isDocumentation(normalized, directory)
  )) return false;
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
  const members = [...match[1].matchAll(/"([^"]+)"/g)].flatMap(entry => {
    const member = entry[1];
    if (!member.endsWith("/*")) return [member];
    const directory = member.slice(0, -2);
    return readdirSync(directory, { withFileTypes: true })
      .filter(item => item.isDirectory())
      .map(item => `${directory}/${item.name}`);
  });
  for (const member of members) {
    let manifest;
    try {
      manifest = readMemberManifest(member);
    } catch {
      continue;
    }
    const name = /^name\s*=\s*"([^"]+)"$/m.exec(manifest)?.[1];
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
    const registry = collectRegistry({
      basePath: join(root, "apps/catalog.json"),
      sourcePaths: [join(root, "apps"), join(root, "examples")]
    });
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
