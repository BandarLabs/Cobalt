import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  affectsDevicePackage,
  devicePackageChanges,
  installedPackageNames,
  installedPackagesOutsideTheGate,
  nextPatchVersion,
  unpublishedChangeReport,
  workspaceMembers
} from "./device-package-changes.mjs";

test("packaged source, manifests and toolchain pins reach readers only in a release", () => {
  for (const path of [
    "crates/kobod/src/main.rs",
    "crates/kobo-protocol/src/lib.rs",
    "examples/todo/src/main.rs",
    "examples/launcher/Cargo.toml",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/config.toml"
  ]) {
    assert.equal(affectsDevicePackage(path), true, path);
  }
});

// A Store-only application is published by its own workflow on the merge that
// lands it. Demanding a platform release for it would turn every app landing
// red, which is the failure the rest of this file exists to avoid.
test("Store-only applications and documentation reach readers without a release", () => {
  for (const path of [
    "apps/backgammon/src/main.rs",
    "apps/backgammon/Cargo.toml",
    "apps/catalog.json",
    "README.md",
    "docs/index.html",
    "docs/apps/backgammon/index.html",
    "docs/media/site/apps/backgammon.png",
    "docs/sitemap.xml",
    "kobo-shot.png",
    ".github/workflows/ci.yml",
    "tools/check-app-versions.mjs"
  ]) {
    assert.equal(affectsDevicePackage(path), false, path);
  }
});

test("directory names are matched whole", () => {
  assert.equal(affectsDevicePackage("crates-notes.md"), false);
  assert.equal(affectsDevicePackage("examples-old/todo/src/main.rs"), false);
  assert.equal(affectsDevicePackage("docs/Cargo.toml"), false);
  assert.equal(affectsDevicePackage("docs/rust-toolchain.toml"), false);
});

test("a mixed push reports only the paths that need a release", () => {
  assert.deepEqual(
    devicePackageChanges([
      "docs/index.html",
      "crates/kobo-ui/src/list.rs",
      "apps/backgammon/src/main.rs",
      "README.md",
      "Cargo.lock",
      "crates/kobo-ui/src/list.rs"
    ]),
    ["Cargo.lock", "crates/kobo-ui/src/list.rs"]
  );
  assert.deepEqual(
    devicePackageChanges(["docs/index.html", "apps/backgammon/src/main.rs"]),
    []
  );
});

test("the packaged binary list is read from the packaging code, not copied", () => {
  const names = installedPackageNames(
    'const INSTALLED_PACKAGES: &[(&str, Option<&str>)] = &[\n    ("kobod", Some("device-write")),\n    ("kobo-launcher", None),\n];\n'
  );
  assert.deepEqual(names, ["kobod", "kobo-launcher"]);
  assert.throws(
    () => installedPackageNames("nothing here"),
    /read INSTALLED_PACKAGES/
  );
});

test("a packaged application outside the watched directories is named", () => {
  const directories = new Map([
    ["kobod", "crates/kobod"],
    ["kobo-todo", "examples/todo"],
    ["kobo-backgammon", "apps/backgammon"]
  ]);
  const directoryOf = name => directories.get(name);
  assert.deepEqual(
    installedPackagesOutsideTheGate(["kobod", "kobo-todo"], directoryOf),
    []
  );
  assert.deepEqual(
    installedPackagesOutsideTheGate(["kobod", "kobo-backgammon"], directoryOf),
    ["kobo-backgammon"]
  );
  assert.throws(
    () => installedPackagesOutsideTheGate(["kobo-unknown"], directoryOf),
    /kobo-unknown is packaged for the device but names no workspace member/
  );
});

// The exclusion of apps/ is only safe while nothing packaged lives there, and
// this is what notices the day that changes.
test("every package this tree ships to a device is inside the gate", () => {
  const members = workspaceMembers(readFileSync("Cargo.toml", "utf8"));
  const directories = new Map();
  for (const member of members) {
    const name = /^name\s*=\s*"([^"]+)"$/m.exec(
      readFileSync(`${member}/Cargo.toml`, "utf8")
    )?.[1];
    if (name) directories.set(name, member);
  }
  const names = installedPackageNames(
    readFileSync("crates/kobo-cli/src/main.rs", "utf8")
  );
  assert.ok(names.length > 1, "INSTALLED_PACKAGES should name several packages");
  assert.deepEqual(
    installedPackagesOutsideTheGate(names, name => directories.get(name)),
    []
  );
});

test("the report names the exact version bump that publishes the change", () => {
  const report = unpublishedChangeReport("beta-v0.3.5", "0.3.5", ["Cargo.lock"]);
  assert.match(report, /beta-v0\.3\.5 is already published/);
  assert.match(report, /^ {2}Cargo\.lock$/m);
  assert.match(report, /from 0\.3\.5 to 0\.3\.6/);
  assert.match(report, /publish beta-v0\.3\.6/);
});

test("version arithmetic refuses anything that is not a release number", () => {
  assert.equal(nextPatchVersion("0.3.9"), "0.3.10");
  assert.equal(nextPatchVersion("1.0.0"), "1.0.1");
  assert.throws(() => nextPatchVersion("0.3"), /invalid workspace version 0\.3/);
  assert.throws(() => nextPatchVersion("0.3.5-beta"), /invalid workspace version/);
});
