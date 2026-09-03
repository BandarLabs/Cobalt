import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Everything whose content reaches a reader inside the device package, or that
// decides how that package is built. A change under any of these cannot arrive
// on a device without a new release, so publishing nothing is the wrong answer.
// Documentation, marketing assets and site content are deliberately absent:
// they land on beta constantly and reach readers without a release at all.
const DEVICE_PACKAGE_DIRECTORIES = ["crates", "apps", ".cargo"];
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

function argumentsFrom(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if ((flag !== "--tag" && flag !== "--version") || !value) {
      throw new Error(
        "usage: node tools/device-package-changes.mjs --tag TAG --version VERSION < changed-paths"
      );
    }
    values.set(flag, value);
  }
  if (values.size !== 2) {
    throw new Error(
      "usage: node tools/device-package-changes.mjs --tag TAG --version VERSION < changed-paths"
    );
  }
  return values;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const values = argumentsFrom(process.argv.slice(2));
    const tag = values.get("--tag");
    const version = values.get("--version");
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
