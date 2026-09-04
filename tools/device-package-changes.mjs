import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Everything whose content reaches a reader inside the device package, or that
// decides how that package is built.
//
// `crates/` and `examples/` are taken whole: the packaged binaries listed in
// kobo-cli's INSTALLED_PACKAGES are kobod and fifteen of the sixteen example
// members, and the crates around them are the closure those are built from.
// Being broader than the closure here costs a version bump nobody needed;
// being narrower ships a device package no reader can get.
//
// `apps/` is deliberately absent. Store-only applications reach a reader
// through the signed app catalog, which is published by its own workflow on
// every merge, so demanding a platform release for them would turn every app
// landing red. `installedPackagesOutsideTheGate` below is what stops that
// exclusion from quietly becoming wrong.
const DEVICE_PACKAGE_DIRECTORIES = ["crates", "examples", ".cargo"];
const DEVICE_PACKAGE_FILES = ["Cargo.toml", "Cargo.lock"];
const TOOLCHAIN_PREFIX = "rust-toolchain";

export function affectsDevicePackage(path) {
  if (typeof path !== "string") return false;
  const normalized = path.trim().split("\\").join("/");
  if (normalized.length === 0) return false;
  if (DEVICE_PACKAGE_FILES.includes(normalized)) return true;
  // rust-toolchain and rust-toolchain.toml pin the compiler the package is
  // built with; anything else at the root sharing that name is a pin too.
  if (!normalized.includes("/") && normalized.startsWith(TOOLCHAIN_PREFIX)) return true;
  return DEVICE_PACKAGE_DIRECTORIES.some(
    directory => normalized.startsWith(`${directory}/`)
  );
}

export function devicePackageChanges(paths) {
  return [...new Set(paths.filter(path => affectsDevicePackage(path)))].sort();
}

// kobo-cli decides what goes into the package. Read its list back rather than
// keeping a second copy of it here.
export function installedPackageNames(cliSource) {
  const match = /const INSTALLED_PACKAGES[^=]*=\s*&\[([\s\S]*?)\n\];/.exec(cliSource);
  if (!match) throw new Error("read INSTALLED_PACKAGES from crates/kobo-cli/src/main.rs");
  // The first string of each tuple is the package; the second is a feature.
  const names = [...match[1].matchAll(/^\s*\(\s*"([^"]+)"/gm)].map(entry => entry[1]);
  if (names.length === 0) throw new Error("INSTALLED_PACKAGES names no packages");
  return names;
}

export function workspaceMembers(rootManifest) {
  const match = /^members\s*=\s*\[([\s\S]*?)^\]/m.exec(rootManifest);
  if (!match) throw new Error("read the workspace members from Cargo.toml");
  return [...match[1].matchAll(/"([^"]+)"/g)].flatMap(entry => {
    const member = entry[1];
    if (!member.endsWith("/*")) return [member];
    const directory = member.slice(0, -2);
    return readdirSync(directory, { withFileTypes: true })
      .filter(item => item.isDirectory())
      .map(item => `${directory}/${item.name}`);
  });
}

// A packaged application added somewhere this gate does not watch would be a
// silent hole in it, which is the whole class of bug being fixed here.
export function installedPackagesOutsideTheGate(names, directoryOf) {
  return names.filter(name => {
    const directory = directoryOf(name);
    if (!directory) {
      throw new Error(`${name} is packaged for the device but names no workspace member`);
    }
    return !affectsDevicePackage(`${directory}/Cargo.toml`);
  });
}

export function nextPatchVersion(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) throw new Error(`invalid workspace version ${version}`);
  return `${match[1]}.${match[2]}.${Number(match[3]) + 1}`;
}

// The run that finds these changes is the one that decided there was nothing to
// publish, so the report has to name the single edit that makes it publish.
export function unpublishedChangeReport(tag, version, changes) {
  const next = nextPatchVersion(version);
  return [
    `${tag} is already published, but the device package is not what beta now builds.`,
    "These paths differ between the published tag and this commit:",
    ...changes.map(path => `  ${path}`),
    "",
    `No reader receives them while the workspace version stays at ${version}.`,
    `Raise version in [workspace.package] in Cargo.toml from ${version} to ${next},`,
    `push again, and this workflow will build and publish beta-v${next}.`
  ].join("\n");
}

function packageDirectories(root) {
  const directories = new Map();
  for (const member of workspaceMembers(readFileSync(join(root, "Cargo.toml"), "utf8"))) {
    const manifest = readFileSync(join(root, member, "Cargo.toml"), "utf8");
    const name = /^name\s*=\s*"([^"]+)"$/m.exec(manifest)?.[1];
    if (name) directories.set(name, member);
  }
  return directories;
}

function argumentsFrom(argv) {
  const allowed = ["--tag", "--version", "--root"];
  const usage =
    "usage: node tools/device-package-changes.mjs --tag TAG --version VERSION [--root PATH] < changed-paths";
  const values = new Map([["--root", "."]]);
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowed.includes(flag) || !value) throw new Error(usage);
    values.set(flag, value);
  }
  if (!values.has("--tag") || !values.has("--version")) throw new Error(usage);
  return values;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const values = argumentsFrom(process.argv.slice(2));
    const tag = values.get("--tag");
    const version = values.get("--version");
    const root = values.get("--root");

    const directories = packageDirectories(root);
    const unwatched = installedPackagesOutsideTheGate(
      installedPackageNames(
        readFileSync(join(root, "crates/kobo-cli/src/main.rs"), "utf8")
      ),
      name => directories.get(name)
    );
    if (unwatched.length > 0) {
      throw new Error(
        `these packages go into the device package from a directory this check does not watch: ${unwatched.join(", ")}\n` +
          "Add their directory to DEVICE_PACKAGE_DIRECTORIES in tools/device-package-changes.mjs."
      );
    }

    const paths = readFileSync(0, "utf8").split("\n").filter(Boolean);
    const changes = devicePackageChanges(paths);
    if (changes.length > 0) {
      console.error(unpublishedChangeReport(tag, version, changes));
      process.exitCode = 1;
    } else {
      console.log(
        `No device package input changed since ${tag}; ${paths.length} changed path(s) reach readers without a release.`
      );
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
