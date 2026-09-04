import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  affectsDevicePackage,
  devicePackageChanges
} from "./device-package-changes.mjs";
import {
  affectsStoreCatalog,
  registeredStorePackages,
  storeCatalogChanges,
  storeWatchDirectories,
  unpublishedStoreChangeReport,
  workspacePackageDirectories
} from "./store-catalog-changes.mjs";

function directoriesOfThisTree() {
  return storeWatchDirectories(
    workspacePackageDirectories(readFileSync("Cargo.toml", "utf8"), member =>
      readFileSync(`${member}/Cargo.toml`, "utf8")
    ),
    registeredStorePackages(JSON.parse(readFileSync("apps/catalog.json", "utf8")))
  );
}

test("Store-only sources and the registry reach readers only in a catalog publication", () => {
  const directories = directoriesOfThisTree();
  for (const path of [
    "apps/backgammon/src/main.rs",
    "apps/backgammon/Cargo.toml",
    "apps/catalog.json",
    "apps/arxiv/src/atom.rs",
    "examples/todo/src/main.rs",
    "examples/chat/src/conversation.rs"
  ]) {
    assert.equal(affectsStoreCatalog(path, directories), true, path);
  }
});

test("platform crates, device-only examples and documentation do not force a catalog publication", () => {
  const directories = directoriesOfThisTree();
  for (const path of [
    "crates/kobo-profile/src/lib.rs",
    "crates/kobo-ui/src/lib.rs",
    "crates/kobo-sim/src/lib.rs",
    "crates/kobod/src/main.rs",
    "examples/launcher/src/main.rs",
    "examples/settings/src/main.rs",
    "examples/store/src/main.rs",
    "examples/terminal/src/main.rs",
    "examples/hello/src/main.rs",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/config.toml",
    "README.md",
    "docs/index.html",
    "docs/apps/backgammon/index.html",
    ".github/workflows/beta-release.yml",
    "tools/check-app-versions.mjs"
  ]) {
    assert.equal(affectsStoreCatalog(path, directories), false, path);
  }
});

test("directory names are matched whole", () => {
  const directories = ["apps", "examples/todo"];
  assert.equal(affectsStoreCatalog("apps-notes.md", directories), false);
  assert.equal(affectsStoreCatalog("examples/todo-old/src/main.rs", directories), false);
  assert.equal(affectsStoreCatalog("docs/apps/backgammon/index.html", directories), false);
  assert.equal(affectsStoreCatalog("examples/todo", directories), true);
});

test("a mixed push reports only the Store catalog inputs", () => {
  const directories = directoriesOfThisTree();
  assert.deepEqual(
    storeCatalogChanges(
      [
        "docs/index.html",
        "crates/kobo-profile/src/lib.rs",
        "apps/backgammon/src/main.rs",
        "README.md",
        "Cargo.lock",
        "apps/backgammon/src/main.rs"
      ],
      directories
    ),
    ["apps/backgammon/src/main.rs"]
  );
  assert.deepEqual(
    storeCatalogChanges(
      ["crates/kobo-profile/src/lib.rs", "Cargo.toml", "examples/launcher/src/main.rs"],
      directories
    ),
    []
  );
});

test("every catalog package in this tree is inside the gate", () => {
  const directories = directoriesOfThisTree();
  const packages = registeredStorePackages(
    JSON.parse(readFileSync("apps/catalog.json", "utf8"))
  );
  const packageDirectories = workspacePackageDirectories(
    readFileSync("Cargo.toml", "utf8"),
    member => readFileSync(`${member}/Cargo.toml`, "utf8")
  );
  assert.ok(packages.length > 1, "the catalog should name several packages");
  for (const name of packages) {
    const directory = packageDirectories.get(name);
    assert.ok(directory, `${name} must name a workspace member`);
    assert.ok(
      directories.includes(directory) || directories.includes("apps"),
      `${name} in ${directory} is not watched`
    );
    assert.equal(affectsStoreCatalog(`${directory}/src/main.rs`, directories), true, name);
  }
  assert.throws(
    () => storeWatchDirectories(new Map(), ["kobo-notes"]),
    /kobo-notes is in the Store catalog but names no workspace member/
  );
  assert.deepEqual(
    storeWatchDirectories(new Map([["kobo-notes", "apps/notes"]]), ["kobo-notes"]),
    ["apps", "apps/notes"]
  );
});

test("the report names the catalog publication that carries the change", () => {
  const report = unpublishedStoreChangeReport("app-catalog-beta", [
    "apps/backgammon/src/main.rs"
  ]);
  assert.match(report, /app-catalog-beta is already published/);
  assert.match(report, /^ {2}apps\/backgammon\/src\/main\.rs$/m);
  assert.match(report, /Publish apps will sign a new beta catalog/);
});

// The two gates are the independence proof. A Store-only landing must stay
// quiet for the platform, and a platform-only landing must stay quiet for the
// catalog. #101 (kobo-profile) is the historical case that rebuilt every app
// and then failed the publish job because no Store version had moved.
test("Store-only and platform-only changes do not block each other", () => {
  const directories = directoriesOfThisTree();

  const storeOnly = ["apps/backgammon/src/main.rs", "docs/apps/backgammon/index.html"];
  assert.deepEqual(devicePackageChanges(storeOnly), []);
  assert.deepEqual(storeCatalogChanges(storeOnly, directories), [
    "apps/backgammon/src/main.rs"
  ]);

  const platformOnly = ["crates/kobo-profile/src/lib.rs"];
  assert.deepEqual(devicePackageChanges(platformOnly), [
    "crates/kobo-profile/src/lib.rs"
  ]);
  assert.deepEqual(storeCatalogChanges(platformOnly, directories), []);

  const platformBump = ["Cargo.toml", "Cargo.lock"];
  assert.deepEqual(devicePackageChanges(platformBump), ["Cargo.lock", "Cargo.toml"]);
  assert.deepEqual(storeCatalogChanges(platformBump, directories), []);

  const bundledStoreApp = ["examples/todo/src/main.rs"];
  assert.equal(affectsDevicePackage(bundledStoreApp[0]), true);
  assert.deepEqual(storeCatalogChanges(bundledStoreApp, directories), bundledStoreApp);
});
