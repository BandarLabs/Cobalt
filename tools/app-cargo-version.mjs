#!/usr/bin/env node
// A Store package's version lives in one place: the `version` field of its
// cobalt-app.json. That is what the catalog signs, what the release gate
// compares against the published channel, and what a reader sees in Store.
//
// Cargo.toml inherits the workspace version instead of repeating the app's.
// When every app carried its own copy there, all thirty-four of them drifted
// from their manifests within a few releases, because nothing read the Cargo
// copy and nothing checked it. A number nobody reads and nobody checks is
// worse than none: it looks like the version. `version.workspace = true`
// makes the Cargo field mean what it says, the platform the app was built
// against, and leaves the manifest as the only version an app has.
//
// Run with no arguments to check every package beside a manifest; the Node
// test suite runs the same check, so CI refuses a drifted package.
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE_DIRECTORIES = ["apps", "examples"];

// The `version` entry of the `[package]` table, however it is spelled, or
// null when the table has none. Only `[package]` is read: a `version` line
// inside a dependency table is a different thing. A compliant manifest that
// this misreads is told it has no version, so comments, spaced headers and a
// `[package.version]` table all have to read the way Cargo reads them.
export function packageVersion(cargoSource) {
  let table = null;
  for (const raw of cargoSource.split("\n")) {
    const line = withoutComment(raw).trim();
    if (line.startsWith("[")) {
      table = tableName(line);
      continue;
    }
    if (table === "package") {
      if (/^version\s*\.\s*workspace\s*=\s*true$/.test(line)) return { workspace: true };
      if (/^version\s*=\s*\{\s*workspace\s*=\s*true\s*\}$/.test(line)) return { workspace: true };
      const explicit = /^version\s*=\s*(?:"([^"]*)"|'([^']*)')$/.exec(line);
      if (explicit) return { workspace: false, value: explicit[1] ?? explicit[2] };
    } else if (table === "package.version" && /^workspace\s*=\s*true$/.test(line)) {
      return { workspace: true };
    }
  }
  return null;
}

// The dotted name of a `[table]` header with the spacing TOML allows removed,
// or null for an array of tables, which is never the package table, and for a
// quoted name, which no Cargo manifest here writes and this does not read.
function tableName(header) {
  const name = /^\[\s*([\w-]+(?:\s*\.\s*[\w-]+)*)\s*\]$/.exec(header);
  return name ? name[1].replace(/\s+/g, "") : null;
}

// The line up to its comment. A `#` inside a quoted string is not one.
function withoutComment(line) {
  let quote = null;
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (quote === null) {
      if (character === "#") return line.slice(0, index);
      if (character === '"' || character === "'") quote = character;
    } else if (character === "\\" && quote === '"') {
      index += 1;
    } else if (character === quote) {
      quote = null;
    }
  }
  return line;
}

// Every directory under apps/ and examples/ that carries a Store manifest,
// paired with its Cargo manifest. Built-in examples without a cobalt-app.json
// are not Store packages and are not read.
export function storePackages(rootDirectory = root) {
  const packages = [];
  for (const directory of SOURCE_DIRECTORIES) {
    const source = join(rootDirectory, directory);
    let entries;
    try {
      entries = readdirSync(source, { withFileTypes: true });
    } catch (error) {
      if (error.code === "ENOENT") continue;
      throw error;
    }
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      const packageDirectory = join(source, entry.name);
      let manifest;
      try {
        manifest = readFileSync(join(packageDirectory, "cobalt-app.json"), "utf8");
      } catch (error) {
        if (error.code === "ENOENT") continue;
        throw error;
      }
      packages.push({
        directory: relative(rootDirectory, packageDirectory).split("\\").join("/"),
        manifestVersion: JSON.parse(manifest).version,
        cargo: readFileSync(join(packageDirectory, "Cargo.toml"), "utf8")
      });
    }
  }
  packages.sort((left, right) => left.directory.localeCompare(right.directory));
  return packages;
}

// One line per package whose Cargo manifest carries a version of its own,
// naming the drift so the fix is obvious; empty when every package is right.
export function cargoVersionFaults(packages) {
  const faults = [];
  for (const { directory, manifestVersion, cargo } of packages) {
    const version = packageVersion(cargo);
    if (version === null) {
      faults.push(`${directory}/Cargo.toml has no [package] version; use version.workspace = true`);
    } else if (!version.workspace) {
      faults.push(
        `${directory}/Cargo.toml says version = "${version.value}" while cobalt-app.json says ` +
          `${manifestVersion}; replace the Cargo line with version.workspace = true, ` +
          `the manifest is the app's version`
      );
    }
  }
  return faults;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const faults = cargoVersionFaults(storePackages());
  if (faults.length > 0) {
    console.error(faults.join("\n"));
    process.exitCode = 1;
  } else {
    console.log("Every Store package inherits the workspace version; the manifest is the app's version.");
  }
}
